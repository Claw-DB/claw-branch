//! Isolated simulation environments backed by a forked branch.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};
use uuid::Uuid;

use crate::{
    branch::lifecycle::BranchLifecycle,
    config::BranchConfig,
    error::BranchResult,
    types::{Branch, SimulationOutcome},
};

/// Configures a simulation scenario run in an isolated sandbox branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationScenario {
    /// A short name for the scenario (used in the branch label).
    pub name: String,
    /// Human-readable description of what the scenario tests.
    pub description: String,
    /// Maximum number of write operations before the sandbox is halted.
    pub max_ops: Option<u32>,
    /// Timeout in seconds after which the run is aborted.
    pub timeout_secs: Option<u64>,
    /// Optional JSON seed data that may be loaded into the sandbox on setup.
    pub seed_data: Option<serde_json::Value>,
}

/// Describes the current lifecycle phase of a sandbox environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SandboxStatus {
    /// The sandbox is actively running an agent.
    Running,
    /// The agent finished and produced an outcome.
    Completed {
        /// The captured simulation result.
        outcome: SimulationOutcome,
    },
    /// The run failed with an error message.
    Failed(String),
    /// The environment was abandoned without completing.
    Abandoned,
}

/// An isolated, forked branch environment used to run and evaluate agent simulations.
///
/// # Example
/// ```rust,ignore
/// let env = SimulationEnvironment::setup(&parent, scenario, config, lifecycle).await?;
/// let pool = env.branch_pool()?;
/// // run agent against pool ...
/// env.teardown(lifecycle).await?;
/// ```
#[derive(Debug, Clone)]
pub struct SimulationEnvironment {
    /// A unique identifier for this simulation run.
    pub id: Uuid,
    /// A human-readable label for the sandbox branch.
    pub label: String,
    /// The isolated branch backing this environment.
    pub branch: Branch,
    /// The parent branch from which this sandbox was forked.
    pub parent_branch_id: Uuid,
    /// The timestamp the environment was initialized.
    pub started_at: DateTime<Utc>,
    /// The scenario definition that governs this run.
    pub scenario: SimulationScenario,
    /// The current lifecycle status of the sandbox.
    pub status: SandboxStatus,
}

impl SimulationEnvironment {
    /// Forks `parent` into a new sandbox branch and returns an initialized environment.
    ///
    /// The branch name follows the pattern `sandbox/{scenario.name}/{id_short}`.
    pub async fn setup(
        parent: &Branch,
        scenario: SimulationScenario,
        _config: Arc<BranchConfig>,
        lifecycle: Arc<BranchLifecycle>,
    ) -> BranchResult<Self> {
        let id = Uuid::new_v4();
        let id_short = &id.to_string()[..8];
        let branch_name = format!("sandbox/{}/{}", scenario.name, id_short);
        let label = branch_name.clone();

        let branch = lifecycle
            .fork(parent.id, &branch_name, Some(&scenario.description))
            .await?;

        Ok(Self {
            id,
            label,
            branch,
            parent_branch_id: parent.id,
            started_at: Utc::now(),
            scenario,
            status: SandboxStatus::Running,
        })
    }

    /// Discards the sandbox branch, freeing any on-disk state.
    ///
    /// After calling `teardown`, the branch database is marked as `Discarded` and
    /// will be removed on the next GC pass.
    pub async fn teardown(&mut self, lifecycle: Arc<BranchLifecycle>) -> BranchResult<()> {
        lifecycle.discard(self.branch.id).await?;
        self.status = SandboxStatus::Abandoned;
        Ok(())
    }

    /// Opens a read-write SQLite pool connected to the sandbox branch database.
    pub async fn branch_pool(&self) -> BranchResult<SqlitePool> {
        SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&self.branch.db_path)
                    .create_if_missing(false)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await
            .map_err(crate::error::BranchError::Database)
    }
}
