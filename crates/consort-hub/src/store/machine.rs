//! Workers, clients, pairing codes and the command queue.

use consort_proto::auth::PairingCode;
use consort_proto::client_api::EntityPayload;
use consort_proto::frame::FrameId;
use consort_proto::ids::{ClientId, SessionId, WorkerId};
use consort_proto::model::{Client, PairedBy, PushProvider, Worker, WorkerState};
use consort_proto::time::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use super::{Tx, get_doc, list_docs, to_doc};
use crate::error::{HubError, Result};
use crate::util::pairing_code_hash;

/// Who made a pairing code, or who removed a client or a worker: a paired
/// client, or a command on the hub machine (`consort hub ...`, the setup
/// code at the first start, or the dev mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A paired client, through the client API.
    Client(ClientId),
    /// A command on the hub machine.
    HubMachine,
}

/// The stored text of [`Origin::HubMachine`].
const HUB_MACHINE: &str = "hub";

impl Origin {
    fn as_text(self) -> String {
        match self {
            Self::Client(id) => id.to_string(),
            Self::HubMachine => HUB_MACHINE.to_owned(),
        }
    }

    /// The wire form of who paired a client.
    pub fn paired_by(self) -> PairedBy {
        match self {
            Self::Client(client_id) => PairedBy::Client { client_id },
            Self::HubMachine => PairedBy::HubMachine,
        }
    }

    fn from_text(text: &str) -> Option<Self> {
        if text == HUB_MACHINE {
            return Some(Self::HubMachine);
        }
        text.parse().ok().map(Self::Client)
    }
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(id) => write!(f, "{id}"),
            Self::HubMachine => f.write_str("the hub machine"),
        }
    }
}

/// Reads an optional origin column. Text that is not an origin reads as
/// unknown.
fn origin_column(text: Option<String>) -> Option<Origin> {
    text.as_deref().and_then(Origin::from_text)
}

// Workers ----------------------------------------------------------------

pub fn worker(conn: &Connection, id: WorkerId) -> Result<Option<Worker>> {
    get_doc(
        conn,
        "SELECT data FROM workers WHERE id = ?1",
        [id.to_string()],
    )
}

pub fn worker_or_404(conn: &Connection, id: WorkerId) -> Result<Worker> {
    worker(conn, id)?.ok_or_else(|| {
        HubError::new(
            consort_proto::error::ErrorCode::WorkerUnknown,
            format!("No worker has the ID {id}."),
        )
    })
}

/// Every worker, revoked ones included, in pairing order.
pub fn workers(conn: &Connection) -> Result<Vec<Worker>> {
    list_docs(conn, "SELECT data FROM workers ORDER BY rowid", [])
}

/// The IDs of the workers that were removed.
pub fn revoked_worker_ids(conn: &Connection) -> Result<Vec<WorkerId>> {
    let mut stmt =
        conn.prepare_cached("SELECT id FROM workers WHERE revoked_at IS NOT NULL ORDER BY rowid")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?.parse()?);
    }
    Ok(out)
}

/// A worker with who paired it and who removed it.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerRecord {
    pub worker: Worker,
    /// Who made the pairing code that the worker used. Empty when the hub
    /// does not know, for a worker that paired before the hub recorded it.
    pub paired_by: Option<Origin>,
    /// Who removed the worker. Empty while it is paired, or when the hub
    /// does not know.
    pub revoked_by: Option<Origin>,
}

/// Every worker with its records, revoked ones included, in pairing order.
pub fn worker_records(conn: &Connection) -> Result<Vec<WorkerRecord>> {
    let mut stmt =
        conn.prepare_cached("SELECT data, paired_by, revoked_by FROM workers ORDER BY rowid")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (data, paired_by, revoked_by) = row?;
        out.push(WorkerRecord {
            worker: super::parse_doc(&data)?,
            paired_by: origin_column(paired_by),
            revoked_by: origin_column(revoked_by),
        });
    }
    Ok(out)
}

