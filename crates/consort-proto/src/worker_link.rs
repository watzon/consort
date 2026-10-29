//! The worker link: messages between a worker and the hub.
//!
//! [`WorkerToHub`] has what a worker sends, and [`HubToWorker`] has the hub
//! commands. Every hub command is a request. Its envelope `id` is the command
//! ID, and the worker answers with the result type, or with an empty body
//! when the result is "None" in protocol.md. The worker stores each result
//! for [`COMMAND_RESULT_TTL_MS`] and answers a repeated command ID with the
//! stored result.
//!
//! [`SessionEvent`] is the vocabulary that every harness adapter translates
//! its harness's events into.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::{WorkerHello, WorkerHelloResult};
use crate::frame::{Empty, message_family};
use crate::ids::{
    AccountId, ChannelId, ItemId, NativeRef, RepoId, RequestId, SessionId, SignInId, TaskId, TurnId,
};
use crate::model::{
    AcpAccountSpec, ApprovalDetails, ApprovalMode, BlobRef, ConnectorContextEntry, FileRef,
    Inventory, Isolation, ModelOptions, RequestOption, SecretEnv, SessionItem, SessionRole,
    SessionState, Sha256Hash, SignInMethod, SignInState, SkillBundle, SkillBundleRef,
    SkillContextEntry, ToolId, Usage, ValidationError, render_connectors_block,
    render_skills_block,
};
use crate::time::Timestamp;
use crate::tools::{ToolCallResult, ToolDefinition};

/// The most events in one `session.events` batch.
pub const MAX_EVENTS_PER_BATCH: usize = 256;
/// The most bytes of events in one `session.events` batch (256 KiB).
pub const MAX_BATCH_BYTES: usize = 256 * 1024;
/// The worker merges `output.delta` events into at most one event per this
/// many milliseconds.
pub const OUTPUT_DELTA_INTERVAL_MS: u64 = 250;
/// The worker sends a WebSocket ping this often, in seconds.
pub const PING_INTERVAL_SECS: u64 = 20;
/// The hub marks a worker offline after this many seconds with no frames.
pub const OFFLINE_AFTER_SECS: u64 = 60;
/// How long the worker keeps a command result, in milliseconds (8 days):
/// the 7 days that the hub keeps a queued command
/// ([`QUEUED_COMMAND_TTL_MS`]), and one more day. So a command that the hub
/// sends again always finds its stored result and never runs twice.
pub const COMMAND_RESULT_TTL_MS: i64 = QUEUED_COMMAND_TTL_MS + 24 * 60 * 60 * 1_000;

// A stored result must outlive every resend of its command.
const _: () = assert!(COMMAND_RESULT_TTL_MS > QUEUED_COMMAND_TTL_MS);
/// How long the hub keeps a queued command for an offline worker, in
/// milliseconds (7 days).
pub const QUEUED_COMMAND_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

/// The kind of a harness request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// A request for permission. The hub makes an approval.
    Approval,
    /// A question to the user. The hub makes a decision with `request_id`.
    Question,
}

/// The details of a harness question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionDetails {
    /// The question.
    pub question: String,
    /// More context, in markdown.
    pub context: Option<String>,
    /// Whether the user may answer with free text.
    #[serde(default)]
    pub allow_free_text: bool,
}

/// The `kind` and `details` of a `request.opened` event. It serializes as
/// two fields: `"kind": "approval", "details": {"kind": "command", ...}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "details", rename_all = "snake_case")]
pub enum RequestBody {
    /// A request for permission.
    Approval(ApprovalDetails),
    /// A question.
    Question(QuestionDetails),
}

impl RequestBody {
    /// The request kind.
    pub fn kind(&self) -> RequestKind {
        match self {
            Self::Approval(_) => RequestKind::Approval,
            Self::Question(_) => RequestKind::Question,
        }
    }
}

/// The body of a `request.opened` event. The harness waits for
/// `session.respond`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestOpened {
    /// The harness request ID.
    pub request_id: RequestId,
    /// One line for the card and the push notification.
    pub summary: String,
    /// `kind` and `details`.
    #[serde(flatten)]
    pub body: RequestBody,
    /// The options from the harness.
    #[serde(default)]
    pub options: Vec<RequestOption>,
}

/// How a harness request closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    /// The user allowed the action.
    Allowed,
    /// The user denied the action.
    Denied,
    /// The user answered the question.
    Answered,
    /// The harness withdrew the request, for example after an interrupt.
    Canceled,
    /// Nobody answered in time.
    Expired,
}

/// Why a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEndReason {
    /// The session finished its work.
    Completed,
    /// `session.interrupt` stopped it.
    Interrupted,
    /// The harness failed.
    Failed,
}

/// One event from a harness session, in the shape of protocol.md. Serialized
/// with a `type` field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEvent {
    /// The session state changed.
    #[serde(rename = "state")]
    State {
        /// The new state.
        state: SessionState,
    },
    /// The harness's own session ID is known.
    #[serde(rename = "native_ref")]
    NativeRef {
        /// The harness's session ID.
        #[serde(rename = "ref")]
        native_ref: NativeRef,
    },
    /// A turn started.
    #[serde(rename = "turn.started")]
    TurnStarted {
        /// The turn.
        turn_id: TurnId,
    },
    /// A full session item. Replaces any earlier item with the same `id`.
    #[serde(rename = "item")]
    Item(SessionItem),
    /// Command output. The worker merges deltas into at most one event every
    /// 250 ms.
    #[serde(rename = "output.delta")]
    OutputDelta {
        /// The command item.
        item_id: ItemId,
        /// New output text.
        text: String,
    },
    /// The harness asks for an approval or an answer.
    #[serde(rename = "request.opened")]
    RequestOpened(RequestOpened),
    /// A harness request closed.
    #[serde(rename = "request.closed")]
    RequestClosed {
        /// The request.
        request_id: RequestId,
        /// How it closed.
        outcome: RequestOutcome,
    },
    /// Token totals for the session.
    #[serde(rename = "usage")]
    Usage(Usage),
    /// A turn ended.
    #[serde(rename = "turn.ended")]
    TurnEnded {
        /// The turn.
        turn_id: TurnId,
        /// Why it ended.
        reason: TurnEndReason,
        /// The error text, when the turn failed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// The worker opened the session again after the worker restarted. A
    /// turn or a request that was open before the restart is gone.
    #[serde(rename = "reopened")]
    Reopened,
    /// An error that is not tied to an item.
    #[serde(rename = "error")]
    Error {
        /// A short code from the adapter, for example `harness_exited`.
        code: String,
        /// The error text.
        message: String,
    },
}

