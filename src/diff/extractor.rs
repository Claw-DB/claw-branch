//! Entity diff extraction between two branch databases.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use chrono::Utc;
use sqlx::{Row, sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteJournalMode}, SqlitePool};
use uuid::Uuid;

use crate::{
    config::BranchConfig,
    diff::scorer::score_divergence,
    error::{BranchError, BranchResult},
    types::{Branch, DiffKind, DiffResult, DiffStats, EntityDiff, EntityType, FieldDiff},
};

/// Opens branch databases and computes entity-level diffs across the ClawDB schema.
pub struct DiffExtractor {
    /// Configuration used for connection limits and timeouts.
    pub config: Arc<BranchConfig>,
}

impl DiffExtractor {
    /// Creates a new extractor with the given workspace config.
    pub fn new(config: Arc<BranchConfig>) -> Self {
        Self { config }
    }

    /// Computes the full diff between `branch_a` and `branch_b`.
    ///
    /// When `entity_types` is `None`, all three entity types are compared.
    pub async fn diff(
        &self,
        branch_a: &Branch,
        branch_b: &Branch,
        entity_types: Option<&[EntityType]>,
    ) -> BranchResult<DiffResult> {
        let pool_a = open_pool(&branch_a.db_path).await?;
        let pool_b = open_pool(&branch_b.db_path).await?;

        let types: &[EntityType] = entity_types.unwrap_or(&[
            EntityType::MemoryRecord,
            EntityType::Session,
            EntityType::ToolOutput,
        ]);

        let mut entity_diffs: Vec<EntityDiff> = Vec::new();
        let mut stats = DiffStats::default();

        for entity_type in types {
            let map_a = fetch_all_entities(&pool_a, entity_type).await?;
            let map_b = fetch_all_entities(&pool_b, entity_type).await?;

            let all_ids: HashSet<&String> = map_a.keys().chain(map_b.keys()).collect();
            stats.total_entities += all_ids.len() as u32;

            for id in all_ids {
                let ed = match (map_a.get(id), map_b.get(id)) {
                    (Some(_), None) => {
                        stats.removed += 1;
                        EntityDiff {
                            entity_id: id.clone(),
                            entity_type: entity_type.clone(),
                            diff_kind: DiffKind::Removed,
                            field_diffs: Vec::new(),
                        }
                    }
                    (None, Some(_)) => {
                        stats.added += 1;
                        EntityDiff {
                            entity_id: id.clone(),
                            entity_type: entity_type.clone(),
                            diff_kind: DiffKind::Added,
                            field_diffs: Vec::new(),
                        }
                    }
                    (Some(va), Some(vb)) => {
                        let ed = compare_entity_values(id, entity_type.clone(), va, vb);
                        match ed.diff_kind {
                            DiffKind::Modified => stats.modified += 1,
                            DiffKind::Unchanged => stats.unchanged += 1,
                            _ => {}
                        }
                        ed
                    }
                    (None, None) => unreachable!(),
                };
                entity_diffs.push(ed);
            }
        }

        let divergence_score = score_divergence(&stats);

        pool_a.close().await;
        pool_b.close().await;

        Ok(DiffResult {
            branch_a_id: branch_a.id,
            branch_b_id: branch_b.id,
            compared_at: Utc::now(),
            entity_diffs,
            stats,
            divergence_score,
        })
    }

    /// Diffs a single named entity between two already-opened pools.
    pub async fn diff_entity(
        &self,
        entity_id: &str,
        entity_type: &EntityType,
        pool_a: &SqlitePool,
        pool_b: &SqlitePool,
    ) -> BranchResult<EntityDiff> {
        let map_a = fetch_all_entities(pool_a, entity_type).await?;
        let map_b = fetch_all_entities(pool_b, entity_type).await?;
        Ok(match (map_a.get(entity_id), map_b.get(entity_id)) {
            (Some(_), None) => EntityDiff {
                entity_id: entity_id.to_string(),
                entity_type: entity_type.clone(),
                diff_kind: DiffKind::Removed,
                field_diffs: Vec::new(),
            },
            (None, Some(_)) => EntityDiff {
                entity_id: entity_id.to_string(),
                entity_type: entity_type.clone(),
                diff_kind: DiffKind::Added,
                field_diffs: Vec::new(),
            },
            (Some(va), Some(vb)) => {
                compare_entity_values(entity_id, entity_type.clone(), va, vb)
            }
            (None, None) => EntityDiff {
                entity_id: entity_id.to_string(),
                entity_type: entity_type.clone(),
                diff_kind: DiffKind::Unchanged,
                field_diffs: Vec::new(),
            },
        })
    }
}

