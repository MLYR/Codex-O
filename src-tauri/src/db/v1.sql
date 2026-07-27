CREATE TABLE providers (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    root_path TEXT NOT NULL,
    display_name TEXT NOT NULL,
    read_only INTEGER NOT NULL CHECK (read_only IN (0, 1)),
    capabilities_json TEXT NOT NULL,
    last_scan_at INTEGER
);

CREATE TABLE skills (
    id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    display_name TEXT NOT NULL,
    scope TEXT NOT NULL,
    lifecycle_state TEXT NOT NULL,
    latest_snapshot_id TEXT,
    first_seen_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE RESTRICT,
    FOREIGN KEY (latest_snapshot_id) REFERENCES artifact_snapshots(id) ON DELETE SET NULL,
    UNIQUE (provider_id, relative_path)
);

CREATE TABLE artifact_snapshots (
    id TEXT PRIMARY KEY,
    skill_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    parser_version TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    resources_json TEXT NOT NULL,
    diagnostics_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (skill_id) REFERENCES skills(id) ON DELETE CASCADE
);

CREATE TABLE skill_analyses (
    id TEXT PRIMARY KEY,
    snapshot_id TEXT NOT NULL,
    analysis_key TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL,
    passport_json TEXT,
    evidence_json TEXT,
    provider TEXT,
    model TEXT,
    prompt_version TEXT NOT NULL,
    schema_version TEXT NOT NULL,
    language TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (snapshot_id) REFERENCES artifact_snapshots(id) ON DELETE CASCADE
);

CREATE INDEX artifact_snapshots_skill_id_idx ON artifact_snapshots(skill_id);
CREATE INDEX skill_analyses_snapshot_id_idx ON skill_analyses(snapshot_id);
