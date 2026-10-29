//! `acp_registry.list`: the agents in the ACP registry.
//!
//! The hub fetches the registry file with its own HTTP client and keeps the
//! answer in memory for one hour. When a later fetch fails, the hub answers
//! with the stale copy. It fails only when it never had a copy.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use consort_proto::client_api::{AcpRegistryList, AcpRegistryListResult};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::model::AcpRegistryAgent;
use consort_tools::registry::{REGISTRY_URL, Registry};

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};

/// How long the hub keeps a fetched registry.
pub(crate) const REGISTRY_TTL: Duration = Duration::from_secs(60 * 60);
/// How long the hub waits after a failed fetch before it tries again.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(30);
/// The longest wait for the registry server.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// The registry file is about 100 KiB. This limit stops a wrong server.
const MAX_REGISTRY_BYTES: usize = 8 * 1024 * 1024;

struct Fetched {
    at: Instant,
    agents: Vec<AcpRegistryAgent>,
}

#[derive(Default)]
struct State {
    fetched: Option<Fetched>,
    /// When the last fetch failed, and why.
    failed: Option<(Instant, String)>,
}

/// The registry of ACP agents, with its cache.
pub struct RegistryCache {
    url: Mutex<String>,
    ttl: Mutex<Duration>,
    state: tokio::sync::Mutex<State>,
}

impl Default for RegistryCache {
    fn default() -> Self {
        Self {
            url: Mutex::new(REGISTRY_URL.to_owned()),
            ttl: Mutex::new(REGISTRY_TTL),
            state: tokio::sync::Mutex::default(),
        }
    }
}

impl RegistryCache {
    /// Uses another registry address. Tests use it with a local server.
    #[cfg(test)]
    pub(crate) fn set_url(&self, url: &str) {
        if let Ok(mut slot) = self.url.lock() {
            *slot = url.to_owned();
        }
    }

    /// Changes how long a fetched registry stays valid.
    #[cfg(test)]
    pub(crate) fn set_ttl(&self, ttl: Duration) {
        if let Ok(mut slot) = self.ttl.lock() {
            *slot = ttl;
        }
    }

    fn url(&self) -> String {
        self.url
            .lock()
            .map(|u| u.clone())
            .unwrap_or_else(|_| REGISTRY_URL.to_owned())
    }

    fn ttl(&self) -> Duration {
        self.ttl.lock().map(|t| *t).unwrap_or(REGISTRY_TTL)
    }

    /// The agents. A fresh copy comes from the cache. An old copy is fetched
    /// again, and the old one answers when the fetch fails.
    pub(crate) async fn agents(&self) -> Result<Vec<AcpRegistryAgent>> {
        // One request at a time, so a burst of clients fetches once.
        let mut state = self.state.lock().await;
        let ttl = self.ttl();
        if let Some(fetched) = &state.fetched
            && fetched.at.elapsed() < ttl
        {
            return Ok(fetched.agents.clone());
        }
        // After a failure, do not ask the server again at once.
        if let Some((at, reason)) = &state.failed
            && at.elapsed() < RETRY_AFTER_FAILURE
        {
            return match &state.fetched {
                Some(stale) => Ok(stale.agents.clone()),
                None => Err(unavailable(reason)),
            };
        }
        match fetch(&self.url()).await {
            Ok(agents) => {
                state.failed = None;
                state.fetched = Some(Fetched {
                    at: Instant::now(),
                    agents: agents.clone(),
                });
                Ok(agents)
            }
            Err(reason) => {
                tracing::warn!(%reason, "could not fetch the ACP registry");
                state.failed = Some((Instant::now(), reason.clone()));
                match &state.fetched {
                    Some(stale) => Ok(stale.agents.clone()),
                    None => Err(unavailable(&reason)),
                }
            }
        }
    }
}

fn unavailable(reason: &str) -> HubError {
    HubError::Api(ErrorBody::new(
        ErrorCode::Internal,
        format!(
            "The hub could not load the ACP registry ({reason}). Check that the hub can reach the internet, then try again."
        ),
    ))
}

