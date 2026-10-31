//! Approvals and decisions: the client API handlers.

use consort_proto::client_api::*;
use consort_proto::ids::TaskId;
use consort_proto::model::{ApprovalStatus, BlobRef, DecisionAnswer, DecisionStatus};
use consort_proto::session_host_link::{HubToSessionHost, RequestAnswer, SessionRespond};
use consort_proto::status::{Actor, TaskStatus};
use consort_proto::tools::MessageDelivery;
use consort_proto::turn_input::{WakeEvent, render_events};

use super::flow::{Delivered, deliver_in_tx, transition};
use crate::connectors::replace::{DraftScope, replace_waiting_drafts};
use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::store::work;

impl Hub {
    pub(crate) async fn approval_answer(
        &self,
        caller: &Caller,
        request: ApprovalAnswer,
    ) -> Result<ApprovalResult> {
        let client_id = caller.client_id();
        let hub = self.clone();
        let mut voice_edits = None;
        let voice_edits_slot = &mut voice_edits;
        let (approval, worker_id) = self
            .store
            .write(move |tx| {
                let mut approval = work::approval(tx.db(), request.approval_id)?
                    .ok_or_else(|| HubError::not_found("The approval does not exist."))?;
                if approval.status != ApprovalStatus::Pending {
                    return Err(HubError::conflict(
                        "This approval is already answered or closed, so it takes no more answers.",
                    ));
                }
                let option = approval
                    .option(&request.option_id)
                    .cloned()
                    .ok_or_else(|| {
                        HubError::invalid(
                            "The approval has no option with this ID. Choose one of its options.",
                        )
                    })?;
                crate::voice::card::check_allow(
                    &approval,
                    !option.kind.is_deny(),
                    request.checked,
                    request.checked_hash.as_deref(),
                )?;
                approval.status = if option.kind.is_deny() {
                    ApprovalStatus::Denied
                } else {
                    ApprovalStatus::Allowed
                };
                approval.answer_option_id = Some(option.id.clone());
                approval.answered_at = Some(tx.now());
                approval.answered_by_client_id = client_id;
                tx.save_approval(&approval)?;
                if approval.is_hub_card() {
                    // The hub made this card and carries out the answer
                    // itself, after the commit. No session.respond.
                    resume_if_nothing_waits(tx, approval.task_id)?;
                    // A draft that goes out becomes a draft pair.
                    let tutti = crate::voice::card::record_pair_in_tx(tx, &approval)?;
                    return Ok((approval, None, tutti));
                }
                let session = work::session_or_404(tx.db(), approval.session_id)?;
                tx.queue_command(
                    session.worker_id,
                    &HubToSessionHost::SessionRespond(SessionRespond {
                        session_id: session.id,
                        request_id: approval.request_id.clone(),
                        answer: RequestAnswer {
                            option_id: Some(option.id),
                            text: None,
                        },
                    }),
                )?;
                resume_if_nothing_waits(tx, approval.task_id)?;
                let _ = &hub;
                Ok((approval, Some(session.worker_id), None))
            })
            .await
            .map(|(approval, worker_id, tutti)| {
                *voice_edits_slot = tutti;
                (approval, worker_id)
            })?;
        if let Some(tutti) = voice_edits {
            self.kick_coordinator(tutti);
        }
        match worker_id {
            Some(worker_id) => self.pump_worker(worker_id),
            None => self.hub_card_answered(approval.clone()),
        }
        Ok(ApprovalResult { approval })
    }

