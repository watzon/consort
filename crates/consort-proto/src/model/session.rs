//! Sessions and session items.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::common::ModelOptions;
use crate::ids::{AccountId, ChannelId, ItemId, NativeRef, SessionId, TaskId, TurnId, WorkerId};
use crate::time::Timestamp;

/// What a session is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRole {
    /// The session that speaks for the assistant in one channel.
    Coordinator,
    /// The session of one task.
    Task,
}

/// The life of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The worker is preparing the workspace or opening the harness session.
    Starting,
    /// A turn runs.
    Active,
    /// The session waits for input.
    Idle,
    /// The session stopped.
    Ended,
    /// The session cannot continue or resume.
    Lost,
}

impl SessionState {
    /// Whether the session can never run again.
    pub fn is_final(self) -> bool {
        matches!(self, Self::Ended | Self::Lost)
    }
}

/// Token totals for a session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cached input tokens.
    pub cached_tokens: u64,
    /// Cost in US dollars, when the harness reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// One conversation with a harness on one worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The ID.
    pub id: SessionId,
    /// Coordinator or task.
    pub role: SessionRole,
    /// The channel.
    pub channel_id: ChannelId,
    /// The task. Set when `role` is `task`.
    pub task_id: Option<TaskId>,
    /// The worker. A session never moves to another worker.
    pub worker_id: WorkerId,
    /// The harness account on that worker.
    pub account_id: AccountId,
    /// `claude`, `codex`, `pi`, `opencode` or `acp:<agent-id>`.
    pub harness: String,
    /// The model ID.
    pub model: String,
    /// The model options.
    #[serde(default)]
    pub options: ModelOptions,
    /// The workspace path on the worker.
    pub workspace_path: String,
    /// The harness's own session ID, used to resume.
    pub native_ref: Option<NativeRef>,
    /// The state.
    pub state: SessionState,
    /// The highest event sequence number that the hub stored.
    pub last_seq: u64,
    /// Token totals.
    #[serde(default)]
    pub usage: Usage,
    /// When the session started.
    pub started_at: Timestamp,
    /// When the session ended.
    pub ended_at: Option<Timestamp>,
    /// Why the session ended.
    pub end_reason: Option<String>,
}

/// The number of bytes of command output that a session item keeps. The full
/// output stays on the worker (see `item.detail`).
pub const COMMAND_OUTPUT_TAIL_BYTES: usize = 8 * 1024;

/// The last `max_bytes` bytes of `text`, cut at a character boundary, and
/// whether anything was cut.
pub fn tail_of(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (&text[start..], true)
}

/// The status of a session item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// The item is still changing.
    InProgress,
    /// The item finished.
    Completed,
    /// The item failed.
    Failed,
}

/// The `kind` of a session item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// Text that the assistant wrote.
    AssistantMessage,
    /// Reasoning text.
    Reasoning,
    /// A shell command.
    Command,
    /// File edits.
    FileChange,
    /// An MCP or harness tool call.
    ToolCall,
    /// A plan with steps.
    Plan,
    /// A subagent run.
    Subagent,
    /// A web search.
    WebSearch,
    /// An error that the harness reported.
    Error,
}

/// The payload of an `assistant_message` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMessagePayload {
    /// Markdown text.
    pub text: String,
}

/// The payload of a `reasoning` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningPayload {
    /// The reasoning text or its summary.
    pub text: String,
}

/// The payload of a `command` item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandPayload {
    /// The command line.
    pub command: String,
    /// The folder it runs in.
    pub cwd: Option<String>,
    /// The last [`COMMAND_OUTPUT_TAIL_BYTES`] of output.
    #[serde(default)]
    pub output: String,
    /// True when `output` is only the tail.
    #[serde(default)]
    pub output_truncated: bool,
    /// The exit code, when the command finished.
    pub exit_code: Option<i32>,
    /// How long it ran, in milliseconds.
    pub duration_ms: Option<u64>,
}

impl CommandPayload {
    /// Appends an `output.delta` and keeps only the tail.
    pub fn append_output(&mut self, delta: &str) {
        self.output.push_str(delta);
        let (tail, cut) = tail_of(&self.output, COMMAND_OUTPUT_TAIL_BYTES);
        if cut {
            self.output = tail.to_owned();
            self.output_truncated = true;
        }
    }
}

