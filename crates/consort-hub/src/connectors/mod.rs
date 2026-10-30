//! Connectors: the Composio key, the catalog, connections, connector calls
//! and the hub cards (ADR 0011, docs/CONNECTORS-SKILLS-PLAN.md).
//!
//! Only the hub talks to Composio. The key stays in the encrypted
//! `composio.key` (see [`key`]), and no request, log line or feed entry
//! holds it.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use consort_proto::client_api::{
    CATALOG_PAGE_MAX, ComposioGetStatus, ComposioRemoveKey, ComposioSetKey, ComposioStatusResult,
    ConnectorCatalogCategories, ConnectorCatalogCategoriesResult, ConnectorCatalogSearch,
    ConnectorCatalogSearchResult, ConnectorCatalogService, ConnectorServiceDetail,
};
use consort_proto::error::ErrorCode;
use consort_proto::ids::{ApprovalId, ConnectionId};
use consort_proto::model::ApprovalStatus;
use consort_proto::model::{
    AuthScheme, AuthSchemeKind, CATALOG_REFRESH_MS, CONNECT_ATTEMPT_TIMEOUT_MS, ComposioStatus,
    ComposioUsage, ConnectorCategory, ConnectorService, ConnectorToolSummary, CredentialField,
    ToolKind,
};
use consort_proto::time::Timestamp;
use tokio::sync::oneshot;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::store::{connectors as con_store, meta_get};

pub(crate) mod cards;
pub(crate) mod composio;
pub(crate) mod connections;
pub(crate) mod extensions;
pub(crate) mod key;
pub(crate) mod logos;
pub(crate) mod run;

use composio::{ApiKey, Client, KeyHealth, Tool, Toolkit};

const STATUS_META_KEY: &str = "composio_status";
const USER_ID_META_KEY: &str = "composio_user_id";
/// The longest key that the hub accepts. Composio keys are much shorter.
const KEY_MAX_CHARS: usize = 512;
/// How often the hub asks Composio about a connection that waits for a
/// browser sign-in.
const CONNECT_POLL_INTERVAL: Duration = Duration::from_secs(3);
/// How often the hub checks that active connections still work.
pub(crate) const STATUS_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// `run_connector_tool` stops waiting for a card this much before
/// `CONNECTOR_APPROVAL_WAIT_MS`. A worker gives up on a hub request after
/// 5 minutes too, so the `waiting_for_user` answer must reach it first.
const APPROVAL_WAIT_MARGIN: Duration = Duration::from_secs(15);
/// How long a failed catalog fetch keeps the hub from asking again.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(30);
/// The default page of `connector_catalog.search`.
const CATALOG_PAGE_DEFAULT: u32 = 50;
/// How old the last key check may be before `composio.status` checks again.
const STATUS_MAX_AGE_MS: i64 = 10 * 60 * 1_000;
/// What `composio.status` says when Composio refuses the key. It names the
/// next step.
pub(crate) const KEY_REFUSED_MESSAGE: &str = "Composio refuses the key now. It may be revoked or replaced. Copy the project key from the Composio dashboard, then add it again in Settings > Connectors. Connections stay as they are until then.";

/// The start of the month of `now` and the start of the next month, in
/// Unix milliseconds (UTC).
fn month_bounds(now: chrono::DateTime<chrono::Utc>) -> (i64, i64) {
    use chrono::{Datelike, TimeZone, Utc};
    let start = Utc
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or(now);
    let (year, month) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    let end = Utc
        .with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .unwrap_or(now);
    (start.timestamp_millis(), end.timestamp_millis())
}

/// The waits that tests make shorter.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timings {
    /// How long `run_connector_tool` waits for the answer to a card.
    pub approval_wait: Duration,
    /// How often a connect attempt asks Composio.
    pub poll_interval: Duration,
    /// How long a connect attempt waits for the browser sign-in.
    pub connect_timeout_ms: i64,
    /// How long one request to Composio may take. `None` is the default of
    /// the client.
    pub request_timeout: Option<Duration>,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            approval_wait: Duration::from_millis(consort_proto::tools::CONNECTOR_APPROVAL_WAIT_MS)
                .saturating_sub(APPROVAL_WAIT_MARGIN),
            poll_interval: CONNECT_POLL_INTERVAL,
            connect_timeout_ms: CONNECT_ATTEMPT_TIMEOUT_MS,
            request_timeout: None,
        }
    }
}

