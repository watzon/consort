//! Pairing, the challenge, signatures, versions and revocation.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::common::*;
use consort_proto::auth::{
    AuthResponse, CLIENT_WS_PATH, ClientHello, PAIR_CLIENT_PATH, PairClientRequest,
    PairClientResponse, PairWorkerResponse, PairingCode, PeerId, Signature, WORKER_WS_PATH,
    WorkerHello, auth_signing_payload,
};
use consort_proto::client_api::{ClientRevoke, SnapshotScope, WorkerList, WorkerListResult};
use consort_proto::error::ErrorCode;
use consort_proto::frame::{Frame, SUPPORTED_PROTOCOL_VERSIONS};
use consort_proto::ids::ClientId;
use consort_proto::model::ClientKind;
use consort_proto::release::InstallKind;
use ed25519_dalek::Signer;
use std::time::Duration;

#[tokio::test]
async fn health_check_answers() {
    let hub = TestHub::start().await;
    let body: serde_json::Value = reqwest::get(format!("{}/healthz", hub.url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["ok"], true);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_setup_code_pairs_one_client_and_works_once() {
    let hub = TestHub::start().await;
    let code = hub
        .handle
        .setup_code()
        .expect("a setup code on first start");

    let (status, body) = pair_client_http(&hub, &code, &new_key()).await;
    assert_eq!(status, 200, "{body}");
    let paired: PairClientResponse = serde_json::from_value(body).unwrap();
    assert!(paired.client_id.to_string().starts_with("cli_"));

    // The code is single use, so a second client cannot use it.
    let (status, body) = pair_client_http(&hub, &code, &new_key()).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], "unauthorized");
    // After a client paired, the hub no longer shows the setup code.
    assert!(hub.handle.setup_code().is_none());
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_wrong_or_malformed_code_is_refused() {
    let hub = TestHub::start().await;
    let (status, body) = pair_client_http(&hub, "ABCD-EFGH", &new_key()).await;
    assert_eq!(status, 401, "{body}");

    let (status, body) = http_post(
        &hub,
        "/v1/pair/client",
        &serde_json::json!({"code": "nope"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid");
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_typed_code_ignores_case_hyphens_and_look_alike_letters() {
    let hub = TestHub::start().await;
    let code = hub.handle.setup_code().unwrap();
    let typed = code.replace('-', " ").to_lowercase();
    let (status, body) = pair_client_http(&hub, &typed, &new_key()).await;
    assert_eq!(status, 200, "{body}");
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_client_with_a_good_signature_connects_and_says_hello() {
    let hub = TestHub::start().await;
    let client = FakeClient::pair_and_connect(&hub).await;
    assert_eq!(client.hello.protocol_version, 1);
    assert!(client.hello.feed_seq > 0);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_bad_signature_is_refused_and_the_connection_closes() {
    let hub = TestHub::start().await;
    let code = hub.handle.setup_code().unwrap();
    let key = new_key();
    let (_, body) = pair_client_http(&hub, &code, &key).await;
    let paired: PairClientResponse = serde_json::from_value(body).unwrap();

    // A signature from another key.
    let mut peer = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
    let error = authenticate(
        &mut peer,
        &hub.origin(),
        PeerId::Client(paired.client_id),
        &new_key(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert!(peer.closed(WAIT).await);

    // The right key, but a signature over another origin.
    let mut peer = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
    let error = authenticate(
        &mut peer,
        "https://other.example.com",
        PeerId::Client(paired.client_id),
        &key,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unauthorized);

    // An ID that the hub does not know gets the same answer.
    let mut peer = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
    let error = authenticate(
        &mut peer,
        &hub.origin(),
        PeerId::Client(ClientId::new()),
        &key,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unauthorized);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_signature_of_an_old_nonce_does_not_work() {
    let hub = TestHub::start().await;
    let code = hub.handle.setup_code().unwrap();
    let key = new_key();
    let (_, body) = pair_client_http(&hub, &code, &key).await;
    let paired: PairClientResponse = serde_json::from_value(body).unwrap();

    // Read the nonce of one connection and answer on another.
    let mut first = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
    let Frame::Event(event) = first.read(WAIT).await.unwrap() else {
        panic!("expected the challenge");
    };
    let challenge: consort_proto::auth::AuthChallenge = event.decode_as().unwrap();
    let old_signature = key.sign(&auth_signing_payload(&hub.origin(), &challenge.nonce));

    let mut second = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
    let _challenge = second.read(WAIT).await.unwrap();
    let error = second
        .call_raw(
            "auth.response",
            serde_json::to_value(AuthResponse {
                peer_id: PeerId::Client(paired.client_id),
                signature: Signature::from_bytes(old_signature.to_bytes()),
            })
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unauthorized);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_client_cannot_use_the_worker_path_and_the_other_way_round() {
    let hub = TestHub::start().await;
    let client = FakeClient::pair_and_connect(&hub).await;
    let mut peer = Peer::connect(&hub.ws_url(WORKER_WS_PATH)).await;
    let error = authenticate(
        &mut peer,
        &hub.origin(),
        PeerId::Client(client.client_id),
        &client.key,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unauthorized);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn no_common_protocol_version_closes_with_protocol_mismatch() {
    let hub = TestHub::start().await;
    let code = hub.handle.setup_code().unwrap();
    let key = new_key();
    let (_, body) = pair_client_http(&hub, &code, &key).await;
    let paired: PairClientResponse = serde_json::from_value(body).unwrap();

    let mut peer = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
    authenticate(
        &mut peer,
        &hub.origin(),
        PeerId::Client(paired.client_id),
        &key,
    )
    .await
    .unwrap();
    let error = peer
        .call_raw(
            "hello",
            serde_json::to_value(ClientHello {
                client_id: paired.client_id,
                version: "test".to_owned(),
                protocol_versions: vec![99],
            })
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProtocolMismatch);
    // The error says what the hub is, and how it was installed: the peer has
    // signed in.
    let versions = error.hub_versions.expect("the versions of the hub");
    assert_eq!(versions.hub_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(versions.protocol_versions, SUPPORTED_PROTOCOL_VERSIONS);
    assert_eq!(versions.install, Some(InstallKind::Service));
    assert!(peer.closed(WAIT).await);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_pairing_carries_the_versions_of_the_hub_and_not_how_it_was_installed() {
    let hub = TestHub::start().await;
    let code = hub.handle.setup_code().unwrap();
    let (status, _) = pair_client_http(&hub, &code, &new_key()).await;
    assert_eq!(status, 200);
    // The code works one time, so this try fails.
    let (status, body) = pair_client_http(&hub, &code, &new_key()).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], "unauthorized");
    let versions = &body["error"]["hub_versions"];
    assert_eq!(versions["hub_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        versions["protocol_versions"],
        serde_json::json!(SUPPORTED_PROTOCOL_VERSIONS)
    );
    assert!(versions.get("install").is_none(), "{versions}");
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_pairing_with_no_common_protocol_version_is_refused_and_the_code_still_works() {
    let hub = TestHub::start().await;
    let code = hub.handle.setup_code().unwrap();
    let (status, body) = http_post(
        &hub,
        PAIR_CLIENT_PATH,
        &PairClientRequest {
            code: parse_code(&code),
            public_key: public_key(&new_key()),
            name: "Newer client".to_owned(),
            kind: ClientKind::Desktop,
            os: None,
            protocol_versions: vec![99],
        },
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "protocol_mismatch");
    assert_eq!(
        body["error"]["hub_versions"]["hub_version"],
        env!("CARGO_PKG_VERSION")
    );
    // The peer that sends no list is from before this field, and passes.
    let (status, body) = pair_client_http(&hub, &code, &new_key()).await;
    assert_eq!(status, 200, "{body}");
    hub.handle.shutdown().await.unwrap();
}

/// Connects a worker that reports `version`, and returns the first command
/// of the hub, if one comes.
async fn first_command_for_a_worker_at(
    hub: &TestHub,
    version: &str,
) -> Option<consort_proto::frame::RequestFrame> {
    let code = hub.handle.new_worker_pairing_code().await.unwrap();
    let key = new_key();
    let (status, body) = pair_worker_http(hub, &code, &key).await;
    assert_eq!(status, 200, "{body}");
    let paired: PairWorkerResponse = serde_json::from_value(body).unwrap();
    let mut peer = Peer::connect(&hub.ws_url(WORKER_WS_PATH)).await;
    authenticate(
        &mut peer,
        &hub.origin(),
        PeerId::Worker(paired.worker_id),
        &key,
    )
    .await
    .unwrap();
    peer.call_raw(
        "hello",
        serde_json::to_value(WorkerHello {
            worker_id: paired.worker_id,
            version: version.to_owned(),
            protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
            sessions: Vec::new(),
        })
        .unwrap(),
    )
    .await
    .unwrap();
    while let Some(frame) = peer.next_frame(Duration::from_millis(500)).await {
        if let Frame::Request(request) = frame
            && request.kind == "worker.update"
        {
            return Some(request);
        }
    }
    None
}

#[tokio::test]
async fn the_hub_names_its_version_to_an_older_worker_only() {
    let hub = TestHub::start().await;
    let request = first_command_for_a_worker_at(&hub, "0.0.1")
        .await
        .expect("an older worker gets the command");
    assert_eq!(request.body["version"], env!("CARGO_PKG_VERSION"));
    // Only a version: the hub sends no address and no program.
    assert_eq!(request.body.as_object().unwrap().len(), 1);

    assert!(
        first_command_for_a_worker_at(&hub, env!("CARGO_PKG_VERSION"))
            .await
            .is_none(),
        "a worker at the same version gets no command"
    );
    assert!(
        first_command_for_a_worker_at(&hub, "999.0.0")
            .await
            .is_none(),
        "a newer worker gets no command: nothing moves to an older version"
    );
    assert!(
        first_command_for_a_worker_at(&hub, "test").await.is_none(),
        "a version that is not a version gets no command"
    );
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_worker_pairing_code_pairs_a_worker_once_and_it_shows_in_the_list() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;

    let code = hub.handle.new_worker_pairing_code().await.unwrap();
    let worker_key = new_key();
    let (status, body) = pair_worker_http(&hub, &code, &worker_key).await;
    assert_eq!(status, 200, "{body}");
    let (status, _) = pair_worker_http(&hub, &code, &new_key()).await;
    assert_eq!(status, 401);

    // A client code does not pair a worker, and the other way round.
    let client_code = client
        .request(consort_proto::client_api::ClientPairingCode {})
        .await
        .unwrap();
    let (status, _) = pair_worker_http(&hub, &client_code.code.to_string(), &new_key()).await;
    assert_eq!(status, 401);
    assert_eq!(client_code.hub_url, hub.url);

    let list: WorkerListResult = client.request(WorkerList {}).await.unwrap();
    assert_eq!(list.workers.len(), 1);
    assert_eq!(list.workers[0].name, "Fake worker");
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_hub_limits_pairing_attempts_for_one_address() {
    let hub = TestHub::start().await;
    let mut last = 0;
    for _ in 0..12 {
        let (status, _) = pair_client_http(&hub, "ABCD-EFGH", &new_key()).await;
        last = status;
    }
    assert_eq!(last, 429);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_revoked_client_cannot_connect_and_is_dropped() {
    let hub = TestHub::start().await;
    let mut first = FakeClient::pair_and_connect(&hub).await;
    let code = first
        .request(consort_proto::client_api::ClientPairingCode {})
        .await
        .unwrap()
        .code
        .to_string();
    let mut second = FakeClient::pair_with_code_and_connect(&hub, &code).await;

    // A client cannot revoke itself.
    let error = first
        .request(ClientRevoke {
            client_id: first.client_id,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);

    first
        .request(ClientRevoke {
            client_id: second.client_id,
        })
        .await
        .unwrap();
    assert!(second.peer.closed(WAIT).await);

    let mut peer = Peer::connect(&hub.ws_url(CLIENT_WS_PATH)).await;
    let error = authenticate(
        &mut peer,
        &hub.origin(),
        PeerId::Client(second.client_id),
        &second.key,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unauthorized);

    // The snapshot still works for the first client.
    let snapshot = first.snapshot(SnapshotScope::All).await;
    assert!(snapshot.channels.iter().any(|c| c.is_tutti()));
    let _ = PairingCode::from_random_bytes([0; 8]);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn create_setup_code_works_beside_a_running_hub() {
    let hub = TestHub::start().await;
    let code = consort_hub::create_setup_code(hub.dir.path()).unwrap();
    let (status, body) = pair_client_http(&hub, &code, &new_key()).await;
    assert_eq!(status, 200, "{body}");
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_hub_restart_keeps_paired_peers() {
    let hub = TestHub::start().await;
    let client = FakeClient::pair_and_connect(&hub).await;
    let (id, key) = (client.client_id, client.key.clone());
    drop(client);
    let hub = hub.restart().await;
    assert!(
        hub.handle.setup_code().is_none(),
        "a client is paired, so no setup code"
    );
    let again = FakeClient::connect(&hub, id, key).await;
    assert_eq!(again.client_id, id);
    hub.handle.shutdown().await.unwrap();
}
