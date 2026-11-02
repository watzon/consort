//! A small client for the Chrome DevTools protocol over a WebSocket.
//!
//! [`Cdp::call`] sends a command with a new ID and waits for the reply
//! with that ID. The reader task routes each reply to its caller and each
//! event to the page session that it belongs to ([`Cdp::events`]). When the
//! socket closes, every call that waits fails with [`CdpError::Closed`],
//! and the event channels of all sessions end.
//!
//! Each message from the browser gets the next number of a sequence. An
//! event with a larger number than a reply ([`Cdp::call_ordered`]) came
//! after that reply. A page uses this to wait for an event after its
//! navigation, and not for an older one that still waits in its channel.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// The largest message from the browser. A screenshot of a page 20,000 px
/// tall at scale 3 is a large PNG, and base64 makes it a third larger. The
/// browser's own send buffer is 256 MiB.
pub(crate) const MAX_MESSAGE_BYTES: usize = 256 * 1024 * 1024;

/// Why a DevTools call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CdpError {
    /// The connection could not open.
    #[error("could not connect to the browser: {0}")]
    Connect(String),
    /// The connection closed before the reply came.
    #[error("the connection to the browser closed: {0}")]
    Closed(String),
    /// No reply came in time.
    #[error("{method} got no reply in {} s", .after.as_secs())]
    Timeout { method: String, after: Duration },
    /// The browser answered with an error.
    #[error("{method} failed: {message}")]
    Protocol { method: String, message: String },
}

/// One event of a page session.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Event {
    /// The event name, such as `Page.loadEventFired`.
    pub method: String,
    /// The event parameters.
    pub params: Value,
    /// The number of the message in the sequence of the connection.
    pub seq: u64,
}

type Reply = Result<(Value, u64), CdpError>;

#[derive(Default)]
struct State {
    /// The calls that wait for a reply, by ID, with their method.
    pending: HashMap<u64, (String, oneshot::Sender<Reply>)>,
    /// The event channel of each page session.
    sessions: HashMap<String, mpsc::UnboundedSender<Event>>,
    /// Why the connection closed, once it has.
    closed: Option<String>,
    /// The number of the last message from the browser.
    seq: u64,
}

struct Inner {
    out: mpsc::UnboundedSender<Message>,
    next_id: AtomicU64,
    state: Mutex<State>,
    timeout: Duration,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let tasks = match self.tasks.get_mut() {
            Ok(tasks) => tasks,
            Err(poisoned) => poisoned.into_inner(),
        };
        for task in tasks.drain(..) {
            task.abort();
        }
    }
}

/// A DevTools connection. Clones share one socket.
#[derive(Clone)]
pub(crate) struct Cdp {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Cdp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cdp").finish_non_exhaustive()
    }
}

