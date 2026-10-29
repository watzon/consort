//! The session manager: one actor per session that runs the hub commands
//! for that session in order, and one pump per session that numbers the
//! harness events, writes them to the spool and wakes the sender.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use consort_harness::approval::effective_mode;
use consort_harness::{
    EventReceiver, HarnessSession, McpServerSpec, SIGNED_OUT_ERROR_CODE, SessionSpec, StdioCommand,
};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::FrameId;
use consort_proto::ids::{AccountId, ItemId, RequestId, SessionId};
use consort_proto::model::{
    ApprovalDetails, ApprovalMode, Capabilities, RequestOptionKind, SessionState, Sha256Hash,
};
use consort_proto::tools::{ToolCallResult, ToolDefinition};
use consort_proto::worker_link::{
    HubToWorker, OUTPUT_DELTA_INTERVAL_MS, RequestAnswer, RequestBody, SessionEvent, SessionStart,
    SessionStartResult, SessionSteer, SessionTurnResult, ToolCall, ToolList, TurnInput,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::config::{RUNTIME_DIR, create_private_dir, local_server_name_problem};
use crate::core::{Core, GroupLog};
use crate::mcp::{self, McpEndpoint, ToolBackend, ToolCache};
use crate::store::{SessionRecord, WorkspaceEntry};
use crate::{WorkerError, workspace};

/// Work for a session actor.
pub(crate) enum Job {
    /// A hub command, with its command ID.
    Command {
        id: FrameId,
        command: Box<HubToWorker>,
    },
    /// Resume a session after the worker restarted.
    Restore(Box<SessionRecord>),
    /// Deny a request because of worker policy.
    PolicyDeny {
        request_id: RequestId,
        option_id: Option<String>,
    },
    /// Allow a call to a hub tool. The hub checks its own tools, so the
    /// user never sees an approval for them.
    AllowHubTool {
        request_id: RequestId,
        option_id: String,
    },
    /// Deny an approval that is too long to show in full. `reason` goes to
    /// the model as a steer when the harness can steer.
    DenyTooLong {
        request_id: RequestId,
        option_id: Option<String>,
        reason: String,
    },
    /// The harness reported the session as lost. Close what is left of it.
    HarnessLost,
    /// Close the harness but keep the session for resume.
    Shutdown,
}

/// A running session actor.
pub(crate) struct SessionHandle {
    pub jobs: mpsc::UnboundedSender<Job>,
    pub task: tokio::task::JoinHandle<()>,
}

/// Merges events that replace each other, so the worker sends at most one
/// update per item and one output delta per command every 250 ms.
#[derive(Debug, Default)]
pub struct Coalescer {
    buffer: Vec<SessionEvent>,
    items: HashMap<ItemId, usize>,
    deltas: HashMap<ItemId, usize>,
}

impl Coalescer {
    /// Adds an event. Returns the events to write now, in order.
    pub fn push(&mut self, event: SessionEvent) -> Vec<SessionEvent> {
        match event {
            SessionEvent::Item(item) => {
                if let Some(&i) = self.items.get(&item.id) {
                    self.buffer[i] = SessionEvent::Item(item);
                } else {
                    self.items.insert(item.id, self.buffer.len());
                    self.buffer.push(SessionEvent::Item(item));
                }
                Vec::new()
            }
            SessionEvent::OutputDelta { item_id, text } => {
                if let Some(&i) = self.deltas.get(&item_id)
                    && let SessionEvent::OutputDelta { text: existing, .. } = &mut self.buffer[i]
                {
                    existing.push_str(&text);
                } else {
                    self.deltas.insert(item_id, self.buffer.len());
                    self.buffer
                        .push(SessionEvent::OutputDelta { item_id, text });
                }
                Vec::new()
            }
            SessionEvent::Usage(usage) => {
                // Usage is a total, so only the newest one matters.
                self.buffer.retain(|e| !matches!(e, SessionEvent::Usage(_)));
                self.rebuild_index();
                self.buffer.push(SessionEvent::Usage(usage));
                Vec::new()
            }
            other => {
                let mut out = self.flush();
                out.push(other);
                out
            }
        }
    }

    fn rebuild_index(&mut self) {
        self.items.clear();
        self.deltas.clear();
        for (i, e) in self.buffer.iter().enumerate() {
            match e {
                SessionEvent::Item(item) => {
                    self.items.insert(item.id, i);
                }
                SessionEvent::OutputDelta { item_id, .. } => {
                    self.deltas.insert(*item_id, i);
                }
                _ => {}
            }
        }
    }

    /// Whether events wait in the buffer.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Takes every buffered event.
    pub fn flush(&mut self) -> Vec<SessionEvent> {
        self.items.clear();
        self.deltas.clear();
        std::mem::take(&mut self.buffer)
    }
}

/// The name of the MCP server that carries the hub tools.
pub(crate) const HUB_SERVER: &str = "hub";

/// The allow-once option of an approval for a hub tool, if the request is
/// one. Falls back to an allow-always option when the harness offers no
/// allow-once option.
///
/// The request is for a hub tool only when the server is `hub` and the tool
/// is in the session's hub tool list (`hub_tools`, from `tool.list`). The
/// server name comes from a tool name such as `mcp__hub__post_message`, so a
/// local server named `hub__x` would give the server `hub`; the list check
/// stops that. Before the harness lists the tools, nothing is allowed here.
fn hub_tool_allow(
    opened: &consort_proto::worker_link::RequestOpened,
    hub_tools: Option<&[ToolDefinition]>,
) -> Option<String> {
    let RequestBody::Approval(ApprovalDetails::Tool {
        server: Some(server),
        tool,
        ..
    }) = &opened.body
    else {
        return None;
    };
    if server != HUB_SERVER || !hub_tools?.iter().any(|t| &t.name == tool) {
        return None;
    }
    let pick = |kind: RequestOptionKind| {
        opened
            .options
            .iter()
            .find(|o| o.kind == kind)
            .map(|o| o.id.clone())
    };
    pick(RequestOptionKind::AllowOnce).or_else(|| pick(RequestOptionKind::AllowAlways))
}

/// The deny option of a request, for a policy denial.
fn deny_option(options: &[consort_proto::model::RequestOption]) -> Option<String> {
    options
        .iter()
        .find(|o| o.kind.is_deny())
        .map(|o| o.id.clone())
}

/// How long the pump holds the first event that says the harness exited,
/// to see whether the worker stops. A harness that gets the same stop
/// signal as the worker can exit a moment before the worker sets its
/// stopping flag.
pub(crate) const EXIT_HOLD: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(1)
};

