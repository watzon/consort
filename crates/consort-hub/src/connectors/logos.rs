//! The logo cache.
//!
//! Composio gives each service a logo URL on its own servers. The hub does
//! not pass those URLs to clients. It fetches each logo once, keeps it in
//! `<data dir>/logos/`, and gives clients its own URL,
//! `<public url>/v1/logos/<key>`. The key is the SHA-256 of the origin URL
//! in hex. So a client never contacts a third party, a logo still shows when
//! Composio is down, and a hub on https gives https logos.
//!
//! Rules:
//! - Only `https` origins (`http` too in tests). A logo is at most
//!   [`LOGO_MAX_BYTES`] and must be a PNG, JPEG, GIF, WebP or SVG image. The
//!   check reads the bytes, not the `Content-Type` header.
//! - A cached logo is fresh for [`LOGO_REFRESH`]. After that, the next
//!   request fetches it again. When that fails, the old copy answers.
//! - The cache keeps at most [`CACHE_MAX_FILES`] files and
//!   [`CACHE_MAX_BYTES`] bytes. The oldest logos go first. A logo that went
//!   away is fetched again when a client asks for it.
//! - The route `GET /v1/logos/{key}` needs no login, because a client loads
//!   it as a plain image. It serves only a key that the hub registered, so
//!   it is not a proxy for other URLs.
//! - A key that the hub gave to a persistent record (a connection) has a
//!   `<key>.url` file with its origin, so it works after a restart. Catalog
//!   keys live in memory only. The client asks for the catalog again when it
//!   opens the dialog.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::error::{HubError, Result};

/// The largest logo that the hub keeps. The client loads the same size.
pub(crate) const LOGO_MAX_BYTES: usize = 256 * 1024;
/// How long a cached logo is fresh.
pub(crate) const LOGO_REFRESH: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// The most logo files in the cache.
pub(crate) const CACHE_MAX_FILES: usize = 1_000;
/// The most bytes in the cache.
pub(crate) const CACHE_MAX_BYTES: u64 = 32 * 1024 * 1024;
/// The time limit of one fetch.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a failed fetch keeps the hub from asking the origin again.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(10 * 60);
/// The path prefix of the route, below the hub address.
pub(crate) const ROUTE_PREFIX: &str = "/v1/logos/";

/// A logo to send.
#[derive(Debug, Clone)]
pub(crate) struct Logo {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

/// The kind of image in `bytes`, as a content type. `None` when the bytes are
/// not an image that clients can draw, or are empty or too large.
pub(crate) fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.is_empty() || bytes.len() > LOGO_MAX_BYTES {
        return None;
    }
    if bytes.starts_with(b"\x89PNG") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let head = text.trim_start_matches('\u{feff}').trim_start();
    let is_svg = head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg"));
    is_svg.then_some("image/svg+xml")
}

/// The logo cache of a hub.
pub struct LogoCache {
    dir: PathBuf,
    /// Origin URLs by key, for every key that the hub gave out.
    origins: Mutex<HashMap<String, String>>,
    /// Keys whose last fetch failed, and when.
    failed: Mutex<HashMap<String, std::time::Instant>>,
    /// One fetch at a time for each key.
    fetching: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Allow `http` origins and an origin on this machine. Tests only.
    allow_local: Mutex<bool>,
    /// The age after which a logo is fetched again. Tests make it short.
    refresh: Mutex<Duration>,
    limits: Mutex<(usize, u64)>,
    /// The time limit of one fetch.
    timeout: Mutex<Duration>,
}

