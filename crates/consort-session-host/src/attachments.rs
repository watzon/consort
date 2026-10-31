//! Turn attachments and channel files on the worker.
//!
//! Before a turn starts, the worker downloads each attachment of the turn
//! with the signed blob GET, checks its SHA-256, and saves it at
//! [`consort_harness::attachments::attachment_path`] in the workspace. The
//! adapters find the files there and tell the model the paths.
//!
//! The channel files go into a read-only copy in
//! `<workspace>/.channel/files/` (see [`ChannelCopy`]). The copy is only a
//! copy: the worker never sends it back to the hub. A session publishes a
//! file with `publish_artifact`, and a coordinator writes one with
//! `files_write`.
//!
//! Both use one cache by hash under the Consort home,
//! `<consort home>/cache/blobs/<sha256>`. A file enters the cache only after
//! its hash matches, and each use checks the hash again.

use std::collections::{BTreeMap, HashMap};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use consort_harness::attachments::{CHANNEL_FILES_DIR, attachment_path, format_size};
use consort_proto::model::{BlobRef, FileRef, Sha256Hash, validate_relative_path};
use sha2::{Digest, Sha256};

use crate::HostError;

/// The cache folder, inside the Consort home.
pub const CACHE_DIR: &str = "cache/blobs";
/// The largest blob that the worker downloads: the hub's blob limit.
pub const BLOB_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// The `code` of the error event for a file that a session did not get. The
/// hub also posts the message as a notice for the user.
pub const ATTACHMENT_FAILED_CODE: &str = "attachment_failed";

/// Gets blobs from the hub. The worker uses the signed blob GET. Tests use
/// a fake.
#[async_trait]
pub trait BlobFetch: Send + Sync {
    /// Downloads a blob into the new file `dest`. Stops with an error when
    /// the blob is larger than `max_bytes`. The caller checks the hash.
    async fn fetch(
        &self,
        sha256: &Sha256Hash,
        max_bytes: u64,
        dest: &Path,
    ) -> Result<(), HostError>;
}

/// Why the worker could not get a file for a session.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// The file is larger than the blob limit.
    #[error("it is larger than {}", format_size(BLOB_MAX_BYTES))]
    TooLarge,
    /// The bytes from the hub do not have the expected hash.
    #[error("the downloaded bytes do not match the file's SHA-256")]
    HashMismatch,
    /// The download failed.
    #[error("the download failed: {0}")]
    Download(HostError),
    /// A file could not be read or written on this worker.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The hub gave a path that is not a safe relative path.
    #[error("the path is not valid: {0}")]
    InvalidPath(String),
}

/// One file that a session did not get.
#[derive(Debug)]
pub struct Failure {
    /// The attachment name or the channel file path.
    pub name: String,
    /// Why.
    pub error: FetchError,
}

impl Failure {
    /// The text for the user.
    pub fn message(&self, what: &str) -> String {
        format!(
            "The worker could not get the {what} {}, because {}. The session continues without it.",
            self.name, self.error
        )
    }
}

/// The SHA-256 of a file.
async fn hash_file(path: &Path) -> std::io::Result<Sha256Hash> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0_u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(Sha256Hash::from_bytes(hasher.finalize().into()))
    })
    .await
    .map_err(std::io::Error::other)?
}

/// A temporary file name next to `target`.
fn temp_next_to(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let short: String = name.chars().take(40).collect();
    target.with_file_name(format!(".{short}.{}.tmp", uuid::Uuid::new_v4().simple()))
}

/// Whether a path is a regular file, without following a link.
fn regular_file(path: &Path) -> Option<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.file_type().is_file())
}

/// Removes a file or a link, also one that is read-only on Windows.
fn remove_entry(path: &Path) -> std::io::Result<()> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if meta.file_type().is_dir() {
        return std::fs::remove_dir_all(path);
    }
    if meta.file_type().is_file() && meta.permissions().readonly() {
        let mut perms = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(path, perms);
    }
    std::fs::remove_file(path)
}

