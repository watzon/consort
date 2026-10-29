//! Approvals and decisions: the client API handlers.

use consort_proto::client_api::*;
use consort_proto::model::{ApprovalStatus, DecisionStatus};
use consort_proto::status::{Actor, TaskStatus};
use consort_proto::turn_input::{WakeEvent, render_events};
use consort_proto::worker_link::{HubToWorker, RequestAnswer, SessionRespond};

use super::flow::{deliver_in_tx, transition};
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
                    return Ok((approval, None));
                }
                let session = work::session_or_404(tx.db(), approval.session_id)?;
                tx.queue_command(
                    session.worker_id,
                    &HubToWorker::SessionRespond(SessionRespond {
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
                Ok((approval, Some(session.worker_id)))
            })
            .await?;
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
                        &HubToWorker::SessionRespond(SessionRespond {
                            session_id: session.id,
                            request_id,
                            answer: RequestAnswer { option_id, text },
                        }),
                    )?;
                    worker_id = Some(session.worker_id);
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
                                deliver_in_tx(&hub, tx, task_id, &render_events(&[event]), Actor::System)
                            };
                            match delivered {
                                Ok(delivered) => worker_id = Some(delivered.worker_id),
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
        if let Some(worker_id) = worker_id {
            self.pump_worker(worker_id);
        }
        if let Some(channel) = kick {
            self.kick_coordinator(channel);
        }
        Ok(DecisionResult { decision })
    }
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
