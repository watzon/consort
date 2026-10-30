//! Memory with expected hashes, the tool lists, search, blobs and files.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::common::*;
use consort_proto::auth::AuthHttpToken;
use consort_proto::auth::{
    HEADER_SIGNATURE, HEADER_TIMESTAMP, HEADER_WORKER_ID, blob_path, http_signing_payload,
};
use consort_proto::client_api::{
    FilesList, FilesPut, MemoryHistory, MemoryList, MemoryRead, MemoryWrite, MessageSearch,
    MessageSend,
};
use consort_proto::error::ErrorCode;
use consort_proto::model::{ExpectedHash, MemoryScope, Sha256Hash};
use consort_proto::time::Timestamp;
use ed25519_dalek::Signer;
use serde_json::json;
use sha2::{Digest, Sha256};

fn hash_of(bytes: &[u8]) -> Sha256Hash {
    Sha256Hash::from_bytes(Sha256::digest(bytes).into())
}

#[tokio::test]
async fn a_memory_write_with_a_stale_hash_is_refused() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let write = |content: &str, expected: ExpectedHash| MemoryWrite {
        scope: MemoryScope::Global,
        path: "people/kevin.md".to_owned(),
        content: content.to_owned(),
        expected_sha256: expected,
    };
    let first = client
        .request(write("# Kevin\nLikes tea.", ExpectedHash::NotExists))
        .await
        .unwrap()
        .note;
    assert_eq!(first.version, 1);

    // Another writer already created it, so not_exists is stale.
    let error = client
        .request(write("Overwrite", ExpectedHash::NotExists))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::StaleWrite);

    // A write with an old hash is stale too.
    let second = client
        .request(write(
            "# Kevin\nLikes coffee.",
            ExpectedHash::Sha256(first.sha256.clone()),
        ))
        .await
        .unwrap()
        .note;
    assert_eq!(second.version, 2);
    let error = client
        .request(write(
            "Late write",
            ExpectedHash::Sha256(first.sha256.clone()),
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::StaleWrite);

    let read = client
        .request(MemoryRead {
            scope: MemoryScope::Global,
            path: "people/kevin.md".to_owned(),
        })
        .await
        .unwrap()
        .note;
    assert_eq!(read.content, "# Kevin\nLikes coffee.");
    let list = client
        .request(MemoryList {
            scope: None,
            deleted: false,
        })
        .await
        .unwrap();
    // Kevin's note, beside the check-in note that the hub writes at start.
    assert_eq!(
        list.notes
            .iter()
            .filter(|n| n.path != consort_proto::model::CHECK_IN_NOTE_PATH)
            .count(),
        1
    );
    let history = client
        .request(MemoryHistory {
            scope: MemoryScope::Global,
            path: "people/kevin.md".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(history.versions.len(), 2);

    // A path must be a relative markdown path.
    let error = client
        .request(write("x", ExpectedHash::NotExists))
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::StaleWrite);
    let bad = client
        .request(MemoryWrite {
            scope: MemoryScope::Global,
            path: "../escape.txt".to_owned(),
            content: "x".to_owned(),
            expected_sha256: ExpectedHash::NotExists,
        })
        .await
        .unwrap_err();
    assert_eq!(bad.code, ErrorCode::Invalid);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_coordinator_writes_memory_with_the_hash_it_read_and_a_stale_write_fails() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Remember this").await;
    let sid = start.session_id;
    let written = world
        .worker
        .tool_json(sid, "memory_write", json!({"path": "prefs.md", "content": "Likes short answers.", "expected_sha256": "not_exists"}))
        .await;
    let hash = written["sha256"].as_str().unwrap().to_owned();
    // The index of memory is in the system prompt of a later session, and the
    // note is readable now.
    let read = world
        .worker
        .tool_json(sid, "memory_read", json!({"path": "prefs.md"}))
        .await;
    assert_eq!(read["content"], "Likes short answers.");
    assert_eq!(read["sha256"], hash);

    // The user changes the note meanwhile.
    let note = world
        .client
        .request(MemoryWrite {
            scope: MemoryScope::Global,
            path: "prefs.md".to_owned(),
            content: "Likes short answers. Hates emoji.".to_owned(),
            expected_sha256: ExpectedHash::Sha256(hash.parse().unwrap()),
        })
        .await
        .unwrap()
        .note;
    assert_eq!(note.version, 2);

    // The coordinator writes with the old hash and gets stale_write.
    let result = world
        .worker
        .tool_call(
            sid,
            "memory_write",
            json!({"path": "prefs.md", "content": "Overwrite.", "expected_sha256": hash}),
        )
        .await;
    assert!(result.is_error);
    assert_eq!(
        result.structured_content.unwrap()["error"]["code"],
        "stale_write"
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn each_kind_of_session_gets_its_own_tool_list() {
    let mut world = World::start().await;
    let coordinator = world.start_coordinator("Hi").await;
    let started = world
        .worker
        .tool_json(
            coordinator.session_id,
            "start_task",
            json!({"title": "T", "brief": "B"}),
        )
        .await;
    let _ = started;
    let task = world.worker.accept_session_start().await;

    let names = |value: serde_json::Value| -> Vec<String> {
        value["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    };
    async fn list(worker: &mut FakeWorker, session: SessionId) -> serde_json::Value {
        let call = serde_json::to_value(consort_proto::worker_link::ToolList {
            session_id: session,
        })
        .unwrap();
        worker.peer.call_raw("tool.list", call).await.unwrap()
    }
    use consort_proto::ids::SessionId;
    let tutti_tools = names(list(&mut world.worker, coordinator.session_id).await);
    assert!(tutti_tools.contains(&"post_message".to_owned()));
    assert!(tutti_tools.contains(&"list_channels".to_owned()));
    assert!(!tutti_tools.contains(&"post_update".to_owned()));
    let task_tools = names(list(&mut world.worker, task.session_id).await);
    assert!(task_tools.contains(&"post_update".to_owned()));
    assert!(task_tools.contains(&"submit_for_review".to_owned()));
    assert!(!task_tools.contains(&"post_message".to_owned()));

    // A task cannot call a coordinator tool or write memory.
    let denied = world
        .worker
        .tool_call(
            task.session_id,
            "post_message",
            json!({"body": "I am a task."}),
        )
        .await;
    assert!(denied.is_error);
    assert_eq!(
        denied.structured_content.unwrap()["error"]["code"],
        "policy_denied"
    );
    let denied = world
        .worker
        .tool_call(
            task.session_id,
            "memory_write",
            json!({"path": "x.md", "content": "x", "expected_sha256": "not_exists"}),
        )
        .await;
    assert!(denied.is_error);
    // A tool that does not exist is an error, not a crash.
    let unknown = world
        .worker
        .tool_call(task.session_id, "launch_missiles", json!({}))
        .await;
    assert!(unknown.is_error);
    // Bad arguments say which field is wrong.
    let invalid = world
        .worker
        .tool_call(coordinator.session_id, "reply", json!({"body": "x"}))
        .await;
    assert!(invalid.is_error);
    assert_eq!(
        invalid.structured_content.unwrap()["error"]["code"],
        "invalid"
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn suggestions_travel_one_level_up() {
    let mut world = World::start().await;
    let coordinator = world.start_coordinator("Hi").await;
    world
        .worker
        .send_events(
            coordinator.session_id,
            vec![turn_ended(
                "t1",
                consort_proto::worker_link::TurnEndReason::Completed,
            )],
        )
        .await;
    let started = world
        .worker
        .tool_json(
            coordinator.session_id,
            "start_task",
            json!({"title": "T", "brief": "B"}),
        )
        .await;
    let task_id = started["task_id"].as_str().unwrap().to_owned();
    let task = world.worker.accept_session_start().await;
    world
        .worker
        .tool_json(
            task.session_id,
            "suggest_memory",
            json!({"note": "Never push to main.", "reason": "The user said so twice."}),
        )
        .await;
    // The coordinator of the channel gets it in its next turn.
    let (_, command) = world.worker.next_command().await;
    let consort_proto::worker_link::HubToWorker::SessionTurn(turn) = command else {
        panic!("expected session.turn");
    };
    assert!(
        turn.turn
            .text
            .contains(&format!("<memory_suggestion from=\"{task_id}\"")),
        "{}",
        turn.turn.text
    );
    assert!(turn.turn.text.contains("Never push to main."));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn message_search_finds_messages_and_memory_with_a_tool() {
    let mut world = World::start().await;
    world
        .say(world.tutti, "The quarterly report is due on Friday.")
        .await;
    let hits = world
        .client
        .request(MessageSearch {
            query: "quarterly".to_owned(),
            channel_id: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(hits.hits.len(), 1);

    let start = world.start_coordinator("Find things").await;
    world
        .worker
        .tool_json(start.session_id, "memory_write", json!({"path": "work.md", "content": "Quarterly planning notes.", "expected_sha256": "not_exists"}))
        .await;
    let found = world
        .worker
        .tool_json(start.session_id, "search", json!({"query": "quarterly"}))
        .await;
    let kinds: Vec<_> = found["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["kind"].as_str().unwrap().to_owned())
        .collect();
    assert!(kinds.contains(&"message".to_owned()), "{kinds:?}");
    assert!(kinds.contains(&"memory_note".to_owned()), "{kinds:?}");
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn blobs_need_a_token_or_a_worker_signature_and_check_the_hash() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let token = client.request(AuthHttpToken {}).await.unwrap().token;
    let bytes = b"hello blob".to_vec();
    let hash = hash_of(&bytes);
    let url = format!("{}{}", hub.url, blob_path(&hash));
    let http = reqwest::Client::new();

    // No credentials.
    let response = http.put(&url).body(bytes.clone()).send().await.unwrap();
    assert_eq!(response.status(), 401);
    // A wrong token.
    let response = http
        .put(&url)
        .bearer_auth("nope")
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    // A body that does not match the hash.
    let response = http
        .put(&url)
        .bearer_auth(&token)
        .body(b"other".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    // The right upload.
    let response = http
        .put(&url)
        .bearer_auth(&token)
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response = http.get(&url).bearer_auth(&token).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().await.unwrap().as_ref(), bytes.as_slice());
    // Unknown blob.
    let missing = format!("{}{}", hub.url, blob_path(&hash_of(b"missing")));
    let response = http.get(&missing).bearer_auth(&token).send().await.unwrap();
    assert_eq!(response.status(), 404);
    // HEAD says whether the hub has a blob, and its size, without the body.
    let response = http.head(&url).bearer_auth(&token).send().await.unwrap();
    assert_eq!(response.status(), 200);
    // reqwest's content_length() gives the received body size, which is 0
    // for HEAD, so read the header.
    let length = response.headers()[reqwest::header::CONTENT_LENGTH]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(length, bytes.len().to_string());
    let response = http
        .head(&missing)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let response = http.head(&url).send().await.unwrap();
    assert_eq!(response.status(), 401, "HEAD needs a token too");

    // The blob becomes a channel file.
    let tutti = client.tutti().await;
    let file = client
        .request(FilesPut {
            channel_id: tutti,
            path: "notes/hello.txt".to_owned(),
            blob_sha256: hash.clone(),
            size: u64::try_from(bytes.len()).unwrap(),
            mime: "text/plain".to_owned(),
        })
        .await
        .unwrap()
        .file;
    assert_eq!(file.path, "notes/hello.txt");
    let listed = client
        .request(FilesList {
            channel_id: tutti,
            prefix: Some("notes/".to_owned()),
        })
        .await
        .unwrap();
    assert_eq!(listed.files.len(), 1);
    // A file whose blob was never uploaded is refused.
    let error = client
        .request(FilesPut {
            channel_id: tutti,
            path: "ghost.txt".to_owned(),
            blob_sha256: hash_of(b"ghost"),
            size: 5,
            mime: "text/plain".to_owned(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Invalid);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_worker_signs_its_blob_requests() {
    let hub = TestHub::start().await;
    let worker = FakeWorker::pair_and_connect(&hub).await;
    let bytes = b"artifact".to_vec();
    let hash = hash_of(&bytes);
    let path = blob_path(&hash);
    let url = format!("{}{path}", hub.url);
    let http = reqwest::Client::new();

    let sign = |method: &str, timestamp: Timestamp, key: &ed25519_dalek::SigningKey| {
        let payload =
            http_signing_payload(method, &hub.origin(), &path, timestamp, worker.worker_id);
        consort_proto::auth::Signature::from_bytes(key.sign(&payload).to_bytes()).to_hex()
    };
    let request = |method: reqwest::Method, timestamp: Timestamp, signature: String| {
        http.request(method, &url)
            .header(HEADER_WORKER_ID, worker.worker_id.to_string())
            .header(HEADER_TIMESTAMP, timestamp.as_millis().to_string())
            .header(HEADER_SIGNATURE, signature)
    };

    let now = Timestamp::now();
    let good = sign("PUT", now, &worker.key);
    let response = request(reqwest::Method::PUT, now, good.clone())
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // The signature covers the method, so it does not work for GET.
    let response = request(reqwest::Method::GET, now, good)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    // A signature from another key.
    let forged = sign("GET", now, &new_key());
    let response = request(reqwest::Method::GET, now, forged)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    // A request that is too old.
    let old = Timestamp::from_millis(now.as_millis() - 10 * 60 * 1000);
    let stale = sign("GET", old, &worker.key);
    let response = request(reqwest::Method::GET, old, stale)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    // A good GET works.
    let ok = sign("GET", now, &worker.key);
    let response = request(reqwest::Method::GET, now, ok).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().await.unwrap().as_ref(), bytes.as_slice());
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_message_attachment_and_thread_reply_counts_work_through_the_api() {
    let mut world = World::start().await;
    let root = world.say(world.tutti, "Root").await;
    let reply = world
        .client
        .request(MessageSend {
            channel_id: world.tutti,
            client_message_id: "reply-1".to_owned(),
            body: "A reply".to_owned(),
            attachments: Vec::new(),
            thread_root_id: Some(root.id),
            in_timeline: false,
        })
        .await
        .unwrap()
        .message;
    assert_eq!(reply.thread_root_id, Some(root.id));
    let snapshot = world
        .client
        .snapshot(consort_proto::client_api::SnapshotScope::Channel(
            world.tutti,
        ))
        .await;
    let root_now = snapshot.messages.iter().find(|m| m.id == root.id).unwrap();
    assert_eq!(root_now.reply_count, 1);
    world.hub.handle.shutdown().await.unwrap();
}
