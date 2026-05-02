//! Divergence score computation for branch diffs.

use std::collections::HashSet;

use similar::TextDiff;

use crate::types::{DiffResult, DiffStats};

/// Computes divergence scores between branch diffs and individual field values.
pub struct DivergenceScorer;

impl DivergenceScorer {
    /// Computes a normalized divergence score in `[0.0, 1.0]`.
    ///
    /// Formula: `(added + removed + modified × 0.5) / max(total_entities_base, 1)`
    ///
    /// `total_entities_base` is the entity count on the base (reference) branch.
    /// A score of `0.0` means identical; `1.0` means maximally diverged.
    pub fn score(diff: &DiffResult, total_entities_base: u64) -> f64 {
        let numerator = diff.stats.added as f64
            + diff.stats.removed as f64
            + diff.stats.modified as f64 * 0.5;
        let denominator = total_entities_base.max(1) as f64;
        (numerator / denominator).clamp(0.0, 1.0)
    }

    /// Computes similarity between two [`serde_json::Value`]s in `[0.0, 1.0]`.
    ///
    /// - **Strings**: normalised Levenshtein ratio via `similar::TextDiff`
    /// - **Numbers**: `1 - |a - b| / max(|a|, |b|, 1)`
    /// - **Objects**: recursive field-level average
    /// - **Arrays**: Jaccard similarity on serialised element representations
    /// - **Null / Bool / mixed-type**: `1.0` if equal, `0.0` otherwise
    pub fn score_field_similarity(a: &serde_json::Value, b: &serde_json::Value) -> f64 {
        if a == b {
            return 1.0;
        }
        match (a, b) {
            (serde_json::Value::String(sa), serde_json::Value::String(sb)) => {
                string_similarity(sa, sb)
            }
            (serde_json::Value::Number(na), serde_json::Value::Number(nb)) => {
                let fa = na.as_f64().unwrap_or(0.0);
                let fb = nb.as_f64().unwrap_or(0.0);
                numeric_similarity(fa, fb)
            }
            (serde_json::Value::Object(oa), serde_json::Value::Object(ob)) => {
                object_similarity(oa, ob)
            }
            (serde_json::Value::Array(aa), serde_json::Value::Array(ab)) => {
                array_jaccard(aa, ab)
            }
            _ => 0.0, // type mismatch or null
        }
    }
}

/// Legacy free-function for backward compat.
///
/// Computes a basic divergence fraction from raw diff stats.
pub fn score_divergence(stats: &DiffStats) -> f64 {
    if stats.total_entities == 0 {
        0.0
    } else {
        (stats.added + stats.removed + stats.modified) as f64 / stats.total_entities as f64
    }
}

fn string_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let diff = TextDiff::from_chars(a, b);
    f64::from(diff.ratio())
}

fn numeric_similarity(a: f64, b: f64) -> f64 {
    let max = a.abs().max(b.abs()).max(1.0);
    (1.0 - (a - b).abs() / max).clamp(0.0, 1.0)
}

fn object_similarity(
    a: &serde_json::Map<String, serde_json::Value>,
    b: &serde_json::Map<String, serde_json::Value>,
) -> f64 {
    let keys: HashSet<&String> = a.keys().chain(b.keys()).collect();
    if keys.is_empty() {
        return 1.0;
    }
    let total: f64 = keys
        .iter()
        .map(|k| {
            let av = a.get(*k).unwrap_or(&serde_json::Value::Null);
            let bv = b.get(*k).unwrap_or(&serde_json::Value::Null);
            DivergenceScorer::score_field_similarity(av, bv)
        })
        .sum();
    total / keys.len() as f64
}

fn array_jaccard(a: &[serde_json::Value], b: &[serde_json::Value]) -> f64 {
    let a_set: HashSet<String> = a.iter().map(|v| v.to_string()).collect();
    let b_set: HashSet<String> = b.iter().map(|v| v.to_string()).collect();
    let intersection = a_set.intersection(&b_set).count();
    let union = a_set.union(&b_set).count();
    if union == 0 {
        1.0
    } else {
        intersection as f64 / union as f64
    }
}
