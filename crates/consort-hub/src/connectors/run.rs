//! The connector hub tools: `list_connector_tools`, `get_connector_tool`
//! and `run_connector_tool`, with the approval rules and the audit log.

use std::time::Instant;

use consort_proto::error::ErrorCode;
use consort_proto::ids::{ApprovalId, ConnectorCallId};
use consort_proto::model::{
    Approval, ApprovalDetails, ApprovalRule, ApprovalStatus, CallDecision, CallOutcome, Connection,
    ConnectionStatus, ConnectorCall, ConnectorCard, DisplayField, ToolKind, canonical_json,
};
use consort_proto::tools::{
    CONNECTOR_RESULT_MAX_BYTES, CONNECTOR_TOOLS_LIST_MAX, ConnectorToolInfo, GetConnectorToolArgs,
    GetConnectorToolResult, ListConnectorToolsArgs, ListConnectorToolsResult, RunConnectorToolArgs,
    RunConnectorToolResult,
};
use serde_json::Value;

use super::composio::{self, Tool};
use super::tool_kind;
use crate::error::{HubError, Result};
use crate::hub::{Hub, ToolCtx};
use crate::store::connectors as con_store;

/// The longest value of one field on a connector card.
const FIELD_VALUE_MAX_CHARS: usize = 2_000;
/// The most fields on a connector card.
const CARD_FIELDS_MAX: usize = 20;
/// The most result text in a `card_answered` event.
const LATE_RESULT_MAX_BYTES: usize = 32 * 1024;

/// Argument keys with a readable label, in the order the card shows them.
/// An email shows To, Subject and Body first.
const NAMED_FIELDS: &[(&str, &[&str])] = &[
    (
        "To",
        &[
            "to",
            "recipient_email",
            "recipient",
            "recipients",
            "to_email",
            "to_emails",
        ],
    ),
    ("Cc", &["cc", "cc_emails"]),
    ("Bcc", &["bcc", "bcc_emails"]),
    ("Subject", &["subject"]),
    (
        "Body",
        &[
            "body",
            "message_body",
            "html_body",
            "text",
            "content",
            "message",
        ],
    ),
];

fn field_value(value: &Value) -> String {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Array(items) if items.iter().all(Value::is_string) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    };
    crate::util::truncate_chars(&text, FIELD_VALUE_MAX_CHARS)
}

fn humanize(key: &str) -> String {
    let words = key.replace(['_', '-'], " ");
    let mut chars = words.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => key.to_owned(),
    }
}

/// The arguments of a call in a readable form for a connector card.
pub(crate) fn display_fields(arguments: &Value) -> Vec<DisplayField> {
    let Some(map) = arguments.as_object() else {
        return Vec::new();
    };
    let mut used = Vec::new();
    let mut out = Vec::new();
    for (label, keys) in NAMED_FIELDS {
        if let Some((key, value)) = keys
            .iter()
            .find_map(|k| map.get_key_value(*k).filter(|(_, v)| !v.is_null()))
        {
            used.push(key.as_str());
            out.push(DisplayField {
                label: (*label).to_owned(),
                value: field_value(value),
            });
        }
    }
    // Sorted by key: serde_json keeps insertion order only when some crate in
    // the build turns on `preserve_order`, so map order is not stable.
    let mut rest: Vec<(&String, &Value)> = map
        .iter()
        .filter(|(key, value)| !used.contains(&key.as_str()) && !value.is_null())
        .collect();
    rest.sort_by(|a, b| a.0.cmp(b.0));
    for (key, value) in rest {
        out.push(DisplayField {
            label: humanize(key),
            value: field_value(value),
        });
    }
    out.truncate(CARD_FIELDS_MAX);
    out
}

/// The data of a result, cut to the result limit. A cut result is the
/// start of its JSON text, as a string.
fn cut_result(data: Value) -> (Value, u64, bool) {
    let text = data.to_string();
    let size = text.len() as u64;
    if text.len() <= CONNECTOR_RESULT_MAX_BYTES {
        return (data, size, false);
    }
    let cut = crate::limits::json_prefix(&text, CONNECTOR_RESULT_MAX_BYTES - 1024);
    (Value::String(cut.to_owned()), size, true)
}

