//! Merge result application to a target branch database.

use sqlx::{sqlite::SqliteArguments, Arguments, SqlitePool};

use crate::{
    error::{BranchError, BranchResult},
    merge::resolver::ResolvedValue,
    types::{DiffKind, EntityDiff, EntityType},
};

/// Writes resolved entity changes into a target branch's SQLite database.
pub struct MergeApplier {
    pool: SqlitePool,
}

impl MergeApplier {
    /// Creates an applier backed by the given pool (must be the *target* / ours branch).
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Applies a single resolved change to the target database.
    ///
    /// - [`ResolvedValue::UseOurs`] → no-op (ours DB is unchanged)
    /// - [`ResolvedValue::UseTheirs`] → reconstructs entity from field diffs and upserts
    /// - [`ResolvedValue::Merged(value)`] → upserts the merged JSON object directly
    /// - [`ResolvedValue::Escalate`] → returns `BranchError::MergeConflictUnresolved`
    pub async fn apply_change(
        &self,
        entity_type: EntityType,
        diff: &EntityDiff,
        resolved: ResolvedValue,
    ) -> BranchResult<()> {
        match resolved {
            ResolvedValue::UseOurs => Ok(()), // no change needed
            ResolvedValue::Escalate => Err(BranchError::MergeConflictUnresolved {
                entity_ids: vec![diff.entity_id.clone()],
            }),
            ResolvedValue::UseTheirs => {
                // Reconstruct entity from field_diffs (after values).
                let value = reconstruct_from_field_diffs(diff);
                self.apply_value(&entity_type, diff, value).await
            }
            ResolvedValue::Merged(value) => self.apply_value(&entity_type, diff, value).await,
        }
    }

    /// Applies a batch of resolved changes inside a single transaction.
    ///
    /// Returns the number of rows written.
    pub async fn apply_batch(
        &self,
        changes: Vec<(EntityDiff, ResolvedValue)>,
    ) -> BranchResult<u32> {
        let mut tx = self.pool.begin().await?;
        let mut count = 0u32;

        for (diff, resolved) in changes {
            match resolved {
                ResolvedValue::UseOurs => continue,
                ResolvedValue::Escalate => {
                    return Err(BranchError::MergeConflictUnresolved {
                        entity_ids: vec![diff.entity_id.clone()],
                    });
                }
                ResolvedValue::UseTheirs => {
                    let value = reconstruct_from_field_diffs(&diff);
                    apply_value_tx(&mut tx, &diff.entity_type, &diff, value).await?;
                    count += 1;
                }
                ResolvedValue::Merged(value) => {
                    apply_value_tx(&mut tx, &diff.entity_type, &diff, value).await?;
                    count += 1;
                }
            }
        }

        tx.commit().await?;
        Ok(count)
    }

    async fn apply_value(
        &self,
        entity_type: &EntityType,
        diff: &EntityDiff,
        value: serde_json::Value,
    ) -> BranchResult<()> {
        let table = entity_type.table_name();
        match diff.diff_kind {
            DiffKind::Removed => {
                sqlx::query(&format!("DELETE FROM {table} WHERE id = ?"))
                    .bind(&diff.entity_id)
                    .execute(&self.pool)
                    .await?;
            }
            DiffKind::Added | DiffKind::Modified | DiffKind::Unchanged => {
                upsert_entity(&self.pool, table, &diff.entity_id, &value).await?;
            }
        }
        Ok(())
    }
}

// ── Transaction variant ──────────────────────────────────────────────────────

async fn apply_value_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    entity_type: &EntityType,
    diff: &EntityDiff,
    value: serde_json::Value,
) -> BranchResult<()> {
    let table = entity_type.table_name();
    match diff.diff_kind {
        DiffKind::Removed => {
            sqlx::query(&format!("DELETE FROM {table} WHERE id = ?"))
                .bind(&diff.entity_id)
                .execute(&mut **tx)
                .await?;
        }
        DiffKind::Added | DiffKind::Modified | DiffKind::Unchanged => {
            upsert_entity_tx(tx, table, &diff.entity_id, &value).await?;
        }
    }
    Ok(())
}

// ── SQL helpers ──────────────────────────────────────────────────────────────

async fn upsert_entity(
    pool: &SqlitePool,
    table: &str,
    entity_id: &str,
    value: &serde_json::Value,
) -> BranchResult<()> {
    if let Some(obj) = value.as_object() {
        let (sql, args) = build_upsert_sql(table, entity_id, obj)?;
        sqlx::query_with(&sql, args).execute(pool).await?;
    }
    Ok(())
}

async fn upsert_entity_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    entity_id: &str,
    value: &serde_json::Value,
) -> BranchResult<()> {
    if let Some(obj) = value.as_object() {
        let (sql, args) = build_upsert_sql(table, entity_id, obj)?;
        sqlx::query_with(&sql, args).execute(&mut **tx).await?;
    }
    Ok(())
}

fn build_upsert_sql(
    table: &str,
    entity_id: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> BranchResult<(String, SqliteArguments<'static>)> {
    // Collect columns: always ensure "id" is first.
    let mut columns: Vec<String> = vec!["id".to_string()];
    let mut values: Vec<serde_json::Value> = vec![serde_json::Value::String(entity_id.to_string())];

    for (k, v) in obj {
        if k != "id" {
            columns.push(k.clone());
            values.push(v.clone());
        }
    }

    let col_list = columns.join(", ");
    let placeholders = columns.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let sql = format!("INSERT OR REPLACE INTO {table} ({col_list}) VALUES ({placeholders})");

    let mut args = SqliteArguments::default();
    for v in &values {
        args.add(json_to_sqlite_str(v))
            .map_err(|error| BranchError::InvalidConfig(format!("invalid sqlite arg: {error}")))?;
    }

    Ok((sql, args))
}

fn json_to_sqlite_str(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(b) => Some(if *b { "1" } else { "0" }.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => Some(v.to_string()),
    }
}

fn reconstruct_from_field_diffs(diff: &EntityDiff) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "id".to_string(),
        serde_json::Value::String(diff.entity_id.clone()),
    );
    for fd in &diff.field_diffs {
        obj.insert(fd.field.clone(), fd.after.clone());
    }
    serde_json::Value::Object(obj)
}