/// How long the worker waits for the answer to a steer. The adapters have
/// their own shorter limits, so this only catches an adapter that never
/// answers.
const STEER_LIMIT: Duration = Duration::from_secs(15);

/// Whether an event says that the harness process is gone. While the worker
/// stops, these events do not reach the spool, so the session stays open
/// for the next run.
fn is_exit_event(event: &SessionEvent) -> bool {
    match event {
        SessionEvent::State { state } => *state == SessionState::Lost,
        SessionEvent::Error { code, .. } => {
            code == "harness_exited" || code == "harness_unresponsive"
        }
        _ => false,
    }
}

/// What the worker tells the user and the model when it denies an
/// approval that is too long to show.
fn too_long_reason(body: &RequestBody) -> String {
    match body {
        RequestBody::Approval(ApprovalDetails::Command { .. }) => {
            "The command is too long to show for approval, so the worker denied it. Split it into smaller steps."
        }
        _ => {
            "The request is too long to show for approval, so the worker denied it. Split the work into smaller steps."
        }
    }
    .to_owned()
}

/// A name for an approval mode in a message.
fn mode_name(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "Ask",
        ApprovalMode::AutoEdits => "Auto edits",
        ApprovalMode::Auto => "Auto",
        ApprovalMode::FullAccess => "Full access",
    }
}

