//! Hub tools: the tools that the hub gives every session.
//!
//! The worker serves these tools over MCP. It gets the list with `tool.list`
//! and forwards each call as `tool.call`. The hub runs the logic.
//!
//! - [`ToolAudience`] says which kind of session sees a tool.
//! - [`tools_for`] gives the tool list with JSON Schemas, for `tool.list`.
//! - [`HubToolCall::parse_for`] turns a `tool.call` into typed arguments and
//!   checks that the session may call the tool.
//! - The `*Result` structs are the result shapes. Send them with
//!   [`ToolCallResult::json`].

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{ErrorBody, HttpErrorBody};
use crate::ids::{ChannelId, DecisionId, ItemId, MessageId, RepoId, RoutineId, TaskId, WorkerId};
use crate::model::{
    ApprovalMode, AuthState, Channel, ChannelFile, ChannelKind, CheckoutInfo, DecisionOption,
    ExpectedHash, Isolation, MemoryNoteSummary, MemoryScope, Message, ModelSelection, Os,
    PlacementRequire, Routine, RoutineAction, RoutineSchedule, SessionItem, SessionRole,
    Sha256Hash, Suggestion, Task, Worker, WorkerState,
};
use crate::status::TaskStatus;
use crate::time::Timestamp;

mod connectors;
mod management;
mod proactivity;
mod voice;

pub use connectors::{
    CONNECTOR_APPROVAL_WAIT_MS, CONNECTOR_RESULT_MAX_BYTES, CONNECTOR_TOOLS_LIST_MAX,
    ConnectorToolInfo, GetConnectorToolArgs, GetConnectorToolResult, ListConnectorToolsArgs,
    ListConnectorToolsResult, ListSkillsResult, ReadSkillArgs, ReadSkillResult,
    RunConnectorToolArgs, RunConnectorToolResult,
};
pub use management::{
    AddAccountArgs, AddSkillArgs, ArchiveChannelArgs, ChangeStatus, ConnectServiceArgs,
    CreateChannelArgs, DisconnectArgs, InstallToolArgs, ListConnectionsResult,
    ListInstalledSkillsResult, ManagementResult, RemoveSkillArgs, RequestAccessArgs,
    SearchConnectorCatalogArgs, SearchConnectorCatalogResult, SearchSkillsArgs, SearchSkillsResult,
    SetAccountExtrasArgs, SetChannelConnectorArgs, SetChannelSkillArgs, SetChannelTagsArgs,
    SetDefaultModelArgs, SetWorkerTagsArgs, StartSignInArgs, UpdateChannelArgs,
    UpdateConnectionArgs, UpdateSkillArgs, UpdateThisChannelArgs,
};
pub use proactivity::{
    CancelFollowUpArgs, FollowUpLaterArgs, FollowUpResult, NoReplyNeededArgs, SetProactivityArgs,
    SetProactivityResult,
};
pub use voice::{ProposeVoiceRuleArgs, VoiceGuideArgs, VoiceGuideResult, VoiceSampleArgs};

/// The kind of session that a tool is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAudience {
    /// The coordinator of the Tutti channel.
    #[serde(alias = "everything_coordinator")]
    TuttiCoordinator,
    /// The coordinator of any other channel.
    ChannelCoordinator,
    /// A task session, in any channel.
    TaskSession,
}

impl ToolAudience {
    /// Every audience.
    pub const ALL: [ToolAudience; 3] = [
        Self::TuttiCoordinator,
        Self::ChannelCoordinator,
        Self::TaskSession,
    ];

    /// The audience of a session with this role in a channel of this kind.
    pub fn for_session(role: SessionRole, channel_kind: ChannelKind) -> Self {
        match (role, channel_kind) {
            (SessionRole::Task, _) => Self::TaskSession,
            (SessionRole::Coordinator, ChannelKind::Tutti) => Self::TuttiCoordinator,
            (SessionRole::Coordinator, ChannelKind::Standard) => Self::ChannelCoordinator,
        }
    }
}

const EVERY_SESSION: &[ToolAudience] = &ToolAudience::ALL;
const COORDINATORS: &[ToolAudience] = &[
    ToolAudience::TuttiCoordinator,
    ToolAudience::ChannelCoordinator,
];
const TUTTI_ONLY: &[ToolAudience] = &[ToolAudience::TuttiCoordinator];
const CHANNEL_COORDINATOR_AND_TASK: &[ToolAudience] =
    &[ToolAudience::ChannelCoordinator, ToolAudience::TaskSession];
const TASK_ONLY: &[ToolAudience] = &[ToolAudience::TaskSession];
const CHANNEL_COORDINATOR_ONLY: &[ToolAudience] = &[ToolAudience::ChannelCoordinator];

/// One tool in `tool.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The tool name.
    pub name: String,
    /// What the tool does, for the model.
    pub description: String,
    /// The JSON Schema of the arguments. Always an object schema with no
    /// `$ref`.
    pub input_schema: Value,
}

/// One block of tool result content, in the MCP shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolContent {
    /// Text.
    Text {
        /// The text.
        text: String,
    },
}

/// The response to `tool.call`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// The content for the model.
    pub content: Vec<ToolContent>,
    /// Whether the tool failed. The model sees the content either way.
    #[serde(default)]
    pub is_error: bool,
    /// The same result as JSON, for MCP `structuredContent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
}

impl ToolCallResult {
    /// A result whose content is the JSON of `value`.
    pub fn json<T: Serialize>(value: &T) -> Self {
        match serde_json::to_value(value) {
            Ok(value) => Self {
                content: vec![ToolContent::Text {
                    text: value.to_string(),
                }],
                is_error: false,
                structured_content: Some(value),
            },
            Err(_) => Self::error(&ErrorBody::internal()),
        }
    }

    /// A plain text result.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text { text: text.into() }],
            is_error: false,
            structured_content: None,
        }
    }

    /// An error result: `{"error": {"code", "message"}}` as text.
    pub fn error(error: &ErrorBody) -> Self {
        let body = HttpErrorBody {
            error: error.clone(),
        };
        let value = serde_json::to_value(&body).unwrap_or(Value::Null);
        Self {
            content: vec![ToolContent::Text {
                text: value.to_string(),
            }],
            is_error: true,
            structured_content: Some(value),
        }
    }
}

/// The errors from reading a `tool.call`.
#[derive(Debug, thiserror::Error)]
pub enum ToolCallError {
    /// No hub tool has this name.
    #[error("no hub tool is named `{0}`")]
    UnknownTool(String),
    /// The session's audience does not have this tool.
    #[error("this session cannot use `{tool}`")]
    NotAllowed {
        /// The tool name.
        tool: &'static str,
        /// The session's audience.
        audience: ToolAudience,
    },
    /// The arguments do not match the schema.
    #[error("arguments of `{tool}` are invalid: {source}")]
    InvalidArguments {
        /// The tool name.
        tool: &'static str,
        /// The serde error, which names the field.
        #[source]
        source: serde_json::Error,
    },
}

impl ToolCallError {
    /// The error body: `invalid`, or `policy_denied` for a tool that the
    /// session does not have.
    pub fn to_error_body(&self) -> ErrorBody {
        match self {
            Self::NotAllowed { .. } => {
                ErrorBody::new(crate::error::ErrorCode::PolicyDenied, self.to_string())
            }
            _ => ErrorBody::invalid(self.to_string()),
        }
    }

    /// The error as a tool result for the model.
    pub fn to_result(&self) -> ToolCallResult {
        ToolCallResult::error(&self.to_error_body())
    }
}

/// The JSON Schema of a tool's arguments: inline, with no `$schema`,
/// `title` or `$ref`, and always with `properties`.
fn input_schema_for<T: JsonSchema>() -> Value {
    let generator = SchemaSettings::draft2020_12()
        .with(|s| {
            s.inline_subschemas = true;
            s.meta_schema = None;
        })
        .into_generator();
    let mut value = generator.into_root_schema_for::<T>().to_value();
    if let Value::Object(map) = &mut value {
        // The tool description says what the tool does. The struct doc is
        // for Rust readers.
        map.remove("title");
        map.remove("description");
        map.entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
    }
    value
}

fn decode_args<T: DeserializeOwned>(
    tool: &'static str,
    arguments: Value,
) -> Result<T, ToolCallError> {
    let arguments = match arguments {
        Value::Null => Value::Object(Map::new()),
        other => other,
    };
    serde_json::from_value(arguments)
        .map_err(|source| ToolCallError::InvalidArguments { tool, source })
}

// Arguments --------------------------------------------------------------

/// A tool with no arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoArgs {}