#[derive(Default)]
struct Catalog {
    fetched: Option<(Instant, Vec<Toolkit>, Vec<composio::Category>)>,
    failed: Option<(Instant, String)>,
}

/// The connector state of a running hub.
pub struct Connectors {
    keys: key::KeyStore,
    base_url: Mutex<String>,
    key: Mutex<Option<ApiKey>>,
    /// Whether Composio accepted the key at the last call.
    health: Arc<KeyHealth>,
    /// The logos of the services, fetched once.
    pub(crate) logos: logos::LogoCache,
    catalog: tokio::sync::Mutex<Catalog>,
    tools: tokio::sync::Mutex<HashMap<String, (Instant, Vec<Tool>)>>,
    /// Calls of `run_connector_tool` that wait for the answer to a card.
    waiters: Mutex<HashMap<ApprovalId, oneshot::Sender<ApprovalStatus>>>,
    /// Connections whose connect attempt a task follows.
    polling: Mutex<HashSet<ConnectionId>>,
    timings: Mutex<Timings>,
}

impl Connectors {
    /// The tool lists in the cache, by service, also stale ones. It never
    /// asks Composio.
    pub(crate) async fn cached_tools(&self) -> HashMap<String, Vec<Tool>> {
        self.tools
            .lock()
            .await
            .iter()
            .map(|(service, (_, tools))| (service.clone(), tools.clone()))
            .collect()
    }

    /// Reads the stored key. A key that cannot be read leaves the hub with
    /// no key, and a warning in the log.
    pub(crate) fn open(data_dir: &Path) -> Self {
        let keys = key::KeyStore::new(data_dir);
        let key = match keys.load() {
            Ok(key) => key,
            Err(err) => {
                tracing::warn!(error = %err, "could not read the Composio key, so connectors are off");
                None
            }
        };
        let base_url = std::env::var("CONSORT_COMPOSIO_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| composio::DEFAULT_BASE_URL.to_owned());
        Self {
            keys,
            base_url: Mutex::new(base_url),
            key: Mutex::new(key),
            health: Arc::default(),
            logos: {
                let cache = logos::LogoCache::open(data_dir);
                cache.clean();
                cache
            },
            catalog: tokio::sync::Mutex::default(),
            tools: tokio::sync::Mutex::default(),
            waiters: Mutex::default(),
            polling: Mutex::default(),
            timings: Mutex::new(Timings::default()),
        }
    }

    /// Uses another Composio API address, for example a local fake server
    /// in tests. It drops the cached catalog.
    pub(crate) fn set_base_url(&self, url: &str) {
        if let Ok(mut slot) = self.base_url.lock() {
            *slot = url.trim_end_matches('/').to_owned();
        }
        if let Ok(mut catalog) = self.catalog.try_lock() {
            *catalog = Catalog::default();
        }
        if let Ok(mut tools) = self.tools.try_lock() {
            tools.clear();
        }
    }

    fn base_url(&self) -> String {
        self.base_url
            .lock()
            .map(|u| u.clone())
            .unwrap_or_else(|_| composio::DEFAULT_BASE_URL.to_owned())
    }

    fn key(&self) -> Option<ApiKey> {
        self.key.lock().ok().and_then(|k| k.clone())
    }

    pub(crate) fn timings(&self) -> Timings {
        self.timings.lock().map(|t| *t).unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn set_timings(&self, timings: Timings) {
        if let Ok(mut slot) = self.timings.lock() {
            *slot = timings;
        }
    }

    /// A client for the stored key, or `composio_not_configured`.
    pub(crate) fn client(&self) -> Result<Client> {
        let key = self.key().ok_or_else(not_configured)?;
        let health = self.health.clone();
        match self.timings().request_timeout {
            Some(limit) => Client::with_timeout(&self.base_url(), key, health, limit),
            None => Client::with_health(&self.base_url(), key, health),
        }
    }

    /// Registers a call that waits for a card. The receiver gets the
    /// answer.
    pub(crate) fn wait_for(&self, approval_id: ApprovalId) -> oneshot::Receiver<ApprovalStatus> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut waiters) = self.waiters.lock() {
            waiters.insert(approval_id, tx);
        }
        rx
    }

    /// Takes the waiter of a card. Whoever takes it owns the answer: the
    /// call that stops waiting, or the answer that finds it.
    pub(crate) fn take_waiter(
        &self,
        approval_id: ApprovalId,
    ) -> Option<oneshot::Sender<ApprovalStatus>> {
        self.waiters.lock().ok()?.remove(&approval_id)
    }

