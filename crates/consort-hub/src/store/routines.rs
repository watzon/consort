//! Routines and their runs.

use consort_proto::client_api::{EntityKind, EntityPayload};
use consort_proto::ids::{ChannelId, MessageId, RoutineId, SessionId, TaskId};
use consort_proto::model::{Routine, RoutineRun, RoutineRunStatus, Task, TaskCreator};
use consort_proto::status::TaskStatus;
use consort_proto::time::Timestamp;
use consort_proto::turn_input::WakeEvent;
use rusqlite::{Connection, OptionalExtension, params};

use super::{Tx, get_doc, list_docs, to_doc};
use crate::error::{HubError, Result};

pub fn routine(conn: &Connection, id: RoutineId) -> Result<Option<Routine>> {
    get_doc(
        conn,
        "SELECT data FROM routines WHERE id = ?1",
        [id.to_string()],
    )
}

pub fn routine_or_404(conn: &Connection, id: RoutineId) -> Result<Routine> {
    routine(conn, id)?
        .ok_or_else(|| HubError::not_found(format!("The routine {id} does not exist.")))
}

pub fn routines(conn: &Connection) -> Result<Vec<Routine>> {
    list_docs(conn, "SELECT data FROM routines ORDER BY rowid", [])
}

pub fn routines_in_channel(conn: &Connection, channel_id: ChannelId) -> Result<Vec<Routine>> {
    list_docs(
        conn,
        "SELECT data FROM routines WHERE channel_id = ?1 ORDER BY rowid",
        [channel_id.to_string()],
    )
}

/// Enabled routines whose next run time has passed, oldest due first.
pub fn due_routines(conn: &Connection, now: Timestamp) -> Result<Vec<Routine>> {
    list_docs(
        conn,
        "SELECT data FROM routines WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1
         ORDER BY next_run_at",
        [now.as_millis()],
    )
}

const RUN_COLUMNS: &str =
    "id, routine_id, channel_id, started_at, ended_at, status, manual, message_id, task_id, error";

type RunRow = (
    i64,
    String,
    String,
    i64,
    Option<i64>,
    String,
    bool,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn run_from_row(row: RunRow) -> Result<RoutineRun> {
    let (
        id,
        routine_id,
        channel_id,
        started_at,
        ended_at,
        status,
        manual,
        message_id,
        task_id,
        error,
    ) = row;
    Ok(RoutineRun {
        id: u64::try_from(id).unwrap_or(0),
        routine_id: routine_id.parse()?,
        // A run from before version 8 of a routine that was gone has no
        // channel. It gets an ID that names no channel.
        channel_id: channel_id
            .parse()
            .unwrap_or_else(|_| ChannelId::from_uuid(uuid::Uuid::nil())),
        started_at: Timestamp::from_millis(started_at),
        ended_at: ended_at.map(Timestamp::from_millis),
        status: RoutineRunStatus::parse(&status).unwrap_or(RoutineRunStatus::Error),
        manual,
        message_id: message_id.and_then(|m| m.parse::<MessageId>().ok()),
        task_id: task_id.and_then(|t| t.parse::<TaskId>().ok()),
        error,
    })
}

fn runs_where(
    conn: &Connection,
    filter: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<RoutineRun>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {RUN_COLUMNS} FROM routine_runs WHERE {filter}"
    ))?;
    let rows = stmt.query_map(params, run_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(run_from_row(row?)?);
    }
    Ok(out)
}

/// The newest runs first.
pub fn routine_runs(conn: &Connection, id: RoutineId, limit: usize) -> Result<Vec<RoutineRun>> {
    runs_where(
        conn,
        "routine_id = ?1 ORDER BY id DESC LIMIT ?2",
        params![
            id.to_string(),
            i64::try_from(limit.clamp(1, 500)).unwrap_or(500)
        ],
    )
}

