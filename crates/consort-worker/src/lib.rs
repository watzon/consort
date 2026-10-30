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

mod accounts;
#[cfg(test)]
mod accounts_tests;
pub mod attachments;
pub mod bridge;
pub mod cap;
pub mod config;
mod core;
mod error;
pub mod http;
pub mod identity;
pub mod item_details;
pub mod link;
pub mod mcp;
pub mod profile;
mod sessions;
mod signin;
pub mod skills;
pub mod store;
pub mod tags;
pub mod tools;
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

use crate::config::{DB_FILE, KEY_FILE, LOCK_FILE, WorkerToml, create_private_dir};
use crate::core::Core;
use crate::identity::Identity;
use crate::link::Link;
use crate::sessions::Job;
use crate::store::Store;

/// Takes the exclusive lock on the worker lock file of a config folder.
/// The lock lasts as long as the returned file is open. Another process
/// that holds it makes this fail with [`WorkerError::AlreadyRunning`].
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
        Err(std::fs::TryLockError::Error(e)) => Err(WorkerError::io(
            format!("could not lock {}", path.display()),
            e,
        )),
    }
}

/// Where the worker keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConfig {
    /// The folder with `worker.toml`, the key, `worker.db` and
    /// `workspaces/`.
    pub config_dir: PathBuf,
    /// The Consort home, with the profile folders and the managed tools.
    /// `None` means [`profile::consort_home`]: `CONSORT_HOME`, else
    /// `~/.consort`.
    pub consort_home: Option<PathBuf>,
    /// Whether the worker checks its managed tools for updates once a day.
    /// Tests turn it off, so they make no network request.
    pub check_tool_updates: bool,
}

impl WorkerConfig {
    /// The settings for a config folder, with the default Consort home and
    /// the daily update check.
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            config_dir,
            consort_home: None,
            check_tool_updates: true,
        }
    }

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
    };
    let worker_id = http::pair_worker(hub_url, &request).await?;
    let mut toml = WorkerToml::load(config_dir)?;
    toml.hub_url = Some(http::base_url(hub_url));
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
    ///
    /// The stopping flag turns on first. A harness that exits from now on,
    /// for example because the same signal reached it, does not make its
    /// session lost.
    pub async fn shutdown(self) -> Result<(), WorkerError> {
        self.core.stopping.send_replace(true);
        self.core.cancel_all_sign_ins();
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
        // Everything has stopped, so the next worker may use the folder.
        let lock = match self.core.lock.lock() {
            Ok(mut l) => l.take(),
            Err(p) => p.into_inner().take(),
        };
        drop(lock);
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
    let toml = WorkerToml::load(&dir)?;
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
    // One worker per folder. A second worker would kill the first one's
    // harness groups below, and both would write the same spool.
    let lock = lock_config_dir(&dir)?;
    if toml.auto_discover.is_some() {
        tracing::info!(
            "worker.toml has `auto_discover`, which this build ignores: a worker no longer adds accounts by itself. Add accounts in Settings > Workers"
        );
    }
    let consort_home = match config.consort_home {
        Some(home) => home,
        None => profile::consort_home().ok_or_else(|| {
            WorkerError::Config(format!(
                "could not find the home folder; set {}",
                profile::HOME_VAR
            ))
        })?,
    };
    create_private_dir(&consort_home)
        .map_err(|e| WorkerError::io(format!("could not create {}", consort_home.display()), e))?;
    let tools = tools::Tools::new(
        consort_home.join(profile::TOOLS_DIR),
        config.check_tool_updates,
    )
    .map_err(|e| WorkerError::Config(format!("could not open the managed tools: {e}")))?;
    let detected_tags = tokio::task::spawn_blocking(tags::detected_tags)
        .await
        .unwrap_or_default();
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
        login_expired: Mutex::default(),
        spool_notify: Notify::new(),
        inventory_notify: Notify::new(),
        last_inventory_hash: Mutex::new(None),
        bridge_program,
        inflight: Mutex::new(Default::default()),
        stopping: watch::channel(false).0,
        lock: Mutex::new(Some(lock)),
        consort_home,
        tools: Arc::new(tools),
        sign_ins: Mutex::new(Default::default()),
        browser_sign_in: tokio::sync::Mutex::new(()),
        detected_tags,
    });

    // Resume the sessions that were open when the worker stopped.
    crate::core::restore_sessions(&core)?;

    let (shutdown, shutdown_rx) = watch::channel(false);
    let tasks = vec![
        tokio::spawn(link::maintain(
            core.link.clone(),
            core.clone(),
            shutdown_rx.clone(),
        )),
        tokio::spawn(crate::core::spool_loop(core.clone(), shutdown_rx.clone())),
        tokio::spawn(crate::core::inventory_loop(
            core.clone(),
            shutdown_rx.clone(),
        )),
        tokio::spawn(crate::accounts::tools_loop(core.clone(), shutdown_rx)),
    ];
    Ok(WorkerHandle {
        worker_id,
        core,
        shutdown,
        tasks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_worker_per_config_folder() {
        let dir = tempfile::tempdir().unwrap();
        let first = lock_config_dir(dir.path()).unwrap();
        let second = lock_config_dir(dir.path());
        assert!(
            matches!(&second, Err(WorkerError::AlreadyRunning(_))),
            "{second:?}"
        );
        assert!(second.unwrap_err().to_string().contains("--config-dir"));
        // The lock ends with the worker. A process that another test forks
        // holds the open file until its `exec` closes it (the file is
        // close-on-exec), so try a few times.
        drop(first);
        let again = (0..50).find_map(|_| {
            lock_config_dir(dir.path()).ok().or_else(|| {
                std::thread::sleep(std::time::Duration::from_millis(20));
                None
            })
        });
        assert!(again.is_some());
    }
}
