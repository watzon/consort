//! The smaller connector items over the real links: the Composio status
//! after a key that Composio refuses, the logo route, and connections after
//! a key of another Composio project.
//!
//! Composio is the fake local server (`src/connectors/fake.rs`). No test
//! talks to the real Composio.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::common::*;
use crate::connectors::fake::{FakeComposio, GOOD_KEY};
use consort_proto::client_api::{
    ComposioGetStatus, ComposioSetKey, ConnectionConnect, ConnectorCatalogSearch, EntityPayload,
    SnapshotScope,
};
use consort_proto::error::ErrorCode;
use consort_proto::model::{AuthState, ConnectionStatus, SecretEnv, ToolApprovals, UseIn};
use consort_proto::session_host_link::{HubToSessionHost, SessionEvent, TurnEndReason};

async fn world_with_composio() -> (World, FakeComposio) {
    let world = World::start().await;
    let fake = FakeComposio::start().await;
    world.hub.handle.set_composio_url(&fake.url);
    (world, fake)
}

async fn set_key(world: &mut World) {
    world
        .client
        .request(ComposioSetKey {
            api_key: GOOD_KEY.to_owned(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_key_that_composio_refuses_later_shows_in_the_status_with_the_next_step() {
    let (mut world, fake) = world_with_composio().await;
    set_key(&mut world).await;
    let first = world
        .client
        .request(ComposioGetStatus {})
        .await
        .unwrap()
        .status;
    assert_eq!(first.error, None);
    assert_eq!(first.usage.map(|u| u.used), Some(0));
    // Composio revokes the key. The next call that reaches Composio meets
    // the refusal, and the status says so.
    fake.revoke_key();
    let err = world
        .client
        .request(ConnectorCatalogSearch::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ComposioError);
    let status = world
        .client
        .request(ComposioGetStatus {})
        .await
        .unwrap()
        .status;
    assert!(status.key_present);
    let message = status.error.expect("an error in the status");
    assert!(message.contains("Settings > Connectors"), "{message}");
    assert!(message.contains("project key"), "{message}");
    // A new key that works clears it.
    fake.restore_key();
    set_key(&mut world).await;
    let status = world
        .client
        .request(ComposioGetStatus {})
        .await
        .unwrap()
        .status;
    assert_eq!(status.error, None);
}

#[tokio::test]
async fn the_hub_serves_a_cached_logo_to_a_client_with_no_login() {
    let (mut world, fake) = world_with_composio().await;
    fake.serve_logos();
    world.hub.handle.allow_local_logo_origins(true);
    set_key(&mut world).await;
    let page = world
        .client
        .request(ConnectorCatalogSearch {
            search: Some("gmail".to_owned()),
            ..Default::default()
        })
        .await
        .unwrap();
    let url = page.services[0].logo_url.clone().expect("a logo URL");
    assert!(
        url.starts_with(&world.hub.url),
        "the hub's own address: {url}"
    );
    assert!(url.contains("/v1/logos/"));

    // A plain GET: the client loads the logo as an image, with no token.
    let http = reqwest::Client::new();
    let response = http.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let headers = response.headers().clone();
    assert_eq!(headers["content-type"], "image/svg+xml");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    assert!(
        headers["cache-control"]
            .to_str()
            .unwrap()
            .contains("max-age")
    );
    assert!(response.text().await.unwrap().starts_with("<svg"));
    // The second request is served from the hub's cache.
    http.get(&url).send().await.unwrap();
    assert_eq!(fake.logo_hits("gmail.svg"), 1);

    // A key that the hub never gave out is not found, and it is no proxy.
    for bad in [
        format!("{}/v1/logos/{}", world.hub.url, "0".repeat(64)),
        format!("{}/v1/logos/not-a-key", world.hub.url),
        format!("{}/v1/logos/..%2F..%2Fetc%2Fpasswd", world.hub.url),
    ] {
        let response = http.get(&bad).send().await.unwrap();
        assert_eq!(response.status(), 404, "{bad}");
    }
}

#[tokio::test]
async fn connections_of_another_composio_project_are_marked_in_the_feed() {
    let (mut world, fake) = world_with_composio().await;
    fake.set_auto_activate(true);
    set_key(&mut world).await;
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
    let id = attempt.connection.id;
    world
        .client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Connection(c)) if c.id == id && c.status == ConnectionStatus::Active)
        })
        .await;
    // The user adds the key of another project. It has none of the accounts.
    fake.forget_accounts();
    set_key(&mut world).await;
    let entry = world
        .client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Connection(c)) if c.id == id && matches!(c.status, ConnectionStatus::NeedsSignIn { .. }))
        })
        .await;
    let Some(EntityPayload::Connection(marked)) = upserted(&entry) else {
        unreachable!()
    };
    let ConnectionStatus::NeedsSignIn { message } = &marked.status else {
        unreachable!()
    };
    assert!(
        message
            .as_deref()
            .unwrap()
            .contains("another Composio project")
    );
    // The connection stays in the snapshot, so the user can reconnect it.
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    assert_eq!(snapshot.connections.len(), 1);
}

/// A Claude login that expired: `claude auth status` cannot see it, so the
/// first turn finds out. The worker sends `login_expired`. The hub sets the
/// account to `error` with "The login expired. Sign in again.", which the app
/// shows as Expired, and the coordinator waits for a sign-in.
#[tokio::test]
async fn a_login_that_a_turn_found_expired_marks_the_account_and_stops_the_coordinator() {
    let mut world = World::start().await;
    let start = world.start_coordinator("hello").await;
    let session = start.session_id;
    world
        .worker
        .send_events(
            session,
            vec![
                SessionEvent::Error {
                    code: "login_expired".to_owned(),
                    message: "The login expired. Sign in again.".to_owned(),
                },
                SessionEvent::TurnEnded {
                    turn_id: "t1".into(),
                    reason: TurnEndReason::Failed,
                    error: Some("The login expired. Sign in again.".to_owned()),
                    error_kind: None,
                    resets_at: None,
                },
            ],
        )
        .await;
    // The hub ends the session on the worker, and the notice says where.
    let (request, command) = world.worker.next_command().await;
    assert!(matches!(command, HubToSessionHost::SessionStop(_)));
    world
        .worker
        .respond_ok(&request, &serde_json::json!({}))
        .await;
    world.wait_blocked(&["Claude Code"]).await;
    let id = world.worker.worker_id;
    let account_id = world.worker.account_id;
    let workers = world
        .client
        .request(consort_proto::client_api::WorkerList {})
        .await
        .unwrap();
    let account = workers
        .workers
        .iter()
        .find(|w| w.id == id)
        .and_then(|w| w.inventory.as_ref())
        .and_then(|inventory| inventory.account(account_id))
        .expect("the account")
        .clone();
    assert_eq!(account.auth_state, AuthState::Error);
    let detail = account.auth_detail.expect("a detail");
    assert!(
        detail.contains("expired") && detail.contains("Sign in again"),
        "{detail}"
    );
    world.hub.handle.shutdown().await.unwrap();
}
