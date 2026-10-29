//! Connections, the connector call audit log and "ask once per task"
//! answers.
//!
//! A row keeps the Composio connected account ID and auth config ID in
//! their own columns. They are not part of the `Connection` record that
//! clients see. No row holds a credential.

use consort_proto::client_api::{EntityKind, EntityPayload};
use consort_proto::ids::{ConnectionId, ConnectorCallId};
use consort_proto::model::{Channel, Connection, ConnectionStatus, ConnectorCall, ToolKind};
use rusqlite::{Connection as Db, OptionalExtension, params};

use super::{Tx, get_doc, list_docs, parse_doc, to_doc};
use crate::error::{HubError, Result};

/// A connection with the Composio IDs that only the hub sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionRow {
    pub connection: Connection,
    /// The Composio connected account (`ca_...`).
    pub composio_account_id: Option<String>,
    /// The Composio auth config (`ac_...`).
    pub auth_config_id: Option<String>,
    /// The auth scheme of the connected account, such as `oauth2`.
    pub auth_scheme: Option<String>,
}

pub fn connection(conn: &Db, id: ConnectionId) -> Result<Option<Connection>> {
    get_doc(
        conn,
        "SELECT data FROM connections WHERE id = ?1",
        [id.to_string()],
    )
}

pub fn connection_or_404(conn: &Db, id: ConnectionId) -> Result<Connection> {
    connection(conn, id)?.ok_or_else(|| HubError::not_found("The connection does not exist."))
}

pub fn connection_row(conn: &Db, id: ConnectionId) -> Result<Option<ConnectionRow>> {
    let row = conn
        .query_row(
            "SELECT data, composio_account_id, auth_config_id, auth_scheme FROM connections WHERE id = ?1",
            [id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(data, composio_account_id, auth_config_id, auth_scheme)| {
        Ok(ConnectionRow {
            connection: parse_doc(&data)?,
            composio_account_id,
            auth_config_id,
            auth_scheme,
        })
    })
    .transpose()
}

/// Every connection, by name.
pub fn connections(conn: &Db) -> Result<Vec<Connection>> {
    let mut all: Vec<Connection> = list_docs(conn, "SELECT data FROM connections", [])?;
    all.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.handle.cmp(&b.handle))
    });
    Ok(all)
}

/// Every handle in use.
pub fn handles(conn: &Db) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached("SELECT handle FROM connections")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// The connections that wait for a browser sign-in.
pub fn connecting(conn: &Db) -> Result<Vec<Connection>> {
    Ok(connections(conn)?
        .into_iter()
        .filter(|c| matches!(c.status, ConnectionStatus::Connecting { .. }))
        .collect())
}

/// Finds a connection by its handle or its ID.
pub fn find_connection(conn: &Db, key: &str) -> Result<Option<Connection>> {
    let key = key.trim();
    if let Ok(id) = key.parse::<ConnectionId>() {
        return connection(conn, id);
    }
    get_doc(
        conn,
        "SELECT data FROM connections WHERE handle = ?1",
        [key.to_ascii_lowercase()],
    )
}

/// Whether a connection can be in a session's effective set: it works, or
/// it needs a new sign-in (the assistant then tells the user). A connection
/// that is still connecting or that failed is left out.
pub fn is_usable(connection: &Connection) -> bool {
    matches!(
        connection.status,
        ConnectionStatus::Active | ConnectionStatus::NeedsSignIn { .. }
    )
}

/// The connections of a channel's effective set: their `use_in` matches
/// the channel, the channel did not turn them off, and they can be used.
pub fn effective_connections(conn: &Db, channel: &Channel) -> Result<Vec<Connection>> {
    Ok(connections(conn)?
        .into_iter()
        .filter(|c| c.is_available_in(channel) && is_usable(c))
        .collect())
}

/// One page of the audit log of a connection, newest first, and whether
/// older calls exist.
pub fn connector_calls(
    conn: &Db,
    connection_id: ConnectionId,
    before: Option<ConnectorCallId>,
    limit: u32,
) -> Result<(Vec<ConnectorCall>, bool)> {
    let mut calls: Vec<ConnectorCall> = list_docs(
        conn,
        "SELECT data FROM connector_calls
         WHERE connection_id = ?1
           AND (?2 IS NULL OR rowid < (SELECT rowid FROM connector_calls WHERE id = ?2))
         ORDER BY rowid DESC LIMIT ?3",
        params![
            connection_id.to_string(),
            before.map(|b| b.to_string()),
            i64::from(limit) + 1
        ],
    )?;
    let has_more = calls.len() > limit as usize;
    calls.truncate(limit as usize);
    Ok((calls, has_more))
}

