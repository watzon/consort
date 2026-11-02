//! Move files between this machine and the hub.
//!
//! The hub keeps every file as a blob that has the SHA-256 hash of its bytes
//! as its name. This module hashes a file, uploads it, and downloads it
//! again. It talks to `/v1/blobs/{sha256}` with a bearer token. The caller
//! gets the token from `auth.http_token` and asks for a new one when a call
//! returns [`BlobError::Unauthorized`].
//!
//! Rules that this module keeps:
//!
//! - A file over [`MAX_BLOB_BYTES`] is refused before the first request.
//! - An upload streams from the file. A large file is never in memory.
//! - A download goes to a temporary file. The file gets its final name only
//!   after its hash is right. A wrong hash leaves no file behind.
//! - A download is kept in the cache folder, one file for each hash. A second
//!   download of the same hash makes no request.

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use consort_proto::auth::blob_path;
use consort_proto::model::Sha256Hash;
use futures_util::stream;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::{Body, Method, Response, StatusCode, header};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::{Host, Url};

use crate::ClientError;

/// The largest blob that the hub accepts: 256 MiB.
pub const MAX_BLOB_BYTES: u64 = 256 * 1024 * 1024;

/// The size of one read from a file, and of one piece of an upload.
const CHUNK_BYTES: usize = 256 * 1024;
/// The most bytes of an error answer that this module reads.
const ERROR_BODY_LIMIT: usize = 16 * 1024;
const MIB: u64 = 1024 * 1024;

/// Progress of one transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Bytes moved so far.
    pub done: u64,
    /// Bytes in the whole transfer. `0` when the size is not known.
    pub total: u64,
}

/// Why a file transfer failed. Each message says what happened and what to
/// do next, so a client can show it as it is.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BlobError {
    #[error("This file is {} MB. The limit is 256 MB. Choose a smaller file", size.div_ceil(MIB))]
    TooLarge { size: u64 },
    #[error("The hub did not accept the sign-in for files. Try again")]
    Unauthorized,
    #[error("The hub does not have this file any more. Ask for it again")]
    NotFound,
    #[error("Cannot reach the hub to move the file. Check the connection, then try again")]
    Unreachable,
    /// The hub refused the file. This is the hub's own message.
    #[error("{message}")]
    Refused { message: String },
    #[error("{message}")]
    Hub { status: u16, message: String },
    #[error("The file changed on the way. Try again")]
    Mismatch,
    #[error(
        "Cannot read or write {path}: {message}. Check that the file exists and that you may use it, then try again"
    )]
    Io { path: String, message: String },
    #[error("Consort has no folder to keep files in. Restart Consort, then try again")]
    NoCache,
    #[error("The hub URL is not valid: {message}. Enter a URL such as https://hub.example.com")]
    InvalidUrl { message: String },
    #[error("Consort cannot start file transfers: {message}. Restart Consort, then try again")]
    Setup { message: String },
}

impl From<BlobError> for ClientError {
    fn from(err: BlobError) -> Self {
        let message = err.to_string();
        match err {
            BlobError::Unauthorized => ClientError::Hub {
                code: "unauthorized".into(),
                message,
            },
            BlobError::NotFound => ClientError::Hub {
                code: "not_found".into(),
                message,
            },
            BlobError::Io { path, message } => ClientError::Storage { path, message },
            BlobError::InvalidUrl { message } => ClientError::InvalidUrl(message),
            _ => ClientError::Other(message),
        }
    }
}

fn io_error(path: &Path, err: &std::io::Error) -> BlobError {
    BlobError::Io {
        path: path.display().to_string(),
        message: err.to_string(),
    }
}

/// A callback that gets the progress of a transfer.
type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

/// File transfers to one hub. Cheap to clone.
#[derive(Clone, Debug)]
pub struct BlobTransfer {
    http: reqwest::Client,
    /// The hub origin, with no trailing slash.
    base: String,
    cache_dir: Option<PathBuf>,
}