/// Makes `base/<parts>` a real folder. A link or a file in the way is
/// removed first, so the worker never writes through a link that a session
/// put there.
fn real_dir(base: &Path, parts: &[&str]) -> std::io::Result<PathBuf> {
    let mut dir = base.to_path_buf();
    for part in parts {
        dir.push(part);
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => {
                remove_entry(&dir)?;
                std::fs::create_dir(&dir)?;
            }
            Err(_) => std::fs::create_dir(&dir)?,
        }
    }
    Ok(dir)
}

/// Copies `src` to `dest` through a temporary file in the same folder, so
/// a link at `dest` is replaced and not followed. A read-only copy cannot
/// be changed by accident.
fn copy_into(src: &Path, dest: &Path, read_only: bool) -> std::io::Result<()> {
    let temp = temp_next_to(dest);
    let copied = (|| {
        std::fs::copy(src, &temp)?;
        let mut perms = std::fs::metadata(&temp)?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            perms.set_mode(if read_only { 0o444 } else { 0o644 });
        }
        #[cfg(not(unix))]
        {
            perms.set_readonly(read_only);
        }
        std::fs::set_permissions(&temp, perms)?;
        if std::fs::symlink_metadata(dest).is_ok() {
            remove_entry(dest)?;
        }
        std::fs::rename(&temp, dest)
    })();
    if copied.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    copied
}

/// The cache of verified blobs.
#[derive(Debug, Clone)]
pub struct BlobCache {
    root: PathBuf,
}

impl BlobCache {
    /// The cache in a Consort home.
    pub fn new(consort_home: &Path) -> Self {
        Self {
            root: consort_home.join(CACHE_DIR),
        }
    }

    /// The cache folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn make_root(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// A checked copy of a blob in the cache. The worker downloads the blob
    /// when the cache does not have it, or when the cached copy fails the
    /// check.
    pub async fn ensure(
        &self,
        fetch: &dyn BlobFetch,
        sha256: &Sha256Hash,
    ) -> Result<PathBuf, FetchError> {
        self.make_root()?;
        let cached = self.root.join(sha256.as_str());
        if regular_file(&cached).is_some() {
            if hash_file(&cached).await? == *sha256 {
                return Ok(cached);
            }
            tracing::warn!(sha256 = %sha256, "a cached blob failed its check; the worker downloads it again");
            remove_entry(&cached)?;
        }
        let temp = temp_next_to(&cached);
        if let Err(err) = fetch.fetch(sha256, BLOB_MAX_BYTES, &temp).await {
            let _ = std::fs::remove_file(&temp);
            return Err(FetchError::Download(err));
        }
        let actual = match hash_file(&temp).await {
            Ok(actual) => actual,
            Err(err) => {
                let _ = std::fs::remove_file(&temp);
                return Err(err.into());
            }
        };
        if actual != *sha256 {
            let _ = std::fs::remove_file(&temp);
            return Err(FetchError::HashMismatch);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temp, &cached)?;
        Ok(cached)
    }

    /// Copies a blob from the cache to `dest`, from a task that may block.
    async fn copy_to(
        &self,
        src: PathBuf,
        dest: PathBuf,
        read_only: bool,
    ) -> Result<(), FetchError> {
        tokio::task::spawn_blocking(move || copy_into(&src, &dest, read_only))
            .await
            .map_err(std::io::Error::other)??;
        Ok(())
    }
}

/// Saves the attachments of a turn in the workspace, where the adapters
/// find them. Returns the attachments that the session does not get. A
/// failed attachment does not stop the others or the turn.
pub async fn save_attachments(
    cache: &BlobCache,
    fetch: &dyn BlobFetch,
    workspace: &Path,
    attachments: &[BlobRef],
) -> Vec<Failure> {
    let mut failures = Vec::new();
    if attachments.is_empty() {
        return failures;
    }
    if let Err(err) = real_dir(workspace, &[".consort", "attachments"]) {
        return attachments
            .iter()
            .map(|blob| Failure {
                name: blob.name.clone(),
                error: FetchError::Io(std::io::Error::new(err.kind(), err.to_string())),
            })
            .collect();
    }
    for blob in attachments {
        let dest = attachment_path(workspace, blob);
        let result = async {
            if blob.size > BLOB_MAX_BYTES {
                return Err(FetchError::TooLarge);
            }
            let src = cache.ensure(fetch, &blob.sha256).await?;
            cache.copy_to(src, dest.clone(), false).await
        }
        .await;
        if let Err(error) = result {
            // The adapter then tells the model that the file is missing.
            let _ = remove_entry(&dest);
            failures.push(Failure {
                name: blob.name.clone(),
                error,
            });
        }
    }
    failures
}

