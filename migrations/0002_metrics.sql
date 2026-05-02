CREATE TABLE IF NOT EXISTS branch_metrics (
    branch_id TEXT PRIMARY KEY REFERENCES branches(id),
    op_count INTEGER NOT NULL DEFAULT 0,
    memory_record_count INTEGER NOT NULL DEFAULT 0,
    session_count INTEGER NOT NULL DEFAULT 0,
    tool_output_count INTEGER NOT NULL DEFAULT 0,
    bytes_on_disk INTEGER NOT NULL DEFAULT 0,
    divergence_score REAL NOT NULL DEFAULT 0.0,
    created_entity_count INTEGER NOT NULL DEFAULT 0,
    updated_entity_count INTEGER NOT NULL DEFAULT 0,
    deleted_entity_count INTEGER NOT NULL DEFAULT 0,
    last_activity_at TEXT,
    refreshed_at TEXT NOT NULL
);