/// Whether the user allowed this kind of call for this connection earlier
/// in the task (or in the coordinator session).
pub fn has_grant(
    conn: &Db,
    scope: &str,
    connection_id: ConnectionId,
    kind: ToolKind,
) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM connector_grants WHERE scope = ?1 AND connection_id = ?2 AND kind = ?3",
            params![scope, connection_id.to_string(), kind.as_str()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

impl Tx<'_> {
    /// Stores a connection and adds a feed upsert. The Composio IDs keep
    /// their stored values.
    pub fn save_connection(&mut self, connection: &Connection) -> Result<()> {
        self.db().execute(
            "INSERT INTO connections (id, handle, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET handle = excluded.handle, data = excluded.data",
            params![
                connection.id.to_string(),
                connection.handle,
                to_doc(connection)?
            ],
        )?;
        self.emit(EntityPayload::Connection(connection.clone()))
    }

    /// Sets the Composio IDs of a connection. It adds no feed entry, because
    /// clients do not see them.
    pub fn set_connection_composio(
        &mut self,
        id: ConnectionId,
        composio_account_id: Option<&str>,
        auth_config_id: Option<&str>,
        auth_scheme: Option<&str>,
    ) -> Result<()> {
        self.db().execute(
            "UPDATE connections SET composio_account_id = ?2, auth_config_id = ?3, auth_scheme = ?4
             WHERE id = ?1",
            params![
                id.to_string(),
                composio_account_id,
                auth_config_id,
                auth_scheme
            ],
        )?;
        Ok(())
    }

    /// Deletes a connection, its grants and its channel exceptions, and adds
    /// the feed delete. The audit log stays.
    pub fn delete_connection(&mut self, id: ConnectionId) -> Result<()> {
        self.db()
            .execute("DELETE FROM connections WHERE id = ?1", [id.to_string()])?;
        self.db().execute(
            "DELETE FROM connector_grants WHERE connection_id = ?1",
            [id.to_string()],
        )?;
        for mut channel in super::conversation::channels(self.db())? {
            if channel.disabled_connections.contains(&id) {
                channel.disabled_connections.retain(|c| *c != id);
                channel.updated_at = self.now();
                self.save_channel(&channel)?;
            }
        }
        self.emit_delete(EntityKind::Connection, id.to_string(), None)
    }

    /// Adds one record to the audit log. It is not a feed entity.
    pub fn save_connector_call(&mut self, call: &ConnectorCall) -> Result<()> {
        self.db().execute(
            "INSERT INTO connector_calls (id, connection_id, at, data) VALUES (?1, ?2, ?3, ?4)",
            params![
                call.id.to_string(),
                call.connection_id.to_string(),
                call.at.as_millis(),
                to_doc(call)?
            ],
        )?;
        Ok(())
    }

    /// Remembers an "ask once per task" answer.
    pub fn save_grant(
        &mut self,
        scope: &str,
        connection_id: ConnectionId,
        kind: ToolKind,
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO connector_grants (scope, connection_id, kind, granted_at)
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
            params![
                scope,
                connection_id.to_string(),
                kind.as_str(),
                self.now().as_millis()
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::model::{ToolApprovals, UseIn};
    use consort_proto::time::Timestamp;

    use super::*;
    use crate::store::testing::temp_store;

    fn sample(name: &str, handle: &str) -> Connection {
        Connection {
            id: ConnectionId::new(),
            name: name.to_owned(),
            handle: handle.to_owned(),
            service: "gmail".to_owned(),
            service_name: "Gmail".to_owned(),
            logo_url: None,
            identity: None,
            use_in: UseIn::EveryChannel,
            approvals: ToolApprovals::default(),
            status: ConnectionStatus::Active,
            last_used_at: None,
            created_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(1),
        }
    }

    #[tokio::test]
    async fn a_connection_keeps_its_composio_ids_out_of_the_record() {
        let (store, _dir) = temp_store();
        let connection = sample("Gmail Work", "gmail_work");
        let id = connection.id;
        store
            .write(move |tx| {
                tx.save_connection(&connection)?;
                tx.set_connection_composio(id, Some("ca_1"), Some("ac_1"), Some("oauth2"))
            })
            .await
            .unwrap();
        let row = store
            .read(move |conn| connection_row(conn, id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.composio_account_id.as_deref(), Some("ca_1"));
        let json = serde_json::to_string(&row.connection).unwrap();
        assert!(!json.contains("ca_1"));
        let found = store
            .read(|conn| find_connection(conn, "GMAIL_WORK"))
            .await
            .unwrap();
        assert_eq!(found.map(|c| c.id), Some(id));
    }

    #[tokio::test]
    async fn the_audit_log_pages_newest_first() {
        let (store, _dir) = temp_store();
        let connection = sample("Gmail Work", "gmail_work");
        let id = connection.id;
        let ids: Vec<ConnectorCallId> = (0..3).map(|_| ConnectorCallId::new()).collect();
        let stored = ids.clone();
        store
            .write(move |tx| {
                tx.save_connection(&connection)?;
                for (n, call_id) in stored.iter().enumerate() {
                    tx.save_connector_call(&ConnectorCall {
                        id: *call_id,
                        connection_id: id,
                        channel_id: None,
                        session_id: None,
                        task_id: None,
                        tool: format!("TOOL_{n}"),
                        tool_kind: ToolKind::Read,
                        arguments_sha256: crate::util::sha256(b"{}"),
                        decision: consort_proto::model::CallDecision::RanWithoutAsking,
                        outcome: consort_proto::model::CallOutcome::Succeeded,
                        error: None,
                        composio_log_id: None,
                        result_bytes: 0,
                        duration_ms: 0,
                        at: Timestamp::from_millis(10 + n as i64),
                    })?;
                }
                Ok(())
            })
            .await
            .unwrap();
        let (page, more) = store
            .read(move |conn| connector_calls(conn, id, None, 2))
            .await
            .unwrap();
        assert_eq!(
            page.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![ids[2], ids[1]]
        );
        assert!(more);
        let before = ids[1];
        let (page, more) = store
            .read(move |conn| connector_calls(conn, id, Some(before), 2))
            .await
            .unwrap();
        assert_eq!(page.iter().map(|c| c.id).collect::<Vec<_>>(), vec![ids[0]]);
        assert!(!more);
    }
}
