//! In-memory DAG representation for branch lineage.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use petgraph::{
    algo::is_cyclic_directed,
    graph::{DiGraph, NodeIndex},
    Direction,
};
use uuid::Uuid;

use crate::{
    config::BranchConfig,
    error::{BranchError, BranchResult},
};

/// Metadata stored on each directed edge in the branch lineage DAG.
#[derive(Debug, Clone)]
pub struct EdgeMeta {
    /// The timestamp when the child branch was forked from the parent.
    pub forked_at: DateTime<Utc>,
    /// The data-version cursor captured at fork time, if available.
    pub merge_cursor: Option<String>,
}

struct DagInner {
    graph: DiGraph<Uuid, EdgeMeta>,
    node_index: HashMap<Uuid, NodeIndex>,
}

/// Stores branch lineage as a directed acyclic graph (parent → child edges).
///
/// Uses a synchronous `parking_lot::RwLock` so that both async lifecycle code and
/// the sync [`crate::dag::traversal::DagTraversal`] API can access state without
/// blocking the async runtime.
#[derive(Clone)]
pub struct DagGraph {
    inner: Arc<RwLock<DagInner>>,
    /// Workspace configuration that owns this DAG.
    pub config: Arc<BranchConfig>,
}

impl DagGraph {
    /// Creates a new empty DAG graph for the given workspace configuration.
    pub fn new(config: Arc<BranchConfig>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(DagInner {
                graph: DiGraph::new(),
                node_index: HashMap::new(),
            })),
            config,
        }
    }

    /// Ensures a branch node exists in the graph, adding it if absent.
    ///
    /// Returns the underlying petgraph `NodeIndex` for advanced callers.
    pub fn add_node(&self, branch_id: Uuid) -> BranchResult<NodeIndex> {
        let mut inner = self.inner.write();
        if let Some(&idx) = inner.node_index.get(&branch_id) {
            return Ok(idx);
        }
        let idx = inner.graph.add_node(branch_id);
        inner.node_index.insert(branch_id, idx);
        Ok(idx)
    }

    /// Removes a branch node only when it has no children (out-degree 0).
    pub fn remove_node(&self, branch_id: Uuid) -> BranchResult<()> {
        let mut inner = self.inner.write();
        let idx = *inner
            .node_index
            .get(&branch_id)
            .ok_or(BranchError::DagNodeNotFound(branch_id))?;
        let child_count = inner
            .graph
            .neighbors_directed(idx, Direction::Outgoing)
            .count();
        if child_count > 0 {
            return Err(BranchError::SandboxError(format!(
                "cannot remove branch {branch_id}: it still has {child_count} child branch(es)"
            )));
        }
        inner.graph.remove_node(idx);
        inner.node_index.remove(&branch_id);
        Ok(())
    }

    /// Adds a directed parent→child edge, stamping it with the current UTC time.
    ///
    /// Returns [`BranchError::DagCycle`] if the edge would create a cycle.
    pub fn add_edge(&self, parent_id: Uuid, child_id: Uuid) -> BranchResult<()> {
        self.add_edge_meta(
            parent_id,
            child_id,
            EdgeMeta {
                forked_at: Utc::now(),
                merge_cursor: None,
            },
        )
    }

    /// Adds a directed parent→child edge with explicit metadata.
    ///
    /// Used by [`crate::dag::serializer::DagSerializer`] when reconstructing a
    /// persisted DAG with its original timestamps.  Returns
    /// [`BranchError::DagCycle`] if the edge would create a cycle.
    pub fn add_edge_meta(
        &self,
        parent_id: Uuid,
        child_id: Uuid,
        meta: EdgeMeta,
    ) -> BranchResult<()> {
        if parent_id == child_id {
            return Err(BranchError::DagCycle {
                from: parent_id,
                to: child_id,
            });
        }
        let mut inner = self.inner.write();
        let parent_idx = ensure_or_insert(&mut inner, parent_id);
        let child_idx = ensure_or_insert(&mut inner, child_id);

        // Avoid adding duplicate edge.
        if inner.graph.find_edge(parent_idx, child_idx).is_some() {
            return Ok(());
        }

        inner.graph.add_edge(parent_idx, child_idx, meta);

        if is_cyclic_directed(&inner.graph) {
            // Undo the edge we just added.
            if let Some(e) = inner.graph.find_edge(parent_idx, child_idx) {
                inner.graph.remove_edge(e);
            }
            return Err(BranchError::DagCycle {
                from: parent_id,
                to: child_id,
            });
        }
        Ok(())
    }

    /// Returns the direct parent of a branch, if any.
    pub fn parent_of(&self, branch_id: Uuid) -> BranchResult<Option<Uuid>> {
        let inner = self.inner.read();
        let idx = *inner
            .node_index
            .get(&branch_id)
            .ok_or(BranchError::DagNodeNotFound(branch_id))?;
        let parent = inner
            .graph
            .neighbors_directed(idx, Direction::Incoming)
            .next()
            .map(|n| inner.graph[n]);
        Ok(parent)
    }

    /// Returns the direct children of a branch.
    pub fn children_of(&self, branch_id: Uuid) -> BranchResult<Vec<Uuid>> {
        let inner = self.inner.read();
        let idx = *inner
            .node_index
            .get(&branch_id)
            .ok_or(BranchError::DagNodeNotFound(branch_id))?;
        Ok(inner
            .graph
            .neighbors_directed(idx, Direction::Outgoing)
            .map(|n| inner.graph[n])
            .collect())
    }

    /// Returns all ancestors of a branch, walking upward to the root (closest first).
    pub fn ancestors_of(&self, branch_id: Uuid) -> BranchResult<Vec<Uuid>> {
        let inner = self.inner.read();
        let start = *inner
            .node_index
            .get(&branch_id)
            .ok_or(BranchError::DagNodeNotFound(branch_id))?;
        let mut output = Vec::new();
        let mut stack = vec![start];
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            for parent in inner.graph.neighbors_directed(node, Direction::Incoming) {
                if seen.insert(parent) {
                    output.push(inner.graph[parent]);
                    stack.push(parent);
                }
            }
        }
        Ok(output)
    }

    /// Returns the lowest common ancestor of two branches by intersecting their ancestor chains.
    ///
    /// Returns `None` when the branches share no common ancestor (disconnected subgraphs).
    pub fn find_merge_base(&self, a: Uuid, b: Uuid) -> BranchResult<Option<Uuid>> {
        if a == b {
            return Ok(Some(a));
        }
        let a_ancestors = self.ancestors_of(a)?;
        let a_set: HashSet<Uuid> = a_ancestors
            .iter()
            .copied()
            .chain(std::iter::once(a))
            .collect();
        if a_set.contains(&b) {
            return Ok(Some(b));
        }
        for anc in self.ancestors_of(b)? {
            if a_set.contains(&anc) {
                return Ok(Some(anc));
            }
        }
        Ok(None)
    }

    /// Returns `true` if `ancestor_id` is an ancestor (or equal to) `descendant_id`.
    pub fn is_ancestor(&self, ancestor_id: Uuid, descendant_id: Uuid) -> BranchResult<bool> {
        if ancestor_id == descendant_id {
            return Ok(true);
        }
        let ancestors = self.ancestors_of(descendant_id)?;
        Ok(ancestors.contains(&ancestor_id))
    }

    /// Returns the root node (in-degree 0) of the graph, if one exists.
    pub fn root(&self) -> BranchResult<Option<Uuid>> {
        let inner = self.inner.read();
        let root = inner
            .graph
            .node_indices()
            .filter(|&n| {
                inner
                    .graph
                    .neighbors_directed(n, Direction::Incoming)
                    .count()
                    == 0
            })
            .map(|n| inner.graph[n])
            .min(); // deterministic: smallest UUID
        Ok(root)
    }

    /// Returns the total number of branch nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.inner.read().graph.node_count()
    }

    /// Returns the hop distance from the nearest root node to `branch_id`.
    pub fn depth_of(&self, branch_id: Uuid) -> BranchResult<usize> {
        let inner = self.inner.read();
        let target = *inner
            .node_index
            .get(&branch_id)
            .ok_or(BranchError::DagNodeNotFound(branch_id))?;

        // BFS from all root nodes.
        let mut depth_map: HashMap<NodeIndex, usize> = HashMap::new();
        let mut queue: VecDeque<NodeIndex> = VecDeque::new();
        for idx in inner.graph.node_indices() {
            if inner
                .graph
                .neighbors_directed(idx, Direction::Incoming)
                .count()
                == 0
            {
                depth_map.insert(idx, 0);
                queue.push_back(idx);
            }
        }
        while let Some(node) = queue.pop_front() {
            let d = depth_map[&node];
            for child in inner.graph.neighbors_directed(node, Direction::Outgoing) {
                if let std::collections::hash_map::Entry::Vacant(entry) = depth_map.entry(child) {
                    entry.insert(d + 1);
                    queue.push_back(child);
                }
            }
        }
        depth_map
            .get(&target)
            .copied()
            .ok_or(BranchError::DagNodeNotFound(branch_id))
    }

    /// Returns all edges as `(parent_id, child_id, meta)` triples.
    pub fn all_edges(&self) -> Vec<(Uuid, Uuid, EdgeMeta)> {
        let inner = self.inner.read();
        inner
            .graph
            .edge_indices()
            .filter_map(|e| {
                let (src, dst) = inner.graph.edge_endpoints(e)?;
                let meta = inner.graph[e].clone();
                Some((inner.graph[src], inner.graph[dst], meta))
            })
            .collect()
    }

    /// Returns all branch node UUIDs currently registered in the graph.
    pub fn all_nodes(&self) -> Vec<Uuid> {
        let inner = self.inner.read();
        inner.graph.node_weights().copied().collect()
    }

    /// Exposes a read-only snapshot of the inner petgraph `DiGraph` and node index
    /// for use by [`crate::dag::traversal::DagTraversal`].
    pub(crate) fn with_inner<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&DiGraph<Uuid, EdgeMeta>, &HashMap<Uuid, NodeIndex>) -> T,
    {
        let inner = self.inner.read();
        f(&inner.graph, &inner.node_index)
    }
}

