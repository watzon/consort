//! A user message while an outgoing draft card waits: the card closes as
//! replaced, and the text is the result of the tool call that waited.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use consort_proto::client_api::MessageSend;
use consort_proto::ids::SessionId;
use consort_proto::model::{ApprovalDetails, ApprovalStatus, ToolApprovals};
use serde_json::{Value, json};

use super::flow_tests::{
    active_gmail, answer, coordinator_session, fast, hub_with_fake, pending_card, tutti, wait_for,
};
use super::{Timings, fake::FakeComposio};
use crate::hub::{Caller, Hub, test_task_session};
use crate::store::{coordinator as coord_store, work};

const SEND: &str = "GMAIL_SEND_EMAIL";

fn run(hub: &Hub, session: SessionId, tool: &str, body: &str) -> tokio::task::JoinHandle<Value> {
    let hub = hub.clone();
    let tool = tool.to_owned();
    let body = body.to_owned();
    tokio::spawn(async move {
        let result = hub
            .call_tool(
                session,
                "run_connector_tool",
                json!({"connector": "gmail_work", "tool": tool,
                       "arguments": {"recipient_email": "a@example.com", "subject": "Hi", "body": body}}),
            )
            .await;
        result.structured_content.unwrap_or(Value::Null)
    })
}

async fn say(
    hub: &Hub,
    channel: consort_proto::ids::ChannelId,
    thread: Option<consort_proto::ids::MessageId>,
    text: &str,
) {
    hub.message_send(
        &Caller::Hub,
        MessageSend {
            channel_id: channel,
            client_message_id: String::new(),
            body: text.to_owned(),
            attachments: Vec::new(),
            thread_root_id: thread,
            in_timeline: false,
        },
    )
    .await
    .unwrap();
}

async fn approval(hub: &Hub, id: consort_proto::ids::ApprovalId) -> consort_proto::model::Approval {
    hub.store
        .read(move |conn| work::approval(conn, id))
        .await
        .unwrap()
        .unwrap()
}

async fn setup() -> (Hub, tempfile::TempDir, FakeComposio) {
    let (hub, dir, fake) = hub_with_fake().await;
    active_gmail(&hub, &fake, ToolApprovals::default()).await;
    (hub, dir, fake)
}

/// The coordinator of the channel runs a turn in `session`.
async fn open_coordinator_turn(
    hub: &Hub,
    channel: consort_proto::ids::ChannelId,
    session: SessionId,
) {
    hub.store
        .write(move |tx| {
            tx.save_coordinator(&coord_store::CoordinatorRow {
                channel_id: channel,
                session_id: Some(session),
                turn_open: true,
                turn_started_at: Some(tx.now()),
                turn_events: Vec::new(),
            })
        })
        .await
        .unwrap();
}

/// A file that the hub holds, as an attachment of a message.
async fn stored_file(hub: &Hub) -> consort_proto::model::BlobRef {
    use sha2::{Digest, Sha256};
    let bytes = b"a file of the user";
    let hash = consort_proto::model::Sha256Hash::from_bytes(Sha256::digest(bytes).into());
    tokio::fs::write(hub.store.blobs_dir().join(hash.as_str()), bytes)
        .await
        .unwrap();
    consort_proto::model::BlobRef {
        sha256: hash,
        name: "note.txt".to_owned(),
        mime: "text/plain".to_owned(),
        size: 0,
    }
}

async fn say_with_file(
    hub: &Hub,
    channel: consort_proto::ids::ChannelId,
    thread: Option<consort_proto::ids::MessageId>,
    text: &str,
) {
    let file = stored_file(hub).await;
    hub.message_send(
        &Caller::Hub,
        MessageSend {
            channel_id: channel,
            client_message_id: String::new(),
            body: text.to_owned(),
            attachments: vec![file],
            thread_root_id: thread,
            in_timeline: false,
        },
    )
    .await
    .unwrap();
}

/// The session of a task has its turn closed, as after an interrupt.
async fn close_turn(hub: &Hub, session: SessionId) {
    hub.store
        .write(move |tx| tx.save_session_runtime(session, &work::SessionRuntime::default()))
        .await
        .unwrap();
}