/// Where a call came from, for the audit log.
#[derive(Debug, Clone, Copy)]
struct CallScope {
    channel_id: consort_proto::ids::ChannelId,
    session_id: consort_proto::ids::SessionId,
    task_id: Option<consort_proto::ids::TaskId>,
}

impl CallScope {
    fn of(ctx: &ToolCtx) -> Self {
        Self {
            channel_id: ctx.channel.id,
            session_id: ctx.session.id,
            task_id: ctx.task.as_ref().map(|t| t.id),
        }
    }

    fn of_approval(approval: &Approval) -> Self {
        Self {
            channel_id: approval.channel_id,
            session_id: approval.session_id,
            task_id: approval.task_id,
        }
    }

    /// The key of "ask once per task": the task, or the coordinator session.
    fn grant_key(&self) -> String {
        match self.task_id {
            Some(task) => task.to_string(),
            None => self.session_id.to_string(),
        }
    }
}

/// What the audit log records about one call.
struct CallRecord<'a> {
    connection: &'a Connection,
    tool: &'a str,
    kind: ToolKind,
    arguments: &'a Value,
    decision: CallDecision,
    scope: CallScope,
}

impl Hub {
    /// Finds a connection of the session's effective set by its handle or
    /// ID. Returns it with its Composio account.
    pub(crate) async fn connection_in_set(
        &self,
        ctx: &ToolCtx,
        key: &str,
    ) -> Result<(Connection, String)> {
        let channel = ctx.channel.clone();
        let task_id = ctx.task.as_ref().map(|t| t.id);
        let key = key.trim().to_owned();
        let wanted = key.clone();
        let (set, row, other) = self
            .store
            .read(move |conn| {
                // Read the channel again: its exceptions may have changed.
                let channel = crate::store::conversation::channel_or_404(conn, channel.id)?;
                let set = con_store::effective_connections_for(conn, &channel, task_id)?;
                let other = con_store::requestable_connections(conn, &channel, task_id)?
                    .into_iter()
                    .find(|c| c.handle.eq_ignore_ascii_case(&wanted) || c.id.to_string() == wanted);
                let found = set
                    .iter()
                    .find(|c| c.handle.eq_ignore_ascii_case(&wanted) || c.id.to_string() == wanted)
                    .map(|c| c.id);
                let row = found
                    .map(|id| con_store::connection_row(conn, id))
                    .transpose()?
                    .flatten();
                Ok((set, row, other))
            })
            .await?;
        if let Some(other) = other {
            return Err(HubError::new(
                ErrorCode::ConnectorNotAvailable,
                format!(
                    "This channel does not have {}. Call request_access with connection {} to ask the user for it.",
                    other.name, other.handle
                ),
            ));
        }
        let Some(row) = row else {
            let names: Vec<&str> = set.iter().map(|c| c.handle.as_str()).collect();
            let message = if names.is_empty() {
                format!(
                    "There is no connector {key} in this channel. This channel has no connectors."
                )
            } else {
                format!(
                    "There is no connector {key} in this channel. Use one of these handles: {}.",
                    names.join(", ")
                )
            };
            return Err(HubError::new(ErrorCode::ConnectorNotAvailable, message));
        };
        let connection = row.connection;
        if let ConnectionStatus::NeedsSignIn { .. } = connection.status {
            return Err(needs_sign_in(&connection));
        }
        let account = row
            .composio_account_id
            .filter(|_| connection.status.is_active())
            .ok_or_else(|| needs_sign_in(&connection))?;
        Ok((connection, account))
    }

    pub(crate) async fn connector_tool(&self, connection: &Connection, slug: &str) -> Result<Tool> {
        let tools = self.service_tools(&connection.service).await?;
        tools
            .into_iter()
            .find(|t| t.slug.eq_ignore_ascii_case(slug.trim()))
            .ok_or_else(|| {
                HubError::invalid(format!(
                    "The connector {} has no tool {}. Call list_connector_tools to see its tools.",
                    connection.handle,
                    slug.trim()
                ))
            })
    }