impl BlobTransfer {
    /// Makes a transfer for a hub. `hub_url` is like `https://hub.example.com`
    /// or `http://127.0.0.1:8080`. The same rules apply as for pairing: plain
    /// `http` is only for this machine and private networks.
    ///
    /// `cache_dir` is the folder for downloaded files. With `None`, uploads
    /// work and downloads return [`BlobError::NoCache`].
    pub fn new(hub_url: &str, cache_dir: Option<PathBuf>) -> Result<BlobTransfer, BlobError> {
        Self::pinned(hub_url, None, cache_dir)
    }

    /// As [`BlobTransfer::new`]. With a pin (a hub on This network), the
    /// transfer accepts that certificate and no other.
    pub fn pinned(
        hub_url: &str,
        tls_sha256: Option<&Sha256Hash>,
        cache_dir: Option<PathBuf>,
    ) -> Result<BlobTransfer, BlobError> {
        let base = crate::hub::normalize_hub_url(hub_url).map_err(|err| match err {
            ClientError::InvalidUrl(message) => BlobError::InvalidUrl { message },
            other => BlobError::InvalidUrl {
                message: other.to_string(),
            },
        })?;
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            // A big file takes long, so there is no limit on the whole
            // transfer. A transfer that sends or gets nothing for a minute
            // stops.
            .read_timeout(Duration::from_secs(60))
            // A redirect could send the token to another address.
            .redirect(reqwest::redirect::Policy::none());
        if let Some(pin) = tls_sha256 {
            let config =
                consort_tls::pinned_client_config(pin).map_err(|err| BlobError::Setup {
                    message: err.to_string(),
                })?;
            builder = builder.tls_backend_preconfigured(config);
        }
        // Plain `http` to a name (`hub.local`) passes the URL rule by its
        // spelling. Check where the name points each time it is looked up.
        let plain_http_name = Url::parse(&base)
            .map(|url| url.scheme() == "http" && matches!(url.host(), Some(Host::Domain(_))))
            .unwrap_or(false);
        if plain_http_name {
            builder = builder.dns_resolver(Arc::new(PrivateOnlyResolver));
        }
        let http = builder.build().map_err(|err| BlobError::Setup {
            message: err.to_string(),
        })?;
        Ok(BlobTransfer {
            http,
            base,
            cache_dir,
        })
    }

