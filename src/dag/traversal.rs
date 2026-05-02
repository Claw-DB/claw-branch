//! DAG traversal algorithms: topological sort, path enumeration, LCA, and more.

use std::collections::{HashMap, HashSet, VecDeque};

use petgraph::{algo::toposort, graph::NodeIndex, Direction};
use uuid::Uuid;

use crate::{
    dag::graph::{DagGraph, EdgeMeta},
    error::{BranchError, BranchResult},
};

/// Provides read-only traversal algorithms over a [`DagGraph`].
pub struct DagTraversal<'a> {
    graph: &'a DagGraph,
}

impl<'a> DagTraversal<'a> {
    /// Creates a new traversal handle borrowing the given graph.
    pub fn new(graph: &'a DagGraph) -> Self {
        Self { graph }
    }

    /// Returns all branch UUIDs in a valid topological order (ancestors before descendants).
    ///
    /// Returns [`BranchError::DagCycle`] if the graph is unexpectedly cyclic.
    pub fn topological_sort(&self) -> BranchResult<Vec<Uuid>> {
        self.graph.with_inner(|g, _idx| {
            toposort(g, None)
                .map(|order| order.into_iter().map(|n| g[n]).collect())
                .map_err(|cycle| BranchError::DagCycle {
                    from: g[cycle.node_id()],
                    to: g[cycle.node_id()],
                })
        })
    }

    /// Enumerates every simple directed path from `from` to `to`.
    ///
    /// Returns an empty `Vec` when no path exists.
    pub fn all_paths(&self, from: Uuid, to: Uuid) -> BranchResult<Vec<Vec<Uuid>>> {
        self.graph.with_inner(|g, idx| {
            let from_n = *idx.get(&from).ok_or(BranchError::DagNodeNotFound(from))?;
            let to_n = *idx.get(&to).ok_or(BranchError::DagNodeNotFound(to))?;
            let mut results: Vec<Vec<NodeIndex>> = Vec::new();
            let mut path = vec![from_n];
            let mut visited = HashSet::new();
            visited.insert(from_n);
            dfs_paths(g, from_n, to_n, &mut visited, &mut path, &mut results);
            Ok(results
                .into_iter()
                .map(|p| p.into_iter().map(|n| g[n]).collect())
                .collect())
        })
    }

    /// Returns the lowest common ancestor of `a` and `b`.
    ///
    /// The LCA is the shared ancestor with the greatest depth (hops from root).
    /// Ties are broken by ascending UUID lexicographic order (smaller UUID wins).
    ///
    /// Returns `None` when the two nodes share no common ancestor.
    pub fn lowest_common_ancestor(&self, a: Uuid, b: Uuid) -> BranchResult<Option<Uuid>> {
        if a == b {
            return Ok(Some(a));
        }
        self.graph.with_inner(|g, idx| {
            let depth_map = bfs_depth_map(g);
            let a_ancestors = dfs_ancestor_set(g, idx, a)?;
            let b_ancestors = dfs_ancestor_set(g, idx, b)?;

            let mut a_set = a_ancestors;
            a_set.insert(a);
            let mut b_set = b_ancestors;
            b_set.insert(b);

            let common: Vec<Uuid> = a_set.intersection(&b_set).copied().collect();
            if common.is_empty() {
                return Ok(None);
            }
            // deepest node wins; tie-break: smaller UUID
            let lca = common.into_iter().max_by(|&x, &y| {
                let dx = depth_map.get(&x).copied().unwrap_or(0);
                let dy = depth_map.get(&y).copied().unwrap_or(0);
                match dx.cmp(&dy) {
                    std::cmp::Ordering::Equal => y.cmp(&x), // smaller UUID wins in max_by
                    other => other,
                }
            });
            Ok(lca)
        })
    }

