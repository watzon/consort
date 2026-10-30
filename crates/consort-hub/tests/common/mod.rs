//! A fake client and a fake worker for the integration tests.
//!
//! They use plain WebSocket and HTTP calls and the proto types. They do not
//! depend on `consort-worker` or `consort-testkit`.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Duration;

use consort_proto::auth::{
    AuthChallenge, AuthResponse, CLIENT_WS_PATH, ClientHello, ClientHelloResult, OpenSession,
    PAIR_CLIENT_PATH, PAIR_WORKER_PATH, PairClientRequest, PairClientResponse, PairWorkerRequest,
    PairWorkerResponse, PairingCode, PeerId, PublicKey, Signature, WORKER_WS_PATH, WorkerHello,
    WorkerHelloResult, auth_signing_payload, normalize_origin,
};
use consort_proto::client_api::{
    EntityPayload, FeedChange, FeedEntry, HubToClient, Snapshot, SnapshotScope, SyncSnapshot,
    SyncSubscribe,
};
use consort_proto::error::ErrorBody;
use consort_proto::frame::{Empty, Frame, FrameId, MessageFamily, Request, RequestFrame};
use consort_proto::ids::{AccountId, ChannelId, ClientId, SessionId, WorkerId};
use consort_proto::model::{
    ApprovalMode, AuthState, Capabilities, ClientKind, HarnessAccount, Inventory, ModelInfo, Os,
    Worker, WorkerLoad, WorkerPolicy,
};
use consort_proto::time::Timestamp;
use consort_proto::tools::ToolCallResult;
use consort_proto::worker_link::{
    HubToWorker, SequencedEvent, SessionEvent, SessionEvents, SessionEventsAck, SessionStartResult,
    ToolCall,
};
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use rand::rngs::OsRng;
use serde::Serialize;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use consort_hub::{HubConfig, HubHandle, serve};

pub const WAIT: Duration = Duration::from_secs(10);

pub struct TestHub {
    pub handle: HubHandle,
    pub url: String,
    pub addr: SocketAddr,
    pub dir: tempfile::TempDir,
}

impl TestHub {
    pub async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self::start_in(dir).await
    }

    pub async fn start_in(dir: tempfile::TempDir) -> Self {
        let handle = serve(HubConfig {
            data_dir: dir.path().to_path_buf(),
            listen: "127.0.0.1:0".parse().unwrap(),
            public_url: None,
        })
        .await
        .unwrap();
        Self {
            url: handle.public_url(),
            addr: handle.local_addr(),
            handle,
            dir,
        }
    }

    /// Stops the hub and starts a new one on the same data folder.
    pub async fn restart(self) -> Self {
        let TestHub { handle, dir, .. } = self;
        handle.shutdown().await.unwrap();
        Self::start_in(dir).await
    }

    pub fn ws_url(&self, path: &str) -> String {
        format!("ws://{}{path}", self.addr)
    }

    pub fn origin(&self) -> String {
        normalize_origin(&self.url).unwrap()
    }
}

pub fn new_key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

pub fn public_key(key: &SigningKey) -> PublicKey {
    PublicKey::from_bytes(key.verifying_key().to_bytes())
}

pub fn parse_code(text: &str) -> PairingCode {
    text.parse().unwrap()
}

pub async fn http_post<T: Serialize>(hub: &TestHub, path: &str, body: &T) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}{path}", hub.url))
        .json(body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let value = response.json::<Value>().await.unwrap_or(Value::Null);
    (status, value)
}

pub async fn pair_client_http(hub: &TestHub, code: &str, key: &SigningKey) -> (u16, Value) {
    http_post(
        hub,
        PAIR_CLIENT_PATH,
        &PairClientRequest {
            code: parse_code(code),
            public_key: public_key(key),
            name: "Test client".to_owned(),
            kind: ClientKind::Desktop,
        },
    )
    .await
}

