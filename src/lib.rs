//! # claw-branch
//!
//! A Rust library for isolated, SQLite-backed agent branch management.
//!
//! `claw-branch` gives AI agents a structured way to diverge from a shared knowledge
//! baseline, explore changes in isolation, diff and merge back, and evaluate outcomes
//! in a fully sandboxed environment — all backed by SQLite and enforced through a
//! strongly-typed async API.
//!
//! ## Core concepts
//!
//! | Concept | Description |
//! |---------|-------------|
//! | **Branch** | An isolated SQLite database snapshot with its own lifecycle state |
//! | **Trunk** | The canonical baseline branch; all other branches fork from it |
//! | **Fork** | Creates a copy-on-write snapshot of a parent branch |
//! | **Diff** | Entity-level comparison between two branches |
//! | **Merge** | Three-way merge with configurable conflict resolution strategy |
//! | **Commit** | Selective cherry-pick of entities from one branch to another |
//! | **Sandbox** | Temporary evaluation environment for speculative agent work |
//! | **DAG** | Directed acyclic graph tracking branch lineage |
//!
//! ## Quick-start
//!
//! ```rust,ignore
//! use claw_branch::prelude::*;
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = BranchConfig::builder()
//!         .workspace_dir(PathBuf::from("/tmp/demo"))
//!         .build()?;
//!
//!     let engine = BranchEngine::new(config, std::path::Path::new("/data/source.db")).await?;
//!
//!     // Fork a feature branch from trunk
//!     let feature = engine.fork_trunk("feature/summariser").await?;
//!
//!     // Diff against trunk
//!     let trunk = engine.trunk().await?;
//!     let diff  = engine.diff(trunk.id, feature.id).await?;
//!     println!("changed={}", diff.stats.modified);
//!
//!     // Simulate an agent workload and get a recommendation
//!     let report = engine.simulate(feature.id, SimulationScenario {
//!         name: "trial".into(),
//!         description: "test run".into(),
//!         max_ops: Some(100),
//!         timeout_secs: Some(30),
//!         seed_data: None,
//!     }, |pool| async move {
//!         // agent writes to pool…
//!         Ok(serde_json::json!({"status": "done"}))
//!     }).await?;
//!
//!     match report.recommendation {
//!         Recommendation::Commit => { engine.commit_to_trunk(feature.id).await?; }
//!         Recommendation::Discard => { engine.discard(feature.id).await?; }
//!         Recommendation::NeedsReview(notes) => println!("Review: {notes:?}"),
//!     }
//!
//!     Ok(())
//! }
//! ```

#![deny(missing_docs)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]

/// Branch management primitives.
pub mod branch;
/// Selective commit and history helpers.
pub mod commit;
/// Shared configuration for branch operations.
pub mod config;
/// DAG lineage graph support.
pub mod dag;
/// Diff extraction and reporting.
pub mod diff;
/// Top-level engine orchestration.
pub mod engine;
/// Error types used across the crate.
pub mod error;
/// Guard-aware wrapper around the branch engine.
#[cfg(feature = "guarded")]
pub mod guarded;
/// Merge algorithms and conflict handling.
pub mod merge;
/// Metrics collection and reporting.
pub mod metrics;
/// Simulation sandbox support.
pub mod sandbox;
/// Snapshot creation, verification, and garbage collection.
pub mod snapshot;
/// Shared domain types.
pub mod types;

// ── Primary re-exports ────────────────────────────────────────────────────────

pub use config::{BranchConfig, BranchConfigBuilder};
pub use engine::{BranchConfigError, BranchEngine, BranchEngineBuilder};
pub use error::{BranchError, BranchResult};
#[cfg(feature = "guarded")]
pub use guarded::GuardedBranchEngine;

// types
pub use types::{
    Branch, BranchMetrics, BranchStatus, CommitLogEntry, CommitResult, DiffKind, DiffResult,
    DiffStats, EntityDiff, EntityType, FieldDiff, MergeConflict, MergeResult, SimulationOutcome,
    WorkspaceReport,
};

// diff
pub use diff::formatter::DiffSummary;

// merge
pub use merge::{resolver::ResolvedValue, strategies::MergeStrategy, three_way::MergePreview};

// commit
pub use commit::{
    cherry::{CherryPick, EntitySelection},
    selective::SelectiveCommit,
    validator::{CommitValidator, ValidationReport},
};

// sandbox
pub use sandbox::{
    environment::{SandboxStatus, SimulationEnvironment, SimulationScenario},
    evaluator::{EvaluationReport, Recommendation, SandboxEvaluator},
    runner::SandboxRunner,
};

// metrics
pub use metrics::{
    divergence::{compute_score, divergence_label, time_weighted_score},
    reporter::MetricsReporter,
    tracker::{MetricsTracker, OpKind},
};

// snapshot GC
pub use snapshot::gc::GcReport;

// ── Prelude ───────────────────────────────────────────────────────────────────

/// Convenience re-exports for typical library consumers.
///
/// ```rust,ignore
/// use claw_branch::prelude::*;
/// ```
pub mod prelude {
    pub use crate::{
        Branch, BranchConfig, BranchEngine, BranchError, BranchResult, BranchStatus, CherryPick,
        EvaluationReport, MergeStrategy, Recommendation, SimulationScenario, WorkspaceReport,
    };
}