    /// Returns all descendants of `root` (BFS, not including `root` itself).
    pub fn subtree_of(&self, root: Uuid) -> BranchResult<Vec<Uuid>> {
        self.graph.with_inner(|g, idx| {
            let root_n = *idx.get(&root).ok_or(BranchError::DagNodeNotFound(root))?;
            let mut result = Vec::new();
            let mut queue: VecDeque<NodeIndex> = VecDeque::new();
            let mut visited = HashSet::new();
            visited.insert(root_n);
            queue.push_back(root_n);
            while let Some(node) = queue.pop_front() {
                for child in g.neighbors_directed(node, Direction::Outgoing) {
                    if visited.insert(child) {
                        result.push(g[child]);
                        queue.push_back(child);
                    }
                }
            }
            Ok(result)
        })
    }

    /// Returns all leaf nodes (branches with no children, i.e. out-degree 0).
    pub fn leaves(&self) -> BranchResult<Vec<Uuid>> {
        self.graph.with_inner(|g, _| {
            Ok(g.node_indices()
                .filter(|&n| g.neighbors_directed(n, Direction::Outgoing).count() == 0)
                .map(|n| g[n])
                .collect())
        })
    }

    /// Returns the hop distance between `branch_a` and its LCA with `branch_b`.
    ///
    /// Intuitively measures how far `branch_a` has diverged from the common lineage point.
    pub fn divergence_depth(&self, branch_a: Uuid, branch_b: Uuid) -> BranchResult<usize> {
        let lca = self
            .lowest_common_ancestor(branch_a, branch_b)?
            .ok_or(BranchError::DagNodeNotFound(branch_a))?;
        let depth_a = self.graph.depth_of(branch_a)?;
        let depth_lca = self.graph.depth_of(lca)?;
        Ok(depth_a.saturating_sub(depth_lca))
    }

    /// Returns the given `branches` in topological order (subset of full topological sort).
    pub fn merge_order(&self, branches: &[Uuid]) -> BranchResult<Vec<Uuid>> {
        let full_order = self.topological_sort()?;
        let want: HashSet<Uuid> = branches.iter().copied().collect();
        Ok(full_order.into_iter().filter(|id| want.contains(id)).collect())
    }
}

// ── Internal helpers ────────────────────────────────────────────────────────

fn dfs_paths(
    g: &petgraph::Graph<Uuid, EdgeMeta>,
    current: NodeIndex,
    target: NodeIndex,
    visited: &mut HashSet<NodeIndex>,
    path: &mut Vec<NodeIndex>,
    results: &mut Vec<Vec<NodeIndex>>,
) {
    if current == target {
        results.push(path.clone());
        return;
    }
    for neighbor in g.neighbors_directed(current, Direction::Outgoing) {
        if !visited.contains(&neighbor) {
            visited.insert(neighbor);
            path.push(neighbor);
            dfs_paths(g, neighbor, target, visited, path, results);
            path.pop();
            visited.remove(&neighbor);
        }
    }
}

fn bfs_depth_map(g: &petgraph::Graph<Uuid, EdgeMeta>) -> HashMap<Uuid, usize> {
    let mut map: HashMap<Uuid, usize> = HashMap::new();
    let mut queue: VecDeque<NodeIndex> = VecDeque::new();
    for n in g.node_indices() {
        if g.neighbors_directed(n, Direction::Incoming).count() == 0 {
            map.insert(g[n], 0);
            queue.push_back(n);
        }
    }
    while let Some(node) = queue.pop_front() {
        let d = map[&g[node]];
        for child in g.neighbors_directed(node, Direction::Outgoing) {
            if !map.contains_key(&g[child]) {
                map.insert(g[child], d + 1);
                queue.push_back(child);
            }
        }
    }
    map
}

