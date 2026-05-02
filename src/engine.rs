//! Unified [`BranchEngine`] coordinating all claw-branch subsystems.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use tokio::{sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    branch::{lifecycle::BranchLifecycle, store::BranchStore},
    commit::{cherry::CherryPick, selective::SelectiveCommit, EntitySelection},
    config::BranchConfig,
    dag::graph::DagGraph,
    diff::extractor::DiffExtractor,
    error::{BranchError, BranchResult},
    merge::{
        resolver::ConflictResolver,
        strategies::MergeStrategy,
        three_way::{MergePreview, ThreeWayMerger},
    },
    metrics::{reporter::MetricsReporter, tracker::MetricsTracker},
    sandbox::{
        environment::{SimulationEnvironment, SimulationScenario},
        evaluator::{EvaluationReport, SandboxEvaluator},
        runner::SandboxRunner,
    },
    snapshot::{
        copier::{cleanup_incomplete_tmp_files, SnapshotCopier},
        gc::{GcReport, SnapshotGc},
    },
    types::{
        Branch, BranchMetrics, BranchStatus, CommitResult, DiffResult, MergeResult, WorkspaceReport,
    },
};
use thiserror::Error;

/// Typed builder validation errors for [`BranchEngineBuilder`].
#[derive(Debug, Error)]
pub enum BranchConfigError {
    /// Required workspace id was not set.
    #[error("missing required field: workspace_id")]
    MissingWorkspaceId,
    /// Required branches dir was not set.
    #[error("missing required field: branches_dir")]
    MissingBranchesDir,
    /// Required trunk source db path was not set.
    #[error("missing required field: trunk_source_db")]
    MissingTrunkSourceDb,
    /// The underlying branch config was invalid.
    #[error(transparent)]
    Branch(#[from] BranchError),
}

/// Ergonomic builder for constructing a [`BranchEngine`].
#[derive(Debug, Default, Clone)]
pub struct BranchEngineBuilder {
    workspace_id: Option<Uuid>,
    branches_dir: Option<PathBuf>,
    trunk_source_db: Option<PathBuf>,
    max_branches: Option<usize>,
    gc_interval_secs: Option<u64>,
}

impl BranchEngineBuilder {
    /// Creates a new builder instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the workspace identifier.
    pub fn workspace_id(mut self, workspace_id: Uuid) -> Self {
        self.workspace_id = Some(workspace_id);
        self
    }

    /// Sets the branches directory path.
    pub fn branches_dir<P: Into<PathBuf>>(mut self, branches_dir: P) -> Self {
        self.branches_dir = Some(branches_dir.into());
        self
    }

    /// Sets the trunk source SQLite path.
    pub fn trunk_source_db<P: Into<PathBuf>>(mut self, trunk_source_db: P) -> Self {
        self.trunk_source_db = Some(trunk_source_db.into());
        self
    }

    /// Sets max branch count per workspace.
    pub fn max_branches(mut self, max_branches: usize) -> Self {
        self.max_branches = Some(max_branches);
        self
    }

    /// Sets GC interval in seconds.
    pub fn gc_interval_secs(mut self, gc_interval_secs: u64) -> Self {
        self.gc_interval_secs = Some(gc_interval_secs);
        self
    }

    /// Builds a fully-initialized [`BranchEngine`].
    pub async fn build(self) -> Result<BranchEngine, BranchConfigError> {
        let workspace_id = self
            .workspace_id
            .ok_or(BranchConfigError::MissingWorkspaceId)?;
        let branches_dir = self
            .branches_dir
            .ok_or(BranchConfigError::MissingBranchesDir)?;
        let trunk_source_db = self
            .trunk_source_db
            .ok_or(BranchConfigError::MissingTrunkSourceDb)?;

        let mut builder = BranchConfig::builder()
            .workspace_id(workspace_id)
            .branches_dir(branches_dir);
        if let Some(max) = self.max_branches {
            builder = builder.max_branches_per_workspace(max);
        }
        if let Some(gc_interval) = self.gc_interval_secs {
            builder = builder.gc_interval_secs(gc_interval);
        }

        let config = builder.build().map_err(BranchConfigError::Branch)?;
        BranchEngine::new(config, &trunk_source_db)
            .await
            .map_err(BranchConfigError::Branch)
    }
}

/// Unified coordinator for all claw-branch capabilities.
///
/// `BranchEngine` is the primary entry point for library consumers.  It owns all
/// subsystems and exposes a single, consistent async API.
///
/// This engine does not perform authorization checks. Use it when your runtime
/// already validates caller permissions externally.
///
/// For workspace-scoped authorization checks at this crate boundary, enable the
/// `guarded` feature and use `GuardedBranchEngine`.
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
    gc_scheduler: Arc<Mutex<Option<GcScheduler>>>,
}

