//! The WebSocket link: connect, sign in, first sync, the live loop, and the
//! reconnect loop around them. Every wait has a limit, so a hub that stops
//! answering ends the link and the client reconnects.

use std::{
    collections::HashMap,
    sync::{Arc, atomic::Ordering},
    time::{Duration, SystemTime},
};

use consort_proto::{
    auth::{
        AuthChallenge, AuthResponse, ClientHello, PeerId, Signature, auth_signing_payload,
        normalize_origin,
    },
    client_api::{
        FeedChange, FeedEntry, FeedSkipped, MessageList, MessageListResult, SnapshotScope,
        SyncSnapshot, SyncSubscribe,
    },
    error::{ErrorBody, ErrorCode, HubVersions},
    frame::{EventFrame, Frame, FrameError, Request, SUPPORTED_PROTOCOL_VERSIONS},
    ids::{ChannelId, FrameId, MessageId},
    model::Message,
};
use ed25519_dalek::Signer as _;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::Value;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::{
    Failure, Outgoing, Reply, Shared, is_transport,
    url::{addr_is_private, plain_http_name, resolve_private, ws_url},
};
use crate::{ClientError, cache::Cache, store::Identity, view::Connection};

/// Why a link ends when the user paired with another hub or unpaired while
/// it ran.
const OWNER_CHANGED: &str = "The app paired with another hub, so this link ended.";

/// Why a session on the link ended.
pub(super) enum SessionEnd {
    /// The hub refused this client. It must pair again.
    Refused(String),
    /// The link dropped or failed. Try again later.
    Lost(String),
    /// The hub and this app have no common protocol version. The pairing is
    /// good, so nothing is forgotten. Try again now and then.
    Incompatible(Option<HubVersions>),
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The most replies one thread page asks for.
pub(super) const THREAD_PAGE: u32 = 200;
/// The most pages of one thread. A thread this long is not a real thread.
const MAX_THREAD_PAGES: usize = 100;

/// How far the wall clock may run ahead of the timer clock between two ticks
/// before the client takes it for a sleep of the computer. The timer clock of
/// Tokio does not count the time that a Mac sleeps, so after a wake the link
/// can look fresh while its connection died long ago. Scheduling jitter is
/// far below this.
const SLEEP_SLACK: Duration = Duration::from_secs(5);

/// True when the wall clock moved on by much more than the timer clock did
/// between two ticks: the computer slept. A wall clock that was set back
/// reads as no jump.
fn slept(timer: Duration, wall: Duration) -> bool {
    wall > timer.saturating_add(SLEEP_SLACK)
}

/// Waits for `delay` on the wall clock too. A timer that stops while the
/// computer sleeps would leave an offline client waiting for the rest of its
/// delay after a wake. A wall clock that was set back does not stretch the
/// wait: the timer clock ends it as well.
async fn sleep_counting_sleep(delay: Duration) {
    let start_wall = SystemTime::now();
    let start = Instant::now();
    loop {
        let waited = start.elapsed();
        let wall = SystemTime::now()
            .duration_since(start_wall)
            .unwrap_or_default();
        if waited >= delay || wall >= delay {
            return;
        }
        let left = delay - waited.max(wall);
        tokio::time::sleep(left.min(Duration::from_secs(1))).await;
    }
}

fn lost(reason: impl Into<String>) -> SessionEnd {
    SessionEnd::Lost(reason.into())
}

fn seconds(limit: Duration) -> String {
    format!("{} s", limit.as_secs_f64().ceil())
}

/// The next frame, for the handshake. Ping, pong and binary messages are
/// skipped.
async fn read_frame(ws: &mut Ws) -> Result<Frame, SessionEnd> {
    loop {
        let message = ws
            .next()
            .await
            .ok_or_else(|| lost("The hub closed the connection."))?
            .map_err(|e| lost(e.to_string()))?;
        match message {
            WsMessage::Text(text) => {
                return Frame::parse(text.as_str()).map_err(|e| lost(e.to_string()));
            }
            WsMessage::Close(_) => return Err(lost("The hub closed the connection.")),
            _ => continue,
        }
    }
}

/// Sends one message, but not for longer than `limit`: a hub that does not
/// read fills the socket buffer, and a write would wait for ever.
async fn send_ws(ws: &mut Ws, message: WsMessage, limit: Duration) -> Result<(), SessionEnd> {
    match tokio::time::timeout(limit, ws.send(message)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(lost(e.to_string())),
        Err(_) => Err(lost(format!(
            "The hub did not take a message in {}.",
            seconds(limit)
        ))),
    }
}

async fn send_frame(ws: &mut Ws, frame: &Frame, limit: Duration) -> Result<(), SessionEnd> {
    let text = frame.to_text().map_err(|e| lost(e.to_string()))?;
    send_ws(ws, WsMessage::text(text), limit).await
}

/// The answer for a request that cannot be written as a frame, such as one
/// that is larger than a frame may be. The problem is in this one request,
/// not in the link, so the request fails by itself and the link stays up.
/// Sending it again would fail again.
fn encode_failure(err: &FrameError) -> ErrorBody {
    ErrorBody::invalid(format!(
        "The request is too large to send ({err}). Shorten the text, then try again"
    ))
}

/// One request during the handshake, before the main loop runs. The whole
/// step has `limit` to finish.
async fn handshake_call<R: Request>(
    ws: &mut Ws,
    body: &R,
    limit: Duration,
) -> Result<Result<R::Response, ErrorBody>, SessionEnd> {
    let step = async {
        let id = FrameId::generate();
        let frame = Frame::request(id.clone(), body).map_err(|e| lost(e.to_string()))?;
        send_frame(ws, &frame, limit).await?;
        loop {
            if let Frame::Response(response) = read_frame(ws).await?
                && response.re == id
            {
                return Ok(match response.into_response::<R>() {
                    Ok(body) => Ok(body),
                    Err(consort_proto::frame::ResponseError::Remote(err)) => Err(err),
                    Err(e) => Err(ErrorBody::invalid(e.to_string())),
                });
            }
        }
    };
    tokio::time::timeout(limit, step)
        .await
        .map_err(|_| lost(format!("The hub did not answer in {}.", seconds(limit))))?
}

fn refused_or_lost(err: ErrorBody) -> SessionEnd {
    match err.code {
        ErrorCode::Unauthorized => SessionEnd::Refused(err.message),
        ErrorCode::ProtocolMismatch => SessionEnd::Incompatible(err.hub_versions),
        _ => SessionEnd::Lost(err.message),
    }
}

/// A failed step of the first sync that can be skipped when the hub said no.
enum Fetch {
    Session(SessionEnd),
    Remote(ErrorBody),
}

async fn handshake_fetch<R: Request>(
    ws: &mut Ws,
    body: &R,
    limit: Duration,
) -> Result<R::Response, Fetch> {
    match handshake_call(ws, body, limit).await {
        Err(end) => Err(Fetch::Session(end)),
        Ok(Err(err)) => Err(Fetch::Remote(err)),
        Ok(Ok(value)) => Ok(value),
    }
}

/// Walks through the replies of one thread, a page at a time, until the hub
/// says that no older replies exist.
///
/// ```text
/// while let Some(request) = pager.next_request() {
///     replies.extend(pager.accept(call(request).await?));
/// }
/// ```
pub(super) struct ThreadPager {
    channel_id: ChannelId,
    root: MessageId,
    before: Option<MessageId>,
    pages: usize,
    done: bool,
}

impl ThreadPager {
    pub(super) fn new(channel_id: ChannelId, root: MessageId) -> ThreadPager {
        ThreadPager {
            channel_id,
            root,
            before: None,
            pages: 0,
            done: false,
        }
    }

