//! `request_access`, with the fake Composio and the fake Monid.

use std::sync::atomic::Ordering;

use consort_proto::client_api::{ChannelCreate, ChannelResult, MonidSetKey};
use consort_proto::ids::SessionId;
use consort_proto::model::{
    ApprovalMode, Connection, HUB_CARD_DENY, MONID_CARD_NEVER, MonidSettings, ToolApprovals,
};
use serde_json::json;
use tokio::task::JoinHandle;

use super::*;
use crate::connectors::flow_tests::{
    answer, connect, coordinator_session, hub_with_fake, pending_card, stored, tutti, wait_for,
};
use crate::connectors::monid_run::tests::{GOOD_KEY, Shared, start_fake};
use crate::hub::test_task_session;

async fn call(hub: &Hub, session: SessionId, tool: &str, args: Value) -> Value {
    let result = hub.call_tool(session, tool, args).await;
    let text = format!("{result:?}");
    result
        .structured_content
        .unwrap_or_else(|| panic!("{tool} failed: {text}"))
}

fn request(hub: &Hub, session: SessionId, args: Value) -> JoinHandle<Value> {
    let hub = hub.clone();
    tokio::spawn(async move { call(&hub, session, "request_access", args).await })
}

async fn other_channel(hub: &Hub, name: &str) -> ChannelId {
    hub.channel_create(
        &Caller::Hub,
        ChannelCreate {
            name: name.to_owned(),
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
        },
    )
    .await
    .unwrap()
    .channel
    .id
}

/// A hub with Gmail Work, only in another channel. Tutti does not have it.
async fn gmail_elsewhere() -> (Hub, tempfile::TempDir, Connection, ChannelId) {
    let (hub, dir, fake) = hub_with_fake().await;
    fake.set_auto_activate(true);
    let elsewhere = other_channel(&hub, "Elsewhere").await;
    let connection = connect(
        &hub,
        "gmail",
        "Gmail Work",
        UseIn::Channels {
            channel_ids: vec![elsewhere],
        },
        ToolApprovals::default(),
    )
    .await;
    let id = connection.id;
    let connection = wait_for(async || {
        let c = stored(&hub, id).await;
        c.status.is_active().then_some(c)
    })
    .await;
    (hub, dir, connection, elsewhere)
}

/// Adds Monid with a Use in that leaves out Tutti.
async fn monid_elsewhere(hub: &Hub, elsewhere: ChannelId) -> Shared {
    let fake = start_fake().await;
    hub.connectors
        .set_monid_test_server(&fake.url.lock().unwrap().clone());
    hub.monid_set_key(
        &Caller::Hub,
        MonidSetKey {
            api_key: GOOD_KEY.to_owned(),
            settings: Some(MonidSettings {
                use_in: UseIn::Channels {
                    channel_ids: vec![elsewhere],
                },
                ..Default::default()
            }),
        },
    )
    .await
    .unwrap();
    fake
}

async fn channel_now(hub: &Hub, id: ChannelId) -> Channel {
    hub.store
        .read(move |conn| conversation::channel_or_404(conn, id))
        .await
        .unwrap()
}

#[tokio::test]
async fn a_session_sees_what_it_lacks_with_only_names() {
    let (hub, _dir, connection, _) = gmail_elsewhere().await;
    let channel = tutti(&hub).await;
    let session = coordinator_session(&hub, channel, "acp").await;
    let extensions = hub
        .store
        .read(move |conn| crate::connectors::extensions::extensions_for(conn, channel))
        .await
        .unwrap();
    assert!(extensions.connectors.is_empty());
    assert_eq!(
        extensions.other_connections,
        vec!["Gmail Work (gmail_work)"]
    );
    let block = extensions.prompt_blocks(false);
    assert!(block.contains("Gmail Work (gmail_work)"), "{block}");
    assert!(block.contains("request_access"), "{block}");

    let found = call(
        &hub,
        session,
        "search_tools",
        json!({"query": "send email"}),
    )
    .await;
    let row = &found["tools"][0];
    assert_eq!(row["source"], "gmail_work", "{found}");
    assert_eq!(row["access"], "request");

    // The tools refuse, and the error says how to ask.
    let refused = hub
        .call_tool(
            session,
            "list_connector_tools",
            json!({"connector": connection.handle}),
        )
        .await;
    assert!(refused.is_error);
    let text = serde_json::to_string(&refused.content).unwrap();
    assert!(text.contains("request_access"), "{text}");
}

