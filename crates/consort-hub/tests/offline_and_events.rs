//! The worker link under stress: offline queues, resend after a reconnect,
//! contiguous event acks and replays, unknown sessions.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use consort_proto::auth::OpenSession;
use consort_proto::client_api::{
    EntityPayload, MessageSend, WorkerList, WorkerListResult, WorkerRevoke,
};
use consort_proto::ids::{SessionId, TaskId};
use consort_proto::model::{ItemStatus, SessionState, WorkerState};
use consort_proto::status::TaskStatus;
use consort_proto::worker_link::{HubToWorker, SequencedEvent, TurnEndReason};
use serde_json::json;

async fn wait_worker_state(world: &mut World, state: WorkerState) {
    let id = world.worker_id_for_wait();
    world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Worker(w)) if w.id == id && w.state == state))
        .await;
}

trait WorldExt {
    fn worker_id_for_wait(&self) -> consort_proto::ids::WorkerId;
}
impl WorldExt for World {
    fn worker_id_for_wait(&self) -> consort_proto::ids::WorkerId {
        self.worker.worker_id
    }
}

/// Starts a task and brings it to review, so the session is idle.
async fn idle_task(world: &mut World) -> (TaskId, SessionId, SessionId) {
    let coordinator = world.start_coordinator("Start a task").await;
    let started = world
        .worker
        .tool_json(
            coordinator.session_id,
            "start_task",
            json!({"title": "T", "brief": "Do it"}),
        )
        .await;
    let task_id: TaskId = started["task_id"].as_str().unwrap().parse().unwrap();
    let task = world.worker.accept_session_start().await;
    world
        .worker
        .send_events(
            task.session_id,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Task(t)) if t.id == task_id && t.status == TaskStatus::Review))
        .await;
    (task_id, task.session_id, coordinator.session_id)
}

