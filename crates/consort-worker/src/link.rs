//! The worker link: one WebSocket to the hub, with challenge
//! authentication, `hello`, request and response matching, pings, a
//! liveness check and reconnects with jittered backoff.
//!
//! The worker sends a ping every 20 seconds, and the hub answers each one
//! with a pong. When no byte arrives for 60 seconds, the worker closes the
//! link and connects again. A socket that a sleep or a network change left
//! half open so does not look connected for long. The check counts bytes,
//! not whole frames, so a large frame on a slow link that is still
//! arriving keeps the link open.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use consort_proto::auth::{
    AuthChallenge, AuthResponse, PeerId, WorkerHello, WorkerHelloResult, auth_signing_payload,
};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{
    Frame, FrameError, FrameId, Message as ProtoMessage, Request, ResponseError, ResponseFrame,
    SUPPORTED_PROTOCOL_VERSIONS,
};
use consort_proto::worker_link::{OFFLINE_AFTER_SECS, PING_INTERVAL_SECS};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::WorkerError;

/// How long the worker waits for the hub to answer a request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// How long the worker waits for the challenge and the `hello` answer.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/// The shortest and longest wait between reconnects.
const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// How often the worker pings, and how long it waits for any frame or pong
/// before it closes the link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkTiming {
    /// The time between two pings.
    pub ping: Duration,
    /// The longest time with no frame and no pong from the hub.
    pub liveness: Duration,
}

impl Default for LinkTiming {
    fn default() -> Self {
        Self {
            ping: Duration::from_secs(PING_INTERVAL_SECS),
            liveness: Duration::from_secs(OFFLINE_AFTER_SECS),
        }
    }
}

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

    pub(crate) fn attach(&self, out: mpsc::UnboundedSender<String>) {
        *lock(&self.inner.out) = Some(out);
        self.inner.connected.send_replace(true);
    }

    pub(crate) fn detach(&self) {
        *lock(&self.inner.out) = None;
        self.inner.connected.send_replace(false);
        // Dropping the senders fails every request that waits.
        lock(&self.inner.pending).clear();
    }

    /// Sends a frame.
    pub fn send_frame(&self, frame: &Frame) -> Result<(), WorkerError> {
        let text = frame.to_text().map_err(|e| match e {
            FrameError::TooLarge { .. } => WorkerError::TooLarge(e.to_string()),
            other => WorkerError::Network(other.to_string()),
        })?;
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

    pub(crate) fn resolve(&self, response: ResponseFrame) {
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
            r = connect_once(&link, handler.clone(), LinkTiming::default()) => r,
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

/// The time of the last read from the hub that returned bytes.
#[derive(Clone)]
struct LastRead(Arc<Mutex<Instant>>);

impl LastRead {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Instant::now())))
    }

    fn mark(&self) {
        *lock(&self.0) = Instant::now();
    }

    fn elapsed(&self) -> Duration {
        lock(&self.0).elapsed()
    }
}

/// A TCP stream that marks the time of each read that returns bytes, below
/// TLS and the WebSocket framing.
struct Progress {
    inner: TcpStream,
    last: LastRead,
}

impl AsyncRead for Progress {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(result, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            self.last.mark();
        }
        result
    }
}

impl AsyncWrite for Progress {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<Progress>>,
>;

/// Opens the WebSocket to the hub over a [`Progress`] stream. Returns the
/// socket and the time of its last read.
async fn open_socket(
    url: &str,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<Progress>>,
        LastRead,
    ),
    WorkerError,
> {
    let request = url
        .into_client_request()
        .map_err(|e| WorkerError::Network(format!("{url}: {e}")))?;
    let uri = request.uri();
    let host = uri
        .host()
        .map(|h| h.trim_start_matches('[').trim_end_matches(']').to_owned())
        .ok_or_else(|| WorkerError::Network(format!("{url} has no host")))?;
    let port = uri
        .port_u16()
        .or(match uri.scheme_str() {
            Some("wss") => Some(443),
            Some("ws") => Some(80),
            _ => None,
        })
        .ok_or_else(|| WorkerError::Network(format!("{url} has no port")))?;
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| WorkerError::Network(format!("{url}: {e}")))?;
    let last = LastRead::new();
    let stream = Progress {
        inner: tcp,
        last: last.clone(),
    };
    let (ws, _) = tokio_tungstenite::client_async_tls(request, stream)
        .await
        .map_err(|e| WorkerError::Network(format!("{url}: {e}")))?;
    Ok((ws, last))
}

/// The text of the error that ends the read loop when the hub closes.
const HUB_CLOSED: &str = "the hub closed the connection";

/// Reads one WebSocket message. A text frame gives a frame. A ping, a pong
/// or another control message gives `None`: it shows that the link is
/// alive, but it holds no frame.
async fn read_one(stream: &mut WsStream) -> Result<Option<Frame>, WorkerError> {
    match stream.next().await {
        Some(Ok(Message::Text(text))) => Frame::parse(text.as_str())
            .map(Some)
            .map_err(|e| WorkerError::Network(e.to_string())),
        Some(Ok(Message::Close(_))) | None => Err(WorkerError::Network(HUB_CLOSED.to_owned())),
        Some(Ok(_)) => Ok(None),
        Some(Err(e)) => Err(WorkerError::Network(e.to_string())),
    }
}

