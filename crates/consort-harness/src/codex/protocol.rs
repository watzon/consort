//! The subset of the `codex app-server` protocol that the adapter uses.
//!
//! Written by hand from the JSON Schema of codex-cli 0.165.0 in
//! `crates/consort-harness/schema/codex/` (`ClientRequest.json`,
//! `ServerNotification.json`, `ServerRequest.json` and the v2 schema
//! bundle). Fields are camelCase on the wire. Readers ignore unknown fields,
//! and unknown item types read as [`ThreadItem::Other`], so a newer Codex
//! that adds fields or items does not break the adapter.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Method names.
pub mod method {
    /// Client requests.
    pub const INITIALIZE: &str = "initialize";
    /// The notification after `initialize`.
    pub const INITIALIZED: &str = "initialized";
    /// Start a thread.
    pub const THREAD_START: &str = "thread/start";
    /// Resume a thread.
    pub const THREAD_RESUME: &str = "thread/resume";
    /// Start a turn.
    pub const TURN_START: &str = "turn/start";
    /// Add input to the running turn.
    pub const TURN_STEER: &str = "turn/steer";
    /// Stop the running turn.
    pub const TURN_INTERRUPT: &str = "turn/interrupt";
    /// Set the extra skill folders of the app-server (experimental).
    pub const SKILLS_EXTRA_ROOTS_SET: &str = "skills/extraRoots/set";
    /// Read the account.
    pub const ACCOUNT_READ: &str = "account/read";
    /// Start a sign-in.
    pub const ACCOUNT_LOGIN_START: &str = "account/login/start";
    /// Cancel a sign-in.
    pub const ACCOUNT_LOGIN_CANCEL: &str = "account/login/cancel";
    /// Sign out.
    pub const ACCOUNT_LOGOUT: &str = "account/logout";
    /// A sign-in ended (notification).
    pub const ACCOUNT_LOGIN_COMPLETED: &str = "account/login/completed";
    /// List models.
    pub const MODEL_LIST: &str = "model/list";

    /// Server notifications.
    pub const TURN_STARTED: &str = "turn/started";
    /// A turn completed.
    pub const TURN_COMPLETED: &str = "turn/completed";
    /// An item started.
    pub const ITEM_STARTED: &str = "item/started";
    /// An item completed.
    pub const ITEM_COMPLETED: &str = "item/completed";
    /// Assistant text.
    pub const AGENT_MESSAGE_DELTA: &str = "item/agentMessage/delta";
    /// Reasoning text.
    pub const REASONING_TEXT_DELTA: &str = "item/reasoning/textDelta";
    /// Reasoning summary text.
    pub const REASONING_SUMMARY_DELTA: &str = "item/reasoning/summaryTextDelta";
    /// Command output.
    pub const COMMAND_OUTPUT_DELTA: &str = "item/commandExecution/outputDelta";
    /// Token counts.
    pub const TOKEN_USAGE_UPDATED: &str = "thread/tokenUsage/updated";
    /// The turn plan.
    pub const TURN_PLAN_UPDATED: &str = "turn/plan/updated";
    /// An error.
    pub const ERROR: &str = "error";
    /// A server request was answered or withdrawn.
    pub const SERVER_REQUEST_RESOLVED: &str = "serverRequest/resolved";

    /// Server requests.
    pub const COMMAND_APPROVAL: &str = "item/commandExecution/requestApproval";
    /// File change approval.
    pub const FILE_CHANGE_APPROVAL: &str = "item/fileChange/requestApproval";
    /// Extra permissions.
    pub const PERMISSIONS_APPROVAL: &str = "item/permissions/requestApproval";
    /// A question to the user.
    pub const USER_INPUT: &str = "item/tool/requestUserInput";
    /// An MCP server elicitation.
    pub const MCP_ELICITATION: &str = "mcpServer/elicitation/request";
}

/// `initialize` params.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Who the client is.
    pub client_info: ClientInfo,
    /// Client capabilities.
    pub capabilities: InitializeCapabilities,
}

/// The client's name and version.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    /// A short name.
    pub name: String,
    /// A display title.
    pub title: String,
    /// The version.
    pub version: String,
}

/// Client capabilities.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeCapabilities {
    /// Opt into experimental methods and fields.
    pub experimental_api: bool,
    /// Notifications that the client does not want.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub opt_out_notification_methods: Vec<String>,
}

