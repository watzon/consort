//! Connector calls in a proactive turn and in a draft task: every call that
//! writes or sends asks the user each time, whatever the connection allows
//! and whatever the user allowed before. Read calls run as usual.

use std::time::Duration;

use consort_proto::model::{
    ApprovalRule, ProactiveKind, ProactiveOutcome, ProactiveWake, ToolApprovals,
};
use consort_proto::turn_input::WakeEvent;
use serde_json::json;

use super::Timings;
use super::flow_tests::{
    active_gmail, call, coordinator_session, fast, hub_with_fake, tutti, wait_for,
};
use crate::hub::{Hub, test_task_session};
use crate::store::coordinator::CoordinatorRow;
use crate::store::work;

/// Opens a check-in turn in a coordinator session.
async fn open_check_in(hub: &Hub, session: consort_proto::ids::SessionId) {
    let channel_id = tutti(hub).await;
    hub.store
        .write(move |tx| {
            let mut wake = ProactiveWake {
                id: 0,
                channel_id,
                kind: ProactiveKind::CheckIn,
                at: tx.now(),
                outcome: ProactiveOutcome::Pending,
                detail: None,
                message_ids: Vec::new(),
                task_id: None,
                routine_id: None,
                notify_held: false,
                ended_at: None,
            };
            tx.insert_wake(&mut wake)?;
            tx.save_coordinator(&CoordinatorRow {
                channel_id,
                session_id: Some(session),
                turn_open: true,
                turn_started_at: Some(tx.now()),
                turn_events: vec![WakeEvent::CheckIn {
                    routine: consort_proto::ids::RoutineId::new(),
                    summary: String::new(),
                    note: String::new(),
                    recent_findings: Vec::new(),
                    log: wake.id,
                }],
            })
        })
        .await
        .unwrap();
}

