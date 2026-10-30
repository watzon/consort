//! Review 3, finding 8, and a follow-up from the worker: a `session.start`
//! that fails with `conflict` (the worker has the session but forgot the
//! result) left the harness running, and a `signed_out` event that comes
//! after a failed start must still mark the account.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::common::*;
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::MessageFamily;
use consort_proto::model::AuthState;
use consort_proto::status::TaskStatus;
use consort_proto::worker_link::{HubToWorker, SessionEvent};

/// Starts a coordinator, which starts a task, and returns the task ID and
/// the task's `session.start` request, which the worker has not answered.
async fn task_start_request(
    world: &mut World,
) -> (
    consort_proto::ids::TaskId,
    consort_proto::frame::RequestFrame,
    consort_proto::worker_link::SessionStart,
) {
    let coordinator = world.start_coordinator("Please do this").await;
    let started = world
        .worker
        .tool_json(
            coordinator.session_id,
            "start_task",
            serde_json::json!({"title": "Fix the bug", "brief": "Fix it."}),
        )
        .await;
    let task_id = started["task_id"].as_str().unwrap().parse().unwrap();
    let (request, command) = world.worker.next_command().await;
    let HubToWorker::SessionStart(start) = command else {
        panic!("expected session.start, got {}", command.type_name());
    };
    (task_id, request, start)
}

#[tokio::test]
async fn a_start_that_conflicts_stops_the_session_on_the_worker() {
    let mut world = World::start().await;
    let (task_id, request, start) = task_start_request(&mut world).await;
    world
        .worker
        .respond_error(
            &request,
            ErrorBody::new(
                ErrorCode::Conflict,
                "A session with this ID exists already.",
            ),
        )
        .await;
    world.wait_task(task_id, TaskStatus::Failed).await;
    loop {
        let (request, command) = world.worker.next_command().await;
        world
            .worker
            .respond_ok(&request, &serde_json::json!({}))
            .await;
        if let HubToWorker::SessionStop(stop) = command {
            assert_eq!(stop.session_id, start.session_id);
            break;
        }
    }
}

#[tokio::test]
async fn signed_out_after_a_failed_start_still_marks_the_account() {
    let mut world = World::start().await;
    let (task_id, request, start) = task_start_request(&mut world).await;
    world
        .worker
        .respond_error(
            &request,
            ErrorBody::new(
                ErrorCode::Internal,
                "Claude Code is signed out. Run `claude /login` on this machine.",
            ),
        )
        .await;
    world.wait_task(task_id, TaskStatus::Failed).await;
    // The worker spooled the event, so it comes after the start error.
    world
        .worker
        .send_events(
            start.session_id,
            vec![SessionEvent::Error {
                code: "signed_out".to_owned(),
                message: "Run `claude /login` on this machine.".to_owned(),
            }],
        )
        .await;
    let id = world.worker.worker_id;
    let account = world.worker.account_id;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let workers: consort_proto::client_api::WorkerListResult = world
            .client
            .request(consort_proto::client_api::WorkerList {})
            .await
            .unwrap();
        let state = workers
            .workers
            .iter()
            .find(|w| w.id == id)
            .and_then(|w| w.inventory.as_ref())
            .and_then(|i| i.account(account))
            .map(|a| a.auth_state);
        if state == Some(AuthState::SignedOut) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the account stayed {state:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}
