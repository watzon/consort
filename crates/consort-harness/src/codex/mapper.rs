//! Maps `codex app-server` notifications and server requests to Consort
//! session events.

use std::path::PathBuf;

use consort_proto::ids::{RequestId, SessionId, TurnId};
use consort_proto::model::{
    ApprovalDetails, AssistantMessagePayload, COMMAND_OUTPUT_TAIL_BYTES, CommandPayload,
    FileChange, FileChangeKind, FileChangePayload, ItemPayload, ItemStatus, PlanPayload, PlanStep,
    PlanStepStatus, ReasoningPayload, RequestOption, RequestOptionKind, SessionState,
    ToolCallPayload, Usage, WebSearchPayload, tail_of,
};
use consort_proto::session_host_link::{
    QuestionDetails, RequestBody, RequestOpened, SessionEvent, TurnEndReason,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::protocol::{
    CommandApprovalParams, DeltaNotification, ErrorNotification, ExecStatus,
    FileChangeApprovalParams, FileUpdateChange, ItemNotification, PatchChangeKind,
    PermissionsApprovalParams, ThreadItem, TokenUsageNotification, TurnError, TurnNotification,
    TurnPlanNotification, TurnStatus, UserInputQuestion, method,
};
use crate::{HarnessKind, ItemTracker, SIGNED_OUT_ERROR_CODE, command_cwd, sign_in_hint};

fn parse<T: DeserializeOwned>(params: &Value) -> Option<T> {
    match serde_json::from_value(params.clone()) {
        Ok(v) => Some(v),
        Err(err) => {
            tracing::debug!(error = %err, "codex sent params that the adapter cannot read");
            None
        }
    }
}

fn exec_status(status: ExecStatus) -> ItemStatus {
    match status {
        ExecStatus::InProgress => ItemStatus::InProgress,
        ExecStatus::Completed => ItemStatus::Completed,
        ExecStatus::Failed | ExecStatus::Declined => ItemStatus::Failed,
    }
}

fn file_change(change: &FileUpdateChange) -> FileChange {
    let (kind, from_path, path) = match &change.kind {
        PatchChangeKind::Add => (FileChangeKind::Add, None, change.path.clone()),
        PatchChangeKind::Delete => (FileChangeKind::Delete, None, change.path.clone()),
        PatchChangeKind::Update {
            move_path: Some(to),
        } => (
            FileChangeKind::Rename,
            Some(change.path.clone()),
            to.clone(),
        ),
        PatchChangeKind::Update { move_path: None } => {
            (FileChangeKind::Modify, None, change.path.clone())
        }
    };
    let mut added = 0_u32;
    let mut removed = 0_u32;
    for line in change.diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added = added.saturating_add(1);
        } else if line.starts_with('-') {
            removed = removed.saturating_add(1);
        }
    }
    if matches!(change.kind, PatchChangeKind::Add) && added == 0 {
        added = u32::try_from(change.diff.lines().count()).unwrap_or(u32::MAX);
    }
    FileChange {
        path,
        change: kind,
        from_path,
        lines_added: added,
        lines_removed: removed,
        diff: (!change.diff.trim().is_empty()).then(|| change.diff.clone()),
    }
}

/// The text content of an MCP tool result.
fn mcp_output(result: Option<&Value>, error: Option<&Value>) -> Option<String> {
    if let Some(message) = error.and_then(|e| e.get("message")).and_then(Value::as_str) {
        return Some(message.to_owned());
    }
    let content = result?.get("content")?.as_array()?;
    let text: Vec<&str> = content
        .iter()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect();
    if text.is_empty() {
        None
    } else {
        Some(
            tail_of(&text.join("\n"), COMMAND_OUTPUT_TAIL_BYTES)
                .0
                .to_owned(),
        )
    }
}

