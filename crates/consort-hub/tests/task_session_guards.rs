//! Review cycle 2: events of a replaced session, refused steers and
//! answers, and what the hub drops or sends when it ends a task session.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::client_api::{
    ApprovalAnswer, MessageSend, SnapshotScope, TaskArchive, TaskDelete, TaskResolve,
};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::MessageFamily;
use consort_proto::ids::{SessionId, TaskId};
use consort_proto::model::{ApprovalStatus, ItemStatus, SessionState};
use consort_proto::status::TaskStatus;
use consort_proto::worker_link::{HubToWorker, SessionEvent, TurnEndReason};
use serde_json::json;

/// Fails the first turn of a task and retries it. Returns the old and the
/// new session, with the new turn running.
async fn retried_task(world: &mut World) -> (TaskId, SessionId, SessionId) {
    let (task_id, start) = world.start_task("Do it").await;
    let s1 = start.session_id;
    world
        .worker
        .send_events(
            s1,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                SessionEvent::TurnEnded {
                    turn_id: "t1".into(),
                    reason: TurnEndReason::Failed,
                    error: Some("boom".to_owned()),
                    error_kind: None,
                    resets_at: None,
                },
            ],
        )
        .await;
    world.wait_task(task_id, TaskStatus::Failed).await;
    let s2 = world.retry_task(task_id, s1).await;
    world
        .worker
        .send_events(s2, vec![state(SessionState::Active), turn_started("t2")])
        .await;
    world.wait_task(task_id, TaskStatus::Working).await;
    (task_id, s1, s2)
}