/// Downloads and parses the registry. The error is a short reason for the
/// user.
async fn fetch(url: &str) -> std::result::Result<Vec<AcpRegistryAgent>, String> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("consort-hub/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "the HTTP client did not start".to_owned())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| format!("the request failed: {}", err.without_url()))?;
    if !response.status().is_success() {
        return Err(format!("the server answered {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_REGISTRY_BYTES as u64)
    {
        return Err("the file is too large".to_owned());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("the download failed: {}", err.without_url()))?;
    if bytes.len() > MAX_REGISTRY_BYTES {
        return Err("the file is too large".to_owned());
    }
    let registry = Registry::parse(&bytes).map_err(|_| "the file is not a registry".to_owned())?;
    Ok(registry
        .agents
        .iter()
        .map(|agent| AcpRegistryAgent {
            id: agent.id.clone(),
            name: agent.name.clone(),
            description: agent.description.clone(),
            version: agent.version.clone(),
            icon_url: agent.icon.clone(),
            supported: agent.supported_anywhere(),
        })
        .collect())
}

impl Hub {
    pub(crate) async fn acp_registry_list(
        &self,
        _caller: &Caller,
        _request: AcpRegistryList,
    ) -> Result<AcpRegistryListResult> {
        Ok(AcpRegistryListResult {
            agents: self.registry.agents().await?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::get;

    use super::*;
    use crate::hub::test_hub;

    const REGISTRY: &str = r#"{
        "version": "1.0.0",
        "agents": [
            {"id": "goose", "name": "Goose", "version": "1.2.3",
             "description": "A local agent.",
             "icon": "https://example.test/goose.svg",
             "distribution": {"binary": {"darwin-aarch64": {
                 "archive": "https://example.test/goose.tar.gz", "cmd": "goose",
                 "sha256": "abc"}}}},
            {"id": "cline", "name": "Cline", "version": "2.0.0",
             "description": "An npm agent.",
             "distribution": {"npx": {"package": "cline@2.0.0"}}},
            {"id": "unsafe", "name": "Unsafe", "version": "0.1.0",
             "description": "A binary without a checksum.",
             "distribution": {"binary": {"linux-x86_64": {
                 "archive": "https://example.test/unsafe.tar.gz", "cmd": "unsafe"}}}},
            {"id": "py", "name": "Py", "version": "0.2.0",
             "description": "A Python agent.",
             "distribution": {"uvx": {"package": "py-agent"}}}
        ]
    }"#;

    #[derive(Default)]
    struct Server {
        hits: AtomicUsize,
        broken: AtomicBool,
    }

    /// Serves the registry on a local port. Returns the address of the
    /// registry file.
    async fn serve_registry() -> (String, Arc<Server>) {
        let server = Arc::new(Server::default());
        async fn registry(State(server): State<Arc<Server>>) -> (StatusCode, &'static str) {
            server.hits.fetch_add(1, Ordering::SeqCst);
            if server.broken.load(Ordering::SeqCst) {
                (StatusCode::INTERNAL_SERVER_ERROR, "broken")
            } else {
                (StatusCode::OK, REGISTRY)
            }
        }
        let app = Router::new()
            .route("/registry.json", get(registry))
            .with_state(server.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/registry.json", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, server)
    }

    async fn list(hub: &Hub) -> Result<AcpRegistryListResult> {
        hub.acp_registry_list(&Caller::Hub, AcpRegistryList {})
            .await
    }

    #[tokio::test]
    async fn the_registry_maps_to_agents_with_a_supported_flag() {
        let (hub, _dir) = test_hub().await;
        let (url, _server) = serve_registry().await;
        hub.registry.set_url(&url);
        let agents = list(&hub).await.unwrap().agents;
        let by_id = |id: &str| agents.iter().find(|a| a.id == id).unwrap();
        assert_eq!(agents.len(), 4);
        let goose = by_id("goose");
        assert_eq!(goose.name, "Goose");
        assert_eq!(goose.version, "1.2.3");
        assert_eq!(goose.description, "A local agent.");
        assert_eq!(
            goose.icon_url.as_deref(),
            Some("https://example.test/goose.svg")
        );
        assert!(goose.supported, "a binary with a checksum is supported");
        assert!(by_id("cline").supported, "npx is supported");
        assert!(by_id("cline").icon_url.is_none());
        assert!(!by_id("unsafe").supported, "a binary without a checksum");
        assert!(!by_id("py").supported, "uvx is not supported yet");
    }

    #[tokio::test]
    async fn the_hub_caches_the_registry_until_it_gets_old() {
        let (hub, _dir) = test_hub().await;
        let (url, server) = serve_registry().await;
        hub.registry.set_url(&url);
        list(&hub).await.unwrap();
        list(&hub).await.unwrap();
        assert_eq!(
            server.hits.load(Ordering::SeqCst),
            1,
            "the second call is cached"
        );
        hub.registry.set_ttl(Duration::ZERO);
        list(&hub).await.unwrap();
        assert_eq!(
            server.hits.load(Ordering::SeqCst),
            2,
            "an old copy is fetched again"
        );
    }

    #[test]
    fn the_cache_lives_for_one_hour() {
        assert_eq!(REGISTRY_TTL, Duration::from_secs(3600));
        assert_eq!(
            RegistryCache::default().url(),
            "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json"
        );
    }

    #[tokio::test]
    async fn a_failed_fetch_answers_with_the_stale_copy() {
        let (hub, _dir) = test_hub().await;
        let (url, server) = serve_registry().await;
        hub.registry.set_url(&url);
        let first = list(&hub).await.unwrap();
        hub.registry.set_ttl(Duration::ZERO);
        server.broken.store(true, Ordering::SeqCst);
        let second = list(&hub).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(
            server.hits.load(Ordering::SeqCst),
            2,
            "the hub tried to fetch"
        );
    }

    #[tokio::test]
    async fn a_failed_fetch_without_a_copy_gives_a_clear_error() {
        let (hub, _dir) = test_hub().await;
        let (url, server) = serve_registry().await;
        server.broken.store(true, Ordering::SeqCst);
        hub.registry.set_url(&url);
        let error = list(&hub).await.unwrap_err();
        let body = error.to_error_body();
        assert_eq!(body.code, ErrorCode::Internal);
        assert!(
            body.message.contains("could not load the ACP registry")
                && body.message.contains("500"),
            "{}",
            body.message
        );
        // The hub does not ask the server again at once.
        list(&hub).await.unwrap_err();
        assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn text_that_is_not_a_registry_is_an_error() {
        let (hub, _dir) = test_hub().await;
        let app = Router::new().route("/r.json", get(|| async { "not json" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        hub.registry
            .set_url(&format!("http://{}/r.json", listener.local_addr().unwrap()));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let body = list(&hub).await.unwrap_err().to_error_body();
        assert!(body.message.contains("not a registry"), "{}", body.message);
    }

    #[tokio::test]
    async fn the_request_reaches_the_handler_through_the_dispatch() {
        let (hub, _dir) = test_hub().await;
        let (url, _server) = serve_registry().await;
        hub.registry.set_url(&url);
        let value = crate::api::dispatch(
            &hub,
            Caller::Hub,
            consort_proto::client_api::ClientRequest::AcpRegistryList(AcpRegistryList {}),
        )
        .await
        .unwrap();
        assert_eq!(value["agents"].as_array().unwrap().len(), 4);
    }
}
