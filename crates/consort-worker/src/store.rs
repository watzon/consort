//! `worker.db`: the event spool, processed command IDs, the local session
//! registry and the task workspaces.
//!
//! The worker writes every event to the spool before it sends it, and
//! deletes events after the hub acknowledges them. Each command result is
//! kept for 8 days ([`COMMAND_RESULT_KEEP_MS`]), longer than the hub keeps a
//! queued command, so a repeated command ID returns the same result.
//!
//! The database also lists the harness process groups that run, so the
//! next start can kill the ones that a killed worker left behind. On Unix
//! the file has mode 0600.

use std::path::Path;
use std::sync::Mutex;

use consort_proto::error::ErrorBody;
use consort_proto::ids::{NativeRef, SessionId, TaskId};
use consort_proto::model::SessionState;
use consort_proto::time::Timestamp;
use consort_proto::worker_link::{
    COMMAND_RESULT_TTL_MS, MAX_BATCH_BYTES, MAX_EVENTS_PER_BATCH, QUEUED_COMMAND_TTL_MS,
    SequencedEvent, SessionEvent, SessionStart,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::WorkerError;
use crate::cap::cap_event;

/// One day, in milliseconds.
const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

/// How long the worker keeps a command result, in milliseconds: 8 days.
///
/// The hub keeps a queued command for [`QUEUED_COMMAND_TTL_MS`] (7 days)
/// from the time it makes the command, and it can send the command again
/// until then. The worker stores a result after the hub made the command,
/// so a command that arrives again always finds its result. The extra day
/// covers a clock difference between the hub and the worker and the hub's
/// expiry loop. The worker does not use `COMMAND_RESULT_TTL_MS` (24 hours)
/// alone, because a `session.turn` that the hub sends again after that would
/// run twice.
pub const COMMAND_RESULT_KEEP_MS: i64 = {
    let keep = QUEUED_COMMAND_TTL_MS + DAY_MS;
    if keep > COMMAND_RESULT_TTL_MS {
        keep
    } else {
        COMMAND_RESULT_TTL_MS
    }
};

// The worker keeps each result for longer than the hub resends a command.
const _: () = assert!(COMMAND_RESULT_KEEP_MS > QUEUED_COMMAND_TTL_MS);

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS spool (
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    at INTEGER NOT NULL,
    event TEXT NOT NULL,
    acked INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (session_id, seq)
);
CREATE TABLE IF NOT EXISTS processed_commands (
    command_id TEXT PRIMARY KEY,
    result TEXT NOT NULL,
    at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    start TEXT NOT NULL,
    native_ref TEXT,
    workspace_path TEXT NOT NULL,
    state TEXT NOT NULL,
    last_seq INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS workspaces (
    key TEXT PRIMARY KEY,
    task_id TEXT,
    root TEXT NOT NULL,
    entries TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS harness_processes (
    pgid INTEGER PRIMARY KEY,
    program TEXT NOT NULL,
    started TEXT NOT NULL,
    recorded_at INTEGER NOT NULL
);
";

/// One harness process group that runs, or ran when the worker stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRecord {
    /// The process group ID, which is the leader's process ID.
    pub pgid: u32,
    /// The program of the leader.
    pub program: String,
    /// The leader's start time, as `ps -o lstart` shows it.
    pub started: String,
}

/// The event that replaces a spooled event that the worker cannot read.
fn unreadable_event(seq: i64) -> SessionEvent {
    SessionEvent::Error {
        code: "event_unreadable".to_owned(),
        message: format!(
            "The worker could not read event {seq} of this session from its spool, so it sent this note instead."
        ),
    }
}

/// Creates the database file with mode 0600 on Unix, or sets that mode on
/// an existing file. SQLite gives its journal files the mode of the
/// database file.
fn make_private(path: &Path) -> Result<(), WorkerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let context = || format!("could not make {} private", path.display());
        if path.exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| WorkerError::io(context(), e))?;
        } else {
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(path)
                .map_err(|e| WorkerError::io(context(), e))?;
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// A stored command result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ok")]
pub enum StoredResult {
    /// The `ok` body.
    #[serde(rename = "true")]
    Ok {
        /// The body.
        body: Value,
    },
    /// The error.
    #[serde(rename = "false")]
    Err {
        /// The error body.
        error: ErrorBody,
    },
}

impl From<&Result<Value, ErrorBody>> for StoredResult {
    fn from(result: &Result<Value, ErrorBody>) -> Self {
        match result {
            Ok(body) => Self::Ok { body: body.clone() },
            Err(error) => Self::Err {
                error: error.clone(),
            },
        }
    }
}

impl StoredResult {
    /// The result.
    pub fn into_result(self) -> Result<Value, ErrorBody> {
        match self {
            Self::Ok { body } => Ok(body),
            Self::Err { error } => Err(error),
        }
    }
}

/// One session in the local registry.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    /// The `session.start` command.
    pub start: SessionStart,
    /// The harness's own session ID.
    pub native_ref: Option<NativeRef>,
    /// The workspace folder.
    pub workspace_path: String,
    /// The last state.
    pub state: SessionState,
    /// The last sequence number given to an event.
    pub last_seq: u64,
}

