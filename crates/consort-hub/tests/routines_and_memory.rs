//! Phase 2: memory notes in Recently deleted, renames, and routines that
//! move to another channel.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::common::*;
use consort_proto::client_api::{
    ChannelCreate, EntityPayload, MemoryDelete, MemoryHistory, MemoryList, MemoryMove, MemoryRead,
    MemoryRestore, MemoryWrite, RoutineCreate, RoutineUpdate,
};
use consort_proto::error::ErrorCode;
use consort_proto::model::{ExpectedHash, MemoryScope, RoutineAction, RoutineSchedule};

fn write(path: &str, content: &str, expected: ExpectedHash) -> MemoryWrite {
    MemoryWrite {
        scope: MemoryScope::Global,
        path: path.to_owned(),
        content: content.to_owned(),
        expected_sha256: expected,
    }
}

fn list(deleted: bool) -> MemoryList {
    MemoryList {
        scope: Some(MemoryScope::Global),
        deleted,
    }
}

#[tokio::test]
async fn a_deleted_note_goes_to_recently_deleted_and_comes_back() {
    let mut world = World::start().await;
    let note = world
        .client
        .request(write("people/kevin.md", "Kevin.", ExpectedHash::NotExists))
        .await
        .unwrap()
        .note;

    world
        .client
        .request(MemoryDelete {
            scope: MemoryScope::Global,
            path: note.path.clone(),
            expected_sha256: note.sha256.clone(),
        })
        .await
        .unwrap();

    // The feed carries the note with its deletion time.
    let entry = world
        .client
        .wait_feed(
            |e| matches!(upserted(e), Some(EntityPayload::MemoryNote(n)) if n.deleted_at.is_some()),
        )
        .await;
    assert_eq!(entry.entity_id, "global:people/kevin.md");
    // The note is gone from the list and from reads, and is in Recently
    // deleted.
    assert!(
        world
            .client
            .request(list(false))
            .await
            .unwrap()
            .notes
            .is_empty()
    );
    let deleted = world.client.request(list(true)).await.unwrap().notes;
    assert_eq!(deleted.len(), 1);
    assert!(deleted[0].deleted_at.is_some());
    let error = world
        .client
        .request(MemoryRead {
            scope: MemoryScope::Global,
            path: note.path.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);

    let restored = world
        .client
        .request(MemoryRestore {
            scope: MemoryScope::Global,
            path: note.path.clone(),
        })
        .await
        .unwrap()
        .note;
    assert_eq!(restored.deleted_at, None);
    assert_eq!(restored.version, 1);
    assert_eq!(restored.content, "Kevin.");
    assert_eq!(
        world.client.request(list(false)).await.unwrap().notes.len(),
        1
    );
    assert!(
        world
            .client
            .request(list(true))
            .await
            .unwrap()
            .notes
            .is_empty()
    );

    // A note that is not deleted cannot be restored.
    let error = world
        .client
        .request(MemoryRestore {
            scope: MemoryScope::Global,
            path: note.path.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_write_to_the_path_of_a_deleted_note_starts_it_again_with_the_next_version() {
    let mut world = World::start().await;
    let note = world
        .client
        .request(write("a.md", "One.", ExpectedHash::NotExists))
        .await
        .unwrap()
        .note;
    world
        .client
        .request(MemoryDelete {
            scope: MemoryScope::Global,
            path: "a.md".to_owned(),
            expected_sha256: note.sha256,
        })
        .await
        .unwrap();

    let again = world
        .client
        .request(write("a.md", "Two.", ExpectedHash::NotExists))
        .await
        .unwrap()
        .note;

    assert_eq!(again.version, 2);
    assert_eq!(again.deleted_at, None);
    assert!(
        world
            .client
            .request(list(true))
            .await
            .unwrap()
            .notes
            .is_empty()
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_rename_moves_the_note_and_its_versions() {
    let mut world = World::start().await;
    let first = world
        .client
        .request(write("kevin.md", "Kevin.", ExpectedHash::NotExists))
        .await
        .unwrap()
        .note;
    let second = world
        .client
        .request(write(
            "kevin.md",
            "Kevin likes tea.",
            ExpectedHash::Sha256(first.sha256.clone()),
        ))
        .await
        .unwrap()
        .note;
    world
        .client
        .request(write("taken.md", "Taken.", ExpectedHash::NotExists))
        .await
        .unwrap();

    // A rename with an old hash is stale. A rename onto a note is refused.
    let error = world
        .client
        .request(MemoryMove {
            scope: MemoryScope::Global,
            from_path: "kevin.md".to_owned(),
            to_path: "people/kevin.md".to_owned(),
            expected_sha256: first.sha256.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::StaleWrite);
    let error = world
        .client
        .request(MemoryMove {
            scope: MemoryScope::Global,
            from_path: "kevin.md".to_owned(),
            to_path: "taken.md".to_owned(),
            expected_sha256: second.sha256.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    let error = world
        .client
        .request(MemoryMove {
            scope: MemoryScope::Global,
            from_path: "kevin.md".to_owned(),
            to_path: "people/kevin.txt".to_owned(),
            expected_sha256: second.sha256.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Invalid);

    let moved = world
        .client
        .request(MemoryMove {
            scope: MemoryScope::Global,
            from_path: "kevin.md".to_owned(),
            to_path: "people/kevin.md".to_owned(),
            expected_sha256: second.sha256.clone(),
        })
        .await
        .unwrap()
        .note;
    assert_eq!(moved.path, "people/kevin.md");
    assert_eq!(moved.version, 2);
    let history = world
        .client
        .request(MemoryHistory {
            scope: MemoryScope::Global,
            path: "people/kevin.md".to_owned(),
        })
        .await
        .unwrap()
        .versions;
    assert_eq!(history.len(), 2);
    let paths: Vec<String> = world
        .client
        .request(list(false))
        .await
        .unwrap()
        .notes
        .into_iter()
        .map(|n| n.path)
        .collect();
    assert_eq!(paths, vec!["people/kevin.md", "taken.md"]);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_client_moves_a_routine_to_another_channel() {
    let mut world = World::start().await;
    let other = world
        .client
        .request(ChannelCreate {
            name: "house-lights".to_owned(),
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
    let routine = world
        .client
        .request(RoutineCreate {
            channel_id: world.tutti,
            name: "Sunset check".to_owned(),
            schedule: RoutineSchedule::Interval { seconds: 3_600 },
            prompt: "Check the sunset time.".to_owned(),
            action: RoutineAction::CoordinatorTurn,
            enabled: true,
        })
        .await
        .unwrap()
        .routine;

    let moved = world
        .client
        .request(RoutineUpdate {
            routine_id: routine.id,
            channel_id: Some(other.id),
            name: None,
            schedule: None,
            prompt: None,
            action: None,
            enabled: None,
        })
        .await
        .unwrap()
        .routine;

    assert_eq!(moved.channel_id, other.id);
    // The schedule did not change, so the next run stays.
    assert_eq!(moved.next_run_at, routine.next_run_at);
    world.hub.handle.shutdown().await.unwrap();
}
