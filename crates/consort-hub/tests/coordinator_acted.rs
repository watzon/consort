//! Review 3, finding 6: when a coordinator session was lost in the middle
//! of a turn, the hub gave the turn's events to a new session, also when
//! the old turn had acted already. So the new session could start the same
//! task again.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::*;
use consort_proto::model::{SessionRole, SessionState};
use consort_proto::worker_link::{HubToWorker, SessionStartResult};

/// Answers commands for `wait` and returns the first turn of each new
/// coordinator session that starts.
async fn new_coordinator_turns(
    world: &mut World,
    old: consort_proto::ids::SessionId,
    wait: Duration,
) -> Vec<String> {
    let mut turns = Vec::new();
    while let Some((request, command)) = world.worker.try_next_command(wait).await {
        if let HubToWorker::SessionStart(start) = &command {
            if start.session_id != old && start.role == SessionRole::Coordinator {
                turns.push(
                    start
                        .turn
                        .as_ref()
                        .map(|t| t.text.clone())
                        .unwrap_or_default(),
                );
            }
            world
                .worker
                .respond_ok(
                    &request,
                    &SessionStartResult {
                        workspace_path: "/tmp/w".to_owned(),
                        native_ref: None,
                    },
                )
                .await;
        } else {
            world
                .worker
                .respond_ok(&request, &serde_json::json!({}))
                .await;
        }
    }
    turns
}

#[tokio::test]
async fn a_lost_turn_that_started_a_task_is_not_run_again() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Please fix the login bug").await;
    let c = start.session_id;
    world
        .worker
        .tool_json(
            c,
            "start_task",
            serde_json::json!({"title": "Fix the login bug", "brief": "Fix it."}),
        )
        .await;
    let _task_start = world.worker.accept_session_start().await;
    // The coordinator harness dies before its turn ends.
    world
        .worker
        .send_events(c, vec![state(SessionState::Lost)])
        .await;
    world
        .wait_notice(None, &["turn_failed", "already started to act"])
        .await;
    let turns = new_coordinator_turns(&mut world, c, Duration::from_secs(2)).await;
    assert!(
        !turns.iter().any(|t| t.contains("Please fix the login bug")),
        "a new session got the same message: {turns:?}"
    );
}

#[tokio::test]
async fn a_lost_turn_that_called_no_tool_runs_again() {
    let mut world = World::start().await;
    let start = world.start_coordinator("What time is it?").await;
    let c = start.session_id;
    world
        .worker
        .send_events(c, vec![state(SessionState::Lost)])
        .await;
    let turns = new_coordinator_turns(&mut world, c, Duration::from_secs(2)).await;
    assert!(
        turns.iter().any(|t| t.contains("What time is it?")),
        "the message did not reach a new session: {turns:?}"
    );
}
