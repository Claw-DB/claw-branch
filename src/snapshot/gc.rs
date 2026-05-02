//! Garbage collection for orphaned branch snapshots and stale discarded state.

use std::{path::{Path, PathBuf}, sync::Arc, time::{Duration, Instant, SystemTime}};

use chrono::{DateTime, Utc};
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
    /// The number of branch directories scanned.
    pub scanned: u32,
    /// The number of orphaned directories deleted.
    pub orphaned_deleted: u32,
    /// The number of discarded branch directories deleted.
    pub discarded_deleted: u32,
    /// The estimated number of bytes freed.
    pub bytes_freed: u64,
    /// Errors encountered during the collection run.
    pub errors: Vec<String>,
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
        let mut entries = match tokio::fs::read_dir(&self.config.branches_dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                report.duration_ms = started_at.elapsed().as_millis() as u64;
                return Ok(report);
            }
            Err(error) => return Err(error.into()),
        };

        while let Some(entry) = entries.next_entry().await? {
            report.scanned += 1;
            let path = entry.path();
            let file_type = entry.file_type().await?;
            if !file_type.is_dir() {
                if path.file_name().is_some_and(|name| name == "manifest.json") {
                    let bytes = file_size(&path).await.unwrap_or_default();
                    if tokio::fs::remove_file(&path).await.is_ok() {
                        report.bytes_freed += bytes;
                    }
                }
                continue;
            }

            match parse_branch_dir_uuid(&path) {
                Ok(branch_id) => match self.registry.get(branch_id).await {
                    Ok(branch) => {
                        if let BranchStatus::Discarded { discarded_at } = branch.status {
                            if age_exceeds(discarded_at, self.config.gc_orphan_threshold_secs) {
                                report.bytes_freed += dir_size(&path).await.unwrap_or_default();
                                tokio::fs::remove_dir_all(&path).await?;
                                report.discarded_deleted += 1;
                            }
                        }
                    }
                    Err(error) if error.is_not_found() => {
                        report.bytes_freed += dir_size(&path).await.unwrap_or_default();
                        tokio::fs::remove_dir_all(&path).await?;
                        report.orphaned_deleted += 1;
                    }
                    Err(error) => report.errors.push(error.to_string()),
                },
                Err(_) => {
                    if path_older_than(&path, self.config.gc_orphan_threshold_secs).await.unwrap_or(false) {
                        report.bytes_freed += dir_size(&path).await.unwrap_or_default();
                        tokio::fs::remove_dir_all(&path).await?;
                        report.orphaned_deleted += 1;
                    }
                }
            }
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
                    if matches!(self.registry.get(branch_id).await, Err(error) if error.is_not_found()) {
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
        let branches = self.registry.list(self.config.workspace_id, Some(BranchStatus::Discarded { discarded_at: Utc::now() })).await?;
        let mut deleted = 0_u64;
        for branch in branches {
            if let BranchStatus::Discarded { discarded_at } = branch.status {
                if discarded_at < cutoff {
                    let path = self.config.branches_dir.join(branch.id.to_string());
                    if tokio::fs::remove_dir_all(&path).await.is_ok() {
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

fn age_exceeds(timestamp: DateTime<Utc>, threshold_secs: u64) -> bool {
    let age = Utc::now() - timestamp;
    age.num_seconds() >= threshold_secs as i64
}

async fn path_older_than(path: &Path, threshold_secs: u64) -> BranchResult<bool> {
    let metadata = tokio::fs::metadata(path).await?;
    let modified = metadata.modified()?;
    Ok(system_time_age(modified) >= Duration::from_secs(threshold_secs))
}

fn system_time_age(timestamp: SystemTime) -> Duration {
    SystemTime::now().duration_since(timestamp).unwrap_or_default()
}

async fn dir_size(path: &Path) -> BranchResult<u64> {
    let mut total = 0_u64;
    let mut entries = tokio::fs::read_dir(path).await?;
    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();
        let file_type = entry.file_type().await?;
        if file_type.is_dir() {
            total += Box::pin(dir_size(&entry_path)).await?;
        } else {
            total += file_size(&entry_path).await?;
        }
    }
    Ok(total)
}

async fn file_size(path: &Path) -> BranchResult<u64> {
    Ok(tokio::fs::metadata(path).await?.len())
}