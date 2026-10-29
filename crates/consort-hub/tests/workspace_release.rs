//! Review 3, findings 4 and 5: deleting a channel stopped its sessions but
//! released no workspace, and a task in Ready for review or Resolved whose
//! session had ended could not go on and could not retry.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::*;
use consort_proto::client_api::*;
use consort_proto::error::ErrorCode;
use consort_proto::frame::MessageFamily;
use consort_proto::ids::{ChannelId, SessionId, TaskId};
use consort_proto::model::{ItemStatus, PlacementRequest, SessionState};
use consort_proto::status::TaskStatus;
use consort_proto::worker_link::{HubToWorker, SessionStartResult, TurnEndReason};

async fn task_in_review(world: &mut World, channel_id: ChannelId) -> (TaskId, SessionId) {
    let task = world
        .client
        .request(TaskCreate {
            channel_id,
            title: "Fix it".to_owned(),
            brief: "Fix the bug.".to_owned(),
            model_selection: None,
            placement: PlacementRequest::default(),
            approval_mode: None,
            start_at: None,
        })
        .await
        .unwrap()
        .task;
    let start = world.worker.accept_session_start().await;
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
    (task.id, s)
}

/// Answers every command that comes within `wait` of the one before, and
/// returns them. A `session.start` gets a start result.
async fn drain(world: &mut World, wait: Duration) -> Vec<HubToWorker> {
    let mut commands = Vec::new();
    while let Some((request, command)) = world.worker.try_next_command(wait).await {
        if matches!(command, HubToWorker::SessionStart(_)) {
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
        commands.push(command);
    }
    commands
}

#[tokio::test]
async fn deleting_a_channel_releases_the_workspaces_of_its_tasks() {
    let mut world = World::start().await;
    let channel = world
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
            tags: Vec::new(),
        })
        .await
        .unwrap()
        .channel;
    let (task_id, s) = task_in_review(&mut world, channel.id).await;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let snapshot = world
            .client
            .snapshot(SnapshotScope::Channel(channel.id))
            .await;
        if snapshot
            .tasks
            .iter()
            .any(|t| t.id == task_id && t.status == TaskStatus::Review)
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no review");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let _ = drain(&mut world, Duration::from_millis(500)).await;
    world
        .client
        .request(ChannelDelete {
            channel_id: channel.id,
        })
        .await
        .unwrap();
    let commands = drain(&mut world, Duration::from_secs(2)).await;
    let stop = commands
        .iter()
        .position(|c| matches!(c, HubToWorker::SessionStop(stop) if stop.session_id == s));
    let release = commands.iter().position(
        |c| matches!(c, HubToWorker::WorkspaceRelease(r) if r.task_id == task_id && r.delete),
    );
    let names: Vec<&str> = commands.iter().map(|c| c.type_name()).collect();
    assert!(stop.is_some(), "no session.stop: {names:?}");
    assert!(release.is_some(), "no workspace.release: {names:?}");
    assert!(
        stop < release,
        "the release must come after the stop: {names:?}"
    );
}

#[tokio::test]
async fn not_done_after_archive_is_refused_and_retry_starts_again() {
    let mut world = World::start().await;
    let everything = world.everything;
    let (task_id, old) = task_in_review(&mut world, everything).await;
    world.wait_task(task_id, TaskStatus::Review).await;
    world.client.request(TaskResolve { task_id }).await.unwrap();
    world
        .client
        .request(TaskArchive {
            task_id,
            archived: true,
        })
        .await
        .unwrap();
    let commands = drain(&mut world, Duration::from_secs(2)).await;
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, HubToWorker::WorkspaceRelease(r) if r.task_id == task_id)),
        "the archive released nothing"
    );
    world
        .client
        .request(TaskArchive {
            task_id,
            archived: false,
        })
        .await
        .unwrap();

    // Not done needs the deleted workspace, so the hub refuses it and
    // names the way on.
    let refused = world
        .client
        .request(TaskReopen { task_id })
        .await
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(refused.message.contains("Retry"), "{}", refused.message);
    assert_eq!(world.task(task_id).await.status, TaskStatus::Resolved);

    // Retry starts the task again with a new session.
    let retried = world
        .client
        .request(TaskRetry { task_id })
        .await
        .unwrap()
        .task;
    assert_eq!(retried.status, TaskStatus::Working);
    let start = world.worker.accept_session_start().await;
    assert_eq!(start.task_id, Some(task_id));
    assert_ne!(start.session_id, old);
}

#[tokio::test]
async fn a_task_in_review_whose_session_ended_can_retry() {
    let mut world = World::start().await;
    let everything = world.everything;
    let (task_id, s) = task_in_review(&mut world, everything).await;
    let task = world.wait_task(task_id, TaskStatus::Review).await;
    // A retry while the session runs would end a good session.
    let refused = world
        .client
        .request(TaskRetry { task_id })
        .await
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.message.contains("still runs"),
        "{}",
        refused.message
    );

    // The worker lost the session. The task stays Ready for review and
    // says why it cannot go on.
    world
        .worker
        .send_events(s, vec![state(SessionState::Lost)])
        .await;
    let deadline = tokio::time::Instant::now() + WAIT;
    let task_now = loop {
        let now = world.task(task_id).await;
        if now.status_reason.is_some() {
            break now;
        }
        assert!(tokio::time::Instant::now() < deadline, "no status reason");
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(task_now.status, TaskStatus::Review);
    assert!(
        task_now.status_reason.as_deref().unwrap().contains("Retry"),
        "{:?}",
        task_now.status_reason
    );

    // A message gets a notice that names the way on.
    world
        .client
        .request(MessageSend {
            channel_id: everything,
            client_message_id: "cm-more".to_owned(),
            body: "One more thing.".to_owned(),
            attachments: Vec::new(),
            thread_root_id: Some(task.card_message_id),
            in_timeline: false,
        })
        .await
        .unwrap();
    world
        .wait_notice(
            Some(task.card_message_id),
            &["message_not_sent", "Retry the task"],
        )
        .await;

    // Retry starts a new session.
    world.client.request(TaskRetry { task_id }).await.unwrap();
    let new = drain_until_start(&mut world, task_id).await;
    assert_ne!(new, s);
    let task_after = world.task(task_id).await;
    assert_eq!(task_after.status, TaskStatus::Working);
    assert_eq!(task_after.status_reason, None);
}

/// Answers commands until the `session.start` of a new session of the task.
async fn drain_until_start(world: &mut World, task_id: TaskId) -> SessionId {
    loop {
        let (request, command) = world.worker.next_command().await;
        if let HubToWorker::SessionStart(start) = &command
            && start.task_id == Some(task_id)
        {
            world
                .worker
                .respond_ok(
                    &request,
                    &SessionStartResult {
                        workspace_path: "/tmp/w2".to_owned(),
                        native_ref: None,
                    },
                )
                .await;
            return start.session_id;
        }
        world
            .worker
            .respond_ok(&request, &serde_json::json!({}))
            .await;
    }
}
