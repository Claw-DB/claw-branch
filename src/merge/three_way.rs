//! Three-way merge orchestration over branch diffs.

use std::sync::Arc;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use crate::{
    config::BranchConfig,
    diff::extractor::DiffExtractor,
    error::BranchResult,
    merge::{
        applier::MergeApplier,
        conflict::detect_conflicts,
        resolver::{ConflictResolver, ResolvedValue},
        strategies::MergeStrategy,
    },
    types::{Branch, DiffKind, EntityDiff, EntityType, MergeConflict, MergeResult},
};

/// Estimation constant for apply_batch throughput (milliseconds per entity change).
const MS_PER_ENTITY: u64 = 2;

/// A preview of what a three-way merge would do without actually applying changes.
#[derive(Debug, Clone)]
pub struct MergePreview {
    /// Number of entities that can be automatically resolved.
    pub auto_resolved: u32,
    /// Conflicts that require manual intervention.
    pub conflicts: Vec<MergeConflict>,
    /// Rough estimate of how long the real merge would take.
    pub estimated_duration_ms: u64,
}

/// Orchestrates a full three-way merge between a base, ours, and theirs branch.
pub struct ThreeWayMerger {
    resolver: Arc<ConflictResolver>,
    config: Arc<BranchConfig>,
}

impl ThreeWayMerger {
    /// Creates a new merger with the given resolver and workspace config.
    pub fn new(resolver: Arc<ConflictResolver>, config: Arc<BranchConfig>) -> Self {
        Self { resolver, config }
    }

    /// Performs a three-way merge of `theirs` into `ours` using `base` as the common ancestor.
    ///
    /// Steps:
    /// 1. Diff `base → ours` and `base → theirs`.
    /// 2. For each entity changed in theirs: auto-merge if disjoint from ours, else detect conflicts.
    /// 3. Resolve conflicts via `strategy`.
    /// 4. Apply resolved changes to `ours` database.
    pub async fn merge(
        &self,
        base: &Branch,
        ours: &Branch,
        theirs: &Branch,
        strategy: &MergeStrategy,
        entity_types: Option<&[EntityType]>,
    ) -> BranchResult<MergeResult> {
        let start = std::time::Instant::now();
        let extractor = DiffExtractor::new(Arc::clone(&self.config));

        let ours_diff = extractor.diff(base, ours, entity_types).await?;
        let theirs_diff = extractor.diff(base, theirs, entity_types).await?;

        // Index ours changes by entity_id (only non-unchanged diffs).
        let ours_by_id: std::collections::HashMap<String, &EntityDiff> = ours_diff
            .entity_diffs
            .iter()
            .filter(|d| !matches!(d.diff_kind, DiffKind::Unchanged))
            .map(|d| (d.entity_id.clone(), d))
            .collect();

        let mut auto_changes: Vec<(EntityDiff, ResolvedValue)> = Vec::new();
        let mut unresolved_conflicts: Vec<MergeConflict> = Vec::new();

        for theirs_entity in theirs_diff
            .entity_diffs
            .iter()
            .filter(|d| !matches!(d.diff_kind, DiffKind::Unchanged))
        {
            if let Some(ours_entity) = ours_by_id.get(&theirs_entity.entity_id) {
                // Both sides changed this entity — detect field conflicts.
                let base_val = serde_json::Value::Object(serde_json::Map::new());
                let field_conflicts = detect_conflicts(ours_entity, theirs_entity, &base_val);

                if field_conflicts.is_empty() {
                    // Disjoint fields: apply theirs on top of ours.
                    let merged = merge_field_diffs(ours_entity, theirs_entity);
                    auto_changes.push((merged, ResolvedValue::UseOurs));
                } else {
                    // Attempt to resolve per strategy.
                    let resolved = self.resolver.resolve_batch(field_conflicts, strategy)?;
                    for (conflict, resolution) in resolved {
                        match resolution {
                            ResolvedValue::Escalate => {
                                unresolved_conflicts.push(conflict);
                            }
                            other => {
                                auto_changes.push((theirs_entity.clone(), other));
                            }
                        }
                    }
                }
            } else {
                // Only theirs changed this entity: auto-apply.
                let resolved = theirs_entity_to_resolved(theirs_entity);
                auto_changes.push((theirs_entity.clone(), resolved));
            }
        }

        if !unresolved_conflicts.is_empty() {
            return Ok(MergeResult {
                source_branch_id: theirs.id,
                target_branch_id: ours.id,
                base_branch_id: base.id,
                applied: 0,
                skipped: 0,
                conflicts: unresolved_conflicts,
                duration_ms: start.elapsed().as_millis() as u64,
                success: false,
            });
        }

        // Apply changes to the ours database.
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&ours.db_path)
                    .create_if_missing(false)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await?;

