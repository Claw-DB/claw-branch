//! Commit history helpers for branch registries.

use uuid::Uuid;

use crate::{branch::store::BranchStore, error::BranchResult, types::CommitLogEntry};

/// Lists recent commits for a branch from the shared store.
pub async fn recent_commits(
    store: &BranchStore,
    branch_id: Uuid,
    limit: u32,
) -> BranchResult<Vec<CommitLogEntry>> {
    store.list_commits(branch_id, limit).await
}