/// One task workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    /// The task, when the workspace belongs to one.
    pub task_id: Option<TaskId>,
    /// The workspace root.
    pub root: String,
    /// What the worker made, so it can remove it again.
    pub entries: Vec<WorkspaceEntry>,
}

/// One thing that the worker made for a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceEntry {
    /// A user checkout used in place. Never deleted.
    InPlace {
        /// The checkout.
        path: String,
    },
    /// A git worktree of a checkout.
    Worktree {
        /// The checkout.
        checkout: String,
        /// The worktree folder.
        path: String,
    },
    /// A fresh clone.
    Clone {
        /// The clone folder.
        path: String,
    },
    /// An empty folder.
    Empty {
        /// The folder.
        path: String,
    },
}

fn state_name(state: SessionState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "lost".to_owned())
}

fn parse_state(name: &str) -> SessionState {
    serde_json::from_value(Value::String(name.to_owned())).unwrap_or(SessionState::Lost)
}

fn json_err(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn to_u64(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// The worker database.
#[derive(Debug)]
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Opens or creates the database.
    pub fn open(path: &Path) -> Result<Self, WorkerError> {
        make_private(path)?;
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// An in-memory database, for tests.
    pub fn in_memory() -> Result<Self, WorkerError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> Result<T, WorkerError> {
        // A poisoned lock means a panic in another call. The connection is
        // still usable, so take it.
        let mut conn = match self.conn.lock() {
            Ok(c) => c,
            Err(poisoned) => poisoned.into_inner(),
        };
        f(&mut conn).map_err(WorkerError::from)
    }

    // Sessions.

    /// Adds a session to the registry.
    pub fn put_session(&self, record: &SessionRecord) -> Result<(), WorkerError> {
        let start =
            serde_json::to_string(&record.start).map_err(|e| WorkerError::Config(e.to_string()))?;
        self.with(|c| {
            c.execute(
                "INSERT INTO sessions (session_id, start, native_ref, workspace_path, state, last_seq, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(session_id) DO UPDATE SET start = ?2, native_ref = ?3, workspace_path = ?4, state = ?5",
                params![
                    record.start.session_id.to_string(),
                    start,
                    record.native_ref.as_ref().map(|r| r.as_str().to_owned()),
                    record.workspace_path,
                    state_name(record.state),
                    to_i64(record.last_seq),
                    Timestamp::now().as_millis(),
                ],
            )
            .map(|_| ())
        })
    }

    /// A session from the registry.
    pub fn session(&self, id: SessionId) -> Result<Option<SessionRecord>, WorkerError> {
        self.with(|c| {
            c.query_row(
                "SELECT start, native_ref, workspace_path, state, last_seq FROM sessions WHERE session_id = ?1",
                params![id.to_string()],
                read_session,
            )
            .optional()
        })
    }

    /// The sessions that have not ended.
    pub fn open_sessions(&self) -> Result<Vec<SessionRecord>, WorkerError> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT start, native_ref, workspace_path, state, last_seq FROM sessions
                 WHERE state NOT IN ('ended', 'lost') ORDER BY created_at",
            )?;
            let rows = stmt.query_map([], read_session)?;
            rows.collect()
        })
    }

    /// Sets a session's state.
    pub fn set_session_state(&self, id: SessionId, state: SessionState) -> Result<(), WorkerError> {
        self.with(|c| {
            c.execute(
                "UPDATE sessions SET state = ?2 WHERE session_id = ?1",
                params![id.to_string(), state_name(state)],
            )
            .map(|_| ())
        })
    }

    /// Sets a session's native reference.
    pub fn set_native_ref(&self, id: SessionId, native: &NativeRef) -> Result<(), WorkerError> {
        self.with(|c| {
            c.execute(
                "UPDATE sessions SET native_ref = ?2 WHERE session_id = ?1",
                params![id.to_string(), native.as_str()],
            )
            .map(|_| ())
        })
    }

    // Spool.

    /// Numbers an event and writes it to the spool. Sequence numbers start
    /// at 1 and have no gaps. An event larger than
    /// [`MAX_EVENT_BYTES`](crate::cap::MAX_EVENT_BYTES) is cut first (see
    /// [`cap_event`]).
    pub fn append_event(
        &self,
        id: SessionId,
        event: &SessionEvent,
    ) -> Result<SequencedEvent, WorkerError> {
        let at = Timestamp::now();
        let event = cap_event(event.clone());
        let text = serde_json::to_string(&event).map_err(|e| WorkerError::Config(e.to_string()))?;
        let seq = self.with(|c| {
            let tx = c.transaction()?;
            let last: Option<i64> = tx
                .query_row(
                    "SELECT last_seq FROM sessions WHERE session_id = ?1",
                    params![id.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            let seq = last.unwrap_or(0) + 1;
            if last.is_some() {
                tx.execute(
                    "UPDATE sessions SET last_seq = ?2 WHERE session_id = ?1",
                    params![id.to_string(), seq],
                )?;
            }
            tx.execute(
                "INSERT INTO spool (session_id, seq, at, event) VALUES (?1, ?2, ?3, ?4)",
                params![id.to_string(), seq, at.as_millis(), text],
            )?;
            tx.commit()?;
            Ok(seq)
        })?;
        Ok(SequencedEvent {
            seq: to_u64(seq),
            at,
            event,
        })
    }

    /// The sessions that have events in the spool.
    pub fn spooled_sessions(&self) -> Result<Vec<SessionId>, WorkerError> {
        self.with(|c| {
            let mut stmt =
                c.prepare("SELECT DISTINCT session_id FROM spool ORDER BY session_id")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                if let Ok(id) = row?.parse() {
                    out.push(id);
                }
            }
            Ok(out)
        })
    }

    /// The next batch of spooled events of a session: at most
    /// [`MAX_EVENTS_PER_BATCH`] events and [`MAX_BATCH_BYTES`] bytes, and at
    /// least one event when any is spooled. A row that the worker cannot
    /// read becomes an `event_unreadable` error event with the same number,
    /// so one bad row never stops the session's events. A row that is too
    /// large (from an older build) is cut.
    pub fn next_batch(&self, id: SessionId) -> Result<Vec<SequencedEvent>, WorkerError> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT seq, at, event FROM spool WHERE session_id = ?1 ORDER BY seq LIMIT ?2",
            )?;
            let rows = stmt.query_map(
                params![id.to_string(), to_i64(MAX_EVENTS_PER_BATCH as u64)],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )?;
            let mut out = Vec::new();
            let mut bytes = 0_usize;
            for row in rows {
                let (seq, at, text) = row?;
                bytes += text.len() + 40;
                if !out.is_empty() && bytes > MAX_BATCH_BYTES {
                    break;
                }
                let event = match serde_json::from_str::<SessionEvent>(&text) {
                    Ok(event) => cap_event(event),
                    Err(err) => {
                        tracing::warn!(session = %id, seq, error = %err, "a spooled event cannot be read; the worker sends a note instead");
                        unreadable_event(seq)
                    }
                };
                out.push(SequencedEvent {
                    seq: to_u64(seq),
                    at: Timestamp::from_millis(at),
                    event,
                });
            }
            Ok(out)
        })
    }

    /// Deletes every spooled event of a session.
    pub fn drop_spool(&self, id: SessionId) -> Result<usize, WorkerError> {
        self.with(|c| {
            c.execute(
                "DELETE FROM spool WHERE session_id = ?1",
                params![id.to_string()],
            )
        })
    }

    /// Replaces one spooled event, for example with a note when the event
    /// cannot be sent.
    pub fn replace_event(
        &self,
        id: SessionId,
        seq: u64,
        event: &SessionEvent,
    ) -> Result<(), WorkerError> {
        let text = serde_json::to_string(event).map_err(|e| WorkerError::Config(e.to_string()))?;
        self.with(|c| {
            c.execute(
                "UPDATE spool SET event = ?3 WHERE session_id = ?1 AND seq = ?2",
                params![id.to_string(), to_i64(seq), text],
            )
            .map(|_| ())
        })
    }

    /// Deletes the events up to `last_seq`, which the hub stored.
    pub fn ack(&self, id: SessionId, last_seq: u64) -> Result<usize, WorkerError> {
        self.with(|c| {
            c.execute(
                "DELETE FROM spool WHERE session_id = ?1 AND seq <= ?2",
                params![id.to_string(), to_i64(last_seq)],
            )
        })
    }

    // Commands.

    /// The stored result of a command ID, if it is younger than
    /// [`COMMAND_RESULT_KEEP_MS`].
    pub fn command_result(&self, command_id: &str) -> Result<Option<StoredResult>, WorkerError> {
        self.command_result_at(command_id, Timestamp::now())
    }

    /// [`Store::command_result`] at the time `now`.
    fn command_result_at(
        &self,
        command_id: &str,
        now: Timestamp,
    ) -> Result<Option<StoredResult>, WorkerError> {
        let cutoff = now.as_millis() - COMMAND_RESULT_KEEP_MS;
        let text: Option<String> = self.with(|c| {
            c.query_row(
                "SELECT result FROM processed_commands WHERE command_id = ?1 AND at >= ?2",
                params![command_id, cutoff],
                |r| r.get(0),
            )
            .optional()
        })?;
        Ok(text.and_then(|t| serde_json::from_str(&t).ok()))
    }

    /// Stores the result of a command ID.
    pub fn store_command(
        &self,
        command_id: &str,
        result: &StoredResult,
    ) -> Result<(), WorkerError> {
        self.store_command_at(command_id, result, Timestamp::now())
    }

    /// [`Store::store_command`] at the time `at`.
    fn store_command_at(
        &self,
        command_id: &str,
        result: &StoredResult,
        at: Timestamp,
    ) -> Result<(), WorkerError> {
        let text = serde_json::to_string(result).map_err(|e| WorkerError::Config(e.to_string()))?;
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO processed_commands (command_id, result, at) VALUES (?1, ?2, ?3)",
                params![command_id, text, at.as_millis()],
            )
            .map(|_| ())
        })
    }

    /// Deletes command results older than [`COMMAND_RESULT_KEEP_MS`].
    pub fn prune_commands(&self, now: Timestamp) -> Result<usize, WorkerError> {
        let cutoff = now.as_millis() - COMMAND_RESULT_KEEP_MS;
        self.with(|c| {
            c.execute(
                "DELETE FROM processed_commands WHERE at < ?1",
                params![cutoff],
            )
        })
    }

    // Workspaces.

    /// Records a workspace.
    pub fn put_workspace(&self, key: &str, record: &WorkspaceRecord) -> Result<(), WorkerError> {
        let entries = serde_json::to_string(&record.entries)
            .map_err(|e| WorkerError::Config(e.to_string()))?;
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO workspaces (key, task_id, root, entries, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![key, record.task_id.map(|t| t.to_string()), record.root, entries, Timestamp::now().as_millis()],
            )
            .map(|_| ())
        })
    }

    /// The workspace of a key (a task ID or a session ID).
    pub fn workspace(&self, key: &str) -> Result<Option<WorkspaceRecord>, WorkerError> {
        self.with(|c| {
            c.query_row(
                "SELECT task_id, root, entries FROM workspaces WHERE key = ?1",
                params![key],
                |r| {
                    let task: Option<String> = r.get(0)?;
                    let entries: String = r.get(2)?;
                    Ok(WorkspaceRecord {
                        task_id: task.and_then(|t| t.parse().ok()),
                        root: r.get(1)?,
                        entries: serde_json::from_str(&entries).map_err(json_err)?,
                    })
                },
            )
            .optional()
        })
    }

    /// Forgets a workspace.
    pub fn delete_workspace(&self, key: &str) -> Result<(), WorkerError> {
        self.with(|c| {
            c.execute("DELETE FROM workspaces WHERE key = ?1", params![key])
                .map(|_| ())
        })
    }

    // Harness processes.

    /// Records a harness process group.
    pub fn put_process(&self, record: &ProcessRecord) -> Result<(), WorkerError> {
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO harness_processes (pgid, program, started, recorded_at) VALUES (?1, ?2, ?3, ?4)",
                params![i64::from(record.pgid), record.program, record.started, Timestamp::now().as_millis()],
            )
            .map(|_| ())
        })
    }

    /// Forgets a harness process group.
    pub fn delete_process(&self, pgid: u32) -> Result<(), WorkerError> {
        self.with(|c| {
            c.execute(
                "DELETE FROM harness_processes WHERE pgid = ?1",
                params![i64::from(pgid)],
            )
            .map(|_| ())
        })
    }

    /// The recorded harness process groups.
    pub fn processes(&self) -> Result<Vec<ProcessRecord>, WorkerError> {
        self.with(|c| {
            let mut stmt =
                c.prepare("SELECT pgid, program, started FROM harness_processes ORDER BY pgid")?;
            let rows = stmt.query_map([], |r| {
                Ok(ProcessRecord {
                    pgid: u32::try_from(r.get::<_, i64>(0)?).unwrap_or(0),
                    program: r.get(1)?,
                    started: r.get(2)?,
                })
            })?;
            rows.collect()
        })
    }

    /// Writes a raw spool row, for tests of rows that the worker cannot
    /// read.
    #[cfg(test)]
    pub(crate) fn put_raw_event(&self, id: SessionId, seq: u64, text: &str) {
        self.with(|c| {
            c.execute(
                "INSERT INTO spool (session_id, seq, at, event) VALUES (?1, ?2, 0, ?3)",
                params![id.to_string(), to_i64(seq), text],
            )
            .map(|_| ())
        })
        .unwrap();
    }
}

