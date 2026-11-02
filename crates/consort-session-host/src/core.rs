//! The worker core: shared state, hub command dispatch with dedup, the
//! spool sender and the inventory.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use consort_harness::{AccountConfig, AccountStatus, Harness, HarnessKind};
use consort_proto::auth::{OpenSession, Signature, WorkerHello, WorkerHelloResult};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{Frame, FrameId, RequestFrame, SUPPORTED_PROTOCOL_VERSIONS};
use consort_proto::ids::{AccountId, SessionId, SignInId, WorkerId};
use consort_proto::model::{
    AuthState, Capabilities, HarnessAccount, Inventory, LocalMcpServerInfo, Os, SessionState,
    WorkerLoad,
};
use consort_proto::session_host_link::{HubToSessionHost, SessionEvent, SessionEvents};
use consort_proto::time::Timestamp;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, watch};

use crate::HostError;
use crate::accounts::managed_launch;
use crate::config::{HostToml, SESSIONS_DIR, local_server_name_problem};
use crate::sessions::{Job, SessionHandle, run_session};
use crate::store::{ProcessRecord, Store, StoredResult};
use consort_link::identity::Identity;
use consort_link::link::{Link, LinkHandler};

/// The harness adapters that a worker can use, by kind.
pub type Harnesses = HashMap<HarnessKind, Arc<dyn Harness>>;

/// The adapters in this build.
pub fn default_harnesses() -> Harnesses {
    let mut out: Harnesses = HashMap::new();
    for kind in HarnessKind::ALL {
        if let Some(adapter) = consort_harness::adapter_for(kind) {
            out.insert(kind, Arc::from(adapter));
        }
    }
    out
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Shared worker state.
pub(crate) struct Core {
    /// The route of this daemon on the hub.
    pub ws_path: &'static str,
    pub config_dir: PathBuf,
    pub config: RwLock<HostToml>,
    pub identity: Identity,
    pub worker_id: WorkerId,
    pub hub_url: String,
    pub store: Store,
    pub link: Link,
    pub harnesses: Harnesses,
    pub sessions: Mutex<HashMap<SessionId, SessionHandle>>,
    pub statuses: Mutex<HashMap<AccountId, AccountStatus>>,
    /// Accounts whose login a turn found expired. The probe cannot see an
    /// expiry (`claude auth status` reports none), so a later probe that
    /// says "signed in" does not undo the mark. A sign-in or a turn that
    /// works clears it.
    pub login_expired: Mutex<std::collections::HashSet<AccountId>>,
    pub spool_notify: Notify,
    pub inventory_notify: Notify,
    pub last_inventory_hash: Mutex<Option<String>>,
    pub bridge_program: Option<PathBuf>,
    /// The command IDs that run now. A command that arrives again while it
    /// runs gets the result of the first run, when that run ends.
    pub inflight: Mutex<HashSet<String>>,
    /// Turns true when the worker starts to stop. From then on, a harness
    /// that exits does not make its session lost: the next run resumes it.
    pub stopping: watch::Sender<bool>,
    /// The lock on the config folder, held while the worker runs. `None` in
    /// tests. `HostHandle::shutdown` releases it at its end.
    pub lock: Mutex<Option<std::fs::File>>,
    /// The Consort home, with `profiles/` and `tools/`.
    pub consort_home: PathBuf,
    /// The managed tools.
    pub tools: Arc<crate::tools::Tools>,
    /// The sign-ins that run now.
    pub sign_ins: Mutex<HashMap<SignInId, crate::signin::SignInControl>>,
    /// Held by a ChatGPT browser sign-in. Codex listens for the callback on
    /// a fixed port, so only one runs at a time.
    pub browser_sign_in: tokio::sync::Mutex<()>,
    /// The tags that the worker found about this machine.
    pub detected_tags: Vec<String>,
    /// The renderer of visuals: one browser for the whole session host.
    pub visual: Arc<dyn crate::visual::VisualEngine>,
}

/// The answer to a command for a session whose actor has ended.
pub(crate) fn session_gone() -> ErrorBody {
    ErrorBody::new(
        ErrorCode::NotFound,
        "the session ended on this worker before the command ran",
    )
}

/// What happened to the spool of one session in a flush.
enum Flush {
    /// The hub has every event, or the worker dropped them.
    Done,
    /// The hub refused a batch. The worker tries again in the next flush.
    Later,
    /// The link is down. The flush stops.
    Offline,
}

/// Records the harness process groups in `worker.db`, so the next start can
/// kill the ones that a killed worker left behind.
pub(crate) struct GroupLog {
    pub core: Weak<Core>,
}

impl consort_harness::process::ProcessWatcher for GroupLog {
    fn started(&self, pgid: u32, program: &str) {
        let Some(core) = self.core.upgrade() else {
            return;
        };
        // `ps` runs here, once per harness process. It is short, and the
        // record must exist before the session goes on.
        #[cfg(unix)]
        let Some(started) = consort_harness::process::group_start_time(pgid) else {
            return;
        };
        #[cfg(not(unix))]
        let started = String::new();
        let record = ProcessRecord {
            pgid,
            program: program.to_owned(),
            started,
        };
        if let Err(err) = core.store.put_process(&record) {
            tracing::warn!(error = %err, pgid, "could not record a harness process group");
        }
    }

    fn ended(&self, pgid: u32) {
        if let Some(core) = self.core.upgrade()
            && let Err(err) = core.store.delete_process(pgid)
        {
            tracing::warn!(error = %err, pgid, "could not forget a harness process group");
        }
    }
}

/// Kills the harness process groups that the last run of the worker
/// recorded and did not end, for example because the worker was killed.
///
/// - When the leader still has the recorded start time, the whole group is
///   killed.
/// - When the leader is gone, the processes that it left in the group keep
///   running. The worker kills each process of the group that started at or
///   after the recorded time.
/// - When another process has the leader's ID now, the group is not ours,
///   and nothing is killed.
///
/// This blocks, so call it from a blocking task.
#[cfg(unix)]
pub(crate) fn kill_stale_groups(store: &Store) {
    use consort_harness::process::{group_start_time, kill_group, kill_pid, members_started_since};
    let records = match store.processes() {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(error = %err, "could not read the recorded harness processes");
            return;
        }
    };
    for record in records {
        match group_start_time(record.pgid) {
            Some(now) if now == record.started => {
                if kill_group(record.pgid) {
                    tracing::warn!(
                        pgid = record.pgid,
                        program = %record.program,
                        "killed a harness process group that the last run of the worker left behind"
                    );
                }
            }
            Some(_) => {
                tracing::debug!(
                    pgid = record.pgid,
                    "another process has the ID of a recorded harness group; the worker leaves it"
                );
            }
            None => {
                let members = members_started_since(record.pgid, &record.started);
                let killed = members.into_iter().filter(|pid| kill_pid(*pid)).count();
                if killed > 0 {
                    tracing::warn!(
                        pgid = record.pgid,
                        program = %record.program,
                        killed,
                        "killed the processes that a harness left behind in its group"
                    );
                } else {
                    tracing::debug!(
                        pgid = record.pgid,
                        "a recorded harness process group is gone"
                    );
                }
            }
        }
        let _ = store.delete_process(record.pgid);
    }
}

/// Starts an actor for each session that was open when the worker
/// stopped, and asks it to resume the session. Returns how many it
/// resumes.
///
/// A session that is still `starting` and has no harness reference stopped
/// in the middle of `session.start`. That command has no stored result, so
/// the hub sends it again. The worker does not resume such a session: it
/// leaves it for that start.
pub(crate) fn restore_sessions(core: &Arc<Core>) -> Result<usize, HostError> {
    // Skill folders of sessions that ended while the worker was down. A
    // session that resumes writes its folder again from the cache.
    crate::skills::sweep(&core.config_dir, &core.consort_home);
    core.prune_skill_cache();
    let mut count = 0;
    for record in core.store.open_sessions()? {
        if record.state == SessionState::Starting && record.native_ref.is_none() {
            tracing::info!(
                session = %record.start.session_id,
                "a session start did not finish before the worker stopped; the worker waits for the hub to send the start again"
            );
            continue;
        }
        let jobs = core.spawn_session(record.start.session_id);
        let _ = jobs.send(Job::Restore(Box::new(record)));
        count += 1;
    }
    Ok(count)
}

impl Core {
    /// Removes the skill bundles from the cache that no open session names
    /// and that nobody used for 30 days. A session that needs one again gets
    /// it from the hub.
    pub(crate) fn prune_skill_cache(&self) {
        let in_use: std::collections::HashSet<String> = match self.store.open_sessions() {
            Ok(records) => records
                .iter()
                .flat_map(|r| r.start.extensions.skills.iter())
                .map(|skill| skill.content_hash.to_string())
                .collect(),
            Err(err) => {
                // Without the list of open sessions, nothing is safe to remove.
                tracing::warn!(error = %err, "could not read the open sessions, so the worker does not prune the skill cache");
                return;
            }
        };
        let report = crate::skills::SkillCache::new(&self.consort_home)
            .prune(crate::skills::PRUNE_AGE, &in_use);
        if report.removed > 0 {
            tracing::info!(
                removed = report.removed,
                kept = report.kept,
                freed_bytes = report.freed_bytes,
                "removed old skills from the cache"
            );
        }
    }

    /// Whether the worker has started to stop.
    pub fn is_stopping(&self) -> bool {
        *self.stopping.borrow()
    }

    /// Waits up to `hold` for the worker to start to stop. Returns whether
    /// it stops. A harness can exit just before the worker learns that it
    /// must stop, for example when systemd sends SIGTERM to every process
    /// of the unit. The hold covers that gap.
    pub async fn stops_within(&self, hold: Duration) -> bool {
        let mut rx = self.stopping.subscribe();
        if *rx.borrow_and_update() {
            return true;
        }
        tokio::time::timeout(hold, rx.wait_for(|stopping| *stopping))
            .await
            .is_ok_and(|r| r.is_ok())
    }

