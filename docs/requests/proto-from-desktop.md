# Requests to consort-proto from the desktop client

The desktop app works without these. Each one would replace a guess in the client reducers (`crates/consort-client/src/reduce.rs`).

1. **Where a coordinator runs.** The composer shows "Runs on <worker> with <account>". Add the coordinator's current `worker_id` (and the account label it uses) to `Channel`, or send the coordinator session in the snapshot. Today the client guesses from the preferred workers and the account label (decision 52).
2. **The workspace of a task.** The thread panel shows "Worktree of <path> at <branch>". Add `workspace_path` and `branch` of the current session to `Task`, or put sessions in the snapshot. Today the client shows only the isolation.
3. **Task references in a message.** The morning brief lists tasks with their status and channel. Add an optional `task_refs: Vec<TaskId>` to `Message`, so a client can draw those rows from live task state. Today only the mock has them (decision 54).
4. **Session items in the snapshot.** The "Ran 14 commands and changed 6 files" line comes from session items, which arrive only on the feed. A channel snapshot with the recent items of each open task (or a count per task) would show the line after a restart.
