//! A small client for the Composio v3.1 HTTP API, with only the calls that
//! the hub needs.
//!
//! The shapes come from the published OpenAPI file
//! (<https://docs.composio.dev/openapi.json>, version 3.1.0) and the API
//! reference (<https://docs.composio.dev/reference>). Every request sends
//! the project key in the `x-api-key` header
//! (<https://docs.composio.dev/reference/authenticating-to-composio>).
//!
//! The key is never logged: [`ApiKey`] has a redacted `Debug`, and errors
//! never hold a request header.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use consort_proto::error::ErrorCode;
use consort_proto::model::SecretEnv;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::error::{HubError, Result};

/// The default API base. The OpenAPI file's server is
/// `https://backend.composio.dev`, and every path starts with `/api/v3.1`.
pub(crate) const DEFAULT_BASE_URL: &str = "https://backend.composio.dev/api/v3.1";
/// The longest wait for one request. A tool call can take a while.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
/// The largest answer that the hub reads. A tool result over the frame
/// limit is cut later; this stops a wrong server.
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
/// The page size for lists. The toolkit and tool lists allow up to 1,000
/// (OpenAPI `GET /toolkits`, `GET /tools`).
pub(crate) const PAGE_LIMIT: u32 = 1_000;
/// The page size for auth configs, which allow at most 200.
const AUTH_CONFIG_PAGE_LIMIT: u32 = 200;
/// The most pages that the hub reads from one list.
const MAX_PAGES: usize = 20;

/// The Composio project key. `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ApiKey(String);

impl ApiKey {
    pub(crate) fn new(key: String) -> Self {
        Self(key)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// The last 4 characters, for `composio.status`.
    pub(crate) fn hint(&self) -> String {
        let chars: Vec<char> = self.0.chars().collect();
        chars[chars.len().saturating_sub(4)..].iter().collect()
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// One page of a list. Every list in v3.1 has this shape.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Page<T> {
    #[serde(default = "Vec::new")]
    pub items: Vec<T>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// An item of `GET /toolkits`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Toolkit {
    pub slug: String,
    pub name: String,
    /// Lowercase scheme names, for example `oauth2` and `api_key`.
    pub auth_schemes: Vec<String>,
    pub composio_managed_auth_schemes: Vec<String>,
    pub no_auth: bool,
    pub meta: ToolkitMeta,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ToolkitMeta {
    pub description: String,
    pub logo: Option<String>,
    pub categories: Vec<CategoryRef>,
    pub tools_count: Option<f64>,
}

/// A category inside a toolkit. The list gives `id`, the detail gives
/// `slug` (OpenAPI `GET /toolkits` and `GET /toolkits/{slug}`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CategoryRef {
    pub id: Option<String>,
    pub slug: Option<String>,
    pub name: String,
}

impl CategoryRef {
    pub(crate) fn key(&self) -> String {
        self.slug
            .clone()
            .or_else(|| self.id.clone())
            .unwrap_or_else(|| self.name.to_lowercase())
    }
}

/// `GET /toolkits/{slug}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ToolkitDetail {
    pub slug: String,
    pub name: String,
    pub composio_managed_auth_schemes: Vec<String>,
    pub no_auth: bool,
    pub meta: ToolkitMeta,
    pub auth_config_details: Vec<AuthConfigDetail>,
}

/// One auth mode of a toolkit. The fields that the user gives for a new
/// connected account are in `fields.connected_account_initiation`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct AuthConfigDetail {
    pub mode: String,
    pub name: String,
    pub fields: AuthFields,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct AuthFields {
    pub connected_account_initiation: FieldGroup,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FieldGroup {
    pub required: Vec<AuthField>,
    pub optional: Vec<AuthField>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct AuthField {
    pub name: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub description: String,
    pub is_secret: Option<bool>,
}

/// An item of `GET /toolkits/categories`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Category {
    pub id: String,
    pub name: String,
}

/// An item of `GET /tools`, and the answer of `GET /tools/{tool_slug}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Tool {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub input_parameters: Value,
    /// Holds the hints `readOnlyHint`, `createHint`, `updateHint` and
    /// `destructiveHint` (<https://docs.composio.dev/docs/tool-tags>).
    pub tags: Vec<String>,
    pub is_deprecated: bool,
}

impl Tool {
    pub(crate) fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
    }
}

