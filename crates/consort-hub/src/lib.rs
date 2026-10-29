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
//! [`HubHandle`] and [`HubError`].

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod api;
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

use crate::hub::Hub;
use crate::store::machine::{self, PairingKind};

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
    let hub = Hub::open(&config.data_dir, public_url.clone()).await?;

    let has_client = hub.store.read(machine::active_client_count).await? > 0;
    if has_client {
        hub.setup.mark_paired();
    } else {
        let (code, expires_at) = hub.new_pairing_code(PairingKind::Client).await?;
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
    let background = vec![scheduler::spawn(&hub), maintenance::spawn(&hub)];
    hub.recover();
    tracing::info!(address = %local_addr, url = %public_url, "the hub listens");
    Ok(HubHandle {
        hub,
        local_addr,
        public_url,
        server,
        background,
    })
}

impl HubHandle {
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
        let (code, _) = self.hub.new_pairing_code(PairingKind::Worker).await?;
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
