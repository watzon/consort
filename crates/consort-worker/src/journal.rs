//! The operation journal: `worker.db`.
//!
//! The key of an operation is the session, the operation ID and the hash of the
//! arguments. A model picks IDs that repeat, so a journal keyed on the ID alone
//! returned false results in the experiment
//! (`docs/research/hub-boundary-experiment.md`, "The defect"). So:
//!
//! - The same key replays the stored result, and marks it as a replay.
//! - The same session and ID with other arguments is refused.
//! - A replay of a change checks the file against the recorded hash.
//! - A process record persists, so `process_wait` works after a restart and a
//!   command with an unknown result never runs again by itself.

use std::path::Path;
use std::sync::Mutex;

use consort_proto::ids::{SessionId, TaskId};
use consort_proto::time::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::WorkerError;
use crate::config::PolicyConfig;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS ops (
    session_id    TEXT NOT NULL,
    operation_id  TEXT NOT NULL,
    argument_hash TEXT NOT NULL,
    tool          TEXT NOT NULL,
    state         TEXT NOT NULL,
    result        TEXT,
    check_path    TEXT,
    check_sha256  TEXT,
    created_at    INTEGER NOT NULL,
    PRIMARY KEY (session_id, operation_id)
);
CREATE TABLE IF NOT EXISTS processes (
    process_id   TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL,
    command      TEXT NOT NULL,
    cwd          TEXT NOT NULL,
    pid          INTEGER,
    start_mark   TEXT,
    state        TEXT NOT NULL,
    exit_code    INTEGER,
    output_path  TEXT NOT NULL,
    started_at   INTEGER NOT NULL,
    ended_at     INTEGER,
    deadline     INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS workspaces (
    key        TEXT PRIMARY KEY,
    capability TEXT NOT NULL,
    task_id    TEXT,
    root       TEXT NOT NULL,
    path       TEXT NOT NULL,
    entries    TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS workspaces_capability ON workspaces (capability);
CREATE TABLE IF NOT EXISTS policy (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT NOT NULL
);
";

/// How long the journal keeps an operation or a process that ended, in
/// milliseconds (8 days).
pub const KEEP_MS: i64 = 8 * 24 * 60 * 60 * 1_000;

/// What the worker made for a workspace, so it can remove it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    /// The task, when the workspace belongs to one.
    pub task_id: Option<TaskId>,
    /// The workspace root.
    pub root: String,
    /// What the worker made.
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

/// A workspace and what names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredWorkspace {
    /// The key: the task ID or the session ID.
    pub key: String,
    /// The capability that the hub made.
    pub capability: String,
    /// The folder where the work happens.
    pub path: String,
    /// What the worker made.
    pub record: WorkspaceRecord,
}

/// What the journal knows about an operation that is about to run.
#[derive(Debug, Clone, PartialEq)]
pub enum Begin {
    /// New. The caller runs it, then calls [`Journal::finish`].
    Run,
    /// It ran before. This is the result, and the file to check if any.
    Replay {
        /// The stored result.
        result: Value,
        /// The file and the hash that the operation left.
        check: Option<(String, String)>,
    },
    /// The same ID with other arguments.
    Reused,
    /// The operation began and never finished: the worker stopped during it.
    /// Nobody knows what it did, so it does not run again.
    Unknown,
}

/// A process of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRecord {
    /// The ID that the tools use.
    pub process_id: String,
    /// The session.
    pub session_id: SessionId,
    /// The command line.
    pub command: String,
    /// The folder.
    pub cwd: String,
    /// The process ID on this machine.
    pub pid: Option<u32>,
    /// The start time of the process, as the system shows it. A later check
    /// uses it, so a new process with the same ID is not taken for this one.
    pub start_mark: Option<String>,
    /// `running`, `exited` or `unknown`.
    pub state: String,
    /// The exit code.
    pub exit_code: Option<i32>,
    /// The file with the output.
    pub output_path: String,
    /// When it started.
    pub started_at: Timestamp,
    /// When it ended.
    pub ended_at: Option<Timestamp>,
    /// When the worker ends it.
    pub deadline: Timestamp,
}

/// The journal.
#[derive(Debug)]
pub struct Journal {
    conn: Mutex<Connection>,
}

