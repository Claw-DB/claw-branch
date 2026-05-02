//! Snapshot copying for isolated per-branch SQLite databases.

use std::{path::{Path, PathBuf}, sync::Arc, time::Instant};

use chrono::Utc;
use sqlx::{sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions}, Row};
use tempfile::NamedTempFile;
use tracing::info;
use uuid::Uuid;

use crate::{
    config::BranchConfig,
    error::{BranchError, BranchResult},
    snapshot::{manifest::{EntityCounts, SnapshotManifest}, verifier::{hash_file_blake3, verify_snapshot}},
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
        tokio::fs::create_dir_all(&destination_dir).await?;

        let temp_file = NamedTempFile::new_in(&destination_dir)?;
        let temp_path = temp_file.into_temp_path();
        tokio::fs::copy(source_db_path, &temp_path).await?;
        temp_path.persist(&destination_path).map_err(|error| BranchError::Io(error.error))?;

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
        let temp_file = NamedTempFile::new_in(
            target_db_path.parent().unwrap_or_else(|| Path::new(".")),
        )?;
        let temp_path = temp_file.into_temp_path();
        tokio::fs::copy(snapshot_path, &temp_path).await?;
        temp_path.persist(target_db_path).map_err(|error| BranchError::Io(error.error))?;
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