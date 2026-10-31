//! Task flow helpers that run inside a transaction: notices, thread posts,
//! status transitions and message delivery to a task session.

use consort_proto::ids::{ChannelId, MessageId, TaskId, WorkerId};
use consort_proto::model::{
    Author, BlobRef, CoordinatorBlocked, Message, MessageKind, Session, SessionRole, Task,
};
use consort_proto::session_host_link::{HubToSessionHost, SessionSteer, SessionTurn, TurnInput};
use consort_proto::status::{Actor, TaskStatus};
use consort_proto::tools::MessageDelivery;
use consort_proto::turn_input::WakeEvent;

use crate::error::{HubError, Result};
use crate::hub::Hub;
use crate::limits::{NOTICE_TEXT_MAX_BYTES, cut_text};
use crate::ops::model_switch::{ModelPlan, detach_session, is_switching, plan_model};
use crate::store::conversation::NewMessage;
use crate::store::{Tx, machine, work};

/// The body of a notice message: a JSON object with a `kind`.
pub(crate) fn notice_body(kind: &str, text: &str) -> String {
    serde_json::json!({ "kind": kind, "text": text }).to_string()
}

/// What a failed turn tells the user, for the `turn_failed` notice. All
/// fields except the text are optional: an old worker sends no kind, and
/// the notice then reads as `unknown`.
#[derive(Debug, Clone, Default)]
pub(crate) struct TurnFailure {
    pub kind: Option<consort_proto::model::TurnErrorKind>,
    pub resets_at: Option<consort_proto::time::Timestamp>,
    /// The name of the account that the turn ran on.
    pub account: Option<String>,
    /// The error text of the worker, unchanged.
    pub raw: Option<String>,
    /// What the fallback path tried, when no entry could take the work. The
    /// error card lists one line for each entry.
    pub path: Vec<consort_proto::model::PathLine>,
}

/// The body of a `turn_failed` notice. `text` is the line that older
/// clients show. The other fields are left out when empty.
pub(crate) fn turn_failed_body(text: &str, failure: &TurnFailure) -> String {
    let mut body = serde_json::json!({ "kind": "turn_failed", "text": text });
    if let Some(object) = body.as_object_mut() {
        if let Some(kind) = failure.kind {
            object.insert("error_kind".to_owned(), kind.as_str().into());
        }
        if let Some(at) = failure.resets_at
            && let Ok(value) = serde_json::to_value(at)
        {
            object.insert("resets_at".to_owned(), value);
        }
        if let Some(account) = &failure.account {
            object.insert("account".to_owned(), account.clone().into());
        }
        if let Some(raw) = &failure.raw {
            object.insert("raw".to_owned(), raw.clone().into());
        }
        if !failure.path.is_empty()
            && let Ok(value) = serde_json::to_value(&failure.path)
        {
            object.insert("path".to_owned(), value);
        }
    }
    body.to_string()
}

