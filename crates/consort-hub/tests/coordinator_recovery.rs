//! Review cycle 2: a coordinator on a signed-out account, and a coordinator
//! start that keeps failing.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{EventFrame, Frame, MessageFamily};
use consort_proto::ids::AccountId;
use consort_proto::model::{AuthState, CoordinatorBlockedKind, SessionState};
use consort_proto::session_host_link::{HubToSessionHost, SessionEvent, TurnEndReason};

fn signed_out_events() -> Vec<SessionEvent> {
    vec![
        SessionEvent::Error {
            code: "signed_out".to_owned(),
            message: "Authentication required. Run `claude` and /login.".to_owned(),
        },
        SessionEvent::TurnEnded {
            turn_id: "t1".into(),
            reason: TurnEndReason::Failed,
            error: Some("Authentication required.".to_owned()),
            error_kind: None,
            resets_at: None,
        },
    ]
}

async fn send_inventory(worker: &mut FakeWorker, inventory: consort_proto::model::Inventory) {
    worker
        .peer
        .send(&Frame::Event(EventFrame {
            kind: "inventory".to_owned(),
            body: serde_json::to_value(inventory.with_hash()).unwrap(),
        }))
        .await;
}

/// Finding 6: after a `signed_out` error and a failed turn, the next message
/// still went to the same session on the signed-out account. With no other
/// account, the events wait, the notice says where to sign in, and a sign-in
/// starts a new session with the waiting events.
#[tokio::test]
async fn a_signed_out_coordinator_waits_for_a_sign_in_and_says_where() {
    let mut world = World::start().await;
    let start = world.start_coordinator("hello").await;
    let s1 = start.session_id;
    world.worker.send_events(s1, signed_out_events()).await;
    // The hub ends the session on the worker.
    let (request, command) = world.worker.next_command().await;
    let HubToSessionHost::SessionStop(stop) = command else {
        panic!("expected session.stop, got {}", command.type_name());
    };
    assert_eq!(stop.session_id, s1);
    world
        .worker
        .respond_ok(&request, &serde_json::json!({}))
        .await;
    let blocked = world.wait_blocked(&["signed out"]).await;
    assert_eq!(blocked.kind, CoordinatorBlockedKind::SignedOut);
    assert!(
        blocked
            .account
            .as_deref()
            .is_some_and(|a| a.starts_with("Claude Code")),
        "{blocked:?}"
    );
    // The hub posts no message for it.
    assert!(world.notices(None).await.is_empty());

    world.say(world.tutti, "are you there?").await;
    if let Some((_, command)) = world.worker.try_next_command(Duration::from_secs(1)).await {
        panic!(
            "the hub sent {} although no account is signed in",
            command.type_name()
        );
    }

    // The user signs in again. The worker reports it.
    let inventory = inventory(world.worker.account_id);
    send_inventory(&mut world.worker, inventory).await;
    let (_, command) = world.worker.next_command().await;
    let HubToSessionHost::SessionStart(again) = command else {
        panic!("expected session.start, got {}", command.type_name());
    };
    assert_ne!(again.session_id, s1);
    let text = again.turn.unwrap().text;
    assert!(text.contains("hello"), "{text}");
    assert!(text.contains("are you there?"), "{text}");
    // The sign-in works, so the state goes away by itself when the turn
    // goes out.
    assert!(world.blocked().await.is_none());
    world.hub.handle.shutdown().await.unwrap();
}

/// A probe can say "signed in" while the login does not work, so the hub tries
/// again and fails again. The channel gets the live state again for each
/// failure, and the hub posts no message at all.
#[tokio::test]
async fn a_login_that_keeps_failing_posts_no_message() {
    let mut world = World::start().await;
    let mut session = world.start_coordinator("hello").await.session_id;
    for round in 0..3 {
        world.worker.send_events(session, signed_out_events()).await;
        let (request, command) = world.worker.next_command().await;
        let HubToSessionHost::SessionStop(stop) = command else {
            panic!("expected session.stop, got {}", command.type_name());
        };
        assert_eq!(stop.session_id, session);
        world
            .worker
            .respond_ok(&request, &serde_json::json!({}))
            .await;
        world.wait_blocked(&["Claude Code"]).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let notices = world.notices(None).await;
        assert!(notices.is_empty(), "round {round}: {notices:?}");

        // The next probe says "signed in" again.
        let inventory = inventory(world.worker.account_id);
        send_inventory(&mut world.worker, inventory).await;
        session = world.worker.accept_session_start().await.session_id;
        world
            .worker
            .send_events(
                session,
                vec![state(SessionState::Active), turn_started("t1")],
            )
            .await;
    }
    world.hub.handle.shutdown().await.unwrap();
}

