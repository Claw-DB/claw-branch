//! Simulation sandbox modules for isolated agent evaluation.

/// Sandbox cleanup helpers.
pub mod cleanup;
/// Isolated branch-backed simulation environments.
pub mod environment;
/// Sandbox result evaluators.
pub mod evaluator;
/// Scenario runner implementations.
pub mod runner;

pub use environment::{SandboxStatus, SimulationEnvironment, SimulationScenario};
pub use evaluator::{EvaluationReport, Recommendation, SandboxEvaluator};
pub use runner::SandboxRunner;
