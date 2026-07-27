PRAGMA foreign_keys = OFF;

CREATE TABLE threads (
    id TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    tokens_used INTEGER NOT NULL
);

INSERT INTO threads (id, created_at, tokens_used)
VALUES ('fixture-minimal', 1, 1);
