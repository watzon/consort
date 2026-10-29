//! Maps ACP v1 `session/update` notifications and permission requests to
//! Consort session events.
//!
//! The mapper works on the JSON wire shape (camelCase fields, the
//! `sessionUpdate` tag), so tests can feed it recorded agent output. It keeps
//! the state that ACP spreads over many notifications: the text of the
//! current assistant message, and the latest state of each tool call.

use std::collections::HashMap;

use consort_proto::ids::{RequestId, SessionId, TurnId};
use consort_proto::model::{
    ApprovalDetails, AssistantMessagePayload, COMMAND_OUTPUT_TAIL_BYTES, CommandPayload,
    FileChange, FileChangeKind, FileChangePayload, ItemPayload, ItemStatus, PlanPayload, PlanStep,
    PlanStepStatus, ReasoningPayload, RequestOption, RequestOptionKind, ToolCallPayload, Usage,
    WebSearchPayload, tail_of,
};
use consort_proto::worker_link::{RequestBody, RequestOpened, SessionEvent, TurnEndReason};
use serde_json::Value;

use crate::ItemTracker;

/// Which kind of text block is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextKind {
    Message,
    Thought,
}

/// The state of one ACP session's mapping.
#[derive(Debug, Clone)]
pub struct AcpMapper {
    tracker: ItemTracker,
    open_text: Option<(TextKind, String, String)>,
    text_counter: u64,
    last_message_id: Option<String>,
    /// The latest kind, title and merged JSON of each tool call.
    tool_meta: HashMap<String, (String, String, Value)>,
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// The text of a content block, or `None` for a block with no text.
fn block_text(block: &Value) -> Option<&str> {
    match str_field(block, "type") {
        Some("text") => str_field(block, "text"),
        Some("resource_link") => str_field(block, "uri"),
        Some("resource") => block.get("resource").and_then(|r| str_field(r, "text")),
        _ => None,
    }
}

/// Splits an MCP tool name such as `mcp__hub__post_update` into the server
/// and the tool.
pub fn split_mcp_name(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    Some((server.to_owned(), tool.to_owned()))
}

fn tool_status(value: &Value) -> Option<ItemStatus> {
    match str_field(value, "status")? {
        "pending" | "in_progress" => Some(ItemStatus::InProgress),
        "completed" => Some(ItemStatus::Completed),
        "failed" => Some(ItemStatus::Failed),
        _ => None,
    }
}

/// The text output in a tool call's `content`, or its `rawOutput`.
fn tool_output(value: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(content) = value.get("content").and_then(Value::as_array) {
        for item in content {
            if str_field(item, "type") == Some("content")
                && let Some(text) = item.get("content").and_then(block_text)
            {
                parts.push(text.to_owned());
            }
        }
    }
    if parts.is_empty() {
        match value.get("rawOutput") {
            Some(Value::String(s)) => parts.push(s.clone()),
            Some(Value::Object(map)) => {
                for key in ["output", "stdout", "result", "content"] {
                    if let Some(Value::String(s)) = map.get(key) {
                        parts.push(s.clone());
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn count_lines(text: Option<&str>) -> u32 {
    text.map(|t| u32::try_from(t.lines().count()).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

/// File changes from a tool call's `diff` content and `locations`.
fn file_changes(value: &Value, kind: &str) -> Vec<FileChange> {
    let mut changes = Vec::new();
    if let Some(content) = value.get("content").and_then(Value::as_array) {
        for item in content {
            if str_field(item, "type") != Some("diff") {
                continue;
            }
            let Some(path) = str_field(item, "path") else {
                continue;
            };
            let old = str_field(item, "oldText");
            let new = str_field(item, "newText");
            let change = match (kind, old) {
                ("delete", _) => FileChangeKind::Delete,
                ("move", _) => FileChangeKind::Rename,
                (_, None) => FileChangeKind::Add,
                _ => FileChangeKind::Modify,
            };
            changes.push(FileChange {
                path: path.to_owned(),
                change,
                from_path: None,
                lines_added: count_lines(new),
                lines_removed: count_lines(old),
            });
        }
    }
    if changes.is_empty() {
        for path in tool_paths(value) {
            changes.push(FileChange {
                path,
                change: match kind {
                    "delete" => FileChangeKind::Delete,
                    "move" => FileChangeKind::Rename,
                    _ => FileChangeKind::Modify,
                },
                from_path: None,
                lines_added: 0,
                lines_removed: 0,
            });
        }
    }
    changes
}

/// The paths that a tool call touches.
fn tool_paths(value: &Value) -> Vec<String> {
    let mut paths: Vec<String> = value
        .get("locations")
        .and_then(Value::as_array)
        .map(|locs| {
            locs.iter()
                .filter_map(|l| str_field(l, "path").map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if paths.is_empty()
        && let Some(raw) = value.get("rawInput")
    {
        for key in ["file_path", "path", "filePath", "notebook_path"] {
            if let Some(p) = str_field(raw, key) {
                paths.push(p.to_owned());
                break;
            }
        }
    }
    paths
}

/// The command of an `execute` tool call.
fn tool_command(value: &Value, title: &str) -> String {
    value
        .get("rawInput")
        .and_then(|raw| match raw.get("command") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Array(parts)) => Some(
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .unwrap_or_else(|| title.to_owned())
}

/// The tool name of a tool call: the Claude tool name in `_meta` when there
/// is one, else the title.
fn tool_name(value: &Value, title: &str) -> String {
    value
        .get("_meta")
        .and_then(|m| m.get("claudeCode"))
        .and_then(|c| str_field(c, "toolName"))
        .unwrap_or(title)
        .to_owned()
}

fn tool_payload(value: &Value, kind: &str, title: &str) -> ItemPayload {
    let output = tool_output(value);
    match kind {
        "execute" => {
            let mut payload = CommandPayload {
                command: tool_command(value, title),
                cwd: value
                    .get("rawInput")
                    .and_then(|r| str_field(r, "cwd"))
                    .map(str::to_owned),
                ..CommandPayload::default()
            };
            if let Some(output) = output {
                payload.append_output(&output);
            }
            ItemPayload::Command(payload)
        }
        "edit" | "delete" | "move" => ItemPayload::FileChange(FileChangePayload {
            changes: file_changes(value, kind),
        }),
        "fetch" if value.get("rawInput").and_then(|r| r.get("query")).is_some() => {
            let raw = value.get("rawInput").cloned().unwrap_or(Value::Null);
            ItemPayload::WebSearch(WebSearchPayload {
                query: str_field(&raw, "query").unwrap_or(title).to_owned(),
                urls: raw
                    .get("url")
                    .and_then(Value::as_str)
                    .map(|u| vec![u.to_owned()])
                    .unwrap_or_default(),
            })
        }
        _ => {
            let name = tool_name(value, title);
            let (server, tool) = match split_mcp_name(&name) {
                Some((server, tool)) => (Some(server), tool),
                None => (None, name),
            };
            let output = output.map(|o| tail_of(&o, COMMAND_OUTPUT_TAIL_BYTES).0.to_owned());
            ItemPayload::ToolCall(ToolCallPayload {
                server,
                tool,
                arguments: value.get("rawInput").cloned().unwrap_or(Value::Null),
                output,
                is_error: false,
            })
        }
    }
}

fn plan_steps(update: &Value) -> Vec<PlanStep> {
    update
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|e| PlanStep {
                    text: str_field(e, "content").unwrap_or_default().to_owned(),
                    status: match str_field(e, "status") {
                        Some("in_progress") => PlanStepStatus::InProgress,
                        Some("completed") => PlanStepStatus::Completed,
                        _ => PlanStepStatus::Pending,
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Maps an ACP permission option kind to a Consort option kind.
pub fn option_kind(kind: &str) -> RequestOptionKind {
    match kind {
        "allow_once" => RequestOptionKind::AllowOnce,
        "allow_always" => RequestOptionKind::AllowAlways,
        "reject_once" => RequestOptionKind::DenyOnce,
        "reject_always" => RequestOptionKind::DenyAlways,
        _ => RequestOptionKind::Choice,
    }
}

/// The ACP stop reason of a `session/prompt` response, as a turn end
/// reason, with the error text for a turn that did not finish its work.
pub fn stop_reason(reason: Option<&str>) -> (TurnEndReason, Option<String>) {
    let failed = |text: &str| (TurnEndReason::Failed, Some(text.to_owned()));
    match reason {
        Some("end_turn") => (TurnEndReason::Completed, None),
        Some("cancelled") => (TurnEndReason::Interrupted, None),
        Some("refusal") => failed("The model refused to continue."),
        Some("max_tokens") => {
            failed("The model reached its output token limit before it finished the turn.")
        }
        Some("max_turn_requests") => {
            failed("The turn reached the agent's limit of model requests before it finished.")
        }
        Some(other) => (
            TurnEndReason::Failed,
            Some(format!(
                "The agent ended the turn with the stop reason `{other}`."
            )),
        ),
        None => failed("The agent ended the turn without a stop reason."),
    }
}

impl AcpMapper {
    /// A mapper for one session.
    pub fn new(session_id: SessionId) -> Self {
        Self {
            tracker: ItemTracker::new(session_id),
            open_text: None,
            text_counter: 0,
            last_message_id: None,
            tool_meta: HashMap::new(),
        }
    }

    /// Starts a new turn.
    pub fn start_turn(&mut self, turn_id: TurnId) {
        self.tracker.set_turn(turn_id);
    }

    /// The current turn.
    pub fn turn(&self) -> &TurnId {
        self.tracker.turn()
    }

    fn close_text(&mut self, events: &mut Vec<SessionEvent>) {
        if let Some((_, key, _)) = self.open_text.take()
            && let Some(event) = self.tracker.update(&key, |_, status| {
                *status = ItemStatus::Completed;
            })
        {
            events.push(event);
        }
    }

    fn push_text(
        &mut self,
        kind: TextKind,
        text: &str,
        message_id: Option<&str>,
        events: &mut Vec<SessionEvent>,
    ) {
        let same_message = message_id.is_none() || message_id == self.last_message_id.as_deref();
        let continues =
            matches!(&self.open_text, Some((open, _, _)) if *open == kind) && same_message;
        if !continues {
            self.close_text(events);
            self.text_counter += 1;
            let key = match kind {
                TextKind::Message => format!("message:{}", self.text_counter),
                TextKind::Thought => format!("thought:{}", self.text_counter),
            };
            self.open_text = Some((kind, key, String::new()));
        }
        if message_id.is_some() {
            self.last_message_id = message_id.map(str::to_owned);
        }
        let Some((_, key, buffer)) = &mut self.open_text else {
            return;
        };
        buffer.push_str(text);
        let payload = match kind {
            TextKind::Message => ItemPayload::AssistantMessage(AssistantMessagePayload {
                text: buffer.clone(),
            }),
            TextKind::Thought => ItemPayload::Reasoning(ReasoningPayload {
                text: buffer.clone(),
            }),
        };
        let key = key.clone();
        events.push(self.tracker.upsert(&key, payload, ItemStatus::InProgress));
    }

    /// Maps the `update` object of one `session/update` notification.
    pub fn map_update(&mut self, update: &Value) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        match str_field(update, "sessionUpdate") {
            Some("agent_message_chunk") => {
                if let Some(text) = update.get("content").and_then(block_text) {
                    self.push_text(
                        TextKind::Message,
                        text,
                        str_field(update, "messageId"),
                        &mut events,
                    );
                }
            }
            Some("agent_thought_chunk") => {
                if let Some(text) = update.get("content").and_then(block_text) {
                    self.push_text(TextKind::Thought, text, None, &mut events);
                }
            }
            Some("tool_call") => {
                self.close_text(&mut events);
                let Some(id) = str_field(update, "toolCallId") else {
                    return events;
                };
                let key = format!("tool:{id}");
                let kind = str_field(update, "kind").unwrap_or("other").to_owned();
                let title = str_field(update, "title").unwrap_or_default().to_owned();
                let payload = tool_payload(update, &kind, &title);
                let status = tool_status(update).unwrap_or(ItemStatus::InProgress);
                self.tool_meta
                    .insert(key.clone(), (kind, title, update.clone()));
                events.push(self.tracker.upsert(&key, payload, status));
            }
            Some("tool_call_update") => {
                let Some(id) = str_field(update, "toolCallId") else {
                    return events;
                };
                let key = format!("tool:{id}");
                let (kind, title, mut merged) =
                    self.tool_meta.get(&key).cloned().unwrap_or_else(|| {
                        (
                            "other".to_owned(),
                            String::new(),
                            Value::Object(Default::default()),
                        )
                    });
                if let (Value::Object(base), Value::Object(new)) = (&mut merged, update) {
                    for (k, v) in new {
                        if !v.is_null() {
                            base.insert(k.clone(), v.clone());
                        }
                    }
                }
                let kind = str_field(update, "kind").map(str::to_owned).unwrap_or(kind);
                let title = str_field(update, "title")
                    .map(str::to_owned)
                    .unwrap_or(title);
                let payload = tool_payload(&merged, &kind, &title);
                let status = tool_status(update)
                    .or_else(|| self.tracker.status(&key))
                    .unwrap_or(ItemStatus::InProgress);
                let payload = match (payload, status) {
                    (ItemPayload::ToolCall(mut p), ItemStatus::Failed) => {
                        p.is_error = true;
                        ItemPayload::ToolCall(p)
                    }
                    (p, _) => p,
                };
                self.tool_meta.insert(key.clone(), (kind, title, merged));
                if !self.tracker.contains(&key) {
                    self.close_text(&mut events);
                }
                events.push(self.tracker.upsert(&key, payload, status));
            }
            Some("plan") => {
                let key = format!("plan:{}", self.tracker.turn());
                let payload = ItemPayload::Plan(PlanPayload {
                    steps: plan_steps(update),
                });
                let done = matches!(&payload, ItemPayload::Plan(p) if !p.steps.is_empty() && p.steps.iter().all(|s| s.status == PlanStepStatus::Completed));
                let status = if done {
                    ItemStatus::Completed
                } else {
                    ItemStatus::InProgress
                };
                events.push(self.tracker.upsert(&key, payload, status));
            }
            Some("usage_update") => {
                let used = update.get("used").and_then(Value::as_u64).unwrap_or(0);
                let cost = update.get("cost").and_then(|c| {
                    let usd =
                        str_field(c, "currency").is_none_or(|cur| cur.eq_ignore_ascii_case("USD"));
                    if usd {
                        c.get("amount").and_then(Value::as_f64)
                    } else {
                        None
                    }
                });
                events.push(SessionEvent::Usage(Usage {
                    input_tokens: used,
                    output_tokens: 0,
                    cached_tokens: 0,
                    cost_usd: cost,
                }));
            }
            _ => {}
        }
        events
    }

    /// Closes the open items at the end of a turn and returns the
    /// `turn.ended` event last.
    pub fn end_turn(&mut self, reason: TurnEndReason, error: Option<String>) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        self.close_text(&mut events);
        let item_status = match reason {
            TurnEndReason::Failed => ItemStatus::Failed,
            _ => ItemStatus::Completed,
        };
        events.extend(self.tracker.finish_open(item_status));
        events.push(SessionEvent::TurnEnded {
            turn_id: self.tracker.turn().clone(),
            reason,
            error,
        });
        events
    }

    /// Maps the params of a `session/request_permission` request.
    pub fn map_permission(&self, request_id: RequestId, params: &Value) -> RequestOpened {
        let call = params.get("toolCall").cloned().unwrap_or(Value::Null);
        let key = str_field(&call, "toolCallId").map(|id| format!("tool:{id}"));
        let known = key.as_ref().and_then(|k| self.tool_meta.get(k));
        let mut merged = known
            .map(|(_, _, v)| v.clone())
            .unwrap_or(Value::Object(Default::default()));
        if let (Value::Object(base), Value::Object(new)) = (&mut merged, &call) {
            for (k, v) in new {
                if !v.is_null() {
                    base.insert(k.clone(), v.clone());
                }
            }
        }
        let kind = str_field(&merged, "kind").unwrap_or("other").to_owned();
        let title = str_field(&merged, "title")
            .map(str::to_owned)
            .unwrap_or_else(|| "Use a tool".to_owned());
        let details = match kind.as_str() {
            "execute" => ApprovalDetails::Command {
                command: tool_command(&merged, &title),
                cwd: merged
                    .get("rawInput")
                    .and_then(|r| str_field(r, "cwd"))
                    .map(str::to_owned),
            },
            "edit" | "delete" | "move" => {
                let mut paths: Vec<String> = file_changes(&merged, &kind)
                    .into_iter()
                    .map(|c| c.path)
                    .collect();
                paths.dedup();
                ApprovalDetails::FileChange { paths }
            }
            "fetch" => ApprovalDetails::Network {
                target: merged
                    .get("rawInput")
                    .and_then(|r| str_field(r, "url").or_else(|| str_field(r, "query")))
                    .unwrap_or(&title)
                    .to_owned(),
            },
            _ => {
                let name = tool_name(&merged, &title);
                let (server, tool) = match split_mcp_name(&name) {
                    Some((server, tool)) => (Some(server), tool),
                    None => (None, name),
                };
                ApprovalDetails::Tool {
                    server,
                    tool,
                    arguments: merged.get("rawInput").cloned().unwrap_or(Value::Null),
                }
            }
        };
        let options = params
            .get("options")
            .and_then(Value::as_array)
            .map(|opts| {
                opts.iter()
                    .filter_map(|o| {
                        Some(RequestOption {
                            id: str_field(o, "optionId")?.to_owned(),
                            label: str_field(o, "name").unwrap_or("Choose").to_owned(),
                            kind: option_kind(str_field(o, "kind").unwrap_or_default()),
                            description: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        RequestOpened {
            request_id,
            summary: title,
            body: RequestBody::Approval(details),
            options,
        }
    }
}