/// The run of a routine that still runs. There is at most one, because a
/// run that would start while another one runs is skipped.
pub fn running_run(conn: &Connection, id: RoutineId) -> Result<Option<RoutineRun>> {
    Ok(runs_where(
        conn,
        "routine_id = ?1 AND status = 'running' ORDER BY id DESC LIMIT 1",
        [id.to_string()],
    )?
    .pop())
}

/// The run that started a task, while it still runs.
pub fn running_run_of_task(conn: &Connection, task_id: TaskId) -> Result<Option<RoutineRun>> {
    Ok(runs_where(
        conn,
        "task_id = ?1 AND status = 'running' ORDER BY id DESC LIMIT 1",
        [task_id.to_string()],
    )?
    .pop())
}

impl Tx<'_> {
    pub fn save_routine(&mut self, routine: &Routine) -> Result<()> {
        self.db().execute(
            "INSERT INTO routines (id, channel_id, enabled, next_run_at, data) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled,
                next_run_at = excluded.next_run_at, data = excluded.data",
            params![
                routine.id.to_string(),
                routine.channel_id.to_string(),
                routine.enabled,
                routine.next_run_at.map(|t| t.as_millis()),
                to_doc(routine)?
            ],
        )?;
        self.emit(EntityPayload::Routine(routine.clone()))
    }

    pub fn delete_routine(&mut self, routine: &Routine) -> Result<()> {
        let id = routine.id.to_string();
        self.db()
            .execute("DELETE FROM routines WHERE id = ?1", [&id])?;
        self.db()
            .execute("DELETE FROM routine_runs WHERE routine_id = ?1", [&id])?;
        self.emit_delete(EntityKind::Routine, id, Some(routine.channel_id))
    }

    /// Records a new run, gives it its number and emits it.
    pub fn record_routine_run(&mut self, run: &mut RoutineRun) -> Result<()> {
        self.db().execute(
            "INSERT INTO routine_runs (routine_id, channel_id, started_at, ended_at, status,
                manual, message_id, task_id, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                run.routine_id.to_string(),
                run.channel_id.to_string(),
                run.started_at.as_millis(),
                run.ended_at.map(|t| t.as_millis()),
                run.status.as_str(),
                run.manual,
                run.message_id.map(|m| m.to_string()),
                run.task_id.map(|t| t.to_string()),
                run.error
            ],
        )?;
        run.id = u64::try_from(self.db().last_insert_rowid()).unwrap_or(0);
        self.emit(EntityPayload::RoutineRun(run.clone()))
    }

    /// Writes a changed run and emits it.
    pub fn save_routine_run(&mut self, run: &RoutineRun) -> Result<()> {
        self.db().execute(
            "UPDATE routine_runs SET ended_at = ?2, status = ?3, message_id = ?4, task_id = ?5,
                error = ?6 WHERE id = ?1",
            params![
                i64::try_from(run.id).unwrap_or(i64::MAX),
                run.ended_at.map(|t| t.as_millis()),
                run.status.as_str(),
                run.message_id.map(|m| m.to_string()),
                run.task_id.map(|t| t.to_string()),
                run.error
            ],
        )?;
        self.emit(EntityPayload::RoutineRun(run.clone()))
    }
}

// Run endings -------------------------------------------------------------
//
// A run is `running` until its work ends. These functions end it from the
// places where the work ends: the coordinator turn that got the routine's
// prompt, and the status of the task that the run started.

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    /// The work ended well.
    Ok,
    /// The work failed. The text says why, with the next step.
    Error(String),
}

/// What a task status means for the run that started the task: `None` while
/// the task still works or waits for input.
pub fn task_run_end(task: &Task) -> Option<RunEnd> {
    match task.status {
        TaskStatus::Review | TaskStatus::Resolved => Some(RunEnd::Ok),
        TaskStatus::Failed => {
            Some(RunEnd::Error(task.status_reason.clone().unwrap_or_else(
                || "The task failed. Open the task to see why.".to_owned(),
            )))
        }
        TaskStatus::Scheduled
        | TaskStatus::UpNext
        | TaskStatus::Working
        | TaskStatus::NeedsInput => None,
    }
}