/// Finding 6: with a second signed-in account, the next session goes there.
/// The failed turn alone is enough when the inventory says signed out.
#[tokio::test]
async fn a_signed_out_coordinator_moves_to_another_signed_in_account() {
    let mut world = World::start().await;
    let mut inventory = inventory(world.worker.account_id);
    let mut second = inventory.accounts[0].clone();
    second.account_id = AccountId::new();
    inventory.accounts.push(second);
    send_inventory(&mut world.worker, inventory.clone()).await;
    let start = world.start_coordinator("hello").await;
    let s1 = start.session_id;
    let first_account = start.account_id;

    // The worker saw the sign-out in its inventory, and the turn fails.
    for account in &mut inventory.accounts {
        if account.account_id == first_account {
            account.auth_state = AuthState::SignedOut;
        }
    }
    send_inventory(&mut world.worker, inventory).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    world
        .worker
        .send_events(
            s1,
            vec![SessionEvent::TurnEnded {
                turn_id: "t1".into(),
                reason: TurnEndReason::Failed,
                error: Some("Authentication required.".to_owned()),
                error_kind: None,
                resets_at: None,
            }],
        )
        .await;
    let mut stopped = false;
    let mut moved = None;
    while !stopped || moved.is_none() {
        let (request, command) = world.worker.next_command().await;
        match command {
            HubToSessionHost::SessionStop(stop) => {
                assert_eq!(stop.session_id, s1);
                world
                    .worker
                    .respond_ok(&request, &serde_json::json!({}))
                    .await;
                stopped = true;
            }
            HubToSessionHost::SessionStart(start) => {
                world
                    .worker
                    .respond_ok(
                        &request,
                        &consort_proto::session_host_link::SessionStartResult {
                            workspace_path: "/tmp/w".to_owned(),
                            native_ref: None,
                        },
                    )
                    .await;
                moved = Some(start);
            }
            other => panic!("unexpected command {}", other.type_name()),
        }
    }
    let moved = moved.unwrap();
    assert_ne!(moved.account_id, first_account);
    assert!(moved.turn.unwrap().text.contains("hello"));
    // The assistant moved, so nothing is blocked and nothing is posted.
    assert!(world.blocked().await.is_none());
    assert!(world.notices(None).await.is_empty());
    world.hub.handle.shutdown().await.unwrap();
}

/// Finding 7: the worker's `start_failed` and `lost` events came before the
/// command error, and each time they started the coordinator again: three
/// `session.start` commands in a row, with a notice each.
#[tokio::test]
async fn a_coordinator_start_that_fails_is_not_tried_again_at_once() {
    let mut world = World::start().await;
    world.say(world.tutti, "hello").await;
    let (request, command) = world.worker.next_command().await;
    let HubToSessionHost::SessionStart(start) = command else {
        panic!("expected session.start, got {}", command.type_name());
    };
    world
        .worker
        .send_events(
            start.session_id,
            vec![
                state(SessionState::Starting),
                SessionEvent::Error {
                    code: "start_failed".to_owned(),
                    message: "could not start the harness".to_owned(),
                },
                state(SessionState::Lost),
            ],
        )
        .await;
    world
        .worker
        .respond_error(
            &request,
            ErrorBody::new(ErrorCode::Internal, "could not start the harness"),
        )
        .await;
    let blocked = world.wait_blocked(&["could not start"]).await;
    assert_eq!(blocked.kind, CoordinatorBlockedKind::StartFailed);
    if let Some((_, command)) = world.worker.try_next_command(Duration::from_secs(2)).await {
        panic!("the hub sent {} after a failed start", command.type_name());
    }
    // A new message in the backoff waits too: the first wait is 10 s.
    world.say(world.tutti, "again").await;
    if let Some((_, command)) = world.worker.try_next_command(Duration::from_secs(2)).await {
        panic!("the hub sent {} in the backoff", command.type_name());
    }
    let notices = world.notices(None).await;
    assert!(notices.is_empty(), "{notices:?}");
    world.hub.handle.shutdown().await.unwrap();
}
