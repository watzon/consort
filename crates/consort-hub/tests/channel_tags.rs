//! Channel tags: all work from a channel with tags (coordinator turns and
//! tasks) runs only on workers that have every tag. Work waits, and does
//! not fail, while no online worker qualifies.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::*;
use consort_proto::client_api::{
    ChannelCreate, ChannelUpdate, MessageSend, RoutineCreate, RoutineRunNow, TaskCreate,
    WorkerList, WorkerSetTags,
};
use consort_proto::frame::{EventFrame, Frame, MessageFamily};
use consort_proto::ids::{ChannelId, TaskId, WorkerId};
use consort_proto::model::{
    AuthState, PlacementRequest, RoutineAction, RoutineSchedule, SessionRole, Task, WorkerState,
};
use consort_proto::status::TaskStatus;
use consort_proto::worker_link::{HubToWorker, SessionEvent, SessionStart, TurnEndReason};

const QUIET: Duration = Duration::from_millis(400);

fn tags(names: &[&str]) -> Vec<String> {
    names.iter().map(|t| (*t).to_owned()).collect()
}

/// Pairs a second worker and waits until the hub has its inventory.
async fn add_worker(world: &mut World) -> FakeWorker {
    let worker = FakeWorker::pair_and_connect(&world.hub).await;
    let id = worker.worker_id;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let list = world.client.request(WorkerList {}).await.unwrap();
        if list
            .workers
            .iter()
            .any(|w| w.id == id && w.inventory.is_some() && w.state == WorkerState::Online)
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the worker did not come online"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    worker
}

async fn set_tags(world: &mut World, worker_id: WorkerId, names: &[&str]) {
    world
        .client
        .request(WorkerSetTags {
            worker_id,
            tags: tags(names),
        })
        .await
        .unwrap();
}

async fn tagged_channel(world: &mut World, names: &[&str]) -> ChannelId {
    world
        .client
        .request(ChannelCreate {
            name: "Work".to_owned(),
            purpose: String::new(),
            instructions: String::new(),
            icon: None,
            color: None,
            group_id: None,
            coordinator_selection: None,
            task_selection: None,
            repo_ids: Vec::new(),
            default_isolation: None,
            default_approval_mode: None,
            memory_enabled: None,
            tags: tags(names),
        })
        .await
        .unwrap()
        .channel
        .id
}

async fn create_task(world: &mut World, channel_id: ChannelId) -> Task {
    world
        .client
        .request(TaskCreate {
            channel_id,
            title: "Build it".to_owned(),
            brief: "Build the thing.".to_owned(),
            model_selection: None,
            placement: PlacementRequest::default(),
            approval_mode: None,
            start_at: None,
        })
        .await
        .unwrap()
        .task
}

async fn task_in(world: &mut World, channel_id: ChannelId, id: TaskId) -> Task {
    world
        .client
        .snapshot(consort_proto::client_api::SnapshotScope::Channel(
            channel_id,
        ))
        .await
        .tasks
        .into_iter()
        .find(|t| t.id == id)
        .unwrap()
}

/// Takes the next `session.start` for a task, answers it, and returns it.
async fn accept_task_start(worker: &mut FakeWorker) -> SessionStart {
    loop {
        let (request, command) = worker.next_command().await;
        let HubToWorker::SessionStart(start) = command else {
            panic!("expected session.start, got {}", command.type_name());
        };
        worker
            .respond_ok(
                &request,
                &consort_proto::worker_link::SessionStartResult {
                    workspace_path: "/tmp/workspace".to_owned(),
                    native_ref: None,
                },
            )
            .await;
        if start.role == SessionRole::Task {
            return start;
        }
    }
}

async fn assert_silent(worker: &mut FakeWorker, who: &str) {
    if let Some((_, command)) = worker.try_next_command(QUIET).await {
        panic!("{who} got {} but must get nothing", command.type_name());
    }
}