impl Tx<'_> {
    /// Posts a `turn_failed` notice with the kind of the failure.
    pub(crate) fn post_turn_failed(
        &mut self,
        channel_id: ChannelId,
        thread_root: Option<MessageId>,
        task_id: Option<TaskId>,
        text: &str,
        failure: &TurnFailure,
    ) -> Result<Message> {
        let text = cut_text(text, NOTICE_TEXT_MAX_BYTES);
        let mut failure = failure.clone();
        failure.raw = failure.raw.map(|raw| cut_text(&raw, NOTICE_TEXT_MAX_BYTES));
        let mut new = NewMessage::new(
            channel_id,
            Author::System,
            MessageKind::Notice,
            turn_failed_body(&text, &failure),
        );
        new.thread_root_id = thread_root;
        new.author_task_id = task_id;
        self.insert_message(new)
    }

    /// Posts a system notice, in a thread when `thread_root` is set.
    pub fn post_notice(
        &mut self,
        channel_id: ChannelId,
        thread_root: Option<MessageId>,
        task_id: Option<TaskId>,
        kind: &str,
        text: &str,
    ) -> Result<Message> {
        // Notice text often comes from a worker error, so it is cut, not refused.
        let text = cut_text(text, NOTICE_TEXT_MAX_BYTES);
        let mut new = NewMessage::new(
            channel_id,
            Author::System,
            MessageKind::Notice,
            notice_body(kind, &text),
        );
        new.thread_root_id = thread_root;
        new.author_task_id = task_id;
        self.insert_message(new)
    }

    /// Sets or clears the live state "the assistant cannot answer" of a
    /// channel (see [`CoordinatorBlocked`]). It is not a message: it never
    /// stays in the timeline. A write that changes nothing saves nothing, so
    /// a cause that lasts sends one feed entry, however many times the hub
    /// tries again.
    pub(crate) fn set_coordinator_blocked(
        &mut self,
        channel_id: ChannelId,
        blocked: Option<CoordinatorBlocked>,
    ) -> Result<()> {
        let mut channel = crate::store::conversation::channel_or_404(self.db(), channel_id)?;
        let blocked = blocked.map(|mut b| {
            b.reason = cut_text(&b.reason, NOTICE_TEXT_MAX_BYTES);
            Box::new(b)
        });
        if channel.coordinator_blocked == blocked {
            return Ok(());
        }
        channel.coordinator_blocked = blocked;
        self.save_channel(&channel)
    }

    /// Posts in the thread of a task card.
    pub fn post_in_task_thread(
        &mut self,
        task: &Task,
        session: Option<&Session>,
        author: Author,
        kind: MessageKind,
        body: &str,
    ) -> Result<Message> {
        let mut new = NewMessage::new(task.channel_id, author, kind, body);
        new.thread_root_id = Some(task.card_message_id);
        new.author_task_id = Some(task.id);
        new.author_session_id = session.map(|s| s.id);
        self.insert_message(new)
    }
}

/// Moves a task to a new status if the state machine allows it for `actor`.
/// Returns whether the status changed. Queues a wake event for the channel
/// coordinator when the new status wakes it; the caller must then kick the
/// coordinator after the commit.
pub(crate) fn transition(
    tx: &mut Tx<'_>,
    task: &mut Task,
    to: TaskStatus,
    actor: Actor,
    reason: Option<String>,
    wake_text: Option<String>,
) -> Result<bool> {
    change_status(tx, task, to, actor, reason, wake_text, true)
}

/// Moves a task to Needs input for an approval. The coordinator does not wake:
/// the user sees the card and gets the notification, and the coordinator can
/// neither answer an approval nor add to it.
pub(crate) fn wait_for_approval(
    tx: &mut Tx<'_>,
    task: &mut Task,
    actor: Actor,
    reason: Option<String>,
) -> Result<bool> {
    change_status(tx, task, TaskStatus::NeedsInput, actor, reason, None, false)
}

fn change_status(
    tx: &mut Tx<'_>,
    task: &mut Task,
    to: TaskStatus,
    actor: Actor,
    reason: Option<String>,
    wake_text: Option<String>,
    wake: bool,
) -> Result<bool> {
    // A reason often holds text from a worker, so it is cut, not refused.
    let reason = reason.map(|r| cut_text(&r, NOTICE_TEXT_MAX_BYTES));
    if task.status == to {
        if reason.is_some() && task.status_reason != reason {
            task.status_reason = reason;
            task.updated_at = tx.now();
        }
        return Ok(false);
    }
    task.status.check_transition(to, actor)?;
    let now = tx.now();
    task.status = to;
    task.status_reason = reason;
    task.updated_at = now;
    task.resolved_at = (to == TaskStatus::Resolved).then_some(now);
    // For stale-task nudges: when the task entered this status.
    tx.note_task_status(task)?;
    if wake && to.wakes_coordinator() {
        let text = wake_text
            .or_else(|| task.status_reason.clone())
            .or_else(|| task.review_summary.clone())
            .unwrap_or_else(|| to.label().to_owned());
        // An update of a task that a proactive turn started is proactive
        // too, so the turn that it wakes keeps the proactive limits.
        let proactive =
            crate::store::proactivity::proactive_task(tx.db(), task.id)?.map(|origin| origin.kind);
        tx.queue_wake(
            task.channel_id,
            &WakeEvent::TaskUpdate {
                task: task.id,
                title: task.title.clone(),
                status: to,
                text,
                proactive,
            },
        )?;
    }
    // A task that a routine started ends the routine's run when it is
    // ready for review, resolved or failed.
    tx.end_task_run(task)?;
    Ok(true)
}

