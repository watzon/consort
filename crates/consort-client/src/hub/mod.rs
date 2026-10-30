//! The hub backend: pairing over HTTP, the authenticated WebSocket, sync,
//! reconnects with jittered backoff, and the commands as client requests.
//!
//! Where things live:
//! - this file: the shared state, the request/response plumbing and pairing,
//! - `link`: the WebSocket session, the reconnect loop and the channel loader,
//! - `outbox`: the messages that wait for the hub, sent in order,
//! - `commands`: what each [`Command`] asks of the hub,
//! - `url`: the hub URL rules.

mod commands;
mod extensions;
mod files;
mod knowledge;
mod link;
mod outbox;
mod url;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use consort_proto::{
    auth::{PAIR_CLIENT_PATH, PairClientRequest, PairClientResponse, PairingCode, PublicKey},
    client_api::TaskGet,
    error::{ErrorBody, ErrorCode},
    frame::{Frame, Request},
    ids::{FrameId, TaskId},
    model::ClientKind,
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
pub use url::normalize_hub_url;
use url::{plain_http_name, resolve_private};

use crate::{
    ClientError,
    cache::{Cache, PendingSend},
    format,
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
        let initials = name.chars().take(2).collect::<String>().to_uppercase();
        ClientOptions {
            name: host,
            kind: ClientKind::Desktop,
            user: UserView { name, initials },
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
}

pub(crate) struct Shared {
    store: Option<Store>,
    options: ClientOptions,
    cache: Mutex<Cache>,
    identity: Mutex<Option<Identity>>,
    connection: Mutex<Connection>,
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
        let cache = Cache {
            pending: outbox.clone(),
            ..Cache::default()
        };
        let shared = Arc::new(Shared {
            store,
            options,
            cache: Mutex::new(cache),
            identity: Mutex::new(identity),
            connection: Mutex::new(connection),
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
        });
        shared.publish_now();
        Ok(shared)
    }

    fn set_connection(&self, connection: Connection) {
        if let Ok(mut c) = self.connection.lock() {
            *c = connection;
        }
        self.publish_now();
    }

    /// Asks for a new view state. A burst of requests makes one state, at
    /// most every [`PUBLISH_INTERVAL`]. The outbox goes to disk at once.
    pub(crate) fn publish(&self) {
        self.sync_outbox();
        self.publish_wake.notify_one();
    }

    /// Builds the view state from the cache and sends it to subscribers when
    /// it differs from the one they have.
    fn publish_now(&self) {
        self.sync_outbox();
        let connection = self
            .connection
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default();
        let pairing = self.pairing.lock().ok().and_then(|p| p.clone());
        let Ok(cache) = self.cache.lock() else {
            return;
        };
        let mut state = view_state(
            &cache,
            connection,
            self.options.user.clone(),
            format::now_ms(),
        );
        drop(cache);
        state.worker_pairing = pairing;
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
            })
            .map_err(|_| ClientError::NotConnected)?;
        // The link checks the deadline of each request. This longer limit
        // only guards against a link that stopped checking.
        let backstop = self.options.timeouts.call * 2;
        let value = match tokio::time::timeout(backstop, rx).await {
            Err(_) | Ok(Ok(Err(Failure::TimedOut))) => return Err(ClientError::TimedOut),
            Ok(Err(_)) | Ok(Ok(Err(Failure::Dropped))) => return Err(ClientError::NotConnected),
            Ok(Ok(Err(Failure::Remote(err)))) => return Err(hub_error(err)),
            Ok(Ok(Ok(value))) => value,
        };
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
    let hub_url = normalize_hub_url(hub_url)?;
    let code: PairingCode = code.parse().map_err(|_| ClientError::BadCode)?;
    let mut secret = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut secret);
    let key = SigningKey::from_bytes(&secret);
    let body = PairClientRequest {
        code,
        public_key: PublicKey::from_bytes(key.verifying_key().to_bytes()),
        name: options.name.clone(),
        kind: options.kind,
    };
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        // A redirect could send the pairing code to another address. The
        // hub answers the pairing itself.
        .redirect(reqwest::redirect::Policy::none());
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
        .map_err(|_| ClientError::Unreachable {
            url: hub_url.clone(),
        })?;
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
        },
        key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
