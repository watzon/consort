//! Gets a release of the `consort` program and puts it in place. The worker
//! ([`consort_worker`](../../consort_worker)) and `consort hub update` use it.
//!
//! The source is always the fixed repository
//! ([`consort_proto::release::RELEASES_REPO`]): a peer can name only a version.
//! A release file counts only when its SHA-256 matches the line in the
//! `SHA256SUMS` file of that release. The new program replaces the old one
//! with one rename, so a crash never leaves half a program.
//!
//! Two traits are the seams: [`ReleaseHost`] for the network and
//! [`ProgramSwap`] for the file replacement. Tests need neither a network nor
//! a program that they can replace.

use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use consort_proto::release::{
    CHECKSUMS_ASSET, Version, latest_release_url, parse_checksums, release_file_url,
};
use futures_util::StreamExt as _;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

/// The most bytes of a release file that this code accepts: 512 MiB.
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
/// The most bytes of the program in a release file: 1 GiB.
const MAX_PROGRAM_BYTES: u64 = 1024 * 1024 * 1024;
/// The most bytes of `SHA256SUMS` and of the answer about the latest release.
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
/// How long a small call may take.
const TEXT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a download may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// What can go wrong. The text says what happened and what to do next.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    /// The release server could not be reached.
    #[error("could not reach the release server: {0}. Check the network, then try again")]
    Network(String),
    /// The release server answered with an error.
    #[error(
        "the release server answered with {status} for {url}. Check the version, then try again"
    )]
    Status {
        /// The HTTP status.
        status: u16,
        /// The address.
        url: String,
    },
    /// The answer of the release server cannot be read.
    #[error("the release information cannot be read: {0}. Try again later")]
    BadRelease(String),
    /// This system has no release file.
    #[error("no release has a program for {os} on {arch}. Build Consort from source")]
    NoProgram {
        /// The operating system.
        os: String,
        /// The processor.
        arch: String,
    },
    /// `SHA256SUMS` has no line for the file.
    #[error("the release has no checksum for {0}, so the file was not used. Try another release")]
    MissingChecksum(String),
    /// The file does not match its line in `SHA256SUMS`.
    #[error("the file {0} does not match its checksum, so it was deleted. Try again later")]
    ChecksumMismatch(String),
    /// The file is larger than this code accepts.
    #[error("the file {0} is too large, so it was not used")]
    TooLarge(String),
    /// The release file holds no program.
    #[error("the file {0} holds no consort program. Try another release")]
    NoProgramInArchive(String),
    /// A file could not be read or written.
    #[error(
        "could not use {path}: {message}. Check that this user may write there, then try again"
    )]
    Storage {
        /// The path.
        path: String,
        /// The error.
        message: String,
    },
}

fn storage(path: &Path, err: impl std::fmt::Display) -> UpdateError {
    UpdateError::Storage {
        path: path.display().to_string(),
        message: err.to_string(),
    }
}

/// The network side: reads an address, or writes it to a file.
#[async_trait]
pub trait ReleaseHost: Send + Sync {
    /// The body of `url`, up to `limit` bytes.
    async fn fetch(&self, url: &str, limit: usize) -> Result<Vec<u8>, UpdateError>;
    /// Writes the body of `url` to `dest`, up to `limit` bytes.
    async fn download(&self, url: &str, dest: &Path, limit: u64) -> Result<(), UpdateError>;
}

/// Replaces a program file.
pub trait ProgramSwap: Send + Sync {
    /// Puts `new` in the place of `target`, in one step.
    fn swap(&self, new: &Path, target: &Path) -> Result<(), UpdateError>;
}

/// GitHub, over HTTPS.
#[derive(Debug, Clone)]
pub struct GithubHost {
    http: reqwest::Client,
}

impl GithubHost {
    /// A host with a user agent, which the GitHub API asks for.
    pub fn new(agent: &str) -> Result<Self, UpdateError> {
        let http = reqwest::Client::builder()
            .user_agent(agent)
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        Ok(Self { http })
    }
}

fn network(err: reqwest::Error) -> UpdateError {
    // The text of a reqwest error can hold the URL. The URL is public.
    UpdateError::Network(err.without_url().to_string())
}