#[tokio::test]
async fn a_task_in_a_tagged_channel_runs_only_on_the_tagged_worker() {
    let mut world = World::start().await;
    // The first worker is the better pick without a tag: it comes first.
    let mut tagged = add_worker(&mut world).await;
    set_tags(&mut world, tagged.worker_id, &["work"]).await;
    let channel = tagged_channel(&mut world, &["work"]).await;

    let task = create_task(&mut world, channel).await;
    let start = accept_task_start(&mut tagged).await;
    assert_eq!(start.task_id, Some(task.id));
    assert_silent(&mut world.worker, "the untagged worker").await;
    let task = task_in(&mut world, channel, task.id).await;
    assert_eq!(task.worker_id, Some(tagged.worker_id));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_untagged_channel_still_uses_the_first_worker() {
    let mut world = World::start().await;
    let mut tagged = add_worker(&mut world).await;
    set_tags(&mut world, tagged.worker_id, &["work"]).await;
    let channel = tagged_channel(&mut world, &[]).await;

    create_task(&mut world, channel).await;
    accept_task_start(&mut world.worker).await;
    assert_silent(&mut tagged, "the tagged worker").await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_task_waits_for_the_tagged_worker_and_starts_when_it_connects() {
    let mut world = World::start().await;
    let tagged = add_worker(&mut world).await;
    let tagged_id = tagged.worker_id;
    set_tags(&mut world, tagged_id, &["work"]).await;
    let away = tagged.go_away().await;
    world.wait_worker(tagged_id, WorkerState::Offline).await;
    let channel = tagged_channel(&mut world, &["work"]).await;

    let task = create_task(&mut world, channel).await;
    // The first worker is online and could run the task, but it lacks the
    // tag. The task waits, and it does not fail.
    assert_silent(&mut world.worker, "the untagged worker").await;
    let waiting = task_in(&mut world, channel, task.id).await;
    assert_eq!(waiting.status, TaskStatus::UpNext);
    assert_eq!(
        waiting.status_reason.as_deref(),
        Some("Waiting for an online worker with the tag work.")
    );

    let mut tagged = away.come_back(&world.hub).await;
    let start = accept_task_start(&mut tagged).await;
    assert_eq!(start.task_id, Some(task.id));
    assert_silent(&mut world.worker, "the untagged worker").await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_task_waits_with_a_hint_when_no_worker_has_the_tag() {
    let mut world = World::start().await;
    let mut other = add_worker(&mut world).await;
    let channel = tagged_channel(&mut world, &["work"]).await;

    let task = create_task(&mut world, channel).await;
    let waiting = task_in(&mut world, channel, task.id).await;
    assert_eq!(waiting.status, TaskStatus::UpNext);
    assert_eq!(
        waiting.status_reason.as_deref(),
        Some(
            "Waiting for an online worker with the tag work. \
             Add the tag to a worker in Settings > Workers."
        )
    );
    assert_silent(&mut world.worker, "the first worker").await;
    assert_silent(&mut other, "the second worker").await;

    // The user adds the tag to a worker, and the task starts there.
    set_tags(&mut world, other.worker_id, &["work"]).await;
    let start = accept_task_start(&mut other).await;
    assert_eq!(start.task_id, Some(task.id));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_tag_that_comes_from_the_inventory_counts() {
    let mut world = World::start().await;
    let channel = tagged_channel(&mut world, &["hub"]).await;
    // `hub` is a detected tag in the inventory of the test worker.
    create_task(&mut world, channel).await;
    accept_task_start(&mut world.worker).await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn removing_the_tags_of_a_channel_starts_the_waiting_task() {
    let mut world = World::start().await;
    let channel = tagged_channel(&mut world, &["gpu"]).await;
    let task = create_task(&mut world, channel).await;
    assert_silent(&mut world.worker, "the worker").await;

    world
        .client
        .request(ChannelUpdate {
            tags: Some(Vec::new()),
            ..channel_update(channel)
        })
        .await
        .unwrap();
    let start = accept_task_start(&mut world.worker).await;
    assert_eq!(start.task_id, Some(task.id));
    world.hub.handle.shutdown().await.unwrap();
}

fn channel_update(channel_id: ChannelId) -> ChannelUpdate {
    serde_json::from_value(serde_json::json!({ "channel_id": channel_id })).unwrap()
}

#[tokio::test]
async fn the_coordinator_of_a_tagged_channel_runs_on_the_tagged_worker() {
    let mut world = World::start().await;
    let mut tagged = add_worker(&mut world).await;
    set_tags(&mut world, tagged.worker_id, &["work"]).await;
    let channel = tagged_channel(&mut world, &["work"]).await;

    world
        .client
        .request(MessageSend {
            channel_id: channel,
            client_message_id: "cm-1".to_owned(),
            body: "Hello".to_owned(),
            attachments: Vec::new(),
            thread_root_id: None,
            in_timeline: false,
        })
        .await
        .unwrap();
    let (_, command) = tagged.next_command().await;
    let HubToWorker::SessionStart(start) = command else {
        panic!("expected session.start, got {}", command.type_name());
    };
    assert_eq!(start.role, SessionRole::Coordinator);
    assert_eq!(start.channel_id, channel);
    assert_silent(&mut world.worker, "the untagged worker").await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_replacement_coordinator_respects_the_tags_after_a_sign_out() {
    let mut world = World::start().await;
    // Everything is the channel that the World helpers use. Give it a tag
    // and put the tag on the first worker only.
    let first = world.worker.worker_id;
    set_tags(&mut world, first, &["work"]).await;
    world
        .client
        .request(ChannelUpdate {
            tags: Some(tags(&["work"])),
            ..channel_update(world.everything)
        })
        .await
        .unwrap();
    let mut other = add_worker(&mut world).await;

    let start = world.start_coordinator("hello").await;
    let session = start.session_id;
    assert_silent(&mut other, "the untagged worker").await;

    // The account of the first worker signs out.
    let mut signed_out = inventory(world.worker.account_id);
    for account in &mut signed_out.accounts {
        account.auth_state = AuthState::SignedOut;
    }
    world
        .worker
        .peer
        .send(&Frame::Event(EventFrame {
            kind: "inventory".to_owned(),
            body: serde_json::to_value(signed_out.with_hash()).unwrap(),
        }))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    world
        .worker
        .send_events(
            session,
            vec![SessionEvent::TurnEnded {
                turn_id: "t1".into(),
                reason: TurnEndReason::Failed,
                error: Some("Authentication required.".to_owned()),
            }],
        )
        .await;
    let (request, command) = world.worker.next_command().await;
    assert!(matches!(command, HubToWorker::SessionStop(_)));
    world
        .worker
        .respond_ok(&request, &serde_json::json!({}))
        .await;

    // The other worker is signed in, but it lacks the tag. The assistant
    // waits for the sign-in on the tagged worker.
    world
        .wait_notice(None, &["assistant_signed_out", "cannot answer"])
        .await;
    world.say(world.everything, "are you there?").await;
    assert_silent(&mut other, "the untagged worker").await;
    assert_silent(&mut world.worker, "the signed-out worker").await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn list_workers_shows_the_tags_of_each_worker() {
    let mut world = World::start().await;
    let id = world.worker.worker_id;
    set_tags(&mut world, id, &["work", "gpu"]).await;
    let coordinator = world.start_coordinator("Which machines?").await;
    let listed = world
        .worker
        .tool_json(
            coordinator.session_id,
            "list_workers",
            serde_json::json!({}),
        )
        .await;
    let worker = &listed["workers"][0];
    let shown: Vec<&str> = worker["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    // User tags and detected tags, sorted.
    assert_eq!(shown, ["gpu", "hub", "work"]);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_routine_run_in_a_tagged_channel_runs_only_on_the_tagged_worker() {
    let mut world = World::start().await;
    let mut tagged = add_worker(&mut world).await;
    set_tags(&mut world, tagged.worker_id, &["work"]).await;
    let channel = tagged_channel(&mut world, &["work"]).await;
    let routine = world
        .client
        .request(RoutineCreate {
            channel_id: channel,
            name: "Nightly".to_owned(),
            schedule: RoutineSchedule::Interval { seconds: 3600 },
            prompt: "Build it.".to_owned(),
            action: RoutineAction::StartTask {
                title: "Nightly build".to_owned(),
                model_selection: None,
                placement: PlacementRequest::default(),
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
    let start = accept_task_start(&mut tagged).await;
    assert_eq!(start.channel_id, channel);
    assert_silent(&mut world.worker, "the untagged worker").await;
    world.hub.handle.shutdown().await.unwrap();
}