impl Tx<'_> {
    /// Records who made the pairing code that a new worker used.
    pub fn set_worker_paired_by(&mut self, id: WorkerId, by: Option<Origin>) -> Result<()> {
        self.db().execute(
            "UPDATE workers SET paired_by = ?2 WHERE id = ?1",
            params![id.to_string(), by.map(Origin::as_text)],
        )?;
        Ok(())
    }

    /// Removes a worker: it can no longer connect, and its queued commands
    /// go, because nothing can reach it. Returns the worker, and whether it
    /// was paired until now. The caller ends the open sessions of the
    /// worker and closes its link.
    pub fn revoke_worker(&mut self, id: WorkerId, by: Origin) -> Result<(Worker, bool)> {
        let mut worker = worker_or_404(self.db(), id)?;
        let newly = worker.revoked_at.is_none();
        if newly {
            worker.revoked_at = Some(self.now());
            worker.state = WorkerState::Offline;
            self.save_worker(&worker)?;
            self.db().execute(
                "UPDATE workers SET revoked_by = ?2 WHERE id = ?1",
                params![id.to_string(), by.as_text()],
            )?;
        }
        self.db().execute(
            "DELETE FROM commands WHERE worker_id = ?1",
            [id.to_string()],
        )?;
        Ok((worker, newly))
    }

    pub fn save_worker(&mut self, worker: &Worker) -> Result<()> {
        self.db().execute(
            "INSERT INTO workers (id, revoked_at, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET revoked_at = excluded.revoked_at, data = excluded.data",
            params![
                worker.id.to_string(),
                worker.revoked_at.map(|t| t.as_millis()),
                to_doc(worker)?
            ],
        )?;
        self.emit(EntityPayload::Worker(worker.clone()))
    }

    /// Marks every worker offline. The hub runs this at start, because no
    /// worker is connected yet.
    pub fn mark_all_workers_offline(&mut self) -> Result<()> {
        let online: Vec<Worker> = list_docs(
            self.db(),
            "SELECT data FROM workers WHERE revoked_at IS NULL ORDER BY rowid",
            [],
        )?;
        for mut worker in online {
            if worker.state != WorkerState::Offline {
                worker.state = WorkerState::Offline;
                self.save_worker(&worker)?;
            }
        }
        Ok(())
    }
}

// Clients ----------------------------------------------------------------

pub fn client(conn: &Connection, id: ClientId) -> Result<Option<Client>> {
    get_doc(
        conn,
        "SELECT data FROM clients WHERE id = ?1",
        [id.to_string()],
    )
}

/// Every client, revoked ones included, in pairing order. A client that
/// paired before the client record held `paired_by` gets it from the
/// `paired_by` column.
pub fn clients(conn: &Connection) -> Result<Vec<Client>> {
    Ok(client_records(conn)?
        .into_iter()
        .map(|record| {
            let mut client = record.client;
            if client.paired_by.is_none() {
                client.paired_by = record.paired_by.map(Origin::paired_by);
            }
            client
        })
        .collect())
}

/// The number of clients that are paired and not revoked.
pub fn active_client_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(*) FROM clients WHERE revoked_at IS NULL",
        [],
        |row| row.get(0),
    )?)
}

/// The push registration of a client. The token never leaves the hub.
pub fn client_push(conn: &Connection, id: ClientId) -> Result<Option<(PushProvider, String)>> {
    let row: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT push_provider, push_token FROM clients WHERE id = ?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((Some(provider), Some(token))) = row else {
        return Ok(None);
    };
    let provider = serde_json::from_value(Value::String(provider))?;
    Ok(Some((provider, token)))
}

/// The IDs of the clients that were removed.
pub fn revoked_client_ids(conn: &Connection) -> Result<Vec<ClientId>> {
    let mut stmt =
        conn.prepare_cached("SELECT id FROM clients WHERE revoked_at IS NOT NULL ORDER BY rowid")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?.parse()?);
    }
    Ok(out)
}

/// A client with who paired it and who removed it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientRecord {
    pub client: Client,
    /// Who made the pairing code that the client used. Empty when the hub
    /// does not know, for a client that paired before the hub recorded it.
    pub paired_by: Option<Origin>,
    /// Who removed the client. Empty while it is paired, or when the hub
    /// does not know.
    pub revoked_by: Option<Origin>,
}

/// Every client with its records, revoked ones included, in pairing order.
pub fn client_records(conn: &Connection) -> Result<Vec<ClientRecord>> {
    let mut stmt =
        conn.prepare_cached("SELECT data, paired_by, revoked_by FROM clients ORDER BY rowid")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (data, paired_by, revoked_by) = row?;
        out.push(ClientRecord {
            client: super::parse_doc(&data)?,
            paired_by: origin_column(paired_by),
            revoked_by: origin_column(revoked_by),
        });
    }
    Ok(out)
}

