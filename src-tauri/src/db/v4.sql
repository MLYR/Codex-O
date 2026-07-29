-- Import provenance and operation outcomes stay in the app-local database, never in Codex state.
CREATE TABLE IF NOT EXISTS install_receipts (
    skill_id TEXT PRIMARY KEY,
    source_type TEXT NOT NULL,
    source_url TEXT,
    repo_ref TEXT,
    commit_sha TEXT,
    subdirectory TEXT,
    installed_hash TEXT NOT NULL,
    installed_at INTEGER NOT NULL,
    managed_by TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS management_operations (
    id TEXT PRIMARY KEY,
    skill_id TEXT,
    operation TEXT NOT NULL,
    status TEXT NOT NULL,
    plan_json TEXT NOT NULL,
    result_json TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER
);
