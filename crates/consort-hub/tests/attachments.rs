//! Attachments and channel files: user messages, coordinator turns, task
//! sessions, hub tools, artifacts and attachment notices.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::auth::{
    AuthHttpToken, HEADER_SIGNATURE, HEADER_TIMESTAMP, HEADER_WORKER_ID, blob_path,
    http_signing_payload,
};
use consort_proto::client_api::{
    FilesList, FilesPut, MessageList, MessageSend, WorkerList, WorkerListResult,
};
use consort_proto::error::ErrorCode;
use consort_proto::frame::{EventFrame, Frame, MessageFamily};
use consort_proto::ids::{MessageId, SessionId, TaskId};
use consort_proto::model::{
    Author, BlobRef, ChannelFile, ItemStatus, Message, SessionState, Sha256Hash,
};
use consort_proto::status::TaskStatus;
use consort_proto::time::Timestamp;
use consort_proto::tools::ToolCallResult;
use consort_proto::worker_link::{HubToWorker, SessionEvent, SessionStart, TurnEndReason};
use ed25519_dalek::Signer;
use serde_json::json;
use sha2::{Digest, Sha256};

// Helpers -----------------------------------------------------------------------

fn hash_of(bytes: &[u8]) -> Sha256Hash {
    Sha256Hash::from_bytes(Sha256::digest(bytes).into())
}

fn size_of(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap()
}

