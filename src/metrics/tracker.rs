//! Per-branch metrics tracking with SQLite-backed persistence.

use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};
use uuid::Uuid;

use crate::{
    branch::store::BranchStore,
    config::BranchConfig,
    diff::extractor::DiffExtractor,
    error::BranchResult,
    types::{Branch, BranchMetrics, EntityType},
};

/// The kind of entity write operation being recorded.
#[derive(Debug, Clone, Copy)]
pub enum OpKind {
    /// An entity was inserted.
    Insert,
    /// An entity was updated.
    Update,
    /// An entity was deleted.
    Delete,
}

/// Tracks, refreshes, and persists per-branch operational metrics.
///
/// # Example
/// ```rust,ignore
/// let tracker = MetricsTracker::new(store, config);
/// let metrics = tracker.refresh(&branch).await?;
/// println!("ops={} divergence={:.3}", metrics.op_count, metrics.divergence_score);
/// ```
pub struct MetricsTracker {
    store: Arc<BranchStore>,
    config: Arc<BranchConfig>,
}

impl MetricsTracker {
    /// Creates a new tracker from the shared store and config.
    pub fn new(store: Arc<BranchStore>, config: Arc<BranchConfig>) -> Self {
        Self { store, config }
    }

    /// Refreshes metrics for a single branch and persists the result.
    ///
    /// Steps:
    /// 1. Open a read-only pool on the branch database
    /// 2. `COUNT(*)` each entity table
    /// 3. Read `bytes_on_disk` from filesystem metadata
    /// 4. Count commits from the registry
    /// 5. Compute divergence vs parent (if parent exists)
    /// 6. Persist via `BranchStore::update_metrics`
    pub async fn refresh(&self, branch: &Branch) -> BranchResult<BranchMetrics> {
        let pool = open_pool(&branch.db_path).await?;

        let memory_record_count = count_table(&pool, EntityType::MemoryRecord.table_name()).await?;
        let session_count = count_table(&pool, EntityType::Session.table_name()).await?;
        let tool_output_count = count_table(&pool, EntityType::ToolOutput.table_name()).await?;
        pool.close().await;

        let bytes_on_disk = tokio::fs::metadata(&branch.db_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        let op_count = self.store.count_commits(branch.id).await?;

        // Compute divergence score relative to the parent branch.
        let divergence_score = if let Some(parent_id) = branch.parent_id {
            match self.store.get(parent_id).await {
                Ok(parent) => {
                    let extractor = DiffExtractor::new(Arc::clone(&self.config));
                    match extractor.diff(branch, &parent, None).await {
                        Ok(diff) => {
                            let base_count = (parent.metrics.memory_record_count
                                + parent.metrics.session_count
                                + parent.metrics.tool_output_count)
                                as u64;
                            crate::metrics::divergence::compute_score(&diff, base_count)
                        }
                        Err(_) => branch.metrics.divergence_score,
                    }
                }
                Err(_) => branch.metrics.divergence_score,
            }
        } else {
            0.0
        };

        let metrics = BranchMetrics {
            op_count,
            memory_record_count,
            session_count,
            tool_output_count,
            bytes_on_disk,
            divergence_score,
            created_entity_count: branch.metrics.created_entity_count,
            updated_entity_count: branch.metrics.updated_entity_count,
            deleted_entity_count: branch.metrics.deleted_entity_count,
            last_activity_at: branch.metrics.last_activity_at,
        };

        self.store.update_metrics(branch.id, &metrics).await?;
        Ok(metrics)
    }

    /// Refreshes metrics for all live branches in the workspace.
    pub async fn refresh_all(
        &self,
        workspace_id: Uuid,
    ) -> BranchResult<HashMap<Uuid, BranchMetrics>> {
        let branches = self.store.list(workspace_id, None).await?;
        let mut result = HashMap::with_capacity(branches.len());
        for branch in branches {
            let metrics = self.refresh(&branch).await?;
            result.insert(branch.id, metrics);
        }
        Ok(result)
    }

    /// Records a single write operation on a branch, incrementing the relevant counters.
    pub async fn track_op(&self, branch_id: Uuid, op_kind: OpKind) -> BranchResult<()> {
        let mut branch = self.store.get(branch_id).await?;
        branch.metrics.op_count += 1;
        branch.metrics.last_activity_at = Some(Utc::now());
        match op_kind {
            OpKind::Insert => branch.metrics.created_entity_count += 1,
            OpKind::Update => branch.metrics.updated_entity_count += 1,
            OpKind::Delete => branch.metrics.deleted_entity_count += 1,
        }
        self.store.update_metrics(branch_id, &branch.metrics).await
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn open_pool(path: &std::path::Path) -> BranchResult<SqlitePool> {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(false)
                .read_only(true)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await
        .map_err(crate::error::BranchError::Database)
}

async fn count_table(pool: &SqlitePool, table: &str) -> BranchResult<i64> {
    // Check table exists first to avoid hard errors on empty snapshots.
    let exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
            .bind(table)
            .fetch_one(pool)
            .await?;

    if exists == 0 {
        return Ok(0);
    }

    let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await?;
    Ok(count)
}
