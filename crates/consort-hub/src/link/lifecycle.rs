//! A worker's life on the hub: hello, inventory, going offline, and what
//! happens when a command to a worker fails.

use consort_proto::auth::{SessionSeq, WorkerHello, WorkerHelloResult};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::ids::{RequestId, SessionId, WorkerId};
use consort_proto::model::{
    ApprovalStatus, CoordinatorBlocked, CoordinatorBlockedKind, DecisionStatus, Inventory,
    SessionRole, SessionState, WorkerState,
};
use consort_proto::session_host_link::{SessionRespond, SessionStartResult, SessionSteer};
use consort_proto::status::{Actor, TaskStatus};

use super::events::{Ctx, Effects, finish_session, mark_lost_in_tx, queue_stop};
use crate::error::{HubError, Result};
use crate::hub::{HUB_VERSION, Hub};
use crate::ops::flow::{deliver_without_steer, transition};
use crate::ops::model_switch::detach_session;
use crate::store::coordinator as coord_store;
use crate::store::{Tx, machine, work};

impl Hub {
    /// Records a worker's hello. Returns the answer, and the sessions that the
    /// worker reported but must stop, each with the reason: sessions that the
    /// hub does not know, and sessions that have ended on the hub.
    pub(crate) async fn worker_hello(
        &self,
        worker_id: WorkerId,
        hello: WorkerHello,
        protocol_version: u32,
    ) -> Result<(WorkerHelloResult, Vec<(SessionId, String)>)> {
        let hub = self.clone();
        let (result, stops, fx) =
            self.store
                .write(move |tx| {
                    let mut worker = machine::worker_or_404(tx.db(), worker_id)?;
                    // The signature check refuses a revoked worker, but a revoke
                    // can come between that check and this hello.
                    if worker.revoked_at.is_some() {
                        return Err(HubError::unauthorized(
                            "This worker was removed. Pair this machine again.",
                        ));
                    }
                    worker.version = hello.version.clone();
                    worker.protocol_version = protocol_version;
                    worker.state = WorkerState::Online;
                    worker.last_seen_at = Some(tx.now());
                    tx.save_worker(&worker)?;

                    let mut known = Vec::new();
                    let mut stops = Vec::new();
                    for reported in &hello.sessions {
                        match work::session(tx.db(), reported.session_id)? {
                            Some(session)
                                if session.worker_id == worker_id && !session.state.is_final() =>
                            {
                                known.push(SessionSeq {
                                    session_id: session.id,
                                    last_seq: session.last_seq,
                                });
                            }
                            // The hub ended or lost this session, so it has no
                            // more turns. The harness must not keep running.
                            Some(session) if session.worker_id == worker_id => stops
                                .push((session.id, "The session has ended on the hub.".to_owned())),
                            _ => stops.push((
                                reported.session_id,
                                "The hub does not know this session.".to_owned(),
                            )),
                        }
                    }

                    // A session that the hub holds open but the worker no longer
                    // has is lost, unless its start command still waits.
                    let queued = machine::commands_for_worker(tx.db(), worker_id)?;
                    let mut fx = Effects::default();
                    for session in work::open_sessions_of_worker(tx.db(), worker_id)? {
                        let reported = hello.sessions.iter().any(|s| s.session_id == session.id);
                        let start_waits = queued
                            .iter()
                            .any(|c| c.kind == "session.start" && c.session_id == Some(session.id));
                        if reported || start_waits {
                            continue;
                        }
                        let mut ctx = Ctx::load(&hub, tx, session.id)?;
                        ctx.session.end_reason =
                            Some("The worker no longer has this session.".to_owned());
                        finish_session(tx, &mut ctx, SessionState::Lost)?;
                        ctx.persist(tx)?;
                        fx.merge(ctx.fx);
                    }
                    Ok((
                        WorkerHelloResult {
                            protocol_version,
                            hub_version: HUB_VERSION.to_owned(),
                            sessions: known,
                        },
                        stops,
                        fx,
                    ))
                })
                .await?;
        self.run_effects(fx);
        Ok((result, stops))
    }

    /// Runs after a worker has said hello and the hub answered.
    pub(crate) fn worker_came_online(&self, worker_id: WorkerId) {
        self.pump_worker(worker_id);
        self.retry_waiting_work();
    }