impl Tx<'_> {
    /// Records who made the pairing code that a new client used.
    pub fn set_client_paired_by(&mut self, id: ClientId, by: Option<Origin>) -> Result<()> {
        self.db().execute(
            "UPDATE clients SET paired_by = ?2 WHERE id = ?1",
            params![id.to_string(), by.map(Origin::as_text)],
        )?;
        Ok(())
    }

    /// Removes a client and cancels the pairing codes that it made and that
    /// were not used yet. Returns the client, and whether it was paired
    /// until now. The caller closes its connections.
    pub fn revoke_client(&mut self, id: ClientId, by: Origin) -> Result<(Client, bool)> {
        let mut client = client(self.db(), id)?.ok_or_else(|| {
            HubError::not_found(format!(
                "No client has the ID {id}. Check the ID in the list of clients."
            ))
        })?;
        let newly = client.revoked_at.is_none();
        if newly {
            client.revoked_at = Some(self.now());
            self.save_client(&client)?;
            self.db().execute(
                "UPDATE clients SET revoked_by = ?2 WHERE id = ?1",
                params![id.to_string(), by.as_text()],
            )?;
        }
        // Also when it was removed before version 3 recorded the codes.
        self.db().execute(
            "DELETE FROM pairing_codes WHERE created_by = ?1 AND used_at IS NULL",
            [Origin::Client(id).as_text()],
        )?;
        Ok((client, newly))
    }

    pub fn save_client(&mut self, client: &Client) -> Result<()> {
        self.db().execute(
            "INSERT INTO clients (id, revoked_at, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET revoked_at = excluded.revoked_at, data = excluded.data",
            params![
                client.id.to_string(),
                client.revoked_at.map(|t| t.as_millis()),
                to_doc(client)?
            ],
        )?;
        self.emit(EntityPayload::Client(client.clone()))
    }

    pub fn set_client_push(
        &mut self,
        id: ClientId,
        provider: PushProvider,
        token: &str,
    ) -> Result<()> {
        let provider = serde_json::to_value(provider)?;
        self.db().execute(
            "UPDATE clients SET push_provider = ?2, push_token = ?3 WHERE id = ?1",
            params![id.to_string(), provider.as_str(), token],
        )?;
        Ok(())
    }
}

// Pairing codes ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingKind {
    Worker,
    Client,
}

impl PairingKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Client => "client",
        }
    }
}

/// Stores the hash of a pairing code, with who made it. Works on any
/// connection, so `create_setup_code` can run beside a hub.
pub fn insert_pairing_code(
    conn: &Connection,
    kind: PairingKind,
    code: &PairingCode,
    now: Timestamp,
    ttl_ms: i64,
    created_by: Origin,
) -> Result<Timestamp> {
    let expires = now.plus_millis(ttl_ms);
    conn.execute(
        "INSERT INTO pairing_codes (code_hash, kind, created_at, expires_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            pairing_code_hash(code),
            kind.as_str(),
            now.as_millis(),
            expires.as_millis(),
            created_by.as_text()
        ],
    )?;
    Ok(expires)
}

/// Who made a pairing code. Empty for an unknown code, or a code from
/// before the hub recorded it.
pub fn pairing_code_origin(
    conn: &Connection,
    kind: PairingKind,
    code: &PairingCode,
) -> Result<Option<Origin>> {
    let text: Option<Option<String>> = conn
        .query_row(
            "SELECT created_by FROM pairing_codes WHERE code_hash = ?1 AND kind = ?2",
            params![pairing_code_hash(code), kind.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(origin_column(text.flatten()))
}

/// Whether a pairing code can be used now. It only reads, so the hub can
/// refuse a wrong code without the writer. [`consume_pairing_code`] still
/// decides.
pub fn pairing_code_usable(
    conn: &Connection,
    kind: PairingKind,
    code: &PairingCode,
    now: Timestamp,
) -> Result<bool> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM pairing_codes
             WHERE code_hash = ?1 AND kind = ?2 AND used_at IS NULL AND expires_at > ?3",
            params![pairing_code_hash(code), kind.as_str(), now.as_millis()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Uses a pairing code once. Returns false for an unknown, used, expired or
/// wrong-kind code. The update is one statement, so two requests cannot both
/// use the same code.
pub fn consume_pairing_code(
    conn: &Connection,
    kind: PairingKind,
    code: &PairingCode,
    now: Timestamp,
) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE pairing_codes SET used_at = ?4
         WHERE code_hash = ?1 AND kind = ?2 AND used_at IS NULL AND expires_at > ?3",
        params![
            pairing_code_hash(code),
            kind.as_str(),
            now.as_millis(),
            now.as_millis()
        ],
    )?;
    Ok(changed == 1)
}

impl Tx<'_> {
    pub fn delete_expired_pairing_codes(&mut self) -> Result<usize> {
        let cutoff = self.now().plus_millis(-86_400_000);
        Ok(self.db().execute(
            "DELETE FROM pairing_codes WHERE expires_at < ?1 OR used_at < ?1",
            [cutoff.as_millis()],
        )?)
    }
}

