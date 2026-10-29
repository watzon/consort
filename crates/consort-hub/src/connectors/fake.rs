//! A fake Composio v3.1 server for tests. It answers only the calls that
//! the hub makes, with the shapes of the OpenAPI file. It never talks to
//! the real Composio.
//!
//! The integration tests include this file with `#[path]`, so it uses
//! only axum, serde_json and tokio, and nothing from the hub.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

/// The key that the fake accepts.
pub const GOOD_KEY: &str = "ak_fake_project_key_7Q2x";
/// The API key that the fake SendGrid accepts.
pub const SENDGRID_KEY: &str = "SG.good-key";

#[derive(Default)]
pub struct FakeState {
    pub calls: Vec<String>,
    pub auth_configs: Vec<Value>,
    /// Connected accounts by ID: (status, toolkit, alias, email).
    pub accounts: BTreeMap<String, (String, String, String, String)>,
    /// The bodies of `POST /tools/execute/{slug}`, with the slug.
    pub executions: Vec<(String, Value)>,
    /// Link accounts become active the first time the hub reads them.
    pub auto_activate: bool,
    next: u32,
}

#[derive(Clone)]
pub struct FakeComposio {
    pub url: String,
    pub state: Arc<Mutex<FakeState>>,
}

type Shared = Arc<Mutex<FakeState>>;

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(json!({"error": {"message": "Invalid API key", "code": 401, "slug": "unauthorized", "status": 401}})),
    )
        .into_response()
}

fn not_found(what: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(json!({"error": {"message": format!("{what} not found"), "code": 404, "slug": "not_found", "status": 404}})),
    )
        .into_response()
}

fn check(state: &Shared, headers: &HeaderMap, call: String) -> bool {
    state.lock().unwrap().calls.push(call);
    headers.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(GOOD_KEY)
}

fn toolkits() -> Vec<Value> {
    vec![
        json!({
            "slug": "gmail", "name": "Gmail", "type": "native",
            "auth_schemes": ["oauth2"], "composio_managed_auth_schemes": ["oauth2"], "no_auth": false,
            "meta": {"description": "Read and send email", "logo": "https://logos.example/gmail.svg",
                     "categories": [{"id": "productivity", "name": "Productivity"}], "tools_count": 3.0}
        }),
        json!({
            "slug": "github", "name": "GitHub", "type": "native",
            "auth_schemes": ["oauth2"], "composio_managed_auth_schemes": ["oauth2"], "no_auth": false,
            "meta": {"description": "Code hosting", "logo": "https://logos.example/github.svg",
                     "categories": [{"id": "developer-tools", "name": "Developer tools"}], "tools_count": 1.0}
        }),
        json!({
            "slug": "sendgrid", "name": "SendGrid", "type": "native",
            "auth_schemes": ["api_key"], "composio_managed_auth_schemes": [], "no_auth": false,
            "meta": {"description": "Transactional email", "logo": null,
                     "categories": [{"id": "productivity", "name": "Productivity"}], "tools_count": 1.0}
        }),
    ]
}

fn tools_of(toolkit: &str) -> Vec<Value> {
    let tool = |slug: &str, description: &str, tags: &[&str]| {
        json!({
            "slug": slug, "name": slug, "description": description,
            "toolkit": {"slug": toolkit, "name": toolkit},
            "input_parameters": {"type": "object", "properties": {"q": {"type": "string"}}},
            "tags": tags, "is_deprecated": false,
        })
    };
    match toolkit {
        "gmail" => vec![
            tool("GMAIL_FETCH_EMAILS", "Fetch emails", &["readOnlyHint"]),
            tool("GMAIL_SEND_EMAIL", "Send an email", &["createHint"]),
            tool(
                "GMAIL_DELETE_MESSAGE",
                "Delete a message",
                &["destructiveHint"],
            ),
            tool("GMAIL_BIG_EXPORT", "Export everything", &["readOnlyHint"]),
        ],
        "github" => vec![tool("GITHUB_LIST_REPOS", "List repos", &["readOnlyHint"])],
        "sendgrid" => vec![tool("SENDGRID_SEND", "Send mail", &["createHint"])],
        _ => Vec::new(),
    }
}