    /// Marks a connection as followed. False when a task follows it already.
    fn start_polling(&self, id: ConnectionId) -> bool {
        self.polling.lock().is_ok_and(|mut p| p.insert(id))
    }

    fn stop_polling(&self, id: ConnectionId) {
        if let Ok(mut p) = self.polling.lock() {
            p.remove(&id);
        }
    }
}

pub(crate) fn not_configured() -> HubError {
    HubError::new(
        ErrorCode::ComposioNotConfigured,
        "Connectors need a Composio key. Ask the user to add one in Settings > Connectors.",
    )
}

/// The scheme name in the catalog (`oauth2`, `api_key`, ...).
pub(crate) fn scheme_kind(name: &str) -> AuthSchemeKind {
    let lower = name.trim().to_ascii_lowercase();
    let lower = match lower.as_str() {
        "basic_auth" => "basic".to_owned(),
        "bearer" => "bearer_token".to_owned(),
        _ => lower,
    };
    serde_json::from_value(serde_json::Value::String(lower)).unwrap_or(AuthSchemeKind::Other)
}

/// The scheme name that `POST /auth_configs` and `POST /connected_accounts`
/// take (`OAUTH2`, `API_KEY`, ...).
pub(crate) fn scheme_wire(kind: AuthSchemeKind) -> Option<&'static str> {
    Some(match kind {
        AuthSchemeKind::Oauth2 => "OAUTH2",
        AuthSchemeKind::Oauth1 => "OAUTH1",
        AuthSchemeKind::ApiKey => "API_KEY",
        AuthSchemeKind::BearerToken => "BEARER_TOKEN",
        AuthSchemeKind::Basic => "BASIC",
        AuthSchemeKind::NoAuth => "NO_AUTH",
        AuthSchemeKind::Other => return None,
    })
}

/// The tool kind from Composio's hints (DECISIONS row 348).
pub(crate) fn tool_kind(tool: &Tool) -> ToolKind {
    ToolKind::from_hints(
        tool.has_tag("readOnlyHint"),
        tool.has_tag("createHint"),
        tool.has_tag("updateHint"),
        tool.has_tag("destructiveHint"),
    )
}

fn service_of(toolkit: &Toolkit, connected_count: u32) -> ConnectorService {
    let managed: HashSet<String> = toolkit
        .composio_managed_auth_schemes
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let mut auth_schemes: Vec<AuthScheme> = toolkit
        .auth_schemes
        .iter()
        .map(|s| AuthScheme {
            kind: scheme_kind(s),
            managed: managed.contains(&s.to_ascii_lowercase()),
            fields: Vec::new(),
        })
        .collect();
    if auth_schemes.is_empty() && toolkit.no_auth {
        auth_schemes.push(AuthScheme {
            kind: AuthSchemeKind::NoAuth,
            managed: false,
            fields: Vec::new(),
        });
    }
    ConnectorService {
        slug: toolkit.slug.clone(),
        name: toolkit.name.clone(),
        description: toolkit.meta.description.clone(),
        // The URL that Composio gives. The hub swaps it for its own URL
        // (`Hub::hub_logo_url`) before a client sees it.
        logo_url: toolkit.meta.logo.clone(),
        categories: toolkit.meta.categories.iter().map(|c| c.key()).collect(),
        tool_count: toolkit
            .meta
            .tools_count
            .map_or(0, |n| n.max(0.0).min(f64::from(u32::MAX)) as u32),
        auth_schemes,
        connected_count,
    }
}

fn matches_search(toolkit: &Toolkit, search: &str) -> bool {
    let search = search.trim().to_lowercase();
    search.is_empty()
        || toolkit.slug.to_lowercase().contains(&search)
        || toolkit.name.to_lowercase().contains(&search)
        || toolkit.meta.description.to_lowercase().contains(&search)
}

fn in_category(toolkit: &Toolkit, category: &str) -> bool {
    let category = category.trim().to_lowercase();
    toolkit
        .meta
        .categories
        .iter()
        .any(|c| c.key().to_lowercase() == category || c.name.to_lowercase() == category)
}

