//! The ACP adapter: Claude Code through `claude-agent-acp`, OpenCode through
//! `opencode acp`, Gemini CLI through its ACP mode, and other ACP agents.
//!
//! The adapter speaks ACP v1 with the `agent-client-protocol` crate. It
//! spawns the agent itself (so it controls the folder, the environment and
//! the process tree), and sends and receives untyped JSON-RPC messages, so
//! that [`mapper::AcpMapper`] can map the wire shape directly.
//!
//! One agent process serves one session.

pub mod mapper;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_client_protocol::{
    Agent, ByteStreams, Client, ConnectionTo, ErrorCode, Responder, UntypedMessage,
};
use async_trait::async_trait;
use consort_proto::ids::{NativeRef, RequestId, TurnId};
use consort_proto::model::{
    ApprovalMode, AuthState, Capabilities, ModelInfo, OptionChoice, OptionDescriptor, OptionValue,
    RequestOption, SessionState,
};
use consort_proto::worker_link::{RequestOutcome, SessionEvent, TurnEndReason, TurnInput};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::account::prepare_home;
use crate::approval::{CLAUDE_SHELL_TOOLS, claude_mode, gemini_mode, opencode_permission};
use crate::process::{self, ProcessSpec, StderrTail, resolve_program};
use crate::{
    AccountConfig, AccountStatus, Answer, EVENT_CHANNEL_CAPACITY, EventReceiver, Harness,
    HarnessError, HarnessKind, HarnessSession, McpServerSpec, SIGNED_OUT_ERROR_CODE, SessionSpec,
    sign_in_hint, turn_text,
};
use mapper::{AcpMapper, stop_reason};

/// The pinned `claude-agent-acp` package.
pub const CLAUDE_AGENT_ACP: &str = "@agentclientprotocol/claude-agent-acp@0.99.0";

/// How long the adapter waits for the agent to start and open a session.
/// The first `npx` run downloads the package, so this is long.
const OPEN_TIMEOUT: Duration = Duration::from_secs(180);
/// How long a probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a steer may wait for the agent's answer. The worker's session
/// actor waits for the answer, so this stays short.
const STEER_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(500)
} else {
    Duration::from_secs(10)
};
/// How long a turn may run after `session/cancel`. After that, the adapter
/// kills the agent and reports the session as lost.
const INTERRUPT_GRACE: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(30)
};
/// The most bytes of repo instructions that the adapter adds to the system
/// prompt of a Claude session.
const PROJECT_INSTRUCTIONS_MAX_BYTES: usize = 64 * 1024;
/// A CLAUDE.md file larger than this is not read.
const PROJECT_FILE_MAX_BYTES: u64 = 1024 * 1024;

/// The ACP adapter for one kind of ACP agent.
#[derive(Debug, Clone, Copy)]
pub struct AcpHarness {
    kind: HarnessKind,
}

impl AcpHarness {
    /// An adapter for Claude Code, OpenCode, Gemini CLI or a generic ACP
    /// agent.
    pub fn new(kind: HarnessKind) -> Self {
        Self { kind }
    }
}

/// How to launch an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The program.
    pub program: PathBuf,
    /// The arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: BTreeMap<String, String>,
}

