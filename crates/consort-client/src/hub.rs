//! The hub backend: pairing over HTTP, the authenticated WebSocket, sync,
//! reconnects with jittered backoff, and the commands as client requests.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use consort_proto::{
    auth::{
        AuthChallenge, AuthResponse, CLIENT_WS_PATH, ClientHello, Nonce, PAIR_CLIENT_PATH,
        PairClientRequest, PairClientResponse, PairingCode, PeerId, PublicKey, Signature,
        auth_signing_payload, normalize_origin,
    },
    client_api::{
        ChannelArchive, ChannelCreate, ChannelDelete, ChannelMove, ChannelUpdate,
        DecisionAnswerRequest, FeedEntry, GroupUpdate, MessageDelete, MessageEdit, MessageSend,
        ReadMark, SnapshotScope, SyncSnapshot, SyncSubscribe, TaskInterrupt, TaskReopen,
        TaskResolve, TaskRetry, WorkerPairingCode, WorkerRename, WorkerRevoke, WorkerSetLabels,
        WorkerSetPaused,
    },
    error::{ErrorBody, ErrorCode},
    frame::{Frame, Request, SUPPORTED_PROTOCOL_VERSIONS},
    ids::FrameId,
    model::{ClientKind, DecisionAnswer, ModelSelection, RequestOptionKind},
};
use ed25519_dalek::{Signer as _, SigningKey};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::Value;
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::{
    Backend, ClientError, Command,
    cache::{Cache, PendingSend},
    format,
    reduce::view_state,
    store::{ClientConfig, Identity, Store},
    view::{ApprovalChoice, Connection, PairingCodeView, UserView, ViewState},
};

/// Who this client is, for pairing.
#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub name: String,
    pub kind: ClientKind,
    pub user: UserView,
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
        }
    }
}

/// Where the response to one request goes.
type Reply = oneshot::Sender<Result<Value, ErrorBody>>;

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
    wake: Notify,
}

impl Shared {
    pub(crate) fn new(
        store: Option<Store>,
        options: ClientOptions,
        publish: watch::Sender<Arc<ViewState>>,
    ) -> Result<Arc<Shared>, ClientError> {
        let identity = match &store {
            Some(store) => store.load()?,
            None => None,
        };
        let connection = if identity.is_some() {
            Connection::Connecting
        } else {
            Connection::NotPaired
        };
        let shared = Arc::new(Shared {
            store,
            options,
            cache: Mutex::new(Cache::default()),
            identity: Mutex::new(identity),
            connection: Mutex::new(connection),
            pairing: Mutex::new(None),
            link: Mutex::new(None),
            publish,
            wake: Notify::new(),
        });
        shared.publish();
        Ok(shared)
    }

    fn set_connection(&self, connection: Connection) {
        if let Ok(mut c) = self.connection.lock() {
            *c = connection;
        }
        self.publish();
    }

    /// Builds the view state from the cache and sends it to subscribers.
    pub(crate) fn publish(&self) {
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
        state.worker_pairing = pairing;
        let _ = self.publish.send(Arc::new(state));
    }

    fn identity(&self) -> Option<Identity> {
        self.identity.lock().ok().and_then(|i| i.clone())
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
        let value = rx
            .await
            .map_err(|_| ClientError::NotConnected)?
            .map_err(hub_error)?;
        serde_json::from_value(value).map_err(|e| {
            ClientError::Other(format!(
                "The hub sent a response that this app cannot read: {e}"
            ))
        })
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

/// The hub URL as the user typed it, made complete: a missing scheme is
/// `http` for this machine and `https` for any other host.
pub fn normalize_hub_url(input: &str) -> Result<String, ClientError> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(ClientError::InvalidUrl("it is empty".into()));
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        let host = trimmed.split([':', '/']).next().unwrap_or_default();
        let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
        format!("{}://{trimmed}", if local { "http" } else { "https" })
    };
    let url = url::Url::parse(&with_scheme).map_err(|e| ClientError::InvalidUrl(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ClientError::InvalidUrl(format!(
            "{with_scheme} is not an http or https address"
        )));
    }
    Ok(with_scheme)
}