    /// Placement runs again for tasks in Up next, and coordinators with
    /// queued events try again.
    pub(crate) fn retry_waiting_work(&self) {
        let hub = self.clone();
        tokio::spawn(async move {
            if let Err(err) = hub.dispatch_waiting_tasks().await {
                tracing::warn!(error = %err, "could not place waiting tasks");
            }
            match hub.store.read(coord_store::channels_with_wakes).await {
                Ok(channels) => {
                    for channel in channels {
                        hub.kick_coordinator(channel);
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "could not list channels with queued events")
                }
            }
        });
    }

    /// Runs after the link of a new connection is registered. A revoke closes
    /// only registered links, so the hub reads the revoke time again now.
    /// Returns false when the worker was revoked: the caller closes the link.
    ///
    /// An old connection that ended during the hello of this one can have
    /// marked the worker offline. The worker is online again here.
    pub(crate) async fn worker_link_registered(&self, worker_id: WorkerId) -> Result<bool> {
        self.store
            .write(move |tx| {
                let Some(mut worker) = machine::worker(tx.db(), worker_id)? else {
                    return Ok(false);
                };
                if worker.revoked_at.is_some() {
                    return Ok(false);
                }
                if worker.state != WorkerState::Online {
                    worker.state = WorkerState::Online;
                    worker.last_seen_at = Some(tx.now());
                    tx.save_worker(&worker)?;
                }
                Ok(true)
            })
            .await
    }

    /// Marks a worker offline when its connection ends. A worker that has a
    /// newer registered link stays online. The check runs on the writer
    /// thread, so it is ordered with [`Hub::worker_link_registered`].
    pub(crate) async fn worker_went_offline(&self, worker_id: WorkerId) -> Result<()> {
        let hub = self.clone();
        self.store
            .write(move |tx| {
                if hub.links.get(worker_id).is_some() {
                    return Ok(());
                }
                if let Some(mut worker) = machine::worker(tx.db(), worker_id)?
                    && worker.state != WorkerState::Offline
                {
                    worker.state = WorkerState::Offline;
                    worker.last_seen_at = Some(tx.now());
                    tx.save_worker(&worker)?;
                }
                // A sign-in flow runs on the worker. Without the link, no
                // update can reach the hub, so the sign-in ends.
                tx.fail_sign_ins_of_worker(
                    worker_id,
                    crate::ops::accounts::SIGN_IN_WORKER_OFFLINE,
                )?;
                Ok(())
            })
            .await
    }

    /// Stores a new inventory.
    pub(crate) async fn update_inventory(
        &self,
        worker_id: WorkerId,
        inventory: Inventory,
    ) -> Result<()> {
        let changed =
            self.store
                .write(move |tx| {
                    let mut worker = machine::worker_or_404(tx.db(), worker_id)?;
                    let same = worker.inventory.as_ref().is_some_and(|old| {
                        !inventory.hash.is_empty() && old.hash == inventory.hash
                    });
                    if same {
                        return Ok(false);
                    }
                    worker.inventory = Some(inventory);
                    worker.last_seen_at = Some(tx.now());
                    tx.save_worker(&worker)?;
                    Ok(true)
                })
                .await?;
        if changed {
            self.retry_waiting_work();
        }
        Ok(())
    }

    // Results of commands ------------------------------------------------
    //
    // These run inside the transaction that takes the command from the
    // queue (see `link::commands`), so a result that arrives twice changes
    // the hub once.