    /// Hashes a file with SHA-256 and returns the hash and the size. Refuses
    /// a file over [`MAX_BLOB_BYTES`] before it reads the file. Calls
    /// `progress` after each read.
    pub async fn hash_file(
        path: &Path,
        progress: impl Fn(Progress) + Send + Sync,
    ) -> Result<(Sha256Hash, u64), BlobError> {
        let total = file_size(path).await?;
        let mut file = tokio::fs::File::open(path)
            .await
            .map_err(|err| io_error(path, &err))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; CHUNK_BYTES];
        let mut done = 0_u64;
        loop {
            let read = file
                .read(&mut buffer)
                .await
                .map_err(|err| io_error(path, &err))?;
            if read == 0 {
                break;
            }
            done += read as u64;
            // The file may grow while it is read.
            if done > MAX_BLOB_BYTES {
                return Err(BlobError::TooLarge { size: done });
            }
            hasher.update(&buffer[..read]);
            progress(Progress { done, total });
        }
        if done == 0 {
            progress(Progress { done, total });
        }
        Ok((digest(hasher), done))
    }

    /// The SHA-256 hash of bytes in memory.
    pub fn hash_bytes(bytes: &[u8]) -> Sha256Hash {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        digest(hasher)
    }

    /// Asks the hub if it has a blob: `HEAD /v1/blobs/{sha}`. `true` for 200.
    /// `false` for 404, and for a status that means the hub cannot say (405
    /// and 501), so the caller uploads the file.
    pub async fn exists(&self, token: &str, sha: &Sha256Hash) -> Result<bool, BlobError> {
        let response = self
            .http
            .request(Method::HEAD, self.url(sha))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| BlobError::Unreachable)?;
        match response.status() {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::NOT_IMPLEMENTED => Ok(false),
            _ => Err(status_error(response).await),
        }
    }

    /// Uploads a file: `PUT /v1/blobs/{sha}`. The body streams from the file,
    /// and `progress` runs after each piece. `sha` and `size` come from
    /// [`BlobTransfer::hash_file`]. The hub checks the hash. A file that has
    /// another size now gives [`BlobError::Mismatch`].
    pub async fn upload_file(
        &self,
        token: &str,
        path: &Path,
        sha: &Sha256Hash,
        size: u64,
        progress: impl Fn(Progress) + Send + Sync + 'static,
    ) -> Result<(), BlobError> {
        self.upload_file_with(token, path, sha, size, Arc::new(progress))
            .await
    }

    /// Uploads bytes in memory, for example a pasted image.
    pub async fn upload_bytes(
        &self,
        token: &str,
        bytes: Vec<u8>,
        sha: &Sha256Hash,
        progress: impl Fn(Progress) + Send + Sync + 'static,
    ) -> Result<(), BlobError> {
        self.upload_bytes_with(token, bytes, sha, Arc::new(progress))
            .await
    }

    /// Asks the hub if it has the file, and uploads it only when it does not.
    /// Returns `true` when it uploaded and `false` when the hub had the file.
    /// `progress` reaches the total in both cases.
    pub async fn upload_file_if_missing(
        &self,
        token: &str,
        path: &Path,
        sha: &Sha256Hash,
        size: u64,
        progress: impl Fn(Progress) + Send + Sync + 'static,
    ) -> Result<bool, BlobError> {
        check_size(size)?;
        if self.exists(token, sha).await? {
            progress(Progress {
                done: size,
                total: size,
            });
            return Ok(false);
        }
        self.upload_file_with(token, path, sha, size, Arc::new(progress))
            .await?;
        Ok(true)
    }

    /// The same as [`BlobTransfer::upload_file_if_missing`] for bytes in
    /// memory. It checks the hash before it sends anything.
    pub async fn upload_bytes_if_missing(
        &self,
        token: &str,
        bytes: Vec<u8>,
        sha: &Sha256Hash,
        progress: impl Fn(Progress) + Send + Sync + 'static,
    ) -> Result<bool, BlobError> {
        let size = bytes.len() as u64;
        check_size(size)?;
        if BlobTransfer::hash_bytes(&bytes) != *sha {
            return Err(BlobError::Mismatch);
        }
        if self.exists(token, sha).await? {
            progress(Progress {
                done: size,
                total: size,
            });
            return Ok(false);
        }
        self.upload_bytes_with(token, bytes, sha, Arc::new(progress))
            .await?;
        Ok(true)
    }

    /// The cached file for a hash, when it is in the cache and not empty.
    pub fn cached(&self, sha: &Sha256Hash) -> Option<PathBuf> {
        let path = self.cache_dir.as_ref()?.join(sha.as_str());
        let meta = std::fs::metadata(&path).ok()?;
        (meta.is_file() && meta.len() > 0).then_some(path)
    }

    /// Gets a blob into the cache: `GET /v1/blobs/{sha}`. A file that is in
    /// the cache returns at once, with no request. Otherwise the body streams
    /// to a temporary file in the cache folder, and the hash is checked as it
    /// is written. A right hash renames the file to `<cache>/<sha>`. A wrong
    /// hash deletes it and gives [`BlobError::Mismatch`].
    ///
    /// `progress` uses `expected_size` as the total, or the length that the
    /// hub declares when `expected_size` is `None`.
    pub async fn download(
        &self,
        token: &str,
        sha: &Sha256Hash,
        expected_size: Option<u64>,
        progress: impl Fn(Progress) + Send + Sync,
    ) -> Result<PathBuf, BlobError> {
        let Some(cache_dir) = self.cache_dir.as_ref() else {
            return Err(BlobError::NoCache);
        };
        if let Some(path) = self.cached(sha) {
            let size = file_size(&path).await?;
            progress(Progress {
                done: size,
                total: size,
            });
            return Ok(path);
        }
        if let Some(size) = expected_size {
            check_size(size)?;
        }
        tokio::fs::create_dir_all(cache_dir)
            .await
            .map_err(|err| io_error(cache_dir, &err))?;
        let target = cache_dir.join(sha.as_str());
        let temp = cache_dir.join(format!("{sha}.part-{}", uuid::Uuid::now_v7().simple()));
        let result = self
            .fetch_to(token, sha, expected_size, &temp, &progress)
            .await;
        if let Err(err) = result {
            // Best effort: the temporary file may not exist.
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(err);
        }
        if let Err(err) = tokio::fs::rename(&temp, &target).await {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(io_error(&target, &err));
        }
        Ok(target)
    }

    fn url(&self, sha: &Sha256Hash) -> String {
        format!("{}{}", self.base, blob_path(sha))
    }

    async fn upload_file_with(
        &self,
        token: &str,
        path: &Path,
        sha: &Sha256Hash,
        size: u64,
        progress: ProgressFn,
    ) -> Result<(), BlobError> {
        check_size(size)?;
        if file_size(path).await? != size {
            return Err(BlobError::Mismatch);
        }
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|err| io_error(path, &err))?;
        let reader = FileChunks {
            file,
            done: 0,
            total: size,
            progress: progress.clone(),
        };
        let body = Body::wrap_stream(stream::unfold(reader, FileChunks::next));
        self.put(token, sha, size, body).await?;
        progress(Progress {
            done: size,
            total: size,
        });
        Ok(())
    }

    async fn upload_bytes_with(
        &self,
        token: &str,
        bytes: Vec<u8>,
        sha: &Sha256Hash,
        progress: ProgressFn,
    ) -> Result<(), BlobError> {
        let size = bytes.len() as u64;
        check_size(size)?;
        let pieces = ByteChunks {
            bytes,
            offset: 0,
            progress: progress.clone(),
        };
        let body = Body::wrap_stream(stream::unfold(pieces, ByteChunks::next));
        self.put(token, sha, size, body).await?;
        progress(Progress {
            done: size,
            total: size,
        });
        Ok(())
    }

    async fn put(
        &self,
        token: &str,
        sha: &Sha256Hash,
        size: u64,
        body: Body,
    ) -> Result<(), BlobError> {
        let response = self
            .http
            .put(self.url(sha))
            .bearer_auth(token)
            .header(header::CONTENT_LENGTH, size)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(|_| BlobError::Unreachable)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(status_error(response).await)
        }
    }

    /// Streams the blob into `temp` and checks the hash.
    async fn fetch_to(
        &self,
        token: &str,
        sha: &Sha256Hash,
        expected_size: Option<u64>,
        temp: &Path,
        progress: &(impl Fn(Progress) + Send + Sync),
    ) -> Result<(), BlobError> {
        let mut response = self
            .http
            .get(self.url(sha))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| BlobError::Unreachable)?;
        if !response.status().is_success() {
            return Err(status_error(response).await);
        }
        let declared = response.content_length();
        if let Some(size) = declared {
            check_size(size)?;
        }
        let total = expected_size.or(declared).unwrap_or(0);
        let mut file = tokio::fs::File::create(temp)
            .await
            .map_err(|err| io_error(temp, &err))?;
        let mut hasher = Sha256::new();
        let mut done = 0_u64;
        progress(Progress { done, total });
        while let Some(chunk) = response.chunk().await.map_err(|_| BlobError::Unreachable)? {
            done += chunk.len() as u64;
            check_size(done)?;
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|err| io_error(temp, &err))?;
            progress(Progress {
                done,
                total: total.max(done),
            });
        }
        file.flush().await.map_err(|err| io_error(temp, &err))?;
        drop(file);
        if digest(hasher) != *sha {
            return Err(BlobError::Mismatch);
        }
        Ok(())
    }
}

