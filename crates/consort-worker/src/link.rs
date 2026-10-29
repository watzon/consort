//! The worker link: one WebSocket to the hub, with challenge
//! authentication, `hello`, request and response matching, pings and
//! reconnects with jittered backoff.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use consort_proto::auth::{
    AuthChallenge, AuthResponse, PeerId, WorkerHello, WorkerHelloResult, auth_signing_payload,
};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{
    Frame, FrameId, Message as ProtoMessage, Request, ResponseError, ResponseFrame,
    SUPPORTED_PROTOCOL_VERSIONS,
};
use consort_proto::worker_link::PING_INTERVAL_SECS;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;

use crate::WorkerError;

/// How long the worker waits for the hub to answer a request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// How long the worker waits for the challenge and the `hello` answer.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/// The shortest and longest wait between reconnects.
const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

type Pending = HashMap<FrameId, oneshot::Sender<ResponseFrame>>;

struct Inner {
    out: Mutex<Option<mpsc::UnboundedSender<String>>>,
    pending: Mutex<Pending>,
    connected: watch::Sender<bool>,
}

/// The sending side of the worker link. Clones share one connection.
#[derive(Clone)]
pub struct Link {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link")
            .field("connected", &self.is_connected())
            .finish()
    }
}

impl Default for Link {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl Link {
    /// A link with no connection.
    pub fn new() -> Self {
        let (connected, _) = watch::channel(false);
        Self {
            inner: Arc::new(Inner {
                out: Mutex::new(None),
                pending: Mutex::new(HashMap::new()),
                connected,
            }),
        }
    }

    /// Whether the worker is connected and past `hello`.
    pub fn is_connected(&self) -> bool {
        *self.inner.connected.borrow()
    }

    /// Watches the connection state.
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.inner.connected.subscribe()
    }

    fn attach(&self, out: mpsc::UnboundedSender<String>) {
        *lock(&self.inner.out) = Some(out);
        self.inner.connected.send_replace(true);
    }

    fn detach(&self) {
        *lock(&self.inner.out) = None;
        self.inner.connected.send_replace(false);
        // Dropping the senders fails every request that waits.
        lock(&self.inner.pending).clear();
    }

    /// Sends a frame.
    pub fn send_frame(&self, frame: &Frame) -> Result<(), WorkerError> {
        let text = frame
            .to_text()
            .map_err(|e| WorkerError::Network(e.to_string()))?;
        let out = lock(&self.inner.out);
        match out.as_ref() {
            Some(tx) => tx.send(text).map_err(|_| WorkerError::Offline),
            None => Err(WorkerError::Offline),
        }
    }

    /// Sends a message that needs no response.
    pub fn send_event<M: ProtoMessage>(&self, message: &M) -> Result<(), WorkerError> {
        let frame = Frame::event(message).map_err(|e| WorkerError::Network(e.to_string()))?;
        self.send_frame(&frame)
    }

    /// Sends a request and waits for the response.
    pub async fn request<R: Request>(&self, body: &R) -> Result<R::Response, WorkerError> {
        let id = FrameId::generate();
        let frame =
            Frame::request(id.clone(), body).map_err(|e| WorkerError::Network(e.to_string()))?;
        let (tx, rx) = oneshot::channel();
        lock(&self.inner.pending).insert(id.clone(), tx);
        if let Err(err) = self.send_frame(&frame) {
            lock(&self.inner.pending).remove(&id);
            return Err(err);
        }
        let response = match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => return Err(WorkerError::Offline),
            Err(_) => {
                lock(&self.inner.pending).remove(&id);
                return Err(WorkerError::Network(format!(
                    "the hub did not answer `{}` in time",
                    R::TYPE
                )));
            }
        };
        response.into_response::<R>().map_err(|e| match e {
            ResponseError::Remote(body) => WorkerError::Hub {
                code: body.code,
                message: body.message,
            },
            ResponseError::InvalidBody(err) => WorkerError::Network(format!(
                "the hub sent an invalid `{}` response: {err}",
                R::TYPE
            )),
        })
    }

    fn resolve(&self, response: ResponseFrame) {
        if let Some(tx) = lock(&self.inner.pending).remove(&response.re) {
            let _ = tx.send(response);
        }
    }
}

