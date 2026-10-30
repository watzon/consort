//! A task from start to resolve: approvals, decisions, failures, retries
//! and messages in the task thread.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::common::*;
use consort_proto::client_api::{
    ApprovalAnswer, DecisionAnswerRequest, EntityPayload, MessageSend, TaskInterrupt, TaskItems,
    TaskReopen, TaskResolve, TaskRetry,
};
use consort_proto::error::ErrorCode;
use consort_proto::frame::MessageFamily;
use consort_proto::ids::TaskId;
use consort_proto::model::{
    ApprovalStatus, DecisionAnswer, DecisionStatus, ItemStatus, MessageKind, SessionRole,
    SessionState,
};
use consort_proto::status::TaskStatus;
use consort_proto::worker_link::{HubToWorker, SessionEvent, SessionStart, TurnEndReason};
use serde_json::json;

struct Started {
    coordinator: SessionStart,
    task_id: TaskId,
    task_session: SessionStart,
}

/// The coordinator starts a task, and the worker accepts the task session.
async fn start_task(world: &mut World, brief: &str) -> Started {
    let coordinator = world.start_coordinator("Please do this").await;
    let started = world
        .worker
        .tool_json(
            coordinator.session_id,
            "start_task",
            json!({"title": "Fix the bug", "brief": brief}),
        )
        .await;
    assert_eq!(started["status"], "working", "{started}");
    let task_id: TaskId = started["task_id"].as_str().unwrap().parse().unwrap();
    let task_session = world.worker.accept_session_start().await;
    assert_eq!(task_session.role, SessionRole::Task);
    assert_eq!(task_session.task_id, Some(task_id));
    Started {
        coordinator,
        task_id,
        task_session,
    }
}

async fn wait_task_status(
    world: &mut World,
    id: TaskId,
    status: TaskStatus,
) -> consort_proto::model::Task {
    let entry = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Task(t)) if t.id == id && t.status == status))
        .await;
    let Some(EntityPayload::Task(task)) = upserted(&entry) else {
        unreachable!()
    };
    task.clone()
}

