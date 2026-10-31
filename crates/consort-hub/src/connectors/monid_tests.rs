//! The Monid gate and the whole flow of a call, with a fake Monid server.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use consort_proto::ids::{ChannelId, SessionId};
use consort_proto::model::{ChannelFile, Message, UseIn};

use super::*;
use crate::connectors::Timings;
use crate::connectors::flow_tests::{answer, coordinator_session, pending_card, tutti, wait_for};
use crate::hub::{test_hub, test_task_session};

pub(crate) const GOOD_KEY: &str = "monid_live_test_3f9a";

#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) url: Mutex<String>,
    pub(crate) starts: AtomicUsize,
    stops: AtomicUsize,
    /// A slow run ends when this is true.
    finish_slow: AtomicBool,
    inputs: Mutex<Vec<Value>>,
    run_endpoints: Mutex<Vec<String>>,
}

pub(crate) type Shared = Arc<Fake>;

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == format!("Bearer {GOOD_KEY}"))
}

fn refused() -> Response {
    (StatusCode::UNAUTHORIZED, axum::Json(json!({}))).into_response()
}

fn price_of(endpoint: &str) -> Value {
    if endpoint == "/per-result" {
        json!({"type": "PER_RESULT", "amount": {"value": 0.0016, "currency": "USD"}})
    } else {
        json!({"type": "PER_CALL", "amount": {"value": 0.015, "currency": "USD"}})
    }
}

async fn discover(headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return refused();
    }
    axum::Json(json!({"results": [
        {"provider": "acme", "providerName": "Acme", "endpoint": "/fast", "summary": "A fast tool. It has a long text.", "price": price_of("/fast")},
        {"provider": "other", "providerName": "Other", "endpoint": "/fast", "summary": "A tool of another provider", "price": price_of("/fast")},
    ], "count": 2}))
    .into_response()
}

async fn inspect(headers: HeaderMap, axum::Json(body): axum::Json<Value>) -> Response {
    if !authorized(&headers) {
        return refused();
    }
    let endpoint = body["endpoint"].as_str().unwrap_or_default();
    axum::Json(json!({
        "provider": body["provider"],
        "providerName": "Acme",
        "endpoint": endpoint,
        "summary": "A fast tool",
        "description": "A fast tool, with more words.",
        "price": price_of(endpoint),
        "input": {"body": {"type": "object"}},
    }))
    .into_response()
}

async fn start(
    State(fake): State<Shared>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    if !authorized(&headers) {
        return refused();
    }
    let endpoint = body["endpoint"].as_str().unwrap_or_default().to_owned();
    if endpoint == "/poor" {
        return (StatusCode::PAYMENT_REQUIRED, axum::Json(json!({}))).into_response();
    }
    if endpoint == "/bad-input" {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"message": "prompt is required"})),
        )
            .into_response();
    }
    fake.starts.fetch_add(1, Ordering::SeqCst);
    fake.inputs.lock().unwrap().push(body["input"].clone());
    let name = endpoint.trim_start_matches('/').to_owned();
    fake.run_endpoints.lock().unwrap().push(name.clone());
    axum::Json(json!({"runId": format!("run{name}"), "status": "RUNNING"})).into_response()
}

async fn run(State(fake): State<Shared>, Path(id): Path<String>) -> Response {
    let cost = json!({"value": 0.015, "currency": "USD"});
    let zero = json!({"value": 0, "currency": "USD"});
    let answer = match id.trim_start_matches("run") {
        "slow" if !fake.finish_slow.load(Ordering::SeqCst) => {
            json!({"runId": id, "status": "RUNNING", "stoppable": true})
        }
        "providererror" => json!({"runId": id, "status": "COMPLETED", "cost": zero,
            "providerResponse": {"httpStatus": 500}, "output": {}}),
        "blocked" => json!({"runId": id, "status": "BLOCKED", "cost": zero}),
        "image" => {
            let url = format!("{}/files/picture.png", fake.url.lock().unwrap());
            json!({"runId": id, "status": "COMPLETED", "cost": cost,
                "output": {"data": {"image_urls": [url]}}})
        }
        "huge" => json!({"runId": id, "status": "COMPLETED", "cost": cost,
            "output": {"text": "x".repeat(MONID_RESULT_MAX_BYTES + 10)}}),
        _ => json!({"runId": id, "status": "COMPLETED", "cost": cost,
            "providerResponse": {"httpStatus": 200}, "output": {"answer": 42}}),
    };
    axum::Json(answer).into_response()
}

async fn stop(State(fake): State<Shared>) -> Response {
    fake.stops.fetch_add(1, Ordering::SeqCst);
    axum::Json(json!({})).into_response()
}

