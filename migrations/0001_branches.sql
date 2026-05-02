CREATE TABLE IF NOT EXISTS branches (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    slug TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    parent_id TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    db_path TEXT NOT NULL,
    snapshot_path TEXT NOT NULL,
    forked_from_cursor TEXT,
    description TEXT,
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(workspace_id, slug)
);