#[async_trait]
impl ReleaseHost for GithubHost {
    async fn fetch(&self, url: &str, limit: usize) -> Result<Vec<u8>, UpdateError> {
        let response = self
            .http
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .timeout(TEXT_TIMEOUT)
            .send()
            .await
            .map_err(network)?;
        if !response.status().is_success() {
            return Err(UpdateError::Status {
                status: response.status().as_u16(),
                url: url.to_owned(),
            });
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(network)?;
            if body.len() + chunk.len() > limit {
                return Err(UpdateError::TooLarge(url.to_owned()));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    async fn download(&self, url: &str, dest: &Path, limit: u64) -> Result<(), UpdateError> {
        let response = self
            .http
            .get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(network)?;
        if !response.status().is_success() {
            return Err(UpdateError::Status {
                status: response.status().as_u16(),
                url: url.to_owned(),
            });
        }
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| storage(dest, e))?;
        let mut total = 0u64;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(network)?;
            total += chunk.len() as u64;
            if total > limit {
                return Err(UpdateError::TooLarge(url.to_owned()));
            }
            file.write_all(&chunk).await.map_err(|e| storage(dest, e))?;
        }
        file.flush().await.map_err(|e| storage(dest, e))
    }
}

/// True when this program must never replace itself: it runs from a cargo
/// `target` folder, or from an app bundle, which the app updates and signs.
pub fn is_build_or_bundle(exe: &Path) -> bool {
    exe.components().any(|part| match part {
        Component::Normal(name) => {
            name == "target" || Path::new(name).extension().is_some_and(|e| e == "app")
        }
        _ => false,
    })
}

/// The version of the latest release that is not a pre-release.
pub async fn latest_version(host: &dyn ReleaseHost) -> Result<Version, UpdateError> {
    #[derive(serde::Deserialize)]
    struct Latest {
        tag_name: String,
    }
    let body = host.fetch(&latest_release_url(), MAX_TEXT_BYTES).await?;
    let latest: Latest =
        serde_json::from_slice(&body).map_err(|e| UpdateError::BadRelease(e.to_string()))?;
    Version::parse(&latest.tag_name)
        .ok_or_else(|| UpdateError::BadRelease(format!("`{}` is not a version", latest.tag_name)))
}

/// The name of the program inside a release file.
fn program_name(asset: &str) -> &'static str {
    if asset.ends_with(".zip") {
        "consort.exe"
    } else {
        "consort"
    }
}

fn file_sha256(path: &Path) -> Result<String, UpdateError> {
    let mut file = std::fs::File::open(path).map_err(|e| storage(path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| storage(path, e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        }))
}

/// Copies at most [`MAX_PROGRAM_BYTES`] to a new file with the program mode.
fn write_program(
    mut from: impl std::io::Read,
    dest: &Path,
    asset: &str,
) -> Result<(), UpdateError> {
    let mut out = std::fs::File::create(dest).map_err(|e| storage(dest, e))?;
    let written = std::io::copy(&mut from.by_ref().take(MAX_PROGRAM_BYTES + 1), &mut out)
        .map_err(|e| storage(dest, e))?;
    if written > MAX_PROGRAM_BYTES {
        return Err(UpdateError::TooLarge(asset.to_owned()));
    }
    out.flush().map_err(|e| storage(dest, e))?;
    Ok(())
}

/// Takes the program out of a release file. Only the file with the right name
/// comes out, and it goes to a fixed path, so a path in the archive cannot
/// write anywhere else.
fn unpack_program(archive: &Path, asset: &str, dest: &Path) -> Result<(), UpdateError> {
    let wanted = program_name(asset);
    let file = std::fs::File::open(archive).map_err(|e| storage(archive, e))?;
    let bad = |e: &dyn std::fmt::Display| UpdateError::BadRelease(format!("{asset}: {e}"));
    if asset.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(&e))?;
        for index in 0..zip.len() {
            let entry = zip.by_index(index).map_err(|e| bad(&e))?;
            let name = entry.name().map_err(|e| bad(&e))?.into_owned();
            let name_matches =
                !entry.is_dir() && Path::new(&name).file_name().is_some_and(|n| n == wanted);
            if name_matches {
                return write_program(entry, dest, asset);
            }
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        for entry in tar.entries().map_err(|e| bad(&e))? {
            let entry = entry.map_err(|e| bad(&e))?;
            let path = entry.path().map_err(|e| bad(&e))?.into_owned();
            if entry.header().entry_type().is_file()
                && path.file_name().is_some_and(|n| n == wanted)
            {
                return write_program(entry, dest, asset);
            }
        }
    }
    Err(UpdateError::NoProgramInArchive(asset.to_owned()))
}