/// Reads harness events until the harness closes the channel.
async fn pump(
    core: Arc<Core>,
    session_id: SessionId,
    account_id: AccountId,
    mut events: EventReceiver,
    jobs: mpsc::UnboundedSender<Job>,
    deny_commands: bool,
    hub_tools: ToolCache,
) {
    let mut coalescer = Coalescer::default();
    let interval = Duration::from_millis(OUTPUT_DELTA_INTERVAL_MS);
    let mut deadline: Option<tokio::time::Instant> = None;
    let mut exit_held = false;
    loop {
        let sleep = async {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break };
                if is_exit_event(&event) {
                    let hold = if exit_held { Duration::ZERO } else { EXIT_HOLD };
                    exit_held = true;
                    if core.stops_within(hold).await {
                        tracing::info!(session = %session_id, "the harness exited while the worker stops; the worker keeps the session for the next run");
                        continue;
                    }
                }
                // The harness says that the login is gone or expired. Update
                // the inventory now, so the hub stops placing work here.
                if let SessionEvent::Error { code, message } = &event
                    && code == SIGNED_OUT_ERROR_CODE
                {
                    core.mark_signed_out(account_id, message);
                }
                if let SessionEvent::RequestOpened(opened) = &event {
                    let allow = {
                        let tools = hub_tools.lock().await;
                        hub_tool_allow(opened, tools.as_deref())
                    };
                    if let Some(option_id) = allow {
                        let _ = jobs.send(Job::AllowHubTool { request_id: opened.request_id.clone(), option_id });
                        continue;
                    }
                    // The worker would have to cut this approval to send it,
                    // so the user would allow a command or a change that
                    // they cannot see in full. The worker denies it and
                    // says why.
                    if matches!(opened.body, RequestBody::Approval(_))
                        && crate::cap::request_must_be_cut(opened)
                    {
                        let reason = too_long_reason(&opened.body);
                        let note = SessionEvent::Error { code: "approval_too_long".to_owned(), message: reason.clone() };
                        for ready in coalescer.push(note) {
                            core.record_event(session_id, &ready);
                        }
                        deadline = None;
                        let _ = jobs.send(Job::DenyTooLong {
                            request_id: opened.request_id.clone(),
                            option_id: deny_option(&opened.options),
                            reason,
                        });
                        continue;
                    }
                }
                let lost = matches!(event, SessionEvent::State { state: SessionState::Lost });
                let mut policy_deny = None;
                if deny_commands
                    && let SessionEvent::RequestOpened(opened) = &event
                    && matches!(opened.body, RequestBody::Approval(ApprovalDetails::Command { .. }))
                {
                    policy_deny = Some((opened.request_id.clone(), deny_option(&opened.options)));
                }
                for ready in coalescer.push(event) {
                    core.record_event(session_id, &ready);
                }
                if coalescer.is_empty() {
                    deadline = None;
                } else if deadline.is_none() {
                    deadline = Some(tokio::time::Instant::now() + interval);
                }
                if let Some((request_id, option_id)) = policy_deny {
                    let _ = jobs.send(Job::PolicyDeny { request_id, option_id });
                }
                if lost {
                    let _ = jobs.send(Job::HarnessLost);
                }
            }
            () = sleep => {
                for ready in coalescer.flush() {
                    core.record_event(session_id, &ready);
                }
                deadline = None;
            }
        }
    }
    for ready in coalescer.flush() {
        core.record_event(session_id, &ready);
    }
}

/// The hub tools through the worker link.
struct LinkTools {
    core: Arc<Core>,
}

fn offline_body(err: WorkerError) -> ErrorBody {
    match err {
        WorkerError::Offline => ErrorBody::new(
            ErrorCode::WorkerOffline,
            "The worker is not connected to the hub. Try again in a minute.",
        ),
        other => other.to_error_body(),
    }
}

#[async_trait]
impl ToolBackend for LinkTools {
    async fn list_tools(&self, session_id: SessionId) -> Result<Vec<ToolDefinition>, ErrorBody> {
        self.core
            .link
            .request(&ToolList { session_id })
            .await
            .map(|r| r.tools)
            .map_err(offline_body)
    }

    async fn call_tool(&self, call: ToolCall) -> Result<ToolCallResult, ErrorBody> {
        self.core.link.request(&call).await.map_err(offline_body)
    }

    async fn upload_blob(&self, sha256: &Sha256Hash, bytes: Vec<u8>) -> Result<(), ErrorBody> {
        crate::http::upload_blob(
            &self.core.hub_url,
            self.core.worker_id,
            &self.core.identity,
            sha256,
            bytes,
        )
        .await
        .map_err(offline_body)
    }
}

struct Runtime {
    core: Arc<Core>,
    session_id: SessionId,
    jobs: mpsc::UnboundedSender<Job>,
    harness: Option<Box<dyn HarnessSession>>,
    capabilities: Capabilities,
    mcp: Option<McpEndpoint>,
    pump: Option<tokio::task::JoinHandle<()>>,
}