/// Reads a file in pieces for an upload and reports progress after each one.
struct FileChunks {
    file: tokio::fs::File,
    done: u64,
    total: u64,
    progress: ProgressFn,
}

impl FileChunks {
    async fn next(mut self) -> Option<(Result<Vec<u8>, std::io::Error>, FileChunks)> {
        let mut buffer = vec![0_u8; CHUNK_BYTES];
        match self.file.read(&mut buffer).await {
            Ok(0) => None,
            Ok(read) => {
                buffer.truncate(read);
                self.done += read as u64;
                (self.progress)(Progress {
                    done: self.done.min(self.total),
                    total: self.total,
                });
                Some((Ok(buffer), self))
            }
            Err(err) => Some((Err(err), self)),
        }
    }
}

/// Cuts bytes in memory into pieces for an upload and reports progress after
/// each one.
struct ByteChunks {
    bytes: Vec<u8>,
    offset: usize,
    progress: ProgressFn,
}

impl ByteChunks {
    async fn next(mut self) -> Option<(Result<Vec<u8>, std::io::Error>, ByteChunks)> {
        if self.offset >= self.bytes.len() {
            return None;
        }
        let end = (self.offset + CHUNK_BYTES).min(self.bytes.len());
        let piece = self.bytes[self.offset..end].to_vec();
        self.offset = end;
        (self.progress)(Progress {
            done: end as u64,
            total: self.bytes.len() as u64,
        });
        Some((Ok(piece), self))
    }
}

