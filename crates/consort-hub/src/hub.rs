//! The shared state of a running hub.
//!
//! [`Hub`] is a cheap clone (an `Arc`). Client API handlers, hub tools, the
//! worker link and the scheduler are methods on it, spread over the `ops`,
//! `api`, `tools`, `link` and `coordinator` modules.

use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use consort_proto::ids::{ChannelId, ClientId};
use consort_proto::model::{
    ApprovalMode, Channel, ChannelKind, Isolation, MemoryActor, Session, Task,
};
use consort_proto::time::Timestamp;
use consort_proto::tools::ToolAudience;
use tokio_util::sync::CancellationToken;

use crate::connectors::Connectors;
use crate::coordinator::Coordinators;
use crate::error::Result;
use crate::link::Links;
use crate::net::clients::ClientRegistry;
use crate::net::pairing::PairingLimiter;
use crate::net::ws::HandshakeSlots;
use crate::ops::machines::RevokeWatch;
use crate::ops::registry::RegistryCache;
use crate::store::{Store, conversation};

pub const HUB_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Who makes a client API request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    /// A paired client.
    Client(ClientId),
    /// The hub itself. It acts for no client, so it has no idempotency key and
    /// no push registration. Tests use it. A future internal caller can too.
    #[cfg_attr(not(test), allow(dead_code))]
    Hub,
}

impl Caller {
    pub fn client_id(self) -> Option<ClientId> {
        match self {
            Self::Client(id) => Some(id),
            Self::Hub => None,
        }
    }
}

/// What a hub tool call knows about its session.
#[derive(Debug, Clone)]
pub struct ToolCtx {
    pub session: Session,
    pub channel: Channel,
    pub task: Option<Task>,
    pub audience: ToolAudience,
}

impl ToolCtx {
    /// The memory rights of the session.
    pub fn memory_actor(&self) -> MemoryActor {
        match self.audience {
            ToolAudience::TuttiCoordinator => MemoryActor::TuttiCoordinator,
            ToolAudience::ChannelCoordinator => MemoryActor::ChannelCoordinator(self.channel.id),
            ToolAudience::TaskSession => MemoryActor::TaskSession(self.channel.memory_scope()),
        }
    }
}

/// The settings that the hub resolves at start.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The URL that peers use to reach the hub.
    pub public_url: String,
    /// `scheme://host[:port]` of `public_url`. Peers sign it.
    pub origin: String,
}

/// The first-start setup code, until a client pairs.
#[derive(Default)]
pub struct SetupState {
    code: Mutex<Option<(String, Timestamp)>>,
    paired: AtomicBool,
}

impl SetupState {
    pub(crate) fn set(&self, code: String, expires_at: Timestamp) {
        if let Ok(mut slot) = self.code.lock() {
            *slot = Some((code, expires_at));
        }
    }

    pub(crate) fn mark_paired(&self) {
        self.paired.store(true, Ordering::Relaxed);
    }

    /// The code, while it is valid and no client has paired.
    pub fn current(&self) -> Option<String> {
        if self.paired.load(Ordering::Relaxed) {
            return None;
        }
        let slot = self.code.lock().ok()?;
        slot.as_ref()
            .filter(|(_, expires)| *expires > Timestamp::now())
            .map(|(code, _)| code.clone())
    }
}

pub struct HubInner {
    pub store: Store,
    pub setup: SetupState,
    pub settings: Settings,
    pub links: Links,
    pub command_checks: crate::command_check::CheckCounters,
    pub clients: ClientRegistry,
    pub coordinators: Coordinators,
    pub pairing_limiter: PairingLimiter,
    pub handshakes: HandshakeSlots,
    pub revokes: RevokeWatch,
    pub registry: RegistryCache,
    pub connectors: Connectors,
    pub skill_sources: crate::skills::SkillSources,
    pub backups: crate::backup::Backups,
    pub voice: crate::voice::VoiceState,
    pub shutdown: CancellationToken,
}

#[derive(Clone)]
pub struct Hub(Arc<HubInner>);

impl Deref for Hub {
    type Target = HubInner;

    fn deref(&self) -> &HubInner {
        &self.0
    }
}

impl Hub {
    /// Opens the store and creates the Tutti channel when it is missing.
    /// It does not listen on a socket.
    pub async fn open(data_dir: &Path, public_url: String) -> Result<Self> {
        let store = Store::open(data_dir)?;
        let origin = consort_proto::auth::normalize_origin(&public_url)
            .map_err(|e| crate::error::HubError::Startup(e.to_string()))?;
        let hub = Self(Arc::new(HubInner {
            store,
            settings: Settings { public_url, origin },
            setup: SetupState::default(),
            links: Links::default(),
            command_checks: crate::command_check::CheckCounters::default(),
            clients: ClientRegistry::default(),
            coordinators: Coordinators::default(),
            pairing_limiter: PairingLimiter::default(),
            handshakes: HandshakeSlots::default(),
            revokes: RevokeWatch::default(),
            registry: RegistryCache::default(),
            connectors: Connectors::open(data_dir),
            skill_sources: crate::skills::SkillSources::open(data_dir),
            backups: crate::backup::Backups::open(data_dir),
            voice: crate::voice::VoiceState::default(),
            shutdown: CancellationToken::new(),
        }));
        hub.bootstrap().await?;
        Ok(hub)
    }

