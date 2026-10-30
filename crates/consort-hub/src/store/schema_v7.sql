-- Version 7: attachments reach the sessions.
--
-- A message for a task that waits in the inbox keeps its attachments, as a
-- JSON list of blob references.
ALTER TABLE task_inbox ADD COLUMN attachments TEXT NOT NULL DEFAULT '[]';

-- The attachments that a coordinator gave with start_task. They go with the
-- first turn of each session of the task, as the brief does.
CREATE TABLE task_attachments (
    task_id     TEXT PRIMARY KEY,
    attachments TEXT NOT NULL
);

-- A digest of the channel file list that each session got last, with
-- session.start or session.turn. A session.turn carries the list again only
-- when the digest changed.
CREATE TABLE session_channel_files (
    session_id TEXT PRIMARY KEY,
    digest     TEXT NOT NULL
);