fn dfs_ancestor_set(
    g: &petgraph::Graph<Uuid, EdgeMeta>,
    idx: &HashMap<Uuid, NodeIndex>,
    id: Uuid,
) -> BranchResult<HashSet<Uuid>> {
    let start = *idx.get(&id).ok_or(BranchError::DagNodeNotFound(id))?;
    let mut visited = HashSet::new();
    let mut stack = vec![start];
    while let Some(node) = stack.pop() {
        for parent in g.neighbors_directed(node, Direction::Incoming) {
            if visited.insert(g[parent]) {
                stack.push(parent);
            }
        }
    }
    Ok(visited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::Path, sync::Arc};
    use crate::config::BranchConfig;

    fn cfg() -> Arc<BranchConfig> {
        Arc::new(BranchConfig::default_for_workspace(
            Uuid::new_v4(),
            Path::new("/tmp"),
        ))
    }

    fn linear_graph(n: usize) -> (DagGraph, Vec<Uuid>) {
        let ids: Vec<Uuid> = (0..n).map(|_| Uuid::new_v4()).collect();
        let g = DagGraph::new(cfg());
        for w in ids.windows(2) {
            g.add_edge(w[0], w[1]).ok();
        }
        (g, ids)
    }

    #[test]
    fn test_topo_sort_linear() -> BranchResult<()> {
        let (g, ids) = linear_graph(4);
        let t = DagTraversal::new(&g);
        let order = t.topological_sort()?;
        for id in &ids {
            assert!(order.contains(id));
        }
        for w in ids.windows(2) {
            let pp = order.iter().position(|x| x == &w[0]).ok_or(BranchError::DagNodeNotFound(w[0]))?;
            let pc = order.iter().position(|x| x == &w[1]).ok_or(BranchError::DagNodeNotFound(w[1]))?;
            assert!(pp < pc);
        }
        Ok(())
    }

    #[test]
    fn test_topo_sort_fork() -> BranchResult<()> {
        let g = DagGraph::new(cfg());
        let root = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(root, b)?;
        g.add_edge(root, c)?;
        let t = DagTraversal::new(&g);
        let order = t.topological_sort()?;
        let pr = order.iter().position(|x| x == &root).ok_or(BranchError::DagNodeNotFound(root))?;
        let pb = order.iter().position(|x| x == &b).ok_or(BranchError::DagNodeNotFound(b))?;
        let pc = order.iter().position(|x| x == &c).ok_or(BranchError::DagNodeNotFound(c))?;
        assert!(pr < pb && pr < pc);
        Ok(())
    }

    #[test]
    fn test_topo_sort_single_node() -> BranchResult<()> {
        let g = DagGraph::new(cfg());
        let a = Uuid::new_v4();
        g.add_node(a)?;
        let t = DagTraversal::new(&g);
        assert_eq!(t.topological_sort()?, vec![a]);
        Ok(())
    }

    #[test]
    fn test_leaves_linear() -> BranchResult<()> {
        let (g, ids) = linear_graph(4);
        let t = DagTraversal::new(&g);
        let leaves = t.leaves()?;
        assert_eq!(leaves, vec![*ids.last().ok_or(BranchError::DagNodeNotFound(Uuid::nil()))?]);
        Ok(())
    }

    #[test]
    fn test_leaves_fork() -> BranchResult<()> {
        let g = DagGraph::new(cfg());
        let root = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(root, b)?;
        g.add_edge(root, c)?;
        let t = DagTraversal::new(&g);
        let mut leaves = t.leaves()?;
        leaves.sort();
        let mut expected = vec![b, c];
        expected.sort();
        assert_eq!(leaves, expected);
        Ok(())
    }

    #[test]
    fn test_lca_same_node() -> BranchResult<()> {
        let (g, ids) = linear_graph(3);
        let t = DagTraversal::new(&g);
        assert_eq!(t.lowest_common_ancestor(ids[1], ids[1])?, Some(ids[1]));
        Ok(())
    }

    #[test]
    fn test_lca_one_is_ancestor() -> BranchResult<()> {
        let (g, ids) = linear_graph(3);
        let t = DagTraversal::new(&g);
        assert_eq!(t.lowest_common_ancestor(ids[0], ids[2])?, Some(ids[0]));
        assert_eq!(t.lowest_common_ancestor(ids[2], ids[0])?, Some(ids[0]));
        Ok(())
    }

    #[test]
    fn test_lca_simple_fork() -> BranchResult<()> {
        let g = DagGraph::new(cfg());
        let root = Uuid::new_v4();
        let left = Uuid::new_v4();
        let right = Uuid::new_v4();
        g.add_edge(root, left)?;
        g.add_edge(root, right)?;
        let t = DagTraversal::new(&g);
        assert_eq!(t.lowest_common_ancestor(left, right)?, Some(root));
        Ok(())
    }

    #[test]
    fn test_lca_five_level_tree() -> BranchResult<()> {
        // root → a → b → c → d
        //              └─────→ e
        // lca(d, e) = b
        let g = DagGraph::new(cfg());
        let root = Uuid::new_v4();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let d = Uuid::new_v4();
        let e = Uuid::new_v4();
        g.add_edge(root, a)?;
        g.add_edge(a, b)?;
        g.add_edge(b, c)?;
        g.add_edge(c, d)?;
        g.add_edge(b, e)?;
        let t = DagTraversal::new(&g);
        assert_eq!(t.lowest_common_ancestor(d, e)?, Some(b));
        Ok(())
    }

    #[test]
    fn test_lca_no_common_ancestor() -> BranchResult<()> {
        let g = DagGraph::new(cfg());
        let x = Uuid::new_v4();
        let y = Uuid::new_v4();
        g.add_node(x)?;
        g.add_node(y)?;
        let t = DagTraversal::new(&g);
        assert_eq!(t.lowest_common_ancestor(x, y)?, None);
        Ok(())
    }

    #[test]
    fn test_all_paths_single() -> BranchResult<()> {
        let (g, ids) = linear_graph(3);
        let t = DagTraversal::new(&g);
        let paths = t.all_paths(ids[0], ids[2])?;
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], vec![ids[0], ids[1], ids[2]]);
        Ok(())
    }

    #[test]
    fn test_all_paths_fork_two_paths() -> BranchResult<()> {
        let g = DagGraph::new(cfg());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let d = Uuid::new_v4();
        g.add_edge(a, b)?;
        g.add_edge(a, c)?;
        g.add_edge(b, d)?;
        g.add_edge(c, d)?;
        let t = DagTraversal::new(&g);
        let paths = t.all_paths(a, d)?;
        assert_eq!(paths.len(), 2);
        Ok(())
    }

    #[test]
    fn test_no_paths_between_siblings() -> BranchResult<()> {
        let g = DagGraph::new(cfg());
        let root = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(root, b)?;
        g.add_edge(root, c)?;
        let t = DagTraversal::new(&g);
        assert!(t.all_paths(b, c)?.is_empty());
        Ok(())
    }

    #[test]
    fn test_subtree_of_root() -> BranchResult<()> {
        let g = DagGraph::new(cfg());
        let root = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(root, b)?;
        g.add_edge(b, c)?;
        let t = DagTraversal::new(&g);
        let mut sub = t.subtree_of(root)?;
        sub.sort();
        let mut expected = vec![b, c];
        expected.sort();
        assert_eq!(sub, expected);
        Ok(())
    }

    #[test]
    fn test_subtree_of_leaf_is_empty() -> BranchResult<()> {
        let (g, ids) = linear_graph(3);
        let t = DagTraversal::new(&g);
        let last = *ids.last().ok_or(BranchError::DagNodeNotFound(Uuid::nil()))?;
        assert!(t.subtree_of(last)?.is_empty());
        Ok(())
    }

    #[test]
    fn test_divergence_depth() -> BranchResult<()> {
        let g = DagGraph::new(cfg());
        let root = Uuid::new_v4();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(root, a)?;
        g.add_edge(a, b)?;
        g.add_edge(a, c)?;
        let t = DagTraversal::new(&g);
        assert_eq!(t.divergence_depth(b, c)?, 1);
        Ok(())
    }

    #[test]
    fn test_merge_order_subset() -> BranchResult<()> {
        let (g, ids) = linear_graph(5);
        let t = DagTraversal::new(&g);
        let subset = vec![ids[4], ids[2], ids[0]];
        let order = t.merge_order(&subset)?;
        assert_eq!(order, vec![ids[0], ids[2], ids[4]]);
        Ok(())
    }
}