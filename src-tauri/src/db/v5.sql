-- Quarantine metadata is app-local and never stores absolute source or destination paths.
CREATE TABLE IF NOT EXISTS quarantine_entries (
    id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL UNIQUE,
    skill_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    original_relative_path TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    display_name TEXT NOT NULL,
    file_count INTEGER NOT NULL,
    total_size_bytes INTEGER NOT NULL,
    status TEXT NOT NULL,
    quarantined_at INTEGER NOT NULL,
    restored_at INTEGER
);
