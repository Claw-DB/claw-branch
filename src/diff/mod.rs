//! Diff extraction and reporting modules.

/// Entity-level diff extraction.
pub mod extractor;
/// Diff formatting helpers.
pub mod formatter;
/// Divergence scoring.
pub mod scorer;
/// Diff type aliases and re-exports.
pub mod types;

pub use extractor::DiffExtractor;
pub use formatter::{DiffSummary, format_diff_human, format_diff_json, format_field_diff, summarise_diff};
pub use scorer::DivergenceScorer;