async fn next_frame(stream: &mut WsStream) -> Result<Frame, WorkerError> {
    loop {
        if let Some(frame) = read_one(stream).await? {
            return Ok(frame);
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

async fn connect_once<H: LinkHandler>(
    link: &Link,
    handler: Arc<H>,
    timing: LinkTiming,
) -> Result<(), WorkerError> {
    let url = crate::http::worker_ws_url(handler.hub_url())?;
    let origin = consort_proto::auth::normalize_origin(handler.hub_url())
        .map_err(|e| WorkerError::Config(e.to_string()))?;
    let (ws, last_read) = tokio::time::timeout(HANDSHAKE_TIMEOUT, open_socket(&url))
        .await
        .map_err(|_| WorkerError::Network(format!("{url} did not answer")))??;
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
        let mut ping = tokio::time::interval(timing.ping);
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
        // Wait until the liveness time after the last byte. When it runs
        // out, a byte that came in the meantime starts the wait again. A
        // read that a time-out drops keeps its bytes in the WebSocket's
        // buffer.
        let left = timing.liveness.saturating_sub(last_read.elapsed());
        let Ok(read) = tokio::time::timeout(left, read_one(&mut stream)).await else {
            if last_read.elapsed() >= timing.liveness {
                break Err(WorkerError::Network(format!(
                    "the hub sent no data for {} seconds",
                    timing.liveness.as_secs()
                )));
            }
            continue;
        };
        match read {
            Ok(None) => {}
            Ok(Some(Frame::Request(request))) => handler.clone().command(request),
            Ok(Some(Frame::Response(response))) => link.resolve(response),
            Ok(Some(Frame::Event(event))) => {
                tracing::debug!(kind = %event.kind, "ignored an event from the hub")
            }
            Err(WorkerError::Network(m)) if m == HUB_CLOSED => break Ok(()),
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

    struct TestHandler {
        url: String,
        identity: crate::identity::Identity,
        worker_id: consort_proto::ids::WorkerId,
        /// Gets the ID of each hub command.
        commands: Option<mpsc::UnboundedSender<String>>,
    }

    impl LinkHandler for TestHandler {
        fn worker_id(&self) -> consort_proto::ids::WorkerId {
            self.worker_id
        }
        fn hub_url(&self) -> &str {
            &self.url
        }
        fn sign(&self, payload: &[u8]) -> consort_proto::auth::Signature {
            self.identity.sign(payload)
        }
        fn hello(&self) -> WorkerHello {
            WorkerHello {
                worker_id: self.worker_id,
                version: "test".into(),
                protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
                sessions: vec![],
            }
        }
        fn connected(self: Arc<Self>, _result: WorkerHelloResult) {}
        fn command(self: Arc<Self>, request: consort_proto::frame::RequestFrame) {
            if let Some(tx) = &self.commands {
                let _ = tx.send(request.id.as_str().to_owned());
            }
        }
    }

    /// A hub that finishes the handshake. With `silent`, it then stops
    /// reading, so it answers no ping, as a half-open socket does.
    async fn handshake_hub(silent: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let (mut sink, mut source) = ws.split();
            let challenge = Frame::event(&AuthChallenge {
                nonce: consort_proto::auth::Nonce::from_bytes([7; 32]),
            })
            .unwrap()
            .to_text()
            .unwrap();
            sink.send(Message::Text(challenge.into())).await.unwrap();
            for answer in [
                serde_json::json!({}),
                serde_json::json!({
                    "protocol_version": 1, "hub_version": "test", "sessions": []
                }),
            ] {
                let Some(Ok(Message::Text(text))) = source.next().await else {
                    return;
                };
                let Frame::Request(request) = Frame::parse(text.as_str()).unwrap() else {
                    panic!("expected a request")
                };
                let reply = request.respond_ok(&answer).unwrap().to_text().unwrap();
                sink.send(Message::Text(reply.into())).await.unwrap();
            }
            if silent {
                tokio::time::sleep(Duration::from_secs(30)).await;
            } else {
                // Reading answers each ping with a pong.
                while let Some(Ok(_)) = source.next().await {}
            }
            drop(sink);
        });
        url
    }

    fn handler(url: String) -> Arc<TestHandler> {
        Arc::new(TestHandler {
            url,
            identity: crate::identity::Identity::generate(),
            worker_id: consort_proto::ids::WorkerId::new(),
            commands: None,
        })
    }

    /// A server stream that, once `slow` is on, writes at most `chunk`
    /// bytes at a time and waits `pause` after each write, as a slow uplink
    /// does.
    struct Throttled {
        inner: TcpStream,
        slow: Arc<std::sync::atomic::AtomicBool>,
        sleep: Option<Pin<Box<tokio::time::Sleep>>>,
        chunk: usize,
        pause: Duration,
    }

    impl AsyncRead for Throttled {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for Throttled {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            if !self.slow.load(std::sync::atomic::Ordering::SeqCst) {
                return Pin::new(&mut self.inner).poll_write(cx, buf);
            }
            if let Some(sleep) = self.sleep.as_mut() {
                std::task::ready!(sleep.as_mut().poll(cx));
                self.sleep = None;
            }
            let n = buf.len().min(self.chunk);
            let written = std::task::ready!(Pin::new(&mut self.inner).poll_write(cx, &buf[..n]));
            let pause = self.pause;
            self.sleep = Some(Box::pin(tokio::time::sleep(pause)));
            Poll::Ready(written)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    #[tokio::test]
    async fn a_large_frame_on_a_slow_link_keeps_the_link_open() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let slow = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stream = Throttled {
                inner: stream,
                slow: slow.clone(),
                sleep: None,
                chunk: 8 * 1024,
                pause: Duration::from_millis(60),
            };
            let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let (mut sink, mut source) = ws.split();
            let challenge = Frame::event(&AuthChallenge {
                nonce: consort_proto::auth::Nonce::from_bytes([7; 32]),
            })
            .unwrap()
            .to_text()
            .unwrap();
            sink.send(Message::Text(challenge.into())).await.unwrap();
            for answer in [
                serde_json::json!({}),
                serde_json::json!({
                    "protocol_version": 1, "hub_version": "test", "sessions": []
                }),
            ] {
                let Some(Ok(Message::Text(text))) = source.next().await else {
                    return;
                };
                let Frame::Request(request) = Frame::parse(text.as_str()).unwrap() else {
                    panic!("expected a request")
                };
                let reply = request.respond_ok(&answer).unwrap().to_text().unwrap();
                sink.send(Message::Text(reply.into())).await.unwrap();
            }
            // About 100 KiB at 8 KiB every 60 ms takes about 750 ms, longer
            // than the liveness time. The hub reads nothing meanwhile, so it
            // answers no ping.
            slow.store(true, std::sync::atomic::Ordering::SeqCst);
            let big = serde_json::json!({
                "v": 1, "id": "big", "type": "inventory.refresh",
                "body": {"pad": "x".repeat(100 * 1024)}
            })
            .to_string();
            sink.send(Message::Text(big.into())).await.unwrap();
            slow.store(false, std::sync::atomic::Ordering::SeqCst);
            while let Some(Ok(_)) = source.next().await {}
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handler = Arc::new(TestHandler {
            url,
            identity: crate::identity::Identity::generate(),
            worker_id: consort_proto::ids::WorkerId::new(),
            commands: Some(tx),
        });
        let link = Link::new();
        let timing = LinkTiming {
            ping: Duration::from_millis(100),
            liveness: Duration::from_millis(400),
        };
        let run = tokio::spawn({
            let link = link.clone();
            async move { connect_once(&link, handler, timing).await }
        });
        let got = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("the large frame arrives");
        assert_eq!(got.as_deref(), Some("big"));
        assert!(!run.is_finished(), "the link stays open: {:?}", run.await);
        assert!(link.is_connected());
        run.abort();
    }

    #[tokio::test]
    async fn a_silent_hub_closes_the_link() {
        let url = handshake_hub(true).await;
        let link = Link::new();
        let timing = LinkTiming {
            ping: Duration::from_millis(100),
            liveness: Duration::from_millis(400),
        };
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            connect_once(&link, handler(url), timing),
        )
        .await
        .expect("the worker notices the silence");
        assert!(
            matches!(&result, Err(WorkerError::Network(m)) if m.contains("no data")),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!link.is_connected());
    }

    #[tokio::test]
    async fn pongs_keep_an_idle_link_open() {
        let url = handshake_hub(false).await;
        let link = Link::new();
        let timing = LinkTiming {
            ping: Duration::from_millis(100),
            liveness: Duration::from_millis(400),
        };
        let run = connect_once(&link, handler(url), timing);
        let outcome = tokio::time::timeout(Duration::from_millis(1500), run).await;
        assert!(outcome.is_err(), "the link stays open: {outcome:?}");
    }

    #[test]
    fn too_large_frames_are_a_typed_error() {
        let link = Link::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        link.attach(tx);
        let small = Frame::error(
            FrameId::generate(),
            ErrorBody::new(ErrorCode::Internal, "x"),
        );
        assert!(link.send_frame(&small).is_ok());
        let big = Frame::error(
            FrameId::generate(),
            ErrorBody::new(ErrorCode::Internal, "x".repeat(2 * 1024 * 1024)),
        );
        assert!(matches!(
            link.send_frame(&big),
            Err(WorkerError::TooLarge(_))
        ));
    }

    #[test]
    fn jitter_stays_in_range() {
        for _ in 0..100 {
            let d = jitter(Duration::from_secs(2));
            assert!(d >= Duration::from_secs(1) && d <= Duration::from_secs(2));
        }
    }
}
