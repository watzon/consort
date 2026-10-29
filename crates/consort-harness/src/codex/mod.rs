//! The Codex adapter: `codex app-server` over stdio.
//!
//! One `codex app-server` process serves one session. The hub MCP endpoint
//! goes into the `config` object of `thread/start` as `mcp_servers.<name>`,
//! with the bearer token in `http_headers`. The token so goes through the
//! stdio pipe of the process, and not in its environment, where another
//! process of the same user could read it.

pub mod login;
pub mod mapper;
pub mod protocol;
pub mod rpc;

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use async_trait::async_trait;
use consort_proto::ids::{NativeRef, RequestId, TurnId};
use consort_proto::model::{
    ApprovalMode, AuthState, Capabilities, ModelInfo, OptionChoice, OptionDescriptor, OptionValue,
    SessionState,
};
use consort_proto::worker_link::{RequestOutcome, SessionEvent, TurnEndReason, TurnInput};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::account::prepare_home;
use crate::approval::{CODEX_SHELL_FEATURES, codex_policy};
use crate::process::{
    self, HarnessProcess, ProcessSpec, parse_version, resolve_program, run_capture,
};
use crate::{
    AccountConfig, AccountStatus, Answer, EVENT_CHANNEL_CAPACITY, EventReceiver, Harness,
    HarnessError, HarnessKind, HarnessSession, McpServerSpec, SessionSpec, SteerFuture,
    sign_in_hint_for, steer_ready, turn_text,
};
use mapper::CodexMapper;
use protocol::{
    ApprovalResponse, ClientInfo, CommandApprovalParams, FileChangeApprovalParams,
    GetAccountParams, GetAccountResponse, InitializeCapabilities, InitializeParams,
    ModelListParams, ModelListResponse, PermissionsApprovalParams, PermissionsResponse,
    ThreadStartParams, ThreadStartResponse, TurnInterruptParams, TurnStartParams,
    TurnStartResponse, TurnSteerParams, UserInput, UserInputAnswer, UserInputParams,
    UserInputResponse, method,
};
use rpc::{Inbound, RpcClient};

/// How long the adapter waits for Codex to start a thread.
const OPEN_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the adapter waits for `initialize`. After that, it kills the
/// process.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a turn may run after `turn/interrupt`. After that, the adapter
/// kills the process and reports the session as lost.
const INTERRUPT_GRACE: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(30)
};
/// How long a probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a steer may wait for Codex's answer. A steer that waits longer
/// fails, so the worker does not hold it for the 90 s request limit.
const STEER_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(500)
} else {
    Duration::from_secs(10)
};

/// The Codex adapter.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodexHarness;

impl CodexHarness {
    /// A new adapter.
    pub fn new() -> Self {
        Self
    }
}

/// What Codex sessions can do.
pub fn codex_capabilities() -> Capabilities {
    Capabilities {
        steer: true,
        resume: true,
        usage: true,
        mcp_http: true,
        approval_modes: ApprovalMode::ALL.to_vec(),
        ..Capabilities::default()
    }
}

/// Codex's limit for one MCP tool call, in seconds. The hub answers a
/// connector call that waits for approval after 285 seconds, and the worker
/// gives up on a hub request after 300.
pub const MCP_TOOL_TIMEOUT_SECS: u64 = 310;

/// The `config` object of `thread/start`: the `mcp_servers`, and with
/// `allow_shell` off, the `features` that remove the shell tools.
///
/// The bearer token of a server goes in `http_headers`. Tested with
/// codex-cli 0.165.0: Codex sends it as the `Authorization` header to the
/// MCP server, and does not write it to its home folder.
///
/// `tool_timeout_sec` raises Codex's per-call limit (60 seconds by default),
/// because a connector call can wait minutes for the user's approval. It
/// stays above the hub's approval wait, so the hub answers first.
pub fn thread_config(
    servers: &[McpServerSpec],
    allow_shell: bool,
) -> serde_json::Map<String, Value> {
    let mut config = serde_json::Map::new();
    for server in servers {
        let mut entry = serde_json::Map::new();
        entry.insert("url".to_owned(), Value::String(server.url.clone()));
        entry.insert(
            "tool_timeout_sec".to_owned(),
            Value::from(MCP_TOOL_TIMEOUT_SECS),
        );
        if let Some(token) = &server.bearer_token {
            entry.insert(
                "http_headers".to_owned(),
                json!({ "Authorization": format!("Bearer {token}") }),
            );
        }
        config.insert(server.name.clone(), Value::Object(entry));
    }
    let mut root = serde_json::Map::new();
    if !config.is_empty() {
        root.insert("mcp_servers".to_owned(), Value::Object(config));
    }
    if !allow_shell {
        let features: serde_json::Map<String, Value> = CODEX_SHELL_FEATURES
            .iter()
            .map(|f| ((*f).to_owned(), Value::Bool(false)))
            .collect();
        root.insert("features".to_owned(), Value::Object(features));
    }
    root
}

