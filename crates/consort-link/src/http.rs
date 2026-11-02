//! HTTP calls to the hub: pairing, and blob uploads and downloads.

use consort_proto::auth::{
    HEADER_SIGNATURE, HEADER_TIMESTAMP, HEADER_WORKER_ID, PairWorkerRequest, PairWorkerResponse,
    blob_path, http_signing_payload, normalize_origin,
};
use consort_proto::error::{ErrorCode, HttpErrorBody};
use consort_proto::ids::WorkerId;
use consort_proto::model::Sha256Hash;
use consort_proto::time::Timestamp;

use consort_tls::certificate_problem;

use crate::LinkError;
use crate::identity::Identity;

/// How to reach a hub: its URL, and the pin of its certificate for a hub on
/// This network. A hub URL alone is a hub with no pin.
#[derive(Debug, Clone, Copy)]
pub struct HubRef<'a> {
    pub url: &'a str,
    pub tls_sha256: Option<&'a Sha256Hash>,
}

impl<'a> From<&'a str> for HubRef<'a> {
    fn from(url: &'a str) -> Self {
        Self {
            url,
            tls_sha256: None,
        }
    }
}

impl<'a> From<&'a String> for HubRef<'a> {
    fn from(url: &'a String) -> Self {
        url.as_str().into()
    }
}

/// Starts a client with the rules of this hub: with a pin, it accepts only the
/// pinned certificate.
fn builder_for(hub: HubRef<'_>) -> Result<reqwest::ClientBuilder, LinkError> {
    let builder = reqwest::Client::builder();
    let Some(pin) = hub.tls_sha256 else {
        return Ok(builder);
    };
    let config =
        consort_tls::pinned_client_config(pin).map_err(|e| LinkError::Network(e.to_string()))?;
    Ok(builder.tls_backend_preconfigured(config))
}

/// The sentence for a certificate that this link does not accept, when the
/// error has one.
pub(crate) fn certificate_message(
    err: &(dyn std::error::Error + 'static),
    hub_url: &str,
) -> Option<String> {
    Some(match certificate_problem(err)? {
        consort_tls::CertificateProblem::NotThePin => format!(
            "the certificate of {hub_url} is not the one that this machine paired with. Someone may be in the way, or the hub has a new certificate. Check the hub, and pair this machine again if the hub is new"
        ),
        consort_tls::CertificateProblem::Untrusted => format!(
            "{hub_url} has a certificate that no authority signed. Use the install command from the app, because it holds the key of the hub"
        ),
    })
}

/// The hub URL without a trailing slash.
pub fn base_url(hub_url: &str) -> String {
    hub_url.trim().trim_end_matches('/').to_owned()
}

/// The WebSocket URL of a link: the hub URL and the path of the route.
pub fn ws_url(hub_url: &str, path: &str) -> Result<String, LinkError> {
    let base = base_url(hub_url);
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if base.starts_with("ws://") || base.starts_with("wss://") {
        base
    } else {
        return Err(LinkError::Config(format!(
            "`{hub_url}` is not an http or https URL"
        )));
    };
    Ok(format!("{ws}{path}"))
}

/// A warning when the hub URL sends traffic without TLS to a host that is
/// not this machine. The worker still connects, because a test on the local
/// network can use `http`. `None` for `https`, `wss` and loopback hosts.
pub fn cleartext_warning(hub_url: &str) -> Option<String> {
    let parsed = url::Url::parse(base_url(hub_url).as_str()).ok()?;
    if !matches!(parsed.scheme(), "http" | "ws") {
        return None;
    }
    let loopback = match parsed.host()? {
        url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    };
    if loopback {
        return None;
    }
    Some(format!(
        "The hub URL {hub_url} has no TLS. Anyone on the network path can read the worker link and the pairing code, and can change what the hub sends. Use https:// for a hub that is not on this machine."
    ))
}

/// The HTTP base URL, with `ws` read as `http`.
fn http_base(hub_url: &str) -> String {
    let base = base_url(hub_url);
    if let Some(rest) = base.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = base.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        base
    }
}