        let applier = MergeApplier::new(pool);
        let applied = applier.apply_batch(auto_changes).await?;

        Ok(MergeResult {
            source_branch_id: theirs.id,
            target_branch_id: ours.id,
            base_branch_id: base.id,
            applied,
            skipped: 0,
            conflicts: Vec::new(),
            duration_ms: start.elapsed().as_millis() as u64,
            success: true,
        })
    }

    /// Returns a preview of what [`merge`](Self::merge) would do without committing any changes.
    pub async fn preview(
        &self,
        base: &Branch,
        ours: &Branch,
        theirs: &Branch,
        entity_types: Option<&[EntityType]>,
    ) -> BranchResult<MergePreview> {
        let extractor = DiffExtractor::new(Arc::clone(&self.config));
        let ours_diff = extractor.diff(base, ours, entity_types).await?;
        let theirs_diff = extractor.diff(base, theirs, entity_types).await?;

        let ours_by_id: std::collections::HashMap<String, &EntityDiff> = ours_diff
            .entity_diffs
            .iter()
            .filter(|d| !matches!(d.diff_kind, DiffKind::Unchanged))
            .map(|d| (d.entity_id.clone(), d))
            .collect();

        let mut auto_resolved = 0u32;
        let mut conflicts: Vec<MergeConflict> = Vec::new();

        for theirs_entity in theirs_diff
            .entity_diffs
            .iter()
            .filter(|d| !matches!(d.diff_kind, DiffKind::Unchanged))
        {
            if let Some(ours_entity) = ours_by_id.get(&theirs_entity.entity_id) {
                let base_val = serde_json::Value::Object(serde_json::Map::new());
                let mut field_conflicts = detect_conflicts(ours_entity, theirs_entity, &base_val);
                if field_conflicts.is_empty() {
                    auto_resolved += 1;
                } else {
                    conflicts.append(&mut field_conflicts);
                }
            } else {
                auto_resolved += 1;
            }
        }

        let estimated = (auto_resolved as u64 + conflicts.len() as u64) * MS_PER_ENTITY;
        Ok(MergePreview {
            auto_resolved,
            conflicts,
            estimated_duration_ms: estimated,
        })
    }
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Merges two EntityDiffs by combining their field changes (theirs wins on overlap).
fn merge_field_diffs(ours: &EntityDiff, theirs: &EntityDiff) -> EntityDiff {
    let mut combined = ours.field_diffs.clone();
    let ours_fields: std::collections::HashSet<&str> =
        ours.field_diffs.iter().map(|f| f.field.as_str()).collect();
    for fd in &theirs.field_diffs {
        if !ours_fields.contains(fd.field.as_str()) {
            combined.push(fd.clone());
        }
    }
    EntityDiff {
        entity_id: ours.entity_id.clone(),
        entity_type: ours.entity_type.clone(),
        diff_kind: DiffKind::Modified,
        field_diffs: combined,
    }
}

/// Converts an entity diff from theirs into a `ResolvedValue` ready to apply.
fn theirs_entity_to_resolved(diff: &EntityDiff) -> ResolvedValue {
    match diff.diff_kind {
        DiffKind::Removed => ResolvedValue::Merged(serde_json::Value::Null),
        _ => {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "id".to_string(),
                serde_json::Value::String(diff.entity_id.clone()),
            );
            for fd in &diff.field_diffs {
                obj.insert(fd.field.clone(), fd.after.clone());
            }
            ResolvedValue::Merged(serde_json::Value::Object(obj))
        }
    }
}