async fn wakes_of(hub: &Hub, channel: consort_proto::ids::ChannelId) -> usize {
    hub.store
        .read(move |conn| coord_store::pending_wakes(conn, channel))
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn a_task_reply_closes_the_waiting_draft_and_is_the_result_of_the_call() {
    let (hub, _dir, fake) = setup().await;
    let (task, session) = test_task_session(&hub).await;
    let running = run(&hub, session.id, SEND, "Hello");
    let card = pending_card(&hub).await;
    say(
        &hub,
        task.channel_id,
        Some(task.card_message_id),
        "Make it shorter",
    )
    .await;

    let body = running.await.unwrap();
    assert_eq!(body["status"], "replaced", "{body}");
    assert_eq!(
        body["message"],
        "The user sent this while your draft waited: Make it shorter. If it asks for a change, write the draft again and call the tool. If not, answer it and call the tool again with the same text."
    );
    let closed = approval(&hub, card.id).await;
    assert_eq!(closed.status, ApprovalStatus::Replaced);
    let ApprovalDetails::Connector(details) = closed.details else {
        panic!("not a connector card");
    };
    assert_eq!(details.replaced_by.as_deref(), Some("Make it shorter"));
    assert!(fake.executions().is_empty(), "nothing is sent");

    // The text went to the tool call only: the session got no turn or steer
    // and its inbox is empty.
    let task_id = task.id;
    let (inbox, commands) = hub
        .store
        .read(move |conn| {
            Ok((
                work::task_inbox(conn, task_id)?,
                crate::store::machine::commands_for_session(conn, session.id)?,
            ))
        })
        .await
        .unwrap();
    assert!(inbox.is_empty());
    assert!(commands.is_empty());

    // The new call asks again.
    let again = run(&hub, session.id, SEND, "Hello, shorter");
    let second = pending_card(&hub).await;
    assert_ne!(second.id, card.id);
    answer(&hub, second.id, "deny").await;
    assert_eq!(again.await.unwrap()["status"], "denied");
}

#[tokio::test]
async fn the_result_holds_the_text_that_the_user_saved_on_the_card() {
    let (hub, _dir, _fake) = setup().await;
    let (task, session) = test_task_session(&hub).await;
    let running = run(&hub, session.id, SEND, "Hello there");
    let card = pending_card(&hub).await;
    // The user edited the body and saved it.
    let edited = {
        let mut approval = card.clone();
        if let ApprovalDetails::Connector(details) = &mut approval.details {
            details.draft_arguments = Some(details.arguments.clone());
            details.arguments["body"] = json!("Hello, my own words");
            details.editable = vec!["subject".to_owned(), "body".to_owned()];
        }
        approval
    };
    hub.store
        .write(move |tx| tx.save_approval(&edited))
        .await
        .unwrap();
    say(
        &hub,
        task.channel_id,
        Some(task.card_message_id),
        "And add my number",
    )
    .await;
    let body = running.await.unwrap();
    let message = body["message"].as_str().unwrap();
    assert!(message.contains("And add my number"), "{message}");
    assert!(message.contains("body: Hello, my own words"), "{message}");
}

#[tokio::test]
async fn two_waiting_drafts_of_a_session_both_close() {
    let (hub, _dir, _fake) = setup().await;
    let (task, session) = test_task_session(&hub).await;
    let first = run(&hub, session.id, SEND, "One");
    wait_for(async || {
        hub.store
            .read(|conn| work::pending_approvals(conn, None))
            .await
            .unwrap()
            .into_iter()
            .next()
    })
    .await;
    let second = run(&hub, session.id, SEND, "Two");
    wait_for(async || {
        let pending = hub
            .store
            .read(|conn| work::pending_approvals(conn, None))
            .await
            .unwrap();
        (pending.len() == 2).then_some(())
    })
    .await;
    say(
        &hub,
        task.channel_id,
        Some(task.card_message_id),
        "Wait, change both",
    )
    .await;
    assert_eq!(first.await.unwrap()["status"], "replaced");
    assert_eq!(second.await.unwrap()["status"], "replaced");
    let pending = hub
        .store
        .read(|conn| work::pending_approvals(conn, None))
        .await
        .unwrap();
    assert!(pending.is_empty());
}

#[tokio::test]
async fn a_message_in_the_channel_replaces_the_draft_of_the_coordinator_and_queues_no_wake() {
    let (hub, _dir, fake) = setup().await;
    let channel = tutti(&hub).await;
    let session = coordinator_session(&hub, channel, "acp").await;
    open_coordinator_turn(&hub, channel, session).await;
    let running = run(&hub, session, SEND, "Hello");
    let card = pending_card(&hub).await;
    say(&hub, channel, None, "Say it more kindly").await;
    let body = running.await.unwrap();
    assert_eq!(body["status"], "replaced", "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Say it more kindly")
    );
    assert_eq!(
        approval(&hub, card.id).await.status,
        ApprovalStatus::Replaced
    );
    let wakes = hub
        .store
        .read(move |conn| coord_store::pending_wakes(conn, channel))
        .await
        .unwrap();
    assert!(wakes.is_empty(), "{wakes:?}");
    assert!(fake.executions().is_empty());
}

