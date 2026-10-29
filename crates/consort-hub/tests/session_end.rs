//! How sessions end: requeued events after a restart, hub tools of ended
//! sessions, `session.stop` when the hub ends a session, refused answers, the
//! fallback post, messages for an offline worker, and a channel delete.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::*;
use consort_proto::auth::{OpenSession, PeerId, WORKER_WS_PATH, WorkerHello};
use consort_proto::client_api::{
    ApprovalAnswer, ChannelCreate, ChannelDelete, EntityKind, EntityPayload, FeedChange,
    MemoryWrite, MessageSend, ReadMark, RoutineCreate, SnapshotScope, TaskCreate,
};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::ids::{ChannelId, SessionId, TaskId, WorkerId};
use consort_proto::model::{
    ExpectedHash, ItemStatus, MemoryScope, MessageKind, PlacementRequest, RoutineAction,
    RoutineSchedule, SessionState, WorkerState,
};
use consort_proto::status::TaskStatus;
use consort_proto::worker_link::{HubToWorker, SessionStartResult, TurnEndReason};
use serde_json::json;

fn channel_named(name: &str) -> ChannelCreate {
    ChannelCreate {
        name: name.to_owned(),
        purpose: format!("About {name}"),
        instructions: String::new(),
        icon: None,
        color: None,
        group_id: None,
        coordinator_selection: None,
        task_selection: None,
        repo_ids: Vec::new(),
        default_isolation: None,
        default_approval_mode: None,
        memory_enabled: None,
    }
}

async fn wait_task_status(world: &mut World, id: TaskId, status: TaskStatus) {
    world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Task(t)) if t.id == id && t.status == status))
        .await;
}

async fn wait_worker_state(world: &mut World, id: WorkerId, state: WorkerState) {
    world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Worker(w)) if w.id == id && w.state == state))
        .await;
}

/// The coordinator starts a task, and the worker accepts the task session.
async fn start_task(world: &mut World) -> (SessionId, TaskId, SessionId) {
    let coordinator = world.start_coordinator("Please do this").await;
    let started = world
        .worker
        .tool_json(
            coordinator.session_id,
            "start_task",
            json!({"title": "Fix the bug", "brief": "Fix it"}),
        )
        .await;
    let task_id: TaskId = started["task_id"].as_str().unwrap().parse().unwrap();
    let task = world.worker.accept_session_start().await;
    (coordinator.session_id, task_id, task.session_id)
}

/// Reads commands until one is a `session.stop` for `session_id`. Other
/// commands get an answer, so their queues move on.
async fn wait_stop(worker: &mut FakeWorker, session_id: SessionId) {
    loop {
        let (request, command) = worker.next_command().await;
        match command {
            HubToWorker::SessionStop(stop) if stop.session_id == session_id => {
                worker.respond_ok(&request, &json!({})).await;
                return;
            }
            HubToWorker::SessionStart(_) => {
                worker
                    .respond_ok(
                        &request,
                        &SessionStartResult {
                            workspace_path: "/tmp/workspace".to_owned(),
                            native_ref: None,
                        },
                    )
                    .await;
            }
            _ => worker.respond_ok(&request, &json!({})).await,
        }
    }
}

