//! Skill bundles on the worker: fetch from the hub, check, and cache by
//! content hash.
//!
//! A `session.start` names the skills of a session by content hash
//! (`SessionExtensions::skills`). For each hash the worker asks the hub for
//! the file list (`skill_bundle.get`), downloads each file with
//! `GET /v1/blobs/{sha256}`, and checks every hash. It keeps the files in
//! `<consort home>/cache/skills/<content hash>/`. The cache is content
//! addressed, so a running session keeps its version when the hub pins a new
//! one, and a resumed session finds its files again without the hub.
//!
//! The worker checks the files again each time it uses a cache folder, so a
//! file that changed on disk is fetched again and never reaches a session.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use consort_harness::AccountConfig;
use consort_harness::skills::{SessionSkills, SkillSource};
use consort_proto::model::Sha256Hash;
use consort_proto::model::{
    SKILL_MAX_FILE_BYTES, SkillBundle, SkillBundleRef, SkillFileInfo, validate_skill_files,
    validate_skill_name,
};
use consort_proto::worker_link::{SessionExtensions, SkillBundleGet};
use sha2::{Digest, Sha256};

use crate::WorkerError;

/// The cache folder, inside the Consort home.
pub const CACHE_DIR: &str = "cache/skills";
/// The file with the file list of a cached bundle.
const BUNDLE_FILE: &str = "bundle.json";
/// The folder with the files of a cached bundle.
const FILES_DIR: &str = "files";

/// Why a skill could not be made ready.
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    /// The hub or the network failed.
    #[error("could not fetch the skill `{name}`: {source}")]
    Fetch {
        /// The skill.
        name: String,
        /// The cause.
        #[source]
        source: WorkerError,
    },
    /// What the hub sent does not match its hash or the skill format.
    #[error("the skill `{name}` is not valid: {reason}")]
    Invalid {
        /// The skill.
        name: String,
        /// What is wrong.
        reason: String,
    },
    /// The cache folder could not be written.
    #[error("could not write the skill `{name}` to the cache: {source}")]
    Io {
        /// The skill.
        name: String,
        /// The cause.
        #[source]
        source: std::io::Error,
    },
}

/// Where the worker gets a bundle and its files. The worker uses the hub
/// link and the blob endpoint. Tests use a fake.
#[async_trait]
pub trait BundleSource: Send + Sync {
    /// The file list of the bundle with this content hash.
    async fn bundle(&self, content_hash: &Sha256Hash) -> Result<SkillBundle, WorkerError>;
    /// The bytes of one file. The source gives at most `max_bytes`.
    async fn blob(&self, sha256: &Sha256Hash, max_bytes: u64) -> Result<Vec<u8>, WorkerError>;
}

/// The cache of verified skill bundles.
#[derive(Debug, Clone)]
pub struct SkillCache {
    root: PathBuf,
}

fn sha256_of(bytes: &[u8]) -> Sha256Hash {
    Sha256Hash::from_bytes(Sha256::digest(bytes).into())
}

impl SkillCache {
    /// The cache in a Consort home.
    pub fn new(consort_home: &Path) -> Self {
        Self {
            root: consort_home.join(CACHE_DIR),
        }
    }

    fn bundle_dir(&self, hash: &Sha256Hash) -> PathBuf {
        self.root.join(hash.as_str())
    }

