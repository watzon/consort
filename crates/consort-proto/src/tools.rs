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
    Sha256Hash, Task, Worker, WorkerState,
};
use crate::status::TaskStatus;
use crate::time::Timestamp;

/// The kind of session that a tool is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAudience {
    /// The coordinator of the Everything channel.
    EverythingCoordinator,
    /// The coordinator of any other channel.
    ChannelCoordinator,
    /// A task session, in any channel.
    TaskSession,
}

impl ToolAudience {
    /// Every audience.
    pub const ALL: [ToolAudience; 3] = [
        Self::EverythingCoordinator,
        Self::ChannelCoordinator,
        Self::TaskSession,
    ];

    /// The audience of a session with this role in a channel of this kind.
    pub fn for_session(role: SessionRole, channel_kind: ChannelKind) -> Self {
        match (role, channel_kind) {
            (SessionRole::Task, _) => Self::TaskSession,
            (SessionRole::Coordinator, ChannelKind::Everything) => Self::EverythingCoordinator,
            (SessionRole::Coordinator, ChannelKind::Standard) => Self::ChannelCoordinator,
        }
    }
}

const EVERY_SESSION: &[ToolAudience] = &ToolAudience::ALL;
const COORDINATORS: &[ToolAudience] = &[
    ToolAudience::EverythingCoordinator,
    ToolAudience::ChannelCoordinator,
];
const EVERYTHING_ONLY: &[ToolAudience] = &[ToolAudience::EverythingCoordinator];
const CHANNEL_COORDINATOR_AND_TASK: &[ToolAudience] =
    &[ToolAudience::ChannelCoordinator, ToolAudience::TaskSession];
const TASK_ONLY: &[ToolAudience] = &[ToolAudience::TaskSession];

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
    /// consequence at most 800.
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
    /// The thread root.
    pub message_id: MessageId,
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
    /// Constraints on the worker: `os`, `labels`, `local_mcp_servers`.
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
}

