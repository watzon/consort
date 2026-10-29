//! The harness layer: one Rust interface for every harness.
//!
//! A [`Harness`] knows one kind of harness and opens sessions for a harness
//! account. A [`HarnessSession`] is one live conversation. Each adapter
//! translates its harness's events into [`SessionEvent`]s from
//! `consort-proto`, and sends them through an mpsc channel. The design is in
//! `docs/harness-adapters.md`.
//!
//! Adapters:
//!
//! - [`acp`]: Claude Code through `claude-agent-acp`, OpenCode, Gemini CLI and
//!   other ACP Registry agents.
//! - [`codex`]: Codex through `codex app-server`.
//!
//! The fake harness for tests is in `consort-testkit`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod account;
pub mod acp;
pub mod approval;
pub mod codex;
mod items;
pub mod process;
pub mod record;
pub mod signin;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

pub use account::{AccountConfig, AccountRuntime};
pub use consort_proto::ids::{NativeRef, RequestId, SessionId, TurnId};
pub use consort_proto::model::SignInMethod;
pub use consort_proto::model::{ApprovalMode, AuthState, Capabilities, ModelInfo, ModelOptions};
pub use consort_proto::worker_link::{RequestAnswer as Answer, SessionEvent, TurnInput};
pub use consort_tools::ToolId;
pub use items::ItemTracker;

/// How many events an adapter may buffer before it waits for the worker.
pub const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// The kind of harness behind an account. The `harness` field of an account
/// in `worker.toml` and in the inventory uses the serialized names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HarnessKind {
    /// Claude Code through `claude-agent-acp`.
    #[serde(rename = "claude")]
    ClaudeCode,
    /// Codex through `codex app-server`.
    #[serde(rename = "codex")]
    Codex,
    /// OpenCode through `opencode acp`.
    #[serde(rename = "opencode")]
    OpenCode,
    /// Gemini CLI through its ACP mode.
    #[serde(rename = "gemini")]
    GeminiCli,
    /// Pi. This build has no Pi adapter.
    #[serde(rename = "pi")]
    Pi,
    /// Any other ACP agent, launched with the account's `binary` and `args`.
    #[serde(rename = "acp")]
    Acp,
}

impl HarnessKind {
    /// Every kind, in the order the worker discovers them.
    pub const ALL: [HarnessKind; 6] = [
        Self::ClaudeCode,
        Self::Codex,
        Self::OpenCode,
        Self::GeminiCli,
        Self::Pi,
        Self::Acp,
    ];

    /// The serialized name, such as `codex`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::GeminiCli => "gemini",
            Self::Pi => "pi",
            Self::Acp => "acp",
        }
    }

    /// The harness display name, which is also the label of the default
    /// account.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::GeminiCli => "Gemini CLI",
            Self::Pi => "Pi",
            Self::Acp => "ACP agent",
        }
    }
}

impl std::fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How to start a harness program: the program, its arguments, and the
/// environment variables to add.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Launch {
    /// The program.
    pub program: PathBuf,
    /// The arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: BTreeMap<String, String>,
}

/// What a probe learned about one harness account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStatus {
    /// The installed version, when the harness reports one.
    pub harness_version: Option<String>,
    /// Whether the account is signed in, as the harness reports it.
    pub auth_state: AuthState,
    /// More detail for the user, such as the plan or the command to sign in.
    /// Never a credential.
    pub auth_detail: Option<String>,
    /// The models that the harness offers, with option descriptors.
    pub models: Vec<ModelInfo>,
    /// What sessions of this account can do.
    pub capabilities: Capabilities,
    /// The signed-in email address, when the harness tells.
    pub email: Option<String>,
    /// The plan, such as "Max", when the harness tells.
    pub plan: Option<String>,
    /// The sign-in methods that the harness itself reports, such as the
    /// `authMethods` of an ACP agent. The worker adds the methods that it
    /// knows for each harness.
    pub sign_in_methods: Vec<SignInMethod>,
}

impl Default for AccountStatus {
    fn default() -> Self {
        Self {
            harness_version: None,
            auth_state: AuthState::Unknown,
            auth_detail: None,
            models: Vec::new(),
            capabilities: Capabilities::default(),
            email: None,
            plan: None,
            sign_in_methods: Vec::new(),
        }
    }
}

