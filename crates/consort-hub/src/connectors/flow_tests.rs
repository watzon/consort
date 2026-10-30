//! Connections, connector calls, hub cards and the management tools, with
//! the fake Composio server.

use std::time::Duration;

use consort_proto::client_api::{
    ApprovalAnswer, ChannelCreate, ChannelSetConnectionEnabled, ConnectionCalls, ConnectionConnect,
    ConnectionDisconnect, ConnectionReconnect,
};
use consort_proto::error::ErrorCode;
use consort_proto::ids::{AccountId, ChannelId, SessionId, WorkerId};
use consort_proto::model::{
    ApprovalDetails, ApprovalRule, ApprovalStatus, AuthSchemeKind, CallDecision, CallOutcome,
    ChannelKind, Connection, ConnectionStatus, MessageKind, ModelOptions, SecretEnv, Session,
    SessionRole, SessionState, ToolApprovals, ToolKind, Usage, UseIn, canonical_json,
};
use consort_proto::time::Timestamp;
use consort_proto::turn_input::WakeEvent;
use serde_json::{Value, json};

use super::Timings;
use super::fake::{FakeComposio, SENDGRID_KEY};
use crate::hub::{Caller, Hub, test_hub, test_task_session};
use crate::store::{connectors as con_store, conversation, coordinator as coord_store, work};

pub(super) fn fast() -> Timings {
    Timings {
        approval_wait: Duration::from_secs(5),
        poll_interval: Duration::from_millis(20),
        connect_timeout_ms: 10 * 60 * 1_000,
        request_timeout: None,
    }
}

pub(super) async fn hub_with_fake() -> (Hub, tempfile::TempDir, FakeComposio) {
    let (hub, dir) = test_hub().await;
    let fake = FakeComposio::start().await;
    fake.configure(&hub).await;
    hub.connectors.set_timings(fast());
    (hub, dir, fake)
}

async fn channel(hub: &Hub, name: &str, tags: &[&str]) -> ChannelId {
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
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        },
    )
    .await
    .unwrap()
    .channel
    .id
}

pub(super) async fn coordinator_session(
    hub: &Hub,
    channel_id: ChannelId,
    harness: &str,
) -> SessionId {
    let session = Session {
        id: SessionId::new(),
        role: SessionRole::Coordinator,
        channel_id,
        task_id: None,
        worker_id: WorkerId::new(),
        account_id: AccountId::new(),
        harness: harness.to_owned(),
        model: "fake-1".to_owned(),
        options: ModelOptions::new(),
        workspace_path: "/tmp/workspace".to_owned(),
        native_ref: None,
        state: SessionState::Idle,
        last_seq: 0,
        usage: Usage::default(),
        started_at: Timestamp::now(),
        ended_at: None,
        end_reason: None,
    };
    let id = session.id;
    hub.store
        .write(move |tx| tx.save_session(&session))
        .await
        .unwrap();
    id
}

pub(super) async fn tutti(hub: &Hub) -> ChannelId {
    hub.store
        .read(conversation::tutti_channel)
        .await
        .unwrap()
        .id
}

pub(super) async fn connect(
    hub: &Hub,
    service: &str,
    name: &str,
    use_in: UseIn,
    approvals: ToolApprovals,
) -> Connection {
    let mut credentials = SecretEnv::new();
    if service == "sendgrid" {
        credentials.insert("api_key", SENDGRID_KEY);
    }
    hub.connection_connect(
        &Caller::Hub,
        ConnectionConnect {
            service: service.to_owned(),
            name: name.to_owned(),
            use_in,
            approvals,
            auth_scheme: None,
            credentials,
            approval_id: None,
        },
    )
    .await
    .unwrap()
    .connection
}