struct GcScheduler {
    cancellation_token: CancellationToken,
    task_handle: JoinHandle<()>,
}

impl BranchEngine {
    // ── Construction ─────────────────────────────────────────────────────────

    /// Creates a new workspace, initialising all subsystems and creating the trunk branch.
    ///
    /// `trunk_db_path` is the source database that will be copied into the trunk branch snapshot.
    #[tracing::instrument(skip(config), fields(workspace_id = %config.workspace_id))]
    pub async fn new(config: BranchConfig, trunk_db_path: &Path) -> BranchResult<Self> {
        validate_branches_dir(&config)?;
        cleanup_incomplete_tmp_files(&config.branches_dir).await?;
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

        let engine = Self {
            config,
            store,
            dag,
            lifecycle,
            metrics,
            gc,
            gc_scheduler: Arc::new(Mutex::new(None)),
        };

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
        validate_branches_dir(&config)?;
        cleanup_incomplete_tmp_files(&config.branches_dir).await?;
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

        Ok(Self {
            config,
            store,
            dag,
            lifecycle,
            metrics,
            gc,
            gc_scheduler: Arc::new(Mutex::new(None)),
        })
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
        let branch = self.store.get(id).await?;
        self.ensure_workspace_access(&branch)?;
        Ok(branch)
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
        self.trunk_opt()
            .await?
            .ok_or_else(|| BranchError::NamingError("trunk branch not found".to_string()))
    }

    /// Forks a new branch from `parent_id`.
    #[tracing::instrument(skip(self))]
    pub async fn fork(
        &self,
        parent_id: Uuid,
        name: &str,
        description: Option<&str>,
    ) -> BranchResult<Branch> {
        let parent = self.store.get(parent_id).await?;
        self.ensure_workspace_access(&parent)?;
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
        let branch = self.store.get(id).await?;
        self.ensure_workspace_access(&branch)?;
        self.lifecycle.discard(id).await
    }

    /// Archives a branch.
    #[tracing::instrument(skip(self))]
    pub async fn archive(&self, id: Uuid) -> BranchResult<()> {
        let branch = self.store.get(id).await?;
        self.ensure_workspace_access(&branch)?;
        self.lifecycle.archive(id).await
    }

    // ── Diff ─────────────────────────────────────────────────────────────────

    /// Computes the diff between two branches.
    #[tracing::instrument(skip(self))]
    pub async fn diff(&self, a: Uuid, b: Uuid) -> BranchResult<DiffResult> {
        let branch_a = self.store.get(a).await?;
        let branch_b = self.store.get(b).await?;
        self.ensure_workspace_access(&branch_a)?;
        self.ensure_workspace_access(&branch_b)?;
        let extractor = DiffExtractor::new(Arc::clone(&self.config));
        extractor.diff(&branch_a, &branch_b, None).await
    }

    /// Compares two branches using the same semantics as [`Self::diff`].
    #[tracing::instrument(skip(self))]
    pub async fn compare_branches(&self, a: Uuid, b: Uuid) -> BranchResult<DiffResult> {
        self.diff(a, b).await
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
        self.ensure_workspace_access(&source_branch)?;
        self.ensure_workspace_access(&target_branch)?;
        let base_id = source_branch.parent_id.unwrap_or(target);
        let base_branch = self.store.get(base_id).await?;
        self.ensure_workspace_access(&base_branch)?;
        let resolver = Arc::new(ConflictResolver);
        let merger = ThreeWayMerger::new(resolver, Arc::clone(&self.config));
        merger
            .merge(
                &base_branch,
                &source_branch,
                &target_branch,
                &strategy,
                None,
            )
            .await
    }

    /// Previews a three-way merge without applying any changes.
    #[tracing::instrument(skip(self))]
    pub async fn merge_preview(&self, source: Uuid, target: Uuid) -> BranchResult<MergePreview> {
        let source_branch = self.store.get(source).await?;
        let target_branch = self.store.get(target).await?;
        self.ensure_workspace_access(&source_branch)?;
        self.ensure_workspace_access(&target_branch)?;
        let base_id = source_branch.parent_id.unwrap_or(target);
        let base_branch = self.store.get(base_id).await?;
        self.ensure_workspace_access(&base_branch)?;
        let resolver = Arc::new(ConflictResolver);
        let merger = ThreeWayMerger::new(resolver, Arc::clone(&self.config));
        merger
            .preview(&base_branch, &source_branch, &target_branch, None)
            .await
    }

    // ── Commit ───────────────────────────────────────────────────────────────