/// Downloads the release file `asset` of `version` into `work`, checks it
/// against `SHA256SUMS`, and returns the path of the program in it. A file
/// that fails the check is deleted, and nothing else changes.
pub async fn fetch_program(
    host: &dyn ReleaseHost,
    version: &Version,
    asset: &str,
    work: &Path,
) -> Result<PathBuf, UpdateError> {
    std::fs::create_dir_all(work).map_err(|e| storage(work, e))?;
    let sums = host
        .fetch(&release_file_url(version, CHECKSUMS_ASSET), MAX_TEXT_BYTES)
        .await?;
    let sums = parse_checksums(&String::from_utf8_lossy(&sums));
    let expected = sums
        .get(asset)
        .ok_or_else(|| UpdateError::MissingChecksum(asset.to_owned()))?;
    let archive = work.join(format!("{asset}.part"));
    let downloaded = host
        .download(
            &release_file_url(version, asset),
            &archive,
            MAX_ARCHIVE_BYTES,
        )
        .await;
    if let Err(err) = downloaded {
        let _ = std::fs::remove_file(&archive);
        return Err(err);
    }
    if file_sha256(&archive)? != *expected {
        let _ = std::fs::remove_file(&archive);
        return Err(UpdateError::ChecksumMismatch(asset.to_owned()));
    }
    let program = work.join(program_name(asset));
    let unpacked = {
        let archive = archive.clone();
        let asset = asset.to_owned();
        let program = program.clone();
        tokio::task::spawn_blocking(move || unpack_program(&archive, &asset, &program))
            .await
            .map_err(|e| UpdateError::BadRelease(e.to_string()))?
    };
    let _ = std::fs::remove_file(&archive);
    unpacked?;
    Ok(program)
}

/// Replaces a program on this disk: the new file first goes next to the old
/// one, so the swap is a rename on one volume.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenameSwap;

impl ProgramSwap for RenameSwap {
    fn swap(&self, new: &Path, target: &Path) -> Result<(), UpdateError> {
        let folder = target
            .parent()
            .ok_or_else(|| storage(target, "the program has no folder"))?;
        let name = target
            .file_name()
            .ok_or_else(|| storage(target, "the program has no name"))?
            .to_string_lossy();
        let staged = folder.join(format!(".{name}.update-{}", std::process::id()));
        let result = stage_and_rename(new, &staged, target);
        if result.is_err() {
            let _ = std::fs::remove_file(&staged);
        }
        result
    }
}

fn stage_and_rename(new: &Path, staged: &Path, target: &Path) -> Result<(), UpdateError> {
    std::fs::copy(new, staged).map_err(|e| storage(staged, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| storage(staged, e))?;
    }
    std::fs::File::open(staged)
        .and_then(|f| f.sync_all())
        .map_err(|e| storage(staged, e))?;
    #[cfg(windows)]
    {
        // Windows does not let a rename replace a program that runs, but the
        // program can move aside. `remove_old_program` deletes it later.
        let aside = old_program_path(target);
        let _ = std::fs::remove_file(&aside);
        std::fs::rename(target, &aside).map_err(|e| storage(target, e))?;
        if let Err(err) = std::fs::rename(staged, target) {
            let _ = std::fs::rename(&aside, target);
            return Err(storage(target, err));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(staged, target).map_err(|e| storage(target, e))
    }
}

/// Where a program that runs moves to on Windows.
pub fn old_program_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(".old");
    PathBuf::from(name)
}

