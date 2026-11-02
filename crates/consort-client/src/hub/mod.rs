//! The hub backend: pairing over HTTP, the authenticated WebSocket, sync,
//! reconnects with jittered backoff, and the commands as client requests.
//!
//! Where things live:
//! - this file: the shared state, the request/response plumbing and pairing,
//! - `link`: the WebSocket session, the reconnect loop and the channel loader,
//! - `outbox`: the messages that wait for the hub, sent in order,
//! - `save`: the cache on disk, so the app opens with data while the hub is down,
//! - `commands`: what each [`Command`] asks of the hub,
//! - `url`: the hub URL rules.

mod admin;
mod commands;
mod convo;
mod extensions;
mod files;
mod knowledge;
mod link;
mod monid;
mod outbox;
mod proactivity;
mod save;
mod url;
mod visual;
mod voice;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use consort_proto::{
    auth::{
        PAIR_CLIENT_PATH, PairClientRequest, PairClientResponse, PairingCode, PairingInvite,
        PublicKey,
    },
    client_api::TaskGet,
    error::{ErrorBody, ErrorCode},
    frame::{Frame, Request},
    ids::{FrameId, TaskId},
    model::{ClientKind, Sha256Hash},
};
use ed25519_dalek::SigningKey;
use serde_json::Value;
use tokio::sync::{Notify, mpsc, oneshot, watch};

pub(crate) use commands::HubBackend;
#[cfg(test)]
pub(crate) use commands::option_for;
pub(crate) use extensions::run_logos;
pub use link::backoff;
pub(crate) use link::{run_link, run_loader};
pub(crate) use outbox::run_outbox;
pub(crate) use save::run_save;
pub use url::normalize_hub_url;
use url::{plain_http_name, resolve_private};

use crate::{
    ClientError,
    cache::{Cache, PendingSend},
    format, persist,
    reduce::view_state,
    store::{ClientConfig, Identity, Store},
    view::{Connection, PairingCodeView, UserView, ViewState},
};

/// How long the client waits for the hub. A test can shorten these.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    /// To open the WebSocket.
    pub connect: Duration,
    /// For each step of the handshake and of the first sync.
    pub handshake: Duration,
    /// For each request after the sync. A request that the hub does not
    /// answer in this time ends the link, because a link that eats requests
    /// is not working.
    pub call: Duration,
    /// How long the link may be silent. The client pings at a third of this,
    /// so a working link is never silent for long.
    pub idle: Duration,
    /// How long the outbox waits before it sends a message again after an
    /// answer that does not say what happened.
    pub retry: Duration,
    /// How often the view state refreshes the labels that follow the clock,
    /// such as "Now" and "Today". The client also refreshes them at each
    /// full minute.
    pub clock: Duration,
}

impl Default for Timeouts {
    fn default() -> Timeouts {
        Timeouts {
            connect: Duration::from_secs(10),
            handshake: Duration::from_secs(20),
            call: Duration::from_secs(30),
            idle: Duration::from_secs(60),
            retry: Duration::from_secs(2),
            clock: Duration::from_secs(60),
        }
    }
}

/// The shortest time between two published view states. A burst of feed
/// entries makes one state, not one per entry.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(50);

/// Who this client is, for pairing.
#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub name: String,
    pub kind: ClientKind,
    pub user: UserView,
    pub timeouts: Timeouts,
}

impl ClientOptions {
    /// A desktop client named after this machine.
    pub fn desktop() -> ClientOptions {
        let host = gethostname::gethostname().to_string_lossy().into_owned();
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "You".to_owned());
        let mut chars = user.chars();
        let name = chars
            .next()
            .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
            .unwrap_or_default();
        ClientOptions {
            name: host,
            kind: ClientKind::Desktop,
            // Only a guess: the name on the hub replaces it in `view_state`.
            user: UserView::from_name(&name),
            timeouts: Timeouts::default(),
        }
    }
}

/// Why a request got no answer from the hub.
enum Failure {
    /// The hub answered with an error.
    Remote(ErrorBody),
    /// The link dropped before the answer came. The hub may have done the
    /// work.
    Dropped,
    /// The hub did not answer in time.
    TimedOut,
}