    /// Executes a selective cherry-pick commit.
    #[tracing::instrument(skip(self))]
    pub async fn commit(&self, cherry: CherryPick) -> BranchResult<CommitResult> {
        let source = self.store.get(cherry.source_branch_id).await?;
        let target = self.store.get(cherry.target_branch_id).await?;
        self.ensure_workspace_access(&source)?;
        self.ensure_workspace_access(&target)?;
        let committer = SelectiveCommit::from_store(
            Arc::clone(&self.store),
            cherry.source_branch_id,
            cherry.target_branch_id,
            self.config.workspace_id,
        )
        .await?;
        committer.commit(&cherry).await
    }

    /// Cherry-picks selected entities from source into target.
    #[tracing::instrument(skip(self, selections))]
    pub async fn cherry_pick(
        &self,
        source_id: Uuid,
        target_id: Uuid,
        selections: Vec<EntitySelection>,
        message: Option<String>,
    ) -> BranchResult<CommitResult> {
        self.commit(CherryPick {
            source_branch_id: source_id,
            target_branch_id: target_id,
            entity_selections: selections,
            message,
        })
        .await
    }

    /// Commits all entities from `source_id` to the trunk branch.
    #[tracing::instrument(skip(self))]
    pub async fn commit_to_trunk(&self, source_id: Uuid) -> BranchResult<CommitResult> {
        let source = self.store.get(source_id).await?;
        self.ensure_workspace_access(&source)?;
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
        self.ensure_workspace_access(&parent)?;
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
        MetricsReporter
            .workspace_report(self.config.workspace_id, &self.store)
            .await
    }

    // ── GC ───────────────────────────────────────────────────────────────────

    /// Runs snapshot garbage collection, deleting orphaned and discarded branch data.
    #[tracing::instrument(skip(self))]
    pub async fn gc(&self) -> BranchResult<GcReport> {
        self.gc.run().await
    }

    /// Starts the background GC scheduler if it is not already running.
    #[tracing::instrument(skip(self))]
    pub async fn start_gc_scheduler(&self) -> BranchResult<()> {
        let mut guard = self.gc_scheduler.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let cancellation_token = CancellationToken::new();
        let child_token = cancellation_token.clone();
        let engine = self.clone();
        let interval_seconds = self.config.gc_interval_secs.max(1);

        let task_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_seconds));
            loop {
                tokio::select! {
                    _ = child_token.cancelled() => break,
                    _ = interval.tick() => {
                        let _ = engine.gc().await;
                    }
                }
            }
        });

        *guard = Some(GcScheduler {
            cancellation_token,
            task_handle,
        });
        Ok(())
    }

    /// Stops the background GC scheduler and waits for shutdown completion.
    #[tracing::instrument(skip(self))]
    pub async fn stop_gc_scheduler(&self) -> BranchResult<()> {
        let scheduler = {
            let mut guard = self.gc_scheduler.lock().await;
            guard.take()
        };

        if let Some(scheduler) = scheduler {
            scheduler.cancellation_token.cancel();
            scheduler
                .task_handle
                .await
                .map_err(|error| BranchError::SandboxError(error.to_string()))?;
        }

        Ok(())
    }

    /// Gracefully shuts down background tasks owned by the engine.
    #[tracing::instrument(skip(self))]
    pub async fn shutdown(&self) -> BranchResult<()> {
        self.stop_gc_scheduler().await
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

    fn ensure_workspace_access(&self, branch: &Branch) -> BranchResult<()> {
        if branch.workspace_id != self.config.workspace_id {
            return Err(BranchError::PermissionDenied(
                "cross-workspace branch access denied".to_string(),
            ));
        }
        Ok(())
    }
}

fn validate_branches_dir(config: &BranchConfig) -> BranchResult<()> {
    fs::create_dir_all(&config.branches_dir)?;
    let expected_root = fs::canonicalize(
        config
            .branches_dir
            .parent()
            .unwrap_or_else(|| Path::new(".")),
    )?;
    let resolved_dir = fs::canonicalize(&config.branches_dir)?;

    if !resolved_dir.is_absolute() || !resolved_dir.starts_with(&expected_root) {
        return Err(BranchError::InvalidConfig(
            "branches_dir resolves outside expected root".to_string(),
        ));
    }

    let mut cursor = resolved_dir.as_path();
    while cursor.starts_with(&expected_root) && cursor != expected_root {
        let meta = fs::symlink_metadata(cursor)?;
        if meta.file_type().is_symlink() {
            let link_target = fs::canonicalize(cursor)?;
            if !link_target.starts_with(&expected_root) {
                return Err(BranchError::InvalidConfig(
                    "branches_dir resolves outside expected root".to_string(),
                ));
            }
        }
        if let Some(parent) = cursor.parent() {
            cursor = parent;
        } else {
            break;
        }
    }

    Ok(())
}