/// `thread/start` params. `thread/resume` adds `threadId`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    /// The thread to resume, for `thread/resume` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// The model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The working folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// `untrusted`, `on-request` or `never`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    /// `read-only`, `workspace-write` or `danger-full-access`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    /// Config overrides, with the keys of `config.toml`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Map<String, Value>>,
    /// Text added to the developer instructions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
}

/// A thread.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// The thread ID.
    pub id: String,
}

/// The response to `thread/start` and `thread/resume`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartResponse {
    /// The thread.
    pub thread: Thread,
    /// The model the thread uses.
    #[serde(default)]
    pub model: String,
}

/// One input item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UserInput {
    /// Text.
    Text {
        /// The text.
        text: String,
        /// Rich text spans. Always empty here.
        text_elements: Vec<Value>,
    },
    /// An image file that Codex reads itself (`localImage`). The path is
    /// absolute.
    LocalImage {
        /// The image file.
        path: String,
    },
}

impl UserInput {
    /// A text input.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            text_elements: Vec::new(),
        }
    }
}

/// `turn/start` params.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    /// The thread.
    pub thread_id: String,
    /// The input.
    pub input: Vec<UserInput>,
    /// The reasoning effort, such as `high`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// A new model for this turn and the next ones. Sent only when it
    /// changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The approval policy for this turn and the next ones, such as
    /// `untrusted`. Sent only when it changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    /// The sandbox policy for this turn and the next ones, such as
    /// `{"type": "readOnly"}`. Sent only when it changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_policy: Option<Value>,
}

/// `turn/steer` params.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerParams {
    /// The thread.
    pub thread_id: String,
    /// The input.
    pub input: Vec<UserInput>,
    /// The turn that must still run.
    pub expected_turn_id: String,
}

/// `turn/interrupt` params.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    /// The thread.
    pub thread_id: String,
    /// The turn.
    pub turn_id: String,
}

/// The status of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    /// It finished.
    Completed,
    /// It was interrupted.
    Interrupted,
    /// It failed.
    Failed,
    /// It runs.
    InProgress,
}

/// A turn error.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnError {
    /// The message.
    pub message: String,
    /// More detail.
    #[serde(default)]
    pub additional_details: Option<String>,
    /// The kind of error. Codex sends `"unauthorized"` as a string when the
    /// login is gone or expired. Other kinds can be objects.
    #[serde(default)]
    pub codex_error_info: Option<Value>,
}

impl TurnError {
    /// Whether Codex says that the login is gone or expired.
    pub fn is_unauthorized(&self) -> bool {
        self.codex_error_info.as_ref().and_then(Value::as_str) == Some("unauthorized")
    }

    /// The kind of the failure. `codexErrorInfo` is structured data, so it
    /// decides first: `usageLimitExceeded` and `rateLimitExceeded` are a
    /// rate limit, and `unauthorized` is an expired sign-in. Without a
    /// known value, the text decides (see [`crate::classify_failure`]).
    pub fn failure(&self) -> crate::FailureInfo {
        use crate::TurnErrorKind;
        let by_text = || {
            let text = match &self.additional_details {
                Some(more) => format!("{} {more}", self.message),
                None => self.message.clone(),
            };
            crate::classify_failure(&text)
        };
        match self.codex_error_info.as_ref().and_then(Value::as_str) {
            Some("usageLimitExceeded" | "rateLimitExceeded") => crate::FailureInfo {
                kind: TurnErrorKind::RateLimit,
                resets_at: by_text().resets_at,
            },
            Some("unauthorized") => crate::FailureInfo {
                kind: TurnErrorKind::ExpiredSignIn,
                resets_at: None,
            },
            _ => by_text(),
        }
    }
}

/// A turn.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    /// The turn ID.
    pub id: String,
    /// The status.
    pub status: TurnStatus,
    /// The error, when it failed.
    #[serde(default)]
    pub error: Option<TurnError>,
}

/// The response to `turn/start`.
#[derive(Debug, Clone, Deserialize)]
pub struct TurnStartResponse {
    /// The new turn.
    pub turn: Turn,
}

/// `turn/started` and `turn/completed`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnNotification {
    /// The thread.
    pub thread_id: String,
    /// The turn.
    pub turn: Turn,
}

