//! The Composio status (the usage and a key that Composio refuses later),
//! the logo cache, and connections after a change to another Composio
//! project. All with the fake Composio server.

use std::time::Duration;

use consort_proto::client_api::{ComposioGetStatus, ComposioSetKey, ConnectorCatalogSearch};
use consort_proto::error::ErrorCode;
use consort_proto::model::{ConnectionStatus, ToolApprovals};
use serde_json::json;

use super::fake::{FakeComposio, GOOD_KEY};
use super::flow_tests::{active_gmail, call, coordinator_session, hub_with_fake, stored, tutti};
use super::logos::{LogoCache, ROUTE_PREFIX};
use super::{KEY_REFUSED_MESSAGE, Timings, connections::OTHER_PROJECT_MESSAGE};
use crate::hub::{Caller, Hub, test_hub};

async fn status(hub: &Hub) -> consort_proto::model::ComposioStatus {
    hub.composio_get_status(&Caller::Hub, ComposioGetStatus {})
        .await
        .unwrap()
        .status
}

// The status -------------------------------------------------------------------

#[tokio::test]
async fn the_status_counts_the_calls_of_this_month_and_has_no_plan() {
    let (hub, _dir, fake) = hub_with_fake().await;
    let first = status(&hub).await;
    let usage = first.usage.expect("usage");
    assert_eq!(usage.used, 0);
    assert_eq!(
        usage.limit, None,
        "Composio gives no limit to a project key"
    );
    assert!(usage.resets_at.is_some_and(|at| at > hub.now()));
    assert_eq!(first.plan, None);
    assert_eq!(first.error, None);
    active_gmail(&hub, &fake, ToolApprovals::default()).await;
    let session = coordinator_session(&hub, tutti(&hub).await, "acp").await;
    for _ in 0..2 {
        call(
            &hub,
            session,
            "run_connector_tool",
            json!({"connector": "gmail_work", "tool": "GMAIL_FETCH_EMAILS"}),
        )
        .await;
    }
    assert_eq!(status(&hub).await.usage.unwrap().used, 2);
}

#[tokio::test]
async fn the_usage_period_is_a_calendar_month_in_utc() {
    use chrono::TimeZone;
    let december = chrono::Utc
        .with_ymd_and_hms(2026, 12, 31, 23, 59, 59)
        .unwrap();
    let (start, end) = super::month_bounds(december);
    assert_eq!(
        start,
        chrono::Utc
            .with_ymd_and_hms(2026, 12, 1, 0, 0, 0)
            .unwrap()
            .timestamp_millis()
    );
    assert_eq!(
        end,
        chrono::Utc
            .with_ymd_and_hms(2027, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp_millis()
    );
}

#[tokio::test]
async fn a_key_that_composio_refuses_later_shows_with_the_next_step() {
    let (hub, _dir, fake) = hub_with_fake().await;
    assert_eq!(status(&hub).await.error, None);
    // Composio revokes the key. The hub finds out at its next check.
    fake.revoke_key();
    hub.refresh_composio_status().await.unwrap();
    let refused = status(&hub).await;
    assert!(refused.key_present, "the hub still has the key");
    let message = refused.error.expect("an error");
    assert_eq!(message, KEY_REFUSED_MESSAGE);
    assert!(
        message.contains("Settings > Connectors") && message.contains("project key"),
        "{message}"
    );
    assert!(refused.checked_at.is_some());
    // The error stays across a restart of the hub, because it is stored.
    let stored = hub
        .store
        .read(|conn| crate::store::meta_get(conn, super::STATUS_META_KEY))
        .await
        .unwrap()
        .unwrap();
    assert!(stored.contains("Composio refuses the key"));
    // A new key that works clears it.
    fake.restore_key();
    hub.composio_set_key(
        &Caller::Hub,
        ComposioSetKey {
            api_key: GOOD_KEY.to_owned(),
        },
    )
    .await
    .unwrap();
    assert_eq!(status(&hub).await.error, None);
}

#[tokio::test]
async fn a_refused_call_shows_in_the_status_before_the_next_check() {
    let (hub, _dir, fake) = hub_with_fake().await;
    // Warm nothing: the next catalog call meets a revoked key.
    fake.revoke_key();
    let err = hub
        .connector_catalog_search(&Caller::Hub, ConnectorCatalogSearch::default())
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ComposioError);
    assert!(err.to_error_body().message.contains("refused the key"));
    assert_eq!(
        status(&hub).await.error.as_deref(),
        Some(KEY_REFUSED_MESSAGE)
    );
    // Any answer that is not a refusal clears the flag.
    fake.restore_key();
    hub.refresh_composio_status().await.unwrap();
    assert_eq!(status(&hub).await.error, None);
}