async fn balance(headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return refused();
    }
    axum::Json(json!({"balance": {"value": 49.34}, "held": {"value": 0}})).into_response()
}

async fn file() -> Response {
    ([("content-type", "image/png")], vec![1_u8, 2, 3]).into_response()
}

pub(crate) async fn start_fake() -> Shared {
    let fake: Shared = Arc::default();
    let app = Router::new()
        .route("/v1/discover", post(discover))
        .route("/v1/inspect", post(inspect))
        .route("/v1/run", post(start))
        .route("/v1/runs/{id}", get(run))
        .route("/v1/runs/{id}/stop", post(stop))
        .route("/v1/wallet/balance", get(balance))
        .route("/files/{name}", get(file))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    *fake.url.lock().unwrap() = format!("http://{addr}");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    fake
}

pub(crate) fn fast() -> Timings {
    Timings {
        approval_wait: Duration::from_secs(5),
        poll_interval: Duration::from_millis(10),
        connect_timeout_ms: 60_000,
        request_timeout: None,
    }
}

pub(crate) async fn hub_with_monid() -> (Hub, tempfile::TempDir, Shared) {
    let (hub, dir) = test_hub().await;
    let fake = start_fake().await;
    hub.connectors
        .set_monid_test_server(&fake.url.lock().unwrap().clone());
    hub.connectors.set_timings(fast());
    hub.monid_set_key(
        &Caller::Hub,
        MonidSetKey {
            api_key: GOOD_KEY.to_owned(),
            settings: Some(MonidSettings {
                use_in: UseIn::EveryChannel,
                ..Default::default()
            }),
        },
    )
    .await
    .unwrap();
    (hub, dir, fake)
}

async fn set_settings(hub: &Hub, change: impl FnOnce(&mut MonidSettings)) {
    let mut settings = hub.store.read(store::settings).await.unwrap();
    change(&mut settings);
    hub.monid_update_settings(&Caller::Hub, MonidUpdateSettings { settings })
        .await
        .unwrap();
}

pub(crate) async fn set_access(hub: &Hub, channel_id: ChannelId, access: MonidChannelAccess) {
    hub.channel_set_monid(&Caller::Hub, ChannelSetMonid { channel_id, access })
        .await
        .unwrap();
}

async fn runs_without_asking_here(hub: &Hub, channel_id: ChannelId) {
    let access = MonidChannelAccess {
        approval: Some(ApprovalRule::RunWithoutAsking),
        ..Default::default()
    };
    set_access(hub, channel_id, access).await;
}

async fn call(hub: &Hub, session: SessionId, endpoint: &str) -> Value {
    hub.call_tool(
        session,
        "run_monid_tool",
        json!({"provider": "acme", "endpoint": endpoint, "body": {"text": "hello"}}),
    )
    .await
    .structured_content
    .unwrap_or_else(|| panic!("the tool failed for {endpoint}"))
}

fn spawn_call(
    hub: &Hub,
    session: SessionId,
    endpoint: &'static str,
) -> tokio::task::JoinHandle<Value> {
    let hub = hub.clone();
    tokio::spawn(async move { call(&hub, session, endpoint).await })
}

async fn calls(hub: &Hub, channel_id: ChannelId) -> Vec<MonidCall> {
    hub.store
        .read(move |conn| store::calls_in_channel(conn, channel_id, 100))
        .await
        .unwrap()
}

async fn row_of(hub: &Hub, call: &MonidCall) -> Message {
    let id = call.message_id.unwrap();
    hub.store
        .read(move |conn| conversation::message(conn, id))
        .await
        .unwrap()
        .unwrap()
}

pub(crate) async fn set_up() -> (Hub, tempfile::TempDir, Shared, ChannelId, SessionId) {
    let (hub, dir, fake) = hub_with_monid().await;
    let channel_id = tutti(&hub).await;
    let session = coordinator_session(&hub, channel_id, "acp").await;
    (hub, dir, fake, channel_id, session)
}

fn facts() -> GateFacts {
    GateFacts {
        access: MonidChannelAccess::default(),
        settings: MonidSettings::default(),
        unattended: None,
        full_access: false,
        granted: false,
        over_limit: None,
    }
}

fn asks(rule: ApprovalRule) -> Gate {
    Gate::Ask {
        rule: Some(rule),
        over_limit: None,
    }
}

#[test]
fn a_new_channel_asks_at_each_call() {
    assert_eq!(
        decide(&facts(), "a", "/b"),
        asks(ApprovalRule::AskEveryTime)
    );
}