    /// Ends the session of a queued command that expired. The worker was
    /// away for 7 days, so the hub stops waiting: the session is lost, its
    /// task fails, and the channel or the task thread gets a notice. A
    /// session that has already ended does not change.
    pub(crate) async fn session_command_expired(&self, session_id: SessionId) -> Result<()> {
        let hub = self.clone();
        let fx = self
            .store
            .write(move |tx| {
                let Some(session) = work::session(tx.db(), session_id)? else {
                    return Ok(Effects::default());
                };
                if session.state.is_final() {
                    return Ok(Effects::default());
                }
                let mut ctx = Ctx::load(&hub, tx, session_id)?;
                ctx.session.end_reason =
                    Some("The worker was offline for 7 days, so the hub stopped waiting.".to_owned());
                finish_session(tx, &mut ctx, SessionState::Lost)?;
                match ctx.task.as_ref() {
                    Some(task) => {
                        let text = "The worker was offline for 7 days, so the hub stopped waiting for this task. Retry it to run it on another worker.";
                        tx.post_notice(
                            task.channel_id,
                            Some(task.card_message_id),
                            Some(task.id),
                            "worker_away",
                            text,
                        )?;
                        ctx.set_failed_reason(text.to_owned());
                    }
                    None => {
                        tx.post_notice(
                            session.channel_id,
                            None,
                            None,
                            "worker_away",
                            "The worker of the assistant was offline for 7 days, so the hub ended its session there. The assistant starts a new session.",
                        )?;
                    }
                }
                ctx.persist(tx)?;
                Ok(ctx.fx)
            })
            .await?;
        self.run_effects(fx);
        Ok(())
    }

    /// The `session.respond` for an answer expired before the worker got it.
    /// The approval or decision then shows Expired, not the answer that the
    /// session never saw.
    pub(crate) async fn answer_expired(
        &self,
        session_id: SessionId,
        request_id: RequestId,
    ) -> Result<()> {
        self.store
            .write(move |tx| {
                if let Some(mut approval) =
                    work::approval_by_request(tx.db(), session_id, &request_id)?
                    && matches!(
                        approval.status,
                        ApprovalStatus::Allowed | ApprovalStatus::Denied
                    )
                {
                    approval.status = ApprovalStatus::Expired;
                    tx.save_approval(&approval)?;
                }
                if let Some(mut decision) =
                    work::decision_by_request(tx.db(), session_id, &request_id)?
                    && decision.status == DecisionStatus::Answered
                {
                    decision.status = DecisionStatus::Expired;
                    tx.save_decision(&decision)?;
                }
                Ok(())
            })
            .await
    }

    /// Runs once at start: the hub kicks coordinators with queued events.
    pub(crate) fn recover(&self) {
        self.retry_waiting_work();
    }
}

/// The worker started a session.
pub(crate) fn session_started_tx(
    tx: &mut Tx<'_>,
    session_id: SessionId,
    started: SessionStartResult,
) -> Result<Effects> {
    let mut fx = Effects::default();
    let Some(mut session) = work::session(tx.db(), session_id)? else {
        return Ok(fx);
    };
    session.workspace_path = started.workspace_path;
    if let Some(native) = started.native_ref {
        session.native_ref = Some(native);
    }
    if session.state == SessionState::Starting {
        session.state = SessionState::Active;
    }
    tx.save_session(&session)?;
    if session.role == SessionRole::Coordinator {
        fx.start_ok.push(session.channel_id);
    }
    Ok(fx)
}

/// The worker could not start a session.
pub(crate) fn session_start_failed_tx(
    hub: &Hub,
    tx: &mut Tx<'_>,
    session_id: SessionId,
    error: ErrorBody,
) -> Result<Effects> {
    let Some(session) = work::session(tx.db(), session_id)? else {
        return Ok(Effects::default());
    };
    let mut ctx = Ctx::load(hub, tx, session_id)?;
    ctx.session.end_reason = Some(error.message.clone());
    if !ctx.session.state.is_final() {
        finish_session(tx, &mut ctx, SessionState::Lost)?;
    }
    if error.code == ErrorCode::Conflict {
        // The worker has a session with this ID but no stored result, for
        // example after a resent start. The harness may still run, so it
        // must stop.
        let reason = "The session could not start, so the hub stopped it.";
        queue_stop(hub, tx, &mut ctx.fx, &ctx.session, reason)?;
    }
    if session.role == SessionRole::Coordinator {
        // Do not kick: a start that fails would fail again at once. The
        // channel waits longer before each new try.
        ctx.fx.kick.clear();
        ctx.fx.start_failed.push(session.channel_id);
        tx.set_coordinator_blocked(
            session.channel_id,
            Some(CoordinatorBlocked {
                kind: CoordinatorBlockedKind::StartFailed,
                reason: format!("The assistant could not start: {}", error.message),
                account: None,
            }),
        )?;
    } else {
        // The task failed in finish_session. Give the reason in the
        // worker's words. `persist` saves it.
        ctx.set_failed_reason(format!("The session could not start: {}", error.message));
    }
    ctx.persist(tx)?;
    Ok(ctx.fx)
}

