-- Version 9: the proactive assistant.
--
-- The proactivity settings are one JSON record in `meta` under the key
-- `proactivity`, like the preferences.

-- The proactivity log: every proactive wake and its outcome. The hub keeps
-- 30 days. `notified` counts the notifying messages of a wake, for the
-- daily limit.
CREATE TABLE proactive_wakes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id TEXT NOT NULL,
    kind       TEXT NOT NULL,
    at         INTEGER NOT NULL,
    outcome    TEXT NOT NULL,
    notified   INTEGER NOT NULL DEFAULT 0,
    data       TEXT NOT NULL
);
CREATE INDEX proactive_wakes_channel ON proactive_wakes (channel_id, at);
CREATE INDEX proactive_wakes_at ON proactive_wakes (at);

-- Per channel: when the last check-in looked at the hub, the silent
-- check-ins in a row, and the interval before back-off doubled it.
CREATE TABLE proactive_channels (
    channel_id       TEXT PRIMARY KEY,
    last_check_in_at INTEGER,
    silent_streak    INTEGER NOT NULL DEFAULT 0,
    base_seconds     INTEGER
);

-- When each task entered its status, for stale-task nudges. `nudged` is
-- set when the hub woke the coordinator for this status entry or skipped
-- it for good. `held_logged` is set when the hub wrote a skipped row for
-- a nudge that waits for the end of a snooze.
CREATE TABLE task_status_since (
    task_id     TEXT PRIMARY KEY,
    status      TEXT NOT NULL,
    since       INTEGER NOT NULL,
    nudged      INTEGER NOT NULL DEFAULT 0,
    held_logged INTEGER NOT NULL DEFAULT 0
);

-- The follow-ups that the assistant set: their key and when it set them.
CREATE TABLE follow_ups (
    routine_id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    key        TEXT,
    set_at     INTEGER NOT NULL
);
CREATE UNIQUE INDEX follow_ups_key ON follow_ups (channel_id, key) WHERE key IS NOT NULL;