impl Hub {
    /// The hub's own URL for a logo that lives at `origin`, and the logo is
    /// fetched into the cache when a client asks for it. `None` when there is
    /// no logo, or the origin is not one that the hub fetches. A URL that is
    /// the hub's own already stays as it is. With `persist`, the key works
    /// after a restart too (for a record that the hub stores).
    pub(crate) fn hub_logo_url(&self, origin: Option<&str>, persist: bool) -> Option<String> {
        let origin = origin?.trim();
        if origin.is_empty() {
            return None;
        }
        // The hub's own URL, maybe under an older address of the hub.
        if let Some((_, tail)) = origin.rsplit_once(logos::ROUTE_PREFIX)
            && self.connectors.logos.knows(tail)
        {
            return Some(format!("{}{}{tail}", self.logo_base(), logos::ROUTE_PREFIX));
        }
        let key = self.connectors.logos.register(origin, persist)?;
        Some(format!("{}{}{key}", self.logo_base(), logos::ROUTE_PREFIX))
    }

    fn logo_base(&self) -> String {
        self.settings.public_url.trim_end_matches('/').to_owned()
    }

    /// Gives the connections that were stored with a logo URL of Composio,
    /// or of an older address of the hub, the hub's current logo URL. It
    /// starts the fetch of each logo, so it is in the cache when the app
    /// opens.
    pub(crate) async fn refresh_connection_logos(&self) -> Result<()> {
        let connections = self.store.read(con_store::connections).await?;
        for connection in connections {
            let Some(current) = connection.logo_url.clone() else {
                continue;
            };
            let Some(logo_url) = self.hub_logo_url(Some(&current), true) else {
                continue;
            };
            if let Some((_, key)) = logo_url.rsplit_once(logos::ROUTE_PREFIX) {
                let (hub, key) = (self.clone(), key.to_owned());
                tokio::spawn(async move {
                    hub.connectors.logos.get(&key).await;
                });
            }
            if logo_url != current {
                let id = connection.id;
                self.store
                    .write(move |tx| {
                        if let Some(mut c) = con_store::connection(tx.db(), id)? {
                            c.logo_url = Some(logo_url);
                            tx.save_connection(&c)?;
                        }
                        Ok(())
                    })
                    .await?;
            }
        }
        Ok(())
    }

    async fn composio_status_stored(&self) -> Result<ComposioStatus> {
        let stored = self
            .store
            .read(|conn| meta_get(conn, STATUS_META_KEY))
            .await?;
        let mut status: ComposioStatus = stored
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        let key = self.connectors.key();
        status.key_present = key.is_some();
        status.key_hint = key.map(|k| k.hint());
        if !status.key_present {
            return Ok(ComposioStatus::default());
        }
        // A call that Composio refused since the last check shows at once.
        if self.connectors.health.is_refused() {
            status.error = Some(KEY_REFUSED_MESSAGE.to_owned());
        }
        status.usage = Some(self.composio_usage().await?);
        Ok(status)
    }

    /// The tool calls that this hub sent to Composio in the current month
    /// (UTC). Composio gives no usage to a project key (its billing calls
    /// need a dashboard login), so the hub counts its own audit log. The
    /// plan limit is not known, so `limit` is empty.
    async fn composio_usage(&self) -> Result<ComposioUsage> {
        let now =
            chrono::DateTime::from_timestamp_millis(self.now().as_millis()).unwrap_or_default();
        let (start, end) = month_bounds(now);
        let used = self
            .store
            .read(move |conn| con_store::calls_between(conn, start, end))
            .await?;
        Ok(ComposioUsage {
            used,
            limit: None,
            resets_at: Some(Timestamp::from_millis(end)),
        })
    }

    /// Asks Composio whether it still accepts the key, and stores the
    /// answer. A network failure changes nothing, because it says nothing
    /// about the key. Does nothing when the hub has no key.
    pub(crate) async fn refresh_composio_status(&self) -> Result<()> {
        let Ok(client) = self.connectors.client() else {
            return Ok(());
        };
        let mut status = self.composio_status_stored().await?;
        match client.check_key().await {
            Ok(()) => status.error = None,
            Err(err) if composio::is_unauthorized(&err) => {
                status.error = Some(KEY_REFUSED_MESSAGE.to_owned());
            }
            Err(err) => {
                tracing::warn!(error = %err, "could not check the Composio key");
                return Ok(());
            }
        }
        status.checked_at = Some(self.now());
        // The usage is not stored. It is counted each time.
        status.usage = None;
        self.save_composio_status(&status).await
    }

    async fn save_composio_status(&self, status: &ComposioStatus) -> Result<()> {
        let text = serde_json::to_string(status)?;
        self.store
            .write(move |tx| tx.meta_set(STATUS_META_KEY, &text))
            .await
    }