/// The worker refused a `session.turn` that carried a model change, with
/// `unsupported`: the harness cannot switch its model in a running session,
/// or it refused the model. The turn did not run, so nothing is lost. The
/// old session ends, the text of the turn waits in the inbox, and a new
/// session on the new model takes it (see `ops::model_switch`). Without a
/// task that this session drives, it is an ordinary refused turn.
pub(crate) fn model_change_refused_tx(
    hub: &Hub,
    tx: &mut Tx<'_>,
    turn: &consort_proto::session_host_link::SessionTurn,
    error: ErrorBody,
) -> Result<Effects> {
    let session_id = turn.session_id;
    let Some(session) = work::session(tx.db(), session_id)? else {
        return Ok(Effects::default());
    };
    let drives = match session.task_id {
        Some(task_id) if session.role == SessionRole::Task && !session.state.is_final() => {
            work::task(tx.db(), task_id)?.is_some_and(|t| {
                t.current_session_id == Some(session_id)
                    && matches!(t.status, TaskStatus::Working | TaskStatus::NeedsInput)
            })
        }
        _ => false,
    };
    let Some(task_id) = session.task_id.filter(|_| drives) else {
        return turn_command_failed_tx(hub, tx, session_id, error);
    };
    tracing::info!(%session_id, %error, "the harness did not switch its model, so the task gets a new session");
    // The messages that waited behind this turn come after its text.
    let waiting = work::task_inbox(tx.db(), task_id)?;
    tx.clear_task_inbox(task_id)?;
    tx.push_task_inbox(task_id, &turn.turn.text, &turn.turn.attachments)?;
    for entry in waiting {
        tx.push_task_inbox(task_id, &entry.text, &entry.attachments)?;
    }
    // A task that moved from Ready for review for this turn stays Working:
    // the new session takes the text.
    let mut runtime = work::session_runtime(tx.db(), session_id)?;
    runtime.turn_open = false;
    runtime.turn_from_review = false;
    tx.save_session_runtime(session_id, &runtime)?;
    detach_session(hub, tx, task_id, None)
}

/// The worker refused a `session.turn`.
///
/// No turn runs then. A coordinator puts the events of the turn back in the
/// queue and tries again after the start backoff, or at once when the
/// session ends a turn. A task that the hub moved from Ready for review for
/// this turn goes back to Ready for review. Any other task that waits in
/// Working fails with Retry, because nothing would move it again.
pub(crate) fn turn_command_failed_tx(
    hub: &Hub,
    tx: &mut Tx<'_>,
    session_id: SessionId,
    error: ErrorBody,
) -> Result<Effects> {
    let Some(session) = work::session(tx.db(), session_id)? else {
        return Ok(Effects::default());
    };
    if error.code == ErrorCode::NotFound {
        // The worker does not have the session.
        let mut ctx = Ctx::load(hub, tx, session_id)?;
        mark_lost_in_tx(hub, tx, &mut ctx, &error.message)?;
        return Ok(ctx.fx);
    }
    let mut fx = Effects::default();
    let mut runtime = work::session_runtime(tx.db(), session_id)?;
    let from_review = runtime.turn_from_review;
    runtime.turn_open = false;
    runtime.turn_from_review = false;
    tx.save_session_runtime(session_id, &runtime)?;
    match session.role {
        SessionRole::Coordinator => {
            let mut row = coord_store::coordinator(tx.db(), session.channel_id)?;
            if row.session_id == Some(session_id) && row.turn_open {
                if !row.turn_events.is_empty() {
                    tx.requeue_wakes_first(session.channel_id, &row.turn_events)?;
                }
                row.turn_open = false;
                row.turn_started_at = None;
                row.turn_events.clear();
                tx.save_coordinator(&row)?;
                tx.set_coordinator_blocked(
                    session.channel_id,
                    Some(CoordinatorBlocked {
                        kind: CoordinatorBlockedKind::StartFailed,
                        reason: format!(
                            "The assistant did not take your message: {} The hub tries again soon.",
                            error.message
                        ),
                        account: None,
                    }),
                )?;
                fx.turn_refused.push(session.channel_id);
            }
        }
        SessionRole::Task => {
            let Some(mut task) = session
                .task_id
                .map(|t| work::task(tx.db(), t))
                .transpose()?
                .flatten()
            else {
                return Ok(fx);
            };
            tx.post_notice(
                task.channel_id,
                Some(task.card_message_id),
                Some(task.id),
                "message_not_sent",
                &format!("The task did not take the message: {}", error.message),
            )?;
            let current = task.current_session_id == Some(session_id) && !session.state.is_final();
            if current && task.status == TaskStatus::Working {
                let (to, reason) = if from_review {
                    (
                        TaskStatus::Review,
                        "The task did not take your last message. Send it again.".to_owned(),
                    )
                } else {
                    (
                        TaskStatus::Failed,
                        format!(
                            "The worker did not start the next turn of the task: {} Retry the task.",
                            error.message
                        ),
                    )
                };
                if transition(tx, &mut task, to, Actor::System, Some(reason), None)? {
                    tx.save_task(&task)?;
                    fx.kick.push(task.channel_id);
                }
            }
        }
    }
    Ok(fx)
}

