//! Automatic and manual conflict resolution helpers.

use serde::{Deserialize, Serialize};

use crate::{
    error::BranchResult,
    merge::{
        conflict::is_trivially_resolvable,
        strategies::MergeStrategy,
    },
    types::MergeConflict,
};

/// Represents the resolved outcome for a single merge conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResolvedValue {
    /// Keep our current value unchanged.
    UseOurs,
    /// Accept the incoming (theirs) value.
    UseTheirs,
    /// Apply a computed merged value (e.g. from union logic).
    Merged(serde_json::Value),
    /// Cannot be automatically resolved; must be escalated to a human.
    Escalate,
}

/// Resolves merge conflicts according to the active [`MergeStrategy`].
pub struct ConflictResolver;

impl ConflictResolver {
    /// Resolves a single conflict according to `strategy`.
    pub fn resolve(
        &self,
        conflict: &MergeConflict,
        strategy: &MergeStrategy,
    ) -> BranchResult<ResolvedValue> {
        // Trivially resolvable: both sides agree on the final value.
        if is_trivially_resolvable(conflict) {
            return Ok(ResolvedValue::Merged(conflict.ours_value.clone()));
        }

        let resolved = match strategy {
            MergeStrategy::Ours => ResolvedValue::UseOurs,
            MergeStrategy::Theirs => ResolvedValue::UseTheirs,
            MergeStrategy::Union => {
                let merged = Self::merge_json_union(&conflict.ours_value, &conflict.theirs_value);
                ResolvedValue::Merged(merged)
            }
            MergeStrategy::FieldLevel(field_map) => {
                // Use field-level strategy when available; fall back to Union.
                let field = conflict.conflicting_fields.first();
                let field_strategy = field
                    .and_then(|f| field_map.get(f))
                    .unwrap_or(&MergeStrategy::Union);
                return self.resolve(
                    &MergeConflict {
                        conflicting_fields: conflict.conflicting_fields.clone(),
                        entity_id: conflict.entity_id.clone(),
                        entity_type: conflict.entity_type.clone(),
                        base_value: conflict.base_value.clone(),
                        ours_value: conflict.ours_value.clone(),
                        theirs_value: conflict.theirs_value.clone(),
                    },
                    field_strategy,
                );
            }
            MergeStrategy::Manual => ResolvedValue::Escalate,
        };

        Ok(resolved)
    }

    /// Resolves a batch of conflicts, returning each paired with its resolution.
    pub fn resolve_batch(
        &self,
        conflicts: Vec<MergeConflict>,
        strategy: &MergeStrategy,
    ) -> BranchResult<Vec<(MergeConflict, ResolvedValue)>> {
        conflicts
            .into_iter()
            .map(|c| {
                let r = self.resolve(&c, strategy)?;
                Ok((c, r))
            })
            .collect()
    }

    /// Merges two JSON values using a union strategy:
    ///
    /// - **Arrays**: deduplicated union of both element sets
    /// - **Objects**: field-level merge where `theirs` wins on conflict
    /// - **Scalars / Null**: `theirs` wins
    pub fn merge_json_union(
        ours: &serde_json::Value,
        theirs: &serde_json::Value,
    ) -> serde_json::Value {
        match (ours, theirs) {
            (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
                let mut seen: Vec<String> = Vec::new();
                let mut result = Vec::new();
                for v in a.iter().chain(b.iter()) {
                    let key = v.to_string();
                    if !seen.contains(&key) {
                        seen.push(key);
                        result.push(v.clone());
                    }
                }
                serde_json::Value::Array(result)
            }
            (serde_json::Value::Object(oa), serde_json::Value::Object(ob)) => {
                let mut merged = oa.clone();
                for (k, v) in ob {
                    merged.insert(k.clone(), v.clone()); // theirs wins
                }
                serde_json::Value::Object(merged)
            }
            _ => theirs.clone(), // scalars: theirs wins
        }
    }
}
