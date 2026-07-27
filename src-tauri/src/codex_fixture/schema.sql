PRAGMA foreign_keys = OFF;

CREATE TABLE threads (
    id TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    tokens_used INTEGER NOT NULL,
    title TEXT,
    cwd TEXT,
    source TEXT,
    model TEXT,
    model_provider TEXT,
    preview TEXT,
    first_user_message TEXT,
    rollout_path TEXT,
    archived INTEGER
);

CREATE TABLE thread_dynamic_tools (
    id INTEGER PRIMARY KEY,
    thread_id TEXT NOT NULL,
    tool_name TEXT NOT NULL
);

CREATE TABLE thread_spawn_edges (
    parent_thread_id TEXT NOT NULL,
    child_thread_id TEXT NOT NULL
);

INSERT INTO threads (
    id,
    created_at,
    tokens_used,
    title,
    cwd,
    source,
    model,
    model_provider,
    preview,
    first_user_message,
    rollout_path,
    archived
) VALUES (
    'fixture-parent',
    1,
    42,
    'fixture title',
    NULL,
    '{"kind":"subagent","origin":"fixture"}',
    'fixture-model',
    'fixture-provider',
    'fixture preview',
    'fixture user message',
    'sessions/fixture.jsonl',
    0
);

INSERT INTO threads (
    id,
    created_at,
    tokens_used,
    title,
    cwd,
    source,
    model,
    model_provider,
    preview,
    first_user_message,
    rollout_path,
    archived
) VALUES (
    'fixture-child',
    2,
    7,
    NULL,
    NULL,
    'cli',
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    0
);

INSERT INTO thread_dynamic_tools (id, thread_id, tool_name)
VALUES (1, 'fixture-parent', 'fixture-tool');

INSERT INTO thread_spawn_edges (parent_thread_id, child_thread_id)
VALUES ('fixture-parent', 'fixture-child');
