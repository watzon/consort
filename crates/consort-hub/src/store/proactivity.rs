//! Proactivity: the settings, the proactivity log, the state of each
//! channel's check-ins, when tasks entered their status, and the keys of
//! follow-ups.

use consort_proto::client_api::EntityPayload;
use consort_proto::ids::{ChannelId, RoutineId, TaskId};
use consort_proto::model::{
    ProactiveKind, ProactiveOutcome, ProactiveWake, ProactivitySettings, Task,
};
use consort_proto::status::TaskStatus;
use consort_proto::time::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};

use super::{Tx, get_doc, list_docs, meta_get, parse_doc, to_doc};
use crate::error::Result;

const SETTINGS_KEY: &str = "proactivity";

/// The settings. A hub that has none stored yet has the defaults.
pub fn settings(conn: &Connection) -> Result<ProactivitySettings> {
    match meta_get(conn, SETTINGS_KEY)? {
        Some(text) => parse_doc(&text),
        None => Ok(ProactivitySettings::default()),
    }
}

impl Tx<'_> {
    /// Stores the settings and sends a feed upsert.
    pub fn save_proactivity_settings(&mut self, settings: &ProactivitySettings) -> Result<()> {
        self.meta_set(SETTINGS_KEY, &to_doc(settings)?)?;
        self.emit(EntityPayload::ProactivitySettings(settings.clone()))
    }
}

// The log ------------------------------------------------------------------

/// The outcomes that mean a model ran, or will run, for the wake.
const RAN_A_MODEL: &str = "('pending', 'posted', 'notified', 'silent', 'failed')";

pub fn wake(conn: &Connection, id: u64) -> Result<Option<ProactiveWake>> {
    get_doc(
        conn,
        "SELECT data FROM proactive_wakes WHERE id = ?1",
        [i64::try_from(id).unwrap_or(i64::MAX)],
    )
}

/// A page of the log, newest first, and whether older entries exist.
pub fn wakes_page(
    conn: &Connection,
    channel_id: Option<ChannelId>,
    kind: Option<ProactiveKind>,
    before: Option<u64>,
    limit: usize,
) -> Result<(Vec<ProactiveWake>, bool)> {
    let limit = limit.max(1);
    let mut out: Vec<ProactiveWake> = list_docs(
        conn,
        "SELECT data FROM proactive_wakes
         WHERE (?1 IS NULL OR channel_id = ?1) AND (?2 IS NULL OR kind = ?2)
           AND (?3 IS NULL OR id < ?3)
         ORDER BY id DESC LIMIT ?4",
        params![
            channel_id.map(|c| c.to_string()),
            kind.map(ProactiveKind::as_str),
            before.map(|b| i64::try_from(b).unwrap_or(i64::MAX)),
            i64::try_from(limit + 1).unwrap_or(i64::MAX),
        ],
    )?;
    let has_more = out.len() > limit;
    out.truncate(limit);
    Ok((out, has_more))
}