    async fn bootstrap(&self) -> Result<()> {
        self.store
            .write(|tx| {
                if conversation::tutti_channel(tx.db()).is_err() {
                    let now = tx.now();
                    tx.save_channel(&Channel {
                        id: ChannelId::new(),
                        kind: ChannelKind::Tutti,
                        name: "Tutti".to_owned(),
                        icon: None,
                        color: None,
                        group_id: None,
                        position: 0,
                        purpose:
                            "The main channel. The assistant can read every other channel here."
                                .to_owned(),
                        instructions: String::new(),
                        coordinator_selection: None,
                        task_selection: None,
                        preferred_workers: Vec::new(),
                        repo_ids: Vec::new(),
                        default_isolation: Isolation::Worktree,
                        default_approval_mode: ApprovalMode::Auto,
                        allowed_local_mcp_servers: Vec::new(),
                        memory_enabled: true,
                        tags: Vec::new(),
                        disabled_connections: Vec::new(),
                        disabled_skills: Vec::new(),
                        archived_at: None,
                        created_at: now,
                        updated_at: now,
                        proactive_tasks: Default::default(),
                        coordinator_fallback: None,
                        coordinator_blocked: None,
                        monid: Default::default(),
                        access_grants: Vec::new(),
                        proactivity: None,
                    })?;
                }
                // The state "the assistant cannot answer" is live. A stored
                // one is from before the restart, so it goes away: the
                // session host and the workers have not connected yet.
                for channel in conversation::channels(tx.db())? {
                    if channel.coordinator_blocked.is_some() {
                        tx.set_coordinator_blocked(channel.id, None)?;
                    }
                }
                // No worker is connected yet.
                tx.mark_all_workers_offline()
            })
            .await
    }

    pub fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// A hub in a temp dir, for unit tests.
#[cfg(test)]
pub(crate) async fn test_hub() -> (Hub, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let hub = Hub::open(dir.path(), "http://127.0.0.1:7600".to_owned())
        .await
        .unwrap();
    // A test that needs the coordinator calls `test_hub_with_coordinators`.
    hub.coordinators.set_disabled(true);
    // Nothing starts on its own, so a new channel queues no greeting. The
    // tests of proactivity end the pause themselves.
    hub.store
        .write(|tx| {
            let mut settings = crate::store::proactivity::settings(tx.db())?;
            settings.paused = true;
            tx.save_proactivity_settings(&settings)
        })
        .await
        .unwrap();
    (hub, dir)
}

/// A worker, a task in Working and its Active task session with an open
/// turn, stored in a test hub. Nothing is queued and no link is registered.
#[cfg(test)]
pub(crate) async fn test_task_session(
    hub: &Hub,
) -> (consort_proto::model::Task, consort_proto::model::Session) {
    use consort_proto::ids::{AccountId, SessionId, WorkerId};
    use consort_proto::model::{
        ModelOptions, Os, PlacementRequest, Session, SessionRole, SessionState, TaskCreator, Usage,
        Worker, WorkerState,
    };
    use consort_proto::status::TaskStatus;

    let channel = hub.store.read(conversation::tutti_channel).await.unwrap();
    let task = hub
        .create_task(crate::ops::hooks::NewTask {
            channel_id: channel.id,
            title: "Work".to_owned(),
            brief: "Do the work.".to_owned(),
            model_selection: None,
            placement: PlacementRequest::default(),
            approval_mode: None,
            start_at: Some(Timestamp::now().plus_millis(86_400_000)),
            created_by: TaskCreator::User,
            routine_id: None,
            attachments: Vec::new(),
            proactive: None,
            user_request: String::new(),
            voice_task: false,
        })
        .await
        .unwrap();
    let worker = Worker {
        kind: Default::default(),
        id: WorkerId::new(),
        name: "Laptop".to_owned(),
        hostname: "laptop".to_owned(),
        os: Os::Macos,
        arch: "aarch64".to_owned(),
        public_key: consort_proto::auth::PublicKey::from_bytes([1; 32]),
        version: String::new(),
        protocol_version: 1,
        tags: Vec::new(),
        paused: false,
        state: WorkerState::Online,
        last_seen_at: None,
        inventory: None,
        paired_at: Timestamp::now(),
        revoked_at: None,
    };
    let session = Session {
        execution_worker_id: None,
        id: SessionId::new(),
        role: SessionRole::Task,
        channel_id: channel.id,
        task_id: Some(task.id),
        worker_id: worker.id,
        account_id: AccountId::new(),
        harness: "fake".to_owned(),
        model: "fake-1".to_owned(),
        options: ModelOptions::new(),
        workspace_path: "/tmp/workspace".to_owned(),
        native_ref: None,
        state: SessionState::Active,
        last_seq: 0,
        usage: Usage::default(),
        started_at: Timestamp::now(),
        ended_at: None,
        end_reason: None,
    };
    let (task_id, stored) = (task.id, session.clone());
    let task = hub
        .store
        .write(move |tx| {
            tx.save_worker(&worker)?;
            tx.save_session(&stored)?;
            tx.save_session_runtime(
                stored.id,
                &crate::store::work::SessionRuntime {
                    turn_open: true,
                    active_at: Some(tx.now()),
                    ..Default::default()
                },
            )?;
            let mut task = crate::store::work::task_or_404(tx.db(), task_id)?;
            task.status = TaskStatus::Working;
            task.start_at = None;
            task.current_session_id = Some(stored.id);
            task.session_ended = false;
            task.worker_id = Some(stored.worker_id);
            tx.save_task(&task)?;
            Ok(task)
        })
        .await
        .unwrap();
    (task, session)
}
