//! Reach from other devices: the ways that a peer gets to a hub that listens
//! only on loopback.
//!
//! - **Tailscale.** The hub asks `tailscale serve` for an address on the
//!   tailnet, with a real certificate. The hub stays on loopback.
//! - **This network.** The hub opens a second port with TLS and a self-signed
//!   certificate. A peer pins the certificate.
//!
//! Each way that is on has a row in the store. Its address joins the origins
//! that a peer may sign. An address leaves the list when its way turns off, so
//! a peer still cannot pass a challenge from another hub.
//!
//! Only a hub on the user's computer and a development hub may change Tailscale
//! or open a port. Any other hub reports its state, and its owner runs
//! `consort hub reach` on the server.

pub mod cli;
mod program;
mod tailscale;
mod tls;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use consort_proto::client_api::{
    HubReachSet, HubReachSetResult, HubReachStatus, HubReachStatusResult, NetworkState,
    PairingAddress, ReachInfo, ReachMethod, TailscaleState,
};
use consort_proto::release::InstallKind;
use consort_proto::time::Timestamp;
use tokio::task::JoinHandle;

pub use program::{ProgramError, ProgramOutput, SystemTailscale, TailscaleProgram};
pub(crate) use tls::{TLS_CERT_FILE, TLS_KEY_FILE};

use crate::error::Result;
use crate::hub::{Caller, Hub};
use crate::store::reach::{self as reach_store, ReachRow, TARGET_KEY};

/// How often the hub checks that its TLS port matches the store. `consort hub
/// reach off` changes the store from another process.
const RECONCILE_TICK: Duration = Duration::from_secs(10);

/// Whether a hub may change its reach.
pub fn may_change(install: InstallKind, dev: bool) -> bool {
    dev || install == InstallKind::Local
}

/// The URL that a Serve entry forwards to, for the address the hub listens on.
pub(crate) fn serve_target(local: SocketAddr) -> String {
    let host = if local.ip().is_unspecified() {
        SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), local.port())
    } else {
        local
    };
    format!("http://{host}")
}

/// `<hostname>.local`, from the name of this machine. `None` when the name has
/// no usable letter.
pub(crate) fn local_host(raw: &str) -> Option<String> {
    let first = raw
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let clean: String = first
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let clean = clean.trim_matches('-');
    (!clean.is_empty()).then(|| format!("{clean}.local"))
}

/// The reach state of a running hub. The store holds what is on. This holds
/// what is live.
pub(crate) struct ReachState {
    data_dir: PathBuf,
    program: Mutex<Arc<dyn TailscaleProgram>>,
    can_change: AtomicBool,
    /// What a Serve entry forwards to.
    target: Mutex<String>,
    /// The port of the main listener, for the port after it.
    main_port: Mutex<u16>,
    network_host: Mutex<Option<String>>,
    listener: tokio::sync::Mutex<Option<tls::NetworkListener>>,
    /// A change runs at a time.
    changing: tokio::sync::Mutex<()>,
    /// What the last failed change of Tailscale said, until the next try.
    last_failure: Mutex<Option<TailscaleState>>,
    /// The user asked for Tailscale and HTTPS was off. The hub turns it on
    /// when HTTPS is there.
    wants_tailscale: AtomicBool,
    /// Why the TLS port did not open.
    network_failure: Mutex<Option<String>>,
}