impl Journal {
    /// Opens the database file.
    pub fn open(path: &Path) -> Result<Self, WorkerError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// A journal in memory, for tests.
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, WorkerError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, WorkerError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        Ok(f(&conn)?)
    }

    /// Starts an operation. A new one is recorded as running, in the same
    /// statement that checks for an old one.
    pub fn begin(
        &self,
        session: SessionId,
        operation_id: &str,
        argument_hash: &str,
        tool: &str,
    ) -> Result<Begin, WorkerError> {
        self.with(|c| {
            let inserted = c.execute(
                "INSERT OR IGNORE INTO ops (session_id, operation_id, argument_hash, tool, state, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
                params![session.to_string(), operation_id, argument_hash, tool, Timestamp::now().as_millis()],
            )?;
            if inserted == 1 {
                return Ok(Begin::Run);
            }
            let row: (String, String, Option<String>, Option<String>, Option<String>) = c.query_row(
                "SELECT argument_hash, state, result, check_path, check_sha256 FROM ops WHERE session_id = ?1 AND operation_id = ?2",
                params![session.to_string(), operation_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?;
            let (hash, state, result, check_path, check_sha) = row;
            if hash != argument_hash {
                return Ok(Begin::Reused);
            }
            if state != "done" {
                return Ok(Begin::Unknown);
            }
            let result = result
                .as_deref()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null);
            Ok(Begin::Replay {
                result,
                check: check_path.zip(check_sha),
            })
        })
    }

    /// Stores the result of an operation that ran, with the file and the hash
    /// that a replay checks.
    pub fn finish(
        &self,
        session: SessionId,
        operation_id: &str,
        result: &Value,
        check: Option<(&str, &str)>,
    ) -> Result<(), WorkerError> {
        let text = result.to_string();
        self.with(|c| {
            c.execute(
                "UPDATE ops SET state = 'done', result = ?3, check_path = ?4, check_sha256 = ?5
                 WHERE session_id = ?1 AND operation_id = ?2",
                params![
                    session.to_string(),
                    operation_id,
                    text,
                    check.map(|x| x.0),
                    check.map(|x| x.1)
                ],
            )
            .map(|_| ())
        })
    }

    /// Forgets an operation that failed before it changed anything, so the
    /// same ID can run again.
    pub fn forget(&self, session: SessionId, operation_id: &str) -> Result<(), WorkerError> {
        self.with(|c| {
            c.execute(
                "DELETE FROM ops WHERE session_id = ?1 AND operation_id = ?2",
                params![session.to_string(), operation_id],
            )
            .map(|_| ())
        })
    }

    /// Stores a process.
    pub fn put_process(&self, p: &ProcessRecord) -> Result<(), WorkerError> {
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO processes (process_id, session_id, command, cwd, pid, state, exit_code, output_path, started_at, ended_at, deadline, start_mark)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    p.process_id,
                    p.session_id.to_string(),
                    p.command,
                    p.cwd,
                    p.pid,
                    p.state,
                    p.exit_code,
                    p.output_path,
                    p.started_at.as_millis(),
                    p.ended_at.map(|t| t.as_millis()),
                    p.deadline.as_millis(),
                    p.start_mark,
                ],
            )
            .map(|_| ())
        })
    }

    fn process_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProcessRecord> {
        let session: String = row.get(1)?;
        let session_id = session.parse().map_err(|_| {
            rusqlite::Error::InvalidColumnType(
                1,
                "session_id".to_owned(),
                rusqlite::types::Type::Text,
            )
        })?;
        Ok(ProcessRecord {
            process_id: row.get(0)?,
            session_id,
            command: row.get(2)?,
            cwd: row.get(3)?,
            pid: row.get(4)?,
            state: row.get(5)?,
            exit_code: row.get(6)?,
            output_path: row.get(7)?,
            started_at: Timestamp::from_millis(row.get(8)?),
            ended_at: row.get::<_, Option<i64>>(9)?.map(Timestamp::from_millis),
            deadline: Timestamp::from_millis(row.get(10)?),
            start_mark: row.get(11)?,
        })
    }

    /// One process.
    pub fn process(&self, process_id: &str) -> Result<Option<ProcessRecord>, WorkerError> {
        self.with(|c| {
            c.query_row(
                "SELECT process_id, session_id, command, cwd, pid, state, exit_code, output_path, started_at, ended_at, deadline, start_mark FROM processes WHERE process_id = ?1",
                params![process_id],
                Self::process_from,
            )
            .optional()
        })
    }

    /// The processes that were running when the worker last stopped.
    pub fn running_processes(&self) -> Result<Vec<ProcessRecord>, WorkerError> {
        self.with(|c| {
            let mut statement = c.prepare(
                "SELECT process_id, session_id, command, cwd, pid, state, exit_code, output_path, started_at, ended_at, deadline, start_mark FROM processes WHERE state = 'running'",
            )?;
            let rows = statement.query_map([], Self::process_from)?;
            rows.collect()
        })
    }

    /// The newest processes of a session, newest first.
    pub fn session_processes(
        &self,
        session: SessionId,
        limit: u32,
    ) -> Result<Vec<ProcessRecord>, WorkerError> {
        self.with(|c| {
            let mut statement = c.prepare(
                "SELECT process_id, session_id, command, cwd, pid, state, exit_code, output_path, started_at, ended_at, deadline, start_mark FROM processes WHERE session_id = ?1 ORDER BY started_at DESC LIMIT ?2",
            )?;
            let rows = statement.query_map(params![session.to_string(), limit], Self::process_from)?;
            rows.collect()
        })
    }

    /// The operations of a session that began and have no stored result, as
    /// pairs of operation ID and tool.
    pub fn open_operations(
        &self,
        session: SessionId,
    ) -> Result<Vec<(String, String)>, WorkerError> {
        self.with(|c| {
            let mut statement = c.prepare(
                "SELECT operation_id, tool FROM ops WHERE session_id = ?1 AND state != 'done' ORDER BY created_at",
            )?;
            let rows = statement.query_map(params![session.to_string()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            rows.collect()
        })
    }

    /// The saved limits of this worker. `None` before the first change.
    pub fn policy(&self) -> Result<Option<PolicyConfig>, WorkerError> {
        let text: Option<String> = self.with(|c| {
            c.query_row("SELECT data FROM policy WHERE id = 1", [], |r| r.get(0))
                .optional()
        })?;
        text.map(|text| serde_json::from_str(&text).map_err(|e| WorkerError::Config(e.to_string())))
            .transpose()
    }

    /// Saves the limits of this worker.
    pub fn put_policy(&self, policy: &PolicyConfig) -> Result<(), WorkerError> {
        let data = serde_json::to_string(policy).map_err(|e| WorkerError::Config(e.to_string()))?;
        self.with(|c| {
            c.execute(
                "INSERT INTO policy (id, data) VALUES (1, ?1)
                 ON CONFLICT (id) DO UPDATE SET data = excluded.data",
                params![data],
            )
            .map(|_| ())
        })
    }

    /// Stores a workspace with its capability.
    pub fn put_workspace(&self, workspace: &StoredWorkspace) -> Result<(), WorkerError> {
        let entries = serde_json::to_string(&workspace.record.entries)
            .map_err(|e| WorkerError::Config(e.to_string()))?;
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO workspaces (key, capability, task_id, root, path, entries, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    workspace.key,
                    workspace.capability,
                    workspace.record.task_id.map(|t| t.to_string()),
                    workspace.record.root,
                    workspace.path,
                    entries,
                    Timestamp::now().as_millis()
                ],
            )
            .map(|_| ())
        })
    }

    fn workspace_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredWorkspace> {
        let task: Option<String> = row.get(2)?;
        let entries: String = row.get(5)?;
        Ok(StoredWorkspace {
            key: row.get(0)?,
            capability: row.get(1)?,
            path: row.get(4)?,
            record: WorkspaceRecord {
                task_id: task.and_then(|t| t.parse().ok()),
                root: row.get(3)?,
                entries: serde_json::from_str(&entries).unwrap_or_default(),
            },
        })
    }

    /// The workspace of a key.
    pub fn workspace(&self, key: &str) -> Result<Option<StoredWorkspace>, WorkerError> {
        self.with(|c| {
            c.query_row(
                "SELECT key, capability, task_id, root, path, entries FROM workspaces WHERE key = ?1",
                params![key],
                Self::workspace_from,
            )
            .optional()
        })
    }

    /// The workspace of a capability.
    pub fn workspace_of(&self, capability: &str) -> Result<Option<StoredWorkspace>, WorkerError> {
        self.with(|c| {
            c.query_row(
                "SELECT key, capability, task_id, root, path, entries FROM workspaces WHERE capability = ?1",
                params![capability],
                Self::workspace_from,
            )
            .optional()
        })
    }

    /// Removes a workspace.
    pub fn delete_workspace(&self, key: &str) -> Result<(), WorkerError> {
        self.with(|c| {
            c.execute("DELETE FROM workspaces WHERE key = ?1", params![key])
                .map(|_| ())
        })
    }

    /// Removes old operations and the processes that ended long ago. Returns
    /// the output files of the removed processes.
    pub fn prune(&self, now: Timestamp) -> Result<Vec<String>, WorkerError> {
        let before = now.as_millis() - KEEP_MS;
        self.with(|c| {
            c.execute("DELETE FROM ops WHERE created_at < ?1", params![before])?;
            let mut statement = c.prepare(
                "SELECT output_path FROM processes WHERE state != 'running' AND ended_at < ?1",
            )?;
            let files = statement
                .query_map(params![before], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            c.execute(
                "DELETE FROM processes WHERE state != 'running' AND ended_at < ?1",
                params![before],
            )?;
            Ok(files)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn session() -> SessionId {
        SessionId::new()
    }

    #[test]
    fn the_same_key_replays_and_other_arguments_are_refused() {
        let journal = Journal::in_memory().unwrap();
        let s = session();
        assert_eq!(
            journal.begin(s, "op", "h1", "fs_write").unwrap(),
            Begin::Run
        );
        journal
            .finish(s, "op", &json!({"ok": true}), Some(("a.txt", "abc")))
            .unwrap();
        assert_eq!(
            journal.begin(s, "op", "h1", "fs_write").unwrap(),
            Begin::Replay {
                result: json!({"ok": true}),
                check: Some(("a.txt".to_owned(), "abc".to_owned())),
            }
        );
        assert_eq!(
            journal.begin(s, "op", "h2", "fs_write").unwrap(),
            Begin::Reused
        );
    }

    /// The defect of the experiment: a model picked the same ID in a later
    /// session, and the journal gave a false result.
    #[test]
    fn another_session_with_the_same_id_runs() {
        let journal = Journal::in_memory().unwrap();
        let (a, b) = (session(), session());
        assert_eq!(
            journal.begin(a, "run-1", "h", "process_start").unwrap(),
            Begin::Run
        );
        journal.finish(a, "run-1", &json!({}), None).unwrap();
        assert_eq!(
            journal.begin(b, "run-1", "h", "process_start").unwrap(),
            Begin::Run
        );
    }

    #[test]
    fn an_operation_that_never_finished_has_an_unknown_result_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        let s = session();
        {
            let journal = Journal::open(&path).unwrap();
            assert_eq!(
                journal.begin(s, "op", "h", "process_start").unwrap(),
                Begin::Run
            );
        }
        let journal = Journal::open(&path).unwrap();
        assert_eq!(
            journal.begin(s, "op", "h", "process_start").unwrap(),
            Begin::Unknown
        );
    }

    #[test]
    fn process_records_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        let record = ProcessRecord {
            process_id: "p1".to_owned(),
            session_id: session(),
            command: "sleep 1".to_owned(),
            cwd: "/w".to_owned(),
            pid: Some(4),
            start_mark: None,
            state: "running".to_owned(),
            exit_code: None,
            output_path: "/o".to_owned(),
            started_at: Timestamp::from_millis(1),
            ended_at: None,
            deadline: Timestamp::from_millis(2),
        };
        Journal::open(&path).unwrap().put_process(&record).unwrap();
        let journal = Journal::open(&path).unwrap();
        assert_eq!(journal.process("p1").unwrap().unwrap(), record);
        assert_eq!(journal.running_processes().unwrap().len(), 1);
    }

    #[test]
    fn capabilities_find_their_workspace() {
        let journal = Journal::in_memory().unwrap();
        let workspace = StoredWorkspace {
            key: "key".to_owned(),
            capability: "cap".to_owned(),
            path: "/w/repo".to_owned(),
            record: WorkspaceRecord {
                task_id: None,
                root: "/w".to_owned(),
                entries: vec![WorkspaceEntry::Empty {
                    path: "/w".to_owned(),
                }],
            },
        };
        journal.put_workspace(&workspace).unwrap();
        assert_eq!(journal.workspace_of("cap").unwrap().unwrap(), workspace);
        assert_eq!(journal.workspace("key").unwrap().unwrap(), workspace);
        assert!(journal.workspace_of("other").unwrap().is_none());
        journal.delete_workspace("key").unwrap();
        assert!(journal.workspace("key").unwrap().is_none());
    }
}