/// The payload and status of a thread item, or `None` for an item that
/// Consort does not show.
pub fn item_payload(item: &ThreadItem) -> Option<(String, ItemPayload, ItemStatus)> {
    Some(match item {
        ThreadItem::AgentMessage { id, text } => (
            id.clone(),
            ItemPayload::AssistantMessage(AssistantMessagePayload { text: text.clone() }),
            ItemStatus::InProgress,
        ),
        ThreadItem::Reasoning {
            id,
            summary,
            content,
        } => {
            let text = if summary.is_empty() {
                content.join("\n")
            } else {
                summary.join("\n")
            };
            (
                id.clone(),
                ItemPayload::Reasoning(ReasoningPayload { text }),
                ItemStatus::InProgress,
            )
        }
        ThreadItem::CommandExecution {
            id,
            command,
            cwd,
            aggregated_output,
            exit_code,
            duration_ms,
            status,
        } => {
            let mut payload = CommandPayload {
                command: command.clone(),
                cwd: cwd.clone(),
                exit_code: *exit_code,
                duration_ms: duration_ms.and_then(|d| u64::try_from(d).ok()),
                ..CommandPayload::default()
            };
            if let Some(output) = aggregated_output {
                payload.append_output(output);
            }
            (
                id.clone(),
                ItemPayload::Command(payload),
                exec_status(*status),
            )
        }
        ThreadItem::FileChange {
            id,
            changes,
            status,
        } => (
            id.clone(),
            ItemPayload::FileChange(FileChangePayload {
                changes: changes.iter().map(file_change).collect(),
            }),
            exec_status(*status),
        ),
        ThreadItem::McpToolCall {
            id,
            server,
            tool,
            arguments,
            result,
            error,
            status,
        } => {
            let failed = status == "failed" || error.is_some();
            let item_status = match status.as_str() {
                "completed" if !failed => ItemStatus::Completed,
                "inProgress" => ItemStatus::InProgress,
                _ => ItemStatus::Failed,
            };
            (
                id.clone(),
                ItemPayload::ToolCall(ToolCallPayload {
                    server: Some(server.clone()),
                    tool: tool.clone(),
                    arguments: arguments.clone(),
                    output: mcp_output(result.as_ref(), error.as_ref()),
                    is_error: failed,
                }),
                item_status,
            )
        }
        ThreadItem::WebSearch { id, query } => (
            id.clone(),
            ItemPayload::WebSearch(WebSearchPayload {
                query: query.clone(),
                urls: Vec::new(),
            }),
            ItemStatus::InProgress,
        ),
        ThreadItem::UserMessage { .. } | ThreadItem::Plan { .. } | ThreadItem::Other => {
            return None;
        }
    })
}

/// The state of one Codex session's mapping.
#[derive(Debug, Clone)]
pub struct CodexMapper {
    tracker: ItemTracker,
    current_turn: Option<String>,
    /// The session's workspace: the `cwd` of `thread/start`.
    workspace: Option<PathBuf>,
}

impl CodexMapper {
    /// A mapper for one session.
    pub fn new(session_id: SessionId) -> Self {
        Self {
            tracker: ItemTracker::new(session_id),
            current_turn: None,
            workspace: None,
        }
    }

    /// Sets the session's workspace. A command approval that names no
    /// folder runs there.
    pub fn with_workspace(mut self, workspace: impl Into<PathBuf>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }

    /// The Codex turn that runs, if any.
    pub fn current_turn(&self) -> Option<&str> {
        self.current_turn.as_deref()
    }

    /// Marks a turn as running, from the `turn/start` response.
    pub fn turn_accepted(&mut self, turn_id: &str) {
        self.current_turn = Some(turn_id.to_owned());
        self.tracker.set_turn(TurnId::new(turn_id));
    }

