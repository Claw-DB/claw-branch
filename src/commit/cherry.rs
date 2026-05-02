//! Cherry-pick selection models for selective branch promotion.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::EntityType;

/// Selects a subset of entities (and optionally fields) from a branch to promote.
///
/// # Example
/// ```rust,ignore
/// let pick = CherryPick::specific_entities(src, tgt, EntityType::MemoryRecord, vec!["id1".to_string()]);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CherryPick {
    /// The branch from which entities are sourced.
    pub source_branch_id: Uuid,
    /// The branch into which the entities are written.
    pub target_branch_id: Uuid,
    /// Ordered list of entity selections to process.
    pub entity_selections: Vec<EntitySelection>,
    /// Optional human-readable commit message.
    pub message: Option<String>,
}

/// Describes which entities (and which fields) to promote for one entity type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntitySelection {
    /// The entity table to read from.
    pub entity_type: EntityType,
    /// The entity identifiers to cherry-pick.  An empty vec means *all* entities of this type.
    pub entity_ids: Vec<String>,
    /// Restrict promotion to these fields only.  `None` means all fields.
    pub fields: Option<Vec<String>>,
}

impl CherryPick {
    /// Promotes every entity of `entity_type` from `source` to `target`.
    ///
    /// # Example
    /// ```rust,ignore
    /// let pick = CherryPick::all_of_type(src_id, tgt_id, EntityType::MemoryRecord);
    /// ```
    pub fn all_of_type(source: Uuid, target: Uuid, entity_type: EntityType) -> Self {
        Self {
            source_branch_id: source,
            target_branch_id: target,
            entity_selections: vec![EntitySelection {
                entity_type,
                entity_ids: Vec::new(),
                fields: None,
            }],
            message: None,
        }
    }

    /// Promotes specific entities of `entity_type` from `source` to `target`.
    ///
    /// # Example
    /// ```rust,ignore
    /// let pick = CherryPick::specific_entities(src_id, tgt_id, EntityType::MemoryRecord, ids);
    /// ```
    pub fn specific_entities(
        source: Uuid,
        target: Uuid,
        entity_type: EntityType,
        ids: Vec<String>,
    ) -> Self {
        Self {
            source_branch_id: source,
            target_branch_id: target,
            entity_selections: vec![EntitySelection {
                entity_type,
                entity_ids: ids,
                fields: None,
            }],
            message: None,
        }
    }

    /// Restricts the last added selection to only the specified field names.
    ///
    /// Useful for chaining with [`specific_entities`](Self::specific_entities).
    pub fn specific_fields(mut self, fields: Vec<String>) -> Self {
        if let Some(sel) = self.entity_selections.last_mut() {
            sel.fields = Some(fields);
        }
        self
    }

    /// Attaches a commit message to the cherry-pick.
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}
