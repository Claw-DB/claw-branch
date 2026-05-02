//! Human-readable and JSON diff formatting utilities.

use std::collections::HashMap;

use crate::{
    error::BranchResult,
    types::{DiffKind, DiffResult, DiffStats, FieldDiff},
};

/// High-level summary of a diff run, aggregated across entity types.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiffSummary {
    /// Number of entities added in branch B.
    pub added: u32,
    /// Number of entities removed from branch A.
    pub removed: u32,
    /// Number of entities modified between branches.
    pub modified: u32,
    /// Number of entities identical across both branches.
    pub unchanged: u32,
    /// Aggregate divergence score in `[0.0, 1.0]`.
    pub divergence_score: f64,
    /// True when all entities are identical.
    pub is_identical: bool,
    /// Per-entity-type breakdown of diff stats.
    pub entity_type_breakdown: HashMap<String, DiffStats>,
}

/// Formats a diff result as a human-readable multi-line string with `[+]`, `[-]`, and `[~]` tags.
pub fn format_diff_human(diff: &DiffResult) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "diff {} → {}\n",
        diff.branch_a_id, diff.branch_b_id
    ));
    out.push_str(&format!(
        "  compared_at : {}\n",
        diff.compared_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    out.push_str(&format!("  divergence  : {:.4}\n", diff.divergence_score));
    out.push_str(&format!(
        "  stats       : +{} -{}  ~{}  ={}\n\n",
        diff.stats.added, diff.stats.removed, diff.stats.modified, diff.stats.unchanged
    ));

    for ed in &diff.entity_diffs {
        if matches!(ed.diff_kind, DiffKind::Unchanged) {
            continue;
        }
        let tag = match ed.diff_kind {
            DiffKind::Added => "[+]",
            DiffKind::Removed => "[-]",
            DiffKind::Modified => "[~]",
            DiffKind::Unchanged => "[=]",
        };
        out.push_str(&format!("{tag} {} ({:?})\n", ed.entity_id, ed.entity_type));
        for fd in &ed.field_diffs {
            out.push_str(&format!("    {}\n", format_field_diff(fd)));
        }
    }
    out
}

/// Serializes a diff result as a structured JSON value.
pub fn format_diff_json(diff: &DiffResult) -> BranchResult<serde_json::Value> {
    Ok(serde_json::to_value(diff)?)
}

/// Formats a single field-level diff as `field: before → after`.
pub fn format_field_diff(fd: &FieldDiff) -> String {
    format!("{}: {} → {}", fd.field, fd.before, fd.after)
}

/// Computes an aggregate [`DiffSummary`] from a diff result.
pub fn summarise_diff(diff: &DiffResult) -> DiffSummary {
    let mut breakdown: HashMap<String, DiffStats> = HashMap::new();

    for ed in &diff.entity_diffs {
        let key = format!("{:?}", ed.entity_type);
        let entry = breakdown.entry(key).or_default();
        entry.total_entities += 1;
        match ed.diff_kind {
            DiffKind::Added => entry.added += 1,
            DiffKind::Removed => entry.removed += 1,
            DiffKind::Modified => entry.modified += 1,
            DiffKind::Unchanged => entry.unchanged += 1,
        }
    }

    let is_identical = diff.stats.added == 0 && diff.stats.removed == 0 && diff.stats.modified == 0;

    DiffSummary {
        added: diff.stats.added,
        removed: diff.stats.removed,
        modified: diff.stats.modified,
        unchanged: diff.stats.unchanged,
        divergence_score: diff.divergence_score,
        is_identical,
        entity_type_breakdown: breakdown,
    }
}

/// Formats a diff result as a compact text summary (legacy helper).
pub fn format_text(diff: &DiffResult) -> String {
    format!(
        "diff {} → {}: {} entities, divergence {:.3}",
        diff.branch_a_id, diff.branch_b_id, diff.stats.total_entities, diff.divergence_score
    )
}
