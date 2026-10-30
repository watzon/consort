//! The Consort hub: the server that stores everything and decides what runs
//! where.
//!
//! The hub never runs a model. It keeps conversations, tasks, memory and
//! routines in SQLite, serves clients and workers over HTTP and WebSocket,
//! and asks workers to run sessions.
//!
//! TLS is not part of the hub yet. Run it behind a reverse proxy or on a
//! tailnet.
//!
//! The public API is [`serve`], [`create_setup_code`], [`HubConfig`],
//! [`HubHandle`], [`HubError`], and the [`admin`] commands that list and
//! remove clients and workers from the hub machine.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod admin;
mod api;
mod backup;
mod connectors;
mod coordinator;
mod error;
mod hub;
mod limits;
mod link;
mod maintenance;
mod net;
mod ops;
mod placement;
mod prompts;
mod schedule;
mod scheduler;
mod skills;
pub mod store;
mod tools;
mod util;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use consort_proto::auth::PAIRING_CODE_TTL_MS;
use consort_proto::time::Timestamp;
use tokio::task::JoinHandle;

pub use error::{HubError, Result};
pub use skills::UpdateReport;

use crate::hub::Hub;
use crate::store::machine::{self, Origin, PairingKind};

/// How to start a hub.
#[derive(Debug, Clone)]
pub struct HubConfig {
    /// The folder with `hub.db` and `blobs/`. The hub creates it.
    pub data_dir: PathBuf,
    /// The address to listen on. Use `127.0.0.1:0` in tests.
    pub listen: SocketAddr,
    /// The URL that clients and workers use to reach the hub, for example
    /// `https://hub.example.com`. Peers sign its origin when they connect, so
    /// it must match what they use. The default is `http://{listen}`.
    pub public_url: Option<String>,
}

/// A running hub.
pub struct HubHandle {
    hub: Hub,
    local_addr: SocketAddr,
    public_url: String,
    server: JoinHandle<std::io::Result<()>>,
    background: Vec<JoinHandle<()>>,
    /// The lock on `hub.lock` in the data folder, held while the hub runs.
    _lock: std::fs::File,
}

/// Makes a single-use client setup code that is valid for 10 minutes.
///
/// It opens the database directly, so it works while a hub runs. It creates
/// `hub.db` when the data folder is new.
pub fn create_setup_code(data_dir: &Path) -> Result<String> {
    let conn = store::open_and_migrate(data_dir)?;
    let code = util::random_pairing_code();
    machine::insert_pairing_code(
        &conn,
        PairingKind::Client,
        &code,
        Timestamp::now(),
        PAIRING_CODE_TTL_MS,
        Origin::HubMachine,
    )?;
    Ok(code.to_string())
}

/// Starts a hub. It returns when the hub listens.
///
/// While no client is paired, the hub makes a setup code and logs it at info
/// level. [`HubHandle::setup_code`] returns it too.
pub async fn serve(config: HubConfig) -> Result<HubHandle> {
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|err| {
            HubError::Startup(format!(
                "could not listen on {}: {err}. Choose another address.",
                config.listen
            ))
        })?;
    let local_addr = listener.local_addr()?;
    let public_url = match config.public_url {
        Some(url) => url.trim_end_matches('/').to_owned(),
        None => {
            // An unspecified address is not a place a peer can connect to.
            let host = if local_addr.ip().is_unspecified() {
                tracing::warn!(
                    "The hub listens on all interfaces and has no public URL. Set a public URL so that peers sign the right origin."
                );
                SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), local_addr.port())
            } else {
                local_addr
            };
            format!("http://{host}")
        }
    };
    // One hub at a time for a data folder. `consort hub restore` reads the
    // same lock.
    store::create_private_dir(&config.data_dir)?;
    let lock = backup::try_lock(&config.data_dir)?.ok_or_else(|| {
        HubError::Startup(format!(
            "another hub uses the data folder {}. Stop it first, or choose another folder.",
            config.data_dir.display()
        ))
    })?;
    let hub = Hub::open(&config.data_dir, public_url.clone()).await?;

    let has_client = hub.store.read(machine::active_client_count).await? > 0;
    if has_client {
        hub.setup.mark_paired();
    } else {
        let (code, expires_at) = hub
            .new_pairing_code(PairingKind::Client, Origin::HubMachine)
            .await?;
        tracing::info!(
            "Setup code: {code}. Enter it in the app to pair the first client. It works once and expires in 10 minutes."
        );
        hub.setup.set(code.to_string(), expires_at);
    }

    let app = net::router(hub.clone());
    let shutdown = hub.shutdown.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
    });
    let background = vec![
        scheduler::spawn(&hub),
        maintenance::spawn(&hub),
        spawn_connector_sweep(&hub),
        spawn_skill_update_check(&hub),
        backup::spawn_scheduler(&hub),
    ];
    hub.recover();
    if let Err(err) = hub.resume_connect_polls().await {
        tracing::warn!(error = %err, "could not resume the open connect attempts");
    }
    tracing::info!(address = %local_addr, url = %public_url, "the hub listens");
    Ok(HubHandle {
        hub,
        local_addr,
        public_url,
        server,
        background,
        _lock: lock,
    })
}

