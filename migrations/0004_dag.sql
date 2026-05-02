-- DAG node and edge persistence for branch lineage tracking.

CREATE TABLE IF NOT EXISTS dag_nodes (
    branch_id   TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    added_at    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS dag_edges (
    parent_id    TEXT NOT NULL,
    child_id     TEXT NOT NULL,
    forked_at    TEXT NOT NULL,
    merge_cursor TEXT,
    PRIMARY KEY (parent_id, child_id),
    FOREIGN KEY (parent_id) REFERENCES dag_nodes(branch_id),
    FOREIGN KEY (child_id)  REFERENCES dag_nodes(branch_id)
);

CREATE INDEX IF NOT EXISTS idx_dag_nodes_workspace ON dag_nodes(workspace_id);
CREATE INDEX IF NOT EXISTS idx_dag_edges_parent    ON dag_edges(parent_id);
CREATE INDEX IF NOT EXISTS idx_dag_edges_child     ON dag_edges(child_id);