/// `memory_list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryListArgs {
    /// `global` or a channel ID. Leave it out for your own scope.
    #[serde(default)]
    pub scope: Option<MemoryScope>,
}

/// `memory_read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryReadArgs {
    /// `global` or a channel ID. Leave it out for your own scope.
    #[serde(default)]
    pub scope: Option<MemoryScope>,
    /// The note path, for example `people/kevin.md`.
    pub path: String,
}

/// `files_list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilesListArgs {
    /// Only list paths that start with this text.
    #[serde(default)]
    pub prefix: Option<String>,
}

/// `ask_decision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AskDecisionArgs {
    /// The question. At most 1,200 characters.
    pub question: String,
    /// Two to four options. Each label has at most 40 characters and each
    /// consequence at most 800. Leave it out for an offer: the hub sets
    /// "Do it", "Not now" and "Never about this".
    #[serde(default)]
    pub options: Vec<DecisionOption>,
    /// The index of the option that you recommend.
    #[serde(default)]
    pub recommended_index: Option<u8>,
    /// More context for the user, in markdown.
    #[serde(default)]
    pub context: Option<String>,
    /// Let the user answer with their own text.
    #[serde(default)]
    pub allow_free_text: bool,
    /// An offer: in a turn that you started on your own, propose work that
    /// you would do. Say in `context` what you will do and what you will
    /// not do. Only in a channel at the level Eager.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub offer: bool,
}

/// `search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// The words to find.
    pub query: String,
    /// Search only this channel.
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
}

/// `post_message`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PostMessageArgs {
    /// The message, in markdown.
    pub body: String,
    /// Channel file paths to attach.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// At most four quick replies under the message, each at most 40
    /// characters. A `type` suggestion, such as "I will type it", only moves
    /// the focus to the composer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<Suggestion>,
    /// In a turn that you started on your own: ask for an OS notification.
    /// Only when the user must act or a deadline is inside 24 hours. The
    /// hub allows a few a day and none in quiet hours.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notify: bool,
}

/// `reply`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplyArgs {
    /// The message to reply to. The reply goes in its thread.
    pub message_id: MessageId,
    /// The reply, in markdown.
    pub body: String,
    /// Also show the reply in the timeline.
    #[serde(default)]
    pub in_timeline: bool,
    /// Channel file paths to attach.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// At most four quick replies under the reply, each at most 40
    /// characters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<Suggestion>,
}

/// `update_message`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateMessageArgs {
    /// One of your own messages.
    pub message_id: MessageId,
    /// The new text, in markdown.
    pub body: String,
}

/// `react`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReactArgs {
    /// The message.
    pub message_id: MessageId,
    /// One emoji.
    pub emoji: String,
}

/// `fetch_timeline`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FetchTimelineArgs {
    /// Only messages older than this message.
    #[serde(default)]
    pub before: Option<MessageId>,
    /// The most messages to return. The default is 50 and the limit is 200.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `fetch_thread`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FetchThreadArgs {
    /// The thread root, or any reply in the thread.
    pub message_id: MessageId,
    /// Only replies older than this reply. Use it to get the next page.
    #[serde(default)]
    pub before: Option<MessageId>,
    /// The most replies to return. The default is 50 and the limit is 200.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `memory_write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryWriteArgs {
    /// The note path, for example `people/kevin.md`.
    pub path: String,
    /// The full new content, in markdown.
    pub content: String,
    /// The sha256 of the note that you read, or `not_exists` for a new note.
    pub expected_sha256: ExpectedHash,
    /// When this write keeps a memory suggestion, the `from` of that
    /// suggestion. The user then sees which task or channel suggested it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_suggestion: Option<crate::turn_input::SuggestionSource>,
}

/// `memory_delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDeleteArgs {
    /// The note path.
    pub path: String,
    /// The sha256 of the note that you read.
    pub expected_sha256: Sha256Hash,
}

/// `files_write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilesWriteArgs {
    /// The channel file path.
    pub path: String,
    /// The text content.
    pub content: String,
}

/// `start_task`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StartTaskArgs {
    /// A short title.
    pub title: String,
    /// The full instruction for the task, in markdown.
    pub brief: String,
    /// The harness account label, model and options. Leave it out for the
    /// channel's default.
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
    /// Run on this worker only.
    #[serde(default)]
    pub worker_id: Option<WorkerId>,
    /// Constraints on the worker: `os`, `tags`, `local_mcp_servers`. The
    /// channel's tags apply too.
    #[serde(default)]
    pub require: Option<PlacementRequire>,
    /// The repos for the workspace. Leave it out for the channel's repos.
    #[serde(default)]
    pub repos: Option<Vec<RepoId>>,
    /// `in_place`, `worktree` or `clone`. Leave it out for the channel's
    /// default.
    #[serde(default)]
    pub isolation: Option<Isolation>,
    /// `ask`, `auto_edits`, `auto` or `full_access`. Leave it out for the
    /// channel's default.
    #[serde(default)]
    pub approval_mode: Option<ApprovalMode>,
    /// Start later, at this time.
    #[serde(default)]
    pub start_at: Option<Timestamp>,
    /// Channel file paths to give the task with its brief. The task gets
    /// them as attachments of its first turn.
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// `message_task`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageTaskArgs {
    /// The task.
    pub task_id: TaskId,
    /// The message, in markdown.
    pub body: String,
    /// Channel file paths to attach.
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// `list_tasks`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListTasksArgs {
    /// Only tasks with this status.
    #[serde(default)]
    pub status: Option<TaskStatus>,
}

/// `read_task`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadTaskArgs {
    /// The task.
    pub task_id: TaskId,
    /// Only session items after this item.
    #[serde(default)]
    pub since_item_id: Option<ItemId>,
    /// Only session items older than this item. Use it to get the next page.
    #[serde(default)]
    pub before_item_id: Option<ItemId>,
    /// The most items to return. The default is 50 and the limit is 200.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `interrupt_task` and `retry_task`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskIdArgs {
    /// The task.
    pub task_id: TaskId,
}

/// `routine_create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoutineCreateArgs {
    /// The name, for example "Morning brief".
    pub name: String,
    /// When it runs.
    pub schedule: RoutineSchedule,
    /// The prompt, in markdown.
    pub prompt: String,
    /// What it does: `coordinator_turn` or `start_task`.
    pub action: RoutineAction,
    /// Whether it runs. The default is true.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// `routine_update`. Fields that you leave out do not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoutineUpdateArgs {
    /// The routine.
    pub routine_id: RoutineId,
    /// The new name.
    #[serde(default)]
    pub name: Option<String>,
    /// The new schedule.
    #[serde(default)]
    pub schedule: Option<RoutineSchedule>,
    /// The new prompt.
    #[serde(default)]
    pub prompt: Option<String>,
    /// The new action.
    #[serde(default)]
    pub action: Option<RoutineAction>,
    /// Turn the routine on or off.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// `routine_delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoutineDeleteArgs {
    /// The routine.
    pub routine_id: RoutineId,
}

/// `suggest_memory`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SuggestMemoryArgs {
    /// The note to keep, in markdown.
    pub note: String,
    /// Why it is worth keeping.
    pub reason: String,
}

/// `read_channel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadChannelArgs {
    /// The channel.
    pub channel_id: ChannelId,
    /// Only messages after this time.
    #[serde(default)]
    pub since: Option<Timestamp>,
}

/// `message_channel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageChannelArgs {
    /// The channel.
    pub channel_id: ChannelId,
    /// The message, in markdown.
    pub body: String,
}

/// `post_update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PostUpdateArgs {
    /// The update, in markdown.
    pub body: String,
}

/// `submit_for_review`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubmitForReviewArgs {
    /// What you did and what the user should check, in markdown.
    pub summary: String,
}

/// `publish_artifact`. The worker uploads the file and adds `blob_sha256`,
/// `size` and `mime` before it forwards the call. The model does not see
/// those fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PublishArtifactArgs {
    /// The file path in your workspace.
    pub path: String,
    /// A short title.
    pub title: String,
    /// Added by the worker: the uploaded blob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub blob_sha256: Option<Sha256Hash>,
    /// Added by the worker: the size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub size: Option<u64>,
    /// Added by the worker: the media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub mime: Option<String>,
}