/// The time limits of the calls to the hub.
#[derive(Debug, Clone, Copy)]
pub struct HttpLimits {
    /// The whole call, for a small request.
    pub total: std::time::Duration,
    /// Connecting, for a call that may take long (a large blob).
    pub connect: std::time::Duration,
    /// Waiting for the next bytes, for a call that may take long.
    pub read: std::time::Duration,
}

impl HttpLimits {
    /// What the worker uses.
    pub const DEFAULT: Self = Self {
        total: std::time::Duration::from_secs(60),
        connect: std::time::Duration::from_secs(30),
        read: std::time::Duration::from_secs(60),
    };
}

fn client(hub: HubRef<'_>, limits: HttpLimits) -> Result<reqwest::Client, LinkError> {
    builder_for(hub)?
        .timeout(limits.total)
        .build()
        .map_err(|e| LinkError::Network(e.to_string()))
}

/// A failed call as the worker reports it. A time-out says how long the
/// worker waited and what to check.
fn network_error(err: &reqwest::Error, waited: std::time::Duration) -> LinkError {
    if let Some(message) = err
        .url()
        .and_then(|url| certificate_message(err, url.origin().ascii_serialization().as_str()))
    {
        return LinkError::Network(message);
    }
    if err.is_timeout() {
        let secs = waited.as_secs();
        let limit = if secs == 0 {
            format!("{} milliseconds", waited.as_millis().max(1))
        } else {
            format!("{secs} seconds")
        };
        return LinkError::Network(format!(
            "the hub did not answer within {limit}. Check that the hub is running and that this machine reaches it, then try again"
        ));
    }
    LinkError::Network(err.to_string())
}

async fn hub_error(response: reqwest::Response) -> LinkError {
    let status = response.status();
    match response.json::<HttpErrorBody>().await {
        Ok(body) => body.error.into(),
        Err(_) => LinkError::Hub {
            code: ErrorCode::Internal,
            message: format!("HTTP {status}"),
        },
    }
}

/// Pairs a machine at the pairing path of its kind: `/v1/pair/worker` or
/// `/v1/pair/session-host`.
pub async fn pair_machine<'a>(
    hub: impl Into<HubRef<'a>>,
    path: &str,
    request: &PairWorkerRequest,
) -> Result<WorkerId, LinkError> {
    let hub = hub.into();
    let url = format!("{}{path}", http_base(hub.url));
    let limits = HttpLimits::DEFAULT;
    let response = client(hub, limits)?
        .post(&url)
        .json(request)
        .send()
        .await
        .map_err(|e| network_error(&e, limits.total))?;
    if !response.status().is_success() {
        return Err(hub_error(response).await);
    }
    let body: PairWorkerResponse = response
        .json()
        .await
        .map_err(|e| LinkError::Network(format!("the pairing response is not valid: {e}")))?;
    Ok(body.worker_id)
}

/// Uploads a blob with `PUT /v1/blobs/{sha256}` and the worker signature
/// headers.
pub async fn upload_blob<'a>(
    hub: impl Into<HubRef<'a>>,
    worker_id: WorkerId,
    identity: &Identity,
    sha256: &Sha256Hash,
    bytes: Vec<u8>,
) -> Result<(), LinkError> {
    let hub = hub.into();
    let hub_url = hub.url;
    let origin = normalize_origin(hub_url).map_err(|e| LinkError::Config(e.to_string()))?;
    let path = blob_path(sha256);
    let timestamp = Timestamp::now();
    let signature = identity.sign(&http_signing_payload(
        "PUT", &origin, &path, timestamp, worker_id,
    ));
    let url = format!("{}{path}", http_base(hub_url));
    let limits = HttpLimits::DEFAULT;
    let response = client(hub, limits)?
        .put(&url)
        .header(HEADER_WORKER_ID, worker_id.to_string())
        .header(HEADER_TIMESTAMP, timestamp.as_millis().to_string())
        .header(HEADER_SIGNATURE, signature.to_hex())
        .body(bytes)
        .send()
        .await
        .map_err(|e| network_error(&e, limits.total))?;
    if !response.status().is_success() {
        return Err(hub_error(response).await);
    }
    Ok(())
}