/// `message_task`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageTaskArgs {
    /// The task.
    pub task_id: TaskId,
    /// The message, in markdown.
    pub body: String,
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
        "List the workers with their operating system, state, labels, harness accounts and models, local MCP servers and repo checkouts. Use this before you choose where a task runs."),
    /// `ask_decision`.
    AskDecision("ask_decision", AskDecisionArgs, EVERY_SESSION,
        "Ask the user a question with two to four options. Each option has a label and its consequence. This does not wait: end your turn. The answer arrives as input to a later turn."),
    /// `search`.
    Search("search", SearchArgs, EVERY_SESSION,
        "Search messages, memory notes and task titles."),
    // Coordinators.
    /// `post_message`.
    PostMessage("post_message", PostMessageArgs, COORDINATORS,
        "Post a message in the channel timeline. You can post several short messages in one turn."),
    /// `reply`.
    Reply("reply", ReplyArgs, COORDINATORS,
        "Reply in the thread of a message. Set in_timeline to also show the reply in the timeline."),
    /// `update_message`.
    UpdateMessage("update_message", UpdateMessageArgs, COORDINATORS,
        "Change the text of one of your own messages."),
    /// `react`.
    React("react", ReactArgs, COORDINATORS,
        "Add an emoji reaction to a message."),
    /// `no_reply_needed`.
    NoReplyNeeded("no_reply_needed", NoArgs, COORDINATORS,
        "End the turn without a visible reply. Use it when the events need no answer."),
    /// `fetch_timeline`.
    FetchTimeline("fetch_timeline", FetchTimelineArgs, COORDINATORS,
        "Get older messages from the channel timeline, newest first."),
    /// `fetch_thread`.
    FetchThread("fetch_thread", FetchThreadArgs, COORDINATORS,
        "Get a thread: the root message and its replies."),
    /// `memory_write`.
    MemoryWrite("memory_write", MemoryWriteArgs, COORDINATORS,
        "Write a memory note in your own memory. Give the sha256 that you read, or not_exists for a new note. If the note changed since you read it, the write fails with stale_write: read it again and retry."),
    /// `memory_delete`.
    MemoryDelete("memory_delete", MemoryDeleteArgs, COORDINATORS,
        "Delete a memory note in your own memory. Give the sha256 that you read."),
    /// `files_write`.
    FilesWrite("files_write", FilesWriteArgs, COORDINATORS,
        "Write a text file in the channel files."),
    /// `start_task`.
    StartTask("start_task", StartTaskArgs, COORDINATORS,
        "Start a task. The task runs in its own session on a worker and reports in its own thread. Placement uses the channel defaults for anything that you leave out."),
    /// `message_task`.
    MessageTask("message_task", MessageTaskArgs, COORDINATORS,
        "Send a message to a task. It starts a new turn, or steers the running turn when the harness can steer."),
    /// `list_tasks`.
    ListTasks("list_tasks", ListTasksArgs, COORDINATORS,
        "List the tasks in this channel."),
    /// `read_task`.
    ReadTask("read_task", ReadTaskArgs, COORDINATORS,
        "Read a task: its status, its recent session items and its review summary."),
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
    /// `suggest_memory`.
    SuggestMemory("suggest_memory", SuggestMemoryArgs, CHANNEL_COORDINATOR_AND_TASK,
        "Suggest a note for memory. A task sends it to the channel coordinator. A channel coordinator sends it to the Everything coordinator as a global memory suggestion. The coordinator that gets it decides whether to keep it."),
    // The Everything coordinator.
    /// `list_channels`.
    ListChannels("list_channels", NoArgs, EVERYTHING_ONLY,
        "List the channels with their purpose and task counts."),
    /// `read_channel`.
    ReadChannel("read_channel", ReadChannelArgs, EVERYTHING_ONLY,
        "Read a channel: its timeline, its tasks and its memory index."),
    /// `message_channel`.
    MessageChannel("message_channel", MessageChannelArgs, EVERYTHING_ONLY,
        "Post in a channel as a message from Everything. This wakes the coordinator of that channel."),
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
    /// The user's labels and the inventory labels.
    pub labels: Vec<String>,
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
            labels: worker.all_labels().into_iter().map(str::to_owned).collect(),
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
    },
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
    /// The replies, oldest first.
    pub replies: Vec<Message>,
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
    /// `everything` or `standard`.
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
    use serde_json::json;

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
        ];
        let everything_only = ["list_channels", "read_channel", "message_channel"];
        let task_only = ["post_update", "submit_for_review", "publish_artifact"];

        let everything = names(ToolAudience::EverythingCoordinator);
        let channel = names(ToolAudience::ChannelCoordinator);
        let task = names(ToolAudience::TaskSession);

        for name in every {
            assert!(everything.contains(&name.to_owned()));
            assert!(channel.contains(&name.to_owned()));
            assert!(task.contains(&name.to_owned()));
        }
        for name in coordinator {
            assert!(everything.contains(&name.to_owned()), "{name}");
            assert!(channel.contains(&name.to_owned()), "{name}");
            assert!(!task.contains(&name.to_owned()), "{name}");
        }
        for name in everything_only {
            assert!(everything.contains(&name.to_owned()));
            assert!(!channel.contains(&name.to_owned()));
            assert!(!task.contains(&name.to_owned()));
        }
        for name in task_only {
            assert!(task.contains(&name.to_owned()));
            assert!(!everything.contains(&name.to_owned()));
            assert!(!channel.contains(&name.to_owned()));
        }
        // suggest_memory: channel coordinators and tasks, not Everything.
        assert!(channel.contains(&"suggest_memory".to_owned()));
        assert!(task.contains(&"suggest_memory".to_owned()));
        assert!(!everything.contains(&"suggest_memory".to_owned()));

        assert_eq!(
            everything.len(),
            every.len() + coordinator.len() + everything_only.len()
        );
        assert_eq!(channel.len(), every.len() + coordinator.len() + 1);
        assert_eq!(task.len(), every.len() + task_only.len() + 1);
    }

    #[test]
    fn audience_for_session() {
        assert_eq!(
            ToolAudience::for_session(SessionRole::Coordinator, ChannelKind::Everything),
            ToolAudience::EverythingCoordinator
        );
        assert_eq!(
            ToolAudience::for_session(SessionRole::Coordinator, ChannelKind::Standard),
            ToolAudience::ChannelCoordinator
        );
        assert_eq!(
            ToolAudience::for_session(SessionRole::Task, ChannelKind::Everything),
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
        assert_eq!(required, ["question", "options"]);
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
        assert_eq!(none["properties"], json!({}));
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
        assert_eq!(none, HubToolCall::NoReplyNeeded(NoArgs {}));

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
    fn task_counts() {
        let mut counts = TaskCounts::default();
        for status in TaskStatus::ALL {
            counts.add(status);
        }
        assert_eq!(counts.active(), 4);
        assert_eq!(counts.up_next, 1);
    }
}