/// What the worker wrote for one channel file, so a refresh can skip a file
/// that did not change.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Written {
    sha256: Sha256Hash,
    len: u64,
    modified: Option<SystemTime>,
}

impl Written {
    fn matches(&self, sha256: &Sha256Hash, meta: &std::fs::Metadata) -> bool {
        self.sha256 == *sha256 && self.len == meta.len() && self.modified == meta.modified().ok()
    }
}

/// The relative path of a channel file as a path on this machine, when it
/// is safe: relative, with no `..`, and with nothing that Windows reads as a
/// drive or a stream.
fn safe_relative(path: &str) -> Result<PathBuf, FetchError> {
    validate_relative_path("path", path).map_err(|e| FetchError::InvalidPath(e.to_string()))?;
    if path.contains(':') {
        return Err(FetchError::InvalidPath("path contains a colon".to_owned()));
    }
    Ok(path.split('/').collect())
}

/// Every entry under `root`, without following links. Regular files go in
/// `files` with their path relative to `root` (with `/`). A link or any
/// other entry that is not a file or a folder is removed.
fn walk(root: &Path, rel: &str, files: &mut Vec<(String, PathBuf)>, dirs: &mut Vec<PathBuf>) {
    let dir = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_dir() => {
                walk(root, &child, files, dirs);
                dirs.push(path);
            }
            Ok(meta) if meta.file_type().is_file() => files.push((child, path)),
            _ => {
                let _ = remove_entry(&path);
            }
        }
    }
}

/// The read-only copy of the channel files in `<workspace>/.channel/files/`
/// for one session.
#[derive(Debug, Default)]
pub struct ChannelCopy {
    /// The files that this worker wrote, by path.
    written: HashMap<String, Written>,
}

impl ChannelCopy {
    /// Makes the copy match `files`: it writes new and changed files,
    /// removes files that are no longer channel files, and replaces a file
    /// that the session changed. Returns the files that the session does
    /// not get.
    pub async fn sync(
        &mut self,
        cache: &BlobCache,
        fetch: &dyn BlobFetch,
        workspace: &Path,
        files: &[FileRef],
    ) -> Vec<Failure> {
        let mut failures = Vec::new();
        let parts: Vec<&str> = CHANNEL_FILES_DIR.split('/').collect();
        let root = match real_dir(workspace, &parts) {
            Ok(root) => root,
            Err(err) => {
                return files
                    .iter()
                    .map(|f| Failure {
                        name: f.path.clone(),
                        error: FetchError::Io(std::io::Error::new(err.kind(), err.to_string())),
                    })
                    .collect();
            }
        };
        let mut wanted: BTreeMap<String, (PathBuf, Sha256Hash)> = BTreeMap::new();
        for file in files {
            match safe_relative(&file.path) {
                Ok(rel) => {
                    wanted.insert(file.path.clone(), (rel, file.sha256.clone()));
                }
                Err(error) => failures.push(Failure {
                    name: file.path.clone(),
                    error,
                }),
            }
        }

        let mut present = Vec::new();
        let mut dirs = Vec::new();
        walk(&root, "", &mut present, &mut dirs);
        for (rel, path) in &present {
            if !wanted.contains_key(rel) {
                if let Err(err) = remove_entry(path) {
                    tracing::warn!(path = %path.display(), error = %err, "could not remove an old channel file from a workspace");
                }
                self.written.remove(rel);
            }
        }

        for (name, (rel, sha256)) in &wanted {
            let dest = root.join(rel);
            if let Some(meta) = regular_file(&dest) {
                let known = self
                    .written
                    .get(name)
                    .is_some_and(|w| w.matches(sha256, &meta));
                // After a restart the worker knows nothing it wrote, so it
                // checks the hash once.
                let same = known || hash_file(&dest).await.ok().as_ref() == Some(sha256);
                if same {
                    self.remember(name, sha256, &dest);
                    continue;
                }
            }
            let result = async {
                let src = cache.ensure(fetch, sha256).await?;
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                cache.copy_to(src, dest.clone(), true).await
            }
            .await;
            match result {
                Ok(()) => self.remember(name, sha256, &dest),
                Err(error) => {
                    // An old version must not look current.
                    let _ = remove_entry(&dest);
                    self.written.remove(name);
                    failures.push(Failure {
                        name: name.clone(),
                        error,
                    });
                }
            }
        }

        // Folders that are empty now go. `walk` listed the deepest first.
        for dir in dirs {
            let _ = std::fs::remove_dir(&dir);
        }
        failures
    }

