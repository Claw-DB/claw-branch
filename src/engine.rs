//! Unified [`BranchEngine`] coordinating all claw-branch subsystems.

use std::{path::Path, sync::Arc};

use uuid::Uuid;

use crate::{
    branch::{lifecycle::BranchLifecycle, store::BranchStore},
    commit::{cherry::CherryPick, selective::SelectiveCommit},
    config::BranchConfig,
    dag::graph::DagGraph,
    diff::extractor::DiffExtractor,
    error::{BranchError, BranchResult},
    merge::{
        resolver::ConflictResolver, strategies::MergeStrategy,
        three_way::{MergePreview, ThreeWayMerger},
    },
    metrics::{reporter::MetricsReporter, tracker::MetricsTracker},
    sandbox::{
        environment::{SimulationEnvironment, SimulationScenario},
        evaluator::{EvaluationReport, SandboxEvaluator},
        runner::SandboxRunner,
    },
    snapshot::{
        copier::SnapshotCopier,
        gc::{GcReport, SnapshotGc},
    },
    types::{
        Branch, BranchMetrics, BranchStatus, CommitResult, DiffResult, MergeResult,
        WorkspaceReport,
    },
};

/// Unified coordinator for all claw-branch capabilities.
///
/// `BranchEngine` is the primary entry point for library consumers.  It owns all
/// subsystems and exposes a single, consistent async API.
///
/// # Quick-start
/// ```rust,ignore
/// use claw_branch::{BranchConfig, BranchEngine};
/// use std::path::PathBuf;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let config = BranchConfig::builder()
///         .workspace_dir(PathBuf::from("/tmp/myproject"))
///         .build()?;
///     let engine = BranchEngine::new(config, Path::new("/data/trunk.db")).await?;
///
///     let feature = engine.fork_trunk("feature/my-idea").await?;
///     let diff    = engine.diff(engine.trunk().await?.id, feature.id).await?;
///     println!("{} entities changed", diff.stats.modified);
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct BranchEngine {
    config: Arc<BranchConfig>,
    store: Arc<BranchStore>,
    dag: Arc<DagGraph>,
    lifecycle: Arc<BranchLifecycle>,
    metrics: Arc<MetricsTracker>,
    gc: SnapshotGc,
}

impl BranchEngine {
    // ── Construction ─────────────────────────────────────────────────────────

    /// Creates a new workspace, initialising all subsystems and creating the trunk branch.
    ///
    /// `trunk_db_path` is the source database that will be copied into the trunk branch snapshot.
    #[tracing::instrument(skip(config), fields(workspace_id = %config.workspace_id))]
    pub async fn new(config: BranchConfig, trunk_db_path: &Path) -> BranchResult<Self> {
        let config = Arc::new(config);
        let store = Arc::new(BranchStore::new(&config.registry_db_path).await?);
        let dag = Arc::new(DagGraph::new(Arc::clone(&config)));
        let copier = Arc::new(SnapshotCopier::new(Arc::clone(&config)));
        let lifecycle = Arc::new(BranchLifecycle::new(
            Arc::clone(&store),
            Arc::clone(&copier),
            Arc::clone(&dag),
            Arc::clone(&config),
        ));
        let metrics = Arc::new(MetricsTracker::new(Arc::clone(&store), Arc::clone(&config)));
        let gc = SnapshotGc::new(Arc::clone(&config), Arc::clone(&store));

        let engine = Self { config, store, dag, lifecycle, metrics, gc };

        // Ensure the trunk branch exists; create it from the provided source DB.
        if engine.trunk_opt().await?.is_none() {
            engine
                .lifecycle
                .create_trunk(engine.config.workspace_id, trunk_db_path)
                .await?;
        }

        Ok(engine)
    }

    /// Opens an existing workspace without creating a new trunk.
    #[tracing::instrument(skip(config), fields(workspace_id = %config.workspace_id))]
    pub async fn open(config: BranchConfig) -> BranchResult<Self> {
        let config = Arc::new(config);
        let store = Arc::new(BranchStore::new(&config.registry_db_path).await?);
        let dag = Arc::new(DagGraph::new(Arc::clone(&config)));
        let copier = Arc::new(SnapshotCopier::new(Arc::clone(&config)));
        let lifecycle = Arc::new(BranchLifecycle::new(
            Arc::clone(&store),
            Arc::clone(&copier),
            Arc::clone(&dag),
            Arc::clone(&config),
        ));
        let metrics = Arc::new(MetricsTracker::new(Arc::clone(&store), Arc::clone(&config)));
        let gc = SnapshotGc::new(Arc::clone(&config), Arc::clone(&store));

        Ok(Self { config, store, dag, lifecycle, metrics, gc })
    }