#[tokio::test]
async fn temporary_access_in_a_task_stays_in_that_task() {
    let (hub, _dir, connection, _) = gmail_elsewhere().await;
    let (task, session) = test_task_session(&hub).await;
    let asked = request(
        &hub,
        session.id,
        json!({"connection": "gmail_work", "reason": "I need the March invoices."}),
    );
    let card = pending_card(&hub).await;
    let ApprovalDetails::AccessRequest(details) = &card.details else {
        panic!("not an access card");
    };
    assert_eq!(details.title, "Give this task access to Gmail Work");
    assert_eq!(
        details.reason.as_deref(),
        Some("I need the March invoices.")
    );
    let ids: Vec<&str> = card.options.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(
        ids,
        [ACCESS_CARD_TEMPORARY, ACCESS_CARD_ALWAYS, HUB_CARD_DENY]
    );
    assert_eq!(card.options[0].label, "Allow for this task");
    answer(&hub, card.id, ACCESS_CARD_TEMPORARY).await;
    assert_eq!(asked.await.unwrap()["status"], "granted");

    // Its tools work in the task, with no change to the connection.
    let listed = hub
        .call_tool(
            session.id,
            "list_connector_tools",
            json!({"connector": "gmail_work"}),
        )
        .await;
    assert!(!listed.is_error, "{listed:?}");
    assert_ne!(
        stored(&hub, connection.id).await.use_in,
        UseIn::EveryChannel
    );
    // The chat of the same channel does not have it.
    let coordinator = coordinator_session(&hub, task.channel_id, "acp").await;
    let refused = hub
        .call_tool(
            coordinator,
            "list_connector_tools",
            json!({"connector": "gmail_work"}),
        )
        .await;
    assert!(refused.is_error);
}

#[tokio::test]
async fn temporary_access_in_the_chat_ends_on_time_or_when_the_user_ends_it() {
    let (hub, _dir, _connection, _) = gmail_elsewhere().await;
    let channel = tutti(&hub).await;
    let session = coordinator_session(&hub, channel, "acp").await;
    let asked = request(&hub, session, json!({"connection": "gmail_work"}));
    let card = pending_card(&hub).await;
    let ApprovalDetails::AccessRequest(details) = &card.details else {
        panic!("not an access card");
    };
    assert_eq!(details.title, "Give this channel access to Gmail Work");
    assert_eq!(card.options[0].label, "Allow for 24 hours");
    answer(&hub, card.id, ACCESS_CARD_TEMPORARY).await;
    assert_eq!(asked.await.unwrap()["status"], "granted");
    let listed = |hub: Hub| async move {
        hub.call_tool(
            session,
            "list_connector_tools",
            json!({"connector": "gmail_work"}),
        )
        .await
        .is_error
    };
    assert!(!listed(hub.clone()).await);

    // The user ends it.
    let grant = channel_now(&hub, channel).await.access_grants[0].clone();
    assert!(grant.expires_at.is_some());
    let ChannelResult { channel: after } = hub
        .channel_end_access(
            &Caller::Hub,
            ChannelEndAccess {
                channel_id: channel,
                grant_id: grant.id,
            },
        )
        .await
        .unwrap();
    assert!(after.access_grants.is_empty());
    assert!(listed(hub.clone()).await);

    // The time ends it, and the sweep removes the row.
    let mut old = grant;
    old.expires_at = Some(hub.now().plus_millis(-1));
    hub.store
        .write(move |tx| {
            let mut channel = conversation::channel_or_404(tx.db(), channel)?;
            channel.access_grants.push(old);
            tx.save_channel(&channel)
        })
        .await
        .unwrap();
    assert!(listed(hub.clone()).await);
    hub.sweep_access_grants().await.unwrap();
    assert!(channel_now(&hub, channel).await.access_grants.is_empty());
}

#[tokio::test]
async fn the_sweep_removes_access_when_its_task_ended() {
    let (hub, _dir, _connection, _) = gmail_elsewhere().await;
    let (task, session) = test_task_session(&hub).await;
    let asked = request(&hub, session.id, json!({"connection": "gmail_work"}));
    let card = pending_card(&hub).await;
    answer(&hub, card.id, ACCESS_CARD_TEMPORARY).await;
    asked.await.unwrap();
    hub.sweep_access_grants().await.unwrap();
    assert_eq!(
        channel_now(&hub, task.channel_id).await.access_grants.len(),
        1
    );
    let id = task.id;
    hub.store
        .write(move |tx| {
            let mut task = work::task_or_404(tx.db(), id)?;
            task.status = TaskStatus::Resolved;
            tx.save_task(&task)
        })
        .await
        .unwrap();
    hub.sweep_access_grants().await.unwrap();
    assert!(
        channel_now(&hub, task.channel_id)
            .await
            .access_grants
            .is_empty()
    );
}