/// The worker refused a `session.steer`, for example because the turn
/// ended on the worker before the steer arrived. The text must still reach
/// the task: it waits in the inbox while the hub sees the turn open, or it
/// starts a new turn. When the task cannot take it, the thread gets a
/// `message_not_sent` notice.
pub(crate) fn steer_command_failed_tx(
    hub: &Hub,
    tx: &mut Tx<'_>,
    steer: &SessionSteer,
    error: &ErrorBody,
) -> Result<Effects> {
    let mut fx = Effects::default();
    let Some(session) = work::session(tx.db(), steer.session_id)? else {
        return Ok(fx);
    };
    let Some(task) = session
        .task_id
        .map(|t| work::task(tx.db(), t))
        .transpose()?
        .flatten()
    else {
        tracing::warn!(session_id = %session.id, %error, "a worker refused a steer for a session with no task");
        return Ok(fx);
    };
    let result = if task.current_session_id == Some(session.id) {
        deliver_without_steer(
            hub,
            tx,
            task.id,
            &steer.turn.text,
            &steer.turn.attachments,
            Actor::User,
        )
    } else {
        Err(HubError::conflict("The session of this task has ended."))
    };
    match result {
        Ok(delivered) => {
            fx.pump.push(delivered.worker_id);
            fx.switch_tasks.extend(delivered.switch_task);
            if delivered.status_changed {
                fx.kick.push(task.channel_id);
            }
        }
        Err(HubError::Api(refusal)) => {
            tx.post_notice(
                task.channel_id,
                Some(task.card_message_id),
                Some(task.id),
                "message_not_sent",
                &format!(
                    "The task did not take your message: {} {}",
                    error.message, refusal.message
                ),
            )?;
        }
        Err(other) => return Err(other),
    }
    Ok(fx)
}