#[test]
fn the_first_step_that_matches_decides() {
    let blocked = MonidChannelAccess {
        blocked: vec![MonidToolRef {
            provider: "a".to_owned(),
            endpoint: None,
            name: String::new(),
        }],
        ..Default::default()
    };
    // A block wins over Full access.
    let gate = decide(
        &GateFacts {
            access: blocked,
            full_access: true,
            ..facts()
        },
        "a",
        "/b",
    );
    assert_eq!(gate, Gate::Refuse(REFUSED_TEXT.to_owned()));
    // Off wins over Full access.
    let gate = decide(
        &GateFacts {
            unattended: Some(MonidUnattended::Off),
            full_access: true,
            ..facts()
        },
        "a",
        "/b",
    );
    assert!(matches!(gate, Gate::Refuse(_)));
    // A limit asks, also in Full access and with a grant.
    let gate = decide(
        &GateFacts {
            full_access: true,
            granted: true,
            over_limit: Some("over".to_owned()),
            ..facts()
        },
        "a",
        "/b",
    );
    assert_eq!(
        gate,
        Gate::Ask {
            rule: None,
            over_limit: Some("over".to_owned())
        }
    );
    // Full access, a grant and an always-allowed tool run.
    for run in [
        GateFacts {
            full_access: true,
            ..facts()
        },
        GateFacts {
            granted: true,
            ..facts()
        },
        GateFacts {
            access: MonidChannelAccess {
                allowed: vec![MonidAllowedTool {
                    tool: MonidToolRef {
                        provider: "a".to_owned(),
                        endpoint: Some("/b".to_owned()),
                        name: String::new(),
                    },
                    run_without_asking: true,
                }],
                ..Default::default()
            },
            ..facts()
        },
    ] {
        assert_eq!(decide(&run, "a", "/b"), Gate::Run);
    }
}

#[test]
fn unattended_work_uses_its_own_setting() {
    let mut channel_runs = facts();
    channel_runs.access.approval = Some(ApprovalRule::RunWithoutAsking);
    assert_eq!(decide(&channel_runs, "a", "/b"), Gate::Run);
    // The channel's loose rule does not reach a routine that asks.
    channel_runs.unattended = Some(MonidUnattended::Ask);
    assert_eq!(
        decide(&channel_runs, "a", "/b"),
        asks(ApprovalRule::AskEveryTime)
    );
    let run = GateFacts {
        unattended: Some(MonidUnattended::RunWithoutAsking),
        ..facts()
    };
    assert_eq!(decide(&run, "a", "/b"), Gate::Run);
}

#[test]
fn ask_once_per_task_keeps_its_rule_on_the_card() {
    let mut once = facts();
    once.settings.approval = ApprovalRule::AskOncePerTask;
    assert_eq!(decide(&once, "a", "/b"), asks(ApprovalRule::AskOncePerTask));
    once.access.approval = Some(ApprovalRule::AskEveryTime);
    assert_eq!(decide(&once, "a", "/b"), asks(ApprovalRule::AskEveryTime));
}

#[test]
fn no_limit_is_ever_over() {
    let spent = Spent {
        task: Some(i64::MAX / 2),
        channel_month: i64::MAX / 2,
        hub_month: i64::MAX / 2,
    };
    assert_eq!(over_limit_note(&MonidLimits::default(), None, spent), None);
}

#[test]
fn the_limit_note_says_which_limit_and_why() {
    let limits = MonidLimits {
        per_call_micros: Some(1_000_000),
        per_task_micros: Some(5_000_000),
        ..Default::default()
    };
    let spent = Spent {
        task: Some(4_990_000),
        ..Default::default()
    };
    assert_eq!(
        over_limit_note(&limits, Some(15_000), spent).unwrap(),
        "This task has spent $4.99 of its $5.00 limit. This call costs $0.015 and goes over it."
    );
    assert_eq!(
        over_limit_note(&limits, Some(2_000_000), Spent::default()).unwrap(),
        "This call costs about $2.00, and your limit for each call is $1.00."
    );
    // An unknown price cannot show that it stays under the limit.
    assert!(
        over_limit_note(&limits, None, Spent::default())
            .unwrap()
            .contains("cannot tell")
    );
    // Without a limit per call, an unknown price compares what was spent.
    let month = MonidLimits {
        per_channel_month_micros: Some(1_000),
        ..Default::default()
    };
    assert_eq!(over_limit_note(&month, None, Spent::default()), None);
    let at_limit = Spent {
        channel_month: 1_000,
        ..Default::default()
    };
    assert_eq!(
        over_limit_note(&month, None, at_limit).unwrap(),
        "This channel has spent $0.001 of its $0.001 limit this month."
    );
}

#[test]
fn empty_parts_of_the_input_stay_out() {
    assert_eq!(
        run_input(json!({"a": 1}), Value::Null, json!({})),
        json!({"body": {"a": 1}})
    );
    assert_eq!(
        run_input(Value::Null, json!({"q": 1}), json!({"p": 2})),
        json!({"queryParams": {"q": 1}, "pathParams": {"p": 2}})
    );
}

