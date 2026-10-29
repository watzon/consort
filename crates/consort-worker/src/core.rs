//! The worker core: shared state, hub command dispatch with dedup, the
//! spool sender and the inventory.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
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

use crate::config::{WORKSPACES_DIR, WorkerToml};
use crate::identity::Identity;
use crate::link::{Link, LinkHandler};
use crate::sessions::{Job, SessionHandle, run_session};
use crate::store::{Store, StoredResult};
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

    /// Stores a command result and sends it to the hub.
    pub fn finish_command(&self, id: &FrameId, result: Result<Value, ErrorBody>) {
        if let Err(err) = self
            .store
            .store_command(id.as_str(), &StoredResult::from(&result))
        {
            tracing::error!(error = %err, "could not store a command result");
        }
        self.respond(id, result);
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
    /// its stored result.
    pub fn dispatch(self: &Arc<Self>, request: RequestFrame) {
        let id = request.id.clone();
        match self.store.command_result(id.as_str()) {
            Ok(Some(stored)) => {
                self.respond(&id, stored.into_result());
                return;
            }
            Ok(None) => {}
            Err(err) => tracing::error!(error = %err, "could not read the processed commands"),
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
                let _ = jobs.send(Job::Command {
                    id,
                    command: Box::new(HubToWorker::SessionStart(start)),
                });
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
                    Some(jobs) => {
                        let _ = jobs.send(Job::Command {
                            id,
                            command: Box::new(other),
                        });
                    }
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

    /// Sends every spooled event, one batch at a time.
    pub async fn flush_spool(&self) {
        let Ok(sessions) = self.store.spooled_sessions() else {
            return;
        };
        for session_id in sessions {
            loop {
                let batch = match self.store.next_batch(session_id) {
                    Ok(b) if !b.is_empty() => b,
                    _ => break,
                };
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
                        let _ = self.store.ack(session_id, ack.last_seq);
                        if ack.last_seq < last {
                            tracing::warn!(session = %session_id, hub = ack.last_seq, worker = last, "the hub stored fewer events than sent");
                            break;
                        }
                    }
                    Err(err) => {
                        tracing::debug!(error = %err, "could not send spooled events");
                        return;
                    }
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
