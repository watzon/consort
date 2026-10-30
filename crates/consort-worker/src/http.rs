//! HTTP calls to the hub: pairing, and blob uploads and downloads.

use consort_proto::auth::{
    HEADER_SIGNATURE, HEADER_TIMESTAMP, HEADER_WORKER_ID, PAIR_WORKER_PATH, PairWorkerRequest,
    PairWorkerResponse, blob_path, http_signing_payload, normalize_origin,
};
use consort_proto::error::{ErrorCode, HttpErrorBody};
use consort_proto::ids::WorkerId;
use consort_proto::model::Sha256Hash;
use consort_proto::time::Timestamp;

use crate::WorkerError;
use crate::identity::Identity;

/// The hub URL without a trailing slash.
pub fn base_url(hub_url: &str) -> String {
    hub_url.trim().trim_end_matches('/').to_owned()
}

/// The WebSocket URL of the worker link.
pub fn worker_ws_url(hub_url: &str) -> Result<String, WorkerError> {
    let base = base_url(hub_url);
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if base.starts_with("ws://") || base.starts_with("wss://") {
        base
    } else {
        return Err(WorkerError::Config(format!(
            "`{hub_url}` is not an http or https URL"
        )));
    };
    Ok(format!("{ws}{}", consort_proto::auth::WORKER_WS_PATH))
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

fn client(limits: HttpLimits) -> Result<reqwest::Client, WorkerError> {
    reqwest::Client::builder()
        .timeout(limits.total)
        .build()
        .map_err(|e| WorkerError::Network(e.to_string()))
}

/// A failed call as the worker reports it. A time-out says how long the
/// worker waited and what to check.
fn network_error(err: &reqwest::Error, waited: std::time::Duration) -> WorkerError {
    if err.is_timeout() {
        let secs = waited.as_secs();
        let limit = if secs == 0 {
            format!("{} milliseconds", waited.as_millis().max(1))
        } else {
            format!("{secs} seconds")
        };
        return WorkerError::Network(format!(
            "the hub did not answer within {limit}. Check that the hub is running and that this machine reaches it, then try again"
        ));
    }
    WorkerError::Network(err.to_string())
}

async fn hub_error(response: reqwest::Response) -> WorkerError {
    let status = response.status();
    match response.json::<HttpErrorBody>().await {
        Ok(body) => WorkerError::Hub {
            code: body.error.code,
            message: body.error.message,
        },
        Err(_) => WorkerError::Hub {
            code: ErrorCode::Internal,
            message: format!("HTTP {status}"),
        },
    }
}

/// Sends `POST /v1/pair/worker`.
pub async fn pair_worker(
    hub_url: &str,
    request: &PairWorkerRequest,
) -> Result<WorkerId, WorkerError> {
    let url = format!("{}{PAIR_WORKER_PATH}", http_base(hub_url));
    let limits = HttpLimits::DEFAULT;
    let response = client(limits)?
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
        .map_err(|e| WorkerError::Network(format!("the pairing response is not valid: {e}")))?;
    Ok(body.worker_id)
}

/// Uploads a blob with `PUT /v1/blobs/{sha256}` and the worker signature
/// headers.
pub async fn upload_blob(
    hub_url: &str,
    worker_id: WorkerId,
    identity: &Identity,
    sha256: &Sha256Hash,
    bytes: Vec<u8>,
) -> Result<(), WorkerError> {
    let origin = normalize_origin(hub_url).map_err(|e| WorkerError::Config(e.to_string()))?;
    let path = blob_path(sha256);
    let timestamp = Timestamp::now();
    let signature = identity.sign(&http_signing_payload(
        "PUT", &origin, &path, timestamp, worker_id,
    ));
    let url = format!("{}{path}", http_base(hub_url));
    let limits = HttpLimits::DEFAULT;
    let response = client(limits)?
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
pub async fn download_blob(
    hub_url: &str,
    worker_id: WorkerId,
    identity: &Identity,
    sha256: &Sha256Hash,
    max_bytes: u64,
) -> Result<Vec<u8>, WorkerError> {
    download_blob_within(
        hub_url,
        worker_id,
        identity,
        sha256,
        max_bytes,
        HttpLimits::DEFAULT,
    )
    .await
}

/// [`download_blob`] with other time limits, for tests.
pub async fn download_blob_within(
    hub_url: &str,
    worker_id: WorkerId,
    identity: &Identity,
    sha256: &Sha256Hash,
    max_bytes: u64,
    limits: HttpLimits,
) -> Result<Vec<u8>, WorkerError> {
    let origin = normalize_origin(hub_url).map_err(|e| WorkerError::Config(e.to_string()))?;
    let path = blob_path(sha256);
    let timestamp = Timestamp::now();
    let signature = identity.sign(&http_signing_payload(
        "GET", &origin, &path, timestamp, worker_id,
    ));
    let url = format!("{}{path}", http_base(hub_url));
    let response = client(limits)?
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
        return Err(WorkerError::Network(format!(
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
            return Err(WorkerError::Network(format!(
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
pub async fn download_blob_to_file(
    hub_url: &str,
    worker_id: WorkerId,
    identity: &Identity,
    sha256: &Sha256Hash,
    max_bytes: u64,
    dest: &std::path::Path,
) -> Result<u64, WorkerError> {
    download_blob_to_file_within(
        hub_url,
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
pub async fn download_blob_to_file_within(
    hub_url: &str,
    worker_id: WorkerId,
    identity: &Identity,
    sha256: &Sha256Hash,
    max_bytes: u64,
    dest: &std::path::Path,
    limits: HttpLimits,
) -> Result<u64, WorkerError> {
    use tokio::io::AsyncWriteExt as _;
    let origin = normalize_origin(hub_url).map_err(|e| WorkerError::Config(e.to_string()))?;
    let path = blob_path(sha256);
    let timestamp = Timestamp::now();
    let signature = identity.sign(&http_signing_payload(
        "GET", &origin, &path, timestamp, worker_id,
    ));
    let url = format!("{}{path}", http_base(hub_url));
    let client = reqwest::Client::builder()
        .connect_timeout(limits.connect)
        .read_timeout(limits.read)
        .build()
        .map_err(|e| WorkerError::Network(e.to_string()))?;
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
        WorkerError::Network(format!(
            "the hub sent a blob that is larger than {max_bytes} bytes"
        ))
    };
    if response.content_length().is_some_and(|len| len > max_bytes) {
        return Err(too_large());
    }
    let io = |source| WorkerError::Io {
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
            worker_ws_url("http://127.0.0.1:7600/").unwrap(),
            "ws://127.0.0.1:7600/v1/worker"
        );
        assert_eq!(
            worker_ws_url("https://hub.example.com").unwrap(),
            "wss://hub.example.com/v1/worker"
        );
        assert!(worker_ws_url("ftp://x").is_err());
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
