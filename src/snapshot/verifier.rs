//! Snapshot integrity verification and file hashing helpers.

use std::{fs::File, io::Read, path::Path};

use sqlx::{sqlite::{SqliteConnectOptions, SqlitePoolOptions}, Row, SqlitePool};

use crate::{
    error::{BranchError, BranchResult},
    snapshot::manifest::{EntityCounts, SnapshotManifest},
};

/// Verifies a snapshot file against its manifest hashes, counts, and SQLite integrity checks.
pub async fn verify_snapshot(manifest: &SnapshotManifest) -> BranchResult<()> {
    if !manifest.snapshot_db_path.exists() {
        return Err(BranchError::SnapshotCorrupt {
            branch_id: manifest.branch_id,
            path: manifest.snapshot_db_path.clone(),
        });
    }

    let hash = hash_file_blake3(&manifest.snapshot_db_path)?;
    if hash != manifest.snapshot_hash {
        return Err(BranchError::SnapshotCorrupt {
            branch_id: manifest.branch_id,
            path: manifest.snapshot_db_path.clone(),
        });
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&manifest.snapshot_db_path)
                .create_if_missing(false),
        )
        .await?;
    let integrity = sqlx::query("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await?
        .try_get::<String, _>(0)?;
    if integrity != "ok" {
        return Err(BranchError::SnapshotCorrupt {
            branch_id: manifest.branch_id,
            path: manifest.snapshot_db_path.clone(),
        });
    }

    let counts = EntityCounts::from_pool(&pool).await?;
    if counts != manifest.entity_counts {
        return Err(BranchError::SnapshotCorrupt {
            branch_id: manifest.branch_id,
            path: manifest.snapshot_db_path.clone(),
        });
    }
    verify_schema_version(&pool, manifest.schema_version).await
}

/// Computes a BLAKE3 digest for a file in 64KB chunks.
pub fn hash_file_blake3(path: &Path) -> BranchResult<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(*hasher.finalize().as_bytes())
}

/// Verifies the SQLite user version for a pool.
pub async fn verify_schema_version(pool: &SqlitePool, expected: u32) -> BranchResult<()> {
    let row = sqlx::query("PRAGMA user_version").fetch_one(pool).await?;
    let actual = row.try_get::<i64, _>(0)? as u32;
    if actual != expected {
        return Err(BranchError::SnapshotFailed {
            branch_id: uuid::Uuid::nil(),
            reason: format!("schema version mismatch: expected {expected}, found {actual}"),
        });
    }
    Ok(())
}