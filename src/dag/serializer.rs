//! SQLite persistence for the branch lineage DAG.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};
use uuid::Uuid;

use crate::{
    config::BranchConfig,
    dag::graph::{DagGraph, EdgeMeta},
    error::{BranchError, BranchResult},
};

/// Persists and reconstructs the branch lineage DAG in the branch registry database.
pub struct DagSerializer {
    pool: SqlitePool,
    config: Arc<BranchConfig>,
}

impl DagSerializer {
    /// Creates a serializer backed by the given SQLite pool and workspace config.
    pub async fn new(config: Arc<BranchConfig>) -> BranchResult<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&config.registry_db_path)
                    .create_if_missing(true),
            )
            .await?;
        Ok(Self { pool, config })
    }

    /// Creates a serializer from an existing pool (useful in tests and engine wiring).
    pub fn from_pool(pool: SqlitePool, config: Arc<BranchConfig>) -> Self {
        Self { pool, config }
    }

    /// Persists a new DAG node for the given branch.
    pub async fn save_node(&self, branch_id: Uuid, workspace_id: Uuid) -> BranchResult<()> {
        let added_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO dag_nodes (branch_id, workspace_id, added_at) \
             VALUES (?, ?, ?)",
        )
        .bind(branch_id.to_string())
        .bind(workspace_id.to_string())
        .bind(added_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Persists a directed edge between two branches.
    pub async fn save_edge(
        &self,
        parent_id: Uuid,
        child_id: Uuid,
        forked_at: DateTime<Utc>,
    ) -> BranchResult<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO dag_edges (parent_id, child_id, forked_at) \
             VALUES (?, ?, ?)",
        )
        .bind(parent_id.to_string())
        .bind(child_id.to_string())
        .bind(forked_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Removes a branch node and its associated edges.
    pub async fn delete_node(&self, branch_id: Uuid) -> BranchResult<()> {
        let id_str = branch_id.to_string();
        sqlx::query("DELETE FROM dag_edges WHERE parent_id = ? OR child_id = ?")
            .bind(&id_str)
            .bind(&id_str)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM dag_nodes WHERE branch_id = ?")
            .bind(&id_str)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Updates the merge cursor stored on an edge after a merge completes.
    pub async fn update_edge_cursor(
        &self,
        parent_id: Uuid,
        child_id: Uuid,
        cursor: &str,
    ) -> BranchResult<()> {
        sqlx::query("UPDATE dag_edges SET merge_cursor = ? WHERE parent_id = ? AND child_id = ?")
            .bind(cursor)
            .bind(parent_id.to_string())
            .bind(child_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Reconstructs the full in-memory [`DagGraph`] for a workspace from persisted state.
    ///
    /// Nodes are added first; edges are inserted in parent-before-child order.
    pub async fn load_graph(&self, workspace_id: Uuid) -> BranchResult<DagGraph> {
        let workspace_str = workspace_id.to_string();

        let node_rows = sqlx::query("SELECT branch_id FROM dag_nodes WHERE workspace_id = ?")
            .bind(&workspace_str)
            .fetch_all(&self.pool)
            .await?;

        let edge_rows = sqlx::query(
            "SELECT e.parent_id, e.child_id, e.forked_at, e.merge_cursor \
             FROM dag_edges e \
             JOIN dag_nodes n ON n.branch_id = e.parent_id \
             WHERE n.workspace_id = ?",
        )
        .bind(&workspace_str)
        .fetch_all(&self.pool)
        .await?;

        let graph = DagGraph::new(Arc::clone(&self.config));

        for row in &node_rows {
            let id_str: String = row.try_get("branch_id")?;
            let id = Uuid::parse_str(&id_str).map_err(|e| {
                BranchError::SandboxError(format!("invalid UUID in dag_nodes: {e}"))
            })?;
            graph.add_node(id)?;
        }

        struct EdgeRecord {
            parent_id: Uuid,
            child_id: Uuid,
            meta: EdgeMeta,
        }
        let mut edges: Vec<EdgeRecord> = Vec::with_capacity(edge_rows.len());
        for row in &edge_rows {
            let parent_id = Uuid::parse_str(&row.try_get::<String, _>("parent_id")?)
                .map_err(|e| BranchError::SandboxError(format!("invalid parent UUID: {e}")))?;
            let child_id = Uuid::parse_str(&row.try_get::<String, _>("child_id")?)
                .map_err(|e| BranchError::SandboxError(format!("invalid child UUID: {e}")))?;
            let forked_at = DateTime::parse_from_rfc3339(&row.try_get::<String, _>("forked_at")?)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| BranchError::SandboxError(format!("invalid forked_at: {e}")))?;
            let cursor: Option<String> = row.try_get("merge_cursor")?;
            edges.push(EdgeRecord {
                parent_id,
                child_id,
                meta: EdgeMeta {
                    forked_at,
                    merge_cursor: cursor,
                },
            });
        }

        // Topological sort of edge list via Kahn's algorithm so parents are added
        // before children, avoiding any spurious cycle-detection failures.
        let mut out_edges: std::collections::HashMap<Uuid, Vec<usize>> =
            std::collections::HashMap::new();
        let mut in_degree: std::collections::HashMap<Uuid, usize> =
            std::collections::HashMap::new();
        for (i, e) in edges.iter().enumerate() {
            out_edges.entry(e.parent_id).or_default().push(i);
            in_degree
                .entry(e.child_id)
                .and_modify(|d| *d += 1)
                .or_insert(1);
            in_degree.entry(e.parent_id).or_insert(0);
        }

        let mut queue: std::collections::VecDeque<Uuid> = in_degree
            .iter()
            .filter_map(|(&id, &d)| if d == 0 { Some(id) } else { None })
            .collect();
        let mut insert_order: Vec<usize> = Vec::with_capacity(edges.len());
        while let Some(parent) = queue.pop_front() {
            if let Some(child_indices) = out_edges.remove(&parent) {
                for idx in child_indices {
                    let child = edges[idx].child_id;
                    insert_order.push(idx);
                    let d = in_degree.entry(child).or_insert(1);
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        queue.push_back(child);
                    }
                }
            }
        }
        // Append any edges not reached (should not happen for valid DAGs).
        for i in 0..edges.len() {
            if !insert_order.contains(&i) {
                insert_order.push(i);
            }
        }

        for idx in insert_order {
            let e = &edges[idx];
            graph.add_edge_meta(e.parent_id, e.child_id, e.meta.clone())?;
        }

        Ok(graph)
    }
}
