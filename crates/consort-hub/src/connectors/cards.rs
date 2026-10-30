//! Hub cards: approvals that the hub makes itself and answers itself.
//!
//! A hub card has the `request_id` `hub:<approval id>` and the options
//! `allow` and `deny`. The hub never sends `session.respond` for it. When
//! the user answers, the hub carries out the answer and tells the session
//! that asked with a `card_answered` event (a coordinator) or a message (a
//! task). A connector card that a `run_connector_tool` call still waits for
//! goes to that call instead.

use consort_proto::ids::{ApprovalId, ChannelId};
use consort_proto::model::{
    Approval, ApprovalDetails, ApprovalStatus, Author, Connection, MessageKind, Session,
    SessionRole, SignInMethod, Task, hub_card_request_id,
};
use consort_proto::status::{Actor, TaskStatus};
use consort_proto::tools::ToolAudience;
use consort_proto::turn_input::{WakeEvent, render_events};

use super::connections::ConnectSpec;
use crate::error::{HubError, Result};
use crate::hub::{Hub, ToolCtx};
use crate::limits::{MESSAGE_BODY_MAX_BYTES, cut_text};
use crate::ops::flow::{deliver_in_tx, transition};
use crate::ops::waiting::resume_if_nothing_waits;
use crate::store::conversation::NewMessage;
use crate::store::{Tx, conversation, work};

/// The most text of a `card_answered` event.
const CARD_TEXT_MAX_BYTES: usize = 64 * 1024;

/// Posts a hub card in a transaction: the card message and the approval. A
/// card in a task makes the task Needs input. Returns the approval and
/// whether the channel's coordinator must be kicked after the commit.
pub(crate) fn post_card_in_tx(
    tx: &mut Tx<'_>,
    id: ApprovalId,
    session: &Session,
    channel_id: ChannelId,
    task: Option<&Task>,
    summary: &str,
    details: ApprovalDetails,
) -> Result<(Approval, bool)> {
    let body = cut_text(summary, MESSAGE_BODY_MAX_BYTES);
    let message = match task {
        Some(task) => tx.post_in_task_thread(
            task,
            Some(session),
            Author::Assistant,
            MessageKind::Approval,
            &body,
        )?,
        None => {
            let mut new =
                NewMessage::new(channel_id, Author::Assistant, MessageKind::Approval, body);
            new.author_session_id = Some(session.id);
            tx.insert_message(new)?
        }
    };
    let options = details.hub_card_options().ok_or_else(|| {
        HubError::internal("a hub card was posted with a kind that is not a hub card")
    })?;
    let approval = Approval {
        id,
        channel_id,
        session_id: session.id,
        task_id: task.map(|t| t.id),
        message_id: message.id,
        request_id: hub_card_request_id(id),
        summary: summary.to_owned(),
        details,
        options,
        status: ApprovalStatus::Pending,
        answer_option_id: None,
        answered_at: None,
        answered_by_client_id: None,
        created_at: tx.now(),
    };
    tx.save_approval(&approval)?;
    let mut kick = false;
    if let Some(task) = task {
        let mut task = work::task_or_404(tx.db(), task.id)?;
        match transition(
            tx,
            &mut task,
            TaskStatus::NeedsInput,
            Actor::System,
            Some(summary.to_owned()),
            Some(format!("Waiting for the user: {summary}")),
        ) {
            Ok(changed) => {
                if changed {
                    tx.save_task(&task)?;
                    kick = TaskStatus::NeedsInput.wakes_coordinator();
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, task_id = %task.id, "a hub card did not move its task to Needs input");
            }
        }
    }
    Ok((approval, kick))
}

