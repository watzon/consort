//! A minimal fake hub for worker tests: pairing over HTTP, the challenge,
//! `hello`, acks for `session.events`, `tool.list`, `tool.call` and hub
//! commands, with plain `tokio-tungstenite` and the proto types.

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use consort_proto::auth::{
    AuthChallenge, AuthResponse, Nonce, PairWorkerRequest, PairWorkerResponse, PublicKey,
    SessionSeq, WorkerHello, WorkerHelloResult, auth_signing_payload, normalize_origin,
};
use consort_proto::error::ErrorBody;
use consort_proto::frame::{Frame, FrameId, Message as _, ResponseFrame};
use consort_proto::ids::{SessionId, WorkerId};
use consort_proto::model::Inventory;
use consort_proto::tools::{HubTool, ToolCallResult};
use consort_proto::worker_link::{
    HubToWorker, SequencedEvent, SessionEventsAck, ToolCall, ToolListResult, WorkerToHub,
    contiguous_seq,
};
use ed25519_dalek::{Verifier, VerifyingKey};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

pub const CODE: &str = "ABCD-EFGH";

#[derive(Default)]
pub struct HubState {
    pub origin: String,
    pub worker_id: Mutex<Option<WorkerId>>,
    pub public_key: Mutex<Option<PublicKey>>,
    pub stored: Mutex<HashMap<SessionId, Vec<SequencedEvent>>>,
    pub reject_events: AtomicBool,
    pub conn: Mutex<Option<mpsc::UnboundedSender<Message>>>,
    pub waiting: Mutex<HashMap<FrameId, oneshot::Sender<ResponseFrame>>>,
    pub tool_calls: Mutex<Vec<ToolCall>>,
    pub inventories: Mutex<Vec<Inventory>>,
    pub hellos: Mutex<Vec<WorkerHello>>,
    pub blobs: Mutex<Vec<String>>,
    pub tool_lists: Mutex<Vec<SessionId>>,
    pub kick: Notify,
}

#[derive(Clone)]
pub struct FakeHub {
    pub url: String,
    pub state: Arc<HubState>,
}

async fn read_http(stream: &mut TcpStream) -> (String, String, Vec<u8>) {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0, "the client closed the connection");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let length: usize = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap())
        })
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).await.unwrap();
        body.extend_from_slice(&chunk[..n]);
    }
    let first = head.lines().next().unwrap().to_owned();
    let mut parts = first.split(' ');
    (
        parts.next().unwrap().to_owned(),
        parts.next().unwrap().to_owned(),
        body,
    )
}

async fn write_http(stream: &mut TcpStream, status: &str, body: &Value) {
    let text = body.to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
        text.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}

