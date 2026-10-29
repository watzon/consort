//! The worker core: shared state, hub command dispatch with dedup, the
//! spool sender and the inventory.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use consort_harness::{AccountStatus, Harness, HarnessKind};
use consort_proto::auth::{OpenSession, Signature, WorkerHello, WorkerHelloResult};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{Frame, FrameId, RequestFrame, SUPPORTED_PROTOCOL_VERSIONS};
use consort_proto::ids::{AccountId, SessionId, WorkerId};
use consort_proto::model::{
    AuthState, HarnessAccount, Inventory, LocalMcpServerInfo, Os, WorkerLoad,
};
use consort_proto::time::Timestamp;
use consort_proto::worker_link::{HubToWorker, SessionEvent, SessionEvents};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, watch};

use crate::WorkerError;
use crate::config::{WORKSPACES_DIR, WorkerToml, local_server_name_problem};
use crate::identity::Identity;
use crate::link::{Link, LinkHandler};
use crate::sessions::{Job, SessionHandle, run_session};
use crate::store::{ProcessRecord, Store, StoredResult};
use crate::workspace;

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
    pub config_dir: PathBuf,
    pub config: RwLock<WorkerToml>,
    pub identity: Identity,
    pub worker_id: WorkerId,
    pub hub_url: String,
    pub store: Store,
    pub link: Link,
    pub harnesses: Harnesses,
    pub sessions: Mutex<HashMap<SessionId, SessionHandle>>,
    pub statuses: Mutex<HashMap<AccountId, AccountStatus>>,
    pub spool_notify: Notify,
    pub inventory_notify: Notify,
    pub last_inventory_hash: Mutex<Option<String>>,
    pub bridge_program: Option<PathBuf>,
    /// The command IDs that run now. A command that arrives again while it
    /// runs gets the result of the first run, when that run ends.
    pub inflight: Mutex<HashSet<String>>,
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
/// A group is killed only when its leader still has the recorded start
/// time, so a process that got the same ID later is never touched. This
/// blocks, so call it from a blocking task.
#[cfg(unix)]
pub(crate) fn kill_stale_groups(store: &Store) {
    let records = match store.processes() {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(error = %err, "could not read the recorded harness processes");
            return;
        }
    };
    for record in records {
        let now = consort_harness::process::group_start_time(record.pgid);
        if now.as_deref() == Some(record.started.as_str()) {
            if consort_harness::process::kill_group(record.pgid) {
                tracing::warn!(
                    pgid = record.pgid,
                    program = %record.program,
                    "killed a harness process group that the last run of the worker left behind"
                );
            }
        } else {
            tracing::debug!(
                pgid = record.pgid,
                "a recorded harness process group is gone"
            );
        }
        let _ = store.delete_process(record.pgid);
    }
}