impl Hub {
    /// Posts a hub card for the session of a tool call.
    pub(crate) async fn post_hub_card(
        &self,
        ctx: &ToolCtx,
        id: ApprovalId,
        summary: String,
        details: ApprovalDetails,
    ) -> Result<Approval> {
        let session = ctx.session.clone();
        let channel_id = ctx.channel.id;
        let task = ctx.task.clone();
        let (approval, kick) = self
            .store
            .write(move |tx| {
                post_card_in_tx(
                    tx,
                    id,
                    &session,
                    channel_id,
                    task.as_ref(),
                    &summary,
                    details,
                )
            })
            .await?;
        if kick {
            self.kick_coordinator(approval.channel_id);
        }
        Ok(approval)
    }

    /// The tool context of the session that posted a card. The session may
    /// have ended since: the card still speaks for its channel.
    pub(crate) async fn ctx_for_approval(&self, approval: &Approval) -> Result<ToolCtx> {
        let (session_id, channel_id) = (approval.session_id, approval.channel_id);
        self.store
            .read(move |conn| {
                let session = work::session_or_404(conn, session_id)?;
                let channel = conversation::channel_or_404(conn, channel_id)?;
                let task = match (session.role, session.task_id) {
                    (SessionRole::Task, Some(id)) => work::task(conn, id)?,
                    _ => None,
                };
                let audience = ToolAudience::for_session(session.role, channel.kind);
                Ok(ToolCtx {
                    session,
                    channel,
                    task,
                    audience,
                })
            })
            .await
    }