/// A session event with its sequence number and time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SequencedEvent {
    /// The number of the event in its session, from 1 with no gaps.
    pub seq: u64,
    /// When the worker saw the event.
    pub at: Timestamp,
    /// The event.
    pub event: SessionEvent,
}

/// The highest contiguous sequence number after a batch, given the highest
/// contiguous number that is already stored. Events at or below `stored` are
/// repeats. A gap stops the count. The hub stores the events up to the
/// result and answers with it in [`SessionEventsAck`].
pub fn contiguous_seq(stored: u64, batch: &[SequencedEvent]) -> u64 {
    let mut seqs: Vec<u64> = batch
        .iter()
        .map(|e| e.seq)
        .filter(|s| *s > stored)
        .collect();
    seqs.sort_unstable();
    seqs.dedup();
    let mut last = stored;
    for seq in seqs {
        if seq == last + 1 {
            last = seq;
        } else {
            break;
        }
    }
    last
}

/// `session.events`: a batch of events for one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEvents {
    /// The session.
    pub session_id: SessionId,
    /// At most [`MAX_EVENTS_PER_BATCH`] events, in `seq` order.
    pub events: Vec<SequencedEvent>,
}

/// The response to [`SessionEvents`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEventsAck {
    /// The session.
    pub session_id: SessionId,
    /// The highest contiguous `seq` that the hub stored. The worker deletes
    /// the events up to this number from its spool.
    pub last_seq: u64,
}

/// `tool.list`: ask for the hub tools of a session's role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolList {
    /// The session.
    pub session_id: SessionId,
}

/// The response to [`ToolList`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolListResult {
    /// The tools.
    pub tools: Vec<ToolDefinition>,
}

/// `tool.call`: a hub tool call from a session's harness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The session.
    pub session_id: SessionId,
    /// The MCP call ID that the worker chose.
    pub call_id: String,
    /// The tool name.
    pub name: String,
    /// The arguments, as the harness sent them.
    #[serde(default)]
    pub arguments: Value,
}

/// `sign_in.update`: the new state of a sign-in that the hub started with
/// `sign_in.start`. An event, with no response. The hub makes a feed upsert
/// of the sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInUpdate {
    /// The sign-in.
    pub sign_in_id: SignInId,
    /// The new state.
    pub state: SignInState,
}

/// `skill_bundle.get`: the file list of one skill version, by its content
/// hash. The hub answers only for a hash that a `session.start` to this
/// worker named, and `not_found` otherwise. The worker then downloads each
/// file with `GET /v1/blobs/{sha256}` and checks the content hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBundleGet {
    /// The content hash from [`SkillBundleRef`].
    pub content_hash: Sha256Hash,
}

message_family! {
    /// Everything that a worker sends to the hub after authentication.
    pub enum WorkerToHub {
        /// The first message after authentication.
        Hello("hello", WorkerHello => WorkerHelloResult),
        /// A full inventory. No response.
        Inventory("inventory", Inventory),
        /// A batch of session events.
        SessionEvents("session.events", SessionEvents => SessionEventsAck),
        /// The hub tools for a session.
        ToolList("tool.list", ToolList => ToolListResult),
        /// A hub tool call.
        ToolCall("tool.call", ToolCall => ToolCallResult),
        /// A sign-in changed. No response.
        SignInUpdate("sign_in.update", SignInUpdate),
        /// The files of a skill version.
        SkillBundleGet("skill_bundle.get", SkillBundleGet => SkillBundle),
    }
}

/// One repo in a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRepo {
    /// The repo.
    pub repo_id: RepoId,
    /// The git remote URL.
    pub url: String,
    /// The branch to check out, when not the checkout's `HEAD`.
    pub branch: Option<String>,
    /// The worker's checkout of this repo, when it has one.
    pub checkout_path: Option<String>,
}

/// The workspace of a new session. With no repos, the workspace is an empty
/// folder in the worker's `workspaces/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    /// The repos.
    #[serde(default)]
    pub repos: Vec<WorkspaceRepo>,
    /// The isolation.
    pub isolation: Isolation,
    /// Copy uncommitted changes from the checkout into the worktree. Allowed
    /// only with `worktree`.
    #[serde(default)]
    pub carry_uncommitted: bool,
}

impl WorkspaceSpec {
    /// Checks that `carry_uncommitted` is set only with `worktree`, and that
    /// `in_place` has a checkout for each repo.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.carry_uncommitted && self.isolation != Isolation::Worktree {
            return Err(ValidationError::new(
                "workspace.carry_uncommitted",
                "is allowed only with worktree isolation",
            ));
        }
        if self.isolation != Isolation::Clone
            && let Some(i) = self.repos.iter().position(|r| r.checkout_path.is_none())
        {
            return Err(ValidationError::new(
                format!("workspace.repos[{i}].checkout_path"),
                "is needed for in_place and worktree isolation",
            ));
        }
        Ok(())
    }
}

