//! Which account a task runs on, and the move to another account at a usage
//! limit: a task stays on what the assistant runs on, a turn at a limit goes
//! on with another signed-in account of the same harness, and the user can
//! turn the move off.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::client_api::{ChannelUpdate, PreferencesSetFallbackPath};
use consort_proto::ids::{AccountId, SessionId, TaskId};
use consort_proto::model::{FallbackPath, ModelSelection, SessionState, TurnErrorKind};
use consort_proto::session_host_link::{
    HubToSessionHost, SessionEvent, SessionStart, SessionStartResult, TurnEndReason,
};
use consort_proto::status::TaskStatus;
use serde_json::json;

const FIRST: &str = "Claude Code Personal 1";
const SECOND: &str = "Claude Code Personal 2";

fn limit_reached() -> SessionEvent {
    SessionEvent::TurnEnded {
        turn_id: "t1".into(),
        reason: TurnEndReason::Failed,
        error: Some("Internal error: You've hit your weekly limit".to_owned()),
        error_kind: Some(TurnErrorKind::RateLimit),
        resets_at: None,
    }
}

/// The worker has two signed-in Claude accounts. Returns their IDs.
async fn two_accounts(world: &mut World) -> (AccountId, AccountId) {
    let mut inventory = inventory(world.worker.account_id);
    inventory.accounts[0].label = FIRST.to_owned();
    let mut second = inventory.accounts[0].clone();
    second.account_id = AccountId::new();
    second.label = SECOND.to_owned();
    let ids = (inventory.accounts[0].account_id, second.account_id);
    inventory.accounts.push(second);
    world.worker.send_custom_inventory(inventory).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    ids
}

async fn assistant_runs_on(world: &mut World, label: &str) {
    let update: ChannelUpdate = serde_json::from_value(json!({
        "channel_id": world.tutti,
        "coordinator_selection": ModelSelection {
            account_label: label.to_owned(),
            model: "opus".to_owned(),
            options: Default::default(),
        },
    }))
    .unwrap();
    world.client.request(update).await.unwrap();
}

/// Reads commands until a `session.start`. Other commands get an answer.
async fn next_start(worker: &mut FakeWorker) -> SessionStart {
    loop {
        let (request, command) = worker.next_command().await;
        if let HubToSessionHost::SessionStart(start) = command {
            worker
                .respond_ok(
                    &request,
                    &SessionStartResult {
                        workspace_path: "/tmp/workspace".to_owned(),
                        native_ref: None,
                    },
                )
                .await;
            return start;
        }
        worker.respond_ok(&request, &json!({})).await;
    }
}

async fn fail_at_limit(world: &mut World, session: SessionId) {
    world
        .worker
        .send_events(
            session,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                limit_reached(),
            ],
        )
        .await;
}

async fn task_on_first_account(world: &mut World) -> (TaskId, SessionStart) {
    assistant_runs_on(world, FIRST).await;
    world.start_task("Find the warranty").await
}

#[tokio::test]
async fn a_task_runs_on_the_account_and_model_of_the_assistant() {
    let mut world = World::start().await;
    let (_, second) = two_accounts(&mut world).await;
    assistant_runs_on(&mut world, SECOND).await;
    let (task_id, start) = world.start_task("Find the warranty").await;
    assert_eq!(start.account_id, second);
    assert_eq!(start.model, "opus");
    // The task keeps it, also when the channel changes later.
    let task = world.task(task_id).await;
    assert_eq!(task.model_selection.unwrap().account_label, SECOND);
    world.hub.handle.shutdown().await.unwrap();
}

/// The account of the assistant is a default. A task that the assistant puts
/// on a worker without that account still starts.
#[tokio::test]
async fn a_task_at_a_limit_moves_to_the_other_account_and_stays_there() {
    let mut world = World::start().await;
    let (first, second) = two_accounts(&mut world).await;
    let (task_id, start) = task_on_first_account(&mut world).await;
    assert_eq!(start.account_id, first);

    fail_at_limit(&mut world, start.session_id).await;
    let moved = next_start(&mut world.worker).await;
    assert_eq!(moved.account_id, second);
    assert_eq!(moved.model, "opus");
    let turn = moved.turn.unwrap().text;
    assert!(turn.contains("Find the warranty"), "{turn}");

    let task = world.task(task_id).await;
    assert_eq!(task.status, TaskStatus::Working);
    assert_eq!(task.current_session_id, Some(moved.session_id));
    assert_eq!(task.model_selection.unwrap().account_label, SECOND);
    world
        .wait_notice(Some(task.card_message_id), &["account_fallback", SECOND])
        .await;
    world.hub.handle.shutdown().await.unwrap();
}

/// No account is left: the task fails once, and the reason is for a person.
#[tokio::test]
async fn a_task_fails_when_every_account_is_at_its_limit() {
    let mut world = World::start().await;
    two_accounts(&mut world).await;
    let (task_id, start) = task_on_first_account(&mut world).await;
    fail_at_limit(&mut world, start.session_id).await;
    let moved = next_start(&mut world.worker).await;

    fail_at_limit(&mut world, moved.session_id).await;
    let task = world.wait_task(task_id, TaskStatus::Failed).await;
    let reason = task.status_reason.unwrap();
    assert!(reason.starts_with(SECOND), "{reason}");
    assert!(reason.contains("usage limit"), "{reason}");
    assert!(!reason.contains("Internal error"), "{reason}");
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_task_fails_at_a_limit_when_the_user_turned_the_built_in_row_off() {
    let mut world = World::start().await;
    two_accounts(&mut world).await;
    world
        .client
        .request(PreferencesSetFallbackPath {
            path: FallbackPath {
                other_accounts: false,
                steps: Vec::new(),
            },
        })
        .await
        .unwrap();
    let (task_id, start) = task_on_first_account(&mut world).await;
    fail_at_limit(&mut world, start.session_id).await;
    world.wait_task(task_id, TaskStatus::Failed).await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_assistant_at_a_limit_moves_and_answers_the_same_message() {
    let mut world = World::start().await;
    let (first, second) = two_accounts(&mut world).await;
    assistant_runs_on(&mut world, FIRST).await;
    let start = world.start_coordinator("hello").await;
    assert_eq!(start.account_id, first);

    world
        .worker
        .send_events(start.session_id, vec![limit_reached()])
        .await;
    let moved = next_start(&mut world.worker).await;
    assert_eq!(moved.account_id, second);
    assert!(moved.turn.unwrap().text.contains("hello"));
    world.wait_notice(None, &["account_fallback", SECOND]).await;

    // A new task now goes to the account with room, with no failed turn.
    world
        .worker
        .send_events(
            moved.session_id,
            vec![state(SessionState::Active), turn_started("t2")],
        )
        .await;
    let started = world
        .worker
        .tool_json(
            moved.session_id,
            "start_task",
            json!({"title": "Build", "brief": "Build it"}),
        )
        .await;
    assert!(started["task_id"].is_string(), "{started}");
    let task = next_start(&mut world.worker).await;
    assert_eq!(task.account_id, second);
    world.hub.handle.shutdown().await.unwrap();
}