#[tokio::test]
async fn a_network_failure_says_nothing_about_the_key() {
    let (hub, _dir, _fake) = hub_with_fake().await;
    // Nothing listens on port 1.
    hub.connectors.set_base_url("http://127.0.0.1:1/api/v3.1");
    hub.refresh_composio_status().await.unwrap();
    assert_eq!(status(&hub).await.error, None);
}

#[tokio::test]
async fn a_slow_composio_gives_up_at_the_limit_with_a_clear_error() {
    // A server that accepts the connection and never answers.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/v3.1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let (hub, _dir, _fake) = hub_with_fake().await;
    hub.connectors.set_base_url(&url);
    hub.connectors.set_timings(Timings {
        request_timeout: Some(Duration::from_millis(300)),
        ..super::flow_tests::fast()
    });
    let started = std::time::Instant::now();
    let err = hub
        .connector_catalog_search(&Caller::Hub, ConnectorCatalogSearch::default())
        .await
        .unwrap_err();
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(250), "{took:?}");
    assert!(took < Duration::from_secs(10), "{took:?}");
    assert_eq!(err.code(), ErrorCode::ComposioError);
    let message = err.to_error_body().message;
    assert!(
        message.contains("did not answer within 300 milliseconds"),
        "{message}"
    );
    assert!(message.contains("Try again"), "{message}");
    // A time-out says nothing about the key.
    assert_eq!(status(&hub).await.error, None);
    server.abort();
}

#[tokio::test]
async fn without_a_key_the_status_is_empty() {
    let (hub, _dir) = test_hub().await;
    let empty = status(&hub).await;
    assert!(!empty.key_present);
    assert_eq!(empty.usage, None);
    hub.refresh_composio_status().await.unwrap();
}

// The logo cache ---------------------------------------------------------------

fn key_of_url(url: &str) -> &str {
    url.rsplit_once(ROUTE_PREFIX).expect("a hub logo URL").1
}

async fn hub_with_logos() -> (Hub, tempfile::TempDir, FakeComposio) {
    let (hub, dir, fake) = hub_with_fake().await;
    fake.serve_logos();
    hub.connectors.logos.set_allow_local(true);
    (hub, dir, fake)
}

