//! Channel settings that reach the coordinator, routines through the API,
//! and item detail from a worker.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::common::*;
use consort_proto::client_api::{
    ChannelCreate, ChannelUpdate, EntityPayload, MessageSend, RoutineCreate, RoutineDelete,
    RoutineRunNow, RoutineRuns, SnapshotScope, TaskItemDetail,
};
use consort_proto::error::ErrorCode;
use consort_proto::ids::ItemId;
use consort_proto::model::{
    RoutineAction, RoutineRunStatus, RoutineSchedule, SessionState, Sha256Hash,
};
use consort_proto::worker_link::{HubToWorker, ItemDetailResult, TurnEndReason};
use serde_json::json;

fn new_channel(name: &str, purpose: &str, instructions: &str) -> ChannelCreate {
    ChannelCreate {
        name: name.to_owned(),
        purpose: purpose.to_owned(),
        instructions: instructions.to_owned(),
        icon: None,
        color: None,
        group_id: None,
        coordinator_selection: None,
        task_selection: None,
        repo_ids: Vec::new(),
        default_isolation: None,
        default_approval_mode: None,
        memory_enabled: None,
        tags: Vec::new(),
    }
}

#[tokio::test]
async fn the_purpose_and_instructions_of_a_channel_reach_its_coordinator() {
    let mut world = World::start().await;
    let channel = world
        .client
        .request(new_channel(
            "Sayso",
            "Ship the Sayso app.",
            "Never push to main. Open a pull request.",
        ))
        .await
        .unwrap()
        .channel;
    world
        .client
        .request(MessageSend {
            channel_id: channel.id,
            client_message_id: "s1".to_owned(),
            body: "Where are we?".to_owned(),
            attachments: Vec::new(),
            thread_root_id: None,
            in_timeline: false,
        })
        .await
        .unwrap();
    let start = world.worker.accept_session_start().await;
    assert_eq!(start.channel_id, channel.id);
    let prompt = &start.system_prompt_append;
    assert!(prompt.contains("Purpose: Ship the Sayso app."), "{prompt}");
    assert!(prompt.contains("Never push to main. Open a pull request."));
    assert!(prompt.contains(&format!("Channel memory (scope `{}`)", channel.id)));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn changing_the_instructions_replaces_the_coordinator_session() {
    let mut world = World::start().await;
    let first = world.start_coordinator("Hello").await;
    world
        .worker
        .tool_json(first.session_id, "post_message", json!({"body": "Hi."}))
        .await;
    world
        .worker
        .send_events(
            first.session_id,
            vec![turn_ended("t1", TurnEndReason::Completed)],
        )
        .await;

    let mut update = ChannelUpdate::new(world.tutti);
    update.instructions = Some("Always answer in French.".to_owned());
    world.client.request(update).await.unwrap();
    world.say(world.tutti, "Bonjour").await;

    // The old session stops, and a new one starts with the new instructions.
    let mut stopped = None;
    let mut started = None;
    for _ in 0..2 {
        let (request, command) = world.worker.next_command().await;
        match command {
            HubToWorker::SessionStop(stop) => {
                stopped = Some(stop.session_id);
                world
                    .worker
                    .respond_ok(&request, &consort_proto::frame::Empty {})
                    .await;
            }
            HubToWorker::SessionStart(start) => {
                started = Some(start);
                world
                    .worker
                    .respond_ok(
                        &request,
                        &consort_proto::worker_link::SessionStartResult {
                            workspace_path: "/tmp/ws".to_owned(),
                            native_ref: None,
                        },
                    )
                    .await;
            }
            other => panic!(
                "unexpected command {}",
                consort_proto::frame::MessageFamily::type_name(&other)
            ),
        }
    }
    assert_eq!(stopped, Some(first.session_id));
    let started = started.unwrap();
    assert_ne!(started.session_id, first.session_id);
    assert!(
        started
            .system_prompt_append
            .contains("Always answer in French.")
    );
    assert!(started.turn.unwrap().text.contains("Bonjour"));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_routine_runs_now_and_wakes_the_coordinator() {
    let mut world = World::start().await;
    let routine = world
        .client
        .request(RoutineCreate {
            channel_id: world.tutti,
            name: "Morning brief".to_owned(),
            schedule: RoutineSchedule::Interval { seconds: 3600 },
            prompt: "Summarize my open tasks.".to_owned(),
            action: RoutineAction::CoordinatorTurn,
            enabled: true,
        })
        .await
        .unwrap()
        .routine;
    assert!(routine.next_run_at.is_some());
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    // The hub's own check-in and morning brief are there too.
    assert_eq!(
        snapshot
            .routines
            .iter()
            .filter(|r| r.kind == consort_proto::model::RoutineKind::Custom)
            .count(),
        1
    );

    world
        .client
        .request(RoutineRunNow {
            routine_id: routine.id,
        })
        .await
        .unwrap();
    let start = world.worker.accept_session_start().await;
    let text = start.turn.clone().unwrap().text;
    assert!(
        text.contains(&format!("<routine id=\"{}\"", routine.id)),
        "{text}"
    );
    assert!(text.contains("Summarize my open tasks."));

    let runs = world
        .client
        .request(RoutineRuns {
            routine_id: routine.id,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(runs.runs.len(), 1);
    assert!(runs.runs[0].error.is_none());
    assert_eq!(runs.runs[0].status, RoutineRunStatus::Running);
    assert!(runs.runs[0].manual);
    let updated = world
        .client
        .wait_feed(
            |e| matches!(upserted(e), Some(EntityPayload::Routine(r)) if r.last_run_at.is_some()),
        )
        .await;
    let Some(EntityPayload::Routine(updated)) = upserted(&updated) else {
        unreachable!()
    };
    assert_eq!(updated.last_run_status, Some(RoutineRunStatus::Running));

    // The coordinator answers in the timeline and ends its turn. The run
    // ends well and links to the answer.
    let session_id = start.session_id;
    world
        .worker
        .send_events(
            session_id,
            vec![state(SessionState::Active), turn_started("t1")],
        )
        .await;
    let posted = world
        .worker
        .tool_json(
            session_id,
            "post_message",
            json!({"body": "Two tasks are open."}),
        )
        .await;
    world
        .worker
        .send_events(session_id, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    let ended = world
        .client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::RoutineRun(r)) if r.status == RoutineRunStatus::Ok)
        })
        .await;
    let Some(EntityPayload::RoutineRun(ended)) = upserted(&ended) else {
        unreachable!()
    };
    assert_eq!(
        ended.message_id.map(|m| m.to_string()),
        posted["message_id"].as_str().map(str::to_owned)
    );
    assert!(ended.ended_at.is_some());

    // An interval below one minute is refused.
    let error = world
        .client
        .request(RoutineCreate {
            channel_id: world.tutti,
            name: "Too fast".to_owned(),
            schedule: RoutineSchedule::Interval { seconds: 5 },
            prompt: "x".to_owned(),
            action: RoutineAction::CoordinatorTurn,
            enabled: true,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Invalid);
    world
        .client
        .request(RoutineDelete {
            routine_id: routine.id,
        })
        .await
        .unwrap();
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_start_task_routine_creates_a_task() {
    let mut world = World::start().await;
    let routine = world
        .client
        .request(RoutineCreate {
            channel_id: world.tutti,
            name: "Nightly check".to_owned(),
            schedule: RoutineSchedule::Interval { seconds: 3600 },
            prompt: "Run the nightly checks and report.".to_owned(),
            action: RoutineAction::StartTask {
                title: "Nightly check".to_owned(),
                model_selection: None,
                placement: Default::default(),
            },
            enabled: true,
        })
        .await
        .unwrap()
        .routine;
    world
        .client
        .request(RoutineRunNow {
            routine_id: routine.id,
        })
        .await
        .unwrap();
    let start = world.worker.accept_session_start().await;
    assert_eq!(start.role, consort_proto::model::SessionRole::Task);
    assert!(
        start
            .turn
            .unwrap()
            .text
            .contains("Run the nightly checks and report.")
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn item_detail_asks_the_worker_and_fails_clearly_when_it_is_away() {
    let mut world = World::start().await;
    let coordinator = world.start_coordinator("Start").await;
    let started = world
        .worker
        .tool_json(
            coordinator.session_id,
            "start_task",
            json!({"title": "T", "brief": "B"}),
        )
        .await;
    let task_id: consort_proto::ids::TaskId = started["task_id"].as_str().unwrap().parse().unwrap();
    let task = world.worker.accept_session_start().await;
    world
        .worker
        .send_events(
            task.session_id,
            vec![state(SessionState::Active), turn_started("t1")],
        )
        .await;
    let item_id = ItemId::new();
    let detail = Sha256Hash::from_bytes([7; 32]);

    let (result, ()) = tokio::join!(
        world.client.request(TaskItemDetail {
            session_id: task.session_id,
            item_id
        }),
        async {
            let (request, command) = world.worker.next_command().await;
            let HubToWorker::ItemDetail(asked) = command else {
                panic!("expected item.detail");
            };
            assert_eq!(asked.item_id, item_id);
            world
                .worker
                .respond_ok(
                    &request,
                    &ItemDetailResult {
                        sha256: detail.clone(),
                        size: 12,
                        mime: "text/plain".to_owned(),
                    },
                )
                .await;
        }
    );
    assert_eq!(result.unwrap().size, 12);

    let worker = std::mem::replace(&mut world.worker, placeholder_worker(&world.hub).await);
    let id = worker.worker_id;
    worker.peer.close().await;
    world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Worker(w)) if w.id == id && w.state == consort_proto::model::WorkerState::Offline))
        .await;
    let error = world
        .client
        .request(TaskItemDetail {
            session_id: task.session_id,
            item_id,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::WorkerOffline);
    let _ = task_id;
    world.hub.handle.shutdown().await.unwrap();
}