fn harness_body(err: consort_harness::HarnessError) -> ErrorBody {
    WorkerError::Harness(err).to_error_body()
}

impl Runtime {
    fn harness(&mut self) -> Result<&mut Box<dyn HarnessSession>, ErrorBody> {
        self.harness
            .as_mut()
            .ok_or_else(|| ErrorBody::new(ErrorCode::Conflict, "the session has no harness"))
    }

    /// Opens the harness for a new or resumed session.
    async fn open(
        &mut self,
        start: &SessionStart,
        workspace_path: &std::path::Path,
        resume: Option<consort_proto::ids::NativeRef>,
    ) -> Result<(), ErrorBody> {
        let core = self.core.clone();
        let config = core.config_snapshot();
        let account = config
            .accounts
            .iter()
            .find(|a| a.id == start.account_id)
            .cloned()
            .ok_or_else(|| {
                ErrorBody::new(
                    ErrorCode::NotFound,
                    format!("this worker has no harness account {}", start.account_id),
                )
            })?;
        if !account.enabled {
            return Err(ErrorBody::new(
                ErrorCode::PolicyDenied,
                format!(
                    "the account \"{}\" is turned off on this worker",
                    account.label
                ),
            ));
        }
        let adapter = core
            .harnesses
            .get(&account.harness)
            .cloned()
            .ok_or_else(|| {
                ErrorBody::new(
                    ErrorCode::NotFound,
                    format!(
                        "this build has no adapter for {}",
                        account.harness.display_name()
                    ),
                )
            })?;
        // The probed capabilities, when the worker has them: a probe can
        // find that Full access does not work on this machine.
        let capabilities = core.account_capabilities(&account, adapter.as_ref());
        let mut max = config.policy.max_approval_mode;
        if !config.policy.allow_shell {
            max = max.min(ApprovalMode::AutoEdits);
        }
        let mode = effective_mode(start.approval_mode, max, &capabilities).ok_or_else(|| {
            ErrorBody::new(
                ErrorCode::PolicyDenied,
                format!(
                    "{} cannot run at or below the approval mode {:?}",
                    account.label,
                    start.approval_mode.capped_by(max)
                ),
            )
        })?;

        let backend: Arc<dyn ToolBackend> = Arc::new(LinkTools { core: core.clone() });
        let endpoint = mcp::start(self.session_id, workspace_path.to_path_buf(), backend)
            .await
            .map_err(|e| e.to_error_body())?;
        let mut servers = vec![McpServerSpec {
            name: HUB_SERVER.to_owned(),
            url: endpoint.url.clone(),
            bearer_token: Some(endpoint.token.clone()),
            bridge: core.bridge_program.as_ref().map(|program| StdioCommand {
                command: program.clone(),
                args: vec![
                    "mcp-bridge".to_owned(),
                    "--url".to_owned(),
                    endpoint.url.clone(),
                ],
                env: BTreeMap::from([(
                    crate::bridge::TOKEN_VAR.to_owned(),
                    endpoint.token.clone(),
                )]),
            }),
        }];
        for local in &config.local_mcp_servers {
            if let Some(problem) = local_server_name_problem(&local.name) {
                tracing::warn!(
                    server = %local.name,
                    "the worker does not give this local MCP server to sessions: {problem}. Rename it in worker.toml to use it"
                );
                continue;
            }
            if start.allowed_local_mcp_servers.contains(&local.name) {
                servers.push(McpServerSpec {
                    name: local.name.clone(),
                    url: local.url.clone(),
                    bearer_token: None,
                    bridge: None,
                });
            }
        }
        let runtime_root = core.config_dir.join(RUNTIME_DIR);
        let runtime_dir = runtime_root.join(self.session_id.to_string());
        if let Err(err) =
            create_private_dir(&runtime_root).and_then(|()| std::fs::create_dir_all(&runtime_dir))
        {
            endpoint.stop().await;
            return Err(WorkerError::io(
                format!("could not create {}", runtime_dir.display()),
                err,
            )
            .to_error_body());
        }
        let watcher = consort_harness::process::WatcherHandle(Arc::new(GroupLog {
            core: Arc::downgrade(&core),
        }));
        let spec = SessionSpec {
            session_id: self.session_id,
            workspace: workspace_path.to_path_buf(),
            runtime_dir,
            model: start.model.clone(),
            options: start.options.clone(),
            approval_mode: mode,
            allow_shell: config.policy.allow_shell,
            system_prompt_append: start.system_prompt_append.clone(),
            mcp_servers: servers,
            env: BTreeMap::new(),
            resume,
            process_watcher: Some(watcher),
        };
        let (session, events) = match adapter.open(&account, spec).await {
            Ok(s) => s,
            Err(err) => {
                endpoint.stop().await;
                return Err(harness_body(err));
            }
        };
        self.capabilities = session.capabilities().clone();
        if !self.capabilities.approval_modes.contains(&mode) {
            // The adapter had to start the session in a lower mode, for
            // example because Claude Code turned `bypassPermissions` off
            // after the last probe. Say so, and stop offering the mode.
            let used = self
                .capabilities
                .best_mode_up_to(mode)
                .map_or("a lower mode", mode_name);
            core.record_event(
                self.session_id,
                &SessionEvent::Error {
                    code: "approval_mode_lowered".to_owned(),
                    message: format!(
                        "{} cannot run in {} on this worker now, so the session runs in {used}.",
                        account.label,
                        mode_name(mode)
                    ),
                },
            );
            core.narrow_approval_modes(&account, &self.capabilities);
        }
        if let Some(native) = session.native_ref() {
            let _ = core.store.set_native_ref(self.session_id, &native);
        }
        self.harness = Some(session);
        let hub_tools = endpoint.tools();
        self.mcp = Some(endpoint);
        self.pump = Some(tokio::spawn(pump(
            core.clone(),
            self.session_id,
            start.account_id,
            events,
            self.jobs.clone(),
            !config.policy.allow_shell,
            hub_tools,
        )));
        core.inventory_changed();
        Ok(())
    }

