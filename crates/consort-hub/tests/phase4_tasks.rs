//! Phase 4: the task facts that the app needs for Retry, the list of
//! archived tasks with its restore, and the search by kind.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::client_api::*;
use consort_proto::ids::{ChannelId, SessionId, TaskId};
use consort_proto::model::{ItemStatus, PlacementRequest, SessionState};
use consort_proto::session_host_link::{HubToSessionHost, SessionStartResult, TurnEndReason};
use consort_proto::status::TaskStatus;
use consort_proto::time::Timestamp;

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

/// Answers commands until the `session.start` of a new session of the task.
async fn next_start(world: &mut World, task_id: TaskId) -> SessionId {
    loop {
        let (request, command) = world.worker.next_command().await;
        if let HubToSessionHost::SessionStart(start) = &command
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

/// Waits until the task's `session_ended` has this value.
async fn wait_session_ended(world: &mut World, task_id: TaskId, ended: bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if world.task(task_id).await.session_ended == ended {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "session_ended stayed {}",
            !ended
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_task_says_when_its_session_ended_and_a_retry_clears_it() {
    let mut world = World::start().await;
    let tutti = world.tutti;
    let (task_id, s) = task_in_review(&mut world, tutti).await;
    let task = world.wait_task(task_id, TaskStatus::Review).await;
    assert!(!task.session_ended);

    world
        .worker
        .send_events(s, vec![state(SessionState::Lost)])
        .await;
    wait_session_ended(&mut world, task_id, true).await;
    assert_eq!(world.task(task_id).await.status, TaskStatus::Review);

    world.client.request(TaskRetry { task_id }).await.unwrap();
    let new = next_start(&mut world, task_id).await;
    assert_ne!(new, s);
    let after = world.task(task_id).await;
    assert_eq!(after.status, TaskStatus::Working);
    assert!(!after.session_ended);
}

#[tokio::test]
async fn a_resolved_task_whose_session_ended_can_retry() {
    let mut world = World::start().await;
    let tutti = world.tutti;
    let (task_id, s) = task_in_review(&mut world, tutti).await;
    world.wait_task(task_id, TaskStatus::Review).await;
    world.client.request(TaskResolve { task_id }).await.unwrap();

    world
        .worker
        .send_events(s, vec![state(SessionState::Ended)])
        .await;
    wait_session_ended(&mut world, task_id, true).await;
    let task = world.task(task_id).await;
    assert_eq!(task.status, TaskStatus::Resolved);

    let retried = world
        .client
        .request(TaskRetry { task_id })
        .await
        .unwrap()
        .task;
    assert_eq!(retried.status, TaskStatus::Working);
    let new = next_start(&mut world, task_id).await;
    assert_ne!(new, s);
}

#[tokio::test]
async fn archived_tasks_list_newest_first_and_restore() {
    let mut world = World::start().await;
    let tutti = world.tutti;
    // Scheduled tasks need no worker.
    let later = Timestamp::from_millis(Timestamp::now().as_millis() + 86_400_000);
    let mut ids = Vec::new();
    for title in ["One", "Two", "Three"] {
        let task = world
            .client
            .request(TaskCreate {
                channel_id: tutti,
                title: title.to_owned(),
                brief: format!("{title} brief. {}", "x".repeat(2_000)),
                model_selection: None,
                placement: PlacementRequest::default(),
                approval_mode: None,
                start_at: Some(later),
            })
            .await
            .unwrap()
            .task;
        assert_eq!(task.status, TaskStatus::Scheduled);
        ids.push(task.id);
    }
    for id in [ids[0], ids[2]] {
        world
            .client
            .request(TaskArchive {
                task_id: id,
                archived: true,
            })
            .await
            .unwrap();
        // Archive times must differ, so the order is clear.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let all = world
        .client
        .request(TaskListArchived::default())
        .await
        .unwrap();
    let listed: Vec<TaskId> = all.tasks.iter().map(|t| t.id).collect();
    assert_eq!(listed, vec![ids[2], ids[0]]);
    assert!(!all.has_more);
    // The long briefs are previews, as in a snapshot.
    assert_eq!(all.tasks_cut.len(), 2);

    let first = world
        .client
        .request(TaskListArchived {
            limit: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(first.tasks.len(), 1);
    assert!(first.has_more);
    let second = world
        .client
        .request(TaskListArchived {
            offset: Some(1),
            limit: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(second.tasks[0].id, ids[0]);
    assert!(!second.has_more);

    let other = world
        .client
        .request(ChannelCreate {
            name: "Other".to_owned(),
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
    let none = world
        .client
        .request(TaskListArchived {
            channel_id: Some(other.id),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(none.tasks.is_empty());

    // Restore takes a task out of the list and back into the snapshot.
    world
        .client
        .request(TaskArchive {
            task_id: ids[2],
            archived: false,
        })
        .await
        .unwrap();
    let after = world
        .client
        .request(TaskListArchived::default())
        .await
        .unwrap();
    assert_eq!(
        after.tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![ids[0]]
    );
    assert_eq!(world.task(ids[2]).await.archived_at, None);
}
