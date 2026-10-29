//! HTTP calls to the hub: pairing and blob uploads.

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

fn client() -> Result<reqwest::Client, WorkerError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| WorkerError::Network(e.to_string()))
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
    let response = client()?
        .post(&url)
        .json(request)
        .send()
        .await
        .map_err(|e| WorkerError::Network(e.to_string()))?;
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
    let response = client()?
        .put(&url)
        .header(HEADER_WORKER_ID, worker_id.to_string())
        .header(HEADER_TIMESTAMP, timestamp.as_millis().to_string())
        .header(HEADER_SIGNATURE, signature.to_hex())
        .body(bytes)
        .send()
        .await
        .map_err(|e| WorkerError::Network(e.to_string()))?;
    if !response.status().is_success() {
        return Err(hub_error(response).await);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
