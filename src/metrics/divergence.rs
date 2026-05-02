//! Divergence scoring formulae and labels for branch metrics.

use crate::types::DiffResult;

/// Computes a divergence score in `[0.0, 1.0]` from a diff result.
///
/// Formula: `(added + removed + modified * 0.5) / max(base_entity_count, 1)`, clamped to 1.0.
pub fn compute_score(diff: &DiffResult, base_entity_count: u64) -> f64 {
    let numerator =
        diff.stats.added as f64 + diff.stats.removed as f64 + diff.stats.modified as f64 * 0.5;
    let denominator = (base_entity_count.max(1)) as f64;
    (numerator / denominator).min(1.0)
}

/// Maps a divergence score to a human-readable label.
///
/// | Score range | Label         |
/// |-------------|---------------|
/// | 0.0         | `"identical"` |
/// | ≤ 0.05      | `"minimal"`   |
/// | ≤ 0.25      | `"moderate"`  |
/// | ≤ 0.75      | `"significant"` |
/// | > 0.75      | `"diverged"`  |
pub fn divergence_label(score: f64) -> &'static str {
    if score == 0.0 {
        "identical"
    } else if score <= 0.05 {
        "minimal"
    } else if score <= 0.25 {
        "moderate"
    } else if score <= 0.75 {
        "significant"
    } else {
        "diverged"
    }
}

/// Applies exponential time-decay to a divergence score.
///
/// Formula: `score * exp(-age_days * decay_factor)`
///
/// Useful for deprioritizing stale branches that haven't diverged recently.
pub fn time_weighted_score(score: f64, age_days: f64, decay_factor: f64) -> f64 {
    score * (-age_days * decay_factor).exp()
}
