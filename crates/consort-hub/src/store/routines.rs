//! Routines and their runs.

use consort_proto::client_api::{EntityKind, EntityPayload};
use consort_proto::ids::{ChannelId, MessageId, RoutineId, TaskId};
use consort_proto::model::{Routine, RoutineRun};
use consort_proto::time::Timestamp;
use rusqlite::{Connection, params};

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

/// The newest runs first.
pub fn routine_runs(conn: &Connection, id: RoutineId, limit: usize) -> Result<Vec<RoutineRun>> {
    let mut stmt = conn.prepare_cached(
        "SELECT started_at, message_id, task_id, error FROM routine_runs
         WHERE routine_id = ?1 ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(
        params![
            id.to_string(),
            i64::try_from(limit.clamp(1, 500)).unwrap_or(500)
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (started_at, message_id, task_id, error) = row?;
        out.push(RoutineRun {
            routine_id: id,
            started_at: Timestamp::from_millis(started_at),
            message_id: message_id.and_then(|m| m.parse::<MessageId>().ok()),
            task_id: task_id.and_then(|t| t.parse::<TaskId>().ok()),
            error,
        });
    }
    Ok(out)
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

    /// Records a run. It does not emit a feed entry: runs are not entities.
    pub fn record_routine_run(&mut self, run: &RoutineRun) -> Result<()> {
        self.db().execute(
            "INSERT INTO routine_runs (routine_id, started_at, message_id, task_id, error)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                run.routine_id.to_string(),
                run.started_at.as_millis(),
                run.message_id.map(|m| m.to_string()),
                run.task_id.map(|t| t.to_string()),
                run.error
            ],
        )?;
        Ok(())
    }
}
