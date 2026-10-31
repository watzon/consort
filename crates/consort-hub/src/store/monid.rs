//! Monid: the settings, the record of calls and "Allow for this task"
//! (ADR 0016).

use consort_proto::ids::{ChannelId, MonidCallId, TaskId};
use consort_proto::model::{
    AccessGrant, AccessTarget, Channel, MonidAllowedTool, MonidCall, MonidCallState,
    MonidChannelAccess, MonidChannelSpend, MonidSettings, MonidToolRef,
};
use consort_proto::time::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};

use super::{Tx, get_doc, list_docs, meta_get, to_doc};
use crate::error::Result;

const SETTINGS_KEY: &str = "monid_settings";
const KEY_PRESENT_KEY: &str = "monid_key_present";

/// The settings. A hub that never saved any has the defaults.
pub fn settings(conn: &Connection) -> Result<MonidSettings> {
    Ok(meta_get(conn, SETTINGS_KEY)?
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default())
}

/// Whether the hub has a key. The key itself is not in the database.
pub fn key_present(conn: &Connection) -> Result<bool> {
    Ok(meta_get(conn, KEY_PRESENT_KEY)?.is_some())
}

/// Whether a channel has Monid: the hub has a key, "Use in" matches and the
/// channel has no exception.
pub fn available_in(conn: &Connection, channel: &Channel) -> Result<bool> {
    Ok(key_present(conn)?
        && !channel.monid.disabled
        && settings(conn)?.use_in.matches_channel(channel))
}

/// What a session of this task, or a coordinator with `None`, may do with
/// Monid: the channel's access with any temporary access on top. `None` when
/// it has no Monid. A block always stays.
pub fn effective_access(
    conn: &Connection,
    channel: &Channel,
    task_id: Option<TaskId>,
) -> Result<Option<MonidChannelAccess>> {
    if !key_present(conn)? {
        return Ok(None);
    }
    let now = Timestamp::now();
    let grants: Vec<&AccessGrant> = channel
        .access_grants
        .iter()
        .filter(|grant| grant.target.is_monid() && grant.applies(task_id, now))
        .collect();
    let mut access = channel.monid.clone();
    let has_channel_monid = available_in(conn, channel)?;
    if grants.iter().any(|g| g.target == AccessTarget::Monid) {
        access.disabled = false;
        access.only_allowed = false;
        return Ok(Some(access));
    }
    if !has_channel_monid {
        if grants.is_empty() {
            return Ok(None);
        }
        // The channel has no Monid, so it gets only what the user gave.
        access.disabled = false;
        access.only_allowed = true;
        access.allowed.clear();
    }
    for grant in grants {
        if let AccessTarget::MonidTool {
            provider,
            endpoint,
            name,
        } = &grant.target
        {
            access.allowed.push(MonidAllowedTool {
                tool: MonidToolRef {
                    provider: provider.clone(),
                    endpoint: endpoint.clone(),
                    name: name.clone(),
                },
                run_without_asking: false,
            });
        }
    }
    Ok(Some(access))
}

/// When a user last denied this request in this scope.
pub fn access_denied_at(conn: &Connection, scope: &str, target: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT at FROM access_denials WHERE scope = ?1 AND target = ?2",
            params![scope, target],
            |row| row.get(0),
        )
        .optional()?)
}

/// One call.
pub fn call(conn: &Connection, id: MonidCallId) -> Result<Option<MonidCall>> {
    get_doc(
        conn,
        "SELECT data FROM monid_calls WHERE id = ?1",
        [id.to_string()],
    )
}

/// The newest calls of a channel.
pub fn calls_in_channel(
    conn: &Connection,
    channel_id: ChannelId,
    limit: u32,
) -> Result<Vec<MonidCall>> {
    list_docs(
        conn,
        "SELECT data FROM monid_calls WHERE channel_id = ?1 ORDER BY at DESC, rowid DESC LIMIT ?2",
        params![channel_id.to_string(), i64::from(limit)],
    )
}

/// The calls that had a run going when the hub stopped.
pub fn running_calls(conn: &Connection) -> Result<Vec<MonidCall>> {
    list_docs(
        conn,
        "SELECT data FROM monid_calls WHERE state = 'running'",
        params![],
    )
}

