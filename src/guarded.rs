//! Guard-aware branch engine wrapper.

use uuid::Uuid;

use crate::{
    commit::EntitySelection,
    engine::BranchEngine,
    error::{BranchError, BranchResult},
    merge::strategies::MergeStrategy,
    sandbox::environment::SimulationScenario,
    types::{Branch, BranchMetrics, DiffResult, MergeResult},
};

/// Authorization-aware wrapper over [`BranchEngine`].
///
/// # Example
/// ```rust,ignore
/// # use claw_branch::{BranchEngine, GuardedBranchEngine};
/// # use claw_guard::{ClawSession, Guard};
/// # async fn demo(engine: BranchEngine, guard: Guard, session: ClawSession) -> Result<(), Box<dyn std::error::Error>> {
/// let guarded = GuardedBranchEngine::new(engine, guard);
/// let _branch = guarded.fork_trunk(&session, "feature/demo").await?;
/// # Ok(())
/// # }
/// ```
pub struct GuardedBranchEngine {
    inner: BranchEngine,
    guard: claw_guard::Guard,
}

impl GuardedBranchEngine {
    /// Creates a new guarded wrapper.
    pub fn new(engine: BranchEngine, guard: claw_guard::Guard) -> Self {
        Self {
            inner: engine,
            guard,
        }
    }

    /// Forks from trunk after write access check.
    #[tracing::instrument(skip(self, session), fields(workspace_id = %self.inner.config().workspace_id))]
    pub async fn fork_trunk(
        &self,
        session: &claw_guard::ClawSession,
        name: &str,
    ) -> BranchResult<Branch> {
        self.check_write(session, "branch:fork_trunk").await?;
        self.inner.fork_trunk(name).await
    }

    /// Forks from parent after write access check.
    #[tracing::instrument(skip(self, session), fields(workspace_id = %self.inner.config().workspace_id, branch_id = %parent_id))]
    pub async fn fork(
        &self,
        session: &claw_guard::ClawSession,
        parent_id: Uuid,
        name: &str,
    ) -> BranchResult<Branch> {
        self.check_write(session, "branch:fork").await?;
        self.inner.fork(parent_id, name, None).await
    }

    /// Merges source into target after write access check.
    #[tracing::instrument(skip(self, session), fields(workspace_id = %self.inner.config().workspace_id))]
    pub async fn merge(
        &self,
        session: &claw_guard::ClawSession,
        source: Uuid,
        target: Uuid,
        strategy: MergeStrategy,
    ) -> BranchResult<MergeResult> {
        self.check_write(session, "branch:merge").await?;
        self.inner.merge(source, target, strategy).await
    }

    /// Commits a branch into trunk after write access check.
    #[tracing::instrument(skip(self, session), fields(workspace_id = %self.inner.config().workspace_id, branch_id = %branch_id))]
    pub async fn commit_to_trunk(
        &self,
        session: &claw_guard::ClawSession,
        branch_id: Uuid,
    ) -> BranchResult<()> {
        self.check_write(session, "branch:commit_to_trunk").await?;
        let _ = self.inner.commit_to_trunk(branch_id).await?;
        Ok(())
    }

    /// Discards a branch after write access check.
    #[tracing::instrument(skip(self, session), fields(workspace_id = %self.inner.config().workspace_id, branch_id = %branch_id))]
    pub async fn discard(
        &self,
        session: &claw_guard::ClawSession,
        branch_id: Uuid,
    ) -> BranchResult<()> {
        self.check_write(session, "branch:discard").await?;
        self.inner.discard(branch_id).await
    }

    /// Runs simulation after write access check.
    #[tracing::instrument(skip(self, session, f), fields(workspace_id = %self.inner.config().workspace_id, branch_id = %parent_id))]
    pub async fn simulate<F, Fut>(
        &self,
        session: &claw_guard::ClawSession,
        parent_id: Uuid,
        scenario: SimulationScenario,
        f: F,
    ) -> BranchResult<crate::sandbox::evaluator::EvaluationReport>
    where
        F: FnOnce(sqlx::SqlitePool) -> Fut + Send,
        Fut: std::future::Future<Output = Result<serde_json::Value, BranchError>> + Send,
    {
        self.check_write(session, "branch:simulate").await?;
        self.inner.simulate(parent_id, scenario, f).await
    }