    async fn start(&mut self, start: SessionStart) -> Result<Value, ErrorBody> {
        let core = self.core.clone();
        let config = core.config_snapshot();
        let key = start
            .task_id
            .map(|t| t.to_string())
            .unwrap_or_else(|| start.session_id.to_string());
        let prepared = match core.store.workspace(&key).map_err(|e| e.to_error_body())? {
            // A retried task reuses its workspace.
            Some(record) if start.task_id.is_some() => {
                let path = record.entries.first().map(|e| match e {
                    WorkspaceEntry::InPlace { path }
                    | WorkspaceEntry::Worktree { path, .. }
                    | WorkspaceEntry::Clone { path }
                    | WorkspaceEntry::Empty { path } => path.clone(),
                });
                let path = if record.entries.len() == 1 {
                    path.unwrap_or(record.root.clone())
                } else {
                    record.root.clone()
                };
                workspace::Prepared {
                    path: path.into(),
                    record,
                }
            }
            _ => {
                let prepared = workspace::prepare(
                    &core.workspaces_dir(),
                    &key,
                    start.task_id,
                    &start.workspace,
                    &config.policy,
                )
                .await
                .map_err(|e| e.to_error_body())?;
                core.store
                    .put_workspace(&key, &prepared.record)
                    .map_err(|e| e.to_error_body())?;
                prepared
            }
        };
        if !start.channel_files.is_empty() {
            tracing::warn!(
                count = start.channel_files.len(),
                "this build does not download channel files yet"
            );
        }
        let workspace_path = prepared.path.to_string_lossy().into_owned();
        core.store
            .put_session(&SessionRecord {
                start: start.clone(),
                native_ref: None,
                workspace_path: workspace_path.clone(),
                state: SessionState::Starting,
                last_seq: 0,
            })
            .map_err(|e| e.to_error_body())?;
        core.record_event(
            self.session_id,
            &SessionEvent::State {
                state: SessionState::Starting,
            },
        );
        let resume = start.resume.as_ref().map(|r| r.native_ref.clone());
        if let Err(err) = self.open(&start, &prepared.path, resume).await {
            let _ = core
                .store
                .set_session_state(self.session_id, SessionState::Lost);
            core.record_event(
                self.session_id,
                &SessionEvent::Error {
                    code: "start_failed".to_owned(),
                    message: err.message.clone(),
                },
            );
            core.record_event(
                self.session_id,
                &SessionEvent::State {
                    state: SessionState::Lost,
                },
            );
            return Err(err);
        }
        let native_ref = self.harness.as_ref().and_then(|h| h.native_ref());
        if let Some(turn) = start.turn {
            let result = match self.harness() {
                Ok(h) => h.turn(turn).await.map_err(harness_body),
                Err(e) => Err(e),
            };
            if let Err(err) = result {
                // The session did not start, so nothing of it may keep
                // running: not the harness and not the MCP endpoint.
                self.stop_recording();
                self.close_harness().await;
                self.lost("start_failed", &err.message);
                return Err(err);
            }
        }
        serde_json::to_value(SessionStartResult {
            workspace_path,
            native_ref,
        })
        .map_err(|_| ErrorBody::internal())
    }

