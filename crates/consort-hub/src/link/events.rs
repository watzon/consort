//! Session events: how the hub turns what a session does into its own
//! state. One batch runs in one transaction, so the stored `last_seq`, the
//! items, the task status and the feed always agree.

use std::time::Instant;

use consort_proto::ids::{ApprovalId, ChannelId, DecisionId, ItemId, SessionId, WorkerId};
use consort_proto::model::{
    Approval, ApprovalDetails, ApprovalStatus, AuthState, Author, COMMAND_OUTPUT_TAIL_BYTES,
    Channel, Decision, DecisionOption, DecisionStatus, ErrorPayload, ItemPayload, ItemStatus,
    MessageKind, Session, SessionItem, SessionRole, SessionState, Task, tail_of,
};
use consort_proto::status::{Actor, TaskStatus};
use consort_proto::tools::HubTool;
use consort_proto::turn_input::WakeEvent;
use consort_proto::worker_link::{
    HubToWorker, RequestBody, RequestOpened, RequestOutcome, SequencedEvent, SessionEvent,
    SessionEvents, SessionEventsAck, SessionStop, SessionTurn, TurnEndReason, TurnInput,
    contiguous_seq,
};

use super::EmitNow;
use crate::error::{HubError, Result};
use crate::hub::Hub;
use crate::limits::{
    MESSAGE_BODY_MAX_BYTES, NOTICE_TEXT_MAX_BYTES, REVIEW_SUMMARY_MAX_BYTES, TURN_TEXT_MAX_BYTES,
    cut_text, json_len,
};
use crate::ops::flow::transition;
use crate::ops::waiting::resume_if_nothing_waits;
use crate::store::Tx;
use crate::store::conversation::{self, NewMessage};
use crate::store::coordinator as coord_store;
use crate::store::work::{self, SessionRuntime};
use crate::util::truncate_chars;

/// The `code` of the error event that says that the harness account of a
/// session is signed out. The worker sends it (`consort-harness` has the
/// same constant).
pub(crate) const SIGNED_OUT_CODE: &str = "signed_out";
/// The `code` of the error event that a worker sends when it could not get
/// an attachment or a channel file for a session. The hub also posts it as
/// a notice, so the user sees which file the session did not get.
pub(crate) const ATTACHMENT_FAILED_CODE: &str = "attachment_failed";

/// Work to do after the transaction commits.
#[derive(Default)]
pub(crate) struct Effects {
    pub kick: Vec<ChannelId>,
    pub pump: Vec<WorkerId>,
    pub flush_items: Vec<(SessionId, ItemId, ChannelId, std::time::Duration)>,
    pub items_done: Vec<(SessionId, ItemId)>,
    /// Channels whose coordinator session could not start. They wait
    /// longer before the next start.
    pub start_failed: Vec<ChannelId>,
    /// Channels whose coordinator session started.
    pub start_ok: Vec<ChannelId>,
    /// Channels whose coordinator turn the worker refused. They try again
    /// after the start backoff.
    pub turn_refused: Vec<ChannelId>,
}

impl Effects {
    fn kick(&mut self, channel: ChannelId) {
        if !self.kick.contains(&channel) {
            self.kick.push(channel);
        }
    }

    fn pump(&mut self, worker: WorkerId) {
        if !self.pump.contains(&worker) {
            self.pump.push(worker);
        }
    }

    pub(crate) fn merge(&mut self, other: Effects) {
        for c in other.kick {
            self.kick(c);
        }
        for w in other.pump {
            self.pump(w);
        }
        self.flush_items.extend(other.flush_items);
        self.items_done.extend(other.items_done);
        self.start_failed.extend(other.start_failed);
        self.start_ok.extend(other.start_ok);
        self.turn_refused.extend(other.turn_refused);
    }
}

/// The state that event handlers change. `persist` writes what changed.
pub(crate) struct Ctx<'h> {
    pub hub: &'h Hub,
    pub session: Session,
    pub channel: Channel,
    pub task: Option<Task>,
    pub runtime: SessionRuntime,
    pub fx: Effects,
    task_dirty: bool,
}

impl<'h> Ctx<'h> {
    pub(crate) fn load(hub: &'h Hub, tx: &Tx<'_>, session_id: SessionId) -> Result<Self> {
        let session = work::session_or_404(tx.db(), session_id)?;
        let channel = conversation::channel_or_404(tx.db(), session.channel_id)?;
        let task = match (session.role, session.task_id) {
            (SessionRole::Task, Some(id)) => work::task(tx.db(), id)?,
            _ => None,
        };
        let runtime = work::session_runtime(tx.db(), session_id)?;
        Ok(Self {
            hub,
            session,
            channel,
            task,
            runtime,
            fx: Effects::default(),
            task_dirty: false,
        })
    }

    pub(crate) fn persist(&mut self, tx: &mut Tx<'_>) -> Result<()> {
        tx.save_session(&self.session)?;
        tx.save_session_runtime(self.session.id, &self.runtime)?;
        if self.task_dirty
            && let Some(task) = &self.task
        {
            tx.save_task(task)?;
        }
        self.task_dirty = false;
        Ok(())
    }

    /// Sets the status reason of a failed task of this session. `persist`
    /// saves it. Saving the task in another way would be overwritten there.
    pub(crate) fn set_failed_reason(&mut self, reason: String) {
        if let Some(task) = self.task.as_mut()
            && task.status == TaskStatus::Failed
        {
            task.status_reason = Some(cut_text(&reason, crate::limits::NOTICE_TEXT_MAX_BYTES));
            self.task_dirty = true;
        }
    }

    /// Reads the task again from the database, for code that must see
    /// changes from other code in this transaction. A change in this
    /// context that is not saved yet is saved first, so it is not lost.
    fn reload_task(&mut self, tx: &mut Tx<'_>) -> Result<()> {
        self.with_task_row(tx, |_, _| Ok(()))
    }

