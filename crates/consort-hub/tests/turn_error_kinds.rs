//! The `turn_failed` notice carries the kind of the failure, the reset
//! time, the account name and the raw text. A worker that sends no kind
//! gives a notice that reads as unknown.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::common::*;
use consort_proto::model::{SessionState, TurnErrorKind};
use consort_proto::session_host_link::{SessionEvent, TurnEndReason};
use consort_proto::status::TaskStatus;
use consort_proto::time::Timestamp;
use serde_json::Value;

fn failed(kind: Option<TurnErrorKind>, resets_at: Option<Timestamp>, error: &str) -> SessionEvent {
    SessionEvent::TurnEnded {
        turn_id: "t1".into(),
        reason: TurnEndReason::Failed,
        error: Some(error.to_owned()),
        error_kind: kind,
        resets_at,
    }
}

async fn failed_notice(world: &mut World, thread: Option<consort_proto::ids::MessageId>) -> Value {
    world.wait_notice(thread, &["turn_failed"]).await;
    let notices = world.notices(thread).await;
    let body = notices
        .iter()
        .find(|n| n.contains("turn_failed"))
        .expect("a turn_failed notice");
    serde_json::from_str(body).unwrap()
}

#[tokio::test]
async fn a_failed_coordinator_turn_names_its_kind_reset_time_account_and_raw_text() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Hello").await;
    let at = Timestamp::from_millis(1_900_000_000_000);
    world
        .worker
        .send_events(
            start.session_id,
            vec![failed(
                Some(TurnErrorKind::RateLimit),
                Some(at),
                "You hit your weekly limit.",
            )],
        )
        .await;
    let notice = failed_notice(&mut world, None).await;
    assert_eq!(notice["kind"], "turn_failed");
    assert_eq!(notice["error_kind"], "rate_limit");
    assert_eq!(notice["raw"], "You hit your weekly limit.");
    assert_eq!(notice["account"], "Claude Code");
    assert_eq!(
        serde_json::from_value::<Timestamp>(notice["resets_at"].clone()).unwrap(),
        at
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_turn_without_a_kind_has_no_kind_field() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Hello").await;
    world
        .worker
        .send_events(start.session_id, vec![failed(None, None, "boom")])
        .await;
    let notice = failed_notice(&mut world, None).await;
    assert_eq!(notice["text"], "boom");
    assert!(notice.get("error_kind").is_none());
    assert!(notice.get("resets_at").is_none());
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_task_turn_posts_the_notice_in_the_task_thread() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Do it").await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                failed(Some(TurnErrorKind::ExpiredSignIn), None, "Please log in."),
            ],
        )
        .await;
    world.wait_task(task_id, TaskStatus::Failed).await;
    let card = world.task(task_id).await.card_message_id;
    let notice = failed_notice(&mut world, Some(card)).await;
    assert_eq!(notice["error_kind"], "expired_sign_in");
    assert_eq!(notice["raw"], "Please log in.");
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_reopened_session_with_an_open_turn_reports_a_worker_restart() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Hello").await;
    world
        .worker
        .send_events(start.session_id, vec![SessionEvent::Reopened])
        .await;
    let notice = failed_notice(&mut world, None).await;
    assert_eq!(notice["error_kind"], "worker_restarted");
    world.hub.handle.shutdown().await.unwrap();
}