    /// The capabilities of an account: what the last probe found, or what
    /// the adapter knows without a probe.
    pub fn account_capabilities(
        &self,
        account: &AccountConfig,
        adapter: &dyn Harness,
    ) -> Capabilities {
        lock(&self.statuses)
            .get(&account.id)
            .map(|s| s.capabilities.clone())
            .filter(|c| !c.approval_modes.is_empty())
            .unwrap_or_else(|| adapter.capabilities(account))
    }

    /// Removes the approval modes that a session of the account could not
    /// use from the account's capabilities, and sends the inventory, so the
    /// hub does not offer them again.
    pub fn narrow_approval_modes(&self, account: &AccountConfig, session: &Capabilities) {
        let changed = {
            let mut statuses = lock(&self.statuses);
            let Some(status) = statuses.get_mut(&account.id) else {
                return;
            };
            let before = status.capabilities.approval_modes.len();
            status
                .capabilities
                .approval_modes
                .retain(|m| session.approval_modes.contains(m));
            status.capabilities.approval_modes.len() != before
        };
        if changed {
            self.send_inventory(false);
        }
    }

    pub fn config_snapshot(&self) -> HostToml {
        match self.config.read() {
            Ok(c) => c.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Numbers an event, writes it to the spool, and wakes the sender.
    pub fn record_event(&self, session_id: SessionId, event: &SessionEvent) {
        match event {
            SessionEvent::State { state } => {
                let _ = self.store.set_session_state(session_id, *state);
            }
            SessionEvent::NativeRef { native_ref } => {
                let _ = self.store.set_native_ref(session_id, native_ref);
            }
            _ => {}
        }
        match self.store.append_event(session_id, event) {
            Ok(_) => self.spool_notify.notify_one(),
            Err(err) => tracing::error!(error = %err, "could not write an event to the spool"),
        }
    }

    /// Stores a command result and sends it to the hub. A copy of the
    /// command that arrived while it ran gets this answer too: the answer
    /// goes on the current link, and it has the command ID.
    /// `item.detail`: uploads the kept output or diff of an item as a blob
    /// and gives its hash. An item with nothing kept is `not_found`.
    pub async fn item_detail(
        &self,
        detail: consort_proto::session_host_link::ItemDetail,
    ) -> Result<Value, ErrorBody> {
        let details = crate::item_details::ItemDetails::new(&self.consort_home);
        let Some((bytes, mime)) = details.read(detail.session_id, detail.item_id) else {
            return Err(ErrorBody::new(
                ErrorCode::NotFound,
                "This worker has no full output for this item. Only the summary in the thread is left.",
            ));
        };
        let digest: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(&bytes).into();
        let sha256 = consort_proto::model::Sha256Hash::from_bytes(digest);
        let size = bytes.len() as u64;
        consort_link::http::upload_blob(
            &self.hub_url,
            self.worker_id,
            &self.identity,
            &sha256,
            bytes,
        )
        .await
        .map_err(|e| ErrorBody::new(ErrorCode::Internal, e.to_string()))?;
        serde_json::to_value(consort_proto::session_host_link::ItemDetailResult {
            sha256,
            size,
            mime: mime.to_owned(),
        })
        .map_err(|_| ErrorBody::internal())
    }

    pub fn finish_command(&self, id: &FrameId, result: Result<Value, ErrorBody>) {
        if let Err(err) = self
            .store
            .store_command(id.as_str(), &StoredResult::from(&result))
        {
            tracing::error!(error = %err, "could not store a command result");
        }
        // Store first, then forget: `dispatch` then sees the stored result.
        lock(&self.inflight).remove(id.as_str());
        self.respond(id, result);
    }

    /// Answers a command that did not run, for example because its session
    /// ended first. The answer is not stored, so the hub can send the
    /// command again, for example after the worker restarted.
    ///
    /// While the worker stops, it sends no answer. The hub would take the
    /// error as the result: a `session.start` would fail its task. The link
    /// closes instead, the hub keeps the command in its queue, and it sends
    /// the command again after the restart.
    pub fn abandon_command(&self, id: &FrameId, error: ErrorBody) {
        lock(&self.inflight).remove(id.as_str());
        if self.is_stopping() {
            tracing::debug!(command = %id.as_str(), %error, "the worker stops, so it does not answer a command that did not run");
            return;
        }
        self.respond(id, Err(error));
    }

    /// Gives a command to a session actor. An actor that has ended gets no
    /// more work, so the command gets an answer here.
    fn send_job(&self, jobs: &mpsc::UnboundedSender<Job>, id: FrameId, command: HubToSessionHost) {
        let job = Job::Command {
            id,
            command: Box::new(command),
        };
        if let Err(mpsc::error::SendError(Job::Command { id, .. })) = jobs.send(job) {
            self.abandon_command(&id, session_gone());
        }
    }

    fn respond(&self, id: &FrameId, result: Result<Value, ErrorBody>) {
        let frame = match result {
            Ok(body) => Frame::Response(consort_proto::frame::ResponseFrame {
                re: id.clone(),
                result: Ok(body),
            }),
            Err(error) => Frame::error(id.clone(), error),
        };
        if let Err(err) = self.link.send_frame(&frame) {
            tracing::debug!(error = %err, "could not send a command result; the hub resends the command");
        }
    }

    pub fn session_ended(&self, session_id: SessionId) {
        lock(&self.sessions).remove(&session_id);
        self.inventory_changed();
    }

    pub fn inventory_changed(&self) {
        self.inventory_notify.notify_one();
    }

    /// Starts the actor of a session.
    pub fn spawn_session(self: &Arc<Self>, session_id: SessionId) -> mpsc::UnboundedSender<Job> {
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run_session(self.clone(), session_id, tx.clone(), rx));
        lock(&self.sessions).insert(
            session_id,
            SessionHandle {
                jobs: tx.clone(),
                task,
            },
        );
        tx
    }

    /// Routes one hub command. Commands for one session run in order in
    /// that session's actor. A command ID that ran in the last 8 days
    /// ([`crate::store::COMMAND_RESULT_KEEP_MS`]) gets its stored result. A
    /// command ID that runs now does not run again: the worker answers it
    /// when the first run ends.
    pub fn dispatch(self: &Arc<Self>, request: RequestFrame) {
        let id = request.id.clone();
        {
            let mut inflight = lock(&self.inflight);
            if inflight.contains(id.as_str()) {
                tracing::debug!(command = %id.as_str(), "the command runs already; the worker answers when it ends");
                return;
            }
            match self.store.command_result(id.as_str()) {
                Ok(Some(stored)) => {
                    drop(inflight);
                    self.respond(&id, stored.into_result());
                    return;
                }
                Ok(None) => {}
                Err(err) => tracing::error!(error = %err, "could not read the processed commands"),
            }
            inflight.insert(id.as_str().to_owned());
        }
        let command = match request.decode::<HubToSessionHost>() {
            Ok(c) => c,
            Err(err) => {
                self.finish_command(&id, Err(err.to_error_body()));
                return;
            }
        };
        match command {
            HubToSessionHost::SessionStart(start) => {
                let session_id = start.session_id;
                if lock(&self.sessions).contains_key(&session_id) {
                    self.finish_command(
                        &id,
                        Err(ErrorBody::new(
                            ErrorCode::Conflict,
                            "the session is already open on this worker",
                        )),
                    );
                    return;
                }
                let jobs = self.spawn_session(session_id);
                self.send_job(&jobs, id, HubToSessionHost::SessionStart(start));
            }
            // The workspace is on the worker. Here only the folder of the
            // task goes, with the transcripts of its harness.
            HubToSessionHost::WorkspaceRelease(release) => {
                let core = self.clone();
                tokio::spawn(async move {
                    if release.delete {
                        let folder = core
                            .config_dir
                            .join(SESSIONS_DIR)
                            .join(release.task_id.to_string());
                        if let Err(err) = tokio::fs::remove_dir_all(&folder).await
                            && err.kind() != std::io::ErrorKind::NotFound
                        {
                            tracing::warn!(folder = %folder.display(), error = %err, "could not remove the folder of a task");
                        }
                    }
                    core.finish_command(&id, Ok(json!({})));
                });
            }
            HubToSessionHost::InventoryRefresh(_) => {
                let core = self.clone();
                tokio::spawn(async move {
                    core.probe_accounts().await;
                    core.send_inventory(true);
                    core.finish_command(&id, Ok(json!({})));
                });
            }
            // Accounts, sign-in and managed tools (`accounts.rs`).
            HubToSessionHost::AccountCreate(_)
            | HubToSessionHost::AccountUpdate(_)
            | HubToSessionHost::AccountRemove(_)
            | HubToSessionHost::AccountSignOut(_)
            | HubToSessionHost::SignInStart(_)
            | HubToSessionHost::SignInCode(_)
            | HubToSessionHost::SignInCancel(_)
            | HubToSessionHost::ToolsCheck(_)
            | HubToSessionHost::ToolsInstall(_) => {
                let core = self.clone();
                tokio::spawn(async move {
                    let result = core.account_command(command).await;
                    core.finish_command(&id, result.map(|()| json!({})));
                });
            }
            HubToSessionHost::CommandCheckRun(run) => {
                let core = self.clone();
                tokio::spawn(async move {
                    let result = core.command_check(run).await.and_then(|reply| {
                        serde_json::to_value(reply).map_err(|_| ErrorBody::internal())
                    });
                    core.finish_command(&id, result);
                });
            }
            HubToSessionHost::ItemDetail(detail) => {
                let core = self.clone();
                tokio::spawn(async move {
                    let result = core.item_detail(detail).await;
                    core.finish_command(&id, result);
                });
            }
            // A render takes seconds, so it runs beside the link loop.
            HubToSessionHost::VisualRender(command) => {
                let core = self.clone();
                tokio::spawn(async move {
                    let blobs = crate::sessions::LinkTools { core: core.clone() };
                    let result = crate::visual::command::render_command(
                        core.visual.as_ref(),
                        &blobs,
                        command,
                    )
                    .await
                    .and_then(|image| {
                        serde_json::to_value(image).map_err(|_| ErrorBody::internal())
                    });
                    core.finish_command(&id, result);
                });
            }
            other => {
                let Some(session_id) = other.session_id() else {
                    self.finish_command(&id, Err(ErrorBody::internal()));
                    return;
                };
                let jobs = lock(&self.sessions)
                    .get(&session_id)
                    .map(|h| h.jobs.clone());
                match jobs {
                    Some(jobs) => self.send_job(&jobs, id, other),
                    None if matches!(other, HubToSessionHost::SessionStop(_)) => {
                        let _ = self.store.set_session_state(
                            session_id,
                            consort_proto::model::SessionState::Ended,
                        );
                        self.finish_command(&id, Ok(json!({})));
                    }
                    None => self.finish_command(
                        &id,
                        Err(ErrorBody::new(
                            ErrorCode::NotFound,
                            format!("no open session {session_id} on this worker"),
                        )),
                    ),
                }
            }
        }
    }

    // Spool.

    /// Sends every spooled event, one batch at a time. A session that the
    /// hub refuses does not stop the others.
    pub async fn flush_spool(&self) {
        let sessions = match self.store.spooled_sessions() {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(error = %err, "could not list the spooled sessions");
                return;
            }
        };
        for session_id in sessions {
            match self.flush_session(session_id).await {
                Flush::Done | Flush::Later => {}
                Flush::Offline => return,
            }
        }
    }

