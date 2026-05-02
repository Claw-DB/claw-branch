//! Branch model aliases and small helpers.

use serde_json::Value;

pub use crate::types::{Branch, BranchMetrics, BranchStatus};

/// Structured metadata stored alongside a branch record.
pub type BranchMetadata = Value;