/// The proactive turns of a channel since a time: wakes that ran a model or
/// wait for their turn. They count against the channel's daily budget.
pub fn turns_since(conn: &Connection, channel_id: ChannelId, since: Timestamp) -> Result<u32> {
    let count: i64 = conn.query_row(
        &format!(
            "SELECT count(*) FROM proactive_wakes
             WHERE channel_id = ?1 AND at >= ?2 AND outcome IN {RAN_A_MODEL}"
        ),
        params![channel_id.to_string(), since.as_millis()],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// The notifying proactive messages since a time, across the hub.
pub fn notified_since(conn: &Connection, since: Timestamp) -> Result<u32> {
    let count: i64 = conn.query_row(
        "SELECT COALESCE(SUM(notified), 0) FROM proactive_wakes WHERE at >= ?1",
        [since.as_millis()],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// The findings of the newest silent check-ins of a channel, newest first.
pub fn recent_findings(
    conn: &Connection,
    channel_id: ChannelId,
    count: usize,
) -> Result<Vec<String>> {
    let wakes: Vec<ProactiveWake> = list_docs(
        conn,
        "SELECT data FROM proactive_wakes
         WHERE channel_id = ?1 AND kind = 'check_in' AND outcome = 'silent'
         ORDER BY id DESC LIMIT ?2",
        params![channel_id.to_string(), i64::try_from(count).unwrap_or(3)],
    )?;
    Ok(wakes.into_iter().filter_map(|w| w.detail).collect())
}

/// The offers that research turns posted since a time, across the hub.
pub fn research_offers_since(conn: &Connection, since: Timestamp) -> Result<u32> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM messages
         WHERE created_at >= ?1
           AND json_extract(data, '$.proactive.offer') = 1
           AND json_extract(data, '$.proactive.kind') = 'research'",
        [since.as_millis()],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

impl Tx<'_> {
    /// Records a new log entry, gives it its number and emits it.
    pub fn insert_wake(&mut self, wake: &mut ProactiveWake) -> Result<()> {
        self.db().execute(
            "INSERT INTO proactive_wakes (channel_id, kind, at, outcome, data)
             VALUES (?1, ?2, ?3, ?4, '{}')",
            params![
                wake.channel_id.to_string(),
                wake.kind.as_str(),
                wake.at.as_millis(),
                wake.outcome.as_str()
            ],
        )?;
        wake.id = u64::try_from(self.db().last_insert_rowid()).unwrap_or(0);
        self.save_wake(wake)
    }

    /// Writes a changed log entry and emits it. `notified` counts the
    /// messages that notified.
    pub fn save_wake(&mut self, wake: &ProactiveWake) -> Result<()> {
        let notified = i64::from(wake.outcome == ProactiveOutcome::Notified);
        self.db().execute(
            "UPDATE proactive_wakes SET outcome = ?2, notified = ?3, data = ?4 WHERE id = ?1",
            params![
                i64::try_from(wake.id).unwrap_or(i64::MAX),
                wake.outcome.as_str(),
                notified,
                to_doc(wake)?
            ],
        )?;
        self.emit(EntityPayload::ProactiveWake(wake.clone()))
    }

    /// Removes log entries older than `before`. Clients drop them by age, so
    /// the feed gets no delete for each.
    pub fn purge_wakes(&mut self, before: Timestamp) -> Result<usize> {
        Ok(self.db().execute(
            "DELETE FROM proactive_wakes WHERE at < ?1",
            [before.as_millis()],
        )?)
    }

    /// Removes the proactivity state of a channel that the user deleted:
    /// its log, its check-in state, its follow-up keys and the status times
    /// of its tasks.
    pub fn delete_channel_wakes(&mut self, channel_id: ChannelId) -> Result<()> {
        let id = channel_id.to_string();
        self.db().execute(
            "DELETE FROM task_status_since WHERE task_id IN (SELECT id FROM tasks WHERE channel_id = ?1)",
            [&id],
        )?;
        self.db()
            .execute("DELETE FROM proactive_wakes WHERE channel_id = ?1", [&id])?;
        self.db().execute(
            "DELETE FROM proactive_channels WHERE channel_id = ?1",
            [&id],
        )?;
        self.db()
            .execute("DELETE FROM follow_ups WHERE channel_id = ?1", [&id])?;
        Ok(())
    }
}

// The state of each channel's check-ins ------------------------------------

/// What the hub keeps about the check-ins of one channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelState {
    /// When the last check-in looked at the hub. The next change summary
    /// starts here.
    pub last_check_in_at: Option<Timestamp>,
    /// Silent check-ins in a row.
    pub silent_streak: u32,
    /// The interval of the check-in before back-off doubled it.
    pub base_seconds: Option<u64>,
}

pub fn channel_state(conn: &Connection, channel_id: ChannelId) -> Result<ChannelState> {
    let row = conn
        .query_row(
            "SELECT last_check_in_at, silent_streak, base_seconds FROM proactive_channels
             WHERE channel_id = ?1",
            [channel_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(match row {
        Some((last, streak, base)) => ChannelState {
            last_check_in_at: last.map(Timestamp::from_millis),
            silent_streak: u32::try_from(streak).unwrap_or(0),
            base_seconds: base.and_then(|b| u64::try_from(b).ok()),
        },
        None => ChannelState::default(),
    })
}

impl Tx<'_> {
    pub fn save_channel_state(
        &mut self,
        channel_id: ChannelId,
        state: &ChannelState,
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO proactive_channels (channel_id, last_check_in_at, silent_streak, base_seconds)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(channel_id) DO UPDATE SET last_check_in_at = excluded.last_check_in_at,
                silent_streak = excluded.silent_streak, base_seconds = excluded.base_seconds",
            params![
                channel_id.to_string(),
                state.last_check_in_at.map(|t| t.as_millis()),
                i64::from(state.silent_streak),
                state.base_seconds.and_then(|b| i64::try_from(b).ok()),
            ],
        )?;
        Ok(())
    }
}

// When tasks entered their status ------------------------------------------

/// One task and when it entered its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSince {
    pub task_id: TaskId,
    pub status: TaskStatus,
    pub since: Timestamp,
    pub nudged: bool,
    pub held_logged: bool,
}

pub fn status_since(conn: &Connection, task_id: TaskId) -> Result<Option<StatusSince>> {
    let row = conn
        .query_row(
            "SELECT status, since, nudged, held_logged FROM task_status_since WHERE task_id = ?1",
            [task_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()?;
    Ok(row.and_then(|(status, since, nudged, held_logged)| {
        Some(StatusSince {
            task_id,
            status: serde_json::from_value(serde_json::Value::String(status)).ok()?,
            since: Timestamp::from_millis(since),
            nudged,
            held_logged,
        })
    }))
}

/// The tasks whose status changed after `since`, in one channel or (for
/// `None`) in every channel, oldest change first.
pub fn status_changes_since(
    conn: &Connection,
    channel_id: Option<ChannelId>,
    since: Timestamp,
) -> Result<Vec<(Task, Timestamp)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.data, s.since FROM task_status_since s JOIN tasks t ON t.id = s.task_id
         WHERE s.since > ?1 AND (?2 IS NULL OR t.channel_id = ?2) AND t.archived_at IS NULL
         ORDER BY s.since",
    )?;
    let rows = stmt.query_map(
        params![since.as_millis(), channel_id.map(|c| c.to_string())],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (data, at) = row?;
        out.push((parse_doc::<Task>(&data)?, Timestamp::from_millis(at)));
    }
    Ok(out)
}

impl Tx<'_> {
    /// Notes that a task entered its current status now. A task that keeps
    /// its status keeps its row, so a nudge comes once per status entry.
    pub fn note_task_status(&mut self, task: &Task) -> Result<()> {
        if let Some(row) = status_since(self.db(), task.id)?
            && row.status == task.status
        {
            return Ok(());
        }
        self.db().execute(
            "INSERT INTO task_status_since (task_id, status, since, nudged, held_logged)
             VALUES (?1, ?2, ?3, 0, 0)
             ON CONFLICT(task_id) DO UPDATE SET status = excluded.status, since = excluded.since,
                nudged = 0, held_logged = 0",
            params![
                task.id.to_string(),
                task.status.as_str(),
                self.now().as_millis()
            ],
        )?;
        Ok(())
    }

    /// Notes a task that the hub had no row for, with the time of its last
    /// change as the best guess of its status entry.
    pub fn note_task_status_at(&mut self, task: &Task, since: Timestamp) -> Result<()> {
        self.db().execute(
            "INSERT OR IGNORE INTO task_status_since (task_id, status, since, nudged, held_logged)
             VALUES (?1, ?2, ?3, 0, 0)",
            params![task.id.to_string(), task.status.as_str(), since.as_millis()],
        )?;
        Ok(())
    }

    pub fn mark_status_nudged(&mut self, task_id: TaskId) -> Result<()> {
        self.db().execute(
            "UPDATE task_status_since SET nudged = 1 WHERE task_id = ?1",
            [task_id.to_string()],
        )?;
        Ok(())
    }

    pub fn mark_status_held_logged(&mut self, task_id: TaskId) -> Result<()> {
        self.db().execute(
            "UPDATE task_status_since SET held_logged = 1 WHERE task_id = ?1",
            [task_id.to_string()],
        )?;
        Ok(())
    }

    pub fn forget_task_status(&mut self, task_id: TaskId) -> Result<()> {
        self.db().execute(
            "DELETE FROM task_status_since WHERE task_id = ?1",
            [task_id.to_string()],
        )?;
        Ok(())
    }
}

