//! Merge strategies for resolving conflicts automatically or escalating them.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::EntityType;

/// Selects how merge conflicts should be resolved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MergeStrategy {
    /// Prefer our branch values for all conflicting fields.
    Ours,
    /// Prefer incoming (theirs) branch values for all conflicting fields.
    Theirs,
    /// Union compatible fields when possible; escalate unresolvable conflicts.
    Union,
    /// Apply a per-field strategy: keys are field names, values are strategies for that field.
    FieldLevel(HashMap<String, MergeStrategy>),
    /// Surface all conflicts for manual resolution.
    Manual,
}

/// Returns the default [`MergeStrategy`] for a given entity type.
///
/// - `MemoryRecord` → `FieldLevel` (field-by-field theirs wins unless overridden)
/// - `Session` → `Ours` (session data is local)
/// - `ToolOutput` → `Theirs` (tool outputs come from incoming branch)
pub fn strategy_for_entity_type(entity_type: &EntityType) -> MergeStrategy {
    match entity_type {
        EntityType::MemoryRecord => MergeStrategy::FieldLevel(HashMap::new()),
        EntityType::Session => MergeStrategy::Ours,
        EntityType::ToolOutput => MergeStrategy::Theirs,
    }
}