impl Cdp {
    /// Opens the WebSocket at `url`. Each call waits at most `timeout` for
    /// its reply.
    pub(crate) async fn connect(url: &str, timeout: Duration) -> Result<Self, CdpError> {
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        let (socket, _) = tokio_tungstenite::connect_async_with_config(url, Some(config), false)
            .await
            .map_err(|e| CdpError::Connect(e.to_string()))?;
        let (mut sink, mut stream) = socket.split();
        let (out, mut out_rx) = mpsc::unbounded_channel::<Message>();
        let inner = Arc::new(Inner {
            out,
            next_id: AtomicU64::new(0),
            state: Mutex::new(State::default()),
            timeout,
            tasks: Mutex::new(Vec::new()),
        });

        let weak = Arc::downgrade(&inner);
        let writer = tokio::spawn(async move {
            while let Some(message) = out_rx.recv().await {
                if let Err(err) = sink.send(message).await {
                    if let Some(inner) = weak.upgrade() {
                        close(&inner, format!("could not send: {err}"));
                    }
                    return;
                }
            }
            let _ = sink.close().await;
        });

        let weak = Arc::downgrade(&inner);
        let reader = tokio::spawn(async move {
            let reason = loop {
                match stream.next().await {
                    Some(Ok(Message::Text(text))) => {
                        let Some(inner) = weak.upgrade() else { return };
                        route(&inner, text.as_str());
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        let Some(inner) = weak.upgrade() else { return };
                        if let Ok(text) = std::str::from_utf8(&bytes) {
                            route(&inner, text);
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        break frame.map_or_else(
                            || "the browser closed the socket".to_owned(),
                            |f| format!("the browser closed the socket ({})", f.reason.as_str()),
                        );
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => break err.to_string(),
                    None => break "the socket ended".to_owned(),
                }
            };
            if let Some(inner) = weak.upgrade() {
                close(&inner, reason);
            }
        });
        if let Ok(mut tasks) = inner.tasks.lock() {
            tasks.push(writer);
            tasks.push(reader);
        }
        Ok(Self { inner })
    }

    /// Sends a command and waits for its result, with the default timeout.
    /// `session` is the page session, or `None` for the browser.
    pub(crate) async fn call(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, CdpError> {
        self.call_with_timeout(method, params, session, self.inner.timeout)
            .await
    }

    /// Like [`Cdp::call`], and also returns the sequence number of the
    /// reply.
    pub(crate) async fn call_ordered(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<(Value, u64), CdpError> {
        self.send(method, params, session, self.inner.timeout).await
    }

    /// Like [`Cdp::call`], with another timeout.
    pub(crate) async fn call_with_timeout(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, CdpError> {
        self.send(method, params, session, timeout)
            .await
            .map(|(value, _)| value)
    }

    async fn send(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<(Value, u64), CdpError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        {
            let mut state = lock(&self.inner.state);
            if let Some(reason) = &state.closed {
                return Err(CdpError::Closed(reason.clone()));
            }
            state.pending.insert(id, (method.to_owned(), tx));
        }
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = Value::String(session.to_owned());
        }
        if self
            .inner
            .out
            .send(Message::text(message.to_string()))
            .is_err()
        {
            lock(&self.inner.state).pending.remove(&id);
            return Err(CdpError::Closed("the writer stopped".to_owned()));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(CdpError::Closed(self.closed_reason())),
            Err(_) => {
                lock(&self.inner.state).pending.remove(&id);
                Err(CdpError::Timeout {
                    method: method.to_owned(),
                    after: timeout,
                })
            }
        }
    }

    /// The events of a page session from now on. A second call for the
    /// same session replaces the first channel. The channel ends when the
    /// connection closes or [`Cdp::forget`] runs.
    pub(crate) fn events(&self, session: &str) -> mpsc::UnboundedReceiver<Event> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = lock(&self.inner.state);
        // A closed connection drops the sender at once, so the receiver
        // ends.
        if state.closed.is_none() {
            state.sessions.insert(session.to_owned(), tx);
        }
        rx
    }

    /// Stops the events of a page session.
    pub(crate) fn forget(&self, session: &str) {
        lock(&self.inner.state).sessions.remove(session);
    }

    /// Whether the connection has closed.
    pub(crate) fn is_closed(&self) -> bool {
        lock(&self.inner.state).closed.is_some()
    }

    fn closed_reason(&self) -> String {
        lock(&self.inner.state)
            .closed
            .clone()
            .unwrap_or_else(|| "the connection closed".to_owned())
    }
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Delivers one message from the browser.
fn route(inner: &Inner, text: &str) {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        tracing::debug!("the preview browser sent a message that is not JSON");
        return;
    };
    let mut state = lock(&inner.state);
    state.seq += 1;
    let seq = state.seq;
    if let Some(id) = message.get("id").and_then(Value::as_u64) {
        let Some((method, reply)) = state.pending.remove(&id) else {
            return;
        };
        drop(state);
        let result = match message.get("error") {
            Some(error) => Err(CdpError::Protocol {
                method,
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("no message")
                    .to_owned(),
            }),
            None => Ok((message.get("result").cloned().unwrap_or(Value::Null), seq)),
        };
        let _ = reply.send(result);
        return;
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    // `Target.detachedFromTarget` comes on the browser session and names
    // the page session in its parameters.
    let session = message
        .get("sessionId")
        .and_then(Value::as_str)
        .or_else(|| {
            (method == "Target.detachedFromTarget")
                .then(|| params.get("sessionId").and_then(Value::as_str))
                .flatten()
        });
    let Some(session) = session else { return };
    if let Some(events) = state.sessions.get(session) {
        let _ = events.send(Event {
            method: method.to_owned(),
            params,
            seq,
        });
    }
}

/// Marks the connection closed, fails every call that waits, and ends the
/// event channels.
fn close(inner: &Inner, reason: String) {
    let (pending, sessions) = {
        let mut state = lock(&inner.state);
        if state.closed.is_none() {
            state.closed = Some(reason.clone());
        }
        (
            std::mem::take(&mut state.pending),
            std::mem::take(&mut state.sessions),
        )
    };
    for (_, (_, reply)) in pending {
        let _ = reply.send(Err(CdpError::Closed(reason.clone())));
    }
    drop(sessions);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A tiny DevTools server. It answers each command by ID with
    /// `{"echo": <method>}`, out of order when the method is `Test.slow`.
    /// `Test.fail` gets an error, `Test.event` makes an event for the
    /// session `S1`, `Test.silent` gets no reply, and `Test.close` closes
    /// the socket.
    async fn fake_browser() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut held: Option<u64> = None;
            while let Some(Ok(message)) = ws.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let command: Value = serde_json::from_str(text.as_str()).unwrap();
                let id = command["id"].as_u64().unwrap();
                let method = command["method"].as_str().unwrap().to_owned();
                let reply = match method.as_str() {
                    "Test.slow" => {
                        held = Some(id);
                        continue;
                    }
                    "Test.silent" => continue,
                    "Test.fail" => {
                        json!({"id": id, "error": {"code": -32000, "message": "no such thing"}})
                    }
                    "Test.event" => {
                        ws.send(Message::text(
                            json!({"method": "Page.loadEventFired", "sessionId": "S1", "params": {"timestamp": 1}}).to_string(),
                        ))
                        .await
                        .unwrap();
                        ws.send(Message::text(
                            json!({"method": "Page.loadEventFired", "sessionId": "S2", "params": {}}).to_string(),
                        ))
                        .await
                        .unwrap();
                        ws.send(Message::text(
                            json!({"method": "Target.detachedFromTarget", "params": {"sessionId": "S1"}}).to_string(),
                        ))
                        .await
                        .unwrap();
                        json!({"id": id, "result": {}})
                    }
                    "Test.close" => {
                        let _ = ws.close(None).await;
                        return;
                    }
                    _ => {
                        json!({"id": id, "result": {"echo": method, "session": command.get("sessionId")}})
                    }
                };
                ws.send(Message::text(reply.to_string())).await.unwrap();
                if let Some(slow) = held.take() {
                    ws.send(Message::text(
                        json!({"id": slow, "result": {"echo": "Test.slow"}}).to_string(),
                    ))
                    .await
                    .unwrap();
                }
            }
        });
        format!("ws://127.0.0.1:{port}/devtools/browser/test")
    }