impl FakeHub {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(HubState {
            origin: normalize_origin(&url).unwrap(),
            ..HubState::default()
        });
        let hub = Self { url, state };
        let accept = hub.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let hub = accept.clone();
                tokio::spawn(async move { hub.connection(stream).await });
            }
        });
        hub
    }

    async fn connection(&self, mut stream: TcpStream) {
        let mut peek = [0_u8; 16];
        let n = stream.peek(&mut peek).await.unwrap();
        if peek[..n].starts_with(b"GET /v1/worker") {
            self.websocket(stream).await;
            return;
        }
        let (method, path, body) = read_http(&mut stream).await;
        match (method.as_str(), path.as_str()) {
            ("POST", "/v1/pair/worker") => {
                let request: PairWorkerRequest = serde_json::from_slice(&body).unwrap();
                if request.code.canonical() != CODE.replace('-', "") {
                    write_http(
                        &mut stream,
                        "403 Forbidden",
                        &json!({"error": {"code": "unauthorized", "message": "bad code"}}),
                    )
                    .await;
                    return;
                }
                let worker_id = WorkerId::new();
                *self.state.worker_id.lock().unwrap() = Some(worker_id);
                *self.state.public_key.lock().unwrap() = Some(request.public_key);
                write_http(
                    &mut stream,
                    "200 OK",
                    &serde_json::to_value(PairWorkerResponse { worker_id }).unwrap(),
                )
                .await;
            }
            ("PUT", p) if p.starts_with("/v1/blobs/") => {
                self.state.blobs.lock().unwrap().push(p.to_owned());
                write_http(&mut stream, "200 OK", &json!({})).await;
            }
            _ => {
                write_http(
                    &mut stream,
                    "404 Not Found",
                    &json!({"error": {"code": "not_found", "message": "no"}}),
                )
                .await
            }
        }
    }

    async fn websocket(&self, stream: TcpStream) {
        let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let (mut sink, mut source) = ws.split();
        let mut nonce = [0_u8; 32];
        rand::Rng::fill(&mut rand::thread_rng(), &mut nonce);
        let nonce = Nonce::from_bytes(nonce);
        let challenge = Frame::event(&AuthChallenge { nonce })
            .unwrap()
            .to_text()
            .unwrap();
        sink.send(Message::Text(challenge.into())).await.unwrap();

        let next_request = |text: &str| match Frame::parse(text).unwrap() {
            Frame::Request(r) => r,
            other => panic!("expected a request, got {other:?}"),
        };
        let Some(Ok(Message::Text(text))) = source.next().await else {
            return;
        };
        let auth = next_request(text.as_str());
        let response: AuthResponse = auth.decode_as().unwrap();
        let key = self.state.public_key.lock().unwrap().unwrap();
        let vk = VerifyingKey::from_bytes(key.as_bytes()).unwrap();
        let sig = ed25519_dalek::Signature::from_bytes(response.signature.as_bytes());
        vk.verify(&auth_signing_payload(&self.state.origin, &nonce), &sig)
            .expect("the worker signs the challenge");
        sink.send(Message::Text(
            auth.respond_ok(&json!({}))
                .unwrap()
                .to_text()
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();

        let Some(Ok(Message::Text(text))) = source.next().await else {
            return;
        };
        let hello_frame = next_request(text.as_str());
        let hello: WorkerHello = hello_frame.decode_as().unwrap();
        self.state.hellos.lock().unwrap().push(hello.clone());
        let sessions = {
            let stored = self.state.stored.lock().unwrap();
            hello
                .sessions
                .iter()
                .map(|s| SessionSeq {
                    session_id: s.session_id,
                    last_seq: stored
                        .get(&s.session_id)
                        .map(|e| e.len() as u64)
                        .unwrap_or(0),
                })
                .collect()
        };
        let result = WorkerHelloResult {
            protocol_version: 1,
            hub_version: "fake".into(),
            sessions,
        };
        sink.send(Message::Text(
            hello_frame
                .respond_ok(&result)
                .unwrap()
                .to_text()
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        *self.state.conn.lock().unwrap() = Some(tx.clone());
        let writer = tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                if sink.send(m).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        loop {
            tokio::select! {
                message = source.next() => {
                    let Some(Ok(message)) = message else { break };
                    let Message::Text(text) = message else { continue };
                    self.frame(Frame::parse(text.as_str()).unwrap(), &tx);
                }
                () = self.state.kick.notified() => break,
            }
        }
        *self.state.conn.lock().unwrap() = None;
        writer.abort();
    }

    fn frame(&self, frame: Frame, tx: &mpsc::UnboundedSender<Message>) {
        let send = |f: Frame| {
            let _ = tx.send(Message::Text(f.to_text().unwrap().into()));
        };
        match frame {
            Frame::Response(r) => {
                if let Some(waiter) = self.state.waiting.lock().unwrap().remove(&r.re) {
                    let _ = waiter.send(r);
                }
            }
            Frame::Event(e) => {
                if e.kind == Inventory::TYPE {
                    self.state
                        .inventories
                        .lock()
                        .unwrap()
                        .push(e.decode_as().unwrap());
                }
            }
            Frame::Request(r) => match r.decode::<WorkerToHub>().unwrap() {
                WorkerToHub::SessionEvents(batch) => {
                    if self.state.reject_events.load(Ordering::SeqCst) {
                        send(r.respond_error(ErrorBody::internal()));
                        return;
                    }
                    let mut stored = self.state.stored.lock().unwrap();
                    let list = stored.entry(batch.session_id).or_default();
                    let have = list.len() as u64;
                    let last = contiguous_seq(have, &batch.events);
                    for event in batch
                        .events
                        .iter()
                        .filter(|e| e.seq > have && e.seq <= last)
                    {
                        list.push(event.clone());
                    }
                    send(
                        r.respond_ok(&SessionEventsAck {
                            session_id: batch.session_id,
                            last_seq: last,
                        })
                        .unwrap(),
                    );
                }
                WorkerToHub::ToolList(list) => {
                    self.state.tool_lists.lock().unwrap().push(list.session_id);
                    let tools = vec![
                        HubTool::PostUpdate.definition(),
                        HubTool::SubmitForReview.definition(),
                    ];
                    send(r.respond_ok(&ToolListResult { tools }).unwrap());
                }
                WorkerToHub::ToolCall(call) => {
                    self.state.tool_calls.lock().unwrap().push(call.clone());
                    send(
                        r.respond_ok(&ToolCallResult::json(&json!({"ok": true})))
                            .unwrap(),
                    );
                }
                other => panic!("unexpected request {other:?}"),
            },
        }
    }

    pub async fn command(&self, id: &str, command: HubToWorker) -> ResponseFrame {
        let id = FrameId::new(id);
        let (tx, rx) = oneshot::channel();
        self.state.waiting.lock().unwrap().insert(id.clone(), tx);
        let text = Frame::family_request(id, &command)
            .unwrap()
            .to_text()
            .unwrap();
        let conn = self
            .state
            .conn
            .lock()
            .unwrap()
            .clone()
            .expect("the worker is connected");
        conn.send(Message::Text(text.into())).unwrap();
        tokio::time::timeout(Duration::from_secs(20), rx)
            .await
            .expect("the worker answers")
            .unwrap()
    }

    pub fn events(&self, session: SessionId) -> Vec<SequencedEvent> {
        self.state
            .stored
            .lock()
            .unwrap()
            .get(&session)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn wait_for<T>(&self, what: &str, mut check: impl FnMut(&FakeHub) -> Option<T>) -> T {
        for _ in 0..500 {
            if let Some(v) = check(self) {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {what}");
    }

    pub async fn wait_connected(&self) {
        self.wait_for("the worker to connect", |h| {
            h.state.conn.lock().unwrap().as_ref().map(|_| ())
        })
        .await;
    }
}