fn read_session(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let start: String = r.get(0)?;
    let native: Option<String> = r.get(1)?;
    let state: String = r.get(3)?;
    Ok(SessionRecord {
        start: serde_json::from_str(&start).map_err(json_err)?,
        native_ref: native.map(NativeRef::new),
        workspace_path: r.get(2)?,
        state: parse_state(&state),
        last_seq: to_u64(r.get(4)?),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use consort_proto::error::ErrorCode;
    use consort_proto::ids::{AccountId, ChannelId};
    use consort_proto::model::{ApprovalMode, Isolation, SessionRole};
    use consort_proto::worker_link::WorkspaceSpec;

    pub(crate) fn start(id: SessionId) -> SessionStart {
        SessionStart {
            session_id: id,
            role: SessionRole::Task,
            channel_id: ChannelId::new(),
            task_id: None,
            account_id: AccountId::new(),
            model: "m".into(),
            options: Default::default(),
            approval_mode: ApprovalMode::Ask,
            workspace: WorkspaceSpec {
                repos: vec![],
                isolation: Isolation::Clone,
                carry_uncommitted: false,
            },
            system_prompt_append: String::new(),
            allowed_local_mcp_servers: vec![],
            channel_files: vec![],
            resume: None,
            turn: None,
        }
    }

    pub(crate) fn record(id: SessionId) -> SessionRecord {
        SessionRecord {
            start: start(id),
            native_ref: None,
            workspace_path: "/w".into(),
            state: SessionState::Starting,
            last_seq: 0,
        }
    }

    fn state(s: SessionState) -> SessionEvent {
        SessionEvent::State { state: s }
    }

    #[test]
    fn spool_numbers_without_gaps_and_acks_delete() {
        let store = Store::in_memory().unwrap();
        let id = SessionId::new();
        store.put_session(&record(id)).unwrap();
        for _ in 0..5 {
            store
                .append_event(id, &state(SessionState::Active))
                .unwrap();
        }
        let batch = store.next_batch(id).unwrap();
        assert_eq!(
            batch.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        assert_eq!(store.ack(id, 3).unwrap(), 3);
        let batch = store.next_batch(id).unwrap();
        assert_eq!(batch.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![4, 5]);
        assert_eq!(
            store
                .append_event(id, &state(SessionState::Idle))
                .unwrap()
                .seq,
            6
        );
        assert_eq!(store.session(id).unwrap().unwrap().last_seq, 6);
        assert_eq!(store.spooled_sessions().unwrap(), vec![id]);
        store.ack(id, 6).unwrap();
        assert!(store.spooled_sessions().unwrap().is_empty());
    }

    #[test]
    fn batches_respect_limits() {
        let store = Store::in_memory().unwrap();
        let id = SessionId::new();
        store.put_session(&record(id)).unwrap();
        for _ in 0..300 {
            store
                .append_event(id, &state(SessionState::Active))
                .unwrap();
        }
        assert_eq!(store.next_batch(id).unwrap().len(), MAX_EVENTS_PER_BATCH);
        let big = SessionEvent::Error {
            code: "x".into(),
            message: "y".repeat(100 * 1024),
        };
        let other = SessionId::new();
        store.put_session(&record(other)).unwrap();
        for _ in 0..4 {
            store.append_event(other, &big).unwrap();
        }
        let batch = store.next_batch(other).unwrap();
        assert_eq!(batch.len(), 2, "two 100 KiB events fit in 256 KiB");
    }

    #[test]
    fn spool_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        let id = SessionId::new();
        {
            let store = Store::open(&path).unwrap();
            store.put_session(&record(id)).unwrap();
            store
                .append_event(id, &state(SessionState::Active))
                .unwrap();
            store.append_event(id, &state(SessionState::Idle)).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.next_batch(id).unwrap().len(), 2);
        assert_eq!(store.open_sessions().unwrap().len(), 1);
        assert_eq!(
            store
                .append_event(id, &state(SessionState::Active))
                .unwrap()
                .seq,
            3
        );
    }

    #[test]
    fn command_results_dedup_until_they_expire() {
        let store = Store::in_memory().unwrap();
        assert!(store.command_result("c1").unwrap().is_none());
        let ok = StoredResult::from(&Ok(serde_json::json!({"turn_id": "t1"})));
        store.store_command("c1", &ok).unwrap();
        assert_eq!(store.command_result("c1").unwrap(), Some(ok));
        let err = StoredResult::from(&Err(ErrorBody::new(ErrorCode::NotFound, "gone")));
        store.store_command("c2", &err).unwrap();
        assert_eq!(
            store
                .command_result("c2")
                .unwrap()
                .unwrap()
                .into_result()
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        let later = Timestamp::now().plus_millis(COMMAND_RESULT_KEEP_MS + 1000);
        assert_eq!(store.prune_commands(later).unwrap(), 2);
        assert!(store.command_result("c1").unwrap().is_none());
    }

    #[test]
    fn command_results_outlive_the_hubs_command_queue() {
        // The hub sends a queued command again for up to 7 days. A result
        // that the worker forgot earlier would let the command run twice.
        let store = Store::in_memory().unwrap();
        let stored_at = Timestamp::now();
        let ok = StoredResult::from(&Ok(serde_json::json!({"turn_id": "t1"})));
        store.store_command_at("c1", &ok, stored_at).unwrap();
        // The last resend of a command that the hub made just before the
        // worker ran it.
        let last_resend = stored_at.plus_millis(QUEUED_COMMAND_TTL_MS);
        assert_eq!(store.prune_commands(last_resend).unwrap(), 0);
        assert_eq!(
            store.command_result_at("c1", last_resend).unwrap(),
            Some(ok.clone())
        );
        assert_eq!(
            store
                .command_result_at("c1", stored_at.plus_millis(3 * DAY_MS))
                .unwrap(),
            Some(ok)
        );
        let gone = stored_at.plus_millis(COMMAND_RESULT_KEEP_MS + 1000);
        assert!(store.command_result_at("c1", gone).unwrap().is_none());
        assert_eq!(store.prune_commands(gone).unwrap(), 1);
    }

    #[test]
    fn an_unreadable_row_becomes_a_note_and_the_rest_follows() {
        let store = Store::in_memory().unwrap();
        let id = SessionId::new();
        store.put_session(&record(id)).unwrap();
        store
            .append_event(id, &state(SessionState::Active))
            .unwrap();
        store.put_raw_event(id, 2, "{\"type\": \"no such event\"}");
        store.put_raw_event(
            id,
            3,
            &serde_json::to_string(&state(SessionState::Idle)).unwrap(),
        );
        let batch = store.next_batch(id).unwrap();
        assert_eq!(
            batch.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(
            matches!(&batch[1].event, SessionEvent::Error { code, .. } if code == "event_unreadable")
        );
        assert_eq!(batch[2].event, state(SessionState::Idle));
    }

    #[test]
    fn oversized_events_are_cut_before_the_spool() {
        let store = Store::in_memory().unwrap();
        let id = SessionId::new();
        store.put_session(&record(id)).unwrap();
        let big = SessionEvent::Error {
            code: "x".into(),
            message: "y".repeat(1_200_000),
        };
        let stored = store.append_event(id, &big).unwrap();
        let batch = store.next_batch(id).unwrap();
        assert_eq!(batch[0], stored);
        assert!(serde_json::to_vec(&batch[0].event).unwrap().len() <= crate::cap::MAX_EVENT_BYTES);
        // An old oversized row is cut when it is read.
        store.put_raw_event(id, 2, &serde_json::to_string(&big).unwrap());
        store.ack(id, 1).unwrap();
        let batch = store.next_batch(id).unwrap();
        assert_eq!(batch.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2]);
        let frame = consort_proto::frame::Frame::request(
            consort_proto::frame::FrameId::generate(),
            &consort_proto::worker_link::SessionEvents {
                session_id: id,
                events: batch,
            },
        )
        .unwrap();
        assert!(frame.to_text().is_ok(), "the batch fits in one frame");
    }

    #[cfg(unix)]
    #[test]
    fn the_database_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        Store::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        Store::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "an existing database becomes private");
    }

    #[test]
    fn processes_round_trip() {
        let store = Store::in_memory().unwrap();
        let record = ProcessRecord {
            pgid: 4242,
            program: "npx".into(),
            started: "Thu Oct 29 02:59:29 2026".into(),
        };
        store.put_process(&record).unwrap();
        assert_eq!(store.processes().unwrap(), vec![record]);
        store.delete_process(4242).unwrap();
        assert!(store.processes().unwrap().is_empty());
    }

    #[test]
    fn sessions_and_workspaces() {
        let store = Store::in_memory().unwrap();
        let id = SessionId::new();
        store.put_session(&record(id)).unwrap();
        store.set_native_ref(id, &NativeRef::new("n1")).unwrap();
        store.set_session_state(id, SessionState::Idle).unwrap();
        let got = store.session(id).unwrap().unwrap();
        assert_eq!(got.native_ref, Some(NativeRef::new("n1")));
        assert_eq!(got.state, SessionState::Idle);
        store.set_session_state(id, SessionState::Ended).unwrap();
        assert!(store.open_sessions().unwrap().is_empty());
        let ws = WorkspaceRecord {
            task_id: Some(TaskId::new()),
            root: "/w".into(),
            entries: vec![WorkspaceEntry::Clone {
                path: "/w/repo".into(),
            }],
        };
        store.put_workspace("k", &ws).unwrap();
        assert_eq!(store.workspace("k").unwrap(), Some(ws));
        store.delete_workspace("k").unwrap();
        assert!(store.workspace("k").unwrap().is_none());
    }
}