    // ── Accessors ────────────────────────────────────────────────────────────

    /// Returns the engine configuration.
    pub fn config(&self) -> &BranchConfig {
        &self.config
    }

    /// Returns the shared branch store.
    pub fn store(&self) -> Arc<BranchStore> {
        Arc::clone(&self.store)
    }

    /// Returns the shared DAG graph.
    pub fn dag(&self) -> Arc<DagGraph> {
        Arc::clone(&self.dag)
    }

    /// Returns the lifecycle coordinator.
    pub fn lifecycle(&self) -> Arc<BranchLifecycle> {
        Arc::clone(&self.lifecycle)
    }

    // ── Branch management ────────────────────────────────────────────────────

    /// Retrieves a branch by ID.
    #[tracing::instrument(skip(self))]
    pub async fn get(&self, id: Uuid) -> BranchResult<Branch> {
        self.store.get(id).await
    }

    /// Retrieves a branch by human-readable name within the workspace.
    #[tracing::instrument(skip(self))]
    pub async fn get_by_name(&self, name: &str) -> BranchResult<Branch> {
        self.store.get_by_name(self.config.workspace_id, name).await
    }

    /// Lists all branches in the workspace, optionally filtered by status.
    #[tracing::instrument(skip(self))]
    pub async fn list(&self, status: Option<BranchStatus>) -> BranchResult<Vec<Branch>> {
        self.store.list(self.config.workspace_id, status).await
    }

    /// Returns the trunk branch.
    #[tracing::instrument(skip(self))]
    pub async fn trunk(&self) -> BranchResult<Branch> {
        self.trunk_opt().await?.ok_or_else(|| {
            BranchError::NamingError("trunk branch not found".to_string())
        })
    }

    /// Forks a new branch from `parent_id`.
    #[tracing::instrument(skip(self))]
    pub async fn fork(
        &self,
        parent_id: Uuid,
        name: &str,
        description: Option<&str>,
    ) -> BranchResult<Branch> {
        self.lifecycle.fork(parent_id, name, description).await
    }

    /// Forks a new branch from the trunk.
    #[tracing::instrument(skip(self))]
    pub async fn fork_trunk(&self, name: &str) -> BranchResult<Branch> {
        let trunk = self.trunk().await?;
        self.lifecycle.fork(trunk.id, name, None).await
    }

    /// Discards a branch, marking it as inactive.
    #[tracing::instrument(skip(self))]
    pub async fn discard(&self, id: Uuid) -> BranchResult<()> {
        self.lifecycle.discard(id).await
    }

    /// Archives a branch.
    #[tracing::instrument(skip(self))]
    pub async fn archive(&self, id: Uuid) -> BranchResult<()> {
        self.lifecycle.archive(id).await
    }

    // ── Diff ─────────────────────────────────────────────────────────────────

    /// Computes the diff between two branches.
    #[tracing::instrument(skip(self))]
    pub async fn diff(&self, a: Uuid, b: Uuid) -> BranchResult<DiffResult> {
        let branch_a = self.store.get(a).await?;
        let branch_b = self.store.get(b).await?;
        let extractor = DiffExtractor::new(Arc::clone(&self.config));
        extractor.diff(&branch_a, &branch_b, None).await
    }

    // ── Merge ────────────────────────────────────────────────────────────────

    /// Merges `source` into `target` using the given strategy.
    ///
    /// Uses the parent of `source` as the merge base. Falls back to `target` as base
    /// when no explicit common ancestor is available.
    #[tracing::instrument(skip(self))]
    pub async fn merge(
        &self,
        source: Uuid,
        target: Uuid,
        strategy: MergeStrategy,
    ) -> BranchResult<MergeResult> {
        let source_branch = self.store.get(source).await?;
        let target_branch = self.store.get(target).await?;
        let base_id = source_branch.parent_id.unwrap_or(target);
        let base_branch = self.store.get(base_id).await?;
        let resolver = Arc::new(ConflictResolver);
        let merger = ThreeWayMerger::new(resolver, Arc::clone(&self.config));
        merger.merge(&base_branch, &source_branch, &target_branch, &strategy, None).await
    }

    /// Previews a three-way merge without applying any changes.
    #[tracing::instrument(skip(self))]
    pub async fn merge_preview(&self, source: Uuid, target: Uuid) -> BranchResult<MergePreview> {
        let source_branch = self.store.get(source).await?;
        let target_branch = self.store.get(target).await?;
        let base_id = source_branch.parent_id.unwrap_or(target);
        let base_branch = self.store.get(base_id).await?;
        let resolver = Arc::new(ConflictResolver);
        let merger = ThreeWayMerger::new(resolver, Arc::clone(&self.config));
        merger.preview(&base_branch, &source_branch, &target_branch, None).await
    }