    /// Makes the skill ready: the cached copy when it passes the check, else
    /// a fresh download. Returns the folder with the skill's files.
    pub async fn ensure(
        &self,
        source: &dyn BundleSource,
        wanted: &SkillBundleRef,
    ) -> Result<SkillSource, SkillError> {
        let name = wanted.name.clone();
        let invalid = |reason: String| SkillError::Invalid {
            name: name.clone(),
            reason,
        };
        validate_skill_name(&wanted.name).map_err(|e| invalid(e.to_string()))?;
        let dir = self.bundle_dir(&wanted.content_hash);
        if verify_dir(&dir, &wanted.content_hash) {
            return Ok(SkillSource {
                name,
                dir: dir.join(FILES_DIR),
            });
        }
        // A copy that fails the check goes away before the new download.
        if dir.exists() {
            tracing::warn!(skill = %wanted.name, "a cached skill failed its check; the worker fetches it again");
            let _ = std::fs::remove_dir_all(&dir);
        }
        let bundle = source
            .bundle(&wanted.content_hash)
            .await
            .map_err(|source| SkillError::Fetch {
                name: name.clone(),
                source,
            })?;
        if bundle.content_hash != wanted.content_hash {
            return Err(invalid(
                "the hub sent files for another content hash".to_owned(),
            ));
        }
        if !bundle.is_consistent() {
            return Err(invalid(
                "the file list does not give the content hash".to_owned(),
            ));
        }
        validate_skill_files(&bundle.files).map_err(|e| invalid(e.to_string()))?;

        std::fs::create_dir_all(&self.root).map_err(|source| SkillError::Io {
            name: name.clone(),
            source,
        })?;
        let temp = self
            .root
            .join(format!(".tmp-{}", uuid::Uuid::now_v7().simple()));
        let result = self.download(source, &bundle, &temp).await;
        match result {
            Ok(()) => {}
            Err(err) => {
                let _ = std::fs::remove_dir_all(&temp);
                return Err(err);
            }
        }
        // Two sessions can fetch the same hash at the same time. The first
        // rename wins, and the other copy goes away.
        if let Err(err) = std::fs::rename(&temp, &dir) {
            let _ = std::fs::remove_dir_all(&temp);
            if !verify_dir(&dir, &wanted.content_hash) {
                return Err(SkillError::Io { name, source: err });
            }
        }
        Ok(SkillSource {
            name,
            dir: dir.join(FILES_DIR),
        })
    }

    /// Downloads every file of `bundle` into `temp`, and checks each one.
    async fn download(
        &self,
        source: &dyn BundleSource,
        bundle: &SkillBundle,
        temp: &Path,
    ) -> Result<(), SkillError> {
        let name = bundle.name.clone();
        let io = |source: std::io::Error| SkillError::Io {
            name: name.clone(),
            source,
        };
        let files_dir = temp.join(FILES_DIR);
        std::fs::create_dir_all(&files_dir).map_err(io)?;
        // One file after the other. A skill has few files, and a plain loop
        // keeps the future simple for the compiler (`Send` for `&dyn`).
        for file in &bundle.files {
            let (file, bytes) = fetch_file(source, &bundle.name, file).await?;
            if bytes.len() as u64 != file.size || sha256_of(&bytes) != file.sha256 {
                return Err(SkillError::Invalid {
                    name: bundle.name.clone(),
                    reason: format!("the file {} does not match its hash", file.path),
                });
            }
            write_file(&files_dir, file, &bytes).map_err(io)?;
        }
        let list = serde_json::to_vec(bundle).map_err(|e| SkillError::Invalid {
            name: bundle.name.clone(),
            reason: e.to_string(),
        })?;
        std::fs::write(temp.join(BUNDLE_FILE), list).map_err(io)?;
        Ok(())
    }
}

/// Downloads one file of a bundle.
async fn fetch_file<'a>(
    source: &'a dyn BundleSource,
    name: &'a str,
    file: &'a SkillFileInfo,
) -> Result<(&'a SkillFileInfo, Vec<u8>), SkillError> {
    let bytes = source
        .blob(&file.sha256, file.size.min(SKILL_MAX_FILE_BYTES))
        .await
        .map_err(|source| SkillError::Fetch {
            name: name.to_owned(),
            source,
        })?;
    Ok((file, bytes))
}