    /// The next request, or `None` when the thread is complete.
    pub(super) fn next_request(&self) -> Option<MessageList> {
        (!self.done).then_some(MessageList {
            channel_id: self.channel_id,
            thread_root_id: Some(self.root),
            before: self.before,
            limit: Some(THREAD_PAGE),
        })
    }

    /// Takes one answer and returns its messages.
    pub(super) fn accept(&mut self, page: MessageListResult) -> Vec<Message> {
        self.pages += 1;
        let oldest = page
            .messages
            .iter()
            .min_by_key(|m| (m.created_at, m.id.to_string()))
            .map(|m| m.id);
        match oldest {
            // Stop when the hub makes no progress, so a bad answer cannot
            // loop for ever.
            Some(oldest)
                if page.has_more
                    && Some(oldest) != self.before
                    && self.pages < MAX_THREAD_PAGES =>
            {
                self.before = Some(oldest);
            }
            _ => self.done = true,
        }
        page.messages
    }
}

/// A connection that did not open. The link tries again, because a hub can come
/// back. A certificate problem says what to do.
fn connection_failed(err: &tokio_tungstenite::tungstenite::Error, hub_url: &str) -> SessionEnd {
    match consort_tls::certificate_problem(err) {
        Some(consort_tls::CertificateProblem::NotThePin) => lost(format!(
            "The hub at {hub_url} shows another certificate than the one this app paired with. Someone may be in the way, or the hub has a new certificate. If the hub is new, pair this app again"
        )),
        Some(consort_tls::CertificateProblem::Untrusted) => lost(format!(
            "The hub at {hub_url} has a certificate that no authority signed. Check the hub, or pair this app again with its code"
        )),
        None => lost(format!("The connection failed: {err}")),
    }
}

/// Opens the WebSocket. Plain `http` to a host name (`hub.local`,
/// `localhost`) passes the URL rule by its spelling only. For such a name,
/// the client looks it up, refuses it when it points to a public address,
/// connects to the addresses it checked, and checks the peer address again
/// on the open connection. Nothing is sent before these checks. Any other
/// hub URL connects as it is.
///
/// With a pin (a hub on This network), the client accepts that certificate and
/// no other.
async fn open(
    hub_url: &str,
    ws_url: String,
    pin: Option<&consort_proto::model::Sha256Hash>,
) -> Result<Ws, SessionEnd> {
    if let Some(pin) = pin {
        let config = consort_tls::pinned_client_config(pin).map_err(|e| lost(e.to_string()))?;
        let connector = tokio_tungstenite::Connector::Rustls(Arc::new(config));
        let (ws, _) =
            tokio_tungstenite::connect_async_tls_with_config(ws_url, None, false, Some(connector))
                .await
                .map_err(|e| connection_failed(&e, hub_url))?;
        return Ok(ws);
    }
    // Two crates can bring a crypto provider. With no default, rustls panics
    // on the first connection that has no provider of its own.
    consort_tls::use_default_provider();
    let Some((host, port)) = plain_http_name(hub_url) else {
        let (ws, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| connection_failed(&e, hub_url))?;
        return Ok(ws);
    };
    let addrs = resolve_private(&host, port, hub_url)
        .await
        .map_err(|err| match err {
            // The name points to a public address. The hub URL is not safe,
            // so the client stops and asks for a new pairing.
            ClientError::InvalidUrl(detail) => SessionEnd::Refused(format!(
                "Consort did not connect to the hub, because {detail}. Pair this app again with an address that uses https"
            )),
            other => lost(other.to_string()),
        })?;
    let stream = tokio::net::TcpStream::connect(&addrs[..])
        .await
        .map_err(|e| lost(format!("The connection failed: {e}")))?;
    let peer = stream
        .peer_addr()
        .map_err(|e| lost(format!("The connection failed: {e}")))?;
    if !addr_is_private(peer.ip()) {
        return Err(SessionEnd::Refused(format!(
            "Consort did not connect to the hub, because {host} connected to {}, which is not on this machine or a private network. Pair this app again with an address that uses https",
            peer.ip()
        )));
    }
    let (ws, _) = tokio_tungstenite::client_async_tls(ws_url, stream)
        .await
        .map_err(|e| lost(format!("The connection failed: {e}")))?;
    Ok(ws)
}

/// Signs in, loads or catches up the data, and then runs the live loop until
/// the link ends. `online_since` is set when the link reaches Online.
async fn session(
    shared: &Arc<Shared>,
    identity: &Identity,
    online_since: &mut Option<Instant>,
) -> Result<(), SessionEnd> {
    let limits = shared.options.timeouts;
    // The owner that this link works for. After a Pair or an Unpair, the
    // link stops and writes nothing more into the cache.
    let generation = shared.generation();
    let url = ws_url(&identity.config.hub_url).map_err(|e| lost(e.to_string()))?;
    let mut ws = tokio::time::timeout(
        limits.connect,
        open(
            &identity.config.hub_url,
            url,
            identity.config.tls_sha256.as_ref(),
        ),
    )
    .await
    .map_err(|_| {
        lost(format!(
            "The hub did not answer in {}.",
            seconds(limits.connect)
        ))
    })??;
    if shared.generation() != generation {
        return Err(lost(OWNER_CHANGED));
    }
    // A restart request from an earlier link is over.
    shared
        .restart
        .store(false, std::sync::atomic::Ordering::Release);

    // auth.challenge, then auth.response.
    let challenge = match tokio::time::timeout(limits.handshake, read_frame(&mut ws))
        .await
        .map_err(|_| lost("The hub did not send a challenge."))??
    {
        Frame::Event(event) if event.kind == "auth.challenge" => event
            .decode_as::<AuthChallenge>()
            .map_err(|e| lost(e.to_string()))?,
        _ => return Err(lost("The hub did not send a challenge.")),
    };
    let origin = normalize_origin(&identity.config.hub_url).map_err(|e| lost(e.0))?;
    let signature = identity
        .key
        .sign(&auth_signing_payload(&origin, &challenge.nonce));
    let auth = AuthResponse {
        peer_id: PeerId::Client(identity.config.client_id),
        signature: Signature::from_bytes(signature.to_bytes()),
    };
    handshake_call(&mut ws, &auth, limits.handshake)
        .await?
        .map_err(refused_or_lost)?;
    let hello = ClientHello {
        client_id: identity.config.client_id,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
    };
    let hello = handshake_call(&mut ws, &hello, limits.handshake)
        .await?
        .map_err(refused_or_lost)?;
    shared.set_hub_versions(Some(HubVersions {
        // A hub from before the list has only the version that it picked.
        protocol_versions: if hello.protocol_versions.is_empty() {
            vec![hello.protocol_version]
        } else {
            hello.protocol_versions.clone()
        },
        hub_version: hello.hub_version.clone(),
        install: None,
    }));

    initial_sync(
        shared,
        &mut ws,
        limits.handshake,
        generation,
        hello.hub_epoch,
    )
    .await?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Outgoing>();
    if let Ok(mut link) = shared.link.lock() {
        if shared.generation() != generation {
            return Err(lost(OWNER_CHANGED));
        }
        *link = Some(tx);
    }
    *online_since = Some(Instant::now());
    shared.set_connection(Connection::Online);
    shared.outbox_wake.notify_one();
    shared.loader_wake.notify_one();

    struct Waiting {
        kind: &'static str,
        reply: Reply,
        deadline: Instant,
        limit: Duration,
    }
    let mut waiting: HashMap<FrameId, Waiting> = HashMap::new();
    let tick_every = (limits.idle.min(limits.call) / 10)
        .clamp(Duration::from_millis(20), Duration::from_secs(1));
    let ping_every = limits.idle / 3;
    let mut tick = tokio::time::interval(tick_every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_seen = Instant::now();
    let mut last_ping = last_seen;
    let mut last_tick = (Instant::now(), SystemTime::now());
    let result = loop {
        tokio::select! {
            incoming = ws.next() => {
                // Any message, a pong too, shows that the hub is there.
                last_seen = Instant::now();
                match incoming {
                    None => break Err(lost("The hub closed the connection.")),
                    Some(Err(e)) => break Err(lost(e.to_string())),
                    Some(Ok(WsMessage::Text(text))) => match Frame::parse(text.as_str()) {
                        Ok(Frame::Event(event)) => {
                            if let Some(end) = on_event(shared, &event, generation) {
                                break Err(end);
                            }
                        }
                        Ok(Frame::Response(response)) => {
                            if let Some(w) = waiting.remove(&response.re) {
                                let _ = w.reply.send(response.result.map_err(Failure::Remote));
                            }
                        }
                        Ok(Frame::Request(_)) => {}
                        Err(e) => break Err(lost(format!("The hub sent a message that this app cannot read: {e}"))),
                    },
                    Some(Ok(WsMessage::Close(_))) => break Err(lost("The hub closed the connection.")),
                    Some(Ok(_)) => {}
                }
            }
            outgoing = rx.recv() => {
                let Some(outgoing) = outgoing else { break Ok(()) };
                let text = match outgoing.frame.to_text() {
                    Ok(text) => text,
                    Err(err) => {
                        // Only this request fails. The link stays up.
                        let _ = outgoing.reply.send(Err(Failure::Remote(encode_failure(&err))));
                        continue;
                    }
                };
                match send_ws(&mut ws, WsMessage::text(text), limits.call).await {
                    Ok(()) => {
                        waiting.insert(outgoing.id, Waiting {
                            kind: outgoing.kind,
                            reply: outgoing.reply,
                            deadline: Instant::now() + outgoing.limit,
                            limit: outgoing.limit,
                        });
                    }
                    Err(end) => {
                        let _ = outgoing.reply.send(Err(Failure::Dropped));
                        break Err(end);
                    }
                }
            }
            _ = tick.tick() => {
                let now = Instant::now();
                let wall = SystemTime::now();
                let timer_passed = now.duration_since(last_tick.0);
                let wall_passed = wall.duration_since(last_tick.1).unwrap_or_default();
                last_tick = (now, wall);
                if slept(timer_passed, wall_passed) {
                    // The connection is not worth checking: it has been
                    // silent for the whole sleep. Start again at once.
                    break Err(lost("The computer was asleep, so Consort connects to the hub again."));
                }
                if shared.generation() != generation {
                    break Err(lost(OWNER_CHANGED));
                }
                if shared.restart.swap(false, std::sync::atomic::Ordering::AcqRel) {
                    break Err(lost("Consort could not load all the changes from the hub. It connects again to load the data"));
                }
                if now.duration_since(last_seen) > limits.idle {
                    break Err(lost(format!("The hub has been silent for {}.", seconds(limits.idle))));
                }
                if now.duration_since(last_ping) >= ping_every {
                    last_ping = now;
                    if let Err(end) = send_ws(&mut ws, WsMessage::Ping(Vec::new().into()), limits.call).await {
                        break Err(end);
                    }
                }
                let expired: Vec<FrameId> = waiting
                    .iter()
                    .filter(|(_, w)| w.deadline <= now)
                    .map(|(id, _)| id.clone())
                    .collect();
                if !expired.is_empty() {
                    let mut waited = limits.call;
                    for id in expired {
                        if let Some(w) = waiting.remove(&id) {
                            tracing::warn!(request = w.kind, "the hub did not answer in time");
                            waited = w.limit;
                            let _ = w.reply.send(Err(Failure::TimedOut));
                        }
                    }
                    // A link that eats requests is not working. Start again.
                    break Err(lost(format!("The hub did not answer a request in {}.", seconds(waited))));
                }
            }
        }
    };
    if let Ok(mut link) = shared.link.lock()
        && shared.generation() == generation
    {
        *link = None;
    }
    for (_, w) in waiting.drain() {
        let _ = w.reply.send(Err(Failure::Dropped));
    }
    result
}

/// Catches up from the feed position, or loads a snapshot, and subscribes.
/// The cache counts as `loaded` only when all of it is done, so a link that
/// drops half way loads again the next time.
async fn initial_sync(
    shared: &Arc<Shared>,
    ws: &mut Ws,
    limit: Duration,
    generation: u64,
    hub_epoch: Option<String>,
) -> Result<(), SessionEnd> {
    // Every write of the cache below checks that the owner is still the one
    // that this link started with.
    let changed = || lost(OWNER_CHANGED);
    let (loaded, seq, same_epoch) = shared
        .with_cache_of(generation, |c| (c.loaded, c.seq, c.hub_epoch == hub_epoch))
        .ok_or_else(changed)?;
    if loaded && !same_epoch {
        // The hub was restored from a backup (or is another database at
        // the same address). Its feed numbers name other changes, so the
        // saved position means nothing there.
        tracing::info!("the hub has a new epoch, so the client loads a snapshot");
    }
    let mut subscribed = false;
    if loaded && same_epoch {
        match handshake_call(
            ws,
            &SyncSubscribe {
                after_seq: seq,
                channel_ids: None,
            },
            limit,
        )
        .await?
        {
            Ok(_) => subscribed = true,
            Err(err) if err.code == ErrorCode::ResyncRequired => {}
            Err(err) => return Err(refused_or_lost(err)),
        }
    }
    if !subscribed {
        let snapshot = handshake_call(
            ws,
            &SyncSnapshot {
                scope: SnapshotScope::All,
            },
            limit,
        )
        .await?
        .map_err(refused_or_lost)?;
        let after = snapshot.seq;
        let epoch = hub_epoch.clone();
        let channels = shared
            .with_cache_of(generation, move |cache| {
                cache.load_snapshot(snapshot);
                cache.hub_epoch = epoch;
                cache.channels_to_load()
            })
            .ok_or_else(changed)?;
        // The `all` snapshot has no messages. Each channel snapshot brings
        // the newest timeline messages of one channel. A channel that the
        // hub cannot answer for is left for the loader to try again.
        for channel_id in channels {
            match handshake_fetch(
                ws,
                &SyncSnapshot {
                    scope: SnapshotScope::Channel(channel_id),
                },
                limit,
            )
            .await
            {
                Ok(channel) => {
                    shared
                        .with_cache_of(generation, |cache| cache.load_channel(channel_id, channel))
                        .ok_or_else(changed)?;
                }
                Err(Fetch::Session(end)) => return Err(end),
                Err(Fetch::Remote(err)) if err.code == ErrorCode::Unauthorized => {
                    return Err(refused_or_lost(err));
                }
                Err(Fetch::Remote(err)) => {
                    tracing::warn!(%channel_id, error = %err, "a channel snapshot failed, so the loader tries again later");
                }
            }
        }
        // Threads that were open before a resync load again.
        let threads: Vec<(String, String)> = shared
            .cache
            .lock()
            .map(|c| {
                c.loaded_threads
                    .iter()
                    .filter_map(|root| {
                        c.messages
                            .get(root)
                            .map(|m| (m.channel_id.to_string(), root.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (channel_id, root) in threads {
            let (Ok(channel_id), Ok(root)) = (channel_id.parse(), root.parse()) else {
                continue;
            };
            let mut pager = ThreadPager::new(channel_id, root);
            while let Some(request) = pager.next_request() {
                match handshake_fetch(ws, &request, limit).await {
                    Ok(page) => {
                        let messages = pager.accept(page);
                        shared
                            .with_cache_of(generation, |cache| cache.merge_messages(messages))
                            .ok_or_else(changed)?;
                    }
                    Err(Fetch::Session(end)) => return Err(end),
                    Err(Fetch::Remote(err)) => {
                        tracing::warn!(%root, error = %err, "a thread failed to load again");
                        break;
                    }
                }
            }
        }
        // Before the subscription: a handshake call drops the feed events
        // that come while it waits.
        load_proactivity(shared, ws, limit, generation).await?;
        handshake_call(
            ws,
            &SyncSubscribe {
                after_seq: after,
                channel_ids: None,
            },
            limit,
        )
        .await?
        .map_err(refused_or_lost)?;
    }
    shared
        .with_cache_of(generation, Cache::finish_load)
        .ok_or_else(changed)?;
    Ok(())
}

/// Loads the newest page of the proactivity log, and gives the hub this
/// computer's time zone when it has none. An older hub has neither, so a
/// refusal is fine.
async fn load_proactivity(
    shared: &Arc<Shared>,
    ws: &mut Ws,
    limit: Duration,
    generation: u64,
) -> Result<(), SessionEnd> {
    let zone_unset = shared
        .cache
        .lock()
        .map(|c| {
            c.proactivity
                .settings
                .as_ref()
                .is_some_and(|s| s.time_zone.is_none())
        })
        .unwrap_or(false);
    if zone_unset {
        let request = consort_proto::client_api::ProactivitySet {
            time_zone: Some(crate::admin::local_time_zone()),
            time_zone_if_unset: true,
            ..Default::default()
        };
        if let Ok(result) = handshake_call(ws, &request, limit).await? {
            shared.with_cache_of(generation, |cache| {
                cache.proactivity.apply_settings(result.settings);
            });
        }
    }
    let page = consort_proto::client_api::ProactivityLog {
        limit: Some(crate::proactivity::LOG_PAGE),
        ..Default::default()
    };
    if let Ok(page) = handshake_call(ws, &page, limit).await? {
        shared.with_cache_of(generation, |cache| {
            cache.proactivity.load_page(page.wakes, page.has_more);
        });
    }
    Ok(())
}

/// Handles one event from the hub.
fn on_event(shared: &Arc<Shared>, event: &EventFrame, generation: u64) -> Option<SessionEnd> {
    match event.kind.as_str() {
        "feed" => on_feed(shared, event, generation),
        "feed.skipped" => {
            match event.decode_as::<FeedSkipped>() {
                Ok(skipped) => recover(shared, skipped.seq, skipped.channel_id, generation),
                Err(err) => {
                    tracing::warn!(error = %err, "a feed.skipped event could not be read");
                }
            }
            None
        }
        _ => None,
    }
}

fn on_feed(shared: &Arc<Shared>, event: &EventFrame, generation: u64) -> Option<SessionEnd> {
    match event.decode_as::<FeedEntry>() {
        Ok(entry) => {
            let channel_changed = matches!(
                &entry.change,
                FeedChange::Upsert(consort_proto::client_api::EntityPayload::Channel(_))
            );
            let Some((changed, outbox_shrank)) = shared.with_cache_of(generation, |c| {
                let before = c.pending.len();
                let changed = c.apply(entry);
                (changed, c.pending.len() < before)
            }) else {
                return Some(lost(OWNER_CHANGED));
            };
            if changed {
                shared.publish();
            }
            if outbox_shrank {
                // A deleted channel took its waiting messages out of the
                // outbox. The callers that wait for them can stop.
                shared.settle_waiters();
            }
            if channel_changed {
                // A new channel, or one that was unarchived, may have no
                // snapshot yet.
                shared.loader_wake.notify_one();
            }
            None
        }
        Err(err) => {
            // Replaying an entry that cannot be read would fail for ever.
            // Move past it, read its `seq` and channel when the body has
            // them, and load that data again.
            let seq = event.body.get("seq").and_then(Value::as_u64);
            let channel = event
                .body
                .get("channel_id")
                .and_then(Value::as_str)
                .and_then(|c| c.parse().ok());
            tracing::warn!(?seq, error = %err, "a feed entry could not be read, so the client skips it");
            match seq {
                Some(seq) => {
                    recover(shared, seq, channel, generation);
                    None
                }
                None => {
                    shared.with_cache_of(generation, |cache| cache.loaded = false);
                    Some(lost(
                        "The hub sent a change that this app cannot read. Consort loads the data again",
                    ))
                }
            }
        }
    }
}

/// Moves the feed position past an entry that is missing, and asks the hub for
/// the data again: the channel when the entry names one, else everything
/// that is not in a channel.
///
/// The live position moves at once, so the next entries apply. The saved
/// position does not: no save runs until the data is in (see
/// `Shared::recovering`). A quit before that keeps the position before the
/// gap in the file, and the next start replays the change.
fn recover(shared: &Arc<Shared>, seq: u64, channel_id: Option<ChannelId>, generation: u64) {
    let skipped = shared.with_cache_of(generation, |cache| cache.skip_to(seq));
    if skipped.is_none() {
        return;
    }
    shared.recovering.fetch_add(1, Ordering::AcqRel);
    let shared = shared.clone();
    tokio::spawn(async move {
        let scope = match channel_id {
            Some(id) => SnapshotScope::Channel(id),
            None => SnapshotScope::All,
        };
        match shared.call(SyncSnapshot { scope }).await {
            Ok(snapshot) => {
                shared.with_cache_of(generation, |cache| match channel_id {
                    Some(id) if cache.channels.contains_key(&id.to_string()) => {
                        cache.load_channel(id, snapshot);
                    }
                    Some(_) => {}
                    None => cache.merge_entities(snapshot),
                });
                shared.recovering.fetch_sub(1, Ordering::AcqRel);
                shared.publish();
                shared.loader_wake.notify_one();
            }
            Err(err) => {
                tracing::warn!(error = %err, "cannot load the data of a skipped change, so the client connects again and loads a snapshot");
                // The cache misses a change. Nothing else retries this while
                // the link stays up, so end the link: the next connection
                // sees that the cache is not loaded and loads everything.
                shared.with_cache_of(generation, |cache| cache.loaded = false);
                shared.recovering.fetch_sub(1, Ordering::AcqRel);
                shared.restart_link();
            }
        }
    });
}

/// The longest wait between two tries to load a channel snapshot or a task.
const LOADER_RETRY_MAX: Duration = Duration::from_secs(60);

/// Loads what the cache misses and the user did not ask for:
/// - the snapshot of every open channel that has none: a channel whose first
///   snapshot failed, one that someone created, and one that someone
///   unarchived,
/// - the tasks from [`Cache::tasks_to_fetch`]: tasks that a task card names
///   and no snapshot held, and previews of tasks with an open thread.
///
/// Tries again with a growing delay.
pub(crate) async fn run_loader(shared: Arc<Shared>) {
    let start = Duration::from_secs(1);
    let mut delay = start;
    loop {
        shared.loader_wake.notified().await;
        loop {
            // The link wakes the loader when it comes up.
            if !shared.is_online() {
                break;
            }
            let (channels, tasks) = shared
                .cache
                .lock()
                .map(|c| (c.channels_to_load(), c.tasks_to_fetch()))
                .unwrap_or_default();
            if channels.is_empty() && tasks.is_empty() {
                delay = start;
                break;
            }
            let mut failed = false;
            for id in channels {
                match shared
                    .call(SyncSnapshot {
                        scope: SnapshotScope::Channel(id),
                    })
                    .await
                {
                    Ok(snapshot) => {
                        if let Ok(mut cache) = shared.cache.lock()
                            && cache.channels.contains_key(&id.to_string())
                        {
                            cache.load_channel(id, snapshot);
                        }
                        shared.publish();
                    }
                    Err(err) => {
                        tracing::warn!(channel_id = %id, error = %err, "cannot load a channel, so the loader tries again");
                        failed = true;
                        if is_transport(&err) {
                            break;
                        }
                    }
                }
            }
            for id in tasks {
                if let Err(err) = shared.fetch_task(id).await {
                    // A task that the hub does not have is remembered, so
                    // this is not a reason to try again.
                    if !matches!(&err, ClientError::Hub { code, .. } if code == "not_found") {
                        tracing::warn!(task_id = %id, error = %err, "cannot load a task, so the loader tries again");
                        failed = true;
                    }
                    if is_transport(&err) {
                        break;
                    }
                }
            }
            if failed {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shared.loader_wake.notified() => {}
                }
                delay = (delay * 2).min(LOADER_RETRY_MAX);
            } else {
                delay = start;
            }
        }
    }
}

/// The delay before reconnect attempt `attempt`: 0.5 s doubling up to 30 s,
/// with up to 25 percent random jitter either way.
pub fn backoff(attempt: u32) -> Duration {
    let base = 500u64.saturating_mul(1u64 << attempt.min(6)).min(30_000);
    let jitter = rand::Rng::gen_range(&mut rand::thread_rng(), -0.25..=0.25);
    Duration::from_millis(((base as f64) * (1.0 + jitter)).max(100.0) as u64)
}

/// How long a link must stay up before the next drop counts as a new
/// problem. A hub that accepts a link and drops it at once keeps the growing
/// delay.
const STABLE_ONLINE: Duration = Duration::from_secs(5);

/// The reconnect attempt counter.
#[derive(Debug, Default)]
pub(super) struct Reconnect {
    attempt: u32,
}

impl Reconnect {
    /// True before the first attempt, and after a stable link.
    pub(super) fn is_fresh(&self) -> bool {
        self.attempt == 0
    }

    /// Call when a session ends. `online_for` is how long the link was
    /// Online, or `None` when it never was.
    pub(super) fn after_session(&mut self, online_for: Option<Duration>) {
        if online_for.is_some_and(|d| d >= STABLE_ONLINE) {
            self.attempt = 0;
        }
    }

    /// The delay before the next attempt.
    pub(super) fn next_delay(&mut self) -> Duration {
        let delay = backoff(self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        delay
    }

    pub(super) fn reset(&mut self) {
        self.attempt = 0;
    }
}

/// Runs the link for as long as the client lives.
pub(crate) async fn run_link(shared: Arc<Shared>) {
    let mut reconnect = Reconnect::default();
    loop {
        let Some(identity) = shared.identity() else {
            shared.wake.notified().await;
            reconnect.reset();
            continue;
        };
        if let Ok(mut cache) = shared.cache.lock() {
            cache.admin.own_url.clone_from(&identity.config.hub_url);
        }
        if reconnect.is_fresh() {
            shared.set_connection(Connection::Connecting);
        }
        let mut online_since = None;
        let result = session(&shared, &identity, &mut online_since).await;
        reconnect.after_session(online_since.map(|since: Instant| since.elapsed()));
        if online_since.is_some() {
            // The data was current until the link ended. The saved data and
            // the offline pill say so.
            shared.mark_fresh();
        }
        // The user unpaired while the session ran.
        if shared.identity().is_none() && !matches!(result, Err(SessionEnd::Refused(_))) {
            shared.set_connection(Connection::NotPaired);
            reconnect.reset();
            continue;
        }
        let reason = match result {
            Err(SessionEnd::Refused(reason)) => {
                // The hub does not trust this client any more, so the saved
                // data of that hub goes too.
                shared.forget_saved();
                shared.set_connection(Connection::Refused {
                    reason: if reason.is_empty() {
                        "The hub does not accept this app any more. Pair it again.".into()
                    } else {
                        reason
                    },
                });
                if let Ok(mut id) = shared.identity.lock() {
                    *id = None;
                }
                reconnect.reset();
                continue;
            }
            Err(SessionEnd::Incompatible(hub)) => {
                if hub.is_some() {
                    shared.set_hub_versions(hub.clone());
                }
                let delay = crate::versions::INCOMPATIBLE_RETRY;
                shared.set_connection(Connection::Incompatible {
                    too_old: crate::versions::too_old(hub.as_ref()),
                    hub,
                    app_version: env!("CARGO_PKG_VERSION").to_owned(),
                    retry_in_ms: delay.as_millis() as u64,
                });
                tokio::select! {
                    _ = sleep_counting_sleep(delay) => {}
                    _ = shared.wake.notified() => { reconnect.reset(); }
                }
                continue;
            }
            Err(SessionEnd::Lost(reason)) => reason,
            Ok(()) => "The link closed.".to_owned(),
        };
        let delay = reconnect.next_delay();
        shared.set_connection(Connection::Offline {
            retry_in_ms: delay.as_millis() as u64,
            reason,
        });
        tokio::select! {
            _ = sleep_counting_sleep(delay) => {}
            _ = shared.wake.notified() => { reconnect.reset(); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_stays_bounded() {
        for attempt in 0..20 {
            let d = backoff(attempt).as_millis();
            assert!((100..=37_500).contains(&d), "{attempt}: {d}");
        }
        assert!(backoff(0) < Duration::from_millis(700));
        assert!(backoff(10) >= Duration::from_millis(22_500));
    }

    /// Review cycle 3, finding 10: the idle check used a clock that does not
    /// count sleep on macOS, so a link read Online for up to a minute after
    /// a wake.
    #[test]
    fn a_wall_clock_that_ran_ahead_of_the_timer_means_the_computer_slept() {
        let tick = Duration::from_secs(1);
        // Normal ticks, with some jitter.
        assert!(!slept(tick, tick));
        assert!(!slept(tick, tick + Duration::from_secs(2)));
        // The computer slept for ten minutes: the timer saw one tick.
        assert!(slept(tick, Duration::from_secs(600)));
        assert!(slept(
            Duration::ZERO,
            SLEEP_SLACK + Duration::from_millis(1)
        ));
        // A wall clock that was set back is not a sleep.
        assert!(!slept(tick, Duration::ZERO));
    }

    #[tokio::test]
    async fn the_offline_wait_ends_at_its_delay() {
        let started = std::time::Instant::now();
        sleep_counting_sleep(Duration::from_millis(60)).await;
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(60), "{took:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        // No wait at all is fine.
        sleep_counting_sleep(Duration::ZERO).await;
    }

    #[test]
    fn the_attempt_counter_resets_after_a_stable_link_and_not_after_a_flap() {
        let mut r = Reconnect::default();
        assert!(r.is_fresh());
        for _ in 0..5 {
            r.next_delay();
        }
        // A link that never came up keeps the growing delay.
        r.after_session(None);
        assert!(!r.is_fresh());
        // A link that came up and dropped at once keeps it too.
        r.after_session(Some(Duration::from_millis(300)));
        assert!(!r.is_fresh());
        assert!(r.next_delay() >= Duration::from_millis(22_500 / 2));
        // A link that stayed up starts over.
        r.after_session(Some(Duration::from_secs(60)));
        assert!(r.is_fresh());
        assert!(r.next_delay() < Duration::from_millis(700));
    }

    fn reply(id: MessageId, at: i64) -> Message {
        Message {
            id,
            channel_id: ChannelId::new(),
            thread_root_id: None,
            author: consort_proto::model::Author::Assistant,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            kind: consort_proto::model::MessageKind::Text,
            body: format!("reply {at}"),
            attachments: vec![],
            in_timeline: false,
            client_message_id: None,
            reply_count: 0,
            last_reply_at: None,
            reactions: vec![],
            edited_at: None,
            deleted_at: None,
            created_at: consort_proto::time::Timestamp::from_millis(at),
            proactive: None,
            suggestions: Vec::new(),
            source_message_id: None,
        }
    }

    /// Finding 23: a thread stopped at its newest 200 replies.
    #[test]
    fn the_thread_pager_asks_for_older_pages_until_the_hub_says_stop() {
        let mut pager = ThreadPager::new(ChannelId::new(), MessageId::new());
        let first = pager.next_request().expect("first request");
        assert_eq!(first.before, None);
        assert_eq!(first.limit, Some(THREAD_PAGE));
        let (old, newer) = (MessageId::new(), MessageId::new());
        // Newest first, as the hub sends a page.
        let got = pager.accept(MessageListResult {
            messages: vec![reply(newer, 20), reply(old, 10)],
            has_more: true,
        });
        assert_eq!(got.len(), 2);
        let second = pager.next_request().expect("second request");
        assert_eq!(second.before, Some(old), "the oldest message of the page");
        pager.accept(MessageListResult {
            messages: vec![reply(MessageId::new(), 5)],
            has_more: false,
        });
        assert!(pager.next_request().is_none());
    }

    #[test]
    fn the_thread_pager_stops_when_the_hub_makes_no_progress() {
        let mut pager = ThreadPager::new(ChannelId::new(), MessageId::new());
        let stuck = MessageId::new();
        for _ in 0..3 {
            if pager.next_request().is_none() {
                return;
            }
            pager.accept(MessageListResult {
                messages: vec![reply(stuck, 10)],
                has_more: true,
            });
        }
        assert!(
            pager.next_request().is_none(),
            "the same page twice ends it"
        );
    }

    #[test]
    fn the_thread_pager_stops_on_an_empty_page() {
        let mut pager = ThreadPager::new(ChannelId::new(), MessageId::new());
        pager.accept(MessageListResult {
            messages: vec![],
            has_more: true,
        });
        assert!(pager.next_request().is_none());
    }
}
