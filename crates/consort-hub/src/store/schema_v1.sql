-- Schema version 1 of hub.db.
--
-- Most entities are stored as the JSON of their client view in `data`, next
-- to the columns that the hub queries and indexes. A reader ignores unknown
-- fields and fills missing ones, so an entity can gain a field without a
-- migration. Private hub state (public keys excepted) has real columns.

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- The feed. `seq` has no gaps because the single writer inserts one row for
-- each change in the same transaction as the change.
CREATE TABLE feed (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    at         INTEGER NOT NULL,
    channel_id TEXT,
    entity     TEXT NOT NULL,
    entity_id  TEXT NOT NULL,
    op         TEXT NOT NULL,
    payload    TEXT
);
CREATE INDEX feed_at ON feed (at);

CREATE TABLE groups (
    id       TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    data     TEXT NOT NULL
);

CREATE TABLE channels (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    group_id    TEXT,
    position    INTEGER NOT NULL,
    archived_at INTEGER,
    data        TEXT NOT NULL
);
CREATE UNIQUE INDEX channels_one_everything ON channels (kind) WHERE kind = 'everything';

CREATE TABLE repos (
    id      TEXT PRIMARY KEY,
    url_key TEXT NOT NULL UNIQUE,
    data    TEXT NOT NULL
);

-- Messages keep their insertion order in the implicit rowid.
CREATE TABLE messages (
    id                TEXT PRIMARY KEY,
    channel_id        TEXT NOT NULL,
    thread_root_id    TEXT,
    in_timeline       INTEGER NOT NULL,
    created_at        INTEGER NOT NULL,
    origin_client_id  TEXT,
    client_message_id TEXT,
    deleted_at        INTEGER,
    data              TEXT NOT NULL
);
CREATE INDEX messages_channel ON messages (channel_id, thread_root_id);
CREATE UNIQUE INDEX messages_idempotency ON messages (origin_client_id, client_message_id)
    WHERE client_message_id IS NOT NULL;

CREATE TABLE read_markers (
    channel_id     TEXT NOT NULL,
    thread_root_id TEXT NOT NULL DEFAULT '',
    data           TEXT NOT NULL,
    PRIMARY KEY (channel_id, thread_root_id)
);

CREATE TABLE tasks (
    id          TEXT PRIMARY KEY,
    channel_id  TEXT NOT NULL,
    status      TEXT NOT NULL,
    card_id     TEXT NOT NULL,
    session_id  TEXT,
    start_at    INTEGER,
    archived_at INTEGER,
    data        TEXT NOT NULL
);
CREATE INDEX tasks_channel ON tasks (channel_id);
CREATE INDEX tasks_status ON tasks (status);
CREATE INDEX tasks_card ON tasks (card_id);

CREATE TABLE sessions (
    id         TEXT PRIMARY KEY,
    role       TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    task_id    TEXT,
    worker_id  TEXT NOT NULL,
    state      TEXT NOT NULL,
    last_seq   INTEGER NOT NULL DEFAULT 0,
    data       TEXT NOT NULL
);
CREATE INDEX sessions_worker ON sessions (worker_id, state);
CREATE INDEX sessions_channel ON sessions (channel_id, role);

-- Hub-private state of a session.
CREATE TABLE session_runtime (
    session_id          TEXT PRIMARY KEY,
    turn_id             TEXT,
    turn_open           INTEGER NOT NULL DEFAULT 0,
    last_assistant_text TEXT,
    tools_called        TEXT NOT NULL DEFAULT '[]',
    review_summary      TEXT,
    prompt_fingerprint  TEXT
);

CREATE TABLE session_items (
    session_id TEXT NOT NULL,
    item_id    TEXT NOT NULL,
    data       TEXT NOT NULL,
    PRIMARY KEY (session_id, item_id)
);

CREATE TABLE approvals (
    id         TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    task_id    TEXT,
    request_id TEXT NOT NULL,
    status     TEXT NOT NULL,
    data       TEXT NOT NULL
);
CREATE INDEX approvals_session ON approvals (session_id, request_id);
CREATE INDEX approvals_status ON approvals (status);

CREATE TABLE decisions (
    id         TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    task_id    TEXT,
    request_id TEXT,
    status     TEXT NOT NULL,
    data       TEXT NOT NULL
);
CREATE INDEX decisions_session ON decisions (session_id, request_id);
CREATE INDEX decisions_status ON decisions (status);

CREATE TABLE routines (
    id          TEXT PRIMARY KEY,
    channel_id  TEXT NOT NULL,
    enabled     INTEGER NOT NULL,
    next_run_at INTEGER,
    data        TEXT NOT NULL
);
CREATE INDEX routines_due ON routines (enabled, next_run_at);

CREATE TABLE routine_runs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    routine_id TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    message_id TEXT,
    task_id    TEXT,
    error      TEXT
);
CREATE INDEX routine_runs_routine ON routine_runs (routine_id, id);

CREATE TABLE memory_notes (
    scope      TEXT NOT NULL,
    path       TEXT NOT NULL,
    content    TEXT NOT NULL,
    sha256     TEXT NOT NULL,
    version    INTEGER NOT NULL,
    updated_by TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (scope, path)
);

CREATE TABLE memory_note_versions (
    scope      TEXT NOT NULL,
    path       TEXT NOT NULL,
    version    INTEGER NOT NULL,
    content    TEXT NOT NULL,
    sha256     TEXT NOT NULL,
    updated_by TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (scope, path, version)
);

CREATE TABLE channel_files (
    channel_id TEXT NOT NULL,
    path       TEXT NOT NULL,
    data       TEXT NOT NULL,
    PRIMARY KEY (channel_id, path)
);

CREATE TABLE workers (
    id         TEXT PRIMARY KEY,
    revoked_at INTEGER,
    data       TEXT NOT NULL
);

CREATE TABLE clients (
    id            TEXT PRIMARY KEY,
    revoked_at    INTEGER,
    push_provider TEXT,
    push_token    TEXT,
    data          TEXT NOT NULL
);

-- `kind` is `worker` or `client`. The code itself is never stored.
CREATE TABLE pairing_codes (
    code_hash  TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at    INTEGER
);

-- Commands for workers. A command stays here until the worker answers it.
CREATE TABLE commands (
    id         TEXT PRIMARY KEY,
    worker_id  TEXT NOT NULL,
    session_id TEXT,
    kind       TEXT NOT NULL,
    body       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX commands_worker ON commands (worker_id);

CREATE TABLE wake_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id TEXT NOT NULL,
    event      TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX wake_events_channel ON wake_events (channel_id, id);

CREATE TABLE coordinators (
    channel_id      TEXT PRIMARY KEY,
    session_id      TEXT,
    turn_open       INTEGER NOT NULL DEFAULT 0,
    turn_started_at INTEGER,
    turn_events     TEXT NOT NULL DEFAULT '[]'
);

-- Full-text index over messages, memory notes and task titles.
CREATE VIRTUAL TABLE search_idx USING fts5(
    kind UNINDEXED,
    ref UNINDEXED,
    channel_id UNINDEXED,
    scope UNINDEXED,
    title,
    body,
    tokenize = 'porter unicode61'
);

-- User messages for a task session that the hub holds until the current turn
-- ends (the harness cannot take a steering message).
CREATE TABLE task_inbox (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id    TEXT NOT NULL,
    text       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX task_inbox_task ON task_inbox (task_id, id);
