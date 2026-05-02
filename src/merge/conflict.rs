//! Merge conflict detection and trivial-resolution helpers.

use crate::types::{EntityDiff, MergeConflict};

pub use crate::types::MergeConflict as Conflict;

/// Detects field-level conflicts between `ours_diff` and `theirs_diff` relative to `base_entity`.
///
/// A conflict is raised when *both* sides changed the *same field* to *different values*.
pub fn detect_conflicts(
    ours_diff: &EntityDiff,
    theirs_diff: &EntityDiff,
    base_entity: &serde_json::Value,
) -> Vec<MergeConflict> {
    if ours_diff.entity_id != theirs_diff.entity_id {
        return Vec::new();
    }

    let entity_id = ours_diff.entity_id.clone();
    let entity_type = ours_diff.entity_type.clone();

    // Build maps of field → after-value for each side.
    let ours_changes: std::collections::HashMap<&str, &serde_json::Value> = ours_diff
        .field_diffs
        .iter()
        .map(|fd| (fd.field.as_str(), &fd.after))
        .collect();
    let theirs_changes: std::collections::HashMap<&str, &serde_json::Value> = theirs_diff
        .field_diffs
        .iter()
        .map(|fd| (fd.field.as_str(), &fd.after))
        .collect();

    let mut conflicts = Vec::new();

    for (field, ours_after) in &ours_changes {
        if let Some(theirs_after) = theirs_changes.get(field) {
            if ours_after != theirs_after {
                // Both sides changed this field to different values → conflict.
                let base_value = base_entity
                    .get(*field)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                conflicts.push(MergeConflict {
                    entity_id: entity_id.clone(),
                    entity_type: entity_type.clone(),
                    base_value: base_value.clone(),
                    ours_value: (*ours_after).clone(),
                    theirs_value: (*theirs_after).clone(),
                    conflicting_fields: vec![field.to_string()],
                });
            }
        }
    }

    conflicts
}

/// Returns `true` when a conflict is trivially resolvable because both sides
/// chose the same value (i.e. they changed from the base to the identical target).
pub fn is_trivially_resolvable(conflict: &MergeConflict) -> bool {
    conflict.ours_value == conflict.theirs_value
}