/// Where the response to one request goes.
type Reply = oneshot::Sender<Result<Value, Failure>>;

struct Outgoing {
    kind: &'static str,
    frame: Frame,
    id: FrameId,
    reply: Reply,
    /// How long the link waits for the answer before it ends.
    limit: Duration,
}

/// How long the client waits for `visual.render`. The hub waits up to 60
/// seconds for the session host, so the client waits longer. A shorter
/// `Timeouts::call` does not shorten it.
const RENDER_LIMIT: Duration = Duration::from_secs(75);

pub(crate) struct Shared {
    store: Option<Store>,
    options: ClientOptions,
    cache: Mutex<Cache>,
    identity: Mutex<Option<Identity>>,
    connection: Mutex<Connection>,
    /// The versions of the hub, from the last time it answered.
    hub_versions: Mutex<Option<consort_proto::error::HubVersions>>,
    pairing: Mutex<Option<PairingCodeView>>,
    link: Mutex<Option<mpsc::UnboundedSender<Outgoing>>>,
    publish: watch::Sender<Arc<ViewState>>,
    /// Reconnect now.
    wake: Notify,
    /// The live link must end, so the next link loads the data again. The
    /// link reads this flag at its next tick.
    restart: std::sync::atomic::AtomicBool,
    /// The view state may have changed.
    publish_wake: Notify,
    /// The outbox may have a message to send, or the link came up.
    outbox_wake: Notify,
    /// A channel may need its snapshot, or the link came up.
    loader_wake: Notify,
    /// The view may use a logo that has not loaded.
    logo_wake: Notify,
    /// The callers that wait for the outcome of a queued message.
    waiters: Mutex<std::collections::HashMap<String, oneshot::Sender<outbox::SendOutcome>>>,
    /// The outbox as last written to disk.
    saved_outbox: Mutex<Vec<PendingSend>>,
    /// The files that the user attached and that are not sent yet.
    uploads: Mutex<crate::uploads::Uploads>,
    /// The last token from `auth.http_token` and when it ends, in Unix
    /// milliseconds.
    http_token: Mutex<Option<(String, i64)>>,
    /// The cache on disk. See [`save`].
    cache_file: Mutex<save::Slot>,
    /// The cache may need a save.
    save_wake: Notify,
    /// When the data was last known to be current, in Unix milliseconds, or
    /// 0. The saved data from an earlier run sets it at the start.
    fresh_at: std::sync::atomic::AtomicI64,
    /// The number of the identity that the cache belongs to. Pair and
    /// Unpair raise it. A link, a call and a recover that started with an
    /// older number never write the cache, so data of the old hub never
    /// lands under the new owner.
    generation: std::sync::atomic::AtomicU64,
    /// Recovers that wait for the data of a skipped change. The cache is
    /// not saved while one runs, so the saved feed position never moves past
    /// a change whose data has not arrived.
    recovering: std::sync::atomic::AtomicUsize,
    /// The images of visuals that this run asked for.
    visual_images: Mutex<crate::visual::VisualImages>,
}

