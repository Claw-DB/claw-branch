//! DAG lineage graph modules.

/// Graphviz DOT exporters.
pub mod dot;
/// Graph storage and mutation operations.
pub mod graph;
/// Persistence helpers for DAG reconstruction.
pub mod serializer;
/// Traversal and merge-base helpers.
pub mod traversal;

pub use graph::{DagGraph, EdgeMeta};
pub use serializer::DagSerializer;
pub use traversal::DagTraversal;
