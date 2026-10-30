//! The session manager: one actor per session that runs the hub commands
//! for that session in order, and one pump per session that numbers the
//! harness events, writes them to the spool and wakes the sender.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use consort_harness::approval::effective_mode;
use consort_harness::{
    AccountConfig, EventReceiver, HarnessError, HarnessSession, McpServerSpec,
    SIGNED_OUT_ERROR_CODE, SessionSpec, StdioCommand, sign_in_hint_for,
};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::FrameId;
use consort_proto::ids::{AccountId, ItemId, RequestId, SessionId};
use consort_proto::model::{
    ApprovalDetails, ApprovalMode, Capabilities, FileRef, RequestOptionKind, SessionRole,
    SessionState, Sha256Hash,
};
use consort_proto::tools::{ToolCallResult, ToolDefinition};
use consort_proto::worker_link::{
    HubToWorker, OUTPUT_DELTA_INTERVAL_MS, RequestAnswer, RequestBody, SessionEvent, SessionStart,
    SessionStartResult, SessionSteer, SessionTurnResult, ToolCall, ToolList, TurnInput,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::attachments::{ATTACHMENT_FAILED_CODE, BlobCache, BlobFetch, ChannelCopy, Failure};
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
    /// Deny an approval without showing it to the user. `reason` goes to
    /// the model as a steer when the harness can steer, because the harness
    /// gives the model only "denied". `why` is for the log.
    DenyWithReason {
        request_id: RequestId,
        option_id: Option<String>,
        reason: String,
        why: &'static str,
    },
    /// The harness reported the session as lost. Close what is left of it.
    HarnessLost,
    /// Close the harness but keep the session for resume.
    Shutdown,
    /// The account of the session was removed. The session ends as lost.
    AccountRemoved,
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

/// What the worker tells the model when it denies an approval in a
/// coordinator session. A coordinator does its work through hub tools. Shell,
/// web and file changes belong to a task.
pub(crate) const COORDINATOR_DENIED_REASON: &str = "The coordinator does not run shell commands, use the web or change files, so the worker denied this request. Start a task with the start_task tool. The task does this work.";

/// Whether the worker denies this request in a coordinator session, without
/// a card. Every approval is denied: the caller has already allowed the
/// listed hub tools. A question goes to the user as usual.
fn coordinator_denies(opened: &consort_proto::worker_link::RequestOpened) -> bool {
    matches!(opened.body, RequestBody::Approval(_))
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

/// What the pump denies without asking the user.
#[derive(Debug, Clone, Copy)]
struct PumpPolicy {
    /// Worker policy: no shell commands (`policy.allow_shell = false`).
    deny_commands: bool,
    /// The session is a coordinator: it does no shell, web or file work.
    coordinator: bool,
}

/// Reads harness events until the harness closes the channel.
async fn pump(
    core: Arc<Core>,
    session_id: SessionId,
    account_id: AccountId,
    mut events: EventReceiver,
    jobs: mpsc::UnboundedSender<Job>,
    policy: PumpPolicy,
    hub_tools: ToolCache,
) {
    let PumpPolicy {
        deny_commands,
        coordinator,
    } = policy;
    let mut coalescer = Coalescer::default();
    let item_details = crate::item_details::ItemDetails::new(&core.consort_home);
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
                let Some(mut event) = event else { break };
                // The full output and the diffs stay here for `item.detail`.
                item_details.observe(session_id, &mut event);
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
                // A turn found the login expired, which the probe cannot
                // see. The account shows "Sign in again" until the user does.
                if let SessionEvent::Error { code, .. } = &event
                    && code == consort_harness::LOGIN_EXPIRED_ERROR_CODE
                {
                    core.mark_login_expired(account_id);
                }
                // A turn that works shows that the login is fine.
                if matches!(
                    &event,
                    SessionEvent::TurnEnded { reason: consort_proto::worker_link::TurnEndReason::Completed, .. }
                ) {
                    core.clear_login_expired(account_id);
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
                        let _ = jobs.send(Job::DenyWithReason {
                            request_id: opened.request_id.clone(),
                            option_id: deny_option(&opened.options),
                            reason,
                            why: "the approval is too long to show",
                        });
                        continue;
                    }
                    // A coordinator session runs in Ask and does no shell,
                    // web or file work. The worker denies the request at
                    // once, and the user sees no card for it.
                    if coordinator && coordinator_denies(opened) {
                        deadline = None;
                        let _ = jobs.send(Job::DenyWithReason {
                            request_id: opened.request_id.clone(),
                            option_id: deny_option(&opened.options),
                            reason: COORDINATOR_DENIED_REASON.to_owned(),
                            why: "a coordinator session does no shell, web or file work",
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
    /// The approval mode that the session opened in, and the mode that the
    /// harness runs in now. They differ after a turn with a stricter mode
    /// of its own (`SessionTurn::approval_mode`).
    mode: ApprovalMode,
    turn_mode: ApprovalMode,
    mcp: Option<McpEndpoint>,
    pump: Option<tokio::task::JoinHandle<()>>,
    /// Keeps the managed tool of the session from an update.
    tool_lease: Option<crate::tools::ToolLease>,
    /// The skills of the session. The worker removes their folder when the
    /// session ends.
    skills: Option<consort_harness::skills::SessionSkills>,
    /// The session's workspace, once the start or the resume knows it.
    workspace: Option<std::path::PathBuf>,
    /// The read-only copy of the channel files in the workspace.
    channel_copy: ChannelCopy,
    /// The newest channel file list that the hub sent.
    channel_files: Vec<FileRef>,
    /// The last copy of the channel files missed a file, so the next turn
    /// tries again.
    channel_retry: bool,
}

/// Gets blobs over HTTP with the worker's signature.
struct LinkBlobs {
    core: Arc<Core>,
}

#[async_trait]
impl BlobFetch for LinkBlobs {
    async fn fetch(
        &self,
        sha256: &Sha256Hash,
        max_bytes: u64,
        dest: &std::path::Path,
    ) -> Result<(), WorkerError> {
        crate::http::download_blob_to_file(
            &self.core.hub_url,
            self.core.worker_id,
            &self.core.identity,
            sha256,
            max_bytes,
            dest,
        )
        .await
        .map(|_| ())
    }
}

fn harness_body(err: consort_harness::HarnessError) -> ErrorBody {
    WorkerError::Harness(err).to_error_body()
}

/// Why [`Runtime::open`] failed.
struct OpenFailure {
    /// The error for the hub.
    body: ErrorBody,
    /// The harness failed in a way that a stop signal can cause, for
    /// example because its process exited. A refusal from the worker, or a
    /// sign-in answer from the harness, is not such a failure.
    harness_failed: bool,
}

impl From<ErrorBody> for OpenFailure {
    fn from(body: ErrorBody) -> Self {
        Self {
            body,
            harness_failed: false,
        }
    }
}

/// Why a `session.start` did not finish.
enum StartFailure {
    /// The start failed. The worker stores the error, and the session
    /// ends.
    Failed(ErrorBody),
    /// The worker stops, and the harness stopped with it. The worker stores
    /// no result, and the session stays `starting` with no harness
    /// reference. So the next run does not resume it, and the hub sends the
    /// start again after the restart.
    Stopped(ErrorBody),
}

impl From<ErrorBody> for StartFailure {
    fn from(body: ErrorBody) -> Self {
        Self::Failed(body)
    }
}

/// What the actor does with the result of a command.
enum Outcome {
    /// Store the result and send it to the hub.
    Finish(Result<Value, ErrorBody>),
    /// Store nothing, so the hub can send the command again (see
    /// [`Core::abandon_command`]).
    Abandon(ErrorBody),
}

impl Runtime {
    /// Tells the user, through the hub, about each file that the session did
    /// not get.
    fn report_failures(&self, failures: &[Failure], what: &str) {
        for failure in failures {
            tracing::warn!(session = %self.session_id, file = %failure.name, error = %failure.error, "a session did not get a file");
            self.core.record_event(
                self.session_id,
                &SessionEvent::Error {
                    code: ATTACHMENT_FAILED_CODE.to_owned(),
                    message: failure.message(what),
                },
            );
        }
    }

    /// Makes the channel file copy in the workspace current. `files` is a
    /// new list from the hub, or `None` when the list did not change. With
    /// `None`, the worker only tries again the files that failed before.
    async fn refresh_channel_files(&mut self, files: Option<Vec<FileRef>>) {
        let changed = files.is_some();
        if let Some(files) = files {
            if let Err(err) = self.core.store.set_channel_files(self.session_id, &files) {
                tracing::warn!(session = %self.session_id, error = %err, "could not store the new channel file list");
            }
            self.channel_files = files;
        }
        if !changed && !self.channel_retry {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let cache = BlobCache::new(&self.core.consort_home);
        let fetch = LinkBlobs {
            core: self.core.clone(),
        };
        let failures = self
            .channel_copy
            .sync(&cache, &fetch, &workspace, &self.channel_files)
            .await;
        self.channel_retry = !failures.is_empty();
        self.report_failures(&failures, "channel file");
    }

    /// Saves the attachments of a turn or a steer in the workspace before
    /// the harness gets the input.
    async fn save_attachments(&mut self, input: &TurnInput) {
        if input.attachments.is_empty() {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let cache = BlobCache::new(&self.core.consort_home);
        let fetch = LinkBlobs {
            core: self.core.clone(),
        };
        let failures =
            crate::attachments::save_attachments(&cache, &fetch, &workspace, &input.attachments)
                .await;
        self.report_failures(&failures, "attachment");
    }

    fn harness(&mut self) -> Result<&mut Box<dyn HarnessSession>, ErrorBody> {
        self.harness
            .as_mut()
            .ok_or_else(|| ErrorBody::new(ErrorCode::Conflict, "the session has no harness"))
    }

    /// Puts the harness in the approval mode of the next turn: the lower
    /// of `wanted` and the session's own mode, or the session's own mode
    /// when the turn names none. A harness that cannot change its mode
    /// refuses a stricter turn, so the turn never runs with more access
    /// than the hub asked for.
    async fn prepare_turn_mode(&mut self, wanted: Option<ApprovalMode>) -> Result<(), ErrorBody> {
        let target = wanted.map_or(self.mode, |m| m.min(self.mode));
        if target == self.turn_mode {
            return Ok(());
        }
        let session_id = self.session_id;
        let result = self.harness()?.set_approval_mode(target).await;
        match result {
            Ok(()) => {
                tracing::info!(session = %session_id, mode = mode_name(target), "the next turn runs in another approval mode");
                self.turn_mode = target;
                Ok(())
            }
            Err(consort_harness::HarnessError::Unsupported(_)) => Err(ErrorBody::new(
                ErrorCode::PolicyDenied,
                format!(
                    "This harness cannot change its approval mode in a running session, so it cannot run this turn in {}. Set the default approval mode of the channel to Ask.",
                    mode_name(target)
                ),
            )),
            Err(err) => Err(harness_body(err)),
        }
    }

    /// Changes the model and the options in the open session, for a turn
    /// that carries a model change. A harness that cannot do it, or that
    /// refuses the model, makes the worker refuse the turn with
    /// `unsupported`: the hub then starts a new session in the same
    /// workspace. The turn has not run, so nothing is lost.
    async fn apply_model_change(
        &mut self,
        change: &consort_proto::worker_link::SessionModelChange,
    ) -> Result<(), ErrorBody> {
        let session_id = self.session_id;
        let harness = self.harness()?;
        if !harness.capabilities().switch_model {
            return Err(ErrorBody::new(
                ErrorCode::Unsupported,
                "This harness cannot change its model in a running session.",
            ));
        }
        harness
            .set_model(&change.model, &change.options)
            .await
            .map_err(|err| {
                ErrorBody::new(
                    ErrorCode::Unsupported,
                    format!("The harness did not change its model: {err}"),
                )
            })?;
        tracing::info!(session = %session_id, model = %change.model, "the session runs another model");
        if let Err(err) =
            self.core
                .store
                .set_session_model(session_id, &change.model, &change.options)
        {
            tracing::warn!(session = %session_id, error = %err, "could not save the new model of a session");
        }
        Ok(())
    }

    /// Opens the harness for a new or resumed session.
    async fn open(
        &mut self,
        start: &SessionStart,
        workspace_path: &std::path::Path,
        resume: Option<consort_proto::ids::NativeRef>,
    ) -> Result<(), OpenFailure> {
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
            )
            .into());
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
        // The profile, and the managed tool. A missing tool installs first,
        // and the session waits for it.
        let (account, lease) = core.prepare_for_run(&account, adapter.as_ref()).await?;
        // The probed capabilities, when the worker has them: a probe can
        // find that Full access does not work on this machine.
        let capabilities = core.account_capabilities(&account, adapter.as_ref());
        let mut max = config.policy.max_approval_mode;
        if !config.policy.allow_shell {
            max = max.min(ApprovalMode::AutoEdits);
        }
        // A coordinator session always runs in Ask, whatever the hub sends.
        // It does no shell, web or file work (see `coordinator_denies`).
        let requested = if start.role == SessionRole::Coordinator {
            ApprovalMode::Ask
        } else {
            start.approval_mode
        };
        let mode = effective_mode(requested, max, &capabilities).ok_or_else(|| {
            ErrorBody::new(
                ErrorCode::PolicyDenied,
                format!(
                    "{} cannot run at or below the approval mode {:?}",
                    account.label,
                    requested.capped_by(max)
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
            .to_error_body()
            .into());
        }
        let watcher = consort_harness::process::WatcherHandle(Arc::new(GroupLog {
            core: Arc::downgrade(&core),
        }));
        // The skills of the session: fetched, checked and written into a
        // folder of this session. A skill that fails does not fail the
        // session.
        let delivery = crate::skills::deliver(
            &account,
            &crate::skills::SkillCache::new(&core.consort_home),
            &crate::skills::HubBundles { core: core.clone() },
            &start.extensions,
            &runtime_dir.join(crate::skills::SESSION_SKILLS_DIR),
        )
        .await;
        self.skills = delivery.skills.clone();
        if !delivery.failed.is_empty() {
            core.record_event(
                self.session_id,
                &SessionEvent::Error {
                    code: "skills_not_delivered".to_owned(),
                    message: format!(
                        "Some skills could not be set up, so this session runs without them. {}",
                        delivery.failed.join(" ")
                    ),
                },
            );
        }
        // The hub's text, then the connectors and skills of the channel.
        let mut system_prompt_append = start.system_prompt_append.clone();
        let blocks = delivery.prompt(&start.extensions);
        if !blocks.is_empty() {
            if !system_prompt_append.is_empty() {
                system_prompt_append.push_str("\n\n");
            }
            system_prompt_append.push_str(&blocks);
        }
        // Provider extras follow the account's setting at each start and
        // each resume.
        let extras_enabled = core.store.extras_enabled(account.id).unwrap_or_else(|err| {
            tracing::warn!(account = %account.id, error = %err, "could not read the provider extras setting; the session uses the extras");
            true
        });
        let spec = SessionSpec {
            session_id: self.session_id,
            workspace: workspace_path.to_path_buf(),
            runtime_dir,
            model: start.model.clone(),
            options: start.options.clone(),
            approval_mode: mode,
            // A coordinator runs no shell, whatever the worker policy says.
            // Every harness removes the shell at the source, so a command
            // that the harness would run without asking cannot run.
            allow_shell: config.policy.allow_shell && start.role != SessionRole::Coordinator,
            coordinator: start.role == SessionRole::Coordinator,
            system_prompt_append,
            mcp_servers: servers,
            env: BTreeMap::new(),
            extras_enabled,
            skills: delivery.skills,
            resume,
            process_watcher: Some(watcher),
        };
        // The model and its options (such as the effort) go in the log, so a
        // user can check what a session started with. They hold no secret.
        tracing::info!(
            session = %self.session_id,
            account = %account.label,
            model = %spec.model,
            options = ?spec.options,
            extras_enabled = spec.extras_enabled,
            skills = ?spec.skills.as_ref().map(|s| &s.names),
            "opening a harness session"
        );
        let (session, events) = match adapter.open(&account, spec).await {
            Ok(s) => s,
            Err(err) => {
                endpoint.stop().await;
                self.remove_skills();
                return Err(self.open_failed(&account, err));
            }
        };
        self.tool_lease = lease;
        self.capabilities = session.capabilities().clone();
        // The mode that the harness runs in: the asked mode, or the lower
        // one that the adapter had to use.
        self.mode = self.capabilities.best_mode_up_to(mode).unwrap_or(mode);
        self.turn_mode = self.mode;
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
            PumpPolicy {
                deny_commands: !config.policy.allow_shell,
                coordinator: start.role == SessionRole::Coordinator,
            },
            hub_tools,
        )));
        core.inventory_changed();
        Ok(())
    }

    /// What a failed harness open tells the hub. When the harness says
    /// that the account is signed out, the worker marks the account signed
    /// out at once, so placement stops choosing it before the next probe.
    /// The session also gets a `signed_out` error event with the step to
    /// sign in, as after a failed turn.
    fn open_failed(&self, account: &AccountConfig, err: HarnessError) -> OpenFailure {
        match err {
            HarnessError::SignedOut(reason) => {
                let hint = sign_in_hint_for(account, &reason);
                self.core.mark_signed_out(account.id, &hint);
                self.core.record_event(
                    self.session_id,
                    &SessionEvent::Error {
                        code: SIGNED_OUT_ERROR_CODE.to_owned(),
                        message: hint.clone(),
                    },
                );
                harness_body(HarnessError::SignedOut(hint)).into()
            }
            other => OpenFailure {
                body: harness_body(other),
                harness_failed: true,
            },
        }
    }

    async fn start(&mut self, start: SessionStart) -> Result<Value, StartFailure> {
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
        let workspace_path = prepared.path.to_string_lossy().into_owned();
        let starting = SessionRecord {
            start: start.clone(),
            native_ref: None,
            workspace_path: workspace_path.clone(),
            state: SessionState::Starting,
            last_seq: 0,
        };
        core.store
            .put_session(&starting)
            .map_err(|e| e.to_error_body())?;
        core.record_event(
            self.session_id,
            &SessionEvent::State {
                state: SessionState::Starting,
            },
        );
        // The channel files, before the harness opens, so the first turn
        // sees them. Git does not see the worker's folders in a repo.
        workspace::exclude_worker_folders(&prepared.path).await;
        self.workspace = Some(prepared.path.clone());
        self.refresh_channel_files(Some(start.channel_files.clone()))
            .await;
        let resume = start.resume.as_ref().map(|r| r.native_ref.clone());
        if let Err(failure) = self.open(&start, &prepared.path, resume).await {
            // The same stop signal can reach the harness a moment before
            // the worker, so wait for the flag as the pump does.
            if failure.harness_failed && core.stops_within(EXIT_HOLD).await {
                tracing::info!(session = %self.session_id, "the harness stopped with the worker during the start; the hub sends the start again after the restart");
                return Err(StartFailure::Stopped(failure.body));
            }
            let err = failure.body;
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
            return Err(err.into());
        }
        let native_ref = self.harness.as_ref().and_then(|h| h.native_ref());
        if let Some(turn) = start.turn {
            self.save_attachments(&turn).await;
            let result = match self.prepare_turn_mode(start.turn_approval_mode).await {
                Ok(()) => match self.harness() {
                    Ok(h) => h.turn(turn).await.map_err(harness_body),
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            };
            if let Err(err) = result {
                // The session did not start, so nothing of it may keep
                // running: not the harness and not the MCP endpoint.
                self.stop_recording().await;
                self.close_harness().await;
                if core.stops_within(EXIT_HOLD).await {
                    // The harness has a reference now. Forget it, so the
                    // next run does not resume a session that the hub never
                    // got. The start that the hub sends again opens a new
                    // harness session.
                    if let Err(store_err) = core.store.put_session(&starting) {
                        tracing::warn!(session = %self.session_id, error = %store_err, "could not reset a session start that the stop cut short");
                    }
                    tracing::info!(session = %self.session_id, "the harness stopped with the worker in the first turn; the hub sends the start again after the restart");
                    return Err(StartFailure::Stopped(err));
                }
                self.lost("start_failed", &err.message);
                return Err(err.into());
            }
        }
        serde_json::to_value(SessionStartResult {
            workspace_path,
            native_ref,
        })
        .map_err(|_| ErrorBody::internal().into())
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
        // The copy of the channel files stays as it is. The next turn with a
        // new list refreshes it.
        self.workspace = Some(path.clone());
        self.channel_files = record.start.channel_files.clone();
        if let Err(failure) = self.open(&record.start, &path, Some(native)).await {
            self.lost("resume_failed", &failure.body.message);
            return false;
        }
        true
    }

    /// Stops recording harness events, so the `ended` state that a harness
    /// reports when it closes does not reach the hub.
    ///
    /// It waits until the pump has stopped, so no event that the pump
    /// still holds changes the session record after this.
    async fn stop_recording(&mut self) {
        if let Some(pump) = self.pump.take() {
            pump.abort();
            let _ = pump.await;
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
        self.tool_lease = None;
        self.remove_skills();
    }

    /// Removes the skill folder of the session. A running session keeps
    /// its skills until it ends, and a resumed session gets a new folder
    /// from the same cached files.
    fn remove_skills(&mut self) {
        if let Some(skills) = self.skills.take()
            && let Err(err) = consort_harness::skills::finish_session_skills(&skills)
        {
            tracing::warn!(session = %self.session_id, folder = %skills.root.display(), error = %err, "could not remove the skills of a session");
        }
    }

    /// Runs one command. Returns what to do with the result, and whether
    /// the session ends. A session that fails to start ends, so its ID is
    /// free again.
    async fn execute(&mut self, command: HubToWorker) -> (Outcome, bool) {
        match command {
            HubToWorker::SessionStart(start) => match self.start(start).await {
                Ok(value) => (Outcome::Finish(Ok(value)), false),
                Err(StartFailure::Failed(err)) => (Outcome::Finish(Err(err)), true),
                Err(StartFailure::Stopped(err)) => (Outcome::Abandon(err), true),
            },
            other => {
                let (result, ended) = self.execute_session_command(other).await;
                (Outcome::Finish(result), ended)
            }
        }
    }

    /// Runs one command for an open session.
    async fn execute_session_command(
        &mut self,
        command: HubToWorker,
    ) -> (Result<Value, ErrorBody>, bool) {
        let empty = || Ok(json!({}));
        match command {
            HubToWorker::SessionTurn(turn) => {
                if self.harness.is_some() {
                    self.refresh_channel_files(turn.channel_files).await;
                    self.save_attachments(&turn.turn).await;
                }
                // The model first: for Claude Code it sets the permission
                // mode again, and the mode of this turn follows.
                let changed = match &turn.model_change {
                    Some(change) if self.harness.is_some() => self.apply_model_change(change).await,
                    _ => Ok(()),
                };
                let result = match changed {
                    Err(e) => Err(e),
                    Ok(()) => match self.prepare_turn_mode(turn.approval_mode).await {
                        Ok(()) => match self.harness() {
                            Ok(h) => h.turn(turn.turn).await.map_err(harness_body),
                            Err(e) => Err(e),
                        },
                        Err(e) => Err(e),
                    },
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
            // `execute` runs `session.start`, `run_session` starts a steer
            // with `begin_steer`, and `Core` runs the other commands itself.
            HubToWorker::SessionStart(_)
            | HubToWorker::SessionSteer(_)
            | HubToWorker::WorkspaceRelease(_)
            | HubToWorker::ItemDetail(_)
            | HubToWorker::InventoryRefresh(_)
            | HubToWorker::AccountCreate(_)
            | HubToWorker::AccountUpdate(_)
            | HubToWorker::AccountRemove(_)
            | HubToWorker::AccountSignOut(_)
            | HubToWorker::SignInStart(_)
            | HubToWorker::SignInCode(_)
            | HubToWorker::SignInCancel(_)
            | HubToWorker::ToolsCheck(_)
            | HubToWorker::ToolsInstall(_) => (Err(ErrorBody::internal()), false),
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
        mode: ApprovalMode::Ask,
        turn_mode: ApprovalMode::Ask,
        mcp: None,
        pump: None,
        tool_lease: None,
        skills: None,
        workspace: None,
        channel_copy: ChannelCopy::default(),
        channel_files: Vec::new(),
        channel_retry: false,
    };
    while let Some(job) = jobs.recv().await {
        match job {
            Job::Command { id, command } => {
                if let HubToWorker::SessionSteer(steer) = *command {
                    // The files are saved first, so the harness can open
                    // them. An interrupt waits for this download.
                    if runtime.capabilities.steer && runtime.harness.is_some() {
                        runtime.save_attachments(&steer.turn).await;
                    }
                    runtime.begin_steer(id, steer);
                    continue;
                }
                let (outcome, ended) = runtime.execute(*command).await;
                match outcome {
                    Outcome::Finish(result) => core.finish_command(&id, result),
                    Outcome::Abandon(error) => core.abandon_command(&id, error),
                }
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
            Job::DenyWithReason {
                request_id,
                option_id,
                reason,
                why,
            } => {
                let can_steer = runtime.capabilities.steer;
                if let Ok(h) = runtime.harness() {
                    tracing::info!(session = %session_id, why, "the worker denies an approval");
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
                runtime.stop_recording().await;
                runtime.close_harness().await;
                break;
            }
            Job::Shutdown => {
                // Stop recording first: the harness reports `ended` when it
                // closes, but the session stays open for the next run.
                runtime.stop_recording().await;
                runtime.close_harness().await;
                break;
            }
            Job::AccountRemoved => {
                tracing::info!(session = %session_id, "the account of the session was removed");
                runtime.stop_recording().await;
                runtime.close_harness().await;
                runtime.lost(
                    "account_removed",
                    "The account of this session was removed from the worker.",
                );
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