/// How a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    /// A new file.
    Add,
    /// A changed file.
    Modify,
    /// A removed file.
    Delete,
    /// A moved file. `from_path` has the old path.
    Rename,
}

/// One changed file. The diff stays on the worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// The path, relative to the workspace when possible.
    pub path: String,
    /// How it changed.
    pub change: FileChangeKind,
    /// The old path of a renamed file.
    pub from_path: Option<String>,
    /// Lines added.
    #[serde(default)]
    pub lines_added: u32,
    /// Lines removed.
    #[serde(default)]
    pub lines_removed: u32,
}

/// The payload of a `file_change` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChangePayload {
    /// The changed files.
    pub changes: Vec<FileChange>,
}

/// The payload of a `tool_call` item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallPayload {
    /// The MCP server name, when the tool comes from one. `hub` for hub tools.
    pub server: Option<String>,
    /// The tool name.
    pub tool: String,
    /// The arguments.
    #[serde(default)]
    pub arguments: Value,
    /// The result text, shortened like command output.
    pub output: Option<String>,
    /// Whether the tool reported an error.
    #[serde(default)]
    pub is_error: bool,
}

/// The status of one plan step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    /// Not started.
    Pending,
    /// Running.
    InProgress,
    /// Done.
    Completed,
}

/// One plan step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    /// The step text.
    pub text: String,
    /// The step status.
    pub status: PlanStepStatus,
}

/// The payload of a `plan` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanPayload {
    /// The steps, in order.
    pub steps: Vec<PlanStep>,
}

/// The payload of a `subagent` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentPayload {
    /// The subagent's name or type, when the harness reports it.
    pub agent: Option<String>,
    /// What the subagent was asked to do.
    pub description: String,
    /// The subagent's final text.
    pub result: Option<String>,
}

/// The payload of a `web_search` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchPayload {
    /// The query.
    pub query: String,
    /// The result URLs, when the harness reports them.
    #[serde(default)]
    pub urls: Vec<String>,
}

/// The payload of an `error` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorPayload {
    /// The error text.
    pub message: String,
    /// The harness's error code, when it has one.
    pub code: Option<String>,
}

/// The `kind` and `payload` of a session item. It serializes as two fields,
/// `"kind": "command", "payload": {...}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum ItemPayload {
    /// Text that the assistant wrote.
    AssistantMessage(AssistantMessagePayload),
    /// Reasoning text.
    Reasoning(ReasoningPayload),
    /// A shell command.
    Command(CommandPayload),
    /// File edits.
    FileChange(FileChangePayload),
    /// A tool call.
    ToolCall(ToolCallPayload),
    /// A plan.
    Plan(PlanPayload),
    /// A subagent run.
    Subagent(SubagentPayload),
    /// A web search.
    WebSearch(WebSearchPayload),
    /// An error.
    Error(ErrorPayload),
}

impl ItemPayload {
    /// The item kind.
    pub fn kind(&self) -> ItemKind {
        match self {
            Self::AssistantMessage(_) => ItemKind::AssistantMessage,
            Self::Reasoning(_) => ItemKind::Reasoning,
            Self::Command(_) => ItemKind::Command,
            Self::FileChange(_) => ItemKind::FileChange,
            Self::ToolCall(_) => ItemKind::ToolCall,
            Self::Plan(_) => ItemKind::Plan,
            Self::Subagent(_) => ItemKind::Subagent,
            Self::WebSearch(_) => ItemKind::WebSearch,
            Self::Error(_) => ItemKind::Error,
        }
    }
}

/// A normalized record of one thing a session did. An update with the same
/// `id` replaces the earlier copy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionItem {
    /// The ID, unique per session.
    pub id: ItemId,
    /// The session.
    pub session_id: SessionId,
    /// The turn.
    pub turn_id: TurnId,
    /// `kind` and `payload`.
    #[serde(flatten)]
    pub payload: ItemPayload,
    /// The status.
    pub status: ItemStatus,
    /// When the item started.
    pub created_at: Timestamp,
    /// When the item last changed.
    pub updated_at: Timestamp,
}