// Follow-ups ---------------------------------------------------------------

/// What the hub keeps about a follow-up besides its routine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUpRow {
    pub routine_id: RoutineId,
    pub channel_id: ChannelId,
    pub key: Option<String>,
    pub set_at: Timestamp,
}

fn follow_up_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, Option<String>, i64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn follow_up_parts(parts: (String, String, Option<String>, i64)) -> Result<FollowUpRow> {
    let (routine_id, channel_id, key, set_at) = parts;
    Ok(FollowUpRow {
        routine_id: routine_id.parse()?,
        channel_id: channel_id.parse()?,
        key,
        set_at: Timestamp::from_millis(set_at),
    })
}

pub fn follow_up(conn: &Connection, routine_id: RoutineId) -> Result<Option<FollowUpRow>> {
    conn.query_row(
        "SELECT routine_id, channel_id, key, set_at FROM follow_ups WHERE routine_id = ?1",
        [routine_id.to_string()],
        follow_up_from_row,
    )
    .optional()?
    .map(follow_up_parts)
    .transpose()
}

pub fn follow_up_by_key(
    conn: &Connection,
    channel_id: ChannelId,
    key: &str,
) -> Result<Option<FollowUpRow>> {
    conn.query_row(
        "SELECT routine_id, channel_id, key, set_at FROM follow_ups WHERE channel_id = ?1 AND key = ?2",
        params![channel_id.to_string(), key],
        follow_up_from_row,
    )
    .optional()?
    .map(follow_up_parts)
    .transpose()
}

impl Tx<'_> {
    pub fn save_follow_up(&mut self, row: &FollowUpRow) -> Result<()> {
        self.db().execute(
            "INSERT INTO follow_ups (routine_id, channel_id, key, set_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(routine_id) DO UPDATE SET channel_id = excluded.channel_id,
                key = excluded.key, set_at = excluded.set_at",
            params![
                row.routine_id.to_string(),
                row.channel_id.to_string(),
                row.key,
                row.set_at.as_millis()
            ],
        )?;
        Ok(())
    }

    pub fn delete_follow_up_row(&mut self, routine_id: RoutineId) -> Result<()> {
        self.db().execute(
            "DELETE FROM follow_ups WHERE routine_id = ?1",
            [routine_id.to_string()],
        )?;
        Ok(())
    }
}