impl LogoCache {
    pub(crate) fn open(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("logos"),
            origins: Mutex::default(),
            failed: Mutex::default(),
            fetching: Mutex::default(),
            allow_local: Mutex::new(false),
            refresh: Mutex::new(LOGO_REFRESH),
            limits: Mutex::new((CACHE_MAX_FILES, CACHE_MAX_BYTES)),
            timeout: Mutex::new(FETCH_TIMEOUT),
        }
    }

    pub(crate) fn set_allow_local(&self, allow: bool) {
        if let Ok(mut slot) = self.allow_local.lock() {
            *slot = allow;
        }
    }

    #[cfg(test)]
    pub(crate) fn set_refresh(&self, refresh: Duration) {
        if let Ok(mut slot) = self.refresh.lock() {
            *slot = refresh;
        }
    }

    #[cfg(test)]
    pub(crate) fn set_limits(&self, files: usize, bytes: u64) {
        if let Ok(mut slot) = self.limits.lock() {
            *slot = (files, bytes);
        }
    }

    #[cfg(test)]
    pub(crate) fn set_timeout(&self, timeout: Duration) {
        if let Ok(mut slot) = self.timeout.lock() {
            *slot = timeout;
        }
    }

    fn timeout(&self) -> Duration {
        self.timeout.lock().map_or(FETCH_TIMEOUT, |t| *t)
    }

    fn allow_local(&self) -> bool {
        self.allow_local.lock().is_ok_and(|a| *a)
    }

    fn refresh(&self) -> Duration {
        self.refresh.lock().map_or(LOGO_REFRESH, |r| *r)
    }

    fn limits(&self) -> (usize, u64) {
        self.limits
            .lock()
            .map_or((CACHE_MAX_FILES, CACHE_MAX_BYTES), |l| *l)
    }

    /// The key of an origin URL.
    pub(crate) fn key_of(origin: &str) -> String {
        crate::util::sha256(origin.trim().as_bytes())
            .as_str()
            .to_owned()
    }

    /// Whether `key` is a key (64 lowercase hex characters).
    pub(crate) fn is_key(key: &str) -> bool {
        key.len() == 64 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }

    fn origin_allowed(&self, origin: &str) -> bool {
        let lower = origin.trim().to_ascii_lowercase();
        let ok_scheme =
            lower.starts_with("https://") || (self.allow_local() && lower.starts_with("http://"));
        ok_scheme && !lower.contains(char::is_whitespace) && lower.len() <= 2_048
    }

    fn file(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    fn origin_file(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.url"))
    }

    /// Registers an origin URL and returns its key. `None` when the URL is
    /// not one that the hub fetches. With `persist`, the origin is also kept
    /// in a file, so the key works after a restart.
    pub(crate) fn register(&self, origin: &str, persist: bool) -> Option<String> {
        let origin = origin.trim();
        if !self.origin_allowed(origin) {
            return None;
        }
        let key = Self::key_of(origin);
        if let Ok(mut origins) = self.origins.lock() {
            origins.insert(key.clone(), origin.to_owned());
        }
        if persist {
            let path = self.origin_file(&key);
            if !path.exists() {
                let written = std::fs::create_dir_all(&self.dir)
                    .and_then(|()| std::fs::write(&path, origin.as_bytes()));
                if let Err(err) = written {
                    tracing::warn!(error = %err, "could not record the origin of a logo");
                }
            }
        }
        Some(key)
    }

    fn origin_of(&self, key: &str) -> Option<String> {
        if let Some(origin) = self.origins.lock().ok().and_then(|o| o.get(key).cloned()) {
            return Some(origin);
        }
        let origin = std::fs::read_to_string(self.origin_file(key)).ok()?;
        let origin = origin.trim().to_owned();
        // The file name is the hash of its content. A file that does not
        // match is not ours.
        (Self::key_of(&origin) == key && self.origin_allowed(&origin)).then_some(origin)
    }

    /// Whether the hub has an origin for this key.
    pub(crate) fn knows(&self, key: &str) -> bool {
        Self::is_key(key) && self.origin_of(key).is_some()
    }

    fn read_cached(&self, key: &str) -> Option<(Logo, SystemTime)> {
        let path = self.file(key);
        let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
        let bytes = std::fs::read(&path).ok()?;
        // The type comes from the bytes again, so a damaged file is not sent.
        let content_type = image_type(&bytes)?;
        Some((
            Logo {
                bytes,
                content_type,
            },
            modified,
        ))
    }

    /// The logo for a key: the cached copy while it is fresh, else a fresh
    /// fetch. `None` when the key is not known, or the origin gives no
    /// logo and the cache has none.
    pub(crate) async fn get(&self, key: &str) -> Option<Logo> {
        if !Self::is_key(key) {
            return None;
        }
        let cached = self.read_cached(key);
        if let Some((logo, modified)) = &cached
            && SystemTime::now()
                .duration_since(*modified)
                .is_ok_and(|age| age < self.refresh())
        {
            return Some(logo.clone());
        }
        let origin = self.origin_of(key)?;
        let lock = {
            let mut fetching = self.fetching.lock().ok()?;
            fetching.entry(key.to_owned()).or_default().clone()
        };
        let _one_at_a_time = lock.lock().await;
        // Another request may have fetched it while this one waited.
        if let Some((logo, modified)) = self.read_cached(key)
            && SystemTime::now()
                .duration_since(modified)
                .is_ok_and(|age| age < self.refresh())
        {
            return Some(logo);
        }
        let recently_failed = self
            .failed
            .lock()
            .ok()
            .and_then(|f| f.get(key).copied())
            .is_some_and(|at| at.elapsed() < RETRY_AFTER_FAILURE);
        if !recently_failed {
            match self.fetch(&origin).await {
                Ok(bytes) => {
                    if let Ok(mut failed) = self.failed.lock() {
                        failed.remove(key);
                    }
                    if let Err(err) = self.store(key, &bytes) {
                        tracing::warn!(error = %err, "could not keep a logo in the cache");
                    }
                    let content_type = image_type(&bytes)?;
                    return Some(Logo {
                        bytes,
                        content_type,
                    });
                }
                Err(message) => {
                    tracing::warn!(%origin, %message, "could not fetch a service logo");
                    if let Ok(mut failed) = self.failed.lock() {
                        failed.insert(key.to_owned(), std::time::Instant::now());
                    }
                }
            }
        }
        // The old copy is better than no logo.
        cached.map(|(logo, _)| logo)
    }

    /// Fetches the bytes of a logo, and checks them.
    async fn fetch(&self, origin: &str) -> std::result::Result<Vec<u8>, String> {
        let limit = self.timeout();
        let http = reqwest::Client::builder()
            .timeout(limit)
            .https_only(!self.allow_local())
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent(concat!("consort-hub/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| "the HTTP client did not start".to_owned())?;
        let mut response = http.get(origin).send().await.map_err(|err| {
            if err.is_timeout() {
                format!(
                    "the origin did not answer within {}",
                    crate::util::duration_text(limit)
                )
            } else {
                "the origin could not be reached".to_owned()
            }
        })?;
        if !response.status().is_success() {
            return Err(format!(
                "the origin answered {}",
                response.status().as_u16()
            ));
        }
        if response
            .content_length()
            .is_some_and(|len| len > LOGO_MAX_BYTES as u64)
        {
            return Err("the logo is too large".to_owned());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "the answer stopped early".to_owned())?
        {
            body.extend_from_slice(&chunk);
            if body.len() > LOGO_MAX_BYTES {
                return Err("the logo is too large".to_owned());
            }
        }
        if image_type(&body).is_none() {
            return Err("the answer is not an image that clients can draw".to_owned());
        }
        Ok(body)
    }

    /// Writes a logo through a temp file and a rename, then keeps the cache
    /// inside its limits.
    fn store(&self, key: &str, bytes: &[u8]) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let temp = self
            .dir
            .join(format!(".tmp-{}", &crate::util::random_token()[..12]));
        std::fs::write(&temp, bytes)?;
        if let Err(err) = std::fs::rename(&temp, self.file(key)) {
            let _ = std::fs::remove_file(&temp);
            return Err(HubError::from(err));
        }
        self.enforce_limits(key);
        Ok(())
    }

    /// Removes the oldest logos while the cache is over a limit. The logo
    /// that was just written stays. A removed logo keeps its `.url` file, so
    /// the next request fetches it again.
    fn enforce_limits(&self, keep: &str) {
        let (max_files, max_bytes) = self.limits();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, u64, PathBuf, String)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !Self::is_key(&name) {
                    return None;
                }
                let meta = entry.metadata().ok()?;
                Some((meta.modified().ok()?, meta.len(), entry.path(), name))
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        let mut count = files.len();
        files.sort_by_key(|f| f.0);
        for (_, size, path, name) in files {
            if count <= max_files && total <= max_bytes {
                break;
            }
            if name == keep {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                count -= 1;
                total = total.saturating_sub(size);
            }
        }
    }

    /// The number of logo files and their total size. For tests and logs.
    #[cfg(test)]
    pub(crate) fn usage(&self) -> (usize, u64) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return (0, 0);
        };
        entries
            .flatten()
            .filter(|e| Self::is_key(&e.file_name().to_string_lossy()))
            .filter_map(|e| e.metadata().ok())
            .fold((0, 0), |(n, b), m| (n + 1, b + m.len()))
    }

    /// Deletes leftovers of a fetch that a stop interrupted.
    pub(crate) fn clean(&self) {
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(".tmp-") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n0000";
    const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";

    #[test]
    fn only_images_that_clients_draw_pass() {
        assert_eq!(image_type(PNG), Some("image/png"));
        assert_eq!(image_type(SVG), Some("image/svg+xml"));
        assert_eq!(
            image_type(b"<?xml version=\"1.0\"?><svg/>"),
            Some("image/svg+xml")
        );
        assert_eq!(image_type(b"<html>not a logo</html>"), None);
        assert_eq!(image_type(b""), None);
        assert_eq!(image_type(&vec![b'a'; LOGO_MAX_BYTES + 1]), None);
        assert_eq!(image_type(b"GIF89a.."), Some("image/gif"));
    }

    #[test]
    fn keys_are_hashes_of_the_origin() {
        let key = LogoCache::key_of("https://logos.example/a.svg");
        assert!(LogoCache::is_key(&key));
        assert!(!LogoCache::is_key("../etc/passwd"));
        assert!(!LogoCache::is_key(&key.to_uppercase()));
        assert_eq!(key, LogoCache::key_of(" https://logos.example/a.svg "));
    }

    #[test]
    fn only_https_origins_register() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LogoCache::open(dir.path());
        assert!(
            cache
                .register("https://logos.example/a.svg", false)
                .is_some()
        );
        assert!(
            cache
                .register("http://logos.example/a.svg", false)
                .is_none()
        );
        assert!(cache.register("file:///etc/passwd", false).is_none());
        assert!(cache.register("not a url", false).is_none());
        cache.set_allow_local(true);
        assert!(cache.register("http://127.0.0.1:1/a.svg", false).is_some());
    }

    #[test]
    fn a_persisted_origin_survives_a_restart_and_a_planted_file_does_not_count() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LogoCache::open(dir.path());
        let key = cache.register("https://logos.example/a.svg", true).unwrap();
        let again = LogoCache::open(dir.path());
        assert_eq!(
            again.origin_of(&key).as_deref(),
            Some("https://logos.example/a.svg")
        );
        // A key with another origin in its file is not trusted.
        let other = LogoCache::key_of("https://logos.example/b.svg");
        std::fs::write(
            dir.path().join("logos").join(format!("{other}.url")),
            "https://evil.example/x.svg",
        )
        .unwrap();
        assert!(again.origin_of(&other).is_none());
    }
}