/// An item of `GET /auth_configs`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct AuthConfig {
    pub id: String,
    pub auth_scheme: Option<String>,
    pub is_composio_managed: bool,
    pub status: Option<String>,
}

impl AuthConfig {
    pub(crate) fn is_enabled(&self) -> bool {
        self.status
            .as_deref()
            .is_none_or(|s| s.eq_ignore_ascii_case("ENABLED"))
    }
}

/// The answer of `POST /connected_accounts/link`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Link {
    pub redirect_url: String,
    pub connected_account_id: String,
}

/// `GET /connected_accounts/{nanoid}`. `state.val` can hold tokens, so
/// the hub keeps only the fields it names here and never logs the answer.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ConnectedAccount {
    pub id: String,
    /// `INITIALIZING`, `INITIATED`, `ACTIVE`, `FAILED`, `EXPIRED`,
    /// `INACTIVE` or `REVOKED`.
    pub status: String,
    pub status_reason: Option<String>,
    #[serde(skip)]
    pub identity: Option<String>,
}

/// Keys in a connected account's data that name the account. A key that
/// is not on this list is never read, so no token leaves the hub's memory.
const IDENTITY_KEYS: &[&str] = &[
    "email",
    "user_email",
    "account_email",
    "email_address",
    "login",
    "username",
    "user_name",
    "account_name",
];

fn identity_of(value: &Value) -> Option<String> {
    let maps = [
        value.pointer("/state/val"),
        value.get("data"),
        value.get("params"),
    ];
    for map in maps.into_iter().flatten().filter_map(Value::as_object) {
        for key in IDENTITY_KEYS {
            if let Some(text) = map.get(*key).and_then(Value::as_str) {
                let text = text.trim();
                if !text.is_empty() && text.chars().count() <= 200 {
                    return Some(text.to_owned());
                }
            }
        }
    }
    None
}

/// The answer of `POST /tools/execute/{tool_slug}`. A tool that failed
/// answers 200 with `successful: false` and an `error`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Execution {
    pub data: Value,
    pub error: Option<String>,
    pub successful: bool,
    pub log_id: Option<String>,
}

/// The status of a connected account, as the hub reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountState {
    Waiting,
    Active,
    Expired,
    Failed,
}

impl ConnectedAccount {
    pub(crate) fn state(&self) -> AccountState {
        match self.status.to_ascii_uppercase().as_str() {
            "ACTIVE" => AccountState::Active,
            "INITIALIZING" | "INITIATED" => AccountState::Waiting,
            "EXPIRED" | "INACTIVE" | "REVOKED" => AccountState::Expired,
            _ => AccountState::Failed,
        }
    }
}

/// Whether Composio accepted the key at the last call. Every [`Client`] of
/// the hub shares one, so a call that Composio refuses shows in
/// `composio.status` at once, with the next step.
#[derive(Debug, Default)]
pub(crate) struct KeyHealth {
    refused: AtomicBool,
}

impl KeyHealth {
    pub(crate) fn is_refused(&self) -> bool {
        self.refused.load(Ordering::Relaxed)
    }

    pub(crate) fn set_refused(&self, refused: bool) {
        self.refused.store(refused, Ordering::Relaxed);
    }
}

/// A Composio client for one key and one base URL.
#[derive(Clone)]
pub(crate) struct Client {
    http: reqwest::Client,
    base: String,
    key: ApiKey,
    health: Arc<KeyHealth>,
    timeout: Duration,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base)
            .field("key", &self.key)
            .finish()
    }
}

/// The hub's error for an answer from Composio.
pub(crate) fn composio_error(message: impl Into<String>) -> HubError {
    HubError::new(ErrorCode::ComposioError, message)
}

impl Client {
    #[cfg(test)]
    pub(crate) fn new(base: &str, key: ApiKey) -> Result<Self> {
        Self::with_health(base, key, Arc::default())
    }