/// Writes one file below `root`. The path is relative and has no `..`
/// (`validate_skill_files` checked it), and `root` is new, so no link can
/// lead out of it.
fn write_file(root: &Path, file: &SkillFileInfo, bytes: &[u8]) -> std::io::Result<()> {
    let target = root.join(&file.path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if file.executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// Whether a cache folder holds the bundle with this content hash, and each
/// of its files still has its recorded size and hash.
fn verify_dir(dir: &Path, content_hash: &Sha256Hash) -> bool {
    let Ok(text) = std::fs::read(dir.join(BUNDLE_FILE)) else {
        return false;
    };
    let Ok(bundle) = serde_json::from_slice::<SkillBundle>(&text) else {
        return false;
    };
    if bundle.content_hash != *content_hash
        || !bundle.is_consistent()
        || validate_skill_files(&bundle.files).is_err()
    {
        return false;
    }
    let files = dir.join(FILES_DIR);
    bundle.files.iter().all(|file| {
        let path = files.join(&file.path);
        // A link would let a file outside the cache pass the check.
        let plain = path
            .symlink_metadata()
            .is_ok_and(|m| m.is_file() && m.len() == file.size);
        plain
            && std::fs::read(&path)
                .map(|bytes| sha256_of(&bytes) == file.sha256)
                .unwrap_or(false)
    })
}

/// The folder of a session's skills, inside the session's runtime folder.
pub const SESSION_SKILLS_DIR: &str = "skills";

/// What the worker delivered for a session.
#[derive(Debug, Default)]
pub struct Delivery {
    /// The skills written for the harness. `None` when the harness has no
    /// native skills or the session has none.
    pub skills: Option<SessionSkills>,
    /// Whether the harness loads the skills itself, so the prompt says
    /// "installed" and not "call read_skill".
    pub native: bool,
    /// The skills that could not be made ready, for a note to the user.
    pub failed: Vec<String>,
    /// The names of the skills that the session has, when the worker could
    /// not deliver all of them. `None` means all.
    delivered: Option<Vec<String>>,
}

impl Delivery {
    /// The context text for the system prompt (`SessionExtensions::
    /// prompt_blocks`). A skill that the worker could not deliver is not in
    /// it, so the assistant never reads about a skill that it does not have.
    pub fn prompt(&self, extensions: &SessionExtensions) -> String {
        let Some(delivered) = &self.delivered else {
            return extensions.prompt_blocks(self.native);
        };
        let mut ext = extensions.clone();
        ext.skill_context.retain(|s| delivered.contains(&s.name));
        ext.prompt_blocks(self.native)
    }
}

/// Makes the skills of a session ready: for a harness with native skills,
/// the worker gets each bundle (from the cache or the hub), checks it, and
/// writes the session folder `root`. A harness without native skills gets
/// the hub tools `list_skills` and `read_skill`, so nothing is written.
///
/// A skill that fails does not fail the session. The session starts
/// without it, and `Delivery::failed` names it.
pub async fn deliver(
    account: &AccountConfig,
    cache: &SkillCache,
    source: &dyn BundleSource,
    extensions: &SessionExtensions,
    root: &Path,
) -> Delivery {
    let native = consort_harness::skills::native_delivery(account.harness);
    if !native || extensions.skills.is_empty() {
        return Delivery {
            native,
            ..Delivery::default()
        };
    }
    let mut ready = Vec::new();
    let mut failed = Vec::new();
    for wanted in &extensions.skills {
        match cache.ensure(source, wanted).await {
            Ok(skill) => ready.push(skill),
            Err(err) => {
                tracing::warn!(skill = %wanted.name, error = %err, "could not make a skill ready for a session");
                failed.push(format!("{err}"));
            }
        }
    }
    let written = consort_harness::skills::write_session_skills(account, root, &ready);
    match written {
        Ok(skills) => {
            let delivered =
                (!failed.is_empty()).then(|| ready.iter().map(|s| s.name.clone()).collect());
            Delivery {
                skills,
                native,
                failed,
                delivered,
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "could not write the skill folder of a session");
            let mut failed = failed;
            failed.push(format!("could not write the skill folder: {err}"));
            Delivery {
                skills: None,
                native,
                failed,
                delivered: Some(Vec::new()),
            }
        }
    }
}

/// Removes the skill folder of every session under `config_dir/runtime`, and
/// the temporary folders of the cache. A session that ended while the worker
/// was down leaves them. A session that resumes writes its folder again.
pub fn sweep(config_dir: &Path, consort_home: &Path) {
    let runtime = config_dir.join(crate::config::RUNTIME_DIR);
    if let Ok(sessions) = std::fs::read_dir(&runtime) {
        for session in sessions.flatten() {
            let folder = session.path().join(SESSION_SKILLS_DIR);
            if folder.exists()
                && let Err(err) = std::fs::remove_dir_all(&folder)
            {
                tracing::warn!(folder = %folder.display(), error = %err, "could not remove the skills of an ended session");
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(consort_home.join(CACHE_DIR)) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(".tmp-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// The hub as a source of bundles: `skill_bundle.get` over the worker link,
/// and the blob endpoint over HTTP.
pub(crate) struct HubBundles {
    pub core: std::sync::Arc<crate::core::Core>,
}

#[async_trait]
impl BundleSource for HubBundles {
    async fn bundle(&self, content_hash: &Sha256Hash) -> Result<SkillBundle, WorkerError> {
        self.core
            .link
            .request(&SkillBundleGet {
                content_hash: content_hash.clone(),
            })
            .await
    }

    async fn blob(&self, sha256: &Sha256Hash, max_bytes: u64) -> Result<Vec<u8>, WorkerError> {
        crate::http::download_blob(
            &self.core.hub_url,
            self.core.worker_id,
            &self.core.identity,
            sha256,
            max_bytes,
        )
        .await
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use consort_proto::model::skill_content_hash;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A bundle source in memory, which counts its calls.
    #[derive(Default)]
    pub(crate) struct MemorySource {
        pub bundles: Mutex<HashMap<String, SkillBundle>>,
        pub blobs: Mutex<HashMap<String, Vec<u8>>>,
        pub bundle_calls: AtomicUsize,
        pub blob_calls: AtomicUsize,
    }

    impl MemorySource {
        /// Adds a skill from `(path, content, executable)` and returns its
        /// reference.
        pub(crate) fn add(&self, name: &str, files: &[(&str, &str, bool)]) -> SkillBundleRef {
            let infos: Vec<SkillFileInfo> = files
                .iter()
                .map(|(path, content, executable)| SkillFileInfo {
                    path: (*path).to_owned(),
                    sha256: sha256_of(content.as_bytes()),
                    size: content.len() as u64,
                    executable: *executable,
                })
                .collect();
            for (_, content, _) in files {
                self.blobs.lock().unwrap().insert(
                    sha256_of(content.as_bytes()).to_string(),
                    content.as_bytes().to_vec(),
                );
            }
            let content_hash = skill_content_hash(&infos);
            let skill_id = consort_proto::ids::SkillId::new();
            self.bundles.lock().unwrap().insert(
                content_hash.to_string(),
                SkillBundle {
                    skill_id,
                    name: name.to_owned(),
                    content_hash: content_hash.clone(),
                    files: infos,
                },
            );
            SkillBundleRef {
                skill_id,
                name: name.to_owned(),
                content_hash,
            }
        }
    }

    #[async_trait]
    impl BundleSource for MemorySource {
        async fn bundle(&self, content_hash: &Sha256Hash) -> Result<SkillBundle, WorkerError> {
            self.bundle_calls.fetch_add(1, Ordering::SeqCst);
            self.bundles
                .lock()
                .unwrap()
                .get(content_hash.as_str())
                .cloned()
                .ok_or_else(|| WorkerError::Hub {
                    code: consort_proto::error::ErrorCode::NotFound,
                    message: "no such bundle".into(),
                })
        }

        async fn blob(&self, sha256: &Sha256Hash, _max: u64) -> Result<Vec<u8>, WorkerError> {
            self.blob_calls.fetch_add(1, Ordering::SeqCst);
            self.blobs
                .lock()
                .unwrap()
                .get(sha256.as_str())
                .cloned()
                .ok_or_else(|| WorkerError::Hub {
                    code: consort_proto::error::ErrorCode::NotFound,
                    message: "no such blob".into(),
                })
        }
    }

    fn files() -> Vec<(&'static str, &'static str, bool)> {
        vec![
            (
                "SKILL.md",
                "---\nname: pdf-forms\ndescription: Forms.\n---\nUse it.\n",
                false,
            ),
            ("scripts/fill.sh", "#!/bin/sh\necho hi\n", true),
        ]
    }

    #[tokio::test]
    async fn fetches_checks_and_caches_a_bundle() {
        let home = tempfile::tempdir().unwrap();
        let source = MemorySource::default();
        let wanted = source.add("pdf-forms", &files());
        let cache = SkillCache::new(home.path());

        let ready = cache.ensure(&source, &wanted).await.unwrap();
        assert_eq!(ready.name, "pdf-forms");
        assert!(
            ready.dir.starts_with(
                home.path()
                    .join("cache/skills")
                    .join(wanted.content_hash.as_str())
            )
        );
        assert_eq!(
            std::fs::read_to_string(ready.dir.join("SKILL.md")).unwrap(),
            files()[0].1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(ready.dir.join("scripts/fill.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "the script stays executable");
            let doc = std::fs::metadata(ready.dir.join("SKILL.md")).unwrap();
            assert_eq!(doc.permissions().mode() & 0o111, 0);
        }
        assert_eq!(source.bundle_calls.load(Ordering::SeqCst), 1);
        assert_eq!(source.blob_calls.load(Ordering::SeqCst), 2);

        // The second use reads the cache and asks the hub for nothing.
        let again = cache.ensure(&source, &wanted).await.unwrap();
        assert_eq!(again.dir, ready.dir);
        assert_eq!(source.bundle_calls.load(Ordering::SeqCst), 1);
        assert_eq!(source.blob_calls.load(Ordering::SeqCst), 2);
        // No temporary folder stays behind.
        let left: Vec<_> = std::fs::read_dir(home.path().join("cache/skills"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, [wanted.content_hash.to_string()]);
    }

    #[tokio::test]
    async fn a_changed_cache_file_is_fetched_again() {
        let home = tempfile::tempdir().unwrap();
        let source = MemorySource::default();
        let wanted = source.add("pdf-forms", &files());
        let cache = SkillCache::new(home.path());
        let ready = cache.ensure(&source, &wanted).await.unwrap();

        std::fs::write(ready.dir.join("SKILL.md"), "tampered").unwrap();
        let again = cache.ensure(&source, &wanted).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(again.dir.join("SKILL.md")).unwrap(),
            files()[0].1
        );
        assert_eq!(source.bundle_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn refuses_a_blob_that_does_not_match_its_hash() {
        let home = tempfile::tempdir().unwrap();
        let source = MemorySource::default();
        let wanted = source.add("pdf-forms", &files());
        let hash = sha256_of(files()[0].1.as_bytes());
        source
            .blobs
            .lock()
            .unwrap()
            .insert(hash.to_string(), b"other text of another size".to_vec());
        let cache = SkillCache::new(home.path());
        let err = cache.ensure(&source, &wanted).await.unwrap_err();
        assert!(matches!(err, SkillError::Invalid { .. }), "{err}");
        // A failed download leaves nothing in the cache.
        let left = std::fs::read_dir(home.path().join("cache/skills"))
            .map(|d| d.count())
            .unwrap_or(0);
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn refuses_a_bundle_whose_list_does_not_give_its_hash() {
        let home = tempfile::tempdir().unwrap();
        let source = MemorySource::default();
        let wanted = source.add("pdf-forms", &files());
        // The hub lists a file that the hash does not cover.
        source
            .bundles
            .lock()
            .unwrap()
            .get_mut(wanted.content_hash.as_str())
            .unwrap()
            .files[1]
            .executable = false;
        let cache = SkillCache::new(home.path());
        let err = cache.ensure(&source, &wanted).await.unwrap_err();
        assert!(
            err.to_string().contains("does not give the content hash"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn refuses_a_bundle_for_another_hash_and_unsafe_paths() {
        let home = tempfile::tempdir().unwrap();
        let source = MemorySource::default();
        let first = source.add("pdf-forms", &files());
        let other = source.add("other", &[("SKILL.md", "x", false)]);
        // The hub answers the request for `first` with the files of `other`.
        let swapped = source
            .bundles
            .lock()
            .unwrap()
            .get(other.content_hash.as_str())
            .cloned()
            .unwrap();
        source
            .bundles
            .lock()
            .unwrap()
            .insert(first.content_hash.to_string(), swapped);
        let cache = SkillCache::new(home.path());
        let err = cache.ensure(&source, &first).await.unwrap_err();
        assert!(err.to_string().contains("another content hash"), "{err}");

        // A path that leaves the folder is refused, though the hash fits.
        let evil = source.add(
            "evil",
            &[("SKILL.md", "x", false), ("../escape", "y", false)],
        );
        let err = cache.ensure(&source, &evil).await.unwrap_err();
        assert!(matches!(err, SkillError::Invalid { .. }), "{err}");
        assert!(!home.path().join("cache/escape").exists());

        // A name that is not a skill name never becomes a folder name.
        let bad = SkillBundleRef {
            name: "../x".into(),
            ..first
        };
        assert!(matches!(
            cache.ensure(&source, &bad).await.unwrap_err(),
            SkillError::Invalid { .. }
        ));
    }

    #[tokio::test]
    async fn a_hub_error_is_a_fetch_error() {
        let home = tempfile::tempdir().unwrap();
        let source = MemorySource::default();
        let wanted = source.add("pdf-forms", &files());
        source.bundles.lock().unwrap().clear();
        let cache = SkillCache::new(home.path());
        let err = cache.ensure(&source, &wanted).await.unwrap_err();
        assert!(matches!(err, SkillError::Fetch { .. }), "{err}");
    }
}
