//! The Consort worker daemon.
//!
//! A worker pairs with a hub once ([`pair`]), then runs ([`run`]): it keeps
//! a WebSocket to the hub, runs sessions with the harness adapters from
//! `consort-harness`, spools their events in SQLite until the hub
//! acknowledges them, gives each session a local MCP endpoint for the hub
//! tools, and makes workspaces with `git`. The protocol is in
//! `docs/protocol.md`; the local files are in `docs/data-model.md`
//! ("Worker-local state").

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod bridge;
pub mod cap;
pub mod config;
mod core;
mod error;
pub mod http;
pub mod identity;
pub mod link;
pub mod mcp;
mod sessions;
pub mod store;
pub mod workspace;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use consort_proto::auth::{PairWorkerRequest, PairingCode};
use consort_proto::ids::WorkerId;
use consort_proto::model::Os;
use tokio::sync::{Notify, watch};

pub use crate::core::{Harnesses, default_harnesses};
pub use crate::error::WorkerError;
pub use crate::sessions::Coalescer;

use crate::config::{DB_FILE, KEY_FILE, WorkerToml, create_private_dir, harness_installed};
use crate::core::Core;
use crate::identity::Identity;
use crate::link::Link;
use crate::sessions::Job;
use crate::store::Store;

/// Where the worker keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConfig {
    /// The folder with `worker.toml`, the key, `worker.db` and
    /// `workspaces/`.
    pub config_dir: PathBuf,
}

impl WorkerConfig {
    /// The default folder: the platform's config folder plus `consort/worker`.
    pub fn default_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "consort").map(|d| d.config_dir().join("worker"))
    }
}

/// Pairs this machine with a hub, with a pairing code from a client.
/// Writes the key and `worker.toml`, and returns the new worker ID.
pub async fn pair(
    hub_url: &str,
    code: &str,
    config_dir: &Path,
    name: Option<String>,
    labels: Vec<String>,
) -> Result<WorkerId, WorkerError> {
    let code: PairingCode = code
        .parse()
        .map_err(|e: consort_proto::auth::PairingCodeError| WorkerError::Config(e.to_string()))?;
    create_private_dir(config_dir)
        .map_err(|e| WorkerError::io(format!("could not create {}", config_dir.display()), e))?;
    let identity = Identity::load_or_create(&config_dir.join(KEY_FILE))?;
    let request = PairWorkerRequest {
        code,
        public_key: identity.public_key(),
        hostname: gethostname::gethostname().to_string_lossy().into_owned(),
        os: Os::from_std_os(std::env::consts::OS).unwrap_or(Os::Linux),
        arch: std::env::consts::ARCH.to_owned(),
        name: name.clone(),
    };
    let worker_id = http::pair_worker(hub_url, &request).await?;
    let mut toml = WorkerToml::load(config_dir)?;
    toml.hub_url = Some(http::base_url(hub_url));
    toml.worker_id = Some(worker_id);
    if name.is_some() {
        toml.name = name;
    }
    for label in labels {
        if !toml.labels.contains(&label) {
            toml.labels.push(label);
        }
    }
    toml.discover_accounts(&harness_installed);
    toml.save(config_dir)?;
    Ok(worker_id)
}

/// A running worker.
pub struct WorkerHandle {
    worker_id: WorkerId,
    core: Arc<Core>,
    shutdown: watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for WorkerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerHandle")
            .field("worker_id", &self.worker_id)
            .finish()
    }
}

impl WorkerHandle {
    /// The worker ID.
    pub fn worker_id(&self) -> WorkerId {
        self.worker_id
    }

    /// Whether the worker is connected to the hub.
    pub fn is_connected(&self) -> bool {
        self.core.link.is_connected()
    }

    /// Waits until the worker is connected, or the time runs out.
    pub async fn wait_connected(&self, timeout: Duration) -> bool {
        let mut rx = self.core.link.subscribe();
        tokio::time::timeout(timeout, async {
            while !*rx.borrow_and_update() {
                if rx.changed().await.is_err() {
                    return false;
                }
            }
            true
        })
        .await
        .unwrap_or(false)
    }

