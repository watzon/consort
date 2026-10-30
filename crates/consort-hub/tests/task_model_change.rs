//! A model change for a task that already started: `task.update` saves the
//! selection at once and never cuts a running turn. The next turn carries
//! the change in the same session, or the hub moves the task to a new
//! session on the same worker.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::client_api::{
    DecisionAnswerRequest, MessageSend, TaskArchive, TaskResolve, TaskRetry, TaskUpdate,
};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::MessageFamily as _;
use consort_proto::ids::{AccountId, DecisionId, SessionId, TaskId};
use consort_proto::model::{
    AuthState, DecisionAnswer, HarnessAccount, ItemStatus, ModelInfo, ModelSelection, OptionValue,
    SessionState,
};
use consort_proto::status::TaskStatus;
use consort_proto::worker_link::{HubToWorker, SessionStart, SessionTurn, TurnEndReason};
use serde_json::json;

fn selection(label: &str, model: &str, effort: Option<&str>) -> ModelSelection {
    ModelSelection {
        account_label: label.to_owned(),
        model: model.to_owned(),
        options: effort
            .map(|e| ("effort".to_owned(), OptionValue::Text(e.to_owned())))
            .into_iter()
            .collect(),
    }
}

async fn choose(
    world: &mut World,
    task_id: TaskId,
    value: Option<ModelSelection>,
) -> Result<consort_proto::model::Task, ErrorBody> {
    let update: TaskUpdate = serde_json::from_value(json!({
        "task_id": task_id,
        "model_selection": value,
    }))
    .unwrap();
    world.client.request(update).await.map(|r| r.task)
}

/// A task whose first turn ended. It is in Ready for review, and its
/// session is idle.
async fn idle_task(world: &mut World) -> (TaskId, SessionStart) {
    let (task_id, start) = world.start_task("Do it").await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                assistant_text(start.session_id, "Done.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    world.wait_task(task_id, TaskStatus::Review).await;
    (task_id, start)
}

async fn reply(world: &mut World, task_id: TaskId, text: &str) {
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
}

async fn next_turn(world: &mut World) -> (consort_proto::frame::RequestFrame, SessionTurn) {
    let (request, command) = world.worker.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!("expected session.turn, got {}", command.type_name());
    };
    (request, turn)
}

/// An inventory with a second account, "Codex", on another harness.
fn two_accounts(first: AccountId) -> (consort_proto::model::Inventory, AccountId) {
    let mut inventory = inventory(first);
    let mut second: HarnessAccount = inventory.accounts[0].clone();
    let second_id = AccountId::new();
    second.account_id = second_id;
    second.harness = "codex".to_owned();
    second.label = "Codex".to_owned();
    second.auth_state = AuthState::SignedIn;
    second.models = vec![ModelInfo {
        id: "gpt-5".to_owned(),
        name: "GPT 5".to_owned(),
        options: Vec::new(),
    }];
    inventory.accounts.push(second);
    (inventory, second_id)
}

/// Answers `session.stop` and `session.start` of a session switch. Returns
/// the stopped and the new session.
async fn switch_commands(world: &mut World) -> (SessionId, SessionStart) {
    let mut stopped = None;
    let mut started = None;
    while stopped.is_none() || started.is_none() {
        let (request, command) = world.worker.next_command().await;
        match command {
            HubToWorker::SessionStop(stop) => {
                world.worker.respond_ok(&request, &json!({})).await;
                stopped = Some(stop.session_id);
            }
            HubToWorker::SessionStart(start) => {
                world
                    .worker
                    .respond_ok(
                        &request,
                        &consort_proto::worker_link::SessionStartResult {
                            workspace_path: "/tmp/workspace".to_owned(),
                            native_ref: None,
                        },
                    )
                    .await;
                started = Some(start);
            }
            other => panic!("unexpected command {}", other.type_name()),
        }
    }
    (stopped.unwrap(), started.unwrap())
}