/// The status of a command or file change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecStatus {
    /// It runs.
    InProgress,
    /// It finished.
    Completed,
    /// It failed.
    Failed,
    /// The user declined it.
    Declined,
}

/// How a file changes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PatchChangeKind {
    /// A new file.
    Add,
    /// A deleted file.
    Delete,
    /// A changed or moved file.
    Update {
        /// The new path of a moved file.
        #[serde(default)]
        move_path: Option<String>,
    },
}

/// One file in a file change.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileUpdateChange {
    /// The path.
    pub path: String,
    /// How it changes.
    pub kind: PatchChangeKind,
    /// The unified diff.
    #[serde(default)]
    pub diff: String,
}

/// One thread item.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadItem {
    /// The user's input.
    UserMessage {
        /// The item ID.
        id: String,
    },
    /// Assistant text.
    AgentMessage {
        /// The item ID.
        id: String,
        /// The text.
        #[serde(default)]
        text: String,
    },
    /// A plan.
    Plan {
        /// The item ID.
        id: String,
        /// The plan text.
        #[serde(default)]
        text: String,
    },
    /// Reasoning.
    Reasoning {
        /// The item ID.
        id: String,
        /// Summary parts.
        #[serde(default)]
        summary: Vec<String>,
        /// Raw reasoning parts.
        #[serde(default)]
        content: Vec<String>,
    },
    /// A shell command.
    #[serde(rename_all = "camelCase")]
    CommandExecution {
        /// The item ID.
        id: String,
        /// The command line.
        command: String,
        /// The folder.
        #[serde(default)]
        cwd: Option<String>,
        /// All output so far.
        #[serde(default)]
        aggregated_output: Option<String>,
        /// The exit code.
        #[serde(default)]
        exit_code: Option<i32>,
        /// How long it ran.
        #[serde(default)]
        duration_ms: Option<i64>,
        /// The status.
        status: ExecStatus,
    },
    /// File edits.
    FileChange {
        /// The item ID.
        id: String,
        /// The files.
        #[serde(default)]
        changes: Vec<FileUpdateChange>,
        /// The status.
        status: ExecStatus,
    },
    /// An MCP tool call.
    McpToolCall {
        /// The item ID.
        id: String,
        /// The MCP server.
        server: String,
        /// The tool.
        tool: String,
        /// The arguments.
        #[serde(default)]
        arguments: Value,
        /// The result.
        #[serde(default)]
        result: Option<Value>,
        /// The error.
        #[serde(default)]
        error: Option<Value>,
        /// `inProgress`, `completed` or `failed`.
        status: String,
    },
    /// A web search.
    WebSearch {
        /// The item ID.
        id: String,
        /// The query.
        #[serde(default)]
        query: String,
    },
    /// An item type that the adapter does not map.
    #[serde(other)]
    Other,
}

/// `item/started` and `item/completed`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemNotification {
    /// The thread.
    pub thread_id: String,
    /// The turn.
    pub turn_id: String,
    /// The item.
    pub item: ThreadItem,
}

/// A text delta for one item.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaNotification {
    /// The turn.
    #[serde(default)]
    pub turn_id: String,
    /// The item.
    pub item_id: String,
    /// The new text.
    pub delta: String,
}

/// Token counts.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageBreakdown {
    /// Input tokens.
    #[serde(default)]
    pub input_tokens: u64,
    /// Input tokens from the cache.
    #[serde(default)]
    pub cached_input_tokens: u64,
    /// Output tokens.
    #[serde(default)]
    pub output_tokens: u64,
}

/// The token counts of a thread.
#[derive(Debug, Clone, Deserialize)]
pub struct ThreadTokenUsage {
    /// The totals.
    pub total: TokenUsageBreakdown,
}

/// `thread/tokenUsage/updated`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageNotification {
    /// The usage.
    pub token_usage: ThreadTokenUsage,
}

/// One step of a turn plan.
#[derive(Debug, Clone, Deserialize)]
pub struct TurnPlanStep {
    /// The step text.
    pub step: String,
    /// `pending`, `inProgress` or `completed`.
    pub status: String,
}

/// `turn/plan/updated`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlanNotification {
    /// The turn.
    pub turn_id: String,
    /// The steps.
    #[serde(default)]
    pub plan: Vec<TurnPlanStep>,
}