/// What a task has spent.
pub fn spent_in_task(conn: &Connection, task_id: TaskId) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(cost_micros), 0) FROM monid_calls WHERE task_id = ?1",
        [task_id.to_string()],
        |row| row.get(0),
    )?)
}

/// What a channel has spent since a time.
pub fn spent_in_channel_since(
    conn: &Connection,
    channel_id: ChannelId,
    since_ms: i64,
) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(cost_micros), 0) FROM monid_calls WHERE channel_id = ?1 AND at >= ?2",
        params![channel_id.to_string(), since_ms],
        |row| row.get(0),
    )?)
}

/// What the hub has spent since a time.
pub fn spent_since(conn: &Connection, since_ms: i64) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(cost_micros), 0) FROM monid_calls WHERE at >= ?1",
        [since_ms],
        |row| row.get(0),
    )?)
}

/// The spend per channel since a time, the largest first. Only calls that
/// ran count as calls.
pub fn spend_by_channel_since(conn: &Connection, since_ms: i64) -> Result<Vec<MonidChannelSpend>> {
    let mut statement = conn.prepare(
        "SELECT channel_id, COUNT(*), COALESCE(SUM(cost_micros), 0) FROM monid_calls
         WHERE at >= ?1 AND state NOT IN ('denied', 'blocked')
         GROUP BY channel_id ORDER BY 3 DESC, 1",
    )?;
    let rows = statement.query_map([since_ms], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, u32>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut spend = Vec::new();
    for row in rows {
        let (channel, calls, cost_micros) = row?;
        if let Ok(channel_id) = channel.parse() {
            spend.push(MonidChannelSpend {
                channel_id,
                calls,
                cost_micros,
            });
        }
    }
    Ok(spend)
}

/// Whether "Allow for this task" covers the tool in this scope.
pub fn has_grant(conn: &Connection, scope: &str, provider: &str, endpoint: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM monid_grants WHERE scope = ?1 AND provider = ?2 AND endpoint = ?3",
            params![scope, provider, endpoint],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn state_text(state: MonidCallState) -> Result<String> {
    Ok(serde_json::to_value(state)?
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

impl Tx<'_> {
    /// Saves the settings.
    pub fn save_monid_settings(&mut self, settings: &MonidSettings) -> Result<()> {
        self.meta_set(SETTINGS_KEY, &serde_json::to_string(settings)?)
    }

    /// Notes whether the hub has a key.
    pub fn set_monid_key_present(&mut self, present: bool) -> Result<()> {
        if present {
            self.meta_set(KEY_PRESENT_KEY, "1")
        } else {
            self.meta_delete(KEY_PRESENT_KEY)
        }
    }

    /// Adds or replaces the record of a call.
    pub fn save_monid_call(&mut self, call: &MonidCall) -> Result<()> {
        self.db().execute(
            "INSERT INTO monid_calls (id, channel_id, task_id, at, cost_micros, state, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET cost_micros = excluded.cost_micros,
                 state = excluded.state, data = excluded.data",
            params![
                call.id.to_string(),
                call.channel_id.to_string(),
                call.task_id.map(|t| t.to_string()),
                call.at.as_millis(),
                call.cost_micros,
                state_text(call.state)?,
                to_doc(call)?
            ],
        )?;
        Ok(())
    }

    /// Remembers that the user denied an access request.
    pub fn save_access_denial(&mut self, scope: &str, target: &str, at: i64) -> Result<()> {
        self.db().execute(
            "INSERT INTO access_denials (scope, target, at) VALUES (?1, ?2, ?3)
             ON CONFLICT(scope, target) DO UPDATE SET at = excluded.at",
            params![scope, target, at],
        )?;
        Ok(())
    }

    /// Remembers "Allow for this task".
    pub fn save_monid_grant(&mut self, scope: &str, provider: &str, endpoint: &str) -> Result<()> {
        self.db().execute(
            "INSERT OR IGNORE INTO monid_grants (scope, provider, endpoint) VALUES (?1, ?2, ?3)",
            params![scope, provider, endpoint],
        )?;
        Ok(())
    }
}