/// Inserts a node if absent and returns its index (internal helper, no locking).
fn ensure_or_insert(inner: &mut DagInner, id: Uuid) -> NodeIndex {
    if let Some(&idx) = inner.node_index.get(&id) {
        idx
    } else {
        let idx = inner.graph.add_node(id);
        inner.node_index.insert(id, idx);
        idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_config() -> Arc<BranchConfig> {
        Arc::new(BranchConfig::default_for_workspace(
            Uuid::new_v4(),
            Path::new("/tmp"),
        ))
    }

    #[test]
    fn test_add_node_idempotent() -> BranchResult<()> {
        let g = DagGraph::new(test_config());
        let id = Uuid::new_v4();
        let idx1 = g.add_node(id)?;
        let idx2 = g.add_node(id)?;
        assert_eq!(idx1, idx2);
        assert_eq!(g.node_count(), 1);
        Ok(())
    }

    #[test]
    fn test_cycle_detection_linear() -> BranchResult<()> {
        let g = DagGraph::new(test_config());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(a, b)?;
        g.add_edge(b, c)?;
        let result = g.add_edge(c, a);
        assert!(
            matches!(result, Err(BranchError::DagCycle { .. })),
            "c→a should be rejected as it would cycle"
        );
        // Original edges must still be intact.
        assert_eq!(g.children_of(a)?, vec![b]);
        Ok(())
    }

    #[test]
    fn test_self_edge_rejected() -> BranchResult<()> {
        let g = DagGraph::new(test_config());
        let a = Uuid::new_v4();
        let result = g.add_edge(a, a);
        assert!(matches!(result, Err(BranchError::DagCycle { .. })));
        Ok(())
    }

    #[test]
    fn test_depth_linear() -> BranchResult<()> {
        let g = DagGraph::new(test_config());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(a, b)?;
        g.add_edge(b, c)?;
        assert_eq!(g.depth_of(a)?, 0);
        assert_eq!(g.depth_of(b)?, 1);
        assert_eq!(g.depth_of(c)?, 2);
        Ok(())
    }

    #[test]
    fn test_remove_node_with_children_fails() -> BranchResult<()> {
        let g = DagGraph::new(test_config());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        g.add_edge(a, b)?;
        let result = g.remove_node(a);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_ancestors_of() -> BranchResult<()> {
        let g = DagGraph::new(test_config());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(a, b)?;
        g.add_edge(b, c)?;
        let ancestors = g.ancestors_of(c)?;
        assert!(ancestors.contains(&a));
        assert!(ancestors.contains(&b));
        assert!(!ancestors.contains(&c));
        Ok(())
    }

    #[test]
    fn test_find_merge_base() -> BranchResult<()> {
        let g = DagGraph::new(test_config());
        let root = Uuid::new_v4();
        let left = Uuid::new_v4();
        let right = Uuid::new_v4();
        g.add_edge(root, left)?;
        g.add_edge(root, right)?;
        assert_eq!(g.find_merge_base(left, right)?, Some(root));
        Ok(())
    }
}