impl Shared {
    pub(crate) fn new(
        store: Option<Store>,
        options: ClientOptions,
        publish: watch::Sender<Arc<ViewState>>,
    ) -> Result<Arc<Shared>, ClientError> {
        // A saved identity that cannot be read is not a reason to stop. The
        // app starts unpaired and says what to do, and pairing again writes
        // new files.
        let (identity, refused) = match &store {
            Some(store) => match store.load() {
                // The saved hub URL gets the same checks as a typed one. A
                // file from an older version, or an edited file, may hold
                // `http` to a public host, a path or a password.
                Ok(Some(mut identity)) => match url::checked_saved_url(&identity.config.hub_url) {
                    Ok(hub_url) => {
                        identity.config.hub_url = hub_url;
                        (Some(identity), None)
                    }
                    Err(reason) => (None, Some(reason)),
                },
                Ok(None) => (None, None),
                Err(ClientError::Storage { path, message }) => (
                    None,
                    Some(format!(
                        "Consort cannot read its saved sign-in in {path}: {message}. Pair this app again to make a new sign-in"
                    )),
                ),
                Err(other) => return Err(other),
            },
            None => (None, None),
        };
        let connection = match (&identity, refused) {
            (Some(_), _) => Connection::Connecting,
            (None, Some(reason)) => Connection::Refused { reason },
            (None, None) => Connection::NotPaired,
        };
        let outbox = match (&store, &identity) {
            (Some(store), Some(identity)) => store.load_outbox(&identity.config.client_id),
            _ => Vec::new(),
        };
        // The saved data of the last run, for the hub that this client is
        // paired with. The app opens with it before the first connect.
        let saved = match (&store, &identity) {
            (Some(store), Some(identity)) => {
                let (file, saved) = crate::persist::open_or_recover(
                    &store.cache_path(),
                    &persist::Owner::of(identity),
                );
                (
                    file.map_or(save::Slot::Unusable, |f| save::Slot::Open(Box::new(f))),
                    saved,
                )
            }
            // Not paired: no saved data belongs to anyone.
            (Some(store), None) => {
                crate::persist::delete(&store.cache_path());
                (save::Slot::Closed, None)
            }
            (None, _) => (save::Slot::Unusable, None),
        };
        let (cache_file, saved) = saved;
        let fresh_at = saved.as_ref().map_or(0, |s| s.fresh_at);
        let cache = Cache {
            pending: outbox.clone(),
            ..saved.map(|s| s.cache).unwrap_or_default()
        };
        let shared = Arc::new(Shared {
            store,
            options,
            cache: Mutex::new(cache),
            identity: Mutex::new(identity),
            connection: Mutex::new(connection),
            hub_versions: Mutex::new(None),
            pairing: Mutex::new(None),
            link: Mutex::new(None),
            publish,
            wake: Notify::new(),
            restart: std::sync::atomic::AtomicBool::new(false),
            publish_wake: Notify::new(),
            outbox_wake: Notify::new(),
            loader_wake: Notify::new(),
            logo_wake: Notify::new(),
            waiters: Mutex::new(std::collections::HashMap::new()),
            saved_outbox: Mutex::new(outbox),
            uploads: Mutex::new(crate::uploads::Uploads::default()),
            http_token: Mutex::new(None),
            cache_file: Mutex::new(cache_file),
            save_wake: Notify::new(),
            fresh_at: std::sync::atomic::AtomicI64::new(fresh_at),
            generation: std::sync::atomic::AtomicU64::new(0),
            recovering: std::sync::atomic::AtomicUsize::new(0),
            visual_images: Mutex::new(crate::visual::VisualImages::default()),
        });
        shared.publish_now();
        Ok(shared)
    }

    fn set_hub_versions(&self, versions: Option<consort_proto::error::HubVersions>) {
        if let Ok(mut held) = self.hub_versions.lock() {
            *held = versions;
        }
    }

    fn set_connection(&self, connection: Connection) {
        // The app no longer belongs to the hub that it knew.
        if matches!(
            connection,
            Connection::NotPaired | Connection::Refused { .. }
        ) {
            self.set_hub_versions(None);
        }
        if let Ok(mut c) = self.connection.lock() {
            *c = connection;
        }
        self.publish_now();
    }

    /// Asks for a new view state. A burst of requests makes one state, at
    /// most every [`PUBLISH_INTERVAL`]. The outbox goes to disk at once.
    pub(crate) fn publish(&self) {
        self.sync_outbox();
        self.request_save();
        self.publish_wake.notify_one();
    }