    fn remember(&mut self, name: &str, sha256: &Sha256Hash, dest: &Path) {
        if let Some(meta) = regular_file(dest) {
            self.written.insert(
                name.to_owned(),
                Written {
                    sha256: sha256.clone(),
                    len: meta.len(),
                    modified: meta.modified().ok(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sha(bytes: &[u8]) -> Sha256Hash {
        Sha256Hash::from_bytes(Sha256::digest(bytes).into())
    }

    /// A hub that serves blobs from memory. `lie` serves other bytes for a
    /// hash.
    #[derive(Default)]
    struct FakeBlobs {
        blobs: Mutex<HashMap<String, Vec<u8>>>,
        calls: AtomicUsize,
    }

    impl FakeBlobs {
        fn add(&self, bytes: &[u8]) -> Sha256Hash {
            let hash = sha(bytes);
            self.blobs
                .lock()
                .unwrap()
                .insert(hash.to_string(), bytes.to_vec());
            hash
        }

        fn lie(&self, hash: &Sha256Hash, bytes: &[u8]) {
            self.blobs
                .lock()
                .unwrap()
                .insert(hash.to_string(), bytes.to_vec());
        }
    }

    #[async_trait]
    impl BlobFetch for FakeBlobs {
        async fn fetch(
            &self,
            sha256: &Sha256Hash,
            max_bytes: u64,
            dest: &Path,
        ) -> Result<(), HostError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let bytes = self
                .blobs
                .lock()
                .unwrap()
                .get(sha256.as_str())
                .cloned()
                .ok_or_else(|| HostError::Hub {
                    code: consort_proto::error::ErrorCode::NotFound,
                    message: "no such blob".into(),
                })?;
            assert!(bytes.len() as u64 <= max_bytes);
            std::fs::write(dest, bytes).unwrap();
            Ok(())
        }
    }

    fn blob(name: &str, bytes: &[u8]) -> BlobRef {
        BlobRef {
            sha256: sha(bytes),
            name: name.into(),
            mime: "text/plain".into(),
            size: bytes.len() as u64,
        }
    }

    fn file_ref(path: &str, bytes: &[u8]) -> FileRef {
        FileRef {
            path: path.into(),
            sha256: sha(bytes),
        }
    }

    #[tokio::test]
    async fn attachments_are_saved_checked_and_cached() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let cache = BlobCache::new(home.path());
        let hub = FakeBlobs::default();
        hub.add(b"hello");
        let good = blob("../../evil\nname.txt", b"hello");
        let failures = save_attachments(&cache, &hub, ws.path(), std::slice::from_ref(&good)).await;
        assert!(failures.is_empty(), "{failures:?}");
        let path = attachment_path(ws.path(), &good);
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("{}-_.._evil_name.txt", &good.sha256.as_str()[..12])
        );
        assert!(path.starts_with(ws.path().join(".consort/attachments")));
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        assert!(
            home.path()
                .join(CACHE_DIR)
                .join(good.sha256.as_str())
                .is_file()
        );
        assert_eq!(hub.calls.load(Ordering::SeqCst), 1);

        // A second session uses the cache and asks the hub for nothing.
        let other = tempfile::tempdir().unwrap();
        let failures =
            save_attachments(&cache, &hub, other.path(), std::slice::from_ref(&good)).await;
        assert!(failures.is_empty());
        assert_eq!(hub.calls.load(Ordering::SeqCst), 1);

        // A cache file that changed is fetched again.
        std::fs::write(
            home.path().join(CACHE_DIR).join(good.sha256.as_str()),
            b"bad",
        )
        .unwrap();
        let failures = save_attachments(&cache, &hub, other.path(), &[good]).await;
        assert!(failures.is_empty());
        assert_eq!(hub.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_failed_attachment_fails_alone() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let cache = BlobCache::new(home.path());
        let hub = FakeBlobs::default();
        hub.add(b"fine");
        let fine = blob("fine.txt", b"fine");
        // The hub sends other bytes than the hash says.
        let wrong = blob("wrong.txt", b"expected");
        hub.lie(&wrong.sha256, b"tampered");
        let missing = blob("missing.txt", b"never uploaded");
        let mut huge = blob("huge.bin", b"x");
        huge.size = BLOB_MAX_BYTES + 1;
        // An old file with the same name goes, so it does not look current.
        let stale = attachment_path(ws.path(), &wrong);
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(&stale, b"old").unwrap();

        let failures = save_attachments(
            &cache,
            &hub,
            ws.path(),
            &[fine.clone(), wrong.clone(), missing, huge],
        )
        .await;
        let reasons: Vec<(&str, String)> = failures
            .iter()
            .map(|f| (f.name.as_str(), f.error.to_string()))
            .collect();
        assert_eq!(reasons.len(), 3, "{reasons:?}");
        assert!(matches!(failures[0].error, FetchError::HashMismatch));
        assert!(matches!(failures[1].error, FetchError::Download(_)));
        assert!(matches!(failures[2].error, FetchError::TooLarge));
        assert_eq!(
            failures[2].message("attachment"),
            "The worker could not get the attachment huge.bin, because it is larger than 256.0 MB. The session continues without it."
        );
        assert!(attachment_path(ws.path(), &fine).is_file());
        assert!(!stale.exists());
        // Nothing with a wrong hash stays in the cache.
        let cached: Vec<_> = std::fs::read_dir(home.path().join(CACHE_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(cached, vec![fine.sha256.to_string()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_in_the_workspace_is_not_written_through() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let cache = BlobCache::new(home.path());
        let hub = FakeBlobs::default();
        hub.add(b"data");
        // The session made `.consort` a link to a folder outside.
        std::os::unix::fs::symlink(outside.path(), ws.path().join(".consort")).unwrap();
        let b = blob("a.txt", b"data");
        assert!(
            save_attachments(&cache, &hub, ws.path(), std::slice::from_ref(&b))
                .await
                .is_empty()
        );
        assert!(
            std::fs::symlink_metadata(ws.path().join(".consort"))
                .unwrap()
                .is_dir()
        );
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        // The saved file itself is a link to a secret: it is replaced.
        let secret = outside.path().join("secret");
        std::fs::write(&secret, b"secret").unwrap();
        let dest = attachment_path(ws.path(), &b);
        std::fs::remove_file(&dest).unwrap();
        std::os::unix::fs::symlink(&secret, &dest).unwrap();
        assert!(
            save_attachments(&cache, &hub, ws.path(), &[b])
                .await
                .is_empty()
        );
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
        assert!(regular_file(&dest).is_some());
    }

    #[tokio::test]
    async fn the_channel_copy_is_read_only_and_follows_the_list() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let cache = BlobCache::new(home.path());
        let hub = FakeBlobs::default();
        hub.add(b"plan v1");
        hub.add(b"plan v2");
        hub.add(b"notes");
        let root = ws.path().join(".channel/files");
        let mut copy = ChannelCopy::default();

        let first = [
            file_ref("docs/plan.md", b"plan v1"),
            file_ref("notes.txt", b"notes"),
        ];
        assert!(copy.sync(&cache, &hub, ws.path(), &first).await.is_empty());
        assert_eq!(
            std::fs::read(root.join("docs/plan.md")).unwrap(),
            b"plan v1"
        );
        assert!(
            std::fs::metadata(root.join("docs/plan.md"))
                .unwrap()
                .permissions()
                .readonly()
        );
        let calls = hub.calls.load(Ordering::SeqCst);
        assert_eq!(calls, 2);

        // The same list again does nothing.
        assert!(copy.sync(&cache, &hub, ws.path(), &first).await.is_empty());
        assert_eq!(hub.calls.load(Ordering::SeqCst), calls);

        // A new version, a removed file, and a file that the session added.
        std::fs::write(root.join("mine.txt"), b"from the session").unwrap();
        let second = [file_ref("docs/plan.md", b"plan v2")];
        assert!(copy.sync(&cache, &hub, ws.path(), &second).await.is_empty());
        assert_eq!(
            std::fs::read(root.join("docs/plan.md")).unwrap(),
            b"plan v2"
        );
        assert!(!root.join("notes.txt").exists());
        assert!(!root.join("mine.txt").exists());

        // A file that the session changed goes back to the hub's version.
        let plan = root.join("docs/plan.md");
        let mut perms = std::fs::metadata(&plan).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&plan, perms).unwrap();
        std::fs::write(&plan, b"edited by the session").unwrap();
        assert!(copy.sync(&cache, &hub, ws.path(), &second).await.is_empty());
        assert_eq!(std::fs::read(&plan).unwrap(), b"plan v2");

        // An empty list leaves an empty folder.
        assert!(copy.sync(&cache, &hub, ws.path(), &[]).await.is_empty());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_restarted_worker_keeps_a_copy_that_is_current() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let cache = BlobCache::new(home.path());
        let hub = FakeBlobs::default();
        hub.add(b"same");
        let list = [file_ref("a.txt", b"same")];
        assert!(
            ChannelCopy::default()
                .sync(&cache, &hub, ws.path(), &list)
                .await
                .is_empty()
        );
        // The cache is gone, so a copy would need the hub.
        std::fs::remove_dir_all(home.path().join(CACHE_DIR)).unwrap();
        let before = hub.calls.load(Ordering::SeqCst);
        assert!(
            ChannelCopy::default()
                .sync(&cache, &hub, ws.path(), &list)
                .await
                .is_empty()
        );
        assert_eq!(hub.calls.load(Ordering::SeqCst), before);
    }

    #[tokio::test]
    async fn unsafe_channel_paths_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let cache = BlobCache::new(home.path());
        let hub = FakeBlobs::default();
        hub.add(b"x");
        let bad = [
            file_ref("../escape.txt", b"x"),
            file_ref("/abs.txt", b"x"),
            file_ref("a/../../b.txt", b"x"),
            file_ref("C:evil.txt", b"x"),
            file_ref("a\\b.txt", b"x"),
        ];
        let failures = ChannelCopy::default()
            .sync(&cache, &hub, ws.path(), &bad)
            .await;
        assert_eq!(failures.len(), bad.len());
        assert!(
            failures
                .iter()
                .all(|f| matches!(f.error, FetchError::InvalidPath(_)))
        );
        assert!(!ws.path().join("escape.txt").exists());
        assert!(!ws.path().join(".channel/escape.txt").exists());
        assert_eq!(hub.calls.load(Ordering::SeqCst), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_in_the_channel_copy_is_removed_not_followed() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("keep.txt"), b"keep").unwrap();
        let cache = BlobCache::new(home.path());
        let hub = FakeBlobs::default();
        hub.add(b"doc");
        let root = ws.path().join(".channel/files");
        std::fs::create_dir_all(&root).unwrap();
        // A link to a folder outside, under a name that the list does not
        // have, and one under a folder name that the list needs.
        std::os::unix::fs::symlink(outside.path(), root.join("gone")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("docs")).unwrap();
        let list = [file_ref("docs/doc.md", b"doc")];
        assert!(
            ChannelCopy::default()
                .sync(&cache, &hub, ws.path(), &list)
                .await
                .is_empty()
        );
        assert_eq!(
            std::fs::read(outside.path().join("keep.txt")).unwrap(),
            b"keep"
        );
        assert!(!outside.path().join("doc.md").exists());
        assert!(
            std::fs::symlink_metadata(root.join("docs"))
                .unwrap()
                .is_dir()
        );
        assert!(std::fs::symlink_metadata(root.join("gone")).is_err());
    }
}