/// The `config` object of a session: [`thread_config`], and with the
/// provider extras off, the keys that stop what ChatGPT sign-in syncs to the
/// account (see [`extras_off_config`]).
pub fn session_config(spec: &SessionSpec) -> serde_json::Map<String, Value> {
    let mut config = thread_config(&spec.mcp_servers, spec.allow_shell);
    if !spec.extras_enabled {
        merge_config(&mut config, extras_off_config());
    }
    config
}

/// The Codex settings that stop the account-synced extras for one thread.
///
/// With ChatGPT sign-in, Codex adds a hosted `codex_apps` MCP server (the
/// ChatGPT connectors), syncs the plugins that the account has installed
/// and, when the host gives it a provider, loads cloud skills. These keys
/// turn each of them off (`codex-rs/codex-mcp/src/mcp/mod.rs` and
/// `core-plugins/src/manager.rs` in `openai/codex`). `features.plugins`
/// is one switch for every plugin, so it also stops the plugins that the
/// user installed by hand. Tested with codex-cli 0.165.0: `thread/start`
/// accepts the `config`.
pub fn extras_off_config() -> serde_json::Map<String, Value> {
    match json!({
        "features": {"apps": false, "plugins": false},
        "cloud": {"skills": {"enabled": false}},
    }) {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    }
}

/// Adds the keys of `extra` to `config`. An object that both have is
/// merged one level down, so `features` keeps the keys of both.
fn merge_config(
    config: &mut serde_json::Map<String, Value>,
    extra: serde_json::Map<String, Value>,
) {
    for (key, value) in extra {
        match (config.get_mut(&key), value) {
            (Some(Value::Object(have)), Value::Object(add)) => {
                for (k, v) in add {
                    have.insert(k, v);
                }
            }
            (_, value) => {
                config.insert(key, value);
            }
        }
    }
}

/// The `thread/start` or `thread/resume` params for a session.
pub fn thread_params(
    spec: &SessionSpec,
    config: serde_json::Map<String, Value>,
) -> ThreadStartParams {
    let (policy, sandbox) = codex_policy(spec.approval_mode);
    ThreadStartParams {
        thread_id: spec.resume.as_ref().map(|r| r.as_str().to_owned()),
        model: (!spec.model.is_empty()).then(|| spec.model.clone()),
        cwd: Some(spec.workspace.to_string_lossy().into_owned()),
        approval_policy: Some(policy.to_owned()),
        sandbox: Some(sandbox.to_owned()),
        config: (!config.is_empty()).then_some(config),
        developer_instructions: (!spec.system_prompt_append.is_empty())
            .then(|| spec.system_prompt_append.clone()),
    }
}

/// The models from `model/list`, with an `effort` option.
pub fn models_from_list(models: &[protocol::Model]) -> Vec<ModelInfo> {
    models
        .iter()
        .filter(|m| !m.hidden)
        .map(|m| {
            let mut options = Vec::new();
            if !m.supported_reasoning_efforts.is_empty() {
                options.push(OptionDescriptor::Select {
                    id: "effort".to_owned(),
                    label: "Effort".to_owned(),
                    choices: m
                        .supported_reasoning_efforts
                        .iter()
                        .map(|e| OptionChoice {
                            value: e.reasoning_effort.clone(),
                            label: capitalize(&e.reasoning_effort),
                        })
                        .collect(),
                    default: m.default_reasoning_effort.clone(),
                });
            }
            ModelInfo {
                id: m.id.clone(),
                name: if m.display_name.is_empty() {
                    m.id.clone()
                } else {
                    m.display_name.clone()
                },
                options,
            }
        })
        .collect()
}

