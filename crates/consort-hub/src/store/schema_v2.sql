-- Version 2: the stuck-turn check for task sessions, and the cleanup of
-- queued commands when the hub ends a session.

-- The last sign of life of a session: an event, or a turn that the hub
-- opened. Unix milliseconds.
ALTER TABLE session_runtime ADD COLUMN active_at INTEGER;

-- When the hub last sent a queued command to a worker. Unix milliseconds.
-- NULL means that the command never went out.
ALTER TABLE commands ADD COLUMN sent_at INTEGER;
CREATE INDEX commands_session ON commands (session_id);
