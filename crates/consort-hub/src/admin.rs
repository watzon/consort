//! Commands that run on the hub machine against the data folder: list and
//! remove clients and workers. They open `hub.db` directly, as
//! [`crate::create_setup_code`] does, so they work while a hub runs. A
//! running hub reads a removal within a few seconds, closes the connection
//! of the client or the worker, and ends the open sessions of a removed
//! worker.
//!
//! Use them when a client key is lost or stolen. A client can remove other
//! clients too, but a stolen client could remove every client of the user.
//! These commands need access to the hub machine instead.

use std::path::Path;

use consort_proto::ids::{ClientId, WorkerId};

use crate::error::Result;
use crate::store::{self, machine};

pub use crate::backup::{LOCK_FILE, RestoreError, RestoreReport, restore};
pub use crate::store::machine::{ClientRecord, Origin, WorkerRecord};

/// What a remove command did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// The name of the client or the worker.
    pub name: String,
    /// True when it was removed before this command.
    pub already: bool,
}

/// Every client of the hub, removed ones included, in pairing order.
pub fn list_clients(data_dir: &Path) -> Result<Vec<ClientRecord>> {
    let conn = store::open_and_migrate(data_dir)?;
    machine::client_records(&conn)
}

/// Removes a client. Its pairing codes that were not used yet stop working.
pub fn revoke_client(data_dir: &Path, id: ClientId) -> Result<Removed> {
    let mut conn = store::open_and_migrate(data_dir)?;
    let (client, newly) =
        store::write_direct(&mut conn, |tx| tx.revoke_client(id, Origin::HubMachine))?;
    Ok(Removed {
        name: client.name,
        already: !newly,
    })
}

/// Every worker of the hub, removed ones included, in pairing order.
pub fn list_workers(data_dir: &Path) -> Result<Vec<WorkerRecord>> {
    let conn = store::open_and_migrate(data_dir)?;
    machine::worker_records(&conn)
}

/// Removes a worker. Its queued commands go. A running hub ends its open
/// sessions; a hub that is stopped does so when it starts.
pub fn revoke_worker(data_dir: &Path, id: WorkerId) -> Result<Removed> {
    let mut conn = store::open_and_migrate(data_dir)?;
    let (worker, newly) =
        store::write_direct(&mut conn, |tx| tx.revoke_worker(id, Origin::HubMachine))?;
    Ok(Removed {
        name: worker.name,
        already: !newly,
    })
}