/// Checks every hour that the active connections still work. A connection
/// that Composio reports as expired becomes "needs a new sign-in".
fn spawn_connector_sweep(hub: &Hub) -> JoinHandle<()> {
    let hub = hub.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = hub.shutdown.cancelled() => return,
                () = tokio::time::sleep(connectors::STATUS_SWEEP_INTERVAL) => {}
            }
            if let Err(err) = hub.sweep_connection_status().await {
                tracing::warn!(error = %err, "could not check the connections");
            }
        }
    })
}

/// How often the hub looks whether the daily skill update check is due.
const SKILL_UPDATE_TICK: Duration = Duration::from_secs(60 * 60);

/// Checks the sources of installed skills for a new version once a day. A
/// restart does not make the check run again sooner: the time of the last
/// check is in the store.
fn spawn_skill_update_check(hub: &Hub) -> JoinHandle<()> {
    let hub = hub.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = hub.shutdown.cancelled() => return,
                () = tokio::time::sleep(SKILL_UPDATE_TICK) => {}
            }
            match hub.check_skill_updates_if_due().await {
                Ok(Some(report)) => tracing::info!(
                    checked = report.checked,
                    available = report.available,
                    failed = report.failed,
                    "checked the skills for new versions"
                ),
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "could not check the skills for new versions")
                }
            }
        }
    })
}

impl HubHandle {
    /// Uses another Composio API base URL, for example a local fake server
    /// in tests or a proxy. The default is the public v3.1 API. The
    /// `CONSORT_COMPOSIO_URL` variable sets it at start too.
    pub fn set_composio_url(&self, url: &str) {
        self.hub.connectors.set_base_url(url);
    }

    /// Uses other addresses for the built-in skill directories: the
    /// Smithery API (default `https://api.smithery.ai`) and Anthropic's
    /// skills repo (default `https://github.com/anthropics/skills`). Tests
    /// point them at local fakes. `CONSORT_SMITHERY_URL` and
    /// `CONSORT_ANTHROPIC_SKILLS_URL` set them at start too. It drops the
    /// directory caches.
    pub fn set_skill_directory_urls(&self, smithery: &str, anthropic: &str) {
        let (smithery, anthropic) = (smithery.to_owned(), anthropic.to_owned());
        self.hub.skill_sources.set_config(move |c| {
            c.smithery_url = smithery;
            c.anthropic_url = anthropic;
        });
    }

    /// Allows `file://` and `http://` skill sources. Only tests turn this
    /// on: a real hub fetches skills over https only.
    pub fn allow_local_skill_sources(&self, allow: bool) {
        self.hub
            .skill_sources
            .set_config(move |c| c.allow_local = allow);
    }

    /// Checks every installed skill for a new version now, as the daily
    /// check does.
    pub async fn check_skill_updates(&self) -> Result<UpdateReport> {
        self.hub.check_skill_updates().await
    }

    /// Stores a skill from its files, with the checks of an install: the
    /// `SKILL.md` front matter, the name rule and the file limits. A skill
    /// with scripts stays off unless `enabled` says otherwise, and
    /// Claude-specific `hooks` are removed. Each file is
    /// `(path, bytes, executable)`. Tests and tools on the hub machine use
    /// it; clients use `skill.install`.
    pub async fn add_skill_from_files(
        &self,
        file_name: &str,
        files: Vec<(String, Vec<u8>, bool)>,
        use_in: consort_proto::model::UseIn,
        enabled: Option<bool>,
    ) -> Result<consort_proto::model::Skill> {
        self.hub
            .store_skill(skills::NewSkill {
                source: consort_proto::model::SkillSource::Upload {
                    file_name: file_name.to_owned(),
                },
                commit: None,
                files: files
                    .into_iter()
                    .map(|(path, bytes, executable)| skills::NewSkillFile {
                        path,
                        bytes,
                        executable,
                    })
                    .collect(),
                use_in,
                enabled,
                license: None,
                audits: Vec::new(),
            })
            .await
    }

    /// Starts the scheduled backup when one is due at `at`, as the hub does
    /// every 30 seconds. Returns true when it started one. Tests use it to
    /// run the schedule at a chosen time.
    pub fn backup_if_due_at(&self, at: Timestamp) -> Result<bool> {
        self.hub.backup_if_due(at)
    }

    /// Whether a backup runs now.
    pub fn backup_running(&self) -> bool {
        self.hub.backup_running()
    }

    /// The address that the hub listens on, with the real port.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn public_url(&self) -> String {
        self.public_url.clone()
    }

    /// The setup code that the hub made at start, while it is valid and no
    /// client has paired.
    pub fn setup_code(&self) -> Option<String> {
        self.hub.setup.current()
    }

    /// Makes a single-use worker pairing code that is valid for 10 minutes.
    pub async fn new_worker_pairing_code(&self) -> Result<String> {
        let (code, _) = self
            .hub
            .new_pairing_code(PairingKind::Worker, Origin::HubMachine)
            .await?;
        Ok(code.to_string())
    }

    /// Stops the hub. Open connections close.
    pub async fn shutdown(self) -> Result<()> {
        self.hub.shutdown.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(10), async {
            let served = self.server.await;
            for task in self.background {
                let _ = task.await;
            }
            served
        })
        .await;
        match stopped {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(err))) => Err(err.into()),
            Ok(Err(err)) => Err(err.into()),
            Err(_) => {
                tracing::warn!("the hub did not stop in 10 seconds");
                Ok(())
            }
        }
    }
}