#[tokio::test]
async fn a_task_runs_with_an_approval_and_ends_resolved() {
    let mut world = World::start().await;
    let Started {
        coordinator,
        task_id,
        task_session,
    } = start_task(&mut world, "Fix the bug in foo.rs").await;

    // The task session got its brief and its own prompt.
    let first = task_session.turn.as_ref().unwrap().text.clone();
    assert!(first.contains("Fix the bug in foo.rs"), "{first}");
    assert!(
        task_session
            .system_prompt_append
            .contains("submit_for_review")
    );
    assert!(
        !task_session.system_prompt_append.contains("post_message"),
        "a task does not speak like a coordinator"
    );

    // The client sees the task and its card in the channel.
    let task = wait_task_status(&mut world, task_id, TaskStatus::Working).await;
    assert_eq!(task.channel_id, world.tutti);
    let card = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.id == task.card_message_id))
        .await;
    let Some(EntityPayload::Message(card)) = upserted(&card) else {
        unreachable!()
    };
    assert_eq!(card.kind, MessageKind::TaskCard);

    let sid = task_session.session_id;
    world
        .worker
        .send_events(
            sid,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                assistant_text(sid, "I will run the tests.", ItemStatus::Completed),
                approval_request("req-1", "cargo test"),
            ],
        )
        .await;

    // An approval opens: the task needs input, and the card is in the thread.
    let approval_entry = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Approval(a)) if a.request_id.as_str() == "req-1"))
        .await;
    let Some(EntityPayload::Approval(approval)) = upserted(&approval_entry) else {
        unreachable!()
    };
    assert_eq!(approval.status, ApprovalStatus::Pending);
    assert_eq!(approval.task_id, Some(task_id));
    let needs_input = wait_task_status(&mut world, task_id, TaskStatus::NeedsInput).await;
    assert!(
        needs_input
            .status_reason
            .as_deref()
            .unwrap()
            .contains("cargo test")
    );
    let thread_card = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.id == approval.message_id))
        .await;
    let Some(EntityPayload::Message(thread_card)) = upserted(&thread_card) else {
        unreachable!()
    };
    assert_eq!(thread_card.kind, MessageKind::Approval);
    assert_eq!(thread_card.thread_root_id, Some(task.card_message_id));

    // The user allows it. The worker gets the answer with the harness request ID.
    let answered = world
        .client
        .request(ApprovalAnswer {
            approval_id: approval.id,
            option_id: "allow".to_owned(),
            checked: false,
            checked_hash: None,
        })
        .await
        .unwrap()
        .approval;
    assert_eq!(answered.status, ApprovalStatus::Allowed);
    let (request, command) = world.worker.next_command().await;
    let HubToWorker::SessionRespond(respond) = command else {
        panic!("expected session.respond");
    };
    assert_eq!(respond.session_id, sid);
    assert_eq!(respond.request_id.as_str(), "req-1");
    assert_eq!(respond.answer.option_id.as_deref(), Some("allow"));
    world
        .worker
        .respond_ok(&request, &consort_proto::frame::Empty {})
        .await;
    wait_task_status(&mut world, task_id, TaskStatus::Working).await;

    // A second answer is refused.
    let error = world
        .client
        .request(ApprovalAnswer {
            approval_id: approval.id,
            option_id: "deny".to_owned(),
            checked: false,
            checked_hash: None,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);

    // The task finishes with a summary.
    world
        .worker
        .send_events(
            sid,
            vec![SessionEvent::RequestClosed {
                request_id: "req-1".into(),
                outcome: consort_proto::worker_link::RequestOutcome::Allowed,
            }],
        )
        .await;
    world
        .worker
        .tool_json(
            sid,
            "submit_for_review",
            json!({"summary": "Fixed foo.rs. Tests pass."}),
        )
        .await;
    world
        .worker
        .send_events(sid, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    let review = wait_task_status(&mut world, task_id, TaskStatus::Review).await;
    assert_eq!(
        review.review_summary.as_deref(),
        Some("Fixed foo.rs. Tests pass.")
    );

    // The coordinator hears about it in its next turn. First its own turn ends.
    world
        .worker
        .send_events(
            coordinator.session_id,
            vec![turn_ended("t1", TurnEndReason::Completed)],
        )
        .await;
    let (_, command) = world.worker.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!("expected session.turn for the coordinator");
    };
    assert_eq!(turn.session_id, coordinator.session_id);
    assert!(
        turn.turn
            .text
            .contains(&format!("<task_update task=\"{task_id}\"")),
        "{}",
        turn.turn.text
    );
    assert!(turn.turn.text.contains("status=\"review\""));
    assert!(turn.turn.text.contains("Fixed foo.rs. Tests pass."));

    // Only the user resolves.
    let resolved = world
        .client
        .request(TaskResolve { task_id })
        .await
        .unwrap()
        .task;
    assert_eq!(resolved.status, TaskStatus::Resolved);
    assert!(resolved.resolved_at.is_some());
    // Not done moves it back to review.
    let reopened = world
        .client
        .request(TaskReopen { task_id })
        .await
        .unwrap()
        .task;
    assert_eq!(reopened.status, TaskStatus::Review);
    assert!(reopened.resolved_at.is_none());

    // The activity view has the session items.
    let items = world
        .client
        .request(TaskItems {
            task_id,
            session_id: None,
            after_item_id: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(!items.items.is_empty());
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_user_cannot_resolve_a_task_that_is_not_in_review() {
    let mut world = World::start().await;
    let Started { task_id, .. } = start_task(&mut world, "Do it").await;
    let error = world
        .client
        .request(TaskResolve { task_id })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    let error = world
        .client
        .request(TaskRetry { task_id })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_denied_approval_sends_the_deny_option_back() {
    let mut world = World::start().await;
    let Started { task_session, .. } = start_task(&mut world, "Do it").await;
    let sid = task_session.session_id;
    world
        .worker
        .send_events(
            sid,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                approval_request("req-9", "rm -rf /"),
            ],
        )
        .await;
    let entry = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Approval(_))))
        .await;
    let Some(EntityPayload::Approval(approval)) = upserted(&entry) else {
        unreachable!()
    };
    let denied = world
        .client
        .request(ApprovalAnswer {
            approval_id: approval.id,
            option_id: "deny".to_owned(),
            checked: false,
            checked_hash: None,
        })
        .await
        .unwrap()
        .approval;
    assert_eq!(denied.status, ApprovalStatus::Denied);
    let (_, command) = world.worker.next_command().await;
    let HubToWorker::SessionRespond(respond) = command else {
        panic!("expected session.respond");
    };
    assert_eq!(respond.answer.option_id.as_deref(), Some("deny"));
    // An option that the harness did not offer is refused.
    let error = world
        .client
        .request(ApprovalAnswer {
            approval_id: approval.id,
            option_id: "other".to_owned(),
            checked: false,
            checked_hash: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        error.code,
        ErrorCode::Invalid | ErrorCode::Conflict
    ));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_harness_question_becomes_a_decision_that_answers_through_respond() {
    let mut world = World::start().await;
    let Started {
        task_id,
        task_session,
        ..
    } = start_task(&mut world, "Ask me").await;
    let sid = task_session.session_id;
    world
        .worker
        .send_events(
            sid,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                question_request("q-1", "Use the new API?"),
            ],
        )
        .await;
    let entry = world
        .client
        .wait_feed(
            |e| matches!(upserted(e), Some(EntityPayload::Decision(d)) if d.request_id.is_some()),
        )
        .await;
    let Some(EntityPayload::Decision(decision)) = upserted(&entry) else {
        unreachable!()
    };
    assert_eq!(decision.options.len(), 2);
    assert_eq!(decision.options[0].id.as_deref(), Some("yes"));
    wait_task_status(&mut world, task_id, TaskStatus::NeedsInput).await;

    let answered = world
        .client
        .request(DecisionAnswerRequest {
            decision_id: decision.id,
            answer: DecisionAnswer {
                option_index: Some(0),
                text: None,
            },
        })
        .await
        .unwrap()
        .decision;
    assert_eq!(answered.status, DecisionStatus::Answered);
    let (_, command) = world.worker.next_command().await;
    let HubToWorker::SessionRespond(respond) = command else {
        panic!("expected session.respond, not a new turn");
    };
    assert_eq!(respond.request_id.as_str(), "q-1");
    assert_eq!(respond.answer.option_id.as_deref(), Some("yes"));
    wait_task_status(&mut world, task_id, TaskStatus::Working).await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_decision_from_a_task_keeps_it_in_needs_input_and_the_answer_is_the_next_turn() {
    let mut world = World::start().await;
    let Started {
        task_id,
        task_session,
        ..
    } = start_task(&mut world, "Choose").await;
    let sid = task_session.session_id;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let result = world
        .worker
        .tool_json(
            sid,
            "ask_decision",
            json!({
                "question": "Which database?",
                "options": [
                    {"label": "SQLite", "consequence": "One file. No server."},
                    {"label": "Postgres", "consequence": "A server to run."}
                ],
                "recommended_index": 0
            }),
        )
        .await;
    let decision_id = result["decision_id"].as_str().unwrap().to_owned();
    // The turn ends with the decision open: the task waits for the user.
    world
        .worker
        .send_events(sid, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    let entry = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Decision(d)) if d.id.to_string() == decision_id))
        .await;
    let Some(EntityPayload::Decision(decision)) = upserted(&entry) else {
        unreachable!()
    };
    assert!(decision.request_id.is_none());
    assert_eq!(decision.status, DecisionStatus::Pending);
    let task = wait_task_status(&mut world, task_id, TaskStatus::NeedsInput).await;
    assert!(task.review_summary.is_none(), "it is not in review");

    world
        .client
        .request(DecisionAnswerRequest {
            decision_id: decision.id,
            answer: DecisionAnswer {
                option_index: Some(1),
                text: None,
            },
        })
        .await
        .unwrap();
    let (_, command) = world.worker.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!("expected session.turn with the answer");
    };
    assert_eq!(turn.session_id, sid);
    assert!(
        turn.turn
            .text
            .contains(&format!("<decision_answered id=\"{decision_id}\"")),
        "{}",
        turn.turn.text
    );
    assert!(turn.turn.text.contains("option=\"Postgres\""));
    wait_task_status(&mut world, task_id, TaskStatus::Working).await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_turn_fails_the_task_and_retry_starts_a_new_session() {
    let mut world = World::start().await;
    let Started {
        task_id,
        task_session,
        ..
    } = start_task(&mut world, "Break").await;
    let sid = task_session.session_id;
    world
        .worker
        .send_events(
            sid,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                SessionEvent::TurnEnded {
                    turn_id: "t1".into(),
                    reason: TurnEndReason::Failed,
                    error: Some("The model refused.".to_owned()),
                },
            ],
        )
        .await;
    let failed = wait_task_status(&mut world, task_id, TaskStatus::Failed).await;
    assert_eq!(failed.status_reason.as_deref(), Some("The model refused."));
    // The user cannot resolve a failed task. They retry it.
    let error = world
        .client
        .request(TaskResolve { task_id })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    let retried = world
        .client
        .request(TaskRetry { task_id })
        .await
        .unwrap()
        .task;
    assert!(matches!(
        retried.status,
        TaskStatus::UpNext | TaskStatus::Working
    ));
    // A failed turn does not end the session, so the retry stops the old
    // session and starts a new one. The two commands have their own queues,
    // so they can come in either order.
    let mut stopped = false;
    let mut second = None;
    while !stopped || second.is_none() {
        let (request, command) = world.worker.next_command().await;
        match command {
            HubToWorker::SessionStop(stop) => {
                assert_eq!(stop.session_id, sid);
                world.worker.respond_ok(&request, &json!({})).await;
                stopped = true;
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
                second = Some(start);
            }
            other => panic!("unexpected command {}", other.type_name()),
        }
    }
    let second = second.unwrap();
    assert_eq!(second.task_id, Some(task_id));
    assert_ne!(second.session_id, sid);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_lost_task_session_fails_the_task() {
    let mut world = World::start().await;
    let Started {
        task_id,
        task_session,
        ..
    } = start_task(&mut world, "Vanish").await;
    world
        .worker
        .send_events(
            task_session.session_id,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                state(SessionState::Lost),
            ],
        )
        .await;
    let failed = wait_task_status(&mut world, task_id, TaskStatus::Failed).await;
    assert!(failed.status_reason.as_deref().unwrap().contains("lost"));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_message_in_the_task_thread_reaches_the_session() {
    let mut world = World::start().await;
    let Started {
        task_id,
        task_session,
        ..
    } = start_task(&mut world, "Talk").await;
    let sid = task_session.session_id;
    world
        .worker
        .send_events(
            sid,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    let review = wait_task_status(&mut world, task_id, TaskStatus::Review).await;

    // A user reply in the thread goes to the task, not to the coordinator.
    world
        .client
        .request(MessageSend {
            channel_id: world.tutti,
            client_message_id: "thread-1".to_owned(),
            body: "Also update the docs.".to_owned(),
            attachments: Vec::new(),
            thread_root_id: Some(review.card_message_id),
            in_timeline: false,
        })
        .await
        .unwrap();
    let (_, command) = world.worker.next_command().await;
    let HubToWorker::SessionTurn(turn) = command else {
        panic!("expected session.turn");
    };
    assert_eq!(turn.session_id, sid);
    assert_eq!(turn.turn.text, "Also update the docs.");
    wait_task_status(&mut world, task_id, TaskStatus::Working).await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_coordinator_can_message_a_running_task_and_steer_it() {
    let mut world = World::start().await;
    let Started {
        coordinator,
        task_id,
        task_session,
    } = start_task(&mut world, "Long work").await;
    let sid = task_session.session_id;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let result = world
        .worker
        .tool_json(
            coordinator.session_id,
            "message_task",
            json!({"task_id": task_id.to_string(), "body": "Use tabs."}),
        )
        .await;
    assert_eq!(result["delivery"], "steer");
    let (_, command) = world.worker.next_command().await;
    let HubToWorker::SessionSteer(steer) = command else {
        panic!("expected session.steer");
    };
    assert_eq!(steer.session_id, sid);
    assert_eq!(steer.turn.text, "Use tabs.");

    // list_tasks and read_task work for the coordinator.
    let list = world
        .worker
        .tool_json(coordinator.session_id, "list_tasks", json!({}))
        .await;
    assert_eq!(list["tasks"].as_array().unwrap().len(), 1);
    let read = world
        .worker
        .tool_json(
            coordinator.session_id,
            "read_task",
            json!({"task_id": task_id.to_string()}),
        )
        .await;
    assert_eq!(read["task"]["title"], "Fix the bug");
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn interrupting_a_task_asks_the_worker_and_ends_in_review() {
    let mut world = World::start().await;
    let Started {
        task_id,
        task_session,
        ..
    } = start_task(&mut world, "Spin").await;
    let sid = task_session.session_id;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let (result, ()) = tokio::join!(world.client.request(TaskInterrupt { task_id }), async {
        let (request, command) = world.worker.next_command().await;
        let HubToWorker::SessionInterrupt(interrupt) = command else {
            panic!("expected session.interrupt");
        };
        assert_eq!(interrupt.session_id, sid);
        world
            .worker
            .respond_ok(&request, &consort_proto::frame::Empty {})
            .await;
    });
    result.unwrap();
    world
        .worker
        .send_events(sid, vec![turn_ended("t1", TurnEndReason::Interrupted)])
        .await;
    let task = wait_task_status(&mut world, task_id, TaskStatus::Review).await;
    assert_eq!(
        task.status_reason.as_deref(),
        Some("Stopped before it finished.")
    );
    world.hub.handle.shutdown().await.unwrap();
}