async fn pending_cards(hub: &Hub) -> usize {
    hub.store
        .read(|conn| work::pending_approvals(conn, None))
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn a_proactive_turn_asks_before_each_send_even_when_the_connection_runs_without_asking() {
    let (hub, _dir, fake) = hub_with_fake().await;
    hub.connectors.set_timings(Timings {
        approval_wait: Duration::from_millis(50),
        ..fast()
    });
    active_gmail(
        &hub,
        &fake,
        ToolApprovals {
            change: ApprovalRule::RunWithoutAsking,
            ..ToolApprovals::default()
        },
    )
    .await;
    let session = coordinator_session(&hub, tutti(&hub).await, "acp").await;
    open_check_in(&hub, session).await;
    let sent = call(
        &hub,
        session,
        "run_connector_tool",
        json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL", "arguments": {"to": "a@example.com"}}),
    )
    .await;
    assert_eq!(sent["status"], "waiting_for_user");
    assert_eq!(pending_cards(&hub).await, 1);
    assert!(fake.executions().is_empty(), "no mail went out");
    // A read tool still runs at once.
    let read = call(
        &hub,
        session,
        "run_connector_tool",
        json!({"connector": "gmail_work", "tool": "GMAIL_FETCH_EMAILS", "arguments": {}}),
    )
    .await;
    assert_eq!(read["status"], "ran");
}

#[tokio::test]
async fn a_proactive_turn_ignores_an_earlier_allow_of_its_session() {
    let (hub, _dir, fake) = hub_with_fake().await;
    hub.connectors.set_timings(Timings {
        approval_wait: Duration::from_millis(50),
        ..fast()
    });
    active_gmail(
        &hub,
        &fake,
        ToolApprovals {
            change: ApprovalRule::AskOncePerTask,
            ..ToolApprovals::default()
        },
    )
    .await;
    let session = coordinator_session(&hub, tutti(&hub).await, "acp").await;
    // The user allowed one send in a normal turn of this session.
    let key = session.to_string();
    hub.store
        .write(move |tx| {
            let connection = crate::store::connectors::connections(tx.db())?.remove(0);
            tx.save_grant(&key, connection.id, consort_proto::model::ToolKind::Change)
        })
        .await
        .unwrap();
    let normal = call(
        &hub,
        session,
        "run_connector_tool",
        json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL", "arguments": {}}),
    )
    .await;
    assert_eq!(normal["status"], "ran", "a normal turn uses the allow");
    // A check-in in the same session asks again.
    open_check_in(&hub, session).await;
    let proactive = call(
        &hub,
        session,
        "run_connector_tool",
        json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL", "arguments": {}}),
    )
    .await;
    assert_eq!(proactive["status"], "waiting_for_user");
    assert_eq!(fake.executions().len(), 1);
}

#[tokio::test]
async fn a_draft_task_asks_before_each_send() {
    let (hub, _dir, fake) = hub_with_fake().await;
    hub.connectors.set_timings(Timings {
        approval_wait: Duration::from_millis(50),
        ..fast()
    });
    active_gmail(
        &hub,
        &fake,
        ToolApprovals {
            change: ApprovalRule::RunWithoutAsking,
            ..ToolApprovals::default()
        },
    )
    .await;
    let (task, session) = test_task_session(&hub).await;
    let draft = task.clone();
    hub.store
        .write(move |tx| tx.save_proactive_task(&draft, ProactiveKind::CheckIn, true))
        .await
        .unwrap();
    let sent = call(
        &hub,
        session.id,
        "run_connector_tool",
        json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL", "arguments": {}}),
    )
    .await;
    assert_eq!(sent["status"], "waiting_for_user");
    assert!(fake.executions().is_empty());
}

/// Whether the coordinator session has an allow of change tools for Gmail.
async fn has_session_grant(hub: &Hub, session: consort_proto::ids::SessionId) -> bool {
    let key = session.to_string();
    hub.store
        .read(move |conn| {
            let connection = crate::store::connectors::connections(conn)?.remove(0);
            crate::store::connectors::has_grant(
                conn,
                &key,
                connection.id,
                consort_proto::model::ToolKind::Change,
            )
        })
        .await
        .unwrap()
}

/// Allows the one pending card, after its call stopped waiting, and waits
/// until the call ran.
async fn allow_late(hub: &Hub, fake: &crate::connectors::fake::FakeComposio, ran: usize) {
    let card = hub
        .store
        .read(|conn| work::pending_approvals(conn, None))
        .await
        .unwrap()
        .remove(0);
    hub.approval_answer(
        &crate::hub::Caller::Hub,
        consort_proto::client_api::ApprovalAnswer {
            approval_id: card.id,
            option_id: "allow".to_owned(),
            checked: false,
            checked_hash: None,
        },
    )
    .await
    .unwrap();
    wait_for(async || (fake.executions().len() == ran).then_some(())).await;
}

#[tokio::test]
async fn a_late_answer_saves_an_allow_only_when_the_card_asked_once_per_task() {
    let (hub, _dir, fake) = hub_with_fake().await;
    hub.connectors.set_timings(Timings {
        approval_wait: Duration::from_millis(50),
        ..fast()
    });
    active_gmail(
        &hub,
        &fake,
        ToolApprovals {
            change: ApprovalRule::AskOncePerTask,
            ..ToolApprovals::default()
        },
    )
    .await;
    // A check-in drafts a reply. The user allows it after the call stopped
    // waiting: the card asked every time, so no allow is saved.
    let session = coordinator_session(&hub, tutti(&hub).await, "acp").await;
    open_check_in(&hub, session).await;
    let sent = call(
        &hub,
        session,
        "run_connector_tool",
        json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL", "arguments": {"to": "a@example.com"}}),
    )
    .await;
    assert_eq!(sent["status"], "waiting_for_user");
    let card = hub
        .store
        .read(|conn| work::pending_approvals(conn, None))
        .await
        .unwrap()
        .remove(0);
    let consort_proto::model::ApprovalDetails::Connector(details) = &card.details else {
        unreachable!()
    };
    assert_eq!(details.ask_rule, Some(ApprovalRule::AskEveryTime));
    allow_late(&hub, &fake, 1).await;
    assert!(!has_session_grant(&hub, session).await);

    // In a normal turn, the same late answer saves the allow, as the rule
    // says.
    let normal = coordinator_session(&hub, tutti(&hub).await, "acp").await;
    let sent = call(
        &hub,
        normal,
        "run_connector_tool",
        json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL", "arguments": {"to": "a@example.com"}}),
    )
    .await;
    assert_eq!(sent["status"], "waiting_for_user");
    allow_late(&hub, &fake, 2).await;
    assert!(has_session_grant(&hub, normal).await);
}