    async fn flush_session(&self, session_id: SessionId) -> Flush {
        let mut replaced = false;
        loop {
            let batch = match self.store.next_batch(session_id) {
                Ok(b) if b.is_empty() => return Flush::Done,
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!(session = %session_id, error = %err, "could not read the spooled events of a session; the worker tries again later");
                    return Flush::Later;
                }
            };
            let first = batch.first().map(|e| e.seq).unwrap_or(0);
            let last = batch.last().map(|e| e.seq).unwrap_or(0);
            match self
                .link
                .request(&SessionEvents {
                    session_id,
                    events: batch,
                })
                .await
                .map_err(HostError::from)
            {
                Ok(ack) => {
                    if let Err(err) = self.store.ack(session_id, ack.last_seq) {
                        tracing::warn!(session = %session_id, error = %err, "could not delete acknowledged events");
                    }
                    if ack.last_seq < last {
                        tracing::warn!(session = %session_id, hub = ack.last_seq, worker = last, "the hub stored fewer events than sent");
                        return Flush::Later;
                    }
                }
                Err(HostError::Hub {
                    code: ErrorCode::NotFound,
                    message,
                }) => {
                    tracing::warn!(session = %session_id, %message, "the hub does not know this session; the worker drops its spooled events");
                    if let Err(err) = self.store.drop_spool(session_id) {
                        tracing::warn!(session = %session_id, error = %err, "could not drop the spooled events");
                        return Flush::Later;
                    }
                    return Flush::Done;
                }
                Err(HostError::TooLarge(why)) if !replaced => {
                    // The cap in `append_event` makes this rare. Send a note
                    // in place of the first event, so the session goes on.
                    tracing::warn!(session = %session_id, seq = first, %why, "a spooled event does not fit in a frame; the worker sends a note instead");
                    let note = SessionEvent::Error {
                        code: "event_too_large".to_owned(),
                        message: format!(
                            "Event {first} of this session did not fit in a frame, so the worker dropped it."
                        ),
                    };
                    if self.store.replace_event(session_id, first, &note).is_err() {
                        return Flush::Later;
                    }
                    replaced = true;
                }
                Err(HostError::Offline) => return Flush::Offline,
                Err(err) => {
                    tracing::debug!(session = %session_id, error = %err, "could not send spooled events");
                    return if self.link.is_connected() {
                        Flush::Later
                    } else {
                        Flush::Offline
                    };
                }
            }
        }
    }

    // Inventory.

    /// Probes every enabled account and keeps the results.
    pub async fn probe_accounts(self: &Arc<Self>) {
        let config = self.config_snapshot();
        let probes = config
            .accounts
            .iter()
            .filter(|a| a.enabled)
            .map(|account| async move { (account.id, self.probe_account(account).await) });
        let results = futures::future::join_all(probes).await;
        let mut statuses = lock(&self.statuses);
        for (id, status) in results {
            statuses.insert(id, status);
        }
    }

    /// Probes one account. When its managed tool is not installed, the
    /// probe does not wait: the install starts in the background and probes
    /// the account when it ends.
    pub async fn probe_account(self: &Arc<Self>, account: &AccountConfig) -> AccountStatus {
        let Some(adapter) = self.harnesses.get(&account.harness).cloned() else {
            return AccountStatus::unknown(
                format!(
                    "This build has no adapter for {}.",
                    account.harness.display_name()
                ),
                Default::default(),
            );
        };
        // A harness with no proof of the boundary starts no process, not even
        // for a probe.
        if let consort_harness::boundary::BoundarySupport::Off(reason) =
            adapter.boundary_support(account)
        {
            return AccountStatus::unknown(reason, adapter.capabilities(account));
        }
        let mut prepared = match self.with_profile(account) {
            Ok(prepared) => prepared,
            Err(err) => {
                return AccountStatus {
                    auth_state: AuthState::Error,
                    auth_detail: Some(err.message),
                    capabilities: adapter.capabilities(account),
                    ..AccountStatus::default()
                };
            }
        };
        let mut lease = None;
        if let Some(tool) = adapter.managed_tool(&prepared) {
            if self.tools.store().installed(&tool).is_none() {
                self.install_in_background(account.id, tool.clone());
                return AccountStatus::unknown(
                    format!("Installing {}.", crate::tools::tool_name(&tool)),
                    adapter.capabilities(account),
                );
            }
            match self.tools.store().launch(&tool) {
                Ok(launch) => prepared.runtime.launch = Some(managed_launch(launch)),
                Err(err) => {
                    return AccountStatus {
                        auth_state: AuthState::Error,
                        auth_detail: Some(format!(
                            "Could not start {}: {err}",
                            crate::tools::tool_name(&tool)
                        )),
                        capabilities: adapter.capabilities(account),
                        ..AccountStatus::default()
                    };
                }
            }
            lease = Some(self.tools.lease(&tool));
        }
        if let Err(err) = self.apply_boundary(&mut prepared, adapter.as_ref()) {
            return AccountStatus {
                auth_state: AuthState::Error,
                auth_detail: Some(err.message),
                capabilities: adapter.capabilities(account),
                ..AccountStatus::default()
            };
        }
        let status = adapter.probe(&prepared).await;
        drop(lease);
        self.apply_login_expired(account.id, status)
    }

    /// Marks the account's login as expired in its inventory entry, until a
    /// sign-in clears it. See [`consort_harness::LOGIN_EXPIRED_ERROR_CODE`].
    pub fn mark_login_expired(&self, account_id: AccountId) {
        lock(&self.login_expired).insert(account_id);
        lock(&self.statuses)
            .entry(account_id)
            .and_modify(|status| {
                status.auth_state = AuthState::Error;
                status.auth_detail = Some(consort_harness::LOGIN_EXPIRED_MESSAGE.to_owned());
            })
            .or_insert_with(|| AccountStatus {
                auth_state: AuthState::Error,
                auth_detail: Some(consort_harness::LOGIN_EXPIRED_MESSAGE.to_owned()),
                ..AccountStatus::default()
            });
        self.send_inventory(false);
    }

    /// Forgets that the account's login expired: the user signed in again,
    /// or a turn on the account worked.
    pub fn clear_login_expired(&self, account_id: AccountId) {
        lock(&self.login_expired).remove(&account_id);
    }

    /// A probe that says "signed in" for an account whose login a turn found
    /// expired stays "expired". A probe that finds the account signed out or
    /// in error says more, and stays as it is.
    fn apply_login_expired(
        &self,
        account_id: AccountId,
        mut status: AccountStatus,
    ) -> AccountStatus {
        if status.auth_state == AuthState::SignedIn
            && lock(&self.login_expired).contains(&account_id)
        {
            status.auth_state = AuthState::Error;
            status.auth_detail = Some(consort_harness::LOGIN_EXPIRED_MESSAGE.to_owned());
        }
        status
    }

    /// Installs the managed tool of an account in the background, then
    /// probes the account and sends the inventory.
    fn install_in_background(self: &Arc<Self>, account_id: AccountId, tool: consort_tools::ToolId) {
        let core = self.clone();
        tokio::spawn(async move {
            core.inventory_changed();
            let result = core.tools.ensure(&tool).await;
            let config = core.config_snapshot();
            let Some(account) = config.account(account_id).cloned() else {
                core.inventory_changed();
                return;
            };
            let status = match result {
                Ok(_) => core.probe_account(&account).await,
                Err(err) => AccountStatus {
                    auth_state: AuthState::Error,
                    auth_detail: Some(format!(
                        "Could not install {}: {err}. Try again from Settings > Providers.",
                        crate::tools::tool_name(&tool)
                    )),
                    ..AccountStatus::default()
                },
            };
            lock(&core.statuses).insert(account_id, status);
            core.send_inventory(false);
        });
    }

    /// Marks an account as signed out, because one of its sessions reported
    /// it. `detail` is the text for the user, with the step to sign in. The
    /// models stay as the last probe found them. The hub does not place work
    /// on an account that is not signed in. The next probe can set the state
    /// back. Sends the new inventory to the hub at once.
    pub fn mark_signed_out(&self, account_id: AccountId, detail: &str) {
        let profile = self
            .config_snapshot()
            .account(account_id)
            .is_some_and(|a| a.profile);
        // A profile account signs in from the app, not with a command on
        // the worker.
        let detail = if profile {
            &format!("Signed out. {}", consort_proto::model::SIGN_IN_IN_SETTINGS)
        } else {
            detail
        };
        lock(&self.statuses)
            .entry(account_id)
            .and_modify(|status| {
                status.auth_state = AuthState::SignedOut;
                status.auth_detail = Some(detail.to_owned());
            })
            .or_insert_with(|| AccountStatus {
                auth_state: AuthState::SignedOut,
                auth_detail: Some(detail.to_owned()),
                ..AccountStatus::default()
            });
        self.send_inventory(false);
    }

    pub fn inventory(&self) -> Inventory {
        let config = self.config_snapshot();
        let statuses = lock(&self.statuses).clone();
        let accounts = config
            .accounts
            .iter()
            .map(|account| {
                let capabilities = self
                    .harnesses
                    .get(&account.harness)
                    .map(|h| self.account_capabilities(account, h.as_ref()))
                    .unwrap_or_default();
                let status = statuses.get(&account.id);
                let enabled = account.enabled && self.harnesses.contains_key(&account.harness);
                let mut auth_detail = status.and_then(|s| s.auth_detail.clone()).or_else(|| {
                    (!self.harnesses.contains_key(&account.harness)).then(|| {
                        format!(
                            "This build has no adapter for {}.",
                            account.harness.display_name()
                        )
                    })
                });
                // While the managed tool installs, the account says so.
                if let Some(tool) = self
                    .harnesses
                    .get(&account.harness)
                    .and_then(|h| h.managed_tool(account))
                    && self.tools.installing(&tool)
                {
                    auth_detail = Some(format!("Installing {}.", crate::tools::tool_name(&tool)));
                }
                HarnessAccount {
                    account_id: account.id,
                    harness: crate::accounts::inventory_harness(account),
                    label: account.label.clone(),
                    harness_version: status.and_then(|s| s.harness_version.clone()),
                    auth_state: status.map(|s| s.auth_state).unwrap_or(AuthState::Unknown),
                    auth_detail,
                    blocked_reason: self.harnesses.get(&account.harness).and_then(|adapter| {
                        match adapter.boundary_support(account) {
                            consort_harness::boundary::BoundarySupport::Off(reason) => Some(reason),
                            _ => None,
                        }
                    }),
                    models: status.map(|s| s.models.clone()).unwrap_or_default(),
                    capabilities,
                    enabled,
                    profile: account.profile,
                    email: status.and_then(|s| s.email.clone()),
                    plan: status.and_then(|s| s.plan.clone()),
                    secret_names: self.secret_names(account),
                    sign_in_methods: crate::accounts::sign_in_methods(account, status),
                }
            })
            .collect();
        let active = u32::try_from(lock(&self.sessions).len()).unwrap_or(u32::MAX);
        Inventory {
            boundary: boundary_info(),
            hash: String::new(),
            hostname: gethostname::gethostname().to_string_lossy().into_owned(),
            os: Os::from_std_os(std::env::consts::OS).unwrap_or(Os::Linux),
            arch: std::env::consts::ARCH.to_owned(),
            tags: self.inventory_tags(&config),
            accounts,
            local_mcp_servers: config
                .local_mcp_servers
                .iter()
                .filter(|s| local_server_name_problem(&s.name).is_none())
                .map(|s| LocalMcpServerInfo {
                    name: s.name.clone(),
                    description: s.description.clone(),
                    tools: Vec::new(),
                })
                .collect(),
            checkouts: config.checkout_infos(),
            policy: config.policy.to_proto(),
            load: WorkerLoad {
                active_sessions: active,
            },
            tools: self.tools.statuses(),
            update: None,
        }
        .with_hash()
    }

    /// Sends the inventory when it changed, or always with `force`.
    pub fn send_inventory(&self, force: bool) {
        if !self.link.is_connected() {
            return;
        }
        let inventory = self.inventory();
        {
            let mut last = lock(&self.last_inventory_hash);
            if !force && last.as_deref() == Some(inventory.hash.as_str()) {
                return;
            }
            *last = Some(inventory.hash.clone());
        }
        if let Err(err) = self.link.send_event(&inventory) {
            tracing::debug!(error = %err, "could not send the inventory");
            *lock(&self.last_inventory_hash) = None;
        }
    }
}