    pub(crate) async fn decision_answer(
        &self,
        _caller: &Caller,
        request: DecisionAnswerRequest,
    ) -> Result<DecisionResult> {
        let hub = self.clone();
        let (decision, worker_id, kick) = self
            .store
            .write(move |tx| {
                let mut decision = work::decision(tx.db(), request.decision_id)?
                    .ok_or_else(|| HubError::not_found("The decision does not exist."))?;
                if decision.status != DecisionStatus::Pending {
                    return Err(HubError::conflict(
                        "This decision is already answered or closed, so it takes no more answers.",
                    ));
                }
                decision.check_answer(&request.answer)?;
                decision.status = DecisionStatus::Answered;
                decision.answer = Some(request.answer.clone());
                decision.answered_at = Some(tx.now());
                tx.save_decision(&decision)?;
                // The offer "Save this style guide?": the hub carries out
                // the answer itself.
                if crate::voice::learn::proposal_answered(tx, &decision)? {
                    return Ok((decision, None, None));
                }

                let text = request
                    .answer
                    .text
                    .clone()
                    .filter(|t| !t.trim().is_empty());
                let session = work::session_or_404(tx.db(), decision.session_id)?;
                let mut worker_id = None;
                let mut kick = None;
                if let Some(request_id) = decision.request_id.clone() {
                    // A harness question: the answer goes back with session.respond.
                    let option_id = request
                        .answer
                        .option_index
                        .and_then(|i| decision.options.get(usize::from(i)))
                        .and_then(|o| o.id.clone());
                    tx.queue_command(
                        session.worker_id,
                        &HubToSessionHost::SessionRespond(SessionRespond {
                            session_id: session.id,
                            request_id,
                            answer: RequestAnswer { option_id, text },
                        }),
                    )?;
                    worker_id = Some(Delivered {
                        delivery: MessageDelivery::Steer,
                        worker_id: session.worker_id,
                        status_changed: false,
                        switch_task: None,
                        replaced: Vec::new(),
                    });
                    resume_if_nothing_waits(tx, decision.task_id)?;
                } else {
                    // A decision from the ask_decision tool: the answer is input
                    // to the next turn.
                    let event = WakeEvent::DecisionAnswered {
                        id: decision.id,
                        option: decision.answer_label().map(str::to_owned),
                        text,
                    };
                    match decision.task_id {
                        Some(task_id) => {
                            let delivered = {
                                let mut task = work::task_or_404(tx.db(), task_id)?;
                                // The last waiting item is closed now.
                                if task.status == TaskStatus::NeedsInput
                                    && work::pending_count_for_task(tx.db(), task_id)? == 0
                                {
                                    transition(tx, &mut task, TaskStatus::Working, Actor::System, None, None)?;
                                    tx.save_task(&task)?;
                                }
                                deliver_in_tx(&hub, tx, task_id, &render_events(&[event]), &[], Actor::System)
                            };
                            match delivered {
                                Ok(delivered) => worker_id = Some(delivered),
                                Err(err) => tracing::warn!(error = %err, "could not give a decision answer to its task"),
                            }
                        }
                        None => {
                            tx.queue_wake(decision.channel_id, &event)?;
                            kick = Some(decision.channel_id);
                        }
                    }
                }
                Ok((decision, worker_id, kick))
            })
            .await?;
        if let Some(delivered) = worker_id {
            self.after_delivery(&delivered);
        }
        if let Some(channel) = kick {
            self.kick_coordinator(channel);
        }
        Ok(DecisionResult { decision })
    }
}

