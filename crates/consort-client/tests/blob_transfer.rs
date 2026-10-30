//! File transfers against an in-process fake hub that behaves like the real
//! blob endpoints: bearer auth, a hash check, and a 256 MiB limit.

// Tests may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::put,
};
use consort_client::blob::{BlobError, BlobTransfer, MAX_BLOB_BYTES, Progress};
use consort_proto::model::Sha256Hash;
use serde_json::json;

const TOKEN: &str = "good-token";

#[derive(Default)]
struct Hub {
    blobs: HashMap<String, Vec<u8>>,
    /// Every request, as `METHOD sha`.
    log: Vec<String>,
    /// The Content-Length header of each upload.
    put_lengths: Vec<Option<u64>>,
    /// Answer a download with a changed first byte.
    tamper: bool,
}

type Shared = Arc<Mutex<Hub>>;

struct FakeHub {
    hub: Shared,
    url: String,
}

impl FakeHub {
    async fn start() -> FakeHub {
        let hub: Shared = Arc::default();
        let app = Router::new()
            .route("/v1/blobs/{sha256}", put(put_blob).get(get_blob))
            .with_state(hub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        FakeHub {
            hub,
            url: format!("http://{addr}"),
        }
    }

    fn log(&self) -> Vec<String> {
        self.hub.lock().unwrap().log.clone()
    }

    fn stored(&self, sha: &Sha256Hash) -> Option<Vec<u8>> {
        self.hub.lock().unwrap().blobs.get(sha.as_str()).cloned()
    }

    fn insert(&self, bytes: &[u8]) -> Sha256Hash {
        let sha = BlobTransfer::hash_bytes(bytes);
        self.hub
            .lock()
            .unwrap()
            .blobs
            .insert(sha.as_str().to_owned(), bytes.to_vec());
        sha
    }
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {TOKEN}"))
}

fn unauthorized() -> Response {
    error(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Blob requests need a client token.",
    )
}

async fn put_blob(
    State(hub): State<Shared>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    {
        let mut hub = hub.lock().unwrap();
        hub.log.push(format!("PUT {name}"));
        hub.put_lengths.push(length);
    }
    if !authorized(&headers) {
        return unauthorized();
    }
    if length.is_some_and(|len| len > MAX_BLOB_BYTES) {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid",
            "The blob is too large. The limit is 256 MiB.",
        );
    }
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BLOB_BYTES as usize).await else {
        return error(StatusCode::BAD_REQUEST, "invalid", "The upload stopped.");
    };
    if BlobTransfer::hash_bytes(&bytes).as_str() != name {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid",
            "The content does not match the hash in the URL. Upload the file again.",
        );
    }
    hub.lock()
        .unwrap()
        .blobs
        .insert(name.clone(), bytes.to_vec());
    Json(json!({"sha256": name, "size": bytes.len()})).into_response()
}

/// Answers GET, and HEAD too: axum drops the body of a HEAD answer.
async fn get_blob(
    State(hub): State<Shared>,
    AxumPath(name): AxumPath<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let mut hub = hub.lock().unwrap();
    hub.log.push(format!("{method} {name}"));
    if !authorized(&headers) {
        return unauthorized();
    }
    match hub.blobs.get(&name) {
        None => error(
            StatusCode::NOT_FOUND,
            "not_found",
            "The hub has no blob with this hash.",
        ),
        Some(bytes) => {
            let mut bytes = bytes.clone();
            if hub.tamper
                && let Some(first) = bytes.first_mut()
            {
                *first ^= 0xff;
            }
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/octet-stream")],
                bytes,
            )
                .into_response()
        }
    }
}

fn recorder() -> (
    Arc<Mutex<Vec<Progress>>>,
    impl Fn(Progress) + Send + Sync + 'static,
) {
    let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
    let sink = seen.clone();
    (seen, move |p| sink.lock().unwrap().push(p))
}

fn assert_ends_at_total(seen: &Mutex<Vec<Progress>>, total: u64) {
    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty());
    assert!(
        seen.windows(2).all(|w| w[0].done <= w[1].done),
        "progress went back: {seen:?}"
    );
    assert!(seen.iter().all(|p| p.done <= p.total && p.total == total));
    assert_eq!(seen.last().copied(), Some(Progress { done: total, total }));
}