/// The input of a turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnInput {
    /// The text.
    pub text: String,
    /// Attached blobs. The worker downloads them before it starts the turn.
    #[serde(default)]
    pub attachments: Vec<BlobRef>,
}

/// Resume an earlier harness session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeSpec {
    /// The harness's own session ID.
    pub native_ref: NativeRef,
}

/// The connectors and skills of a session: its channel's effective set
/// when the session starts. The hub fills it, and the worker renders the
/// context blocks with [`SessionExtensions::prompt_blocks`] and delivers the
/// skills. A change to the effective set reaches the next session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionExtensions {
    /// One entry per connection that the session can use, for the
    /// "Connectors in this channel" block.
    #[serde(default)]
    pub connectors: Vec<ConnectorContextEntry>,
    /// The skills that the worker writes into the session's skill folder.
    #[serde(default)]
    pub skills: Vec<SkillBundleRef>,
    /// The name and description of each skill, for the "Skills in this
    /// channel" block, in the order of `skills`.
    #[serde(default)]
    pub skill_context: Vec<SkillContextEntry>,
}

impl SessionExtensions {
    /// Whether the session has no connector and no skill.
    pub fn is_empty(&self) -> bool {
        self.connectors.is_empty() && self.skills.is_empty() && self.skill_context.is_empty()
    }

    /// The context blocks to append to the system prompt, with a blank line
    /// between them. `native_skills` is true when the harness loads the
    /// skills itself. Empty when there is nothing to say.
    pub fn prompt_blocks(&self, native_skills: bool) -> String {
        [
            render_connectors_block(&self.connectors),
            render_skills_block(&self.skill_context, native_skills),
        ]
        .into_iter()
        .filter(|b| !b.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
    }
}

/// `session.start`: open a harness session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionStart {
    /// The new session ID.
    pub session_id: SessionId,
    /// Coordinator or task.
    pub role: SessionRole,
    /// The channel.
    pub channel_id: ChannelId,
    /// The task, for a task session. `workspace.release` names it.
    pub task_id: Option<TaskId>,
    /// The harness account on this worker.
    pub account_id: AccountId,
    /// The model ID.
    pub model: String,
    /// The model options.
    #[serde(default)]
    pub options: ModelOptions,
    /// The requested approval mode. The worker caps it at its policy.
    pub approval_mode: ApprovalMode,
    /// The workspace.
    pub workspace: WorkspaceSpec,
    /// Text to append to the harness's system prompt.
    #[serde(default)]
    pub system_prompt_append: String,
    /// Local MCP servers that the session may use, by name.
    #[serde(default)]
    pub allowed_local_mcp_servers: Vec<String>,
    /// The channel files. The worker puts a read-only copy in
    /// `<workspace>/.channel/files/`.
    #[serde(default)]
    pub channel_files: Vec<FileRef>,
    /// The connectors and skills of the session. Empty from an older hub.
    #[serde(default, skip_serializing_if = "SessionExtensions::is_empty")]
    pub extensions: SessionExtensions,
    /// Resume this harness session instead of starting a new one.
    pub resume: Option<ResumeSpec>,
    /// The first turn.
    pub turn: Option<TurnInput>,
}

/// The result of [`SessionStart`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStartResult {
    /// The workspace path on the worker.
    pub workspace_path: String,
    /// The harness's session ID, when it is known already.
    pub native_ref: Option<NativeRef>,
}

/// `session.turn`: start a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTurn {
    /// The session.
    pub session_id: SessionId,
    /// The input.
    pub turn: TurnInput,
}

/// The result of [`SessionTurn`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTurnResult {
    /// The new turn.
    pub turn_id: TurnId,
}

/// `session.steer`: add input to the running turn. The worker answers
/// `policy_denied` if the harness cannot steer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSteer {
    /// The session.
    pub session_id: SessionId,
    /// The input.
    pub turn: TurnInput,
}

/// `session.interrupt`: stop the running turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInterrupt {
    /// The session.
    pub session_id: SessionId,
}

/// The answer to a harness request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestAnswer {
    /// The chosen option's harness ID.
    pub option_id: Option<String>,
    /// Free text, for a question.
    pub text: Option<String>,
}

/// `session.respond`: answer a harness request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRespond {
    /// The session.
    pub session_id: SessionId,
    /// The request.
    pub request_id: RequestId,
    /// The answer.
    pub answer: RequestAnswer,
}

/// `session.stop`: close a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStop {
    /// The session.
    pub session_id: SessionId,
    /// Why, for logs.
    pub reason: String,
}

/// `workspace.release`: the task no longer needs its workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRelease {
    /// The task.
    pub task_id: TaskId,
    /// Delete the worktree or clone. A checkout used in place is never
    /// deleted.
    pub delete: bool,
}

/// `item.detail`: upload the full output or diff of an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDetail {
    /// The session.
    pub session_id: SessionId,
    /// The item.
    pub item_id: ItemId,
}

/// The result of [`ItemDetail`]: a blob that the worker uploaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDetailResult {
    /// The blob hash.
    pub sha256: Sha256Hash,
    /// The size in bytes.
    pub size: u64,
    /// The media type, `text/plain` for output or `text/x-diff` for a diff.
    pub mime: String,
}

/// `inventory.refresh`: send a new inventory now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryRefresh {}

/// `account.create`: make an account with its own profile folder. The hub
/// chooses the account ID. The worker writes `env` to `env.json` in the
/// profile folder, adds the account to `worker.toml`, and sends a new
/// inventory.
///
/// `Debug` shows the names in `env`, never the values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountCreateCommand {
    /// The new account.
    pub account_id: AccountId,
    /// The harness: `claude`, `codex`, `opencode`, `gemini` or `acp`.
    pub harness: String,
    /// The account name.
    pub label: String,
    /// The ACP agent, for an `acp` account.
    #[serde(default)]
    pub acp: Option<AcpAccountSpec>,
    /// Secret variables, such as an API key.
    #[serde(default)]
    pub env: SecretEnv,
}