/// What a delivery did, for the caller to finish after the commit.
#[derive(Debug, Clone)]
pub(crate) struct Delivered {
    pub delivery: MessageDelivery,
    pub worker_id: WorkerId,
    pub status_changed: bool,
    /// The task moves to a new session on another model. The text waits in
    /// its inbox. The caller calls [`Hub::after_delivery`] after the commit.
    pub switch_task: Option<TaskId>,
    /// Draft cards that the text replaced. The text went to the tool calls
    /// that wait for them, not to the session. The caller calls
    /// [`Hub::after_delivery`] after the commit.
    pub replaced: Vec<consort_proto::ids::ApprovalId>,
}

/// Queues text for the session of a task: a new turn when no turn is open,
/// a steering message for an open turn, or a place in the inbox while a turn
/// is open and the hub cannot steer it (the worker is offline or the harness
/// cannot steer). Moves a task under review back to Working. The caller pumps
/// the worker and kicks the coordinator after the commit.
pub(crate) fn deliver_in_tx(
    hub: &Hub,
    tx: &mut Tx<'_>,
    task_id: TaskId,
    text: &str,
    attachments: &[BlobRef],
    actor: Actor,
) -> Result<Delivered> {
    deliver(hub, tx, task_id, text, attachments, actor, true)
}

/// Like [`deliver_in_tx`], but never steers: while a turn is open, the text
/// waits in the inbox. The hub uses it when the worker refused a steer.
pub(crate) fn deliver_without_steer(
    hub: &Hub,
    tx: &mut Tx<'_>,
    task_id: TaskId,
    text: &str,
    attachments: &[BlobRef],
    actor: Actor,
) -> Result<Delivered> {
    deliver(hub, tx, task_id, text, attachments, actor, false)
}