    /// A client that reports to `health` whether Composio accepts the key.
    pub(crate) fn with_health(base: &str, key: ApiKey, health: Arc<KeyHealth>) -> Result<Self> {
        Self::with_timeout(base, key, health, REQUEST_TIMEOUT)
    }

    /// The same, with another time limit for one request. Tests use a short
    /// one with a server that never answers.
    pub(crate) fn with_timeout(
        base: &str,
        key: ApiKey,
        health: Arc<KeyHealth>,
        timeout: Duration,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("consort-hub/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| HubError::internal("the HTTP client did not start"))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            key,
            health,
            timeout,
        })
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut url = format!("{}{path}", self.base);
        for (i, (name, value)) in query.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(&query_part(name));
            url.push('=');
            url.push_str(&query_part(value));
        }
        let mut request = self
            .http
            .request(method, &url)
            .header("x-api-key", self.key.expose());
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|err| {
            if err.is_timeout() {
                return composio_error(format!(
                    "Composio did not answer within {}. Try again in a moment.",
                    crate::util::duration_text(self.timeout)
                ));
            }
            composio_error(format!(
                "The hub could not reach Composio: {}. Check that the hub can reach the internet, then try again.",
                err.without_url()
            ))
        })?;
        let status = response.status();
        // A refused key is a fact about the key. Any other answer, also an
        // error about the request, shows that Composio accepts the key.
        self.health
            .set_refused(matches!(status.as_u16(), 401 | 403));
        if response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
        {
            return Err(composio_error("Composio sent an answer that is too large."));
        }
        let bytes = response.bytes().await.map_err(|err| {
            if err.is_timeout() {
                return composio_error(format!(
                    "Composio did not finish its answer within {}. Try again in a moment.",
                    crate::util::duration_text(self.timeout)
                ));
            }
            composio_error(format!(
                "The answer from Composio stopped: {}. Try again.",
                err.without_url()
            ))
        })?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(composio_error("Composio sent an answer that is too large."));
        }
        let value: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        if status.is_success() {
            return Ok(value);
        }
        Err(status_error(status, &value))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let value = self.send(Method::GET, path, query, None).await?;
        decode(value)
    }

    async fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        let value = self.send(Method::POST, path, &[], Some(body)).await?;
        decode(value)
    }

    /// Reads every page of a list, up to [`MAX_PAGES`].
    async fn all_pages<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        limit: u32,
    ) -> Result<Vec<T>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut q = query.to_vec();
            q.push(("limit", limit.to_string()));
            if let Some(c) = &cursor {
                q.push(("cursor", c.clone()));
            }
            let page: Page<T> = self.get(path, &q).await?;
            out.extend(page.items);
            match page.next_cursor.filter(|c| !c.is_empty()) {
                Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
                _ => break,
            }
        }
        Ok(out)
    }

    /// A cheap call that checks the key: `GET /toolkits?limit=1`. The API
    /// has no call that checks a project key alone.
    pub(crate) async fn check_key(&self) -> Result<()> {
        let _: Page<Value> = self.get("/toolkits", &[("limit", "1".to_owned())]).await?;
        Ok(())
    }

    /// `GET /toolkits`, every page.
    pub(crate) async fn toolkits(&self) -> Result<Vec<Toolkit>> {
        self.all_pages("/toolkits", &[("sort_by", "usage".to_owned())], PAGE_LIMIT)
            .await
    }

    /// `GET /toolkits/{slug}`.
    pub(crate) async fn toolkit(&self, slug: &str) -> Result<ToolkitDetail> {
        self.get(&format!("/toolkits/{}", path_part(slug)), &[])
            .await
    }

    /// `GET /toolkits/categories`.
    pub(crate) async fn categories(&self) -> Result<Vec<Category>> {
        let page: Page<Category> = self.get("/toolkits/categories", &[]).await?;
        Ok(page.items)
    }

    /// `GET /tools?toolkit_slug=...`, every page. Deprecated tools are left
    /// out.
    pub(crate) async fn tools(&self, toolkit: &str) -> Result<Vec<Tool>> {
        let tools: Vec<Tool> = self
            .all_pages(
                "/tools",
                &[
                    ("toolkit_slug", toolkit.to_owned()),
                    ("include_deprecated", "false".to_owned()),
                ],
                PAGE_LIMIT,
            )
            .await?;
        Ok(tools.into_iter().filter(|t| !t.is_deprecated).collect())
    }

    /// `GET /tools/{tool_slug}`.
    pub(crate) async fn tool(&self, slug: &str) -> Result<Tool> {
        self.get(&format!("/tools/{}", path_part(slug)), &[]).await
    }

    /// `GET /auth_configs?toolkit_slug=...`.
    pub(crate) async fn auth_configs(&self, toolkit: &str) -> Result<Vec<AuthConfig>> {
        self.all_pages(
            "/auth_configs",
            &[("toolkit_slug", toolkit.to_owned())],
            AUTH_CONFIG_PAGE_LIMIT,
        )
        .await
    }

    /// `POST /auth_configs`. With no scheme, a Composio-managed config;
    /// with a scheme such as `API_KEY`, a custom one. Returns its ID.
    /// Body: OpenAPI `POST /auth_configs`
    /// (<https://docs.composio.dev/reference/api-reference/auth-configs>).
    pub(crate) async fn create_auth_config(
        &self,
        toolkit: &str,
        custom_scheme: Option<&str>,
    ) -> Result<String> {
        let auth_config = match custom_scheme {
            None => json!({"type": "use_composio_managed_auth"}),
            Some(scheme) => json!({"type": "use_custom_auth", "authScheme": scheme}),
        };
        let body = json!({"toolkit": {"slug": toolkit}, "auth_config": auth_config});
        let value: Value = self.post("/auth_configs", &body).await?;
        value
            .pointer("/auth_config/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| composio_error("Composio made an auth config but sent no ID."))
    }

    /// `POST /connected_accounts/link`: a hosted sign-in page for one new
    /// connected account. The alias must be unique per user and toolkit.
    pub(crate) async fn link(
        &self,
        auth_config_id: &str,
        user_id: &str,
        alias: &str,
    ) -> Result<Link> {
        let body = json!({
            "auth_config_id": auth_config_id,
            "user_id": user_id,
            "alias": alias,
        });
        self.post("/connected_accounts/link", &body).await
    }

    /// `POST /connected_accounts` for a scheme with no browser step. The
    /// credentials go in `connection.state.val`
    /// (<https://docs.composio.dev/docs/authentication/importing-existing-connections>).
    /// Returns the new account ID.
    pub(crate) async fn create_connected_account(
        &self,
        auth_config_id: &str,
        user_id: &str,
        alias: &str,
        scheme: &str,
        credentials: &SecretEnv,
    ) -> Result<ConnectedAccount> {
        let mut val = Map::new();
        val.insert("status".to_owned(), Value::String("ACTIVE".to_owned()));
        for (name, value) in credentials.iter() {
            val.insert(name.to_owned(), Value::String(value.to_owned()));
        }
        let body = json!({
            "auth_config": {"id": auth_config_id},
            "connection": {
                "user_id": user_id,
                "alias": alias,
                "state": {"authScheme": scheme, "val": Value::Object(val)},
            },
        });
        let value = self
            .send(Method::POST, "/connected_accounts", &[], Some(&body))
            .await?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| composio_error("Composio made the account but sent no ID."))?;
        // The create answer can hold the credentials in `connectionData`, so
        // read the account again for its state.
        self.connected_account(id).await
    }

    /// `GET /connected_accounts/{nanoid}`.
    pub(crate) async fn connected_account(&self, id: &str) -> Result<ConnectedAccount> {
        let value = self
            .send(
                Method::GET,
                &format!("/connected_accounts/{}", path_part(id)),
                &[],
                None,
            )
            .await?;
        let identity = identity_of(&value);
        let mut account: ConnectedAccount = decode(value)?;
        account.identity = identity;
        Ok(account)
    }

    /// `DELETE /connected_accounts/{nanoid}`. An account that Composio does
    /// not know is gone already.
    pub(crate) async fn delete_connected_account(&self, id: &str) -> Result<()> {
        match self
            .send(
                Method::DELETE,
                &format!("/connected_accounts/{}", path_part(id)),
                &[],
                None,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if is_not_found(&err) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// `POST /tools/execute/{tool_slug}` with the exact connected account.
    /// v3.1 runs the latest toolkit version when `version` is left out
    /// (<https://docs.composio.dev/docs/migration-guide/toolkit-versioning>).
    pub(crate) async fn execute(
        &self,
        tool: &str,
        connected_account_id: &str,
        user_id: &str,
        arguments: &Value,
    ) -> Result<Execution> {
        let body = json!({
            "connected_account_id": connected_account_id,
            "user_id": user_id,
            "arguments": arguments,
        });
        self.post(&format!("/tools/execute/{}", path_part(tool)), &body)
            .await
    }
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|err| {
        composio_error(format!(
            "Composio sent an answer that the hub cannot read: {err}."
        ))
    })
}