    // ── Commit ───────────────────────────────────────────────────────────────

    /// Executes a selective cherry-pick commit.
    #[tracing::instrument(skip(self))]
    pub async fn commit(&self, cherry: CherryPick) -> BranchResult<CommitResult> {
        let committer = SelectiveCommit::from_store(
            Arc::clone(&self.store),
            cherry.source_branch_id,
            cherry.target_branch_id,
            self.config.workspace_id,
        )
        .await?;
        committer.commit(&cherry).await
    }

    /// Commits all entities from `source_id` to the trunk branch.
    #[tracing::instrument(skip(self))]
    pub async fn commit_to_trunk(&self, source_id: Uuid) -> BranchResult<CommitResult> {
        let trunk = self.trunk().await?;
        let committer = SelectiveCommit::from_store(
            Arc::clone(&self.store),
            source_id,
            trunk.id,
            self.config.workspace_id,
        )
        .await?;
        committer.commit_all(source_id, trunk.id).await
    }

    // ── Simulation ───────────────────────────────────────────────────────────

    /// Runs an agent simulation in an isolated sandbox branch.
    ///
    /// Returns an evaluation report with diff, metrics, and a promotion recommendation.
    #[tracing::instrument(skip(self, agent_fn))]
    pub async fn simulate<F, Fut>(
        &self,
        parent_id: Uuid,
        scenario: SimulationScenario,
        agent_fn: F,
    ) -> BranchResult<EvaluationReport>
    where
        F: FnOnce(sqlx::SqlitePool) -> Fut,
        Fut: std::future::Future<Output = BranchResult<serde_json::Value>>,
    {
        let parent = self.store.get(parent_id).await?;
        let env = SimulationEnvironment::setup(
            &parent,
            scenario,
            Arc::clone(&self.config),
            Arc::clone(&self.lifecycle),
        )
        .await?;

        let mut runner = SandboxRunner::new(env, Arc::clone(&self.config));
        let _ = runner.run(agent_fn).await?;

        let evaluator = SandboxEvaluator;
        evaluator
            .evaluate(&runner.env, &parent, Arc::clone(&self.config))
            .await
    }

    // ── DAG ──────────────────────────────────────────────────────────────────

    /// Returns the ancestor lineage of a branch as an ordered list (root-first).
    #[tracing::instrument(skip(self))]
    pub async fn lineage(&self, branch_id: Uuid) -> BranchResult<Vec<Uuid>> {
        let mut ancestors = self.dag.ancestors_of(branch_id)?;
        ancestors.reverse();
        ancestors.push(branch_id);
        Ok(ancestors)
    }

    /// Exports the branch DAG as a Graphviz DOT string.
    #[tracing::instrument(skip(self))]
    pub async fn dag_dot(&self) -> BranchResult<String> {
        crate::dag::dot::export_dot(&self.dag, &self.store).await
    }

    // ── Metrics ──────────────────────────────────────────────────────────────

    /// Returns up-to-date metrics for the given branch.
    #[tracing::instrument(skip(self))]
    pub async fn metrics(&self, branch_id: Uuid) -> BranchResult<BranchMetrics> {
        let branch = self.store.get(branch_id).await?;
        self.metrics.refresh(&branch).await
    }

    /// Generates a workspace-wide metrics report.
    #[tracing::instrument(skip(self))]
    pub async fn workspace_report(&self) -> BranchResult<WorkspaceReport> {
        MetricsReporter.workspace_report(self.config.workspace_id, &self.store).await
    }

    // ── GC ───────────────────────────────────────────────────────────────────

    /// Runs snapshot garbage collection, deleting orphaned and discarded branch data.
    #[tracing::instrument(skip(self))]
    pub async fn gc(&self) -> BranchResult<GcReport> {
        self.gc.run().await
    }

    // ── Private ──────────────────────────────────────────────────────────────

    async fn trunk_opt(&self) -> BranchResult<Option<Branch>> {
        match self
            .store
            .get_by_slug(self.config.workspace_id, &self.config.trunk_branch_name)
            .await
        {
            Ok(b) => Ok(Some(b)),
            Err(BranchError::BranchNotFound(_)) => Ok(None),
            // BranchStore currently reports lookup misses as BranchAlreadyExists(name).
            // Treat trunk-name misses as absent so engine creation remains idempotent.
            Err(BranchError::BranchAlreadyExists(name))
                if name == self.config.trunk_branch_name =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}
