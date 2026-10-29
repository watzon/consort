//! Client API handlers for workers and clients, and the check for clients
//! and workers that a command on the hub machine removed.

use std::collections::HashSet;
use std::sync::Mutex;

use consort_proto::client_api::*;
use consort_proto::frame::Empty;
use consort_proto::ids::{ClientId, WorkerId};

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::store::machine::{self, Origin, PairingKind};

impl Caller {
    /// Who acts, for the records of pairing codes and revokes.
    pub(crate) fn origin(self) -> Origin {
        match self {
            Caller::Client(id) => Origin::Client(id),
            Caller::Hub => Origin::HubMachine,
        }
    }
}

/// The clients and workers that this hub process knows as removed. A
/// command on the hub machine (`consort hub client revoke`) writes to the
/// database directly, so the hub compares the database with this list and
/// acts on what is new.
#[derive(Default)]
pub struct RevokeWatch {
    known: Mutex<Option<Known>>,
}

#[derive(Default)]
struct Known {
    clients: HashSet<ClientId>,
    workers: HashSet<WorkerId>,
}

/// What a check of the database found.
struct NewRevokes {
    clients: Vec<ClientId>,
    workers: Vec<WorkerId>,
    /// True at the first check after the hub started. The hub then does
    /// not send the entities to the feed again: the feed has them already.
    first: bool,
}

impl RevokeWatch {
    fn note_client(&self, id: ClientId) {
        if let Ok(mut known) = self.known.lock()
            && let Some(known) = known.as_mut()
        {
            known.clients.insert(id);
        }
    }

    fn note_worker(&self, id: WorkerId) {
        if let Ok(mut known) = self.known.lock()
            && let Some(known) = known.as_mut()
        {
            known.workers.insert(id);
        }
    }

    fn update(&self, clients: &[ClientId], workers: &[WorkerId]) -> NewRevokes {
        let Ok(mut slot) = self.known.lock() else {
            return NewRevokes {
                clients: Vec::new(),
                workers: Vec::new(),
                first: false,
            };
        };
        let first = slot.is_none();
        let known = slot.get_or_insert_with(Known::default);
        NewRevokes {
            clients: clients
                .iter()
                .filter(|id| known.clients.insert(**id))
                .copied()
                .collect(),
            workers: workers
                .iter()
                .filter(|id| known.workers.insert(**id))
                .copied()
                .collect(),
            first,
        }
    }
}

const NAME_MAX_CHARS: usize = 120;

fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX_CHARS {
        return Err(HubError::invalid("The name needs 1 to 120 characters."));
    }
    Ok(name.to_owned())
}

impl Hub {
    pub(crate) async fn worker_list(
        &self,
        _caller: &Caller,
        _request: WorkerList,
    ) -> Result<WorkerListResult> {
        let workers = self.store.read(machine::workers).await?;
        Ok(WorkerListResult { workers })
    }

    pub(crate) async fn worker_pairing_code(
        &self,
        caller: &Caller,
        _request: WorkerPairingCode,
    ) -> Result<PairingCodeResult> {
        let (code, expires_at) = self
            .new_pairing_code(PairingKind::Worker, caller.origin())
            .await?;
        Ok(PairingCodeResult { code, expires_at })
    }

    pub(crate) async fn worker_rename(
        &self,
        _caller: &Caller,
        request: WorkerRename,
    ) -> Result<WorkerResult> {
        let name = clean_name(&request.name)?;
        let worker = self
            .store
            .write(move |tx| {
                let mut worker = machine::worker_or_404(tx.db(), request.worker_id)?;
                if worker.name != name {
                    worker.name = name;
                    tx.save_worker(&worker)?;
                }
                Ok(worker)
            })
            .await?;
        Ok(WorkerResult { worker })
    }

    pub(crate) async fn worker_set_labels(
        &self,
        _caller: &Caller,
        request: WorkerSetLabels,
    ) -> Result<WorkerResult> {
        let mut labels: Vec<String> = Vec::new();
        for label in &request.labels {
            let label = label.trim();
            if label.is_empty() || label.chars().count() > 60 {
                return Err(HubError::invalid("A label needs 1 to 60 characters."));
            }
            if !labels.iter().any(|l| l == label) {
                labels.push(label.to_owned());
            }
        }
        let worker = self
            .store
            .write(move |tx| {
                let mut worker = machine::worker_or_404(tx.db(), request.worker_id)?;
                if worker.labels != labels {
                    worker.labels = labels;
                    tx.save_worker(&worker)?;
                }
                Ok(worker)
            })
            .await?;
        self.retry_waiting_work();
        Ok(WorkerResult { worker })
    }

    pub(crate) async fn worker_set_paused(
        &self,
        _caller: &Caller,
        request: WorkerSetPaused,
    ) -> Result<WorkerResult> {
        let paused = request.paused;
        let worker = self
            .store
            .write(move |tx| {
                let mut worker = machine::worker_or_404(tx.db(), request.worker_id)?;
                if worker.paused != paused {
                    worker.paused = paused;
                    tx.save_worker(&worker)?;
                }
                Ok(worker)
            })
            .await?;
        if !paused {
            self.retry_waiting_work();
        }
        Ok(WorkerResult { worker })
    }