/// The launch recipe for an account. `resolve` finds a program on `PATH`;
/// tests pass a fake.
pub fn launch_recipe(
    kind: HarnessKind,
    account: &AccountConfig,
    mode: ApprovalMode,
    allow_shell: bool,
    resolve: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<Launch, HarnessError> {
    let explicit = |default: &str| -> Result<PathBuf, HarnessError> {
        let name = account.binary.as_deref().unwrap_or(default);
        resolve(name).ok_or_else(|| HarnessError::Spawn(format!("`{name}` is not on PATH")))
    };
    let mut env = BTreeMap::new();
    let (program, mut args) = match kind {
        HarnessKind::ClaudeCode => {
            if account.binary.is_some() {
                (explicit("claude-agent-acp")?, Vec::new())
            } else if let Some(npx) = resolve("npx") {
                (npx, vec!["-y".to_owned(), CLAUDE_AGENT_ACP.to_owned()])
            } else if let Some(bunx) = resolve("bunx") {
                (bunx, vec![CLAUDE_AGENT_ACP.to_owned()])
            } else {
                return Err(HarnessError::Spawn(
                    "Claude Code through ACP needs Node.js 22 or later (npx) or Bun (bunx) on PATH"
                        .to_owned(),
                ));
            }
        }
        HarnessKind::OpenCode => {
            env.insert(
                "OPENCODE_CONFIG_CONTENT".to_owned(),
                opencode_permission(mode, allow_shell).to_string(),
            );
            (explicit("opencode")?, vec!["acp".to_owned()])
        }
        HarnessKind::GeminiCli => (
            explicit("gemini")?,
            vec![
                "--experimental-acp".to_owned(),
                "--approval-mode".to_owned(),
                gemini_mode(mode).to_owned(),
            ],
        ),
        HarnessKind::Acp => {
            let Some(binary) = account.binary.as_deref() else {
                return Err(HarnessError::Spawn(
                    "an ACP agent account needs `binary` in worker.toml".to_owned(),
                ));
            };
            (explicit(binary)?, Vec::new())
        }
        HarnessKind::Codex | HarnessKind::Pi => {
            return Err(HarnessError::Unsupported("run through the ACP adapter"));
        }
    };
    args.extend(account.args.iter().cloned());
    Ok(Launch { program, args, env })
}

/// The capabilities that an ACP account has before the agent reports its
/// own. The session refines `resume`, `steer` and `mcp_http` from the
/// agent's `initialize` response.
pub fn static_capabilities(kind: HarnessKind) -> Capabilities {
    match kind {
        HarnessKind::ClaudeCode => Capabilities {
            steer: true,
            resume: true,
            usage: true,
            mcp_http: true,
            approval_modes: ApprovalMode::ALL.to_vec(),
            ..Capabilities::default()
        },
        HarnessKind::OpenCode => Capabilities {
            resume: true,
            approval_modes: ApprovalMode::ALL.to_vec(),
            ..Capabilities::default()
        },
        HarnessKind::GeminiCli => Capabilities {
            approval_modes: vec![
                ApprovalMode::Ask,
                ApprovalMode::AutoEdits,
                ApprovalMode::Auto,
            ],
            ..Capabilities::default()
        },
        _ => Capabilities {
            approval_modes: vec![ApprovalMode::Ask],
            ..Capabilities::default()
        },
    }
}

/// The `mcpServers` list of `session/new`. An HTTP server goes in as `http`
/// with an `Authorization` header when the agent supports HTTP MCP, and
/// through its stdio bridge otherwise.
pub fn mcp_servers_json(servers: &[McpServerSpec], http: bool) -> Vec<Value> {
    servers
        .iter()
        .filter_map(|server| {
            if http {
                let headers: Vec<Value> = server
                    .bearer_token
                    .iter()
                    .map(|t| json!({"name": "Authorization", "value": format!("Bearer {t}")}))
                    .collect();
                Some(json!({"type": "http", "name": server.name, "url": server.url, "headers": headers}))
            } else {
                let bridge = server.bridge.as_ref()?;
                let env: Vec<Value> = bridge
                    .env
                    .iter()
                    .map(|(k, v)| json!({"name": k, "value": v}))
                    .collect();
                Some(json!({
                    "name": server.name,
                    "command": bridge.command.to_string_lossy(),
                    "args": bridge.args,
                    "env": env,
                }))
            }
        })
        .collect()
}

/// The first `max` bytes of `text`, cut at a character boundary.
fn head_of(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Reads one CLAUDE.md file of the workspace. The file must be a plain file
/// inside the workspace, not a link, so a repo cannot make the worker read a
/// file from somewhere else.
fn read_project_file(workspace: &Path, rel: &str) -> Option<String> {
    let path = workspace.join(rel);
    let meta = path.symlink_metadata().ok()?;
    if !meta.is_file() || meta.len() > PROJECT_FILE_MAX_BYTES {
        return None;
    }
    let root = std::fs::canonicalize(workspace).ok()?;
    let real = std::fs::canonicalize(&path).ok()?;
    if !real.starts_with(&root) {
        return None;
    }
    let text = std::fs::read_to_string(&real).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The repo instructions of a workspace: `CLAUDE.md` and `.claude/CLAUDE.md`
/// in the workspace folder, at most [`PROJECT_INSTRUCTIONS_MAX_BYTES`].
///
/// A Claude session below Full access does not load the repo's Claude
/// settings, and Claude Code loads CLAUDE.md files only with those settings.
/// So the adapter reads these two files and adds them to the system prompt.
pub fn project_instructions(workspace: &Path) -> Option<String> {
    let mut budget = PROJECT_INSTRUCTIONS_MAX_BYTES;
    let mut parts = Vec::new();
    for rel in ["CLAUDE.md", ".claude/CLAUDE.md"] {
        if budget == 0 {
            break;
        }
        let Some(text) = read_project_file(workspace, rel) else {
            continue;
        };
        let kept = head_of(&text, budget);
        budget -= kept.len();
        parts.push(format!(
            "Instructions from {rel} in the workspace:\n\n{kept}"
        ));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// The `_meta` object of `session/new` and `session/load` for Claude Code.
///
/// - `systemPrompt.append` holds the text from the hub, and below Full
///   access the repo instructions (see [`project_instructions`]).
/// - Below Full access, `settingSources` is `["user"]`. The repo's
///   `.claude/settings.json` and `.claude/settings.local.json` then cannot
///   allow tools, set a permission mode, add hooks or start MCP servers. And
///   `allowDangerouslySkipPermissions` is false, so nothing can turn on
///   `bypassPermissions`.
/// - With `allow_shell` off, `disallowedTools` removes the shell tools.
pub fn claude_meta(spec: &SessionSpec) -> Value {
    let restricted = spec.approval_mode < ApprovalMode::FullAccess;
    let mut append = spec.system_prompt_append.clone();
    if restricted && let Some(text) = project_instructions(&spec.workspace) {
        if !append.is_empty() {
            append.push_str("\n\n");
        }
        append.push_str(&text);
    }
    let mut options = serde_json::Map::new();
    if restricted {
        options.insert("settingSources".to_owned(), json!(["user"]));
        options.insert("allowDangerouslySkipPermissions".to_owned(), json!(false));
    }
    if !spec.allow_shell {
        options.insert("disallowedTools".to_owned(), json!(CLAUDE_SHELL_TOOLS));
    }
    let mut meta = serde_json::Map::new();
    if !append.is_empty() {
        meta.insert("systemPrompt".to_owned(), json!({ "append": append }));
    }
    if !options.is_empty() {
        meta.insert("claudeCode".to_owned(), json!({ "options": options }));
    }
    Value::Object(meta)
}

/// The choices of a select config option, flat or grouped.
fn option_choices(option: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(list) = option.get("options").and_then(Value::as_array) {
        for entry in list {
            if let Some(inner) = entry.get("options").and_then(Value::as_array) {
                for o in inner {
                    if let Some(v) = o.get("value").and_then(Value::as_str) {
                        let name = o.get("name").and_then(Value::as_str).unwrap_or(v);
                        out.push((v.to_owned(), name.to_owned()));
                    }
                }
            } else if let Some(v) = entry.get("value").and_then(Value::as_str) {
                let name = entry.get("name").and_then(Value::as_str).unwrap_or(v);
                out.push((v.to_owned(), name.to_owned()));
            }
        }
    }
    out
}

/// Finds a config option by category, or by one of the given IDs.
fn find_option<'a>(options: &'a [Value], category: &str, ids: &[&str]) -> Option<&'a Value> {
    options
        .iter()
        .find(|o| o.get("category").and_then(Value::as_str) == Some(category))
        .or_else(|| {
            options.iter().find(|o| {
                o.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| ids.contains(&id))
            })
        })
}

/// The models in a session's config options, with an `effort` option when
/// the agent has a thought level option.
pub fn models_from_config(options: &[Value]) -> Vec<ModelInfo> {
    let effort = find_option(options, "thought_level", &["effort", "thought_level"]).map(|o| {
        let choices = option_choices(o);
        OptionDescriptor::Select {
            id: "effort".to_owned(),
            label: "Effort".to_owned(),
            choices: choices
                .into_iter()
                .map(|(value, label)| OptionChoice { value, label })
                .collect(),
            default: o
                .get("currentValue")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    });
    let Some(model) = find_option(options, "model", &["model"]) else {
        return Vec::new();
    };
    option_choices(model)
        .into_iter()
        .map(|(id, name)| ModelInfo {
            id,
            name,
            options: effort.iter().cloned().collect(),
        })
        .collect()
}

/// What the actor learned while it opened the session.
#[derive(Debug, Clone)]
struct Ready {
    native_ref: NativeRef,
    capabilities: Capabilities,
    agent_version: Option<String>,
    config_options: Vec<Value>,
    auth_label: Option<String>,
}

/// The account label from `claude-agent-acp`'s `_auth/status_update`
/// extension, such as "Claude Max". The email in the same message is not
/// kept.
fn auth_label(params: &Value) -> Option<String> {
    let status = params.get("authStatus")?;
    if status.get("kind").and_then(Value::as_str) != Some("account") {
        return None;
    }
    status
        .get("label")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

enum Incoming {
    Update(Value),
    Request {
        method: String,
        params: Value,
        responder: Box<Responder<Value>>,
    },
}

enum Command {
    Turn {
        input: TurnInput,
        reply: oneshot::Sender<Result<TurnId, HarnessError>>,
    },
    Steer {
        input: TurnInput,
        reply: oneshot::Sender<Result<(), HarnessError>>,
    },
    Interrupt {
        reply: oneshot::Sender<Result<(), HarnessError>>,
    },
    Respond {
        request: RequestId,
        answer: Answer,
        reply: oneshot::Sender<Result<(), HarnessError>>,
    },
    Close,
}

struct PendingPermission {
    responder: Responder<Value>,
    options: Vec<RequestOption>,
}

/// One live ACP session.
pub struct AcpSession {
    capabilities: Capabilities,
    native_ref: NativeRef,
    commands: mpsc::Sender<Command>,
    task: tokio::task::JoinHandle<()>,
}

fn protocol_error(err: &agent_client_protocol::Error) -> HarnessError {
    if err.code == ErrorCode::AuthRequired {
        HarnessError::SignedOut(err.message.clone())
    } else {
        HarnessError::Protocol(format!("{} ({})", err, i32::from(err.code)))
    }
}

async fn request(
    cx: &ConnectionTo<Agent>,
    method: &str,
    params: Value,
) -> Result<Value, agent_client_protocol::Error> {
    let message = UntypedMessage::new(method, params)?;
    cx.send_request(message).block_task().await
}

struct Actor {
    kind: HarnessKind,
    spec: SessionSpec,
    capabilities: Capabilities,
    events: mpsc::Sender<SessionEvent>,
    stderr: StderrTail,
}

impl Actor {
    async fn emit(&self, event: SessionEvent) {
        let _ = self.events.send(event).await;
    }

    async fn emit_all(&self, events: Vec<SessionEvent>) {
        for event in events {
            self.emit(event).await;
        }
    }

    /// Initializes the agent and opens or loads the session.
    async fn open(
        &mut self,
        cx: &ConnectionTo<Agent>,
        incoming: &mut mpsc::UnboundedReceiver<Incoming>,
    ) -> Result<Ready, HarnessError> {
        let init = request(
            cx,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": {"readTextFile": false, "writeTextFile": false},
                    "terminal": false
                },
                "clientInfo": {"name": "consort", "title": "Consort", "version": env!("CARGO_PKG_VERSION")}
            }),
        )
        .await
        .map_err(|e| protocol_error(&e))?;
        let agent_caps = init
            .get("agentCapabilities")
            .cloned()
            .unwrap_or(Value::Null);
        let load = agent_caps
            .get("loadSession")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let http = agent_caps
            .get("mcpCapabilities")
            .and_then(|m| m.get("http"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let steering = init
            .get("_meta")
            .and_then(|m| m.get("steering"))
            .and_then(|s| s.get("supported"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.capabilities.resume = self.capabilities.resume && load;
        self.capabilities.steer = self.capabilities.steer && steering;
        self.capabilities.mcp_http = http;
        let agent_version = init
            .get("agentInfo")
            .and_then(|i| i.get("version"))
            .and_then(Value::as_str)
            .map(str::to_owned);

        let cwd = self.spec.workspace.to_string_lossy().into_owned();
        let servers = mcp_servers_json(&self.spec.mcp_servers, http);
        // Claude Code gets the same `_meta` on `session/load` as on
        // `session/new`, so a resumed session keeps its system prompt and its
        // limits.
        let meta = (self.kind == HarnessKind::ClaudeCode)
            .then(|| claude_meta(&self.spec))
            .filter(|m| m.as_object().is_some_and(|o| !o.is_empty()));
        let (native_ref, response) = if let Some(resume) = &self.spec.resume {
            if !load {
                return Err(HarnessError::ResumeFailed(
                    "the agent cannot load sessions".to_owned(),
                ));
            }
            let mut params =
                json!({"sessionId": resume.as_str(), "cwd": cwd, "mcpServers": servers});
            if let Some(meta) = &meta {
                params["_meta"] = meta.clone();
            }
            let response = request(cx, "session/load", params)
                .await
                .map_err(|e| HarnessError::ResumeFailed(e.to_string()))?;
            // `session/load` replays the history as updates. The hub has it
            // already, so drop the replay.
            while let Ok(message) = incoming.try_recv() {
                if let Incoming::Request { responder, .. } = message {
                    let _ = responder
                        .respond_with_error(agent_client_protocol::Error::internal_error());
                }
            }
            (resume.clone(), response)
        } else {
            let mut params = json!({"cwd": cwd, "mcpServers": servers});
            if let Some(meta) = &meta {
                params["_meta"] = meta.clone();
            }
            let response = request(cx, "session/new", params)
                .await
                .map_err(|e| protocol_error(&e))?;
            let Some(id) = response.get("sessionId").and_then(Value::as_str) else {
                return Err(HarnessError::Protocol(
                    "session/new returned no sessionId".to_owned(),
                ));
            };
            (NativeRef::new(id), response)
        };
        let config_options: Vec<Value> = response
            .get("configOptions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let config_options = self
            .configure(cx, &native_ref, &response, config_options)
            .await?;
        Ok(Ready {
            native_ref,
            capabilities: self.capabilities.clone(),
            agent_version,
            config_options,
            auth_label: None,
        })
    }

    /// Sets one select config option. Returns the config options that the
    /// agent sent back, or `None` when nothing changed.
    async fn set_option(
        &self,
        cx: &ConnectionTo<Agent>,
        session: &NativeRef,
        option: &Value,
        value: &str,
    ) -> Result<Option<Vec<Value>>, String> {
        let Some(id) = option.get("id").and_then(Value::as_str) else {
            return Err("the option has no ID".to_owned());
        };
        if option.get("currentValue").and_then(Value::as_str) == Some(value) {
            return Ok(None);
        }
        if !option_choices(option).iter().any(|(v, _)| v == value) {
            return Err(format!("the harness does not offer `{value}` for `{id}`"));
        }
        let reply = request(
            cx,
            "session/set_config_option",
            json!({"sessionId": session.as_str(), "configId": id, "value": value}),
        )
        .await
        .map_err(|err| format!("could not set `{id}` to `{value}`: {err}"))?;
        Ok(reply
            .get("configOptions")
            .and_then(Value::as_array)
            .cloned())
    }

    /// Sets one option that may fail without harm, such as the model. A
    /// failure becomes an error event, and the harness uses its default.
    async fn set_soft_option(
        &self,
        cx: &ConnectionTo<Agent>,
        session: &NativeRef,
        option: &Value,
        value: &str,
    ) -> Option<Vec<Value>> {
        match self.set_option(cx, session, option, value).await {
            Ok(updated) => updated,
            Err(message) => {
                self.emit(SessionEvent::Error {
                    code: "option_failed".to_owned(),
                    message: format!("{message}. It uses its default."),
                })
                .await;
                None
            }
        }
    }

    /// Sets the Claude Code permission mode and reads it back. Any doubt is a
    /// failure, because a wrong mode can run tools that the worker policy
    /// does not allow.
    async fn set_claude_mode(
        &self,
        cx: &ConnectionTo<Agent>,
        session: &NativeRef,
        response: &Value,
        options: &mut Vec<Value>,
    ) -> Result<(), HarnessError> {
        let mode = claude_mode(self.spec.approval_mode);
        let refuse = |why: String| {
            HarnessError::Protocol(format!(
                "could not set the Claude Code permission mode to `{mode}`: {why}"
            ))
        };
        if let Some(option) = find_option(options, "mode", &["mode"]).cloned() {
            if let Some(updated) = self
                .set_option(cx, session, &option, mode)
                .await
                .map_err(refuse)?
            {
                *options = updated;
            }
            let current = find_option(options, "mode", &["mode"])
                .and_then(|o| o.get("currentValue"))
                .and_then(Value::as_str);
            if current != Some(mode) {
                return Err(refuse(format!(
                    "the agent reports `{}`",
                    current.unwrap_or("no mode")
                )));
            }
            return Ok(());
        }
        let Some(modes) = response.get("modes") else {
            return Err(refuse("the agent offers no permission mode".to_owned()));
        };
        if modes.get("currentModeId").and_then(Value::as_str) == Some(mode) {
            return Ok(());
        }
        let offered = modes
            .get("availableModes")
            .and_then(Value::as_array)
            .is_some_and(|list| {
                list.iter()
                    .any(|m| m.get("id").and_then(Value::as_str) == Some(mode))
            });
        if !offered {
            return Err(refuse("the agent does not offer it".to_owned()));
        }
        // A successful `session/set_mode` sets the mode. ACP has no way to
        // read the mode back in the answer.
        request(
            cx,
            "session/set_mode",
            json!({"sessionId": session.as_str(), "modeId": mode}),
        )
        .await
        .map_err(|e| refuse(e.to_string()))?;
        Ok(())
    }

    /// Sets the approval mode, the model and the options. Returns the config
    /// options as the agent last reported them. Fails when the approval mode
    /// cannot be set.
    async fn configure(
        &self,
        cx: &ConnectionTo<Agent>,
        session: &NativeRef,
        response: &Value,
        mut options: Vec<Value>,
    ) -> Result<Vec<Value>, HarnessError> {
        if self.kind == HarnessKind::ClaudeCode {
            self.set_claude_mode(cx, session, response, &mut options)
                .await?;
        }
        if !self.spec.model.is_empty()
            && let Some(option) = find_option(&options, "model", &["model"]).cloned()
            && let Some(updated) = self
                .set_soft_option(cx, session, &option, &self.spec.model)
                .await
        {
            options = updated;
        }
        for (key, value) in &self.spec.options {
            let OptionValue::Text(value) = value else {
                continue;
            };
            let option = if key == "effort" {
                find_option(&options, "thought_level", &["effort", "thought_level"])
            } else {
                options
                    .iter()
                    .find(|o| o.get("id").and_then(Value::as_str) == Some(key.as_str()))
            };
            if let Some(option) = option.cloned()
                && let Some(updated) = self.set_soft_option(cx, session, &option, value).await
            {
                options = updated;
            }
        }
        Ok(options)
    }

    async fn run(
        mut self,
        cx: ConnectionTo<Agent>,
        mut incoming: mpsc::UnboundedReceiver<Incoming>,
        mut commands: mpsc::Receiver<Command>,
        ready: oneshot::Sender<Result<Ready, HarnessError>>,
    ) -> Result<(), agent_client_protocol::Error> {
        let opened = self.open(&cx, &mut incoming).await;
        let native = match opened {
            Ok(r) => {
                let native = r.native_ref.clone();
                let _ = ready.send(Ok(r));
                native
            }
            Err(err) => {
                let _ = ready.send(Err(err));
                return Ok(());
            }
        };
        self.emit(SessionEvent::NativeRef {
            native_ref: native.clone(),
        })
        .await;
        self.emit(SessionEvent::State {
            state: SessionState::Idle,
        })
        .await;

        let mut mapper = AcpMapper::new(self.spec.session_id);
        let mut preamble = (self.kind != HarnessKind::ClaudeCode
            && self.spec.resume.is_none()
            && !self.spec.system_prompt_append.is_empty())
        .then(|| self.spec.system_prompt_append.clone());
        let mut pending: HashMap<RequestId, PendingPermission> = HashMap::new();
        let mut running: Option<TurnId> = None;
        let mut request_counter = 0_u64;
        let (done_tx, mut done_rx) =
            mpsc::unbounded_channel::<(TurnId, Result<Value, agent_client_protocol::Error>)>();
        let closed = cx.incoming_closed();
        tokio::pin!(closed);
        let mut exit = Exit::Harness;
        // Set while an interrupted turn has not ended yet.
        let mut cancel_deadline: Option<tokio::time::Instant> = None;

        loop {
            let deadline = async {
                match cancel_deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { exit = Exit::User; break };
                    match command {
                        Command::Close => { exit = Exit::User; break }
                        Command::Turn { input, reply } => {
                            if running.is_some() {
                                let _ = reply.send(Err(HarnessError::Protocol("a turn is already running".to_owned())));
                                continue;
                            }
                            let turn_id = TurnId::new(uuid::Uuid::now_v7().to_string());
                            mapper.start_turn(turn_id.clone());
                            let mut text = turn_text(&input);
                            if let Some(pre) = preamble.take() {
                                text = format!("{pre}\n\n{text}");
                            }
                            let message = match UntypedMessage::new(
                                "session/prompt",
                                json!({"sessionId": native.as_str(), "prompt": [{"type": "text", "text": text}]}),
                            ) {
                                Ok(m) => m,
                                Err(e) => { let _ = reply.send(Err(protocol_error(&e))); continue; }
                            };
                            let future = cx.send_request(message).block_task();
                            let done = done_tx.clone();
                            let id = turn_id.clone();
                            tokio::spawn(async move {
                                let result = future.await;
                                let _ = done.send((id, result));
                            });
                            running = Some(turn_id.clone());
                            self.emit(SessionEvent::TurnStarted { turn_id: turn_id.clone() }).await;
                            self.emit(SessionEvent::State { state: SessionState::Active }).await;
                            let _ = reply.send(Ok(turn_id));
                        }
                        Command::Steer { input, reply } => {
                            if !self.capabilities.steer {
                                let _ = reply.send(Err(HarnessError::Unsupported("steer")));
                                continue;
                            }
                            if running.is_none() {
                                let _ = reply.send(Err(HarnessError::Protocol("no turn is running".to_owned())));
                                continue;
                            }
                            let message = match UntypedMessage::new("_session/steering", json!({
                                "sessionId": native.as_str(),
                                "prompt": [{"type": "text", "text": turn_text(&input)}],
                                "_meta": {"steering": {"idleBehavior": "promptRequired"}}
                            })) {
                                Ok(m) => m,
                                Err(e) => { let _ = reply.send(Err(protocol_error(&e))); continue; }
                            };
                            // The answer comes in its own task, so approvals and
                            // interrupts go on while the agent takes the steer.
                            let future = cx.send_request(message).block_task();
                            tokio::spawn(async move {
                                let result = tokio::time::timeout(STEER_TIMEOUT, future).await;
                                let _ = reply.send(match result {
                                    Err(_) => Err(HarnessError::Timeout("the agent did not take the steer".to_owned())),
                                    Ok(Ok(v)) if v.get("outcome").and_then(Value::as_str) == Some("promptRequired") =>
                                        Err(HarnessError::Protocol("the turn ended before the steer arrived".to_owned())),
                                    Ok(Ok(_)) => Ok(()),
                                    Ok(Err(e)) => Err(protocol_error(&e)),
                                });
                            });
                        }
                        Command::Interrupt { reply } => {
                            for (request_id, permission) in pending.drain() {
                                let _ = permission.responder.respond(json!({"outcome": {"outcome": "cancelled"}}));
                                self.emit(SessionEvent::RequestClosed { request_id, outcome: RequestOutcome::Canceled }).await;
                            }
                            let message = UntypedMessage::new("session/cancel", json!({"sessionId": native.as_str()}));
                            let result = match message {
                                Ok(m) => cx.send_notification(m).map_err(|e| protocol_error(&e)),
                                Err(e) => Err(protocol_error(&e)),
                            };
                            if result.is_ok() && running.is_some() && cancel_deadline.is_none() {
                                cancel_deadline = Some(tokio::time::Instant::now() + INTERRUPT_GRACE);
                            }
                            let _ = reply.send(result);
                        }
                        Command::Respond { request: request_id, answer, reply } => {
                            let Some(permission) = pending.remove(&request_id) else {
                                let _ = reply.send(Err(HarnessError::UnknownRequest(request_id.to_string())));
                                continue;
                            };
                            match answer.option_id {
                                Some(option_id) => {
                                    let Some(option) = permission.options.iter().find(|o| o.id == option_id).cloned() else {
                                        pending.insert(request_id, permission);
                                        let _ = reply.send(Err(HarnessError::InvalidAnswer(format!("no option `{option_id}`"))));
                                        continue;
                                    };
                                    let sent = permission.responder.respond(json!({"outcome": {"outcome": "selected", "optionId": option_id}}));
                                    let outcome = if option.kind.is_allow() { RequestOutcome::Allowed } else { RequestOutcome::Denied };
                                    self.emit(SessionEvent::RequestClosed { request_id, outcome }).await;
                                    let _ = reply.send(sent.map_err(|e| protocol_error(&e)));
                                }
                                None => {
                                    let sent = permission.responder.respond(json!({"outcome": {"outcome": "cancelled"}}));
                                    self.emit(SessionEvent::RequestClosed { request_id, outcome: RequestOutcome::Canceled }).await;
                                    let _ = reply.send(sent.map_err(|e| protocol_error(&e)));
                                }
                            }
                        }
                    }
                }
                message = incoming.recv() => {
                    let Some(message) = message else { break };
                    match message {
                        Incoming::Update(params) => {
                            if let Some(update) = params.get("update") {
                                let events = mapper.map_update(update);
                                self.emit_all(events).await;
                            }
                        }
                        Incoming::Request { method, params, responder } => {
                            if method == "session/request_permission" {
                                request_counter += 1;
                                let request_id = RequestId::new(format!("acp-{request_counter}"));
                                let opened = mapper.map_permission(request_id.clone(), &params);
                                pending.insert(request_id, PendingPermission { responder: *responder, options: opened.options.clone() });
                                self.emit(SessionEvent::RequestOpened(opened)).await;
                            } else {
                                tracing::debug!(%method, "the ACP agent sent a request that Consort does not handle");
                                let _ = responder.respond_with_error(agent_client_protocol::Error::method_not_found());
                            }
                        }
                    }
                }
                done = done_rx.recv() => {
                    let Some((turn_id, result)) = done else { continue };
                    if running.as_ref() != Some(&turn_id) {
                        continue;
                    }
                    running = None;
                    cancel_deadline = None;
                    for (request_id, _) in pending.drain() {
                        self.emit(SessionEvent::RequestClosed { request_id, outcome: RequestOutcome::Canceled }).await;
                    }
                    let events = match result {
                        Ok(response) => {
                            let (reason, error) = stop_reason(response.get("stopReason").and_then(Value::as_str));
                            mapper.end_turn(reason, error)
                        }
                        Err(err) if err.code == ErrorCode::AuthRequired => {
                            // The login is gone or expired. Tell the worker
                            // first, then end the turn with the next step.
                            let hint = sign_in_hint(self.kind, &err.message);
                            self.emit(SessionEvent::Error {
                                code: SIGNED_OUT_ERROR_CODE.to_owned(),
                                message: hint.clone(),
                            })
                            .await;
                            mapper.end_turn(TurnEndReason::Failed, Some(hint))
                        }
                        Err(err) => mapper.end_turn(TurnEndReason::Failed, Some(err.to_string())),
                    };
                    self.emit_all(events).await;
                    self.emit(SessionEvent::State { state: SessionState::Idle }).await;
                }
                () = deadline => {
                    cancel_deadline = None;
                    if running.is_some() {
                        exit = Exit::Unresponsive;
                        break;
                    }
                }
                () = &mut closed => break,
            }
        }

        match exit {
            Exit::User => {
                self.emit(SessionEvent::State {
                    state: SessionState::Ended,
                })
                .await;
            }
            Exit::Harness | Exit::Unresponsive => {
                let (code, message) = if exit == Exit::Unresponsive {
                    (
                        "harness_unresponsive",
                        "The harness did not stop the turn after an interrupt, so the worker stopped the harness."
                            .to_owned(),
                    )
                } else {
                    let tail = self.stderr.get();
                    (
                        "harness_exited",
                        if tail.is_empty() {
                            "The harness process exited.".to_owned()
                        } else {
                            format!("The harness process exited. Its last output:\n{tail}")
                        },
                    )
                };
                if running.is_some() {
                    let error = if exit == Exit::Unresponsive {
                        "the harness did not stop the turn"
                    } else {
                        "the harness exited"
                    };
                    let events = mapper.end_turn(TurnEndReason::Failed, Some(error.to_owned()));
                    self.emit_all(events).await;
                }
                self.emit(SessionEvent::Error {
                    code: code.to_owned(),
                    message,
                })
                .await;
                self.emit(SessionEvent::State {
                    state: SessionState::Lost,
                })
                .await;
            }
        }
        Ok(())
    }
}

/// Why the session actor stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exit {
    /// `close` asked for it.
    User,
    /// The agent closed the connection.
    Harness,
    /// The agent did not end an interrupted turn in time. The connection
    /// task then kills the agent.
    Unresponsive,
}

/// The process of the agent for a session. The agent starts in the
/// session's runtime folder. The workspace goes to the agent as `cwd` in
/// `session/new`.
pub fn agent_process_spec(
    kind: HarnessKind,
    account: &AccountConfig,
    spec: &SessionSpec,
    resolve: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<ProcessSpec, HarnessError> {
    let mut env = prepare_home(account)?;
    let launch = launch_recipe(kind, account, spec.approval_mode, spec.allow_shell, resolve)?;
    env.extend(launch.env);
    env.extend(spec.env.clone());
    Ok(ProcessSpec {
        program: launch.program,
        args: launch.args,
        env,
        env_remove: Vec::new(),
        cwd: Some(spec.runtime_dir.clone()),
        watcher: spec.process_watcher.clone(),
    })
}

async fn start(
    kind: HarnessKind,
    account: &AccountConfig,
    spec: SessionSpec,
) -> Result<(AcpSession, EventReceiver, Ready), HarnessError> {
    let mut child = process::spawn(&agent_process_spec(kind, account, &spec, &resolve_program)?)?;
    let stdin = child
        .take_stdin()
        .ok_or_else(|| HarnessError::Spawn("no stdin".to_owned()))?;
    let stdout = child
        .take_stdout()
        .ok_or_else(|| HarnessError::Spawn("no stdout".to_owned()))?;
    let stderr = child.stderr_handle();
    connect(kind, spec, stdin, stdout, Some(child), stderr).await
}

/// Runs the ACP client over any byte streams. `start` passes the stdio of
/// the agent process; tests pass in-memory pipes.
async fn connect<W, R>(
    kind: HarnessKind,
    spec: SessionSpec,
    stdin: W,
    stdout: R,
    child: Option<process::HarnessProcess>,
    stderr: StderrTail,
) -> Result<(AcpSession, EventReceiver, Ready), HarnessError>
where
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
{
    let (incoming_tx, incoming_rx) = mpsc::unbounded_channel::<Incoming>();
    let (command_tx, command_rx) = mpsc::channel::<Command>(64);
    let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
    let (ready_tx, ready_rx) = oneshot::channel();

    let actor = Actor {
        kind,
        capabilities: static_capabilities(kind),
        spec,
        events: event_tx,
        stderr: stderr.clone(),
    };
    let notify_tx = incoming_tx.clone();
    let request_tx = incoming_tx;
    let label = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let label_writer = label.clone();
    let task = tokio::spawn(async move {
        let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
        let result = Client
            .builder()
            .on_receive_notification(
                async move |message: UntypedMessage, _cx| {
                    crate::record::record(
                        "acp",
                        &json!({"method": message.method, "params": message.params}),
                    );
                    if message.method == "session/update" {
                        let _ = notify_tx.send(Incoming::Update(message.params));
                    } else if message.method == "_auth/status_update"
                        && let Some(found) = auth_label(&message.params)
                        && let Ok(mut slot) = label_writer.lock()
                    {
                        *slot = Some(found);
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |message: UntypedMessage, responder: Responder<Value>, _cx| {
                    crate::record::record(
                        "acp",
                        &json!({"request": message.method, "params": message.params}),
                    );
                    let UntypedMessage { method, params } = message;
                    let _ = request_tx.send(Incoming::Request {
                        method,
                        params,
                        responder: Box::new(responder),
                    });
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
                actor.run(cx, incoming_rx, command_rx, ready_tx).await
            })
            .await;
        if let Err(err) = result {
            tracing::debug!(error = %err, "ACP connection ended with an error");
        }
        if let Some(mut child) = child {
            child.shutdown(Duration::from_secs(2)).await;
        }
    });

    let mut ready = match tokio::time::timeout(OPEN_TIMEOUT, ready_rx).await {
        Ok(Ok(Ok(ready))) => ready,
        Ok(Ok(Err(err))) => {
            task.abort();
            return Err(err);
        }
        Ok(Err(_)) => {
            task.abort();
            let tail = stderr.get();
            return Err(HarnessError::Spawn(if tail.is_empty() {
                "the agent exited before it opened a session".to_owned()
            } else {
                format!("the agent exited before it opened a session: {tail}")
            }));
        }
        Err(_) => {
            task.abort();
            return Err(HarnessError::Timeout(
                "the agent did not open a session".to_owned(),
            ));
        }
    };
    ready.auth_label = label.lock().ok().and_then(|l| l.clone());
    let session = AcpSession {
        capabilities: ready.capabilities.clone(),
        native_ref: ready.native_ref.clone(),
        commands: command_tx,
        task,
    };
    Ok((session, event_rx, ready))
}

#[async_trait]
impl Harness for AcpHarness {
    fn kind(&self) -> HarnessKind {
        self.kind
    }

    fn capabilities(&self, _account: &AccountConfig) -> Capabilities {
        static_capabilities(self.kind)
    }

    async fn probe(&self, account: &AccountConfig) -> AccountStatus {
        let caps = static_capabilities(self.kind);
        if let Err(err) = launch_recipe(
            self.kind,
            account,
            ApprovalMode::Ask,
            true,
            &resolve_program,
        ) {
            return AccountStatus::unknown(err.to_string(), caps);
        }
        let dir = std::env::temp_dir().join(format!("consort-probe-{}", uuid::Uuid::now_v7()));
        if let Err(err) = std::fs::create_dir_all(&dir) {
            return AccountStatus::unknown(err.to_string(), caps);
        }
        let spec = SessionSpec {
            session_id: consort_proto::ids::SessionId::new(),
            workspace: dir.clone(),
            runtime_dir: dir.clone(),
            model: String::new(),
            options: Default::default(),
            approval_mode: ApprovalMode::Ask,
            allow_shell: true,
            system_prompt_append: String::new(),
            mcp_servers: Vec::new(),
            env: BTreeMap::new(),
            resume: None,
            process_watcher: None,
        };
        let result = tokio::time::timeout(PROBE_TIMEOUT, start(self.kind, account, spec)).await;
        let status = match result {
            Ok(Ok((session, _events, ready))) => {
                let _ = Box::new(session).close().await;
                let mut capabilities = static_capabilities(self.kind);
                capabilities.resume = ready.capabilities.resume;
                capabilities.steer = ready.capabilities.steer;
                capabilities.mcp_http = ready.capabilities.mcp_http;
                AccountStatus {
                    harness_version: ready.agent_version,
                    auth_state: AuthState::SignedIn,
                    auth_detail: ready.auth_label.or_else(|| {
                        (self.kind == HarnessKind::OpenCode).then(|| {
                            "OpenCode signs in per model provider. Run `opencode auth list` to see which ones.".to_owned()
                        })
                    }),
                    models: models_from_config(&ready.config_options),
                    capabilities,
                }
            }
            Ok(Err(HarnessError::SignedOut(message))) => AccountStatus {
                harness_version: None,
                auth_state: AuthState::SignedOut,
                auth_detail: Some(sign_in_hint(self.kind, &message)),
                models: Vec::new(),
                capabilities: caps,
            },
            Ok(Err(err)) => AccountStatus {
                harness_version: None,
                auth_state: AuthState::Error,
                auth_detail: Some(err.to_string()),
                models: Vec::new(),
                capabilities: caps,
            },
            Err(_) => AccountStatus::unknown("The probe timed out.", caps),
        };
        let _ = std::fs::remove_dir_all(&dir);
        status
    }

    async fn open(
        &self,
        account: &AccountConfig,
        spec: SessionSpec,
    ) -> Result<(Box<dyn HarnessSession>, EventReceiver), HarnessError> {
        let (session, events, _ready) = start(self.kind, account, spec).await?;
        Ok((Box::new(session), events))
    }
}

impl AcpSession {
    async fn call<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T, HarnessError>>) -> Command,
    ) -> Result<T, HarnessError> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(make(tx))
            .await
            .map_err(|_| HarnessError::Closed)?;
        rx.await.map_err(|_| HarnessError::Closed)?
    }
}

#[async_trait]
impl HarnessSession for AcpSession {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn native_ref(&self) -> Option<NativeRef> {
        Some(self.native_ref.clone())
    }

    async fn turn(&mut self, input: TurnInput) -> Result<TurnId, HarnessError> {
        self.call(|reply| Command::Turn { input, reply }).await
    }

    async fn steer(&mut self, input: TurnInput) -> Result<(), HarnessError> {
        if !self.capabilities.steer {
            return Err(HarnessError::Unsupported("steer"));
        }
        self.call(|reply| Command::Steer { input, reply }).await
    }

    async fn interrupt(&mut self) -> Result<(), HarnessError> {
        self.call(|reply| Command::Interrupt { reply }).await
    }

    async fn respond(&mut self, request: RequestId, answer: Answer) -> Result<(), HarnessError> {
        self.call(|reply| Command::Respond {
            request,
            answer,
            reply,
        })
        .await
    }

    async fn close(self: Box<Self>) -> Result<(), HarnessError> {
        let _ = self.commands.send(Command::Close).await;
        let mut task = self.task;
        if tokio::time::timeout(Duration::from_secs(5), &mut task)
            .await
            .is_err()
        {
            task.abort();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