#[tokio::test]
async fn a_card_that_is_not_an_outgoing_draft_stays_open() {
    let (hub, _dir, _fake) = setup().await;
    let (task, session) = test_task_session(&hub).await;
    let running = run(&hub, session.id, "GMAIL_DELETE_MESSAGE", "x");
    let card = pending_card(&hub).await;
    say(
        &hub,
        task.channel_id,
        Some(task.card_message_id),
        "Also check the docs",
    )
    .await;
    assert_eq!(
        approval(&hub, card.id).await.status,
        ApprovalStatus::Pending
    );
    // The text went to the session as a message.
    let task_id = task.id;
    let queued = hub
        .store
        .read(move |conn| {
            Ok((
                work::task_inbox(conn, task_id)?.len(),
                crate::store::machine::commands_for_session(conn, session.id)?.len(),
            ))
        })
        .await
        .unwrap();
    assert!(queued.0 + queued.1 > 0, "the message reached the session");
    answer(&hub, card.id, "deny").await;
    assert_eq!(running.await.unwrap()["status"], "denied");
}

#[tokio::test]
async fn a_draft_card_that_no_call_waits_for_stays_open() {
    let (hub, _dir, _fake) = setup().await;
    hub.connectors.set_timings(Timings {
        approval_wait: Duration::from_millis(50),
        ..fast()
    });
    let (task, session) = test_task_session(&hub).await;
    let waiting = run(&hub, session.id, SEND, "Hello").await.unwrap();
    assert_eq!(waiting["status"], "waiting_for_user");
    let card = pending_card(&hub).await;
    say(&hub, task.channel_id, Some(task.card_message_id), "Hello?").await;
    assert_eq!(
        approval(&hub, card.id).await.status,
        ApprovalStatus::Pending
    );
}

#[tokio::test]
async fn a_task_reply_after_an_interrupt_reaches_the_session_and_leaves_the_card_open() {
    let (hub, _dir, fake) = setup().await;
    let (task, session) = test_task_session(&hub).await;
    let running = run(&hub, session.id, SEND, "Hello");
    let card = pending_card(&hub).await;
    // The interrupt closed the turn. The waiter of the dead call is still
    // there, for up to the approval wait.
    close_turn(&hub, session.id).await;
    say(
        &hub,
        task.channel_id,
        Some(task.card_message_id),
        "Make it shorter",
    )
    .await;
    assert_eq!(
        approval(&hub, card.id).await.status,
        ApprovalStatus::Pending
    );
    let task_id = task.id;
    let queued = hub
        .store
        .read(move |conn| {
            Ok(work::task_inbox(conn, task_id)?.len()
                + crate::store::machine::commands_for_session(conn, session.id)?.len())
        })
        .await
        .unwrap();
    assert!(queued > 0, "the text reached the session");
    assert!(fake.executions().is_empty());
    answer(&hub, card.id, "deny").await;
    assert_eq!(running.await.unwrap()["status"], "denied");
}

#[tokio::test]
async fn a_task_reply_for_an_ended_session_does_not_replace_the_card() {
    let (hub, _dir, _fake) = setup().await;
    let (task, session) = test_task_session(&hub).await;
    let running = run(&hub, session.id, SEND, "Hello");
    let card = pending_card(&hub).await;
    let mut ended = session.clone();
    ended.state = consort_proto::model::SessionState::Ended;
    hub.store
        .write(move |tx| tx.save_session(&ended))
        .await
        .unwrap();
    say(
        &hub,
        task.channel_id,
        Some(task.card_message_id),
        "Make it shorter",
    )
    .await;
    assert_eq!(
        approval(&hub, card.id).await.status,
        ApprovalStatus::Pending
    );
    answer(&hub, card.id, "deny").await;
    assert_eq!(running.await.unwrap()["status"], "denied");
}

