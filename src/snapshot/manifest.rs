//! Snapshot manifests capture hashes, counts, and SQLite metadata for branch files.

use std::{fs, path::{Path, PathBuf}};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::BranchResult;

/// Entity counts captured when a snapshot manifest is created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct EntityCounts {
    /// The number of memory records present in the snapshot.
    pub memory_records: i64,
    /// The number of sessions present in the snapshot.
    pub sessions: i64,
    /// The number of tool outputs present in the snapshot.
    pub tool_outputs: i64,
}

impl EntityCounts {
    /// Returns the total entities represented by the counts.
    pub fn total(&self) -> i64 {
        self.memory_records + self.sessions + self.tool_outputs
    }

    /// Counts tracked entities from a live SQLite pool.
    pub async fn from_pool(pool: &SqlitePool) -> BranchResult<Self> {
        Ok(Self {
            memory_records: count_table(pool, "memory_records").await?,
            sessions: count_table(pool, "sessions").await?,
            tool_outputs: count_table(pool, "tool_outputs").await?,
        })
    }
}

/// Metadata and integrity details describing a snapshot file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotManifest {
    /// The branch identifier that owns the snapshot.
    pub branch_id: Uuid,
    /// The source database path used to create the snapshot.
    pub source_db_path: PathBuf,
    /// The destination snapshot database path.
    pub snapshot_db_path: PathBuf,
    /// The BLAKE3 hash of the source file at copy time.
    pub source_hash: [u8; 32],
    /// The BLAKE3 hash of the snapshot file after copy.
    pub snapshot_hash: [u8; 32],
    /// The SQLite user version captured from the snapshot.
    pub schema_version: u32,
    /// The timestamp when the snapshot was created.
    pub created_at: DateTime<Utc>,
    /// The snapshot file size in bytes.
    pub file_size_bytes: u64,
    /// The human-readable snapshot label.
    pub label: String,
    /// Entity counts captured from the snapshot.
    pub entity_counts: EntityCounts,
    /// SQLite page size for the snapshot file.
    pub sqlite_page_size: u32,
    /// SQLite page count for the snapshot file.
    pub sqlite_page_count: u64,
}

impl SnapshotManifest {
    /// Saves the manifest to `manifest.json` in the target directory.
    pub fn save(&self, dir: &Path) -> BranchResult<()> {
        fs::create_dir_all(dir)?;
        let path = dir.join("manifest.json");
        let contents = serde_json::to_vec_pretty(self)?;
        fs::write(path, contents)?;
        Ok(())
    }

    /// Loads the manifest from `manifest.json` in the target directory.
    pub fn load(dir: &Path) -> BranchResult<Self> {
        let path = dir.join("manifest.json");
        let contents = fs::read(path)?;
        Ok(serde_json::from_slice(&contents)?)
    }

    /// Returns a stable content identifier derived from the snapshot hash.
    pub fn content_id(&self) -> String {
        let mut output = String::with_capacity(self.snapshot_hash.len() * 2);
        for byte in self.snapshot_hash {
            use std::fmt::Write as _;
            let _ = write!(&mut output, "{byte:02x}");
        }
        output
    }
}

async fn count_table(pool: &SqlitePool, table: &str) -> BranchResult<i64> {
    let query = format!("SELECT COUNT(*) AS count FROM {table}");
    let row = sqlx::query(&query).fetch_one(pool).await?;
    Ok(row.try_get::<i64, _>("count")?)
}