    /// The hub's one Composio `user_id`. The hub makes it the first time.
    pub(crate) async fn composio_user_id(&self) -> Result<String> {
        self.store
            .write(|tx| {
                if let Some(id) = meta_get(tx.db(), USER_ID_META_KEY)? {
                    return Ok(id);
                }
                let id = format!("consort-hub-{}", uuid::Uuid::now_v7().simple());
                tx.meta_set(USER_ID_META_KEY, &id)?;
                Ok(id)
            })
            .await
    }

    pub(crate) async fn composio_set_key(
        &self,
        _caller: &Caller,
        request: ComposioSetKey,
    ) -> Result<ComposioStatusResult> {
        let key = request.api_key.trim().to_owned();
        if key.is_empty() {
            return Err(HubError::invalid(
                "The key is empty. Paste the Composio project key.",
            ));
        }
        if key.chars().count() > KEY_MAX_CHARS
            || key.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(HubError::invalid(
                "This does not look like a Composio key. Copy the project key again from the Composio dashboard.",
            ));
        }
        let key = ApiKey::new(key);
        let client = Client::with_health(
            &self.connectors.base_url(),
            key.clone(),
            self.connectors.health.clone(),
        )?;
        if let Err(err) = client.check_key().await {
            if composio::is_unauthorized(&err) {
                return Err(HubError::invalid(
                    "Composio refused this key. Copy the project key again from the Composio dashboard.",
                ));
            }
            return Err(err);
        }
        let keys = self.connectors.keys.clone();
        let stored = key.clone();
        tokio::task::spawn_blocking(move || keys.save(&stored)).await??;
        if let Ok(mut slot) = self.connectors.key.lock() {
            *slot = Some(key.clone());
        }
        let status = ComposioStatus {
            key_present: true,
            key_hint: Some(key.hint()),
            plan: None,
            usage: None,
            error: None,
            checked_at: Some(self.now()),
        };
        self.save_composio_status(&status).await?;
        tracing::info!("the Composio key changed");
        // The new key may belong to another Composio project.
        self.reconcile_connections_with_key().await?;
        Ok(ComposioStatusResult {
            status: self.composio_status_stored().await?,
        })
    }

    pub(crate) async fn composio_remove_key(
        &self,
        _caller: &Caller,
        _request: ComposioRemoveKey,
    ) -> Result<ComposioStatusResult> {
        let keys = self.connectors.keys.clone();
        tokio::task::spawn_blocking(move || keys.remove()).await??;
        if let Ok(mut slot) = self.connectors.key.lock() {
            *slot = None;
        }
        let status = ComposioStatus::default();
        self.save_composio_status(&status).await?;
        tracing::info!("the Composio key was removed");
        Ok(ComposioStatusResult { status })
    }

    pub(crate) async fn composio_get_status(
        &self,
        _caller: &Caller,
        _request: ComposioGetStatus,
    ) -> Result<ComposioStatusResult> {
        // Settings asks for the status when the page opens. A check that is
        // older than a few minutes runs again, so a key that Composio
        // refuses later shows here.
        let stored = self.composio_status_stored().await?;
        let stale = stored.key_present
            && stored
                .checked_at
                .is_none_or(|at| self.now().as_millis() - at.as_millis() > STATUS_MAX_AGE_MS);
        if stale {
            self.refresh_composio_status().await?;
        }
        Ok(ComposioStatusResult {
            status: self.composio_status_stored().await?,
        })
    }

    // Catalog ----------------------------------------------------------------

    /// The toolkits and categories, from the cache. A copy older than a day
    /// is fetched again; when that fails, the old copy answers.
    async fn catalog(&self) -> Result<(Vec<Toolkit>, Vec<composio::Category>)> {
        let client = self.connectors.client()?;
        let mut catalog = self.connectors.catalog.lock().await;
        let ttl = Duration::from_millis(CATALOG_REFRESH_MS.unsigned_abs());
        if let Some((at, toolkits, categories)) = &catalog.fetched
            && at.elapsed() < ttl
        {
            return Ok((toolkits.clone(), categories.clone()));
        }
        if let Some((at, reason)) = &catalog.failed
            && at.elapsed() < RETRY_AFTER_FAILURE
        {
            return match &catalog.fetched {
                Some((_, t, c)) => Ok((t.clone(), c.clone())),
                None => Err(composio::composio_error(reason.clone())),
            };
        }
        let fetched = async {
            let toolkits = client.toolkits().await?;
            // The category list is extra. The toolkits name their categories.
            let categories = client.categories().await.unwrap_or_default();
            Ok::<_, HubError>((toolkits, categories))
        }
        .await;
        match fetched {
            Ok((toolkits, categories)) => {
                catalog.failed = None;
                catalog.fetched = Some((Instant::now(), toolkits.clone(), categories.clone()));
                Ok((toolkits, categories))
            }
            Err(err) => {
                let reason = err.to_error_body().message;
                tracing::warn!(%reason, "could not fetch the connector catalog");
                catalog.failed = Some((Instant::now(), reason));
                match &catalog.fetched {
                    Some((_, t, c)) => Ok((t.clone(), c.clone())),
                    None => Err(err),
                }
            }
        }
    }

    /// The tools of one service, from the cache (one day).
    pub(crate) async fn service_tools(&self, service: &str) -> Result<Vec<Tool>> {
        let client = self.connectors.client()?;
        let ttl = Duration::from_millis(CATALOG_REFRESH_MS.unsigned_abs());
        let mut cache = self.connectors.tools.lock().await;
        if let Some((at, tools)) = cache.get(service)
            && at.elapsed() < ttl
        {
            return Ok(tools.clone());
        }
        match client.tools(service).await {
            Ok(tools) => {
                cache.insert(service.to_owned(), (Instant::now(), tools.clone()));
                Ok(tools)
            }
            Err(err) => match cache.get(service) {
                Some((_, stale)) => Ok(stale.clone()),
                None => Err(err),
            },
        }
    }

    async fn connected_counts(&self) -> Result<HashMap<String, u32>> {
        let connections = self.store.read(con_store::connections).await?;
        let mut counts = HashMap::new();
        for c in connections {
            *counts.entry(c.service).or_insert(0) += 1;
        }
        Ok(counts)
    }

    /// Services that match a search, in Composio's order (most used first).
    pub(crate) async fn search_services(
        &self,
        search: Option<&str>,
        category: Option<&str>,
    ) -> Result<Vec<ConnectorService>> {
        let (toolkits, _) = self.catalog().await?;
        let counts = self.connected_counts().await?;
        Ok(toolkits
            .iter()
            .filter(|t| search.is_none_or(|s| matches_search(t, s)))
            .filter(|t| category.is_none_or(|c| c.trim().is_empty() || in_category(t, c)))
            .map(|t| {
                let mut service = service_of(t, counts.get(&t.slug).copied().unwrap_or(0));
                service.logo_url = self.hub_logo_url(service.logo_url.as_deref(), false);
                service
            })
            .collect())
    }

    pub(crate) async fn connector_catalog_search(
        &self,
        _caller: &Caller,
        request: ConnectorCatalogSearch,
    ) -> Result<ConnectorCatalogSearchResult> {
        let limit = request
            .limit
            .unwrap_or(CATALOG_PAGE_DEFAULT)
            .clamp(1, CATALOG_PAGE_MAX) as usize;
        let offset: usize = match request.cursor.as_deref() {
            None | Some("") => 0,
            Some(c) => c.parse().map_err(|_| {
                HubError::invalid("The cursor is not valid. Start the search again.")
            })?,
        };
        let services = self
            .search_services(request.search.as_deref(), request.category.as_deref())
            .await?;
        let end = offset.saturating_add(limit).min(services.len());
        let next_cursor = (end < services.len()).then(|| end.to_string());
        let page = services
            .get(offset.min(services.len())..end)
            .unwrap_or_default()
            .to_vec();
        Ok(ConnectorCatalogSearchResult {
            services: page,
            next_cursor,
        })
    }

    pub(crate) async fn connector_catalog_categories(
        &self,
        _caller: &Caller,
        _request: ConnectorCatalogCategories,
    ) -> Result<ConnectorCatalogCategoriesResult> {
        let (toolkits, categories) = self.catalog().await?;
        let mut counts: HashMap<String, (String, u32)> = HashMap::new();
        for toolkit in &toolkits {
            for c in &toolkit.meta.categories {
                let entry = counts.entry(c.key()).or_insert_with(|| (c.name.clone(), 0));
                entry.1 += 1;
            }
        }
        let mut out: Vec<ConnectorCategory> = if categories.is_empty() {
            counts
                .iter()
                .map(|(slug, (name, n))| ConnectorCategory {
                    slug: slug.clone(),
                    name: name.clone(),
                    service_count: Some(*n),
                })
                .collect()
        } else {
            categories
                .iter()
                .map(|c| ConnectorCategory {
                    slug: c.id.clone(),
                    name: c.name.clone(),
                    service_count: counts.get(&c.id).map(|(_, n)| *n),
                })
                .collect()
        };
        out.sort_by_key(|a| a.name.to_lowercase());
        Ok(ConnectorCatalogCategoriesResult { categories: out })
    }

    /// One service with its auth schemes, their fields and its tools.
    pub(crate) async fn service_detail(&self, service: &str) -> Result<ConnectorServiceDetail> {
        let client = self.connectors.client()?;
        let slug = service.trim().to_ascii_lowercase();
        let detail = client.toolkit(&slug).await.map_err(|err| {
            if composio::is_not_found(&err) {
                HubError::not_found(format!(
                    "Composio has no service {slug}. Search the catalog for its name."
                ))
            } else {
                err
            }
        })?;
        let counts = self.connected_counts().await?;
        let listed = self
            .catalog()
            .await
            .ok()
            .and_then(|(t, _)| t.into_iter().find(|t| t.slug == detail.slug));
        let toolkit = listed.unwrap_or_else(|| Toolkit {
            slug: detail.slug.clone(),
            name: detail.name.clone(),
            auth_schemes: detail
                .auth_config_details
                .iter()
                .map(|d| d.mode.clone())
                .collect(),
            composio_managed_auth_schemes: detail.composio_managed_auth_schemes.clone(),
            no_auth: detail.no_auth,
            meta: detail.meta.clone(),
        });
        let mut service = service_of(&toolkit, counts.get(&detail.slug).copied().unwrap_or(0));
        service.logo_url = self.hub_logo_url(service.logo_url.as_deref(), false);
        for scheme in &mut service.auth_schemes {
            if let Some(details) = detail
                .auth_config_details
                .iter()
                .find(|d| scheme_kind(&d.mode) == scheme.kind)
            {
                let group = &details.fields.connected_account_initiation;
                scheme.fields = group
                    .required
                    .iter()
                    .map(|f| (f, true))
                    .chain(group.optional.iter().map(|f| (f, false)))
                    .map(|(f, required)| CredentialField {
                        name: f.name.clone(),
                        label: if f.display_name.trim().is_empty() {
                            f.name.clone()
                        } else {
                            f.display_name.clone()
                        },
                        description: Some(f.description.clone()).filter(|d| !d.trim().is_empty()),
                        // A field is secret unless Composio says it is not.
                        secret: f.is_secret.unwrap_or(true),
                        required,
                    })
                    .collect();
            }
        }
        let tools = self
            .service_tools(&detail.slug)
            .await?
            .iter()
            .map(|t| ConnectorToolSummary {
                slug: t.slug.clone(),
                description: t.description.clone(),
                kind: tool_kind(t),
            })
            .collect();
        Ok(ConnectorServiceDetail { service, tools })
    }

    pub(crate) async fn connector_catalog_service(
        &self,
        _caller: &Caller,
        request: ConnectorCatalogService,
    ) -> Result<ConnectorServiceDetail> {
        self.service_detail(&request.service).await
    }
}

