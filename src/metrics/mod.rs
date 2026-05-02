//! Branch metrics subsystem: tracking, divergence scoring, and reporting.

/// Divergence scoring formulae.
pub mod divergence;
/// Workspace-level metrics reporting.
pub mod reporter;
/// Per-branch metrics tracking.
pub mod tracker;

pub use divergence::{compute_score, divergence_label, time_weighted_score};
pub use reporter::MetricsReporter;
pub use tracker::{MetricsTracker, OpKind};