fn sample(len: usize) -> Vec<u8> {
    (0..len).map(|n| (n % 251) as u8).collect()
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn count_entries(dir: &Path) -> usize {
    std::fs::read_dir(dir).unwrap().count()
}

#[tokio::test]
async fn uploads_a_new_file_after_a_head_check() {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    let data = sample(900_000);
    let path = write_file(dir.path(), "big.bin", &data);
    let transfer = BlobTransfer::new(&hub.url, None).unwrap();
    let (sha, size) = BlobTransfer::hash_file(&path, |_| {}).await.unwrap();

    let (seen, progress) = recorder();
    let uploaded = transfer
        .upload_file_if_missing(TOKEN, &path, &sha, size, progress)
        .await
        .unwrap();

    assert!(uploaded);
    assert_eq!(hub.log(), vec![format!("HEAD {sha}"), format!("PUT {sha}")]);
    assert_eq!(hub.stored(&sha), Some(data));
    assert_eq!(hub.hub.lock().unwrap().put_lengths, vec![Some(size)]);
    assert!(seen.lock().unwrap().len() > 2, "expected several pieces");
    assert_ends_at_total(&seen, size);
}

#[tokio::test]
async fn skips_the_upload_when_the_hub_has_the_blob() {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    let data = sample(5_000);
    let path = write_file(dir.path(), "a.bin", &data);
    let sha = hub.insert(&data);
    let transfer = BlobTransfer::new(&hub.url, None).unwrap();

    let (seen, progress) = recorder();
    let uploaded = transfer
        .upload_file_if_missing(TOKEN, &path, &sha, data.len() as u64, progress)
        .await
        .unwrap();

    assert!(!uploaded);
    assert_eq!(hub.log(), vec![format!("HEAD {sha}")]);
    assert_ends_at_total(&seen, data.len() as u64);
}

#[tokio::test]
async fn a_wrong_token_gives_unauthorized_and_stores_nothing() {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    let data = sample(10_000);
    let path = write_file(dir.path(), "a.bin", &data);
    let sha = BlobTransfer::hash_bytes(&data);
    let transfer = BlobTransfer::new(&hub.url, None).unwrap();

    let put = transfer
        .upload_file(
            "bad-token",
            &path,
            &sha,
            data.len() as u64,
            |_: Progress| {},
        )
        .await;
    assert_eq!(put, Err(BlobError::Unauthorized));
    let head = transfer.exists("bad-token", &sha).await;
    assert_eq!(head, Err(BlobError::Unauthorized));
    assert!(hub.hub.lock().unwrap().blobs.is_empty());
}

#[tokio::test]
async fn a_file_over_the_limit_is_refused_before_any_request() {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.bin");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(MAX_BLOB_BYTES + 1)
        .unwrap();
    let transfer = BlobTransfer::new(&hub.url, None).unwrap();
    let too_large = BlobError::TooLarge {
        size: MAX_BLOB_BYTES + 1,
    };

    let hashed = BlobTransfer::hash_file(&path, |_| {}).await;
    assert_eq!(hashed, Err(too_large.clone()));

    // A caller that skips hash_file is stopped as well.
    let sha = BlobTransfer::hash_bytes(b"unused");
    let uploaded = transfer
        .upload_file_if_missing(TOKEN, &path, &sha, MAX_BLOB_BYTES + 1, |_: Progress| {})
        .await;
    assert_eq!(uploaded, Err(too_large));
    assert!(hub.log().is_empty());
}

#[tokio::test]
async fn a_refused_upload_carries_the_hub_message() {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    let data = sample(2_000);
    let path = write_file(dir.path(), "a.bin", &data);
    let wrong = BlobTransfer::hash_bytes(b"other bytes");
    let transfer = BlobTransfer::new(&hub.url, None).unwrap();

    let err = transfer
        .upload_file(TOKEN, &path, &wrong, data.len() as u64, |_: Progress| {})
        .await
        .unwrap_err();

    assert_eq!(
        err,
        BlobError::Refused {
            message: "The content does not match the hash in the URL. Upload the file again."
                .into()
        }
    );
    assert!(hub.hub.lock().unwrap().blobs.is_empty());
}

#[tokio::test]
async fn a_file_that_changed_size_gives_mismatch() {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(dir.path(), "a.bin", &sample(1_000));
    let sha = BlobTransfer::hash_bytes(&sample(1_000));
    let transfer = BlobTransfer::new(&hub.url, None).unwrap();

    let err = transfer
        .upload_file(TOKEN, &path, &sha, 999, |_: Progress| {})
        .await
        .unwrap_err();

    assert_eq!(err, BlobError::Mismatch);
    assert!(hub.log().is_empty());
}

#[tokio::test]
async fn download_fills_the_cache_and_a_second_one_makes_no_request() {
    let hub = FakeHub::start().await;
    let data = sample(700_000);
    let sha = hub.insert(&data);
    let cache = tempfile::tempdir().unwrap();
    let transfer = BlobTransfer::new(&hub.url, Some(cache.path().join("blobs"))).unwrap();
    assert_eq!(transfer.cached(&sha), None);

    let (seen, progress) = recorder();
    let path = transfer
        .download(TOKEN, &sha, Some(data.len() as u64), progress)
        .await
        .unwrap();

    assert_eq!(path, cache.path().join("blobs").join(sha.as_str()));
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert_eq!(count_entries(&cache.path().join("blobs")), 1);
    assert_eq!(transfer.cached(&sha), Some(path.clone()));
    assert_eq!(hub.log(), vec![format!("GET {sha}")]);
    assert_ends_at_total(&seen, data.len() as u64);

    let again = transfer
        .download(TOKEN, &sha, None, |_: Progress| {})
        .await
        .unwrap();
    assert_eq!(again, path);
    assert_eq!(hub.log().len(), 1, "a cached file needs no request");
}

#[tokio::test]
async fn wrong_bytes_give_mismatch_and_leave_no_file() {
    let hub = FakeHub::start().await;
    let data = sample(50_000);
    let sha = hub.insert(&data);
    hub.hub.lock().unwrap().tamper = true;
    let cache = tempfile::tempdir().unwrap();
    let transfer = BlobTransfer::new(&hub.url, Some(cache.path().to_owned())).unwrap();

    let err = transfer
        .download(TOKEN, &sha, Some(data.len() as u64), |_: Progress| {})
        .await
        .unwrap_err();

    assert_eq!(err, BlobError::Mismatch);
    assert_eq!(count_entries(cache.path()), 0);
    assert_eq!(transfer.cached(&sha), None);
}

#[tokio::test]
async fn a_hash_the_hub_lacks_gives_not_found() {
    let hub = FakeHub::start().await;
    let cache = tempfile::tempdir().unwrap();
    let transfer = BlobTransfer::new(&hub.url, Some(cache.path().to_owned())).unwrap();
    let sha = BlobTransfer::hash_bytes(b"nobody has this");

    let err = transfer
        .download(TOKEN, &sha, None, |_: Progress| {})
        .await
        .unwrap_err();

    assert_eq!(err, BlobError::NotFound);
    assert_eq!(count_entries(cache.path()), 0);
}

#[tokio::test]
async fn a_download_with_a_wrong_token_gives_unauthorized() {
    let hub = FakeHub::start().await;
    let sha = hub.insert(b"secret bytes");
    let cache = tempfile::tempdir().unwrap();
    let transfer = BlobTransfer::new(&hub.url, Some(cache.path().to_owned())).unwrap();

    let err = transfer
        .download("bad-token", &sha, None, |_: Progress| {})
        .await
        .unwrap_err();

    assert_eq!(err, BlobError::Unauthorized);
    assert_eq!(count_entries(cache.path()), 0);
}

#[tokio::test]
async fn uploads_bytes_in_memory() {
    let hub = FakeHub::start().await;
    let data = sample(600_000);
    let sha = BlobTransfer::hash_bytes(&data);
    let transfer = BlobTransfer::new(&hub.url, None).unwrap();

    let (seen, progress) = recorder();
    transfer
        .upload_bytes(TOKEN, data.clone(), &sha, progress)
        .await
        .unwrap();

    assert_eq!(hub.stored(&sha), Some(data.clone()));
    assert_eq!(hub.log(), vec![format!("PUT {sha}")]);
    assert_ends_at_total(&seen, data.len() as u64);

    // The same bytes again: the hub has them, so nothing is sent.
    let (seen, progress) = recorder();
    let uploaded = transfer
        .upload_bytes_if_missing(TOKEN, data.clone(), &sha, progress)
        .await
        .unwrap();
    assert!(!uploaded);
    assert_eq!(hub.log().len(), 2);
    assert_ends_at_total(&seen, data.len() as u64);
}

#[tokio::test]
async fn bytes_that_do_not_match_their_hash_are_not_sent() {
    let hub = FakeHub::start().await;
    let sha = BlobTransfer::hash_bytes(b"one");
    let transfer = BlobTransfer::new(&hub.url, None).unwrap();

    let err = transfer
        .upload_bytes_if_missing(TOKEN, b"two".to_vec(), &sha, |_: Progress| {})
        .await
        .unwrap_err();

    assert_eq!(err, BlobError::Mismatch);
    assert!(hub.log().is_empty());
}

#[tokio::test]
async fn an_unreachable_hub_gives_unreachable() {
    // Bind a port, then close it, so nothing listens there.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let cache = tempfile::tempdir().unwrap();
    let transfer = BlobTransfer::new(&url, Some(cache.path().to_owned())).unwrap();
    let sha = BlobTransfer::hash_bytes(b"x");

    assert_eq!(
        transfer.exists(TOKEN, &sha).await,
        Err(BlobError::Unreachable)
    );
    assert_eq!(
        transfer
            .upload_bytes(TOKEN, b"x".to_vec(), &sha, |_: Progress| {})
            .await,
        Err(BlobError::Unreachable)
    );
    assert_eq!(
        transfer.download(TOKEN, &sha, None, |_: Progress| {}).await,
        Err(BlobError::Unreachable)
    );
    assert_eq!(count_entries(cache.path()), 0);
}
