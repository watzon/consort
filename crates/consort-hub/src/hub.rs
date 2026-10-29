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

use crate::coordinator::Coordinators;
use crate::error::Result;
use crate::link::Links;
use crate::net::clients::ClientRegistry;
use crate::net::pairing::PairingLimiter;
use crate::net::ws::HandshakeSlots;
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
            ToolAudience::EverythingCoordinator => MemoryActor::EverythingCoordinator,
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
    pub clients: ClientRegistry,
    pub coordinators: Coordinators,
    pub pairing_limiter: PairingLimiter,
    pub handshakes: HandshakeSlots,
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
    /// Opens the store and creates the Everything channel when it is missing.
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
            clients: ClientRegistry::default(),
            coordinators: Coordinators::default(),
            pairing_limiter: PairingLimiter::default(),
            handshakes: HandshakeSlots::default(),
            shutdown: CancellationToken::new(),
        }));
        hub.bootstrap().await?;
        Ok(hub)
    }

    async fn bootstrap(&self) -> Result<()> {
        self.store
            .write(|tx| {
                if conversation::everything_channel(tx.db()).is_err() {
                    let now = tx.now();
                    tx.save_channel(&Channel {
                        id: ChannelId::new(),
                        kind: ChannelKind::Everything,
                        name: "Everything".to_owned(),
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
                        default_approval_mode: ApprovalMode::Ask,
                        memory_enabled: true,
                        archived_at: None,
                        created_at: now,
                        updated_at: now,
                    })?;
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
    (hub, dir)
}