macro_rules! hub_tools {
    (
        $(
            $(#[$meta:meta])*
            $variant:ident($name:literal, $args:ty, $audiences:expr, $description:literal)
        ),* $(,)?
    ) => {
        /// Every hub tool.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum HubTool {
            $( $(#[$meta])* $variant, )*
        }

        impl HubTool {
            /// Every tool, in list order.
            pub const ALL: &'static [HubTool] = &[$(Self::$variant),*];

            /// The tool name.
            pub fn name(self) -> &'static str {
                match self { $(Self::$variant => $name,)* }
            }

            /// The tool with this name.
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(Self::$variant),)*
                    _ => None,
                }
            }

            /// The description for the model.
            pub fn description(self) -> &'static str {
                match self { $(Self::$variant => $description,)* }
            }

            /// The audiences that have this tool.
            pub fn audiences(self) -> &'static [ToolAudience] {
                match self { $(Self::$variant => $audiences,)* }
            }

            /// The JSON Schema of the arguments.
            pub fn input_schema(self) -> Value {
                match self { $(Self::$variant => input_schema_for::<$args>(),)* }
            }
        }

        /// A hub tool call with typed arguments.
        #[derive(Debug, Clone, PartialEq)]
        pub enum HubToolCall {
            $( $(#[$meta])* $variant($args), )*
        }

        impl HubToolCall {
            /// Reads a call by tool name. `null` arguments read as `{}`.
            pub fn parse(name: &str, arguments: Value) -> Result<Self, ToolCallError> {
                match name {
                    $($name => Ok(Self::$variant(decode_args($name, arguments)?)),)*
                    _ => Err(ToolCallError::UnknownTool(name.to_owned())),
                }
            }

            /// The tool.
            pub fn tool(&self) -> HubTool {
                match self { $(Self::$variant(_) => HubTool::$variant,)* }
            }
        }
    };
}