/// Downloads a blob with `GET /v1/blobs/{sha256}` and the worker signature
/// headers. The caller checks the bytes against the hash, and `max_bytes`
/// stops a hub that sends more than the caller expects.
pub async fn download_blob<'a>(
    hub: impl Into<HubRef<'a>>,
    worker_id: WorkerId,
    identity: &Identity,
    sha256: &Sha256Hash,
    max_bytes: u64,
) -> Result<Vec<u8>, LinkError> {
    download_blob_within(
        hub,
        worker_id,
        identity,
        sha256,
        max_bytes,
        HttpLimits::DEFAULT,
    )
    .await
}

/// [`download_blob`] with other time limits, for tests.
pub async fn download_blob_within<'a>(
    hub: impl Into<HubRef<'a>>,
    worker_id: WorkerId,
    identity: &Identity,
    sha256: &Sha256Hash,
    max_bytes: u64,
    limits: HttpLimits,
) -> Result<Vec<u8>, LinkError> {
    let hub = hub.into();
    let hub_url = hub.url;
    let origin = normalize_origin(hub_url).map_err(|e| LinkError::Config(e.to_string()))?;
    let path = blob_path(sha256);
    let timestamp = Timestamp::now();
    let signature = identity.sign(&http_signing_payload(
        "GET", &origin, &path, timestamp, worker_id,
    ));
    let url = format!("{}{path}", http_base(hub_url));
    let response = client(hub, limits)?
        .get(&url)
        .header(HEADER_WORKER_ID, worker_id.to_string())
        .header(HEADER_TIMESTAMP, timestamp.as_millis().to_string())
        .header(HEADER_SIGNATURE, signature.to_hex())
        .send()
        .await
        .map_err(|e| network_error(&e, limits.total))?;
    if !response.status().is_success() {
        return Err(hub_error(response).await);
    }
    if response.content_length().is_some_and(|len| len > max_bytes) {
        return Err(LinkError::Network(format!(
            "the hub sent a blob that is larger than {max_bytes} bytes"
        )));
    }
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| network_error(&e, limits.total))?
    {
        if (body.len() as u64).saturating_add(chunk.len() as u64) > max_bytes {
            return Err(LinkError::Network(format!(
                "the hub sent a blob that is larger than {max_bytes} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Downloads a blob with `GET /v1/blobs/{sha256}` and the worker signature
/// headers into the new file `dest`, so a large blob does not stay in
/// memory. The download stops when the hub sends more than `max_bytes`. The
/// caller checks the hash. A large blob may take long, so this client has no
/// limit for the whole request, only for the connect and for each read.
pub async fn download_blob_to_file<'a>(
    hub: impl Into<HubRef<'a>>,
    worker_id: WorkerId,
    identity: &Identity,
    sha256: &Sha256Hash,
    max_bytes: u64,
    dest: &std::path::Path,
) -> Result<u64, LinkError> {
    download_blob_to_file_within(
        hub,
        worker_id,
        identity,
        sha256,
        max_bytes,
        dest,
        HttpLimits::DEFAULT,
    )
    .await
}

/// [`download_blob_to_file`] with other time limits, for tests.
pub async fn download_blob_to_file_within<'a>(
    hub: impl Into<HubRef<'a>>,
    worker_id: WorkerId,
    identity: &Identity,
    sha256: &Sha256Hash,
    max_bytes: u64,
    dest: &std::path::Path,
    limits: HttpLimits,
) -> Result<u64, LinkError> {
    use tokio::io::AsyncWriteExt as _;
    let hub = hub.into();
    let hub_url = hub.url;
    let origin = normalize_origin(hub_url).map_err(|e| LinkError::Config(e.to_string()))?;
    let path = blob_path(sha256);
    let timestamp = Timestamp::now();
    let signature = identity.sign(&http_signing_payload(
        "GET", &origin, &path, timestamp, worker_id,
    ));
    let url = format!("{}{path}", http_base(hub_url));
    let client = builder_for(hub)?
        .connect_timeout(limits.connect)
        .read_timeout(limits.read)
        .build()
        .map_err(|e| LinkError::Network(e.to_string()))?;
    let mut response = client
        .get(&url)
        .header(HEADER_WORKER_ID, worker_id.to_string())
        .header(HEADER_TIMESTAMP, timestamp.as_millis().to_string())
        .header(HEADER_SIGNATURE, signature.to_hex())
        .send()
        .await
        .map_err(|e| network_error(&e, limits.read))?;
    if !response.status().is_success() {
        return Err(hub_error(response).await);
    }
    let too_large = || {
        LinkError::Network(format!(
            "the hub sent a blob that is larger than {max_bytes} bytes"
        ))
    };
    if response.content_length().is_some_and(|len| len > max_bytes) {
        return Err(too_large());
    }
    let io = |source| LinkError::Io {
        context: format!("could not write {}", dest.display()),
        source,
    };
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .await
        .map_err(io)?;
    let mut written: u64 = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| network_error(&e, limits.read))?
    {
        written = written.saturating_add(chunk.len() as u64);
        if written > max_bytes {
            return Err(too_large());
        }
        file.write_all(&chunk).await.map_err(io)?;
    }
    file.flush().await.map_err(io)?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A server that accepts a connection and never answers.
    async fn silent_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        (url, task)
    }

    fn short(limit: Duration) -> HttpLimits {
        HttpLimits {
            total: limit,
            connect: limit,
            read: limit,
        }
    }

    #[tokio::test]
    async fn a_hub_that_never_answers_ends_a_blob_download_at_its_limit() {
        let (url, task) = silent_server().await;
        let identity = Identity::generate();
        let hash = Sha256Hash::from_bytes([7; 32]);
        let started = Instant::now();
        let err = download_blob_within(
            &url,
            WorkerId::new(),
            &identity,
            &hash,
            1024,
            short(Duration::from_millis(400)),
        )
        .await
        .unwrap_err();
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(350), "{took:?}");
        assert!(took < Duration::from_secs(10), "{took:?}");
        let message = err.to_string();
        assert!(
            message.contains("did not answer within 400 milliseconds"),
            "{message}"
        );
        assert!(message.contains("hub is running"), "{message}");

        // A download to a file has its limit too, and leaves no file behind
        // that could pass for a finished blob.
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("blob");
        let started = Instant::now();
        let err = download_blob_to_file_within(
            &url,
            WorkerId::new(),
            &identity,
            &hash,
            1024,
            &dest,
            short(Duration::from_millis(400)),
        )
        .await
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(err.to_string().contains("did not answer within"), "{err}");
        task.abort();
    }

    #[test]
    fn urls() {
        assert_eq!(
            ws_url(
                "http://127.0.0.1:7600/",
                consort_proto::auth::WORKER_WS_PATH
            )
            .unwrap(),
            "ws://127.0.0.1:7600/v1/worker"
        );
        assert_eq!(
            ws_url(
                "https://hub.example.com",
                consort_proto::auth::WORKER_WS_PATH
            )
            .unwrap(),
            "wss://hub.example.com/v1/worker"
        );
        assert!(ws_url("ftp://x", consort_proto::auth::WORKER_WS_PATH).is_err());
        assert_eq!(http_base("wss://h"), "https://h");
    }

    #[test]
    fn cleartext_to_another_host_gets_a_warning() {
        for url in [
            "http://192.168.188.138:7600",
            "ws://hub.example.com",
            "http://hub.example.com/",
        ] {
            assert!(cleartext_warning(url).is_some(), "{url}");
        }
        for url in [
            "https://hub.example.com",
            "wss://hub.example.com",
            "http://127.0.0.1:7600",
            "http://localhost:7600",
            "http://[::1]:7600",
        ] {
            assert!(cleartext_warning(url).is_none(), "{url}");
        }
    }
}