impl AccountStatus {
    /// A status for a harness that could not be probed.
    pub fn unknown(detail: impl Into<String>, capabilities: Capabilities) -> Self {
        Self {
            auth_state: AuthState::Unknown,
            auth_detail: Some(detail.into()),
            capabilities,
            ..Self::default()
        }
    }
}

/// A command that runs a stdio MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioCommand {
    /// The program.
    pub command: PathBuf,
    /// The arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: BTreeMap<String, String>,
}

/// One MCP server to give a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSpec {
    /// The server name that the model sees, such as `hub`.
    pub name: String,
    /// The streamable HTTP URL.
    pub url: String,
    /// The bearer token for the `Authorization` header, if the server needs
    /// one.
    pub bearer_token: Option<String>,
    /// A stdio command that bridges to the same server, for a harness that
    /// cannot take an HTTP MCP server.
    pub bridge: Option<StdioCommand>,
}

/// What a new session needs.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSpec {
    /// The Consort session ID. Adapters put it in session items.
    pub session_id: SessionId,
    /// The workspace folder. The session works here. The adapter gives this
    /// folder to the harness in the protocol (`cwd` of `session/new` or
    /// `thread/start`).
    pub workspace: PathBuf,
    /// A folder that the adapter may use for generated files. The adapter
    /// starts the harness process in this folder, not in the workspace, so a
    /// launcher such as `npx` does not read the repo's `.npmrc` and a Windows
    /// shell does not find programs in the repo.
    pub runtime_dir: PathBuf,
    /// The model ID. Empty means the harness default.
    pub model: String,
    /// The model options, such as `effort`.
    pub options: ModelOptions,
    /// The effective approval mode, already capped by worker policy.
    pub approval_mode: ApprovalMode,
    /// Whether the worker policy lets the session run shell commands. When
    /// it is false, the adapter removes the harness's shell tools where the
    /// harness allows that (see `docs/harness-adapters.md`).
    pub allow_shell: bool,
    /// Text to append to the harness's system prompt.
    pub system_prompt_append: String,
    /// The MCP servers to give the session. The hub endpoint is first.
    pub mcp_servers: Vec<McpServerSpec>,
    /// Extra environment variables for the harness process.
    pub env: BTreeMap<String, String>,
    /// Resume this harness session instead of starting a new one.
    pub resume: Option<NativeRef>,
    /// Learns the process groups of the harness processes, so the worker can
    /// kill the ones that outlive it.
    pub process_watcher: Option<process::WatcherHandle>,
}

/// The errors of the harness layer.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    /// The session or adapter cannot do this.
    #[error("this harness session cannot {0}")]
    Unsupported(&'static str),
    /// The harness program could not start.
    #[error("could not start the harness: {0}")]
    Spawn(String),
    /// The harness says that the account is not signed in.
    #[error("the harness account is not signed in: {0}")]
    SignedOut(String),
    /// The harness sent something that the adapter did not expect, or
    /// answered a request with an error.
    #[error("the harness returned an error: {0}")]
    Protocol(String),
    /// The harness could not resume the session.
    #[error("the harness could not resume the session: {0}")]
    ResumeFailed(String),
    /// The session ended.
    #[error("the harness session has ended")]
    Closed,
    /// No open request has this ID.
    #[error("no open harness request has the ID `{0}`")]
    UnknownRequest(String),
    /// The answer does not fit the request.
    #[error("the answer is not valid for this request: {0}")]
    InvalidAnswer(String),
    /// The harness did not answer in time.
    #[error("the harness did not answer in time: {0}")]
    Timeout(String),
    /// The harness cannot run in the approval mode that the session asks
    /// for, for example because Claude Code turned `bypassPermissions` off.
    #[error("the harness cannot run in this approval mode: {0}")]
    ModeUnavailable(String),
    /// A file system error.
    #[error("file system error: {0}")]
    Io(#[from] std::io::Error),
}

/// The receiver of a session's events.
pub type EventReceiver = mpsc::Receiver<SessionEvent>;

/// The answer to a steer. It does not borrow the session, so the worker can
/// wait for it in another task and go on with interrupts and answers.
pub type SteerFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), HarnessError>> + Send>>;