pub async fn pair_worker_http(hub: &TestHub, code: &str, key: &SigningKey) -> (u16, Value) {
    http_post(
        hub,
        PAIR_WORKER_PATH,
        &PairWorkerRequest {
            code: parse_code(code),
            public_key: public_key(key),
            hostname: "fake-host".to_owned(),
            os: Os::Macos,
            arch: "aarch64".to_owned(),
            name: Some("Fake worker".to_owned()),
        },
    )
    .await
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A WebSocket with a buffer for frames that arrive while we wait for another.
pub struct Peer {
    socket: Socket,
    inbox: VecDeque<Frame>,
}

impl Peer {
    pub async fn connect(url: &str) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        Self {
            socket,
            inbox: VecDeque::new(),
        }
    }

    pub async fn send(&mut self, frame: &Frame) {
        let text = frame.to_text().unwrap();
        self.socket.send(Message::text(text)).await.unwrap();
    }

    /// Reads the next frame from the socket. `None` on close or timeout.
    pub async fn read(&mut self, wait: Duration) -> Option<Frame> {
        loop {
            let next = tokio::time::timeout(wait, self.socket.next()).await.ok()?;
            match next? {
                Ok(Message::Text(text)) => return Some(Frame::parse(text.as_str()).unwrap()),
                Ok(Message::Ping(_) | Message::Pong(_)) => {}
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
    }

    /// The next frame, from the buffer first.
    pub async fn next_frame(&mut self, wait: Duration) -> Option<Frame> {
        if let Some(frame) = self.inbox.pop_front() {
            return Some(frame);
        }
        self.read(wait).await
    }

    /// Sends a request and waits for its response. Other frames go to the
    /// buffer.
    pub async fn call_raw(&mut self, kind: &str, body: Value) -> Result<Value, ErrorBody> {
        let id = FrameId::generate();
        self.send(&Frame::Request(RequestFrame {
            id: id.clone(),
            kind: kind.to_owned(),
            body,
        }))
        .await;
        loop {
            let frame = self
                .read(WAIT)
                .await
                .unwrap_or_else(|| panic!("no response to {kind}"));
            match frame {
                Frame::Response(response) if response.re == id => return response.result,
                other => self.inbox.push_back(other),
            }
        }
    }

    pub async fn closed(&mut self, wait: Duration) -> bool {
        loop {
            match tokio::time::timeout(wait, self.socket.next()).await {
                Err(_) => return false,
                Ok(None | Some(Err(_) | Ok(Message::Close(_)))) => return true,
                Ok(Some(Ok(_))) => {}
            }
        }
    }

    pub async fn close(mut self) {
        let _ = self.socket.close(None).await;
    }
}

/// Runs the challenge and returns the response of `auth.response`.
pub async fn authenticate(
    peer: &mut Peer,
    origin: &str,
    peer_id: PeerId,
    key: &SigningKey,
) -> Result<Value, ErrorBody> {
    let Frame::Event(event) = peer.read(WAIT).await.expect("a challenge") else {
        panic!("the first frame is not an event");
    };
    let challenge: AuthChallenge = event.decode_as().unwrap();
    let signature = key.sign(&auth_signing_payload(origin, &challenge.nonce));
    peer.call_raw(
        "auth.response",
        serde_json::to_value(AuthResponse {
            peer_id,
            signature: Signature::from_bytes(signature.to_bytes()),
        })
        .unwrap(),
    )
    .await
}

// The fake client ----------------------------------------------------------

pub struct FakeClient {
    pub peer: Peer,
    pub client_id: ClientId,
    pub key: SigningKey,
    pub hello: ClientHelloResult,
    pub feed: Vec<FeedEntry>,
}

impl FakeClient {
    /// Pairs with the setup code and connects.
    pub async fn pair_and_connect(hub: &TestHub) -> Self {
        let code = hub.handle.setup_code().expect("a setup code");
        Self::pair_with_code_and_connect(hub, &code).await
    }

    pub async fn pair_with_code_and_connect(hub: &TestHub, code: &str) -> Self {
        let key = new_key();
        let (status, body) = pair_client_http(hub, code, &key).await;
        assert_eq!(status, 200, "{body}");
        let paired: PairClientResponse = serde_json::from_value(body).unwrap();
        Self::connect(hub, paired.client_id, key).await
    }

    pub async fn connect(hub: &TestHub, client_id: ClientId, key: SigningKey) -> Self {
        let mut peer = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
        authenticate(&mut peer, &hub.origin(), PeerId::Client(client_id), &key)
            .await
            .unwrap();
        let hello = peer
            .call_raw(
                "hello",
                serde_json::to_value(ClientHello {
                    client_id,
                    version: "test".to_owned(),
                    protocol_versions: vec![1],
                })
                .unwrap(),
            )
            .await
            .unwrap();
        Self {
            peer,
            client_id,
            key,
            hello: serde_json::from_value(hello).unwrap(),
            feed: Vec::new(),
        }
    }

    /// Sends a typed request and returns the typed response.
    pub async fn request<R: Request>(&mut self, body: R) -> Result<R::Response, ErrorBody> {
        let value = serde_json::to_value(&body).unwrap();
        let result = self.peer.call_raw(R::TYPE, value).await?;
        Ok(serde_json::from_value(result).unwrap())
    }

    pub async fn snapshot(&mut self, scope: SnapshotScope) -> Snapshot {
        self.request(SyncSnapshot { scope }).await.unwrap()
    }

    pub async fn tutti(&mut self) -> ChannelId {
        let snapshot = self.snapshot(SnapshotScope::All).await;
        snapshot
            .channels
            .iter()
            .find(|c| c.is_tutti())
            .expect("the Tutti channel")
            .id
    }

    pub async fn subscribe(&mut self, after_seq: u64) -> Result<Empty, ErrorBody> {
        self.request(SyncSubscribe {
            after_seq,
            channel_ids: None,
        })
        .await
    }

    /// Reads feed entries until `matches` accepts one. Entries read so far
    /// stay in `self.feed`.
    pub async fn wait_feed<F>(&mut self, mut matches: F) -> FeedEntry
    where
        F: FnMut(&FeedEntry) -> bool,
    {
        if let Some(found) = self.feed.iter().find(|e| matches(e)) {
            return found.clone();
        }
        loop {
            let frame = self
                .peer
                .next_frame(WAIT)
                .await
                .expect("the expected feed entry did not arrive");
            if let Frame::Event(event) = frame
                && let Ok(HubToClient::Feed(entry)) = event.decode::<HubToClient>()
            {
                self.feed.push(entry.clone());
                if matches(&entry) {
                    return entry;
                }
            }
        }
    }

    /// Drops the entries read so far, so the next wait looks only at new ones.
    pub fn forget_feed(&mut self) {
        self.feed.clear();
    }
}

pub fn upserted(entry: &FeedEntry) -> Option<&EntityPayload> {
    match &entry.change {
        FeedChange::Upsert(payload) => Some(payload),
        FeedChange::Delete(_) => None,
    }
}

// The fake worker ----------------------------------------------------------

pub struct FakeWorker {
    pub peer: Peer,
    pub worker_id: WorkerId,
    pub key: SigningKey,
    pub account_id: AccountId,
    pub hello_result: WorkerHelloResult,
    seqs: std::collections::HashMap<SessionId, u64>,
}

pub fn inventory(account_id: AccountId) -> Inventory {
    Inventory {
        hash: String::new(),
        hostname: "fake-host".to_owned(),
        os: Os::Macos,
        arch: "aarch64".to_owned(),
        tags: vec!["hub".to_owned()],
        accounts: vec![HarnessAccount {
            account_id,
            harness: "claude".to_owned(),
            label: "Claude Code".to_owned(),
            harness_version: Some("1.0.0".to_owned()),
            auth_state: AuthState::SignedIn,
            auth_detail: None,
            models: vec![
                ModelInfo {
                    id: "opus".to_owned(),
                    name: "Opus".to_owned(),
                    options: Vec::new(),
                },
                // A second model with effort levels, for the tests of a
                // model change.
                ModelInfo {
                    id: "sonnet".to_owned(),
                    name: "Sonnet".to_owned(),
                    options: vec![consort_proto::model::OptionDescriptor::Select {
                        id: "effort".to_owned(),
                        label: "Effort".to_owned(),
                        choices: ["low", "high"]
                            .iter()
                            .map(|v| consort_proto::model::OptionChoice {
                                value: (*v).to_owned(),
                                label: (*v).to_owned(),
                            })
                            .collect(),
                        default: Some("low".to_owned()),
                    }],
                },
            ],
            capabilities: Capabilities {
                steer: true,
                resume: true,
                approval_modes: ApprovalMode::ALL.to_vec(),
                ..Capabilities::default()
            },
            enabled: true,
            profile: false,
            email: None,
            plan: None,
            secret_names: Vec::new(),
            sign_in_methods: Vec::new(),
            extras_enabled: true,
            extras: None,
        }],
        local_mcp_servers: Vec::new(),
        checkouts: Vec::new(),
        policy: WorkerPolicy {
            max_approval_mode: ApprovalMode::FullAccess,
            allowed_roots: Vec::new(),
            allow_shell: true,
        },
        load: WorkerLoad::default(),
        tools: Vec::new(),
    }
    .with_hash()
}

impl FakeWorker {
    /// Pairs with a worker code, connects and sends an inventory.
    pub async fn pair_and_connect(hub: &TestHub) -> Self {
        let code = hub.handle.new_worker_pairing_code().await.unwrap();
        let key = new_key();
        let (status, body) = pair_worker_http(hub, &code, &key).await;
        assert_eq!(status, 200, "{body}");
        let paired: PairWorkerResponse = serde_json::from_value(body).unwrap();
        let account_id = AccountId::new();
        let mut worker = Self::connect(hub, paired.worker_id, key, account_id, Vec::new()).await;
        worker.send_inventory().await;
        worker
    }

    pub async fn connect(
        hub: &TestHub,
        worker_id: WorkerId,
        key: SigningKey,
        account_id: AccountId,
        sessions: Vec<OpenSession>,
    ) -> Self {
        let mut peer = Peer::connect(&hub.ws_url(WORKER_WS_PATH)).await;
        authenticate(&mut peer, &hub.origin(), PeerId::Worker(worker_id), &key)
            .await
            .unwrap();
        let result = peer
            .call_raw(
                "hello",
                serde_json::to_value(WorkerHello {
                    worker_id,
                    version: "test".to_owned(),
                    protocol_versions: vec![1],
                    sessions,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        Self {
            peer,
            worker_id,
            key,
            account_id,
            hello_result: serde_json::from_value(result).unwrap(),
            seqs: std::collections::HashMap::new(),
        }
    }

    /// Connects again with the same identity.
    pub async fn reconnect(self, hub: &TestHub, sessions: Vec<OpenSession>) -> Self {
        let FakeWorker {
            worker_id,
            key,
            account_id,
            seqs,
            peer,
            ..
        } = self;
        peer.close().await;
        let mut again = Self::connect(hub, worker_id, key, account_id, sessions).await;
        again.seqs = seqs;
        again
    }

    pub async fn send_inventory(&mut self) {
        let inventory = serde_json::to_value(inventory(self.account_id)).unwrap();
        self.peer
            .send(&Frame::Event(consort_proto::frame::EventFrame {
                kind: "inventory".to_owned(),
                body: inventory,
            }))
            .await;
    }

    /// The next hub command. Panics when none arrives in time.
    pub async fn next_command(&mut self) -> (RequestFrame, HubToWorker) {
        self.try_next_command(WAIT).await.expect("a hub command")
    }

    pub async fn try_next_command(
        &mut self,
        wait: Duration,
    ) -> Option<(RequestFrame, HubToWorker)> {
        loop {
            match self.peer.next_frame(wait).await? {
                Frame::Request(request) => {
                    let command: HubToWorker = request.decode().unwrap();
                    return Some((request, command));
                }
                _ => continue,
            }
        }
    }

    pub async fn respond_ok<T: Serialize>(&mut self, request: &RequestFrame, body: &T) {
        self.peer.send(&request.respond_ok(body).unwrap()).await;
    }

    pub async fn respond_error(&mut self, request: &RequestFrame, error: ErrorBody) {
        self.peer.send(&request.respond_error(error)).await;
    }

    /// Takes the next command, which must be `session.start`, and answers it.
    pub async fn accept_session_start(&mut self) -> consort_proto::worker_link::SessionStart {
        let (request, command) = self.next_command().await;
        let HubToWorker::SessionStart(start) = command else {
            panic!("expected session.start, got {}", command.type_name());
        };
        self.respond_ok(
            &request,
            &SessionStartResult {
                workspace_path: "/tmp/workspace".to_owned(),
                native_ref: Some("native-1".into()),
            },
        )
        .await;
        start
    }

    /// Sends session events with the next sequence numbers and returns the ack.
    pub async fn send_events(
        &mut self,
        session_id: SessionId,
        events: Vec<SessionEvent>,
    ) -> SessionEventsAck {
        let counter = self.seqs.entry(session_id).or_insert(0);
        let batch: Vec<SequencedEvent> = events
            .into_iter()
            .map(|event| {
                *counter += 1;
                SequencedEvent {
                    seq: *counter,
                    at: Timestamp::now(),
                    event,
                }
            })
            .collect();
        self.send_batch(session_id, batch).await
    }

    /// Sends events with the sequence numbers that the caller chose.
    pub async fn send_batch(
        &mut self,
        session_id: SessionId,
        events: Vec<SequencedEvent>,
    ) -> SessionEventsAck {
        let value = self
            .peer
            .call_raw(
                "session.events",
                serde_json::to_value(SessionEvents { session_id, events }).unwrap(),
            )
            .await
            .unwrap();
        serde_json::from_value(value).unwrap()
    }

    pub async fn tool_call(
        &mut self,
        session_id: SessionId,
        name: &str,
        arguments: Value,
    ) -> ToolCallResult {
        let value = self
            .peer
            .call_raw(
                "tool.call",
                serde_json::to_value(ToolCall {
                    session_id,
                    call_id: "call-1".to_owned(),
                    name: name.to_owned(),
                    arguments,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        serde_json::from_value(value).unwrap()
    }

    pub async fn tool_json(
        &mut self,
        session_id: SessionId,
        name: &str,
        arguments: Value,
    ) -> Value {
        let result = self.tool_call(session_id, name, arguments).await;
        assert!(!result.is_error, "{name} failed: {:?}", result.content);
        result.structured_content.unwrap_or(Value::Null)
    }
}

/// A worker whose connection is closed. It keeps its identity, so it can
/// connect again as the same worker.
pub struct AwayWorker {
    pub worker_id: WorkerId,
    key: SigningKey,
    account_id: AccountId,
}

impl FakeWorker {
    /// Closes the connection. The hub keeps the worker and its inventory.
    pub async fn go_away(self) -> AwayWorker {
        let FakeWorker {
            peer,
            worker_id,
            key,
            account_id,
            ..
        } = self;
        peer.close().await;
        AwayWorker {
            worker_id,
            key,
            account_id,
        }
    }
}

impl AwayWorker {
    /// Connects again with the same identity.
    pub async fn come_back(self, hub: &TestHub) -> FakeWorker {
        FakeWorker::connect(hub, self.worker_id, self.key, self.account_id, Vec::new()).await
    }
}

/// Polls until `check` returns Some.
pub async fn eventually<T, F, Fut>(mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Some(found) = check().await {
            return found;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the condition did not hold in time"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub fn worker_state(workers: &[Worker], id: WorkerId) -> consort_proto::model::WorkerState {
    workers.iter().find(|w| w.id == id).unwrap().state
}

// Builders for session events ------------------------------------------------

use consort_proto::ids::ItemId;
use consort_proto::model::{ApprovalDetails, RequestOption, RequestOptionKind};
use consort_proto::model::{
    AssistantMessagePayload, ItemPayload, ItemStatus, SessionItem, SessionState,
};
use consort_proto::worker_link::{QuestionDetails, RequestBody, RequestOpened, TurnEndReason};

pub fn state(state: SessionState) -> SessionEvent {
    SessionEvent::State { state }
}

pub fn turn_started(id: &str) -> SessionEvent {
    SessionEvent::TurnStarted { turn_id: id.into() }
}

pub fn turn_ended(id: &str, reason: TurnEndReason) -> SessionEvent {
    SessionEvent::TurnEnded {
        turn_id: id.into(),
        reason,
        error: None,
    }
}

pub fn assistant_text(session_id: SessionId, text: &str, status: ItemStatus) -> SessionEvent {
    SessionEvent::Item(SessionItem {
        id: ItemId::new(),
        session_id,
        turn_id: "t1".into(),
        payload: ItemPayload::AssistantMessage(AssistantMessagePayload {
            text: text.to_owned(),
        }),
        status,
        created_at: Timestamp::now(),
        updated_at: Timestamp::now(),
    })
}

pub fn approval_request(request_id: &str, command: &str) -> SessionEvent {
    SessionEvent::RequestOpened(RequestOpened {
        request_id: request_id.into(),
        summary: format!("Run {command}"),
        body: RequestBody::Approval(ApprovalDetails::Command {
            command: command.to_owned(),
            cwd: Some("/tmp/workspace".to_owned()),
        }),
        options: vec![
            RequestOption {
                id: "allow".to_owned(),
                label: "Allow once".to_owned(),
                kind: RequestOptionKind::AllowOnce,
                description: None,
            },
            RequestOption {
                id: "deny".to_owned(),
                label: "Deny".to_owned(),
                kind: RequestOptionKind::DenyOnce,
                description: None,
            },
        ],
    })
}

pub fn question_request(request_id: &str, question: &str) -> SessionEvent {
    SessionEvent::RequestOpened(RequestOpened {
        request_id: request_id.into(),
        summary: question.to_owned(),
        body: RequestBody::Question(QuestionDetails {
            question: question.to_owned(),
            context: None,
            allow_free_text: true,
        }),
        options: vec![
            RequestOption {
                id: "yes".to_owned(),
                label: "Yes".to_owned(),
                kind: RequestOptionKind::Choice,
                description: Some("Do it.".to_owned()),
            },
            RequestOption {
                id: "no".to_owned(),
                label: "No".to_owned(),
                kind: RequestOptionKind::Choice,
                description: None,
            },
        ],
    })
}

// A hub with a client and a worker ------------------------------------------

pub struct World {
    pub hub: TestHub,
    pub client: FakeClient,
    pub worker: FakeWorker,
    pub tutti: ChannelId,
    /// The coordinator session that `start_coordinator` started last.
    pub coordinator_session: Option<SessionId>,
}

impl World {
    /// A hub, a subscribed client and an online worker with one account.
    pub async fn start() -> Self {
        let hub = TestHub::start().await;
        let mut client = FakeClient::pair_and_connect(&hub).await;
        let worker = FakeWorker::pair_and_connect(&hub).await;
        let tutti = client.tutti().await;
        let snapshot = client.snapshot(SnapshotScope::All).await;
        client.subscribe(snapshot.seq).await.unwrap();
        // Wait until the hub has stored the inventory.
        let id = worker.worker_id;
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let workers: consort_proto::client_api::WorkerListResult = client
                .request(consort_proto::client_api::WorkerList {})
                .await
                .unwrap();
            if workers.workers.iter().any(|w| {
                w.id == id
                    && w.inventory.is_some()
                    && w.state == consort_proto::model::WorkerState::Online
            }) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the worker did not come online"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Self {
            hub,
            client,
            worker,
            tutti,
            coordinator_session: None,
        }
    }

    /// Sends a user message in a channel.
    pub async fn say(
        &mut self,
        channel_id: ChannelId,
        text: &str,
    ) -> consort_proto::model::Message {
        let result = self
            .client
            .request(consort_proto::client_api::MessageSend {
                channel_id,
                client_message_id: format!("cm-{}", ItemId::new()),
                body: text.to_owned(),
                attachments: Vec::new(),
                thread_root_id: None,
                in_timeline: false,
            })
            .await
            .unwrap();
        result.message
    }

    /// Waits for the coordinator session of Tutti to start and gets
    /// through its first turn. Returns the `session.start` command.
    pub async fn start_coordinator(
        &mut self,
        text: &str,
    ) -> consort_proto::worker_link::SessionStart {
        self.say(self.tutti, text).await;
        let start = self.worker.accept_session_start().await;
        self.worker
            .send_events(
                start.session_id,
                vec![state(SessionState::Active), turn_started("t1")],
            )
            .await;
        self.coordinator_session = Some(start.session_id);
        start
    }
}

impl World {
    /// A second connection of the worker exists. The user writes a message
    /// and the hub has to start a session on `back`.
    pub async fn worker_connected_say(&mut self, _back: &mut FakeWorker) {
        self.say(self.tutti, "Are you back?").await;
    }
}

/// A worker value that holds no connection, to swap into `World::worker`
/// while the real one is closed.
pub async fn placeholder_worker(hub: &TestHub) -> FakeWorker {
    // Another paired worker that stays connected. It has no inventory.
    let code = hub.handle.new_worker_pairing_code().await.unwrap();
    let key = new_key();
    let (status, body) = pair_worker_http(hub, &code, &key).await;
    assert_eq!(status, 200, "{body}");
    let paired: PairWorkerResponse = serde_json::from_value(body).unwrap();
    FakeWorker::connect(hub, paired.worker_id, key, AccountId::new(), Vec::new()).await
}

// Tasks -----------------------------------------------------------------------

impl World {
    /// Starts a coordinator, which starts a task. Returns the task ID and the
    /// `session.start` of the task session, which the worker accepted.
    pub async fn start_task(
        &mut self,
        brief: &str,
    ) -> (
        consort_proto::ids::TaskId,
        consort_proto::worker_link::SessionStart,
    ) {
        let coordinator = self.start_coordinator("Please do this").await;
        let started = self
            .worker
            .tool_json(
                coordinator.session_id,
                "start_task",
                serde_json::json!({"title": "Fix the bug", "brief": brief}),
            )
            .await;
        let task_id = started["task_id"].as_str().unwrap().parse().unwrap();
        let start = self.worker.accept_session_start().await;
        (task_id, start)
    }

    /// The task as the Tutti channel snapshot shows it.
    pub async fn task(&mut self, id: consort_proto::ids::TaskId) -> consort_proto::model::Task {
        self.client
            .snapshot(SnapshotScope::Channel(self.tutti))
            .await
            .tasks
            .into_iter()
            .find(|t| t.id == id)
            .unwrap()
    }

    /// Waits until the task has this status, and returns it.
    pub async fn wait_task(
        &mut self,
        id: consort_proto::ids::TaskId,
        status: consort_proto::status::TaskStatus,
    ) -> consort_proto::model::Task {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let task = self.task(id).await;
            if task.status == status {
                return task;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the task stayed {:?}, wanted {status:?}",
                task.status
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Retries a failed task. The worker answers the stop of the old session
    /// and the start of the new one. Returns the new session ID.
    pub async fn retry_task(
        &mut self,
        id: consort_proto::ids::TaskId,
        old: SessionId,
    ) -> SessionId {
        self.client
            .request(consort_proto::client_api::TaskRetry { task_id: id })
            .await
            .unwrap();
        let mut stopped = false;
        let mut new = None;
        while !stopped || new.is_none() {
            let (request, command) = self.worker.next_command().await;
            match command {
                HubToWorker::SessionStop(stop) => {
                    assert_eq!(stop.session_id, old);
                    self.worker
                        .respond_ok(&request, &serde_json::json!({}))
                        .await;
                    stopped = true;
                }
                HubToWorker::SessionStart(start) => {
                    self.worker
                        .respond_ok(
                            &request,
                            &SessionStartResult {
                                workspace_path: "/tmp/workspace-2".to_owned(),
                                native_ref: None,
                            },
                        )
                        .await;
                    new = Some(start.session_id);
                }
                other => panic!("unexpected command {}", other.type_name()),
            }
        }
        new.unwrap()
    }

    /// Waits until the hub shows the worker in this state.
    pub async fn wait_worker(&mut self, id: WorkerId, state: consort_proto::model::WorkerState) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let workers: consort_proto::client_api::WorkerListResult = self
                .client
                .request(consort_proto::client_api::WorkerList {})
                .await
                .unwrap();
            if worker_state(&workers.workers, id) == state {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the worker did not become {state:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Waits for a notice in a thread (or the timeline) whose body holds
    /// every one of `parts`.
    pub async fn wait_notice(
        &mut self,
        thread: Option<consort_proto::ids::MessageId>,
        parts: &[&str],
    ) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let notices = self.notices(thread).await;
            if notices.iter().any(|n| parts.iter().all(|p| n.contains(p))) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no notice with {parts:?} in {notices:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The bodies of the notices in a thread, or in the timeline when
    /// `thread` is `None`.
    pub async fn notices(&mut self, thread: Option<consort_proto::ids::MessageId>) -> Vec<String> {
        let page = self
            .client
            .request(consort_proto::client_api::MessageList {
                channel_id: self.tutti,
                thread_root_id: thread,
                before: None,
                limit: Some(200),
            })
            .await
            .unwrap();
        page.messages
            .into_iter()
            .filter(|m| m.kind == consort_proto::model::MessageKind::Notice)
            .map(|m| m.body)
            .collect()
    }
}
