//! Snapshot copying for isolated per-branch SQLite databases.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use chrono::Utc;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    Row,
};
use tracing::info;
use uuid::Uuid;

use crate::{
    config::BranchConfig,
    error::{BranchError, BranchResult},
    snapshot::{
        manifest::{EntityCounts, SnapshotManifest},
        verifier::{
            hash_file_blake3, sidecar_hash_path_for_db, verify_snapshot, write_sidecar_hash,
        },
    },
};

/// Creates snapshot-backed branch SQLite files using full-file copy semantics.
#[derive(Clone)]
pub struct SnapshotCopier {
    config: Arc<BranchConfig>,
}

impl SnapshotCopier {
    /// Creates a snapshot copier from shared configuration.
    pub fn new(config: Arc<BranchConfig>) -> Self {
        Self { config }
    }

    /// Creates an isolated snapshot copy for a new branch database.
    pub async fn create_snapshot(
        &self,
        source_db_path: &Path,
        branch_id: Uuid,
        label: &str,
    ) -> BranchResult<SnapshotManifest> {
        let started_at = Instant::now();
        let source_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(source_db_path)
                    .create_if_missing(false)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await?;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&source_pool)
            .await?;

        let source_hash = hash_file_blake3(source_db_path)?;
        let destination_dir = self.snapshot_dir_for(branch_id);
        let destination_path = self.snapshot_path_for(branch_id);
        let temp_destination_path = destination_path.with_extension("db.tmp");
        tokio::fs::create_dir_all(&destination_dir).await?;

        tokio::fs::copy(source_db_path, &temp_destination_path).await?;
        tokio::fs::rename(&temp_destination_path, &destination_path).await?;

        let destination_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&destination_path)
                    .create_if_missing(false),
            )
            .await?;
        let integrity = sqlx::query("PRAGMA integrity_check")
            .fetch_one(&destination_pool)
            .await?
            .try_get::<String, _>(0)?;
        if integrity != "ok" {
            return Err(BranchError::SnapshotCorrupt {
                branch_id,
                path: destination_path,
            });
        }

        let snapshot_hash = hash_file_blake3(&destination_path)?;
        if source_hash != snapshot_hash {
            return Err(BranchError::SnapshotFailed {
                branch_id,
                reason: "source and destination snapshot hashes differ".to_string(),
            });
        }
        write_sidecar_hash(&sidecar_hash_path_for_db(&destination_path), &snapshot_hash)?;

        let schema_version = sqlx::query("PRAGMA user_version")
            .fetch_one(&destination_pool)
            .await?
            .try_get::<i64, _>(0)? as u32;
        let sqlite_page_size = sqlx::query("PRAGMA page_size")
            .fetch_one(&destination_pool)
            .await?
            .try_get::<i64, _>(0)? as u32;
        let sqlite_page_count = sqlx::query("PRAGMA page_count")
            .fetch_one(&destination_pool)
            .await?
            .try_get::<i64, _>(0)? as u64;
        let file_size_bytes = tokio::fs::metadata(&destination_path).await?.len();
        let entity_counts = EntityCounts::from_pool(&destination_pool).await?;

        let manifest = SnapshotManifest {
            branch_id,
            source_db_path: source_db_path.to_path_buf(),
            snapshot_db_path: destination_path.clone(),
            source_hash,
            snapshot_hash,
            schema_version,
            created_at: Utc::now(),
            file_size_bytes,
            label: label.to_string(),
            entity_counts,
            sqlite_page_size,
            sqlite_page_count,
        };
        manifest.save(&destination_dir)?;
        info!(
            branch_id = %branch_id,
            source = %source_db_path.display(),
            dest = %destination_path.display(),
            file_size_bytes,
            duration_ms = started_at.elapsed().as_millis() as u64,
            "created branch snapshot"
        );
        Ok(manifest)
    }

    /// Restores a verified snapshot file to a target database path.
    pub async fn restore_snapshot(
        &self,
        snapshot_path: &Path,
        target_db_path: &Path,
        manifest: &SnapshotManifest,
    ) -> BranchResult<()> {
        verify_snapshot(manifest).await?;
        if snapshot_path != manifest.snapshot_db_path {
            return Err(BranchError::SnapshotFailed {
                branch_id: manifest.branch_id,
                reason: "snapshot path does not match manifest".to_string(),
            });
        }
        if let Some(parent) = target_db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let temp_path = target_db_path.with_extension("db.tmp");
        tokio::fs::copy(snapshot_path, &temp_path).await?;
        tokio::fs::rename(&temp_path, target_db_path).await?;
        Ok(())
    }

    /// Deletes the snapshot directory for a branch and reports whether it existed.
    pub async fn delete_snapshot(&self, branch_id: Uuid) -> BranchResult<bool> {
        let path = self.snapshot_dir_for(branch_id);
        match tokio::fs::remove_dir_all(&path).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Returns the snapshot file path for a branch id.
    pub fn snapshot_path_for(&self, branch_id: Uuid) -> PathBuf {
        self.snapshot_dir_for(branch_id).join("branch.db")
    }

    /// Returns the snapshot directory path for a branch id.
    pub fn snapshot_dir_for(&self, branch_id: Uuid) -> PathBuf {
        self.config.branches_dir.join(branch_id.to_string())
    }
}

/// Deletes any stale `.tmp` files under the branches directory at startup.
pub async fn cleanup_incomplete_tmp_files(branches_dir: &Path) -> BranchResult<()> {
    let mut dirs = match tokio::fs::read_dir(branches_dir).await {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };

    while let Some(dir_entry) = dirs.next_entry().await? {
        let dir_path = dir_entry.path();
        if !dir_entry.file_type().await?.is_dir() {
            continue;
        }
        let mut nested = tokio::fs::read_dir(&dir_path).await?;
        while let Some(file_entry) = nested.next_entry().await? {
            let file_path = file_entry.path();
            if !file_entry.file_type().await?.is_file() {
                continue;
            }
            let is_tmp = file_path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "tmp");
            if is_tmp {
                let _ = tokio::fs::remove_file(file_path).await;
            }
        }
    }

    Ok(())
}
