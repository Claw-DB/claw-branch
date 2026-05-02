//! Branch engine configuration and environment loading.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{BranchError, BranchResult};

/// Validated runtime configuration for the branch engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchConfig {
    /// The workspace identifier associated with these branches.
    pub workspace_id: Uuid,
    /// The root directory that stores per-branch SQLite files.
    pub branches_dir: PathBuf,
    /// The SQLite registry database path used for metadata and DAG persistence.
    pub registry_db_path: PathBuf,
    /// The maximum number of branches allowed per workspace.
    pub max_branches_per_workspace: usize,
    /// The optional retention limit for branch age in days.
    pub max_branch_age_days: Option<u64>,
    /// Reserved flag for future snapshot compression support.
    pub snapshot_compression: bool,
    /// The interval between garbage collection passes in seconds.
    pub gc_interval_secs: u64,
    /// The orphan age threshold in seconds before snapshot cleanup.
    pub gc_orphan_threshold_secs: u64,
    /// The divergence score threshold that should trigger warnings.
    pub divergence_threshold: f64,
    /// Enables automatic metric refreshes on write operations.
    pub auto_metrics: bool,
    /// The cadence for metric refreshes in seconds.
    pub metrics_refresh_interval_secs: u64,
    /// The canonical name for the trunk branch.
    pub trunk_branch_name: String,
}

/// Builder for a validated [`BranchConfig`].
#[derive(Debug, Clone, Default)]
pub struct BranchConfigBuilder {
    workspace_id: Option<Uuid>,
    branches_dir: Option<PathBuf>,
    registry_db_path: Option<PathBuf>,
    max_branches_per_workspace: Option<usize>,
    max_branch_age_days: Option<Option<u64>>,
    snapshot_compression: Option<bool>,
    gc_interval_secs: Option<u64>,
    gc_orphan_threshold_secs: Option<u64>,
    divergence_threshold: Option<f64>,
    auto_metrics: Option<bool>,
    metrics_refresh_interval_secs: Option<u64>,
    trunk_branch_name: Option<String>,
}

impl BranchConfig {
    /// Creates a new configuration builder.
    pub fn builder() -> BranchConfigBuilder {
        BranchConfigBuilder::default()
    }

    /// Loads configuration from environment variables.
    pub fn from_env() -> BranchResult<Self> {
        let workspace_id = env::var("CLAW_BRANCH_WORKSPACE_ID")
            .ok()
            .and_then(|value| Uuid::parse_str(&value).ok())
            .ok_or_else(|| {
                BranchError::NamingError("missing or invalid CLAW_BRANCH_WORKSPACE_ID".to_string())
            })?;
        let branches_dir = env::var("CLAW_BRANCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./branches"));
        let registry_db_path = env::var("CLAW_BRANCH_REGISTRY")
            .map(PathBuf::from)
            .unwrap_or_else(|_| branches_dir.join("branch_registry.db"));

        Self::builder()
            .workspace_id(workspace_id)
            .branches_dir(branches_dir)
            .registry_db_path(registry_db_path)
            .max_branches_per_workspace(parse_env_or_default("CLAW_BRANCH_MAX", 50_usize)?)
            .max_branch_age_days(parse_optional_env("CLAW_BRANCH_MAX_AGE_DAYS")?)
            .snapshot_compression(false)
            .gc_interval_secs(parse_env_or_default("CLAW_BRANCH_GC_INTERVAL", 3600_u64)?)
            .gc_orphan_threshold_secs(parse_env_or_default(
                "CLAW_BRANCH_GC_ORPHAN_THRESHOLD",
                86400_u64,
            )?)
            .divergence_threshold(parse_env_or_default(
                "CLAW_BRANCH_DIVERGENCE_THRESHOLD",
                0.8_f64,
            )?)
            .auto_metrics(parse_env_or_default("CLAW_BRANCH_AUTO_METRICS", true)?)
            .metrics_refresh_interval_secs(parse_env_or_default(
                "CLAW_BRANCH_METRICS_REFRESH_INTERVAL",
                300_u64,
            )?)
            .trunk_branch_name(
                env::var("CLAW_BRANCH_TRUNK_NAME").unwrap_or_else(|_| "trunk".to_string()),
            )
            .build()
    }

    /// Creates the default configuration rooted under the provided base directory.
    pub fn default_for_workspace(workspace_id: Uuid, base_dir: &Path) -> Self {
        let workspace_root = base_dir.join(workspace_id.to_string());

        Self {
            workspace_id,
            branches_dir: workspace_root.join("branches"),
            registry_db_path: workspace_root.join("branch_registry.db"),
            max_branches_per_workspace: 50,
            max_branch_age_days: None,
            snapshot_compression: false,
            gc_interval_secs: 3600,
            gc_orphan_threshold_secs: 86400,
            divergence_threshold: 0.8,
            auto_metrics: true,
            metrics_refresh_interval_secs: 300,
            trunk_branch_name: "trunk".to_string(),
        }
    }
}

impl BranchConfigBuilder {
    /// Sets the workspace identifier.
    pub fn workspace_id(mut self, workspace_id: Uuid) -> Self {
        self.workspace_id = Some(workspace_id);
        self
    }

