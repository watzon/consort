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
use std::path::PathBuf;
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
use crate::approval::{claude_mode, gemini_mode, opencode_permission};
use crate::process::{self, ProcessSpec, StderrTail, resolve_program};
use crate::{
    AccountConfig, AccountStatus, Answer, EVENT_CHANNEL_CAPACITY, EventReceiver, Harness,
    HarnessError, HarnessKind, HarnessSession, McpServerSpec, SessionSpec, turn_text,
};
use mapper::{AcpMapper, stop_reason};

/// The pinned `claude-agent-acp` package.
pub const CLAUDE_AGENT_ACP: &str = "@agentclientprotocol/claude-agent-acp@0.99.0";

/// How long the adapter waits for the agent to start and open a session.
/// The first `npx` run downloads the package, so this is long.
const OPEN_TIMEOUT: Duration = Duration::from_secs(180);
/// How long a probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);

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
                opencode_permission(mode).to_string(),
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
        let (native_ref, response) = if let Some(resume) = &self.spec.resume {
            if !load {
                return Err(HarnessError::ResumeFailed(
                    "the agent cannot load sessions".to_owned(),
                ));
            }
            let response = request(
                cx,
                "session/load",
                json!({"sessionId": resume.as_str(), "cwd": cwd, "mcpServers": servers}),
            )
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
            if self.kind == HarnessKind::ClaudeCode && !self.spec.system_prompt_append.is_empty() {
                params["_meta"] =
                    json!({"systemPrompt": {"append": self.spec.system_prompt_append}});
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
        self.configure(cx, &native_ref, &response, &config_options)
            .await;
        Ok(Ready {
            native_ref,
            capabilities: self.capabilities.clone(),
            agent_version,
            config_options,
            auth_label: None,
        })
    }

    async fn set_option(
        &self,
        cx: &ConnectionTo<Agent>,
        session: &NativeRef,
        option: &Value,
        value: &str,
    ) {
        let Some(id) = option.get("id").and_then(Value::as_str) else {
            return;
        };
        if option.get("currentValue").and_then(Value::as_str) == Some(value) {
            return;
        }
        if !option_choices(option).iter().any(|(v, _)| v == value) {
            self.emit(SessionEvent::Error {
                code: "option_not_offered".to_owned(),
                message: format!(
                    "The harness does not offer `{value}` for `{id}`. It uses its default."
                ),
            })
            .await;
            return;
        }
        if let Err(err) = request(
            cx,
            "session/set_config_option",
            json!({"sessionId": session.as_str(), "configId": id, "value": value}),
        )
        .await
        {
            self.emit(SessionEvent::Error {
                code: "option_failed".to_owned(),
                message: format!("Could not set `{id}` to `{value}`: {err}"),
            })
            .await;
        }
    }

    /// Sets the approval mode, the model and the options.
    async fn configure(
        &self,
        cx: &ConnectionTo<Agent>,
        session: &NativeRef,
        response: &Value,
        options: &[Value],
    ) {
        if self.kind == HarnessKind::ClaudeCode {
            let mode = claude_mode(self.spec.approval_mode);
            if let Some(option) = find_option(options, "mode", &["mode"]) {
                self.set_option(cx, session, option, mode).await;
            } else if response
                .get("modes")
                .and_then(|m| m.get("availableModes"))
                .and_then(Value::as_array)
                .is_some_and(|modes| {
                    modes
                        .iter()
                        .any(|m| m.get("id").and_then(Value::as_str) == Some(mode))
                })
            {
                let _ = request(
                    cx,
                    "session/set_mode",
                    json!({"sessionId": session.as_str(), "modeId": mode}),
                )
                .await;
            }
        }
        if !self.spec.model.is_empty()
            && let Some(option) = find_option(options, "model", &["model"])
        {
            self.set_option(cx, session, option, &self.spec.model).await;
        }
        for (key, value) in &self.spec.options {
            let OptionValue::Text(value) = value else {
                continue;
            };
            let option = if key == "effort" {
                find_option(options, "thought_level", &["effort", "thought_level"])
            } else {
                options
                    .iter()
                    .find(|o| o.get("id").and_then(Value::as_str) == Some(key.as_str()))
            };
            if let Some(option) = option {
                self.set_option(cx, session, option, value).await;
            }
        }
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
        let mut by_user = false;

        loop {
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { by_user = true; break };
                    match command {
                        Command::Close => { by_user = true; break }
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
                            let result = request(&cx, "_session/steering", json!({
                                "sessionId": native.as_str(),
                                "prompt": [{"type": "text", "text": turn_text(&input)}],
                                "_meta": {"steering": {"idleBehavior": "promptRequired"}}
                            })).await;
                            let _ = reply.send(match result {
                                Ok(v) if v.get("outcome").and_then(Value::as_str) == Some("promptRequired") =>
                                    Err(HarnessError::Protocol("the turn ended before the steer arrived".to_owned())),
                                Ok(_) => Ok(()),
                                Err(e) => Err(protocol_error(&e)),
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
                    for (request_id, _) in pending.drain() {
                        self.emit(SessionEvent::RequestClosed { request_id, outcome: RequestOutcome::Canceled }).await;
                    }
                    let events = match result {
                        Ok(response) => mapper.end_turn(stop_reason(response.get("stopReason").and_then(Value::as_str)), None),
                        Err(err) => mapper.end_turn(TurnEndReason::Failed, Some(err.to_string())),
                    };
                    self.emit_all(events).await;
                    self.emit(SessionEvent::State { state: SessionState::Idle }).await;
                }
                () = &mut closed => break,
            }
        }

        if !by_user {
            if running.is_some() {
                let events =
                    mapper.end_turn(TurnEndReason::Failed, Some("the harness exited".to_owned()));
                self.emit_all(events).await;
            }
            let tail = self.stderr.get();
            self.emit(SessionEvent::Error {
                code: "harness_exited".to_owned(),
                message: if tail.is_empty() {
                    "The harness process exited.".to_owned()
                } else {
                    format!("The harness process exited. Its last output:\n{tail}")
                },
            })
            .await;
            self.emit(SessionEvent::State {
                state: SessionState::Lost,
            })
            .await;
        } else {
            self.emit(SessionEvent::State {
                state: SessionState::Ended,
            })
            .await;
        }
        Ok(())
    }
}

async fn start(
    kind: HarnessKind,
    account: &AccountConfig,
    spec: SessionSpec,
) -> Result<(AcpSession, EventReceiver, Ready), HarnessError> {
    let mut env = prepare_home(account)?;
    let launch = launch_recipe(kind, account, spec.approval_mode, &resolve_program)?;
    env.extend(launch.env);
    env.extend(spec.env.clone());
    let mut child = process::spawn(&ProcessSpec {
        program: launch.program,
        args: launch.args,
        env,
        env_remove: Vec::new(),
        cwd: Some(spec.workspace.clone()),
    })?;
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
        if let Err(err) = launch_recipe(self.kind, account, ApprovalMode::Ask, &resolve_program) {
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
            system_prompt_append: String::new(),
            mcp_servers: Vec::new(),
            env: BTreeMap::new(),
            resume: None,
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

/// What to tell the user when an ACP agent is not signed in.
fn sign_in_hint(kind: HarnessKind, message: &str) -> String {
    let command = match kind {
        HarnessKind::ClaudeCode => "Run `claude` on this machine and sign in with /login.",
        HarnessKind::OpenCode => "Run `opencode auth login` on this machine.",
        HarnessKind::GeminiCli => "Run `gemini` on this machine and sign in.",
        _ => "Sign in with the agent's own command on this machine.",
    };
    if message.is_empty() {
        command.to_owned()
    } else {
        format!("{message} {command}")
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
