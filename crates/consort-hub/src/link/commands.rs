//! The command queue. Every command for a worker is stored first. The hub
//! sends it when the worker is online, and deletes it when the worker has
//! answered. After a restart or a reconnect, the hub sends the unanswered
//! commands again. The worker runs a command ID once, so a repeat is safe.

use std::collections::HashSet;
use std::time::Duration;

use consort_proto::error::ErrorBody;
use consort_proto::frame::{Frame, FrameId, MessageFamily, RequestFrame};
use consort_proto::ids::WorkerId;
use consort_proto::worker_link::{HubToWorker, QUEUED_COMMAND_TTL_MS, SessionStartResult};
use serde_json::Value;

use super::{CallError, WorkerLink};
use crate::error::{HubError, Result};
use crate::hub::Hub;
use crate::store::Tx;
use crate::store::machine::{self, QueuedCommand};

/// How long the hub waits for a worker to answer one command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long a direct request (not queued) waits.
const RPC_TIMEOUT: Duration = Duration::from_secs(60);

fn encode(id: FrameId, command: &HubToWorker) -> Result<RequestFrame> {
    match Frame::family_request(id, command)? {
        Frame::Request(request) => Ok(request),
        _ => Err(HubError::internal("a command did not encode as a request")),
    }
}

impl Tx<'_> {
    /// Stores a command for a worker. Call [`Hub::pump_worker`] after the
    /// transaction commits.
    pub fn queue_command(&mut self, worker_id: WorkerId, command: &HubToWorker) -> Result<FrameId> {
        let request = encode(FrameId::generate(), command)?;
        let now = self.now();
        self.enqueue_command(&QueuedCommand {
            id: request.id.clone(),
            worker_id,
            session_id: command.session_id(),
            kind: request.kind,
            body: request.body,
            created_at: now,
            expires_at: now.plus_millis(QUEUED_COMMAND_TTL_MS),
        })?;
        Ok(request.id)
    }
}

impl Hub {
    /// Sends the queued commands of a worker that are ready. It returns at
    /// once. Commands for one session go out one at a time, in order.
    pub(crate) fn pump_worker(&self, worker_id: WorkerId) {
        let hub = self.clone();
        tokio::spawn(async move {
            if let Err(err) = hub.pump_now(worker_id).await {
                tracing::warn!(error = %err, worker_id = %worker_id, "could not send queued commands");
            }
        });
    }

    async fn pump_now(&self, worker_id: WorkerId) -> Result<()> {
        let Some(link) = self.links.get(worker_id) else {
            return Ok(());
        };
        let (expired, commands) = self
            .store
            .write(move |tx| {
                let expired = tx.expire_commands()?;
                Ok((expired, machine::commands_for_worker(tx.db(), worker_id)?))
            })
            .await?;
        for command in expired {
            tracing::warn!(kind = %command.kind, worker_id = %worker_id, "a queued command expired after 7 days");
        }
        let mut seen = HashSet::new();
        for command in commands {
            // Keep the order inside one session: only the oldest command of
            // each session may be in flight.
            if !seen.insert(command.session_id) {
                continue;
            }
            if !link.mark_inflight(command.session_id) {
                continue;
            }
            let hub = self.clone();
            let link = link.clone();
            tokio::spawn(async move { hub.send_one(link, command).await });
        }
        Ok(())
    }

    async fn send_one(&self, link: std::sync::Arc<WorkerLink>, command: QueuedCommand) {
        let key = command.session_id;
        let worker_id = command.worker_id;
        let outcome = link
            .call(
                command.id.clone(),
                command.kind.clone(),
                command.body.clone(),
                COMMAND_TIMEOUT,
            )
            .await;
        match outcome {
            Ok(result) => {
                if let Err(err) = self.finish_command(&command, result).await {
                    tracing::warn!(error = %err, kind = %command.kind, "could not process a command result");
                }
                let id = command.id.clone();
                if let Err(err) = self.store.write(move |tx| tx.delete_command(&id)).await {
                    tracing::warn!(error = %err, "could not delete an answered command");
                }
            }
            Err(CallError::Gone) => {
                // The command stays queued. The next connection sends it again.
            }
            Err(CallError::Timeout) => {
                tracing::warn!(kind = %command.kind, worker_id = %worker_id, "a worker did not answer a command, so the hub closes the link");
                link.outbox.close();
            }
        }
        link.clear_inflight(key);
        if !link.outbox.is_closed() {
            self.pump_worker(worker_id);
        }
    }

    /// Sends a request that is not queued, for example `item.detail`. It
    /// fails with `worker_offline` when the worker is not connected.
    pub(crate) async fn worker_rpc(
        &self,
        worker_id: WorkerId,
        command: HubToWorker,
    ) -> Result<Value> {
        let link = self
            .links
            .get(worker_id)
            .ok_or_else(|| HubError::Api(super::worker_offline("The worker")))?;
        let request = encode(FrameId::generate(), &command)?;
        match link
            .call(request.id, request.kind, request.body, RPC_TIMEOUT)
            .await
        {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(HubError::Api(error)),
            Err(CallError::Gone) => Err(HubError::Api(super::worker_offline("The worker"))),
            Err(CallError::Timeout) => Err(HubError::new(
                consort_proto::error::ErrorCode::WorkerOffline,
                "The worker did not answer in time. Try again.",
            )),
        }
    }

    async fn finish_command(
        &self,
        command: &QueuedCommand,
        result: std::result::Result<Value, ErrorBody>,
    ) -> Result<()> {
        let request = RequestFrame {
            id: command.id.clone(),
            kind: command.kind.clone(),
            body: command.body.clone(),
        };
        let Ok(decoded) = request.decode::<HubToWorker>() else {
            return Ok(());
        };
        match (decoded, result) {
            (HubToWorker::SessionStart(start), Ok(value)) => {
                let started: SessionStartResult = serde_json::from_value(value)?;
                self.on_session_started(start.session_id, started).await
            }
            (HubToWorker::SessionStart(start), Err(error)) => {
                self.on_session_start_failed(start.session_id, error).await
            }
            (HubToWorker::SessionTurn(turn), Err(error)) => {
                self.on_turn_command_failed(turn.session_id, error).await
            }
            (HubToWorker::SessionRespond(respond), Err(error)) => {
                tracing::warn!(session_id = %respond.session_id, %error, "a worker refused an answer");
                self.on_answer_command_failed(respond.session_id, error)
                    .await
            }
            (other, Err(error)) => {
                tracing::warn!(kind = other.type_name(), %error, "a worker refused a command");
                Ok(())
            }
            (_, Ok(_)) => Ok(()),
        }
    }
}