    /// Opens the harness of a session again after the worker restarted.
    /// Returns false when the session is lost.
    async fn restore(&mut self, record: SessionRecord) -> bool {
        let Some(native) = record.native_ref.clone() else {
            self.lost(
                "resume_failed",
                "the session has no harness reference to resume",
            );
            return false;
        };
        // Tell the hub first, so that it closes the old turn before any
        // event from the new harness process.
        self.core
            .record_event(self.session_id, &SessionEvent::Reopened);
        let path = std::path::PathBuf::from(&record.workspace_path);
        if let Err(err) = self.open(&record.start, &path, Some(native)).await {
            self.lost("resume_failed", &err.message);
            return false;
        }
        true
    }

    /// Stops recording harness events, so the `ended` state that a harness
    /// reports when it closes does not reach the hub.
    fn stop_recording(&mut self) {
        if let Some(pump) = self.pump.take() {
            pump.abort();
        }
    }

    fn lost(&self, code: &str, why: &str) {
        if self.core.is_stopping() {
            // The next run of the worker tries again.
            tracing::info!(session = %self.session_id, %code, "the worker stops, so it does not mark the session lost");
            return;
        }
        let _ = self
            .core
            .store
            .set_session_state(self.session_id, SessionState::Lost);
        self.core.record_event(
            self.session_id,
            &SessionEvent::Error {
                code: code.to_owned(),
                message: why.to_owned(),
            },
        );
        self.core.record_event(
            self.session_id,
            &SessionEvent::State {
                state: SessionState::Lost,
            },
        );
    }

    async fn close_harness(&mut self) {
        if let Some(harness) = self.harness.take() {
            let _ = harness.close().await;
        }
        if let Some(pump) = self.pump.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), pump).await;
        }
        if let Some(endpoint) = self.mcp.take() {
            endpoint.stop().await;
        }
    }

    /// Runs one command. Returns the result and whether the session ends.
    /// A session that fails to start ends, so its ID is free again.
    async fn execute(&mut self, command: HubToWorker) -> (Result<Value, ErrorBody>, bool) {
        let empty = || Ok(json!({}));
        match command {
            HubToWorker::SessionStart(start) => {
                let result = self.start(start).await;
                let ended = result.is_err();
                (result, ended)
            }
            HubToWorker::SessionTurn(turn) => {
                let result = match self.harness() {
                    Ok(h) => h.turn(turn.turn).await.map_err(harness_body),
                    Err(e) => Err(e),
                };
                (
                    result.and_then(|turn_id| {
                        serde_json::to_value(SessionTurnResult { turn_id })
                            .map_err(|_| ErrorBody::internal())
                    }),
                    false,
                )
            }
            HubToWorker::SessionInterrupt(_) => {
                let result = match self.harness() {
                    Ok(h) => h.interrupt().await.map_err(harness_body),
                    Err(e) => Err(e),
                };
                (result.and_then(|()| empty()), false)
            }
            HubToWorker::SessionRespond(respond) => {
                let result = match self.harness() {
                    Ok(h) => h
                        .respond(respond.request_id, respond.answer)
                        .await
                        .map_err(harness_body),
                    Err(e) => Err(e),
                };
                (result.and_then(|()| empty()), false)
            }
            HubToWorker::SessionStop(stop) => {
                tracing::info!(session = %self.session_id, reason = %stop.reason, "stopping a session");
                self.close_harness().await;
                let _ = self
                    .core
                    .store
                    .set_session_state(self.session_id, SessionState::Ended);
                (empty(), true)
            }
            // `run_session` starts a steer with `begin_steer`, and `Core`
            // runs the other commands itself.
            HubToWorker::SessionSteer(_)
            | HubToWorker::WorkspaceRelease(_)
            | HubToWorker::ItemDetail(_)
            | HubToWorker::InventoryRefresh(_) => (Err(ErrorBody::internal()), false),
        }
    }

    /// Gives a steer to the harness and answers the command from its own
    /// task, so interrupts, policy denials and hub tool answers do not wait
    /// for the harness to take the steer.
    fn begin_steer(&mut self, id: FrameId, steer: SessionSteer) {
        if !self.capabilities.steer {
            self.core.finish_command(
                &id,
                Err(ErrorBody::new(
                    ErrorCode::PolicyDenied,
                    "this harness cannot steer a running turn",
                )),
            );
            return;
        }
        let answer = match self.harness() {
            Ok(h) => h.steer(steer.turn),
            Err(e) => {
                self.core.finish_command(&id, Err(e));
                return;
            }
        };
        let core = self.core.clone();
        tokio::spawn(async move {
            match tokio::time::timeout(STEER_LIMIT, answer).await {
                Ok(Ok(())) => core.finish_command(&id, Ok(json!({}))),
                // The session closed first, for example because the worker
                // stops. The answer is not stored, so the hub can send the
                // steer again.
                Ok(Err(consort_harness::HarnessError::Closed)) => {
                    core.abandon_command(&id, crate::core::session_gone());
                }
                Ok(Err(err)) => core.finish_command(&id, Err(harness_body(err))),
                Err(_) => core.finish_command(
                    &id,
                    Err(harness_body(consort_harness::HarnessError::Timeout(
                        "the harness did not take the steer".to_owned(),
                    ))),
                ),
            }
        });
    }
}

