//! Automatic and manual conflict resolution helpers.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    error::BranchResult,
    merge::{conflict::is_trivially_resolvable, strategies::MergeStrategy},
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
        Self::merge_json_union_with_timestamp_context(ours, theirs, None, None)
    }

    fn merge_json_union_with_timestamp_context(
        ours: &Value,
        theirs: &Value,
        ours_parent_timestamp: Option<i64>,
        theirs_parent_timestamp: Option<i64>,
    ) -> Value {
        match (ours, theirs) {
            (Value::Object(ours_obj), Value::Object(theirs_obj)) => {
                let ours_timestamp = Self::extract_timestamp(ours_obj).or(ours_parent_timestamp);
                let theirs_timestamp =
                    Self::extract_timestamp(theirs_obj).or(theirs_parent_timestamp);
                let mut merged = Map::new();

                for (key, ours_value) in ours_obj {
                    if let Some(theirs_value) = theirs_obj.get(key) {
                        merged.insert(
                            key.clone(),
                            Self::merge_json_union_with_timestamp_context(
                                ours_value,
                                theirs_value,
                                ours_timestamp,
                                theirs_timestamp,
                            ),
                        );
                    } else {
                        merged.insert(key.clone(), ours_value.clone());
                    }
                }

                for (key, theirs_value) in theirs_obj {
                    if !merged.contains_key(key) {
                        merged.insert(key.clone(), theirs_value.clone());
                    }
                }

                Value::Object(merged)
            }
            (Value::Array(ours_array), Value::Array(theirs_array)) => {
                Self::merge_arrays_by_identity(
                    ours_array,
                    theirs_array,
                    ours_parent_timestamp,
                    theirs_parent_timestamp,
                )
            }
            (Value::Null, value) => value.clone(),
            (value, Value::Null) => value.clone(),
            (ours_scalar, theirs_scalar) => {
                let ours_timestamp = ours_parent_timestamp.unwrap_or_default();
                let theirs_timestamp = theirs_parent_timestamp.unwrap_or_default();
                if theirs_timestamp >= ours_timestamp {
                    theirs_scalar.clone()
                } else {
                    ours_scalar.clone()
                }
            }
        }
    }

    fn merge_arrays_by_identity(
        ours_array: &[Value],
        theirs_array: &[Value],
        ours_parent_timestamp: Option<i64>,
        theirs_parent_timestamp: Option<i64>,
    ) -> Value {
        let mut merged = Vec::new();
        let mut consumed_theirs = vec![false; theirs_array.len()];

        for ours_item in ours_array {
            if let Some((index, theirs_item)) = theirs_array
                .iter()
                .enumerate()
                .find(|(_, candidate)| Self::same_identity(ours_item, candidate))
            {
                consumed_theirs[index] = true;
                merged.push(Self::merge_json_union_with_timestamp_context(
                    ours_item,
                    theirs_item,
                    ours_parent_timestamp,
                    theirs_parent_timestamp,
                ));
            } else {
                merged.push(ours_item.clone());
            }
        }

        for (index, theirs_item) in theirs_array.iter().enumerate() {
            if !consumed_theirs[index] {
                merged.push(theirs_item.clone());
            }
        }

        Value::Array(merged)
    }

    fn same_identity(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Object(left_obj), Value::Object(right_obj)) => {
                match (left_obj.get("id"), right_obj.get("id")) {
                    (Some(left_id), Some(right_id)) => left_id == right_id,
                    _ => left == right,
                }
            }
            _ => left == right,
        }
    }

    fn extract_timestamp(object: &Map<String, Value>) -> Option<i64> {
        let value = object.get("updated_at")?;
        match value {
            Value::Number(number) => number.as_i64(),
            Value::String(timestamp) => timestamp.parse::<i64>().ok(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::ConflictResolver;

    #[test]
    fn union_merges_nested_objects_recursively() {
        let ours = json!({
            "id": "1",
            "updated_at": 10,
            "profile": {
                "updated_at": 10,
                "name": "alice",
                "prefs": { "theme": "light" }
            }
        });
        let theirs = json!({
            "id": "1",
            "updated_at": 11,
            "profile": {
                "updated_at": 11,
                "name": "alice b",
                "prefs": { "lang": "en" }
            }
        });

        let merged = ConflictResolver::merge_json_union(&ours, &theirs);
        assert_eq!(merged["profile"]["name"], json!("alice b"));
        assert_eq!(merged["profile"]["prefs"]["theme"], json!("light"));
        assert_eq!(merged["profile"]["prefs"]["lang"], json!("en"));
    }

    #[test]
    fn union_uses_timestamp_for_scalar_conflict() {
        let ours = json!({
            "updated_at": 30,
            "score": 1
        });
        let theirs = json!({
            "updated_at": 20,
            "score": 2
        });

        let merged = ConflictResolver::merge_json_union(&ours, &theirs);
        assert_eq!(merged["score"], json!(1));
    }

    #[test]
    fn union_merges_arrays_by_entity_identity() {
        let ours = json!([
            { "id": "a", "updated_at": 10, "value": 1 },
            { "id": "b", "updated_at": 10, "value": 2 }
        ]);
        let theirs = json!([
            { "id": "b", "updated_at": 12, "value": 3 },
            { "id": "c", "updated_at": 11, "value": 4 }
        ]);

        let merged = ConflictResolver::merge_json_union(&ours, &theirs);
        assert!(merged.as_array().is_some());
        let Some(merged_array) = merged.as_array() else {
            return;
        };
        assert_eq!(merged_array.len(), 3);
        assert!(merged_array.iter().any(|value| value["id"] == "a"));
        assert!(merged_array
            .iter()
            .any(|value| value["id"] == "b" && value["value"] == 3));
        assert!(merged_array.iter().any(|value| value["id"] == "c"));
    }
}
