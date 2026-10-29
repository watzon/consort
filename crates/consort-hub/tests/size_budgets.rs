//! Review cycle 2: snapshots with many long tasks, long review summaries,
//! and long threads and task item lists in hub tools.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use consort_proto::client_api::{MessageSend, SnapshotScope, SyncSnapshot, TaskCreate, TaskGet};
use consort_proto::ids::{ItemId, MessageId};
use consort_proto::model::{ItemStatus, SessionState};
use consort_proto::status::TaskStatus;
use consort_proto::time::Timestamp;
use consort_proto::tools::{FetchThreadResult, ReadTaskResult};
use consort_proto::worker_link::TurnEndReason;
use serde_json::json;

/// Finding 3: 34 tasks with 31 KiB briefs made both snapshots fail with
/// `internal`, because a snapshot held every brief in full.
#[tokio::test]
async fn many_long_briefs_fit_in_a_snapshot_as_previews_and_task_get_has_them_in_full() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let everything = client.everything().await;
    let brief = "b".repeat(31 * 1024);
    let mut ids = Vec::new();
    for i in 0..34 {
        let created = client
            .request(TaskCreate {
                channel_id: everything,
                title: format!("Task {i}"),
                brief: brief.clone(),
                model_selection: None,
                placement: Default::default(),
                approval_mode: None,
                start_at: Some(Timestamp::now().plus_millis(86_400_000)),
            })
            .await
            .unwrap();
        ids.push(created.task.id);
    }
    for scope in [SnapshotScope::All, SnapshotScope::Channel(everything)] {
        let snapshot = client.request(SyncSnapshot { scope }).await.unwrap();
        assert_eq!(snapshot.tasks.len(), 34);
        assert!(!snapshot.tasks_has_more);
        for task in &snapshot.tasks {
            assert!(snapshot.tasks_cut.contains(&task.id));
            assert!(task.brief.len() <= 1024, "{}", task.brief.len());
            assert!(task.brief.starts_with("bbbb"));
        }
    }
    let full = client.request(TaskGet { task_id: ids[0] }).await.unwrap();
    assert_eq!(full.task.brief, brief);
    hub.handle.shutdown().await.unwrap();
}

/// Finding 3: `submit_for_review` had no size limit.
#[tokio::test]
async fn a_long_review_summary_is_cut_and_the_snapshot_shows_a_preview() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    let sid = start.session_id;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    world
        .worker
        .tool_json(
            sid,
            "submit_for_review",
            json!({"summary": "s".repeat(40 * 1024)}),
        )
        .await;
    world
        .worker
        .send_events(sid, vec![turn_ended("t1", TurnEndReason::Completed)])
        .await;
    world.wait_task(task_id, TaskStatus::Review).await;
    let full = world
        .client
        .request(TaskGet { task_id })
        .await
        .unwrap()
        .task;
    let summary = full.review_summary.unwrap();
    assert!(summary.len() <= 16 * 1024, "{}", summary.len());
    assert!(
        summary.ends_with("because it is too long.]"),
        "{summary:.80}"
    );
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    let preview = snapshot.tasks.iter().find(|t| t.id == task_id).unwrap();
    assert!(snapshot.tasks_cut.contains(&task_id));
    assert!(preview.review_summary.as_ref().unwrap().len() <= 1024);
    world.hub.handle.shutdown().await.unwrap();
}

async fn fetch_thread(world: &mut World, args: serde_json::Value) -> FetchThreadResult {
    let session = world.coordinator_session.unwrap();
    let value = world.worker.tool_json(session, "fetch_thread", args).await;
    serde_json::from_value(value).unwrap()
}

/// Finding 8: five replies of 200 KB made `fetch_thread` fail at 2005 KiB,
/// so the pointer in a cut message led nowhere.
#[tokio::test]
async fn a_long_thread_comes_in_pages_and_the_named_reply_is_whole() {
    let mut world = World::start().await;
    world.start_coordinator("hi").await;
    let root = world.say(world.everything, "root").await;
    let mut replies: Vec<MessageId> = Vec::new();
    for i in 0..5 {
        let sent = world
            .client
            .request(MessageSend {
                channel_id: world.everything,
                client_message_id: format!("cm-big-{i}"),
                body: format!("{i}{}", "x".repeat(200 * 1024)),
                attachments: Vec::new(),
                thread_root_id: Some(root.id),
                in_timeline: false,
            })
            .await
            .unwrap();
        replies.push(sent.message.id);
    }

    // Page back through the thread.
    let mut seen = Vec::new();
    let mut before = None;
    loop {
        let page = fetch_thread(&mut world, json!({"message_id": root.id, "before": before})).await;
        assert_eq!(page.root.id, root.id);
        assert!(!page.replies.is_empty());
        for reply in page.replies.iter().rev() {
            assert_eq!(reply.body.len(), 200 * 1024 + 1, "a reply was cut");
            seen.push(reply.id);
        }
        if !page.has_more {
            break;
        }
        before = Some(page.replies[0].id);
        assert!(seen.len() < 10, "the pages do not end");
    }
    seen.reverse();
    assert_eq!(seen, replies);

    // The reply that a cut turn names comes whole, even with no room left.
    let page = fetch_thread(&mut world, json!({"message_id": replies[2]})).await;
    let named = page.message.expect("the named reply");
    assert_eq!(named.id, replies[2]);
    assert_eq!(named.body.len(), 200 * 1024 + 1);
    assert!(page.has_more);
    // A reply that does not fit any more is cut and says how to read it.
    if let Some(cut) = page.replies.last()
        && cut.body.len() < 200 * 1024
    {
        assert!(cut.body.contains("Call fetch_thread with message_id"));
    }
    world.hub.handle.shutdown().await.unwrap();
}

/// Finding 8: `read_task` had no byte budget either.
#[tokio::test]
async fn read_task_pages_long_items() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    let sid = start.session_id;
    world
        .worker
        .send_events(sid, vec![state(SessionState::Active), turn_started("t1")])
        .await;
    let mut items: Vec<ItemId> = Vec::new();
    for i in 0..5 {
        let event = assistant_text(
            sid,
            &format!("{i}{}", "y".repeat(150 * 1024)),
            ItemStatus::Completed,
        );
        if let consort_proto::worker_link::SessionEvent::Item(item) = &event {
            items.push(item.id);
        }
        world.worker.send_events(sid, vec![event]).await;
    }
    let coordinator = world.coordinator_session.unwrap();
    let mut seen = Vec::new();
    let mut before: Option<ItemId> = None;
    loop {
        let value = world
            .worker
            .tool_json(
                coordinator,
                "read_task",
                json!({"task_id": task_id, "before_item_id": before}),
            )
            .await;
        let page: ReadTaskResult = serde_json::from_value(value).unwrap();
        assert_eq!(page.task.id, task_id);
        assert!(!page.items.is_empty());
        seen.extend(page.items.iter().rev().map(|i| i.id));
        if !page.has_more {
            break;
        }
        before = Some(page.items[0].id);
        assert!(seen.len() < 10, "the pages do not end");
    }
    seen.reverse();
    assert_eq!(seen, items);
    world.hub.handle.shutdown().await.unwrap();
}