/// Runs the actor of one session until it stops.
pub(crate) async fn run_session(
    core: Arc<Core>,
    session_id: SessionId,
    jobs_tx: mpsc::UnboundedSender<Job>,
    mut jobs: mpsc::UnboundedReceiver<Job>,
) {
    let mut runtime = Runtime {
        core: core.clone(),
        session_id,
        jobs: jobs_tx,
        harness: None,
        capabilities: Capabilities::default(),
        mcp: None,
        pump: None,
    };
    while let Some(job) = jobs.recv().await {
        match job {
            Job::Command { id, command } => {
                if let HubToWorker::SessionSteer(steer) = *command {
                    runtime.begin_steer(id, steer);
                    continue;
                }
                let (result, ended) = runtime.execute(*command).await;
                core.finish_command(&id, result);
                if ended {
                    break;
                }
            }
            Job::Restore(record) => {
                if !runtime.restore(*record).await {
                    break;
                }
            }
            Job::PolicyDeny {
                request_id,
                option_id,
            } => {
                if let Ok(h) = runtime.harness() {
                    tracing::info!(session = %session_id, "worker policy denies a shell command (policy.allow_shell = false)");
                    let _ = h
                        .respond(
                            request_id,
                            RequestAnswer {
                                option_id,
                                text: None,
                            },
                        )
                        .await;
                }
            }
            Job::AllowHubTool {
                request_id,
                option_id,
            } => {
                if let Ok(h) = runtime.harness() {
                    let _ = h
                        .respond(
                            request_id,
                            RequestAnswer {
                                option_id: Some(option_id),
                                text: None,
                            },
                        )
                        .await;
                }
            }
            Job::DenyTooLong {
                request_id,
                option_id,
                reason,
            } => {
                let can_steer = runtime.capabilities.steer;
                if let Ok(h) = runtime.harness() {
                    tracing::info!(session = %session_id, "the worker denies an approval that is too long to show");
                    let _ = h
                        .respond(
                            request_id,
                            RequestAnswer {
                                option_id,
                                text: None,
                            },
                        )
                        .await;
                    // The harness gives the model only "denied". A steer
                    // tells it why, so it can split the work.
                    if can_steer {
                        let steer = h.steer(TurnInput {
                            text: reason,
                            attachments: vec![],
                        });
                        tokio::spawn(async move {
                            if let Err(err) = steer.await {
                                tracing::debug!(error = %err, "could not tell the model why the worker denied an approval");
                            }
                        });
                    }
                }
            }
            Job::HarnessLost => {
                // The harness process is gone, and the pump recorded the
                // lost state and every event before it. Stop recording, so
                // no `ended` from the close follows. Stop the MCP endpoint,
                // so its token works no more.
                tracing::info!(session = %session_id, "the harness lost the session");
                runtime.stop_recording();
                runtime.close_harness().await;
                break;
            }
            Job::Shutdown => {
                // Stop recording first: the harness reports `ended` when it
                // closes, but the session stays open for the next run.
                runtime.stop_recording();
                runtime.close_harness().await;
                break;
            }
        }
    }
    runtime.close_harness().await;
    // Commands that wait in the queue will not run. Answer them, so the hub
    // does not wait for ever.
    jobs.close();
    while let Ok(job) = jobs.try_recv() {
        if let Job::Command { id, .. } = job {
            core.abandon_command(&id, crate::core::session_gone());
        }
    }
    core.session_ended(session_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::ids::TurnId;
    use consort_proto::model::{
        AssistantMessagePayload, ItemPayload, ItemStatus, SessionItem, Usage,
    };
    use consort_proto::time::Timestamp;

    fn item(id: ItemId, text: &str) -> SessionEvent {
        SessionEvent::Item(SessionItem {
            id,
            session_id: SessionId::new(),
            turn_id: TurnId::new("t"),
            payload: ItemPayload::AssistantMessage(AssistantMessagePayload { text: text.into() }),
            status: ItemStatus::InProgress,
            created_at: Timestamp::EPOCH,
            updated_at: Timestamp::EPOCH,
        })
    }

    #[test]
    fn only_listed_hub_tools_are_allowed_on_the_worker() {
        use consort_proto::model::{RequestOption, RequestOptionKind};
        use consort_proto::worker_link::RequestOpened;
        let opened = |server: &str, tool: &str| RequestOpened {
            request_id: RequestId::new("r"),
            summary: String::new(),
            body: RequestBody::Approval(ApprovalDetails::Tool {
                server: Some(server.into()),
                tool: tool.into(),
                arguments: Value::Null,
            }),
            options: vec![RequestOption {
                id: "yes".into(),
                label: "Allow".into(),
                kind: RequestOptionKind::AllowOnce,
                description: None,
            }],
        };
        let listed = vec![consort_proto::tools::HubTool::PostMessage.definition()];
        let tools = Some(listed.as_slice());
        assert_eq!(
            hub_tool_allow(&opened("hub", "post_message"), tools).as_deref(),
            Some("yes")
        );
        // `mcp__hub__x__post_message` from a local server named `hub__x`.
        assert_eq!(
            hub_tool_allow(&opened("hub", "x__post_message"), tools),
            None
        );
        assert_eq!(hub_tool_allow(&opened("hub", "rm_rf"), tools), None);
        assert_eq!(
            hub_tool_allow(&opened("github", "post_message"), tools),
            None
        );
        assert_eq!(
            hub_tool_allow(&opened("hub", "post_message"), None),
            None,
            "nothing is allowed before the harness lists the tools"
        );
    }

    #[test]
    fn coalescer_merges_item_updates_and_deltas() {
        let mut c = Coalescer::default();
        let a = ItemId::new();
        let b = ItemId::new();
        assert!(c.push(item(a, "H")).is_empty());
        assert!(c.push(item(a, "He")).is_empty());
        assert!(
            c.push(SessionEvent::OutputDelta {
                item_id: b,
                text: "1\n".into()
            })
            .is_empty()
        );
        assert!(
            c.push(SessionEvent::OutputDelta {
                item_id: b,
                text: "2\n".into()
            })
            .is_empty()
        );
        assert!(
            c.push(SessionEvent::Usage(Usage {
                input_tokens: 1,
                ..Usage::default()
            }))
            .is_empty()
        );
        assert!(c.push(item(a, "Hello")).is_empty());
        assert!(
            c.push(SessionEvent::Usage(Usage {
                input_tokens: 2,
                ..Usage::default()
            }))
            .is_empty()
        );
        let out = c.push(SessionEvent::State {
            state: SessionState::Idle,
        });
        assert_eq!(out.len(), 4);
        assert!(
            matches!(&out[0], SessionEvent::Item(i) if matches!(&i.payload, ItemPayload::AssistantMessage(m) if m.text == "Hello"))
        );
        assert!(matches!(&out[1], SessionEvent::OutputDelta { text, .. } if text == "1\n2\n"));
        assert!(matches!(&out[2], SessionEvent::Usage(u) if u.input_tokens == 2));
        assert!(matches!(&out[3], SessionEvent::State { .. }));
        assert!(c.is_empty());
    }
}