/// The sign-in state, email and plan from an `account/read` result. The
/// models and the capabilities stay empty.
pub fn account_status(account: &AccountConfig, info: GetAccountResponse) -> AccountStatus {
    let mut status = AccountStatus::default();
    let (auth_state, auth_detail) = match info.account {
        Some(protocol::Account::Chatgpt { email, plan_type }) => {
            status.plan = plan_type.as_deref().map(capitalize);
            let detail = match &plan_type {
                Some(plan) => format!("ChatGPT, {} plan", capitalize(plan)),
                None => "ChatGPT".to_owned(),
            };
            status.email = email;
            (AuthState::SignedIn, Some(detail))
        }
        Some(protocol::Account::ApiKey) => {
            status.plan = Some("API key".to_owned());
            (AuthState::SignedIn, Some("API key".to_owned()))
        }
        Some(protocol::Account::AmazonBedrock) => {
            (AuthState::SignedIn, Some("Amazon Bedrock".to_owned()))
        }
        Some(protocol::Account::Gcp) => (AuthState::SignedIn, Some("Google Cloud".to_owned())),
        Some(protocol::Account::Other) => (AuthState::SignedIn, None),
        None if info.requires_openai_auth => (
            AuthState::SignedOut,
            Some(sign_in_hint_for(account, "Not signed in")),
        ),
        None => (
            AuthState::SignedIn,
            Some("The model provider needs no OpenAI login.".to_owned()),
        ),
    };
    status.auth_state = auth_state;
    status.auth_detail = auth_detail;
    status
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub(crate) struct Connection {
    pub(crate) process: HarnessProcess,
    pub(crate) client: RpcClient,
    pub(crate) inbound: mpsc::UnboundedReceiver<Inbound>,
}

/// The Codex program and its first arguments: the account's `binary`, the
/// managed tool, or `codex` on `PATH`, with `app-server`. The extra
/// environment variables of the managed tool come with it.
pub fn codex_program(
    account: &AccountConfig,
    resolve: &dyn Fn(&str) -> Option<std::path::PathBuf>,
) -> Result<crate::Launch, HarnessError> {
    if let Some(launch) = account.managed_launch() {
        return Ok(launch.clone());
    }
    let name = account.binary.as_deref().unwrap_or("codex");
    let program =
        resolve(name).ok_or_else(|| HarnessError::Spawn(format!("`{name}` is not on PATH")))?;
    Ok(crate::Launch {
        program,
        args: vec!["app-server".to_owned()],
        env: BTreeMap::new(),
    })
}

/// The `codex app-server` process. It starts in `cwd`: the session's
/// runtime folder, not the workspace. The workspace goes to Codex as `cwd`
/// in `thread/start`.
pub fn app_server_spec(
    account: &AccountConfig,
    cwd: &std::path::Path,
    extra_env: BTreeMap<String, String>,
    watcher: Option<process::WatcherHandle>,
    resolve: &dyn Fn(&str) -> Option<std::path::PathBuf>,
) -> Result<ProcessSpec, HarnessError> {
    let launch = codex_program(account, resolve)?;
    let mut env = prepare_home(account)?;
    env.extend(launch.env);
    env.extend(extra_env);
    let mut args = launch.args;
    args.extend(account.args.iter().cloned());
    Ok(ProcessSpec {
        program: launch.program,
        args,
        env,
        env_remove: account.runtime.env_remove.clone(),
        cwd: Some(cwd.to_path_buf()),
        watcher,
    })
}

/// Whether a failed start of `codex app-server` came from another Codex
/// process that set up the same new `CODEX_HOME` at the same time. Tested
/// with codex-cli 0.165.0: two app-servers that start together on an empty
/// home can fail with "failed to initialize sqlite state runtime". A second
/// try works.
pub fn is_state_init_race(err: &HarnessError) -> bool {
    matches!(err, HarnessError::Spawn(m) if m.contains("failed to initialize") && m.contains("state runtime"))
}

pub(crate) async fn connect(
    account: &AccountConfig,
    cwd: &std::path::Path,
    extra_env: BTreeMap<String, String>,
    watcher: Option<process::WatcherHandle>,
) -> Result<Connection, HarnessError> {
    let mut tries = 0;
    loop {
        tries += 1;
        match connect_once(account, cwd, extra_env.clone(), watcher.clone()).await {
            Err(err) if tries < 3 && is_state_init_race(&err) => {
                tracing::debug!("codex app-server could not set up its home; it tries again");
                tokio::time::sleep(Duration::from_millis(500 * tries)).await;
            }
            result => return result,
        }
    }
}

async fn connect_once(
    account: &AccountConfig,
    cwd: &std::path::Path,
    extra_env: BTreeMap<String, String>,
    watcher: Option<process::WatcherHandle>,
) -> Result<Connection, HarnessError> {
    let mut process = process::spawn(&app_server_spec(
        account,
        cwd,
        extra_env,
        watcher,
        &resolve_program,
    )?)?;
    let stdin = process
        .take_stdin()
        .ok_or_else(|| HarnessError::Spawn("no stdin".to_owned()))?;
    let stdout = process
        .take_stdout()
        .ok_or_else(|| HarnessError::Spawn("no stdout".to_owned()))?;
    let (client, inbound) = RpcClient::start(stdin, stdout);
    let init = InitializeParams {
        client_info: ClientInfo {
            name: "consort".to_owned(),
            title: "Consort".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        capabilities: InitializeCapabilities {
            experimental_api: true,
            opt_out_notification_methods: Vec::new(),
        },
    };
    let initialized: Result<Value, HarnessError> = match tokio::time::timeout(
        INITIALIZE_TIMEOUT,
        client.request(method::INITIALIZE, &init),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(HarnessError::Timeout(
            "codex app-server did not answer `initialize`".to_owned(),
        )),
    };
    if let Err(err) = initialized {
        let tail = process.stderr_tail();
        process.kill().await;
        return Err(match err {
            HarnessError::Closed => HarnessError::Spawn(format!("codex app-server exited: {tail}")),
            other => other,
        });
    }
    client.notify(method::INITIALIZED, &Value::Null)?;
    Ok(Connection {
        process,
        client,
        inbound,
    })
}

#[async_trait]
impl Harness for CodexHarness {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Codex
    }

    fn capabilities(&self, _account: &AccountConfig) -> Capabilities {
        codex_capabilities()
    }

    fn managed_tool(&self, account: &AccountConfig) -> Option<crate::ToolId> {
        account.managed_tool()
    }

    async fn probe(&self, account: &AccountConfig) -> AccountStatus {
        let caps = codex_capabilities();
        let launch = match codex_program(account, &resolve_program) {
            Ok(launch) => launch,
            Err(err) => return AccountStatus::unknown(err.to_string(), caps),
        };
        let mut env = match prepare_home(account) {
            Ok(env) => env,
            Err(err) => return AccountStatus::unknown(err.to_string(), caps),
        };
        env.extend(launch.env.clone());
        let version = run_capture(
            &launch.program,
            &["--version"],
            &env,
            Duration::from_secs(10),
        )
        .await
        .ok()
        .and_then(|out| parse_version(&out));
        let dir = std::env::temp_dir();
        let probe = async {
            let mut conn = connect(account, &dir, BTreeMap::new(), None).await?;
            let account_info: GetAccountResponse = conn
                .client
                .request(
                    method::ACCOUNT_READ,
                    &GetAccountParams {
                        refresh_token: false,
                    },
                )
                .await?;
            let mut models = Vec::new();
            let mut cursor = None;
            for _ in 0..10 {
                let page: Result<ModelListResponse, HarnessError> = conn
                    .client
                    .request(
                        method::MODEL_LIST,
                        &ModelListParams {
                            cursor: cursor.clone(),
                        },
                    )
                    .await;
                let Ok(page) = page else { break };
                models.extend(page.data);
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            conn.process.shutdown(Duration::from_secs(1)).await;
            drop(conn.inbound);
            Ok::<_, HarnessError>((account_info, models))
        };
        match tokio::time::timeout(PROBE_TIMEOUT, probe).await {
            Ok(Ok((info, models))) => {
                let mut status = account_status(account, info);
                status.harness_version = version;
                status.models = models_from_list(&models);
                status.capabilities = caps;
                status
            }
            Ok(Err(err)) => AccountStatus {
                harness_version: version,
                auth_state: AuthState::Error,
                auth_detail: Some(err.to_string()),
                capabilities: caps,
                ..AccountStatus::default()
            },
            Err(_) => AccountStatus {
                harness_version: version,
                auth_state: AuthState::Unknown,
                auth_detail: Some("The probe timed out.".to_owned()),
                capabilities: caps,
                ..AccountStatus::default()
            },
        }
    }

    async fn open(
        &self,
        account: &AccountConfig,
        spec: SessionSpec,
    ) -> Result<(Box<dyn HarnessSession>, EventReceiver), HarnessError> {
        let config = session_config(&spec);
        let conn = connect(
            account,
            &spec.runtime_dir,
            spec.env.clone(),
            spec.process_watcher.clone(),
        )
        .await?;
        let session =
            start_thread(spec, config, conn.client, conn.inbound, Some(conn.process)).await?;
        Ok((Box::new(session.0), session.1))
    }
}

/// Starts or resumes the thread on an initialized connection and starts
/// the session actor. Tests pass a connection to a fake app-server and no
/// process.
pub(crate) async fn start_thread(
    spec: SessionSpec,
    config: serde_json::Map<String, Value>,
    client: RpcClient,
    inbound: mpsc::UnboundedReceiver<Inbound>,
    process: Option<HarnessProcess>,
) -> Result<(CodexSession, EventReceiver), HarnessError> {
    let mut process = process;
    // The skills of the session are an extra skill root of this app-server,
    // which serves this session only. Codex reads no setting for it.
    let skills_note = match &spec.skills {
        Some(skills) => {
            let roots = json!({"extraRoots": [crate::skills::codex_extra_root(skills)]});
            match tokio::time::timeout(
                OPEN_TIMEOUT,
                client.request::<Value, Value>(method::SKILLS_EXTRA_ROOTS_SET, &roots),
            )
            .await
            {
                Ok(Ok(_)) => None,
                Ok(Err(err)) => Some(err.to_string()),
                Err(_) => Some("codex did not answer in time".to_owned()),
            }
        }
        None => None,
    };
    let params = thread_params(&spec, config);
    let method_name = if spec.resume.is_some() {
        method::THREAD_RESUME
    } else {
        method::THREAD_START
    };
    let started: Result<ThreadStartResponse, HarnessError> =
        match tokio::time::timeout(OPEN_TIMEOUT, client.request(method_name, &params)).await {
            Ok(r) => r,
            Err(_) => Err(HarnessError::Timeout(
                "codex did not start the thread".to_owned(),
            )),
        };
    let started = match started {
        Ok(s) => s,
        Err(err) => {
            if let Some(p) = process.as_mut() {
                p.kill().await;
            }
            return Err(if spec.resume.is_some() {
                HarnessError::ResumeFailed(err.to_string())
            } else {
                err
            });
        }
    };
    let thread_id = started.thread.id;
    let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
    let (command_tx, command_rx) = mpsc::channel(64);
    if let Some(why) = skills_note {
        // The thread runs without its Consort skills. The user sees why.
        let _ = event_tx.try_send(SessionEvent::Error {
            code: "skills_not_delivered".to_owned(),
            message: format!("Codex did not take the skill folder of this session ({why}), so the session runs without these skills."),
        });
    }
    let native_ref = NativeRef::new(&thread_id);
    let effort = match spec.options.get("effort") {
        Some(OptionValue::Text(e)) => Some(e.clone()),
        _ => None,
    };
    let actor = Actor {
        thread_id,
        client,
        events: event_tx,
        mapper: CodexMapper::new(spec.session_id).with_workspace(&spec.workspace),
        pending: HashMap::new(),
        effort,
        counter: 0,
    };
    let task = tokio::spawn(actor.run(process, inbound, command_rx));
    Ok((
        CodexSession {
            capabilities: codex_capabilities(),
            native_ref,
            commands: command_tx,
            task,
        },
        event_rx,
    ))
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

/// What a pending server request is.
enum PendingKind {
    /// A command or file change approval. The option ID is the decision.
    Decision,
    /// A permissions request, with the requested permissions.
    Permissions(Value),
    /// One question of a user input request.
    Question {
        /// The shared key of the user input request.
        group: String,
        /// The question ID.
        question_id: String,
        /// The option labels by option ID.
        labels: Vec<String>,
    },
}

struct Pending {
    rpc_id: Value,
    kind: PendingKind,
    options: Vec<consort_proto::model::RequestOption>,
}

/// The answers collected for one user input request.
struct QuestionGroup {
    rpc_id: Value,
    remaining: usize,
    answers: BTreeMap<String, UserInputAnswer>,
}

struct Actor {
    thread_id: String,
    client: RpcClient,
    events: mpsc::Sender<SessionEvent>,
    mapper: CodexMapper,
    pending: HashMap<RequestId, Pending>,
    effort: Option<String>,
    counter: u64,
}

impl Actor {
    async fn emit(&self, event: SessionEvent) {
        let _ = self.events.send(event).await;
    }

    fn next_request_id(&mut self) -> RequestId {
        self.counter += 1;
        RequestId::new(format!("codex-{}", self.counter))
    }

    async fn run(
        mut self,
        mut process: Option<HarnessProcess>,
        mut inbound: mpsc::UnboundedReceiver<Inbound>,
        mut commands: mpsc::Receiver<Command>,
    ) {
        self.emit(SessionEvent::NativeRef {
            native_ref: NativeRef::new(&self.thread_id),
        })
        .await;
        self.emit(SessionEvent::State {
            state: SessionState::Idle,
        })
        .await;
        let mut groups: HashMap<String, QuestionGroup> = HashMap::new();
        let mut exit = Exit::Harness;
        // Set while an interrupted turn has not ended yet.
        let mut cancel_deadline: Option<tokio::time::Instant> = None;
        loop {
            if self.mapper.current_turn().is_none() {
                cancel_deadline = None;
            }
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
                            let _ = reply.send(self.start_turn(&input).await);
                        }
                        // Steer and interrupt answer in their own tasks, so
                        // approvals go on while Codex takes them.
                        Command::Steer { input, reply } => {
                            match self.mapper.current_turn().map(str::to_owned) {
                                None => {
                                    let _ = reply.send(Err(HarnessError::Protocol("no turn is running".to_owned())));
                                }
                                Some(turn) => {
                                    let client = self.client.clone().with_timeout(STEER_TIMEOUT);
                                    let params = TurnSteerParams {
                                        thread_id: self.thread_id.clone(),
                                        input: vec![UserInput::text(turn_text(&input))],
                                        expected_turn_id: turn,
                                    };
                                    tokio::spawn(async move {
                                        let result = client
                                            .request::<_, Value>(method::TURN_STEER, &params)
                                            .await
                                            .map(|_| ());
                                        let _ = reply.send(result);
                                    });
                                }
                            }
                        }
                        Command::Interrupt { reply } => {
                            match self.mapper.current_turn().map(str::to_owned) {
                                None => {
                                    let _ = reply.send(Ok(()));
                                }
                                Some(turn) => {
                                    if cancel_deadline.is_none() {
                                        cancel_deadline = Some(tokio::time::Instant::now() + INTERRUPT_GRACE);
                                    }
                                    let client = self.client.clone();
                                    let params = TurnInterruptParams {
                                        thread_id: self.thread_id.clone(),
                                        turn_id: turn,
                                    };
                                    tokio::spawn(async move {
                                        let result = client
                                            .request::<_, Value>(method::TURN_INTERRUPT, &params)
                                            .await
                                            .map(|_| ());
                                        let _ = reply.send(result);
                                    });
                                }
                            }
                        }
                        Command::Respond { request, answer, reply } => {
                            let _ = reply.send(self.respond(request, answer, &mut groups).await);
                        }
                    }
                }
                message = inbound.recv() => {
                    let Some(message) = message else { break };
                    match message {
                        Inbound::Notification { method: name, params } => {
                            if name == method::SERVER_REQUEST_RESOLVED {
                                self.resolved(params.get("requestId")).await;
                                continue;
                            }
                            let events = self.mapper.map_notification(&name, &params);
                            for event in events {
                                self.emit(event).await;
                            }
                        }
                        Inbound::Request { id, method: name, params } => {
                            self.server_request(id, &name, &params, &mut groups).await;
                        }
                    }
                }
                () = deadline => {
                    cancel_deadline = None;
                    if self.mapper.current_turn().is_some() {
                        exit = Exit::Unresponsive;
                        break;
                    }
                }
            }
        }
        if exit == Exit::User {
            if let Some(p) = process.as_mut() {
                p.shutdown(Duration::from_secs(2)).await;
            }
            self.emit(SessionEvent::State {
                state: SessionState::Ended,
            })
            .await;
            return;
        }
        let mut tail = String::new();
        if let Some(p) = process.as_mut() {
            tail = p.stderr_tail();
            p.kill().await;
        }
        let (code, message, turn_error) = if exit == Exit::Unresponsive {
            (
                "harness_unresponsive",
                "Codex did not stop the turn after an interrupt, so the worker stopped codex app-server."
                    .to_owned(),
                "codex did not stop the turn",
            )
        } else {
            (
                "harness_exited",
                if tail.is_empty() {
                    "codex app-server exited.".to_owned()
                } else {
                    format!("codex app-server exited. Its last output:\n{tail}")
                },
                "codex app-server exited",
            )
        };
        if let Some(turn) = self.mapper.current_turn().map(str::to_owned) {
            self.emit(SessionEvent::TurnEnded {
                turn_id: TurnId::new(turn),
                reason: TurnEndReason::Failed,
                error: Some(turn_error.to_owned()),
            })
            .await;
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

    async fn start_turn(&mut self, input: &TurnInput) -> Result<TurnId, HarnessError> {
        if self.mapper.current_turn().is_some() {
            return Err(HarnessError::Protocol(
                "a turn is already running".to_owned(),
            ));
        }
        let response: TurnStartResponse = self
            .client
            .request(
                method::TURN_START,
                &TurnStartParams {
                    thread_id: self.thread_id.clone(),
                    input: vec![UserInput::text(turn_text(input))],
                    effort: self.effort.clone(),
                },
            )
            .await?;
        self.mapper.turn_accepted(&response.turn.id);
        Ok(TurnId::new(response.turn.id))
    }

    async fn resolved(&mut self, rpc_id: Option<&Value>) {
        let Some(rpc_id) = rpc_id else { return };
        let closed: Vec<RequestId> = self
            .pending
            .iter()
            .filter(|(_, p)| &p.rpc_id == rpc_id)
            .map(|(id, _)| id.clone())
            .collect();
        for request_id in closed {
            self.pending.remove(&request_id);
            self.emit(SessionEvent::RequestClosed {
                request_id,
                outcome: RequestOutcome::Canceled,
            })
            .await;
        }
    }

    async fn server_request(
        &mut self,
        id: Value,
        name: &str,
        params: &Value,
        groups: &mut HashMap<String, QuestionGroup>,
    ) {
        match name {
            method::COMMAND_APPROVAL => {
                let Ok(p) = serde_json::from_value::<CommandApprovalParams>(params.clone()) else {
                    let _ = self.client.respond_error(&id, -32602, "invalid params");
                    return;
                };
                let request_id = self.next_request_id();
                let opened = self.mapper.command_approval(request_id.clone(), &p);
                self.pending.insert(
                    request_id,
                    Pending {
                        rpc_id: id,
                        kind: PendingKind::Decision,
                        options: opened.options.clone(),
                    },
                );
                self.emit(SessionEvent::RequestOpened(opened)).await;
            }
            method::FILE_CHANGE_APPROVAL => {
                let Ok(p) = serde_json::from_value::<FileChangeApprovalParams>(params.clone())
                else {
                    let _ = self.client.respond_error(&id, -32602, "invalid params");
                    return;
                };
                let request_id = self.next_request_id();
                let opened = self.mapper.file_change_approval(request_id.clone(), &p);
                self.pending.insert(
                    request_id,
                    Pending {
                        rpc_id: id,
                        kind: PendingKind::Decision,
                        options: opened.options.clone(),
                    },
                );
                self.emit(SessionEvent::RequestOpened(opened)).await;
            }
            method::PERMISSIONS_APPROVAL => {
                let Ok(p) = serde_json::from_value::<PermissionsApprovalParams>(params.clone())
                else {
                    let _ = self.client.respond_error(&id, -32602, "invalid params");
                    return;
                };
                let request_id = self.next_request_id();
                let opened = CodexMapper::permissions_approval(request_id.clone(), &p);
                self.pending.insert(
                    request_id,
                    Pending {
                        rpc_id: id,
                        kind: PendingKind::Permissions(p.permissions),
                        options: opened.options.clone(),
                    },
                );
                self.emit(SessionEvent::RequestOpened(opened)).await;
            }
            method::USER_INPUT => {
                let Ok(p) = serde_json::from_value::<UserInputParams>(params.clone()) else {
                    let _ = self.client.respond_error(&id, -32602, "invalid params");
                    return;
                };
                let group = id.to_string();
                groups.insert(
                    group.clone(),
                    QuestionGroup {
                        rpc_id: id.clone(),
                        remaining: p.questions.len(),
                        answers: BTreeMap::new(),
                    },
                );
                if p.questions.is_empty() {
                    let _ = self.client.respond(
                        &id,
                        &UserInputResponse {
                            answers: BTreeMap::new(),
                        },
                    );
                    groups.remove(&group);
                    return;
                }
                for question in &p.questions {
                    let request_id = self.next_request_id();
                    let opened = CodexMapper::question(request_id.clone(), question);
                    let labels = opened.options.iter().map(|o| o.label.clone()).collect();
                    self.pending.insert(
                        request_id,
                        Pending {
                            rpc_id: id.clone(),
                            kind: PendingKind::Question {
                                group: group.clone(),
                                question_id: question.id.clone(),
                                labels,
                            },
                            options: opened.options.clone(),
                        },
                    );
                    self.emit(SessionEvent::RequestOpened(opened)).await;
                }
            }
            method::MCP_ELICITATION => {
                let _ = self.client.respond(&id, &json!({"action": "decline"}));
                self.emit(SessionEvent::Error {
                    code: "elicitation_declined".to_owned(),
                    message: "An MCP server asked for input. Consort does not support MCP elicitation yet, so the worker declined it.".to_owned(),
                })
                .await;
            }
            other => {
                tracing::debug!(
                    method = other,
                    "codex sent a request that Consort does not handle"
                );
                let _ =
                    self.client
                        .respond_error(&id, -32601, "Consort does not support this request");
            }
        }
    }

    async fn respond(
        &mut self,
        request_id: RequestId,
        answer: Answer,
        groups: &mut HashMap<String, QuestionGroup>,
    ) -> Result<(), HarnessError> {
        let Some(pending) = self.pending.remove(&request_id) else {
            return Err(HarnessError::UnknownRequest(request_id.to_string()));
        };
        let chosen = match &answer.option_id {
            Some(id) => match pending.options.iter().find(|o| &o.id == id) {
                Some(o) => Some(o.clone()),
                None => {
                    let id = id.clone();
                    self.pending.insert(request_id, pending);
                    return Err(HarnessError::InvalidAnswer(format!("no option `{id}`")));
                }
            },
            None => None,
        };
        let outcome = match &pending.kind {
            PendingKind::Decision => {
                let decision = chosen
                    .as_ref()
                    .map(|o| o.id.clone())
                    .unwrap_or_else(|| "cancel".to_owned());
                self.client
                    .respond(&pending.rpc_id, &ApprovalResponse { decision })?;
                match &chosen {
                    Some(o) if o.kind.is_allow() => RequestOutcome::Allowed,
                    Some(_) => RequestOutcome::Denied,
                    None => RequestOutcome::Canceled,
                }
            }
            PendingKind::Permissions(requested) => {
                let allow = chosen.as_ref().is_some_and(|o| o.kind.is_allow());
                let scope = if chosen.as_ref().is_some_and(|o| o.id == "session") {
                    "session"
                } else {
                    "turn"
                };
                let permissions = if allow { requested.clone() } else { json!({}) };
                self.client.respond(
                    &pending.rpc_id,
                    &PermissionsResponse {
                        permissions,
                        scope: scope.to_owned(),
                    },
                )?;
                if allow {
                    RequestOutcome::Allowed
                } else {
                    RequestOutcome::Denied
                }
            }
            PendingKind::Question {
                group,
                question_id,
                labels,
            } => {
                let mut values = Vec::new();
                if let Some(o) = &chosen
                    && let Ok(index) = o.id.parse::<usize>()
                    && let Some(label) = labels.get(index)
                {
                    values.push(label.clone());
                }
                if let Some(text) = answer.text.as_ref().filter(|t| !t.trim().is_empty()) {
                    values.push(text.clone());
                }
                if values.is_empty() {
                    self.pending.insert(request_id, pending);
                    return Err(HarnessError::InvalidAnswer(
                        "a question needs an option or text".to_owned(),
                    ));
                }
                if let Some(entry) = groups.get_mut(group) {
                    entry
                        .answers
                        .insert(question_id.clone(), UserInputAnswer { answers: values });
                    entry.remaining = entry.remaining.saturating_sub(1);
                    if entry.remaining == 0
                        && let Some(done) = groups.remove(group)
                    {
                        self.client.respond(
                            &done.rpc_id,
                            &UserInputResponse {
                                answers: done.answers,
                            },
                        )?;
                    }
                }
                RequestOutcome::Answered
            }
        };
        self.emit(SessionEvent::RequestClosed {
            request_id,
            outcome,
        })
        .await;
        Ok(())
    }
}

/// Why the session actor stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exit {
    /// `close` asked for it.
    User,
    /// codex app-server closed its stdout.
    Harness,
    /// Codex did not end an interrupted turn in time.
    Unresponsive,
}

/// One live Codex session.
pub struct CodexSession {
    capabilities: Capabilities,
    native_ref: NativeRef,
    commands: mpsc::Sender<Command>,
    task: tokio::task::JoinHandle<()>,
}

impl CodexSession {
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
impl HarnessSession for CodexSession {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn native_ref(&self) -> Option<NativeRef> {
        Some(self.native_ref.clone())
    }

    async fn turn(&mut self, input: TurnInput) -> Result<TurnId, HarnessError> {
        self.call(|reply| Command::Turn { input, reply }).await
    }

    fn steer(&mut self, input: TurnInput) -> SteerFuture {
        // The command goes in the queue now, so it keeps its place before
        // a later interrupt. Only the answer is waited for later.
        let (reply, answer) = oneshot::channel();
        if let Err(err) = self.commands.try_send(Command::Steer { input, reply }) {
            return steer_ready(Err(match err {
                mpsc::error::TrySendError::Closed(_) => HarnessError::Closed,
                mpsc::error::TrySendError::Full(_) => {
                    HarnessError::Protocol("the session has too many commands waiting".to_owned())
                }
            }));
        }
        Box::pin(async move { answer.await.map_err(|_| HarnessError::Closed)? })
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