    /// Builds the view state from the cache and sends it to subscribers when
    /// it differs from the one they have.
    fn publish_now(&self) {
        self.sync_outbox();
        self.request_save();
        let connection = self
            .connection
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default();
        let pairing = self.pairing.lock().ok().and_then(|p| p.clone());
        let Ok(cache) = self.cache.lock() else {
            return;
        };
        let now = format::now_ms();
        // While the link is up, the data is current.
        if connection == Connection::Online {
            self.mark_fresh();
        }
        let has_data = cache.loaded && !cache.channels.is_empty();
        let mut state = view_state(&cache, connection.clone(), self.options.user.clone(), now);
        drop(cache);
        crate::offline::mark_saved(
            &mut state,
            crate::offline::saved_data(&connection, has_data, self.fresh_at(), now),
        );
        state.worker_pairing = pairing;
        state.hub_versions = self.hub_versions.lock().ok().and_then(|v| v.clone());
        state.hub_url = self
            .identity
            .lock()
            .ok()
            .and_then(|i| i.as_ref().map(|i| i.config.hub_url.clone()))
            .unwrap_or_default();
        if let Ok(uploads) = self.uploads.lock() {
            state.uploads = uploads.views();
        }
        // A blob from the hub shows its thumbnail once it is in the cache.
        let local = self
            .cache
            .lock()
            .map(|c| c.local_blobs.clone())
            .unwrap_or_default();
        for upload in &mut state.uploads {
            if upload.local_path.is_none()
                && let Some(sha) = &upload.sha256
            {
                upload.local_path = local.get(sha).cloned();
            }
        }
        self.logo_wake.notify_one();
        self.publish.send_if_modified(|current| {
            if **current == state {
                false
            } else {
                *current = Arc::new(state);
                true
            }
        });
    }

    /// Writes the outbox to disk when it changed since the last write.
    fn sync_outbox(&self) {
        let (Some(store), Some(identity)) = (&self.store, self.identity()) else {
            return;
        };
        let Some(current) = self.cache.lock().ok().map(|c| c.pending.clone()) else {
            return;
        };
        let Ok(mut saved) = self.saved_outbox.lock() else {
            return;
        };
        if *saved == current {
            return;
        }
        match store.save_outbox(&identity.config.client_id, &current) {
            Ok(()) => *saved = current,
            Err(err) => tracing::warn!(error = %err, "cannot save the outbox"),
        }
    }

    /// The number of the current identity (see `generation`).
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Changes the cache, but only while the identity is still the one of
    /// `generation`. Returns `None` when it changed: the caller drops what
    /// it got from the old hub.
    pub(crate) fn with_cache_of<T>(
        &self,
        generation: u64,
        change: impl FnOnce(&mut Cache) -> T,
    ) -> Option<T> {
        let mut cache = self.cache.lock().ok()?;
        // Pair and Unpair raise the number under the cache lock, so this
        // check and the change see the same owner.
        (self.generation() == generation).then(|| change(&mut cache))
    }

    /// Starts a new owner of the cache: raises the generation and resets the
    /// cache with `reset`, under the cache lock. The live link to the old
    /// hub stops taking requests.
    pub(crate) fn new_owner(&self, reset: impl FnOnce(&mut Cache)) {
        if let Ok(mut cache) = self.cache.lock() {
            self.generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            reset(&mut cache);
        }
        if let Ok(mut link) = self.link.lock() {
            *link = None;
        }
        // The images belong to the old hub.
        if let Ok(mut images) = self.visual_images.lock() {
            images.forget_ready();
        }
    }

    /// Ends the live link, so the client connects again and loads the data
    /// again. Use it when the cache is known to miss a change.
    pub(crate) fn restart_link(&self) {
        self.restart
            .store(true, std::sync::atomic::Ordering::Release);
    }

    fn identity(&self) -> Option<Identity> {
        self.identity.lock().ok().and_then(|i| i.clone())
    }

    fn is_online(&self) -> bool {
        self.link.lock().is_ok_and(|l| l.is_some())
    }

    /// Sends one request on the live link and waits for its response.
    async fn call<R: Request>(&self, body: R) -> Result<R::Response, ClientError> {
        self.call_within(body, self.options.timeouts.call).await
    }