impl LinkHandler for Core {
    fn worker_id(&self) -> WorkerId {
        self.worker_id
    }

    fn ws_path(&self) -> &str {
        self.ws_path
    }

    fn hub_url(&self) -> &str {
        &self.hub_url
    }

    fn sign(&self, payload: &[u8]) -> Signature {
        self.identity.sign(payload)
    }

    fn hello(&self) -> WorkerHello {
        let sessions = self
            .store
            .open_sessions()
            .unwrap_or_default()
            .into_iter()
            .map(|r| OpenSession {
                session_id: r.start.session_id,
                last_seq: r.last_seq,
                state: r.state,
            })
            .collect();
        WorkerHello {
            worker_id: self.worker_id,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
            sessions,
        }
    }

    fn connected(self: Arc<Self>, result: WorkerHelloResult) {
        for seq in &result.sessions {
            let _ = self.store.ack(seq.session_id, seq.last_seq);
        }
        self.spool_notify.notify_one();
        self.send_inventory(true);
    }

    fn command(self: Arc<Self>, request: RequestFrame) {
        self.dispatch(request);
    }
}

/// Sends spooled events whenever new ones arrive or the link connects.
pub(crate) async fn spool_loop(core: Arc<Core>, mut shutdown: watch::Receiver<bool>) {
    let mut connected = core.link.subscribe();
    loop {
        if core.link.is_connected() {
            core.flush_spool().await;
        }
        tokio::select! {
            () = core.spool_notify.notified() => {}
            _ = connected.changed() => {}
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = shutdown.changed() => break,
        }
    }
}

/// Sends the inventory when it changes, and probes the accounts now and
/// then.
/// The boundary of this machine. The first look is the answer: the tools do
/// not change while the session host runs.
fn boundary_info() -> Option<consort_proto::model::BoundaryInfo> {
    use consort_harness::boundary::{BoundaryKind, detect};
    use consort_proto::model::BoundaryInfo;
    static FOUND: std::sync::OnceLock<Option<BoundaryInfo>> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| match detect() {
        Ok(BoundaryKind::MacSandbox) => Some(BoundaryInfo::MacSandbox),
        Ok(BoundaryKind::Bubblewrap) => Some(BoundaryInfo::Bubblewrap),
        Err(_) => None,
    })
}