/// Fetches all rows from an entity table as a `HashMap<entity_id, JSON object>`.
///
/// Uses `PRAGMA table_info` to discover column names, then builds each row into a
/// `serde_json::Value::Object` via SQLite's `json_object()` function.
pub async fn fetch_all_entities(
    pool: &SqlitePool,
    entity_type: &EntityType,
) -> BranchResult<HashMap<String, serde_json::Value>> {
    let table = entity_type.table_name();

    // Discover column names dynamically.
    let pragma_sql = format!("PRAGMA table_info({table})");
    let col_rows = sqlx::query(&pragma_sql).fetch_all(pool).await?;
    let columns: Vec<String> = col_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("name").ok())
        .collect();

    if columns.is_empty() {
        // Table does not exist in this snapshot — treat as empty.
        return Ok(HashMap::new());
    }

    // Build SELECT id, json_object('col', col, …) AS __data FROM <table>
    let json_args: String = columns
        .iter()
        .map(|c| format!("'{}', {}", c, c))
        .collect::<Vec<_>>()
        .join(", ");
    let query_sql = format!("SELECT id, json_object({json_args}) AS __data FROM {table}");

    let rows = sqlx::query(&query_sql).fetch_all(pool).await?;
    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: String = row.try_get("id")?;
        let data_str: String = row.try_get("__data")?;
        let data: serde_json::Value = serde_json::from_str(&data_str)?;
        map.insert(id, data);
    }
    Ok(map)
}

// ── Internal helpers ────────────────────────────────────────────────────────

async fn open_pool(path: &std::path::Path) -> BranchResult<SqlitePool> {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(false)
                .read_only(true)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await
        .map_err(BranchError::Database)
}

fn compare_entity_values(
    entity_id: &str,
    entity_type: EntityType,
    a: &serde_json::Value,
    b: &serde_json::Value,
) -> EntityDiff {
    let a_obj = a.as_object().cloned().unwrap_or_default();
    let b_obj = b.as_object().cloned().unwrap_or_default();

    let all_fields: HashSet<String> =
        a_obj.keys().chain(b_obj.keys()).cloned().collect();

    let mut field_diffs: Vec<FieldDiff> = Vec::new();
    for field in &all_fields {
        let av = a_obj.get(field).cloned().unwrap_or(serde_json::Value::Null);
        let bv = b_obj.get(field).cloned().unwrap_or(serde_json::Value::Null);
        if av != bv {
            field_diffs.push(FieldDiff {
                field: field.clone(),
                before: av,
                after: bv,
            });
        }
    }

    let diff_kind = if field_diffs.is_empty() {
        DiffKind::Unchanged
    } else {
        DiffKind::Modified
    };

    EntityDiff {
        entity_id: entity_id.to_string(),
        entity_type,
        diff_kind,
        field_diffs,
    }
}

// Keep the old free-function for backward compat.
/// Returns an empty diff placeholder.  Use [`DiffExtractor`] for real extraction.
pub async fn extract_diff(branch_a_id: Uuid, branch_b_id: Uuid) -> BranchResult<DiffResult> {
    let stats = DiffStats::default();
    Ok(DiffResult {
        branch_a_id,
        branch_b_id,
        compared_at: Utc::now(),
        entity_diffs: Vec::new(),
        divergence_score: score_divergence(&stats),
        stats,
    })
}