hub_tools! {
    // Every session.
    /// `memory_list`.
    MemoryList("memory_list", MemoryListArgs, EVERY_SESSION,
        "List memory notes with their paths, hashes and update times. Without a scope, it lists your own memory."),
    /// `memory_read`.
    MemoryRead("memory_read", MemoryReadArgs, EVERY_SESSION,
        "Read one memory note. The result has the content and its sha256. Use the sha256 when you write the note."),
    /// `files_list`.
    FilesList("files_list", FilesListArgs, EVERY_SESSION,
        "List the channel files with their paths, sizes and hashes."),
    /// `list_workers`.
    ListWorkers("list_workers", NoArgs, EVERY_SESSION,
        "List the workers with their operating system, state, tags, harness accounts and models, local MCP servers and repo checkouts. Use this before you choose where a task runs."),
    /// `ask_decision`.
    AskDecision("ask_decision", AskDecisionArgs, EVERY_SESSION,
        "Ask the user a question with two to four options. Each option has a label and its consequence. This does not wait: end your turn. The answer arrives as input to a later turn."),
    /// `search`.
    Search("search", SearchArgs, EVERY_SESSION,
        "Search messages, memory notes and task titles."),
    /// `list_connector_tools`. The hub lists it only when the channel can
    /// use a connection.
    ListConnectorTools("list_connector_tools", ListConnectorToolsArgs, EVERY_SESSION,
        "List the tools of one connector from \"Connectors in this channel\", by its handle. Each tool has its slug, a short description, its kind (read, change or delete) and whether the user must approve it. Use search for a service with many tools."),
    /// `get_connector_tool`.
    GetConnectorTool("get_connector_tool", GetConnectorToolArgs, EVERY_SESSION,
        "Get the input schema of one connector tool. Call it before run_connector_tool."),
    /// `run_connector_tool`.
    RunConnectorTool("run_connector_tool", RunConnectorToolArgs, EVERY_SESSION,
        "Run one connector tool with its arguments. When the connection's rule asks the user, the user sees a card with the arguments and allows or denies the call. The result has a status: ran, waiting_for_user or denied. When the connector needs a new sign-in, ask the user to reconnect it in Settings > Connectors."),
    /// `voice_guide`. The hub lists it only while "Write in my voice" is
    /// on.
    VoiceGuide("voice_guide", VoiceGuideArgs, EVERY_SESSION,
        "Get the user's style guide for one context before you write a draft that goes out in the user's name: mail, a chat message or a post. Use it for wording only. Never add a date, a time, a price, a promise or an agreement that the request does not contain. Never use it for your replies to the user, task briefs, code or commits."),
    /// `list_skills`. The hub lists it only for a harness that does not
    /// load skills itself.
    ListSkills("list_skills", NoArgs, EVERY_SESSION,
        "List the skills of this channel with their descriptions."),
    /// `read_skill`.
    ReadSkill("read_skill", ReadSkillArgs, EVERY_SESSION,
        "Read a skill's SKILL.md, or another file of the skill. Follow its instructions when the task fits the skill."),
    // Coordinators.
    /// `post_message`.
    PostMessage("post_message", PostMessageArgs, COORDINATORS,
        "Post a message in the channel timeline. You can post several short messages in one turn. To attach channel files, give their paths from files_list."),
    /// `reply`.
    Reply("reply", ReplyArgs, COORDINATORS,
        "Reply in the thread of a message. Set in_timeline to also show the reply in the timeline. To attach channel files, give their paths from files_list."),
    /// `update_message`.
    UpdateMessage("update_message", UpdateMessageArgs, COORDINATORS,
        "Change the text of one of your own messages."),
    /// `react`.
    React("react", ReactArgs, COORDINATORS,
        "Add an emoji reaction to a message."),
    /// `no_reply_needed`.
    NoReplyNeeded("no_reply_needed", NoReplyNeededArgs, COORDINATORS,
        "End the turn without a visible reply. Use it when the events need no answer. In a turn that you started on your own, give a finding: one short line about what you saw and did not report."),
    /// `fetch_timeline`.
    FetchTimeline("fetch_timeline", FetchTimelineArgs, COORDINATORS,
        "Get older messages from the channel timeline, newest first."),
    /// `fetch_thread`.
    FetchThread("fetch_thread", FetchThreadArgs, COORDINATORS,
        "Get a thread: the root message and a page of its replies, oldest first. When has_more is true, older replies exist: call again with before set to the ID of the oldest reply that you got."),
    /// `memory_write`.
    MemoryWrite("memory_write", MemoryWriteArgs, COORDINATORS,
        "Write a memory note in your own memory. Give the sha256 that you read, or not_exists for a new note. If the note changed since you read it, the write fails with stale_write: read it again and retry. When the write keeps a memory suggestion, set from_suggestion to the suggestion's `from`. The user sees each write as a line in the channel."),
    /// `memory_delete`.
    MemoryDelete("memory_delete", MemoryDeleteArgs, COORDINATORS,
        "Delete a memory note in your own memory. Give the sha256 that you read."),
    /// `files_write`.
    FilesWrite("files_write", FilesWriteArgs, COORDINATORS,
        "Write a text file in the channel files."),
    /// `start_task`.
    StartTask("start_task", StartTaskArgs, COORDINATORS,
        "Start a task. The task runs in its own session on a worker and reports in its own thread. Placement uses the channel defaults for anything that you leave out. To give the task channel files, put their paths in attachments."),
    /// `message_task`.
    MessageTask("message_task", MessageTaskArgs, COORDINATORS,
        "Send a message to a task. It starts a new turn, or steers the running turn when the harness can steer. To attach channel files, give their paths from files_list."),
    /// `list_tasks`.
    ListTasks("list_tasks", ListTasksArgs, COORDINATORS,
        "List the tasks in this channel. A long brief or review summary is cut to a preview here. Use read_task for the full task."),
    /// `read_task`.
    ReadTask("read_task", ReadTaskArgs, COORDINATORS,
        "Read a task: its status, its review summary and a page of its recent session items, oldest first. When has_more is true, older items exist: call again with before_item_id set to the ID of the oldest item that you got."),
    /// `interrupt_task`.
    InterruptTask("interrupt_task", TaskIdArgs, COORDINATORS,
        "Stop the running turn of a task."),
    /// `retry_task`.
    RetryTask("retry_task", TaskIdArgs, COORDINATORS,
        "Retry a failed task. It goes back to Up next."),
    /// `routine_list`.
    RoutineList("routine_list", NoArgs, COORDINATORS,
        "List the routines in this channel."),
    /// `routine_create`.
    RoutineCreate("routine_create", RoutineCreateArgs, COORDINATORS,
        "Create a routine: a prompt that runs on a schedule in this channel."),
    /// `routine_update`.
    RoutineUpdate("routine_update", RoutineUpdateArgs, COORDINATORS,
        "Change a routine. Fields that you leave out do not change."),
    /// `routine_delete`.
    RoutineDelete("routine_delete", RoutineDeleteArgs, COORDINATORS,
        "Delete a routine."),
    /// `follow_up_later`.
    FollowUpLater("follow_up_later", FollowUpLaterArgs, COORDINATORS,
        "Wake yourself later in this channel, once, with a reason. Use it when you said you would check again, for example after a vendor replies. The same key replaces the old follow-up. At least 15 minutes and at most 30 days ahead, and at most 20 waiting in a channel."),
    /// `cancel_follow_up`.
    CancelFollowUp("cancel_follow_up", CancelFollowUpArgs, COORDINATORS,
        "Cancel a follow-up that you set, by its routine ID or its key."),
    /// `set_proactivity`.
    SetProactivity("set_proactivity", SetProactivityArgs, COORDINATORS,
        "Make yourself quieter when the user asks, for example \"stop pinging me\" or \"not today\". Set a quieter level for this channel, or snooze everything that starts on its own. You cannot set a louder level: tell the user to change it in Settings > Proactivity."),
    /// `list_connections`.
    ListConnections("list_connections", NoArgs, COORDINATORS,
        "List the connections with their handle, service, account, scope, approval rules and status. In a channel, it lists the ones that this channel can use, and says which ones this channel turned off."),
    /// `list_installed_skills`.
    ListInstalledSkills("list_installed_skills", NoArgs, COORDINATORS,
        "List the installed skills with their scope, whether they are on, whether they have scripts, and whether a new version waits for review. In a channel, it lists the ones that this channel can use, and says which ones this channel turned off."),
    /// `connect_service`.
    ConnectService("connect_service", ConnectServiceArgs, COORDINATORS,
        "Post a connect card for a service. The user checks the settings and continues in the browser. You never handle a password or a key. In a channel, the default scope is the channels with this channel's tags, or this channel alone."),
    /// `add_skill`.
    AddSkill("add_skill", AddSkillArgs, COORDINATORS,
        "Post an install card for a skill from a git URL, owner/repo, or a search_skills entry. The card shows its license, scripts and audits, and the user adds it or not."),
    /// `suggest_memory`.
    SuggestMemory("suggest_memory", SuggestMemoryArgs, CHANNEL_COORDINATOR_AND_TASK,
        "Suggest a note for memory. A task sends it to the channel coordinator. A channel coordinator sends it to the Tutti coordinator as a global memory suggestion. The coordinator that gets it decides whether to keep it."),
    // The Tutti coordinator.
    /// `voice_sample`.
    VoiceSample("voice_sample", VoiceSampleArgs, TUTTI_ONLY,
        "Return the sample draft for a voice_sample event. Nothing is sent and the hub does not save it. Post nothing else for this event."),
    /// `propose_voice_rule`.
    ProposeVoiceRule("propose_voice_rule", ProposeVoiceRuleArgs, TUTTI_ONLY,
        "Propose one rule for the user's style guide after a voice_edits event. Cite at least 3 edited pairs of that context that agree. The hub checks the pairs and shows the user an offer card; voice.md changes only after the user says Do it. You cannot write voice.md yourself."),
    /// `list_channels`.
    ListChannels("list_channels", NoArgs, TUTTI_ONLY,
        "List the channels with their purpose and task counts."),
    /// `read_channel`.
    ReadChannel("read_channel", ReadChannelArgs, TUTTI_ONLY,
        "Read a channel: its timeline, its tasks and its memory index."),
    /// `message_channel`.
    MessageChannel("message_channel", MessageChannelArgs, TUTTI_ONLY,
        "Post in a channel as a message from Tutti. This wakes the coordinator of that channel."),
    /// `create_channel`.
    CreateChannel("create_channel", CreateChannelArgs, TUTTI_ONLY,
        "Create a channel with a name and a purpose, and optionally a group, tags and models."),
    /// `update_channel`.
    UpdateChannel("update_channel", UpdateChannelArgs, TUTTI_ONLY,
        "Change a channel. Fields that you leave out do not change. A change to its tags asks the user first."),
    /// `archive_channel`.
    ArchiveChannel("archive_channel", ArchiveChannelArgs, TUTTI_ONLY,
        "Archive a channel. This asks the user first."),
    /// `set_default_model`.
    SetDefaultModel("set_default_model", SetDefaultModelArgs, TUTTI_ONLY,
        "Set the model of Tutti, which is also the model of every channel that has no model of its own."),
    /// `set_worker_tags`.
    SetWorkerTags("set_worker_tags", SetWorkerTagsArgs, TUTTI_ONLY,
        "Replace the user tags of a worker. This asks the user first."),
    /// `install_tool`.
    InstallTool("install_tool", InstallToolArgs, TUTTI_ONLY,
        "Install or update a managed tool, such as claude-code or codex, on a worker."),
    /// `add_account`.
    AddAccount("add_account", AddAccountArgs, TUTTI_ONLY,
        "Add a provider account on a worker and post a sign-in card. The user signs in on the card."),
    /// `start_sign_in`.
    StartSignIn("start_sign_in", StartSignInArgs, TUTTI_ONLY,
        "Post a sign-in card for an account that is signed out. The user signs in on the card."),
    /// `set_account_extras`.
    SetAccountExtras("set_account_extras", SetAccountExtrasArgs, TUTTI_ONLY,
        "Turn an account's provider extras, such as claude.ai connectors and synced plugins and skills, on or off. Turning them on asks the user first."),
    /// `search_connector_catalog`.
    SearchConnectorCatalog("search_connector_catalog", SearchConnectorCatalogArgs, TUTTI_ONLY,
        "Search the connector catalog for services to connect, such as Gmail or GitHub."),
    /// `update_connection`.
    UpdateConnection("update_connection", UpdateConnectionArgs, TUTTI_ONLY,
        "Change a connection's name, scope or approval rules. A wider scope or a looser rule asks the user first."),
    /// `disconnect`.
    Disconnect("disconnect", DisconnectArgs, TUTTI_ONLY,
        "Disconnect a connection. This asks the user first."),
    /// `search_skills`.
    SearchSkills("search_skills", SearchSkillsArgs, TUTTI_ONLY,
        "Search the skill directories for skills to add."),
    /// `update_skill`.
    UpdateSkill("update_skill", UpdateSkillArgs, TUTTI_ONLY,
        "Turn a skill on or off, change its scope, or take its new version. Turning on a skill with scripts or a wider scope asks the user first."),
    /// `remove_skill`.
    RemoveSkill("remove_skill", RemoveSkillArgs, TUTTI_ONLY,
        "Remove a skill. This asks the user first."),
    // A channel's coordinator, for its own channel only.
    /// `update_this_channel`.
    UpdateThisChannel("update_this_channel", UpdateThisChannelArgs, CHANNEL_COORDINATOR_ONLY,
        "Change this channel's purpose, instructions, models or repos. Fields that you leave out do not change."),
    /// `set_channel_tags`.
    SetChannelTags("set_channel_tags", SetChannelTagsArgs, CHANNEL_COORDINATOR_ONLY,
        "Replace this channel's tags. This asks the user first."),
    /// `set_channel_connector`.
    SetChannelConnector("set_channel_connector", SetChannelConnectorArgs, CHANNEL_COORDINATOR_ONLY,
        "Turn a connection off or on in this channel. You can turn on only a connection whose scope gives it to this channel."),
    /// `set_channel_skill`.
    SetChannelSkill("set_channel_skill", SetChannelSkillArgs, CHANNEL_COORDINATOR_ONLY,
        "Turn a skill off or on in this channel. You can turn on only a skill whose scope gives it to this channel."),
    /// `request_access`.
    RequestAccess("request_access", RequestAccessArgs, CHANNEL_COORDINATOR_ONLY,
        "Post a card that asks the user to give this channel a connection or a skill that its scope does not give it."),
    // Task sessions.
    /// `post_update`.
    PostUpdate("post_update", PostUpdateArgs, TASK_ONLY,
        "Post an update in your task thread. Your turn continues."),
    /// `submit_for_review`.
    SubmitForReview("submit_for_review", SubmitForReviewArgs, TASK_ONLY,
        "Submit your work for review. When your turn ends, the task goes to Ready for review with this summary."),
    /// `publish_artifact`.
    PublishArtifact("publish_artifact", PublishArtifactArgs, TASK_ONLY,
        "Publish a file from your workspace as an artifact in the channel files. The result has its channel file path."),
}

impl HubTool {
    /// Whether a session of this audience has the tool.
    pub fn is_available_to(self, audience: ToolAudience) -> bool {
        self.audiences().contains(&audience)
    }

    /// Whether this tool is one of the three ways a coordinator speaks:
    /// `post_message`, `reply` or `no_reply_needed`.
    pub fn is_speech(self) -> bool {
        matches!(self, Self::PostMessage | Self::Reply | Self::NoReplyNeeded)
    }

    /// The `tool.list` entry.
    pub fn definition(self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_owned(),
            description: self.description().to_owned(),
            input_schema: self.input_schema(),
        }
    }
}