/// Deletes the program that an earlier update moved aside.
pub fn remove_old_program(target: &Path) {
    let _ = std::fs::remove_file(old_program_path(target));
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use consort_proto::release::RELEASES_REPO;

    use super::*;

    /// Files by address, with no network. It records what was asked.
    #[derive(Default)]
    struct FakeHost {
        files: HashMap<String, Vec<u8>>,
        asked: Mutex<Vec<String>>,
    }

    impl FakeHost {
        fn with_release(version: &Version, asset: &str, bytes: &[u8], listed: &str) -> Self {
            let mut host = Self::default();
            host.files.insert(
                release_file_url(version, CHECKSUMS_ASSET),
                format!("{listed}  {asset}\n").into_bytes(),
            );
            host.files
                .insert(release_file_url(version, asset), bytes.to_vec());
            host
        }
    }

    #[async_trait]
    impl ReleaseHost for FakeHost {
        async fn fetch(&self, url: &str, _limit: usize) -> Result<Vec<u8>, UpdateError> {
            self.asked.lock().unwrap().push(url.to_owned());
            self.files.get(url).cloned().ok_or(UpdateError::Status {
                status: 404,
                url: url.to_owned(),
            })
        }
        async fn download(&self, url: &str, dest: &Path, _limit: u64) -> Result<(), UpdateError> {
            let bytes = self.fetch(url, 0).await?;
            std::fs::write(dest, bytes).map_err(|e| storage(dest, e))
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        {
            let mut tar = tar::Builder::new(&mut gz);
            for (name, bytes) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(&mut header, name, *bytes).unwrap();
            }
            tar.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    const ASSET: &str = "consort-linux-x86_64.tar.gz";

    #[tokio::test]
    async fn a_good_release_gives_the_program_and_comes_from_the_fixed_repository() {
        let dir = tempfile::tempdir().unwrap();
        let version = v("0.8.0");
        let bytes = tarball(&[("consort", b"new program")]);
        let host = FakeHost::with_release(&version, ASSET, &bytes, &sha256_hex(&bytes));
        let program = fetch_program(&host, &version, ASSET, dir.path())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&program).unwrap(), b"new program");
        assert!(
            !dir.path().join(format!("{ASSET}.part")).exists(),
            "the archive goes away"
        );
        for url in host.asked.lock().unwrap().iter() {
            assert!(
                url.starts_with(&format!(
                    "https://github.com/{RELEASES_REPO}/releases/download/"
                )),
                "{url}"
            );
        }
    }

    #[tokio::test]
    async fn a_wrong_checksum_deletes_the_file_and_gives_no_program() {
        let dir = tempfile::tempdir().unwrap();
        let version = v("0.8.0");
        let bytes = tarball(&[("consort", b"tampered")]);
        let host = FakeHost::with_release(&version, ASSET, &bytes, &sha256_hex(b"the real file"));
        let err = fetch_program(&host, &version, ASSET, dir.path())
            .await
            .unwrap_err();
        assert_eq!(err, UpdateError::ChecksumMismatch(ASSET.to_owned()));
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[tokio::test]
    async fn a_release_with_no_checksum_line_is_refused_before_the_download() {
        let dir = tempfile::tempdir().unwrap();
        let version = v("0.8.0");
        let bytes = tarball(&[("consort", b"x")]);
        let host = FakeHost::with_release(&version, "other.tar.gz", &bytes, &sha256_hex(&bytes));
        let err = fetch_program(&host, &version, ASSET, dir.path())
            .await
            .unwrap_err();
        assert_eq!(err, UpdateError::MissingChecksum(ASSET.to_owned()));
        assert_eq!(host.asked.lock().unwrap().len(), 1, "only SHA256SUMS");
    }

    #[tokio::test]
    async fn an_archive_with_no_program_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let version = v("0.8.0");
        let bytes = tarball(&[("README", b"text")]);
        let host = FakeHost::with_release(&version, ASSET, &bytes, &sha256_hex(&bytes));
        let err = fetch_program(&host, &version, ASSET, dir.path())
            .await
            .unwrap_err();
        assert_eq!(err, UpdateError::NoProgramInArchive(ASSET.to_owned()));
    }

    #[tokio::test]
    async fn a_path_in_the_archive_cannot_write_outside_the_work_folder() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let version = v("0.8.0");
        let bytes = tarball(&[("nested/folder/consort", b"from a folder")]);
        let host = FakeHost::with_release(&version, ASSET, &bytes, &sha256_hex(&bytes));
        let program = fetch_program(&host, &version, ASSET, &work).await.unwrap();
        assert_eq!(program, work.join("consort"));
        assert_eq!(std::fs::read(program).unwrap(), b"from a folder");
    }

    #[tokio::test]
    async fn a_zip_release_gives_the_windows_program() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "consort-windows-x86_64.zip";
        let version = v("0.8.0");
        let mut bytes = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            zip.start_file("consort.exe", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"windows program").unwrap();
            zip.finish().unwrap();
        }
        let host = FakeHost::with_release(&version, asset, &bytes, &sha256_hex(&bytes));
        let program = fetch_program(&host, &version, asset, dir.path())
            .await
            .unwrap();
        assert_eq!(std::fs::read(program).unwrap(), b"windows program");
    }

    #[tokio::test]
    async fn the_latest_version_comes_from_the_github_answer() {
        let mut host = FakeHost::default();
        host.files.insert(
            latest_release_url(),
            br#"{"tag_name":"v0.9.1","assets":[]}"#.to_vec(),
        );
        assert_eq!(latest_version(&host).await.unwrap(), v("0.9.1"));
        host.files
            .insert(latest_release_url(), br#"{"tag_name":"nightly"}"#.to_vec());
        assert!(matches!(
            latest_version(&host).await,
            Err(UpdateError::BadRelease(_))
        ));
    }

    #[test]
    fn the_swap_replaces_the_file_and_leaves_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("consort");
        let new = dir.path().join("new");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        RenameSwap.swap(&new, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        remove_old_program(&target);
        let mut names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["consort", "new"]);
    }

    #[test]
    fn a_failed_swap_keeps_the_old_program() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("consort");
        std::fs::write(&target, b"old").unwrap();
        let err = RenameSwap
            .swap(&dir.path().join("no such file"), &target)
            .unwrap_err();
        assert!(matches!(err, UpdateError::Storage { .. }));
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_build_or_a_bundle_never_replaces_itself() {
        for exe in [
            "/Users/me/consort/target/debug/consort",
            "/repo/target/release/consort",
            "/Applications/Consort.app/Contents/MacOS/consort",
        ] {
            assert!(is_build_or_bundle(Path::new(exe)), "{exe}");
        }
        for exe in ["/usr/local/bin/consort", "/home/me/.local/bin/consort"] {
            assert!(!is_build_or_bundle(Path::new(exe)), "{exe}");
        }
    }
}