#[tokio::test]
async fn lasting_access_widens_use_in_and_clears_an_exception() {
    let (hub, _dir, connection, elsewhere) = gmail_elsewhere().await;
    let channel = tutti(&hub).await;
    let session = coordinator_session(&hub, channel, "acp").await;
    let asked = request(
        &hub,
        session,
        json!({"connection": "gmail_work", "duration": "permanent"}),
    );
    let card = pending_card(&hub).await;
    // The length that the session asked for is first.
    assert_eq!(card.options[0].id, ACCESS_CARD_ALWAYS);
    let ApprovalDetails::AccessRequest(details) = &card.details else {
        panic!("not an access card");
    };
    assert!(details.effect.contains("Use in"), "{}", details.effect);
    answer(&hub, card.id, ACCESS_CARD_ALWAYS).await;
    assert_eq!(asked.await.unwrap()["status"], "granted");
    assert_eq!(
        stored(&hub, connection.id).await.use_in,
        UseIn::Channels {
            channel_ids: vec![elsewhere, channel]
        }
    );
    assert!(channel_now(&hub, channel).await.access_grants.is_empty());

    // A channel that turned it off can ask too, and Always turns it on.
    let id = connection.id;
    hub.store
        .write(move |tx| {
            let mut c = conversation::channel_or_404(tx.db(), channel)?;
            c.disabled_connections.push(id);
            tx.save_channel(&c)
        })
        .await
        .unwrap();
    let asked = request(
        &hub,
        session,
        json!({"connection": "gmail_work", "duration": "permanent"}),
    );
    let card = pending_card(&hub).await;
    let ApprovalDetails::AccessRequest(details) = &card.details else {
        panic!("not an access card");
    };
    assert!(details.effect.contains("turns it on"), "{}", details.effect);
    answer(&hub, card.id, ACCESS_CARD_ALWAYS).await;
    asked.await.unwrap();
    assert!(
        channel_now(&hub, channel)
            .await
            .disabled_connections
            .is_empty()
    );
}

#[tokio::test]
async fn a_deny_stops_the_same_request_and_a_second_ask_shares_the_card() {
    let (hub, _dir, _connection, _) = gmail_elsewhere().await;
    let channel = tutti(&hub).await;
    let session = coordinator_session(&hub, channel, "acp").await;
    hub.connectors.set_timings(super::super::Timings {
        approval_wait: std::time::Duration::from_millis(100),
        ..crate::connectors::monid_run::tests::fast()
    });
    let first = call(
        &hub,
        session,
        "request_access",
        json!({"connection": "gmail_work"}),
    )
    .await;
    assert_eq!(first["status"], "waiting_for_user");
    let second = call(
        &hub,
        session,
        "request_access",
        json!({"connection": "gmail_work"}),
    )
    .await;
    assert_eq!(second["status"], "waiting_for_user");
    assert_eq!(second["approval_id"], first["approval_id"]);
    let pending = hub
        .store
        .read(|conn| work::pending_approvals(conn, None))
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);

    // A late answer goes to the session in a later turn.
    answer(&hub, pending[0].id, HUB_CARD_DENY).await;
    let wakes = wait_for(async || {
        let wakes = hub
            .store
            .read(move |conn| crate::store::coordinator::pending_wakes(conn, channel))
            .await
            .unwrap();
        (!wakes.is_empty()).then_some(wakes)
    })
    .await;
    assert!(
        format!("{wakes:?}").contains("did not give access"),
        "{wakes:?}"
    );

    let again = call(
        &hub,
        session,
        "request_access",
        json!({"connection": "gmail_work"}),
    )
    .await;
    assert_eq!(again["status"], "refused");
    assert!(again["message"].as_str().unwrap().contains("said no"));
}

#[tokio::test]
async fn full_access_still_asks_for_access() {
    let (hub, _dir, _connection, _) = gmail_elsewhere().await;
    let channel = tutti(&hub).await;
    let session = coordinator_session(&hub, channel, "acp").await;
    hub.store
        .write(move |tx| {
            let mut c = conversation::channel_or_404(tx.db(), channel)?;
            c.default_approval_mode = ApprovalMode::FullAccess;
            tx.save_channel(&c)
        })
        .await
        .unwrap();
    let asked = request(&hub, session, json!({"connection": "gmail_work"}));
    let card = pending_card(&hub).await;
    answer(&hub, card.id, HUB_CARD_DENY).await;
    assert_eq!(asked.await.unwrap()["status"], "denied");
}

