//! Connectors and skills over the real links: a client sets the Composio
//! key and connects a service, a coordinator session gets the connector in
//! its extensions and runs a tool through a hub card that the client
//! answers, and a worker reads the skill bundle of its session.
//!
//! Composio is a fake local server (`src/connectors/fake.rs`). No test
//! talks to the real Composio.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../src/connectors/fake.rs"]
pub(crate) mod fake;

use crate::common::*;
use consort_proto::client_api::{
    ApprovalAnswer, ComposioGetStatus, ComposioSetKey, ConnectionConnect, EntityPayload,
    SnapshotScope,
};
use consort_proto::error::ErrorCode;
use consort_proto::frame::MessageFamily;
use consort_proto::model::{
    ApprovalDetails, ConnectionStatus, SecretEnv, ToolApprovals, UseIn, skill_content_hash,
};
use consort_proto::worker_link::SkillBundleGet;
use fake::{FakeComposio, GOOD_KEY};
use serde_json::json;

async fn world_with_composio() -> (World, FakeComposio) {
    let world = World::start().await;
    let fake = FakeComposio::start().await;
    world.hub.handle.set_composio_url(&fake.url);
    (world, fake)
}

#[tokio::test]
async fn a_connector_call_waits_for_the_card_that_the_client_answers() {
    let (mut world, fake) = world_with_composio().await;
    fake.set_auto_activate(true);
    let status = world
        .client
        .request(ComposioSetKey {
            api_key: GOOD_KEY.to_owned(),
        })
        .await
        .unwrap()
        .status;
    assert!(status.key_present);
    let again = world.client.request(ComposioGetStatus {}).await.unwrap();
    assert_eq!(again.status.key_hint, status.key_hint);
    let attempt = world
        .client
        .request(ConnectionConnect {
            service: "gmail".to_owned(),
            name: "Gmail Work".to_owned(),
            use_in: UseIn::EveryChannel,
            approvals: ToolApprovals::default(),
            auth_scheme: None,
            credentials: SecretEnv::new(),
            approval_id: None,
        })
        .await
        .unwrap();
    assert!(attempt.redirect_url.is_some());
    let id = attempt.connection.id;
    // The hub follows the sign-in. Each status change is a feed upsert.
    world
        .client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Connection(c)) if c.id == id && c.status == ConnectionStatus::Active)
        })
        .await;
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    assert_eq!(snapshot.connections.len(), 1);
    assert_eq!(
        snapshot.connections[0].identity.as_deref(),
        Some("gmail_work@example.com")
    );

    // A new coordinator session gets the connector in its extensions.
    let start = world.start_coordinator("Email the report to Ana.").await;
    let session_id = start.session_id;
    assert_eq!(start.extensions.connectors.len(), 1);
    assert_eq!(start.extensions.connectors[0].handle, "gmail_work");
    assert!(
        start
            .extensions
            .prompt_blocks(true)
            .contains("gmail_work: Gmail, gmail_work@example.com")
    );

    // Sending asks first. The client answers the card; the call then runs.
    let World { worker, client, .. } = &mut world;
    let (result, ()) = tokio::join!(
        worker.tool_call(
            session_id,
            "run_connector_tool",
            json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL",
                   "arguments": {"to": "ana@example.com", "subject": "Report", "body": "Attached."}}),
        ),
        async {
            let entry = client
                .wait_feed(|e| {
                    matches!(upserted(e), Some(EntityPayload::Approval(a)) if a.is_hub_card())
                })
                .await;
            let Some(EntityPayload::Approval(card)) = upserted(&entry) else {
                unreachable!()
            };
            let ApprovalDetails::Connector(details) = &card.details else {
                panic!("not a connector card");
            };
            assert_eq!(details.fields[0].label, "To");
            assert_eq!(details.fields[0].value, "ana@example.com");
            client
                .request(ApprovalAnswer {
                    approval_id: card.id,
                    option_id: "allow".to_owned(),
                })
                .await
                .unwrap();
        }
    );
    assert!(!result.is_error, "{:?}", result.content);
    let body = result.structured_content.unwrap();
    assert_eq!(body["status"], "ran");
    assert_eq!(fake.executions().len(), 1);
    assert_eq!(fake.executions()[0].1["arguments"]["subject"], "Report");
    // The hub answered the card itself: the worker got no session.respond.
    let next = world
        .worker
        .try_next_command(std::time::Duration::from_millis(300))
        .await;
    assert!(next.is_none(), "{next:?}");
}

#[tokio::test]
async fn a_worker_reads_the_skill_bundle_that_its_session_names() {
    let (mut world, _fake) = world_with_composio().await;
    let skill = world
        .hub
        .handle
        .add_skill_from_files(
            "notes.zip",
            vec![
                (
                    "SKILL.md".to_owned(),
                    b"---\nname: notes\ndescription: Takes notes.\n---\n# Notes\n".to_vec(),
                    false,
                ),
                ("scripts/tidy.sh".to_owned(), b"echo tidy".to_vec(), true),
            ],
            UseIn::EveryChannel,
            Some(true),
        )
        .await
        .unwrap();
    assert_eq!(skill.scripts, vec!["scripts/tidy.sh".to_owned()]);
    let start = world.start_coordinator("Tidy my notes.").await;
    assert_eq!(start.extensions.skills.len(), 1);
    assert_eq!(start.extensions.skills[0].content_hash, skill.content_hash);
    assert_eq!(
        start.extensions.skill_context[0].description,
        "Takes notes."
    );
    let value = world
        .worker
        .peer
        .call_raw(
            "skill_bundle.get",
            serde_json::to_value(SkillBundleGet {
                content_hash: skill.content_hash.clone(),
            })
            .unwrap(),
        )
        .await;
    let bundle: consort_proto::model::SkillBundle = match value {
        Ok(v) => serde_json::from_value(v).unwrap(),
        Err(err) => panic!("{err:?}"),
    };
    assert!(bundle.is_consistent());
    assert_eq!(skill_content_hash(&bundle.files), skill.content_hash);
    // A hash that no session.start named gets not_found.
    let err = world
        .worker
        .peer
        .call_raw("skill_bundle.get", json!({"content_hash": "c".repeat(64)}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    assert_eq!(snapshot.skills.len(), 1);
}

#[tokio::test]
async fn a_new_skill_replaces_the_idle_coordinator_session_at_its_next_turn() {
    let (mut world, _fake) = world_with_composio().await;
    let first = world.start_coordinator("First").await;
    assert!(first.extensions.is_empty());
    world
        .worker
        .send_events(
            first.session_id,
            vec![turn_ended(
                "t1",
                consort_proto::worker_link::TurnEndReason::Completed,
            )],
        )
        .await;
    world
        .hub
        .handle
        .add_skill_from_files(
            "notes.zip",
            vec![(
                "SKILL.md".to_owned(),
                b"---\nname: notes\ndescription: Takes notes.\n---\n".to_vec(),
                false,
            )],
            UseIn::EveryChannel,
            None,
        )
        .await
        .unwrap();
    let tutti = world.tutti;
    world.say(tutti, "Second").await;
    // The set changed, so the hub ends the old session and starts one with
    // the skill.
    let (request, command) = world.worker.next_command().await;
    let consort_proto::worker_link::HubToWorker::SessionStop(stop) = command else {
        panic!("expected session.stop, got {}", command.type_name());
    };
    assert_eq!(stop.session_id, first.session_id);
    world.worker.respond_ok(&request, &json!({})).await;
    let next = world.worker.accept_session_start().await;
    assert_ne!(next.session_id, first.session_id);
    assert_eq!(next.extensions.skill_context[0].name, "notes");
}
