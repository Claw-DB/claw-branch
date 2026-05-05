//! Graphviz DOT exporting for branch lineage graphs.

use crate::{
    branch::store::BranchStore, dag::graph::DagGraph, error::BranchResult, types::BranchStatus,
};

/// Exports the branch lineage graph as a Graphviz DOT string.
///
/// Nodes are labeled with branch name and status and colour-coded by lifecycle state.
/// Edges are labeled with the fork date.  The graph carries a label with the workspace ID.
pub async fn export_dot(graph: &DagGraph, store: &BranchStore) -> BranchResult<String> {
    let workspace_id = graph.config.workspace_id;
    let mut dot = String::from("digraph claw_branch_lineage {\n");
    dot.push_str(&format!(
        "    graph [label=\"workspace: {workspace_id}\" fontsize=12];\n"
    ));
    dot.push_str("    node [shape=box fontsize=10];\n\n");

    // Emit nodes.
    for node_id in graph.all_nodes() {
        let (label, color, fill_color) = match store.get(workspace_id, node_id).await {
            Ok(branch) => {
                let label = format!(
                    "{} ({})",
                    sanitize_dot_label(&branch.name),
                    branch.status.kind()
                );
                let (color, fill) = status_colors(&branch.status);
                (label, color, fill)
            }
            Err(_) => (
                node_id.to_string(),
                "black".to_string(),
                "white".to_string(),
            ),
        };
        dot.push_str(&format!(
            "    \"{node_id}\" [label=\"{label}\" color={color} style=filled fillcolor={fill_color}];\n"
        ));
    }

    dot.push('\n');

    // Emit edges with forked_at labels.
    for (parent_id, child_id, meta) in graph.all_edges() {
        let date_label = meta.forked_at.format("%Y-%m-%d").to_string();
        dot.push_str(&format!(
            "    \"{parent_id}\" -> \"{child_id}\" [label=\"{date_label}\"];\n"
        ));
    }

    dot.push_str("}\n");
    Ok(dot)
}

fn status_colors(status: &BranchStatus) -> (String, String) {
    match status {
        BranchStatus::Active => ("green".to_string(), "lightgreen".to_string()),
        BranchStatus::Dormant => ("goldenrod".to_string(), "lightyellow".to_string()),
        BranchStatus::Merged { .. } => ("blue".to_string(), "lightblue".to_string()),
        BranchStatus::Discarded { .. } => ("red".to_string(), "lightpink".to_string()),
        BranchStatus::Archived => ("gray".to_string(), "lightgray".to_string()),
        BranchStatus::Orphan => ("orange".to_string(), "moccasin".to_string()),
        BranchStatus::Purged => ("black".to_string(), "white".to_string()),
    }
}

fn sanitize_dot_label(s: &str) -> String {
    s.replace('"', "'").replace('\\', "/")
}