#[tokio::test]
async fn a_channel_message_after_the_coordinator_turn_closed_wakes_the_coordinator() {
    let (hub, _dir, fake) = setup().await;
    let channel = tutti(&hub).await;
    let session = coordinator_session(&hub, channel, "acp").await;
    open_coordinator_turn(&hub, channel, session).await;
    let running = run(&hub, session, SEND, "Hello");
    let card = pending_card(&hub).await;
    // The turn closed (an interrupt): the waiter outlives the dead call.
    hub.store
        .write(move |tx| {
            tx.save_coordinator(&coord_store::CoordinatorRow {
                channel_id: channel,
                session_id: Some(session),
                turn_open: false,
                turn_started_at: None,
                turn_events: Vec::new(),
            })
        })
        .await
        .unwrap();
    say(&hub, channel, None, "Say it more kindly").await;
    assert_eq!(
        approval(&hub, card.id).await.status,
        ApprovalStatus::Pending
    );
    assert_eq!(
        wakes_of(&hub, channel).await,
        1,
        "the message woke the coordinator"
    );
    assert!(fake.executions().is_empty());
    answer(&hub, card.id, "deny").await;
    assert_eq!(running.await.unwrap()["status"], "denied");
}

#[tokio::test]
async fn a_channel_message_does_not_replace_the_card_of_an_older_coordinator_session() {
    let (hub, _dir, _fake) = setup().await;
    let channel = tutti(&hub).await;
    let old = coordinator_session(&hub, channel, "acp").await;
    let current = coordinator_session(&hub, channel, "acp").await;
    open_coordinator_turn(&hub, channel, old).await;
    let running = run(&hub, old, SEND, "Hello");
    let card = pending_card(&hub).await;
    // The channel moved to a new session, which runs a turn.
    open_coordinator_turn(&hub, channel, current).await;
    say(&hub, channel, None, "Say it more kindly").await;
    assert_eq!(
        approval(&hub, card.id).await.status,
        ApprovalStatus::Pending
    );
    assert_eq!(wakes_of(&hub, channel).await, 1);
    answer(&hub, card.id, "deny").await;
    assert_eq!(running.await.unwrap()["status"], "denied");
}

#[tokio::test]
async fn a_message_with_a_file_does_not_replace_a_draft() {
    let (hub, _dir, fake) = setup().await;
    // In the task thread.
    let (task, session) = test_task_session(&hub).await;
    let running = run(&hub, session.id, SEND, "Hello");
    let card = pending_card(&hub).await;
    say_with_file(
        &hub,
        task.channel_id,
        Some(task.card_message_id),
        "Use this file",
    )
    .await;
    assert_eq!(
        approval(&hub, card.id).await.status,
        ApprovalStatus::Pending
    );
    let task_id = task.id;
    let queued = hub
        .store
        .read(move |conn| {
            Ok(work::task_inbox(conn, task_id)?.len()
                + crate::store::machine::commands_for_session(conn, session.id)?.len())
        })
        .await
        .unwrap();
    assert!(queued > 0, "the message and its file reached the session");
    answer(&hub, card.id, "deny").await;
    assert_eq!(running.await.unwrap()["status"], "denied");

    // In the channel of a coordinator.
    let channel = tutti(&hub).await;
    let coordinator = coordinator_session(&hub, channel, "acp").await;
    open_coordinator_turn(&hub, channel, coordinator).await;
    let running = run(&hub, coordinator, SEND, "Hello");
    let card = pending_card(&hub).await;
    let before = wakes_of(&hub, channel).await;
    say_with_file(&hub, channel, None, "Use this file").await;
    assert_eq!(
        approval(&hub, card.id).await.status,
        ApprovalStatus::Pending
    );
    assert_eq!(wakes_of(&hub, channel).await, before + 1);
    answer(&hub, card.id, "deny").await;
    assert_eq!(running.await.unwrap()["status"], "denied");
    assert!(fake.executions().is_empty());
}

#[tokio::test]
async fn a_card_replaced_as_the_wait_ends_returns_replaced_not_waiting() {
    let (hub, _dir, _fake) = setup().await;
    hub.connectors.set_timings(Timings {
        approval_wait: Duration::from_millis(300),
        ..fast()
    });
    let (_task, session) = test_task_session(&hub).await;
    let running = run(&hub, session.id, SEND, "Hello");
    let card = pending_card(&hub).await;
    // A message replaced the card, and the wait ended before the hub woke
    // the waiter: the card is Replaced and the waiter is still there.
    let mut closed = card.clone();
    if let ApprovalDetails::Connector(details) = &mut closed.details {
        details.replaced_by = Some("Make it shorter".to_owned());
    }
    closed.status = ApprovalStatus::Replaced;
    hub.store
        .write(move |tx| tx.save_approval(&closed))
        .await
        .unwrap();
    let body = running.await.unwrap();
    assert_eq!(body["status"], "replaced", "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Make it shorter")
    );
}
