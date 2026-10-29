//! The client against an in-process fake hub: pairing, the signed challenge,
//! hello, snapshot, subscribe, the feed, sending, answering an approval, and
//! a reconnect that catches up from the feed position.

// Tests may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{
        State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use consort_client::{
    ApprovalChoice, Client, Connection, TaskStatus, TimelineItem, ViewState, hub::ClientOptions,
    store::Store,
};
use consort_proto::{
    auth::{
        AuthChallenge, AuthResponse, ClientHelloResult, Nonce, PairClientRequest,
        PairClientResponse, auth_signing_payload, normalize_origin,
    },
    client_api::{
        ApprovalAnswer, ApprovalResult, EntityPayload, FeedEntry, MessageResult, MessageSend,
        Snapshot, SyncSubscribe,
    },
    error::{ErrorBody, ErrorCode},
    frame::{Empty, Frame, RequestFrame},
    ids::{ApprovalId, ChannelId, ClientId, MessageId, RequestId, SessionId, TaskId},
    model::{
        Approval, ApprovalDetails, ApprovalMode, ApprovalStatus, Author, Channel, ChannelKind,
        Isolation, Message, MessageKind, PlacementRequest, RequestOption, RequestOptionKind, Task,
        TaskCreator,
    },
    status::TaskStatus as PS,
    time::Timestamp,
};
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};

const CODE: &str = "ABCD-EFGH";

#[derive(Default)]
struct Hub {
    origin: String,
    key: Option<VerifyingKey>,
    client_id: Option<ClientId>,
    seq: u64,
    /// The request types of each WebSocket session, in order.
    sessions: Vec<Vec<String>>,
    channel: Option<Channel>,
    task: Option<Task>,
    approval: Option<Approval>,
    messages: Vec<Message>,
}

type Shared = Arc<Mutex<Hub>>;

fn ts(ms: i64) -> Timestamp {
    Timestamp::from_millis(ms)
}

fn seed(hub: &mut Hub) {
    let channel_id = ChannelId::new();
    let card = MessageId::new();
    let task_id = TaskId::new();
    let now = Timestamp::now().as_millis();
    hub.channel = Some(Channel {
        id: channel_id,
        kind: ChannelKind::Everything,
        name: "Everything".into(),
        icon: None,
        color: None,
        group_id: None,
        position: 0,
        purpose: "All channels, one memory".into(),
        instructions: String::new(),
        coordinator_selection: None,
        task_selection: None,
        preferred_workers: vec![],
        repo_ids: vec![],
        default_isolation: Isolation::Worktree,
        default_approval_mode: ApprovalMode::AutoEdits,
        memory_enabled: true,
        archived_at: None,
        created_at: ts(now - 10_000),
        updated_at: ts(now - 10_000),
    });
    hub.messages.push(Message {
        id: card,
        channel_id,
        thread_root_id: None,
        author: Author::Assistant,
        author_session_id: None,
        author_task_id: Some(task_id),
        author_routine_id: None,
        kind: MessageKind::TaskCard,
        body: String::new(),
        attachments: vec![],
        in_timeline: false,
        client_message_id: None,
        reply_count: 0,
        last_reply_at: None,
        reactions: vec![],
        edited_at: None,
        deleted_at: None,
        created_at: ts(now - 5_000),
    });
    hub.task = Some(Task {
        id: task_id,
        channel_id,
        title: "Build the Windows engine".into(),
        brief: "Build it.".into(),
        status: PS::NeedsInput,
        status_reason: None,
        created_by: TaskCreator::Assistant,
        card_message_id: card,
        model_selection: None,
        placement: PlacementRequest::default(),
        approval_mode: ApprovalMode::Ask,
        start_at: None,
        current_session_id: None,
        worker_id: None,
        review_summary: None,
        resolved_at: None,
        archived_at: None,
        created_at: ts(now - 5_000),
        updated_at: ts(now - 5_000),
    });
    hub.approval = Some(Approval {
        id: ApprovalId::new(),
        channel_id,
        session_id: SessionId::new(),
        task_id: Some(task_id),
        message_id: MessageId::new(),
        request_id: RequestId::new("req-1"),
        summary: "Codex wants to run a command".into(),
        details: ApprovalDetails::Command {
            command: "cargo build --release".into(),
            cwd: Some("C:\\Code\\sayso".into()),
        },
        options: vec![
            RequestOption {
                id: "allow".into(),
                label: "Allow once".into(),
                kind: RequestOptionKind::AllowOnce,
                description: None,
            },
            RequestOption {
                id: "deny".into(),
                label: "Deny".into(),
                kind: RequestOptionKind::DenyOnce,
                description: None,
            },
        ],
        status: ApprovalStatus::Pending,
        answer_option_id: None,
        answered_at: None,
        answered_by_client_id: None,
        created_at: ts(now - 4_000),
    });
}

