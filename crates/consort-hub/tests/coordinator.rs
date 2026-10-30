//! A user message starts a coordinator session on the worker. The worker
//! calls hub tools, and the client sees the result in the feed.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::common::*;
use consort_proto::client_api::{EntityPayload, MessageList, SnapshotScope, WorkerSetPaused};
use consort_proto::model::{Author, ItemStatus, MessageKind, SessionRole, SessionState};
use consort_proto::worker_link::{HubToWorker, SessionEvent, TurnEndReason};
use serde_json::json;

#[tokio::test]
async fn a_user_message_starts_a_coordinator_and_its_post_reaches_the_client() {
    let mut world = World::start().await;
    let message = world
        .say(world.tutti, "What makes a channel special?")
        .await;

    // The hub starts a coordinator session on the worker.
    let (request, command) = world.worker.next_command().await;
    let HubToWorker::SessionStart(start) = command else {
        panic!("expected session.start");
    };
    assert_eq!(start.role, SessionRole::Coordinator);
    assert_eq!(start.channel_id, world.tutti);
    assert_eq!(start.model, "opus");
    assert_eq!(start.account_id, world.worker.account_id);
    assert!(start.task_id.is_none());
    assert!(
        start.system_prompt_append.contains("post_message"),
        "the coordinator rules"
    );
    assert!(
        start.system_prompt_append.contains("Tutti"),
        "the channel name"
    );
    let turn = start.turn.as_ref().expect("a first turn").text.clone();
    assert!(turn.contains("<events>"), "{turn}");
    assert!(
        turn.contains(&format!("<user_message id=\"{}\"", message.id)),
        "{turn}"
    );
    assert!(turn.contains("What makes a channel special?"));
    assert!(
        turn.contains("<context>"),
        "a new session gets the context pack"
    );

    world
        .worker
        .respond_ok(
            &request,
            &consort_proto::worker_link::SessionStartResult {
                workspace_path: "/tmp/ws".to_owned(),
                native_ref: None,
            },
        )
        .await;
    let session_id = start.session_id;
    let ack = world
        .worker
        .send_events(
            session_id,
            vec![state(SessionState::Active), turn_started("t1")],
        )
        .await;
    assert_eq!(ack.last_seq, 2);

    // The coordinator speaks through post_message.
    let result = world
        .worker
        .tool_json(
            session_id,
            "post_message",
            json!({"body": "A channel has its own purpose, memory and tasks."}),
        )
        .await;
    let posted_id = result["message_id"].as_str().unwrap().to_owned();
    let entry = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.id.to_string() == posted_id))
        .await;
    let Some(EntityPayload::Message(posted)) = upserted(&entry) else {
        unreachable!()
    };
    assert_eq!(posted.author, Author::Assistant);
    assert_eq!(posted.author_session_id, Some(session_id));
    assert_eq!(posted.channel_id, world.tutti);

    // The turn ends. The coordinator used a posting tool, so the hub adds nothing.
    world
        .worker
        .send_events(session_id, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    let page = world
        .client
        .request(MessageList {
            channel_id: world.tutti,
            thread_root_id: None,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    let assistant: Vec<_> = page
        .messages
        .iter()
        .filter(|m| m.author == Author::Assistant)
        .collect();
    assert_eq!(assistant.len(), 1);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_second_message_goes_to_the_same_session_as_a_turn() {
    let mut world = World::start().await;
    let start = world.start_coordinator("First").await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![turn_ended("t1", TurnEndReason::Completed)],
        )
        .await;
    // The hub removes the sent turn from the queue only when the worker answers.
    // The first turn was part of session.start, so the next message is a turn.
    world.say(world.tutti, "Second").await;
    let (request, command) = world.worker.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!(
            "expected session.turn, got {}",
            consort_proto::frame::MessageFamily::type_name(&command)
        );
    };
    assert_eq!(turn.session_id, start.session_id);
    assert!(turn.turn.text.contains("Second"));
    assert!(
        !turn.turn.text.contains("<context>"),
        "only a new session gets the context pack"
    );
    world
        .worker
        .respond_ok(
            &request,
            &consort_proto::worker_link::SessionTurnResult {
                turn_id: "t2".into(),
            },
        )
        .await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn messages_during_a_turn_wait_and_go_out_together_after_it() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Start").await;

    // The turn is open, so these two messages must wait.
    world.say(world.tutti, "Wait one").await;
    world.say(world.tutti, "Wait two").await;
    assert!(
        world
            .worker
            .try_next_command(std::time::Duration::from_millis(300))
            .await
            .is_none(),
        "no second turn while one runs"
    );

    world
        .worker
        .send_events(
            start.session_id,
            vec![turn_ended("t1", TurnEndReason::Completed)],
        )
        .await;
    let (_, command) = world.worker.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!("expected session.turn");
    };
    assert!(turn.turn.text.contains("Wait one"));
    assert!(turn.turn.text.contains("Wait two"));
    assert_eq!(turn.turn.text.matches("<user_message").count(), 2);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_turn_without_a_posting_call_gets_a_fallback_post() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Say something").await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![
                assistant_text(
                    start.session_id,
                    "I forgot to call a tool.",
                    ItemStatus::Completed,
                ),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    let entry = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.author == Author::Assistant))
        .await;
    let Some(EntityPayload::Message(posted)) = upserted(&entry) else {
        unreachable!()
    };
    assert_eq!(posted.body, "I forgot to call a tool.");
    assert_eq!(posted.kind, MessageKind::Text);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn no_reply_needed_ends_a_turn_without_a_post() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Nothing to say").await;
    world
        .worker
        .tool_json(start.session_id, "no_reply_needed", json!({}))
        .await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![
                assistant_text(start.session_id, "Staying quiet.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    let page = world
        .client
        .request(MessageList {
            channel_id: world.tutti,
            thread_root_id: None,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(page.messages.iter().all(|m| m.author != Author::Assistant));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_lost_coordinator_session_is_replaced_with_a_context_pack() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Hello").await;
    world
        .worker
        .tool_json(start.session_id, "post_message", json!({"body": "Hi."}))
        .await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![
                turn_ended("t1", TurnEndReason::Completed),
                state(SessionState::Lost),
            ],
        )
        .await;

    world.say(world.tutti, "Are you there?").await;
    let new_start = world.worker.accept_session_start().await;
    assert_ne!(
        new_start.session_id, start.session_id,
        "a new session replaces the lost one"
    );
    let text = new_start.turn.unwrap().text;
    assert!(text.contains("<context>"));
    assert!(
        text.contains("Hello"),
        "the pack has earlier messages: {text}"
    );
    assert!(text.contains("Are you there?"));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_lost_session_in_the_middle_of_a_turn_puts_the_events_back() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Important question").await;
    world
        .worker
        .send_events(start.session_id, vec![state(SessionState::Lost)])
        .await;
    // The hub starts a new session at once, because events wait.
    let new_start = world.worker.accept_session_start().await;
    assert_ne!(new_start.session_id, start.session_id);
    assert!(new_start.turn.unwrap().text.contains("Important question"));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn without_a_worker_the_hub_says_why_and_starts_when_one_connects() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let tutti = client.tutti().await;
    let snapshot = client.snapshot(SnapshotScope::All).await;
    client.subscribe(snapshot.seq).await.unwrap();
    client
        .request(consort_proto::client_api::MessageSend {
            channel_id: tutti,
            client_message_id: "a".to_owned(),
            body: "Anyone there?".to_owned(),
            attachments: Vec::new(),
            thread_root_id: None,
            in_timeline: false,
        })
        .await
        .unwrap();
    let notice = client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.kind == MessageKind::Notice))
        .await;
    let Some(EntityPayload::Message(notice)) = upserted(&notice) else {
        unreachable!()
    };
    assert!(
        notice.body.contains("assistant_unavailable"),
        "{}",
        notice.body
    );
    assert!(notice.body.contains("worker"), "{}", notice.body);

    // A worker connects. The queued message starts a session.
    let mut worker = FakeWorker::pair_and_connect(&hub).await;
    let start = worker.accept_session_start().await;
    assert!(start.turn.unwrap().text.contains("Anyone there?"));
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_paused_worker_does_not_get_a_coordinator() {
    let mut world = World::start().await;
    world
        .client
        .request(WorkerSetPaused {
            worker_id: world.worker.worker_id,
            paused: true,
        })
        .await
        .unwrap();
    world.say(world.tutti, "Hello?").await;
    assert!(
        world
            .worker
            .try_next_command(std::time::Duration::from_millis(400))
            .await
            .is_none(),
        "a paused worker gets no new session"
    );
    world
        .client
        .request(WorkerSetPaused {
            worker_id: world.worker.worker_id,
            paused: false,
        })
        .await
        .unwrap();
    let start = world.worker.accept_session_start().await;
    assert!(start.turn.unwrap().text.contains("Hello?"));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn session_items_reach_the_client_and_fast_updates_merge() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Work").await;
    let item_id = consort_proto::ids::ItemId::new();
    let make = |text: &str, status: ItemStatus| {
        consort_proto::worker_link::SessionEvent::Item(consort_proto::model::SessionItem {
            id: item_id,
            session_id: start.session_id,
            turn_id: "t1".into(),
            payload: consort_proto::model::ItemPayload::AssistantMessage(
                consort_proto::model::AssistantMessagePayload {
                    text: text.to_owned(),
                },
            ),
            status,
            created_at: consort_proto::time::Timestamp::now(),
            updated_at: consort_proto::time::Timestamp::now(),
        })
    };
    // Five updates of one item in separate batches, well inside 250 ms.
    for i in 0..5 {
        world
            .worker
            .send_events(
                start.session_id,
                vec![make(&format!("part {i}"), ItemStatus::InProgress)],
            )
            .await;
    }
    world
        .worker
        .send_events(
            start.session_id,
            vec![make("part 5", ItemStatus::Completed)],
        )
        .await;
    world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::SessionItem(i)) if i.id == item_id && i.status == ItemStatus::Completed))
        .await;
    let updates = world
        .client
        .feed
        .iter()
        .filter(|e| matches!(upserted(e), Some(EntityPayload::SessionItem(i)) if i.id == item_id))
        .count();
    assert!(
        updates <= 3,
        "the hub merged the fast updates, but sent {updates} entries"
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_reopened_session_drops_its_old_turn_and_requests() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Start").await;
    world
        .worker
        .send_events(start.session_id, vec![approval_request("r1", "ls")])
        .await;
    world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Approval(a)) if a.request_id.as_str() == "r1"))
        .await;

    // A message waits behind the open turn.
    world.say(world.tutti, "Still there?").await;
    assert!(
        world
            .worker
            .try_next_command(std::time::Duration::from_millis(300))
            .await
            .is_none(),
        "no second turn while one runs"
    );

    // The worker restarted and opened the session again. The old turn and its
    // approval are gone, so the waiting message goes out.
    world
        .worker
        .send_events(start.session_id, vec![SessionEvent::Reopened])
        .await;
    let canceled = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Approval(a)) if a.status == consort_proto::model::ApprovalStatus::Canceled))
        .await;
    let Some(EntityPayload::Approval(canceled)) = upserted(&canceled) else {
        unreachable!()
    };
    assert_eq!(canceled.request_id.as_str(), "r1");
    let notice = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.kind == MessageKind::Notice))
        .await;
    let Some(EntityPayload::Message(notice)) = upserted(&notice) else {
        unreachable!()
    };
    assert!(notice.body.contains("worker restarted"), "{}", notice.body);
    let (_, command) = world.worker.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!("expected session.turn");
    };
    assert_eq!(turn.session_id, start.session_id);
    assert!(turn.turn.text.contains("Still there?"));
    world.hub.handle.shutdown().await.unwrap();
}