fn ws_url(hub_url: &str) -> Result<String, ClientError> {
    let mut url = url::Url::parse(hub_url).map_err(|e| ClientError::InvalidUrl(e.to_string()))?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme)
        .map_err(|_| ClientError::InvalidUrl(hub_url.to_owned()))?;
    url.set_path(CLIENT_WS_PATH);
    Ok(url.to_string())
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
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
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
            "The hub answered the pairing in a form this app cannot read: {e}"
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

/// Why a session on the link ended.
enum SessionEnd {
    /// The hub refused this client. It must pair again.
    Refused(String),
    /// The link dropped or failed. Try again later.
    Lost(String),
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn read_frame(ws: &mut Ws) -> Result<Frame, SessionEnd> {
    loop {
        let message = ws
            .next()
            .await
            .ok_or_else(|| SessionEnd::Lost("The hub closed the connection.".into()))?
            .map_err(|e| SessionEnd::Lost(e.to_string()))?;
        match message {
            WsMessage::Text(text) => {
                return Frame::parse(text.as_str()).map_err(|e| SessionEnd::Lost(e.to_string()));
            }
            WsMessage::Close(_) => {
                return Err(SessionEnd::Lost("The hub closed the connection.".into()));
            }
            _ => continue,
        }
    }
}

async fn send_frame(ws: &mut Ws, frame: &Frame) -> Result<(), SessionEnd> {
    let text = frame
        .to_text()
        .map_err(|e| SessionEnd::Lost(e.to_string()))?;
    ws.send(WsMessage::text(text))
        .await
        .map_err(|e| SessionEnd::Lost(e.to_string()))
}

/// One request during the handshake, before the main loop runs.
async fn handshake_call<R: Request>(
    ws: &mut Ws,
    body: &R,
) -> Result<Result<R::Response, ErrorBody>, SessionEnd> {
    let id = FrameId::generate();
    let frame = Frame::request(id.clone(), body).map_err(|e| SessionEnd::Lost(e.to_string()))?;
    send_frame(ws, &frame).await?;
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
}

fn refused_or_lost(err: ErrorBody) -> SessionEnd {
    match err.code {
        ErrorCode::Unauthorized => SessionEnd::Refused(err.message),
        _ => SessionEnd::Lost(err.message),
    }
}

async fn session(shared: &Arc<Shared>, identity: &Identity) -> Result<(), SessionEnd> {
    let url = ws_url(&identity.config.hub_url).map_err(|e| SessionEnd::Lost(e.to_string()))?;
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| SessionEnd::Lost(format!("Cannot reach the hub: {e}")))?;

    // auth.challenge, then auth.response.
    let challenge = match read_frame(&mut ws).await? {
        Frame::Event(event) if event.kind == "auth.challenge" => event
            .decode_as::<AuthChallenge>()
            .map_err(|e| SessionEnd::Lost(e.to_string()))?,
        _ => return Err(SessionEnd::Lost("The hub did not send a challenge.".into())),
    };
    let origin = normalize_origin(&identity.config.hub_url).map_err(|e| SessionEnd::Lost(e.0))?;
    let signature = identity
        .key
        .sign(&auth_signing_payload(&origin, &challenge.nonce));
    let auth = AuthResponse {
        peer_id: PeerId::Client(identity.config.client_id),
        signature: Signature::from_bytes(signature.to_bytes()),
    };
    handshake_call(&mut ws, &auth)
        .await?
        .map_err(refused_or_lost)?;
    let hello = ClientHello {
        client_id: identity.config.client_id,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
    };
    handshake_call(&mut ws, &hello)
        .await?
        .map_err(refused_or_lost)?;