impl ReachState {
    pub(crate) fn open(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            program: Mutex::new(Arc::new(SystemTailscale::default())),
            can_change: AtomicBool::new(false),
            target: Mutex::new(String::new()),
            main_port: Mutex::new(0),
            network_host: Mutex::new(None),
            listener: tokio::sync::Mutex::new(None),
            changing: tokio::sync::Mutex::new(()),
            last_failure: Mutex::new(None),
            wants_tailscale: AtomicBool::new(false),
            network_failure: Mutex::new(None),
        }
    }

    pub(crate) fn set_program(&self, program: Arc<dyn TailscaleProgram>) {
        if let Ok(mut held) = self.program.lock() {
            *held = program;
        }
    }

    pub(crate) fn set_network_host(&self, host: &str) {
        if let Ok(mut held) = self.network_host.lock() {
            *held = Some(host.to_owned());
        }
    }

    fn program(&self) -> Arc<dyn TailscaleProgram> {
        match self.program.lock() {
            Ok(held) => held.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn target(&self) -> String {
        self.target.lock().map(|t| t.clone()).unwrap_or_default()
    }

    fn host(&self) -> Option<String> {
        let chosen = self.network_host.lock().ok().and_then(|h| h.clone());
        chosen.or_else(|| local_host(&gethostname::gethostname().to_string_lossy()))
    }

    fn last_failure(&self) -> Option<TailscaleState> {
        self.last_failure.lock().ok().and_then(|f| f.clone())
    }

    fn set_last_failure(&self, failure: Option<TailscaleState>) {
        if let Ok(mut held) = self.last_failure.lock() {
            *held = failure;
        }
    }

    fn network_failure(&self) -> Option<String> {
        self.network_failure.lock().ok().and_then(|f| f.clone())
    }

    fn set_network_failure(&self, failure: Option<String>) {
        if let Ok(mut held) = self.network_failure.lock() {
            *held = failure;
        }
    }

    /// Closes the TLS port.
    pub(crate) async fn stop_listener(&self) {
        let listener = self.listener.lock().await.take();
        if let Some(listener) = listener {
            listener.stop().await;
        }
    }
}

/// The state to show, from the live state and the last failed change. A state
/// that the live check can show by itself wins over an old failure.
fn shown_state(live: TailscaleState, failure: Option<TailscaleState>) -> TailscaleState {
    match (live, failure) {
        (TailscaleState::Available { .. } | TailscaleState::HttpsOff { .. }, Some(failure)) => {
            failure
        }
        (live, _) => live,
    }
}

/// The address on a TLS port.
fn network_address(host: &str, port: u16) -> String {
    format!("https://{host}:{port}")
}

/// Checks the TLS port against the store while the hub runs.
pub(crate) fn spawn(hub: &Hub) -> JoinHandle<()> {
    let hub = hub.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = hub.shutdown.cancelled() => return,
                () = tokio::time::sleep(RECONCILE_TICK) => {}
            }
            if let Err(err) = hub.reconcile_network().await {
                tracing::warn!(error = %err, "could not check the TLS port");
            }
        }
    })
}

/// True when a program on this computer answers on this port.
async fn port_answers(port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_secs(1),
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
        )
        .await,
        Ok(Ok(_))
    )
}

impl Hub {
    /// Prepares reach at the start of the hub: it saves the Serve target for
    /// `consort hub reach`, opens the TLS port when This network is on, and
    /// checks the Serve entry.
    pub(crate) async fn start_reach(&self, local: SocketAddr, can_change: bool) -> Result<()> {
        let target = serve_target(local);
        if let Ok(mut held) = self.reach.target.lock() {
            held.clone_from(&target);
        }
        if let Ok(mut held) = self.reach.main_port.lock() {
            *held = local.port();
        }
        self.reach.can_change.store(can_change, Ordering::Relaxed);
        self.store
            .write(move |tx| tx.meta_set(TARGET_KEY, &target))
            .await?;
        if let Err(err) = self.reconcile_network().await {
            tracing::warn!(error = %err, "could not open the TLS port");
        }
        // A hub that never used Tailscale does not run the program at its start.
        let uses_tailscale = self
            .store
            .read(reach_store::rows)
            .await?
            .iter()
            .any(|row| row.method == ReachMethod::Tailscale);
        if uses_tailscale {
            // Off the start path: the program can take seconds. The check
            // shows again each time that a client asks.
            let hub = self.clone();
            tokio::spawn(async move {
                if let Ok(info) = hub.reach_info().await
                    && let TailscaleState::Removed { address } = info.tailscale
                {
                    tracing::warn!(%address, "the Tailscale entry of the hub is gone. Open Settings > Hub to decide");
                }
            });
        }
        Ok(())
    }

    /// The origins that a peer may sign: the public URL and the address of
    /// each reach that is on. The store is the only list, so a change from
    /// `consort hub reach` counts at once, with no restart.
    pub(crate) async fn accepted_origins(&self) -> Vec<String> {
        let mut origins = vec![self.settings.origin.clone()];
        match self.store.read(reach_store::rows).await {
            Ok(rows) => origins.extend(
                rows.iter()
                    .filter_map(|row| consort_proto::auth::normalize_origin(&row.address).ok()),
            ),
            Err(err) => tracing::error!(error = %err, "could not read the reach addresses"),
        }
        origins
    }

    /// The address for a new device or worker: Tailscale, then This network,
    /// then the public URL.
    pub(crate) async fn pairing_address(&self) -> Result<PairingAddress> {
        let rows = self.store.read(reach_store::rows).await?;
        let by = |method| rows.iter().find(|row| row.method == method);
        if let Some(row) = by(ReachMethod::Tailscale) {
            return Ok(PairingAddress {
                hub_url: row.address.clone(),
                tls_sha256: None,
            });
        }
        if let Some(row) = by(ReachMethod::ThisNetwork)
            && let Some(identity) = tls::load_identity(&self.reach.data_dir)
        {
            return Ok(PairingAddress {
                hub_url: row.address.clone(),
                tls_sha256: Some(identity.pin()),
            });
        }
        Ok(PairingAddress {
            hub_url: self.settings.public_url.clone(),
            tls_sha256: None,
        })
    }

