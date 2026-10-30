-- Version 8: routine runs with a status, and memory notes that can come
-- back from Recently deleted.
--
-- A run is 'running' until its work ends, then 'ok' or 'error'. A run that
-- the hub did not start is 'skipped'. Runs from before this version ended
-- when they started.
ALTER TABLE routine_runs ADD COLUMN channel_id TEXT NOT NULL DEFAULT '';
ALTER TABLE routine_runs ADD COLUMN status TEXT NOT NULL DEFAULT 'ok';
ALTER TABLE routine_runs ADD COLUMN ended_at INTEGER;
ALTER TABLE routine_runs ADD COLUMN manual INTEGER NOT NULL DEFAULT 0;
UPDATE routine_runs
   SET status = CASE WHEN error IS NULL THEN 'ok' ELSE 'error' END,
       ended_at = started_at,
       channel_id = COALESCE(
           (SELECT channel_id FROM routines WHERE routines.id = routine_runs.routine_id), '');
CREATE INDEX routine_runs_running ON routine_runs (routine_id) WHERE status = 'running';
CREATE INDEX routine_runs_task ON routine_runs (task_id) WHERE task_id IS NOT NULL;

-- The task or channel whose memory suggestion a version keeps.
ALTER TABLE memory_notes ADD COLUMN suggested_by TEXT;
ALTER TABLE memory_note_versions ADD COLUMN suggested_by TEXT;
-- A deleted note keeps its row for 30 days. Sessions and lists skip it.
ALTER TABLE memory_notes ADD COLUMN deleted_at INTEGER;
