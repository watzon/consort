//! Wake events and the per-channel coordinator state.

use consort_proto::ids::{ChannelId, SessionId};
use consort_proto::time::Timestamp;
use consort_proto::turn_input::WakeEvent;
use rusqlite::{Connection, OptionalExtension, params};

use super::Tx;
use crate::error::Result;

/// The state of the coordinator of one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct CoordinatorRow {
    pub channel_id: ChannelId,
    pub session_id: Option<SessionId>,
    /// True from the moment the hub sends a turn until `turn.ended`.
    pub turn_open: bool,
    pub turn_started_at: Option<Timestamp>,
    /// The events in the open turn. If the session is lost, they go back to
    /// the queue.
    pub turn_events: Vec<WakeEvent>,
}

impl CoordinatorRow {
    pub fn empty(channel_id: ChannelId) -> Self {
        Self {
            channel_id,
            session_id: None,
            turn_open: false,
            turn_started_at: None,
            turn_events: Vec::new(),
        }
    }
}

pub fn coordinator(conn: &Connection, channel_id: ChannelId) -> Result<CoordinatorRow> {
    let row = conn
        .query_row(
            "SELECT session_id, turn_open, turn_started_at, turn_events FROM coordinators WHERE channel_id = ?1",
            [channel_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((session_id, turn_open, started, events)) = row else {
        return Ok(CoordinatorRow::empty(channel_id));
    };
    Ok(CoordinatorRow {
        channel_id,
        session_id: session_id.map(|s| s.parse()).transpose()?,
        turn_open,
        turn_started_at: started.map(Timestamp::from_millis),
        turn_events: serde_json::from_str(&events)?,
    })
}

/// The coordinator whose open turn belongs to a session.
pub fn coordinator_of_session(
    conn: &Connection,
    session_id: SessionId,
) -> Result<Option<CoordinatorRow>> {
    let channel: Option<String> = conn
        .query_row(
            "SELECT channel_id FROM coordinators WHERE session_id = ?1",
            [session_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    channel.map(|c| coordinator(conn, c.parse()?)).transpose()
}

/// Channels that have queued wake events.
pub fn channels_with_wakes(conn: &Connection) -> Result<Vec<ChannelId>> {
    let mut stmt = conn.prepare_cached("SELECT DISTINCT channel_id FROM wake_events")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?.parse()?);
    }
    Ok(out)
}

/// Channels with a coordinator turn that was open when the hub stopped.
pub fn channels_with_open_turn(conn: &Connection) -> Result<Vec<ChannelId>> {
    let mut stmt =
        conn.prepare_cached("SELECT channel_id FROM coordinators WHERE turn_open = 1")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?.parse()?);
    }
    Ok(out)
}

/// The queued wake events of a channel, oldest first, with their row IDs.
pub fn pending_wakes(conn: &Connection, channel_id: ChannelId) -> Result<Vec<(i64, WakeEvent)>> {
    let mut stmt =
        conn.prepare_cached("SELECT id, event FROM wake_events WHERE channel_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map([channel_id.to_string()], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, event) = row?;
        out.push((id, serde_json::from_str(&event)?));
    }
    Ok(out)
}

impl Tx<'_> {
    /// Queues a wake event. The caller must call `kick_coordinator` for the
    /// channel after the transaction commits.
    pub fn queue_wake(&mut self, channel_id: ChannelId, event: &WakeEvent) -> Result<()> {
        if matches!(event, WakeEvent::UserMessage { .. }) {
            // The user is here: check-ins go back to their own pace.
            self.reset_check_in_backoff(channel_id)?;
        }
        self.db().execute(
            "INSERT INTO wake_events (channel_id, event, created_at) VALUES (?1, ?2, ?3)",
            params![
                channel_id.to_string(),
                serde_json::to_string(event)?,
                self.now().as_millis()
            ],
        )?;
        Ok(())
    }

    /// Puts events back at the front of the queue, for example after a lost
    /// session. Row IDs only matter for order, so the events get IDs lower
    /// than every queued row. The IDs are unique across the whole table, not
    /// only in one channel, so the lowest ID comes from every channel.
    pub fn requeue_wakes_first(
        &mut self,
        channel_id: ChannelId,
        events: &[WakeEvent],
    ) -> Result<()> {
        let lowest: Option<i64> =
            self.db()
                .query_row("SELECT MIN(id) FROM wake_events", [], |row| row.get(0))?;
        let mut next = lowest.unwrap_or(1) - i64::try_from(events.len()).unwrap_or(0) - 1;
        for event in events {
            next += 1;
            self.db().execute(
                "INSERT INTO wake_events (id, channel_id, event, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![
                    next,
                    channel_id.to_string(),
                    serde_json::to_string(event)?,
                    self.now().as_millis()
                ],
            )?;
        }
        Ok(())
    }

    pub fn delete_wakes(&mut self, ids: &[i64]) -> Result<()> {
        for id in ids {
            self.db()
                .execute("DELETE FROM wake_events WHERE id = ?1", [id])?;
        }
        Ok(())
    }

    pub fn save_coordinator(&mut self, row: &CoordinatorRow) -> Result<()> {
        self.db().execute(
            "INSERT INTO coordinators (channel_id, session_id, turn_open, turn_started_at, turn_events)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(channel_id) DO UPDATE SET session_id = excluded.session_id,
                turn_open = excluded.turn_open, turn_started_at = excluded.turn_started_at,
                turn_events = excluded.turn_events",
            params![
                row.channel_id.to_string(),
                row.session_id.map(|s| s.to_string()),
                row.turn_open,
                row.turn_started_at.map(|t| t.as_millis()),
                serde_json::to_string(&row.turn_events)?
            ],
        )?;
        Ok(())
    }
}
