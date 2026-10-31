//! `request_access`: a session asks for a connection or for Monid that its
//! channel does not have (ADR 0017). The answer is temporary access, lasting
//! access or no. Access does not change approvals: each call still follows
//! the rules of the connection or of Monid.

use consort_proto::client_api::{ChannelEndAccess, ChannelResult};
use consort_proto::ids::{AccessGrantId, ApprovalId, ChannelId, TaskId};
use consort_proto::model::{
    ACCESS_CARD_ALWAYS, ACCESS_CARD_TEMPORARY, AccessGrant, AccessRequestCard, AccessTarget,
    Approval, ApprovalDetails, ApprovalStatus, Channel, MonidAllowedTool, MonidToolRef,
    TEMPORARY_ACCESS_MS, UseIn,
};
use consort_proto::status::TaskStatus;
use consort_proto::tools::{RequestAccessArgs, RequestAccessResult};
use serde_json::Value;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub, ToolCtx};
use crate::store::{connectors as con_store, conversation, monid as monid_store, work};
use crate::tools::management::{describe_use_in, widen_to};

/// How long a Deny stops the same request in a task or a coordinator
/// session.
const DENIAL_HOLD_MS: i64 = 24 * 60 * 60 * 1_000;
/// The longest reason on a card.
const REASON_MAX_CHARS: usize = 300;

/// The key of a Deny: the task, or the coordinator session.
fn denial_scope(approval_task: Option<TaskId>, session: &str) -> String {
    match approval_task {
        Some(task) => task.to_string(),
        None => session.to_owned(),
    }
}

/// What a lasting answer changes for a connection or for Monid: the channel
/// has an exception to remove, or the "Use in" must widen.
fn use_in_after(use_in: &UseIn, channel: &Channel, channels: &[Channel]) -> Option<UseIn> {
    (!use_in.matches_channel(channel)).then(|| widen_to(use_in, channel, channels))
}

/// Whether a target is the one that a lasting answer or a grant covers.
fn same_tool(tool: &MonidToolRef, provider: &str, endpoint: Option<&str>) -> bool {
    tool.provider == provider && tool.endpoint.as_deref() == endpoint
}

/// Removes the grants that have ended: the time passed, or the task ended.
fn live_grants(
    channel: &Channel,
    now: consort_proto::time::Timestamp,
    task_ended: impl Fn(TaskId) -> bool,
) -> Vec<AccessGrant> {
    channel
        .access_grants
        .iter()
        .filter(|grant| grant.expires_at.is_none_or(|end| end > now))
        .filter(|grant| grant.task_id.is_none_or(|task| !task_ended(task)))
        .cloned()
        .collect()
}

impl Hub {
    pub(crate) async fn tool_request_access(
        &self,
        ctx: &ToolCtx,
        args: RequestAccessArgs,
    ) -> Result<Value> {
        let result = self.request_access(ctx, args).await?;
        Ok(serde_json::to_value(result)?)
    }

