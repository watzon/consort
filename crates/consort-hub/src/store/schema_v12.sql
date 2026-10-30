-- Version 12: the review fixes of "Write in my voice".
--
-- A hub that ran a build from the time when version 10 was an empty
-- migration is at version 11 without the voice tables. This migration
-- makes them when they are missing, and does nothing to a hub that has
-- them.
CREATE TABLE IF NOT EXISTS draft_pairs (
    approval_id TEXT PRIMARY KEY,
    context     TEXT,
    unchanged   INTEGER NOT NULL,
    used        INTEGER NOT NULL DEFAULT 0,
    at          INTEGER NOT NULL,
    data        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS draft_pairs_at ON draft_pairs (at);
CREATE INDEX IF NOT EXISTS draft_pairs_context ON draft_pairs (context, unchanged, used);

CREATE TABLE IF NOT EXISTS voice_uses (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    context    TEXT NOT NULL,
    at         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS voice_uses_session ON voice_uses (session_id, id);

-- Every task that learns the voice. "Is a voice task" is a mark on the
-- task that stays after Learn again, so an old voice task keeps its
-- limits. `at` is when it started; its brief is cut after 90 days.
CREATE TABLE IF NOT EXISTS voice_tasks (
    task_id TEXT PRIMARY KEY,
    at      INTEGER NOT NULL
);
INSERT OR IGNORE INTO voice_tasks (task_id, at)
    SELECT json_extract(value, '$.task_id'), CAST(strftime('%s', 'now') AS INTEGER) * 1000
    FROM meta
    WHERE key = 'voice' AND json_extract(value, '$.task_id') IS NOT NULL;

-- The user's own words that started a task: the user messages and answers
-- of the coordinator turn that started it. The voice scan compares the
-- task's drafts with them, never with the brief that a model wrote.
CREATE TABLE IF NOT EXISTS task_requests (
    task_id TEXT PRIMARY KEY,
    text    TEXT NOT NULL
);