    /// Saves a changed task now and reads it again after `f`, for code that
    /// changes the task row itself, such as [`resume_if_nothing_waits`].
    /// Without this, `persist` would write the older copy over the change.
    fn with_task_row(
        &mut self,
        tx: &mut Tx<'_>,
        f: impl FnOnce(&mut Tx<'_>, consort_proto::ids::TaskId) -> Result<()>,
    ) -> Result<()> {
        let Some(task_id) = self.task.as_ref().map(|t| t.id) else {
            return Ok(());
        };
        if self.task_dirty
            && let Some(task) = &self.task
        {
            tx.save_task(task)?;
            self.task_dirty = false;
        }
        f(tx, task_id)?;
        self.task = work::task(tx.db(), task_id)?;
        Ok(())
    }

    /// True when this session runs the task now: the task names it as its
    /// current session, and the session has not ended. Events of an older
    /// session (for example one that a retry replaced) must not change the
    /// task.
    fn drives_task(&self) -> bool {
        !self.session.state.is_final()
            && self
                .task
                .as_ref()
                .is_some_and(|t| t.current_session_id == Some(self.session.id))
    }

    /// True when this session may open approvals and decisions and end
    /// turns that change the hub: a task session that drives its task, or
    /// the current coordinator session of the channel.
    fn is_current(&self, tx: &Tx<'_>) -> Result<bool> {
        if self.session.state.is_final() {
            return Ok(false);
        }
        Ok(match self.session.role {
            SessionRole::Task => self.drives_task(),
            SessionRole::Coordinator => {
                coord_store::coordinator(tx.db(), self.channel.id)?.session_id
                    == Some(self.session.id)
            }
        })
    }

    /// Moves the task of this session, if it has one.
    fn move_task(
        &mut self,
        tx: &mut Tx<'_>,
        to: TaskStatus,
        reason: Option<String>,
        wake_text: Option<String>,
    ) -> Result<()> {
        let Some(task) = self.task.as_mut() else {
            return Ok(());
        };
        match transition(tx, task, to, Actor::System, reason, wake_text) {
            Ok(changed) => {
                if changed {
                    self.task_dirty = true;
                    if to.wakes_coordinator() {
                        self.fx.kick(task.channel_id);
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, task_id = %task.id, "ignored a status change that the rules do not allow");
            }
        }
        Ok(())
    }
}

impl Hub {
    /// Stores a batch of session events and returns the ack.
    pub(crate) async fn ingest_events(
        &self,
        worker_id: WorkerId,
        batch: SessionEvents,
    ) -> Result<SessionEventsAck> {
        let session_id = batch.session_id;
        let hub = self.clone();
        let (last_seq, fx) = self
            .store
            .write(move |tx| apply_batch(&hub, tx, worker_id, batch))
            .await?;
        self.run_effects(fx);
        Ok(SessionEventsAck {
            session_id,
            last_seq,
        })
    }

    pub(crate) fn run_effects(&self, fx: Effects) {
        for (session, item) in fx.items_done {
            self.links.item_done(session, item);
        }
        for channel in fx.start_ok {
            self.coordinators.start_worked(channel);
        }
        for channel in fx.start_failed {
            self.coordinators.start_failed(channel);
        }
        for channel in fx.turn_refused {
            // The worker refused the turn, so the events wait in the queue.
            // Try again after the start backoff. A `turn.ended` of the
            // session kicks the channel sooner.
            self.coordinators.start_failed(channel);
            self.retry_coordinator_later(channel);
        }
        for channel in fx.kick {
            self.kick_coordinator(channel);
        }
        for worker in fx.pump {
            self.pump_worker(worker);
        }
        for (session, item, channel, delay) in fx.flush_items {
            let hub = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let result = hub
                    .store
                    .write(move |tx| {
                        if let Some(item) = work::session_item(tx.db(), session, item)? {
                            tx.emit_item(&item, channel)?;
                        }
                        Ok(())
                    })
                    .await;
                if let Err(err) = result {
                    tracing::warn!(error = %err, "could not emit a merged item update");
                }
                hub.links.item_flushed(session, item, Instant::now());
            });
        }
    }

    /// Records that a coordinator session called a hub tool in its open turn.
    pub(crate) async fn note_tool_call(
        &self,
        ctx: &crate::hub::ToolCtx,
        tool: HubTool,
    ) -> Result<()> {
        if ctx.session.role != SessionRole::Coordinator {
            return Ok(());
        }
        let session_id = ctx.session.id;
        self.store
            .write(move |tx| {
                let mut runtime = work::session_runtime(tx.db(), session_id)?;
                runtime.tools_called.push(tool.name().to_owned());
                tx.save_session_runtime(session_id, &runtime)
            })
            .await
    }
}

fn apply_batch(
    hub: &Hub,
    tx: &mut Tx<'_>,
    worker_id: WorkerId,
    batch: SessionEvents,
) -> Result<(u64, Effects)> {
    let session = work::session(tx.db(), batch.session_id)?
        .filter(|s| s.worker_id == worker_id)
        .ok_or_else(|| HubError::not_found("The hub does not know this session. Stop it."))?;
    let stored = session.last_seq;
    let new_last = contiguous_seq(stored, &batch.events);
    if new_last == stored {
        return Ok((stored, Effects::default()));
    }
    let mut events: Vec<&SequencedEvent> = batch
        .events
        .iter()
        .filter(|e| e.seq > stored && e.seq <= new_last)
        .collect();
    events.sort_by_key(|e| e.seq);
    events.dedup_by_key(|e| e.seq);

    let mut ctx = Ctx::load(hub, tx, batch.session_id)?;
    ctx.runtime.active_at = Some(tx.now());
    // Only the newest copy of an item in a batch matters.
    let last_item_index: std::collections::HashMap<ItemId, usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match &e.event {
            SessionEvent::Item(item) => Some((item.id, i)),
            _ => None,
        })
        .collect();
    for (index, event) in events.iter().enumerate() {
        if let SessionEvent::Item(item) = &event.event
            && last_item_index.get(&item.id) != Some(&index)
        {
            continue;
        }
        apply_event(tx, &mut ctx, &event.event)?;
    }
    ctx.session.last_seq = new_last;
    ctx.persist(tx)?;
    Ok((new_last, ctx.fx))
}

fn apply_event(tx: &mut Tx<'_>, ctx: &mut Ctx<'_>, event: &SessionEvent) -> Result<()> {
    match event {
        SessionEvent::State { state } => on_state(tx, ctx, *state),
        SessionEvent::NativeRef { native_ref } => {
            ctx.session.native_ref = Some(native_ref.clone());
            Ok(())
        }
        SessionEvent::TurnStarted { turn_id } => {
            ctx.runtime.turn_id = Some(turn_id.clone());
            ctx.runtime.turn_open = true;
            ctx.runtime.last_assistant_text = None;
            if matches!(
                ctx.session.state,
                SessionState::Starting | SessionState::Idle
            ) {
                ctx.session.state = SessionState::Active;
            }
            Ok(())
        }
        SessionEvent::Item(item) => on_item(tx, ctx, item.clone()),
        SessionEvent::OutputDelta { item_id, text } => on_delta(tx, ctx, *item_id, text),
        SessionEvent::RequestOpened(opened) => open_request(tx, ctx, opened),
        SessionEvent::RequestClosed {
            request_id,
            outcome,
        } => close_request(tx, ctx, request_id, *outcome),
        SessionEvent::Usage(usage) => {
            ctx.session.usage = usage.clone();
            Ok(())
        }
        SessionEvent::TurnEnded { reason, error, .. } => {
            on_turn_ended(tx, ctx, *reason, error.as_deref())
        }
        SessionEvent::Reopened => on_reopened(tx, ctx),
        SessionEvent::Error { code, message } => {
            if code == SIGNED_OUT_CODE {
                on_signed_out(tx, ctx, message)?;
            }
            if code == ATTACHMENT_FAILED_CODE {
                // The user sees it where the files went: in the task thread,
                // or in the channel of a coordinator.
                let (thread, task) = match ctx.task.as_ref() {
                    Some(task) => (Some(task.card_message_id), Some(task.id)),
                    None => (None, None),
                };
                tx.post_notice(
                    ctx.channel.id,
                    thread,
                    task,
                    ATTACHMENT_FAILED_CODE,
                    message,
                )?;
            }
            let item = SessionItem {
                id: ItemId::new(),
                session_id: ctx.session.id,
                turn_id: ctx.runtime.turn_id.clone().unwrap_or_else(|| "".into()),
                payload: ItemPayload::Error(ErrorPayload {
                    message: message.clone(),
                    code: Some(code.clone()),
                }),
                status: ItemStatus::Failed,
                created_at: tx.now(),
                updated_at: tx.now(),
            };
            tx.save_item(&item, ctx.channel.id)
        }
    }
}

