//! Merge subsystem: three-way merges, conflict resolution, and apply helpers.

/// Merge result application to target databases.
pub mod applier;
/// Conflict detection utilities.
pub mod conflict;
/// Conflict resolution strategies and helpers.
pub mod resolver;
/// Merge strategy selection.
pub mod strategies;
/// Three-way merge orchestration.
pub mod three_way;

pub use applier::MergeApplier;
pub use conflict::{detect_conflicts, is_trivially_resolvable};
pub use resolver::{ConflictResolver, ResolvedValue};
pub use strategies::{strategy_for_entity_type, MergeStrategy};
pub use three_way::{MergePreview, ThreeWayMerger};