/// A [`SteerFuture`] that is ready with `result`.
pub fn steer_ready(result: Result<(), HarnessError>) -> SteerFuture {
    Box::pin(std::future::ready(result))
}

/// One kind of harness.
#[async_trait]
pub trait Harness: Send + Sync {
    /// The kind of harness.
    fn kind(&self) -> HarnessKind;

    /// The capabilities of sessions for this account, without starting the
    /// harness.
    fn capabilities(&self, account: &AccountConfig) -> Capabilities;

    /// The managed tool that runs this account, or `None` when the adapter
    /// needs none. The worker installs it and puts its launch in
    /// `account.runtime.launch` before a probe or an open.
    fn managed_tool(&self, account: &AccountConfig) -> Option<ToolId> {
        let _ = account;
        None
    }

    /// Installed version, sign-in state, models with option descriptors,
    /// capabilities. A probe never reads credential files.
    async fn probe(&self, account: &AccountConfig) -> AccountStatus;

    /// Starts a new session, or resumes one when `spec.resume` is set.
    async fn open(
        &self,
        account: &AccountConfig,
        spec: SessionSpec,
    ) -> Result<(Box<dyn HarnessSession>, EventReceiver), HarnessError>;
}

/// One live conversation with a harness.
#[async_trait]
pub trait HarnessSession: Send {
    /// What this session can do.
    fn capabilities(&self) -> &Capabilities;

    /// The harness's own session ID, once it is known.
    fn native_ref(&self) -> Option<NativeRef>;

    /// Starts a turn. Returns when the harness accepts the input. The end of
    /// the turn comes as a `turn.ended` event.
    async fn turn(&mut self, input: TurnInput) -> Result<TurnId, HarnessError>;

    /// Adds input to the running turn. The session takes the input at once,
    /// in order with the other calls, and the returned future gives the
    /// harness's answer. The future fails with
    /// [`HarnessError::Unsupported`] when `capabilities().steer` is false.
    fn steer(&mut self, input: TurnInput) -> SteerFuture;

    /// Stops the running turn.
    async fn interrupt(&mut self) -> Result<(), HarnessError>;

    /// Answers an open request.
    async fn respond(&mut self, request: RequestId, answer: Answer) -> Result<(), HarnessError>;

    /// Ends the session and the harness process.
    async fn close(self: Box<Self>) -> Result<(), HarnessError>;
}

/// The text of a turn input for a harness that takes only text. Attachments
/// are listed by name, because this build does not download them yet.
pub fn turn_text(input: &TurnInput) -> String {
    if input.attachments.is_empty() {
        return input.text.clone();
    }
    let names: Vec<&str> = input.attachments.iter().map(|a| a.name.as_str()).collect();
    format!("{}\n\n(Attached files: {})", input.text, names.join(", "))
}

/// The folder of a command approval. `given` is the folder that the harness
/// names, and `workspace` is the session's workspace, where the harness
/// starts commands. A relative folder is in the workspace. When the harness
/// names no folder, the command runs in the workspace. So the user always
/// sees where a command runs before they allow it.
pub(crate) fn command_cwd(given: Option<&str>, workspace: Option<&Path>) -> Option<String> {
    let given = given.map(str::trim).filter(|g| !g.is_empty());
    match (given, workspace) {
        (Some(given), Some(workspace)) if !Path::new(given).is_absolute() => {
            Some(workspace.join(given).to_string_lossy().into_owned())
        }
        (Some(given), _) => Some(given.to_owned()),
        (None, Some(workspace)) => Some(workspace.to_string_lossy().into_owned()),
        (None, None) => None,
    }
}

/// The `code` of the [`SessionEvent::Error`] that tells the worker the
/// harness account is signed out. The `message` of that event is the text for
/// the user, and it includes what to do next (see [`sign_in_hint`]).
pub const SIGNED_OUT_ERROR_CODE: &str = "signed_out";

