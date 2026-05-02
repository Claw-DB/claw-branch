//! DAG lineage graph modules.

/// Graph storage and mutation operations.
pub mod graph;
/// Graphviz DOT exporters.
pub mod dot;
/// Persistence helpers for DAG reconstruction.
pub mod serializer;
/// Traversal and merge-base helpers.
pub mod traversal;

pub use graph::{DagGraph, EdgeMeta};
pub use traversal::DagTraversal;
pub use serializer::DagSerializer;