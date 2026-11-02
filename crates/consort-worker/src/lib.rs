//! The Consort execution worker. A worker is a machine where work runs. It
//! holds no account and it runs no harness.
//!
//! A worker pairs with a hub once ([`pair`]), then runs ([`run`]): it keeps
//! one WebSocket to the hub, makes workspaces with `git`, and answers the
//! file, process and local MCP requests of the execution protocol
//! (`consort_proto::execution_link`). A journal in SQLite runs each change
//! once, and keeps the record of each process across a restart.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod attachments;
pub mod config;
mod core;
mod error;
pub mod exec;
pub mod journal;
pub mod limits;
pub mod local_mcp;
pub mod paths;
pub mod process;
pub mod self_update;
pub mod tags;
pub mod tools;
pub mod workspace;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use consort_link::http;
use consort_link::identity::Identity;
use consort_link::link::Link;
use consort_proto::auth::{PAIR_WORKER_PATH, PairWorkerRequest, PairingCode};
use consort_proto::ids::WorkerId;
use consort_proto::model::Os;
use tokio::sync::watch;

pub use crate::error::WorkerError;

use crate::attachments::BlobCache;
use crate::config::{
    CACHE_DIR, DB_FILE, KEY_FILE, LOCK_FILE, PROCESSES_DIR, WorkerToml, create_private_dir,
};
use crate::core::{Core, CoreHub};
use crate::exec::Exec;
use crate::journal::Journal;
use crate::limits::Limits;
use crate::paths::PathRules;
use crate::process::Processes;
use crate::self_update::{SelfUpdate, Updater};

/// The folder of a download of a new worker program, in the worker folder.
const UPDATE_DIR: &str = "cache/update";

/// Takes the exclusive lock on the lock file of a config folder. The lock
/// lasts as long as the returned file is open.
pub fn lock_config_dir(dir: &Path) -> Result<std::fs::File, WorkerError> {
    let path = dir.join(LOCK_FILE);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| WorkerError::io(format!("could not open {}", path.display()), e))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err(WorkerError::AlreadyRunning(dir.display().to_string()))
        }
        Err(std::fs::TryLockError::Error(err)) => Err(WorkerError::io(
            format!("could not lock {}", path.display()),
            err,
        )),
    }
}

/// How the worker runs.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// The folder of the worker.
    pub config_dir: PathBuf,
    /// How the worker replaces its program when it follows the hub. `None`
    /// keeps the program, and the worker reports that it cannot update.
    pub self_update: Option<SelfUpdate>,
}

/// Pairs this machine as a worker. Returns its ID.
pub async fn pair(
    hub_url: &str,
    code: &str,
    config_dir: &Path,
    name: Option<String>,
    tags: Vec<String>,
) -> Result<WorkerId, WorkerError> {
    pair_pinned(hub_url, None, code, config_dir, name, tags).await
}

/// [`pair`] for a hub on This network: the worker keeps the pin, and from then
/// on it accepts only that certificate for this hub.
pub async fn pair_pinned(
    hub_url: &str,
    tls_sha256: Option<consort_proto::model::Sha256Hash>,
    code: &str,
    config_dir: &Path,
    name: Option<String>,
    tags: Vec<String>,
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
        protocol_versions: consort_proto::frame::SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
    };
    let hub = http::HubRef {
        url: hub_url,
        tls_sha256: tls_sha256.as_ref(),
    };
    let worker_id = http::pair_machine(hub, PAIR_WORKER_PATH, &request).await?;
    let mut toml = WorkerToml::load(config_dir)?;
    toml.hub_url = Some(http::base_url(hub_url));
    toml.tls_sha256 = tls_sha256;
    toml.worker_id = Some(worker_id);
    if name.is_some() {
        toml.name = name;
    }
    for tag in tags {
        if !toml.tags.contains(&tag) {
            toml.tags.push(tag);
        }
    }
    toml.save(config_dir)?;
    Ok(worker_id)
}

/// A running worker.
pub struct WorkerHandle {
    worker_id: WorkerId,
    core: Arc<Core>,
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    lock: std::fs::File,
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

    /// Waits until the worker has a new program and no work runs. The caller
    /// then shuts down this worker and starts the program again.
    pub async fn restart_requested(&self) {
        let mut restart = self.core.updater.restart_requested();
        if restart.wait_for(|asked| *asked).await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    /// Stops the worker. The processes go on, and the next run finds them.
    pub async fn shutdown(self) -> Result<(), WorkerError> {
        let _ = self.shutdown.send(true);
        let mut task = self.task;
        if tokio::time::timeout(Duration::from_secs(5), &mut task)
            .await
            .is_err()
        {
            task.abort();
        }
        *self.core.exec.lock().unwrap_or_else(|e| e.into_inner()) = None;
        drop(self.lock);
        Ok(())
    }
}

/// Runs the worker until [`WorkerHandle::shutdown`].
pub async fn run(config: WorkerConfig) -> Result<WorkerHandle, WorkerError> {
    let dir = config.config_dir.clone();
    let toml = WorkerToml::load(&dir)?;
    let (Some(hub_url), Some(worker_id)) = (toml.hub_url.clone(), toml.worker_id) else {
        return Err(WorkerError::NotPaired);
    };
    if let Some(warning) = http::cleartext_warning(&hub_url) {
        tracing::warn!("{warning}");
    }
    create_private_dir(&dir)
        .map_err(|e| WorkerError::io(format!("could not create {}", dir.display()), e))?;
    let lock = lock_config_dir(&dir)?;
    let identity = Identity::load(&dir.join(KEY_FILE))?;
    let journal = Arc::new(Journal::open(&dir.join(DB_FILE))?);
    let files = journal.prune(consort_proto::time::Timestamp::now())?;
    for file in files {
        let _ = std::fs::remove_file(file);
    }
    let processes = Arc::new(Processes::new(journal.clone(), dir.join(PROCESSES_DIR)));
    processes.adopt().await?;
    let limits = Arc::new(Limits::load(journal.clone())?);
    let link = Link::new();
    let core = Arc::new(Core {
        config_dir: dir.clone(),
        identity,
        worker_id,
        hub_url,
        link: link.clone(),
        journal: journal.clone(),
        exec: Mutex::new(None),
        detected_tags: core::detected_tags(),
        blob_cache: BlobCache::new(
            &dir.join(CACHE_DIR)
                .parent()
                .map_or(dir.clone(), Path::to_path_buf),
        ),
        channel_copies: tokio::sync::Mutex::default(),
        toml: toml.clone(),
        limits: limits.clone(),
        updater: Arc::new(Updater::new(config.self_update, dir.join(UPDATE_DIR))),
        in_flight: AtomicUsize::new(0),
    });
    let exec = Arc::new(Exec::new(
        journal,
        processes,
        limits,
        toml.local_mcp_servers.clone(),
        PathRules::native(),
        Arc::new(CoreHub(Arc::downgrade(&core))),
    ));
    *core.exec.lock().unwrap_or_else(|e| e.into_inner()) = Some(exec);
    let (shutdown, rx) = watch::channel(false);
    let task = tokio::spawn(consort_link::link::maintain(link, core.clone(), rx));
    Ok(WorkerHandle {
        worker_id,
        core,
        shutdown,
        task,
        lock,
    })
}