    async fn request_access(
        &self,
        ctx: &ToolCtx,
        args: RequestAccessArgs,
    ) -> Result<RequestAccessResult> {
        let refused = |message: String| Ok(RequestAccessResult::Refused { message });
        let task_id = ctx.task.as_ref().map(|t| t.id);
        let channel_id = ctx.channel.id;
        let channel = self
            .store
            .read(move |conn| conversation::channel_or_404(conn, channel_id))
            .await?;
        let (target, permanent_effect) = match (args.connection.as_deref(), args.monid.as_ref()) {
            (Some(key), None) => {
                let connection = self.find_connection(key).await?;
                if connection.is_available_for(&channel, task_id, self.now()) {
                    return refused(format!(
                        "You have {} already. Call list_connector_tools with connector {}.",
                        connection.name, connection.handle
                    ));
                }
                if !connection.status.is_active() {
                    return refused(format!(
                        "{} needs a new sign-in first. Ask the user to reconnect it in Settings > Connectors.",
                        connection.name
                    ));
                }
                let channels = self.all_channels().await?;
                let effect = match use_in_after(&connection.use_in, &channel, &channels) {
                    Some(wider) => format!(
                        "Always changes its Use in to {}.",
                        describe_use_in(&wider, &channels)
                    ),
                    None => "Always turns it on in this channel.".to_owned(),
                };
                let name = connection.name.clone();
                (
                    AccessTarget::Connection {
                        connection_id: connection.id,
                        name: name.clone(),
                    },
                    format!("Calls still follow the approval rules of {name}. {effect}"),
                )
            }
            (None, Some(monid)) => {
                let provider = monid
                    .provider
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty());
                let endpoint = monid
                    .endpoint
                    .as_deref()
                    .map(str::trim)
                    .filter(|e| !e.is_empty());
                let wanted = channel.clone();
                let (key_present, access, settings) = self
                    .store
                    .read(move |conn| {
                        Ok((
                            monid_store::key_present(conn)?,
                            monid_store::effective_access(conn, &wanted, task_id)?,
                            monid_store::settings(conn)?,
                        ))
                    })
                    .await?;
                if !key_present {
                    return refused(
                        "This hub has no Monid key. Ask the user to add Monid in Settings > Connectors."
                            .to_owned(),
                    );
                }
                let channels = self.all_channels().await?;
                let wider = use_in_after(&settings.use_in, &channel, &channels);
                let rule = "Calls still follow the Monid approval rule of this channel.";
                match (provider, endpoint) {
                    (None, Some(_)) => {
                        return refused("Name the provider of this endpoint.".to_owned());
                    }
                    (None, None) => {
                        if access.as_ref().is_some_and(|a| !a.only_allowed) {
                            return refused(
                                "You have Monid already. Call search_tools to find a tool."
                                    .to_owned(),
                            );
                        }
                        let effect = match &wider {
                            Some(wider) => format!(
                                "Always changes Monid's Use in to {}.",
                                describe_use_in(wider, &channels)
                            ),
                            None => "Always gives this channel all Monid tools.".to_owned(),
                        };
                        (AccessTarget::Monid, format!("{rule} {effect}"))
                    }
                    (Some(provider), endpoint) => {
                        let probe = endpoint.unwrap_or("");
                        let blocked = channel.monid.blocked.iter().any(|b| {
                            b.covers(provider, probe)
                                && (endpoint.is_some() || b.endpoint.is_none())
                        });
                        if blocked {
                            return refused(
                                "The user marked this tool \"Never use here\", so you cannot ask for it. Find another way, or tell the user."
                                    .to_owned(),
                            );
                        }
                        if let (Some(access), Some(endpoint)) = (&access, endpoint)
                            && !access.refuses(provider, endpoint)
                        {
                            return refused(
                                "You have this tool already. Call get_monid_tool.".to_owned(),
                            );
                        }
                        let name = match endpoint {
                            Some(endpoint) => self.monid_tool_label(provider, endpoint).await,
                            None => format!("All {provider} tools"),
                        };
                        let effect = match &wider {
                            Some(wider) => format!(
                                "Always gives this channel only this, and changes Monid's Use in to {}.",
                                describe_use_in(wider, &channels)
                            ),
                            None => "Always adds it to Only these tools.".to_owned(),
                        };
                        (
                            AccessTarget::MonidTool {
                                provider: provider.to_owned(),
                                endpoint: endpoint.map(str::to_owned),
                                name,
                            },
                            format!("Monid tool. {rule} {effect}"),
                        )
                    }
                }
            }
            _ => {
                return refused(
                    "Name one connection, one skill, or monid. To ask for all of Monid, pass an empty monid object."
                        .to_owned(),
                );
            }
        };

        // The same request does not come again and again.
        let scope = denial_scope(task_id, &ctx.session.id.to_string());
        let key = target.key();
        let (denied_at, pending) = {
            let (scope, key) = (scope.clone(), key.clone());
            self.store
                .read(move |conn| {
                    let denied = monid_store::access_denied_at(conn, &scope, &key)?;
                    let pending = work::pending_approvals(conn, Some(channel_id))?
                        .into_iter()
                        .find_map(|approval| match &approval.details {
                            ApprovalDetails::AccessRequest(card)
                                if card.target.key() == key && approval.task_id == task_id =>
                            {
                                Some(approval.id)
                            }
                            _ => None,
                        });
                    Ok((denied, pending))
                })
                .await?
        };
        if denied_at.is_some_and(|at| self.now().as_millis() - at < DENIAL_HOLD_MS) {
            return refused(format!(
                "The user said no to {} earlier. Do not ask again unless the user asks for it.",
                target.label()
            ));
        }
        if let Some(approval_id) = pending {
            return Ok(RequestAccessResult::WaitingForUser {
                approval_id,
                message: "The card for this request still waits for the user.".to_owned(),
            });
        }