impl HubToolCall {
    /// Reads a call and checks that a session of `audience` has the tool.
    pub fn parse_for(
        audience: ToolAudience,
        name: &str,
        arguments: Value,
    ) -> Result<Self, ToolCallError> {
        let tool =
            HubTool::from_name(name).ok_or_else(|| ToolCallError::UnknownTool(name.to_owned()))?;
        if !tool.is_available_to(audience) {
            return Err(ToolCallError::NotAllowed {
                tool: tool.name(),
                audience,
            });
        }
        Self::parse(name, arguments)
    }
}

/// Whether the hub must post the coordinator's last assistant message after
/// a turn: true when the turn called none of `post_message`, `reply` and
/// `no_reply_needed`. The caller also checks that the turn has assistant
/// text.
pub fn needs_fallback_post(called: impl IntoIterator<Item = HubTool>) -> bool {
    !called.into_iter().any(HubTool::is_speech)
}

/// The tools of one audience, in list order, for `tool.list`.
pub fn tools_for(audience: ToolAudience) -> Vec<ToolDefinition> {
    HubTool::ALL
        .iter()
        .filter(|t| t.is_available_to(audience))
        .map(|t| t.definition())
        .collect()
}

// Results ----------------------------------------------------------------

/// The result of a tool that only confirms: `{"ok": true}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkResult {
    /// Always true.
    pub ok: bool,
}

impl OkResult {
    /// `{"ok": true}`.
    pub const OK: Self = Self { ok: true };
}

/// The result of a tool that posts a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageIdResult {
    /// The new message.
    pub message_id: MessageId,
}

/// `memory_list` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryListResult {
    /// The notes.
    pub notes: Vec<MemoryNoteSummary>,
}

/// `memory_read` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryReadResult {
    /// The scope.
    pub scope: MemoryScope,
    /// The path.
    pub path: String,
    /// The content.
    pub content: String,
    /// The hash of the content.
    pub sha256: Sha256Hash,
}

/// `files_list` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesListResult {
    /// The files.
    pub files: Vec<ChannelFile>,
}

/// A harness account in `list_workers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSummary {
    /// The label.
    pub label: String,
    /// The harness.
    pub harness: String,
    /// Whether it is signed in.
    pub auth_state: AuthState,
    /// The model IDs.
    pub models: Vec<String>,
}

/// A worker in `list_workers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSummary {
    /// The ID.
    pub id: WorkerId,
    /// The name.
    pub name: String,
    /// The operating system.
    pub os: Os,
    /// Online or offline.
    pub state: WorkerState,
    /// Whether the user paused it.
    pub paused: bool,
    /// The user tags and the inventory tags.
    #[serde(alias = "labels")]
    pub tags: Vec<String>,
    /// The enabled harness accounts.
    pub accounts: Vec<AccountSummary>,
    /// The local MCP server names.
    pub local_mcp_servers: Vec<String>,
    /// The repo checkouts.
    pub checkouts: Vec<CheckoutInfo>,
}

impl WorkerSummary {
    /// The summary of a worker.
    pub fn from_worker(worker: &Worker) -> Self {
        let inventory = worker.inventory.as_ref();
        Self {
            id: worker.id,
            name: worker.name.clone(),
            os: worker.os,
            state: worker.state,
            paused: worker.paused,
            tags: worker.all_tags().into_iter().map(str::to_owned).collect(),
            accounts: inventory
                .map(|inv| {
                    inv.accounts
                        .iter()
                        .filter(|a| a.enabled)
                        .map(|a| AccountSummary {
                            label: a.label.clone(),
                            harness: a.harness.clone(),
                            auth_state: a.auth_state,
                            models: a.models.iter().map(|m| m.id.clone()).collect(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            local_mcp_servers: inventory
                .map(|inv| {
                    inv.local_mcp_servers
                        .iter()
                        .map(|s| s.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
            checkouts: inventory
                .map(|inv| inv.checkouts.clone())
                .unwrap_or_default(),
        }
    }
}

/// `list_workers` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListWorkersResult {
    /// The workers.
    pub workers: Vec<WorkerSummary>,
}

/// `ask_decision` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskDecisionResult {
    /// The new decision.
    pub decision_id: DecisionId,
}

/// One search result. Serialized with a `kind` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchHit {
    /// A message.
    Message {
        /// The message.
        message_id: MessageId,
        /// Its channel.
        channel_id: ChannelId,
        /// Its thread root, when it is a reply.
        thread_root_id: Option<MessageId>,
        /// The matching text.
        snippet: String,
        /// When it was posted.
        created_at: Timestamp,
    },
    /// A memory note.
    MemoryNote {
        /// Its scope.
        scope: MemoryScope,
        /// Its path.
        path: String,
        /// The matching text.
        snippet: String,
        /// When the note last changed. Older hubs leave it out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_at: Option<Timestamp>,
    },
    /// A task.
    Task {
        /// The task.
        task_id: TaskId,
        /// Its channel.
        channel_id: ChannelId,
        /// Its title.
        title: String,
        /// Its status.
        status: TaskStatus,
        /// When the task last changed. Older hubs leave it out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_at: Option<Timestamp>,
    },
}

impl SearchHit {
    /// The kind of the hit.
    pub fn kind(&self) -> SearchKind {
        match self {
            SearchHit::Message { .. } => SearchKind::Message,
            SearchHit::MemoryNote { .. } => SearchKind::MemoryNote,
            SearchHit::Task { .. } => SearchKind::Task,
        }
    }
}

/// What a search can find. `message.search` takes a list of them to search
/// only those kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    /// Messages in timelines and threads.
    Message,
    /// Memory notes.
    MemoryNote,
    /// Tasks, by title and brief.
    Task,
}

impl SearchKind {
    /// The kind as the search index names it.
    pub fn as_str(self) -> &'static str {
        match self {
            SearchKind::Message => "message",
            SearchKind::MemoryNote => "memory_note",
            SearchKind::Task => "task",
        }
    }
}

/// `search` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    /// The results, best first.
    pub hits: Vec<SearchHit>,
}

/// `fetch_timeline` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchTimelineResult {
    /// The messages, newest first.
    pub messages: Vec<Message>,
    /// Whether older messages exist.
    pub has_more: bool,
}

/// `fetch_thread` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchThreadResult {
    /// The root.
    pub root: Message,
    /// A page of replies, oldest first: the newest replies that fit, or the
    /// newest ones older than `before`.
    pub replies: Vec<Message>,
    /// True when older replies exist than the oldest one in `replies`.
    #[serde(default)]
    pub has_more: bool,
    /// The reply that `message_id` names, when it is not in `replies`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
}

/// `memory_write` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryWriteResult {
    /// The hash of the new content.
    pub sha256: Sha256Hash,
    /// The new version.
    pub version: u64,
}

/// `files_write` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesWriteResult {
    /// The path.
    pub path: String,
    /// The hash of the content.
    pub sha256: Sha256Hash,
    /// The size in bytes.
    pub size: u64,
}

/// `start_task` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartTaskResult {
    /// The new task.
    pub task_id: TaskId,
    /// Its status.
    pub status: TaskStatus,
    /// Why it is in that status, for example no worker fits yet.
    pub status_reason: Option<String>,
    /// What the hub changed in the request, because the channel does not
    /// allow it, for example a lower approval mode. Empty when nothing
    /// changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// How `message_task` delivered a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDelivery {
    /// It started a new turn.
    Turn,
    /// It steered the running turn.
    Steer,
    /// It waits for the turn to end or for the worker to come online.
    Queued,
}

/// `message_task` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageTaskResult {
    /// The task.
    pub task_id: TaskId,
    /// How the message was delivered.
    pub delivery: MessageDelivery,
}

/// `list_tasks` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListTasksResult {
    /// The tasks.
    pub tasks: Vec<Task>,
}

/// `read_task` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadTaskResult {
    /// The task, with its review summary.
    pub task: Task,
    /// Recent session items, oldest first.
    pub items: Vec<SessionItem>,
    /// True when more items exist: newer ones after the last item when the
    /// call gave `since_item_id`, and older ones before the first item
    /// otherwise.
    #[serde(default)]
    pub has_more: bool,
}

/// `interrupt_task` and `retry_task` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStatusResult {
    /// The task.
    pub task_id: TaskId,
    /// Its status after the call.
    pub status: TaskStatus,
}

/// `routine_list` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineListResult {
    /// The routines.
    pub routines: Vec<Routine>,
}

