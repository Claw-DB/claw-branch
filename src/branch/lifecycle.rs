//! Branch lifecycle state transitions and fork orchestration.

use std::{path::Path, sync::Arc};

use chrono::Utc;
use serde_json::json;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    Row,
};
use tracing::info;
use uuid::Uuid;

use crate::{
    branch::{naming::NamingValidator, store::BranchStore},
    config::BranchConfig,
    dag::graph::DagGraph,
    error::{BranchError, BranchResult},
    snapshot::{copier::SnapshotCopier, verifier::verify_snapshot},
    types::{Branch, BranchMetrics, BranchStatus},
};

/// Applies validated branch lifecycle transitions and snapshot-backed forks.
#[derive(Clone)]
pub struct BranchLifecycle {
    store: Arc<BranchStore>,
    copier: Arc<SnapshotCopier>,
    dag: Arc<DagGraph>,
    config: Arc<BranchConfig>,
}

impl BranchLifecycle {
    /// Creates a lifecycle coordinator from the shared branch subsystems.
    pub fn new(
        store: Arc<BranchStore>,
        copier: Arc<SnapshotCopier>,
        dag: Arc<DagGraph>,
        config: Arc<BranchConfig>,
    ) -> Self {
        Self {
            store,
            copier,
            dag,
            config,
        }
    }

    /// Creates the root trunk branch from an existing source database.
    pub async fn create_trunk(
        &self,
        workspace_id: Uuid,
        source_db_path: &Path,
    ) -> BranchResult<Branch> {
        let branch_id = Uuid::new_v4();
        let manifest = self
            .copier
            .create_snapshot(source_db_path, branch_id, &self.config.trunk_branch_name)
            .await?;
        let now = Utc::now();
        let branch = Branch {
            id: branch_id,
            name: self.config.trunk_branch_name.clone(),
            slug: self.config.trunk_branch_name.clone(),
            parent_id: None,
            status: BranchStatus::Active,
            db_path: manifest.snapshot_db_path.clone(),
            snapshot_path: manifest.snapshot_db_path.clone(),
            workspace_id,
            created_at: now,
            updated_at: now,
            forked_from_cursor: None,
            description: Some("Primary trunk branch".to_string()),
            metadata: json!({"role": "trunk", "content_id": manifest.content_id()}),
            metrics: BranchMetrics {
                memory_record_count: manifest.entity_counts.memory_records,
                session_count: manifest.entity_counts.sessions,
                tool_output_count: manifest.entity_counts.tool_outputs,
                bytes_on_disk: manifest.file_size_bytes,
                last_activity_at: Some(now),
                ..BranchMetrics::default()
            },
        };

        self.store.insert(&branch).await?;
        self.dag.add_node(branch.id)?;
        info!(branch_id = %branch.id, workspace_id = %workspace_id, "created trunk branch");
        Ok(branch)
    }