/// What to tell the user when a harness account is not signed in. `message`
/// is the reason that the harness gave, and it may be empty.
pub fn sign_in_hint(kind: HarnessKind, message: &str) -> String {
    let command = match kind {
        HarnessKind::ClaudeCode => "Run `claude` on this machine and sign in with /login.",
        HarnessKind::Codex => "Run `codex login` on this machine.",
        HarnessKind::OpenCode => "Run `opencode auth login` on this machine.",
        HarnessKind::GeminiCli => "Run `gemini` on this machine and sign in.",
        _ => "Sign in with the agent's own command on this machine.",
    };
    let message = message.trim();
    if message.is_empty() {
        command.to_owned()
    } else if message.ends_with(['.', '!', '?']) {
        format!("{message} {command}")
    } else {
        format!("{message}. {command}")
    }
}

/// What to tell the user when an account is not signed in. An account that
/// Consort made signs in from the app. An account from `worker.toml` signs
/// in with the harness's own command (see [`sign_in_hint`]).
pub fn sign_in_hint_for(account: &AccountConfig, message: &str) -> String {
    if !account.profile {
        return sign_in_hint(account.harness, message);
    }
    let step = "Sign in to this account in Settings > Workers.";
    let message = message.trim();
    if message.is_empty() {
        step.to_owned()
    } else if message.ends_with(['.', '!', '?']) {
        format!("{message} {step}")
    } else {
        format!("{message}. {step}")
    }
}

/// Returns the adapter for a kind, or `None` for a kind with no adapter in
/// this build.
pub fn adapter_for(kind: HarnessKind) -> Option<Box<dyn Harness>> {
    match kind {
        HarnessKind::Codex => Some(Box::new(codex::CodexHarness::new())),
        HarnessKind::ClaudeCode
        | HarnessKind::OpenCode
        | HarnessKind::GeminiCli
        | HarnessKind::Acp => Some(Box::new(acp::AcpHarness::new(kind))),
        HarnessKind::Pi => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_names_round_trip() {
        for kind in HarnessKind::ALL {
            let value = serde_json::to_value(kind).unwrap();
            assert_eq!(value, kind.as_str());
            let back: HarnessKind = serde_json::from_value(value).unwrap();
            assert_eq!(back, kind);
        }
        assert_eq!(HarnessKind::GeminiCli.display_name(), "Gemini CLI");
    }

    #[test]
    fn sign_in_hint_ends_the_reason_with_a_period() {
        assert_eq!(
            sign_in_hint(HarnessKind::ClaudeCode, "Authentication required"),
            "Authentication required. Run `claude` on this machine and sign in with /login."
        );
        assert_eq!(
            sign_in_hint(HarnessKind::Codex, "Token expired."),
            "Token expired. Run `codex login` on this machine."
        );
        assert_eq!(
            sign_in_hint(HarnessKind::OpenCode, ""),
            "Run `opencode auth login` on this machine."
        );
    }

    #[test]
    fn turn_text_lists_attachments() {
        let input = TurnInput {
            text: "Look".into(),
            attachments: vec![consort_proto::model::BlobRef {
                sha256: consort_proto::model::Sha256Hash::from_bytes([0; 32]),
                name: "a.png".into(),
                mime: "image/png".into(),
                size: 1,
            }],
        };
        assert_eq!(turn_text(&input), "Look\n\n(Attached files: a.png)");
    }

    #[test]
    fn a_command_folder_falls_back_to_the_workspace() {
        let workspace = std::env::temp_dir().join("ws");
        let inside = workspace.join("sub").to_string_lossy().into_owned();
        let ws = Some(workspace.as_path());
        let ws_text = workspace.to_string_lossy().into_owned();
        assert_eq!(command_cwd(None, ws), Some(ws_text.clone()));
        assert_eq!(command_cwd(Some("  "), ws), Some(ws_text));
        assert_eq!(command_cwd(Some("sub"), ws), Some(inside.clone()));
        assert_eq!(command_cwd(Some(&inside), ws), Some(inside.clone()));
        assert_eq!(command_cwd(Some("sub"), None), Some("sub".to_owned()));
        assert_eq!(command_cwd(None, None), None);
    }
}