#[tokio::test]
async fn a_channel_with_no_monid_can_ask_for_all_of_it_or_one_tool() {
    let (hub, _dir, _connection, elsewhere) = gmail_elsewhere().await;
    let fake = monid_elsewhere(&hub, elsewhere).await;
    let channel = tutti(&hub).await;
    let session = coordinator_session(&hub, channel, "acp").await;
    let extensions = hub
        .store
        .read(move |conn| crate::connectors::extensions::extensions_for(conn, channel))
        .await
        .unwrap();
    assert!(extensions.monid_requestable);
    let found = call(&hub, session, "search_tools", json!({"query": "fast"})).await;
    let monid_row = found["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["source"] == "monid")
        .unwrap();
    assert_eq!(monid_row["access"], "request");

    // One tool for 24 hours: that tool only.
    let asked = request(
        &hub,
        session,
        json!({"monid": {"provider": "acme", "endpoint": "/fast"}}),
    );
    let card = pending_card(&hub).await;
    answer(&hub, card.id, ACCESS_CARD_TEMPORARY).await;
    assert_eq!(asked.await.unwrap()["status"], "granted");
    let running = {
        let hub = hub.clone();
        tokio::spawn(async move {
            call(
                &hub,
                session,
                "run_monid_tool",
                json!({"provider": "acme", "endpoint": "/fast"}),
            )
            .await
        })
    };
    // Access does not change approvals: the call still asks.
    let card = pending_card(&hub).await;
    assert!(matches!(card.details, ApprovalDetails::Monid(_)));
    answer(&hub, card.id, "allow").await;
    assert_eq!(running.await.unwrap()["status"], "ran");
    assert_eq!(fake.starts.load(Ordering::SeqCst), 1);
    let refused = call(
        &hub,
        session,
        "run_monid_tool",
        json!({"provider": "other", "endpoint": "/fast"}),
    )
    .await;
    assert_eq!(refused["status"], "denied");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("request_access"),
        "{refused}"
    );

    // All of Monid, for good.
    let asked = request(&hub, session, json!({"monid": {}, "duration": "permanent"}));
    let card = pending_card(&hub).await;
    answer(&hub, card.id, ACCESS_CARD_ALWAYS).await;
    assert_eq!(asked.await.unwrap()["status"], "granted");
    let settings = hub.store.read(monid_store::settings).await.unwrap();
    assert_eq!(
        settings.use_in,
        UseIn::Channels {
            channel_ids: vec![elsewhere, channel]
        }
    );
    let access = channel_now(&hub, channel).await.monid;
    assert!(!access.only_allowed);
}

#[tokio::test]
async fn lasting_access_to_one_tool_gives_a_new_channel_only_that_tool() {
    let (hub, _dir, _connection, elsewhere) = gmail_elsewhere().await;
    monid_elsewhere(&hub, elsewhere).await;
    let channel = tutti(&hub).await;
    let session = coordinator_session(&hub, channel, "acp").await;
    let asked = request(
        &hub,
        session,
        json!({"monid": {"provider": "acme", "endpoint": "/fast"}, "duration": "permanent"}),
    );
    let card = pending_card(&hub).await;
    answer(&hub, card.id, ACCESS_CARD_ALWAYS).await;
    asked.await.unwrap();
    let access = channel_now(&hub, channel).await.monid;
    assert!(access.only_allowed);
    assert!(!access.refuses("acme", "/fast"));
    assert!(access.refuses("other", "/fast"));
}

#[tokio::test]
async fn a_blocked_tool_cannot_be_requested_or_found() {
    let (hub, _dir, _connection, _) = gmail_elsewhere().await;
    let fake = monid_elsewhere(&hub, tutti(&hub).await).await;
    let channel = tutti(&hub).await;
    let session = coordinator_session(&hub, channel, "acp").await;
    let running = {
        let hub = hub.clone();
        tokio::spawn(async move {
            call(
                &hub,
                session,
                "run_monid_tool",
                json!({"provider": "other", "endpoint": "/fast"}),
            )
            .await
        })
    };
    let card = pending_card(&hub).await;
    answer(&hub, card.id, MONID_CARD_NEVER).await;
    running.await.unwrap();
    let refused = call(
        &hub,
        session,
        "request_access",
        json!({"monid": {"provider": "other", "endpoint": "/fast"}}),
    )
    .await;
    assert_eq!(refused["status"], "refused");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("Never use here")
    );
    let found = call(&hub, session, "search_tools", json!({"query": "fast"})).await;
    let rows = found["tools"].as_array().unwrap();
    assert!(!rows.iter().any(|r| r["provider"] == "other"), "{found}");
    assert_eq!(fake.starts.load(Ordering::SeqCst), 0);
}