#[tokio::test]
async fn a_message_for_an_offline_worker_waits_and_arrives_after_it_reconnects() {
    let mut world = World::start().await;
    let (task_id, task_session, coordinator_session) = idle_task(&mut world).await;
    let card = {
        let entry = world
            .client
            .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Task(t)) if t.id == task_id))
            .await;
        let Some(EntityPayload::Task(task)) = upserted(&entry) else {
            unreachable!()
        };
        task.card_message_id
    };

    // The worker goes away. The hub marks it offline.
    let worker = std::mem::replace(&mut world.worker, placeholder_worker(&world.hub).await);
    let (old_id, key, account) = (worker.worker_id, worker.key.clone(), worker.account_id);
    worker.peer.close().await;
    world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Worker(w)) if w.id == old_id && w.state == WorkerState::Offline))
        .await;

    // The user writes in the task thread. The message waits in the queue.
    world
        .client
        .request(MessageSend {
            channel_id: world.tutti,
            client_message_id: "offline-1".to_owned(),
            body: "Please add tests.".to_owned(),
            attachments: Vec::new(),
            thread_root_id: Some(card),
            in_timeline: false,
        })
        .await
        .unwrap();
    // The task shows Working although the worker is away.
    world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Task(t)) if t.id == task_id && t.status == TaskStatus::Working))
        .await;

    // The worker comes back and reports its open sessions.
    let mut back = FakeWorker::connect(
        &world.hub,
        old_id,
        key,
        account,
        vec![
            OpenSession {
                session_id: task_session,
                last_seq: 3,
                state: SessionState::Idle,
            },
            OpenSession {
                session_id: coordinator_session,
                last_seq: 2,
                state: SessionState::Active,
            },
        ],
    )
    .await;
    // The hub tells the worker how far it has stored each session.
    let stored: std::collections::HashMap<_, _> = back
        .hello_result
        .sessions
        .iter()
        .map(|s| (s.session_id, s.last_seq))
        .collect();
    assert_eq!(stored.get(&task_session), Some(&3));
    assert_eq!(stored.get(&coordinator_session), Some(&2));

    let (request, command) = back.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!("expected the queued session.turn");
    };
    assert_eq!(turn.session_id, task_session);
    assert_eq!(turn.turn.text, "Please add tests.");
    back.respond_ok(
        &request,
        &consort_proto::worker_link::SessionTurnResult {
            turn_id: "t2".into(),
        },
    )
    .await;
    world.worker = back;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_command_without_an_answer_is_sent_again_after_a_reconnect() {
    let mut world = World::start().await;
    let (_, task_session, coordinator_session) = idle_task(&mut world).await;
    let coordinator_session_id = coordinator_session;

    // End the coordinator turn so a new wake event starts a turn at once.
    world
        .worker
        .send_events(
            coordinator_session_id,
            vec![turn_ended("t1", TurnEndReason::Completed)],
        )
        .await;
    // The hub sends the task_update turn first. Take it without an answer.
    let (first_request, first_command) = world.worker.next_command().await;
    let HubToWorker::SessionTurn(first_turn) = first_command else {
        panic!("expected session.turn");
    };
    assert_eq!(first_turn.session_id, coordinator_session_id);

    // The worker drops before it answers. It comes back.
    let old = std::mem::replace(&mut world.worker, placeholder_worker(&world.hub).await);
    let (id, key, account) = (old.worker_id, old.key.clone(), old.account_id);
    old.peer.close().await;
    let mut back = FakeWorker::connect(
        &world.hub,
        id,
        key,
        account,
        vec![
            OpenSession {
                session_id: task_session,
                last_seq: 3,
                state: SessionState::Idle,
            },
            OpenSession {
                session_id: coordinator_session_id,
                last_seq: 4,
                state: SessionState::Active,
            },
        ],
    )
    .await;
    let (again_request, again_command) = back.next_command().await;
    assert_eq!(
        again_request.id, first_request.id,
        "the same command ID, so the worker can deduplicate"
    );
    let HubToWorker::SessionTurn(again) = again_command else {
        panic!("expected the same session.turn");
    };
    assert_eq!(again.turn.text, first_turn.turn.text);
    world.worker = back;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn event_acks_cover_only_the_contiguous_part_and_replays_change_nothing() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Count").await;
    let sid = start.session_id;
    // start_coordinator sent seq 1 and 2.
    let mk = |seq: u64, text: &str| SequencedEvent {
        seq,
        at: consort_proto::time::Timestamp::now(),
        event: assistant_text(sid, text, ItemStatus::Completed),
    };
    let ack = world
        .worker
        .send_batch(sid, vec![mk(3, "three"), mk(4, "four")])
        .await;
    assert_eq!(ack.last_seq, 4);

    // A replay of old events keeps the ack and changes nothing.
    world.client.forget_feed();
    let ack = world
        .worker
        .send_batch(sid, vec![mk(3, "three again"), mk(4, "four again")])
        .await;
    assert_eq!(ack.last_seq, 4);

    // A gap: seq 7 and 8 arrive but 5 and 6 do not.
    let ack = world
        .worker
        .send_batch(sid, vec![mk(7, "seven"), mk(8, "eight")])
        .await;
    assert_eq!(ack.last_seq, 4, "the ack stops at the gap");
    // The worker sends 5 to 8 again. Now everything is stored.
    let ack = world
        .worker
        .send_batch(
            sid,
            vec![mk(5, "five"), mk(6, "six"), mk(7, "seven"), mk(8, "eight")],
        )
        .await;
    assert_eq!(ack.last_seq, 8);

    // After a reconnect, the hub reports the stored position.
    let worker = std::mem::replace(&mut world.worker, placeholder_worker(&world.hub).await);
    let (id, key, account) = (worker.worker_id, worker.key.clone(), worker.account_id);
    worker.peer.close().await;
    let back = FakeWorker::connect(
        &world.hub,
        id,
        key,
        account,
        vec![OpenSession {
            session_id: sid,
            last_seq: 8,
            state: SessionState::Active,
        }],
    )
    .await;
    assert_eq!(back.hello_result.sessions.len(), 1);
    assert_eq!(back.hello_result.sessions[0].last_seq, 8);
    world.worker = back;

    // The replayed texts never reached the client as items.
    assert!(!world.client.feed.iter().any(|e| matches!(
        upserted(e),
        Some(EntityPayload::SessionItem(i)) if matches!(&i.payload, consort_proto::model::ItemPayload::AssistantMessage(m) if m.text.contains("again"))
    )));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn events_for_an_unknown_session_are_refused() {
    let mut world = World::start().await;
    let error = world
        .worker
        .peer
        .call_raw(
            "session.events",
            serde_json::to_value(consort_proto::worker_link::SessionEvents {
                session_id: SessionId::new(),
                events: vec![SequencedEvent {
                    seq: 1,
                    at: consort_proto::time::Timestamp::now(),
                    event: state(SessionState::Active),
                }],
            })
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, consort_proto::error::ErrorCode::NotFound);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_session_that_the_hub_does_not_know_gets_a_stop_command() {
    let mut world = World::start().await;
    let stray = SessionId::new();
    let worker = std::mem::replace(&mut world.worker, placeholder_worker(&world.hub).await);
    let (id, key, account) = (worker.worker_id, worker.key.clone(), worker.account_id);
    worker.peer.close().await;
    let mut back = FakeWorker::connect(
        &world.hub,
        id,
        key,
        account,
        vec![OpenSession {
            session_id: stray,
            last_seq: 5,
            state: SessionState::Active,
        }],
    )
    .await;
    let (_, command) = back.next_command().await;
    let HubToWorker::SessionStop(stop) = command else {
        panic!("expected session.stop");
    };
    assert_eq!(stop.session_id, stray);
    world.worker = back;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_session_that_the_worker_no_longer_has_is_lost() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Hello").await;
    let worker = std::mem::replace(&mut world.worker, placeholder_worker(&world.hub).await);
    let (id, key, account) = (worker.worker_id, worker.key.clone(), worker.account_id);
    worker.peer.close().await;
    // The worker restarted and lost its sessions: it reports none.
    let mut back = FakeWorker::connect(&world.hub, id, key, account, Vec::new()).await;
    // The next message starts a new session.
    world.worker_connected_say(&mut back).await;
    let new_start = back.accept_session_start().await;
    assert_ne!(new_start.session_id, start.session_id);
    world.worker = back;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn revoking_a_worker_closes_its_connection_and_it_cannot_return() {
    let mut world = World::start().await;
    let id = world.worker.worker_id;
    world
        .client
        .request(WorkerRevoke { worker_id: id })
        .await
        .unwrap();
    assert!(world.worker.peer.closed(WAIT).await);

    let mut peer = Peer::connect(&world.hub.ws_url(consort_proto::auth::WORKER_WS_PATH)).await;
    let error = authenticate(
        &mut peer,
        &world.hub.origin(),
        consort_proto::auth::PeerId::Worker(id),
        &world.worker.key,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, consort_proto::error::ErrorCode::Unauthorized);

    let list: WorkerListResult = world.client.request(WorkerList {}).await.unwrap();
    assert!(
        list.workers
            .iter()
            .find(|w| w.id == id)
            .unwrap()
            .revoked_at
            .is_some()
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn tags_rename_and_a_state_change_reach_the_client() {
    let mut world = World::start().await;
    let id = world.worker.worker_id;
    world
        .client
        .request(consort_proto::client_api::WorkerRename {
            worker_id: id,
            name: "MacBook".to_owned(),
        })
        .await
        .unwrap();
    world
        .client
        .request(consort_proto::client_api::WorkerSetTags {
            worker_id: id,
            tags: vec!["gpu".to_owned(), " gpu ".to_owned()],
        })
        .await
        .unwrap();
    // A tag that breaks the tag rule is refused with `invalid_tag`.
    let refused = world
        .client
        .request(consort_proto::client_api::WorkerSetTags {
            worker_id: id,
            tags: vec!["GPU box".to_owned()],
        })
        .await
        .unwrap_err();
    assert!(format!("{refused:?}").contains("InvalidTag"), "{refused:?}");
    let list: WorkerListResult = world.client.request(WorkerList {}).await.unwrap();
    let worker = list.workers.iter().find(|w| w.id == id).unwrap();
    assert_eq!(worker.name, "MacBook");
    assert_eq!(worker.tags, vec!["gpu".to_owned()]);
    wait_worker_state(&mut world, WorkerState::Online).await;
    world.hub.handle.shutdown().await.unwrap();
}