fn on_state(tx: &mut Tx<'_>, ctx: &mut Ctx<'_>, state: SessionState) -> Result<()> {
    if ctx.session.state.is_final() {
        return Ok(());
    }
    if state.is_final() {
        // `finish_session` reads the state before this one.
        return finish_session(tx, ctx, state);
    }
    ctx.session.state = state;
    Ok(())
}

/// The worker restarted and opened the session again. The old harness
/// process is gone, so its open turn and its harness requests are gone too.
///
/// A turn whose `session.turn` still waits in the queue is not lost: the
/// worker never got it, and the hub sends it after this event. That turn
/// stays open, and a coordinator keeps the events of the turn.
fn on_reopened(tx: &mut Tx<'_>, ctx: &mut Ctx<'_>) -> Result<()> {
    if ctx.session.state.is_final() {
        return Ok(());
    }
    cancel_harness_requests(tx, ctx)?;
    if ctx.runtime.turn_open {
        if turn_waits_unsent(tx, ctx.session.id)? || turn_sent_on_this_connection(ctx) {
            return Ok(());
        }
        let text = match ctx.session.role {
            SessionRole::Coordinator => {
                "The worker restarted before the assistant finished. Send your message again."
            }
            SessionRole::Task => "The worker restarted during this turn. Retry the task.",
        };
        on_turn_ended(tx, ctx, TurnEndReason::Failed, Some(text))?;
    } else if ctx.session.role == SessionRole::Task && ctx.drives_task() {
        // A canceled request can leave the task in Working with no turn.
        // The session waits now, as after the end of a turn.
        task_turn_ended(tx, ctx, TurnEndReason::Completed, None)?;
    }
    Ok(())
}

/// True when the hub sent the open turn of the session on the connection
/// that the worker has now. After a reconnect the hub flushes its queue at
/// once, so the turn can go out before the worker's `reopened` event is
/// handled. That turn is new to the restarted worker. Only a turn from an
/// earlier connection was in flight when the worker restarted.
fn turn_sent_on_this_connection(ctx: &Ctx<'_>) -> bool {
    let current = ctx.hub.links.get(ctx.session.worker_id).map(|l| l.conn_id);
    current.is_some() && ctx.hub.links.turn_sent_on(ctx.session.id) == current
}

/// True when the open turn of a session is one that the hub has not sent
/// yet: its `session.turn` is still in the queue and never went out. The
/// hub never sends a second `session.turn` while a turn is open, so a
/// queued one is the one that opened the turn.
fn turn_waits_unsent(tx: &Tx<'_>, session_id: SessionId) -> Result<bool> {
    Ok(
        crate::store::machine::commands_for_session(tx.db(), session_id)?
            .iter()
            .any(|(command, sent)| command.kind == "session.turn" && !sent),
    )
}

/// Cancels the approvals and the decisions of a session that wait on a
/// harness request. Nobody can answer them once the request is gone.
/// Decisions from `ask_decision` stay: they need no harness request, and
/// their answer is input to a later turn (or, for a coordinator, to the
/// channel queue). A task with nothing left to wait for goes back to
/// Working.
fn cancel_harness_requests(tx: &mut Tx<'_>, ctx: &mut Ctx<'_>) -> Result<()> {
    let (approvals, decisions) = work::pending_for_session(tx.db(), ctx.session.id)?;
    let mut canceled = false;
    // A hub card needs no harness request: the hub answers it, and the
    // answer reaches the channel or the task, not the session.
    for mut approval in approvals.into_iter().filter(|a| !a.is_hub_card()) {
        approval.status = ApprovalStatus::Canceled;
        tx.save_approval(&approval)?;
        canceled = true;
    }
    for mut decision in decisions {
        if decision.request_id.is_none() {
            continue;
        }
        decision.status = DecisionStatus::Canceled;
        tx.save_decision(&decision)?;
        canceled = true;
    }
    if canceled && ctx.drives_task() {
        ctx.with_task_row(tx, |tx, task_id| resume_if_nothing_waits(tx, Some(task_id)))?;
    }
    Ok(())
}

/// Called when a session ended or was lost, whoever noticed it.
pub(crate) fn finish_session(
    tx: &mut Tx<'_>,
    ctx: &mut Ctx<'_>,
    state: SessionState,
) -> Result<()> {
    let lost = state == SessionState::Lost;
    ctx.hub.links.forget_turn(ctx.session.id);
    // A session that never left Starting did not start.
    let started = ctx.session.state != SessionState::Starting;
    ctx.session.ended_at.get_or_insert_with(|| tx.now());
    if ctx.session.end_reason.is_none() {
        ctx.session.end_reason = Some(if lost { "lost" } else { "ended" }.to_owned());
    }
    // Nobody can answer a harness request of a session that is gone. This
    // runs while the session still drives its task, so a task with nothing
    // left to wait for goes back to Working first.
    cancel_harness_requests(tx, ctx)?;
    ctx.session.state = state;
    ctx.runtime.turn_open = false;

    match ctx.session.role {
        SessionRole::Coordinator => release_coordinator(tx, ctx, started)?,
        SessionRole::Task => {
            let current = ctx
                .task
                .as_ref()
                .is_some_and(|t| t.current_session_id == Some(ctx.session.id));
            let running = ctx
                .task
                .as_ref()
                .is_some_and(|t| matches!(t.status, TaskStatus::Working | TaskStatus::NeedsInput));
            if current && running {
                let reason = if lost {
                    "The session was lost. Retry the task."
                } else {
                    "The session ended before the task finished. Retry the task."
                };
                ctx.move_task(tx, TaskStatus::Failed, Some(reason.to_owned()), None)?;
            } else if current
                && let Some(task) = ctx.task.as_mut()
                && task.status == TaskStatus::Review
            {
                // The task stays Ready for review, but it cannot take a
                // message any more. Retry starts a new session.
                task.status_reason = Some(
                    "The session of this task ended, so it cannot take a message. Retry the task to work on it again."
                        .to_owned(),
                );
                task.updated_at = tx.now();
                ctx.task_dirty = true;
            }
        }
    }
    Ok(())
}