    /// Like [`Shared::call`], but the link waits `limit` for the answer. For
    /// a request that the hub answers slowly, such as `visual.render`.
    async fn call_within<R: Request>(
        &self,
        body: R,
        limit: Duration,
    ) -> Result<R::Response, ClientError> {
        // An answer from the old hub after a Pair is not used.
        let generation = self.generation();
        let id = FrameId::generate();
        let frame =
            Frame::request(id.clone(), &body).map_err(|e| ClientError::Other(e.to_string()))?;
        let (reply, rx) = oneshot::channel();
        let sender = self
            .link
            .lock()
            .ok()
            .and_then(|l| l.clone())
            .ok_or(ClientError::NotConnected)?;
        sender
            .send(Outgoing {
                kind: R::TYPE,
                frame,
                id,
                reply,
                limit,
            })
            .map_err(|_| ClientError::NotConnected)?;
        // The link checks the deadline of each request. This longer limit
        // only guards against a link that stopped checking.
        let backstop = limit * 2;
        let value = match tokio::time::timeout(backstop, rx).await {
            Err(_) | Ok(Ok(Err(Failure::TimedOut))) => return Err(ClientError::TimedOut),
            Ok(Err(_)) | Ok(Ok(Err(Failure::Dropped))) => return Err(ClientError::NotConnected),
            Ok(Ok(Err(Failure::Remote(err)))) => return Err(hub_error(err)),
            Ok(Ok(Ok(value))) => value,
        };
        if self.generation() != generation {
            return Err(ClientError::NotConnected);
        }
        serde_json::from_value(value).map_err(|e| {
            ClientError::Other(format!(
                "The hub sent a response that this app cannot read: {e}. Update Consort, then try again"
            ))
        })
    }
}

impl Shared {
    /// Asks the hub for the full task (`task.get`) and merges the answer into
    /// the cache. The answer never overwrites a change that the feed made
    /// after the request went out, see [`Cache::merge_task`]. A task that
    /// the hub does not have is remembered, so nobody asks for it again, and
    /// the error goes to the caller.
    pub(crate) async fn fetch_task(&self, task_id: TaskId) -> Result<(), ClientError> {
        let asked_at = self.cache.lock().map(|c| c.seq).unwrap_or(0);
        match self.call(TaskGet { task_id }).await {
            Ok(found) => {
                if let Ok(mut cache) = self.cache.lock() {
                    cache.merge_task(found.task, asked_at);
                }
                self.publish();
                Ok(())
            }
            Err(err) => {
                if matches!(&err, ClientError::Hub { code, .. } if code == "not_found")
                    && let Ok(mut cache) = self.cache.lock()
                {
                    cache.mark_task_absent(&task_id.to_string());
                }
                Err(err)
            }
        }
    }

    /// Fetches the task when the cache holds only a preview of it, or does
    /// not hold it while a snapshot left tasks out. Does nothing when the
    /// cache holds the full task. Call it before a view needs the full brief
    /// or review summary.
    pub(crate) async fn ensure_full_task(&self, task_id: &str) -> Result<(), ClientError> {
        let needed = self.cache.lock().is_ok_and(|c| c.needs_task_fetch(task_id));
        if !needed {
            return Ok(());
        }
        match task_id.parse::<TaskId>() {
            Ok(id) => self.fetch_task(id).await,
            // An ID that is not valid names no task on the hub.
            Err(_) => Ok(()),
        }
    }
}