impl Core {
    pub fn config_snapshot(&self) -> WorkerToml {
        match self.config.read() {
            Ok(c) => c.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn workspaces_dir(&self) -> PathBuf {
        self.config_dir.join(WORKSPACES_DIR)
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
    pub fn abandon_command(&self, id: &FrameId, error: ErrorBody) {
        lock(&self.inflight).remove(id.as_str());
        self.respond(id, Err(error));
    }

    /// Gives a command to a session actor. An actor that has ended gets no
    /// more work, so the command gets an answer here.
    fn send_job(&self, jobs: &mpsc::UnboundedSender<Job>, id: FrameId, command: HubToWorker) {
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
    /// that session's actor. A command ID that ran in the last 24 hours gets
    /// its stored result. A command ID that runs now does not run again: the
    /// worker answers it when the first run ends.
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
        let command = match request.decode::<HubToWorker>() {
            Ok(c) => c,
            Err(err) => {
                self.finish_command(&id, Err(err.to_error_body()));
                return;
            }
        };
        match command {
            HubToWorker::SessionStart(start) => {
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
                self.send_job(&jobs, id, HubToWorker::SessionStart(start));
            }
            HubToWorker::WorkspaceRelease(release) => {
                let core = self.clone();
                tokio::spawn(async move {
                    let key = release.task_id.to_string();
                    let result = match core.store.workspace(&key) {
                        Ok(Some(record)) => match workspace::release(&record, release.delete).await
                        {
                            Ok(()) => {
                                if release.delete {
                                    let _ = core.store.delete_workspace(&key);
                                }
                                Ok(json!({}))
                            }
                            Err(err) => Err(err.to_error_body()),
                        },
                        Ok(None) => Ok(json!({})),
                        Err(err) => Err(err.to_error_body()),
                    };
                    core.finish_command(&id, result);
                });
            }
            HubToWorker::InventoryRefresh(_) => {
                let core = self.clone();
                tokio::spawn(async move {
                    core.probe_accounts().await;
                    core.send_inventory(true);
                    core.finish_command(&id, Ok(json!({})));
                });
            }
            HubToWorker::ItemDetail(_) => {
                self.finish_command(
                    &id,
                    Err(ErrorBody::new(
                        ErrorCode::NotFound,
                        "this worker build does not keep the full output of items",
                    )),
                );
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
                    None if matches!(other, HubToWorker::SessionStop(_)) => {
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
                Err(WorkerError::Hub {
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
                Err(WorkerError::TooLarge(why)) if !replaced => {
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
                Err(WorkerError::Offline) => return Flush::Offline,
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

    pub async fn probe_accounts(&self) {
        let config = self.config_snapshot();
        let probes = config
            .accounts
            .iter()
            .filter(|a| a.enabled)
            .map(|account| async move {
                let status = match self.harnesses.get(&account.harness) {
                    Some(adapter) => adapter.probe(account).await,
                    None => AccountStatus::unknown(
                        format!(
                            "This build has no adapter for {}.",
                            account.harness.display_name()
                        ),
                        Default::default(),
                    ),
                };
                (account.id, status)
            });
        let results = futures::future::join_all(probes).await;
        let mut statuses = lock(&self.statuses);
        for (id, status) in results {
            statuses.insert(id, status);
        }
    }

    /// Marks an account as signed out, because one of its sessions reported
    /// it. `detail` is the text for the user, with the step to sign in. The
    /// models stay as the last probe found them. The hub does not place work
    /// on an account that is not signed in. The next probe can set the state
    /// back. Sends the new inventory to the hub at once.
    pub fn mark_signed_out(&self, account_id: AccountId, detail: &str) {
        lock(&self.statuses)
            .entry(account_id)
            .and_modify(|status| {
                status.auth_state = AuthState::SignedOut;
                status.auth_detail = Some(detail.to_owned());
            })
            .or_insert_with(|| AccountStatus {
                harness_version: None,
                auth_state: AuthState::SignedOut,
                auth_detail: Some(detail.to_owned()),
                models: Vec::new(),
                capabilities: Default::default(),
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
                    .map(|h| h.capabilities(account))
                    .unwrap_or_default();
                let status = statuses.get(&account.id);
                let enabled = account.enabled && self.harnesses.contains_key(&account.harness);
                HarnessAccount {
                    account_id: account.id,
                    harness: account.harness.as_str().to_owned(),
                    label: account.label.clone(),
                    harness_version: status.and_then(|s| s.harness_version.clone()),
                    auth_state: status.map(|s| s.auth_state).unwrap_or(AuthState::Unknown),
                    auth_detail: status.and_then(|s| s.auth_detail.clone()).or_else(|| {
                        (!self.harnesses.contains_key(&account.harness)).then(|| {
                            format!(
                                "This build has no adapter for {}.",
                                account.harness.display_name()
                            )
                        })
                    }),
                    models: status.map(|s| s.models.clone()).unwrap_or_default(),
                    capabilities: status
                        .map(|s| s.capabilities.clone())
                        .filter(|c| !c.approval_modes.is_empty())
                        .unwrap_or(capabilities),
                    enabled,
                }
            })
            .collect();
        let active = u32::try_from(lock(&self.sessions).len()).unwrap_or(u32::MAX);
        Inventory {
            hash: String::new(),
            hostname: gethostname::gethostname().to_string_lossy().into_owned(),
            os: Os::from_std_os(std::env::consts::OS).unwrap_or(Os::Linux),
            arch: std::env::consts::ARCH.to_owned(),
            labels: config.labels.clone(),
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
pub(crate) async fn inventory_loop(core: Arc<Core>, mut shutdown: watch::Receiver<bool>) {
    core.probe_accounts().await;
    core.send_inventory(true);
    let mut last_probe = tokio::time::Instant::now();
    loop {
        tokio::select! {
            () = core.inventory_notify.notified() => core.send_inventory(false),
            () = tokio::time::sleep(Duration::from_secs(60)) => {
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
mod tests {
    use super::*;
    use crate::bridge::McpHttpClient;
    use crate::store::tests::{record, start};
    use consort_harness::{AccountConfig, HarnessKind};
    use consort_proto::frame::ResponseFrame;
    use consort_proto::model::SessionState;
    use consort_proto::worker_link::{SessionTurn, TurnInput};
    use consort_testkit::{FakeHarness, FakeScript, FakeStep};

    fn test_core(dir: &std::path::Path, harnesses: Harnesses, config: WorkerToml) -> Arc<Core> {
        Arc::new(Core {
            config_dir: dir.to_path_buf(),
            config: RwLock::new(config),
            identity: Identity::generate(),
            worker_id: WorkerId::new(),
            hub_url: "http://127.0.0.1:1".into(),
            store: Store::in_memory().unwrap(),
            link: Link::new(),
            harnesses,
            sessions: Mutex::new(HashMap::new()),
            statuses: Mutex::new(HashMap::new()),
            spool_notify: Notify::new(),
            inventory_notify: Notify::new(),
            last_inventory_hash: Mutex::new(None),
            bridge_program: None,
            inflight: Mutex::new(HashSet::new()),
        })
    }

    /// A worker with one fake Claude Code account.
    fn fake_worker(fake: &FakeHarness) -> (tempfile::TempDir, Arc<Core>, AccountId) {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountConfig::default_for(HarnessKind::ClaudeCode);
        let account_id = account.id;
        let config = WorkerToml {
            accounts: vec![account],
            ..WorkerToml::default()
        };
        let mut harnesses: Harnesses = HashMap::new();
        harnesses.insert(
            HarnessKind::ClaudeCode,
            Arc::new(fake.clone()) as Arc<dyn Harness>,
        );
        let core = test_core(dir.path(), harnesses, config);
        (dir, core, account_id)
    }

    fn request(id: &FrameId, command: &HubToWorker) -> RequestFrame {
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
        let core = test_core(dir.path(), HashMap::new(), WorkerToml::default());
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
        let core = test_core(dir.path(), HashMap::new(), WorkerToml::default());
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
        let core = test_core(dir.path(), HashMap::new(), WorkerToml::default());
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
        let command: HubToWorker = command.into();
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
        let turn: HubToWorker = SessionTurn {
            session_id,
            turn: TurnInput {
                text: "Again".into(),
                attachments: vec![],
            },
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
        let bad: HubToWorker = start(session_id).into();
        core.dispatch(request(&FrameId::new("c1"), &bad));
        wait_until("the failed start to end its actor", || {
            core.store.command_result("c1").unwrap().is_some() && lock(&core.sessions).is_empty()
        })
        .await;
        assert_eq!(core.inventory().load.active_sessions, 0);
        let mut good = start(session_id);
        good.account_id = account_id;
        let good: HubToWorker = good.into();
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
            turns: vec![vec![FakeStep::Event(SessionEvent::State {
                state: SessionState::Lost,
            })]],
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
        let turn: HubToWorker = SessionTurn {
            session_id,
            turn: TurnInput::default(),
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
        let core = test_core(dir.path(), HashMap::new(), WorkerToml::default());
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
}