/// `account.update`: rename an account or change its secret variables.
///
/// `Debug` shows the names in `env_set`, never the values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountUpdateCommand {
    /// The account.
    pub account_id: AccountId,
    /// The new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Secret variables to add or replace.
    #[serde(default)]
    pub env_set: SecretEnv,
    /// Secret variables to remove, by name.
    #[serde(default)]
    pub env_remove: Vec<String>,
    /// Turn the provider extras of the account on or off. The worker
    /// stores it in `worker.toml` and uses it from the next session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extras_enabled: Option<bool>,
}

/// `account.remove`: sign out where possible and delete the profile folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRemoveCommand {
    /// The account.
    pub account_id: AccountId,
}

/// `account.sign_out`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSignOutCommand {
    /// The account.
    pub account_id: AccountId,
}

/// `sign_in.start`: run a sign-in flow. The worker reports each state with
/// a `sign_in.update` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInStartCommand {
    /// The sign-in that the hub made.
    pub sign_in_id: SignInId,
    /// The account.
    pub account_id: AccountId,
    /// The method.
    pub method: SignInMethod,
}

/// `sign_in.code`: the code that the user pasted, for a sign-in in
/// `needs_code`. The worker writes it to the harness and does not store it.
///
/// `Debug` does not show the code.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInCodeCommand {
    /// The sign-in.
    pub sign_in_id: SignInId,
    /// The code.
    pub code: String,
}

impl std::fmt::Debug for SignInCodeCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignInCodeCommand")
            .field("sign_in_id", &self.sign_in_id)
            .field("code", &crate::model::Redacted)
            .finish()
    }
}

/// `sign_in.cancel`: stop a sign-in flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInCancelCommand {
    /// The sign-in.
    pub sign_in_id: SignInId,
}

/// `tools.check`: check the managed tools for updates now. The result comes
/// in the next inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsCheck {}

/// `tools.install`: install or update one managed tool. The progress comes
/// in the inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsInstall {
    /// The tool.
    pub tool: ToolId,
}

message_family! {
    /// The hub commands to a worker. Each is a request.
    pub enum HubToWorker {
        /// Open a harness session.
        SessionStart("session.start", SessionStart => SessionStartResult),
        /// Start a turn.
        SessionTurn("session.turn", SessionTurn => SessionTurnResult),
        /// Add input to the running turn.
        SessionSteer("session.steer", SessionSteer => Empty),
        /// Stop the running turn.
        SessionInterrupt("session.interrupt", SessionInterrupt => Empty),
        /// Answer a harness request.
        SessionRespond("session.respond", SessionRespond => Empty),
        /// Close a session.
        SessionStop("session.stop", SessionStop => Empty),
        /// Release a task's workspace.
        WorkspaceRelease("workspace.release", WorkspaceRelease => Empty),
        /// Upload an item's full data.
        ItemDetail("item.detail", ItemDetail => ItemDetailResult),
        /// Send a new inventory.
        InventoryRefresh("inventory.refresh", InventoryRefresh => Empty),
        /// Make an account.
        AccountCreate("account.create", AccountCreateCommand => Empty),
        /// Rename an account or change its secret variables.
        AccountUpdate("account.update", AccountUpdateCommand => Empty),
        /// Remove an account.
        AccountRemove("account.remove", AccountRemoveCommand => Empty),
        /// Sign an account out.
        AccountSignOut("account.sign_out", AccountSignOutCommand => Empty),
        /// Start a sign-in flow.
        SignInStart("sign_in.start", SignInStartCommand => Empty),
        /// Send the code of a sign-in.
        SignInCode("sign_in.code", SignInCodeCommand => Empty),
        /// Cancel a sign-in flow.
        SignInCancel("sign_in.cancel", SignInCancelCommand => Empty),
        /// Check the managed tools for updates.
        ToolsCheck("tools.check", ToolsCheck => Empty),
        /// Install or update a managed tool.
        ToolsInstall("tools.install", ToolsInstall => Empty),
    }
}