async fn pair_client(
    State(hub): State<Shared>,
    Json(body): Json<PairClientRequest>,
) -> impl IntoResponse {
    if body.code.canonical() != CODE.replace('-', "") {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": {"code": "unauthorized", "message": "The code is not valid."}})),
        )
            .into_response();
    }
    let mut hub = hub.lock().unwrap();
    hub.key = Some(VerifyingKey::from_bytes(body.public_key.as_bytes()).unwrap());
    let id = ClientId::new();
    hub.client_id = Some(id);
    Json(PairClientResponse { client_id: id }).into_response()
}

async fn ws(State(hub): State<Shared>, upgrade: WebSocketUpgrade) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| session(hub, socket))
}

async fn send(socket: &mut WebSocket, frame: Frame) {
    socket
        .send(WsMessage::Text(frame.to_text().unwrap().into()))
        .await
        .unwrap();
}

async fn next_request(socket: &mut WebSocket) -> Option<RequestFrame> {
    while let Some(Ok(msg)) = socket.recv().await {
        if let WsMessage::Text(text) = msg
            && let Ok(Frame::Request(req)) = Frame::parse(text.as_str())
        {
            return Some(req);
        }
    }
    None
}

fn feed(hub: &mut Hub, payload: EntityPayload) -> Frame {
    hub.seq += 1;
    Frame::event(&FeedEntry::upsert(hub.seq, Timestamp::now(), payload)).unwrap()
}

async fn session(hub: Shared, mut socket: WebSocket) {
    let nonce = Nonce::from_bytes([9u8; 32]);
    let index = {
        let mut h = hub.lock().unwrap();
        h.sessions.push(Vec::new());
        h.sessions.len() - 1
    };
    send(&mut socket, Frame::event(&AuthChallenge { nonce }).unwrap()).await;
    while let Some(req) = next_request(&mut socket).await {
        hub.lock().unwrap().sessions[index].push(req.kind.clone());
        let reply = match req.kind.as_str() {
            "auth.response" => {
                let auth: AuthResponse = req.decode_as().unwrap();
                let h = hub.lock().unwrap();
                let payload = auth_signing_payload(&h.origin, &nonce);
                let sig = Signature::from_bytes(auth.signature.as_bytes());
                match h.key.map(|k| k.verify(&payload, &sig)) {
                    Some(Ok(())) => req.respond_ok(&Empty {}).unwrap(),
                    _ => {
                        req.respond_error(ErrorBody::new(ErrorCode::Unauthorized, "Bad signature."))
                    }
                }
            }
            "hello" => req
                .respond_ok(&ClientHelloResult {
                    protocol_version: 1,
                    hub_version: "test".into(),
                    feed_seq: hub.lock().unwrap().seq,
                })
                .unwrap(),
            "sync.snapshot" => {
                let h = hub.lock().unwrap();
                req.respond_ok(&Snapshot {
                    seq: h.seq,
                    channels: h.channel.clone().into_iter().collect(),
                    messages: h.messages.clone(),
                    tasks: h.task.clone().into_iter().collect(),
                    approvals: h.approval.clone().into_iter().collect(),
                    ..Snapshot::default()
                })
                .unwrap()
            }
            "sync.subscribe" => {
                let _: SyncSubscribe = req.decode_as().unwrap();
                req.respond_ok(&Empty {}).unwrap()
            }
            "message.send" => {
                let send_body: MessageSend = req.decode_as().unwrap();
                let message = Message {
                    id: MessageId::new(),
                    channel_id: send_body.channel_id,
                    thread_root_id: send_body.thread_root_id,
                    author: Author::User,
                    author_session_id: None,
                    author_task_id: None,
                    author_routine_id: None,
                    kind: MessageKind::Text,
                    body: send_body.body,
                    attachments: vec![],
                    in_timeline: false,
                    client_message_id: Some(send_body.client_message_id),
                    reply_count: 0,
                    last_reply_at: None,
                    reactions: vec![],
                    edited_at: None,
                    deleted_at: None,
                    created_at: Timestamp::now(),
                };
                let event = {
                    let mut h = hub.lock().unwrap();
                    h.messages.push(message.clone());
                    feed(&mut h, EntityPayload::Message(message.clone()))
                };
                send(
                    &mut socket,
                    req.respond_ok(&MessageResult { message }).unwrap(),
                )
                .await;
                event
            }
            "approval.answer" => {
                let answer: ApprovalAnswer = req.decode_as().unwrap();
                let (approval, events) = {
                    let mut h = hub.lock().unwrap();
                    let mut approval = h.approval.clone().unwrap();
                    assert_eq!(answer.option_id, "allow");
                    approval.status = ApprovalStatus::Allowed;
                    approval.answer_option_id = Some(answer.option_id);
                    h.approval = Some(approval.clone());
                    let mut task = h.task.clone().unwrap();
                    task.status = PS::Working;
                    h.task = Some(task.clone());
                    let a = feed(&mut h, EntityPayload::Approval(approval.clone()));
                    let t = feed(&mut h, EntityPayload::Task(task));
                    (approval, vec![a, t])
                };
                send(
                    &mut socket,
                    req.respond_ok(&ApprovalResult { approval }).unwrap(),
                )
                .await;
                for event in events {
                    send(&mut socket, event).await;
                }
                // Drop the link, so the client must reconnect and catch up.
                let _ = socket.send(WsMessage::Close(None)).await;
                return;
            }
            other => req.respond_error(ErrorBody::new(
                ErrorCode::Invalid,
                format!("The fake hub has no {other}."),
            )),
        };
        send(&mut socket, reply).await;
    }
}