    // Catch up from the feed position, or load a snapshot.
    let (loaded, seq) = shared
        .cache
        .lock()
        .map(|c| (c.loaded, c.seq))
        .unwrap_or((false, 0));
    let mut subscribed = false;
    if loaded {
        match handshake_call(
            &mut ws,
            &SyncSubscribe {
                after_seq: seq,
                channel_ids: None,
            },
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
            &mut ws,
            &SyncSnapshot {
                scope: SnapshotScope::All,
            },
        )
        .await?
        .map_err(refused_or_lost)?;
        let after = snapshot.seq;
        if let Ok(mut cache) = shared.cache.lock() {
            cache.load_snapshot(snapshot);
        }
        handshake_call(
            &mut ws,
            &SyncSubscribe {
                after_seq: after,
                channel_ids: None,
            },
        )
        .await?
        .map_err(refused_or_lost)?;
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<Outgoing>();
    if let Ok(mut link) = shared.link.lock() {
        *link = Some(tx);
    }
    shared.set_connection(Connection::Online);
    resend_pending(shared);

    let mut waiting: HashMap<FrameId, (&'static str, Reply)> = HashMap::new();
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    ping.tick().await;
    let result = loop {
        tokio::select! {
            frame = read_frame(&mut ws) => {
                match frame {
                    Ok(Frame::Event(event)) if event.kind == "feed" => {
                        match event.decode_as::<FeedEntry>() {
                            Ok(entry) => {
                                let changed = shared.cache.lock().map(|mut c| c.apply(entry)).unwrap_or(false);
                                if changed {
                                    shared.publish();
                                }
                            }
                            Err(e) => break Err(SessionEnd::Lost(format!("A feed entry could not be read: {e}"))),
                        }
                    }
                    Ok(Frame::Response(response)) => {
                        if let Some((_, reply)) = waiting.remove(&response.re) {
                            let _ = reply.send(response.result);
                        }
                    }
                    Ok(_) => {}
                    Err(end) => break Err(end),
                }
            }
            outgoing = rx.recv() => {
                let Some(outgoing) = outgoing else { break Ok(()) };
                if let Err(end) = send_frame(&mut ws, &outgoing.frame).await {
                    let _ = outgoing.reply.send(Err(ErrorBody::new(ErrorCode::Internal, "The connection dropped.")));
                    break Err(end);
                }
                waiting.insert(outgoing.id, (outgoing.kind, outgoing.reply));
            }
            _ = ping.tick() => {
                if ws.send(WsMessage::Ping(Vec::new().into())).await.is_err() {
                    break Err(SessionEnd::Lost("The connection dropped.".into()));
                }
            }
        }
    };
    if let Ok(mut link) = shared.link.lock() {
        *link = None;
    }
    for (_, (_, reply)) in waiting.drain() {
        let _ = reply.send(Err(ErrorBody::new(
            ErrorCode::Internal,
            "The connection dropped.",
        )));
    }
    result
}

/// Sends again every message that the hub has not stored. The hub keeps the
/// first copy of a `client_message_id`, so nothing doubles.
fn resend_pending(shared: &Arc<Shared>) {
    let pending = shared
        .cache
        .lock()
        .map(|c| c.pending.clone())
        .unwrap_or_default();
    for p in pending {
        let shared = shared.clone();
        tokio::spawn(async move {
            let _ = send_pending(&shared, &p).await;
        });
    }
}

async fn send_pending(shared: &Arc<Shared>, p: &PendingSend) -> Result<(), ClientError> {
    let body = MessageSend {
        channel_id: p
            .channel_id
            .parse()
            .map_err(|_| invalid("The channel ID is not valid."))?,
        client_message_id: p.client_message_id.clone(),
        body: p.body.clone(),
        attachments: Vec::new(),
        thread_root_id: match &p.thread_root_id {
            Some(id) => Some(
                id.parse()
                    .map_err(|_| invalid("The thread ID is not valid."))?,
            ),
            None => None,
        },
        in_timeline: false,
    };
    let result = shared.call(body).await?;
    if let Ok(mut cache) = shared.cache.lock() {
        cache
            .pending
            .retain(|x| x.client_message_id != p.client_message_id);
        cache
            .messages
            .insert(result.message.id.to_string(), result.message);
    }
    shared.publish();
    Ok(())
}

/// The delay before reconnect attempt `attempt`: 0.5 s doubling up to 30 s,
/// with up to 25 percent random jitter either way.
pub fn backoff(attempt: u32) -> Duration {
    let base = 500u64.saturating_mul(1u64 << attempt.min(6)).min(30_000);
    let jitter = rand::Rng::gen_range(&mut rand::thread_rng(), -0.25..=0.25);
    Duration::from_millis(((base as f64) * (1.0 + jitter)).max(100.0) as u64)
}

/// Runs the link for as long as the client lives.
pub(crate) async fn run_link(shared: Arc<Shared>) {
    let mut attempt = 0u32;
    loop {
        let Some(identity) = shared.identity() else {
            shared.wake.notified().await;
            continue;
        };
        if attempt == 0 {
            shared.set_connection(Connection::Connecting);
        }
        match session(&shared, &identity).await {
            Ok(()) => attempt = 0,
            Err(SessionEnd::Refused(reason)) => {
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
                attempt = 0;
                continue;
            }
            Err(SessionEnd::Lost(reason)) => {
                let delay = backoff(attempt);
                attempt = attempt.saturating_add(1);
                shared.set_connection(Connection::Offline {
                    retry_in_ms: delay.as_millis() as u64,
                    reason,
                });
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shared.wake.notified() => { attempt = 0; }
                }
            }
        }
    }
}

pub(crate) struct HubBackend {
    pub(crate) shared: Arc<Shared>,
}

fn parse<T: std::str::FromStr>(id: &str, what: &str) -> Result<T, ClientError> {
    id.parse()
        .map_err(|_| ClientError::Other(format!("The {what} ID {id} is not valid.")))
}

fn selection(label: &Option<String>) -> Option<ModelSelection> {
    label.as_ref().map(|l| {
        let (account, model) = l.split_once(", ").unwrap_or((l.as_str(), ""));
        ModelSelection {
            account_label: account.to_owned(),
            model: model.to_owned(),
            options: Default::default(),
        }
    })
}

impl HubBackend {
    async fn apply(&mut self, command: Command) -> Result<(), ClientError> {
        let shared = self.shared.clone();
        match command {
            Command::Pair { hub_url, code } => {
                let identity = pair(&hub_url, &code, &shared.options).await?;
                if let Some(store) = &shared.store {
                    store.save(&identity)?;
                }
                if let Ok(mut i) = shared.identity.lock() {
                    *i = Some(identity);
                }
                if let Ok(mut cache) = shared.cache.lock() {
                    *cache = Cache::default();
                }
                shared.set_connection(Connection::Connecting);
                shared.wake.notify_one();
            }
            Command::SendMessage {
                channel_id,
                body,
                client_message_id,
            } => {
                let pending = PendingSend {
                    client_message_id,
                    channel_id,
                    thread_root_id: None,
                    body,
                    created_at: format::now_ms(),
                };
                return queue_send(&shared, pending).await;
            }
            Command::ReplyToTask {
                task_id,
                body,
                client_message_id,
            } => {
                let (channel_id, root) = shared
                    .cache
                    .lock()
                    .ok()
                    .and_then(|c| {
                        c.tasks
                            .get(&task_id)
                            .map(|t| (t.channel_id.to_string(), t.card_message_id.to_string()))
                    })
                    .ok_or_else(|| invalid("That task no longer exists."))?;
                let pending = PendingSend {
                    client_message_id,
                    channel_id,
                    thread_root_id: Some(root),
                    body,
                    created_at: format::now_ms(),
                };
                return queue_send(&shared, pending).await;
            }
            Command::EditMessage { message_id, body } => {
                shared
                    .call(MessageEdit {
                        message_id: parse(&message_id, "message")?,
                        body,
                    })
                    .await?;
            }
            Command::DeleteMessage { message_id } => {
                shared
                    .call(MessageDelete {
                        message_id: parse(&message_id, "message")?,
                    })
                    .await?;
            }
            Command::CreateChannel(new) => {
                let created = shared
                    .call(ChannelCreate {
                        name: new.name.trim().to_owned(),
                        purpose: new.purpose,
                        instructions: String::new(),
                        icon: None,
                        color: None,
                        group_id: match &new.group_id {
                            Some(g) => Some(parse(g, "group")?),
                            None => None,
                        },
                        coordinator_selection: None,
                        task_selection: selection(&new.task_selection),
                        repo_ids: Vec::new(),
                        default_isolation: Some(match new.isolation {
                            crate::view::Isolation::InPlace => {
                                consort_proto::model::Isolation::InPlace
                            }
                            crate::view::Isolation::Worktree => {
                                consort_proto::model::Isolation::Worktree
                            }
                            crate::view::Isolation::Clone => consort_proto::model::Isolation::Clone,
                        }),
                        default_approval_mode: None,
                        memory_enabled: None,
                    })
                    .await?;
                if !new.preferred_workers.is_empty() {
                    let workers = new
                        .preferred_workers
                        .iter()
                        .map(|w| parse(w, "worker"))
                        .collect::<Result<Vec<consort_proto::ids::WorkerId>, _>>()?;
                    let update: ChannelUpdate = serde_json::from_value(serde_json::json!({
                        "channel_id": created.channel.id,
                        "preferred_workers": workers,
                    }))
                    .map_err(|e| ClientError::Other(e.to_string()))?;
                    shared.call(update).await?;
                }
            }
            Command::UpdateChannel {
                channel_id,
                changes,
            } => {
                let id: consort_proto::ids::ChannelId = parse(&channel_id, "channel")?;
                let mut patch = serde_json::json!({ "channel_id": id });
                if let Some(name) = changes.name {
                    patch["name"] = name.into();
                }
                if let Some(purpose) = changes.purpose {
                    patch["purpose"] = purpose.into();
                }
                if let Some(instructions) = changes.instructions {
                    patch["instructions"] = instructions.into();
                }
                if let Some(icon) = changes.icon {
                    patch["icon"] = icon.into();
                }
                if patch.as_object().is_some_and(|o| o.len() > 1) {
                    let update: ChannelUpdate = serde_json::from_value(patch)
                        .map_err(|e| ClientError::Other(e.to_string()))?;
                    shared.call(update).await?;
                }
                if let Some(group) = changes.group_id {
                    shared
                        .call(ChannelMove {
                            channel_id: id,
                            group_id: Some(parse(&group, "group")?),
                            position: 0,
                        })
                        .await?;
                }
            }
            Command::ArchiveChannel { channel_id } => {
                shared
                    .call(ChannelArchive {
                        channel_id: parse(&channel_id, "channel")?,
                        archived: true,
                    })
                    .await?;
            }
            Command::DeleteChannel { channel_id } => {
                shared
                    .call(ChannelDelete {
                        channel_id: parse(&channel_id, "channel")?,
                    })
                    .await?;
            }
            Command::SetGroupCollapsed {
                group_id,
                collapsed,
            } => {
                shared
                    .call(GroupUpdate {
                        group_id: parse(&group_id, "group")?,
                        name: None,
                        collapsed: Some(collapsed),
                    })
                    .await?;
            }
            Command::MarkRead { channel_id } => {
                let last = shared.cache.lock().ok().and_then(|c| {
                    c.messages
                        .values()
                        .filter(|m| m.channel_id.to_string() == channel_id && m.shows_in_timeline())
                        .max_by_key(|m| m.created_at)
                        .map(|m| m.id)
                });
                if let Some(last) = last {
                    // A read marker is a nicety: offline, it waits for the next open.
                    let _ = shared
                        .call(ReadMark {
                            channel_id: parse(&channel_id, "channel")?,
                            thread_root_id: None,
                            last_read_message_id: last,
                        })
                        .await;
                }
            }
            Command::ResolveTask { task_id } => {
                shared
                    .call(TaskResolve {
                        task_id: parse(&task_id, "task")?,
                    })
                    .await?;
            }
            Command::ReopenTask { task_id } => {
                shared
                    .call(TaskReopen {
                        task_id: parse(&task_id, "task")?,
                    })
                    .await?;
            }
            Command::RetryTask { task_id } => {
                shared
                    .call(TaskRetry {
                        task_id: parse(&task_id, "task")?,
                    })
                    .await?;
            }
            Command::StopTask { task_id } => {
                shared
                    .call(TaskInterrupt {
                        task_id: parse(&task_id, "task")?,
                    })
                    .await?;
            }
            Command::AnswerApproval {
                approval_id,
                choice,
            } => {
                let option_id = shared
                    .cache
                    .lock()
                    .ok()
                    .and_then(|c| c.approvals.get(&approval_id).cloned())
                    .and_then(|a| {
                        let wanted: &[RequestOptionKind] = match choice {
                            ApprovalChoice::AllowOnce => &[RequestOptionKind::AllowOnce],
                            ApprovalChoice::AllowForTask => &[RequestOptionKind::AllowAlways],
                            ApprovalChoice::Deny => {
                                &[RequestOptionKind::DenyOnce, RequestOptionKind::DenyAlways]
                            }
                        };
                        a.options
                            .iter()
                            .find(|o| wanted.contains(&o.kind))
                            .map(|o| o.id.clone())
                    })
                    .ok_or_else(|| {
                        invalid("This approval has no such answer. It may have closed.")
                    })?;
                shared
                    .call(consort_proto::client_api::ApprovalAnswer {
                        approval_id: parse(&approval_id, "approval")?,
                        option_id,
                    })
                    .await?;
            }
            Command::AnswerDecision {
                decision_id,
                option,
                text,
            } => {
                shared
                    .call(DecisionAnswerRequest {
                        decision_id: parse(&decision_id, "decision")?,
                        answer: DecisionAnswer {
                            option_index: option.and_then(|o| u8::try_from(o).ok()),
                            text,
                        },
                    })
                    .await?;
            }
            Command::WorkerPairingCode => {
                let result = shared.call(WorkerPairingCode {}).await?;
                let hub_url = shared
                    .identity()
                    .map(|i| i.config.hub_url)
                    .unwrap_or_default();
                if let Ok(mut p) = shared.pairing.lock() {
                    *p = Some(PairingCodeView {
                        code: result.code.to_string(),
                        hub_url,
                        expires_at: result.expires_at.as_millis(),
                    });
                }
                shared.publish();
            }
            Command::CancelWorkerPairing => {
                if let Ok(mut p) = shared.pairing.lock() {
                    *p = None;
                }
                shared.publish();
            }
            Command::RenameWorker { worker_id, name } => {
                shared
                    .call(WorkerRename {
                        worker_id: parse(&worker_id, "worker")?,
                        name,
                    })
                    .await?;
            }
            Command::SetWorkerLabels { worker_id, labels } => {
                shared
                    .call(WorkerSetLabels {
                        worker_id: parse(&worker_id, "worker")?,
                        labels,
                    })
                    .await?;
            }
            Command::SetWorkerPaused { worker_id, paused } => {
                shared
                    .call(WorkerSetPaused {
                        worker_id: parse(&worker_id, "worker")?,
                        paused,
                    })
                    .await?;
            }
            Command::RemoveWorker { worker_id } => {
                shared
                    .call(WorkerRevoke {
                        worker_id: parse(&worker_id, "worker")?,
                    })
                    .await?;
            }
        }
        Ok(())
    }
}

/// Shows a message at once as pending, and sends it when the link is up.
async fn queue_send(shared: &Arc<Shared>, pending: PendingSend) -> Result<(), ClientError> {
    if let Ok(mut cache) = shared.cache.lock() {
        cache.pending.push(pending.clone());
    }
    shared.publish();
    match send_pending(shared, &pending).await {
        // Offline: the message stays pending and goes out on reconnect.
        Err(ClientError::NotConnected) => Ok(()),
        Err(err) => {
            if let Ok(mut cache) = shared.cache.lock() {
                cache
                    .pending
                    .retain(|p| p.client_message_id != pending.client_message_id);
            }
            shared.publish();
            Err(err)
        }
        Ok(()) => Ok(()),
    }
}

impl Backend for HubBackend {
    async fn handle(
        &mut self,
        command: Command,
        _publish: &watch::Sender<Arc<ViewState>>,
    ) -> Result<(), ClientError> {
        self.apply(command).await
    }
}

/// Keeps the `Nonce` import in use for docs on every platform.
#[allow(dead_code)]
fn _nonce(_: Nonce) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_urls_get_a_scheme() {
        assert_eq!(
            normalize_hub_url("127.0.0.1:7600").unwrap(),
            "http://127.0.0.1:7600"
        );
        assert_eq!(
            normalize_hub_url("hub.example.com/").unwrap(),
            "https://hub.example.com"
        );
        assert_eq!(
            normalize_hub_url("https://hub.example.com").unwrap(),
            "https://hub.example.com"
        );
        assert!(normalize_hub_url("").is_err());
        assert!(normalize_hub_url("ftp://x").is_err());
    }

    #[test]
    fn websocket_urls_follow_the_hub_scheme() {
        assert_eq!(
            ws_url("http://127.0.0.1:7600").unwrap(),
            "ws://127.0.0.1:7600/v1/client"
        );
        assert_eq!(
            ws_url("https://hub.example.com").unwrap(),
            "wss://hub.example.com/v1/client"
        );
    }

    #[test]
    fn backoff_grows_and_stays_bounded() {
        for attempt in 0..20 {
            let d = backoff(attempt).as_millis();
            assert!((100..=37_500).contains(&d), "{attempt}: {d}");
        }
        assert!(backoff(0) < Duration::from_millis(700));
        assert!(backoff(10) >= Duration::from_millis(22_500));
    }
}