    /// Computes diff after read access check.
    #[tracing::instrument(skip(self, session), fields(workspace_id = %self.inner.config().workspace_id))]
    pub async fn diff(
        &self,
        session: &claw_guard::ClawSession,
        source: Uuid,
        target: Uuid,
    ) -> BranchResult<DiffResult> {
        self.check_read(session, "branch:diff").await?;
        self.inner.diff(source, target).await
    }

    /// Compares branches after read access check.
    #[tracing::instrument(skip(self, session), fields(workspace_id = %self.inner.config().workspace_id))]
    pub async fn compare_branches(
        &self,
        session: &claw_guard::ClawSession,
        source: Uuid,
        target: Uuid,
    ) -> BranchResult<DiffResult> {
        self.check_read(session, "branch:compare").await?;
        self.inner.compare_branches(source, target).await
    }

    /// Lists branches after read access check.
    #[tracing::instrument(skip(self, session), fields(workspace_id = %self.inner.config().workspace_id))]
    pub async fn list_branches(
        &self,
        session: &claw_guard::ClawSession,
    ) -> BranchResult<Vec<Branch>> {
        self.check_read(session, "branch:list").await?;
        self.inner.list(None).await
    }

    /// Reads trunk after read access check.
    #[tracing::instrument(skip(self, session), fields(workspace_id = %self.inner.config().workspace_id))]
    pub async fn trunk(&self, session: &claw_guard::ClawSession) -> BranchResult<Branch> {
        self.check_read(session, "branch:trunk").await?;
        self.inner.trunk().await
    }

    /// Reads branch metrics after read access check.
    #[tracing::instrument(skip(self, session), fields(workspace_id = %self.inner.config().workspace_id, branch_id = %branch_id))]
    pub async fn get_metrics(
        &self,
        session: &claw_guard::ClawSession,
        branch_id: Uuid,
    ) -> BranchResult<BranchMetrics> {
        self.check_read(session, "branch:get_metrics").await?;
        self.inner.metrics(branch_id).await
    }

    /// Cherry-picks selected entities after write access check.
    #[tracing::instrument(skip(self, session, selections), fields(workspace_id = %self.inner.config().workspace_id))]
    pub async fn cherry_pick(
        &self,
        session: &claw_guard::ClawSession,
        source: Uuid,
        target: Uuid,
        selections: Vec<EntitySelection>,
        message: Option<String>,
    ) -> BranchResult<crate::types::CommitResult> {
        self.check_write(session, "branch:cherry_pick").await?;
        self.inner
            .cherry_pick(source, target, selections, message)
            .await
    }

    async fn check_read(
        &self,
        session: &claw_guard::ClawSession,
        resource: &str,
    ) -> BranchResult<()> {
        self.check(session, "read", resource).await
    }

    async fn check_write(
        &self,
        session: &claw_guard::ClawSession,
        resource: &str,
    ) -> BranchResult<()> {
        self.check(session, "write", resource).await
    }

    async fn check(
        &self,
        session: &claw_guard::ClawSession,
        action: &str,
        resource: &str,
    ) -> BranchResult<()> {
        let result = self
            .guard
            .check_access(&session.token, action, resource)
            .await
            .map_err(|error| BranchError::PermissionDenied(error.to_string()))?;

        match result {
            claw_guard::AccessResult::Allowed => Ok(()),
            claw_guard::AccessResult::Denied { reason } => {
                Err(BranchError::PermissionDenied(reason))
            }
            claw_guard::AccessResult::Masked { .. } => Err(BranchError::PermissionDenied(
                "masked access not applicable to branch ops".to_string(),
            )),
        }
    }
}