fn page(items: Vec<Value>) -> Value {
    json!({"items": items, "next_cursor": null, "total_pages": 1, "current_page": 1, "total_items": 0})
}

async fn list_toolkits(State(s): State<Shared>, headers: HeaderMap) -> Response {
    if !check(&s, &headers, "GET /toolkits".into()) {
        return unauthorized();
    }
    axum::Json(page(toolkits())).into_response()
}

async fn categories(State(s): State<Shared>, headers: HeaderMap) -> Response {
    if !check(&s, &headers, "GET /toolkits/categories".into()) {
        return unauthorized();
    }
    axum::Json(page(vec![
        json!({"id": "productivity", "name": "Productivity"}),
        json!({"id": "developer-tools", "name": "Developer tools"}),
    ]))
    .into_response()
}

async fn toolkit(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Response {
    if !check(&s, &headers, format!("GET /toolkits/{slug}")) {
        return unauthorized();
    }
    let Some(listed) = toolkits().into_iter().find(|t| t["slug"] == slug) else {
        return not_found("Toolkit");
    };
    let mode = listed["auth_schemes"][0]
        .as_str()
        .unwrap_or("oauth2")
        .to_owned();
    let fields = if mode == "api_key" {
        json!({"auth_config_creation": {"required": [], "optional": []},
               "connected_account_initiation": {"required": [
                   {"name": "api_key", "displayName": "API key", "type": "string", "description": "Your SendGrid API key", "required": true, "is_secret": true}
               ], "optional": []}})
    } else {
        json!({"auth_config_creation": {"required": [], "optional": []},
               "connected_account_initiation": {"required": [], "optional": []}})
    };
    let mut meta = listed["meta"].clone();
    meta["categories"] = json!(
        listed["meta"]["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| json!({"slug": c["id"], "name": c["name"]}))
            .collect::<Vec<_>>()
    );
    axum::Json(json!({
        "slug": listed["slug"], "name": listed["name"], "type": "native", "enabled": true,
        "composio_managed_auth_schemes": listed["composio_managed_auth_schemes"],
        "no_auth": false, "meta": meta,
        "auth_config_details": [{"mode": mode, "name": mode, "fields": fields}],
    }))
    .into_response()
}

async fn list_tools(
    State(s): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let toolkit = q.get("toolkit_slug").cloned().unwrap_or_default();
    if !check(&s, &headers, format!("GET /tools?toolkit_slug={toolkit}")) {
        return unauthorized();
    }
    axum::Json(page(tools_of(&toolkit))).into_response()
}

async fn tool(State(s): State<Shared>, headers: HeaderMap, Path(slug): Path<String>) -> Response {
    if !check(&s, &headers, format!("GET /tools/{slug}")) {
        return unauthorized();
    }
    for toolkit in ["gmail", "github", "sendgrid"] {
        if let Some(t) = tools_of(toolkit).into_iter().find(|t| t["slug"] == slug) {
            return axum::Json(t).into_response();
        }
    }
    not_found("Tool")
}

async fn list_auth_configs(
    State(s): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let toolkit = q.get("toolkit_slug").cloned().unwrap_or_default();
    if !check(
        &s,
        &headers,
        format!("GET /auth_configs?toolkit_slug={toolkit}"),
    ) {
        return unauthorized();
    }
    let items: Vec<Value> = s
        .lock()
        .unwrap()
        .auth_configs
        .iter()
        .filter(|c| c["toolkit"]["slug"] == toolkit)
        .cloned()
        .collect();
    axum::Json(page(items)).into_response()
}

async fn create_auth_config(
    State(s): State<Shared>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    if !check(&s, &headers, "POST /auth_configs".into()) {
        return unauthorized();
    }
    let mut state = s.lock().unwrap();
    state.next += 1;
    let id = format!("ac_{}", state.next);
    let managed = body["auth_config"]["type"] == "use_composio_managed_auth";
    let scheme = if managed {
        "OAUTH2".to_owned()
    } else {
        body["auth_config"]["authScheme"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    };
    state.auth_configs.push(json!({
        "id": id, "name": id, "type": "default", "auth_scheme": scheme,
        "is_composio_managed": managed, "status": "ENABLED",
        "toolkit": body["toolkit"].clone(),
    }));
    (
        StatusCode::CREATED,
        axum::Json(json!({"toolkit": body["toolkit"], "auth_config": {"id": id, "auth_scheme": scheme, "is_composio_managed": managed}})),
    )
        .into_response()
}

async fn link(
    State(s): State<Shared>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    if !check(&s, &headers, "POST /connected_accounts/link".into()) {
        return unauthorized();
    }
    let mut state = s.lock().unwrap();
    let Some(config) = state
        .auth_configs
        .iter()
        .find(|c| c["id"] == body["auth_config_id"])
        .cloned()
    else {
        return not_found("Auth config");
    };
    let alias = body["alias"].as_str().unwrap_or("").to_owned();
    let toolkit = config["toolkit"]["slug"].as_str().unwrap_or("").to_owned();
    if state
        .accounts
        .values()
        .any(|a| a.1 == toolkit && a.2 == alias)
    {
        return (
            StatusCode::CONFLICT,
            axum::Json(json!({"error": {"message": "Alias in use", "code": 409, "slug": "conflict", "status": 409}})),
        )
            .into_response();
    }
    state.next += 1;
    let id = format!("ca_{}", state.next);
    let email = format!("{alias}@example.com");
    state
        .accounts
        .insert(id.clone(), ("INITIATED".into(), toolkit, alias, email));
    (
        StatusCode::CREATED,
        axum::Json(json!({
            "link_token": "lt_1", "redirect_url": format!("https://connect.example/link/{id}"),
            "expires_at": "2026-10-29T12:00:00Z", "connected_account_id": id,
        })),
    )
        .into_response()
}

async fn create_account(
    State(s): State<Shared>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    if !check(&s, &headers, "POST /connected_accounts".into()) {
        return unauthorized();
    }
    let mut state = s.lock().unwrap();
    let val = &body["connection"]["state"]["val"];
    if val["api_key"] != SENDGRID_KEY {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": {"message": "ConnectedAccount_CredentialValidationRejected", "code": 400, "slug": "bad", "status": 400}})),
        )
            .into_response();
    }
    let Some(config) = state
        .auth_configs
        .iter()
        .find(|c| c["id"] == body["auth_config"]["id"])
        .cloned()
    else {
        return not_found("Auth config");
    };
    state.next += 1;
    let id = format!("ca_{}", state.next);
    let alias = body["connection"]["alias"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    state.accounts.insert(
        id.clone(),
        (
            "ACTIVE".into(),
            config["toolkit"]["slug"].as_str().unwrap_or("").to_owned(),
            alias,
            String::new(),
        ),
    );
    (
        StatusCode::CREATED,
        axum::Json(json!({"id": id, "status": "ACTIVE", "connectionData": {"authScheme": "API_KEY", "val": val}})),
    )
        .into_response()
}

async fn get_account(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !check(&s, &headers, format!("GET /connected_accounts/{id}")) {
        return unauthorized();
    }
    let mut state = s.lock().unwrap();
    let auto = state.auto_activate;
    let Some(account) = state.accounts.get_mut(&id) else {
        return not_found("Connected account");
    };
    if auto && account.0 == "INITIATED" {
        account.0 = "ACTIVE".into();
    }
    let mut val = json!({"status": account.0, "access_token": "secret-token-never-shown"});
    if !account.3.is_empty() {
        val["email"] = json!(account.3);
    }
    axum::Json(json!({
        "id": id, "status": account.0, "status_reason": null, "alias": account.2,
        "toolkit": {"slug": account.1}, "state": {"authScheme": "OAUTH2", "val": val},
    }))
    .into_response()
}

async fn delete_account(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !check(&s, &headers, format!("DELETE /connected_accounts/{id}")) {
        return unauthorized();
    }
    let removed = s.lock().unwrap().accounts.remove(&id);
    if removed.is_none() {
        return not_found("Connected account");
    }
    axum::Json(json!({"success": true})).into_response()
}

async fn list_accounts(State(s): State<Shared>, headers: HeaderMap) -> Response {
    if !check(&s, &headers, "GET /connected_accounts".into()) {
        return unauthorized();
    }
    let items: Vec<Value> = s
        .lock()
        .unwrap()
        .accounts
        .iter()
        .map(|(id, a)| json!({"id": id, "status": a.0, "toolkit": {"slug": a.1}, "alias": a.2}))
        .collect();
    axum::Json(page(items)).into_response()
}

async fn execute(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    if !check(&s, &headers, format!("POST /tools/execute/{slug}")) {
        return unauthorized();
    }
    let mut state = s.lock().unwrap();
    state.executions.push((slug.clone(), body.clone()));
    let account_id = body["connected_account_id"]
        .as_str()
        .unwrap_or("")
        .to_owned();
    match state.accounts.get(&account_id) {
        None => return not_found("Connected account"),
        Some(a) if a.0 != "ACTIVE" => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"error": {"message": "Connected account is not active", "code": 400, "slug": "inactive", "status": 400}})),
            )
                .into_response();
        }
        Some(_) => {}
    }
    let data = if slug == "GMAIL_BIG_EXPORT" {
        json!({"blob": "x".repeat(300 * 1024)})
    } else {
        json!({"echo": body["arguments"], "account": account_id})
    };
    axum::Json(json!({"data": data, "error": null, "successful": true, "log_id": "log_1"}))
        .into_response()
}