/// The coordinator row stops pointing at a session that is gone. Events of an
/// open turn go back to the front of the queue, but only when the turn
/// called no hub tool. A turn that called one may have acted already, for
/// example posted a reply or started a task. A new session must not do that
/// again, so the events go, and the channel gets a notice. The channel gets
/// a new session at once only when this one had started: a start that
/// failed would fail again at once. The next wake event, or a worker that
/// comes online, starts it then, after the backoff in `Coordinators`.
fn release_coordinator(tx: &mut Tx<'_>, ctx: &mut Ctx<'_>, started: bool) -> Result<()> {
    let mut row = coord_store::coordinator(tx.db(), ctx.channel.id)?;
    if row.session_id != Some(ctx.session.id) {
        return Ok(());
    }
    if row.turn_open && !row.turn_events.is_empty() {
        if ctx.runtime.tools_called.is_empty() {
            tx.requeue_wakes_first(ctx.channel.id, &row.turn_events)?;
        } else {
            tx.post_notice(
                ctx.channel.id,
                None,
                None,
                "turn_failed",
                "The assistant stopped before it finished. It had already started to act, so the hub does not give it the same messages again. Check what it did, and send your message again if something is missing.",
            )?;
            tx.end_turn_runs(
                &row.turn_events,
                ctx.channel.id,
                ctx.session.id,
                row.turn_started_at,
                Some("The assistant stopped before it finished. Check what it did, then select Run now if something is missing."),
            )?;
            tx.finish_proactive_turn(
                &ctx.channel,
                ctx.session.id,
                &row,
                TurnEndReason::Failed,
                Some("The assistant stopped before it finished."),
            )?;
        }
    }
    row.session_id = None;
    row.turn_open = false;
    row.turn_started_at = None;
    row.turn_events.clear();
    tx.save_coordinator(&row)?;
    if started {
        ctx.fx.kick(ctx.channel.id);
    }
    Ok(())
}

/// The harness account of a session is signed out. The hub marks the
/// account as signed out in the worker's inventory, so placement does not
/// choose it again until the worker reports it signed in. A coordinator
/// session on that account cannot answer, so the hub replaces it.
fn on_signed_out(tx: &mut Tx<'_>, ctx: &mut Ctx<'_>, detail: &str) -> Result<()> {
    mark_account_signed_out(tx, &ctx.session, detail)?;
    if ctx.session.role == SessionRole::Coordinator && ctx.is_current(tx)? {
        replace_signed_out_coordinator(tx, ctx)?;
    }
    Ok(())
}

/// The label of the session's harness account and the name of its worker,
/// for notices.
fn account_and_worker(tx: &Tx<'_>, session: &Session) -> Result<(String, String)> {
    let worker = crate::store::machine::worker(tx.db(), session.worker_id)?;
    let label = worker
        .as_ref()
        .and_then(|w| w.inventory.as_ref())
        .and_then(|inv| inv.account(session.account_id))
        .map_or_else(|| session.harness.clone(), |a| a.label.clone());
    let name = worker.map_or_else(|| "its worker".to_owned(), |w| w.name);
    Ok((label, name))
}

fn mark_account_signed_out(tx: &mut Tx<'_>, session: &Session, detail: &str) -> Result<()> {
    let Some(mut worker) = crate::store::machine::worker(tx.db(), session.worker_id)? else {
        return Ok(());
    };
    let Some(inventory) = worker.inventory.take() else {
        return Ok(());
    };
    let mut inventory = inventory;
    let mut changed = false;
    if let Some(account) = inventory
        .accounts
        .iter_mut()
        .find(|a| a.account_id == session.account_id)
        && account.auth_state != AuthState::SignedOut
    {
        account.auth_state = AuthState::SignedOut;
        account.auth_detail = Some(cut_text(detail, NOTICE_TEXT_MAX_BYTES));
        changed = true;
    }
    // A new hash, so the next inventory from the worker replaces this one
    // even when the worker did not see the change.
    worker.inventory = Some(if changed {
        inventory.with_hash()
    } else {
        inventory
    });
    if changed {
        tx.save_worker(&worker)?;
    }
    Ok(())
}

/// True when the stored inventory says that the session's account is signed
/// out.
fn account_signed_out(tx: &Tx<'_>, session: &Session) -> Result<bool> {
    Ok(crate::store::machine::worker(tx.db(), session.worker_id)?
        .and_then(|w| w.inventory)
        .and_then(|inv| inv.account(session.account_id).map(|a| a.auth_state))
        == Some(AuthState::SignedOut))
}

/// Ends a coordinator session whose account is signed out. The events of
/// its turn go back to the queue, and placement chooses a new session on an
/// account that is signed in. When no account fits, the events wait, and the
/// notice says where to sign in. A sign-in sends a new inventory, and that
/// starts the coordinator again.
fn replace_signed_out_coordinator(tx: &mut Tx<'_>, ctx: &mut Ctx<'_>) -> Result<()> {
    let reason = "The account of the session is signed out.";
    ctx.session.end_reason = Some(reason.to_owned());
    finish_session(tx, ctx, SessionState::Lost)?;
    queue_stop(ctx.hub, tx, &mut ctx.fx, &ctx.session, reason)?;
    let (label, worker) = account_and_worker(tx, &ctx.session)?;
    let text = if coordinator_can_start(tx, &ctx.channel)? {
        // Also when the session never became active: the cause is known,
        // and another account can take the events now.
        ctx.fx.kick(ctx.channel.id);
        format!(
            "{label} on {worker} is signed out, so the assistant moves to another account. Sign in to {label} on {worker} to use it again."
        )
    } else {
        // Nothing can start now. The events wait for a sign-in.
        ctx.fx.kick.retain(|c| *c != ctx.channel.id);
        format!(
            "The assistant cannot answer, because {label} on {worker} is signed out. Sign in to {label} on {worker}. The assistant then answers your waiting messages."
        )
    };
    tx.post_notice(ctx.channel.id, None, None, "assistant_signed_out", &text)?;
    Ok(())
}

/// True when placement finds a worker and an account for a new coordinator
/// session of the channel now.
fn coordinator_can_start(tx: &Tx<'_>, channel: &Channel) -> Result<bool> {
    let tutti = conversation::tutti_channel(tx.db())?;
    let workers = crate::store::machine::workers(tx.db())?;
    let open = work::open_session_counts(tx.db())?;
    let selection = channel.effective_coordinator_selection(&tutti).cloned();
    let request = consort_proto::model::PlacementRequest::default();
    Ok(crate::placement::place(&crate::placement::PlacementInput {
        subject: crate::placement::Subject::Coordinator,
        workers: &workers,
        open_sessions: &open,
        selection: selection.as_ref(),
        request: &request,
        approval_mode: channel.default_approval_mode,
        preferred_workers: &channel.preferred_workers,
        channel_tags: &channel.tags,
        repos: &[],
        isolation: consort_proto::model::Isolation::Worktree,
    })
    .is_ok())
}

fn on_item(tx: &mut Tx<'_>, ctx: &mut Ctx<'_>, mut item: SessionItem) -> Result<()> {
    item.session_id = ctx.session.id;
    match &mut item.payload {
        ItemPayload::AssistantMessage(message) => {
            ctx.runtime.last_assistant_text = Some(message.text.clone());
        }
        ItemPayload::Command(command) => {
            let (tail, cut) = tail_of(&command.output, COMMAND_OUTPUT_TAIL_BYTES);
            if cut {
                command.output = tail.to_owned();
                command.output_truncated = true;
            }
        }
        _ => {}
    }
    tx.save_item_row(&item)?;
    emit_item(tx, ctx, &item)
}

fn on_delta(tx: &mut Tx<'_>, ctx: &mut Ctx<'_>, item_id: ItemId, text: &str) -> Result<()> {
    let Some(mut item) = work::session_item(tx.db(), ctx.session.id, item_id)? else {
        tracing::debug!(item_id = %item_id, "output arrived for an item that the hub does not have");
        return Ok(());
    };
    if let ItemPayload::Command(command) = &mut item.payload {
        command.append_output(text);
        item.updated_at = tx.now();
        tx.save_item_row(&item)?;
        emit_item(tx, ctx, &item)?;
    }
    Ok(())
}