#[cfg(test)]
pub(crate) mod fake;

#[cfg(test)]
mod flow_tests;

#[cfg(test)]
mod status_tests;

#[cfg(test)]
impl fake::FakeComposio {
    /// Points the hub at this fake and stores the fake's key.
    pub(crate) async fn configure(&self, hub: &Hub) {
        hub.connectors.set_base_url(&self.url);
        hub.composio_set_key(
            &Caller::Hub,
            ComposioSetKey {
                api_key: fake::GOOD_KEY.to_owned(),
            },
        )
        .await
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::test_hub;

    #[tokio::test]
    async fn a_good_key_is_stored_encrypted_and_reported() {
        let (hub, dir) = test_hub().await;
        let fake = fake::FakeComposio::start().await;
        hub.connectors.set_base_url(&fake.url);
        let status = hub
            .composio_set_key(
                &Caller::Hub,
                ComposioSetKey {
                    api_key: fake::GOOD_KEY.to_owned(),
                },
            )
            .await
            .unwrap()
            .status;
        assert!(status.key_present);
        assert_eq!(
            status.key_hint.as_deref(),
            Some(&fake::GOOD_KEY[fake::GOOD_KEY.len() - 4..])
        );
        let raw = std::fs::read(dir.path().join("composio.key")).unwrap();
        assert!(
            !raw.windows(fake::GOOD_KEY.len())
                .any(|w| w == fake::GOOD_KEY.as_bytes())
        );
        // The key never goes into the feed or the database.
        let db = std::fs::read(dir.path().join("hub.db")).unwrap();
        assert!(
            !db.windows(fake::GOOD_KEY.len())
                .any(|w| w == fake::GOOD_KEY.as_bytes())
        );
        let again = hub
            .composio_get_status(&Caller::Hub, ComposioGetStatus {})
            .await
            .unwrap()
            .status;
        // The status holds the usage of this month.
        assert_eq!(again.usage.as_ref().map(|u| u.used), Some(0));
        assert_eq!(again, status);
        let removed = hub
            .composio_remove_key(&Caller::Hub, ComposioRemoveKey {})
            .await
            .unwrap()
            .status;
        assert!(!removed.key_present);
        assert!(!dir.path().join("composio.key").exists());
    }

    #[tokio::test]
    async fn a_refused_key_is_not_stored() {
        let (hub, dir) = test_hub().await;
        let fake = fake::FakeComposio::start().await;
        hub.connectors.set_base_url(&fake.url);
        let err = hub
            .composio_set_key(
                &Caller::Hub,
                ComposioSetKey {
                    api_key: "ak_wrong".to_owned(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::Invalid);
        assert!(!dir.path().join("composio.key").exists());
        let err = hub
            .connector_catalog_search(&Caller::Hub, ConnectorCatalogSearch::default())
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ComposioNotConfigured);
    }

    #[tokio::test]
    async fn the_catalog_searches_pages_and_caches() {
        let (hub, _dir) = test_hub().await;
        let fake = fake::FakeComposio::start().await;
        fake.configure(&hub).await;
        let page = hub
            .connector_catalog_search(
                &Caller::Hub,
                ConnectorCatalogSearch {
                    limit: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.services.len(), 1);
        assert_eq!(page.next_cursor.as_deref(), Some("1"));
        let mail = hub
            .connector_catalog_search(
                &Caller::Hub,
                ConnectorCatalogSearch {
                    search: Some("gmail".to_owned()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(mail.services.len(), 1);
        let gmail = &mail.services[0];
        assert_eq!(gmail.slug, "gmail");
        // The hub gives its own URL, not the one of Composio.
        let logo = gmail.logo_url.as_deref().unwrap();
        assert!(logo.contains("/v1/logos/"), "{logo}");
        assert!(!logo.contains("logos.example"), "{logo}");
        assert!(
            gmail
                .auth_schemes
                .iter()
                .any(|s| s.kind == AuthSchemeKind::Oauth2 && s.managed)
        );
        let by_category = hub
            .connector_catalog_search(
                &Caller::Hub,
                ConnectorCatalogSearch {
                    category: Some("developer-tools".to_owned()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(by_category.services.len(), 1);
        assert_eq!(by_category.services[0].slug, "github");
        // One fetch of the toolkit list served the three searches. The
        // other call checked the key.
        assert_eq!(fake.count("GET /toolkits"), 2);
        let categories = hub
            .connector_catalog_categories(&Caller::Hub, ConnectorCatalogCategories {})
            .await
            .unwrap()
            .categories;
        assert!(
            categories
                .iter()
                .any(|c| c.slug == "developer-tools" && c.service_count == Some(1))
        );
    }

    #[tokio::test]
    async fn a_service_has_its_fields_and_tool_kinds() {
        let (hub, _dir) = test_hub().await;
        let fake = fake::FakeComposio::start().await;
        fake.configure(&hub).await;
        let detail = hub
            .connector_catalog_service(
                &Caller::Hub,
                ConnectorCatalogService {
                    service: "sendgrid".to_owned(),
                },
            )
            .await
            .unwrap();
        let scheme = &detail.service.auth_schemes[0];
        assert_eq!(scheme.kind, AuthSchemeKind::ApiKey);
        assert_eq!(scheme.fields[0].name, "api_key");
        assert!(scheme.fields[0].secret && scheme.fields[0].required);
        let detail = hub
            .connector_catalog_service(
                &Caller::Hub,
                ConnectorCatalogService {
                    service: "gmail".to_owned(),
                },
            )
            .await
            .unwrap();
        let kind = |slug: &str| detail.tools.iter().find(|t| t.slug == slug).unwrap().kind;
        assert_eq!(kind("GMAIL_FETCH_EMAILS"), ToolKind::Read);
        assert_eq!(kind("GMAIL_SEND_EMAIL"), ToolKind::Change);
        assert_eq!(kind("GMAIL_DELETE_MESSAGE"), ToolKind::Delete);
    }
}