impl HubToWorker {
    /// The session that the command is for, if any.
    pub fn session_id(&self) -> Option<SessionId> {
        match self {
            Self::SessionStart(c) => Some(c.session_id),
            Self::SessionTurn(c) => Some(c.session_id),
            Self::SessionSteer(c) => Some(c.session_id),
            Self::SessionInterrupt(c) => Some(c.session_id),
            Self::SessionRespond(c) => Some(c.session_id),
            Self::SessionStop(c) => Some(c.session_id),
            Self::ItemDetail(c) => Some(c.session_id),
            Self::WorkspaceRelease(_)
            | Self::InventoryRefresh(_)
            | Self::AccountCreate(_)
            | Self::AccountUpdate(_)
            | Self::AccountRemove(_)
            | Self::AccountSignOut(_)
            | Self::SignInStart(_)
            | Self::SignInCode(_)
            | Self::SignInCancel(_)
            | Self::ToolsCheck(_)
            | Self::ToolsInstall(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Frame, FrameId, MessageFamily, Request};
    use crate::model::{
        AssistantMessagePayload, ItemPayload, ItemStatus, RequestOptionKind, WorkerPolicy,
    };
    use serde_json::json;

    fn ses() -> SessionId {
        "ses_00000000-0000-0000-0000-000000000001".parse().unwrap()
    }

    fn every_event() -> Vec<SessionEvent> {
        vec![
            SessionEvent::State {
                state: SessionState::Active,
            },
            SessionEvent::NativeRef {
                native_ref: NativeRef::new("thread-1"),
            },
            SessionEvent::TurnStarted {
                turn_id: TurnId::new("t1"),
            },
            SessionEvent::Item(SessionItem {
                id: "itm_00000000-0000-0000-0000-000000000003".parse().unwrap(),
                session_id: ses(),
                turn_id: TurnId::new("t1"),
                payload: ItemPayload::AssistantMessage(AssistantMessagePayload {
                    text: "Done.".into(),
                }),
                status: ItemStatus::Completed,
                created_at: Timestamp::from_millis(5),
                updated_at: Timestamp::from_millis(6),
            }),
            SessionEvent::OutputDelta {
                item_id: "itm_00000000-0000-0000-0000-000000000004".parse().unwrap(),
                text: "line\n".into(),
            },
            SessionEvent::RequestOpened(RequestOpened {
                request_id: RequestId::new("r1"),
                summary: "Run cargo test".into(),
                body: RequestBody::Approval(ApprovalDetails::Command {
                    command: "cargo test".into(),
                    cwd: Some("/w".into()),
                }),
                options: vec![RequestOption {
                    id: "allow".into(),
                    label: "Allow once".into(),
                    kind: RequestOptionKind::AllowOnce,
                    description: None,
                }],
            }),
            SessionEvent::RequestOpened(RequestOpened {
                request_id: RequestId::new("r2"),
                summary: "Which database?".into(),
                body: RequestBody::Question(QuestionDetails {
                    question: "Which database?".into(),
                    context: None,
                    allow_free_text: true,
                }),
                options: vec![],
            }),
            SessionEvent::RequestClosed {
                request_id: RequestId::new("r1"),
                outcome: RequestOutcome::Allowed,
            },
            SessionEvent::Usage(Usage {
                input_tokens: 10,
                output_tokens: 20,
                cached_tokens: 5,
                cost_usd: Some(0.25),
            }),
            SessionEvent::TurnEnded {
                turn_id: TurnId::new("t1"),
                reason: TurnEndReason::Failed,
                error: Some("harness exited".into()),
            },
            SessionEvent::Error {
                code: "harness_exited".into(),
                message: "Codex exited with code 1.".into(),
            },
        ]
    }

    #[test]
    fn every_session_event_round_trips() {
        let types = [
            "state",
            "native_ref",
            "turn.started",
            "item",
            "output.delta",
            "request.opened",
            "request.opened",
            "request.closed",
            "usage",
            "turn.ended",
            "error",
        ];
        for (event, ty) in every_event().into_iter().zip(types) {
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(value["type"], ty);
            let back: SessionEvent = serde_json::from_value(value).unwrap();
            assert_eq!(back, event);
        }
    }

    #[test]
    fn session_event_shapes_match_protocol() {
        let events = every_event();
        assert_eq!(
            serde_json::to_value(&events[1]).unwrap(),
            json!({"type": "native_ref", "ref": "thread-1"})
        );
        assert_eq!(
            serde_json::to_value(&events[5]).unwrap(),
            json!({
                "type": "request.opened",
                "request_id": "r1",
                "summary": "Run cargo test",
                "kind": "approval",
                "details": {"kind": "command", "command": "cargo test", "cwd": "/w"},
                "options": [{"id": "allow", "label": "Allow once", "kind": "allow_once", "description": null}]
            })
        );
        let item = serde_json::to_value(&events[3]).unwrap();
        assert_eq!(item["type"], "item");
        assert_eq!(item["kind"], "assistant_message");
        assert_eq!(item["payload"], json!({"text": "Done."}));
        assert_eq!(
            serde_json::to_value(&events[8]).unwrap(),
            json!({"type": "usage", "input_tokens": 10, "output_tokens": 20, "cached_tokens": 5, "cost_usd": 0.25})
        );
        let ended = SessionEvent::TurnEnded {
            turn_id: TurnId::new("t2"),
            reason: TurnEndReason::Completed,
            error: None,
        };
        assert_eq!(
            serde_json::to_value(&ended).unwrap(),
            json!({"type": "turn.ended", "turn_id": "t2", "reason": "completed"})
        );
    }

    #[test]
    fn session_events_frame_shape() {
        let batch = SessionEvents {
            session_id: ses(),
            events: vec![SequencedEvent {
                seq: 1,
                at: Timestamp::from_millis(1_000),
                event: SessionEvent::State {
                    state: SessionState::Starting,
                },
            }],
        };
        let frame = Frame::request(FrameId::new("c1"), &batch).unwrap();
        let value: Value = serde_json::from_str(&frame.to_text().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({
                "v": 1, "id": "c1", "type": "session.events",
                "body": {
                    "session_id": "ses_00000000-0000-0000-0000-000000000001",
                    "events": [{"seq": 1, "at": 1000, "event": {"type": "state", "state": "starting"}}]
                }
            })
        );
        let Frame::Request(req) = Frame::parse(&frame.to_text().unwrap()).unwrap() else {
            panic!("expected a request");
        };
        assert_eq!(
            req.decode::<WorkerToHub>().unwrap(),
            WorkerToHub::SessionEvents(batch)
        );
    }

    #[test]
    fn worker_to_hub_round_trips() {
        let messages: Vec<WorkerToHub> = vec![
            WorkerHello {
                worker_id: "wrk_00000000-0000-0000-0000-000000000001".parse().unwrap(),
                version: "0.1.0".into(),
                protocol_versions: vec![1],
                sessions: vec![crate::auth::OpenSession {
                    session_id: ses(),
                    last_seq: 12,
                    state: SessionState::Idle,
                }],
            }
            .into(),
            Inventory {
                hash: String::new(),
                hostname: "mbp".into(),
                os: crate::model::Os::Macos,
                arch: "aarch64".into(),
                tags: vec!["macos".into()],
                accounts: vec![],
                local_mcp_servers: vec![],
                checkouts: vec![],
                policy: WorkerPolicy::default(),
                load: Default::default(),
                tools: vec![],
            }
            .with_hash()
            .into(),
            ToolList { session_id: ses() }.into(),
            ToolCall {
                session_id: ses(),
                call_id: "1".into(),
                name: "post_update".into(),
                arguments: json!({"body": "Halfway."}),
            }
            .into(),
            SignInUpdate {
                sign_in_id: "sgn_00000000-0000-0000-0000-000000000002".parse().unwrap(),
                state: SignInState::NeedsCode {
                    url: "https://claude.ai/oauth/authorize".into(),
                },
            }
            .into(),
            SkillBundleGet {
                content_hash: "c".repeat(64).parse().unwrap(),
            }
            .into(),
        ];
        assert_eq!(messages.len(), WorkerToHub::TYPES.len() - 1);
        for message in messages {
            let frame = Frame::family_request(FrameId::generate(), &message).unwrap();
            let Frame::Request(req) = Frame::parse(&frame.to_text().unwrap()).unwrap() else {
                panic!("expected a request");
            };
            assert_eq!(req.kind, message.type_name());
            assert_eq!(req.decode::<WorkerToHub>().unwrap(), message);
        }
        assert!(WorkerToHub::TYPES.contains(&"tool.call"));
    }

    fn extensions() -> SessionExtensions {
        SessionExtensions {
            connectors: vec![ConnectorContextEntry {
                handle: "gmail_work".into(),
                service_name: "Gmail".into(),
                identity: Some("chris@work.example".into()),
                needs_sign_in: false,
            }],
            skills: vec![SkillBundleRef {
                skill_id: "skl_00000000-0000-0000-0000-000000000005".parse().unwrap(),
                name: "pdf-forms".into(),
                content_hash: "c".repeat(64).parse().unwrap(),
            }],
            skill_context: vec![SkillContextEntry {
                name: "pdf-forms".into(),
                description: "Fill PDF forms.".into(),
            }],
        }
    }

    #[test]
    fn session_extensions_render_and_default() {
        let ext = extensions();
        assert_eq!(
            ext.prompt_blocks(true),
            "Connectors in this channel:
- gmail_work: Gmail, chris@work.example
To use one, call list_connector_tools with its handle, then run_connector_tool.

Skills in this channel:
- pdf-forms: Fill PDF forms.
They are installed as skills. Use one when the task fits it."
        );
        assert_eq!(SessionExtensions::default().prompt_blocks(false), "");
        let only_skills = SessionExtensions {
            connectors: vec![],
            ..ext.clone()
        };
        assert!(
            only_skills
                .prompt_blocks(false)
                .starts_with("Skills in this channel:")
        );
        // A `session.start` from an older hub has no extensions, and an
        // empty set is left out of the wire.
        let start = SessionStart {
            session_id: ses(),
            role: SessionRole::Coordinator,
            channel_id: ChannelId::new(),
            task_id: None,
            account_id: AccountId::new(),
            model: "opus".into(),
            options: ModelOptions::new(),
            approval_mode: ApprovalMode::Ask,
            workspace: WorkspaceSpec {
                repos: vec![],
                isolation: Isolation::Clone,
                carry_uncommitted: false,
            },
            system_prompt_append: String::new(),
            allowed_local_mcp_servers: vec![],
            channel_files: vec![],
            extensions: SessionExtensions::default(),
            resume: None,
            turn: None,
        };
        let value = serde_json::to_value(&start).unwrap();
        assert!(value.get("extensions").is_none());
        assert_eq!(
            serde_json::from_value::<SessionStart>(value).unwrap(),
            start
        );
        let with = SessionStart {
            extensions: ext,
            ..start
        };
        let value = serde_json::to_value(&with).unwrap();
        assert_eq!(value["extensions"]["skills"][0]["name"], "pdf-forms");
        assert_eq!(serde_json::from_value::<SessionStart>(value).unwrap(), with);
    }

    #[test]
    fn skill_bundle_get_is_a_request() {
        let get = SkillBundleGet {
            content_hash: "c".repeat(64).parse().unwrap(),
        };
        let frame = Frame::request(FrameId::new("b1"), &get).unwrap();
        let value: Value = serde_json::from_str(&frame.to_text().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({"v": 1, "id": "b1", "type": "skill_bundle.get", "body": {"content_hash": "c".repeat(64)}})
        );
        let bundle = SkillBundle {
            skill_id: "skl_00000000-0000-0000-0000-000000000005".parse().unwrap(),
            name: "pdf-forms".into(),
            content_hash: crate::model::skill_content_hash(&[]),
            files: vec![],
        };
        let ok = Frame::ok(FrameId::new("b1"), &bundle).unwrap();
        let Frame::Response(resp) = Frame::parse(&ok.to_text().unwrap()).unwrap() else {
            panic!("expected a response");
        };
        assert_eq!(resp.into_response::<SkillBundleGet>().unwrap(), bundle);
    }

    #[test]
    fn account_update_carries_the_extras_toggle() {
        let update = AccountUpdateCommand {
            account_id: "acc_00000000-0000-0000-0000-000000000009".parse().unwrap(),
            label: None,
            env_set: SecretEnv::new(),
            env_remove: vec![],
            extras_enabled: Some(false),
        };
        let value = serde_json::to_value(&update).unwrap();
        assert_eq!(value["extras_enabled"], false);
        assert_eq!(
            serde_json::from_value::<AccountUpdateCommand>(value).unwrap(),
            update
        );
        let old: AccountUpdateCommand = serde_json::from_value(
            json!({"account_id": "acc_00000000-0000-0000-0000-000000000009"}),
        )
        .unwrap();
        assert_eq!(old.extras_enabled, None);
    }

    #[test]
    fn sign_in_update_is_an_event() {
        let update = SignInUpdate {
            sign_in_id: "sgn_00000000-0000-0000-0000-000000000002".parse().unwrap(),
            state: SignInState::Succeeded {
                detail: Some("chris@example.com".into()),
            },
        };
        let frame = Frame::family_event(&WorkerToHub::from(update.clone())).unwrap();
        let text = frame.to_text().unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value,
            json!({"v": 1, "type": "sign_in.update", "body": {
                "sign_in_id": "sgn_00000000-0000-0000-0000-000000000002",
                "state": {"kind": "succeeded", "detail": "chris@example.com"}
            }})
        );
        let Frame::Event(event) = Frame::parse(&text).unwrap() else {
            panic!("expected an event");
        };
        assert_eq!(
            event.decode::<WorkerToHub>().unwrap(),
            WorkerToHub::SignInUpdate(update)
        );
    }

    #[test]
    fn account_commands_hide_secrets_in_debug() {
        let create = AccountCreateCommand {
            account_id: AccountId::new(),
            harness: "codex".into(),
            label: "Codex API".into(),
            acp: None,
            env: [("OPENAI_API_KEY", "sk-proj-secret")].into_iter().collect(),
        };
        let update = AccountUpdateCommand {
            account_id: AccountId::new(),
            label: None,
            env_set: [("OPENAI_API_KEY", "sk-proj-other")].into_iter().collect(),
            env_remove: vec![],
            extras_enabled: None,
        };
        let code = SignInCodeCommand {
            sign_in_id: SignInId::new(),
            code: "the-pasted-code".into(),
        };
        let texts = [
            format!("{create:?}"),
            format!("{:?}", HubToWorker::from(create.clone())),
            format!("{update:?}"),
            format!("{:?}", HubToWorker::from(update)),
            format!("{code:?}"),
            format!("{:?}", HubToWorker::from(code.clone())),
        ];
        for text in &texts {
            for secret in ["sk-proj-secret", "sk-proj-other", "the-pasted-code"] {
                assert!(!text.contains(secret), "{text}");
            }
        }
        assert!(texts[0].contains("OPENAI_API_KEY"), "{}", texts[0]);
        // The wire still carries the values.
        assert_eq!(
            serde_json::to_value(&create).unwrap()["env"]["OPENAI_API_KEY"],
            "sk-proj-secret"
        );
        assert_eq!(
            serde_json::to_value(&code).unwrap()["code"],
            "the-pasted-code"
        );
    }

    #[test]
    fn hub_to_worker_round_trips() {
        let workspace = WorkspaceSpec {
            repos: vec![WorkspaceRepo {
                repo_id: "rep_00000000-0000-0000-0000-000000000001".parse().unwrap(),
                url: "git@github.com:watzon/sayso.git".into(),
                branch: None,
                checkout_path: Some("/Users/chris/Code/sayso".into()),
            }],
            isolation: Isolation::Worktree,
            carry_uncommitted: true,
        };
        let commands: Vec<HubToWorker> = vec![
            SessionStart {
                session_id: ses(),
                role: SessionRole::Task,
                channel_id: ChannelId::new(),
                task_id: Some(TaskId::new()),
                account_id: AccountId::new(),
                model: "gpt-5.5".into(),
                options: ModelOptions::new(),
                approval_mode: ApprovalMode::AutoEdits,
                workspace,
                system_prompt_append: "You are a task.".into(),
                allowed_local_mcp_servers: vec!["home-assistant".into()],
                channel_files: vec![],
                extensions: extensions(),
                resume: Some(ResumeSpec {
                    native_ref: NativeRef::new("abc"),
                }),
                turn: Some(TurnInput {
                    text: "Fix the bug.".into(),
                    attachments: vec![],
                }),
            }
            .into(),
            SessionTurn {
                session_id: ses(),
                turn: TurnInput::default(),
            }
            .into(),
            SessionSteer {
                session_id: ses(),
                turn: TurnInput::default(),
            }
            .into(),
            SessionInterrupt { session_id: ses() }.into(),
            SessionRespond {
                session_id: ses(),
                request_id: RequestId::new("r1"),
                answer: RequestAnswer {
                    option_id: Some("allow".into()),
                    text: None,
                },
            }
            .into(),
            SessionStop {
                session_id: ses(),
                reason: "task resolved".into(),
            }
            .into(),
            WorkspaceRelease {
                task_id: TaskId::new(),
                delete: true,
            }
            .into(),
            ItemDetail {
                session_id: ses(),
                item_id: ItemId::new(),
            }
            .into(),
            InventoryRefresh {}.into(),
            AccountCreateCommand {
                account_id: AccountId::new(),
                harness: "acp".into(),
                label: "My agent".into(),
                acp: Some(AcpAccountSpec::Command {
                    program: "my-agent".into(),
                    args: vec!["--acp".into()],
                }),
                env: SecretEnv::new(),
            }
            .into(),
            AccountUpdateCommand {
                account_id: AccountId::new(),
                label: Some("Renamed".into()),
                env_set: [("KEY", "v")].into_iter().collect(),
                env_remove: vec!["OLD".into()],
                extras_enabled: Some(true),
            }
            .into(),
            AccountRemoveCommand {
                account_id: AccountId::new(),
            }
            .into(),
            AccountSignOutCommand {
                account_id: AccountId::new(),
            }
            .into(),
            SignInStartCommand {
                sign_in_id: SignInId::new(),
                account_id: AccountId::new(),
                method: SignInMethod::AcpAgent {
                    method_id: "oauth".into(),
                    name: "Sign in".into(),
                },
            }
            .into(),
            SignInCodeCommand {
                sign_in_id: SignInId::new(),
                code: "code".into(),
            }
            .into(),
            SignInCancelCommand {
                sign_in_id: SignInId::new(),
            }
            .into(),
            ToolsCheck {}.into(),
            ToolsInstall {
                tool: ToolId::acp("goose"),
            }
            .into(),
        ];
        assert_eq!(commands.len(), HubToWorker::TYPES.len());
        for command in &commands {
            assert_eq!(
                command.session_id().is_some(),
                matches!(
                    command,
                    HubToWorker::SessionStart(_)
                        | HubToWorker::SessionTurn(_)
                        | HubToWorker::SessionSteer(_)
                        | HubToWorker::SessionInterrupt(_)
                        | HubToWorker::SessionRespond(_)
                        | HubToWorker::SessionStop(_)
                        | HubToWorker::ItemDetail(_)
                )
            );
        }
        for command in commands {
            let frame = Frame::family_request(FrameId::generate(), &command).unwrap();
            let Frame::Request(req) = Frame::parse(&frame.to_text().unwrap()).unwrap() else {
                panic!("expected a request");
            };
            assert_eq!(req.kind, command.type_name());
            let frame = Frame::family_request(FrameId::generate(), &command).unwrap();
            let Frame::Request(req) = Frame::parse(&frame.to_text().unwrap()).unwrap() else {
                panic!("expected a request");
            };
            assert_eq!(req.decode::<HubToWorker>().unwrap(), command);
        }
    }

    #[test]
    fn empty_results_and_typed_responses() {
        let ok = Frame::ok(FrameId::new("c9"), &Empty {}).unwrap();
        let Frame::Response(resp) = Frame::parse(&ok.to_text().unwrap()).unwrap() else {
            panic!("expected a response");
        };
        let _: <SessionInterrupt as Request>::Response =
            resp.into_response::<SessionInterrupt>().unwrap();

        let ack = SessionEventsAck {
            session_id: ses(),
            last_seq: 7,
        };
        let frame = Frame::ok(FrameId::new("c1"), &ack).unwrap();
        let Frame::Response(resp) = Frame::parse(&frame.to_text().unwrap()).unwrap() else {
            panic!("expected a response");
        };
        assert_eq!(resp.into_response::<SessionEvents>().unwrap(), ack);
    }

    #[test]
    fn unknown_command_type_is_reported() {
        let frame = Frame::parse(r#"{"v":1,"id":"x","type":"session.fork","body":{}}"#).unwrap();
        let Frame::Request(req) = frame else {
            panic!("expected a request");
        };
        let err = req.decode::<HubToWorker>().unwrap_err();
        assert!(matches!(err, crate::frame::FrameError::UnknownType(t) if t == "session.fork"));
    }

    #[test]
    fn bad_body_names_the_field() {
        let frame =
            Frame::parse(r#"{"v":1,"id":"x","type":"session.interrupt","body":{"session":"x"}}"#)
                .unwrap();
        let Frame::Request(req) = frame else {
            panic!("expected a request");
        };
        let err = req.decode::<HubToWorker>().unwrap_err();
        assert!(err.to_string().contains("session_id"), "{err}");
        assert_eq!(err.to_error_body().code, crate::error::ErrorCode::Invalid);
    }

    #[test]
    fn tool_call_frame_shapes() {
        let call = ToolCall {
            session_id: ses(),
            call_id: "7".into(),
            name: "post_update".into(),
            arguments: json!({"body": "Halfway."}),
        };
        let frame = Frame::request(FrameId::new("t1"), &call).unwrap();
        let value: Value = serde_json::from_str(&frame.to_text().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({"v": 1, "id": "t1", "type": "tool.call",
                   "body": {"session_id": "ses_00000000-0000-0000-0000-000000000001",
                            "call_id": "7", "name": "post_update",
                            "arguments": {"body": "Halfway."}}})
        );
        let list = ToolListResult {
            tools: vec![crate::tools::HubTool::PostUpdate.definition()],
        };
        let value = serde_json::to_value(&list).unwrap();
        let tool = &value["tools"][0];
        assert_eq!(tool["name"], "post_update");
        assert!(tool["description"].is_string());
        assert_eq!(tool["input_schema"]["type"], "object");
    }

    #[test]
    fn contiguous_ack() {
        let ev = |seq| SequencedEvent {
            seq,
            at: Timestamp::EPOCH,
            event: SessionEvent::State {
                state: SessionState::Active,
            },
        };
        assert_eq!(contiguous_seq(0, &[ev(1), ev(2), ev(3)]), 3);
        assert_eq!(contiguous_seq(0, &[ev(2), ev(3)]), 0);
        assert_eq!(contiguous_seq(5, &[ev(4), ev(5), ev(6), ev(8)]), 6);
        assert_eq!(contiguous_seq(2, &[ev(4), ev(3), ev(3)]), 4);
        assert_eq!(contiguous_seq(9, &[]), 9);
    }

    #[test]
    fn workspace_rules() {
        let mut spec = WorkspaceSpec {
            repos: vec![],
            isolation: Isolation::Clone,
            carry_uncommitted: true,
        };
        assert!(spec.validate().is_err());
        spec.carry_uncommitted = false;
        assert!(spec.validate().is_ok());
        spec.isolation = Isolation::InPlace;
        spec.repos.push(WorkspaceRepo {
            repo_id: RepoId::new(),
            url: "u".into(),
            branch: None,
            checkout_path: None,
        });
        assert!(spec.validate().is_err());
        spec.isolation = Isolation::Clone;
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn command_session_ids() {
        let cmd: HubToWorker = SessionInterrupt { session_id: ses() }.into();
        assert_eq!(cmd.session_id(), Some(ses()));
        let cmd: HubToWorker = InventoryRefresh {}.into();
        assert_eq!(cmd.session_id(), None);
    }
}