    /// Stops the worker. Open sessions stay in `worker.db`, so the next run
    /// resumes them.
    pub async fn shutdown(self) -> Result<(), WorkerError> {
        let _ = self.shutdown.send(true);
        let handles: Vec<_> = {
            let mut sessions = match self.core.sessions.lock() {
                Ok(s) => s,
                Err(p) => p.into_inner(),
            };
            sessions.drain().map(|(_, h)| h).collect()
        };
        for handle in &handles {
            let _ = handle.jobs.send(Job::Shutdown);
        }
        for handle in handles {
            let mut task = handle.task;
            if tokio::time::timeout(Duration::from_secs(10), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
        for task in self.tasks {
            let mut task = task;
            if tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
        Ok(())
    }
}

/// Runs the worker with the adapters in this build. It connects to the
/// hub and reconnects with jittered backoff until [`WorkerHandle::shutdown`].
pub async fn run(config: WorkerConfig) -> Result<WorkerHandle, WorkerError> {
    run_with(config, default_harnesses()).await
}

/// Runs the worker with the given harness adapters. Tests pass a fake.
pub async fn run_with(
    config: WorkerConfig,
    harnesses: Harnesses,
) -> Result<WorkerHandle, WorkerError> {
    let dir = config.config_dir;
    let mut toml = WorkerToml::load(&dir)?;
    let (Some(hub_url), Some(worker_id)) = (toml.hub_url.clone(), toml.worker_id) else {
        return Err(WorkerError::NotPaired);
    };
    if let Some(warning) = http::cleartext_warning(&hub_url) {
        tracing::warn!("{warning}");
    }
    // The folder holds the key, the spool and the workspaces. An older build
    // made it with the default mode.
    create_private_dir(&dir)
        .map_err(|e| WorkerError::io(format!("could not create {}", dir.display()), e))?;
    if toml.discover_accounts(&harness_installed) {
        toml.save(&dir)?;
    }
    let key_path = dir.join(KEY_FILE);
    if !key_path.exists() {
        return Err(WorkerError::NotPaired);
    }
    let identity = Identity::load(&key_path)?;
    let store = Store::open(&dir.join(DB_FILE))?;
    let _ = store.prune_commands(consort_proto::time::Timestamp::now());
    // A worker that was killed can leave harness process groups behind on
    // Unix. Kill the ones that it recorded before sessions resume.
    #[cfg(unix)]
    let store = tokio::task::spawn_blocking(move || {
        crate::core::kill_stale_groups(&store);
        store
    })
    .await
    .map_err(|e| WorkerError::Config(format!("could not check old harness processes: {e}")))?;
    // The stdio bridge is this program's `mcp-bridge` subcommand. Tests and
    // embedders that run from another program set CONSORT_BRIDGE_PROGRAM.
    let bridge_program = std::env::var_os("CONSORT_BRIDGE_PROGRAM")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe().ok().filter(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("consort"))
            })
        });
    let core = Arc::new(Core {
        config_dir: dir.clone(),
        config: RwLock::new(toml),
        identity,
        worker_id,
        hub_url,
        store,
        link: Link::new(),
        harnesses,
        sessions: Mutex::new(Default::default()),
        statuses: Mutex::new(Default::default()),
        spool_notify: Notify::new(),
        inventory_notify: Notify::new(),
        last_inventory_hash: Mutex::new(None),
        bridge_program,
        inflight: Mutex::new(Default::default()),
    });

    // Resume the sessions that were open when the worker stopped.
    for record in core.store.open_sessions()? {
        let jobs = core.spawn_session(record.start.session_id);
        let _ = jobs.send(Job::Restore(Box::new(record)));
    }

    let (shutdown, shutdown_rx) = watch::channel(false);
    let tasks = vec![
        tokio::spawn(link::maintain(
            core.link.clone(),
            core.clone(),
            shutdown_rx.clone(),
        )),
        tokio::spawn(crate::core::spool_loop(core.clone(), shutdown_rx.clone())),
        tokio::spawn(crate::core::inventory_loop(core.clone(), shutdown_rx)),
    ];
    Ok(WorkerHandle {
        worker_id,
        core,
        shutdown,
        tasks,
    })
}
