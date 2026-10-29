-- Version 3: who made a pairing code, and who paired or removed a client or
-- a worker; the tasks whose workspace was deleted; and task turns that the
-- hub opened from Ready for review.

-- Who made the code: a client ID, or `hub` for a code from the hub machine.
-- NULL for codes from before version 3.
ALTER TABLE pairing_codes ADD COLUMN created_by TEXT;

-- Who made the pairing code that the client or the worker used, and who
-- removed it, in the same form as `pairing_codes.created_by`.
ALTER TABLE clients ADD COLUMN paired_by TEXT;
ALTER TABLE clients ADD COLUMN revoked_by TEXT;
ALTER TABLE workers ADD COLUMN paired_by TEXT;
ALTER TABLE workers ADD COLUMN revoked_by TEXT;

-- When the hub asked the workers to delete the workspace of the task.
-- Unix milliseconds. A retry clears it.
ALTER TABLE tasks ADD COLUMN workspace_released_at INTEGER;

-- 1 while the open turn of a task session is one that the hub opened when
-- it moved the task from Ready for review back to Working.
ALTER TABLE session_runtime ADD COLUMN turn_from_review INTEGER NOT NULL DEFAULT 0;