    pub(crate) async fn tool_list_connector_tools(
        &self,
        ctx: &ToolCtx,
        args: ListConnectorToolsArgs,
    ) -> Result<Value> {
        let (connection, _) = self.connection_in_set(ctx, &args.connector).await?;
        let search = args
            .search
            .as_deref()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty());
        let words: Vec<String> = search
            .as_deref()
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default();
        let tools: Vec<ConnectorToolInfo> = self
            .service_tools(&connection.service)
            .await?
            .iter()
            .filter(|t| {
                let haystack = format!("{} {}", t.slug, t.description).to_lowercase();
                words.iter().all(|w| haystack.contains(w.as_str()))
            })
            .map(|t| {
                let kind = tool_kind(t);
                ConnectorToolInfo {
                    slug: t.slug.clone(),
                    description: crate::util::truncate_chars(t.description.trim(), 300),
                    kind,
                    approval: connection.rule_for_tool(&t.slug, kind),
                }
            })
            .collect();
        let has_more = tools.len() > CONNECTOR_TOOLS_LIST_MAX;
        let tools = tools.into_iter().take(CONNECTOR_TOOLS_LIST_MAX).collect();
        Ok(serde_json::to_value(ListConnectorToolsResult {
            connector: connection.handle.clone(),
            service_name: connection.service_name.clone(),
            tools,
            has_more,
        })?)
    }

    pub(crate) async fn tool_get_connector_tool(
        &self,
        ctx: &ToolCtx,
        args: GetConnectorToolArgs,
    ) -> Result<Value> {
        let (connection, _) = self.connection_in_set(ctx, &args.connector).await?;
        let mut tool = self.connector_tool(&connection, &args.tool).await?;
        if tool.input_parameters.is_null() {
            // The list can leave the schema out; the tool itself has it.
            tool = self.connectors.client()?.tool(&tool.slug).await?;
        }
        let kind = tool_kind(&tool);
        Ok(serde_json::to_value(GetConnectorToolResult {
            connector: connection.handle.clone(),
            tool: ConnectorToolInfo {
                slug: tool.slug.clone(),
                description: tool.description.clone(),
                kind,
                approval: connection.rule_for_tool(&tool.slug, kind),
            },
            input_schema: tool.input_parameters,
        })?)
    }

    pub(crate) async fn tool_run_connector_tool(
        &self,
        ctx: &ToolCtx,
        args: RunConnectorToolArgs,
    ) -> Result<Value> {
        let mut arguments = match args.arguments {
            Value::Null => Value::Object(Default::default()),
            Value::Object(map) => Value::Object(map),
            _ => {
                return Err(HubError::invalid(
                    "The arguments must be an object, as the tool's input schema describes.",
                ));
            }
        };
        let (connection, account) = self.connection_in_set(ctx, &args.connector).await?;
        self.connectors.client()?;
        let tool = self.connector_tool(&connection, &args.tool).await?;
        let kind = tool_kind(&tool);
        let voice_task = self
            .check_voice_task_call(ctx, &connection, &tool.slug, kind, &arguments)
            .await?;
        if kind != consort_proto::model::ToolKind::Read && self.in_research_turn(ctx).await? {
            return Err(crate::proactivity::turn::denied(
                crate::proactivity::turn::RESEARCH_REFUSAL,
            ));
        }
        // A draft or send tool: the voice, the disclosure line and the
        // commitment flags.
        // The assistant's own account sends in the assistant's name.
        let voice = if connection.assistant_owned {
            crate::voice::card::CardVoice::default()
        } else {
            self.voice_for_call(ctx, &tool.slug, kind, &mut arguments)
                .await?
        };
        let scope = CallScope::of(ctx);
        let mut rule = connection.rule_for_tool(&tool.slug, kind);
        // A proactive turn and a draft task ask before each call that writes
        // or sends, whatever the connection allows and whatever the user
        // allowed before. So does a call with flags, and every send while
        // the voice is on: the card is the only place where the user sees
        // the text that goes out in their name. Such an answer saves no
        // allow for later calls.
        // The assistant's own account follows its rules: the user chose
        // that on a card that names the risk (ADR 0020).
        if (kind != consort_proto::model::ToolKind::Read
            && !connection.assistant_owned
            && self.connector_calls_ask_each_time(ctx).await?)
            || voice.must_ask()
        {
            rule = ApprovalRule::AskEveryTime;
        }
        let decision = match rule {
            ApprovalRule::RunWithoutAsking => Some(CallDecision::RanWithoutAsking),
            ApprovalRule::AskOncePerTask => {
                let (key, id) = (scope.grant_key(), connection.id);
                let granted = self
                    .store
                    .read(move |conn| con_store::has_grant(conn, &key, id, kind))
                    .await?;
                granted.then_some(CallDecision::AllowedEarlierInTask)
            }
            ApprovalRule::AskEveryTime => None,
        };
        let record = CallRecord {
            connection: &connection,
            tool: &tool.slug,
            kind,
            arguments: &arguments,
            decision: decision.unwrap_or(CallDecision::AllowedByUser),
            scope,
        };
        if decision.is_some() {
            let mut result = self.execute_call(&record, &account).await?;
            // A draft or send that the user did not see is the assistant's
            // text: it never teaches the voice.
            if voice.outgoing.is_some()
                && matches!(
                    result,
                    RunConnectorToolResult::Ran {
                        successful: true,
                        ..
                    }
                )
            {
                let (id, slug, sent) = (connection.id, tool.slug.clone(), arguments.clone());
                let context = voice.voice.as_ref().map(|v| v.context.clone());
                self.store
                    .write(move |tx| {
                        crate::voice::card::record_unasked_pair(tx, id, &slug, &sent, context)
                    })
                    .await?;
            }
            self.redact_for_voice_task(voice_task, &connection, &mut result)
                .await?;
            return Ok(serde_json::to_value(result)?);
        }

        // Ask the user with a connector card, and wait for the answer.
        let approval_id = ApprovalId::new();
        let mut answer = self.connectors.wait_for(approval_id);
        let summary = format!(
            "Run {} on {} ({})",
            tool.slug, connection.name, connection.service_name
        );
        let mut card = ConnectorCard {
            connection_id: connection.id,
            connection_name: connection.name.clone(),
            service_name: connection.service_name.clone(),
            tool: tool.slug.clone(),
            tool_kind: kind,
            fields: display_fields(&arguments),
            arguments: arguments.clone(),
            outgoing: None,
            editable: Vec::new(),
            voice: None,
            flags: Vec::new(),
            draft_arguments: None,
            not_my_voice: false,
            ask_rule: Some(rule),
            replaced_by: None,
        };
        voice.apply(&mut card);
        // An answer to a card of an outgoing call (a send or a draft) saves
        // no allow: the grant covers the whole kind of the tool, so one
        // click on Send would let later sends to anyone run without a card.
        let saves_grant = card.outgoing.is_none();
        if let Err(err) = self
            .post_hub_card(
                ctx,
                approval_id,
                summary,
                ApprovalDetails::Connector(Box::new(card)),
            )
            .await
        {
            self.connectors.take_waiter(approval_id);
            return Err(err);
        }
        let wait = self.connectors.timings().approval_wait;
        let mut status = match tokio::time::timeout(wait, &mut answer).await {
            Ok(Ok(status)) => Some(status),
            Ok(Err(_)) => None,
            Err(_) => {
                if self.connectors.take_waiter(approval_id).is_some() {
                    // Nobody answered in time. A later answer runs the call.
                    None
                } else {
                    // The answer came at the same moment and took the waiter.
                    answer.await.ok()
                }
            }
        };
        if status.is_none() {
            // A message may have replaced the card at the moment the wait
            // ended. Read the card again: a Replaced card must not return
            // `waiting_for_user`, because nothing would run it later.
            let closed = self
                .store
                .read(move |conn| crate::store::work::approval(conn, approval_id))
                .await?
                .is_some_and(|a| a.status == ApprovalStatus::Replaced);
            if closed {
                status = Some(ApprovalStatus::Replaced);
            }
        }
        let Some(status) = status else {
            return Ok(serde_json::to_value(RunConnectorToolResult::WaitingForUser {
                approval_id,
                message: "The user has not answered the approval card yet. Go on with other work. When the user answers, the hub runs the call and sends the result.".to_owned(),
            })?);
        };
        // The user sent a message while the draft waited. The card closed,
        // and the message is the result of this call. Nothing ran.
        if status == ApprovalStatus::Replaced {
            let approval = self
                .store
                .read(move |conn| crate::store::work::approval(conn, approval_id))
                .await?;
            let message = match approval.map(|a| a.details) {
                Some(ApprovalDetails::Connector(card)) => {
                    crate::connectors::replace::replaced_message(&card)
                }
                _ => "The user sent a message while your draft waited, and the card closed. Ask the user what they want, then write the draft again.".to_owned(),
            };
            return Ok(serde_json::to_value(RunConnectorToolResult::Replaced {
                approval_id,
                message,
            })?);
        }
        // The user may have edited the text on the card: the call runs with
        // the arguments of the card.
        let edited = if status == ApprovalStatus::Allowed {
            self.card_arguments(approval_id)
                .await?
                .unwrap_or_else(|| arguments.clone())
        } else {
            arguments.clone()
        };
        let record = CallRecord {
            arguments: &edited,
            ..record
        };
        let grant_rule = if saves_grant {
            rule
        } else {
            ApprovalRule::AskEveryTime
        };
        let mut result = self
            .answered_call(&record, &account, grant_rule, status)
            .await?;
        self.redact_for_voice_task(voice_task, &connection, &mut result)
            .await?;
        Ok(serde_json::to_value(result)?)
    }

    /// The arguments on a connector card now, after any edit.
    async fn card_arguments(&self, approval_id: ApprovalId) -> Result<Option<Value>> {
        let approval = self
            .store
            .read(move |conn| crate::store::work::approval(conn, approval_id))
            .await?;
        Ok(approval.and_then(|a| match a.details {
            ApprovalDetails::Connector(card) => Some(card.arguments),
            _ => None,
        }))
    }

    /// In the voice task, text that the assistant wrote leaves the result,
    /// and so do direct messages and private channels that the user did not
    /// choose.
    async fn redact_for_voice_task(
        &self,
        voice_task: bool,
        connection: &Connection,
        result: &mut RunConnectorToolResult,
    ) -> Result<()> {
        if !voice_task {
            return Ok(());
        }
        if let RunConnectorToolResult::Ran { data, .. } = result {
            if !self.voice_private_allowed(connection).await? {
                let dropped = crate::voice::learn::drop_private_chats(data);
                if dropped > 0 {
                    tracing::info!(
                        dropped,
                        "left private chats out of what the voice task read"
                    );
                }
            }
            let texts = self.assistant_texts().await?;
            let left_out = crate::voice::learn::redact_assistant_text(data, &texts);
            if left_out > 0 {
                tracing::info!(
                    left_out,
                    "left assistant text out of what the voice task read"
                );
            }
        }
        Ok(())
    }

    /// Finishes a call after the user answered its card.
    async fn answered_call(
        &self,
        record: &CallRecord<'_>,
        account: &str,
        rule: ApprovalRule,
        status: ApprovalStatus,
    ) -> Result<RunConnectorToolResult> {
        match status {
            ApprovalStatus::Allowed => {
                if rule == ApprovalRule::AskOncePerTask {
                    let (key, id, kind) =
                        (record.scope.grant_key(), record.connection.id, record.kind);
                    self.store
                        .write(move |tx| tx.save_grant(&key, id, kind))
                        .await?;
                }
                self.execute_call(record, account).await
            }
            other => {
                let (decision, message) = if other == ApprovalStatus::Denied {
                    (
                        CallDecision::DeniedByUser,
                        "The user did not allow this call. Do not try it again unless the user asks.",
                    )
                } else {
                    (
                        CallDecision::Expired,
                        "The approval card closed without an answer, so the call did not run.",
                    )
                };
                let call_id = self
                    .write_audit(record, decision, CallOutcome::NotRun, None, None, 0, 0)
                    .await?;
                Ok(RunConnectorToolResult::Denied {
                    call_id,
                    message: message.to_owned(),
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_audit(
        &self,
        record: &CallRecord<'_>,
        decision: CallDecision,
        outcome: CallOutcome,
        error: Option<String>,
        log_id: Option<String>,
        result_bytes: u64,
        duration_ms: u64,
    ) -> Result<ConnectorCallId> {
        let call = ConnectorCall {
            id: ConnectorCallId::new(),
            connection_id: record.connection.id,
            channel_id: Some(record.scope.channel_id),
            session_id: Some(record.scope.session_id),
            task_id: record.scope.task_id,
            tool: record.tool.to_owned(),
            tool_kind: record.kind,
            arguments_sha256: crate::util::sha256(canonical_json(record.arguments).as_bytes()),
            decision,
            outcome,
            error: error.map(|e| crate::util::truncate_chars(&e, 1_000)),
            composio_log_id: log_id,
            result_bytes,
            duration_ms,
            at: self.now(),
        };
        let id = call.id;
        let connection_id = record.connection.id;
        let ran = outcome != CallOutcome::NotRun;
        self.store
            .write(move |tx| {
                tx.save_connector_call(&call)?;
                if ran && let Some(mut connection) = con_store::connection(tx.db(), connection_id)?
                {
                    connection.last_used_at = Some(tx.now());
                    tx.save_connection(&connection)?;
                }
                Ok(())
            })
            .await?;
        Ok(id)
    }

    /// Runs a call with the connection's exact connected account, writes
    /// the audit record and cuts a large result.
    async fn execute_call(
        &self,
        record: &CallRecord<'_>,
        account: &str,
    ) -> Result<RunConnectorToolResult> {
        let client = self.connectors.client()?;
        let user_id = self.composio_user_id().await?;
        let started = Instant::now();
        let executed = client
            .execute(record.tool, account, &user_id, record.arguments)
            .await;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let execution = match executed {
            Ok(execution) => execution,
            Err(err) => {
                let message = err.to_error_body().message;
                let expired = !composio::is_unauthorized(&err)
                    && self
                        .check_account(&client, record.connection.id, account)
                        .await;
                self.write_audit(
                    record,
                    record.decision,
                    CallOutcome::Failed,
                    Some(message),
                    None,
                    0,
                    duration_ms,
                )
                .await?;
                if expired {
                    return Err(needs_sign_in(record.connection));
                }
                return Err(err);
            }
        };
        let (data, size, truncated) = cut_result(execution.data);
        let error = execution
            .error
            .filter(|e| !e.trim().is_empty())
            .map(|e| crate::util::truncate_chars(e.trim(), 4_000));
        let outcome = if execution.successful {
            CallOutcome::Succeeded
        } else {
            CallOutcome::Failed
        };
        let call_id = self
            .write_audit(
                record,
                record.decision,
                outcome,
                error.clone(),
                execution.log_id,
                size,
                duration_ms,
            )
            .await?;
        Ok(RunConnectorToolResult::Ran {
            call_id,
            successful: execution.successful,
            data,
            error,
            truncated,
        })
    }

    /// The user answered a connector card after the call stopped waiting.
    /// Allow runs the call now. Returns the text for the session.
    pub(crate) async fn late_connector_answer(&self, approval: &Approval) -> Result<String> {
        let ApprovalDetails::Connector(card) = &approval.details else {
            return Err(HubError::internal("not a connector card"));
        };
        let id = card.connection_id;
        let row = self
            .store
            .read(move |conn| con_store::connection_row(conn, id))
            .await?;
        let Some(row) = row else {
            return Ok(format!(
                "The user answered the card for {}, but the connection {} is gone, so the call did not run.",
                card.tool, card.connection_name
            ));
        };
        let connection = row.connection;
        let scope = CallScope::of_approval(approval);
        let record = CallRecord {
            connection: &connection,
            tool: &card.tool,
            kind: card.tool_kind,
            arguments: &card.arguments,
            decision: CallDecision::AllowedByUser,
            scope,
        };
        // The scope may have changed while the card waited.
        let channel_id = approval.channel_id;
        let approval_task = approval.task_id;
        let still_in_set = self
            .store
            .read(move |conn| {
                let channel = crate::store::conversation::channel_or_404(conn, channel_id)?;
                Ok(
                    con_store::effective_connections_for(conn, &channel, approval_task)?
                        .iter()
                        .any(|c| c.id == id),
                )
            })
            .await?;
        let account = row
            .composio_account_id
            .filter(|_| connection.status.is_active());
        let (Some(account), true) = (account, still_in_set) else {
            if approval.status == ApprovalStatus::Allowed {
                self.write_audit(
                    &record,
                    CallDecision::AllowedByUser,
                    CallOutcome::NotRun,
                    Some("The connection is not available".to_owned()),
                    None,
                    0,
                    0,
                )
                .await?;
            }
            return Ok(format!(
                "{} on {} did not run: the connection is not available in this channel now, or needs a new sign-in.",
                card.tool, connection.name
            ));
        };
        // The rule that made the card ask, as the hub decided when it posted
        // the card. A proactive turn, a draft task, flags and a send in the
        // voice asked every time, so their late answer saves no allow.
        // A card of an outgoing call saves no allow either.
        let rule = if card.outgoing.is_some() {
            ApprovalRule::AskEveryTime
        } else {
            card.ask_rule.unwrap_or(ApprovalRule::AskEveryTime)
        };
        let result = self
            .answered_call(&record, &account, rule, approval.status)
            .await;
        Ok(match result {
            Ok(RunConnectorToolResult::Ran {
                call_id,
                successful,
                data,
                error,
                truncated,
            }) => {
                let text = data.to_string();
                let shown = crate::limits::json_prefix(&text, LATE_RESULT_MAX_BYTES);
                let cut = if truncated || shown.len() < text.len() {
                    " The result is cut."
                } else {
                    ""
                };
                if successful {
                    format!(
                        "The user allowed {} on {}, and it ran (call {call_id}).{cut} Result: {shown}",
                        card.tool, connection.name
                    )
                } else {
                    format!(
                        "The user allowed {} on {}, but it failed (call {call_id}): {}",
                        card.tool,
                        connection.name,
                        error.unwrap_or_else(|| "no reason given".to_owned())
                    )
                }
            }
            Ok(RunConnectorToolResult::Denied { message, .. }) => {
                format!("{} on {}: {message}", card.tool, connection.name)
            }
            Ok(RunConnectorToolResult::WaitingForUser { .. }) => String::new(),
            Ok(RunConnectorToolResult::Replaced { message, .. }) => message,
            Err(err) => format!(
                "The user allowed {} on {}, but it did not run: {}",
                card.tool,
                connection.name,
                err.to_error_body().message
            ),
        })
    }
}

fn needs_sign_in(connection: &Connection) -> HubError {
    HubError::new(
        ErrorCode::NeedsUser,
        format!(
            "{} needs a new sign-in. Ask the user to reconnect it in Settings > Connectors.",
            connection.name
        ),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn an_email_shows_to_subject_and_body_first() {
        let fields = display_fields(&json!({
            "user_id": "me",
            "body": "Hello",
            "recipient_email": "a@example.com",
            "subject": "Hi",
            "extra_recipients": ["b@example.com", "c@example.com"],
        }));
        let labels: Vec<&str> = fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["To", "Subject", "Body", "Extra recipients", "User id"]
        );
        assert_eq!(fields[3].value, "b@example.com, c@example.com");
    }

    #[test]
    fn a_large_result_is_cut() {
        let (data, size, truncated) = cut_result(json!({"blob": "x".repeat(300 * 1024)}));
        assert!(truncated);
        assert!(size > 300 * 1024);
        assert!(data.as_str().unwrap().len() <= CONNECTOR_RESULT_MAX_BYTES);
        let (data, _, truncated) = cut_result(json!({"ok": true}));
        assert!(!truncated);
        assert_eq!(data, json!({"ok": true}));
    }
}