/// `routine_create` and `routine_update` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineResult {
    /// The routine after the change.
    pub routine: Routine,
    /// What the hub changed in the action of the routine, because the
    /// channel does not allow it. Empty when nothing changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Task counts by status, for `list_channels`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCounts {
    /// Up next.
    pub up_next: u32,
    /// Scheduled.
    pub scheduled: u32,
    /// Working.
    pub working: u32,
    /// Needs input.
    pub needs_input: u32,
    /// Ready for review.
    pub review: u32,
    /// Failed.
    pub failed: u32,
}

impl TaskCounts {
    /// Counts one task. Resolved tasks are not counted.
    pub fn add(&mut self, status: TaskStatus) {
        let slot = match status {
            TaskStatus::UpNext => &mut self.up_next,
            TaskStatus::Scheduled => &mut self.scheduled,
            TaskStatus::Working => &mut self.working,
            TaskStatus::NeedsInput => &mut self.needs_input,
            TaskStatus::Review => &mut self.review,
            TaskStatus::Failed => &mut self.failed,
            TaskStatus::Resolved => return,
        };
        *slot += 1;
    }

    /// Tasks on the Active tab.
    pub fn active(&self) -> u32 {
        self.working + self.needs_input + self.review + self.failed
    }
}

/// A channel in `list_channels`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSummary {
    /// The ID.
    pub id: ChannelId,
    /// The name.
    pub name: String,
    /// `tutti` or `standard`.
    pub kind: ChannelKind,
    /// The purpose.
    pub purpose: String,
    /// Whether it is archived.
    pub archived: bool,
    /// Its task counts.
    pub tasks: TaskCounts,
}

/// `list_channels` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListChannelsResult {
    /// The channels.
    pub channels: Vec<ChannelSummary>,
}

/// `read_channel` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadChannelResult {
    /// The channel.
    pub channel: Channel,
    /// Timeline messages, oldest first.
    pub messages: Vec<Message>,
    /// Tasks that are not resolved or archived.
    pub tasks: Vec<Task>,
    /// The channel's memory index.
    pub memory: Vec<MemoryNoteSummary>,
}