fn deliver(
    hub: &Hub,
    tx: &mut Tx<'_>,
    task_id: TaskId,
    text: &str,
    attachments: &[BlobRef],
    actor: Actor,
    may_steer: bool,
) -> Result<Delivered> {
    let mut task = work::task_or_404(tx.db(), task_id)?;
    match task.status {
        TaskStatus::Scheduled | TaskStatus::UpNext => {
            return Err(HubError::conflict(
                "The task has not started yet. Wait until it runs, then send the message.",
            ));
        }
        TaskStatus::Failed => {
            return Err(HubError::conflict(
                "The task failed. Retry it first, then send the message.",
            ));
        }
        TaskStatus::Resolved => {
            return Err(HubError::conflict(
                "The task is resolved. Mark it as not done first.",
            ));
        }
        TaskStatus::Working | TaskStatus::NeedsInput | TaskStatus::Review => {}
    }
    // The task waits for its new session on another model: the text waits
    // in the inbox, and the new session takes it.
    if is_switching(&task)
        && let Some(worker_id) = task.worker_id
    {
        tx.push_task_inbox(task_id, text, attachments)?;
        return Ok(Delivered {
            delivery: MessageDelivery::Queued,
            worker_id,
            status_changed: false,
            switch_task: Some(task_id),
            replaced: Vec::new(),
        });
    }
    let session_id = task
        .current_session_id
        .ok_or_else(|| HubError::conflict("The task has no session. Retry it first."))?;
    let session = work::session_or_404(tx.db(), session_id)?;
    if session.role != SessionRole::Task || session.state.is_final() {
        return Err(HubError::conflict(
            "The session of this task has ended, so it cannot take a message. Retry the task to start a new session, then send the message.",
        ));
    }
    let mut runtime = work::session_runtime(tx.db(), session_id)?;
    let status_changed = if task.status == TaskStatus::Review {
        transition(tx, &mut task, TaskStatus::Working, actor, None, None)?
    } else {
        false
    };

    // The task chose another model after this session started. The change
    // goes with the next turn: in the same session when the harness can
    // switch, else in a new session (the old one ends, the text waits).
    let mut model_change = None;
    if !runtime.turn_open {
        match plan_model(tx.db(), &task, &session)? {
            Ok(ModelPlan::Keep) => {}
            Ok(ModelPlan::InSession(change)) => model_change = Some(change),
            Ok(ModelPlan::NewSession) => {
                task.updated_at = tx.now();
                tx.save_task(&task)?;
                // `detach_session` reads the task again, so the status
                // change above is saved first.
                detach_session(hub, tx, task_id, Some((text, attachments)))?;
                return Ok(Delivered {
                    delivery: MessageDelivery::Queued,
                    worker_id: session.worker_id,
                    status_changed,
                    switch_task: Some(task_id),
                    replaced: Vec::new(),
                });
            }
            Err(reason) => {
                tx.post_notice(
                    task.channel_id,
                    Some(task.card_message_id),
                    Some(task.id),
                    "model_not_changed",
                    &format!("The task keeps its model for now. {reason}"),
                )?;
            }
        }
    }

    let steer = machine::worker(tx.db(), session.worker_id)?
        .and_then(|w| w.inventory)
        .and_then(|inv| {
            inv.account(session.account_id)
                .map(|a| a.capabilities.steer)
        })
        .unwrap_or(false);
    let turn = TurnInput {
        text: text.to_owned(),
        attachments: attachments.to_vec(),
    };
    // A session has one turn at a time. While a turn is open, the text
    // steers it when the worker is online and the harness can steer, and
    // otherwise waits in the inbox for the turn to end. That holds for an
    // offline worker too: a queued turn would reach it while the old turn
    // still runs.
    let online = hub.links.is_online(session.worker_id);
    let delivery = if runtime.turn_open && online && steer && may_steer {
        tx.queue_command(
            session.worker_id,
            &HubToSessionHost::SessionSteer(SessionSteer { session_id, turn }),
        )?;
        MessageDelivery::Steer
    } else if runtime.turn_open {
        tx.push_task_inbox(task_id, text, attachments)?;
        MessageDelivery::Queued
    } else {
        let channel_files =
            tx.channel_files_for_turn(session_id, task.channel_id, Some(task_id))?;
        tx.queue_command(
            session.worker_id,
            &HubToSessionHost::SessionTurn(SessionTurn {
                session_id,
                turn,
                channel_files,
                approval_mode: None,
                model_change,
            }),
        )?;
        runtime.turn_open = true;
        // A refused turn sends the task back where it came from.
        runtime.turn_from_review = status_changed;
        runtime.active_at = Some(tx.now());
        if online {
            MessageDelivery::Turn
        } else {
            MessageDelivery::Queued
        }
    };
    tx.save_session_runtime(session_id, &runtime)?;
    task.updated_at = tx.now();
    tx.save_task(&task)?;
    Ok(Delivered {
        delivery,
        worker_id: session.worker_id,
        status_changed,
        switch_task: None,
        replaced: Vec::new(),
    })
}

impl Hub {
    /// Sends text to the session of a task. See [`deliver_in_tx`].
    pub(crate) async fn deliver_to_task(
        &self,
        task_id: TaskId,
        text: String,
        attachments: Vec<BlobRef>,
        actor: Actor,
    ) -> Result<MessageDelivery> {
        let hub = self.clone();
        let (delivered, channel) = self
            .store
            .write(move |tx| {
                let delivered = deliver_in_tx(&hub, tx, task_id, &text, &attachments, actor)?;
                let task = work::task_or_404(tx.db(), task_id)?;
                Ok((delivered, task.channel_id))
            })
            .await?;
        self.after_delivery(&delivered);
        if delivered.status_changed {
            self.kick_coordinator(channel);
        }
        Ok(delivered.delivery)
    }

    /// Finishes a delivery after its commit: pumps the worker and starts
    /// the new session of a task that changed its model.
    pub(crate) fn after_delivery(&self, delivered: &Delivered) {
        self.pump_worker(delivered.worker_id);
        if let Some(task_id) = delivered.switch_task {
            self.spawn_switch(task_id);
        }
        self.signal_replaced(&delivered.replaced);
    }
}