fn hub_error(err: ErrorBody) -> ClientError {
    let code = serde_json::to_value(err.code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    ClientError::Hub {
        code,
        message: err.message,
    }
}

fn invalid(message: &str) -> ClientError {
    ClientError::Hub {
        code: "invalid".into(),
        message: message.into(),
    }
}

/// The result of an error that means "the link is not usable", not "the hub
/// said no". The request may or may not have reached the hub.
fn is_transport(err: &ClientError) -> bool {
    matches!(err, ClientError::NotConnected | ClientError::TimedOut)
}

/// How long until the next full minute, plus a margin so the clock has
/// passed the minute when the labels are made. At most `clock`.
fn until_next_minute(now_ms: i64, clock: Duration) -> Duration {
    let into_minute = now_ms.rem_euclid(60_000) as u64;
    Duration::from_millis(60_000 - into_minute + 50).min(clock)
}

/// Sends the view state at most every [`PUBLISH_INTERVAL`]. It also makes a
/// new state at each full minute (at most `Timeouts::clock` apart), because
/// labels such as "Now", "2 min" and "Today" follow the clock. Without this,
/// they stay as they were until something else changes.
pub(crate) async fn run_publisher(shared: Arc<Shared>) {
    let mut last: Option<tokio::time::Instant> = None;
    loop {
        let tick = until_next_minute(format::now_ms(), shared.options.timeouts.clock);
        tokio::select! {
            _ = shared.publish_wake.notified() => {}
            _ = tokio::time::sleep(tick) => {}
        }
        if let Some(last) = last {
            let wait = PUBLISH_INTERVAL.saturating_sub(last.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        shared.publish_now();
        last = Some(tokio::time::Instant::now());
    }
}

/// `POST /v1/pair/client`: pairs this client and returns its identity.
pub async fn pair(
    hub_url: &str,
    code: &str,
    options: &ClientOptions,
) -> Result<Identity, ClientError> {
    let (hub_url, code, tls_sha256) = pair_target(hub_url, code)?;
    let mut secret = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut secret);
    let key = SigningKey::from_bytes(&secret);
    let body = PairClientRequest {
        code,
        public_key: PublicKey::from_bytes(key.verifying_key().to_bytes()),
        name: options.name.clone(),
        kind: options.kind,
        os: consort_proto::model::Os::from_std_os(std::env::consts::OS),
        protocol_versions: consort_proto::frame::SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
    };
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        // A redirect could send the pairing code to another address. The
        // hub answers the pairing itself.
        .redirect(reqwest::redirect::Policy::none());
    if let Some(pin) = &tls_sha256 {
        let config = consort_tls::pinned_client_config(pin)
            .map_err(|e| ClientError::Other(e.to_string()))?;
        builder = builder.tls_backend_preconfigured(config);
    }
    // Plain `http` to a name (`hub.local`, `localhost`) passes the URL rule by
    // its spelling. Look the name up, refuse it when it points to a public
    // address, and connect to the addresses that were checked.
    if let Some((host, port)) = plain_http_name(&hub_url) {
        let addrs = resolve_private(&host, port, &hub_url).await?;
        builder = builder.resolve_to_addrs(&host, &addrs);
    }
    let http = builder
        .build()
        .map_err(|e| ClientError::Other(e.to_string()))?;
    let response = http
        .post(format!("{hub_url}{PAIR_CLIENT_PATH}"))
        .json(&body)
        .send()
        .await
        .map_err(|err| send_error(&err, &hub_url))?;
    let status = response.status();
    if status.is_redirection() {
        return Err(ClientError::Other(format!(
            "The hub at {hub_url} answered with a redirect. Consort does not follow it, because the pairing code would go to another address. Enter the final address of the hub, then try again"
        )));
    }
    let text = response
        .text()
        .await
        .map_err(|_| ClientError::Unreachable {
            url: hub_url.clone(),
        })?;
    if !status.is_success() {
        #[derive(serde::Deserialize)]
        struct ErrorEnvelope {
            error: ErrorBody,
        }
        return Err(match serde_json::from_str::<ErrorEnvelope>(&text) {
            Ok(env) => match env.error.code {
                ErrorCode::ProtocolMismatch => {
                    let (title, detail) =
                        crate::versions::pair_error(env.error.hub_versions.as_ref());
                    ClientError::Incompatible { title, detail }
                }
                ErrorCode::Unauthorized | ErrorCode::NotFound | ErrorCode::Invalid => {
                    ClientError::BadCode
                }
                _ => hub_error(env.error),
            },
            Err(_) => ClientError::Unreachable { url: hub_url },
        });
    }
    let paired: PairClientResponse = serde_json::from_str(&text).map_err(|e| {
        ClientError::Other(format!(
            "The hub answered the pairing in a form this app cannot read: {e}. Update Consort, then try again"
        ))
    })?;
    Ok(Identity {
        config: ClientConfig {
            hub_url,
            client_id: paired.client_id,
            name: options.name.clone(),
            tls_sha256,
        },
        key,
    })
}

/// What the user gave: the address and code of an invite (a pasted pairing link
/// or the text of a QR code) or a typed address and code. An invite is in the
/// address field or in the code field.
fn pair_target(
    hub_url: &str,
    code: &str,
) -> Result<(String, PairingCode, Option<Sha256Hash>), ClientError> {
    let invite = match PairingInvite::parse_text(hub_url) {
        Ok(None) => PairingInvite::parse_text(code),
        other => other,
    }
    .map_err(|err| {
        ClientError::Other(format!("{err}. Make a new code on the hub, then try again"))
    })?;
    let Some(invite) = invite else {
        let hub_url = normalize_hub_url(hub_url)?;
        let code = code.parse().map_err(|_| ClientError::BadCode)?;
        return Ok((hub_url, code, None));
    };
    let hub_url = normalize_hub_url(&invite.hub_url)?;
    // A pin is only for `https`. A pin with plain `http` would look safe, and
    // would check nothing.
    if invite.tls_sha256.is_some() && !hub_url.starts_with("https://") {
        return Err(ClientError::InvalidUrl(format!(
            "{hub_url} has a certificate key but does not use https"
        )));
    }
    Ok((hub_url, invite.code, invite.tls_sha256))
}

/// Why a pairing request did not reach the hub.
fn send_error(err: &reqwest::Error, hub_url: &str) -> ClientError {
    let url = hub_url.to_owned();
    match consort_tls::certificate_problem(err) {
        Some(consort_tls::CertificateProblem::NotThePin) => ClientError::PinMismatch { url },
        Some(consort_tls::CertificateProblem::Untrusted) => ClientError::UntrustedHub { url },
        None => ClientError::Unreachable { url },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "consort://pair?hub=https%3A%2F%2Fmac.local%3A7601&code=ABCD-EFGH&tls_sha256=0707070707070707070707070707070707070707070707070707070707070707";

    #[test]
    fn a_pasted_link_gives_the_address_the_code_and_the_pin() {
        for (url, code) in [(LINK, ""), ("", LINK), (LINK, "ZZZZ-ZZZZ")] {
            let (hub_url, code, pin) = pair_target(url, code).unwrap();
            assert_eq!(hub_url, "https://mac.local:7601");
            assert_eq!(code.to_string(), "ABCD-EFGH");
            assert_eq!(pin, Some(Sha256Hash::from_bytes([7; 32])));
        }
    }

    #[test]
    fn a_typed_address_has_no_pin() {
        let (hub_url, _, pin) = pair_target("https://hub.example.com/", "ABCD-EFGH").unwrap();
        assert_eq!(hub_url, "https://hub.example.com");
        assert_eq!(pin, None);
        assert!(matches!(
            pair_target("https://hub.example.com", "nope"),
            Err(ClientError::BadCode)
        ));
    }

    #[test]
    fn a_pin_with_plain_http_is_refused() {
        let text = format!(
            r#"{{"hub_url":"http://192.168.1.5:7600","code":"ABCD-EFGH","tls_sha256":"{}"}}"#,
            "07".repeat(32)
        );
        assert!(matches!(
            pair_target(&text, ""),
            Err(ClientError::InvalidUrl(_))
        ));
    }

    #[test]
    fn a_damaged_link_says_so() {
        let err = pair_target("consort://pair?code=ABCD-EFGH", "").unwrap_err();
        assert!(err.to_string().contains("no hub address"), "{err}");
    }

    #[test]
    fn the_clock_tick_lands_just_after_a_full_minute() {
        let clock = Duration::from_secs(60);
        // 15 s into a minute: wait 45 s and a margin.
        assert_eq!(
            until_next_minute(1_800_000_015_000, clock),
            Duration::from_millis(45_050)
        );
        // Exactly on a minute: wait the whole minute, never more.
        assert_eq!(until_next_minute(1_800_000_000_000, clock), clock);
        // A test can ask for a shorter tick.
        assert_eq!(
            until_next_minute(1_800_000_015_000, Duration::from_millis(100)),
            Duration::from_millis(100)
        );
        // A time before 1970 does not panic.
        assert!(until_next_minute(-1, clock) <= clock);
    }
}
