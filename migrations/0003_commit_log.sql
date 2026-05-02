CREATE TABLE IF NOT EXISTS branch_commits (
    id TEXT PRIMARY KEY,
    branch_id TEXT NOT NULL,
    entity_type TEXT,
    entity_ids TEXT,
    op_kind TEXT NOT NULL,
    committed_at TEXT NOT NULL,
    message TEXT,
    FOREIGN KEY(branch_id) REFERENCES branches(id)
);