async fn start_hub() -> (String, Shared) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    let hub = Arc::new(Mutex::new(Hub {
        origin: normalize_origin(&url).unwrap(),
        ..Hub::default()
    }));
    seed(&mut hub.lock().unwrap());
    let app = Router::new()
        .route("/v1/pair/client", post(pair_client))
        .route("/v1/client", get(ws))
        .with_state(hub.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, hub)
}

async fn wait_for(
    client: &Client,
    what: &str,
    pred: impl Fn(&ViewState) -> bool,
) -> Arc<ViewState> {
    let mut sub = client.subscribe();
    let current = sub.current();
    if pred(&current) {
        return current;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(deadline, sub.next()).await {
            Ok(Some(state)) if pred(&state) => return state,
            Ok(Some(_)) => {}
            _ => panic!(
                "timed out waiting for {what}; last state: {:?}",
                sub.current().connection
            ),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairs_syncs_sends_answers_and_reconnects() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    let client = Client::start_with(Some(store.clone()), ClientOptions::desktop()).unwrap();
    assert_eq!(client.state().connection, Connection::NotPaired);

    // A wrong code is refused with a clear error.
    let err = client
        .pair(url.clone(), "ZZZZ-ZZZZ".into())
        .await
        .unwrap_err();
    assert_eq!(err, consort_client::ClientError::BadCode);

    client.pair(url.clone(), CODE.into()).await.unwrap();
    assert!(store.load().unwrap().is_some(), "the identity is saved");
    let state = wait_for(&client, "the snapshot", |s| {
        s.connection == Connection::Online && s.everything().is_some()
    })
    .await;
    let everything = state.everything().unwrap().id.clone();
    let card = state
        .timeline(&everything)
        .and_then(|t| {
            t.items.iter().find_map(|i| match i {
                TimelineItem::TaskCard(c) => Some(c.clone()),
                _ => None,
            })
        })
        .expect("the task card is in the timeline");
    let approval = card
        .approval
        .clone()
        .expect("the approval shows on the card");
    assert_eq!(approval.folder.as_deref(), Some("C:\\Code\\sayso"));
    assert_eq!(state.tasks_panel.need_you, 1);

    client
        .send_message(everything.clone(), "Hello hub".into())
        .await
        .unwrap();
    wait_for(&client, "the sent message", |s| {
        s.timeline(&everything).is_some_and(|t| {
            t.items.iter().any(
                |i| matches!(i, TimelineItem::Message(m) if m.body == "Hello hub" && !m.pending),
            )
        })
    })
    .await;

    client
        .answer_approval(approval.id.clone(), ApprovalChoice::AllowOnce)
        .await
        .unwrap();
    let task_id = card.task_id.clone();
    wait_for(&client, "the task to work", |s| {
        s.task(&task_id)
            .is_some_and(|t| t.status == TaskStatus::Working)
    })
    .await;

    // The hub dropped the link. The client comes back and catches up from
    // its feed position instead of loading a new snapshot.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let sessions = hub.lock().unwrap().sessions.clone();
        if sessions.len() >= 2 && sessions[1].iter().any(|k| k == "sync.subscribe") {
            assert_eq!(
                sessions[1],
                vec!["auth.response", "hello", "sync.subscribe"]
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no reconnect: {sessions:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    wait_for(&client, "online again", |s| {
        s.connection == Connection::Online
    })
    .await;
    assert_eq!(
        hub.lock().unwrap().sessions[0][..4],
        ["auth.response", "hello", "sync.snapshot", "sync.subscribe"]
    );
}