fn digest(hasher: Sha256) -> Sha256Hash {
    let bytes: [u8; 32] = hasher.finalize().into();
    Sha256Hash::from_bytes(bytes)
}

fn check_size(size: u64) -> Result<(), BlobError> {
    if size > MAX_BLOB_BYTES {
        Err(BlobError::TooLarge { size })
    } else {
        Ok(())
    }
}

/// The size of a file, checked against the limit before anyone reads it.
async fn file_size(path: &Path) -> Result<u64, BlobError> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|err| io_error(path, &err))?;
    if !meta.is_file() {
        return Err(BlobError::Io {
            path: path.display().to_string(),
            message: "it is not a file".into(),
        });
    }
    check_size(meta.len())?;
    Ok(meta.len())
}

/// Turns a failed answer from the hub into an error. Reads the hub's own
/// message from `{"error": {"code", "message"}}` when it is there.
async fn status_error(mut response: Response) -> BlobError {
    let status = response.status();
    let mut body = Vec::new();
    while body.len() < ERROR_BODY_LIMIT {
        match response.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            _ => break,
        }
    }
    let message = hub_message(&body).unwrap_or_else(|| {
        format!(
            "The hub answered the file request with status {}. Try again",
            status.as_u16()
        )
    });
    match status {
        StatusCode::UNAUTHORIZED => BlobError::Unauthorized,
        StatusCode::NOT_FOUND => BlobError::NotFound,
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE => BlobError::Refused { message },
        _ => BlobError::Hub {
            status: status.as_u16(),
            message,
        },
    }
}

fn hub_message(body: &[u8]) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        error: Inner,
    }
    #[derive(serde::Deserialize)]
    struct Inner {
        message: String,
    }
    let envelope: Envelope = serde_json::from_slice(body).ok()?;
    let message = envelope.error.message.trim().to_owned();
    (!message.is_empty()).then_some(message)
}

/// A DNS resolver for a hub that is reached over plain `http` by a name. It
/// refuses a name that points to a public address, as pairing does.
struct PrivateOnlyResolver;

impl Resolve for PrivateOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs: Vec<std::net::SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addrs.is_empty() || addrs.iter().any(|addr| !ip_is_private(addr.ip())) {
                return Err(std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    format!("{host} is not on this machine or a private network"),
                )
                .into());
            }
            let addrs: Addrs = Box::new(addrs.into_iter());
            Ok(addrs)
        })
    }
}

/// True for an address on this machine or on a private network. The same
/// rule as the one for the hub URL: loopback, private, link-local, and the
/// Tailscale range.
fn ip_is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ipv4_is_private(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => ipv4_is_private(v4),
            None => ipv6_is_private(ip),
        },
    }
}

fn ipv4_is_private(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || (a == 100 && (b & 0b1100_0000) == 0b0100_0000)
}

