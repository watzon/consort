-- Version 5: connectors and skills (ADR 0011).
--
-- `connections`: one row for each Composio connected account. `data` is the
-- `Connection` JSON. The Composio IDs stay in their own columns, because
-- the client view does not show them. The row never holds a credential.
CREATE TABLE connections (
    id                  TEXT PRIMARY KEY,
    handle              TEXT NOT NULL UNIQUE,
    composio_account_id TEXT,
    auth_config_id      TEXT,
    auth_scheme         TEXT,
    data                TEXT NOT NULL
);

-- `connector_calls`: the audit log. `data` is the `ConnectorCall` JSON. It
-- has a hash of the arguments, never the arguments or the result.
CREATE TABLE connector_calls (
    id            TEXT PRIMARY KEY,
    connection_id TEXT NOT NULL,
    at            INTEGER NOT NULL,
    data          TEXT NOT NULL
);

CREATE INDEX connector_calls_connection ON connector_calls (connection_id, at);

-- `connector_grants`: "ask once per task" answers. `scope` is a task ID, or
-- a session ID for a coordinator.
CREATE TABLE connector_grants (
    scope         TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    kind          TEXT NOT NULL,
    granted_at    INTEGER NOT NULL,
    PRIMARY KEY (scope, connection_id, kind)
);

-- `skills`: one row for each installed skill. `data` is the `Skill` JSON.
CREATE TABLE skills (
    id   TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    data TEXT NOT NULL
);

-- `skill_versions`: the file list of each skill version, by content hash.
-- The files are blobs. The hub keeps every version that a session may
-- still use.
CREATE TABLE skill_versions (
    content_hash TEXT PRIMARY KEY,
    skill_id     TEXT NOT NULL,
    name         TEXT NOT NULL,
    files        TEXT NOT NULL,
    created_at   INTEGER NOT NULL
);

CREATE INDEX skill_versions_skill ON skill_versions (skill_id);

-- `skill_bundle_grants`: the bundles that a `session.start` named to a
-- worker. `skill_bundle.get` answers only for these.
CREATE TABLE skill_bundle_grants (
    worker_id    TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    granted_at   INTEGER NOT NULL,
    PRIMARY KEY (worker_id, content_hash)
);