    #[tokio::test]
    async fn visual_cdp_routes_replies_by_id() {
        let cdp = Cdp::connect(&fake_browser().await, Duration::from_secs(5))
            .await
            .unwrap();
        // The slow call is answered after the fast one.
        let (slow, fast) = tokio::join!(cdp.call("Test.slow", json!({}), None), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cdp.call("Page.enable", json!({}), Some("S1")).await
        });
        assert_eq!(slow.unwrap()["echo"], "Test.slow");
        let fast = fast.unwrap();
        assert_eq!(fast["echo"], "Page.enable");
        assert_eq!(fast["session"], "S1");
    }

    #[tokio::test]
    async fn visual_cdp_returns_protocol_errors() {
        let cdp = Cdp::connect(&fake_browser().await, Duration::from_secs(5))
            .await
            .unwrap();
        let err = cdp.call("Test.fail", json!({}), None).await.unwrap_err();
        assert_eq!(
            err,
            CdpError::Protocol {
                method: "Test.fail".to_owned(),
                message: "no such thing".to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn visual_cdp_delivers_events_per_session() {
        let cdp = Cdp::connect(&fake_browser().await, Duration::from_secs(5))
            .await
            .unwrap();
        let mut events = cdp.events("S1");
        let (_, reply) = cdp
            .call_ordered("Test.event", json!({}), None)
            .await
            .unwrap();
        let first = events.recv().await.unwrap();
        assert_eq!(first.method, "Page.loadEventFired");
        assert_eq!(first.params["timestamp"], 1);
        // The event of S2 is not for this session. The detach event comes on
        // the browser session and names S1.
        let second = events.recv().await.unwrap();
        assert_eq!(second.method, "Target.detachedFromTarget");
        // The events came before the reply, in order.
        assert!(first.seq < second.seq && second.seq < reply);
        // The events of a second call come after the first reply.
        cdp.call("Test.event", json!({}), None).await.unwrap();
        assert!(events.recv().await.unwrap().seq > reply);
        assert!(events.recv().await.unwrap().seq > reply);
        cdp.forget("S1");
        assert!(events.recv().await.is_none());
    }

    #[tokio::test]
    async fn visual_cdp_times_out_a_call() {
        let cdp = Cdp::connect(&fake_browser().await, Duration::from_millis(100))
            .await
            .unwrap();
        let err = cdp.call("Test.silent", json!({}), None).await.unwrap_err();
        assert!(matches!(err, CdpError::Timeout { .. }), "{err:?}");
        // The connection still works.
        let ok = cdp.call("Page.enable", json!({}), None).await.unwrap();
        assert_eq!(ok["echo"], "Page.enable");
    }

    #[tokio::test]
    async fn visual_cdp_fails_pending_calls_when_the_socket_closes() {
        let cdp = Cdp::connect(&fake_browser().await, Duration::from_secs(5))
            .await
            .unwrap();
        let mut events = cdp.events("S1");
        let waiting = {
            let cdp = cdp.clone();
            tokio::spawn(async move { cdp.call("Test.silent", json!({}), None).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        let closing = cdp.call("Test.close", json!({}), None).await.unwrap_err();
        assert!(matches!(closing, CdpError::Closed(_)), "{closing:?}");
        let waited = waiting.await.unwrap().unwrap_err();
        assert!(matches!(waited, CdpError::Closed(_)), "{waited:?}");
        assert!(cdp.is_closed());
        assert!(events.recv().await.is_none());
        // A new call fails at once.
        let later = cdp.call("Page.enable", json!({}), None).await.unwrap_err();
        assert!(matches!(later, CdpError::Closed(_)), "{later:?}");
    }

    #[tokio::test]
    async fn visual_cdp_reports_a_failed_connect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let err = Cdp::connect(&format!("ws://127.0.0.1:{port}/x"), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(matches!(err, CdpError::Connect(_)), "{err:?}");
    }
}