#[tokio::test]
async fn a_call_asks_then_runs_and_the_row_shows_what_it_cost() {
    let (hub, _dir, fake, channel_id, session) = set_up().await;
    let running = spawn_call(&hub, session, "/fast");
    let card = pending_card(&hub).await;
    let ApprovalDetails::Monid(details) = &card.details else {
        panic!("not a Monid card");
    };
    assert_eq!(card.summary, "Run a paid tool: A fast tool");
    assert_eq!(details.rate, "$0.015 per call");
    assert_eq!(details.estimate_micros, Some(15_000));
    assert_eq!(details.input, json!({"body": {"text": "hello"}}));
    assert_eq!(fake.starts.load(Ordering::SeqCst), 0, "nothing runs first");
    answer(&hub, card.id, "allow").await;
    let body = running.await.unwrap();
    assert_eq!(body["status"], "ran", "{body}");
    assert_eq!(body["successful"], true);
    assert_eq!(body["cost"], "$0.015");
    assert_eq!(body["data"], json!({"answer": 42}));
    assert_eq!(fake.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        fake.inputs.lock().unwrap()[0],
        json!({"body": {"text": "hello"}})
    );

    let saved = calls(&hub, channel_id).await;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].state, MonidCallState::Completed);
    assert_eq!(saved[0].cost_micros, 15_000);
    assert_eq!(saved[0].approval_id, Some(card.id));
    assert_eq!(saved[0].input, json!({"body": {"text": "hello"}}));
    assert_eq!(saved[0].output, json!({"answer": 42}));
    let row = row_of(&hub, &saved[0]).await;
    assert_eq!(row.kind, MessageKind::Notice);
    let notice: MonidCallNotice = serde_json::from_str(&row.body).unwrap();
    assert_eq!(notice.state, MonidCallState::Completed);
    assert_eq!(notice.cost_micros, 15_000);

    // "Allow once" gives no grant: the next call asks again.
    let again = spawn_call(&hub, session, "/fast");
    let card = pending_card(&hub).await;
    answer(&hub, card.id, "deny").await;
    assert_eq!(again.await.unwrap()["status"], "denied");
    assert_eq!(fake.starts.load(Ordering::SeqCst), 1);
    let saved = calls(&hub, channel_id).await;
    assert_eq!(saved[0].state, MonidCallState::Denied);
    assert_eq!(saved[0].cost_micros, 0);
}

#[tokio::test]
async fn allow_for_this_task_covers_that_tool_only() {
    let (hub, _dir, fake, _channel, session) = set_up().await;
    let running = spawn_call(&hub, session, "/fast");
    let card = pending_card(&hub).await;
    answer(&hub, card.id, MONID_CARD_ALLOW_TASK).await;
    assert_eq!(running.await.unwrap()["status"], "ran");
    assert_eq!(call(&hub, session, "/fast").await["status"], "ran");
    assert_eq!(fake.starts.load(Ordering::SeqCst), 2);
    let other = spawn_call(&hub, session, "/other");
    let card = pending_card(&hub).await;
    answer(&hub, card.id, "deny").await;
    assert_eq!(other.await.unwrap()["status"], "denied");
}