/// The worker refused a `session.respond`.
///
/// With `not_found`, the harness has closed the request already, for
/// example because the turn ended or the request timed out on its side. The
/// approval or decision shows Canceled, and the thread gets a notice. The
/// task status does not change here: `turn.ended` or the session state
/// decides it.
///
/// With any other error, the harness will not go on with that request, so a
/// task that waits on it fails, and the user can retry it. Without this, the
/// task would stay in Working for ever.
pub(crate) fn answer_command_failed_tx(
    tx: &mut Tx<'_>,
    respond: &SessionRespond,
    error: &ErrorBody,
) -> Result<Effects> {
    let mut fx = Effects::default();
    let session_id = respond.session_id;
    let Some(session) = work::session(tx.db(), session_id)? else {
        return Ok(fx);
    };
    let task = session
        .task_id
        .map(|t| work::task(tx.db(), t))
        .transpose()?
        .flatten();
    let (root, task_id) = task
        .as_ref()
        .map_or((None, None), |t| (Some(t.card_message_id), Some(t.id)));
    if error.code == ErrorCode::NotFound {
        if let Some(mut approval) =
            work::approval_by_request(tx.db(), session_id, &respond.request_id)?
            && matches!(
                approval.status,
                ApprovalStatus::Allowed | ApprovalStatus::Denied
            )
        {
            approval.status = ApprovalStatus::Canceled;
            tx.save_approval(&approval)?;
        }
        if let Some(mut decision) =
            work::decision_by_request(tx.db(), session_id, &respond.request_id)?
            && decision.status == DecisionStatus::Answered
        {
            decision.status = DecisionStatus::Canceled;
            tx.save_decision(&decision)?;
        }
        tx.post_notice(
            session.channel_id,
            root,
            task_id,
            "answer_not_delivered",
            &format!(
                "The session did not take your answer, because the request was already closed: {}",
                error.message
            ),
        )?;
        return Ok(fx);
    }
    let text = format!("The session did not take your answer: {}", error.message);
    tx.post_notice(
        session.channel_id,
        root,
        task_id,
        "answer_not_delivered",
        &text,
    )?;
    let Some(mut task) = task else {
        return Ok(fx);
    };
    let waits = task.current_session_id == Some(session_id)
        && matches!(task.status, TaskStatus::Working | TaskStatus::NeedsInput);
    if !waits {
        return Ok(fx);
    }
    let reason = format!("{text} Retry the task.");
    if transition(
        tx,
        &mut task,
        TaskStatus::Failed,
        Actor::System,
        Some(reason),
        None,
    )? {
        tx.save_task(&task)?;
        fx.kick.push(task.channel_id);
    }
    Ok(fx)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use consort_proto::model::{Os, Worker};
    use consort_proto::time::Timestamp;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::link::WorkerLink;
    use crate::net::ws::Outbox;

    async fn online_worker(hub: &Hub) -> WorkerId {
        let worker = Worker {
            kind: Default::default(),
            id: WorkerId::new(),
            name: "Laptop".to_owned(),
            hostname: "laptop".to_owned(),
            os: Os::Macos,
            arch: "aarch64".to_owned(),
            public_key: consort_proto::auth::PublicKey::from_bytes([1; 32]),
            version: String::new(),
            protocol_version: 1,
            tags: Vec::new(),
            paused: false,
            state: WorkerState::Online,
            last_seen_at: None,
            inventory: None,
            paired_at: Timestamp::now(),
            revoked_at: None,
        };
        let id = worker.id;
        hub.store
            .write(move |tx| tx.save_worker(&worker))
            .await
            .unwrap();
        id
    }

    async fn state_of(hub: &Hub, id: WorkerId) -> WorkerState {
        hub.store
            .read(move |conn| machine::worker_or_404(conn, id))
            .await
            .unwrap()
            .state
    }

    #[tokio::test]
    async fn an_old_connection_that_ends_late_does_not_take_a_new_one_offline() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let id = online_worker(&hub).await;
        // The new connection has registered its link.
        let (outbox, _rx) = Outbox::new(CancellationToken::new());
        let link = Arc::new(WorkerLink::new(hub.links.next_conn_id(), id, outbox));
        hub.links.register(link.clone());
        // The old connection ends after that.
        hub.worker_went_offline(id).await.unwrap();
        assert_eq!(state_of(&hub, id).await, WorkerState::Online);
        // With no link, the worker goes offline.
        hub.links.remove(id, link.conn_id);
        hub.worker_went_offline(id).await.unwrap();
        assert_eq!(state_of(&hub, id).await, WorkerState::Offline);
        // An old connection that ended before the new link registered set the
        // worker offline. The registration sets it online again.
        hub.links.register(link);
        assert!(hub.worker_link_registered(id).await.unwrap());
        assert_eq!(state_of(&hub, id).await, WorkerState::Online);
    }

    #[tokio::test]
    async fn a_revoked_worker_cannot_say_hello_and_its_link_is_refused() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let id = online_worker(&hub).await;
        hub.store
            .write(move |tx| {
                let mut worker = machine::worker_or_404(tx.db(), id)?;
                worker.revoked_at = Some(tx.now());
                worker.state = WorkerState::Offline;
                tx.save_worker(&worker)
            })
            .await
            .unwrap();
        let hello = WorkerHello {
            worker_id: id,
            version: "test".to_owned(),
            protocol_versions: vec![1],
            sessions: Vec::new(),
        };
        let error = hub.worker_hello(id, hello, 1).await.unwrap_err();
        assert_eq!(error.code(), ErrorCode::Unauthorized);
        assert!(!hub.worker_link_registered(id).await.unwrap());
        assert_eq!(state_of(&hub, id).await, WorkerState::Offline);
    }
}