impl Tx<'_> {
    /// Ends a run that still runs, and copies the result to its routine when
    /// it is still the routine's last run.
    pub fn end_routine_run(&mut self, run: &mut RoutineRun, end: RunEnd) -> Result<()> {
        if !run.is_running() {
            return Ok(());
        }
        run.ended_at = Some(self.now());
        match end {
            // A run that already holds an error (its schedule turned the
            // routine off) did not end well.
            RunEnd::Ok if run.error.is_some() => run.status = RoutineRunStatus::Error,
            RunEnd::Ok => run.status = RoutineRunStatus::Ok,
            RunEnd::Error(text) => {
                run.status = RoutineRunStatus::Error;
                run.error = Some(text);
            }
        }
        self.save_routine_run(run)?;
        if let Some(mut routine) = routine(self.db(), run.routine_id)?
            && routine.last_run_at == Some(run.started_at)
            && routine.last_run_status == Some(RoutineRunStatus::Running)
        {
            routine.last_run_status = Some(run.status);
            self.save_routine(&routine)?;
        }
        Ok(())
    }

    /// Ends the run that started `task`, when the task's status ends it.
    pub fn end_task_run(&mut self, task: &Task) -> Result<()> {
        if task.created_by != TaskCreator::Routine {
            return Ok(());
        }
        let Some(end) = task_run_end(task) else {
            return Ok(());
        };
        if let Some(mut run) = running_run_of_task(self.db(), task.id)? {
            self.end_routine_run(&mut run, end)?;
        }
        Ok(())
    }

    /// Ends the run that started a task that the user deleted.
    pub fn end_deleted_task_run(&mut self, task_id: TaskId) -> Result<()> {
        if let Some(mut run) = running_run_of_task(self.db(), task_id)? {
            self.end_routine_run(
                &mut run,
                RunEnd::Error("The task of this run was deleted.".to_owned()),
            )?;
        }
        Ok(())
    }

    /// Ends the runs whose routine prompts were in a coordinator turn that
    /// ended. `failure` is set when the turn did not finish. The run links
    /// to the first message that the session posted in the timeline in the
    /// turn.
    pub fn end_turn_runs(
        &mut self,
        events: &[WakeEvent],
        channel_id: ChannelId,
        session_id: SessionId,
        turn_started_at: Option<Timestamp>,
        failure: Option<&str>,
    ) -> Result<()> {
        for event in events {
            let WakeEvent::Routine { id, .. } = event else {
                continue;
            };
            let Some(mut run) = running_run(self.db(), *id)? else {
                continue;
            };
            if run.channel_id != channel_id {
                continue;
            }
            match failure {
                Some(text) => self.end_routine_run(&mut run, RunEnd::Error(text.to_owned()))?,
                None => {
                    let since = turn_started_at.unwrap_or(run.started_at);
                    run.message_id = first_turn_message(self.db(), channel_id, session_id, since)?;
                    self.end_routine_run(&mut run, RunEnd::Ok)?;
                }
            }
        }
        Ok(())
    }
}

/// The first text message that a session posted in a channel's timeline
/// since a time.
fn first_turn_message(
    conn: &Connection,
    channel_id: ChannelId,
    session_id: SessionId,
    since: Timestamp,
) -> Result<Option<MessageId>> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM messages
             WHERE channel_id = ?1 AND thread_root_id IS NULL AND created_at >= ?2
               AND json_extract(data, '$.author_session_id') = ?3
               AND json_extract(data, '$.kind') = 'text'
             ORDER BY created_at, rowid LIMIT 1",
            params![
                channel_id.to_string(),
                since.as_millis(),
                session_id.to_string()
            ],
            |row| row.get(0),
        )
        .optional()?;
    Ok(id.and_then(|id| id.parse().ok()))
}
