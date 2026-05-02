//! Branch metrics subsystem: tracking, divergence scoring, and reporting.

/// Divergence scoring formulae.
pub mod divergence;
/// Per-branch metrics tracking.
pub mod tracker;
/// Workspace-level metrics reporting.
pub mod reporter;

pub use divergence::{compute_score, divergence_label, time_weighted_score};
pub use tracker::{MetricsTracker, OpKind};
pub use reporter::MetricsReporter;