/// What the link needs from the worker.
pub trait LinkHandler: Send + Sync + 'static {
    /// The worker ID.
    fn worker_id(&self) -> consort_proto::ids::WorkerId;
    /// The hub URL.
    fn hub_url(&self) -> &str;
    /// Signs the challenge payload.
    fn sign(&self, payload: &[u8]) -> consort_proto::auth::Signature;
    /// The body of `hello`.
    fn hello(&self) -> WorkerHello;
    /// Runs after `hello`, with the hub's answer.
    fn connected(self: Arc<Self>, result: WorkerHelloResult);
    /// Handles a hub command.
    fn command(self: Arc<Self>, request: consort_proto::frame::RequestFrame);
}

fn jitter(base: Duration) -> Duration {
    let ms = u64::try_from(base.as_millis()).unwrap_or(u64::MAX);
    let low = ms / 2;
    Duration::from_millis(rand::thread_rng().gen_range(low..=ms.max(low + 1)))
}

/// Keeps the link connected until `shutdown` turns true.
pub async fn maintain<H: LinkHandler>(
    link: Link,
    handler: Arc<H>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut backoff = BACKOFF_MIN;
    loop {
        if *shutdown.borrow() {
            break;
        }
        let started = Instant::now();
        let result = tokio::select! {
            r = connect_once(&link, handler.clone()) => r,
            _ = shutdown.changed() => { link.detach(); break; }
        };
        link.detach();
        match &result {
            Ok(()) => tracing::info!("the hub closed the worker link"),
            Err(err) => tracing::warn!(error = %err, "the worker link failed"),
        }
        if started.elapsed() > Duration::from_secs(30) {
            backoff = BACKOFF_MIN;
        }
        if matches!(
            result,
            Err(WorkerError::Hub {
                code: ErrorCode::Unauthorized,
                ..
            })
        ) {
            backoff = BACKOFF_MAX;
        }
        let delay = jitter(backoff);
        backoff = (backoff * 2).min(BACKOFF_MAX);
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            _ = shutdown.changed() => break,
        }
    }
}

type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

async fn next_frame(stream: &mut WsStream) -> Result<Frame, WorkerError> {
    loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => {
                return Frame::parse(text.as_str())
                    .map_err(|e| WorkerError::Network(e.to_string()));
            }
            Some(Ok(Message::Close(_))) | None => {
                return Err(WorkerError::Network(
                    "the hub closed the connection".to_owned(),
                ));
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(WorkerError::Network(e.to_string())),
        }
    }
}

async fn await_response(stream: &mut WsStream, id: &FrameId) -> Result<Value, WorkerError> {
    loop {
        if let Frame::Response(r) = next_frame(stream).await?
            && &r.re == id
        {
            return r.result.map_err(|e: ErrorBody| WorkerError::Hub {
                code: e.code,
                message: e.message,
            });
        }
    }
}

