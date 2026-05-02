//! Shared domain types used across the branch engine.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Represents a branch in the ClawDB branching graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Branch {
    /// The unique branch identifier.
    pub id: Uuid,
    /// The human-readable branch name.
    pub name: String,
    /// The URL-safe branch slug.
    pub slug: String,
    /// The parent branch identifier when this branch is forked.
    pub parent_id: Option<Uuid>,
    /// The current branch lifecycle status.
    pub status: BranchStatus,
    /// The isolated SQLite database path for this branch.
    pub db_path: PathBuf,
    /// The snapshot path backing this branch.
    pub snapshot_path: PathBuf,
    /// The workspace identifier that owns the branch.
    pub workspace_id: Uuid,
    /// The timestamp at which the branch was created.
    pub created_at: DateTime<Utc>,
    /// The timestamp at which the branch was last updated.
    pub updated_at: DateTime<Utc>,
    /// The source cursor or data version captured when the branch was forked.
    pub forked_from_cursor: Option<String>,
    /// An optional branch description.
    pub description: Option<String>,
    /// Arbitrary structured metadata associated with the branch.
    pub metadata: Value,
    /// Aggregated operational metrics for the branch.
    pub metrics: BranchMetrics,
}

/// Describes the current lifecycle status of a branch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BranchStatus {
    /// The branch is active and can accept writes.
    Active,
    /// The branch is preserved but inactive.
    Dormant,
    /// The branch has been merged into another branch.
    Merged {
        /// The branch the current branch merged into.
        merged_into: Uuid,
        /// The timestamp when the merge completed.
        merged_at: DateTime<Utc>,
    },
    /// The branch has been discarded and awaits cleanup.
    Discarded {
        /// The timestamp when the discard happened.
        discarded_at: DateTime<Utc>,
    },
    /// The branch has been archived for later recovery.
    Archived,
}

impl BranchStatus {
    /// Returns true when the branch is still live and eligible for activation-related operations.
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Active | Self::Dormant)
    }

    /// Returns true when the branch can accept commits.
    pub fn can_commit(&self) -> bool {
        matches!(self, Self::Active)
    }

    /// Returns the coarse-grained lifecycle kind.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Dormant => "dormant",
            Self::Merged { .. } => "merged",
            Self::Discarded { .. } => "discarded",
            Self::Archived => "archived",
        }
    }

    /// Converts the status to a storage-friendly string.
    pub fn to_storage(&self) -> String {
        match self {
            Self::Active => "active".to_string(),
            Self::Dormant => "dormant".to_string(),
            Self::Archived => "archived".to_string(),
            Self::Merged {
                merged_into,
                merged_at,
            } => format!("merged:{merged_into}:{merged_at}"),
            Self::Discarded { discarded_at } => format!("discarded:{discarded_at}"),
        }
    }

    /// Parses a storage string back into a branch status.
    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "dormant" => Some(Self::Dormant),
            "archived" => Some(Self::Archived),
            _ if value.starts_with("merged:") => {
                let mut parts = value.splitn(3, ':');
                let _ = parts.next();
                let merged_into = parts.next().and_then(|part| Uuid::parse_str(part).ok())?;
                let merged_at = parts.next().and_then(|part| part.parse::<DateTime<Utc>>().ok())?;
                Some(Self::Merged {
                    merged_into,
                    merged_at,
                })
            }
            _ if value.starts_with("discarded:") => {
                let discarded_at = value
                    .split_once(':')
                    .and_then(|(_, timestamp)| timestamp.parse::<DateTime<Utc>>().ok())?;
                Some(Self::Discarded { discarded_at })
            }
            _ => None,
        }
    }
}

/// Captures aggregate metrics for a branch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BranchMetrics {
    /// The number of operations executed on the branch.
    pub op_count: u64,
    /// The number of memory records on the branch.
    pub memory_record_count: i64,
    /// The number of sessions on the branch.
    pub session_count: i64,
    /// The number of tool outputs on the branch.
    pub tool_output_count: i64,
    /// The total branch size on disk in bytes.
    pub bytes_on_disk: u64,
    /// The computed divergence score relative to trunk or a merge base.
    pub divergence_score: f64,
    /// The number of created entities on the branch.
    pub created_entity_count: u64,
    /// The number of updated entities on the branch.
    pub updated_entity_count: u64,
    /// The number of deleted entities on the branch.
    pub deleted_entity_count: u64,
    /// The timestamp of the last recorded branch activity.
    pub last_activity_at: Option<DateTime<Utc>>,
}

/// Captures the result of comparing two branches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffResult {
    /// The first branch identifier in the comparison.
    pub branch_a_id: Uuid,
    /// The second branch identifier in the comparison.
    pub branch_b_id: Uuid,
    /// The timestamp when the comparison ran.
    pub compared_at: DateTime<Utc>,
    /// Per-entity diffs discovered during comparison.
    pub entity_diffs: Vec<EntityDiff>,
    /// Aggregate diff statistics.
    pub stats: DiffStats,
    /// The computed divergence score.
    pub divergence_score: f64,
}