/// Finding 1: the spooled `turn.ended` of the old session moved the task to
/// Ready for review with the old summary while the new session ran.
#[tokio::test]
async fn the_turn_of_a_replaced_session_does_not_move_the_retried_task() {
    let mut world = World::start().await;
    let (task_id, s1, s2) = retried_task(&mut world).await;
    world
        .worker
        .send_events(
            s1,
            vec![
                assistant_text(s1, "OLD SESSION SUMMARY", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    let task = world.task(task_id).await;
    assert_eq!(task.current_session_id, Some(s2));
    assert_eq!(task.status, TaskStatus::Working);
    assert_eq!(task.review_summary, None);
    // The new session still ends its own turn.
    world
        .worker
        .send_events(
            s2,
            vec![
                assistant_text(s2, "New work is done.", ItemStatus::Completed),
                turn_ended("t2", TurnEndReason::Completed),
            ],
        )
        .await;
    let task = world.wait_task(task_id, TaskStatus::Review).await;
    assert_eq!(task.review_summary.as_deref(), Some("New work is done."));
    world.hub.handle.shutdown().await.unwrap();
}

/// Finding 1: a `request.opened` of the old session made a pending approval
/// for a dead session and moved the new run to Needs input.
#[tokio::test]
async fn a_request_of_a_replaced_session_opens_nothing() {
    let mut world = World::start().await;
    let (task_id, s1, _) = retried_task(&mut world).await;
    world
        .worker
        .send_events(s1, vec![approval_request("req-old", "rm -rf build")])
        .await;
    let task = world.task(task_id).await;
    assert_eq!(task.status, TaskStatus::Working);
    let snapshot = world
        .client
        .snapshot(SnapshotScope::Channel(world.tutti))
        .await;
    assert!(
        snapshot.approvals.iter().all(|a| a.session_id != s1),
        "an approval opened for the old session"
    );
    world.hub.handle.shutdown().await.unwrap();
}

/// Sends a thread message while the turn of `session` runs, and returns the
/// steer command that the worker gets.
async fn steer(
    world: &mut World,
    task_id: TaskId,
    text: &str,
) -> consort_proto::frame::RequestFrame {
    let task = world.task(task_id).await;
    world
        .client
        .request(MessageSend {
            channel_id: world.tutti,
            client_message_id: format!("cm-{text}"),
            body: text.to_owned(),
            attachments: Vec::new(),
            thread_root_id: Some(task.card_message_id),
            in_timeline: false,
        })
        .await
        .unwrap();
    let (request, command) = world.worker.next_command().await;
    assert!(
        matches!(command, HubToWorker::SessionSteer(_)),
        "{}",
        command.type_name()
    );
    request
}

/// Finding 4: a refused steer only logged, so the user's message was lost.
/// The refusal comes before the hub sees the end of the turn, so the text
/// waits in the inbox and starts the next turn.
#[tokio::test]
async fn a_refused_steer_waits_for_the_turn_to_end_and_then_starts_a_turn() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    let s1 = start.session_id;
    world
        .worker
        .send_events(s1, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let request = steer(&mut world, task_id, "Also update the docs").await;
    world
        .worker
        .respond_error(
            &request,
            ErrorBody::new(
                ErrorCode::Internal,
                "the harness returned an error: no turn is running",
            ),
        )
        .await;
    world
        .worker
        .send_events(
            s1,
            vec![
                assistant_text(s1, "Done.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    let (_, command) = world.worker.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!("expected session.turn, got {}", command.type_name());
    };
    assert_eq!(turn.session_id, s1);
    assert_eq!(turn.turn.text, "Also update the docs");
    assert_eq!(world.task(task_id).await.status, TaskStatus::Working);
    world.hub.handle.shutdown().await.unwrap();
}

/// Finding 4: the turn ended before the refusal came, so the text starts a
/// new turn at once, and the task goes back to Working.
#[tokio::test]
async fn a_steer_refused_after_the_turn_ended_starts_a_new_turn() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    let s1 = start.session_id;
    world
        .worker
        .send_events(s1, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let request = steer(&mut world, task_id, "One more thing").await;
    world
        .worker
        .send_events(
            s1,
            vec![
                assistant_text(s1, "Done.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    world.wait_task(task_id, TaskStatus::Review).await;
    world
        .worker
        .respond_error(
            &request,
            ErrorBody::new(
                ErrorCode::Internal,
                "the harness returned an error: no turn is running",
            ),
        )
        .await;
    let (_, command) = world.worker.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!("expected session.turn, got {}", command.type_name());
    };
    assert_eq!(turn.turn.text, "One more thing");
    world.wait_task(task_id, TaskStatus::Working).await;
    world.hub.handle.shutdown().await.unwrap();
}

/// Finding 4: when the task cannot take the text any more, the thread says
/// so.
#[tokio::test]
async fn a_refused_steer_for_a_task_that_failed_posts_a_notice() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    let s1 = start.session_id;
    world
        .worker
        .send_events(s1, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let request = steer(&mut world, task_id, "Stop that").await;
    world
        .worker
        .send_events(
            s1,
            vec![SessionEvent::TurnEnded {
                turn_id: "t1".into(),
                reason: TurnEndReason::Failed,
                error: Some("boom".to_owned()),
                error_kind: None,
                resets_at: None,
            }],
        )
        .await;
    world.wait_task(task_id, TaskStatus::Failed).await;
    world
        .worker
        .respond_error(
            &request,
            ErrorBody::new(ErrorCode::Internal, "no turn is running"),
        )
        .await;
    let card = world.task(task_id).await.card_message_id;
    world
        .wait_notice(Some(card), &["message_not_sent", "no turn is running"])
        .await;
    world.hub.handle.shutdown().await.unwrap();
}

/// Finding 5: the harness had closed the request, so the worker answered
/// `not_found`. That failed a task whose turn still ran.
#[tokio::test]
async fn an_answer_for_a_closed_request_cancels_the_approval_and_keeps_the_task() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    let s1 = start.session_id;
    world
        .worker
        .send_events(
            s1,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                approval_request("req-1", "cargo test"),
            ],
        )
        .await;
    world.wait_task(task_id, TaskStatus::NeedsInput).await;
    let snapshot = world
        .client
        .snapshot(SnapshotScope::Channel(world.tutti))
        .await;
    let approval = snapshot
        .approvals
        .iter()
        .find(|a| a.request_id.as_str() == "req-1")
        .unwrap()
        .clone();
    world
        .client
        .request(ApprovalAnswer {
            approval_id: approval.id,
            option_id: "allow".to_owned(),
            checked: false,
            checked_hash: None,
        })
        .await
        .unwrap();
    let (request, command) = world.worker.next_command().await;
    assert!(matches!(command, HubToWorker::SessionRespond(_)));
    world
        .worker
        .respond_error(
            &request,
            ErrorBody::new(
                ErrorCode::NotFound,
                "no open harness request has the ID `req-1`",
            ),
        )
        .await;
    let card = world.task(task_id).await.card_message_id;
    world
        .wait_notice(Some(card), &["answer_not_delivered", "already closed"])
        .await;
    let canceled = world
        .client
        .wait_feed(|e| {
            matches!(upserted(e), Some(consort_proto::client_api::EntityPayload::Approval(a))
                if a.id == approval.id && a.status == ApprovalStatus::Canceled)
        })
        .await;
    assert!(upserted(&canceled).is_some());
    assert_eq!(world.task(task_id).await.status, TaskStatus::Working);
    // The turn decides the status.
    world
        .worker
        .send_events(
            s1,
            vec![
                assistant_text(s1, "Tests pass.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    world.wait_task(task_id, TaskStatus::Review).await;
    world.hub.handle.shutdown().await.unwrap();
}

/// Waits for `session.stop` and then `workspace.release` for the task, and
/// answers both. Fails on any other command.
async fn expect_stop_then_release(world: &mut World, session_id: SessionId, task_id: TaskId) {
    let (request, command) = world.worker.next_command().await;
    let HubToWorker::SessionStop(stop) = command else {
        panic!("expected session.stop, got {}", command.type_name());
    };
    assert_eq!(stop.session_id, session_id);
    world.worker.respond_ok(&request, &json!({})).await;
    let (request, command) = world.worker.next_command().await;
    let HubToWorker::WorkspaceRelease(release) = command else {
        panic!("expected workspace.release, got {}", command.type_name());
    };
    assert_eq!(release.task_id, task_id);
    assert!(release.delete);
    world.worker.respond_ok(&request, &json!({})).await;
}

/// Findings 9 and 12: a turn that waited for an offline worker still went
/// out after the task was deleted, before the stop. The hub never sent
/// `workspace.release`.
#[tokio::test]
async fn deleting_a_task_drops_its_queued_turn_and_releases_its_workspace() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    let s1 = start.session_id;
    world
        .worker
        .send_events(
            s1,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                assistant_text(s1, "Done.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    let task = world.wait_task(task_id, TaskStatus::Review).await;
    // The worker goes away. A message for the task waits in the queue.
    let placeholder = placeholder_worker(&world.hub).await;
    let worker = std::mem::replace(&mut world.worker, placeholder);
    let (worker_id, key, account_id) = (worker.worker_id, worker.key.clone(), worker.account_id);
    worker.peer.close().await;
    world
        .wait_worker(worker_id, consort_proto::model::WorkerState::Offline)
        .await;
    world
        .client
        .request(MessageSend {
            channel_id: world.tutti,
            client_message_id: "cm-queued".to_owned(),
            body: "Please also add a test".to_owned(),
            attachments: Vec::new(),
            thread_root_id: Some(task.card_message_id),
            in_timeline: false,
        })
        .await
        .unwrap();
    world.client.request(TaskDelete { task_id }).await.unwrap();
    // The worker comes back with the session still open.
    let back = FakeWorker::connect(
        &world.hub,
        worker_id,
        key,
        account_id,
        vec![
            consort_proto::auth::OpenSession {
                session_id: world.coordinator_session.unwrap(),
                last_seq: 2,
                state: SessionState::Active,
            },
            consort_proto::auth::OpenSession {
                session_id: s1,
                last_seq: 4,
                state: SessionState::Idle,
            },
        ],
    )
    .await;
    world.worker = back;
    // The hub ended the session, so the hello also stops it. That stop is a
    // direct request; the queued stop and the release follow. No turn.
    let mut stops = 0;
    let mut released = false;
    while !released {
        let (request, command) = world.worker.next_command().await;
        match command {
            HubToWorker::SessionStop(stop) => {
                assert_eq!(stop.session_id, s1);
                stops += 1;
                world.worker.respond_ok(&request, &json!({})).await;
            }
            HubToWorker::WorkspaceRelease(release) => {
                assert!(stops > 0, "the release came before the stop");
                assert_eq!(release.task_id, task_id);
                assert!(release.delete);
                world.worker.respond_ok(&request, &json!({})).await;
                released = true;
            }
            other => panic!("unexpected command {}", other.type_name()),
        }
    }
    assert!(
        world
            .worker
            .try_next_command(Duration::from_millis(500))
            .await
            .is_none(),
        "a queued command of the deleted task went out"
    );
    world.hub.handle.shutdown().await.unwrap();
}

/// Finding 12: archiving a resolved task stops its session and releases its
/// workspace. Resolving alone does neither, because Not done needs them.
#[tokio::test]
async fn archiving_a_resolved_task_stops_its_session_and_releases_its_workspace() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    let s1 = start.session_id;
    world
        .worker
        .send_events(
            s1,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                assistant_text(s1, "Done.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    world.wait_task(task_id, TaskStatus::Review).await;
    world.client.request(TaskResolve { task_id }).await.unwrap();
    assert!(
        world
            .worker
            .try_next_command(Duration::from_millis(500))
            .await
            .is_none(),
        "resolving sent a command"
    );
    world
        .client
        .request(TaskArchive {
            task_id,
            archived: true,
        })
        .await
        .unwrap();
    expect_stop_then_release(&mut world, s1, task_id).await;
    world.hub.handle.shutdown().await.unwrap();
}