pub(super) async fn wait_for<T>(mut probe: impl AsyncFnMut() -> Option<T>) -> T {
    for _ in 0..250 {
        if let Some(value) = probe().await {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out");
}

pub(super) async fn stored(hub: &Hub, id: consort_proto::ids::ConnectionId) -> Connection {
    hub.store
        .read(move |conn| con_store::connection_or_404(conn, id))
        .await
        .unwrap()
}

/// An active Gmail connection for every channel.
pub(super) async fn active_gmail(
    hub: &Hub,
    fake: &FakeComposio,
    approvals: ToolApprovals,
) -> Connection {
    fake.set_auto_activate(true);
    let connection = connect(hub, "gmail", "Gmail Work", UseIn::EveryChannel, approvals).await;
    let id = connection.id;
    wait_for(async || {
        let c = stored(hub, id).await;
        c.status.is_active().then_some(c)
    })
    .await
}

async fn pending_card(hub: &Hub) -> consort_proto::model::Approval {
    wait_for(async || {
        hub.store
            .read(|conn| work::pending_approvals(conn, None))
            .await
            .unwrap()
            .into_iter()
            .next()
    })
    .await
}

async fn answer(hub: &Hub, approval_id: consort_proto::ids::ApprovalId, option: &str) {
    hub.approval_answer(
        &Caller::Hub,
        ApprovalAnswer {
            approval_id,
            option_id: option.to_owned(),
            checked: false,
        },
    )
    .await
    .unwrap();
}

pub(super) async fn call(hub: &Hub, session: SessionId, name: &str, args: Value) -> Value {
    let result = hub.call_tool(session, name, args).await;
    let body = result.structured_content.clone().unwrap_or(Value::Null);
    assert!(!result.is_error, "{name}: {body}");
    body
}

async fn call_error(hub: &Hub, session: SessionId, name: &str, args: Value) -> String {
    let result = hub.call_tool(session, name, args).await;
    assert!(result.is_error, "{name} worked");
    result.structured_content.unwrap()["error"]["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn wakes(hub: &Hub, channel_id: ChannelId) -> Vec<WakeEvent> {
    hub.store
        .read(move |conn| coord_store::pending_wakes(conn, channel_id))
        .await
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect()
}

#[tokio::test]
async fn an_oauth_connection_follows_the_sign_in_until_it_is_active() {
    let (hub, _dir, fake) = hub_with_fake().await;
    let attempt = hub
        .connection_connect(
            &Caller::Hub,
            ConnectionConnect {
                service: "gmail".to_owned(),
                name: "Gmail Work".to_owned(),
                use_in: UseIn::EveryChannel,
                approvals: ToolApprovals::default(),
                auth_scheme: None,
                credentials: SecretEnv::new(),
                approval_id: None,
            },
        )
        .await
        .unwrap();
    assert!(
        attempt
            .redirect_url
            .unwrap()
            .starts_with("https://connect.example/link/")
    );
    assert!(attempt.expires_at.is_some());
    assert_eq!(attempt.connection.handle, "gmail_work");
    assert!(matches!(
        attempt.connection.status,
        ConnectionStatus::Connecting { .. }
    ));
    // The user signs in.
    fake.set_status(None, "ACTIVE");
    let id = attempt.connection.id;
    let active = wait_for(async || {
        let c = stored(&hub, id).await;
        c.status.is_active().then_some(c)
    })
    .await;
    assert_eq!(active.identity.as_deref(), Some("gmail_work@example.com"));
    // A second account of the same service gets its own handle and reuses
    // the managed auth config.
    let home = connect(
        &hub,
        "gmail",
        "Gmail Work",
        UseIn::EveryChannel,
        ToolApprovals::default(),
    )
    .await;
    assert_eq!(home.handle, "gmail_work_2");
    assert_eq!(fake.count("POST /auth_configs"), 1);
    // The connection record has no Composio ID and no token.
    let json = serde_json::to_string(&active).unwrap();
    assert!(
        !json.contains("ca_") && !json.contains("secret-token"),
        "{json}"
    );
}

#[tokio::test]
async fn a_sign_in_that_never_finishes_fails() {
    let (hub, _dir, _fake) = hub_with_fake().await;
    hub.connectors.set_timings(Timings {
        connect_timeout_ms: 100,
        ..fast()
    });
    let connection = connect(
        &hub,
        "gmail",
        "Gmail",
        UseIn::EveryChannel,
        ToolApprovals::default(),
    )
    .await;
    let id = connection.id;
    let failed = wait_for(async || {
        let c = stored(&hub, id).await;
        matches!(c.status, ConnectionStatus::Failed { .. }).then_some(c)
    })
    .await;
    let ConnectionStatus::Failed { message } = failed.status else {
        unreachable!()
    };
    assert!(message.contains("10 minutes"));
}

#[tokio::test]
async fn an_api_key_connection_is_active_at_once_and_keeps_no_key() {
    let (hub, dir, fake) = hub_with_fake().await;
    let connection = connect(
        &hub,
        "sendgrid",
        "Mail API",
        UseIn::EveryChannel,
        ToolApprovals::default(),
    )
    .await;
    assert!(connection.status.is_active());
    assert_eq!(fake.count("POST /connected_accounts"), 1);
    let db = std::fs::read(dir.path().join("hub.db")).unwrap();
    assert!(
        !db.windows(SENDGRID_KEY.len())
            .any(|w| w == SENDGRID_KEY.as_bytes())
    );
    // A missing field and a wrong key are refused, and nothing stays.
    let err = hub
        .connection_connect(
            &Caller::Hub,
            ConnectionConnect {
                service: "sendgrid".to_owned(),
                name: "Other".to_owned(),
                use_in: UseIn::EveryChannel,
                approvals: ToolApprovals::default(),
                auth_scheme: Some(AuthSchemeKind::ApiKey),
                credentials: SecretEnv::new(),
                approval_id: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::Invalid);
    assert!(err.to_error_body().message.contains("API key"));
    let mut wrong = SecretEnv::new();
    wrong.insert("api_key", "SG.wrong");
    let err = hub
        .connection_connect(
            &Caller::Hub,
            ConnectionConnect {
                service: "sendgrid".to_owned(),
                name: "Other".to_owned(),
                use_in: UseIn::EveryChannel,
                approvals: ToolApprovals::default(),
                auth_scheme: None,
                credentials: wrong,
                approval_id: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ComposioError);
    let all = hub.store.read(con_store::connections).await.unwrap();
    assert_eq!(all.len(), 1);
}

#[tokio::test]
async fn disconnect_deletes_the_account_and_reconnect_links_again() {
    let (hub, _dir, fake) = hub_with_fake().await;
    let connection = active_gmail(&hub, &fake, ToolApprovals::default()).await;
    let first = fake.account_ids();
    fake.set_auto_activate(false);
    let attempt = hub
        .connection_reconnect(
            &Caller::Hub,
            ConnectionReconnect {
                connection_id: connection.id,
                credentials: SecretEnv::new(),
            },
        )
        .await
        .unwrap();
    assert!(attempt.redirect_url.is_some());
    let second = fake.account_ids();
    assert_eq!(second.len(), 1);
    assert_ne!(first, second, "the old account went first");
    hub.connection_disconnect(
        &Caller::Hub,
        ConnectionDisconnect {
            connection_id: connection.id,
        },
    )
    .await
    .unwrap();
    assert!(fake.account_ids().is_empty());
    assert!(
        hub.store
            .read(con_store::connections)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_effective_set_follows_use_in_and_channel_exceptions() {
    let (hub, _dir, fake) = hub_with_fake().await;
    fake.set_auto_activate(true);
    let work = channel(&hub, "Work", &["work"]).await;
    let home = channel(&hub, "Home", &[]).await;
    let tutti_id = tutti(&hub).await;
    let tagged = connect(
        &hub,
        "sendgrid",
        "Work Mail",
        UseIn::Tags {
            tags: vec!["work".to_owned()],
        },
        ToolApprovals::default(),
    )
    .await;
    let everywhere = connect(
        &hub,
        "sendgrid",
        "Shared Mail",
        UseIn::EveryChannel,
        ToolApprovals::default(),
    )
    .await;
    let set = |channel_id: ChannelId| {
        let hub = hub.clone();
        async move {
            hub.store
                .read(move |conn| crate::connectors::extensions::extensions_for(conn, channel_id))
                .await
                .unwrap()
                .connectors
                .into_iter()
                .map(|c| c.handle)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(set(work).await, vec!["shared_mail", "work_mail"]);
    assert_eq!(set(home).await, vec!["shared_mail"]);
    // Tutti with no tags matches only "every channel" items.
    assert_eq!(set(tutti_id).await, vec!["shared_mail"]);
    // A channel cannot turn on an item that its scope does not give it.
    let err = hub
        .channel_set_connection_enabled(
            &Caller::Hub,
            ChannelSetConnectionEnabled {
                channel_id: home,
                connection_id: tagged.id,
                enabled: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::Conflict);
    let changed = hub
        .channel_set_connection_enabled(
            &Caller::Hub,
            ChannelSetConnectionEnabled {
                channel_id: work,
                connection_id: everywhere.id,
                enabled: false,
            },
        )
        .await
        .unwrap()
        .channel;
    assert_eq!(changed.disabled_connections, vec![everywhere.id]);
    assert_eq!(set(work).await, vec!["work_mail"]);
    hub.channel_set_connection_enabled(
        &Caller::Hub,
        ChannelSetConnectionEnabled {
            channel_id: work,
            connection_id: everywhere.id,
            enabled: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(set(work).await, vec!["shared_mail", "work_mail"]);
}

#[tokio::test]
async fn connector_tools_show_only_where_a_connection_is_and_skill_tools_only_without_native_skills()
 {
    let (hub, _dir, fake) = hub_with_fake().await;
    let tutti_id = tutti(&hub).await;
    let acp = coordinator_session(&hub, tutti_id, "acp").await;
    let claude = coordinator_session(&hub, tutti_id, "claude").await;
    let names = |session| {
        let hub = hub.clone();
        async move {
            hub.tool_definitions(session)
                .await
                .unwrap()
                .into_iter()
                .map(|t| t.name)
                .collect::<Vec<_>>()
        }
    };
    let before = names(acp).await;
    assert!(!before.contains(&"run_connector_tool".to_owned()));
    assert!(before.contains(&"list_skills".to_owned()));
    assert!(before.contains(&"list_connections".to_owned()));
    assert!(before.contains(&"add_skill".to_owned()));
    assert!(before.contains(&"search_skills".to_owned()));
    assert!(!names(claude).await.contains(&"read_skill".to_owned()));
    active_gmail(&hub, &fake, ToolApprovals::default()).await;
    assert!(names(acp).await.contains(&"run_connector_tool".to_owned()));
}

#[tokio::test]
async fn a_read_tool_runs_at_once_with_the_exact_account_and_an_audit_record() {
    let (hub, _dir, fake) = hub_with_fake().await;
    let connection = active_gmail(&hub, &fake, ToolApprovals::default()).await;
    let session = coordinator_session(&hub, tutti(&hub).await, "acp").await;
    let listed = call(
        &hub,
        session,
        "list_connector_tools",
        json!({"connector": "gmail_work", "search": "fetch"}),
    )
    .await;
    assert_eq!(listed["tools"].as_array().unwrap().len(), 1);
    assert_eq!(listed["tools"][0]["kind"], "read");
    assert_eq!(listed["tools"][0]["approval"], "run_without_asking");
    let schema = call(
        &hub,
        session,
        "get_connector_tool",
        json!({"connector": "gmail_work", "tool": "GMAIL_FETCH_EMAILS"}),
    )
    .await;
    assert_eq!(schema["input_schema"]["type"], "object");
    let arguments = json!({"q": "from:boss", "max": 5});
    let ran = call(
        &hub,
        session,
        "run_connector_tool",
        json!({"connector": "GMAIL_WORK", "tool": "gmail_fetch_emails", "arguments": arguments}),
    )
    .await;
    assert_eq!(ran["status"], "ran");
    assert_eq!(ran["successful"], true);
    let account = fake.account_ids()[0].clone();
    assert_eq!(ran["data"]["account"], account.as_str());
    let executions = fake.executions();
    assert_eq!(executions[0].1["connected_account_id"], account.as_str());
    let calls = hub
        .connection_calls(
            &Caller::Hub,
            ConnectionCalls {
                connection_id: connection.id,
                before: None,
                limit: None,
            },
        )
        .await
        .unwrap()
        .calls;
    assert_eq!(calls.len(), 1);
    let record = &calls[0];
    assert_eq!(record.decision, CallDecision::RanWithoutAsking);
    assert_eq!(record.outcome, CallOutcome::Succeeded);
    assert_eq!(record.tool_kind, ToolKind::Read);
    assert_eq!(record.composio_log_id.as_deref(), Some("log_1"));
    assert_eq!(
        record.arguments_sha256,
        crate::util::sha256(canonical_json(&arguments).as_bytes())
    );
    assert!(stored(&hub, connection.id).await.last_used_at.is_some());
    // A big result is cut.
    let big = call(
        &hub,
        session,
        "run_connector_tool",
        json!({"connector": "gmail_work", "tool": "GMAIL_BIG_EXPORT"}),
    )
    .await;
    assert_eq!(big["truncated"], true);
}

#[tokio::test]
async fn clear_errors_for_missing_connectors_keys_and_sign_ins() {
    let (hub, _dir, fake) = hub_with_fake().await;
    let connection = active_gmail(&hub, &fake, ToolApprovals::default()).await;
    let session = coordinator_session(&hub, tutti(&hub).await, "acp").await;
    assert_eq!(
        call_error(
            &hub,
            session,
            "run_connector_tool",
            json!({"connector": "slack", "tool": "X"})
        )
        .await,
        "connector_not_available"
    );
    // A connection outside the set is "not available", also when it exists.
    let home = channel(&hub, "Home", &[]).await;
    hub.update_connection_fields(
        connection.id,
        None,
        Some(UseIn::Channels {
            channel_ids: vec![home],
        }),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        call_error(
            &hub,
            session,
            "list_connector_tools",
            json!({"connector": "gmail_work"})
        )
        .await,
        "connector_not_available"
    );
    hub.update_connection_fields(connection.id, None, Some(UseIn::EveryChannel), None)
        .await
        .unwrap();
    // Composio says the sign-in expired.
    fake.set_status(None, "EXPIRED");
    assert_eq!(
        call_error(
            &hub,
            session,
            "run_connector_tool",
            json!({"connector": "gmail_work", "tool": "GMAIL_FETCH_EMAILS"})
        )
        .await,
        "needs_user"
    );
    assert!(matches!(
        stored(&hub, connection.id).await.status,
        ConnectionStatus::NeedsSignIn { .. }
    ));
    // Still in the context, with the hint.
    let extensions = hub
        .store
        .read(|conn| {
            let id = conversation::tutti_channel(conn)?.id;
            crate::connectors::extensions::extensions_for(conn, id)
        })
        .await
        .unwrap();
    assert!(extensions.connectors[0].needs_sign_in);
    hub.composio_remove_key(&Caller::Hub, Default::default())
        .await
        .unwrap();
    fake.set_status(None, "ACTIVE");
    hub.set_connection_status(connection.id, ConnectionStatus::Active, None, false)
        .await
        .unwrap();
    assert_eq!(
        call_error(
            &hub,
            session,
            "run_connector_tool",
            json!({"connector": "gmail_work", "tool": "GMAIL_FETCH_EMAILS"})
        )
        .await,
        "composio_not_configured"
    );
}

#[tokio::test]
async fn a_change_asks_with_a_readable_card_and_runs_after_allow() {
    let (hub, _dir, fake) = hub_with_fake().await;
    active_gmail(&hub, &fake, ToolApprovals::default()).await;
    let session = coordinator_session(&hub, tutti(&hub).await, "acp").await;
    let running = {
        let hub = hub.clone();
        tokio::spawn(async move {
            hub.call_tool(
                session,
                "run_connector_tool",
                json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL",
                       "arguments": {"recipient_email": "a@example.com", "subject": "Hi", "body": "Hello"}}),
            )
            .await
        })
    };
    let card = pending_card(&hub).await;
    assert!(card.is_hub_card());
    assert_eq!(card.request_id.as_str(), format!("hub:{}", card.id));
    let ApprovalDetails::Connector(details) = &card.details else {
        panic!("not a connector card");
    };
    let fields: Vec<(&str, &str)> = details
        .fields
        .iter()
        .map(|f| (f.label.as_str(), f.value.as_str()))
        .collect();
    assert_eq!(
        fields,
        vec![
            ("To", "a@example.com"),
            ("Subject", "Hi"),
            ("Body", "Hello")
        ]
    );
    assert!(
        fake.executions().is_empty(),
        "nothing runs before the answer"
    );
    answer(&hub, card.id, "allow").await;
    let result = running.await.unwrap();
    let body = result.structured_content.unwrap();
    assert_eq!(body["status"], "ran", "{body}");
    assert_eq!(fake.executions().len(), 1);
    // The tool call got the answer, so no card_answered event is queued.
    assert!(wakes(&hub, card.channel_id).await.is_empty());

    // Don't returns denied, and nothing runs.
    let running = {
        let hub = hub.clone();
        tokio::spawn(async move {
            hub.call_tool(session, "run_connector_tool", json!({"connector": "gmail_work", "tool": "GMAIL_DELETE_MESSAGE", "arguments": {"id": "1"}}))
                .await
        })
    };
    let card = pending_card(&hub).await;
    answer(&hub, card.id, "deny").await;
    let body = running.await.unwrap().structured_content.unwrap();
    assert_eq!(body["status"], "denied");
    assert_eq!(fake.executions().len(), 1);
}

#[tokio::test]
async fn ask_once_per_task_asks_only_the_first_time() {
    let (hub, _dir, fake) = hub_with_fake().await;
    active_gmail(
        &hub,
        &fake,
        ToolApprovals {
            change: ApprovalRule::AskOncePerTask,
            ..ToolApprovals::default()
        },
    )
    .await;
    let (task, session) = test_task_session(&hub).await;
    let session_id = session.id;
    let running = {
        let hub = hub.clone();
        tokio::spawn(async move {
            hub.call_tool(
                session_id,
                "run_connector_tool",
                json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL", "arguments": {}}),
            )
            .await
        })
    };
    let card = pending_card(&hub).await;
    assert_eq!(card.task_id, Some(task.id));
    let waiting = hub
        .store
        .read(move |conn| work::task_or_404(conn, task.id))
        .await
        .unwrap();
    assert_eq!(
        waiting.status,
        consort_proto::status::TaskStatus::NeedsInput
    );
    answer(&hub, card.id, "allow").await;
    assert_eq!(
        running.await.unwrap().structured_content.unwrap()["status"],
        "ran"
    );
    let again = call(
        &hub,
        session_id,
        "run_connector_tool",
        json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL", "arguments": {}}),
    )
    .await;
    assert_eq!(again["status"], "ran");
    let calls = hub
        .store
        .read(|conn| {
            let c = con_store::connections(conn)?;
            con_store::connector_calls(conn, c[0].id, None, 10)
        })
        .await
        .unwrap()
        .0;
    assert_eq!(calls[0].decision, CallDecision::AllowedEarlierInTask);
    assert_eq!(calls[1].decision, CallDecision::AllowedByUser);
}

#[tokio::test]
async fn a_late_answer_runs_the_call_and_wakes_the_coordinator() {
    let (hub, _dir, fake) = hub_with_fake().await;
    hub.connectors.set_timings(Timings {
        approval_wait: Duration::from_millis(50),
        ..fast()
    });
    active_gmail(&hub, &fake, ToolApprovals::default()).await;
    let tutti_id = tutti(&hub).await;
    let session = coordinator_session(&hub, tutti_id, "acp").await;
    let waiting = call(&hub, session, "run_connector_tool", json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL", "arguments": {"to": "a@example.com"}})).await;
    assert_eq!(waiting["status"], "waiting_for_user");
    let card = pending_card(&hub).await;
    assert_eq!(waiting["approval_id"], card.id.to_string());
    answer(&hub, card.id, "allow").await;
    let events = wait_for(async || {
        let events = wakes(&hub, tutti_id).await;
        (!events.is_empty()).then_some(events)
    })
    .await;
    let WakeEvent::CardAnswered {
        id, outcome, text, ..
    } = &events[0]
    else {
        panic!("not a card answer: {events:?}");
    };
    assert_eq!(*id, card.id);
    assert_eq!(*outcome, ApprovalStatus::Allowed);
    assert!(text.contains("ran"), "{text}");
    assert_eq!(fake.executions().len(), 1);
}

#[tokio::test]
async fn management_changes_follow_the_risk_rule() {
    let (hub, _dir, fake) = hub_with_fake().await;
    let connection = connect(
        &hub,
        "sendgrid",
        "Work Mail",
        UseIn::Tags {
            tags: vec!["work".to_owned()],
        },
        ToolApprovals::default(),
    )
    .await;
    let _ = fake;
    let tutti_id = tutti(&hub).await;
    for name in ["One", "Two", "Three"] {
        channel(&hub, name, &[]).await;
    }
    let session = coordinator_session(&hub, tutti_id, "acp").await;
    // Wider: a management card with the effect.
    let asked = call(
        &hub,
        session,
        "update_connection",
        json!({"connection": "work_mail", "use_in": {"kind": "every_channel"}}),
    )
    .await;
    assert_eq!(asked["status"], "waiting_for_user");
    let card = pending_card(&hub).await;
    let ApprovalDetails::Management(details) = &card.details else {
        panic!("not a management card");
    };
    assert_eq!(
        details.effect,
        "Work Mail becomes available in 4 more channels."
    );
    assert_eq!(
        stored(&hub, connection.id).await.use_in,
        UseIn::Tags {
            tags: vec!["work".to_owned()]
        }
    );
    answer(&hub, card.id, "allow").await;
    wait_for(async || {
        (stored(&hub, connection.id).await.use_in == UseIn::EveryChannel).then_some(())
    })
    .await;
    let events = wait_for(async || {
        let events = wakes(&hub, tutti_id).await;
        (!events.is_empty()).then_some(events)
    })
    .await;
    assert!(matches!(
        &events[0],
        WakeEvent::CardAnswered {
            outcome: ApprovalStatus::Allowed,
            ..
        }
    ));
    // Stricter: runs at once with an activity line.
    let applied = call(
        &hub,
        session,
        "update_connection",
        json!({"connection": "work_mail", "approvals": {"read": "ask_every_time", "change": "ask_every_time", "delete": "ask_every_time"}}),
    )
    .await;
    assert_eq!(applied["status"], "applied");
    let notices = hub
        .store
        .read(move |conn| conversation::messages_page(conn, tutti_id, None, None, 50, usize::MAX))
        .await
        .unwrap()
        .0;
    assert!(
        notices
            .iter()
            .any(|m| m.kind == MessageKind::Notice && m.body.contains("assistant_change"))
    );
    // Disconnect asks first; Don't keeps the connection.
    let asked = call(
        &hub,
        session,
        "disconnect",
        json!({"connection": "work_mail"}),
    )
    .await;
    assert_eq!(asked["status"], "waiting_for_user");
    let card = pending_card(&hub).await;
    answer(&hub, card.id, "deny").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        hub.store.read(con_store::connections).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn a_channel_assistant_manages_only_its_own_channel() {
    let (hub, _dir, fake) = hub_with_fake().await;
    let _ = fake;
    let work = channel(&hub, "Work", &["work"]).await;
    let other = channel(&hub, "Other", &[]).await;
    connect(
        &hub,
        "sendgrid",
        "Work Mail",
        UseIn::Tags {
            tags: vec!["work".to_owned()],
        },
        ToolApprovals::default(),
    )
    .await;
    let private = connect(
        &hub,
        "sendgrid",
        "Private",
        UseIn::Channels {
            channel_ids: vec![other],
        },
        ToolApprovals::default(),
    )
    .await;
    let session = coordinator_session(&hub, work, "acp").await;
    let listed = call(&hub, session, "list_connections", json!({})).await;
    let handles: Vec<&str> = listed["connections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["handle"].as_str().unwrap())
        .collect();
    assert_eq!(handles, vec!["work_mail"]);
    // Turning one off runs at once.
    let off = call(
        &hub,
        session,
        "set_channel_connector",
        json!({"connection": "work_mail", "enabled": false}),
    )
    .await;
    assert_eq!(off["status"], "applied");
    let listed = call(&hub, session, "list_connections", json!({})).await;
    assert_eq!(listed["off_in_this_channel"].as_array().unwrap().len(), 1);
    // One that the scope does not give it is refused; request_access asks.
    let refused = call(
        &hub,
        session,
        "set_channel_connector",
        json!({"connection": "private", "enabled": true}),
    )
    .await;
    assert_eq!(refused["status"], "refused");
    let asked = call(
        &hub,
        session,
        "request_access",
        json!({"connection": "private"}),
    )
    .await;
    assert_eq!(asked["status"], "waiting_for_user");
    let card = pending_card(&hub).await;
    answer(&hub, card.id, "allow").await;
    wait_for(async || {
        let c = stored(&hub, private.id).await;
        (c.use_in
            == UseIn::Channels {
                channel_ids: vec![other, work],
            })
        .then_some(())
    })
    .await;
    // The Tutti tools are not there.
    assert_eq!(
        call_error(
            &hub,
            session,
            "archive_channel",
            json!({"channel": other.to_string()})
        )
        .await,
        "policy_denied"
    );
    let tags = call(
        &hub,
        session,
        "set_channel_tags",
        json!({"tags": ["work", "mail"]}),
    )
    .await;
    assert_eq!(tags["status"], "waiting_for_user");
}

#[tokio::test]
async fn channel_tools_create_at_once_and_ask_before_tags_and_archive() {
    let (hub, _dir) = test_hub().await;
    let tutti_id = tutti(&hub).await;
    let session = coordinator_session(&hub, tutti_id, "acp").await;
    let made = call(
        &hub,
        session,
        "create_channel",
        json!({"name": "Research", "purpose": "Read papers", "group": "Projects"}),
    )
    .await;
    assert_eq!(made["status"], "applied");
    let id: ChannelId = made["channel_id"].as_str().unwrap().parse().unwrap();
    let created = hub
        .store
        .read(move |conn| conversation::channel_or_404(conn, id))
        .await
        .unwrap();
    assert!(created.group_id.is_some());
    assert_eq!(created.kind, ChannelKind::Standard);
    let tagged = call(
        &hub,
        session,
        "create_channel",
        json!({"name": "Tagged", "purpose": "x", "tags": ["work"]}),
    )
    .await;
    assert_eq!(tagged["status"], "waiting_for_user");
    let archive = call(
        &hub,
        session,
        "archive_channel",
        json!({"channel": id.to_string()}),
    )
    .await;
    assert_eq!(archive["status"], "waiting_for_user");
    let renamed = call(
        &hub,
        session,
        "update_channel",
        json!({"channel": id.to_string(), "name": "Papers"}),
    )
    .await;
    assert_eq!(renamed["status"], "applied");
}

#[tokio::test]
async fn connect_service_posts_a_card_and_the_answer_connects() {
    let (hub, _dir, fake) = hub_with_fake().await;
    let work = channel(&hub, "Work", &["work"]).await;
    let session = coordinator_session(&hub, work, "acp").await;
    let asked = call(
        &hub,
        session,
        "connect_service",
        json!({"service": "gmail", "name": "Gmail Work"}),
    )
    .await;
    assert_eq!(asked["status"], "waiting_for_user");
    let card = pending_card(&hub).await;
    let ApprovalDetails::ConnectService(details) = &card.details else {
        panic!("not a connect card");
    };
    assert_eq!(
        details.use_in,
        UseIn::Tags {
            tags: vec!["work".to_owned()]
        }
    );
    assert!(fake.account_ids().is_empty());
    answer(&hub, card.id, "allow").await;
    let events = wait_for(async || {
        let events = wakes(&hub, work).await;
        (!events.is_empty()).then_some(events)
    })
    .await;
    let WakeEvent::CardAnswered { text, .. } = &events[0] else {
        panic!("not a card answer");
    };
    assert!(text.contains("gmail_work"), "{text}");
    let card = hub
        .store
        .read(move |conn| work::approval(conn, card.id))
        .await
        .unwrap()
        .unwrap();
    let ApprovalDetails::ConnectService(details) = &card.details else {
        unreachable!()
    };
    assert!(details.connection_id.is_some());
    assert_eq!(fake.account_ids().len(), 1);
}

#[tokio::test]
async fn skills_reach_the_session_and_the_worker_that_was_told() {
    let (hub, _dir) = test_hub().await;
    let skill = hub
        .store_skill(crate::skills::test_skill("notes", false))
        .await
        .unwrap();
    let tutti_id = tutti(&hub).await;
    let session = coordinator_session(&hub, tutti_id, "acp").await;
    let listed = call(&hub, session, "list_skills", json!({})).await;
    assert_eq!(listed["skills"][0]["name"], "notes");
    let read = call(&hub, session, "read_skill", json!({"skill": "notes"})).await;
    assert!(read["content"].as_str().unwrap().contains("# notes"));
    assert_eq!(read["files"], json!(["reference/notes.md"]));
    let other = call(
        &hub,
        session,
        "read_skill",
        json!({"skill": "notes", "path": "reference/notes.md"}),
    )
    .await;
    assert_eq!(other["content"], "Some notes.");
    // A worker reads only a bundle that a session.start named to it.
    let worker = WorkerId::new();
    let hash = skill.content_hash.clone();
    let err = hub
        .skill_bundle_get(
            worker,
            consort_proto::worker_link::SkillBundleGet {
                content_hash: hash.clone(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
    let extensions = hub
        .store
        .write(move |tx| tx.session_extensions(worker, tutti_id))
        .await
        .unwrap();
    assert_eq!(extensions.skills[0].content_hash, hash);
    assert_eq!(
        extensions.skill_context[0].description,
        "Does notes things. Use it for tests."
    );
    let bundle = hub
        .skill_bundle_get(
            worker,
            consort_proto::worker_link::SkillBundleGet { content_hash: hash },
        )
        .await
        .unwrap();
    assert!(bundle.is_consistent());
    // Turned off in the channel, the skill leaves the set.
    hub.channel_set_skill_enabled(
        &Caller::Hub,
        consort_proto::client_api::ChannelSetSkillEnabled {
            channel_id: tutti_id,
            skill_id: skill.id,
            enabled: false,
        },
    )
    .await
    .unwrap();
    let listed = call(&hub, session, "list_skills", json!({})).await;
    assert!(listed["skills"].as_array().unwrap().is_empty());
    // The management tools.
    let installed = call(&hub, session, "list_installed_skills", json!({})).await;
    assert_eq!(installed["off_in_this_channel"][0], skill.id.to_string());
    let removal = call(&hub, session, "remove_skill", json!({"skill": "notes"})).await;
    assert_eq!(removal["status"], "waiting_for_user");
}

#[tokio::test]
async fn a_late_answer_in_a_task_reaches_the_task_as_a_message() {
    let (hub, _dir, fake) = hub_with_fake().await;
    hub.connectors.set_timings(Timings {
        approval_wait: Duration::from_millis(50),
        ..fast()
    });
    active_gmail(&hub, &fake, ToolApprovals::default()).await;
    let (task, session) = test_task_session(&hub).await;
    let waiting = call(
        &hub,
        session.id,
        "run_connector_tool",
        json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL", "arguments": {}}),
    )
    .await;
    assert_eq!(waiting["status"], "waiting_for_user");
    let card = pending_card(&hub).await;
    answer(&hub, card.id, "allow").await;
    let session_id = session.id;
    let text = wait_for(async || {
        let (inbox, commands) = hub
            .store
            .read(move |conn| {
                Ok((
                    work::task_inbox(conn, task.id)?,
                    crate::store::machine::commands_for_session(conn, session_id)?,
                ))
            })
            .await
            .unwrap();
        inbox
            .into_iter()
            .map(|m| m.text)
            .chain(commands.into_iter().map(|(c, _)| c.body.to_string()))
            .find(|t| t.contains("card_answered"))
    })
    .await;
    assert!(
        text.contains("outcome=\\\"allowed\\\"") || text.contains("outcome=\"allowed\""),
        "{text}"
    );
    assert_eq!(fake.executions().len(), 1);
}

#[tokio::test]
async fn worker_and_account_tools_ask_where_the_rule_says() {
    use consort_proto::model::{
        AuthState, Capabilities, HarnessAccount, Inventory, Os, Worker, WorkerLoad, WorkerPolicy,
        WorkerState,
    };
    let (hub, _dir) = test_hub().await;
    let account_id = AccountId::new();
    let worker = Worker {
        id: WorkerId::new(),
        name: "Laptop".to_owned(),
        hostname: "laptop".to_owned(),
        os: Os::Macos,
        arch: "aarch64".to_owned(),
        public_key: consort_proto::auth::PublicKey::from_bytes([1; 32]),
        version: String::new(),
        protocol_version: 1,
        tags: Vec::new(),
        paused: false,
        state: WorkerState::Online,
        last_seen_at: None,
        inventory: Some(Inventory {
            hash: String::new(),
            hostname: "laptop".to_owned(),
            os: Os::Macos,
            arch: "aarch64".to_owned(),
            tags: Vec::new(),
            accounts: vec![HarnessAccount {
                account_id,
                harness: "claude".to_owned(),
                label: "Claude Work".to_owned(),
                harness_version: None,
                auth_state: AuthState::SignedOut,
                auth_detail: None,
                models: Vec::new(),
                capabilities: Capabilities::default(),
                enabled: true,
                profile: false,
                email: None,
                plan: None,
                secret_names: Vec::new(),
                sign_in_methods: vec![consort_proto::model::SignInMethod::ClaudePlan],
                extras_enabled: false,
                extras: None,
            }],
            local_mcp_servers: Vec::new(),
            checkouts: Vec::new(),
            policy: WorkerPolicy {
                max_approval_mode: consort_proto::model::ApprovalMode::Ask,
                allowed_roots: Vec::new(),
                allow_shell: true,
            },
            load: WorkerLoad::default(),
            tools: Vec::new(),
        }),
        paired_at: Timestamp::now(),
        revoked_at: None,
    };
    let worker_id = worker.id;
    hub.store
        .write(move |tx| tx.save_worker(&worker))
        .await
        .unwrap();
    let tutti_id = tutti(&hub).await;
    let session = coordinator_session(&hub, tutti_id, "acp").await;
    let tags = call(
        &hub,
        session,
        "set_worker_tags",
        json!({"worker": worker_id.to_string(), "tags": ["gpu"]}),
    )
    .await;
    assert_eq!(tags["status"], "waiting_for_user");
    let extras = call(&hub, session, "set_account_extras", json!({"worker": worker_id.to_string(), "account": account_id.to_string(), "enabled": true})).await;
    assert_eq!(extras["status"], "waiting_for_user");
    let sign_in = call(&hub, session, "start_sign_in", json!({"worker": worker_id.to_string(), "account": account_id.to_string(), "method": "claude_plan"})).await;
    assert_eq!(sign_in["status"], "waiting_for_user");
    let cards = hub
        .store
        .read(|conn| work::pending_approvals(conn, None))
        .await
        .unwrap();
    let card = cards
        .iter()
        .find(|c| matches!(c.details, ApprovalDetails::SignIn(_)))
        .unwrap();
    let ApprovalDetails::SignIn(details) = &card.details else {
        unreachable!()
    };
    assert_eq!(details.account_label, "Claude Work");
    assert_eq!(
        details.method,
        Some(consort_proto::model::SignInMethod::ClaudePlan)
    );
    // Not now: the coordinator hears about it.
    answer(&hub, card.id, "deny").await;
    let events = wait_for(async || {
        let events = wakes(&hub, tutti_id).await;
        (!events.is_empty()).then_some(events)
    })
    .await;
    assert!(matches!(
        &events[0],
        WakeEvent::CardAnswered {
            outcome: ApprovalStatus::Denied,
            ..
        }
    ));
    // The default model changes at once.
    let model = call(
        &hub,
        session,
        "set_default_model",
        json!({"model": {"account_label": "Claude Work", "model": "opus", "options": {}}}),
    )
    .await;
    assert_eq!(model["status"], "applied");
    let tutti_channel = hub.store.read(conversation::tutti_channel).await.unwrap();
    assert_eq!(tutti_channel.coordinator_selection.unwrap().model, "opus");
}