/// Emits a feed entry for an item now, or later when the item changed less
/// than 250 ms ago. Later updates in the window merge into one entry.
fn emit_item(tx: &mut Tx<'_>, ctx: &mut Ctx<'_>, item: &SessionItem) -> Result<()> {
    let channel = ctx.channel.id;
    if item.status != ItemStatus::InProgress {
        ctx.fx.items_done.push((item.session_id, item.id));
        return tx.emit_item(item, channel);
    }
    match ctx
        .hub
        .links
        .item_emit_decision(item.session_id, item.id, Instant::now())
    {
        EmitNow::Yes => tx.emit_item(item, channel),
        EmitNow::Later(delay) => {
            ctx.fx
                .flush_items
                .push((item.session_id, item.id, channel, delay));
            Ok(())
        }
        EmitNow::Scheduled => Ok(()),
    }
}

/// The details of an approval from a worker. Only the hub makes hub cards,
/// so a hub card kind from a worker reads as `other` with the summary, and
/// the hub never carries out its answer itself.
fn worker_approval_details(details: &ApprovalDetails, summary: &str) -> ApprovalDetails {
    if details.is_hub_card() {
        tracing::warn!(
            kind = details.kind().as_str(),
            "a worker sent an approval with a hub card kind, so the hub reads it as other"
        );
        ApprovalDetails::Other {
            text: Some(summary.to_owned()),
        }
    } else {
        details.clone()
    }
}

fn open_request(tx: &mut Tx<'_>, ctx: &mut Ctx<'_>, opened: &RequestOpened) -> Result<()> {
    let session_id = ctx.session.id;
    // Nobody can answer a request of a session that the hub ended or
    // replaced, and it must not move the task of a newer session.
    if !ctx.is_current(tx)? {
        tracing::info!(session_id = %session_id, request_id = %opened.request_id, "a session that is not current opened a request, so the hub ignores it");
        return Ok(());
    }
    if work::approval_by_request(tx.db(), session_id, &opened.request_id)?
        .is_some_and(|a| a.status == ApprovalStatus::Pending)
        || work::decision_by_request(tx.db(), session_id, &opened.request_id)?
            .is_some_and(|d| d.status == DecisionStatus::Pending)
    {
        return Ok(());
    }
    let (kind, body) = match &opened.body {
        RequestBody::Approval(_) => (MessageKind::Approval, opened.summary.clone()),
        RequestBody::Question(q) => (MessageKind::Decision, q.question.clone()),
    };
    // The text comes from the harness, so the hub cuts it and does not refuse it.
    let body = cut_text(&body, MESSAGE_BODY_MAX_BYTES);
    let card = match &ctx.task {
        Some(task) => {
            tx.post_in_task_thread(task, Some(&ctx.session), Author::Assistant, kind, &body)?
        }
        None => {
            let mut new = NewMessage::new(ctx.channel.id, Author::Assistant, kind, body);
            new.author_session_id = Some(session_id);
            tx.insert_message(new)?
        }
    };
    let task_id = ctx.task.as_ref().map(|t| t.id);
    let now = tx.now();
    match &opened.body {
        RequestBody::Approval(details) => {
            tx.save_approval(&Approval {
                id: ApprovalId::new(),
                channel_id: ctx.channel.id,
                session_id,
                task_id,
                message_id: card.id,
                request_id: opened.request_id.clone(),
                summary: opened.summary.clone(),
                details: worker_approval_details(details, &opened.summary),
                options: opened.options.clone(),
                status: ApprovalStatus::Pending,
                answer_option_id: None,
                answered_at: None,
                answered_by_client_id: None,
                created_at: now,
            })?;
        }
        RequestBody::Question(question) => {
            tx.save_decision(&Decision {
                id: DecisionId::new(),
                channel_id: ctx.channel.id,
                task_id,
                session_id,
                message_id: card.id,
                request_id: Some(opened.request_id.clone()),
                question: question.question.clone(),
                context: question.context.clone(),
                options: opened
                    .options
                    .iter()
                    .map(|o| DecisionOption {
                        label: o.label.clone(),
                        consequence: o.description.clone().unwrap_or_default(),
                        id: Some(o.id.clone()),
                    })
                    .collect(),
                recommended_index: None,
                allow_free_text: question.allow_free_text,
                status: DecisionStatus::Pending,
                answer: None,
                answered_at: None,
                created_at: now,
            })?;
        }
    }
    let wake_text = format!("Waiting for the user: {}", opened.summary);
    ctx.move_task(
        tx,
        TaskStatus::NeedsInput,
        Some(opened.summary.clone()),
        Some(wake_text),
    )
}

fn close_request(
    tx: &mut Tx<'_>,
    ctx: &mut Ctx<'_>,
    request_id: &consort_proto::ids::RequestId,
    outcome: RequestOutcome,
) -> Result<()> {
    let session_id = ctx.session.id;
    if let Some(mut approval) = work::approval_by_request(tx.db(), session_id, request_id)?
        && approval.status == ApprovalStatus::Pending
    {
        approval.status = match outcome {
            RequestOutcome::Allowed | RequestOutcome::Answered => ApprovalStatus::Allowed,
            RequestOutcome::Denied => ApprovalStatus::Denied,
            RequestOutcome::Canceled => ApprovalStatus::Canceled,
            RequestOutcome::Expired => ApprovalStatus::Expired,
        };
        tx.save_approval(&approval)?;
    } else if let Some(mut decision) = work::decision_by_request(tx.db(), session_id, request_id)?
        && decision.status == DecisionStatus::Pending
    {
        decision.status = match outcome {
            RequestOutcome::Answered | RequestOutcome::Allowed | RequestOutcome::Denied => {
                DecisionStatus::Answered
            }
            RequestOutcome::Canceled => DecisionStatus::Canceled,
            RequestOutcome::Expired => DecisionStatus::Expired,
        };
        tx.save_decision(&decision)?;
    }
    if !ctx.drives_task() {
        return Ok(());
    }
    // The status may have changed earlier in this batch. The helper saves
    // that change before it reads the task again.
    ctx.with_task_row(tx, |tx, task_id| resume_if_nothing_waits(tx, Some(task_id)))
}

fn on_turn_ended(
    tx: &mut Tx<'_>,
    ctx: &mut Ctx<'_>,
    reason: TurnEndReason,
    error: Option<&str>,
) -> Result<()> {
    ctx.runtime.turn_open = false;
    ctx.runtime.turn_from_review = false;
    match ctx.session.role {
        SessionRole::Coordinator => coordinator_turn_ended(tx, ctx, reason, error),
        SessionRole::Task => task_turn_ended(tx, ctx, reason, error),
    }
}

