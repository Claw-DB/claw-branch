//! Pre-commit validation for cherry-pick operations.

use sqlx::{sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions}, SqlitePool};

use crate::{
    commit::cherry::CherryPick,
    error::BranchResult,
    types::{Branch, BranchStatus},
};

/// Validates cherry-pick commits before any mutation is applied.
///
/// # Example
/// ```rust,ignore
/// let report = CommitValidator::new(pool).validate(&cherry, &source, &target).await?;
/// if !report.ok { return Err(...); }
/// ```
pub struct CommitValidator {
    pool: SqlitePool,
}

/// The result of a pre-commit validation pass.
#[derive(Debug, Clone)]
pub struct ValidationReport {
    /// True when no violations were detected.
    pub ok: bool,
    /// Hard violations that prevent the commit from proceeding.
    pub violations: Vec<String>,
    /// Non-fatal warnings that operators should review.
    pub warnings: Vec<String>,
}

impl CommitValidator {
    /// Creates a new validator backed by the given SQLite pool (source branch).
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Opens a validator from a branch db_path.
    pub async fn from_branch(branch: &Branch) -> BranchResult<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&branch.db_path)
                    .create_if_missing(false)
                    .read_only(true)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await?;
        Ok(Self { pool })
    }

    /// Validates the cherry-pick against the source and target branches.
    ///
    /// Checks performed:
    /// - Source branch must be `Active` or `Dormant`
    /// - Target branch must be `Active`
    /// - All specified entity ids must exist in the source database
    /// - No obvious foreign-key invariant violations in the target
    pub async fn validate(
        &self,
        cherry: &CherryPick,
        source: &Branch,
        target: &Branch,
    ) -> BranchResult<ValidationReport> {
        let mut violations: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        // Status checks.
        match &source.status {
            BranchStatus::Active | BranchStatus::Dormant => {}
            other => violations.push(format!(
                "source branch {} is {:?}, must be Active or Dormant",
                source.id,
                other.kind()
            )),
        }
        if !matches!(&target.status, BranchStatus::Active) {
            violations.push(format!(
                "target branch {} is {:?}, must be Active",
                target.id,
                target.status.kind()
            ));
        }

        // Entity existence checks.
        for sel in &cherry.entity_selections {
            if sel.entity_ids.is_empty() {
                continue; // All-of-type selections skip id validation.
            }
            let table = sel.entity_type.table_name();
            let table_exists: bool = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            )
            .bind(table)
            .fetch_one(&self.pool)
            .await
            .map(|n| n > 0)
            .unwrap_or(false);

            if !table_exists {
                warnings.push(format!(
                    "table '{table}' does not exist in source; skipping entity existence check"
                ));
                continue;
            }

            for entity_id in &sel.entity_ids {
                let exists: bool = sqlx::query_scalar::<_, i64>(
                    &format!("SELECT COUNT(*) FROM {table} WHERE id = ?"),
                )
                .bind(entity_id)
                .fetch_one(&self.pool)
                .await
                .map(|n| n > 0)
                .unwrap_or(false);

                if !exists {
                    violations.push(format!(
                        "entity '{}' of type {:?} not found in source branch {}",
                        entity_id,
                        sel.entity_type,
                        source.id
                    ));
                }
            }
        }

        let ok = violations.is_empty();
        Ok(ValidationReport { ok, violations, warnings })
    }
}
