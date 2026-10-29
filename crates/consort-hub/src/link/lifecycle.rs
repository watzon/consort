//! A worker's life on the hub: hello, inventory, going offline, and what
//! happens when a command to a worker fails.

use consort_proto::auth::{SessionSeq, WorkerHello, WorkerHelloResult};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::ids::{SessionId, WorkerId};
use consort_proto::model::{Inventory, SessionRole, SessionState, WorkerState};
use consort_proto::status::TaskStatus;
use consort_proto::worker_link::SessionStartResult;

use super::events::{Ctx, Effects, finish_session};
use crate::error::Result;
use crate::hub::{HUB_VERSION, Hub};
use crate::store::coordinator as coord_store;
use crate::store::{machine, work};

impl Hub {
    /// Records a worker's hello. Returns the answer, and the sessions that the
    /// worker reported but the hub does not know, which the hub must stop.
    pub(crate) async fn worker_hello(
        &self,
        worker_id: WorkerId,
        hello: WorkerHello,
        protocol_version: u32,
    ) -> Result<(WorkerHelloResult, Vec<SessionId>)> {
        let hub = self.clone();
        let (result, unknown, fx) = self
            .store
            .write(move |tx| {
                let mut worker = machine::worker_or_404(tx.db(), worker_id)?;
                worker.version = hello.version.clone();
                worker.protocol_version = protocol_version;
                worker.state = WorkerState::Online;
                worker.last_seen_at = Some(tx.now());
                tx.save_worker(&worker)?;

                let mut known = Vec::new();
                let mut unknown = Vec::new();
                for reported in &hello.sessions {
                    match work::session(tx.db(), reported.session_id)? {
                        Some(session) if session.worker_id == worker_id => known.push(SessionSeq {
                            session_id: session.id,
                            last_seq: session.last_seq,
                        }),
                        _ => unknown.push(reported.session_id),
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
                    unknown,
                    fx,
                ))
            })
            .await?;
        self.run_effects(fx);
        Ok((result, unknown))
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

    /// Marks a worker offline when its connection ends.
    pub(crate) async fn worker_went_offline(&self, worker_id: WorkerId) -> Result<()> {
        self.store
            .write(move |tx| {
                if let Some(mut worker) = machine::worker(tx.db(), worker_id)?
                    && worker.state != WorkerState::Offline
                {
                    worker.state = WorkerState::Offline;
                    worker.last_seen_at = Some(tx.now());
                    tx.save_worker(&worker)?;
                }
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

    pub(crate) async fn on_session_started(
        &self,
        session_id: SessionId,
        started: SessionStartResult,
    ) -> Result<()> {
        self.store
            .write(move |tx| {
                let Some(mut session) = work::session(tx.db(), session_id)? else {
                    return Ok(());
                };
                session.workspace_path = started.workspace_path;
                if let Some(native) = started.native_ref {
                    session.native_ref = Some(native);
                }
                if session.state == SessionState::Starting {
                    session.state = SessionState::Active;
                }
                tx.save_session(&session)
            })
            .await
    }

    /// The worker could not start a session.
    pub(crate) async fn on_session_start_failed(
        &self,
        session_id: SessionId,
        error: ErrorBody,
    ) -> Result<()> {
        let hub = self.clone();
        let fx = self
            .store
            .write(move |tx| {
                let Some(session) = work::session(tx.db(), session_id)? else {
                    return Ok(Effects::default());
                };
                let mut ctx = Ctx::load(&hub, tx, session_id)?;
                ctx.session.end_reason = Some(error.message.clone());
                let was_final = ctx.session.state.is_final();
                if !was_final {
                    finish_session(tx, &mut ctx, SessionState::Lost)?;
                }
                if session.role == SessionRole::Coordinator {
                    // Do not kick: a start that fails would fail again at once.
                    ctx.fx.kick.clear();
                    tx.post_notice(
                        session.channel_id,
                        None,
                        None,
                        "assistant_unavailable",
                        &format!("The assistant could not start: {}", error.message),
                    )?;
                } else if let Some(task) = ctx.task.as_ref()
                    && task.status == TaskStatus::Failed
                {
                    // The task failed in finish_session; give the reason the worker's words.
                    let mut task = task.clone();
                    task.status_reason =
                        Some(format!("The session could not start: {}", error.message));
                    tx.save_task(&task)?;
                }
                ctx.persist(tx)?;
                Ok(ctx.fx)
            })
            .await?;
        self.run_effects(fx);
        Ok(())
    }

    /// The worker refused a `session.turn`.
    pub(crate) async fn on_turn_command_failed(
        &self,
        session_id: SessionId,
        error: ErrorBody,
    ) -> Result<()> {
        let session_gone = error.code == ErrorCode::NotFound;
        if session_gone {
            return self.mark_session_lost(session_id, &error.message).await;
        }
        let hub = self.clone();
        let fx = self
            .store
            .write(move |tx| {
                let Some(session) = work::session(tx.db(), session_id)? else {
                    return Ok(Effects::default());
                };
                let mut fx = Effects::default();
                match session.role {
                    SessionRole::Coordinator => {
                        let mut row = coord_store::coordinator(tx.db(), session.channel_id)?;
                        if row.session_id == Some(session_id) && row.turn_open {
                            if !row.turn_events.is_empty() {
                                tx.requeue_wakes_first(session.channel_id, &row.turn_events)?;
                            }
                            row.turn_open = false;
                            row.turn_events.clear();
                            tx.save_coordinator(&row)?;
                            tx.post_notice(
                                session.channel_id,
                                None,
                                None,
                                "assistant_unavailable",
                                &format!(
                                    "The assistant did not take your message: {}",
                                    error.message
                                ),
                            )?;
                        }
                        let mut runtime = work::session_runtime(tx.db(), session_id)?;
                        runtime.turn_open = false;
                        tx.save_session_runtime(session_id, &runtime)?;
                    }
                    SessionRole::Task => {
                        if let Some(task) = session
                            .task_id
                            .map(|t| work::task(tx.db(), t))
                            .transpose()?
                            .flatten()
                        {
                            tx.post_notice(
                                task.channel_id,
                                Some(task.card_message_id),
                                Some(task.id),
                                "message_not_sent",
                                &format!("The task did not take the message: {}", error.message),
                            )?;
                        }
                        let mut runtime = work::session_runtime(tx.db(), session_id)?;
                        runtime.turn_open = false;
                        tx.save_session_runtime(session_id, &runtime)?;
                    }
                }
                let _ = &hub;
                fx.pump.clear();
                Ok(fx)
            })
            .await?;
        self.run_effects(fx);
        Ok(())
    }

    /// The worker refused a `session.respond`.
    pub(crate) async fn on_answer_command_failed(
        &self,
        session_id: SessionId,
        error: ErrorBody,
    ) -> Result<()> {
        self.store
            .write(move |tx| {
                let Some(session) = work::session(tx.db(), session_id)? else {
                    return Ok(());
                };
                let task = session
                    .task_id
                    .map(|t| work::task(tx.db(), t))
                    .transpose()?
                    .flatten();
                let (root, task_id) =
                    task.map_or((None, None), |t| (Some(t.card_message_id), Some(t.id)));
                tx.post_notice(
                    session.channel_id,
                    root,
                    task_id,
                    "answer_not_delivered",
                    &format!("The session did not take your answer: {}", error.message),
                )?;
                Ok(())
            })
            .await
    }

    /// Runs once at start: the hub kicks coordinators with queued events.
    pub(crate) fn recover(&self) {
        self.retry_waiting_work();
    }
}