impl SessionItem {
    /// The item kind.
    pub fn kind(&self) -> ItemKind {
        self.payload.kind()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn item_shape_has_kind_and_payload() {
        let item = SessionItem {
            id: "itm_00000000-0000-0000-0000-000000000001".parse().unwrap(),
            session_id: "ses_00000000-0000-0000-0000-000000000002".parse().unwrap(),
            turn_id: TurnId::new("t1"),
            payload: ItemPayload::Command(CommandPayload {
                command: "cargo test".into(),
                cwd: Some("/w".into()),
                output: "ok".into(),
                output_truncated: false,
                exit_code: Some(0),
                duration_ms: Some(1200),
            }),
            status: ItemStatus::Completed,
            created_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(2),
        };
        let value = serde_json::to_value(&item).unwrap();
        assert_eq!(
            value,
            json!({
                "id": "itm_00000000-0000-0000-0000-000000000001",
                "session_id": "ses_00000000-0000-0000-0000-000000000002",
                "turn_id": "t1",
                "kind": "command",
                "payload": {
                    "command": "cargo test", "cwd": "/w", "output": "ok",
                    "output_truncated": false, "exit_code": 0, "duration_ms": 1200
                },
                "status": "completed",
                "created_at": 1,
                "updated_at": 2
            })
        );
        let back: SessionItem = serde_json::from_value(value).unwrap();
        assert_eq!(back, item);
        assert_eq!(back.kind(), ItemKind::Command);
    }

    #[test]
    fn every_item_kind_round_trips() {
        let payloads = vec![
            ItemPayload::AssistantMessage(AssistantMessagePayload { text: "hi".into() }),
            ItemPayload::Reasoning(ReasoningPayload { text: "hmm".into() }),
            ItemPayload::Command(CommandPayload::default()),
            ItemPayload::FileChange(FileChangePayload {
                changes: vec![FileChange {
                    path: "src/a.rs".into(),
                    change: FileChangeKind::Rename,
                    from_path: Some("src/b.rs".into()),
                    lines_added: 3,
                    lines_removed: 1,
                }],
            }),
            ItemPayload::ToolCall(ToolCallPayload {
                server: Some("hub".into()),
                tool: "post_update".into(),
                arguments: json!({"body": "x"}),
                output: None,
                is_error: false,
            }),
            ItemPayload::Plan(PlanPayload {
                steps: vec![PlanStep {
                    text: "Read code".into(),
                    status: PlanStepStatus::InProgress,
                }],
            }),
            ItemPayload::Subagent(SubagentPayload {
                agent: None,
                description: "Search".into(),
                result: None,
            }),
            ItemPayload::WebSearch(WebSearchPayload {
                query: "gpui".into(),
                urls: vec![],
            }),
            ItemPayload::Error(ErrorPayload {
                message: "boom".into(),
                code: None,
            }),
        ];
        let kinds = [
            "assistant_message",
            "reasoning",
            "command",
            "file_change",
            "tool_call",
            "plan",
            "subagent",
            "web_search",
            "error",
        ];
        for (payload, kind) in payloads.into_iter().zip(kinds) {
            let value = serde_json::to_value(&payload).unwrap();
            assert_eq!(value["kind"], kind);
            assert_eq!(serde_json::to_value(payload.kind()).unwrap(), json!(kind));
            let back: ItemPayload = serde_json::from_value(value).unwrap();
            assert_eq!(back, payload);
        }
    }

    #[test]
    fn command_output_keeps_the_tail() {
        let mut cmd = CommandPayload::default();
        cmd.append_output(&"a".repeat(COMMAND_OUTPUT_TAIL_BYTES));
        assert!(!cmd.output_truncated);
        cmd.append_output("é end");
        assert!(cmd.output_truncated);
        assert!(cmd.output.len() <= COMMAND_OUTPUT_TAIL_BYTES);
        assert!(cmd.output.ends_with("é end"));
    }

    #[test]
    fn tail_respects_char_boundaries() {
        let text = "ééé";
        let (tail, cut) = tail_of(text, 3);
        assert!(cut);
        assert_eq!(tail, "é");
        assert_eq!(tail_of("abc", 10), ("abc", false));
    }

    #[test]
    fn usage_omits_unknown_cost() {
        let usage = Usage {
            input_tokens: 1,
            output_tokens: 2,
            cached_tokens: 3,
            cost_usd: None,
        };
        assert_eq!(
            serde_json::to_value(&usage).unwrap(),
            json!({"input_tokens": 1, "output_tokens": 2, "cached_tokens": 3})
        );
    }
}