pub(crate) async fn inventory_loop(core: Arc<Core>, mut shutdown: watch::Receiver<bool>) {
    core.probe_accounts().await;
    core.send_inventory(true);
    let mut last_probe = tokio::time::Instant::now();
    // The cache was pruned when the worker started.
    let mut last_prune = tokio::time::Instant::now();
    loop {
        tokio::select! {
            () = core.inventory_notify.notified() => core.send_inventory(false),
            () = tokio::time::sleep(Duration::from_secs(60)) => {
                if last_prune.elapsed() > crate::skills::PRUNE_EVERY {
                    core.prune_skill_cache();
                    last_prune = tokio::time::Instant::now();
                }
                if last_probe.elapsed() > Duration::from_secs(15 * 60) {
                    core.probe_accounts().await;
                    last_probe = tokio::time::Instant::now();
                }
                core.send_inventory(false);
                let _ = core.store.prune_commands(Timestamp::now());
            }
            _ = shutdown.changed() => break,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::bridge::McpHttpClient;
    use crate::store::tests::{record, start};
    use consort_harness::{AccountConfig, HarnessKind};
    use consort_proto::frame::ResponseFrame;
    use consort_proto::model::SessionState;
    use consort_proto::session_host_link::{SessionTurn, TurnInput};
    use consort_testkit::{FakeHarness, FakeOpenFailure, FakeScript, FakeStep};

    pub(crate) fn test_core(
        dir: &std::path::Path,
        harnesses: Harnesses,
        config: HostToml,
    ) -> Arc<Core> {
        Arc::new(Core {
            config_dir: dir.to_path_buf(),
            config: RwLock::new(config),
            ws_path: consort_proto::auth::SESSION_HOST_WS_PATH,
            identity: Identity::generate(),
            worker_id: WorkerId::new(),
            hub_url: "http://127.0.0.1:1".into(),
            store: Store::in_memory().unwrap(),
            link: Link::new(),
            harnesses,
            sessions: Mutex::new(HashMap::new()),
            statuses: Mutex::new(HashMap::new()),
            login_expired: Mutex::default(),
            spool_notify: Notify::new(),
            inventory_notify: Notify::new(),
            last_inventory_hash: Mutex::new(None),
            bridge_program: None,
            inflight: Mutex::new(HashSet::new()),
            stopping: watch::channel(false).0,
            lock: Mutex::new(None),
            consort_home: dir.join("home"),
            tools: Arc::new(
                crate::tools::Tools::new(dir.join("home").join("tools"), false).unwrap(),
            ),
            sign_ins: Mutex::new(HashMap::new()),
            browser_sign_in: tokio::sync::Mutex::new(()),
            detected_tags: vec!["macos".into(), "arm64".into()],
            visual: Arc::new(crate::visual::testing::FakeEngine::new(240)),
        })
    }

    /// A worker with one fake Claude Code account.
    fn fake_worker(fake: &FakeHarness) -> (tempfile::TempDir, Arc<Core>, AccountId) {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountConfig::default_for(HarnessKind::ClaudeCode);
        let account_id = account.id;
        let config = HostToml {
            accounts: vec![account],
            ..HostToml::default()
        };
        let mut harnesses: Harnesses = HashMap::new();
        harnesses.insert(
            HarnessKind::ClaudeCode,
            Arc::new(fake.clone()) as Arc<dyn Harness>,
        );
        let core = test_core(dir.path(), harnesses, config);
        (dir, core, account_id)
    }

    fn request(id: &FrameId, command: &HubToSessionHost) -> RequestFrame {
        let Frame::Request(request) = Frame::family_request(id.clone(), command).unwrap() else {
            panic!("a hub command is a request")
        };
        request
    }

    /// The responses that the worker sent, by command ID.
    fn responses(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<ResponseFrame> {
        let mut out = Vec::new();
        while let Ok(text) = rx.try_recv() {
            if let Ok(Frame::Response(r)) = Frame::parse(&text) {
                out.push(r);
            }
        }
        out
    }

    async fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..250 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {what}");
    }

    /// The batches that the fake hub saw: the session and the numbers.
    type SeenBatches = Arc<Mutex<Vec<(SessionId, Vec<u64>)>>>;

    /// A fake hub for `session.events`: it answers `not_found` for
    /// `unknown` and stores everything else.
    fn spawn_events_hub(
        core: Arc<Core>,
        unknown: Option<SessionId>,
        mut rx: mpsc::UnboundedReceiver<String>,
    ) -> SeenBatches {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Some(text) = rx.recv().await {
                let Ok(Frame::Request(req)) = Frame::parse(&text) else {
                    continue;
                };
                let batch: SessionEvents = req.decode_as().unwrap();
                let seqs: Vec<u64> = batch.events.iter().map(|e| e.seq).collect();
                log.lock().unwrap().push((batch.session_id, seqs.clone()));
                let result = if Some(batch.session_id) == unknown {
                    Err(ErrorBody::new(
                        ErrorCode::NotFound,
                        "The hub does not know this session. Stop it.",
                    ))
                } else {
                    Ok(
                        json!({"session_id": batch.session_id, "last_seq": seqs.last().copied().unwrap_or(0)}),
                    )
                };
                core.link.resolve(ResponseFrame { re: req.id, result });
            }
        });
        seen
    }

    fn active() -> SessionEvent {
        SessionEvent::State {
            state: SessionState::Active,
        }
    }

    #[tokio::test]
    async fn a_session_that_the_hub_does_not_know_does_not_block_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let core = test_core(dir.path(), HashMap::new(), HostToml::default());
        // Session IDs are UUIDv7 and sort by time, so `old` is sent first.
        let old = SessionId::new();
        tokio::time::sleep(Duration::from_millis(5)).await;
        let new = SessionId::new();
        for id in [old, new] {
            core.store.put_session(&record(id)).unwrap();
            core.store.append_event(id, &active()).unwrap();
        }
        let (tx, rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let seen = spawn_events_hub(core.clone(), Some(old), rx);
        core.flush_spool().await;
        let seen = seen.lock().unwrap().clone();
        assert!(seen.iter().any(|(s, _)| *s == new), "{seen:?}");
        assert!(
            core.store.next_batch(new).unwrap().is_empty(),
            "the hub has it"
        );
        assert!(
            core.store.next_batch(old).unwrap().is_empty(),
            "the spool of an unknown session is dropped"
        );
    }

    #[tokio::test]
    async fn an_oversized_event_does_not_block_any_session() {
        let dir = tempfile::tempdir().unwrap();
        let core = test_core(dir.path(), HashMap::new(), HostToml::default());
        let big = SessionId::new();
        tokio::time::sleep(Duration::from_millis(5)).await;
        let other = SessionId::new();
        for id in [big, other] {
            core.store.put_session(&record(id)).unwrap();
        }
        let huge = SessionEvent::Error {
            code: "x".into(),
            message: "y".repeat(1_200_000),
        };
        core.store.append_event(big, &huge).unwrap();
        core.store.append_event(other, &active()).unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let seen = spawn_events_hub(core.clone(), None, rx);
        core.flush_spool().await;
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen, vec![(big, vec![1]), (other, vec![1])]);
        assert!(core.store.spooled_sessions().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unreadable_spool_row_does_not_block_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let core = test_core(dir.path(), HashMap::new(), HostToml::default());
        let id = SessionId::new();
        core.store.put_session(&record(id)).unwrap();
        core.store.append_event(id, &active()).unwrap();
        core.store.put_raw_event(id, 2, "not json");
        let (tx, rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let seen = spawn_events_hub(core.clone(), None, rx);
        core.flush_spool().await;
        assert_eq!(seen.lock().unwrap().clone(), vec![(id, vec![1, 2])]);
        assert!(core.store.spooled_sessions().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_command_that_arrives_again_while_it_runs_runs_once() {
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, mut rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = SessionId::new();
        let mut command = start(session_id);
        command.account_id = account_id;
        command.turn = Some(TurnInput {
            text: "Go".into(),
            attachments: vec![],
        });
        let command: HubToSessionHost = command.into();
        let id = FrameId::new("cmd-start");
        // The link drops, and the hub sends the same command again while it
        // runs.
        core.dispatch(request(&id, &command));
        core.dispatch(request(&id, &command));
        wait_until("the start result", || {
            core.store.command_result(id.as_str()).unwrap().is_some()
        })
        .await;
        let answers = responses(&mut rx);
        assert!(!answers.is_empty());
        for answer in &answers {
            assert_eq!(answer.re, id);
            assert!(answer.result.is_ok(), "no conflict: {:?}", answer.result);
        }
        assert_eq!(record.lock().unwrap().opened.len(), 1, "one harness");
        assert_eq!(record.lock().unwrap().turns.len(), 1, "one first turn");

        // A turn that arrives twice runs once too.
        let turn: HubToSessionHost = SessionTurn {
            session_id,
            turn: TurnInput {
                text: "Again".into(),
                attachments: vec![],
            },
            channel_files: None,
            approval_mode: None,
            model_change: None,
        }
        .into();
        let turn_id = FrameId::new("cmd-turn");
        core.dispatch(request(&turn_id, &turn));
        core.dispatch(request(&turn_id, &turn));
        wait_until("the turn result", || {
            core.store
                .command_result(turn_id.as_str())
                .unwrap()
                .is_some()
        })
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(record.lock().unwrap().turns.len(), 2);
        // After the run, the stored result answers.
        core.dispatch(request(&turn_id, &turn));
        assert!(
            responses(&mut rx)
                .iter()
                .all(|r| r.re == turn_id && r.result.is_ok())
        );
        assert_eq!(record.lock().unwrap().turns.len(), 2);
    }

    #[tokio::test]
    async fn a_failed_start_frees_the_session_id() {
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, mut rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = SessionId::new();
        // An account that this worker does not have.
        let bad: HubToSessionHost = start(session_id).into();
        core.dispatch(request(&FrameId::new("c1"), &bad));
        wait_until("the failed start to end its actor", || {
            core.store.command_result("c1").unwrap().is_some() && lock(&core.sessions).is_empty()
        })
        .await;
        assert_eq!(core.inventory().load.active_sessions, 0);
        let mut good = start(session_id);
        good.account_id = account_id;
        let good: HubToSessionHost = good.into();
        core.dispatch(request(&FrameId::new("c2"), &good));
        wait_until("the second start", || {
            core.store.command_result("c2").unwrap().is_some()
        })
        .await;
        let answers = responses(&mut rx);
        let second = answers.iter().find(|r| r.re.as_str() == "c2").unwrap();
        assert!(second.result.is_ok(), "{:?}", second.result);
    }

    async fn endpoint_answers(url: &str, token: &str) -> bool {
        let client = McpHttpClient::new(url, token).unwrap();
        client
            .post(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}))
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn a_failed_first_turn_closes_the_harness_and_the_endpoint() {
        let mut fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        fake.fail_turn = true;
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = SessionId::new();
        let mut command = start(session_id);
        command.account_id = account_id;
        command.turn = Some(TurnInput::default());
        core.dispatch(request(&FrameId::new("c1"), &command.into()));
        wait_until("the failed start to end its actor", || {
            core.store.command_result("c1").unwrap().is_some() && lock(&core.sessions).is_empty()
        })
        .await;
        let stored = core.store.command_result("c1").unwrap().unwrap();
        assert!(stored.into_result().is_err());
        let (url, token) = {
            let r = record.lock().unwrap();
            assert_eq!(r.closed, 1, "the harness is closed");
            let hub = &r.opened[0].mcp_servers[0];
            (hub.url.clone(), hub.bearer_token.clone().unwrap())
        };
        assert!(
            !endpoint_answers(&url, &token).await,
            "the endpoint is stopped"
        );
        assert_eq!(
            core.store.session(session_id).unwrap().unwrap().state,
            SessionState::Lost
        );
    }

    #[tokio::test]
    async fn a_lost_harness_stops_its_endpoint_and_frees_the_session() {
        let script = FakeScript {
            turns: vec![vec![FakeStep::Event(Box::new(SessionEvent::State {
                state: SessionState::Lost,
            }))]],
        };
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, script);
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let mut command = start(SessionId::new());
        command.account_id = account_id;
        command.turn = Some(TurnInput::default());
        core.dispatch(request(&FrameId::new("c1"), &command.into()));
        wait_until("the lost session to end its actor", || {
            core.store.command_result("c1").unwrap().is_some() && lock(&core.sessions).is_empty()
        })
        .await;
        let (url, token) = {
            let r = record.lock().unwrap();
            let hub = &r.opened[0].mcp_servers[0];
            (hub.url.clone(), hub.bearer_token.clone().unwrap())
        };
        assert!(
            !endpoint_answers(&url, &token).await,
            "the token works no more"
        );
        assert_eq!(core.inventory().load.active_sessions, 0);
    }

    #[tokio::test]
    async fn a_failed_restore_frees_the_session() {
        let mut fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        fake.fail_resume = true;
        let (_dir, core, account_id) = fake_worker(&fake);
        let session_id = SessionId::new();
        let mut stored = record(session_id);
        stored.start.account_id = account_id;
        stored.native_ref = Some(consort_proto::ids::NativeRef::new("old"));
        core.store.put_session(&stored).unwrap();
        let jobs = core.spawn_session(session_id);
        let _ = jobs.send(crate::sessions::Job::Restore(Box::new(stored)));
        wait_until("the failed restore to end its actor", || {
            lock(&core.sessions).is_empty()
        })
        .await;
        assert_eq!(
            core.store.session(session_id).unwrap().unwrap().state,
            SessionState::Lost
        );
    }

    #[tokio::test]
    async fn commands_left_for_an_ended_actor_get_an_answer_that_is_not_stored() {
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        let (_dir, core, _account_id) = fake_worker(&fake);
        let (tx, mut rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = SessionId::new();
        let jobs = core.spawn_session(session_id);
        jobs.send(crate::sessions::Job::Shutdown).unwrap();
        let turn: HubToSessionHost = SessionTurn {
            session_id,
            turn: TurnInput::default(),
            channel_files: None,
            approval_mode: None,
            model_change: None,
        }
        .into();
        lock(&core.inflight).insert("c9".to_owned());
        jobs.send(crate::sessions::Job::Command {
            id: FrameId::new("c9"),
            command: Box::new(turn),
        })
        .unwrap();
        wait_until("the actor to end", || lock(&core.sessions).is_empty()).await;
        let answers = responses(&mut rx);
        let answer = answers.iter().find(|r| r.re.as_str() == "c9").unwrap();
        assert_eq!(
            answer.result.as_ref().unwrap_err().code,
            ErrorCode::NotFound
        );
        assert!(
            core.store.command_result("c9").unwrap().is_none(),
            "a resent command runs again"
        );
        assert!(lock(&core.inflight).is_empty());
    }

    /// The events in the spool of a session.
    fn spooled(core: &Core, session_id: SessionId) -> Vec<SessionEvent> {
        core.store
            .next_batch(session_id)
            .unwrap()
            .into_iter()
            .map(|e| e.event)
            .collect()
    }

    fn has_error(events: &[SessionEvent], wanted: &str) -> bool {
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::Error { code, .. } if code == wanted))
    }

    /// Starts a session with a first turn on the fake worker and waits for
    /// the start result.
    async fn start_with_turn(
        core: &Arc<Core>,
        account_id: AccountId,
        mode: consort_proto::model::ApprovalMode,
    ) -> SessionId {
        let session_id = SessionId::new();
        let mut command = start(session_id);
        command.account_id = account_id;
        command.approval_mode = mode;
        command.turn = Some(TurnInput {
            text: "Go".into(),
            attachments: vec![],
        });
        let id = format!("start-{session_id}");
        core.dispatch(request(&FrameId::new(&id), &command.into()));
        wait_until("the start result", || {
            core.store.command_result(&id).unwrap().is_some()
        })
        .await;
        session_id
    }

    #[tokio::test]
    async fn a_harness_that_exits_while_the_worker_stops_keeps_its_session() {
        // The harness exits as it does when the stop signal reaches it too.
        let script = FakeScript {
            turns: vec![vec![
                FakeStep::Event(Box::new(SessionEvent::Error {
                    code: "marker".into(),
                    message: "about to exit".into(),
                })),
                FakeStep::Event(Box::new(SessionEvent::Error {
                    code: "harness_exited".into(),
                    message: "The harness process exited.".into(),
                })),
                FakeStep::Event(Box::new(SessionEvent::State {
                    state: SessionState::Lost,
                })),
            ]],
        };
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, script);
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id =
            start_with_turn(&core, account_id, consort_proto::model::ApprovalMode::Ask).await;
        wait_until("the marker", || {
            has_error(&spooled(&core, session_id), "marker")
        })
        .await;
        // The exit event reached the pump before the worker set its flag.
        // The pump holds it, and drops it once the flag is set.
        tokio::time::sleep(Duration::from_millis(50)).await;
        core.stopping.send_replace(true);
        tokio::time::sleep(crate::sessions::EXIT_HOLD * 2).await;
        let events = spooled(&core, session_id);
        assert!(!has_error(&events, "harness_exited"), "{events:?}");
        assert!(
            !events.iter().any(|e| matches!(
                e,
                SessionEvent::State {
                    state: SessionState::Lost
                }
            )),
            "{events:?}"
        );
        assert_ne!(
            core.store.session(session_id).unwrap().unwrap().state,
            SessionState::Lost,
            "the next run resumes the session"
        );
    }

    #[tokio::test]
    async fn a_login_that_a_turn_found_expired_shows_expired_until_a_sign_in() {
        let script = FakeScript {
            turns: vec![vec![
                FakeStep::Event(Box::new(SessionEvent::Error {
                    code: consort_harness::LOGIN_EXPIRED_ERROR_CODE.into(),
                    message: consort_harness::LOGIN_EXPIRED_MESSAGE.into(),
                })),
                FakeStep::Fail(consort_harness::LOGIN_EXPIRED_MESSAGE.into()),
            ]],
        };
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, script);
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        // The probe says signed in: it cannot see the expiry.
        lock(&core.statuses).insert(
            account_id,
            AccountStatus {
                auth_state: AuthState::SignedIn,
                ..AccountStatus::default()
            },
        );
        let session_id =
            start_with_turn(&core, account_id, consort_proto::model::ApprovalMode::Ask).await;
        wait_until("the account is marked", || {
            lock(&core.statuses)[&account_id].auth_state == AuthState::Error
        })
        .await;
        assert_eq!(
            lock(&core.statuses)[&account_id].auth_detail.as_deref(),
            Some("The login expired. Sign in again.")
        );
        let _ = session_id;

        // The next probe still says signed in. The mark stays.
        let probed = AccountStatus {
            auth_state: AuthState::SignedIn,
            ..AccountStatus::default()
        };
        let kept = core.apply_login_expired(account_id, probed.clone());
        assert_eq!(kept.auth_state, AuthState::Error);
        assert!(kept.auth_detail.unwrap().contains("expired"));
        // A probe that finds the account signed out says more, and stays.
        let signed_out = AccountStatus {
            auth_state: AuthState::SignedOut,
            auth_detail: Some("Not signed in.".into()),
            ..AccountStatus::default()
        };
        assert_eq!(
            core.apply_login_expired(account_id, signed_out).auth_state,
            AuthState::SignedOut
        );
        // Another account is not touched.
        assert_eq!(
            core.apply_login_expired(AccountId::new(), probed.clone())
                .auth_state,
            AuthState::SignedIn
        );
        // A new sign-in clears the mark.
        core.clear_login_expired(account_id);
        assert_eq!(
            core.apply_login_expired(account_id, probed).auth_state,
            AuthState::SignedIn
        );
    }

    #[tokio::test]
    async fn a_signed_out_open_marks_the_account_signed_out() {
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        fake.set_open_failure(Some(FakeOpenFailure::SignedOut(
            "Authentication required".into(),
        )));
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = SessionId::new();
        let mut command = start(session_id);
        command.account_id = account_id;
        core.dispatch(request(&FrameId::new("c1"), &command.into()));
        wait_until("the failed start", || {
            core.store.command_result("c1").unwrap().is_some() && lock(&core.sessions).is_empty()
        })
        .await;
        let hint = "Authentication required. Run `claude` on this machine and sign in with /login.";
        // Placement stops choosing the account now, not after the next
        // probe.
        let account = core
            .inventory()
            .accounts
            .into_iter()
            .find(|a| a.account_id == account_id)
            .unwrap();
        assert_eq!(account.auth_state, AuthState::SignedOut);
        assert_eq!(
            lock(&core.statuses)[&account_id].auth_detail.as_deref(),
            Some(hint)
        );
        let events = spooled(&core, session_id);
        assert!(
            events.iter().any(|e| matches!(
                e,
                SessionEvent::Error { code, message } if code == "signed_out" && message == hint
            )),
            "{events:?}"
        );
        let answer = core
            .store
            .command_result("c1")
            .unwrap()
            .unwrap()
            .into_result()
            .unwrap_err();
        assert!(answer.message.contains(hint), "{}", answer.message);
    }

    /// Whether the worker sent an answer to `id`.
    fn answered(rx: &mut mpsc::UnboundedReceiver<String>, id: &str) -> bool {
        responses(rx).iter().any(|r| r.re.as_str() == id)
    }

    #[tokio::test]
    async fn a_start_that_the_stop_cuts_short_runs_again_after_the_restart() {
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, mut rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = SessionId::new();
        let mut command = start(session_id);
        command.account_id = account_id;
        command.turn = Some(TurnInput {
            text: "Go".into(),
            attachments: vec![],
        });
        let command: HubToSessionHost = command.into();
        // The stop signal reaches the harness while it opens.
        fake.set_open_failure(Some(FakeOpenFailure::Exited));
        core.stopping.send_replace(true);
        core.dispatch(request(&FrameId::new("c1"), &command));
        wait_until("the start to end its actor", || {
            lock(&core.sessions).is_empty()
        })
        .await;
        assert!(
            core.store.command_result("c1").unwrap().is_none(),
            "no stored result, so the hub's resend runs the start"
        );
        assert!(!answered(&mut rx, "c1"), "the hub gets no error");
        assert!(lock(&core.inflight).is_empty());
        let stored = core.store.session(session_id).unwrap().unwrap();
        assert_eq!(stored.state, SessionState::Starting);
        assert!(stored.native_ref.is_none());
        let events = spooled(&core, session_id);
        assert!(!has_error(&events, "start_failed"), "{events:?}");
        assert!(
            !events.iter().any(|e| matches!(
                e,
                SessionEvent::State {
                    state: SessionState::Lost
                }
            )),
            "{events:?}"
        );
        // The next run does not resume it. The hub sends the start again,
        // and it runs.
        assert_eq!(restore_sessions(&core).unwrap(), 0);
        core.stopping.send_replace(false);
        fake.set_open_failure(None);
        core.dispatch(request(&FrameId::new("c1"), &command));
        wait_until("the second start", || {
            core.store.command_result("c1").unwrap().is_some()
        })
        .await;
        let stored = core.store.command_result("c1").unwrap().unwrap();
        assert!(stored.into_result().is_ok());
        assert_eq!(record.lock().unwrap().turns.len(), 1, "one first turn");
    }

    #[tokio::test]
    async fn a_first_turn_that_the_stop_cuts_short_leaves_no_session_to_resume() {
        let mut fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        // The harness opens, and then the stop signal reaches it before it
        // takes the first turn.
        fake.fail_turn = true;
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, mut rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = SessionId::new();
        let mut command = start(session_id);
        command.account_id = account_id;
        command.turn = Some(TurnInput::default());
        core.stopping.send_replace(true);
        core.dispatch(request(&FrameId::new("c1"), &command.into()));
        wait_until("the start to end its actor", || {
            lock(&core.sessions).is_empty()
        })
        .await;
        assert_eq!(record.lock().unwrap().closed, 1, "the harness is closed");
        assert!(core.store.command_result("c1").unwrap().is_none());
        assert!(!answered(&mut rx, "c1"));
        // The harness reference is gone, so the next run does not resume a
        // session that has no owner on the hub.
        let stored = core.store.session(session_id).unwrap().unwrap();
        assert_eq!(stored.state, SessionState::Starting);
        assert!(stored.native_ref.is_none());
        assert_eq!(restore_sessions(&core).unwrap(), 0);
        assert!(lock(&core.sessions).is_empty());
        let events = spooled(&core, session_id);
        assert!(!has_error(&events, "start_failed"), "{events:?}");
    }

    #[tokio::test]
    async fn an_approval_too_long_to_show_is_denied_with_a_reason() {
        let script = FakeScript {
            turns: vec![vec![
                FakeStep::Approval {
                    request_id: "r1".into(),
                    command: format!("echo {}", "x".repeat(300 * 1024)),
                },
                FakeStep::Text("After.".into()),
            ]],
        };
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, script);
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id =
            start_with_turn(&core, account_id, consort_proto::model::ApprovalMode::Ask).await;
        wait_until("the denial", || !record.lock().unwrap().answers.is_empty()).await;
        let (request_id, answer) = record.lock().unwrap().answers[0].clone();
        assert_eq!(request_id.as_str(), "r1");
        assert_eq!(answer.option_id.as_deref(), Some("deny"));
        wait_until("the steer", || !record.lock().unwrap().steers.is_empty()).await;
        let steer = record.lock().unwrap().steers[0].text.clone();
        assert!(steer.contains("too long to show"), "{steer}");
        let events = spooled(&core, session_id);
        assert!(has_error(&events, "approval_too_long"), "{events:?}");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SessionEvent::RequestOpened(_))),
            "the user never sees a cut approval"
        );
    }

    #[tokio::test]
    async fn a_slow_steer_does_not_hold_an_interrupt() {
        let script = FakeScript {
            turns: vec![vec![FakeStep::Sleep(Duration::from_secs(10))]],
        };
        let mut fake = FakeHarness::new(HarnessKind::ClaudeCode, script);
        fake.steer_delay = Duration::from_secs(2);
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id =
            start_with_turn(&core, account_id, consort_proto::model::ApprovalMode::Ask).await;
        let steer: HubToSessionHost = consort_proto::session_host_link::SessionSteer {
            session_id,
            turn: TurnInput {
                text: "Also this".into(),
                attachments: vec![],
            },
        }
        .into();
        let interrupt: HubToSessionHost =
            consort_proto::session_host_link::SessionInterrupt { session_id }.into();
        core.dispatch(request(&FrameId::new("steer"), &steer));
        core.dispatch(request(&FrameId::new("interrupt"), &interrupt));
        wait_until("the interrupt result", || {
            core.store.command_result("interrupt").unwrap().is_some()
        })
        .await;
        assert!(
            core.store.command_result("steer").unwrap().is_none(),
            "the interrupt did not wait for the steer"
        );
        assert_eq!(record.lock().unwrap().interrupts, 1);
        for _ in 0..200 {
            if core.store.command_result("steer").unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let stored = core.store.command_result("steer").unwrap().unwrap();
        assert!(stored.into_result().is_ok());
    }

    /// A fake worker whose policy allows Full access.
    fn full_access_worker(fake: &FakeHarness) -> (tempfile::TempDir, Arc<Core>, AccountId) {
        let (dir, core, account_id) = fake_worker(fake);
        if let Ok(mut config) = core.config.write() {
            config.policy.max_approval_mode = consort_proto::model::ApprovalMode::FullAccess;
        }
        (dir, core, account_id)
    }

    fn all_but_full_access() -> consort_proto::model::Capabilities {
        use consort_proto::model::ApprovalMode;
        consort_proto::model::Capabilities {
            steer: true,
            resume: true,
            mcp_http: true,
            approval_modes: vec![
                ApprovalMode::Ask,
                ApprovalMode::AutoEdits,
                ApprovalMode::Auto,
            ],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn the_effective_mode_follows_the_probed_capabilities() {
        use consort_proto::model::ApprovalMode;
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        let record = fake.record();
        let (_dir, core, account_id) = full_access_worker(&fake);
        // The probe found that Full access does not work here.
        lock(&core.statuses).insert(
            account_id,
            AccountStatus {
                auth_state: AuthState::SignedIn,
                capabilities: all_but_full_access(),
                ..AccountStatus::default()
            },
        );
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        start_with_turn(&core, account_id, ApprovalMode::FullAccess).await;
        assert_eq!(
            record.lock().unwrap().opened[0].approval_mode,
            ApprovalMode::Auto
        );
        let account = core
            .inventory()
            .accounts
            .into_iter()
            .find(|a| a.account_id == account_id)
            .unwrap();
        assert!(
            !account
                .capabilities
                .approval_modes
                .contains(&ApprovalMode::FullAccess)
        );
    }

    #[tokio::test]
    async fn a_session_in_a_lower_mode_is_reported_and_narrows_the_account() {
        use consort_proto::model::ApprovalMode;
        let mut fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        // The adapter had to open the session without Full access.
        fake.session_capabilities = Some(all_but_full_access());
        let (_dir, core, account_id) = full_access_worker(&fake);
        let probed = fake
            .probe(&AccountConfig::default_for(HarnessKind::ClaudeCode))
            .await;
        lock(&core.statuses).insert(account_id, probed);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = start_with_turn(&core, account_id, ApprovalMode::FullAccess).await;
        let events = spooled(&core, session_id);
        let lowered = events.iter().find_map(|e| match e {
            SessionEvent::Error { code, message } if code == "approval_mode_lowered" => {
                Some(message.clone())
            }
            _ => None,
        });
        let lowered = lowered.expect("an approval_mode_lowered error");
        assert!(
            lowered.contains("Full access") && lowered.contains("runs in Auto"),
            "{lowered}"
        );
        assert!(
            !lock(&core.statuses)[&account_id]
                .capabilities
                .approval_modes
                .contains(&ApprovalMode::FullAccess)
        );
    }

    #[tokio::test]
    async fn an_unfinished_start_is_left_for_the_start_that_the_hub_sends_again() {
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        // The worker stopped in the middle of a start: the record is
        // `starting` and has no harness reference.
        let unfinished = SessionId::new();
        let mut stored = crate::store::tests::record(unfinished);
        stored.start.account_id = account_id;
        core.store.put_session(&stored).unwrap();
        // A session whose open finished is resumed.
        let opened = SessionId::new();
        let mut resumable = crate::store::tests::record(opened);
        resumable.start.account_id = account_id;
        resumable.native_ref = Some(consort_proto::ids::NativeRef::new("fake-old"));
        core.store.put_session(&resumable).unwrap();

        assert_eq!(restore_sessions(&core).unwrap(), 1);
        assert!(lock(&core.sessions).contains_key(&opened));
        assert!(!lock(&core.sessions).contains_key(&unfinished));
        assert_eq!(
            core.store.session(unfinished).unwrap().unwrap().state,
            SessionState::Starting,
            "not lost"
        );
        // The hub sends the start again, and it runs.
        let (tx, mut rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let mut again = start(unfinished);
        again.account_id = account_id;
        core.dispatch(request(&FrameId::new("again"), &again.into()));
        wait_until("the start that the hub sent again", || {
            core.store.command_result("again").unwrap().is_some()
        })
        .await;
        let answers = responses(&mut rx);
        let answer = answers.iter().find(|r| r.re.as_str() == "again").unwrap();
        assert!(answer.result.is_ok(), "{:?}", answer.result);
        assert!(
            record
                .lock()
                .unwrap()
                .opened
                .iter()
                .any(|spec| spec.session_id == unfinished && spec.resume.is_none())
        );
    }

    #[test]
    fn the_start_of_the_worker_prunes_old_skills_but_not_those_of_open_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let core = test_core(dir.path(), Harnesses::default(), HostToml::default());
        let cache = core.consort_home.join(crate::skills::CACHE_DIR);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 60 * 60);
        let make = |hash: &str| {
            let folder = cache.join(hash);
            std::fs::create_dir_all(&folder).unwrap();
            let list = folder.join("bundle.json");
            std::fs::write(&list, "{}").unwrap();
            std::fs::OpenOptions::new()
                .write(true)
                .open(list)
                .unwrap()
                .set_modified(old)
                .unwrap();
            folder
        };
        let (open_hash, unused_hash) = ("a".repeat(64), "b".repeat(64));
        let (open, unused) = (make(&open_hash), make(&unused_hash));
        // An open session names the first one.
        let id = SessionId::new();
        let mut record = record(id);
        record.start.extensions.skills = vec![consort_proto::model::SkillBundleRef {
            skill_id: consort_proto::ids::SkillId::new(),
            name: "pdf-forms".into(),
            content_hash: open_hash.parse().unwrap(),
        }];
        core.store.put_session(&record).unwrap();
        // `restore_sessions` would also start the session, so call the prune
        // that it calls.
        core.prune_skill_cache();
        assert!(open.exists(), "an open session names it");
        assert!(!unused.exists(), "40 days old, and no session names it");
    }

    /// The members of a process group that still run.
    #[cfg(unix)]
    fn live_members(pgid: u32) -> usize {
        let out = std::process::Command::new("ps")
            .args(["-A", "-o", "pgid=,stat="])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| {
                let parts: Vec<&str> = l.split_whitespace().collect();
                parts.first() == Some(&pgid.to_string().as_str())
                    && !parts.get(1).is_some_and(|s| s.starts_with('Z'))
            })
            .count()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_next_start_kills_only_the_groups_that_the_worker_recorded() {
        use consort_harness::process::{ProcessSpec, WatcherHandle, resolve_program, spawn};
        let dir = tempfile::tempdir().unwrap();
        let core = test_core(dir.path(), HashMap::new(), HostToml::default());
        let watcher = WatcherHandle(Arc::new(GroupLog {
            core: Arc::downgrade(&core),
        }));
        let spec = ProcessSpec {
            program: resolve_program("sh").unwrap(),
            args: vec!["-c".to_owned(), "sleep 30 & sleep 30".to_owned()],
            watcher: Some(watcher),
            ..ProcessSpec::default()
        };
        // The worker loses the process, as when it is killed.
        let lost = spawn(&spec).unwrap();
        let lost_pgid = lost.id().unwrap();
        std::mem::forget(lost);
        assert_eq!(core.store.processes().unwrap().len(), 1);
        // A group with a start time that does not match is not ours.
        let other = spawn(&ProcessSpec {
            watcher: None,
            ..spec.clone()
        })
        .unwrap();
        let other_pgid = other.id().unwrap();
        core.store
            .put_process(&ProcessRecord {
                pgid: other_pgid,
                program: "sh".into(),
                started: "Mon Jan  1 00:00:00 2001".into(),
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(live_members(lost_pgid) >= 2);

        let core2 = core.clone();
        tokio::task::spawn_blocking(move || kill_stale_groups(&core2.store))
            .await
            .unwrap();
        wait_until("the lost group to die", || live_members(lost_pgid) == 0).await;
        assert!(live_members(other_pgid) >= 1, "the other group still runs");
        assert!(core.store.processes().unwrap().is_empty());
        drop(other);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_next_start_kills_what_a_dead_leader_left_in_its_group() {
        use consort_harness::process::{ProcessSpec, WatcherHandle, resolve_program, spawn};
        let dir = tempfile::tempdir().unwrap();
        let core = test_core(dir.path(), HashMap::new(), HostToml::default());
        let watcher = WatcherHandle(Arc::new(GroupLog {
            core: Arc::downgrade(&core),
        }));
        // The leader (as `npx` is) starts a child in its group and exits.
        let mut leader = spawn(&ProcessSpec {
            program: resolve_program("sh").unwrap(),
            args: vec!["-c".to_owned(), "sleep 30 & exit 0".to_owned()],
            watcher: Some(watcher),
            ..ProcessSpec::default()
        })
        .unwrap();
        let pgid = leader.id().unwrap();
        leader.wait().await;
        // The worker is killed, so it forgets the process without the
        // kill that a drop does.
        std::mem::forget(leader);
        assert_eq!(core.store.processes().unwrap().len(), 1);
        wait_until("the child in the group", || live_members(pgid) == 1).await;

        let core2 = core.clone();
        tokio::task::spawn_blocking(move || kill_stale_groups(&core2.store))
            .await
            .unwrap();
        wait_until("the child to die", || live_members(pgid) == 0).await;
        assert!(core.store.processes().unwrap().is_empty());
    }

    /// Sends one turn with an approval mode of its own and waits for the
    /// result. Returns whether the worker took it.
    async fn turn_in_mode(
        core: &Arc<Core>,
        session_id: SessionId,
        n: u32,
        mode: Option<consort_proto::model::ApprovalMode>,
    ) -> Result<(), consort_proto::error::ErrorBody> {
        let turn: HubToSessionHost = SessionTurn {
            session_id,
            turn: TurnInput {
                text: format!("Turn {n}"),
                attachments: vec![],
            },
            channel_files: None,
            approval_mode: mode,
            model_change: None,
        }
        .into();
        let id = format!("turn-{session_id}-{n}");
        core.dispatch(request(&FrameId::new(&id), &turn));
        wait_until("the turn result", || {
            core.store.command_result(&id).unwrap().is_some()
        })
        .await;
        // The fake turn ends on its own; give it a moment.
        tokio::time::sleep(Duration::from_millis(50)).await;
        match core.store.command_result(&id).unwrap() {
            Some(crate::store::StoredResult::Ok { .. }) => Ok(()),
            Some(crate::store::StoredResult::Err { error }) => Err(error),
            None => unreachable!("waited for the result"),
        }
    }

    #[tokio::test]
    async fn a_turn_with_a_stricter_mode_runs_in_it_and_the_next_turn_goes_back() {
        use consort_proto::model::ApprovalMode;
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        // A coordinator session in Auto whose first turn is proactive.
        let session_id = SessionId::new();
        let mut command = start(session_id);
        command.account_id = account_id;
        command.approval_mode = ApprovalMode::Auto;
        command.turn = Some(TurnInput {
            text: "Check in".into(),
            attachments: vec![],
        });
        command.turn_approval_mode = Some(ApprovalMode::Ask);
        let id = format!("start-{session_id}");
        core.dispatch(request(&FrameId::new(&id), &command.into()));
        wait_until("the start result", || {
            core.store.command_result(&id).unwrap().is_some()
        })
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        // A normal turn goes back to Auto, a proactive one runs in Ask, and
        // a turn cannot raise the mode above the session's.
        turn_in_mode(&core, session_id, 1, None).await.unwrap();
        turn_in_mode(&core, session_id, 2, Some(ApprovalMode::Ask))
            .await
            .unwrap();
        turn_in_mode(&core, session_id, 3, Some(ApprovalMode::FullAccess))
            .await
            .unwrap();
        let record = record.lock().unwrap();
        assert_eq!(record.opened[0].approval_mode, ApprovalMode::Auto);
        assert_eq!(
            record.turn_modes,
            [
                ApprovalMode::Ask,
                ApprovalMode::Auto,
                ApprovalMode::Ask,
                ApprovalMode::Auto
            ]
        );
        assert_eq!(
            record.mode_changes,
            [
                ApprovalMode::Ask,
                ApprovalMode::Auto,
                ApprovalMode::Ask,
                ApprovalMode::Auto
            ]
        );
    }

    /// Sends one turn that carries a model change and waits for the result.
    async fn turn_with_model(
        core: &Arc<Core>,
        session_id: SessionId,
        n: u32,
        model: &str,
    ) -> Result<(), consort_proto::error::ErrorBody> {
        let turn: HubToSessionHost = SessionTurn {
            session_id,
            turn: TurnInput {
                text: format!("Turn {n}"),
                attachments: vec![],
            },
            channel_files: None,
            approval_mode: None,
            model_change: Some(consort_proto::session_host_link::SessionModelChange {
                model: model.to_owned(),
                options: Default::default(),
            }),
        }
        .into();
        let id = format!("turn-{session_id}-{n}");
        core.dispatch(request(&FrameId::new(&id), &turn));
        wait_until("the turn result", || {
            core.store.command_result(&id).unwrap().is_some()
        })
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        match core.store.command_result(&id).unwrap() {
            Some(crate::store::StoredResult::Ok { .. }) => Ok(()),
            Some(crate::store::StoredResult::Err { error }) => Err(error),
            None => unreachable!("waited for the result"),
        }
    }

    #[tokio::test]
    async fn a_turn_with_a_model_change_sets_the_model_first_and_saves_it() {
        use consort_proto::model::ApprovalMode;
        let fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = start_with_turn(&core, account_id, ApprovalMode::Auto).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        turn_with_model(&core, session_id, 1, "opus-next")
            .await
            .unwrap();
        {
            let record = record.lock().unwrap();
            assert_eq!(record.model_changes.len(), 1);
            assert_eq!(record.model_changes[0].0, "opus-next");
            assert_eq!(record.turns.len(), 2, "the turn ran after the change");
        }
        // The stored start has the new model, so a restart resumes with it.
        let stored = core.store.session(session_id).unwrap().unwrap();
        assert_eq!(stored.start.model, "opus-next");
    }

    #[tokio::test]
    async fn a_failed_model_change_refuses_the_turn_as_unsupported() {
        use consort_proto::model::ApprovalMode;
        let mut fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        fake.fail_model_change = true;
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = start_with_turn(&core, account_id, ApprovalMode::Auto).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let before = core.store.session(session_id).unwrap().unwrap().start.model;
        let err = turn_with_model(&core, session_id, 1, "opus-next")
            .await
            .unwrap_err();
        assert_eq!(err.code, consort_proto::error::ErrorCode::Unsupported);
        assert_eq!(
            record.lock().unwrap().turns.len(),
            1,
            "the turn did not run"
        );
        assert_eq!(
            core.store.session(session_id).unwrap().unwrap().start.model,
            before,
            "the saved model did not change"
        );
    }

    #[tokio::test]
    async fn a_harness_that_cannot_switch_its_model_refuses_the_turn_as_unsupported() {
        use consort_proto::model::{ApprovalMode, Capabilities};
        let mut fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        fake.session_capabilities = Some(Capabilities {
            switch_model: false,
            ..Capabilities::default()
        });
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = start_with_turn(&core, account_id, ApprovalMode::Auto).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let err = turn_with_model(&core, session_id, 1, "opus-next")
            .await
            .unwrap_err();
        assert_eq!(err.code, consort_proto::error::ErrorCode::Unsupported);
        assert!(record.lock().unwrap().model_changes.is_empty());
        // A turn without a change still runs.
        turn_in_mode(&core, session_id, 2, None).await.unwrap();
    }

    #[tokio::test]
    async fn a_harness_that_cannot_change_its_mode_refuses_a_stricter_turn() {
        use consort_proto::model::ApprovalMode;
        let mut fake = FakeHarness::new(HarnessKind::ClaudeCode, FakeScript::reply("Hi."));
        fake.fixed_mode = true;
        let record = fake.record();
        let (_dir, core, account_id) = fake_worker(&fake);
        let (tx, _rx) = mpsc::unbounded_channel();
        core.link.attach(tx);
        let session_id = start_with_turn(&core, account_id, ApprovalMode::Auto).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let err = turn_in_mode(&core, session_id, 1, Some(ApprovalMode::Ask))
            .await
            .unwrap_err();
        assert_eq!(err.code, consort_proto::error::ErrorCode::PolicyDenied);
        assert!(
            err.message.contains("cannot change its approval mode"),
            "{}",
            err.message
        );
        assert_eq!(
            record.lock().unwrap().turns.len(),
            1,
            "the turn did not run"
        );
        // A turn that needs no change still runs.
        turn_in_mode(&core, session_id, 2, None).await.unwrap();
        assert_eq!(record.lock().unwrap().turns.len(), 2);
    }
}
