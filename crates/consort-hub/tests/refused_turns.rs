//! Review 3, finding 3: a refused `session.turn` left a task in Working with
//! no open turn, and Retry, Resolve and Stop could not get it out. A refused
//! coordinator turn left its events in the queue, and nothing ran them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::client_api::{DecisionAnswerRequest, MessageSend, TaskRetry};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::MessageFamily;
use consort_proto::ids::{DecisionId, SessionId};
use consort_proto::model::{DecisionAnswer, ItemStatus, SessionState};
use consort_proto::session_host_link::{HubToSessionHost, TurnEndReason};
use consort_proto::status::TaskStatus;

/// Answers commands until a `session.turn` for `session` comes, and
/// refuses it.
async fn refuse_next_turn(worker: &mut FakeWorker, session: SessionId, why: &str) {
    loop {
        let (request, command) = worker.next_command().await;
        if let HubToSessionHost::SessionTurn(turn) = &command
            && turn.session_id == session
        {
            worker
                .respond_error(&request, ErrorBody::new(ErrorCode::Internal, why))
                .await;
            return;
        }
        worker.respond_ok(&request, &serde_json::json!({})).await;
    }
}

async fn task_in_review(world: &mut World) -> (consort_proto::ids::TaskId, SessionId) {
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
    world.wait_task(task_id, TaskStatus::Review).await;
    (task_id, s)
}

async fn write_in_thread(world: &mut World, card: consort_proto::ids::MessageId, text: &str) {
    world
        .client
        .request(MessageSend {
            channel_id: world.tutti,
            client_message_id: format!("cm-{text}"),
            body: text.to_owned(),
            attachments: Vec::new(),
            thread_root_id: Some(card),
            in_timeline: false,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_refused_turn_from_ready_for_review_goes_back_to_ready_for_review() {
    let mut world = World::start().await;
    let (task_id, s) = task_in_review(&mut world).await;
    let card = world.task(task_id).await.card_message_id;
    write_in_thread(&mut world, card, "Also fix the tests.").await;
    refuse_next_turn(&mut world.worker, s, "a turn is already running").await;
    world
        .wait_notice(Some(card), &["did not take the message"])
        .await;
    let task = world.wait_task(task_id, TaskStatus::Review).await;
    assert!(
        task.status_reason
            .as_deref()
            .is_some_and(|r| r.contains("Send it again")),
        "{:?}",
        task.status_reason
    );
    // The same session takes the next message.
    write_in_thread(&mut world, card, "Please try again.").await;
    loop {
        let (request, command) = world.worker.next_command().await;
        if let HubToSessionHost::SessionTurn(turn) = &command
            && turn.session_id == s
        {
            assert!(turn.turn.text.contains("Please try again."));
            world
                .worker
                .respond_ok(&request, &serde_json::json!({"turn_id": "t3"}))
                .await;
            break;
        }
        world
            .worker
            .respond_ok(&request, &serde_json::json!({}))
            .await;
    }
    world.wait_task(task_id, TaskStatus::Working).await;
}

#[tokio::test]
async fn a_refused_turn_with_a_decision_answer_fails_the_task_with_retry() {
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
        .send_events(s, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    world.wait_task(task_id, TaskStatus::NeedsInput).await;
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
    refuse_next_turn(&mut world.worker, s, "the harness is busy").await;
    let task = world.wait_task(task_id, TaskStatus::Failed).await;
    let reason = task.status_reason.unwrap_or_default();
    assert!(reason.contains("Retry"), "{reason}");
    // Retry works, so the task is not stuck.
    world.client.request(TaskRetry { task_id }).await.unwrap();
    world.wait_task(task_id, TaskStatus::Working).await;
}

#[tokio::test]
async fn a_refused_coordinator_turn_runs_when_the_session_ends_its_turn() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Hello").await;
    let c = start.session_id;
    world
        .worker
        .send_events(c, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    world.say(world.tutti, "First question").await;
    refuse_next_turn(&mut world.worker, c, "a turn is already running").await;
    world
        .wait_blocked(&["did not take your message", "tries again"])
        .await;
    assert!(world.notices(None).await.is_empty());
    // The turn that kept the harness busy ends. The hub sends the waiting
    // message at once.
    world
        .worker
        .send_events(c, vec![turn_ended("t-busy", TurnEndReason::Completed)])
        .await;
    let (request, command) = world.worker.next_command().await;
    let HubToSessionHost::SessionTurn(turn) = command else {
        panic!("expected session.turn, got {}", command.type_name());
    };
    assert_eq!(turn.session_id, c);
    assert!(
        turn.turn.text.contains("First question"),
        "{}",
        turn.turn.text
    );
    world
        .worker
        .respond_ok(&request, &serde_json::json!({"turn_id": "t2"}))
        .await;
}