#[tokio::test]
async fn the_more_menu_changes_the_channels_list() {
    let (hub, _dir, fake, channel_id, session) = set_up().await;
    let running = spawn_call(&hub, session, "/fast");
    let card = pending_card(&hub).await;
    answer(&hub, card.id, MONID_CARD_ALWAYS).await;
    assert_eq!(running.await.unwrap()["status"], "ran");
    assert_eq!(call(&hub, session, "/fast").await["status"], "ran");
    assert_eq!(fake.starts.load(Ordering::SeqCst), 2);

    let running = spawn_call(&hub, session, "/never");
    let card = pending_card(&hub).await;
    answer(&hub, card.id, MONID_CARD_NEVER).await;
    assert_eq!(running.await.unwrap()["status"], "denied");
    assert_eq!(
        calls(&hub, channel_id).await[0].state,
        MonidCallState::Blocked
    );
    // Now the hub refuses with no card.
    let refused = call(&hub, session, "/never").await;
    assert_eq!(refused["status"], "denied");
    assert_eq!(refused["message"], REFUSED_TEXT);
    assert_eq!(fake.starts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn only_these_tools_refuses_the_rest_and_search_marks_it() {
    let (hub, _dir, fake, channel_id, session) = set_up().await;
    let mut access = MonidChannelAccess {
        only_allowed: true,
        ..Default::default()
    };
    access.allowed.push(MonidAllowedTool {
        tool: MonidToolRef {
            provider: "acme".to_owned(),
            endpoint: None,
            name: "All Acme tools".to_owned(),
        },
        run_without_asking: false,
    });
    set_access(&hub, channel_id, access).await;
    let found = hub
        .call_tool(session, "search_tools", json!({"query": "fast"}))
        .await
        .structured_content
        .unwrap();
    let rows = found["tools"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{found}");
    assert_eq!(rows[0]["provider"], "acme");
    assert_eq!(rows[0]["source"], "monid");
    assert_eq!(rows[0]["price"], "$0.015 per call");
    assert_eq!(rows[0]["access"], "available");
    // The other tool shows too, so the session can ask for it.
    assert_eq!(rows[1]["provider"], "other");
    assert_eq!(rows[1]["access"], "request");
    let refused = hub
        .call_tool(
            session,
            "run_monid_tool",
            json!({"provider": "other", "endpoint": "/fast"}),
        )
        .await
        .structured_content
        .unwrap();
    assert_eq!(refused["status"], "denied");
    assert_eq!(fake.starts.load(Ordering::SeqCst), 0);
    let schema = hub
        .call_tool(
            session,
            "get_monid_tool",
            json!({"provider": "other", "endpoint": "/fast"}),
        )
        .await;
    assert!(schema.is_error);
}

#[tokio::test]
async fn full_access_runs_with_no_card_but_a_limit_still_asks() {
    let (hub, _dir, fake, channel_id, session) = set_up().await;
    hub.store
        .write(move |tx| {
            let mut channel = conversation::channel_or_404(tx.db(), channel_id)?;
            channel.default_approval_mode = ApprovalMode::FullAccess;
            tx.save_channel(&channel)
        })
        .await
        .unwrap();
    assert_eq!(call(&hub, session, "/fast").await["status"], "ran");
    set_settings(&hub, |s| s.limits.per_call_micros = Some(1_000)).await;
    let running = spawn_call(&hub, session, "/fast");
    let card = pending_card(&hub).await;
    let ApprovalDetails::Monid(details) = &card.details else {
        panic!("not a Monid card");
    };
    assert!(details.over_limit.is_some());
    assert!(
        !card.options.iter().any(|o| o.id == MONID_CARD_ALLOW_TASK),
        "a card over a limit offers no blanket allow"
    );
    answer(&hub, card.id, "allow").await;
    assert_eq!(running.await.unwrap()["status"], "ran");
    assert_eq!(fake.starts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_unknown_price_asks_under_a_limit_per_call() {
    let (hub, _dir, _fake, channel_id, session) = set_up().await;
    runs_without_asking_here(&hub, channel_id).await;
    assert_eq!(call(&hub, session, "/per-result").await["status"], "ran");
    set_settings(&hub, |s| s.limits.per_call_micros = Some(1_000_000)).await;
    let running = spawn_call(&hub, session, "/per-result");
    let card = pending_card(&hub).await;
    let ApprovalDetails::Monid(details) = &card.details else {
        panic!("not a Monid card");
    };
    assert_eq!(details.estimate_micros, None);
    assert!(
        details
            .over_limit
            .as_ref()
            .unwrap()
            .text
            .contains("cannot tell")
    );
    answer(&hub, card.id, "deny").await;
    assert_eq!(running.await.unwrap()["status"], "denied");
}

#[tokio::test]
async fn the_spent_total_counts_real_costs_against_the_limit() {
    let (hub, _dir, _fake, channel_id, session) = set_up().await;
    runs_without_asking_here(&hub, channel_id).await;
    set_settings(&hub, |s| s.limits.per_channel_month_micros = Some(20_000)).await;
    assert_eq!(call(&hub, session, "/fast").await["status"], "ran");
    // $0.015 was spent, and another $0.015 goes over $0.02.
    let running = spawn_call(&hub, session, "/fast");
    let card = pending_card(&hub).await;
    let ApprovalDetails::Monid(details) = &card.details else {
        panic!("not a Monid card");
    };
    assert_eq!(
        details.over_limit.as_ref().unwrap().text,
        "This channel has spent $0.015 of its $0.02 limit this month. This call costs $0.015 and goes over it."
    );
    answer(&hub, card.id, "deny").await;
    running.await.unwrap();
}

#[tokio::test]
async fn each_way_a_run_can_fail_says_so_and_never_retries() {
    let (hub, _dir, fake, channel_id, session) = set_up().await;
    runs_without_asking_here(&hub, channel_id).await;

    let body = call(&hub, session, "/providererror").await;
    assert_eq!(body["successful"], false);
    assert_eq!(body["cost"], "$0");
    assert_eq!(
        calls(&hub, channel_id).await[0].state,
        MonidCallState::ProviderError
    );

    let body = call(&hub, session, "/blocked").await;
    assert_eq!(body["successful"], false);
    assert_eq!(body["error"], STOPPED_BY_MONID_TEXT);
    assert_eq!(
        calls(&hub, channel_id).await[0].state,
        MonidCallState::StoppedByMonid
    );

    let starts = fake.starts.load(Ordering::SeqCst);
    let body = call(&hub, session, "/poor").await;
    assert_eq!(body["successful"], false);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("app.monid.ai/wallet")
    );
    assert_eq!(
        calls(&hub, channel_id).await[0].state,
        MonidCallState::StoppedByMonid
    );

    let body = call(&hub, session, "/bad-input").await;
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("prompt is required"), "{error}");
    assert!(error.contains("nothing was charged"), "{error}");
    assert!(!error.contains("may have started"), "{error}");
    assert_eq!(
        calls(&hub, channel_id).await[0].state,
        MonidCallState::Failed
    );
    assert_eq!(fake.starts.load(Ordering::SeqCst), starts, "no second try");
}

#[tokio::test]
async fn a_generated_file_goes_to_the_channel_files_and_the_row() {
    let (hub, _dir, _fake, channel_id, session) = set_up().await;
    runs_without_asking_here(&hub, channel_id).await;
    let body = call(&hub, session, "/image").await;
    assert_eq!(body["status"], "ran");
    let path = body["files"][0].as_str().unwrap().to_owned();
    assert_eq!(path, "monid/runimage-picture.png");
    let saved = calls(&hub, channel_id).await;
    assert_eq!(saved[0].files[0].mime, "image/png");
    assert_eq!(saved[0].files[0].size, 3);
    let row = row_of(&hub, &saved[0]).await;
    assert_eq!(row.attachments, saved[0].files);
    let file: Vec<ChannelFile> = hub
        .store
        .read(move |conn| crate::store::knowledge::channel_files(conn, channel_id, None))
        .await
        .unwrap();
    assert!(file.iter().any(|f| f.path == path), "{file:?}");
}

#[tokio::test]
async fn a_huge_output_is_cut_and_the_whole_goes_to_a_blob() {
    let (hub, _dir, _fake, channel_id, session) = set_up().await;
    runs_without_asking_here(&hub, channel_id).await;
    let body = call(&hub, session, "/huge").await;
    assert_eq!(body["truncated"], true);
    assert!(body["data"].as_str().unwrap().len() <= MONID_RESULT_MAX_BYTES);
    let blob = calls(&hub, channel_id).await[0]
        .output_blob
        .clone()
        .unwrap();
    assert!(blob.size > MONID_RESULT_MAX_BYTES as u64);
}

#[tokio::test]
async fn a_long_run_returns_and_its_result_comes_as_a_wake_event() {
    let (hub, _dir, fake, channel_id, session) = set_up().await;
    runs_without_asking_here(&hub, channel_id).await;
    hub.connectors.set_timings(Timings {
        approval_wait: Duration::from_millis(200),
        ..fast()
    });
    let body = call(&hub, session, "/slow").await;
    assert_eq!(body["status"], "still_running", "{body}");

    let running = calls(&hub, channel_id).await.remove(0);
    assert_eq!(running.state, MonidCallState::Running);
    // Monid says that it can stop this run, so the row offers Stop.
    let running = wait_for(async || {
        let call = calls(&hub, channel_id).await.remove(0);
        call.stoppable.then_some(call)
    })
    .await;
    hub.monid_call_stop(&Caller::Hub, MonidCallStop { id: running.id })
        .await
        .unwrap();
    assert_eq!(fake.stops.load(Ordering::SeqCst), 1);

    fake.finish_slow.store(true, Ordering::SeqCst);
    let ended = wait_for(async || {
        let call = calls(&hub, channel_id).await.remove(0);
        call.state.is_final().then_some(call)
    })
    .await;
    assert_eq!(ended.state, MonidCallState::Completed);
    let wakes = wait_for(async || {
        let wakes = hub
            .store
            .read(move |conn| coord_store::pending_wakes(conn, channel_id))
            .await
            .unwrap();
        (!wakes.is_empty()).then_some(wakes)
    })
    .await;
    assert!(
        wakes.iter().any(
            |(_, event)| matches!(event, WakeEvent::MonidResult { id, .. } if *id == ended.id)
        ),
        "{wakes:?}"
    );
    assert_eq!(fake.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_late_answer_runs_the_call_once_and_tells_the_session() {
    let (hub, _dir, fake, channel_id, session) = set_up().await;
    hub.connectors.set_timings(Timings {
        approval_wait: Duration::from_millis(100),
        ..fast()
    });
    let body = call(&hub, session, "/fast").await;
    assert_eq!(body["status"], "waiting_for_user", "{body}");
    let card = pending_card(&hub).await;
    answer(&hub, card.id, "allow").await;
    let wakes = wait_for(async || {
        let wakes = hub
            .store
            .read(move |conn| coord_store::pending_wakes(conn, channel_id))
            .await
            .unwrap();
        (!wakes.is_empty()).then_some(wakes)
    })
    .await;
    let (_, WakeEvent::CardAnswered { text, .. }) = &wakes[0] else {
        panic!("{wakes:?}");
    };
    assert!(text.contains("It cost $0.015"), "{text}");
    assert_eq!(fake.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_late_answer_does_not_run_a_tool_that_the_channel_blocked_since() {
    let (hub, _dir, fake, channel_id, session) = set_up().await;
    hub.connectors.set_timings(Timings {
        approval_wait: Duration::from_millis(100),
        ..fast()
    });
    assert_eq!(
        call(&hub, session, "/fast").await["status"],
        "waiting_for_user"
    );
    let card = pending_card(&hub).await;
    set_access(
        &hub,
        channel_id,
        MonidChannelAccess {
            only_allowed: true,
            ..Default::default()
        },
    )
    .await;
    answer(&hub, card.id, "allow").await;
    wait_for(async || {
        let saved = calls(&hub, channel_id).await;
        saved
            .first()
            .is_some_and(|c| c.state == MonidCallState::Blocked)
            .then_some(())
    })
    .await;
    assert_eq!(fake.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_task_shows_its_total_and_its_row_is_in_the_thread() {
    let (hub, _dir, _fake) = hub_with_monid().await;
    let (task, session) = test_task_session(&hub).await;
    runs_without_asking_here(&hub, task.channel_id).await;
    assert_eq!(call(&hub, session.id, "/fast").await["status"], "ran");
    assert_eq!(call(&hub, session.id, "/fast").await["status"], "ran");
    let task_id = task.id;
    let task = hub
        .store
        .read(move |conn| work::task_or_404(conn, task_id))
        .await
        .unwrap();
    assert_eq!(task.monid_calls, 2);
    assert_eq!(task.monid_cost_micros, 30_000);
    let saved = calls(&hub, task.channel_id).await;
    let row = row_of(&hub, &saved[0]).await;
    assert_eq!(row.thread_root_id, Some(task.card_message_id));
}

#[tokio::test]
async fn a_task_of_a_routine_uses_the_routine_setting() {
    let (hub, _dir, fake) = hub_with_monid().await;
    let (task, session) = test_task_session(&hub).await;
    let task_id = task.id;
    hub.store
        .write(move |tx| {
            let mut task = work::task_or_404(tx.db(), task_id)?;
            task.created_by = TaskCreator::Routine;
            tx.save_task(&task)
        })
        .await
        .unwrap();
    set_settings(&hub, |s| s.routines = MonidUnattended::Off).await;
    let refused = call(&hub, session.id, "/fast").await;
    assert_eq!(refused["status"], "denied");
    assert_eq!(fake.starts.load(Ordering::SeqCst), 0);
    set_settings(&hub, |s| s.routines = MonidUnattended::RunWithoutAsking).await;
    assert_eq!(call(&hub, session.id, "/fast").await["status"], "ran");
}

#[tokio::test]
async fn the_hub_follows_a_running_call_after_a_restart_and_never_starts_it_again() {
    let (hub, _dir, fake, channel_id, session) = set_up().await;
    let mut call = hub.new_call(
        &PendingCall {
            scope: CallScope {
                channel_id,
                session_id: session,
                task_id: None,
            },
            provider: "acme".to_owned(),
            endpoint: "/fast".to_owned(),
            name: "A fast tool".to_owned(),
            rate: String::new(),
            input: json!({}),
            approval_id: None,
        },
        MonidCallState::Running,
    );
    call.run_id = Some("runfast".to_owned());
    let mut lost = call.clone();
    lost.id = MonidCallId::new();
    lost.run_id = None;
    let saved = (call.clone(), lost.clone());
    hub.store
        .write(move |tx| {
            let (mut call, mut lost) = saved;
            insert_call(tx, &mut call)?;
            insert_call(tx, &mut lost)
        })
        .await
        .unwrap();
    hub.resume_monid().await.unwrap();
    wait_for(async || {
        let saved = calls(&hub, channel_id).await;
        saved.iter().all(|c| c.state.is_final()).then_some(())
    })
    .await;
    let saved = calls(&hub, channel_id).await;
    let followed = saved.iter().find(|c| c.id == call.id).unwrap();
    assert_eq!(followed.state, MonidCallState::Completed);
    assert_eq!(followed.cost_micros, 15_000);
    let lost = saved.iter().find(|c| c.id == lost.id).unwrap();
    assert_eq!(lost.state, MonidCallState::Failed);
    assert!(lost.error.as_deref().unwrap().contains("may have started"));
    assert_eq!(fake.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_key_is_checked_and_stays_out_of_the_status() {
    let (hub, dir) = test_hub().await;
    let fake = start_fake().await;
    hub.connectors
        .set_monid_test_server(&fake.url.lock().unwrap().clone());
    let refused = hub
        .monid_set_key(
            &Caller::Hub,
            MonidSetKey {
                api_key: "monid_live_wrong".to_owned(),
                settings: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code(), ErrorCode::MonidKeyRefused);
    assert!(!dir.path().join("monid.key").exists());

    let status = hub
        .monid_set_key(
            &Caller::Hub,
            MonidSetKey {
                api_key: GOOD_KEY.to_owned(),
                settings: None,
            },
        )
        .await
        .unwrap()
        .status;
    assert_eq!(status.key_hint.as_deref(), Some("3f9a"));
    assert_eq!(status.balance_micros, Some(49_340_000));
    assert!(!format!("{status:?}").contains(GOOD_KEY));
    let on_disk = std::fs::read(dir.path().join("monid.key")).unwrap();
    assert!(
        !on_disk
            .windows(GOOD_KEY.len())
            .any(|w| w == GOOD_KEY.as_bytes()),
        "the key is encrypted at rest"
    );

    let status = hub
        .monid_remove_key(&Caller::Hub, MonidRemoveKey {})
        .await
        .unwrap()
        .status;
    assert!(!status.key_present);
    assert!(!hub.store.read(store::key_present).await.unwrap());
}

#[tokio::test]
async fn the_tools_show_when_the_hub_has_monid() {
    let (hub, _dir) = test_hub().await;
    let channel_id = tutti(&hub).await;
    let session = coordinator_session(&hub, channel_id, "acp").await;
    let names = async |hub: &Hub| -> Vec<String> {
        hub.tool_definitions(session)
            .await
            .unwrap()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    };
    let without = names(&hub).await;
    assert!(!without.iter().any(|n| n == "run_monid_tool"));
    assert!(!without.iter().any(|n| n == "search_tools"));
    let fake = start_fake().await;
    hub.connectors
        .set_monid_test_server(&fake.url.lock().unwrap().clone());
    hub.monid_set_key(
        &Caller::Hub,
        MonidSetKey {
            api_key: GOOD_KEY.to_owned(),
            settings: Some(MonidSettings {
                use_in: UseIn::EveryChannel,
                ..Default::default()
            }),
        },
    )
    .await
    .unwrap();
    let with = names(&hub).await;
    assert!(with.iter().any(|n| n == "run_monid_tool"));
    assert!(with.iter().any(|n| n == "search_tools"));
    // A channel with Monid off keeps the tools: it can ask for access, and
    // a harness may not read the list of tools again.
    set_access(
        &hub,
        channel_id,
        MonidChannelAccess {
            disabled: true,
            ..Default::default()
        },
    )
    .await;
    assert!(names(&hub).await.iter().any(|n| n == "run_monid_tool"));
    let refused = hub
        .call_tool(
            session,
            "run_monid_tool",
            json!({"provider": "acme", "endpoint": "/fast"}),
        )
        .await;
    assert!(refused.is_error);
    assert_eq!(fake.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_list_of_calls_leaves_out_the_input_and_the_output() {
    let (hub, _dir, _fake, channel_id, session) = set_up().await;
    runs_without_asking_here(&hub, channel_id).await;
    call(&hub, session, "/fast").await;
    let listed = hub
        .monid_call_list(
            &Caller::Hub,
            MonidCallList {
                channel_id,
                limit: None,
            },
        )
        .await
        .unwrap()
        .calls;
    assert_eq!(listed[0].input, Value::Null);
    let whole = hub
        .monid_call_get(&Caller::Hub, MonidCallGet { id: listed[0].id })
        .await
        .unwrap()
        .call;
    assert_eq!(whole.output, json!({"answer": 42}));
    let status = hub
        .monid_get_status(&Caller::Hub, MonidGetStatus {})
        .await
        .unwrap()
        .status;
    assert_eq!(status.spent_month_micros, 15_000);
    assert_eq!(status.spend[0].channel_id, channel_id);
    assert_eq!(status.spend[0].calls, 1);
}

#[test]
fn a_tool_name_uses_the_description_when_discover_gives_no_summary() {
    let tool = monid::Tool {
        provider: "acme".into(),
        provider_name: "Acme".into(),
        endpoint: "/fast".into(),
        description: "Makes images. It costs money.".into(),
        ..Default::default()
    };
    assert_eq!(tool_name(&tool), "Makes images");
    let bare = monid::Tool {
        description: String::new(),
        ..tool
    };
    assert_eq!(tool_name(&bare), "Acme /fast");
}
