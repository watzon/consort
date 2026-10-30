//! A revoke during the handshake, the cap on pending handshakes, and blob
//! uploads that the hub refuses before it reads the body.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::auth::{
    CLIENT_WS_PATH, ClientHello, PairWorkerResponse, PeerId, WORKER_WS_PATH, WorkerHello,
};
use consort_proto::client_api::{
    ClientPairingCode, ClientPairingCodeResult, ClientRevoke, WorkerList, WorkerRevoke,
};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{Frame, FrameId, RequestFrame};
use consort_proto::ids::ClientId;
use consort_proto::model::WorkerState;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Sends a request and waits for its response. `None` when the connection
/// closes or nothing comes in `wait`.
async fn call_or_none(
    peer: &mut Peer,
    kind: &str,
    body: Value,
    wait: Duration,
) -> Option<Result<Value, ErrorBody>> {
    let id = FrameId::generate();
    peer.send(&Frame::Request(RequestFrame {
        id: id.clone(),
        kind: kind.to_owned(),
        body,
    }))
    .await;
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match peer.read(left).await {
            Some(Frame::Response(r)) if r.re == id => return Some(r.result),
            Some(_) => continue,
            None => return None,
        }
    }
}

#[tokio::test]
async fn a_client_revoked_between_its_signature_and_its_hello_gets_no_access() {
    let hub = TestHub::start().await;
    let mut admin = FakeClient::pair_and_connect(&hub).await;
    let code: ClientPairingCodeResult = admin.request(ClientPairingCode {}).await.unwrap();
    let key = new_key();
    let (status, body) = pair_client_http(&hub, &code.code.to_string(), &key).await;
    assert_eq!(status, 200, "{body}");
    let id: ClientId = serde_json::from_value(body["client_id"].clone()).unwrap();

    // The key signs the challenge, then the connection waits before hello.
    let mut parked = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
    authenticate(&mut parked, &hub.origin(), PeerId::Client(id), &key)
        .await
        .unwrap();
    admin.request(ClientRevoke { client_id: id }).await.unwrap();

    let hello = serde_json::to_value(ClientHello {
        client_id: id,
        version: "test".to_owned(),
        protocol_versions: vec![1],
    })
    .unwrap();
    match call_or_none(&mut parked, "hello", hello, WAIT).await {
        Some(Err(error)) => assert_eq!(error.code, ErrorCode::Unauthorized),
        None => {}
        Some(Ok(_)) => panic!("a revoked client finished its hello"),
    }
    // The connection is closed, so it cannot ask for anything, for example a
    // pairing code for a new client.
    let minted = call_or_none(
        &mut parked,
        "client.pairing_code",
        json!({}),
        Duration::from_secs(2),
    )
    .await;
    assert!(
        !matches!(minted, Some(Ok(_))),
        "a revoked client made a pairing code"
    );
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_worker_revoked_between_its_signature_and_its_hello_stays_offline() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let code = hub.handle.new_worker_pairing_code().await.unwrap();
    let key = new_key();
    let (status, body) = pair_worker_http(&hub, &code, &key).await;
    assert_eq!(status, 200, "{body}");
    let worker_id = serde_json::from_value::<PairWorkerResponse>(body)
        .unwrap()
        .worker_id;

    let mut parked = Peer::connect(&hub.ws_url(WORKER_WS_PATH)).await;
    authenticate(&mut parked, &hub.origin(), PeerId::Worker(worker_id), &key)
        .await
        .unwrap();
    client.request(WorkerRevoke { worker_id }).await.unwrap();

    let hello = serde_json::to_value(WorkerHello {
        worker_id,
        version: "test".to_owned(),
        protocol_versions: vec![1],
        sessions: Vec::new(),
    })
    .unwrap();
    match call_or_none(&mut parked, "hello", hello, WAIT).await {
        Some(Err(error)) => assert_eq!(error.code, ErrorCode::Unauthorized),
        None => {}
        Some(Ok(_)) => panic!("a revoked worker finished its hello"),
    }
    let workers = client.request(WorkerList {}).await.unwrap().workers;
    assert_eq!(worker_state(&workers, worker_id), WorkerState::Offline);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_hub_caps_connections_from_one_address_that_have_not_finished_the_handshake() {
    let hub = TestHub::start().await;
    // MAX_PENDING_HANDSHAKES_PER_ADDRESS in net/ws.rs. Every test connection
    // comes from 127.0.0.1. The cap for all addresses together has a unit
    // test in net/ws.rs.
    const SLOTS: usize = 16;
    let mut parked = Vec::new();
    for _ in 0..SLOTS {
        let mut peer = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
        // The challenge shows that the connection holds a slot.
        assert!(matches!(peer.read(WAIT).await, Some(Frame::Event(_))));
        parked.push(peer);
    }
    // One more gets no challenge. The hub closes it at once.
    let mut extra = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
    assert!(extra.read(Duration::from_secs(2)).await.is_none());

    // A slot is free again when a connection leaves the handshake.
    parked.pop().unwrap().close().await;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let mut next = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
        if matches!(
            next.read(Duration::from_secs(1)).await,
            Some(Frame::Event(_))
        ) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no slot came free");
    }
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_blob_upload_without_credentials_is_refused_before_the_body_arrives() {
    let hub = TestHub::start().await;
    let mut stream = tokio::net::TcpStream::connect(hub.addr).await.unwrap();
    // The request says 100 MB but sends only 1 KiB. A hub that reads the
    // body before it checks the headers waits for the rest.
    let head = format!(
        "PUT /v1/blobs/{} HTTP/1.1\r\nHost: {}\r\nContent-Length: 100000000\r\n\r\n",
        "0".repeat(64),
        hub.addr
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(&[0_u8; 1024]).await.unwrap();
    let mut answer = vec![0_u8; 64];
    let read = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut answer))
        .await
        .expect("the hub answered only after the whole body")
        .unwrap();
    let text = String::from_utf8_lossy(&answer[..read]).to_string();
    assert!(text.starts_with("HTTP/1.1 401"), "{text}");
    hub.handle.shutdown().await.unwrap();
}

/// Review cycle 2, finding 2: each ping restarted the 15 s handshake
/// timeout, so sockets that only pinged held their slots for ever. The
/// handshake now has one deadline from the challenge to `hello`.
#[tokio::test]
async fn pings_do_not_keep_a_connection_in_the_handshake_past_its_deadline() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let hub = TestHub::start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(hub.ws_url(WORKER_WS_PATH))
        .await
        .unwrap();
    let first = ws.next().await.unwrap().unwrap();
    assert!(matches!(first, WsMessage::Text(_)), "{first:?}");
    let started = tokio::time::Instant::now();
    let mut ping = tokio::time::interval(Duration::from_secs(2));
    let close_code = loop {
        tokio::select! {
            _ = ping.tick() => {
                if ws.send(WsMessage::Ping(Vec::new().into())).await.is_err() {
                    break None;
                }
            }
            next = ws.next() => match next {
                Some(Ok(WsMessage::Close(frame))) => break frame.map(|f| u16::from(f.code)),
                Some(Ok(_)) => {}
                None | Some(Err(_)) => break None,
            },
        }
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "the hub kept a pinging socket in the handshake"
        );
    };
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(14) && elapsed < Duration::from_secs(20),
        "closed after {elapsed:?}"
    );
    assert_eq!(
        close_code,
        Some(1008),
        "the hub closes with handshake timed out"
    );
    hub.handle.shutdown().await.unwrap();
}
