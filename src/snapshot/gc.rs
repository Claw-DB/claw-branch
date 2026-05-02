//! Garbage collection for orphaned branch snapshots and stale discarded state.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    branch::store::BranchStore,
    config::BranchConfig,
    error::{BranchError, BranchResult},
    types::BranchStatus,
};

/// Summarizes a snapshot garbage-collection run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GcReport {
    /// The number of branch snapshots purged.
    pub branches_purged: u32,
    /// The estimated number of bytes freed.
    pub bytes_freed: u64,
    /// The duration of the collection run in milliseconds.
    pub duration_ms: u64,
}

/// Collects and purges orphaned or expired snapshot directories.
#[derive(Clone)]
pub struct SnapshotGc {
    config: Arc<BranchConfig>,
    registry: Arc<BranchStore>,
}

impl SnapshotGc {
    /// Creates a new snapshot garbage collector.
    pub fn new(config: Arc<BranchConfig>, registry: Arc<BranchStore>) -> Self {
        Self { config, registry }
    }

    /// Runs snapshot garbage collection and returns a report of deleted state.
    pub async fn run(&self) -> BranchResult<GcReport> {
        let started_at = Instant::now();
        let mut report = GcReport::default();
        let branches = self.registry.list(self.config.workspace_id, None).await?;
        let cutoff =
            Utc::now() - chrono::Duration::seconds(self.config.gc_orphan_threshold_secs as i64);

        for branch in branches {
            let should_purge = match branch.status {
                BranchStatus::Discarded { discarded_at } => discarded_at < cutoff,
                BranchStatus::Orphan => branch.updated_at < cutoff,
                _ => false,
            };
            if !should_purge {
                continue;
            }

            report.bytes_freed += file_size(&branch.db_path).await.unwrap_or_default();
            let sidecar = branch.db_path.with_extension("hash");
            report.bytes_freed += file_size(&sidecar).await.unwrap_or_default();

            let _ = tokio::fs::remove_file(&branch.db_path).await;
            let _ = tokio::fs::remove_file(&sidecar).await;

            if let Some(parent) = branch.db_path.parent() {
                let _ = tokio::fs::remove_dir_all(parent).await;
            }

            self.registry
                .update_status(branch.id, BranchStatus::Purged)
                .await?;
            report.branches_purged += 1;
        }

        report.duration_ms = started_at.elapsed().as_millis() as u64;
        Ok(report)
    }

    /// Lists orphaned paths that would be deleted without mutating disk state.
    pub async fn collect_orphans(&self) -> BranchResult<Vec<PathBuf>> {
        let mut paths = Vec::new();
        let mut entries = match tokio::fs::read_dir(&self.config.branches_dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(paths),
            Err(error) => return Err(error.into()),
        };

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;
            if !file_type.is_dir() {
                continue;
            }

            match parse_branch_dir_uuid(&path) {
                Ok(branch_id) => {
                    if matches!(self.registry.get(branch_id).await, Err(error) if error.is_not_found())
                    {
                        paths.push(path);
                    }
                }
                Err(_) => paths.push(path),
            }
        }
        Ok(paths)
    }

    /// Purges discarded snapshots older than the provided number of days.
    pub async fn purge_discarded(&self, older_than_days: u32) -> BranchResult<u64> {
        let cutoff = Utc::now() - chrono::Duration::days(older_than_days as i64);
        let branches = self.registry.list(self.config.workspace_id, None).await?;
        let mut deleted = 0_u64;
        for branch in branches {
            if let BranchStatus::Discarded { discarded_at } = branch.status {
                if discarded_at < cutoff {
                    let _ = tokio::fs::remove_file(&branch.db_path).await;
                    let _ = tokio::fs::remove_file(branch.db_path.with_extension("hash")).await;
                    self.registry
                        .update_status(branch.id, BranchStatus::Purged)
                        .await?;
                    if let Some(path) = branch.db_path.parent() {
                        let _ = tokio::fs::remove_dir_all(path).await;
                        deleted += 1;
                    }
                }
            }
        }
        Ok(deleted)
    }
}

fn parse_branch_dir_uuid(path: &Path) -> Result<Uuid, BranchError> {
    let value = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| BranchError::OrphanedSnapshot(path.to_path_buf()))?;
    Uuid::parse_str(value).map_err(|_| BranchError::OrphanedSnapshot(path.to_path_buf()))
}

async fn file_size(path: &Path) -> BranchResult<u64> {
    Ok(tokio::fs::metadata(path).await?.len())
}