impl FakeComposio {
    /// Starts the fake on a free local port. The base URL ends with
    /// `/api/v3.1`, like the real one.
    pub async fn start() -> Self {
        let state: Shared = Arc::default();
        let api = Router::new()
            .route("/toolkits", get(list_toolkits))
            .route("/toolkits/categories", get(categories))
            .route("/toolkits/{slug}", get(toolkit))
            .route("/tools", get(list_tools))
            .route("/tools/{slug}", get(tool))
            .route("/tools/execute/{slug}", post(execute))
            .route(
                "/auth_configs",
                get(list_auth_configs).post(create_auth_config),
            )
            .route(
                "/connected_accounts",
                get(list_accounts).post(create_account),
            )
            .route("/connected_accounts/link", post(link))
            .route(
                "/connected_accounts/{id}",
                get(get_account).delete(delete_account),
            )
            .with_state(state.clone());
        let app = Router::new().nest("/api/v3.1", api);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            url: format!("http://{addr}/api/v3.1"),
            state,
        }
    }

    /// How many times the hub made a call, for example `GET /toolkits`.
    pub fn count(&self, call: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.as_str() == call)
            .count()
    }

    pub fn set_auto_activate(&self, on: bool) {
        self.state.lock().unwrap().auto_activate = on;
    }

    /// Sets the status of every account, or of one.
    pub fn set_status(&self, id: Option<&str>, status: &str) {
        let mut state = self.state.lock().unwrap();
        for (account_id, account) in state.accounts.iter_mut() {
            if id.is_none_or(|i| i == account_id) {
                account.0 = status.to_owned();
            }
        }
    }

    pub fn account_ids(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .accounts
            .keys()
            .cloned()
            .collect()
    }

    pub fn executions(&self) -> Vec<(String, Value)> {
        self.state.lock().unwrap().executions.clone()
    }
}