#[tokio::test]
async fn a_worker_with_open_turns_in_two_channels_can_say_hello_after_a_restart() {
    let mut world = World::start().await;
    let b = world
        .client
        .request(channel_named("B"))
        .await
        .unwrap()
        .channel
        .id;
    world.say(world.everything, "one").await;
    world.worker.accept_session_start().await;
    world.say(b, "two").await;
    world.worker.accept_session_start().await;

    // The worker restarts and has neither session. The hub puts the events
    // of both open turns back at the front of their queues.
    let placeholder = placeholder_worker(&world.hub).await;
    let FakeWorker {
        peer,
        worker_id,
        key,
        ..
    } = std::mem::replace(&mut world.worker, placeholder);
    peer.close().await;
    for _ in 0..2 {
        let mut peer = Peer::connect(&world.hub.ws_url(WORKER_WS_PATH)).await;
        authenticate(
            &mut peer,
            &world.hub.origin(),
            PeerId::Worker(worker_id),
            &key,
        )
        .await
        .unwrap();
        let hello = peer
            .call_raw(
                "hello",
                serde_json::to_value(WorkerHello {
                    worker_id,
                    version: "test".to_owned(),
                    protocol_versions: vec![1],
                    sessions: Vec::new(),
                })
                .unwrap(),
            )
            .await;
        assert!(hello.is_ok(), "{hello:?}");
        peer.close().await;
    }
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_lost_session_has_no_hub_tools_and_is_stopped_when_the_worker_reports_it() {
    let mut world = World::start().await;
    let start = world.start_coordinator("hello").await;
    let sid = start.session_id;
    let placeholder = placeholder_worker(&world.hub).await;
    let worker = std::mem::replace(&mut world.worker, placeholder);
    // The worker restarts and no longer reports the session: it is lost.
    let mut worker = worker.reconnect(&world.hub, Vec::new()).await;

    let result = worker
        .tool_call(sid, "post_message", json!({"body": "From a lost session"}))
        .await;
    assert!(result.is_error, "{:?}", result.content);
    let listed = worker
        .peer
        .call_raw("tool.list", json!({"session_id": sid}))
        .await;
    assert_eq!(listed.unwrap_err().code, ErrorCode::Conflict);

    // The worker finds the session again and reports it. The hub tells it to
    // stop the session, because the hub has ended it.
    let mut worker = worker
        .reconnect(
            &world.hub,
            vec![OpenSession {
                session_id: sid,
                last_seq: 2,
                state: SessionState::Active,
            }],
        )
        .await;
    assert!(
        worker
            .hello_result
            .sessions
            .iter()
            .all(|s| s.session_id != sid)
    );
    wait_stop(&mut worker, sid).await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_session_that_the_hub_marks_lost_gets_a_stop_command() {
    let mut world = World::start().await;
    let start = world.start_coordinator("hello").await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![turn_ended("t1", TurnEndReason::Completed)],
        )
        .await;
    world.say(world.everything, "again").await;
    // The worker says that it does not know the session, so the hub marks it
    // lost. The worker may still run the harness, so it gets session.stop.
    let (request, command) = world.worker.next_command().await;
    assert!(matches!(command, HubToWorker::SessionTurn(_)));
    world
        .worker
        .respond_error(
            &request,
            ErrorBody::new(ErrorCode::NotFound, "No such session."),
        )
        .await;
    wait_stop(&mut world.worker, start.session_id).await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_refused_answer_fails_the_task_so_it_can_be_retried() {
    let mut world = World::start().await;
    let (_, task_id, sid) = start_task(&mut world).await;
    world
        .worker
        .send_events(
            sid,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                approval_request("req-1", "cargo test"),
            ],
        )
        .await;
    let entry = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Approval(a)) if a.request_id.as_str() == "req-1"))
        .await;
    let Some(EntityPayload::Approval(approval)) = upserted(&entry) else {
        unreachable!()
    };
    world
        .client
        .request(ApprovalAnswer {
            approval_id: approval.id,
            option_id: "allow".to_owned(),
        })
        .await
        .unwrap();
    let (request, command) = world.worker.next_command().await;
    assert!(matches!(command, HubToWorker::SessionRespond(_)));
    world
        .worker
        .respond_error(
            &request,
            ErrorBody::new(ErrorCode::Conflict, "The request is closed."),
        )
        .await;
    wait_task_status(&mut world, task_id, TaskStatus::Failed).await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_reply_call_does_not_stop_the_fallback_post() {
    let mut world = World::start().await;
    let start = world.start_coordinator("hello").await;
    let sid = start.session_id;
    // A reply to a message that does not exist fails. Nothing was posted.
    let result = world
        .worker
        .tool_call(
            sid,
            "reply",
            json!({"message_id": "msg_00000000-0000-0000-0000-000000000001", "body": "Hi"}),
        )
        .await;
    assert!(result.is_error);
    world
        .worker
        .send_events(
            sid,
            vec![
                assistant_text(sid, "Hello from the fallback.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.body == "Hello from the fallback." && m.kind == MessageKind::Text))
        .await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_thread_message_for_an_offline_worker_waits_for_the_open_turn() {
    let mut world = World::start().await;
    let (coordinator, task_id, sid) = start_task(&mut world).await;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let card = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Task(t)) if t.id == task_id))
        .await;
    let Some(EntityPayload::Task(task)) = upserted(&card) else {
        unreachable!()
    };
    let card_id = task.card_message_id;

    // The worker goes away while the turn runs.
    let placeholder = placeholder_worker(&world.hub).await;
    let worker = std::mem::replace(&mut world.worker, placeholder);
    let (id, key, account) = (worker.worker_id, worker.key.clone(), worker.account_id);
    worker.peer.close().await;
    wait_worker_state(&mut world, id, WorkerState::Offline).await;

    world
        .client
        .request(MessageSend {
            channel_id: world.everything,
            client_message_id: "while-away".to_owned(),
            body: "Also update the docs.".to_owned(),
            attachments: Vec::new(),
            thread_root_id: Some(card_id),
            in_timeline: false,
        })
        .await
        .unwrap();

    // The worker comes back with both sessions and the turn still open. It
    // must not get a second turn while the first one runs.
    let open = |session_id| OpenSession {
        session_id,
        last_seq: 2,
        state: SessionState::Active,
    };
    let mut back = FakeWorker::connect(
        &world.hub,
        id,
        key,
        account,
        vec![open(coordinator), open(sid)],
    )
    .await;
    if let Some((_, command)) = back.try_next_command(Duration::from_secs(1)).await {
        panic!("the worker got {command:?} while its turn was open");
    }
    // The message is the next turn, after the open one ends.
    back.send_batch(
        sid,
        vec![consort_proto::worker_link::SequencedEvent {
            seq: 3,
            at: consort_proto::time::Timestamp::now(),
            event: turn_ended("t1", TurnEndReason::Completed),
        }],
    )
    .await;
    let (_, command) = back.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!("expected session.turn");
    };
    assert_eq!(turn.turn.text, "Also update the docs.");
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn deleting_a_channel_sends_a_feed_delete_for_each_of_its_entities() {
    let mut world = World::start().await;
    let channel: ChannelId = world
        .client
        .request(channel_named("Gone"))
        .await
        .unwrap()
        .channel
        .id;
    let message = world.say(channel, "hello").await;
    world
        .client
        .request(ReadMark {
            channel_id: channel,
            thread_root_id: None,
            last_read_message_id: message.id,
        })
        .await
        .unwrap();
    let routine = world
        .client
        .request(RoutineCreate {
            channel_id: channel,
            name: "Daily".to_owned(),
            schedule: RoutineSchedule::Interval { seconds: 3600 },
            prompt: "Check.".to_owned(),
            action: RoutineAction::CoordinatorTurn,
            enabled: true,
        })
        .await
        .unwrap()
        .routine;
    let note = world
        .client
        .request(MemoryWrite {
            scope: MemoryScope::Channel(channel),
            path: "notes.md".to_owned(),
            content: "A note.".to_owned(),
            expected_sha256: ExpectedHash::NotExists,
        })
        .await
        .unwrap()
        .note;
    let task = world
        .client
        .request(TaskCreate {
            channel_id: channel,
            title: "Doomed".to_owned(),
            brief: "Do it".to_owned(),
            model_selection: None,
            placement: PlacementRequest::default(),
            approval_mode: None,
            start_at: Some(consort_proto::time::Timestamp::now().plus_millis(3_600_000)),
        })
        .await
        .unwrap()
        .task;

    world
        .client
        .request(ChannelDelete {
            channel_id: channel,
        })
        .await
        .unwrap();
    let expected = [
        (EntityKind::Message, message.id.to_string()),
        (EntityKind::Message, task.card_message_id.to_string()),
        (EntityKind::Task, task.id.to_string()),
        (EntityKind::Routine, routine.id.to_string()),
        (EntityKind::MemoryNote, note.entity_id()),
        (EntityKind::ReadMarker, channel.to_string()),
        (EntityKind::Channel, channel.to_string()),
    ];
    for (kind, id) in expected {
        world
            .client
            .wait_feed(|e| {
                matches!(e.change, FeedChange::Delete(k) if k == kind) && e.entity_id == id
            })
            .await;
    }
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    assert!(snapshot.channels.iter().all(|c| c.id != channel));
    world.hub.handle.shutdown().await.unwrap();
}
