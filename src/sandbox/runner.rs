//! Agent scenario execution inside an isolated sandbox environment.

use sqlx::SqlitePool;
use std::{future::Future, sync::Arc, time::Instant};

use crate::{
    config::BranchConfig,
    error::{BranchError, BranchResult},
    sandbox::environment::{SandboxStatus, SimulationEnvironment},
    types::SimulationOutcome,
};

/// Drives an agent function inside a [`SimulationEnvironment`] and captures its outcome.
///
/// # Example
/// ```rust,ignore
/// let mut runner = SandboxRunner::new(env, config);
/// let outcome = runner.run(|pool| async move { /* agent logic */ Ok(json!({})) }).await?;
/// ```
pub struct SandboxRunner {
    /// The simulation environment this runner operates on.
    pub env: SimulationEnvironment,
    /// Workspace configuration for pool and path resolution.
    pub config: Arc<BranchConfig>,
}

impl SandboxRunner {
    /// Creates a new runner for the given environment.
    pub fn new(env: SimulationEnvironment, config: Arc<BranchConfig>) -> Self {
        Self { env, config }
    }

    /// Executes `agent_fn` against the sandbox branch pool, capturing the result.
    ///
    /// On success, the environment status is updated to [`SandboxStatus::Completed`].
    /// On error, the status becomes [`SandboxStatus::Failed`].
    pub async fn run<F, Fut>(&mut self, agent_fn: F) -> BranchResult<SimulationOutcome>
    where
        F: FnOnce(SqlitePool) -> Fut,
        Fut: Future<Output = BranchResult<serde_json::Value>>,
    {
        let started = Instant::now();
        let pool = self.env.branch_pool().await?;

        let agent_result = agent_fn(pool).await;
        let duration_ms = started.elapsed().as_millis() as u64;

        match agent_result {
            Ok(agent_output) => {
                let outcome = SimulationOutcome {
                    agent_output,
                    ops_executed: 1, // updated via track_op in production usage
                    duration_ms,
                };
                self.env.status = SandboxStatus::Completed {
                    outcome: outcome.clone(),
                };
                Ok(outcome)
            }
            Err(error) => {
                self.env.status = SandboxStatus::Failed(error.to_string());
                Err(error)
            }
        }
    }

    /// Executes `agent_fn` with a wall-clock timeout in seconds.
    ///
    /// Returns `BranchError::SandboxError` when the timeout elapses before the agent finishes.
    pub async fn run_with_timeout<F, Fut>(
        &mut self,
        agent_fn: F,
        timeout_secs: u64,
    ) -> BranchResult<SimulationOutcome>
    where
        F: FnOnce(SqlitePool) -> Fut,
        Fut: Future<Output = BranchResult<serde_json::Value>>,
    {
        let pool = self.env.branch_pool().await?;
        let duration = std::time::Duration::from_secs(timeout_secs);
        let started = Instant::now();

        let agent_result = tokio::time::timeout(duration, agent_fn(pool)).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;

        match agent_result {
            Ok(Ok(agent_output)) => {
                let outcome = SimulationOutcome {
                    agent_output,
                    ops_executed: 1,
                    duration_ms: elapsed_ms,
                };
                self.env.status = SandboxStatus::Completed {
                    outcome: outcome.clone(),
                };
                Ok(outcome)
            }
            Ok(Err(error)) => {
                self.env.status = SandboxStatus::Failed(error.to_string());
                Err(error)
            }
            Err(_elapsed) => {
                let msg = format!("sandbox timed out after {timeout_secs}s");
                self.env.status = SandboxStatus::Failed(msg.clone());
                Err(BranchError::SandboxError(msg))
            }
        }
    }

    /// Consumes the runner, returning the wrapped environment.
    pub fn into_env(self) -> SimulationEnvironment {
        self.env
    }
}
