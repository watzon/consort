//! Review 3, findings 1 and 2: `reopened` failed a turn that the worker
//! never got, and it canceled decisions from `ask_decision`, which need no
//! harness request. Both left the hub in a state that it could not leave.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::*;
use consort_proto::auth::OpenSession;
use consort_proto::client_api::{DecisionAnswerRequest, MessageSend};
use consort_proto::ids::{AccountId, DecisionId, WorkerId};
use consort_proto::model::{DecisionAnswer, ItemStatus, SessionState, WorkerState};
use consort_proto::status::TaskStatus;
use consort_proto::time::Timestamp;
use consort_proto::worker_link::{HubToWorker, SequencedEvent, SessionEvent, TurnEndReason};
use ed25519_dalek::SigningKey;

/// Closes the worker's connection and waits until the hub shows it offline.
async fn worker_offline(world: &mut World) -> (WorkerId, SigningKey, AccountId) {
    let placeholder = placeholder_worker(&world.hub).await;
    let worker = std::mem::replace(&mut world.worker, placeholder);
    let ids = (worker.worker_id, worker.key.clone(), worker.account_id);
    worker.peer.close().await;
    world.wait_worker(ids.0, WorkerState::Offline).await;
    ids
}

fn at(seq: u64, event: SessionEvent) -> SequencedEvent {
    SequencedEvent {
        seq,
        at: Timestamp::now(),
        event,
    }
}

/// Answers commands until a `session.turn` for `session` comes, and
/// returns its text.
async fn next_turn(worker: &mut FakeWorker, session: consort_proto::ids::SessionId) -> String {
    loop {
        let (request, command) = worker.next_command().await;
        if let HubToWorker::SessionTurn(turn) = &command
            && turn.session_id == session
        {
            worker
                .respond_ok(&request, &serde_json::json!({"turn_id": "t2"}))
                .await;
            return turn.turn.text.clone();
        }
        worker.respond_ok(&request, &serde_json::json!({})).await;
    }
}