#[tokio::test]
async fn the_catalog_gives_the_hub_url_and_the_hub_fetches_each_logo_once() {
    let (hub, dir, fake) = hub_with_logos().await;
    let page = hub
        .connector_catalog_search(
            &Caller::Hub,
            ConnectorCatalogSearch {
                search: Some("gmail".to_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let url = page.services[0].logo_url.clone().expect("a logo URL");
    let base = hub.settings.public_url.trim_end_matches('/');
    assert!(url.starts_with(&format!("{base}{ROUTE_PREFIX}")), "{url}");
    assert!(!url.contains(&fake.origin()), "no third party URL");
    // Nothing is fetched until a client asks.
    assert_eq!(fake.logo_hits("gmail.svg"), 0);
    let key = key_of_url(&url);
    let first = hub.connectors.logos.get(key).await.expect("a logo");
    assert_eq!(first.content_type, "image/svg+xml");
    let second = hub.connectors.logos.get(key).await.expect("a logo");
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(fake.logo_hits("gmail.svg"), 1, "fetched once");
    assert!(dir.path().join("logos").join(key).exists());
    // The same service gives the same URL, so clients cache it.
    let again = hub
        .connector_catalog_search(
            &Caller::Hub,
            ConnectorCatalogSearch {
                search: Some("gmail".to_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(again.services[0].logo_url.as_deref(), Some(url.as_str()));
}

#[tokio::test]
async fn a_stale_logo_is_fetched_again_and_the_old_copy_answers_when_that_fails() {
    let (hub, _dir, fake) = hub_with_logos().await;
    let origin = format!("{}/logos/gmail.svg", fake.origin());
    let key = hub.connectors.logos.register(&origin, false).unwrap();
    hub.connectors.logos.get(&key).await.unwrap();
    assert_eq!(fake.logo_hits("gmail.svg"), 1);
    hub.connectors.logos.set_refresh(Duration::ZERO);
    hub.connectors.logos.get(&key).await.unwrap();
    assert_eq!(fake.logo_hits("gmail.svg"), 2, "refreshed after its age");
    // The origin is gone. The cached copy still answers.
    fake.revoke_logos_of("gmail.svg");
    let kept = hub.connectors.logos.get(&key).await;
    assert!(kept.is_some(), "the old copy answers");
    assert_eq!(fake.logo_hits("gmail.svg"), 3, "the hub tried the origin");
}

#[tokio::test]
async fn a_big_file_and_a_file_that_is_no_image_are_not_kept() {
    let (hub, dir, fake) = hub_with_logos().await;
    for name in ["big.png", "bad.svg"] {
        let key = hub
            .connectors
            .logos
            .register(&format!("{}/logos/{name}", fake.origin()), false)
            .unwrap();
        assert!(hub.connectors.logos.get(&key).await.is_none(), "{name}");
        assert!(!dir.path().join("logos").join(&key).exists(), "{name}");
    }
    // The hub does not ask again at once for a logo that failed.
    let key = hub
        .connectors
        .logos
        .register(&format!("{}/logos/bad.svg", fake.origin()), false)
        .unwrap();
    hub.connectors.logos.get(&key).await;
    assert_eq!(fake.logo_hits("bad.svg"), 1);
}

#[tokio::test]
async fn the_cache_keeps_to_its_file_limit_and_drops_the_oldest() {
    let (hub, dir, fake) = hub_with_logos().await;
    hub.connectors.logos.set_limits(2, u64::MAX);
    let mut keys = Vec::new();
    for name in ["a.svg", "b.svg", "c.svg"] {
        let key = hub
            .connectors
            .logos
            .register(&format!("{}/logos/{name}", fake.origin()), false)
            .unwrap();
        hub.connectors.logos.get(&key).await.unwrap();
        keys.push(key);
        // File times have a coarse step on some systems.
        tokio::time::sleep(Duration::from_millis(1100)).await;
    }
    assert_eq!(hub.connectors.logos.usage().0, 2);
    assert!(!dir.path().join("logos").join(&keys[0]).exists(), "oldest");
    assert!(dir.path().join("logos").join(&keys[2]).exists(), "newest");
    // The dropped logo comes back when a client asks for it again.
    assert!(hub.connectors.logos.get(&keys[0]).await.is_some());
    assert_eq!(fake.logo_hits("a.svg"), 2);
}

#[tokio::test]
async fn a_logo_that_does_not_answer_gives_up_at_its_limit() {
    // The slow route accepts the request and never answers.
    let (hub, _dir, fake) = hub_with_logos().await;
    hub.connectors.logos.set_timeout(Duration::from_millis(300));
    let key = hub
        .connectors
        .logos
        .register(&format!("{}/logos/slow.svg", fake.origin()), false)
        .unwrap();
    let started = std::time::Instant::now();
    assert!(hub.connectors.logos.get(&key).await.is_none());
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(250), "{took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert_eq!(fake.logo_hits("slow.svg"), 1);
    // The hub does not wait for the same origin again at once.
    let started = std::time::Instant::now();
    assert!(hub.connectors.logos.get(&key).await.is_none());
    assert!(started.elapsed() < Duration::from_millis(250));
    assert_eq!(fake.logo_hits("slow.svg"), 1);
}

#[tokio::test]
async fn a_connection_keeps_a_hub_logo_url_that_works_after_a_restart() {
    let (hub, dir, fake) = hub_with_logos().await;
    fake.set_auto_activate(true);
    let connection = active_gmail(&hub, &fake, ToolApprovals::default()).await;
    let url = connection.logo_url.clone().expect("a logo");
    let key = key_of_url(&url).to_owned();
    assert!(url.contains(ROUTE_PREFIX));
    assert!(dir.path().join("logos").join(format!("{key}.url")).exists());
    // A fresh cache object on the same folder finds the origin.
    let reopened = LogoCache::open(dir.path());
    reopened.set_allow_local(true);
    assert!(reopened.get(&key).await.is_some());
}

#[tokio::test]
async fn a_connection_stored_with_a_composio_logo_gets_the_hub_url_at_start() {
    let (hub, _dir, fake) = hub_with_logos().await;
    let connection = active_gmail(&hub, &fake, ToolApprovals::default()).await;
    // Write the old form: the origin URL of Composio.
    let origin = format!("{}/logos/gmail.svg", fake.origin());
    let id = connection.id;
    let old = origin.clone();
    hub.store
        .write(move |tx| {
            let mut c = crate::store::connectors::connection_or_404(tx.db(), id)?;
            c.logo_url = Some(old);
            tx.save_connection(&c)
        })
        .await
        .unwrap();
    hub.refresh_connection_logos().await.unwrap();
    let url = stored(&hub, id).await.logo_url.unwrap();
    assert!(url.contains(ROUTE_PREFIX), "{url}");
    // An address change of the hub moves the stored URL.
    let moved = url.replace(
        hub.settings.public_url.trim_end_matches('/'),
        "https://old.example",
    );
    hub.store
        .write(move |tx| {
            let mut c = crate::store::connectors::connection_or_404(tx.db(), id)?;
            c.logo_url = Some(moved);
            tx.save_connection(&c)
        })
        .await
        .unwrap();
    hub.refresh_connection_logos().await.unwrap();
    assert_eq!(stored(&hub, id).await.logo_url.unwrap(), url);
}

// Another Composio project -----------------------------------------------------

#[tokio::test]
async fn a_key_of_another_project_marks_the_old_connections_and_the_old_key_restores_them() {
    let (hub, _dir, fake) = hub_with_fake().await;
    let connection = active_gmail(&hub, &fake, ToolApprovals::default()).await;
    assert!(stored(&hub, connection.id).await.status.is_active());
    // The other project has none of the accounts.
    let accounts = fake.state.lock().unwrap().accounts.clone();
    fake.forget_accounts();
    hub.composio_set_key(
        &Caller::Hub,
        ComposioSetKey {
            api_key: GOOD_KEY.to_owned(),
        },
    )
    .await
    .unwrap();
    let marked = stored(&hub, connection.id).await;
    assert_eq!(
        marked.status,
        ConnectionStatus::NeedsSignIn {
            message: Some(OTHER_PROJECT_MESSAGE.to_owned())
        }
    );
    assert!(OTHER_PROJECT_MESSAGE.contains("another Composio project"));
    assert!(OTHER_PROJECT_MESSAGE.contains("Reconnect"));
    // The connection stays in the list, and a call says what to do.
    let session = coordinator_session(&hub, tutti(&hub).await, "acp").await;
    let result = hub
        .call_tool(
            session,
            "run_connector_tool",
            json!({"connector": "gmail_work", "tool": "GMAIL_FETCH_EMAILS"}),
        )
        .await;
    assert!(result.is_error);
    assert_eq!(
        result.structured_content.unwrap()["error"]["code"],
        "needs_user"
    );
    // The old project's key comes back, with its accounts.
    fake.state.lock().unwrap().accounts = accounts;
    hub.composio_set_key(
        &Caller::Hub,
        ComposioSetKey {
            api_key: GOOD_KEY.to_owned(),
        },
    )
    .await
    .unwrap();
    assert!(stored(&hub, connection.id).await.status.is_active());
}

#[tokio::test]
async fn reconnect_replaces_the_account_of_the_other_project() {
    let (hub, _dir, fake) = hub_with_fake().await;
    hub.connectors.set_timings(Timings {
        poll_interval: Duration::from_millis(20),
        ..super::flow_tests::fast()
    });
    let connection = active_gmail(&hub, &fake, ToolApprovals::default()).await;
    fake.forget_accounts();
    hub.reconcile_connections_with_key().await.unwrap();
    assert!(matches!(
        stored(&hub, connection.id).await.status,
        ConnectionStatus::NeedsSignIn { .. }
    ));
    let attempt = hub
        .connection_reconnect(
            &Caller::Hub,
            consort_proto::client_api::ConnectionReconnect {
                connection_id: connection.id,
                credentials: consort_proto::model::SecretEnv::new(),
            },
        )
        .await
        .unwrap();
    assert!(attempt.redirect_url.is_some());
    let id = connection.id;
    super::flow_tests::wait_for(async || stored(&hub, id).await.status.is_active().then_some(()))
        .await;
}

#[tokio::test]
async fn the_hourly_check_names_an_unknown_account_as_another_project() {
    let (hub, _dir, fake) = hub_with_fake().await;
    let connection = active_gmail(&hub, &fake, ToolApprovals::default()).await;
    fake.forget_accounts();
    hub.sweep_connection_status().await.unwrap();
    let ConnectionStatus::NeedsSignIn { message } = stored(&hub, connection.id).await.status else {
        panic!("needs a new sign-in");
    };
    assert_eq!(message.as_deref(), Some(OTHER_PROJECT_MESSAGE));
}

#[tokio::test]
async fn an_expired_account_keeps_the_expired_message() {
    let (hub, _dir, fake) = hub_with_fake().await;
    let connection = active_gmail(&hub, &fake, ToolApprovals::default()).await;
    fake.set_status(None, "EXPIRED");
    hub.sweep_connection_status().await.unwrap();
    let ConnectionStatus::NeedsSignIn { message } = stored(&hub, connection.id).await.status else {
        panic!("needs a new sign-in");
    };
    assert!(message.unwrap().contains("expired"));
}