// Command queue ----------------------------------------------------------

/// A command for a worker. It stays in the queue until the worker answers.
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedCommand {
    pub id: FrameId,
    pub worker_id: WorkerId,
    pub session_id: Option<SessionId>,
    pub kind: String,
    pub body: Value,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

type CommandRow = (String, String, Option<String>, String, String, i64, i64);

fn command_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CommandRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn command_from_parts(parts: CommandRow) -> Result<QueuedCommand> {
    let (id, worker_id, session_id, kind, body, created_at, expires_at) = parts;
    Ok(QueuedCommand {
        id: FrameId::new(id),
        worker_id: worker_id.parse()?,
        session_id: session_id.map(|s| s.parse()).transpose()?,
        kind,
        body: serde_json::from_str(&body)?,
        created_at: Timestamp::from_millis(created_at),
        expires_at: Timestamp::from_millis(expires_at),
    })
}

const COMMAND_COLUMNS: &str = "id, worker_id, session_id, kind, body, created_at, expires_at";

/// The commands of a worker in the order they were queued.
pub fn commands_for_worker(conn: &Connection, worker_id: WorkerId) -> Result<Vec<QueuedCommand>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COMMAND_COLUMNS} FROM commands WHERE worker_id = ?1 ORDER BY rowid"
    ))?;
    let rows = stmt.query_map([worker_id.to_string()], command_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(command_from_parts(row?)?);
    }
    Ok(out)
}