async fn connect_once<H: LinkHandler>(link: &Link, handler: Arc<H>) -> Result<(), WorkerError> {
    let url = crate::http::worker_ws_url(handler.hub_url())?;
    let origin = consort_proto::auth::normalize_origin(handler.hub_url())
        .map_err(|e| WorkerError::Config(e.to_string()))?;
    let (ws, _) = tokio::time::timeout(HANDSHAKE_TIMEOUT, tokio_tungstenite::connect_async(&url))
        .await
        .map_err(|_| WorkerError::Network(format!("{url} did not answer")))?
        .map_err(|e| WorkerError::Network(format!("{url}: {e}")))?;
    let (mut sink, mut stream) = ws.split();

    let handshake = async {
        let challenge: AuthChallenge = loop {
            if let Frame::Event(e) = next_frame(&mut stream).await?
                && e.kind == AuthChallenge::TYPE
            {
                break e
                    .decode_as()
                    .map_err(|e| WorkerError::Network(e.to_string()))?;
            }
        };
        let response = AuthResponse {
            peer_id: PeerId::Worker(handler.worker_id()),
            signature: handler.sign(&auth_signing_payload(&origin, &challenge.nonce)),
        };
        let id = FrameId::generate();
        let text = Frame::request(id.clone(), &response)
            .and_then(|f| f.to_text())
            .map_err(|e| WorkerError::Network(e.to_string()))?;
        sink.send(Message::Text(text.into()))
            .await
            .map_err(|e| WorkerError::Network(e.to_string()))?;
        await_response(&mut stream, &id).await?;

        let id = FrameId::generate();
        let text = Frame::request(id.clone(), &handler.hello())
            .and_then(|f| f.to_text())
            .map_err(|e| WorkerError::Network(e.to_string()))?;
        sink.send(Message::Text(text.into()))
            .await
            .map_err(|e| WorkerError::Network(e.to_string()))?;
        let body = await_response(&mut stream, &id).await?;
        serde_json::from_value::<WorkerHelloResult>(body)
            .map_err(|e| WorkerError::Network(format!("the hello answer is not valid: {e}")))
    };
    let hello = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake)
        .await
        .map_err(|_| WorkerError::Network("the hub did not finish the handshake".to_owned()))??;
    if !SUPPORTED_PROTOCOL_VERSIONS.contains(&hello.protocol_version) {
        return Err(WorkerError::Hub {
            code: ErrorCode::ProtocolMismatch,
            message: format!("the hub picked protocol version {}", hello.protocol_version),
        });
    }
    tracing::info!(hub = handler.hub_url(), "connected to the hub");

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(PING_INTERVAL_SECS));
        ping.tick().await;
        loop {
            tokio::select! {
                text = rx.recv() => {
                    let Some(text) = text else { break };
                    if sink.send(Message::Text(text.into())).await.is_err() { break; }
                }
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
                }
            }
        }
        let _ = sink.close().await;
    });
    link.attach(tx);
    handler.clone().connected(hello);

    let result = loop {
        match next_frame(&mut stream).await {
            Ok(Frame::Request(request)) => handler.clone().command(request),
            Ok(Frame::Response(response)) => link.resolve(response),
            Ok(Frame::Event(event)) => {
                tracing::debug!(kind = %event.kind, "ignored an event from the hub")
            }
            Err(WorkerError::Network(m)) if m == "the hub closed the connection" => break Ok(()),
            Err(err) => break Err(err),
        }
    };
    link.detach();
    writer.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn requests_fail_when_offline() {
        let link = Link::new();
        assert!(!link.is_connected());
        let err = link
            .request(&consort_proto::worker_link::ToolList {
                session_id: consort_proto::ids::SessionId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, WorkerError::Offline));
    }

    #[tokio::test]
    async fn responses_resolve_pending_requests() {
        let link = Link::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        link.attach(tx);
        let l = link.clone();
        let task = tokio::spawn(async move {
            l.request(&consort_proto::worker_link::ToolList {
                session_id: consort_proto::ids::SessionId::new(),
            })
            .await
        });
        let sent = rx.recv().await.unwrap();
        let Frame::Request(req) = Frame::parse(&sent).unwrap() else {
            panic!("expected a request")
        };
        assert_eq!(req.kind, "tool.list");
        link.resolve(ResponseFrame {
            re: req.id,
            result: Ok(serde_json::json!({"tools": []})),
        });
        assert!(task.await.unwrap().unwrap().tools.is_empty());
        link.detach();
        assert!(!link.is_connected());
    }

    #[test]
    fn jitter_stays_in_range() {
        for _ in 0..100 {
            let d = jitter(Duration::from_secs(2));
            assert!(d >= Duration::from_secs(1) && d <= Duration::from_secs(2));
        }
    }
}