/// `publish_artifact` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishArtifactResult {
    /// The artifact's channel file path, under `artifacts/<task-id>/`.
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UseIn;
    use serde_json::json;

    #[test]
    fn the_tutti_audience_reads_the_old_name_and_writes_the_new_one() {
        let old: ToolAudience = serde_json::from_value(json!("everything_coordinator")).unwrap();
        let new: ToolAudience = serde_json::from_value(json!("tutti_coordinator")).unwrap();
        assert_eq!(
            (old, new),
            (
                ToolAudience::TuttiCoordinator,
                ToolAudience::TuttiCoordinator
            )
        );
        assert_eq!(
            serde_json::to_value(ToolAudience::TuttiCoordinator).unwrap(),
            json!("tutti_coordinator")
        );
    }

    fn names(audience: ToolAudience) -> Vec<String> {
        tools_for(audience).into_iter().map(|t| t.name).collect()
    }

    #[test]
    fn tool_lists_per_audience() {
        let every = [
            "memory_list",
            "memory_read",
            "files_list",
            "list_workers",
            "ask_decision",
            "search",
        ];
        let coordinator = [
            "post_message",
            "reply",
            "update_message",
            "react",
            "no_reply_needed",
            "fetch_timeline",
            "fetch_thread",
            "memory_write",
            "memory_delete",
            "files_write",
            "start_task",
            "message_task",
            "list_tasks",
            "read_task",
            "interrupt_task",
            "retry_task",
            "routine_list",
            "routine_create",
            "routine_update",
            "routine_delete",
            "follow_up_later",
            "cancel_follow_up",
            "set_proactivity",
        ];
        let every_extensions = [
            "list_connector_tools",
            "get_connector_tool",
            "run_connector_tool",
            "list_skills",
            "read_skill",
            "voice_guide",
        ];
        let coordinator_extensions = [
            "list_connections",
            "list_installed_skills",
            "connect_service",
            "add_skill",
        ];
        let tutti_only = [
            "voice_sample",
            "propose_voice_rule",
            "list_channels",
            "read_channel",
            "message_channel",
            "create_channel",
            "update_channel",
            "archive_channel",
            "set_default_model",
            "set_worker_tags",
            "install_tool",
            "add_account",
            "start_sign_in",
            "set_account_extras",
            "search_connector_catalog",
            "update_connection",
            "disconnect",
            "search_skills",
            "update_skill",
            "remove_skill",
        ];
        let channel_only = [
            "update_this_channel",
            "set_channel_tags",
            "set_channel_connector",
            "set_channel_skill",
            "request_access",
        ];
        let task_only = ["post_update", "submit_for_review", "publish_artifact"];

        let tutti = names(ToolAudience::TuttiCoordinator);
        let channel = names(ToolAudience::ChannelCoordinator);
        let task = names(ToolAudience::TaskSession);

        for name in every.into_iter().chain(every_extensions) {
            assert!(tutti.contains(&name.to_owned()));
            assert!(channel.contains(&name.to_owned()));
            assert!(task.contains(&name.to_owned()));
        }
        for name in coordinator.into_iter().chain(coordinator_extensions) {
            assert!(tutti.contains(&name.to_owned()), "{name}");
            assert!(channel.contains(&name.to_owned()), "{name}");
            assert!(!task.contains(&name.to_owned()), "{name}");
        }
        for name in tutti_only {
            assert!(tutti.contains(&name.to_owned()));
            assert!(!channel.contains(&name.to_owned()));
            assert!(!task.contains(&name.to_owned()));
        }
        for name in task_only {
            assert!(task.contains(&name.to_owned()));
            assert!(!tutti.contains(&name.to_owned()));
            assert!(!channel.contains(&name.to_owned()));
        }
        for name in channel_only {
            assert!(channel.contains(&name.to_owned()), "{name}");
            assert!(!tutti.contains(&name.to_owned()), "{name}");
            assert!(!task.contains(&name.to_owned()), "{name}");
        }
        // suggest_memory: channel coordinators and tasks, not Tutti.
        assert!(channel.contains(&"suggest_memory".to_owned()));
        assert!(task.contains(&"suggest_memory".to_owned()));
        assert!(!tutti.contains(&"suggest_memory".to_owned()));

        let every_len = every.len() + every_extensions.len();
        let coordinator_len = coordinator.len() + coordinator_extensions.len();
        assert_eq!(tutti.len(), every_len + coordinator_len + tutti_only.len());
        assert_eq!(
            channel.len(),
            every_len + coordinator_len + channel_only.len() + 1
        );
        assert_eq!(task.len(), every_len + task_only.len() + 1);
    }

    #[test]
    fn audience_for_session() {
        assert_eq!(
            ToolAudience::for_session(SessionRole::Coordinator, ChannelKind::Tutti),
            ToolAudience::TuttiCoordinator
        );
        assert_eq!(
            ToolAudience::for_session(SessionRole::Coordinator, ChannelKind::Standard),
            ToolAudience::ChannelCoordinator
        );
        assert_eq!(
            ToolAudience::for_session(SessionRole::Task, ChannelKind::Tutti),
            ToolAudience::TaskSession
        );
    }

    #[test]
    fn every_schema_is_an_inline_object() {
        for tool in HubTool::ALL {
            let schema = tool.input_schema();
            assert_eq!(schema["type"], "object", "{}", tool.name());
            assert!(schema["properties"].is_object(), "{}", tool.name());
            let text = schema.to_string();
            assert!(!text.contains("$ref"), "{} has a $ref: {text}", tool.name());
            assert!(!text.contains("$schema"), "{}", tool.name());
            assert!(!tool.description().is_empty());
            assert_eq!(HubTool::from_name(tool.name()), Some(*tool));
        }
    }

    #[test]
    fn schema_details() {
        let schema = HubTool::AskDecision.input_schema();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        // An offer leaves out the options: the hub sets them.
        assert_eq!(required, ["question"]);
        assert!(schema["properties"]["offer"].is_object());
        let option = &schema["properties"]["options"]["items"];
        assert!(option["properties"]["label"].is_object());
        assert!(option["properties"]["consequence"].is_object());
        assert!(option["properties"].get("id").is_none());

        let publish = HubTool::PublishArtifact.input_schema();
        let props = publish["properties"].as_object().unwrap();
        let mut keys: Vec<&String> = props.keys().collect();
        keys.sort();
        assert_eq!(keys, ["path", "title"]);

        let start = HubTool::StartTask.input_schema();
        assert_eq!(
            start["properties"]["worker_id"]["pattern"]
                .as_str()
                .map(|p| p.starts_with("^wrk_")),
            Some(true)
        );
        assert!(start.get("description").is_none());
        let none = HubTool::NoReplyNeeded.input_schema();
        assert!(none["properties"]["finding"].is_object());
        assert!(none.get("required").is_none());
        let list = HubTool::RoutineList.input_schema();
        assert_eq!(list["properties"], json!({}));
        let post = HubTool::PostMessage.input_schema();
        assert!(post["properties"]["suggestions"]["items"]["properties"]["action"].is_object());
        assert!(post["properties"]["notify"].is_object());
        let follow = HubTool::FollowUpLater.input_schema();
        let required: Vec<&str> = follow["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, ["reason"]);
    }

    #[test]
    fn parse_calls() {
        let call = HubToolCall::parse(
            "ask_decision",
            json!({
                "question": "Ship it?",
                "options": [
                    {"label": "Yes", "consequence": "It ships."},
                    {"label": "Not now", "consequence": "It waits."}
                ],
                "unknown_field": 1
            }),
        )
        .unwrap();
        let HubToolCall::AskDecision(args) = &call else {
            panic!("wrong variant");
        };
        assert!(!args.allow_free_text);
        assert_eq!(call.tool(), HubTool::AskDecision);

        let none = HubToolCall::parse("no_reply_needed", Value::Null).unwrap();
        assert_eq!(
            none,
            HubToolCall::NoReplyNeeded(NoReplyNeededArgs::default())
        );

        let start = HubToolCall::parse(
            "start_task",
            json!({"title": "Test on Windows", "brief": "Run the tests.",
                   "require": {"os": "windows"}, "start_at": "2026-10-30T09:00:00Z"}),
        )
        .unwrap();
        let HubToolCall::StartTask(args) = start else {
            panic!("wrong variant");
        };
        assert_eq!(args.require.unwrap().os, Some(Os::Windows));
        assert_eq!(
            args.start_at,
            Some(Timestamp::parse_rfc3339("2026-10-30T09:00:00Z").unwrap())
        );

        let write = HubToolCall::parse(
            "memory_write",
            json!({"path": "people/kevin.md", "content": "Kevin.", "expected_sha256": "not_exists"}),
        )
        .unwrap();
        assert!(matches!(
            write,
            HubToolCall::MemoryWrite(MemoryWriteArgs {
                expected_sha256: ExpectedHash::NotExists,
                from_suggestion: None,
                ..
            })
        ));
        let task = "tsk_00000000-0000-0000-0000-000000000003";
        let kept = HubToolCall::parse(
            "memory_write",
            json!({"path": "a.md", "content": "A.", "expected_sha256": "not_exists", "from_suggestion": task}),
        )
        .unwrap();
        assert!(matches!(
            kept,
            HubToolCall::MemoryWrite(MemoryWriteArgs {
                from_suggestion: Some(crate::turn_input::SuggestionSource::Task(_)),
                ..
            })
        ));
    }

    #[test]
    fn parse_errors() {
        assert!(matches!(
            HubToolCall::parse("rm_rf", json!({})),
            Err(ToolCallError::UnknownTool(_))
        ));
        let err = HubToolCall::parse("reply", json!({"body": "hi"})).unwrap_err();
        assert!(err.to_string().contains("message_id"), "{err}");
        assert_eq!(err.to_error_body().code, crate::error::ErrorCode::Invalid);

        let err = HubToolCall::parse_for(
            ToolAudience::TaskSession,
            "post_message",
            json!({"body": "x"}),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ToolCallError::NotAllowed {
                tool: "post_message",
                ..
            }
        ));
        assert_eq!(
            err.to_error_body().code,
            crate::error::ErrorCode::PolicyDenied
        );
        let result = err.to_result();
        assert!(result.is_error);

        assert!(
            HubToolCall::parse_for(
                ToolAudience::TaskSession,
                "post_update",
                json!({"body": "Halfway."})
            )
            .is_ok()
        );
    }

    #[test]
    fn publish_artifact_carries_worker_fields() {
        let args = PublishArtifactArgs {
            path: "out/report.pdf".into(),
            title: "Report".into(),
            blob_sha256: Some("d".repeat(64).parse().unwrap()),
            size: Some(10),
            mime: Some("application/pdf".into()),
        };
        let value = serde_json::to_value(&args).unwrap();
        let back = HubToolCall::parse("publish_artifact", value).unwrap();
        assert_eq!(back, HubToolCall::PublishArtifact(args));
    }

    #[test]
    fn tool_results() {
        let result = ToolCallResult::json(&MessageIdResult {
            message_id: "msg_00000000-0000-0000-0000-000000000001".parse().unwrap(),
        });
        assert!(!result.is_error);
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({
                "content": [{"type": "text", "text": "{\"message_id\":\"msg_00000000-0000-0000-0000-000000000001\"}"}],
                "is_error": false,
                "structured_content": {"message_id": "msg_00000000-0000-0000-0000-000000000001"}
            })
        );
        let err = ToolCallResult::error(&ErrorBody::new(
            crate::error::ErrorCode::StaleWrite,
            "The note changed. Read it again.",
        ));
        assert!(err.is_error);
        assert_eq!(
            err.structured_content.unwrap()["error"]["code"],
            "stale_write"
        );
    }

    #[test]
    fn fallback_post_rule() {
        assert!(needs_fallback_post([]));
        assert!(needs_fallback_post([
            HubTool::MemoryRead,
            HubTool::StartTask
        ]));
        assert!(!needs_fallback_post([HubTool::StartTask, HubTool::Reply]));
        assert!(!needs_fallback_post([HubTool::NoReplyNeeded]));
    }

    #[test]
    fn connector_tools_parse() {
        let run = HubToolCall::parse_for(
            ToolAudience::TaskSession,
            "run_connector_tool",
            json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL",
                   "arguments": {"recipient_email": "kevin@example.com", "subject": "Friday"}}),
        )
        .unwrap();
        let HubToolCall::RunConnectorTool(args) = &run else {
            panic!("wrong variant");
        };
        assert_eq!(args.connector, "gmail_work");
        assert_eq!(args.arguments["subject"], "Friday");
        let no_args =
            HubToolCall::parse("run_connector_tool", json!({"connector": "c", "tool": "T"}))
                .unwrap();
        let HubToolCall::RunConnectorTool(args) = no_args else {
            panic!("wrong variant");
        };
        assert_eq!(args.arguments, Value::Null);
        let schema = HubTool::RunConnectorTool.input_schema();
        assert_eq!(schema["properties"]["arguments"]["type"], "object");
        let list = HubToolCall::parse(
            "list_connector_tools",
            json!({"connector": "github_work", "search": "issue"}),
        )
        .unwrap();
        assert!(matches!(
            list,
            HubToolCall::ListConnectorTools(ListConnectorToolsArgs {
                search: Some(_),
                ..
            })
        ));
        assert!(HubToolCall::parse("get_connector_tool", json!({"connector": "c"})).is_err());
        let read = HubToolCall::parse("read_skill", json!({"skill": "pdf-forms"})).unwrap();
        assert_eq!(
            read,
            HubToolCall::ReadSkill(ReadSkillArgs {
                skill: "pdf-forms".into(),
                path: None
            })
        );
        assert_eq!(
            HubToolCall::parse("list_skills", Value::Null).unwrap(),
            HubToolCall::ListSkills(NoArgs {})
        );
    }

    #[test]
    fn management_tools_parse_and_respect_audiences() {
        let update = HubToolCall::parse_for(
            ToolAudience::TuttiCoordinator,
            "update_connection",
            json!({"connection": "github_work", "use_in": {"kind": "tags", "tags": ["work"]},
                   "approvals": {"read": "run_without_asking", "change": "ask_once_per_task", "delete": "ask_every_time"}}),
        )
        .unwrap();
        let HubToolCall::UpdateConnection(args) = update else {
            panic!("wrong variant");
        };
        assert_eq!(
            args.use_in,
            Some(UseIn::Tags {
                tags: vec!["work".into()]
            })
        );
        assert_eq!(
            args.approvals.unwrap().change,
            crate::model::ApprovalRule::AskOncePerTask
        );
        // The channel assistant cannot use Tutti's tools, and the other
        // way round.
        for (audience, tool, args) in [
            (
                ToolAudience::ChannelCoordinator,
                "update_connection",
                json!({"connection": "x"}),
            ),
            (
                ToolAudience::ChannelCoordinator,
                "create_channel",
                json!({"name": "a", "purpose": "b"}),
            ),
            (
                ToolAudience::TuttiCoordinator,
                "set_channel_tags",
                json!({"tags": []}),
            ),
            (
                ToolAudience::TaskSession,
                "connect_service",
                json!({"service": "gmail", "name": "Gmail"}),
            ),
            (ToolAudience::TaskSession, "list_connections", json!({})),
        ] {
            let err = HubToolCall::parse_for(audience, tool, args).unwrap_err();
            assert!(
                matches!(err, ToolCallError::NotAllowed { .. }),
                "{tool}: {err}"
            );
        }
        let calls = [
            (
                "create_channel",
                json!({"name": "Sayso", "purpose": "The Sayso app", "group": "Work", "tags": ["work"],
                "model": {"account_label": "Claude Work", "model": "claude-opus-5-5"}}),
            ),
            (
                "update_channel",
                json!({"channel": "chn_00000000-0000-0000-0000-000000000001", "repos": ["git@github.com:watzon/sayso.git"]}),
            ),
            (
                "archive_channel",
                json!({"channel": "chn_00000000-0000-0000-0000-000000000001"}),
            ),
            (
                "set_default_model",
                json!({"model": {"account_label": "Claude Work", "model": "claude-opus-5-5", "options": {"effort": "high"}}}),
            ),
            (
                "set_worker_tags",
                json!({"worker": "wrk_00000000-0000-0000-0000-000000000006", "tags": ["gpu"]}),
            ),
            (
                "install_tool",
                json!({"worker": "wrk_00000000-0000-0000-0000-000000000006", "tool": "codex"}),
            ),
            (
                "add_account",
                json!({"worker": "wrk_00000000-0000-0000-0000-000000000006", "provider": "claude", "name": "Claude Work"}),
            ),
            (
                "start_sign_in",
                json!({"worker": "wrk_00000000-0000-0000-0000-000000000006", "account": "acc_00000000-0000-0000-0000-000000000009"}),
            ),
            (
                "set_account_extras",
                json!({"worker": "wrk_00000000-0000-0000-0000-000000000006", "account": "acc_00000000-0000-0000-0000-000000000009", "enabled": false}),
            ),
            ("search_connector_catalog", json!({"search": "email"})),
            ("list_connections", json!({})),
            (
                "connect_service",
                json!({"service": "gmail", "name": "Gmail Work", "use_in": {"kind": "every_channel"}}),
            ),
            ("disconnect", json!({"connection": "gmail_work"})),
            (
                "search_skills",
                json!({"search": "pdf", "source": "smithery"}),
            ),
            ("list_installed_skills", Value::Null),
            (
                "add_skill",
                json!({"source": "anthropics/skills", "path": "pdf"}),
            ),
            (
                "update_skill",
                json!({"skill": "pdf-forms", "enabled": true, "accept_new_version": true}),
            ),
            ("remove_skill", json!({"skill": "pdf-forms"})),
            ("update_this_channel", json!({"purpose": "Ship Sayso"})),
            ("set_channel_tags", json!({"tags": ["work"]})),
            (
                "set_channel_connector",
                json!({"connection": "gmail_work", "enabled": false}),
            ),
            (
                "set_channel_skill",
                json!({"skill": "pdf-forms", "enabled": true}),
            ),
            ("request_access", json!({"connection": "github_work"})),
        ];
        for (name, args) in calls {
            let call = HubToolCall::parse(name, args).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(call.tool().name(), name);
        }
        let schema = HubTool::ConnectService.input_schema();
        let text = schema.to_string();
        assert!(text.contains("every_channel"), "{text}");
        assert!(text.contains("ask_once_per_task"), "{text}");
    }

    #[test]
    fn connector_results_have_a_status() {
        let call_id: crate::ids::ConnectorCallId =
            "ccl_00000000-0000-0000-0000-000000000001".parse().unwrap();
        let ran = RunConnectorToolResult::Ran {
            call_id,
            successful: true,
            data: json!({"id": "msg-1"}),
            error: None,
            truncated: false,
        };
        assert_eq!(
            serde_json::to_value(&ran).unwrap(),
            json!({"status": "ran", "call_id": "ccl_00000000-0000-0000-0000-000000000001",
                   "successful": true, "data": {"id": "msg-1"}})
        );
        let waiting = RunConnectorToolResult::WaitingForUser {
            approval_id: "apr_00000000-0000-0000-0000-000000000002".parse().unwrap(),
            message: "The user has not answered yet.".into(),
        };
        let denied = RunConnectorToolResult::Denied {
            call_id,
            message: "The user denied the call.".into(),
        };
        for result in [ran, waiting, denied] {
            let value = serde_json::to_value(&result).unwrap();
            assert!(value["status"].is_string());
            assert_eq!(
                serde_json::from_value::<RunConnectorToolResult>(value).unwrap(),
                result
            );
        }
        let info = ConnectorToolInfo {
            slug: "GMAIL_SEND_EMAIL".into(),
            description: "Send an email.".into(),
            kind: crate::model::ToolKind::Change,
            approval: crate::model::ApprovalRule::AskEveryTime,
        };
        let list = ListConnectorToolsResult {
            connector: "gmail_work".into(),
            service_name: "Gmail".into(),
            tools: vec![info.clone()],
            has_more: true,
        };
        assert_eq!(
            serde_json::from_value::<ListConnectorToolsResult>(
                serde_json::to_value(&list).unwrap()
            )
            .unwrap(),
            list
        );
        let get = GetConnectorToolResult {
            connector: "gmail_work".into(),
            tool: info,
            input_schema: json!({"type": "object"}),
        };
        let value = serde_json::to_value(&get).unwrap();
        assert_eq!(value["slug"], "GMAIL_SEND_EMAIL");
        assert_eq!(
            serde_json::from_value::<GetConnectorToolResult>(value).unwrap(),
            get
        );
        let skills = ListSkillsResult {
            skills: vec![crate::model::SkillContextEntry {
                name: "pdf-forms".into(),
                description: "Fill PDF forms.".into(),
            }],
        };
        assert_eq!(
            serde_json::from_value::<ListSkillsResult>(serde_json::to_value(&skills).unwrap())
                .unwrap(),
            skills
        );
        let read = ReadSkillResult {
            skill: "pdf-forms".into(),
            path: "SKILL.md".into(),
            content: "---\nname: pdf-forms\n---".into(),
            truncated: false,
            files: vec!["scripts/fill.py".into()],
        };
        assert_eq!(
            serde_json::from_value::<ReadSkillResult>(serde_json::to_value(&read).unwrap())
                .unwrap(),
            read
        );
    }

    #[test]
    fn management_results_say_what_happened() {
        let apr: crate::ids::ApprovalId =
            "apr_00000000-0000-0000-0000-000000000002".parse().unwrap();
        let applied = ManagementResult {
            channel_id: Some("chn_00000000-0000-0000-0000-000000000001".parse().unwrap()),
            ..ManagementResult::applied("Created the channel Sayso.")
        };
        assert_eq!(
            serde_json::to_value(&applied).unwrap(),
            json!({"status": "applied", "summary": "Created the channel Sayso.",
                   "channel_id": "chn_00000000-0000-0000-0000-000000000001"})
        );
        let waiting = ManagementResult::waiting_for_user(
            apr,
            "The user must allow this: GitHub Work becomes available in 4 more channels.",
        );
        assert_eq!(waiting.status, ChangeStatus::WaitingForUser);
        assert_eq!(waiting.approval_id, Some(apr));
        let refused = ManagementResult::refused(
            "This channel's scope does not give it Gmail Work. Use request_access.",
        );
        for result in [applied, waiting, refused] {
            let value = serde_json::to_value(&result).unwrap();
            assert_eq!(
                serde_json::from_value::<ManagementResult>(value).unwrap(),
                result
            );
        }
        let lists = ListConnectionsResult {
            connections: vec![],
            off_in_this_channel: vec![],
        };
        assert_eq!(
            serde_json::to_value(&lists).unwrap(),
            json!({"connections": []})
        );
        let skills = ListInstalledSkillsResult {
            skills: vec![],
            off_in_this_channel: vec!["skl_00000000-0000-0000-0000-000000000003".parse().unwrap()],
        };
        assert_eq!(
            serde_json::from_value::<ListInstalledSkillsResult>(
                serde_json::to_value(&skills).unwrap()
            )
            .unwrap(),
            skills
        );
        let search = SearchConnectorCatalogResult { services: vec![] };
        assert_eq!(
            serde_json::from_value::<SearchConnectorCatalogResult>(
                serde_json::to_value(&search).unwrap()
            )
            .unwrap(),
            search
        );
        let entries = SearchSkillsResult { entries: vec![] };
        assert_eq!(
            serde_json::from_value::<SearchSkillsResult>(serde_json::to_value(&entries).unwrap())
                .unwrap(),
            entries
        );
    }

    #[test]
    fn task_counts() {
        let mut counts = TaskCounts::default();
        for status in TaskStatus::ALL {
            counts.add(status);
        }
        assert_eq!(counts.active(), 4);
        assert_eq!(counts.up_next, 1);
    }
}