    /// Tells the session that posted a card what came of it: a
    /// `card_answered` event for a coordinator, a message for a task.
    pub(crate) async fn send_card_answered(&self, approval: &Approval, text: String) -> Result<()> {
        let text = cut_text(&text, CARD_TEXT_MAX_BYTES);
        let event = WakeEvent::CardAnswered {
            id: approval.id,
            kind: approval.details.kind(),
            outcome: approval.status,
            text,
        };
        let (channel_id, task_id) = (approval.channel_id, approval.task_id);
        let hub = self.clone();
        let (worker, kick) = self
            .store
            .write(move |tx| match task_id {
                Some(task_id) => {
                    let text = render_events(std::slice::from_ref(&event));
                    match deliver_in_tx(&hub, tx, task_id, &text, &[], Actor::System) {
                        Ok(delivered) => {
                            let changed = delivered.status_changed;
                            Ok((Some(delivered), changed))
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "could not give a card answer to its task");
                            Ok((None, false))
                        }
                    }
                }
                None => {
                    tx.queue_wake(channel_id, &event)?;
                    Ok((None, true))
                }
            })
            .await?;
        if let Some(delivered) = worker {
            self.after_delivery(&delivered);
        }
        if kick {
            self.kick_coordinator(channel_id);
        }
        Ok(())
    }

    /// Runs after the user answered a hub card with `approval.answer`.
    pub(crate) fn hub_card_answered(&self, approval: Approval) {
        if let ApprovalDetails::Connector(_) = &approval.details
            && let Some(waiter) = self.connectors.take_waiter(approval.id)
        {
            // A run_connector_tool call waits for it and runs the call.
            let _ = waiter.send(approval.status);
            return;
        }
        let hub = self.clone();
        tokio::spawn(async move {
            let text = match hub.carry_out_card(&approval).await {
                Ok(text) => text,
                Err(err) => Some(format!(
                    "The hub could not carry out the answer: {}",
                    err.to_error_body().message
                )),
            };
            if let Some(text) = text
                && let Err(err) = hub.send_card_answered(&approval, text).await
            {
                tracing::warn!(error = %err, approval_id = %approval.id, "could not report a card answer");
            }
        });
    }

    /// Carries out an answered card. Returns the text for the session, or
    /// `None` when another step reports it.
    async fn carry_out_card(&self, approval: &Approval) -> Result<Option<String>> {
        let allowed = approval.status == ApprovalStatus::Allowed;
        match &approval.details {
            ApprovalDetails::Connector(_) => self.late_connector_answer(approval).await.map(Some),
            ApprovalDetails::Management(card) => {
                if !allowed {
                    return Ok(Some(format!(
                        "The user did not allow this change: {}",
                        card.change
                    )));
                }
                self.approved_management(approval, card).await.map(Some)
            }
            ApprovalDetails::ConnectService(card) => {
                if !allowed {
                    return Ok(Some(format!(
                        "The user chose not to connect {} now.",
                        card.name
                    )));
                }
                let attempt = self
                    .connect_service(ConnectSpec {
                        service: card.service.clone(),
                        name: card.name.clone(),
                        use_in: card.use_in.clone(),
                        approvals: card.approvals,
                        auth_scheme: None,
                        credentials: Default::default(),
                    })
                    .await?;
                self.close_connect_card(approval.id, &attempt.connection)
                    .await?;
                Ok(None)
            }
            ApprovalDetails::SignIn(card) => {
                if !allowed {
                    return Ok(Some(format!(
                        "The user chose not to sign in to {} now.",
                        card.account_label
                    )));
                }
                let Some(method) = card.method.clone() else {
                    return Ok(Some(format!(
                        "The user continues the sign-in to {} in the app.",
                        card.account_label
                    )));
                };
                self.start_sign_in_for_card(approval, card.worker_id, card.account_id, method)
                    .await
                    .map(Some)
            }
            ApprovalDetails::SkillInstall(card) => {
                if !allowed {
                    return Ok(Some(format!(
                        "The user chose not to add the skill {} now.",
                        card.name
                    )));
                }
                self.install_from_card(approval, card).await.map(Some)
            }
            _ => Ok(None),
        }
    }

    async fn start_sign_in_for_card(
        &self,
        approval: &Approval,
        worker_id: consort_proto::ids::WorkerId,
        account_id: consort_proto::ids::AccountId,
        method: SignInMethod,
    ) -> Result<String> {
        let result = self
            .sign_in_start(
                &crate::hub::Caller::Hub,
                consort_proto::client_api::SignInStart {
                    worker_id,
                    account_id,
                    method,
                },
            )
            .await?;
        let sign_in_id = result.sign_in.id;
        let approval_id = approval.id;
        self.store
            .write(move |tx| {
                let Some(mut approval) = work::approval(tx.db(), approval_id)? else {
                    return Ok(());
                };
                if let ApprovalDetails::SignIn(card) = &mut approval.details {
                    card.sign_in_id = Some(sign_in_id);
                    tx.save_approval(&approval)?;
                }
                Ok(())
            })
            .await?;
        Ok("The sign-in started. The user finishes it in the app.".to_owned())
    }

    /// Closes a connect card for a connection that the hub made: marks it
    /// allowed when it still waits, puts the connection on it, and tells
    /// the session.
    pub(crate) async fn close_connect_card(
        &self,
        approval_id: ApprovalId,
        connection: &Connection,
    ) -> Result<()> {
        let connection_id = connection.id;
        let approval = self
            .store
            .write(move |tx| {
                let Some(mut approval) = work::approval(tx.db(), approval_id)? else {
                    return Ok(None);
                };
                if approval.status == ApprovalStatus::Pending {
                    approval.status = ApprovalStatus::Allowed;
                    approval.answer_option_id =
                        Some(consort_proto::model::HUB_CARD_ALLOW.to_owned());
                    approval.answered_at = Some(tx.now());
                }
                if let ApprovalDetails::ConnectService(card) = &mut approval.details {
                    card.connection_id = Some(connection_id);
                }
                tx.save_approval(&approval)?;
                resume_if_nothing_waits(tx, approval.task_id)?;
                Ok(Some(approval))
            })
            .await?;
        if let Some(approval) = approval {
            let text = format!(
                "The user started to connect {} ({}). The handle is {}. It becomes available when the user finishes the sign-in.",
                connection.name, connection.service_name, connection.handle
            );
            self.send_card_answered(&approval, text).await?;
        }
        Ok(())
    }
}