    fn append(&mut self, delta: &DeltaNotification, reasoning: bool) -> Option<SessionEvent> {
        if !self.tracker.contains(&delta.item_id) {
            let payload = if reasoning {
                ItemPayload::Reasoning(ReasoningPayload {
                    text: String::new(),
                })
            } else {
                ItemPayload::AssistantMessage(AssistantMessagePayload {
                    text: String::new(),
                })
            };
            self.tracker
                .upsert(&delta.item_id, payload, ItemStatus::InProgress);
        }
        self.tracker
            .update(&delta.item_id, |payload, _| match payload {
                ItemPayload::AssistantMessage(m) => m.text.push_str(&delta.delta),
                ItemPayload::Reasoning(r) => r.text.push_str(&delta.delta),
                _ => {}
            })
    }

    /// Maps one notification.
    pub fn map_notification(&mut self, name: &str, params: &Value) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        match name {
            method::TURN_STARTED => {
                if let Some(n) = parse::<TurnNotification>(params) {
                    self.turn_accepted(&n.turn.id);
                    events.push(SessionEvent::TurnStarted {
                        turn_id: TurnId::new(n.turn.id),
                    });
                    events.push(SessionEvent::State {
                        state: SessionState::Active,
                    });
                }
            }
            method::TURN_COMPLETED => {
                if let Some(n) = parse::<TurnNotification>(params) {
                    let (reason, item_status) = match n.turn.status {
                        TurnStatus::Completed | TurnStatus::InProgress => {
                            (TurnEndReason::Completed, ItemStatus::Completed)
                        }
                        TurnStatus::Interrupted => {
                            (TurnEndReason::Interrupted, ItemStatus::Completed)
                        }
                        TurnStatus::Failed => (TurnEndReason::Failed, ItemStatus::Failed),
                    };
                    let signed_out = n
                        .turn
                        .error
                        .as_ref()
                        .is_some_and(TurnError::is_unauthorized);
                    let info = n
                        .turn
                        .error
                        .as_ref()
                        .map(TurnError::failure)
                        .unwrap_or_default();
                    let mut error = n.turn.error.map(|e| match e.additional_details {
                        Some(more) => format!("{} {more}", e.message),
                        None => e.message,
                    });
                    if signed_out {
                        // The login is gone or expired. Tell the worker
                        // first, and end the turn with the next step.
                        let hint = sign_in_hint(HarnessKind::Codex, error.as_deref().unwrap_or(""));
                        events.push(SessionEvent::Error {
                            code: SIGNED_OUT_ERROR_CODE.to_owned(),
                            message: hint.clone(),
                        });
                        error = Some(hint);
                    }
                    events.extend(self.tracker.finish_open(item_status));
                    events.push(SessionEvent::TurnEnded {
                        turn_id: TurnId::new(n.turn.id),
                        reason,
                        error,
                        error_kind: (reason == TurnEndReason::Failed
                            && info.kind != crate::TurnErrorKind::Unknown)
                            .then_some(info.kind),
                        resets_at: info.resets_at,
                    });
                    events.push(SessionEvent::State {
                        state: SessionState::Idle,
                    });
                    self.current_turn = None;
                }
            }
            method::ITEM_STARTED | method::ITEM_COMPLETED => {
                if let Some(n) = parse::<ItemNotification>(params)
                    && let Some((id, payload, status)) = item_payload(&n.item)
                {
                    let status =
                        if name == method::ITEM_COMPLETED && status == ItemStatus::InProgress {
                            ItemStatus::Completed
                        } else {
                            status
                        };
                    if self.tracker.turn().as_str() != n.turn_id {
                        self.tracker.set_turn(TurnId::new(&n.turn_id));
                    }
                    events.push(self.tracker.upsert(&id, payload, status));
                }
            }
            method::AGENT_MESSAGE_DELTA => {
                if let Some(d) = parse::<DeltaNotification>(params)
                    && let Some(event) = self.append(&d, false)
                {
                    events.push(event);
                }
            }
            method::REASONING_TEXT_DELTA | method::REASONING_SUMMARY_DELTA => {
                if let Some(d) = parse::<DeltaNotification>(params)
                    && let Some(event) = self.append(&d, true)
                {
                    events.push(event);
                }
            }
            method::COMMAND_OUTPUT_DELTA => {
                if let Some(d) = parse::<DeltaNotification>(params) {
                    // Keep the tail in the tracker for the next item event,
                    // and stream the new text.
                    let _ = self.tracker.update(&d.item_id, |payload, _| {
                        if let ItemPayload::Command(c) = payload {
                            c.append_output(&d.delta);
                        }
                    });
                    if let Some(item_id) = self.tracker.item_id(&d.item_id) {
                        events.push(SessionEvent::OutputDelta {
                            item_id,
                            text: d.delta,
                        });
                    }
                }
            }
            method::TOKEN_USAGE_UPDATED => {
                if let Some(n) = parse::<TokenUsageNotification>(params) {
                    let total = n.token_usage.total;
                    events.push(SessionEvent::Usage(Usage {
                        input_tokens: total.input_tokens,
                        output_tokens: total.output_tokens,
                        cached_tokens: total.cached_input_tokens,
                        cost_usd: None,
                    }));
                }
            }
            method::TURN_PLAN_UPDATED => {
                if let Some(n) = parse::<TurnPlanNotification>(params) {
                    let steps: Vec<PlanStep> = n
                        .plan
                        .into_iter()
                        .map(|s| PlanStep {
                            text: s.step,
                            status: match s.status.as_str() {
                                "inProgress" => PlanStepStatus::InProgress,
                                "completed" => PlanStepStatus::Completed,
                                _ => PlanStepStatus::Pending,
                            },
                        })
                        .collect();
                    let done = !steps.is_empty()
                        && steps.iter().all(|s| s.status == PlanStepStatus::Completed);
                    let status = if done {
                        ItemStatus::Completed
                    } else {
                        ItemStatus::InProgress
                    };
                    let key = format!("plan:{}", n.turn_id);
                    events.push(self.tracker.upsert(
                        &key,
                        ItemPayload::Plan(PlanPayload { steps }),
                        status,
                    ));
                }
            }
            method::ERROR => {
                if let Some(n) = parse::<ErrorNotification>(params)
                    && !n.will_retry
                {
                    events.push(SessionEvent::Error {
                        code: "codex_error".to_owned(),
                        message: n.error.message,
                    });
                }
            }
            _ => {}
        }
        events
    }

    /// The `request.opened` of a command approval.
    ///
    /// A network approval (`networkApprovalContext`) is for a command that
    /// needs the network. It keeps the command details, with the command and
    /// its folder, so the user sees what uses the host. The summary names the
    /// host. `ApprovalDetails::Network` has only the target, so it would
    /// hide the command.
    pub fn command_approval(
        &self,
        request_id: RequestId,
        params: &CommandApprovalParams,
    ) -> RequestOpened {
        let command =
            params
                .command
                .clone()
                .unwrap_or_else(|| match self.tracker.payload(&params.item_id) {
                    Some(ItemPayload::Command(c)) => c.command.clone(),
                    _ => "a command".to_owned(),
                });
        let network = params.network_approval_context.as_ref().map(network_target);
        let summary = match (&network, &params.reason) {
            (Some(target), Some(reason)) => {
                let reason = reason.trim_end();
                let stop = if reason.ends_with(['.', '!', '?']) {
                    ""
                } else {
                    "."
                };
                format!("{reason}{stop} The command connects to {target}.")
            }
            (Some(target), None) => format!("Run `{command}` with network access to {target}"),
            (None, Some(reason)) => reason.clone(),
            (None, None) => format!("Run `{command}`"),
        };
        let details = ApprovalDetails::Command {
            command,
            cwd: command_cwd(params.cwd.as_deref(), self.workspace.as_deref()),
            check_note: None,
            blocked: Vec::new(),
        };
        RequestOpened {
            request_id,
            summary,
            body: RequestBody::Approval(details),
            options: decision_options(),
        }
    }

    /// The `request.opened` of a file change approval.
    pub fn file_change_approval(
        &self,
        request_id: RequestId,
        params: &FileChangeApprovalParams,
    ) -> RequestOpened {
        let mut paths: Vec<String> = match self.tracker.payload(&params.item_id) {
            Some(ItemPayload::FileChange(f)) => f.changes.iter().map(|c| c.path.clone()).collect(),
            _ => Vec::new(),
        };
        if paths.is_empty()
            && let Some(root) = &params.grant_root
        {
            paths.push(root.clone());
        }
        let summary = params
            .reason
            .clone()
            .unwrap_or_else(|| match paths.as_slice() {
                [one] => format!("Edit {one}"),
                [] => "Edit files".to_owned(),
                many => format!("Edit {} files", many.len()),
            });
        RequestOpened {
            request_id,
            summary,
            body: RequestBody::Approval(ApprovalDetails::FileChange { paths }),
            options: decision_options(),
        }
    }

    /// The `request.opened` of a permissions request.
    pub fn permissions_approval(
        request_id: RequestId,
        params: &PermissionsApprovalParams,
    ) -> RequestOpened {
        let text = params
            .reason
            .clone()
            .unwrap_or_else(|| "Codex asks for more permissions.".to_owned());
        RequestOpened {
            request_id,
            summary: text.clone(),
            body: RequestBody::Approval(ApprovalDetails::Other {
                text: Some(format!("{text}\n\n{}", params.permissions)),
            }),
            options: vec![
                option("turn", "Allow for this turn", RequestOptionKind::AllowOnce),
                option(
                    "session",
                    "Allow for this session",
                    RequestOptionKind::AllowAlways,
                ),
                option("deny", "Deny", RequestOptionKind::DenyOnce),
            ],
        }
    }

    /// The `request.opened` of one user input question.
    pub fn question(request_id: RequestId, question: &UserInputQuestion) -> RequestOpened {
        let options: Vec<RequestOption> = question
            .options
            .iter()
            .flatten()
            .enumerate()
            .map(|(i, o)| RequestOption {
                id: i.to_string(),
                label: o.label.clone(),
                kind: RequestOptionKind::Choice,
                description: o.description.clone(),
            })
            .collect();
        let allow_free_text = question.is_other || options.is_empty();
        RequestOpened {
            request_id,
            summary: if question.header.is_empty() {
                question.question.clone()
            } else {
                question.header.clone()
            },
            body: RequestBody::Question(QuestionDetails {
                question: question.question.clone(),
                context: None,
                allow_free_text,
            }),
            options,
        }
    }
}

/// The host of a Codex `NetworkApprovalContext`, with the protocol when
/// Codex gives one: `https://example.com`, or `example.com (socks5Tcp)`.
fn network_target(context: &Value) -> String {
    let host = context
        .get("host")
        .and_then(Value::as_str)
        .filter(|h| !h.trim().is_empty())
        .unwrap_or("a host");
    match context.get("protocol").and_then(Value::as_str) {
        Some(protocol @ ("http" | "https")) => format!("{protocol}://{host}"),
        Some(protocol) if !protocol.trim().is_empty() => format!("{host} ({protocol})"),
        _ => host.to_owned(),
    }
}

fn option(id: &str, label: &str, kind: RequestOptionKind) -> RequestOption {
    RequestOption {
        id: id.to_owned(),
        label: label.to_owned(),
        kind,
        description: None,
    }
}

/// The options of a command or file change approval. The IDs are the Codex
/// decisions.
pub fn decision_options() -> Vec<RequestOption> {
    vec![
        option("accept", "Allow once", RequestOptionKind::AllowOnce),
        option(
            "acceptForSession",
            "Allow for this session",
            RequestOptionKind::AllowAlways,
        ),
        option("decline", "Deny", RequestOptionKind::DenyOnce),
    ]
}