/// Uploads a blob with the HTTP token of the client.
async fn upload_blob(world: &mut World, bytes: &[u8]) -> Sha256Hash {
    let token = world.client.request(AuthHttpToken {}).await.unwrap().token;
    let hash = hash_of(bytes);
    let url = format!("{}{}", world.hub.url, blob_path(&hash));
    let response = reqwest::Client::new()
        .put(url)
        .bearer_auth(token)
        .body(bytes.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    hash
}

/// Uploads a blob as the worker, with a signed request.
async fn worker_upload_blob(world: &World, bytes: &[u8]) -> Sha256Hash {
    let hash = hash_of(bytes);
    let path = blob_path(&hash);
    let now = Timestamp::now();
    let payload = http_signing_payload(
        "PUT",
        &world.hub.origin(),
        &path,
        now,
        world.worker.worker_id,
    );
    let signature =
        consort_proto::auth::Signature::from_bytes(world.worker.key.sign(&payload).to_bytes())
            .to_hex();
    let response = reqwest::Client::new()
        .put(format!("{}{path}", world.hub.url))
        .header(HEADER_WORKER_ID, world.worker.worker_id.to_string())
        .header(HEADER_TIMESTAMP, now.as_millis().to_string())
        .header(HEADER_SIGNATURE, signature)
        .body(bytes.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    hash
}

/// Uploads a blob and makes it a channel file.
async fn put_file(
    world: &mut World,
    path: &str,
    bytes: &[u8],
    mime: &str,
) -> (ChannelFile, Sha256Hash) {
    let hash = upload_blob(world, bytes).await;
    let file = world
        .client
        .request(FilesPut {
            channel_id: world.tutti,
            path: path.to_owned(),
            blob_sha256: hash.clone(),
            size: size_of(bytes),
            mime: mime.to_owned(),
        })
        .await
        .unwrap()
        .file;
    (file, hash)
}

/// Uploads a blob and returns the attachment for it.
async fn attachment(world: &mut World, name: &str, bytes: &[u8]) -> BlobRef {
    let sha256 = upload_blob(world, bytes).await;
    BlobRef {
        sha256,
        name: name.to_owned(),
        mime: "text/plain".to_owned(),
        size: size_of(bytes),
    }
}

/// Sends a user message with attachments.
async fn say_with(
    world: &mut World,
    text: &str,
    attachments: Vec<BlobRef>,
    thread_root_id: Option<MessageId>,
) -> Message {
    world
        .client
        .request(MessageSend {
            channel_id: world.tutti,
            client_message_id: format!("cm-{text}"),
            body: text.to_owned(),
            attachments,
            thread_root_id,
            in_timeline: false,
        })
        .await
        .unwrap()
        .message
}

/// Ends the open turn of a session.
async fn end_turn(world: &mut World, session_id: SessionId) {
    world
        .worker
        .send_events(session_id, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
}

/// The next command, which must be `session.turn`.
async fn next_turn(world: &mut World) -> consort_proto::worker_link::SessionTurn {
    let (request, command) = world.worker.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!("expected session.turn, got {}", command.type_name());
    };
    world
        .worker
        .respond_ok(
            &request,
            &consort_proto::worker_link::SessionTurnResult {
                turn_id: "t2".into(),
            },
        )
        .await;
    turn
}

/// The messages of a thread, or of the timeline for `None`.
async fn messages(world: &mut World, thread: Option<MessageId>) -> Vec<Message> {
    world
        .client
        .request(MessageList {
            channel_id: world.tutti,
            thread_root_id: thread,
            before: None,
            limit: Some(200),
        })
        .await
        .unwrap()
        .messages
}

fn paths(files: &[consort_proto::model::FileRef]) -> Vec<&str> {
    let mut out: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    out.sort_unstable();
    out
}

fn error_text(result: &ToolCallResult) -> String {
    format!("{:?}", result.content)
}

/// Starts a coordinator and a task with an optional list of attachments.
/// Returns the task ID and the `session.start` of the task session.
async fn start_task_with(
    world: &mut World,
    coordinator: SessionId,
    attachments: &[&str],
) -> (TaskId, SessionStart) {
    let started = world
        .worker
        .tool_json(
            coordinator,
            "start_task",
            json!({"title": "Fix the bug", "brief": "Do it", "attachments": attachments}),
        )
        .await;
    let task_id: TaskId = started["task_id"].as_str().unwrap().parse().unwrap();
    let start = world.worker.accept_session_start().await;
    (task_id, start)
}

// Tests -------------------------------------------------------------------------

#[tokio::test]
async fn a_user_attachment_reaches_the_coordinator_turn_and_a_missing_blob_is_refused() {
    let mut world = World::start().await;
    let first = attachment(&mut world, "first.txt", b"first file").await;
    // The size in the call is wrong on purpose. The hub takes the stored one.
    let wrong = BlobRef {
        size: 1,
        ..first.clone()
    };
    let message = say_with(&mut world, "Look at this", vec![wrong], None).await;
    assert_eq!(message.attachments, vec![first.clone()]);

    let start = world.worker.accept_session_start().await;
    let turn = start.turn.expect("a first turn");
    assert_eq!(turn.attachments, vec![first]);
    let sid = start.session_id;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    end_turn(&mut world, sid).await;

    let second = attachment(&mut world, "second.txt", b"second file").await;
    say_with(&mut world, "And this", vec![second.clone()], None).await;
    let turn = next_turn(&mut world).await;
    assert_eq!(turn.session_id, sid);
    assert_eq!(turn.turn.attachments, vec![second]);

    // A blob that was never uploaded is refused.
    let ghost = BlobRef {
        sha256: hash_of(b"never uploaded"),
        name: "ghost.txt".to_owned(),
        mime: "text/plain".to_owned(),
        size: 14,
    };
    let error = world
        .client
        .request(MessageSend {
            channel_id: world.tutti,
            client_message_id: "cm-ghost".to_owned(),
            body: "A ghost".to_owned(),
            attachments: vec![ghost],
            thread_root_id: None,
            in_timeline: false,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Invalid);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_channel_files_reach_a_session_when_they_change() {
    let mut world = World::start().await;
    let (first, _) = put_file(&mut world, "docs/a.txt", b"aaa", "text/plain").await;
    let start = world.start_coordinator("Hello").await;
    assert_eq!(paths(&start.channel_files), vec!["docs/a.txt"]);
    assert_eq!(start.channel_files[0].sha256, first.blob_sha256);
    let sid = start.session_id;
    end_turn(&mut world, sid).await;

    // Nothing changed, so the next turn has no list.
    world.say(world.tutti, "Again").await;
    let turn = next_turn(&mut world).await;
    assert_eq!(turn.channel_files, None);
    end_turn(&mut world, sid).await;

    // A new file changes the list, and the turn after it carries the list.
    put_file(&mut world, "docs/b.txt", b"bbb", "text/plain").await;
    world.say(world.tutti, "Once more").await;
    let turn = next_turn(&mut world).await;
    let list = turn.channel_files.expect("a changed list");
    assert_eq!(paths(&list), vec!["docs/a.txt", "docs/b.txt"]);
    end_turn(&mut world, sid).await;

    // The list did not change again.
    world.say(world.tutti, "Last one").await;
    let turn = next_turn(&mut world).await;
    assert_eq!(turn.channel_files, None);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn start_task_attaches_channel_files_to_each_session_and_hides_its_own_artifacts() {
    let mut world = World::start().await;
    let (plan, _) = put_file(&mut world, "docs/plan.png", b"png bytes", "image/png").await;
    put_file(&mut world, "notes/other.txt", b"other", "text/plain").await;
    let coordinator = world.start_coordinator("Please do this").await.session_id;

    // An unknown path is an error, and no task starts.
    let result = world
        .worker
        .tool_call(
            coordinator,
            "start_task",
            json!({"title": "No", "brief": "No", "attachments": ["docs/missing.png"]}),
        )
        .await;
    assert!(result.is_error, "{}", error_text(&result));
    assert!(error_text(&result).contains("docs/missing.png"));
    let snapshot = world
        .client
        .snapshot(consort_proto::client_api::SnapshotScope::Channel(
            world.tutti,
        ))
        .await;
    assert!(snapshot.tasks.is_empty());

    let (task_id, start) = start_task_with(&mut world, coordinator, &["docs/plan.png"]).await;
    let first_turn = start.turn.clone().expect("a first turn");
    assert_eq!(
        first_turn.attachments,
        vec![BlobRef {
            sha256: plan.blob_sha256.clone(),
            name: "plan.png".to_owned(),
            mime: "image/png".to_owned(),
            size: 9,
        }]
    );
    assert_eq!(
        paths(&start.channel_files),
        vec!["docs/plan.png", "notes/other.txt"]
    );

    // The task publishes an artifact. A new session of the same task must
    // not get it, but must get the other files and the attachment again.
    let sid = start.session_id;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let blob = worker_upload_blob(&world, b"artifact bytes").await;
    let published = world
        .worker
        .tool_json(
            sid,
            "publish_artifact",
            json!({
                "path": "out/report.txt", "title": "Report",
                "blob_sha256": blob.to_string(), "size": 14, "mime": "text/plain"
            }),
        )
        .await;
    let artifact = published["path"].as_str().unwrap().to_owned();
    assert_eq!(artifact, format!("artifacts/{task_id}/report.txt"));
    // An artifact of another task stays in the list.
    let other_task = TaskId::new();
    let other_artifact = format!("artifacts/{other_task}/theirs.txt");
    put_file(&mut world, &other_artifact, b"theirs", "text/plain").await;

    world
        .worker
        .send_events(
            sid,
            vec![SessionEvent::TurnEnded {
                turn_id: "t1".into(),
                reason: TurnEndReason::Failed,
                error: Some("boom".to_owned()),
            }],
        )
        .await;
    world.wait_task(task_id, TaskStatus::Failed).await;
    let new = world
        .client
        .request(consort_proto::client_api::TaskRetry { task_id })
        .await;
    new.unwrap();
    let mut second_start = None;
    while second_start.is_none() {
        let (request, command) = world.worker.next_command().await;
        match command {
            HubToWorker::SessionStop(_) => {
                world
                    .worker
                    .respond_ok(&request, &serde_json::json!({}))
                    .await;
            }
            HubToWorker::SessionStart(s) => {
                world
                    .worker
                    .respond_ok(
                        &request,
                        &consort_proto::worker_link::SessionStartResult {
                            workspace_path: "/tmp/workspace-2".to_owned(),
                            native_ref: None,
                        },
                    )
                    .await;
                second_start = Some(s);
            }
            other => panic!("unexpected command {}", other.type_name()),
        }
    }
    let second_start = second_start.unwrap();
    assert_ne!(second_start.session_id, sid);
    let listed = paths(&second_start.channel_files);
    assert!(listed.contains(&"docs/plan.png"), "{listed:?}");
    assert!(listed.contains(&"notes/other.txt"), "{listed:?}");
    assert!(listed.contains(&other_artifact.as_str()), "{listed:?}");
    assert!(!listed.contains(&artifact.as_str()), "{listed:?}");
    let attachments = second_start.turn.expect("a first turn").attachments;
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].name, "plan.png");
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn message_task_delivers_its_attachments_by_steer_and_by_turn_and_posts_them_in_the_thread() {
    let mut world = World::start().await;
    put_file(&mut world, "docs/spec.md", b"# Spec", "text/markdown").await;
    let coordinator = world.start_coordinator("Please do this").await.session_id;
    let (task_id, start) = start_task_with(&mut world, coordinator, &[]).await;
    let sid = start.session_id;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;

    // The turn is open and the harness steers.
    let result = world
        .worker
        .tool_json(
            coordinator,
            "message_task",
            json!({"task_id": task_id.to_string(), "body": "Read this", "attachments": ["docs/spec.md"]}),
        )
        .await;
    assert_eq!(result["delivery"], "steer");
    let (request, command) = world.worker.next_command().await;
    let HubToWorker::SessionSteer(steer) = command else {
        panic!("expected session.steer, got {}", command.type_name());
    };
    world
        .worker
        .respond_ok(&request, &consort_proto::frame::Empty {})
        .await;
    assert_eq!(steer.session_id, sid);
    assert_eq!(steer.turn.attachments.len(), 1);
    let blob = &steer.turn.attachments[0];
    assert_eq!(blob.name, "spec.md");
    assert_eq!(blob.mime, "text/markdown");
    assert_eq!(blob.size, 6);
    assert_eq!(blob.sha256, hash_of(b"# Spec"));

    // The thread shows the message with the attachment.
    let card = world.task(task_id).await.card_message_id;
    let thread = messages(&mut world, Some(card)).await;
    let posted = thread
        .iter()
        .find(|m| m.body == "Read this")
        .expect("the message in the thread");
    assert_eq!(posted.author, Author::Assistant);
    assert_eq!(&posted.attachments, &steer.turn.attachments);

    // With no open turn the attachment goes with a new turn.
    end_turn(&mut world, sid).await;
    let result = world
        .worker
        .tool_json(
            coordinator,
            "message_task",
            json!({"task_id": task_id.to_string(), "body": "And again", "attachments": ["docs/spec.md"]}),
        )
        .await;
    assert_eq!(result["delivery"], "turn");
    let turn = next_turn(&mut world).await;
    assert_eq!(turn.turn.text, "And again");
    assert_eq!(turn.turn.attachments, steer.turn.attachments);

    // An unknown path is an error and nothing reaches the task.
    let result = world
        .worker
        .tool_call(
            coordinator,
            "message_task",
            json!({"task_id": task_id.to_string(), "body": "Nope", "attachments": ["docs/none.md"]}),
        )
        .await;
    assert!(result.is_error, "{}", error_text(&result));
    world.hub.handle.shutdown().await.unwrap();
}

/// Sends the inventory of the worker again, with an account that cannot
/// steer, and waits until the hub shows it.
async fn make_worker_unable_to_steer(world: &mut World) {
    let mut inv = inventory(world.worker.account_id);
    inv.accounts[0].capabilities.steer = false;
    let inv = inv.with_hash();
    world
        .worker
        .peer
        .send(&Frame::Event(EventFrame {
            kind: "inventory".to_owned(),
            body: serde_json::to_value(inv).unwrap(),
        }))
        .await;
    let id = world.worker.worker_id;
    loop {
        let list: WorkerListResult = world.client.request(WorkerList {}).await.unwrap();
        let steers = list
            .workers
            .into_iter()
            .find(|w| w.id == id)
            .and_then(|w| w.inventory)
            .and_then(|i| i.accounts.first().map(|a| a.capabilities.steer));
        if steers == Some(false) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_thread_reply_with_an_attachment_waits_in_the_inbox_when_the_account_cannot_steer() {
    let mut world = World::start().await;
    make_worker_unable_to_steer(&mut world).await;
    let (task_id, start) = world.start_task("Do it").await;
    let sid = start.session_id;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let card = world.task(task_id).await.card_message_id;

    let file = attachment(&mut world, "extra.txt", b"extra input").await;
    say_with(&mut world, "Use this too", vec![file.clone()], Some(card)).await;
    // The turn is open and the harness cannot steer, so the hub sends nothing.
    assert!(
        world
            .worker
            .try_next_command(Duration::from_millis(500))
            .await
            .is_none()
    );

    end_turn(&mut world, sid).await;
    let turn = next_turn(&mut world).await;
    assert_eq!(turn.session_id, sid);
    assert_eq!(turn.turn.text, "Use this too");
    assert_eq!(turn.turn.attachments, vec![file]);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn publish_artifact_stores_a_channel_file_with_the_true_size_and_sends_it_in_the_feed() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Make a report").await;
    let sid = start.session_id;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let bytes = b"the report text";
    let blob = worker_upload_blob(&world, bytes).await;
    world.client.forget_feed();

    // The size in the call is wrong on purpose.
    let result = world
        .worker
        .tool_json(
            sid,
            "publish_artifact",
            json!({
                "path": "out/report.txt", "title": "The report",
                "blob_sha256": blob.to_string(), "size": 9999, "mime": "text/plain"
            }),
        )
        .await;
    let path = format!("artifacts/{task_id}/report.txt");
    assert_eq!(result["path"], path.as_str());

    let entry = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(consort_proto::client_api::EntityPayload::ChannelFile(f)) if f.path == path))
        .await;
    let Some(consort_proto::client_api::EntityPayload::ChannelFile(file)) = upserted(&entry) else {
        panic!("not a channel file");
    };
    assert_eq!(file.task_id, Some(task_id));
    assert_eq!(file.title.as_deref(), Some("The report"));
    assert_eq!(file.size, size_of(bytes));
    assert_eq!(file.mime, "text/plain");
    assert_eq!(file.blob_sha256, blob);

    let listed = world
        .client
        .request(FilesList {
            channel_id: world.tutti,
            prefix: Some("artifacts/".to_owned()),
        })
        .await
        .unwrap();
    assert_eq!(listed.files.len(), 1);
    assert_eq!(listed.files[0].size, size_of(bytes));

    // A blob that the hub does not have is refused.
    let result = world
        .worker
        .tool_call(
            sid,
            "publish_artifact",
            json!({
                "path": "out/ghost.txt", "title": "Ghost",
                "blob_sha256": hash_of(b"ghost").to_string(), "size": 5, "mime": "text/plain"
            }),
        )
        .await;
    assert!(result.is_error, "{}", error_text(&result));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn post_message_and_reply_attach_channel_files() {
    let mut world = World::start().await;
    let (file, _) = put_file(&mut world, "docs/chart.png", b"chart", "image/png").await;
    let start = world.start_coordinator("Make a chart").await;
    let sid = start.session_id;
    let user = world.say(world.tutti, "Hello").await;

    world
        .worker
        .tool_json(
            sid,
            "post_message",
            json!({"body": "Here is the chart", "attachments": ["docs/chart.png"]}),
        )
        .await;
    world
        .worker
        .tool_json(
            sid,
            "reply",
            json!({"message_id": user.id.to_string(), "body": "And in a thread", "attachments": ["docs/chart.png"]}),
        )
        .await;
    let expected = vec![BlobRef {
        sha256: file.blob_sha256.clone(),
        name: "chart.png".to_owned(),
        mime: "image/png".to_owned(),
        size: 5,
    }];
    let timeline = messages(&mut world, None).await;
    let posted = timeline
        .iter()
        .find(|m| m.body == "Here is the chart")
        .expect("the posted message");
    assert_eq!(posted.attachments, expected);
    let thread = messages(&mut world, Some(user.id)).await;
    let reply = thread
        .iter()
        .find(|m| m.body == "And in a thread")
        .expect("the reply");
    assert_eq!(reply.attachments, expected);

    // An unknown path is an error for both tools.
    let result = world
        .worker
        .tool_call(
            sid,
            "post_message",
            json!({"body": "Bad", "attachments": ["docs/none.png"]}),
        )
        .await;
    assert!(result.is_error, "{}", error_text(&result));
    let result = world
        .worker
        .tool_call(
            sid,
            "reply",
            json!({"message_id": user.id.to_string(), "body": "Bad", "attachments": ["docs/none.png"]}),
        )
        .await;
    assert!(result.is_error, "{}", error_text(&result));
    assert!(
        messages(&mut world, None)
            .await
            .iter()
            .all(|m| m.body != "Bad")
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_attachment_failure_in_a_task_session_posts_a_notice_in_the_task_thread() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    let sid = start.session_id;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    world
        .worker
        .send_events(
            sid,
            vec![SessionEvent::Error {
                code: "attachment_failed".to_owned(),
                message: "Could not download plan.png.".to_owned(),
            }],
        )
        .await;
    let card = world.task(task_id).await.card_message_id;
    world
        .wait_notice(
            Some(card),
            &["attachment_failed", "Could not download plan.png."],
        )
        .await;
    // The timeline has no such notice.
    assert!(
        world
            .notices(None)
            .await
            .iter()
            .all(|n| !n.contains("attachment_failed"))
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_attachment_failure_in_a_coordinator_session_posts_a_notice_in_the_timeline() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Hello").await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![
                SessionEvent::Error {
                    code: "attachment_failed".to_owned(),
                    message: "Could not download notes.txt.".to_owned(),
                },
                assistant_text(start.session_id, "Done.", ItemStatus::Completed),
            ],
        )
        .await;
    world
        .wait_notice(
            None,
            &["attachment_failed", "Could not download notes.txt."],
        )
        .await;
    world.hub.handle.shutdown().await.unwrap();
}