/// `error`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorNotification {
    /// The error.
    pub error: TurnError,
    /// Whether Codex retries by itself.
    #[serde(default)]
    pub will_retry: bool,
}

/// `item/commandExecution/requestApproval` params.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandApprovalParams {
    /// The turn.
    pub turn_id: String,
    /// The command item.
    pub item_id: String,
    /// The command line.
    #[serde(default)]
    pub command: Option<String>,
    /// The folder.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Why Codex asks.
    #[serde(default)]
    pub reason: Option<String>,
    /// Set when Codex asks for network access.
    #[serde(default)]
    pub network_approval_context: Option<Value>,
}

/// `item/fileChange/requestApproval` params.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeApprovalParams {
    /// The turn.
    pub turn_id: String,
    /// The file change item.
    pub item_id: String,
    /// Why Codex asks.
    #[serde(default)]
    pub reason: Option<String>,
    /// A folder that Codex asks to write to for the rest of the session.
    #[serde(default)]
    pub grant_root: Option<String>,
}

/// `item/permissions/requestApproval` params.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsApprovalParams {
    /// The turn.
    pub turn_id: String,
    /// Why Codex asks.
    #[serde(default)]
    pub reason: Option<String>,
    /// The requested permissions, returned as is on approval.
    #[serde(default)]
    pub permissions: Value,
}

/// One option of a user input question.
#[derive(Debug, Clone, Deserialize)]
pub struct UserInputOption {
    /// The label.
    pub label: String,
    /// What it means.
    #[serde(default)]
    pub description: Option<String>,
}

/// One question of `item/tool/requestUserInput`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputQuestion {
    /// The question ID, the key of the answer.
    pub id: String,
    /// A short header.
    #[serde(default)]
    pub header: String,
    /// The question.
    pub question: String,
    /// Whether free text is allowed.
    #[serde(default)]
    pub is_other: bool,
    /// The options.
    #[serde(default)]
    pub options: Option<Vec<UserInputOption>>,
}

/// `item/tool/requestUserInput` params.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputParams {
    /// The turn.
    pub turn_id: String,
    /// The questions.
    pub questions: Vec<UserInputQuestion>,
}

/// The answer to one question.
#[derive(Debug, Clone, Serialize)]
pub struct UserInputAnswer {
    /// The chosen labels or the free text.
    pub answers: Vec<String>,
}

/// The response to `item/tool/requestUserInput`.
#[derive(Debug, Clone, Serialize)]
pub struct UserInputResponse {
    /// Answers by question ID.
    pub answers: BTreeMap<String, UserInputAnswer>,
}

/// The response to a command or file change approval: `accept`,
/// `acceptForSession`, `decline` or `cancel`.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalResponse {
    /// The decision.
    pub decision: String,
}

/// The response to `item/permissions/requestApproval`.
#[derive(Debug, Clone, Serialize)]
pub struct PermissionsResponse {
    /// The granted permissions. Empty means none.
    pub permissions: Value,
    /// `turn` or `session`.
    pub scope: String,
}

/// `account/read` params.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAccountParams {
    /// Whether Codex refreshes its token first.
    pub refresh_token: bool,
}

/// The signed-in account.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Account {
    /// An API key.
    ApiKey,
    /// A ChatGPT login.
    #[serde(rename_all = "camelCase")]
    Chatgpt {
        /// The signed-in email address.
        #[serde(default)]
        email: Option<String>,
        /// The plan, such as `plus`.
        #[serde(default)]
        plan_type: Option<String>,
    },
    /// Amazon Bedrock.
    AmazonBedrock,
    /// Google Cloud.
    Gcp,
    /// An account type that the adapter does not know.
    #[serde(other)]
    Other,
}

/// The response to `account/read`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAccountResponse {
    /// The account, or `None` when nobody is signed in.
    #[serde(default)]
    pub account: Option<Account>,
    /// Whether the model provider needs an OpenAI login.
    #[serde(default)]
    pub requires_openai_auth: bool,
}

/// `account/login/start` params. Only the kinds that Consort uses.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LoginAccountParams {
    /// Store an API key in the Codex home.
    #[serde(rename_all = "camelCase")]
    ApiKey {
        /// The key.
        api_key: String,
    },
    /// ChatGPT in a browser, with a callback on this machine.
    Chatgpt,
    /// ChatGPT with a device code.
    ChatgptDeviceCode,
}