#[tokio::test]
async fn reopened_keeps_a_task_turn_that_still_waits_in_the_queue() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Fix it.").await;
    let s = start.session_id;
    world
        .worker
        .send_events(
            s,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                assistant_text(s, "Done.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    let task = world.wait_task(task_id, TaskStatus::Review).await;
    let coordinator = world.coordinator_session.unwrap();
    let (wid, key, acc) = worker_offline(&mut world).await;

    // The user writes in the task thread while the worker is away.
    world
        .client
        .request(MessageSend {
            channel_id: world.everything,
            client_message_id: "cm-follow-up".to_owned(),
            body: "Also fix the tests.".to_owned(),
            attachments: Vec::new(),
            thread_root_id: Some(task.card_message_id),
            in_timeline: false,
        })
        .await
        .unwrap();
    world.wait_task(task_id, TaskStatus::Working).await;

    // The worker restarted, restored the session and spooled `reopened`.
    let mut back = FakeWorker::connect(
        &world.hub,
        wid,
        key,
        acc,
        vec![
            OpenSession {
                session_id: s,
                last_seq: 4,
                state: SessionState::Idle,
            },
            OpenSession {
                session_id: coordinator,
                last_seq: 2,
                state: SessionState::Active,
            },
        ],
    )
    .await;
    back.send_batch(s, vec![at(5, SessionEvent::Reopened)])
        .await;
    let after_reopened = world.task(task_id).await;
    assert_eq!(
        after_reopened.status,
        TaskStatus::Working,
        "{after_reopened:?}"
    );

    // The queued turn goes out, runs, and ends.
    let text = next_turn(&mut back, s).await;
    assert!(text.contains("Also fix the tests."), "{text}");
    back.send_batch(
        s,
        vec![
            at(6, turn_started("t2")),
            at(7, assistant_text(s, "Tests fixed.", ItemStatus::Completed)),
            at(8, turn_ended("t2", TurnEndReason::Completed)),
        ],
    )
    .await;
    let end = world.wait_task(task_id, TaskStatus::Review).await;
    assert_eq!(end.review_summary.as_deref(), Some("Tests fixed."));
}

#[tokio::test]
async fn reopened_keeps_a_coordinator_turn_that_still_waits_in_the_queue() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Hello").await;
    let c = start.session_id;
    world
        .worker
        .send_events(c, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (wid, key, acc) = worker_offline(&mut world).await;
    world.say(world.everything, "What is on my calendar?").await;

    let mut back = FakeWorker::connect(
        &world.hub,
        wid,
        key,
        acc,
        vec![OpenSession {
            session_id: c,
            last_seq: 3,
            state: SessionState::Idle,
        }],
    )
    .await;
    back.send_batch(c, vec![at(4, SessionEvent::Reopened)])
        .await;
    let text = next_turn(&mut back, c).await;
    assert!(text.contains("What is on my calendar?"), "{text}");
    let notices = world.notices(None).await;
    assert!(
        !notices
            .iter()
            .any(|n| n.contains("Send your message again")),
        "{notices:?}"
    );
}

/// After a reconnect the hub flushes its queue at once. The queued turn can
/// reach the worker before the worker's `reopened` event reaches the hub. A
/// turn that went out on the new connection is new to the restarted worker,
/// so `reopened` must not end it. This test makes that order certain: the
/// worker has the turn before it sends `reopened`.
#[tokio::test]
async fn reopened_keeps_a_coordinator_turn_that_went_out_before_it_was_handled() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Hello").await;
    let c = start.session_id;
    world
        .worker
        .send_events(c, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (wid, key, acc) = worker_offline(&mut world).await;
    world.say(world.everything, "What is on my calendar?").await;

    let mut back = FakeWorker::connect(
        &world.hub,
        wid,
        key,
        acc,
        vec![OpenSession {
            session_id: c,
            last_seq: 3,
            state: SessionState::Idle,
        }],
    )
    .await;
    // The hub sent the turn on this connection, and the worker answered it.
    let text = next_turn(&mut back, c).await;
    assert!(text.contains("What is on my calendar?"), "{text}");
    back.send_batch(c, vec![at(4, SessionEvent::Reopened)])
        .await;
    // The turn goes on: it ends in the usual way, with no notice and with
    // the reply of the assistant.
    back.send_batch(
        c,
        vec![
            at(5, turn_started("t2")),
            at(
                6,
                assistant_text(c, "Nothing today.", ItemStatus::Completed),
            ),
            at(7, turn_ended("t2", TurnEndReason::Completed)),
        ],
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let notices = world.notices(None).await;
    assert!(
        !notices
            .iter()
            .any(|n| n.contains("Send your message again")),
        "{notices:?}"
    );
}

/// A turn that was open when the worker restarted went out on an earlier
/// connection, so `reopened` still ends it and says so.
#[tokio::test]
async fn reopened_ends_a_coordinator_turn_that_went_out_on_an_earlier_connection() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Hello").await;
    let c = start.session_id;
    world
        .worker
        .send_events(c, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    // A second turn goes out on this connection. Its worker never ends it.
    world.say(world.everything, "What is on my calendar?").await;
    let text = next_turn(&mut world.worker, c).await;
    assert!(text.contains("What is on my calendar?"), "{text}");
    let (wid, key, acc) = worker_offline(&mut world).await;

    let mut back = FakeWorker::connect(
        &world.hub,
        wid,
        key,
        acc,
        vec![OpenSession {
            session_id: c,
            last_seq: 3,
            state: SessionState::Active,
        }],
    )
    .await;
    back.send_batch(c, vec![at(4, SessionEvent::Reopened)])
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let notices = world.notices(None).await;
    assert!(
        notices
            .iter()
            .any(|n| n.contains("Send your message again")),
        "{notices:?}"
    );
}

#[tokio::test]
async fn reopened_keeps_a_task_decision_from_ask_decision() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Fix it.").await;
    let s = start.session_id;
    world
        .worker
        .send_events(s, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let asked = world
        .worker
        .tool_json(
            s,
            "ask_decision",
            serde_json::json!({
                "question": "Which branch?",
                "options": [
                    {"label": "main", "consequence": "Push to main."},
                    {"label": "dev", "consequence": "Push to dev."}
                ]
            }),
        )
        .await;
    let decision_id: DecisionId = asked["decision_id"].as_str().unwrap().parse().unwrap();
    world
        .worker
        .send_events(
            s,
            vec![
                assistant_text(s, "I asked which branch.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    world.wait_task(task_id, TaskStatus::NeedsInput).await;

    // The worker restarted and opened the session again.
    world
        .worker
        .send_events(s, vec![SessionEvent::Reopened])
        .await;
    assert_eq!(world.task(task_id).await.status, TaskStatus::NeedsInput);

    // The decision still takes its answer, and the answer is the next turn.
    world
        .client
        .request(DecisionAnswerRequest {
            decision_id,
            answer: DecisionAnswer {
                option_index: Some(0),
                text: None,
            },
        })
        .await
        .unwrap();
    let text = next_turn(&mut world.worker, s).await;
    assert!(text.contains("decision_answered"), "{text}");
    world
        .worker
        .send_events(
            s,
            vec![
                turn_started("t2"),
                assistant_text(s, "Pushed to main.", ItemStatus::Completed),
                turn_ended("t2", TurnEndReason::Completed),
            ],
        )
        .await;
    world.wait_task(task_id, TaskStatus::Review).await;
}

#[tokio::test]
async fn reopened_after_the_turn_ended_with_a_harness_question_moves_the_task_on() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Fix it.").await;
    let s = start.session_id;
    world
        .worker
        .send_events(
            s,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                question_request("q1", "Push now?"),
                assistant_text(s, "Waiting for an answer.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    world.wait_task(task_id, TaskStatus::NeedsInput).await;
    // The harness question is gone after the restart. Nothing waits now, so
    // the task does not stay in Needs input.
    world
        .worker
        .send_events(s, vec![SessionEvent::Reopened])
        .await;
    let task = world.wait_task(task_id, TaskStatus::Review).await;
    assert_eq!(
        task.review_summary.as_deref(),
        Some("Waiting for an answer.")
    );
}

#[tokio::test]
async fn a_lost_coordinator_session_keeps_its_channel_decision() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Should I book the trip?").await;
    let c = start.session_id;
    let asked = world
        .worker
        .tool_json(
            c,
            "ask_decision",
            serde_json::json!({
                "question": "Book the trip?",
                "options": [
                    {"label": "Yes", "consequence": "I book it."},
                    {"label": "No", "consequence": "I do nothing."}
                ]
            }),
        )
        .await;
    let decision_id: DecisionId = asked["decision_id"].as_str().unwrap().parse().unwrap();
    world
        .worker
        .send_events(c, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    // The coordinator session is lost.
    world
        .worker
        .send_events(c, vec![state(SessionState::Lost)])
        .await;

    // The answer still goes to the channel queue, and a new session gets it.
    world
        .client
        .request(DecisionAnswerRequest {
            decision_id,
            answer: DecisionAnswer {
                option_index: Some(0),
                text: None,
            },
        })
        .await
        .unwrap();
    loop {
        let (request, command) = world.worker.next_command().await;
        if let HubToWorker::SessionStart(next) = &command
            && next.session_id != c
        {
            let text = next
                .turn
                .as_ref()
                .map(|t| t.text.clone())
                .unwrap_or_default();
            assert!(text.contains("decision_answered"), "{text}");
            break;
        }
        world
            .worker
            .respond_ok(&request, &serde_json::json!({}))
            .await;
    }
}
