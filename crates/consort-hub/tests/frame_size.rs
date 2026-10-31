//! Nothing that the hub sends may be larger than one frame (1 MiB). Large
//! messages split over coordinator turns, pages stop early, and a message
//! that is too large is refused.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::client_api::{
    ClientPairingCode, ClientPairingCodeResult, EntityPayload, HubToClient, MessageList,
    MessageListResult, MessageSend, SnapshotScope,
};
use consort_proto::error::ErrorCode;
use consort_proto::frame::{Frame, MAX_FRAME_BYTES};
use consort_proto::ids::{ChannelId, MessageId};
use consort_proto::session_host_link::{HubToSessionHost, SessionTurnResult, TurnEndReason};

/// The most a message body can have in this build (256 KiB as JSON). These
/// tests use bodies just under it.
const LARGE: usize = 250_000;

fn send(channel_id: ChannelId, key: &str, body: String) -> MessageSend {
    MessageSend {
        channel_id,
        client_message_id: key.to_owned(),
        body,
        attachments: Vec::new(),
        thread_root_id: None,
        in_timeline: false,
    }
}

#[tokio::test]
async fn large_messages_that_arrive_during_a_turn_go_out_over_several_turns() {
    let mut world = World::start().await;
    let start = world.start_coordinator("hello").await;
    // Five large pastes arrive while the turn runs: 1.25 MB together, which
    // is more than one frame.
    let mut ids: Vec<MessageId> = Vec::new();
    for i in 0..5 {
        let message = world
            .say(world.tutti, &format!("{i}{}", "a".repeat(LARGE)))
            .await;
        ids.push(message.id);
    }
    world
        .worker
        .send_events(
            start.session_id,
            vec![turn_ended("t1", TurnEndReason::Completed)],
        )
        .await;

    let mut seen: Vec<MessageId> = Vec::new();
    let mut turns = 0;
    while seen.len() < ids.len() {
        let (request, command) = world
            .worker
            .try_next_command(Duration::from_secs(5))
            .await
            .expect("the hub sent no turn with the queued messages");
        let HubToSessionHost::SessionTurn(turn) = command else {
            panic!("expected session.turn");
        };
        let size = Frame::Request(request.clone()).to_text().unwrap().len();
        assert!(size <= MAX_FRAME_BYTES);
        for id in &ids {
            if turn.turn.text.contains(&id.to_string()) {
                seen.push(*id);
            }
        }
        turns += 1;
        let turn_id = format!("t{}", turns + 1);
        world
            .worker
            .respond_ok(
                &request,
                &SessionTurnResult {
                    turn_id: turn_id.as_str().into(),
                },
            )
            .await;
        world
            .worker
            .send_events(
                start.session_id,
                vec![
                    turn_started(&turn_id),
                    turn_ended(&turn_id, TurnEndReason::Completed),
                ],
            )
            .await;
    }
    // Every message went out once, in the order it came.
    assert_eq!(seen, ids);
    assert!(turns > 1, "the messages did not split");
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_message_list_of_large_messages_answers_with_a_shorter_page() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let tutti = client.tutti().await;
    let mut ids = Vec::new();
    for i in 0..4 {
        let sent = client
            .request(send(tutti, &format!("m{i}"), "x".repeat(LARGE)))
            .await
            .unwrap();
        ids.push(sent.message.id);
    }
    // Four messages of 250 KB do not fit in one frame. The page stops early
    // and says that more exist.
    let page: MessageListResult = tokio::time::timeout(
        Duration::from_secs(5),
        client.request(MessageList {
            channel_id: tutti,
            thread_root_id: None,
            before: None,
            limit: Some(5),
        }),
    )
    .await
    .expect("message.list got no answer")
    .unwrap();
    let large_on_page = page.messages.iter().filter(|m| ids.contains(&m.id)).count();
    assert!(large_on_page > 0 && large_on_page < 4, "{large_on_page}");
    assert!(page.has_more);
    // The next page has the rest.
    let oldest = page.messages.last().unwrap().id;
    let rest: MessageListResult = client
        .request(MessageList {
            channel_id: tutti,
            thread_root_id: None,
            before: Some(oldest),
            limit: Some(5),
        })
        .await
        .unwrap();
    let mut listed: Vec<MessageId> = page
        .messages
        .iter()
        .chain(rest.messages.iter())
        .map(|m| m.id)
        .filter(|id| ids.contains(id))
        .collect();
    listed.reverse();
    assert_eq!(listed, ids);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_channel_snapshot_of_large_messages_answers_and_says_that_more_exist() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let tutti = client.tutti().await;
    for i in 0..5 {
        client
            .request(send(tutti, &format!("m{i}"), "x".repeat(LARGE)))
            .await
            .unwrap();
    }
    let snapshot = tokio::time::timeout(
        Duration::from_secs(5),
        client.snapshot(SnapshotScope::Channel(tutti)),
    )
    .await
    .expect("sync.snapshot got no answer");
    assert!(!snapshot.messages.is_empty() && snapshot.messages.len() < 5);
    assert!(snapshot.messages_has_more);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_message_over_the_limit_is_refused_and_the_feed_goes_on() {
    let hub = TestHub::start().await;
    let mut watcher = FakeClient::pair_and_connect(&hub).await;
    let tutti = watcher.tutti().await;
    let code: ClientPairingCodeResult = watcher.request(ClientPairingCode {}).await.unwrap();
    let mut sender = FakeClient::pair_with_code_and_connect(&hub, &code.code.to_string()).await;
    let snapshot = watcher.snapshot(SnapshotScope::All).await;
    watcher.subscribe(snapshot.seq).await.unwrap();

    // Nearly 1 MiB: the frame of the request fits, but its feed entry would
    // not.
    let error = sender
        .request(send(tutti, "big", "y".repeat(MAX_FRAME_BYTES - 400)))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Invalid);
    assert!(error.message.contains("limit"), "{}", error.message);
    sender
        .request(send(tutti, "small", "small after".to_owned()))
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = watcher
            .peer
            .read(left)
            .await
            .expect("the small message did not reach the subscriber");
        if let Frame::Event(event) = frame
            && let Ok(HubToClient::Feed(entry)) = event.decode::<HubToClient>()
            && let Some(EntityPayload::Message(m)) = upserted(&entry)
            && m.body == "small after"
        {
            break;
        }
    }
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_brief_or_channel_text_over_its_limit_is_refused() {
    use consort_proto::client_api::{ChannelCreate, TaskCreate};
    use consort_proto::model::PlacementRequest;
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let tutti = client.tutti().await;
    // The `all` snapshot holds every open task and every channel, so these
    // texts have small limits: 32 KiB for a brief, 4 KiB for a purpose and
    // 16 KiB for instructions.
    let brief = client
        .request(TaskCreate {
            channel_id: tutti,
            title: "Big".to_owned(),
            brief: "b".repeat(33 * 1024),
            model_selection: None,
            placement: PlacementRequest::default(),
            approval_mode: None,
            start_at: None,
        })
        .await
        .unwrap_err();
    assert_eq!(brief.code, ErrorCode::Invalid);
    let channel = |purpose: usize, instructions: usize| ChannelCreate {
        name: "Big".to_owned(),
        purpose: "p".repeat(purpose),
        instructions: "i".repeat(instructions),
        icon: None,
        color: None,
        group_id: None,
        coordinator_selection: None,
        task_selection: None,
        repo_ids: Vec::new(),
        default_isolation: None,
        default_approval_mode: None,
        memory_enabled: None,
        tags: Vec::new(),
    };
    let purpose = client.request(channel(5 * 1024, 0)).await.unwrap_err();
    assert_eq!(purpose.code, ErrorCode::Invalid);
    let instructions = client.request(channel(0, 17 * 1024)).await.unwrap_err();
    assert_eq!(instructions.code, ErrorCode::Invalid);
    client.request(channel(4 * 1024, 16 * 1024)).await.unwrap();
    hub.handle.shutdown().await.unwrap();
}