impl std::fmt::Debug for LoginAccountParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey { .. } => f.write_str("ApiKey { api_key: <redacted> }"),
            Self::Chatgpt => f.write_str("Chatgpt"),
            Self::ChatgptDeviceCode => f.write_str("ChatgptDeviceCode"),
        }
    }
}

/// The response to `account/login/start`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LoginAccountResponse {
    /// The key is stored.
    ApiKey,
    /// Open `auth_url` in a browser on this machine.
    #[serde(rename_all = "camelCase")]
    Chatgpt {
        /// The sign-in ID.
        login_id: String,
        /// The page to open.
        auth_url: String,
    },
    /// Open `verification_url` and enter `user_code`.
    #[serde(rename_all = "camelCase")]
    ChatgptDeviceCode {
        /// The sign-in ID.
        login_id: String,
        /// The page to open.
        verification_url: String,
        /// The code to enter on the page.
        user_code: String,
    },
    /// A kind that the adapter does not use.
    #[serde(other)]
    Other,
}

/// `account/login/cancel` params.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelLoginAccountParams {
    /// The sign-in ID.
    pub login_id: String,
}

/// The `account/login/completed` notification.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLoginCompleted {
    /// The sign-in ID.
    #[serde(default)]
    pub login_id: Option<String>,
    /// Whether the sign-in worked.
    pub success: bool,
    /// Why it failed.
    #[serde(default)]
    pub error: Option<String>,
}

/// `model/list` params.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListParams {
    /// The page cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// One reasoning effort.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffortOption {
    /// The effort, such as `high`.
    pub reasoning_effort: String,
}

/// One model.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    /// The model ID.
    pub id: String,
    /// The display name.
    #[serde(default)]
    pub display_name: String,
    /// Whether the model is hidden.
    #[serde(default)]
    pub hidden: bool,
    /// The default effort.
    #[serde(default)]
    pub default_reasoning_effort: Option<String>,
    /// The efforts it supports.
    #[serde(default)]
    pub supported_reasoning_efforts: Vec<ReasoningEffortOption>,
}

/// The response to `model/list`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListResponse {
    /// The models.
    pub data: Vec<Model>,
    /// The next page cursor.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn items_read_with_unknown_fields_and_types() {
        let item: ThreadItem = serde_json::from_value(json!({
            "type": "commandExecution", "id": "c1", "command": "ls", "cwd": "/w",
            "commandActions": [], "status": "inProgress", "newField": 1
        }))
        .unwrap();
        assert!(matches!(
            item,
            ThreadItem::CommandExecution {
                status: ExecStatus::InProgress,
                ..
            }
        ));
        let item: ThreadItem =
            serde_json::from_value(json!({"type": "imageGeneration", "id": "x"})).unwrap();
        assert!(matches!(item, ThreadItem::Other));
        let change: FileUpdateChange = serde_json::from_value(
            json!({"path": "a.rs", "kind": {"type": "update", "move_path": null}, "diff": "@@"}),
        )
        .unwrap();
        assert_eq!(change.kind, PatchChangeKind::Update { move_path: None });
    }

    #[test]
    fn requests_serialize_as_camel_case() {
        let params = ThreadStartParams {
            model: Some("gpt-5.5".into()),
            approval_policy: Some("never".into()),
            developer_instructions: Some("Be brief.".into()),
            ..ThreadStartParams::default()
        };
        assert_eq!(
            serde_json::to_value(&params).unwrap(),
            json!({"model": "gpt-5.5", "approvalPolicy": "never", "developerInstructions": "Be brief."})
        );
        let turn = TurnStartParams {
            thread_id: "t".into(),
            input: vec![UserInput::text("hi")],
            effort: None,
            model: None,
            approval_policy: None,
            sandbox_policy: None,
        };
        assert_eq!(
            serde_json::to_value(&turn).unwrap(),
            json!({"threadId": "t", "input": [{"type": "text", "text": "hi", "text_elements": []}]})
        );
        let account: GetAccountResponse = serde_json::from_value(
            json!({"account": {"type": "chatgpt", "email": "a@b", "planType": "plus"}, "requiresOpenaiAuth": true}),
        )
        .unwrap();
        assert!(
            matches!(account.account, Some(Account::Chatgpt { plan_type: Some(p), .. }) if p == "plus")
        );
    }
}