        let label = target.label().to_owned();
        let who = if task_id.is_some() {
            "this task"
        } else {
            "this channel"
        };
        let title = format!("Give {who} access to {label}");
        let card = AccessRequestCard {
            target,
            requested: args.duration,
            title: title.clone(),
            reason: args
                .reason
                .map(|r| crate::util::truncate_chars(r.trim(), REASON_MAX_CHARS))
                .filter(|r| !r.is_empty()),
            effect: permanent_effect,
            temporary_label: if task_id.is_some() {
                "Allow for this task".to_owned()
            } else {
                "Allow for 24 hours".to_owned()
            },
        };

        let approval_id = ApprovalId::new();
        let mut answer = self.connectors.wait_for(approval_id);
        if let Err(err) = self
            .post_hub_card(
                ctx,
                approval_id,
                title,
                ApprovalDetails::AccessRequest(Box::new(card)),
            )
            .await
        {
            self.connectors.take_waiter(approval_id);
            return Err(err);
        }
        let wait = self.connectors.timings().approval_wait;
        let status = match tokio::time::timeout(wait, &mut answer).await {
            Ok(Ok(status)) => Some(status),
            Ok(Err(_)) => None,
            Err(_) => {
                if self.connectors.take_waiter(approval_id).is_some() {
                    None
                } else {
                    answer.await.ok()
                }
            }
        };
        if status.is_none() {
            return Ok(RequestAccessResult::WaitingForUser {
                approval_id,
                message: "The user has not answered the card yet. Go on with other work. The answer arrives in a later turn.".to_owned(),
            });
        }
        let approval = self
            .store
            .read(move |conn| work::approval(conn, approval_id))
            .await?
            .ok_or_else(|| HubError::internal("an access card is gone"))?;
        self.carry_out_access(&approval).await
    }

    /// The name of a Monid tool. A name is only for the card, so a failure
    /// falls back to the endpoint.
    async fn monid_tool_label(&self, provider: &str, endpoint: &str) -> String {
        let client = self.connectors.monid_client();
        let found = match client {
            Ok(client) => client.inspect(provider, endpoint).await.ok(),
            Err(_) => None,
        };
        found
            .map(|tool| super::monid_run::tool_name(&tool))
            .unwrap_or_else(|| format!("{provider} {endpoint}"))
    }

    /// Carries out the answer to an access card.
    async fn carry_out_access(&self, approval: &Approval) -> Result<RequestAccessResult> {
        let ApprovalDetails::AccessRequest(card) = &approval.details else {
            return Err(HubError::internal("not an access card"));
        };
        let label = card.target.label().to_owned();
        let usage = match &card.target {
            AccessTarget::Connection { .. } => "Call list_connector_tools to see its tools.",
            _ => "Call search_tools to find a tool, then get_monid_tool.",
        };
        if approval.status != ApprovalStatus::Allowed {
            let key = card.target.key();
            let scope = denial_scope(approval.task_id, &approval.session_id.to_string());
            let now = self.now().as_millis();
            self.store
                .write(move |tx| tx.save_access_denial(&scope, &key, now))
                .await?;
            return Ok(RequestAccessResult::Denied {
                message: format!(
                    "The user did not give access to {label}. Find another way, or tell the user what you need. Do not ask again unless the user asks for it."
                ),
            });
        }
        let choice = approval.answer_option_id.as_deref().unwrap_or_default();
        if choice == ACCESS_CARD_ALWAYS {
            self.give_lasting_access(approval.channel_id, &card.target)
                .await?;
            return Ok(RequestAccessResult::Granted {
                message: format!("This channel has {label} from now on. {usage}"),
            });
        }
        if choice != ACCESS_CARD_TEMPORARY {
            return Err(HubError::internal("an access card has an unknown answer"));
        }
        let task_id = approval.task_id;
        let now = self.now();
        let grant = AccessGrant {
            id: AccessGrantId::new(),
            target: card.target.clone(),
            task_id,
            expires_at: task_id
                .is_none()
                .then(|| now.plus_millis(TEMPORARY_ACCESS_MS)),
            granted_at: now,
        };
        let channel_id = approval.channel_id;
        self.store
            .write(move |tx| {
                let mut channel = conversation::channel_or_404(tx.db(), channel_id)?;
                channel.access_grants.push(grant);
                channel.updated_at = tx.now();
                tx.save_channel(&channel)
            })
            .await?;
        let until = if task_id.is_some() {
            "until this task ends"
        } else {
            "for 24 hours"
        };
        Ok(RequestAccessResult::Granted {
            message: format!("You have {label} {until}. {usage}"),
        })
    }

    async fn give_lasting_access(
        &self,
        channel_id: ChannelId,
        target: &AccessTarget,
    ) -> Result<()> {
        let channels = self.all_channels().await?;
        let target_key = target.key();
        let channel = channels
            .iter()
            .find(|c| c.id == channel_id)
            .cloned()
            .ok_or_else(|| HubError::not_found("This channel does not exist."))?;
        if let AccessTarget::Connection { connection_id, .. } = target {
            let id = *connection_id;
            let connection = self
                .store
                .read(move |conn| con_store::connection_or_404(conn, id))
                .await?;
            if let Some(wider) = use_in_after(&connection.use_in, &channel, &channels) {
                self.update_connection_fields(id, None, Some(wider), None)
                    .await?;
            }
        }
        let target = target.clone();
        self.store
            .write(move |tx| {
                let mut channel = conversation::channel_or_404(tx.db(), channel_id)?;
                channel
                    .access_grants
                    .retain(|g| g.target.key() != target_key);
                match &target {
                    AccessTarget::Connection { connection_id, .. } => {
                        channel.disabled_connections.retain(|c| c != connection_id);
                    }
                    AccessTarget::Monid | AccessTarget::MonidTool { .. } => {
                        let mut settings = monid_store::settings(tx.db())?;
                        let had_monid =
                            !channel.monid.disabled && settings.use_in.matches_channel(&channel);
                        if let Some(wider) = use_in_after(&settings.use_in, &channel, &channels) {
                            settings.use_in = wider;
                            tx.save_monid_settings(&settings)?;
                        }
                        channel.monid.disabled = false;
                        match &target {
                            AccessTarget::MonidTool {
                                provider,
                                endpoint,
                                name,
                            } => {
                                // A channel with no Monid before gets only
                                // what the user allowed.
                                if !had_monid {
                                    channel.monid.only_allowed = true;
                                    channel.monid.allowed.clear();
                                }
                                let known = channel
                                    .monid
                                    .allowed
                                    .iter()
                                    .any(|a| same_tool(&a.tool, provider, endpoint.as_deref()));
                                if !known {
                                    channel.monid.allowed.push(MonidAllowedTool {
                                        tool: MonidToolRef {
                                            provider: provider.clone(),
                                            endpoint: endpoint.clone(),
                                            name: name.clone(),
                                        },
                                        run_without_asking: false,
                                    });
                                }
                            }
                            _ => channel.monid.only_allowed = false,
                        }
                    }
                }
                channel.updated_at = tx.now();
                tx.save_channel(&channel)
            })
            .await
    }

    /// The user answered an access card after the call stopped waiting.
    pub(crate) async fn late_access_answer(&self, approval: &Approval) -> Result<String> {
        Ok(match self.carry_out_access(approval).await? {
            RequestAccessResult::Granted { message }
            | RequestAccessResult::Denied { message }
            | RequestAccessResult::Refused { message }
            | RequestAccessResult::WaitingForUser { message, .. } => message,
        })
    }

    pub(crate) async fn channel_end_access(
        &self,
        _caller: &Caller,
        request: ChannelEndAccess,
    ) -> Result<ChannelResult> {
        let channel = self
            .store
            .write(move |tx| {
                let mut channel = conversation::channel_or_404(tx.db(), request.channel_id)?;
                channel.access_grants.retain(|g| g.id != request.grant_id);
                channel.updated_at = tx.now();
                tx.save_channel(&channel)?;
                Ok(channel)
            })
            .await?;
        Ok(ChannelResult { channel })
    }

    /// Removes the grants that have ended from every channel.
    pub(crate) async fn sweep_access_grants(&self) -> Result<()> {
        let now = self.now();
        self.store
            .write(move |tx| {
                for mut channel in conversation::channels(tx.db())? {
                    if channel.access_grants.is_empty() {
                        continue;
                    }
                    let db = tx.db();
                    let live = live_grants(&channel, now, |task| {
                        work::task(db, task).ok().flatten().is_none_or(|task| {
                            matches!(task.status, TaskStatus::Failed | TaskStatus::Resolved)
                        })
                    });
                    if live.len() != channel.access_grants.len() {
                        channel.access_grants = live;
                        tx.save_channel(&channel)?;
                    }
                }
                Ok(())
            })
            .await
    }
}

#[cfg(test)]
#[path = "access_tests.rs"]
mod tests;
