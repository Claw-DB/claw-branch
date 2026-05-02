//! Workspace-level metrics aggregation and reporting.

use chrono::Utc;
use uuid::Uuid;

use crate::{
    branch::store::BranchStore,
    error::BranchResult,
    types::WorkspaceReport,
};

/// Aggregates metrics across all live branches in a workspace.
///
/// # Example
/// ```rust,ignore
/// let report = MetricsReporter.workspace_report(workspace_id, &store).await?;
/// println!("branches={} disk={} avg_div={:.3}", report.branch_count, report.total_disk_bytes, report.avg_divergence_score);
/// ```
pub struct MetricsReporter;

impl MetricsReporter {
    /// Generates a [`WorkspaceReport`] for the specified workspace.
    ///
    /// Collects metrics for all live (Active + Dormant) branches:
    /// - Total count and combined disk usage
    /// - Average divergence score
    /// - Most active branch (highest `op_count`)
    /// - Stale branches (no activity in 7 days)
    pub async fn workspace_report(
        &self,
        workspace_id: Uuid,
        store: &BranchStore,
    ) -> BranchResult<WorkspaceReport> {
        let branches = store.list(workspace_id, None).await?;
        let live: Vec<_> = branches
            .iter()
            .filter(|b| b.status.is_live())
            .collect();

        let branch_count = live.len() as u32;
        let total_disk_bytes: u64 = live.iter().map(|b| b.metrics.bytes_on_disk).sum();

        let avg_divergence_score = if live.is_empty() {
            0.0
        } else {
            live.iter().map(|b| b.metrics.divergence_score).sum::<f64>() / live.len() as f64
        };

        let most_active_branch = live
            .iter()
            .max_by_key(|b| b.metrics.op_count)
            .map(|b| b.id);

        let stale_cutoff = Utc::now() - chrono::Duration::days(7);
        let stale_branch_ids: Vec<Uuid> = live
            .iter()
            .filter(|b| {
                b.metrics
                    .last_activity_at
                    .map(|t| t < stale_cutoff)
                    .unwrap_or(true)
            })
            .map(|b| b.id)
            .collect();

        Ok(WorkspaceReport {
            branch_count,
            total_disk_bytes,
            avg_divergence_score,
            most_active_branch,
            stale_branch_ids,
            report_at: Utc::now(),
        })
    }
}