/// Describes how a single entity differs between two branches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntityDiff {
    /// The entity identifier.
    pub entity_id: String,
    /// The entity type represented by the diff.
    pub entity_type: EntityType,
    /// The high-level kind of change.
    pub diff_kind: DiffKind,
    /// The per-field changes discovered for the entity.
    pub field_diffs: Vec<FieldDiff>,
}

/// Describes the coarse-grained kind of change for an entity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DiffKind {
    /// The entity only exists in the second branch.
    Added,
    /// The entity only exists in the first branch.
    Removed,
    /// The entity exists in both branches with field changes.
    Modified,
    /// The entity exists in both branches without changes.
    Unchanged,
}

/// Describes a change to a specific field within an entity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldDiff {
    /// The changed field name.
    pub field: String,
    /// The value before the change.
    pub before: Value,
    /// The value after the change.
    pub after: Value,
}

/// Aggregated counters for a diff run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DiffStats {
    /// The number of added entities.
    pub added: u32,
    /// The number of removed entities.
    pub removed: u32,
    /// The number of modified entities.
    pub modified: u32,
    /// The number of unchanged entities.
    pub unchanged: u32,
    /// The total number of compared entities.
    pub total_entities: u32,
}

/// Describes a field-level merge conflict.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MergeConflict {
    /// The entity identifier that conflicted.
    pub entity_id: String,
    /// The entity type that conflicted.
    pub entity_type: EntityType,
    /// The base branch value.
    pub base_value: Value,
    /// The local branch value.
    pub ours_value: Value,
    /// The incoming branch value.
    pub theirs_value: Value,
    /// The list of conflicting field names.
    pub conflicting_fields: Vec<String>,
}

/// Summarizes the outcome of a merge operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MergeResult {
    /// The source branch identifier.
    pub source_branch_id: Uuid,
    /// The target branch identifier.
    pub target_branch_id: Uuid,
    /// The base branch identifier used for the merge.
    pub base_branch_id: Uuid,
    /// The number of applied entity changes.
    pub applied: u32,
    /// The number of skipped entity changes.
    pub skipped: u32,
    /// The conflicts discovered during the merge.
    pub conflicts: Vec<MergeConflict>,
    /// The merge duration in milliseconds.
    pub duration_ms: u64,
    /// Indicates whether the merge completed successfully.
    pub success: bool,
}

/// Identifies the logical entity tables that branch diffs and merges operate on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EntityType {
    /// A memory record entity.
    MemoryRecord,
    /// A session entity.
    Session,
    /// A tool output entity.
    ToolOutput,
}

impl EntityType {
    /// Returns the SQLite table name associated with the entity type.
    pub fn table_name(&self) -> &'static str {
        match self {
            Self::MemoryRecord => "memory_records",
            Self::Session => "sessions",
            Self::ToolOutput => "tool_outputs",
        }
    }

    /// Parses a string representation into an entity type.
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "memory_record" | "memory_records" => Some(Self::MemoryRecord),
            "session" | "sessions" => Some(Self::Session),
            "tool_output" | "tool_outputs" => Some(Self::ToolOutput),
            _ => None,
        }
    }

    /// Returns the canonical storage string for the entity type.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MemoryRecord => "memory_record",
            Self::Session => "session",
            Self::ToolOutput => "tool_output",
        }
    }
}

/// Describes a persisted commit log entry for a branch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommitLogEntry {
    /// The unique commit log entry identifier.
    pub id: Uuid,
    /// The branch the entry belongs to.
    pub branch_id: Uuid,
    /// The logical entity type included in the commit.
    pub entity_type: Option<EntityType>,
    /// The affected entity identifiers.
    pub entity_ids: Vec<String>,
    /// The operation kind recorded in the log.
    pub op_kind: String,
    /// The commit timestamp.
    pub committed_at: DateTime<Utc>,
    /// An optional commit message.
    pub message: Option<String>,
}

/// Summarizes the outcome of a selective commit operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CommitResult {
    /// The number of entity rows committed to the target.
    pub committed_entity_count: u32,
    /// The number of individual fields updated.
    pub fields_updated: u32,
    /// The commit duration in milliseconds.
    pub duration_ms: u64,
    /// The target branch the entities were committed into.
    pub target_branch_id: Uuid,
    /// The timestamp the commit was recorded.
    pub committed_at: DateTime<Utc>,
}

/// Describes the observable outcome of a sandbox simulation run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimulationOutcome {
    /// The raw value returned by the agent function.
    pub agent_output: Value,
    /// The number of entity write operations recorded during the run.
    pub ops_executed: u32,
    /// The run duration in milliseconds.
    pub duration_ms: u64,
}

/// Aggregated per-workspace metrics report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceReport {
    /// The total number of branches in the workspace.
    pub branch_count: u32,
    /// The combined on-disk size of all branch databases in bytes.
    pub total_disk_bytes: u64,
    /// The mean divergence score across all live branches.
    pub avg_divergence_score: f64,
    /// The branch with the highest op count, if any.
    pub most_active_branch: Option<Uuid>,
    /// Branch identifiers with no activity in the past seven days.
    pub stale_branch_ids: Vec<Uuid>,
    /// The timestamp this report was generated.
    pub report_at: DateTime<Utc>,
}