    /// The state of each way.
    pub(crate) async fn reach_info(&self) -> Result<ReachInfo> {
        let can_change = self.reach.can_change.load(Ordering::Relaxed);
        let program = self.reach.program();
        let target = self.reach.target();
        let rows = self.store.read(reach_store::rows).await?;
        let saved = rows.iter().find(|r| r.method == ReachMethod::Tailscale);
        let mut live = tailscale::read_state(program.as_ref(), saved, &target).await;
        // HTTPS is on now, and the user asked for this: no other step.
        if can_change
            && matches!(live, TailscaleState::Available { .. })
            && self.reach.wants_tailscale.load(Ordering::Relaxed)
        {
            self.turn_on_tailscale().await?;
            let rows = self.store.read(reach_store::rows).await?;
            let saved = rows.iter().find(|r| r.method == ReachMethod::Tailscale);
            live = tailscale::read_state(program.as_ref(), saved, &target).await;
        }
        let network_row = rows.iter().find(|r| r.method == ReachMethod::ThisNetwork);
        let this_network = match network_row {
            None => NetworkState::Off,
            Some(row) => match (
                self.reach.network_failure(),
                tls::load_identity(&self.reach.data_dir),
            ) {
                (None, Some(identity)) => NetworkState::On {
                    address: row.address.clone(),
                    tls_sha256: identity.pin(),
                },
                (failure, _) => NetworkState::Failed {
                    address: row.address.clone(),
                    detail: failure.unwrap_or_else(|| {
                        "The certificate of the hub is missing. Turn This network off and on."
                            .to_owned()
                    }),
                },
            },
        };
        Ok(ReachInfo {
            can_change,
            tailscale: shown_state(live, self.reach.last_failure()),
            this_network,
            pairing: self.pairing_address().await?,
        })
    }

    /// Runs `tailscale serve` for this hub and records what happened.
    async fn turn_on_tailscale(&self) -> Result<()> {
        let _one_at_a_time = self.reach.changing.lock().await;
        let program = self.reach.program();
        let target = self.reach.target();
        let previous = self
            .store
            .read(|conn| reach_store::row(conn, ReachMethod::Tailscale))
            .await?;
        match tailscale::turn_on(program.as_ref(), previous.as_ref(), &target).await {
            Ok(on) => {
                let row = ReachRow {
                    method: ReachMethod::Tailscale,
                    address: on.address,
                    port: on.port,
                    target: Some(target),
                    turned_on_at: Timestamp::now(),
                };
                tracing::info!(address = %row.address, "the hub answers on the tailnet");
                self.store
                    .write(move |tx| reach_store::save(tx.db(), &row))
                    .await?;
                self.reach.set_last_failure(None);
                self.reach.wants_tailscale.store(false, Ordering::Relaxed);
            }
            Err(state) => {
                let waits = matches!(state, TailscaleState::HttpsOff { .. });
                self.reach.wants_tailscale.store(waits, Ordering::Relaxed);
                tracing::warn!("Tailscale did not turn on");
                self.reach.set_last_failure(Some(state));
            }
        }
        Ok(())
    }

    async fn turn_off_tailscale(&self) -> Result<()> {
        let _one_at_a_time = self.reach.changing.lock().await;
        self.reach.wants_tailscale.store(false, Ordering::Relaxed);
        self.reach.set_last_failure(None);
        let row = self
            .store
            .read(|conn| reach_store::row(conn, ReachMethod::Tailscale))
            .await?;
        let Some(row) = row else {
            return Ok(());
        };
        let program = self.reach.program();
        if let Err(state) = tailscale::turn_off(program.as_ref(), &row).await {
            tracing::warn!("Tailscale did not turn off");
            self.reach.set_last_failure(Some(state));
            return Ok(());
        }
        self.store
            .write(|tx| reach_store::remove(tx.db(), ReachMethod::Tailscale))
            .await
    }