    /// Forks a live parent branch into a new active branch.
    pub async fn fork(
        &self,
        parent_id: Uuid,
        name: &str,
        description: Option<&str>,
    ) -> BranchResult<Branch> {
        NamingValidator::validate(name)?;
        let parent = self.store.get(self.config.workspace_id, parent_id).await?;
        if !parent.status.is_live() {
            return Err(BranchError::BranchNotActive {
                id: parent.id,
                status: parent.status.kind().to_string(),
            });
        }
        if self.store.count_active(parent.workspace_id).await?
            >= self.config.max_branches_per_workspace as u64
        {
            return Err(BranchError::BranchLimitExceeded);
        }
        let existing_slugs = self
            .store
            .list(parent.workspace_id, None)
            .await?
            .into_iter()
            .map(|branch| branch.slug)
            .collect::<Vec<_>>();
        let slug = NamingValidator::make_unique(&NamingValidator::slugify(name), &existing_slugs);
        if existing_slugs.iter().any(|existing| existing == &slug) {
            return Err(BranchError::BranchAlreadyExists(slug));
        }

        let source_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&parent.db_path)
                    .create_if_missing(false)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await?;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&source_pool)
            .await?;
        let cursor_row = sqlx::query("PRAGMA data_version")
            .fetch_one(&source_pool)
            .await?;
        let forked_from_cursor = Some(cursor_row.try_get::<i64, _>(0)?.to_string());

        let branch_id = Uuid::new_v4();
        let manifest = self
            .copier
            .create_snapshot(&parent.db_path, branch_id, name)
            .await?;
        let now = Utc::now();
        let branch = Branch {
            id: branch_id,
            name: name.to_string(),
            slug,
            parent_id: Some(parent_id),
            status: BranchStatus::Active,
            db_path: manifest.snapshot_db_path.clone(),
            snapshot_path: manifest.snapshot_db_path.clone(),
            workspace_id: parent.workspace_id,
            created_at: now,
            updated_at: now,
            forked_from_cursor,
            description: description.map(ToOwned::to_owned),
            metadata: json!({"forked_from": parent_id, "content_id": manifest.content_id()}),
            metrics: BranchMetrics {
                memory_record_count: manifest.entity_counts.memory_records,
                session_count: manifest.entity_counts.sessions,
                tool_output_count: manifest.entity_counts.tool_outputs,
                bytes_on_disk: manifest.file_size_bytes,
                last_activity_at: Some(now),
                ..BranchMetrics::default()
            },
        };

        self.store.insert(&branch).await?;
        self.dag.add_node(parent_id)?;
        self.dag.add_edge(parent_id, branch.id)?;
        info!(branch_id = %branch.id, parent_id = %parent.id, slug = %branch.slug, "forked branch");
        Ok(branch)
    }

    /// Activates a dormant branch.
    pub async fn activate(&self, id: Uuid) -> BranchResult<()> {
        let branch = self.store.get(self.config.workspace_id, id).await?;
        match branch.status {
            BranchStatus::Dormant => {
                self.store.update_status(id, BranchStatus::Active).await?;
                info!(branch_id = %id, "activated branch");
                Ok(())
            }
            BranchStatus::Active => Ok(()),
            other => Err(BranchError::BranchNotActive {
                id,
                status: other.kind().to_string(),
            }),
        }
    }

    /// Deactivates an active branch into a dormant branch.
    pub async fn deactivate(&self, id: Uuid) -> BranchResult<()> {
        let branch = self.store.get(self.config.workspace_id, id).await?;
        match branch.status {
            BranchStatus::Active => {
                self.store.update_status(id, BranchStatus::Dormant).await?;
                info!(branch_id = %id, "deactivated branch");
                Ok(())
            }
            other => Err(BranchError::BranchNotActive {
                id,
                status: other.kind().to_string(),
            }),
        }
    }

    /// Marks a live branch as discarded and leaves cleanup to the garbage collector.
    pub async fn discard(&self, id: Uuid) -> BranchResult<()> {
        let branch = self.store.get(self.config.workspace_id, id).await?;
        if !branch.status.is_live() {
            return Err(BranchError::BranchNotActive {
                id,
                status: branch.status.kind().to_string(),
            });
        }
        self.store
            .update_status(
                id,
                BranchStatus::Discarded {
                    discarded_at: Utc::now(),
                },
            )
            .await?;
        info!(branch_id = %id, "discarded branch");
        Ok(())
    }

    /// Archives a live branch for later restore.
    pub async fn archive(&self, id: Uuid) -> BranchResult<()> {
        let branch = self.store.get(self.config.workspace_id, id).await?;
        if !branch.status.is_live() {
            return Err(BranchError::BranchNotActive {
                id,
                status: branch.status.kind().to_string(),
            });
        }
        self.store.update_status(id, BranchStatus::Archived).await?;
        info!(branch_id = %id, "archived branch");
        Ok(())
    }

    /// Restores an archived branch back to active service.
    pub async fn restore_archived(&self, id: Uuid) -> BranchResult<Branch> {
        let branch = self.store.get(self.config.workspace_id, id).await?;
        match branch.status {
            BranchStatus::Archived => {
                let manifest = crate::snapshot::manifest::SnapshotManifest::load(
                    branch
                        .snapshot_path
                        .parent()
                        .ok_or_else(|| BranchError::SnapshotFailed {
                            branch_id: id,
                            reason: "snapshot path missing parent directory".to_string(),
                        })?,
                )?;
                verify_snapshot(&manifest).await?;
                self.store.update_status(id, BranchStatus::Active).await?;
                self.store.get(self.config.workspace_id, id).await
            }
            other => Err(BranchError::BranchNotActive {
                id,
                status: other.kind().to_string(),
            }),
        }
    }
}
