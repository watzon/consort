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
    SessionStartResult, SessionTurnResult, ToolCall, ToolList,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::core::Core;
use crate::mcp::{self, McpEndpoint, ToolBackend};
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
fn hub_tool_allow(opened: &consort_proto::worker_link::RequestOpened) -> Option<String> {
    let RequestBody::Approval(ApprovalDetails::Tool {
        server: Some(server),
        ..
    }) = &opened.body
    else {
        return None;
    };
    if server != HUB_SERVER {
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

/// Reads harness events until the harness closes the channel.
async fn pump(
    core: Arc<Core>,
    session_id: SessionId,
    account_id: AccountId,
    mut events: EventReceiver,
    jobs: mpsc::UnboundedSender<Job>,
    deny_commands: bool,
) {
    let mut coalescer = Coalescer::default();
    let interval = Duration::from_millis(OUTPUT_DELTA_INTERVAL_MS);
    let mut deadline: Option<tokio::time::Instant> = None;
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
                // The harness says that the login is gone or expired. Update
                // the inventory now, so the hub stops placing work here.
                if let SessionEvent::Error { code, message } = &event
                    && code == SIGNED_OUT_ERROR_CODE
                {
                    core.mark_signed_out(account_id, message);
                }
                if let SessionEvent::RequestOpened(opened) = &event
                    && let Some(option_id) = hub_tool_allow(opened)
                {
                    let _ = jobs.send(Job::AllowHubTool { request_id: opened.request_id.clone(), option_id });
                    continue;
                }
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
        let capabilities = adapter.capabilities(&account);
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
            if local.name == HUB_SERVER {
                tracing::warn!(
                    "a local MCP server is named `hub`, which Consort keeps for the hub tools; rename it to use it"
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
        let runtime_dir = core
            .config_dir
            .join("runtime")
            .join(self.session_id.to_string());
        let _ = std::fs::create_dir_all(&runtime_dir);
        let spec = SessionSpec {
            session_id: self.session_id,
            workspace: workspace_path.to_path_buf(),
            runtime_dir,
            model: start.model.clone(),
            options: start.options.clone(),
            approval_mode: mode,
            system_prompt_append: start.system_prompt_append.clone(),
            mcp_servers: servers,
            env: BTreeMap::new(),
            resume,
        };
        let (session, events) = match adapter.open(&account, spec).await {
            Ok(s) => s,
            Err(err) => {
                endpoint.stop().await;
                return Err(harness_body(err));
            }
        };
        self.capabilities = session.capabilities().clone();
        if let Some(native) = session.native_ref() {
            let _ = core.store.set_native_ref(self.session_id, &native);
        }
        self.harness = Some(session);
        self.mcp = Some(endpoint);
        self.pump = Some(tokio::spawn(pump(
            core.clone(),
            self.session_id,
            start.account_id,
            events,
            self.jobs.clone(),
            !config.policy.allow_shell,
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
            self.harness()?.turn(turn).await.map_err(harness_body)?;
        }
        serde_json::to_value(SessionStartResult {
            workspace_path,
            native_ref,
        })
        .map_err(|_| ErrorBody::internal())
    }

    async fn restore(&mut self, record: SessionRecord) {
        let Some(native) = record.native_ref.clone() else {
            self.lost("the session has no harness reference to resume");
            return;
        };
        // Tell the hub first, so that it closes the old turn before any
        // event from the new harness process.
        self.core
            .record_event(self.session_id, &SessionEvent::Reopened);
        let path = std::path::PathBuf::from(&record.workspace_path);
        if let Err(err) = self.open(&record.start, &path, Some(native)).await {
            self.lost(&err.message);
        }
    }

    fn lost(&self, why: &str) {
        let _ = self
            .core
            .store
            .set_session_state(self.session_id, SessionState::Lost);
        self.core.record_event(
            self.session_id,
            &SessionEvent::Error {
                code: "resume_failed".to_owned(),
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
    async fn execute(&mut self, command: HubToWorker) -> (Result<Value, ErrorBody>, bool) {
        let empty = || Ok(json!({}));
        match command {
            HubToWorker::SessionStart(start) => (self.start(start).await, false),
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
            HubToWorker::SessionSteer(steer) => {
                if !self.capabilities.steer {
                    return (
                        Err(ErrorBody::new(
                            ErrorCode::PolicyDenied,
                            "this harness cannot steer a running turn",
                        )),
                        false,
                    );
                }
                let result = match self.harness() {
                    Ok(h) => h.steer(steer.turn).await.map_err(harness_body),
                    Err(e) => Err(e),
                };
                (result.and_then(|()| empty()), false)
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
            HubToWorker::WorkspaceRelease(_)
            | HubToWorker::ItemDetail(_)
            | HubToWorker::InventoryRefresh(_) => (Err(ErrorBody::internal()), false),
        }
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
                let (result, ended) = runtime.execute(*command).await;
                core.finish_command(&id, result);
                if ended {
                    break;
                }
            }
            Job::Restore(record) => runtime.restore(*record).await,
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
            Job::Shutdown => {
                // Stop recording first: the harness reports `ended` when it
                // closes, but the session stays open for the next run.
                if let Some(pump) = runtime.pump.take() {
                    pump.abort();
                }
                runtime.close_harness().await;
                break;
            }
        }
    }
    runtime.close_harness().await;
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