/// The queued commands of one session in the order they were queued, each
/// with whether the hub has sent it at least once.
pub fn commands_for_session(
    conn: &Connection,
    session_id: SessionId,
) -> Result<Vec<(QueuedCommand, bool)>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COMMAND_COLUMNS}, sent_at IS NOT NULL FROM commands WHERE session_id = ?1 ORDER BY rowid"
    ))?;
    let rows = stmt.query_map([session_id.to_string()], |row| {
        Ok((command_from_row(row)?, row.get::<_, bool>(7)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (parts, sent) = row?;
        out.push((command_from_parts(parts)?, sent));
    }
    Ok(out)
}

pub fn command(conn: &Connection, id: &FrameId) -> Result<Option<QueuedCommand>> {
    let row = conn
        .query_row(
            &format!("SELECT {COMMAND_COLUMNS} FROM commands WHERE id = ?1"),
            [id.as_str()],
            command_from_row,
        )
        .optional()?;
    row.map(command_from_parts).transpose()
}

impl Tx<'_> {
    pub fn enqueue_command(&mut self, command: &QueuedCommand) -> Result<()> {
        self.db().execute(
            &format!(
                "INSERT INTO commands ({COMMAND_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
            ),
            params![
                command.id.as_str(),
                command.worker_id.to_string(),
                command.session_id.map(|s| s.to_string()),
                command.kind,
                serde_json::to_string(&command.body)?,
                command.created_at.as_millis(),
                command.expires_at.as_millis()
            ],
        )?;
        Ok(())
    }

    pub fn delete_command(&mut self, id: &FrameId) -> Result<()> {
        self.db()
            .execute("DELETE FROM commands WHERE id = ?1", [id.as_str()])?;
        Ok(())
    }

    /// Deletes a command and returns whether it was still queued. The hub
    /// handles a result only when this returns true, so a result that
    /// arrives twice is handled once.
    pub fn take_command(&mut self, id: &FrameId) -> Result<bool> {
        let deleted = self
            .db()
            .execute("DELETE FROM commands WHERE id = ?1", [id.as_str()])?;
        Ok(deleted > 0)
    }

    /// Records that the hub sends a command now. Returns false when the
    /// command is no longer queued, for example because the worker answered
    /// it or the hub ended its session in the meantime.
    pub fn mark_command_sent(&mut self, id: &FrameId) -> Result<bool> {
        let now = self.now().as_millis();
        let updated = self.db().execute(
            "UPDATE commands SET sent_at = ?1 WHERE id = ?2",
            params![now, id.as_str()],
        )?;
        Ok(updated > 0)
    }

    /// Deletes commands that waited longer than their time to live. Returns
    /// them, so the caller can tell the user.
    pub fn expire_commands(&mut self) -> Result<Vec<QueuedCommand>> {
        let now = self.now().as_millis();
        let expired = {
            let mut stmt = self.db().prepare(&format!(
                "SELECT {COMMAND_COLUMNS} FROM commands WHERE expires_at <= ?1 ORDER BY rowid"
            ))?;
            let rows = stmt.query_map([now], command_from_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(command_from_parts(row?)?);
            }
            out
        };
        self.db()
            .execute("DELETE FROM commands WHERE expires_at <= ?1", [now])?;
        Ok(expired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::temp_store;
    use consort_proto::auth::PAIRING_CODE_TTL_MS;
    use consort_proto::time::Timestamp;
    use consort_proto::worker_link::{HubToWorker, InventoryRefresh, QUEUED_COMMAND_TTL_MS};

    #[tokio::test]
    async fn a_pairing_code_works_once_for_its_kind_and_expires_after_ten_minutes() {
        let (store, _dir) = temp_store();
        let code = crate::util::random_pairing_code();
        let issued = Timestamp::now();
        let stored = code.clone();
        store
            .write(move |tx| {
                insert_pairing_code(
                    tx.db(),
                    PairingKind::Worker,
                    &stored,
                    issued,
                    PAIRING_CODE_TTL_MS,
                    Origin::HubMachine,
                )
                .map(|_| ())
            })
            .await
            .unwrap();

        let try_use = |kind, at: Timestamp| {
            let code = code.clone();
            let store = store.clone();
            async move {
                store
                    .write(move |tx| consume_pairing_code(tx.db(), kind, &code, at))
                    .await
                    .unwrap()
            }
        };
        // The wrong kind and the time after the expiry both fail.
        assert!(!try_use(PairingKind::Client, issued).await);
        assert!(
            !try_use(
                PairingKind::Worker,
                issued.plus_millis(PAIRING_CODE_TTL_MS + 1)
            )
            .await
        );
        // Just before the end it works, and only once.
        assert!(
            try_use(
                PairingKind::Worker,
                issued.plus_millis(PAIRING_CODE_TTL_MS - 1)
            )
            .await
        );
        assert!(!try_use(PairingKind::Worker, issued).await);
    }

    #[tokio::test]
    async fn queued_commands_keep_their_order_and_expire_after_seven_days() {
        let (store, _dir) = temp_store();
        let worker_id = WorkerId::new();
        let ids = store
            .write(move |tx| {
                let first = tx.queue_command(
                    worker_id,
                    &HubToWorker::InventoryRefresh(InventoryRefresh {}),
                )?;
                let second = tx.queue_command(
                    worker_id,
                    &HubToWorker::InventoryRefresh(InventoryRefresh {}),
                )?;
                Ok((first, second))
            })
            .await
            .unwrap();
        let queued = store
            .read(move |conn| commands_for_worker(conn, worker_id))
            .await
            .unwrap();
        assert_eq!(
            queued.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
            [ids.0, ids.1]
        );
        let ttl = queued[0].expires_at.as_millis() - queued[0].created_at.as_millis();
        assert_eq!(ttl, QUEUED_COMMAND_TTL_MS);

        // Nothing expires now.
        let expired = store.write(|tx| tx.expire_commands()).await.unwrap();
        assert!(expired.is_empty());
        // Age the commands by 8 days. Both expire.
        store
            .write(|tx| {
                tx.db().execute(
                    "UPDATE commands SET expires_at = expires_at - 8 * 86400000",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let expired = store.write(|tx| tx.expire_commands()).await.unwrap();
        assert_eq!(expired.len(), 2);
        let left = store
            .read(move |conn| commands_for_worker(conn, worker_id))
            .await
            .unwrap();
        assert!(left.is_empty());
    }
}