/// Gives the user's own text to the session of a task, and lets the text
/// answer the one open decision of the task, when there is exactly one. The
/// user's message in the task thread and the user's message that the
/// coordinator passes on with `message_task` both come here.
///
/// - A decision from the `ask_decision` tool (no harness request): the text
///   goes to the task as a message, and the decision closes as answered with
///   the text. The task leaves Needs input when nothing else waits.
/// - A decision from a harness question that takes free text: the text goes
///   back with `session.respond`, as an answer in the app does. The harness
///   waits for that answer, so a steer would not free it. A message with
///   attachments, and a question that takes no free text, go to the task as
///   a message and leave the question open.
///
/// An approval is never answered here: text is not an answer to it. With two
/// open decisions the text could answer either, so both stay open.
pub(crate) fn deliver_user_text(
    hub: &Hub,
    tx: &mut crate::store::Tx<'_>,
    task_id: TaskId,
    text: &str,
    attachments: &[BlobRef],
) -> Result<Delivered> {
    // A draft card of the task waits: the text may change the draft. It goes
    // to the tool call that waits, as the result, not to the session.
    let replaced = replace_waiting_drafts(hub, tx, DraftScope::Task(task_id), text, attachments)?;
    if !replaced.is_empty() {
        let task = work::task_or_404(tx.db(), task_id)?;
        let session_id = task
            .current_session_id
            .ok_or_else(|| HubError::conflict("The task has no session. Retry it first."))?;
        let session = work::session_or_404(tx.db(), session_id)?;
        return Ok(Delivered {
            delivery: MessageDelivery::Steer,
            worker_id: session.worker_id,
            status_changed: false,
            switch_task: None,
            replaced,
        });
    }
    let mut open = work::pending_decisions_of_task(tx.db(), task_id)?;
    let lone = (open.len() == 1).then(|| open.remove(0));
    if let Some(mut decision) = lone
        .clone()
        .filter(|d| d.request_id.is_some() && d.allow_free_text && attachments.is_empty())
    {
        let task = work::task_or_404(tx.db(), task_id)?;
        let session = work::session_or_404(tx.db(), decision.session_id)?;
        let request_id = decision
            .request_id
            .clone()
            .ok_or_else(|| HubError::internal("the decision has no harness request"))?;
        if session.state.is_final() {
            return Err(HubError::conflict(
                "The session of this task has ended, so it cannot take a message. Retry the task to start a new session, then send the message.",
            ));
        }
        tx.queue_command(
            session.worker_id,
            &HubToSessionHost::SessionRespond(SessionRespond {
                session_id: session.id,
                request_id,
                answer: RequestAnswer {
                    option_id: None,
                    text: Some(text.to_owned()),
                },
            }),
        )?;
        close_with_text(tx, &mut decision, text)?;
        resume_if_nothing_waits(tx, Some(task.id))?;
        return Ok(Delivered {
            delivery: MessageDelivery::Steer,
            worker_id: session.worker_id,
            status_changed: false,
            switch_task: None,
            replaced: Vec::new(),
        });
    }
    let delivered = deliver_in_tx(hub, tx, task_id, text, attachments, Actor::User)?;
    if let Some(mut decision) = lone.filter(|d| d.request_id.is_none()) {
        close_with_text(tx, &mut decision, text)?;
        resume_if_nothing_waits(tx, Some(task_id))?;
    }
    Ok(delivered)
}

/// Closes a decision as answered with the user's text.
fn close_with_text(
    tx: &mut crate::store::Tx<'_>,
    decision: &mut consort_proto::model::Decision,
    text: &str,
) -> Result<()> {
    decision.status = DecisionStatus::Answered;
    decision.answer = Some(DecisionAnswer {
        option_index: None,
        text: Some(text.to_owned()),
    });
    decision.answered_at = Some(tx.now());
    tx.save_decision(decision)
}

/// When the last waiting approval or decision of a task closes, the task
/// goes from Needs input back to Working. Working does not wake a
/// coordinator, so the caller has nothing more to do.
///
/// The time with the user does not count for the quiet-turn check, so the
/// session's `active_at` starts again now. Without this, an answer after
/// more than 2 hours would let the check fail the task at once.
pub(crate) fn resume_if_nothing_waits(
    tx: &mut crate::store::Tx<'_>,
    task_id: Option<consort_proto::ids::TaskId>,
) -> Result<()> {
    let Some(task_id) = task_id else {
        return Ok(());
    };
    let mut task = work::task_or_404(tx.db(), task_id)?;
    if task.status == TaskStatus::NeedsInput && work::pending_count_for_task(tx.db(), task_id)? == 0
    {
        transition(
            tx,
            &mut task,
            TaskStatus::Working,
            Actor::System,
            None,
            None,
        )?;
        tx.save_task(&task)?;
        if let Some(session_id) = task.current_session_id {
            let mut runtime = work::session_runtime(tx.db(), session_id)?;
            runtime.active_at = Some(tx.now());
            tx.save_session_runtime(session_id, &runtime)?;
        }
    }
    Ok(())
}
