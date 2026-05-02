//! Selective entity commit operations between branches.

use std::{sync::Arc, time::Instant};

use chrono::Utc;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    Arguments, SqlitePool,
};
use uuid::Uuid;

use crate::{
    branch::store::BranchStore,
    commit::{
        cherry::{CherryPick, EntitySelection},
        validator::CommitValidator,
    },
    diff::extractor::fetch_all_entities,
    error::{BranchError, BranchResult},
    types::{CommitLogEntry, CommitResult, EntityType},
};

/// Applies cherry-picked entity changes from a source branch into a target branch.
///
/// # Example
/// ```rust,ignore
/// let committer = SelectiveCommit::new(source_pool, target_pool, store);
/// let result = committer.commit(&cherry).await?;
/// ```
pub struct SelectiveCommit {
    /// The pool connected to the source branch database.
    pub source_pool: SqlitePool,
    /// The pool connected to the target branch database.
    pub target_pool: SqlitePool,
    /// Shared branch registry for commit log recording.
    pub store: Arc<BranchStore>,
}

impl SelectiveCommit {
    /// Creates a new selective commit executor with pre-opened pools.
    pub fn new(source_pool: SqlitePool, target_pool: SqlitePool, store: Arc<BranchStore>) -> Self {
        Self {
            source_pool,
            target_pool,
            store,
        }
    }

    /// Opens a `SelectiveCommit` from branch metadata (opens pools internally).
    pub async fn from_store(
        store: Arc<BranchStore>,
        source_id: Uuid,
        target_id: Uuid,
        _workspace_id: Uuid,
    ) -> BranchResult<Self> {
        let source = store.get(source_id).await?;
        let target = store.get(target_id).await?;
        let source_pool = open_pool(&source.db_path, true).await?;
        let target_pool = open_pool(&target.db_path, false).await?;
        Ok(Self {
            source_pool,
            target_pool,
            store,
        })
    }

    /// Applies a cherry-pick to the target branch within a single transaction.
    ///
    /// Steps:
    /// 1. Validate the selection via [`CommitValidator`]
    /// 2. Open a write transaction on target
    /// 3. Fetch and upsert selected entities (field-filtered if requested)
    /// 4. Record a commit log entry
    /// 5. Return [`CommitResult`]
    pub async fn commit(&self, cherry: &CherryPick) -> BranchResult<CommitResult> {
        let started = Instant::now();

        // Load source and target branches for validation.
        let source = self.store.get(cherry.source_branch_id).await?;
        let target = self.store.get(cherry.target_branch_id).await?;

        let validator = CommitValidator::new(self.source_pool.clone());
        let report = validator.validate(cherry, &source, &target).await?;
        if !report.ok {
            return Err(BranchError::CommitValidationFailed {
                branch_id: cherry.target_branch_id,
                violations: report.violations,
            });
        }

        let mut tx = self.target_pool.begin().await?;
        let mut committed_entity_count = 0u32;
        let mut fields_updated = 0u32;
        let mut all_entity_ids: Vec<String> = Vec::new();

        for sel in &cherry.entity_selections {
            let source_map = fetch_all_entities(&self.source_pool, &sel.entity_type).await?;

            let ids_to_process: Vec<&String> = if sel.entity_ids.is_empty() {
                source_map.keys().collect()
            } else {
                sel.entity_ids.iter().collect()
            };

            for entity_id in ids_to_process {
                let source_val = match source_map.get(entity_id) {
                    Some(v) => v.clone(),
                    None => continue,
                };

                let final_val = if let Some(fields) = &sel.fields {
                    // Merge only specified fields into existing target entity.
                    let target_map =
                        fetch_all_entities(&self.target_pool, &sel.entity_type).await?;
                    let mut merged = target_map
                        .get(entity_id)
                        .cloned()
                        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                    if let (Some(merged_obj), Some(source_obj)) =
                        (merged.as_object_mut(), source_val.as_object())
                    {
                        for f in fields {
                            if let Some(v) = source_obj.get(f) {
                                merged_obj.insert(f.clone(), v.clone());
                                fields_updated += 1;
                            }
                        }
                    }
                    merged
                } else {
                    let field_count = source_val.as_object().map(|o| o.len()).unwrap_or(0);
                    fields_updated += field_count as u32;
                    source_val
                };

                upsert_entity_tx(&mut tx, sel.entity_type.table_name(), entity_id, &final_val)
                    .await?;
                committed_entity_count += 1;
                all_entity_ids.push(entity_id.clone());
            }
        }

        tx.commit().await?;

        // Record commit log entry.
        let entry = CommitLogEntry {
            id: Uuid::new_v4(),
            branch_id: cherry.target_branch_id,
            entity_type: cherry
                .entity_selections
                .first()
                .map(|s| s.entity_type.clone()),
            entity_ids: all_entity_ids,
            op_kind: "cherry_pick".to_string(),
            committed_at: Utc::now(),
            message: cherry.message.clone(),
        };
        self.store.insert_commit_log(&entry).await?;

        Ok(CommitResult {
            committed_entity_count,
            fields_updated,
            duration_ms: started.elapsed().as_millis() as u64,
            target_branch_id: cherry.target_branch_id,
            committed_at: entry.committed_at,
        })
    }

    /// Promotes all divergent entities from `source_branch_id` into `target_branch_id`.
    ///
    /// This is a full merge-commit that uses all entity types.
    pub async fn commit_all(
        &self,
        source_branch_id: Uuid,
        target_branch_id: Uuid,
    ) -> BranchResult<CommitResult> {
        let cherry = CherryPick {
            source_branch_id,
            target_branch_id,
            entity_selections: vec![
                EntitySelection {
                    entity_type: EntityType::MemoryRecord,
                    entity_ids: Vec::new(),
                    fields: None,
                },
                EntitySelection {
                    entity_type: EntityType::Session,
                    entity_ids: Vec::new(),
                    fields: None,
                },
                EntitySelection {
                    entity_type: EntityType::ToolOutput,
                    entity_ids: Vec::new(),
                    fields: None,
                },
            ],
            message: Some("commit_all".to_string()),
        };
        self.commit(&cherry).await
    }
}

// ── SQL helpers ──────────────────────────────────────────────────────────────

async fn upsert_entity_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    entity_id: &str,
    value: &serde_json::Value,
) -> BranchResult<()> {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return Ok(()),
    };

    let mut columns: Vec<String> = vec!["id".to_string()];
    let mut values: Vec<Option<String>> = vec![Some(entity_id.to_string())];

    for (k, v) in obj {
        if k != "id" {
            columns.push(k.clone());
            values.push(json_to_str(v));
        }
    }

    let col_list = columns.join(", ");
    let placeholders = columns.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let sql = format!("INSERT OR REPLACE INTO {table} ({col_list}) VALUES ({placeholders})");

    let mut args = sqlx::sqlite::SqliteArguments::default();
    for v in &values {
        args.add(v.clone())
            .map_err(|error| BranchError::InvalidConfig(format!("invalid sqlite arg: {error}")))?;
    }
    sqlx::query_with(&sql, args).execute(&mut **tx).await?;
    Ok(())
}

fn json_to_str(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(b) => Some(if *b { "1" } else { "0" }.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => Some(v.to_string()),
    }
}

async fn open_pool(path: &std::path::Path, read_only: bool) -> BranchResult<SqlitePool> {
    SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(false)
                .read_only(read_only)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await
        .map_err(BranchError::Database)
}
