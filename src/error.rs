//! Error types for branch orchestration.

use std::path::PathBuf;

use thiserror::Error;
use uuid::Uuid;

/// The result type used across the branch engine.
pub type BranchResult<T> = Result<T, BranchError>;

/// Enumerates errors produced by branching, snapshotting, merging, and metrics code.
#[derive(Debug, Error)]
pub enum BranchError {
    /// Indicates a runtime configuration is invalid.
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    /// Indicates permission checks denied the operation.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// Indicates branch data escaped workspace-scoped query isolation.
    #[error("workspace isolation violation: expected {expected}, found {found}")]
    WorkspaceIsolationViolation {
        /// The expected workspace id configured for this engine.
        expected: Uuid,
        /// The unexpected workspace id returned by persistence.
        found: Uuid,
    },
    /// Indicates a branch name failed strict path-safe validation.
    #[error("invalid branch name: {0}")]
    InvalidBranchName(String),
    /// Indicates the workspace has reached its configured branch limit.
    #[error("branch limit exceeded")]
    BranchLimitExceeded,
    /// Wraps SQLx database failures.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    /// Wraps SQLx migration failures.
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    /// Wraps filesystem errors.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Wraps JSON serialization errors.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Indicates a branch lookup failed.
    #[error("branch not found: {0}")]
    BranchNotFound(Uuid),
    /// Indicates a workspace already has a branch with the same logical name.
    #[error("branch already exists: {0}")]
    BranchAlreadyExists(String),
    /// Indicates an operation requires an active branch.
    #[error("branch {id} is not active: {status}")]
    BranchNotActive {
        /// The branch identifier that failed the active-state check.
        id: Uuid,
        /// The current lifecycle status string for the branch.
        status: String,
    },
    /// Indicates snapshot creation or restore failed.
    #[error("snapshot failed for branch {branch_id}: {reason}")]
    SnapshotFailed {
        /// The branch identifier associated with the snapshot failure.
        branch_id: Uuid,
        /// The human-readable failure reason.
        reason: String,
    },
    /// Indicates a snapshot file failed integrity validation.
    #[error("snapshot corrupt for branch {branch_id} at {path:?}")]
    SnapshotCorrupt {
        /// The branch identifier associated with the corrupt snapshot.
        branch_id: Uuid,
        /// The path to the corrupt snapshot file.
        path: PathBuf,
    },
    /// Indicates a required snapshot hash sidecar file is missing.
    #[error("snapshot hash missing for branch {branch_id} at {path:?}")]
    SnapshotHashMissing {
        /// The branch identifier associated with the missing hash sidecar.
        branch_id: Uuid,
        /// The path to the expected sidecar hash file.
        path: PathBuf,
    },
    /// Indicates merge conflicts were not fully resolved.
    #[error("merge conflicts unresolved for entities: {entity_ids:?}")]
    MergeConflictUnresolved {
        /// The entity identifiers that still have unresolved conflicts.
        entity_ids: Vec<String>,
    },
    /// Indicates two branches cannot be merged with the selected base.
    #[error("merge incompatible base for ours={ours} theirs={theirs}: {reason}")]
    MergeIncompatibleBase {
        /// The local branch identifier.
        ours: Uuid,
        /// The incoming branch identifier.
        theirs: Uuid,
        /// The reason the base is incompatible.
        reason: String,
    },
    /// Indicates a lineage edge would create a DAG cycle.
    #[error("dag cycle detected from {from} to {to}")]
    DagCycle {
        /// The proposed source branch id.
        from: Uuid,
        /// The proposed destination branch id.
        to: Uuid,
    },
    /// Indicates a branch operation would introduce a lineage cycle.
    #[error("cycle detected from {from} to {to}")]
    CycleDetected {
        /// The proposed source branch id.
        from: Uuid,
        /// The proposed destination branch id.
        to: Uuid,
    },
    /// Indicates a DAG node lookup failed.
    #[error("dag node not found: {0}")]
    DagNodeNotFound(Uuid),
    /// Indicates diff extraction failed.
    #[error("diff failed between {branch_a} and {branch_b}: {reason}")]
    DiffFailed {
        /// The first branch identifier in the failed diff.
        branch_a: Uuid,
        /// The second branch identifier in the failed diff.
        branch_b: Uuid,
        /// The human-readable diff failure reason.
        reason: String,
    },
    /// Indicates a selective commit failed validation.
    #[error("commit validation failed for branch {branch_id}: {violations:?}")]
    CommitValidationFailed {
        /// The branch identifier whose commit was rejected.
        branch_id: Uuid,
        /// The list of validation violations.
        violations: Vec<String>,
    },
    /// Indicates sandbox execution failed.
    #[error("sandbox error: {0}")]
    SandboxError(String),
    /// Indicates metrics handling failed.
    #[error("metrics error: {0}")]
    MetricsError(String),
    /// Indicates a branch name failed validation.
    #[error("naming error: {0}")]
    NamingError(String),
    /// Indicates an orphaned snapshot path was encountered.
    #[error("orphaned snapshot: {0:?}")]
    OrphanedSnapshot(PathBuf),
}

impl BranchError {
    /// Returns true if this error indicates a missing branch or DAG node.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::BranchNotFound(_) | Self::DagNodeNotFound(_))
    }

    /// Returns true if this error represents an unresolved merge conflict.
    pub fn is_merge_conflict(&self) -> bool {
        matches!(
            self,
            Self::MergeConflictUnresolved { .. } | Self::MergeIncompatibleBase { .. }
        )
    }

    /// Returns true if this error belongs to the snapshot subsystem.
    pub fn is_snapshot_error(&self) -> bool {
        matches!(
            self,
            Self::SnapshotFailed { .. }
                | Self::SnapshotCorrupt { .. }
                | Self::SnapshotHashMissing { .. }
                | Self::OrphanedSnapshot(_)
        )
    }

    /// Returns true if this error belongs to DAG management.
    pub fn is_dag_error(&self) -> bool {
        matches!(
            self,
            Self::DagCycle { .. } | Self::CycleDetected { .. } | Self::DagNodeNotFound(_)
        )
    }
}
