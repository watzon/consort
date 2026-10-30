-- The review fixes of the proactive assistant.
--
-- proactive_tasks: the tasks that a proactive turn started. A task update of
-- such a task is proactive, a task in draft mode asks before each connector
-- call that writes or sends, and a proactive turn may steer or retry only a
-- draft task of its own channel.
CREATE TABLE proactive_tasks (
    task_id    TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    kind       TEXT NOT NULL,
    draft      INTEGER NOT NULL
);
CREATE INDEX proactive_tasks_channel ON proactive_tasks (channel_id);

-- A follow-up that a proactive turn set. The daily turn budget can skip it,
-- so the assistant cannot chain follow-ups without an end.
ALTER TABLE follow_ups ADD COLUMN set_in_proactive_turn INTEGER NOT NULL DEFAULT 0;