/// A slug or an ID as one path segment. Composio slugs and IDs are ASCII
/// letters, digits, `_` and `-`. Anything else is percent-encoded.
fn path_part(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.' {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A query name or value, percent-encoded.
fn query_part(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The status of the last error, when it came from Composio. The message
/// carries it as `(HTTP 404)`.
pub(crate) fn is_not_found(err: &HubError) -> bool {
    matches!(err, HubError::Api(body) if body.code == ErrorCode::ComposioError && body.message.contains("(HTTP 404)"))
}

/// Whether Composio refused the key.
pub(crate) fn is_unauthorized(err: &HubError) -> bool {
    matches!(err, HubError::Api(body) if body.code == ErrorCode::ComposioError && (body.message.contains("(HTTP 401)") || body.message.contains("(HTTP 403)")))
}

/// The error for an answer that is not a success. The body has
/// `{"error": {"message", "code", "slug", "status", "suggested_fix"}}`
/// (<https://docs.composio.dev/reference/errors>).
fn status_error(status: StatusCode, body: &Value) -> HubError {
    let message = body
        .pointer("/error/message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .map(|m| crate::util::truncate_chars(m.trim(), 300));
    let fix = body
        .pointer("/error/suggested_fix")
        .and_then(Value::as_str)
        .map(|m| crate::util::truncate_chars(m.trim(), 300));
    let code = status.as_u16();
    let text = match code {
        401 | 403 => "Composio refused the key. Check the key in Settings > Connectors.".to_owned(),
        429 => {
            "Composio says there are too many requests. Wait a moment, then try again.".to_owned()
        }
        _ => match (&message, &fix) {
            (Some(m), Some(f)) if !m.is_empty() => format!("Composio refused the request: {m} {f}"),
            (Some(m), _) if !m.is_empty() => format!("Composio refused the request: {m}"),
            _ => "Composio refused the request.".to_owned(),
        },
    };
    composio_error(format!("{text} (HTTP {code})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_never_shows_in_debug() {
        let key = ApiKey::new("ak_secret_value_1234".to_owned());
        assert_eq!(key.hint(), "1234");
        let client = Client::new("http://127.0.0.1:1", key).unwrap();
        let text = format!("{client:?}");
        assert!(!text.contains("secret"), "{text}");
    }

    #[test]
    fn identity_comes_only_from_named_keys() {
        let value = json!({
            "state": {"val": {"access_token": "tok", "email": "me@example.com"}},
        });
        assert_eq!(identity_of(&value).as_deref(), Some("me@example.com"));
        let value = json!({"state": {"val": {"access_token": "tok"}}});
        assert_eq!(identity_of(&value), None);
    }

    #[test]
    fn errors_name_the_status_and_hide_nothing_secret() {
        let err = status_error(
            StatusCode::NOT_FOUND,
            &json!({"error": {"message": "Not here", "code": 1, "slug": "x", "status": 404}}),
        );
        assert!(is_not_found(&err));
        assert!(err.to_error_body().message.contains("Not here"));
        let err = status_error(StatusCode::UNAUTHORIZED, &Value::Null);
        assert!(is_unauthorized(&err));
        assert_eq!(err.code(), ErrorCode::ComposioError);
    }

    #[test]
    fn path_parts_are_encoded() {
        assert_eq!(path_part("GMAIL_SEND_EMAIL"), "GMAIL_SEND_EMAIL");
        assert_eq!(path_part("a/b"), "a%2Fb");
    }
}