    /// Sets the branches directory root.
    pub fn branches_dir<P: Into<PathBuf>>(mut self, branches_dir: P) -> Self {
        self.branches_dir = Some(branches_dir.into());
        self
    }

    /// Sets the registry database path.
    pub fn registry_db_path<P: Into<PathBuf>>(mut self, registry_db_path: P) -> Self {
        self.registry_db_path = Some(registry_db_path.into());
        self
    }

    /// Sets the maximum number of branches.
    pub fn max_branches_per_workspace(mut self, max_branches_per_workspace: usize) -> Self {
        self.max_branches_per_workspace = Some(max_branches_per_workspace);
        self
    }

    /// Sets the maximum allowed branch age in days.
    pub fn max_branch_age_days(mut self, max_branch_age_days: Option<u64>) -> Self {
        self.max_branch_age_days = Some(max_branch_age_days);
        self
    }

    /// Sets the reserved snapshot compression flag.
    pub fn snapshot_compression(mut self, snapshot_compression: bool) -> Self {
        self.snapshot_compression = Some(snapshot_compression);
        self
    }

    /// Sets the garbage collection interval.
    pub fn gc_interval_secs(mut self, gc_interval_secs: u64) -> Self {
        self.gc_interval_secs = Some(gc_interval_secs);
        self
    }

    /// Sets the orphan cleanup threshold.
    pub fn gc_orphan_threshold_secs(mut self, gc_orphan_threshold_secs: u64) -> Self {
        self.gc_orphan_threshold_secs = Some(gc_orphan_threshold_secs);
        self
    }

    /// Sets the divergence warning threshold.
    pub fn divergence_threshold(mut self, divergence_threshold: f64) -> Self {
        self.divergence_threshold = Some(divergence_threshold);
        self
    }

    /// Enables or disables automatic metrics refresh.
    pub fn auto_metrics(mut self, auto_metrics: bool) -> Self {
        self.auto_metrics = Some(auto_metrics);
        self
    }

    /// Sets the metrics refresh interval.
    pub fn metrics_refresh_interval_secs(mut self, metrics_refresh_interval_secs: u64) -> Self {
        self.metrics_refresh_interval_secs = Some(metrics_refresh_interval_secs);
        self
    }

    /// Sets the canonical trunk branch name.
    pub fn trunk_branch_name<S: Into<String>>(mut self, trunk_branch_name: S) -> Self {
        self.trunk_branch_name = Some(trunk_branch_name.into());
        self
    }

    /// Builds and validates the final configuration.
    pub fn build(self) -> BranchResult<BranchConfig> {
        let workspace_id = self.workspace_id.unwrap_or_default();
        let branches_dir = self
            .branches_dir
            .unwrap_or_else(|| PathBuf::from("./branches"));
        let registry_db_path = self
            .registry_db_path
            .unwrap_or_else(|| branches_dir.join("branch_registry.db"));
        let max_branches_per_workspace = self.max_branches_per_workspace.unwrap_or(50);
        let divergence_threshold = self.divergence_threshold.unwrap_or(0.8);

        if workspace_id.is_nil() {
            return Err(BranchError::NamingError(
                "workspace_id must not be nil".to_string(),
            ));
        }
        if max_branches_per_workspace < 1 {
            return Err(BranchError::NamingError(
                "max_branches_per_workspace must be at least 1".to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&divergence_threshold) || divergence_threshold == 0.0 {
            return Err(BranchError::NamingError(
                "divergence_threshold must be within (0.0, 1.0]".to_string(),
            ));
        }

        if let Some(parent) = branches_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Some(parent) = registry_db_path.parent() {
            fs::create_dir_all(parent)?;
        }

        Ok(BranchConfig {
            workspace_id,
            branches_dir,
            registry_db_path,
            max_branches_per_workspace,
            max_branch_age_days: self.max_branch_age_days.unwrap_or(None),
            snapshot_compression: self.snapshot_compression.unwrap_or(false),
            gc_interval_secs: self.gc_interval_secs.unwrap_or(3600),
            gc_orphan_threshold_secs: self.gc_orphan_threshold_secs.unwrap_or(86400),
            divergence_threshold,
            auto_metrics: self.auto_metrics.unwrap_or(true),
            metrics_refresh_interval_secs: self.metrics_refresh_interval_secs.unwrap_or(300),
            trunk_branch_name: self
                .trunk_branch_name
                .unwrap_or_else(|| "trunk".to_string()),
        })
    }
}

fn parse_env_or_default<T>(key: &str, default: T) -> BranchResult<T>
where
    T: std::str::FromStr,
{
    match env::var(key) {
        Ok(value) => value
            .parse::<T>()
            .map_err(|_| BranchError::NamingError(format!("invalid value for {key}"))),
        Err(_) => Ok(default),
    }
}

fn parse_optional_env<T>(key: &str) -> BranchResult<Option<T>>
where
    T: std::str::FromStr,
{
    match env::var(key) {
        Ok(value) => value
            .parse::<T>()
            .map(Some)
            .map_err(|_| BranchError::NamingError(format!("invalid value for {key}"))),
        Err(_) => Ok(None),
    }
}