    /// Makes the TLS port match the store. The first free try is the port
    /// after the main port.
    pub(crate) async fn reconcile_network(&self) -> Result<()> {
        let row = self
            .store
            .read(|conn| reach_store::row(conn, ReachMethod::ThisNetwork))
            .await?;
        let mut listener = self.reach.listener.lock().await;
        match row {
            None => {
                if let Some(open) = listener.take() {
                    open.stop().await;
                }
                self.reach.set_network_failure(None);
            }
            Some(row) => {
                if listener.as_ref().is_some_and(|open| open.port == row.port) {
                    return Ok(());
                }
                if let Some(open) = listener.take() {
                    open.stop().await;
                }
                let identity = tls::load_identity(&self.reach.data_dir);
                let started = match &identity {
                    Some(identity) => tls::NetworkListener::start(self, identity, row.port).await,
                    None => Err(crate::error::HubError::internal(
                        "The certificate of the hub is missing. Turn This network off and on.",
                    )),
                };
                match started {
                    Ok(open) => {
                        *listener = Some(open);
                        self.reach.set_network_failure(None);
                    }
                    Err(err) => self.reach.set_network_failure(Some(err.to_string())),
                }
            }
        }
        Ok(())
    }

    async fn turn_on_network(&self) -> Result<()> {
        let _one_at_a_time = self.reach.changing.lock().await;
        let already = self
            .store
            .read(|conn| reach_store::row(conn, ReachMethod::ThisNetwork))
            .await?;
        if already.is_some() {
            return self.reconcile_network().await;
        }
        let Some(host) = self.reach.host() else {
            self.reach.set_network_failure(Some(
                "This computer has no name that the network can use. Set a computer name, then try again."
                    .to_owned(),
            ));
            return Ok(());
        };
        let data_dir = self.reach.data_dir.clone();
        let cert_host = host.clone();
        let identity = tokio::task::spawn_blocking(move || {
            tls::load_or_create_identity(&data_dir, &cert_host)
        })
        .await??;
        let main_port = self.reach.main_port.lock().map(|port| *port).unwrap_or(0);
        let next = main_port.checked_add(1).filter(|_| main_port != 0);
        let mut listener = self.reach.listener.lock().await;
        let mut open = None;
        // The system lets a program bind all addresses of a port when another
        // program holds only the loopback address of it. So ask the port too.
        let next = match next {
            Some(port) if !port_answers(port).await => Some(port),
            _ => None,
        };
        if let Some(next) = next {
            open = tls::NetworkListener::start(self, &identity, next)
                .await
                .ok();
        }
        let open = match open {
            Some(open) => open,
            // The next port is taken, so the system picks one.
            None => tls::NetworkListener::start(self, &identity, 0).await?,
        };
        let row = ReachRow {
            method: ReachMethod::ThisNetwork,
            address: network_address(&host, open.port),
            port: open.port,
            target: None,
            turned_on_at: Timestamp::now(),
        };
        tracing::info!(address = %row.address, "the hub answers on this network");
        *listener = Some(open);
        drop(listener);
        self.reach.set_network_failure(None);
        self.store
            .write(move |tx| reach_store::save(tx.db(), &row))
            .await
    }

    async fn turn_off_network(&self) -> Result<()> {
        let _one_at_a_time = self.reach.changing.lock().await;
        self.store
            .write(|tx| reach_store::remove(tx.db(), ReachMethod::ThisNetwork))
            .await?;
        self.reach.set_network_failure(None);
        self.reach.stop_listener().await;
        Ok(())
    }

    /// Turns a way on or off, and answers with the state. A hub that may not
    /// change its reach changes nothing.
    pub(crate) async fn set_reach(&self, method: ReachMethod, on: bool) -> Result<ReachInfo> {
        if self.reach.can_change.load(Ordering::Relaxed) {
            match (method, on) {
                (ReachMethod::Tailscale, true) => self.turn_on_tailscale().await?,
                (ReachMethod::Tailscale, false) => self.turn_off_tailscale().await?,
                (ReachMethod::ThisNetwork, true) => {
                    if let Err(err) = self.turn_on_network().await {
                        self.reach.set_network_failure(Some(err.to_string()));
                    }
                }
                (ReachMethod::ThisNetwork, false) => self.turn_off_network().await?,
            }
        }
        self.reach_info().await
    }

    /// `hub.reach_status`.
    pub(crate) async fn hub_reach_status(
        &self,
        _caller: &Caller,
        _request: HubReachStatus,
    ) -> Result<HubReachStatusResult> {
        Ok(HubReachStatusResult {
            reach: self.reach_info().await?,
        })
    }

    /// `hub.reach_set`.
    pub(crate) async fn hub_reach_set(
        &self,
        _caller: &Caller,
        request: HubReachSet,
    ) -> Result<HubReachSetResult> {
        Ok(HubReachSetResult {
            reach: self.set_reach(request.method, request.on).await?,
        })
    }
}

#[cfg(test)]
mod tests;