#[tokio::test]
async fn a_started_task_saves_the_new_model_and_the_next_turn_carries_it() {
    let mut world = World::start().await;
    let (task_id, _start) = idle_task(&mut world).await;
    let saved = choose(
        &mut world,
        task_id,
        Some(selection("Claude Code", "sonnet", Some("high"))),
    )
    .await
    .unwrap();
    assert_eq!(
        saved.model_selection,
        Some(selection("Claude Code", "sonnet", Some("high")))
    );
    assert!(
        world
            .worker
            .try_next_command(Duration::from_millis(300))
            .await
            .is_none(),
        "the change alone sends nothing"
    );

    reply(&mut world, task_id, "Now polish it").await;
    let (request, turn) = next_turn(&mut world).await;
    assert_eq!(turn.turn.text, "Now polish it");
    let change = turn.model_change.expect("the turn carries the change");
    assert_eq!(change.model, "sonnet");
    assert_eq!(
        change.options.get("effort"),
        Some(&OptionValue::Text("high".to_owned()))
    );
    world
        .worker
        .respond_ok(&request, &json!({"turn_id": "t2"}))
        .await;

    // The session runs the model now, so the next turn carries no change.
    let session_id = world.task(task_id).await.current_session_id.unwrap();
    world
        .worker
        .send_events(
            session_id,
            vec![
                turn_started("t2"),
                assistant_text(session_id, "Polished.", ItemStatus::Completed),
                turn_ended("t2", TurnEndReason::Completed),
            ],
        )
        .await;
    world.wait_task(task_id, TaskStatus::Review).await;
    reply(&mut world, task_id, "Thanks").await;
    let (_, turn) = next_turn(&mut world).await;
    assert!(turn.model_change.is_none());
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_change_during_a_turn_does_not_cut_it_and_the_waiting_message_carries_it() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    let s1 = start.session_id;
    world
        .worker
        .send_events(s1, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    choose(
        &mut world,
        task_id,
        Some(selection("Claude Code", "sonnet", None)),
    )
    .await
    .unwrap();
    assert!(
        world
            .worker
            .try_next_command(Duration::from_millis(300))
            .await
            .is_none(),
        "a running turn is not cut"
    );
    // The harness cannot steer in this test: a message that waits for the
    // end of the turn starts the next turn, which carries the change.
    let mut inventory = inventory(world.worker.account_id);
    inventory.accounts[0].capabilities.steer = false;
    world.worker.send_custom_inventory(inventory).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    reply(&mut world, task_id, "Also the docs").await;
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
    let (_, turn) = next_turn(&mut world).await;
    assert_eq!(turn.turn.text, "Also the docs");
    assert_eq!(turn.model_change.unwrap().model, "sonnet");
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_harness_that_refuses_the_change_gets_a_new_session_with_the_context() {
    let mut world = World::start().await;
    let (task_id, start) = idle_task(&mut world).await;
    let s1 = start.session_id;
    choose(
        &mut world,
        task_id,
        Some(selection("Claude Code", "sonnet", None)),
    )
    .await
    .unwrap();
    reply(&mut world, task_id, "Now polish it").await;
    let (request, turn) = next_turn(&mut world).await;
    assert!(turn.model_change.is_some());
    world
        .worker
        .respond_error(
            &request,
            ErrorBody::new(
                ErrorCode::Unsupported,
                "This harness cannot change its model in a running session.",
            ),
        )
        .await;

    let (stopped, new_start) = switch_commands(&mut world).await;
    assert_eq!(stopped, s1);
    assert_ne!(new_start.session_id, s1);
    assert_eq!(new_start.model, "sonnet");
    assert_eq!(new_start.task_id, Some(task_id));
    let first = new_start.turn.expect("the new session starts with a turn");
    assert!(first.text.contains("<context>"), "{}", first.text);
    assert!(first.text.contains("Do it"), "the brief is in the context");
    assert!(first.text.contains("Done."), "the thread is in the context");
    assert!(first.text.ends_with("Now polish it"), "{}", first.text);
    assert_eq!(
        first.text.matches("Now polish it").count(),
        1,
        "the message is not in the context twice"
    );

    let task = world.task(task_id).await;
    assert_eq!(task.status, TaskStatus::Working);
    assert_eq!(task.current_session_id, Some(new_start.session_id));
    assert_eq!(task.worker_id, Some(world.worker.worker_id));
    // Events of the old session do not move the task.
    world
        .worker
        .send_events(s1, vec![turn_ended("t9", TurnEndReason::Completed)])
        .await;
    assert_eq!(world.task(task_id).await.status, TaskStatus::Working);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn another_account_on_the_same_worker_starts_a_new_session_at_once() {
    let mut world = World::start().await;
    let (task_id, start) = idle_task(&mut world).await;
    let (inventory, _second) = two_accounts(world.worker.account_id);
    world.worker.send_custom_inventory(inventory).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    choose(&mut world, task_id, Some(selection("Codex", "gpt-5", None)))
        .await
        .unwrap();
    reply(&mut world, task_id, "Continue on Codex").await;
    let (stopped, new_start) = switch_commands(&mut world).await;
    assert_eq!(stopped, start.session_id);
    assert_eq!(new_start.model, "gpt-5");
    assert_ne!(new_start.account_id, start.account_id);
    assert!(new_start.turn.unwrap().text.ends_with("Continue on Codex"));
    world.hub.handle.shutdown().await.unwrap();
}

/// Asks a decision with two options from the task session.
async fn ask(world: &mut World, session: SessionId, question: &str) -> DecisionId {
    let result = world
        .worker
        .tool_json(
            session,
            "ask_decision",
            json!({
                "question": question,
                "options": [
                    {"label": "Yes", "consequence": "Do it."},
                    {"label": "No", "consequence": "Skip it."}
                ]
            }),
        )
        .await;
    result["decision_id"].as_str().unwrap().parse().unwrap()
}

async fn answer_yes(world: &mut World, decision_id: DecisionId) {
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
}

#[tokio::test]
async fn a_task_in_needs_input_gets_its_new_session_and_loses_no_answer() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    let s1 = start.session_id;
    world
        .worker
        .send_events(s1, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    world.wait_task(task_id, TaskStatus::Working).await;
    let first = ask(&mut world, s1, "Use the cache?").await;
    let second = ask(&mut world, s1, "Run the migration?").await;
    world
        .worker
        .send_events(s1, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    world.wait_task(task_id, TaskStatus::NeedsInput).await;
    let (inventory, _second) = two_accounts(world.worker.account_id);
    world.worker.send_custom_inventory(inventory).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    choose(&mut world, task_id, Some(selection("Codex", "gpt-5", None)))
        .await
        .unwrap();

    // The first answer moves the task to a new session at once, although
    // the second decision is still open.
    answer_yes(&mut world, first).await;
    let (stopped, new_start) = switch_commands(&mut world).await;
    assert_eq!(stopped, s1);
    assert_eq!(new_start.model, "gpt-5");
    let turn = new_start.turn.expect("the new session starts with a turn");
    assert!(
        turn.text.contains("Yes"),
        "the answer is in the turn: {}",
        turn.text
    );
    let task = world.task(task_id).await;
    assert_eq!(task.status, TaskStatus::NeedsInput, "one decision is open");
    assert_eq!(task.current_session_id, Some(new_start.session_id));

    // The second answer reaches the new session. It is not lost.
    answer_yes(&mut world, second).await;
    let (_, command) = world.worker.next_command().await;
    let HubToWorker::SessionSteer(steer) = command else {
        panic!("expected session.steer, got {}", command.type_name());
    };
    assert_eq!(steer.session_id, new_start.session_id);
    assert!(steer.turn.text.contains("Yes"), "{}", steer.turn.text);
    world.wait_task(task_id, TaskStatus::Working).await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_harness_without_model_switch_gets_a_new_session() {
    let mut world = World::start().await;
    let (task_id, start) = idle_task(&mut world).await;
    let mut inventory = inventory(world.worker.account_id);
    inventory.accounts[0].capabilities.switch_model = false;
    world.worker.send_custom_inventory(inventory).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    choose(
        &mut world,
        task_id,
        Some(selection("Claude Code", "sonnet", None)),
    )
    .await
    .unwrap();
    reply(&mut world, task_id, "Go on").await;
    let (stopped, new_start) = switch_commands(&mut world).await;
    assert_eq!(stopped, start.session_id);
    assert_eq!(new_start.model, "sonnet");
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_worker_without_the_account_refuses_the_change_and_says_why() {
    let mut world = World::start().await;
    let (task_id, _start) = idle_task(&mut world).await;
    let before = world.task(task_id).await.model_selection;
    assert!(before.is_some(), "a started task keeps its model");
    let err = choose(&mut world, task_id, Some(selection("Codex", "gpt-5", None)))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("Codex"), "{}", err.message);
    assert!(
        err.message.contains("choose a model that this worker has"),
        "{}",
        err.message
    );
    assert_eq!(world.task(task_id).await.model_selection, before);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_resolved_or_archived_task_refuses_a_model_change() {
    let mut world = World::start().await;
    let (task_id, _start) = idle_task(&mut world).await;
    world.client.request(TaskResolve { task_id }).await.unwrap();
    let err = choose(
        &mut world,
        task_id,
        Some(selection("Claude Code", "sonnet", None)),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("resolved"), "{}", err.message);

    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_archived_task_refuses_a_model_change() {
    let mut world = World::start().await;
    let (task_id, _start) = idle_task(&mut world).await;
    world
        .client
        .request(TaskArchive {
            task_id,
            archived: true,
        })
        .await
        .unwrap();
    let err = choose(
        &mut world,
        task_id,
        Some(selection("Claude Code", "sonnet", None)),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("archived"), "{}", err.message);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_retry_starts_the_new_session_with_the_saved_model() {
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
                consort_proto::worker_link::SessionEvent::TurnEnded {
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
    choose(
        &mut world,
        task_id,
        Some(selection("Claude Code", "sonnet", Some("low"))),
    )
    .await
    .unwrap();
    world.client.request(TaskRetry { task_id }).await.unwrap();
    let (_, new_start) = switch_commands(&mut world).await;
    assert_eq!(new_start.model, "sonnet");
    assert_eq!(
        new_start.options.get("effort"),
        Some(&OptionValue::Text("low".to_owned()))
    );
    world.hub.handle.shutdown().await.unwrap();
}