fn coordinator_turn_ended(
    tx: &mut Tx<'_>,
    ctx: &mut Ctx<'_>,
    reason: TurnEndReason,
    error: Option<&str>,
) -> Result<()> {
    let mut row = coord_store::coordinator(tx.db(), ctx.channel.id)?;
    if row.session_id != Some(ctx.session.id) {
        return Ok(());
    }
    if !row.turn_open {
        // A turn that the hub did not open ended, for example the one that
        // made the worker refuse the hub's turn. The session is free now,
        // so the events that wait can go.
        ctx.fx.kick(ctx.channel.id);
        return Ok(());
    }
    // A failed turn on an account that the worker reports as signed out
    // will fail again. A new session goes on another account.
    if reason == TurnEndReason::Failed
        && !ctx.session.state.is_final()
        && account_signed_out(tx, &ctx.session)?
    {
        return replace_signed_out_coordinator(tx, ctx);
    }
    // Which thread did the turn answer? One user message in a thread means a
    // reply in that thread.
    let thread = {
        let threads: Vec<_> = row
            .turn_events
            .iter()
            .filter_map(|e| match e {
                WakeEvent::UserMessage { thread, .. } => Some(*thread),
                _ => None,
            })
            .collect();
        match threads.as_slice() {
            [Some(root)] => Some(*root),
            _ => None,
        }
    };
    let called = ctx
        .runtime
        .tools_called
        .iter()
        .filter_map(|name| HubTool::from_name(name));
    // A proactive turn that called no posting tool stays silent: its text is
    // dropped, not posted.
    let proactive = crate::proactivity::turn::classify(&row.turn_events).is_proactive();
    if reason == TurnEndReason::Completed
        && !proactive
        && consort_proto::tools::needs_fallback_post(called)
    {
        let text = ctx
            .runtime
            .last_assistant_text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        if let Some(text) = text {
            let text = cut_text(text, MESSAGE_BODY_MAX_BYTES);
            let mut new =
                NewMessage::new(ctx.channel.id, Author::Assistant, MessageKind::Text, text);
            new.author_session_id = Some(ctx.session.id);
            new.thread_root_id = thread;
            tx.insert_message(new)?;
        }
    }
    let failure = match reason {
        TurnEndReason::Completed => None,
        TurnEndReason::Interrupted => Some("The turn was stopped before it finished."),
        TurnEndReason::Failed => {
            Some(error.unwrap_or("The assistant stopped with an error. Send your message again."))
        }
    };
    if reason == TurnEndReason::Failed {
        let text = error.unwrap_or("The assistant stopped with an error. Send your message again.");
        tx.post_notice(ctx.channel.id, None, None, "turn_failed", text)?;
    }
    tx.end_turn_runs(
        &row.turn_events,
        ctx.channel.id,
        ctx.session.id,
        row.turn_started_at,
        failure,
    )?;
    tx.finish_proactive_turn(&ctx.channel, ctx.session.id, &row, reason, failure)?;
    row.turn_open = false;
    row.turn_started_at = None;
    row.turn_events.clear();
    tx.save_coordinator(&row)?;
    ctx.fx.kick(ctx.channel.id);
    Ok(())
}

fn task_turn_ended(
    tx: &mut Tx<'_>,
    ctx: &mut Ctx<'_>,
    reason: TurnEndReason,
    error: Option<&str>,
) -> Result<()> {
    let Some(task_id) = ctx.task.as_ref().map(|t| t.id) else {
        return Ok(());
    };
    // Reload: other code in this transaction may have changed the task. A
    // change of an earlier event in this batch, for example a request that
    // moved the task to Needs input, is saved first.
    ctx.reload_task(tx)?;
    // The turn of an older session, for example one that a retry replaced,
    // must not move the task or take its inbox.
    if !ctx.drives_task() {
        ctx.runtime.review_summary = None;
        return Ok(());
    }
    let status = ctx.task.as_ref().map_or(TaskStatus::Resolved, |t| t.status);
    if !matches!(status, TaskStatus::Working | TaskStatus::NeedsInput) {
        ctx.runtime.review_summary = None;
        return Ok(());
    }
    if reason == TurnEndReason::Failed {
        let text = error
            .unwrap_or("The turn failed. Retry the task.")
            .to_owned();
        ctx.runtime.review_summary = None;
        return ctx.move_task(tx, TaskStatus::Failed, Some(text), None);
    }

    // Messages that waited for the turn to end start the next turn. They go
    // in order while the text fits in one turn. The rest wait for the turn
    // after it.
    let inbox = work::task_inbox(tx.db(), task_id)?;
    if !inbox.is_empty() && !ctx.session.state.is_final() {
        let (taken, turn) = inbox_batch(&inbox, TURN_TEXT_MAX_BYTES);
        tx.delete_task_inbox(&taken)?;
        let channel_files =
            tx.channel_files_for_turn(ctx.session.id, ctx.channel.id, Some(task_id))?;
        tx.queue_command(
            ctx.session.worker_id,
            &HubToWorker::SessionTurn(SessionTurn {
                session_id: ctx.session.id,
                turn,
                channel_files,
            }),
        )?;
        ctx.runtime.turn_open = true;
        ctx.runtime.turn_from_review = false;
        ctx.runtime.active_at = Some(tx.now());
        ctx.fx.pump(ctx.session.worker_id);
        return Ok(());
    }

    if work::pending_count_for_task(tx.db(), task_id)? == 0 {
        if status == TaskStatus::NeedsInput {
            // The last request closed in a way that did not move the task,
            // for example an answer that the harness no longer took.
            // Nothing waits now, so the task goes on as after any turn.
            ctx.move_task(tx, TaskStatus::Working, None, None)?;
        }
        let last = ctx
            .runtime
            .last_assistant_text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| truncate_chars(t, 2000));
        let (summary, reason_text) = match reason {
            TurnEndReason::Interrupted => (
                last.unwrap_or_else(|| "The turn was stopped before it finished.".to_owned()),
                Some("Stopped before it finished.".to_owned()),
            ),
            _ => (
                ctx.runtime
                    .review_summary
                    .clone()
                    .or(last)
                    .unwrap_or_else(|| "The task finished its turn.".to_owned()),
                None,
            ),
        };
        // The summary comes from the session, so the hub cuts it.
        let summary = cut_text(&summary, REVIEW_SUMMARY_MAX_BYTES);
        if let Some(task) = ctx.task.as_mut() {
            task.review_summary = Some(summary.clone());
        }
        ctx.move_task(tx, TaskStatus::Review, reason_text, Some(summary))?;
        // The summary changed even if the status did not.
        ctx.task_dirty = true;
    }
    ctx.runtime.review_summary = None;
    Ok(())
}

/// Queues `session.stop` for a session that the hub ended, so the harness
/// on the worker stops too. A removed worker gets nothing: its commands are
/// gone, and it cannot connect again.
///
/// The other queued commands of the session have no use now, so they go.
/// Only the command that is in flight stays: the pump sends the oldest
/// command of a session, and it waits for its answer. When the
/// `session.start` of the session never went out, the worker does not know
/// the session, so the start goes and no stop is queued.
pub(crate) fn queue_stop(
    hub: &Hub,
    tx: &mut Tx<'_>,
    fx: &mut Effects,
    session: &Session,
    reason: &str,
) -> Result<()> {
    let removed = crate::store::machine::worker(tx.db(), session.worker_id)?
        .is_none_or(|w| w.revoked_at.is_some());
    if removed {
        return Ok(());
    }
    let queued = crate::store::machine::commands_for_session(tx.db(), session.id)?;
    let inflight = hub
        .links
        .get(session.worker_id)
        .is_some_and(|link| link.is_inflight(Some(session.id)));
    let mut worker_knows = true;
    for (index, (command, sent)) in queued.iter().enumerate() {
        if inflight && index == 0 {
            continue;
        }
        if command.kind == "session.start" && !sent {
            worker_knows = false;
        }
        tx.delete_command(&command.id)?;
    }
    if !worker_knows {
        return Ok(());
    }
    tx.queue_command(
        session.worker_id,
        &HubToWorker::SessionStop(SessionStop {
            session_id: session.id,
            reason: reason.to_owned(),
        }),
    )?;
    fx.pump(session.worker_id);
    Ok(())
}

