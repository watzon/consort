//! Client API handlers for workers and clients.

use consort_proto::client_api::*;
use consort_proto::frame::Empty;
use consort_proto::model::WorkerState;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::store::machine::{self, PairingKind};

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
        _caller: &Caller,
        _request: WorkerPairingCode,
    ) -> Result<PairingCodeResult> {
        let (code, expires_at) = self.new_pairing_code(PairingKind::Worker).await?;
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
        _caller: &Caller,
        request: WorkerRevoke,
    ) -> Result<Empty> {
        let worker_id = request.worker_id;
        self.store
            .write(move |tx| {
                let mut worker = machine::worker_or_404(tx.db(), worker_id)?;
                if worker.revoked_at.is_none() {
                    worker.revoked_at = Some(tx.now());
                    worker.state = WorkerState::Offline;
                    tx.save_worker(&worker)?;
                }
                // Nothing can reach a worker that is gone.
                tx.db().execute(
                    "DELETE FROM commands WHERE worker_id = ?1",
                    [worker_id.to_string()],
                )?;
                Ok(())
            })
            .await?;
        if let Some(link) = self.links.get(worker_id) {
            link.outbox.close();
        }
        for session in self.open_sessions_of(worker_id).await? {
            self.mark_session_lost(session.id, "The worker was removed.")
                .await?;
        }
        Ok(Empty {})
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
        _caller: &Caller,
        _request: ClientPairingCode,
    ) -> Result<ClientPairingCodeResult> {
        let (code, expires_at) = self.new_pairing_code(PairingKind::Client).await?;
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
        self.store
            .write(move |tx| {
                let mut client = machine::client(tx.db(), client_id)?
                    .ok_or_else(|| HubError::not_found("The client does not exist."))?;
                if client.revoked_at.is_none() {
                    client.revoked_at = Some(tx.now());
                    tx.save_client(&client)?;
                }
                Ok(())
            })
            .await?;
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
