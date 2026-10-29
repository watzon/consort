//! The command queue. Every command for a worker is stored first. The hub
//! sends it when the worker is online, and deletes it when the worker has
//! answered. After a restart or a reconnect, the hub sends the unanswered
//! commands again. The worker runs a command ID once, so a repeat is safe.

use std::collections::HashSet;
use std::time::Duration;

use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{
    Frame, FrameError, FrameId, MAX_FRAME_BYTES, MessageFamily, RequestFrame,
};
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
    ///
    /// A command that does not fit in one frame could never go out, so it
    /// is refused here, and the transaction fails.
    pub fn queue_command(&mut self, worker_id: WorkerId, command: &HubToWorker) -> Result<FrameId> {
        let request = encode(FrameId::generate(), command)?;
        if let Err(FrameError::TooLarge { size }) = Frame::Request(request.clone()).to_text() {
            return Err(HubError::invalid(format!(
                "The {} command has {} KiB, and a frame can have at most {} KiB. Send less text at once.",
                request.kind,
                size.div_ceil(1024),
                MAX_FRAME_BYTES / 1024
            )));
        }
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
        // Expiry runs in the maintenance loop, not here: a worker that comes
        // back must not lose its commands at the moment it connects.
        let commands = self
            .store
            .read(move |conn| machine::commands_for_worker(conn, worker_id))
            .await?;
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
            Err(CallError::Encode(reason)) => {
                // Sending it again gives the same result, so the command
                // fails for good. Its failure handler tells the user.
                tracing::warn!(kind = %command.kind, worker_id = %worker_id, %reason, "a queued command does not encode, so it fails");
                let error = ErrorBody::new(
                    ErrorCode::Invalid,
                    "The hub could not send this to the worker, because it is too large.",
                );
                if let Err(err) = self.finish_command(&command, Err(error)).await {
                    tracing::warn!(error = %err, kind = %command.kind, "could not process a failed command");
                }
                let id = command.id.clone();
                if let Err(err) = self.store.write(move |tx| tx.delete_command(&id)).await {
                    tracing::warn!(error = %err, "could not delete a failed command");
                }
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

    /// Removes the queued commands that waited longer than their time to
    /// live (7 days) and ends what waited on them. The maintenance loop
    /// calls it. A session with an expired command is lost, its task fails,
    /// and an answer that never reached the worker shows Expired.
    pub(crate) async fn expire_queued_commands(&self) -> Result<()> {
        let expired = self.store.write(|tx| tx.expire_commands()).await?;
        let mut sessions = Vec::new();
        for command in &expired {
            tracing::warn!(kind = %command.kind, worker_id = %command.worker_id, "a queued command expired after 7 days");
            let request = RequestFrame {
                id: command.id.clone(),
                kind: command.kind.clone(),
                body: command.body.clone(),
            };
            if let Ok(HubToWorker::SessionRespond(respond)) = request.decode::<HubToWorker>()
                && let Err(err) = self
                    .answer_expired(respond.session_id, respond.request_id)
                    .await
            {
                tracing::warn!(error = %err, "could not mark an undelivered answer as expired");
            }
            if let Some(session_id) = command.session_id
                && !sessions.contains(&session_id)
            {
                sessions.push(session_id);
            }
        }
        for session_id in sessions {
            if let Err(err) = self.session_command_expired(session_id).await {
                tracing::warn!(error = %err, session_id = %session_id, "could not end a session with an expired command");
            }
        }
        Ok(())
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
            Err(CallError::Encode(reason)) => Err(HubError::internal(format!(
                "a direct request did not encode: {reason}"
            ))),
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use consort_proto::ids::{AccountId, ApprovalId, SessionId};
    use consort_proto::model::{
        Approval, ApprovalDetails, ApprovalStatus, Message, ModelOptions, PlacementRequest,
        Session, SessionRole, SessionState, TaskCreator, Usage,
    };
    use consort_proto::status::TaskStatus;
    use consort_proto::time::Timestamp;
    use consort_proto::worker_link::{RequestAnswer, SessionRespond};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::net::ws::Outbox;
    use crate::ops::hooks::NewTask;
    use crate::store::{conversation, work};

    #[tokio::test]
    async fn a_command_that_does_not_encode_fails_once_and_leaves_the_queue() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let worker_id = WorkerId::new();
        let (outbox, mut rx) = Outbox::new(CancellationToken::new());
        let link = WorkerLink::new(hub.links.next_conn_id(), worker_id, outbox);
        hub.links.register(Arc::new(link));
        // Larger than a frame. The size check in queue_command stops this,
        // so the test stores the row directly, as an older hub could have.
        let id = FrameId::generate();
        let now = Timestamp::now();
        let command = QueuedCommand {
            id: id.clone(),
            worker_id,
            session_id: None,
            kind: "inventory.refresh".to_owned(),
            body: json!({"pad": "x".repeat(MAX_FRAME_BYTES)}),
            created_at: now,
            expires_at: now.plus_millis(60_000),
        };
        hub.store
            .write(move |tx| tx.enqueue_command(&command))
            .await
            .unwrap();
        hub.pump_worker(worker_id);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let id = id.clone();
            let queued = hub
                .store
                .read(move |conn| machine::command(conn, &id))
                .await
                .unwrap();
            if queued.is_none() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the command stayed in the queue"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(rx.try_recv().is_err(), "nothing can have gone out");
    }

    #[tokio::test]
    async fn a_queued_command_that_is_too_large_is_refused() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let command = HubToWorker::SessionStop(consort_proto::worker_link::SessionStop {
            session_id: SessionId::new(),
            reason: "x".repeat(MAX_FRAME_BYTES),
        });
        let result = hub
            .store
            .write(move |tx| tx.queue_command(WorkerId::new(), &command))
            .await;
        assert_eq!(result.unwrap_err().code(), ErrorCode::Invalid);
    }

    #[tokio::test]
    async fn an_expired_answer_ends_the_session_fails_the_task_and_shows_expired() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let channel = hub
            .store
            .read(conversation::everything_channel)
            .await
            .unwrap();
        let task = hub
            .create_task(NewTask {
                channel_id: channel.id,
                title: "Wait".to_owned(),
                brief: "Wait for the worker.".to_owned(),
                model_selection: None,
                placement: PlacementRequest::default(),
                approval_mode: None,
                start_at: None,
                created_by: TaskCreator::User,
                routine_id: None,
            })
            .await
            .unwrap();
        let worker_id = WorkerId::new();
        let session = Session {
            id: SessionId::new(),
            role: SessionRole::Task,
            channel_id: channel.id,
            task_id: Some(task.id),
            worker_id,
            account_id: AccountId::new(),
            harness: "fake".to_owned(),
            model: "fake-1".to_owned(),
            options: ModelOptions::new(),
            workspace_path: "/tmp/workspace".to_owned(),
            native_ref: None,
            state: SessionState::Active,
            last_seq: 0,
            usage: Usage::default(),
            started_at: Timestamp::now(),
            ended_at: None,
            end_reason: None,
        };
        let session_id = session.id;
        let approval_id = ApprovalId::new();
        let task_id = task.id;
        let command_id = hub
            .store
            .write(move |tx| {
                tx.save_session(&session)?;
                let mut task = work::task_or_404(tx.db(), task_id)?;
                task.status = TaskStatus::Working;
                task.current_session_id = Some(session_id);
                task.worker_id = Some(worker_id);
                tx.save_task(&task)?;
                // The user allowed it, but the worker was away.
                tx.save_approval(&Approval {
                    id: approval_id,
                    channel_id: task.channel_id,
                    session_id,
                    task_id: Some(task_id),
                    message_id: task.card_message_id,
                    request_id: "req-1".into(),
                    summary: "Run tests".to_owned(),
                    details: ApprovalDetails::Command {
                        command: "cargo test".to_owned(),
                        cwd: None,
                    },
                    options: Vec::new(),
                    status: ApprovalStatus::Allowed,
                    answer_option_id: Some("allow".to_owned()),
                    answered_at: Some(tx.now()),
                    answered_by_client_id: None,
                    created_at: tx.now(),
                })?;
                let id = tx.queue_command(
                    worker_id,
                    &HubToWorker::SessionRespond(SessionRespond {
                        session_id,
                        request_id: "req-1".into(),
                        answer: RequestAnswer {
                            option_id: Some("allow".to_owned()),
                            text: None,
                        },
                    }),
                )?;
                // Seven days later.
                tx.db().execute(
                    "UPDATE commands SET expires_at = ?1 WHERE id = ?2",
                    rusqlite::params![tx.now().as_millis() - 1, id.as_str()],
                )?;
                Ok(id)
            })
            .await
            .unwrap();

        hub.expire_queued_commands().await.unwrap();

        let (session, task, approval, command, thread) = hub
            .store
            .read(move |conn| {
                Ok((
                    work::session_or_404(conn, session_id)?,
                    work::task_or_404(conn, task_id)?,
                    work::approval(conn, approval_id)?.unwrap(),
                    machine::command(conn, &command_id)?,
                    conversation::thread_replies(
                        conn,
                        work::task_or_404(conn, task_id)?.card_message_id,
                    )?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(session.state, SessionState::Lost);
        assert_eq!(task.status, TaskStatus::Failed);
        assert!(
            task.status_reason.as_deref().unwrap().contains("7 days"),
            "{:?}",
            task.status_reason
        );
        assert_eq!(approval.status, ApprovalStatus::Expired);
        assert!(command.is_none());
        assert!(
            thread
                .iter()
                .any(|m: &Message| m.body.contains("worker_away")),
            "no notice in the task thread"
        );
    }
}