/// Ends a session that is still open because the hub no longer needs it,
/// for example the old session of a task that is retried. Its pending
/// requests are canceled and the worker gets `session.stop`. Returns the
/// work to do after the commit.
pub(crate) fn end_session_by_hub(
    hub: &Hub,
    tx: &mut Tx<'_>,
    session_id: SessionId,
    reason: &str,
) -> Result<Effects> {
    let mut ctx = Ctx::load(hub, tx, session_id)?;
    if ctx.session.state.is_final() {
        return Ok(Effects::default());
    }
    ctx.session.end_reason = Some(reason.to_owned());
    finish_session(tx, &mut ctx, SessionState::Ended)?;
    queue_stop(hub, tx, &mut ctx.fx, &ctx.session, reason)?;
    ctx.persist(tx)?;
    Ok(ctx.fx)
}

/// The inbox messages that go into one turn: their row IDs, and the turn
/// with the joined text and all their attachments. A first message that
/// alone does not fit is cut.
fn inbox_batch(inbox: &[work::InboxMessage], room: usize) -> (Vec<i64>, TurnInput) {
    const GAP: &str = "\n\n";
    let mut taken = Vec::new();
    let mut text = String::new();
    let mut attachments = Vec::new();
    let mut used = 0;
    for entry in inbox {
        let (id, message) = (&entry.id, &entry.text);
        let cost = json_len(message) + if taken.is_empty() { 0 } else { json_len(GAP) };
        if used + cost > room {
            if taken.is_empty() {
                taken.push(*id);
                text = cut_text(message, room);
                attachments.extend(entry.attachments.iter().cloned());
            }
            break;
        }
        if !taken.is_empty() {
            text.push_str(GAP);
        }
        text.push_str(message);
        attachments.extend(entry.attachments.iter().cloned());
        used += cost;
        taken.push(*id);
    }
    (taken, TurnInput { text, attachments })
}

/// Marks a loaded session as lost because the hub decided so, and saves it.
/// The worker may still run the harness, so it gets `session.stop`.
pub(crate) fn mark_lost_in_tx(
    hub: &Hub,
    tx: &mut Tx<'_>,
    ctx: &mut Ctx<'_>,
    reason: &str,
) -> Result<()> {
    if ctx.session.state.is_final() {
        return Ok(());
    }
    ctx.session.end_reason = Some(reason.to_owned());
    finish_session(tx, ctx, SessionState::Lost)?;
    queue_stop(hub, tx, &mut ctx.fx, &ctx.session, reason)?;
    ctx.persist(tx)
}

/// A channel got new tags. When its coordinator session runs on a worker
/// that does not have every tag now, the hub ends the session, as it does
/// when the account of the session signs out. The next turn then goes to a
/// worker that qualifies (placement reads the tags). The channel gets a
/// notice that says why. Tasks that run keep running: only new work follows
/// the tags. Returns the work to do after the commit.
pub(crate) fn move_coordinator_off_unqualified_worker(
    hub: &Hub,
    tx: &mut Tx<'_>,
    channel: &Channel,
) -> Result<Effects> {
    let Some(session_id) = coord_store::coordinator(tx.db(), channel.id)?.session_id else {
        return Ok(Effects::default());
    };
    let Some(session) = work::session(tx.db(), session_id)? else {
        return Ok(Effects::default());
    };
    let Some(worker) = crate::store::machine::worker(tx.db(), session.worker_id)? else {
        return Ok(Effects::default());
    };
    let missing: Vec<String> = channel
        .tags
        .iter()
        .filter(|tag| !worker.has_tags(std::slice::from_ref(*tag)))
        .cloned()
        .collect();
    if missing.is_empty() || session.state.is_final() {
        return Ok(Effects::default());
    }
    let mut ctx = Ctx::load(hub, tx, session_id)?;
    mark_lost_in_tx(
        hub,
        tx,
        &mut ctx,
        "The channel now needs a tag that this worker does not have.",
    )?;
    // The channel gets a new coordinator session on a worker with the tags.
    // When no worker has them, the start says what to do.
    ctx.fx.kick(channel.id);
    let (noun, wanted) = match missing.as_slice() {
        [only] => ("tag", only.clone()),
        [init @ .., last] => ("tags", format!("{} and {last}", init.join(", "))),
        [] => return Ok(ctx.fx),
    };
    tx.post_notice(
        channel.id,
        None,
        None,
        "assistant_moved",
        &format!(
            "The assistant moved off {} because the channel now needs the {noun} {wanted}.",
            worker.name
        ),
    )?;
    Ok(ctx.fx)
}

/// Marks a session as lost from outside an event batch, for example when a
/// worker no longer has it.
impl Hub {
    pub(crate) async fn mark_session_lost(
        &self,
        session_id: SessionId,
        reason: &str,
    ) -> Result<()> {
        let hub = self.clone();
        let reason = reason.to_owned();
        let fx = self
            .store
            .write(move |tx| {
                let mut ctx = Ctx::load(&hub, tx, session_id)?;
                if ctx.session.state.is_final() {
                    return Ok(Effects::default());
                }
                mark_lost_in_tx(&hub, tx, &mut ctx, &reason)?;
                Ok(ctx.fx)
            })
            .await?;
        self.run_effects(fx);
        Ok(())
    }