fn ipv6_is_private(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn hashes_bytes() {
        assert_eq!(
            BlobTransfer::hash_bytes(b"").as_str(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            BlobTransfer::hash_bytes(b"abc").as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn hash_of_a_file_matches_the_hash_of_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        let data: Vec<u8> = (0..700_000_u32).map(|n| (n % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let seen = std::sync::Mutex::new(Vec::new());
        let (sha, size) = BlobTransfer::hash_file(&path, |p| seen.lock().unwrap().push(p))
            .await
            .unwrap();
        assert_eq!(sha, BlobTransfer::hash_bytes(&data));
        assert_eq!(size, data.len() as u64);
        let seen = seen.into_inner().unwrap();
        assert!(seen.len() > 1);
        assert!(seen.windows(2).all(|w| w[0].done <= w[1].done));
        assert_eq!(
            seen.last().copied(),
            Some(Progress {
                done: size,
                total: size
            })
        );
    }

    #[tokio::test]
    async fn an_empty_file_has_a_hash_and_one_progress_call() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty");
        std::fs::write(&path, b"").unwrap();
        let count = std::sync::atomic::AtomicUsize::new(0);
        let (sha, size) = BlobTransfer::hash_file(&path, |_| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        })
        .await
        .unwrap();
        assert_eq!(size, 0);
        assert_eq!(sha, BlobTransfer::hash_bytes(b""));
        assert_eq!(count.into_inner(), 1);
    }

    #[tokio::test]
    async fn hash_file_refuses_a_file_over_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_BLOB_BYTES + 1).unwrap();
        let err = BlobTransfer::hash_file(&path, |_| panic!("must not read"))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            BlobError::TooLarge {
                size: MAX_BLOB_BYTES + 1
            }
        );
    }

    #[tokio::test]
    async fn hash_file_names_the_path_when_the_file_is_missing() {
        let err = BlobTransfer::hash_file(Path::new("/no/such/consort-file"), |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, BlobError::Io { ref path, .. } if path.contains("consort-file")));
    }

    #[test]
    fn size_message_rounds_up_to_whole_megabytes() {
        assert_eq!(
            BlobError::TooLarge {
                size: MAX_BLOB_BYTES + 1
            }
            .to_string(),
            "This file is 257 MB. The limit is 256 MB. Choose a smaller file"
        );
        assert_eq!(
            BlobError::TooLarge { size: 300 * MIB }.to_string(),
            "This file is 300 MB. The limit is 256 MB. Choose a smaller file"
        );
    }

    #[test]
    fn the_hub_url_loses_its_path_and_trailing_slash() {
        let transfer = BlobTransfer::new("https://hub.example.com/", None).unwrap();
        assert_eq!(transfer.base, "https://hub.example.com");
        let sha = BlobTransfer::hash_bytes(b"x");
        assert_eq!(
            transfer.url(&sha),
            format!("https://hub.example.com/v1/blobs/{sha}")
        );
    }

    #[test]
    fn a_bad_hub_url_is_refused() {
        let err = BlobTransfer::new("http://example.com", None).unwrap_err();
        assert!(matches!(err, BlobError::InvalidUrl { .. }));
    }

    #[test]
    fn errors_map_to_client_errors() {
        assert!(matches!(
            ClientError::from(BlobError::Unauthorized),
            ClientError::Hub { ref code, .. } if code == "unauthorized"
        ));
        assert!(matches!(
            ClientError::from(BlobError::Io {
                path: "/a".into(),
                message: "denied".into()
            }),
            ClientError::Storage { .. }
        ));
        assert!(matches!(
            ClientError::from(BlobError::Mismatch),
            ClientError::Other(_)
        ));
    }

    #[test]
    fn reads_the_hub_message() {
        let body = br#"{"error":{"code":"invalid","message":"The blob is too large."}}"#;
        assert_eq!(hub_message(body).as_deref(), Some("The blob is too large."));
        assert_eq!(hub_message(b"not json"), None);
    }

    #[test]
    fn only_private_addresses_pass() {
        assert!(ip_is_private("127.0.0.1".parse().unwrap()));
        assert!(ip_is_private("100.100.1.1".parse().unwrap()));
        assert!(ip_is_private("::1".parse().unwrap()));
        assert!(!ip_is_private("8.8.8.8".parse().unwrap()));
        assert!(!ip_is_private("2001:4860:4860::8888".parse().unwrap()));
    }

    #[tokio::test]
    async fn a_download_without_a_cache_folder_says_so() {
        let transfer = BlobTransfer::new("http://127.0.0.1:9", None).unwrap();
        let sha = BlobTransfer::hash_bytes(b"x");
        let err = transfer
            .download("t", &sha, None, |_| {})
            .await
            .unwrap_err();
        assert_eq!(err, BlobError::NoCache);
    }
}
