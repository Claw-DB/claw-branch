//! SQLite-backed persistence for branch metadata, metrics, and commit history.

use std::path::Path;

use chrono::{DateTime, Utc};
use sqlx::{sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions}, Row, SqlitePool};
use uuid::Uuid;

use crate::{
    error::{BranchError, BranchResult},
    types::{Branch, BranchMetrics, BranchStatus, CommitLogEntry, EntityType},
};

/// Stores branch registry state in a SQLite database.
#[derive(Debug, Clone)]
pub struct BranchStore {
    pool: SqlitePool,
}

impl BranchStore {
    /// Opens or creates a branch registry database and applies migrations.
    pub async fn new(db_path: &Path) -> BranchResult<Self> {
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let options = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new().max_connections(5).connect_with(options).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Returns the underlying SQLite pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Inserts a branch and its initial metrics row.
    pub async fn insert(&self, branch: &Branch) -> BranchResult<()> {
        let metadata = serde_json::to_string(&branch.metadata)?;
        sqlx::query(
            "INSERT INTO branches (id, name, slug, workspace_id, parent_id, status, db_path, snapshot_path, forked_from_cursor, description, metadata, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(branch.id.to_string())
        .bind(&branch.name)
        .bind(&branch.slug)
        .bind(branch.workspace_id.to_string())
        .bind(branch.parent_id.map(|value| value.to_string()))
        .bind(branch.status.to_storage())
        .bind(branch.db_path.to_string_lossy().to_string())
        .bind(branch.snapshot_path.to_string_lossy().to_string())
        .bind(&branch.forked_from_cursor)
        .bind(&branch.description)
        .bind(metadata)
        .bind(branch.created_at.to_rfc3339())
        .bind(branch.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(map_unique_name_error(&branch.name))?;

        self.update_metrics(branch.id, &branch.metrics).await
    }

    /// Retrieves a branch by id.
    pub async fn get(&self, id: Uuid) -> BranchResult<Branch> {
        let row = sqlx::query(
            "SELECT b.id, b.name, b.slug, b.workspace_id, b.parent_id, b.status, b.db_path, b.snapshot_path, b.forked_from_cursor, b.description, b.metadata, b.created_at, b.updated_at, m.op_count, m.memory_record_count, m.session_count, m.tool_output_count, m.bytes_on_disk, m.divergence_score, m.created_entity_count, m.updated_entity_count, m.deleted_entity_count, m.last_activity_at FROM branches b LEFT JOIN branch_metrics m ON m.branch_id = b.id WHERE b.id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(branch_from_row)
            .transpose()?
            .ok_or(BranchError::BranchNotFound(id))
    }

    /// Retrieves a branch by workspace and slug.
    pub async fn get_by_slug(&self, workspace_id: Uuid, slug: &str) -> BranchResult<Branch> {
        let row = sqlx::query(
            "SELECT b.id, b.name, b.slug, b.workspace_id, b.parent_id, b.status, b.db_path, b.snapshot_path, b.forked_from_cursor, b.description, b.metadata, b.created_at, b.updated_at, m.op_count, m.memory_record_count, m.session_count, m.tool_output_count, m.bytes_on_disk, m.divergence_score, m.created_entity_count, m.updated_entity_count, m.deleted_entity_count, m.last_activity_at FROM branches b LEFT JOIN branch_metrics m ON m.branch_id = b.id WHERE b.workspace_id = ? AND b.slug = ?",
        )
        .bind(workspace_id.to_string())
        .bind(slug)
        .fetch_optional(&self.pool)
        .await?;

        row.map(branch_from_row)
            .transpose()?
            .ok_or_else(|| BranchError::BranchAlreadyExists(slug.to_string()))
    }

    /// Lists branches for a workspace, optionally filtered by status kind.
    pub async fn list(
        &self,
        workspace_id: Uuid,
        status: Option<BranchStatus>,
    ) -> BranchResult<Vec<Branch>> {
        let rows = sqlx::query(
            "SELECT b.id, b.name, b.slug, b.workspace_id, b.parent_id, b.status, b.db_path, b.snapshot_path, b.forked_from_cursor, b.description, b.metadata, b.created_at, b.updated_at, m.op_count, m.memory_record_count, m.session_count, m.tool_output_count, m.bytes_on_disk, m.divergence_score, m.created_entity_count, m.updated_entity_count, m.deleted_entity_count, m.last_activity_at FROM branches b LEFT JOIN branch_metrics m ON m.branch_id = b.id WHERE b.workspace_id = ? ORDER BY b.created_at ASC",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut branches = rows
            .into_iter()
            .map(branch_from_row)
            .collect::<BranchResult<Vec<_>>>()?;

        if let Some(expected_status) = status {
            let expected_kind = expected_status.kind();
            branches.retain(|branch| branch.status.kind() == expected_kind);
        }

        Ok(branches)
    }

    /// Updates the lifecycle status of a branch.
    pub async fn update_status(&self, id: Uuid, status: BranchStatus) -> BranchResult<()> {
        let result = sqlx::query("UPDATE branches SET status = ?, updated_at = ? WHERE id = ?")
            .bind(status.to_storage())
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(BranchError::BranchNotFound(id));
        }
        Ok(())
    }

    /// Upserts branch metrics for the given branch identifier.
    pub async fn update_metrics(&self, id: Uuid, metrics: &BranchMetrics) -> BranchResult<()> {
        sqlx::query(
            "INSERT INTO branch_metrics (branch_id, op_count, memory_record_count, session_count, tool_output_count, bytes_on_disk, divergence_score, created_entity_count, updated_entity_count, deleted_entity_count, last_activity_at, refreshed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(branch_id) DO UPDATE SET op_count = excluded.op_count, memory_record_count = excluded.memory_record_count, session_count = excluded.session_count, tool_output_count = excluded.tool_output_count, bytes_on_disk = excluded.bytes_on_disk, divergence_score = excluded.divergence_score, created_entity_count = excluded.created_entity_count, updated_entity_count = excluded.updated_entity_count, deleted_entity_count = excluded.deleted_entity_count, last_activity_at = excluded.last_activity_at, refreshed_at = excluded.refreshed_at",
        )
        .bind(id.to_string())
        .bind(metrics.op_count as i64)
        .bind(metrics.memory_record_count)
        .bind(metrics.session_count)
        .bind(metrics.tool_output_count)
        .bind(metrics.bytes_on_disk as i64)
        .bind(metrics.divergence_score)
        .bind(metrics.created_entity_count as i64)
        .bind(metrics.updated_entity_count as i64)
        .bind(metrics.deleted_entity_count as i64)
        .bind(metrics.last_activity_at.map(|value| value.to_rfc3339()))
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Deletes a branch registry row and its metrics entry.
    pub async fn delete(&self, id: Uuid) -> BranchResult<()> {
        sqlx::query("DELETE FROM branch_metrics WHERE branch_id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        let result = sqlx::query("DELETE FROM branches WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(BranchError::BranchNotFound(id));
        }
        Ok(())
    }

    /// Counts branches for a workspace.
    pub async fn count(&self, workspace_id: Uuid) -> BranchResult<u64> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM branches WHERE workspace_id = ?")
            .bind(workspace_id.to_string())
            .fetch_one(&self.pool)
            .await?;
        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    /// Inserts a commit log entry.
    pub async fn insert_commit_log(&self, entry: &CommitLogEntry) -> BranchResult<()> {
        let entity_ids = serde_json::to_string(&entry.entity_ids)?;
        sqlx::query(
            "INSERT INTO branch_commits (id, branch_id, entity_type, entity_ids, op_kind, committed_at, message) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(entry.id.to_string())
        .bind(entry.branch_id.to_string())
        .bind(entry.entity_type.as_ref().map(EntityType::as_str))
        .bind(entity_ids)
        .bind(&entry.op_kind)
        .bind(entry.committed_at.to_rfc3339())
        .bind(&entry.message)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lists recent commit log entries for a branch.
    pub async fn list_commits(&self, branch_id: Uuid, limit: u32) -> BranchResult<Vec<CommitLogEntry>> {
        let rows = sqlx::query(
            "SELECT id, branch_id, entity_type, entity_ids, op_kind, committed_at, message FROM branch_commits WHERE branch_id = ? ORDER BY committed_at DESC LIMIT ?",
        )
        .bind(branch_id.to_string())
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(commit_log_from_row).collect()
    }

    /// Counts total commits recorded for a branch.
    pub async fn count_commits(&self, branch_id: Uuid) -> BranchResult<u64> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM branch_commits WHERE branch_id = ?")
            .bind(branch_id.to_string())
            .fetch_one(&self.pool)
            .await?;
        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    /// Finds a branch by its human-readable name within the workspace.
    pub async fn get_by_name(&self, workspace_id: Uuid, name: &str) -> BranchResult<Branch> {
        let row = sqlx::query(
            "SELECT b.id, b.name, b.slug, b.workspace_id, b.parent_id, b.status, b.db_path, b.snapshot_path, b.forked_from_cursor, b.description, b.metadata, b.created_at, b.updated_at, m.op_count, m.memory_record_count, m.session_count, m.tool_output_count, m.bytes_on_disk, m.divergence_score, m.created_entity_count, m.updated_entity_count, m.deleted_entity_count, m.last_activity_at FROM branches b LEFT JOIN branch_metrics m ON m.branch_id = b.id WHERE b.workspace_id = ? AND b.name = ?",
        )
        .bind(workspace_id.to_string())
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        row.map(branch_from_row)
            .transpose()?
            .ok_or_else(|| BranchError::BranchAlreadyExists(name.to_string()))
    }
}

fn map_unique_name_error(name: &str) -> impl FnOnce(sqlx::Error) -> BranchError + '_ {
    move |error| match &error {
        sqlx::Error::Database(database_error)
            if database_error.message().contains("UNIQUE constraint failed") =>
        {
            BranchError::BranchAlreadyExists(name.to_string())
        }
        _ => BranchError::Database(error),
    }
}

fn branch_from_row(row: sqlx::sqlite::SqliteRow) -> BranchResult<Branch> {
    let metadata = row.try_get::<String, _>("metadata")?;
    let status = row.try_get::<String, _>("status")?;
    Ok(Branch {
        id: parse_uuid(row.try_get("id")?)?,
        name: row.try_get("name")?,
        slug: row.try_get("slug")?,
        workspace_id: parse_uuid(row.try_get("workspace_id")?)?,
        parent_id: row
            .try_get::<Option<String>, _>("parent_id")?
            .map(parse_uuid)
            .transpose()?,
        status: BranchStatus::from_storage(&status).ok_or_else(|| {
            BranchError::Serialization(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid branch status: {status}"),
            )))
        })?,
        db_path: row.try_get::<String, _>("db_path")?.into(),
        snapshot_path: row.try_get::<String, _>("snapshot_path")?.into(),
        forked_from_cursor: row.try_get("forked_from_cursor")?,
        description: row.try_get("description")?,
        metadata: serde_json::from_str(&metadata)?,
        created_at: parse_datetime(row.try_get("created_at")?)?,
        updated_at: parse_datetime(row.try_get("updated_at")?)?,
        metrics: BranchMetrics {
            op_count: row.try_get::<Option<i64>, _>("op_count")?.unwrap_or_default() as u64,
            memory_record_count: row
                .try_get::<Option<i64>, _>("memory_record_count")?
                .unwrap_or_default(),
            session_count: row.try_get::<Option<i64>, _>("session_count")?.unwrap_or_default(),
            tool_output_count: row
                .try_get::<Option<i64>, _>("tool_output_count")?
                .unwrap_or_default(),
            bytes_on_disk: row.try_get::<Option<i64>, _>("bytes_on_disk")?.unwrap_or_default() as u64,
            divergence_score: row
                .try_get::<Option<f64>, _>("divergence_score")?
                .unwrap_or_default(),
            created_entity_count: row
                .try_get::<Option<i64>, _>("created_entity_count")?
                .unwrap_or_default() as u64,
            updated_entity_count: row
                .try_get::<Option<i64>, _>("updated_entity_count")?
                .unwrap_or_default() as u64,
            deleted_entity_count: row
                .try_get::<Option<i64>, _>("deleted_entity_count")?
                .unwrap_or_default() as u64,
            last_activity_at: row
                .try_get::<Option<String>, _>("last_activity_at")?
                .map(parse_datetime)
                .transpose()?,
        },
    })
}

fn commit_log_from_row(row: sqlx::sqlite::SqliteRow) -> BranchResult<CommitLogEntry> {
    let entity_ids = row.try_get::<String, _>("entity_ids")?;
    Ok(CommitLogEntry {
        id: parse_uuid(row.try_get("id")?)?,
        branch_id: parse_uuid(row.try_get("branch_id")?)?,
        entity_type: row
            .try_get::<Option<String>, _>("entity_type")?
            .as_deref()
            .and_then(EntityType::from_str),
        entity_ids: serde_json::from_str(&entity_ids)?,
        op_kind: row.try_get("op_kind")?,
        committed_at: parse_datetime(row.try_get("committed_at")?)?,
        message: row.try_get("message")?,
    })
}

fn parse_uuid(value: String) -> BranchResult<Uuid> {
    Uuid::parse_str(&value).map_err(|error| {
        BranchError::Serialization(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error,
        )))
    })
}

fn parse_datetime(value: String) -> BranchResult<DateTime<Utc>> {
    value.parse::<DateTime<Utc>>().map_err(|error| {
        BranchError::Serialization(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error,
        )))
    })
}