    pub(crate) async fn worker_revoke(
        &self,
        caller: &Caller,
        request: WorkerRevoke,
    ) -> Result<Empty> {
        let worker_id = request.worker_id;
        let by = caller.origin();
        let (_, newly) = self
            .store
            .write(move |tx| tx.revoke_worker(worker_id, by))
            .await?;
        if newly {
            tracing::info!(worker_id = %worker_id, by = %by, "a worker was removed");
        }
        self.revokes.note_worker(worker_id);
        self.cut_off_worker(worker_id).await
    }

    /// Closes the link of a removed worker and ends its open sessions.
    async fn cut_off_worker(&self, worker_id: WorkerId) -> Result<Empty> {
        if let Some(link) = self.links.get(worker_id) {
            link.outbox.close();
        }
        for session in self.open_sessions_of(worker_id).await? {
            self.mark_session_lost(session.id, "The worker was removed.")
                .await?;
        }
        Ok(Empty {})
    }

    /// Acts on clients and workers that a command on the hub machine
    /// removed: their connections close, and the open sessions of a removed
    /// worker end. The feed gets the entity again, so connected clients see
    /// the change. The maintenance loop calls it every few seconds.
    pub(crate) async fn apply_revokes_from_disk(&self) -> Result<()> {
        let (clients, workers) = self
            .store
            .read(|conn| {
                Ok((
                    machine::revoked_client_ids(conn)?,
                    machine::revoked_worker_ids(conn)?,
                ))
            })
            .await?;
        let new = self.revokes.update(&clients, &workers);
        // A removed client keeps no connection, new or old.
        let connected = self.clients.connected_clients();
        for id in clients.iter().filter(|id| connected.contains(id)) {
            tracing::info!(client_id = %id, "a removed client was still connected, so the hub closes its connection");
            self.clients.disconnect(*id);
        }
        // At the first check, every removed worker counts: a revoke while
        // the hub was stopped can have left sessions open.
        let workers_to_cut: Vec<WorkerId> = if new.first {
            workers.clone()
        } else {
            new.workers.clone()
        };
        for id in workers_to_cut {
            self.store
                .write(move |tx| tx.revoke_worker(id, Origin::HubMachine).map(|_| ()))
                .await?;
            self.cut_off_worker(id).await?;
        }
        if new.first {
            return Ok(());
        }
        for id in new.clients {
            self.clients.disconnect(id);
            self.store
                .write(move |tx| match machine::client(tx.db(), id)? {
                    Some(client) => tx.save_client(&client),
                    None => Ok(()),
                })
                .await?;
        }
        for id in new.workers {
            self.store
                .write(move |tx| match machine::worker(tx.db(), id)? {
                    Some(worker) => tx.save_worker(&worker),
                    None => Ok(()),
                })
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn worker_refresh_inventory(
        &self,
        _caller: &Caller,
        request: WorkerRefreshInventory,
    ) -> Result<Empty> {
        self.store
            .read(move |conn| machine::worker_or_404(conn, request.worker_id).map(|_| ()))
            .await?;
        self.request_inventory_refresh(request.worker_id).await?;
        Ok(Empty {})
    }

    pub(crate) async fn client_pairing_code(
        &self,
        caller: &Caller,
        _request: ClientPairingCode,
    ) -> Result<ClientPairingCodeResult> {
        let (code, expires_at) = self
            .new_pairing_code(PairingKind::Client, caller.origin())
            .await?;
        Ok(ClientPairingCodeResult {
            hub_url: self.settings.public_url.clone(),
            code,
            expires_at,
        })
    }

    pub(crate) async fn client_list(
        &self,
        _caller: &Caller,
        _request: ClientList,
    ) -> Result<ClientListResult> {
        let clients = self.store.read(machine::clients).await?;
        Ok(ClientListResult { clients })
    }

    pub(crate) async fn client_revoke(
        &self,
        caller: &Caller,
        request: ClientRevoke,
    ) -> Result<Empty> {
        if caller.client_id() == Some(request.client_id) {
            return Err(HubError::conflict(
                "You cannot revoke the client that you are using. Use another client to remove this one.",
            ));
        }
        let client_id = request.client_id;
        let by = caller.origin();
        let (_, newly) = self
            .store
            .write(move |tx| tx.revoke_client(client_id, by))
            .await?;
        if newly {
            tracing::info!(client_id = %client_id, by = %by, "a client was removed");
        }
        self.revokes.note_client(client_id);
        self.clients.disconnect(client_id);
        Ok(Empty {})
    }

    pub(crate) async fn client_push_register(
        &self,
        caller: &Caller,
        request: ClientPushRegister,
    ) -> Result<Empty> {
        let Some(client_id) = caller.client_id() else {
            return Err(HubError::invalid("Only a client can register for push."));
        };
        self.store
            .write(move |tx| {
                let mut client = machine::client(tx.db(), client_id)?
                    .ok_or_else(|| HubError::not_found("The client does not exist."))?;
                tx.set_client_push(client_id, request.provider, &request.token)?;
                client.push_provider = Some(request.provider);
                tx.save_client(&client)
            })
            .await?;
        Ok(Empty {})
    }
}