    /// The sessions of a worker that the hub thinks are open.
    pub(crate) async fn open_sessions_of(&self, worker_id: WorkerId) -> Result<Vec<Session>> {
        self.store
            .read(move |conn| work::open_sessions_of_worker(conn, worker_id))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use consort_proto::worker_link::{SessionStart, WorkspaceSpec};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::link::WorkerLink;
    use crate::net::ws::Outbox;
    use crate::store::machine;

    /// Only the hub makes hub cards. A worker that sends a hub card kind
    /// gets an ordinary approval, which the hub answers through the
    /// harness.
    #[test]
    fn a_worker_cannot_open_a_hub_card() {
        use consort_proto::model::ManagementCard;
        let forged = ApprovalDetails::Management(Box::new(ManagementCard {
            tool: "update_connection".into(),
            change: "Use GitHub Work in every channel".into(),
            effect: "It reaches every channel".into(),
            arguments: serde_json::Value::Null,
        }));
        let read = worker_approval_details(&forged, "Allow this?");
        assert_eq!(
            read,
            ApprovalDetails::Other {
                text: Some("Allow this?".into())
            }
        );
        assert!(!read.is_hub_card());
        let command = ApprovalDetails::Command {
            command: "ls".into(),
            cwd: None,
        };
        assert_eq!(worker_approval_details(&command, "Run ls"), command);
    }

    fn start(session: &Session) -> HubToWorker {
        HubToWorker::SessionStart(SessionStart {
            session_id: session.id,
            role: SessionRole::Task,
            channel_id: session.channel_id,
            task_id: session.task_id,
            account_id: session.account_id,
            model: session.model.clone(),
            options: session.options.clone(),
            approval_mode: consort_proto::model::ApprovalMode::Ask,
            workspace: WorkspaceSpec {
                repos: Vec::new(),
                isolation: consort_proto::model::Isolation::Worktree,
                carry_uncommitted: false,
            },
            system_prompt_append: String::new(),
            allowed_local_mcp_servers: Vec::new(),
            channel_files: Vec::new(),
            extensions: Default::default(),
            resume: None,
            turn: None,
        })
    }

    fn turn(session: &Session) -> HubToWorker {
        HubToWorker::SessionTurn(SessionTurn {
            session_id: session.id,
            turn: TurnInput {
                text: "More work".to_owned(),
                attachments: Vec::new(),
            },
            channel_files: None,
        })
    }

    async fn queued_kinds(hub: &Hub, session: &Session) -> Vec<String> {
        let id = session.id;
        hub.store
            .read(move |conn| machine::commands_for_session(conn, id))
            .await
            .unwrap()
            .into_iter()
            .map(|(c, _)| c.kind)
            .collect()
    }

    /// Finding 9: queued commands of a session that the hub ended still
    /// went out before `session.stop`.
    #[tokio::test]
    async fn ending_a_session_whose_start_never_went_out_drops_the_start_and_sends_no_stop() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let (_, session) = crate::hub::test_task_session(&hub).await;
        let queued = session.clone();
        hub.store
            .write(move |tx| {
                tx.queue_command(queued.worker_id, &start(&queued))?;
                tx.queue_command(queued.worker_id, &turn(&queued))?;
                Ok(())
            })
            .await
            .unwrap();
        hub.mark_session_lost(session.id, "The test ends it.")
            .await
            .unwrap();
        assert!(queued_kinds(&hub, &session).await.is_empty());
    }

    #[tokio::test]
    async fn ending_a_session_keeps_the_command_in_flight_and_drops_the_rest() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let (_, session) = crate::hub::test_task_session(&hub).await;
        let (outbox, _rx) = Outbox::new(CancellationToken::new());
        let link = Arc::new(WorkerLink::new(
            hub.links.next_conn_id(),
            session.worker_id,
            outbox,
        ));
        hub.links.register(link.clone());
        let queued = session.clone();
        hub.store
            .write(move |tx| {
                let id = tx.queue_command(queued.worker_id, &start(&queued))?;
                tx.mark_command_sent(&id)?;
                tx.queue_command(queued.worker_id, &turn(&queued))?;
                Ok(())
            })
            .await
            .unwrap();
        // The pump sends the oldest command of the session and waits.
        assert!(link.mark_inflight(Some(session.id)));
        hub.mark_session_lost(session.id, "The test ends it.")
            .await
            .unwrap();
        assert_eq!(
            queued_kinds(&hub, &session).await,
            ["session.start", "session.stop"]
        );
    }

    /// Review 3, finding 2: a turn that ended while the task was in Needs
    /// input with nothing pending left the task there for ever.
    #[tokio::test]
    async fn a_turn_that_ends_in_needs_input_with_nothing_pending_goes_to_review() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let (task, session) = crate::hub::test_task_session(&hub).await;
        let task_id = task.id;
        hub.store
            .write(move |tx| {
                let mut task = work::task_or_404(tx.db(), task_id)?;
                task.status = TaskStatus::NeedsInput;
                tx.save_task(&task)
            })
            .await
            .unwrap();
        hub.ingest_events(
            session.worker_id,
            SessionEvents {
                session_id: session.id,
                events: vec![SequencedEvent {
                    seq: 1,
                    at: consort_proto::time::Timestamp::now(),
                    event: SessionEvent::TurnEnded {
                        turn_id: "t1".into(),
                        reason: TurnEndReason::Completed,
                        error: None,
                    },
                }],
            },
        )
        .await
        .unwrap();
        let task = hub
            .store
            .read(move |conn| work::task_or_404(conn, task_id))
            .await
            .unwrap();
        assert_eq!(task.status, TaskStatus::Review);
    }

    /// Review 3, finding 4: a channel delete queued `session.stop` and left
    /// the `session.start` that never went out, so the worker still
    /// started a coordinator for a channel that no longer existed.
    #[tokio::test]
    async fn deleting_a_channel_drops_a_coordinator_start_that_never_went_out() {
        use consort_proto::client_api::{ChannelCreate, ChannelDelete};
        let (hub, _dir) = crate::hub::test_hub().await;
        let channel = hub
            .channel_create(
                &crate::hub::Caller::Hub,
                ChannelCreate {
                    name: "Work".to_owned(),
                    purpose: String::new(),
                    instructions: String::new(),
                    icon: None,
                    color: None,
                    group_id: None,
                    coordinator_selection: None,
                    task_selection: None,
                    repo_ids: Vec::new(),
                    default_isolation: None,
                    default_approval_mode: None,
                    memory_enabled: None,
                    tags: Vec::new(),
                },
            )
            .await
            .unwrap()
            .channel;
        let (_, task_session) = crate::hub::test_task_session(&hub).await;
        let mut session = task_session.clone();
        session.id = SessionId::new();
        session.role = SessionRole::Coordinator;
        session.channel_id = channel.id;
        session.task_id = None;
        session.state = SessionState::Starting;
        let stored = session.clone();
        hub.store
            .write(move |tx| {
                tx.save_session(&stored)?;
                tx.save_coordinator(&crate::store::coordinator::CoordinatorRow {
                    channel_id: stored.channel_id,
                    session_id: Some(stored.id),
                    turn_open: true,
                    turn_started_at: Some(tx.now()),
                    turn_events: Vec::new(),
                })?;
                tx.queue_command(stored.worker_id, &start(&stored))?;
                Ok(())
            })
            .await
            .unwrap();
        hub.channel_delete(
            &crate::hub::Caller::Hub,
            ChannelDelete {
                channel_id: channel.id,
            },
        )
        .await
        .unwrap();
        assert!(queued_kinds(&hub, &session).await.is_empty());
    }

    #[tokio::test]
    async fn ending_a_session_whose_start_went_out_drops_the_rest_and_sends_a_stop() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let (_, session) = crate::hub::test_task_session(&hub).await;
        let queued = session.clone();
        hub.store
            .write(move |tx| {
                // The start went out on an old link that closed before the
                // answer, so the worker may run the session.
                let id = tx.queue_command(queued.worker_id, &start(&queued))?;
                tx.mark_command_sent(&id)?;
                tx.queue_command(queued.worker_id, &turn(&queued))?;
                Ok(())
            })
            .await
            .unwrap();
        hub.mark_session_lost(session.id, "The test ends it.")
            .await
            .unwrap();
        assert_eq!(queued_kinds(&hub, &session).await, ["session.stop"]);
    }
}
