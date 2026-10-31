-- Version 17: Monid calls (ADR 0016).
--
-- One row per call. `cost_micros` is what Monid charged, in millionths of a
-- dollar. The columns are for the totals. `data` holds the whole call, with
-- its input and output, because the user asked to see them.
CREATE TABLE monid_calls (
    id          TEXT PRIMARY KEY,
    channel_id  TEXT NOT NULL,
    task_id     TEXT,
    at          INTEGER NOT NULL,
    cost_micros INTEGER NOT NULL DEFAULT 0,
    state       TEXT NOT NULL,
    data        TEXT NOT NULL
);
CREATE INDEX monid_calls_channel ON monid_calls (channel_id, at);
CREATE INDEX monid_calls_task ON monid_calls (task_id);
CREATE INDEX monid_calls_at ON monid_calls (at);
CREATE INDEX monid_calls_state ON monid_calls (state);

-- A user denied this access request. The same request stops for a while.
CREATE TABLE access_denials (
    scope  TEXT NOT NULL,
    target TEXT NOT NULL,
    at     INTEGER NOT NULL,
    PRIMARY KEY (scope, target)
);

-- "Allow for this task": the tool runs with no card for the rest of the task
-- or the coordinator session.
CREATE TABLE monid_grants (
    scope    TEXT NOT NULL,
    provider TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    PRIMARY KEY (scope, provider, endpoint)
);
