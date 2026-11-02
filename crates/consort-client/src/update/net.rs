//! The network side of app updates and the background service that runs
//! it.
//!
//! The service starts a short time after the app starts, and again every
//! [`crate::update::CHECK_INTERVAL`]. A check asks GitHub for the latest
//! release. A newer release with a file for this platform downloads into the
//! staging folder, in a folder for its version. The file gets its final name
//! only after its SHA-256 matches the line in the release's `SHA256SUMS`, so
//! a wrong or cut file never counts as staged.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use consort_proto::error::HubVersions;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch};

use super::{
    Candidate, Platform, Release, UpdateEvent, UpdateSettings, UpdateState, UpdateView, Version,
    choose_update, parse_checksums,
};

/// The most bytes of a desktop file that the updater accepts: 1 GiB.
const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;
/// The most bytes of the checksum file and of the release answer.
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
/// How long one call to the API may take.
const API_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a download may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// What can go wrong. The text says what happened and what to do next.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    #[error("Consort could not reach the release server: {0}. It tries again later")]
    Network(String),
    #[error("The release server answered with {0}. Consort tries again later")]
    Status(u16),
    #[error("Consort could not read the release information. It tries again later")]
    BadRelease,
    #[error("The release has no checksum for {0}, so Consort did not use the file")]
    MissingChecksum(String),
    #[error(
        "The downloaded file {name} does not match its checksum, so Consort deleted it. It tries again later"
    )]
    ChecksumMismatch { name: String },
    #[error("The update file is larger than Consort accepts, so Consort did not download it")]
    TooLarge,
    #[error("Consort could not use the update folder {path}: {message}")]
    Storage { path: String, message: String },
    #[error("There is no update ready to install")]
    NotReady,
    #[error(
        "Version {version} cannot talk to hub {hub_version}, so Consort did not install it. Update the hub first, then restart"
    )]
    Held {
        version: Version,
        hub_version: String,
    },
    #[error("{0}")]
    Install(String),
}

fn storage(path: &Path, err: impl std::fmt::Display) -> UpdateError {
    UpdateError::Storage {
        path: path.display().to_string(),
        message: err.to_string(),
    }
}

/// How the updater runs.
#[derive(Clone, Debug)]
pub struct UpdateConfig {
    /// The GitHub API address. Tests point it at a local server.
    pub api_base: String,
    /// The repository, as `owner/name`.
    pub repo: String,
    /// The version of this app.
    pub current: Version,
    /// This platform, or `None` when it has no desktop build.
    pub platform: Option<Platform>,
    /// The staging folder. Each version has a folder in it.
    pub staging: PathBuf,
    /// The file for the settings. `None` keeps them in memory only.
    pub settings_path: Option<PathBuf>,
    /// The time between two checks.
    pub interval: Duration,
    /// The time from the start of the service to the first check.
    pub first_check_after: Duration,
    /// This copy of the app can replace itself.
    pub installable: bool,
}

impl UpdateConfig {
    /// The configuration of the desktop app. `current` is the version of the
    /// app, `env!("CARGO_PKG_VERSION")` in the app crate. Returns `None` when
    /// the version is not a release version or no folder exists for the
    /// staging files.
    pub fn desktop(current: &str) -> Option<UpdateConfig> {
        let current = Version::parse(current)?;
        let staging = directories::ProjectDirs::from("", "", "consort")?
            .cache_dir()
            .join("updates");
        let settings_path =
            crate::store::Store::default_location().map(|store| store.dir().join("updates.json"));
        // `scripts/dev.sh` runs a debug build from a bundle. A release must
        // not replace it.
        let installable = !cfg!(debug_assertions)
            && std::env::current_exe()
                .map(|exe| apply::Install::detect(&exe).is_some())
                .unwrap_or(false);
        Some(UpdateConfig {
            api_base: "https://api.github.com".to_owned(),
            repo: super::RELEASES_REPO.to_owned(),
            current,
            platform: Platform::current(),
            staging,
            settings_path,
            interval: super::CHECK_INTERVAL,
            first_check_after: Duration::from_secs(15),
            installable,
        })
    }
}

use super::apply;

/// Reads the settings. A missing or broken file gives the defaults.
pub fn load_settings(path: Option<&Path>) -> UpdateSettings {
    path.and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes the settings through a temporary file, so a crash never leaves
/// half a file.
pub fn save_settings(path: &Path, settings: &UpdateSettings) -> Result<(), UpdateError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| storage(parent, e))?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| storage(path, e))?;
    std::fs::write(&tmp, bytes).map_err(|e| storage(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| storage(path, e))
}

fn http_client(current: &Version) -> Result<reqwest::Client, UpdateError> {
    reqwest::Client::builder()
        .user_agent(format!("consort-desktop/{current}"))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| UpdateError::Network(e.to_string()))
}

fn network(err: reqwest::Error) -> UpdateError {
    // The text of a reqwest error can hold the URL. The URL is public.
    UpdateError::Network(err.without_url().to_string())
}

/// Reads a body up to `limit` bytes.
async fn read_limited(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, UpdateError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(network)?;
        if body.len() + chunk.len() > limit {
            return Err(UpdateError::BadRelease);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Asks for the latest release and chooses the update for this app.
pub async fn check_release(
    http: &reqwest::Client,
    cfg: &UpdateConfig,
) -> Result<Option<Candidate>, UpdateError> {
    let Some(platform) = cfg.platform else {
        return Ok(None);
    };
    let url = format!(
        "{}/repos/{}/releases/latest",
        cfg.api_base.trim_end_matches('/'),
        cfg.repo
    );
    let response = http
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(API_TIMEOUT)
        .send()
        .await
        .map_err(network)?;
    let status = response.status();
    // A repository with no release gives 404. That is not a failure.
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(UpdateError::Status(status.as_u16()));
    }
    let body = read_limited(response, MAX_TEXT_BYTES).await?;
    let release: Release = serde_json::from_slice(&body).map_err(|_| UpdateError::BadRelease)?;
    Ok(choose_update(&cfg.current, &release, platform))
}

/// The protocol versions of a release, from its `release.json`. A release
/// with no such file, or with a file that says something else, gives `None`:
/// the update then runs as it did before that file existed.
pub async fn fetch_release_protocols(
    http: &reqwest::Client,
    candidate: &Candidate,
) -> Option<Vec<u32>> {
    let asset = candidate.manifest.as_ref()?;
    let response = http
        .get(&asset.url)
        .timeout(API_TIMEOUT)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let body = read_limited(response, MAX_TEXT_BYTES).await.ok()?;
    let manifest: super::ReleaseManifest = serde_json::from_slice(&body).ok()?;
    (Version::parse(&manifest.version).as_ref() == Some(&candidate.version)
        && !manifest.protocol_versions.is_empty())
    .then_some(manifest.protocol_versions)
}

#[cfg(test)]
fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// The SHA-256 of a file, read in pieces.
fn file_hash(path: &Path) -> std::io::Result<String> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Downloads the file of a candidate into `<staging>/<version>/` and returns
/// its path. A file that is staged already, with the right hash, is not
/// downloaded again. Other version folders in the staging folder are removed.
pub async fn download(
    http: &reqwest::Client,
    cfg: &UpdateConfig,
    candidate: &Candidate,
) -> Result<PathBuf, UpdateError> {
    let sums_response = http
        .get(&candidate.checksums.url)
        .timeout(API_TIMEOUT)
        .send()
        .await
        .map_err(network)?;
    if !sums_response.status().is_success() {
        return Err(UpdateError::Status(sums_response.status().as_u16()));
    }
    let sums = parse_checksums(&String::from_utf8_lossy(
        &read_limited(sums_response, MAX_TEXT_BYTES).await?,
    ));
    let name = candidate.asset.name.clone();
    let expected = sums
        .get(&name)
        .cloned()
        .ok_or_else(|| UpdateError::MissingChecksum(name.clone()))?;
    if candidate.asset.size > MAX_DOWNLOAD_BYTES {
        return Err(UpdateError::TooLarge);
    }

    let dir = cfg.staging.join(candidate.version.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| storage(&dir, e))?;
    let target = dir.join(&name);
    if target.exists() {
        if file_hash(&target).ok().as_deref() == Some(expected.as_str()) {
            remove_other_versions(&cfg.staging, &dir);
            return Ok(target);
        }
        let _ = std::fs::remove_file(&target);
    }

    let response = http
        .get(&candidate.asset.url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(network)?;
    if !response.status().is_success() {
        return Err(UpdateError::Status(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_DOWNLOAD_BYTES)
    {
        return Err(UpdateError::TooLarge);
    }
    let part = dir.join(format!("{name}.part"));
    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| storage(&part, e))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut stream = response.bytes_stream();
    let streamed: Result<(), UpdateError> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(network)?;
            total += chunk.len() as u64;
            if total > MAX_DOWNLOAD_BYTES {
                return Err(UpdateError::TooLarge);
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|e| storage(&part, e))?;
        }
        file.flush().await.map_err(|e| storage(&part, e))
    }
    .await;
    drop(file);
    if let Err(err) = streamed {
        let _ = std::fs::remove_file(&part);
        return Err(err);
    }
    if hex(&hasher.finalize()) != expected {
        let _ = std::fs::remove_file(&part);
        return Err(UpdateError::ChecksumMismatch { name });
    }
    std::fs::rename(&part, &target).map_err(|e| storage(&target, e))?;
    remove_other_versions(&cfg.staging, &dir);
    Ok(target)
}

/// Removes the staged files of every other version.
fn remove_other_versions(staging: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(staging) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path() != keep {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// What a command to the service asks for.
enum Command {
    CheckNow,
    DownloadNow,
    SetAuto(bool),
}

/// A handle to the update service. Cheap to clone.
#[derive(Clone)]
pub struct Updates {
    view: watch::Receiver<UpdateView>,
    commands: Option<mpsc::UnboundedSender<Command>>,
    /// Keeps the sender of a fixed view alive.
    fixed: Option<Arc<watch::Sender<UpdateView>>>,
    /// Tells the view about the hub. A service in a long download still takes
    /// this at once.
    hub_tx: watch::Sender<UpdateView>,
    _runtime: Option<Arc<crate::RuntimeThread>>,
}

impl Updates {
    /// Starts the service. It checks after `first_check_after`, and then
    /// every `interval`.
    pub fn start(cfg: UpdateConfig) -> Result<Updates, crate::ClientError> {
        let settings = load_settings(cfg.settings_path.as_deref());
        let initial = UpdateView {
            state: UpdateState::Idle,
            auto: settings.auto,
            current: cfg.current.clone(),
            installable: cfg.installable,
            release_protocols: None,
            hub: None,
        };
        let (view_tx, view) = watch::channel(initial);
        let hub_tx = view_tx.clone();
        let (commands, rx) = mpsc::unbounded_channel();
        let runtime = crate::spawn_runtime("consort-updates", move |_stop| async move {
            run_service(cfg, settings, rx, view_tx).await;
        })?;
        Ok(Updates {
            view,
            commands: Some(commands),
            fixed: None,
            hub_tx,
            _runtime: Some(Arc::new(runtime)),
        })
    }

    /// A handle that never checks and shows `view` as it is. The mock mode
    /// and the screenshots use it. `set_auto` still changes the switch.
    pub fn fixed(view: UpdateView) -> Updates {
        let (tx, rx) = watch::channel(view);
        Updates {
            view: rx,
            commands: None,
            hub_tx: tx.clone(),
            fixed: Some(Arc::new(tx)),
            _runtime: None,
        }
    }

    /// The state of the updates now.
    pub fn view(&self) -> UpdateView {
        self.view.borrow().clone()
    }

    /// A receiver for changes of the view.
    pub fn subscribe(&self) -> watch::Receiver<UpdateView> {
        self.view.clone()
    }

    /// Says which hub this app talks to. An update that cannot talk to it
    /// then waits. A hub that the view knows already changes nothing.
    pub fn set_hub(&self, hub: Option<HubVersions>) {
        self.hub_tx.send_if_modified(|view| {
            if view.hub == hub {
                return false;
            }
            view.hub = hub;
            true
        });
    }

    /// Checks now, without waiting for the next time.
    pub fn check_now(&self) {
        self.send(Command::CheckNow);
    }

    /// Downloads the version that a check found, when the switch is off.
    pub fn download_now(&self) {
        self.send(Command::DownloadNow);
    }

    /// Sets the switch "Get updates on their own" and keeps it.
    pub fn set_auto(&self, auto: bool) {
        if let Some(fixed) = &self.fixed {
            fixed.send_modify(|view| view.auto = auto);
        }
        self.send(Command::SetAuto(auto));
    }

    fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }

    /// Replaces the installed app with the staged update and starts the new
    /// one after this process ends. The caller quits the app when this
    /// returns `Ok`.
    pub fn install_and_relaunch(&self) -> Result<(), UpdateError> {
        let view = self.view();
        if let Some(hold) = view.hold() {
            return Err(UpdateError::Held {
                version: hold.version,
                hub_version: hold.hub_version,
            });
        }
        let UpdateState::Ready { archive, .. } = view.state else {
            return Err(UpdateError::NotReady);
        };
        let exe = std::env::current_exe().map_err(|e| UpdateError::Install(e.to_string()))?;
        let install = apply::Install::detect(&exe).ok_or_else(|| {
            UpdateError::Install(
                "This copy of Consort runs from a build folder, so it cannot replace itself. Install a release build to get updates".to_owned(),
            )
        })?;
        let platform = Platform::current()
            .ok_or_else(|| UpdateError::Install("This system has no update file".to_owned()))?;
        apply::install_and_relaunch(platform, &install, &archive)
    }
}

async fn run_service(
    cfg: UpdateConfig,
    mut settings: UpdateSettings,
    mut commands: mpsc::UnboundedReceiver<Command>,
    view_tx: watch::Sender<UpdateView>,
) {
    let http = match http_client(&cfg.current) {
        Ok(http) => http,
        Err(err) => {
            publish(&view_tx, |view| {
                view.state = UpdateState::Failed {
                    message: err.to_string(),
                }
            });
            return;
        }
    };
    let mut service = Service {
        cfg,
        http,
        view_tx,
        state: UpdateState::Idle,
        pending: None,
    };
    let mut next = tokio::time::Instant::now() + service.cfg.first_check_after;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(next) => {
                service.check(settings.auto).await;
                next = tokio::time::Instant::now() + service.cfg.interval;
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                match command {
                    Command::CheckNow => {
                        service.check(settings.auto).await;
                        next = tokio::time::Instant::now() + service.cfg.interval;
                    }
                    Command::DownloadNow => service.download_pending().await,
                    Command::SetAuto(auto) => {
                        settings.auto = auto;
                        if let Some(path) = &service.cfg.settings_path
                            && let Err(err) = save_settings(path, &settings)
                        {
                            tracing::warn!(error = %err, "could not keep the update settings");
                        }
                        publish(&service.view_tx, |view| view.auto = auto);
                        // Turning the switch on starts the download of a
                        // version that waits.
                        if auto {
                            service.download_pending().await;
                        }
                    }
                }
            }
        }
    }
}

fn publish(tx: &watch::Sender<UpdateView>, change: impl FnOnce(&mut UpdateView)) {
    tx.send_modify(change);
}

struct Service {
    cfg: UpdateConfig,
    http: reqwest::Client,
    view_tx: watch::Sender<UpdateView>,
    state: UpdateState,
    /// The candidate that waits for the user, when the switch is off.
    pending: Option<Candidate>,
}

impl Service {
    fn apply(&mut self, event: UpdateEvent) {
        let state = std::mem::replace(&mut self.state, UpdateState::Idle).apply(event);
        self.state = state.clone();
        publish(&self.view_tx, |view| view.state = state);
    }

    /// One check, and the download when the switch is on.
    async fn check(&mut self, auto: bool) {
        self.apply(UpdateEvent::CheckStarted);
        match check_release(&self.http, &self.cfg).await {
            Ok(None) => {
                self.pending = None;
                self.apply(UpdateEvent::NoUpdate {
                    checked_at: crate::format::now_ms(),
                });
            }
            Ok(Some(candidate)) => {
                let protocols = fetch_release_protocols(&self.http, &candidate).await;
                publish(&self.view_tx, |view| view.release_protocols = protocols);
                self.apply(UpdateEvent::Found {
                    version: candidate.version.clone(),
                    auto,
                });
                self.pending = Some(candidate);
                if auto {
                    self.download_pending().await;
                }
            }
            Err(err) => {
                tracing::info!(error = %err, "the update check failed");
                self.apply(UpdateEvent::Failed {
                    message: err.to_string(),
                });
            }
        }
    }

    async fn download_pending(&mut self) {
        let Some(candidate) = self.pending.clone() else {
            return;
        };
        // A ready update of this version needs no second download.
        if self.state.ready_version() == Some(&candidate.version) {
            return;
        }
        self.apply(UpdateEvent::DownloadStarted {
            version: candidate.version.clone(),
        });
        match download(&self.http, &self.cfg, &candidate).await {
            Ok(archive) => {
                self.pending = None;
                self.apply(UpdateEvent::Downloaded {
                    version: candidate.version,
                    archive,
                });
            }
            Err(err) => {
                tracing::info!(error = %err, "the update download failed");
                self.apply(UpdateEvent::Failed {
                    message: err.to_string(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::get;

    use super::super::{Arch, Os};
    use super::*;

    const MAC: Platform = Platform {
        os: Os::MacOs,
        arch: Arch::Arm64,
    };
    const ASSET: &str = "Consort-macos-arm64.zip";

    /// What the fake release server says. Tests change it between calls.
    struct Fake {
        tag: String,
        file: Vec<u8>,
        /// The hash that SHA256SUMS holds for the file.
        listed_hash: String,
        release_status: u16,
        has_release: bool,
        file_requests: AtomicUsize,
        base: String,
    }

    type Shared = Arc<Mutex<Fake>>;

    async fn latest(State(fake): State<Shared>) -> impl IntoResponse {
        let f = fake.lock().unwrap();
        if f.release_status != 200 {
            return (
                StatusCode::from_u16(f.release_status).unwrap(),
                String::new(),
            );
        }
        if !f.has_release {
            return (StatusCode::NOT_FOUND, "{}".to_owned());
        }
        let body = serde_json::json!({
            "tag_name": f.tag, "draft": false, "prerelease": false,
            "assets": [
                {"name": ASSET, "size": f.file.len(), "browser_download_url": format!("{}/files/{ASSET}", f.base)},
                {"name": "SHA256SUMS", "size": 100, "browser_download_url": format!("{}/files/SHA256SUMS", f.base)},
            ]
        });
        (StatusCode::OK, body.to_string())
    }

    async fn sums(State(fake): State<Shared>) -> String {
        let f = fake.lock().unwrap();
        format!("{}  {ASSET}\n", f.listed_hash)
    }

    async fn file(State(fake): State<Shared>) -> Vec<u8> {
        let f = fake.lock().unwrap();
        f.file_requests.fetch_add(1, Ordering::SeqCst);
        f.file.clone()
    }

    async fn serve(file_bytes: &[u8], tag: &str) -> (Shared, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let fake = Arc::new(Mutex::new(Fake {
            tag: tag.to_owned(),
            file: file_bytes.to_vec(),
            listed_hash: sha256_hex(file_bytes),
            release_status: 200,
            has_release: true,
            file_requests: AtomicUsize::new(0),
            base: base.clone(),
        }));
        let app = Router::new()
            .route("/repos/acme/consort/releases/latest", get(latest))
            .route("/files/SHA256SUMS", get(sums))
            .route(&format!("/files/{ASSET}"), get(file))
            .with_state(fake.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (fake, base)
    }

    fn config(base: &str, dir: &Path, current: &str) -> UpdateConfig {
        UpdateConfig {
            api_base: base.to_owned(),
            repo: "acme/consort".to_owned(),
            current: Version::parse(current).unwrap(),
            platform: Some(MAC),
            staging: dir.join("staging"),
            settings_path: Some(dir.join("updates.json")),
            interval: Duration::from_secs(3600),
            first_check_after: Duration::ZERO,
            installable: true,
        }
    }

    async fn wait_for(
        updates: &Updates,
        what: &str,
        done: impl Fn(&UpdateState) -> bool,
    ) -> UpdateView {
        let mut rx = updates.subscribe();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let view = rx.borrow_and_update().clone();
                if done(&view.state) {
                    return view;
                }
                rx.changed().await.unwrap();
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}: {:?}", updates.view().state))
    }

    #[tokio::test]
    async fn a_newer_release_downloads_and_checks_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        let (_, base) = serve(b"the app bytes", "v0.2.0").await;
        let cfg = config(&base, dir.path(), "0.1.0");
        let staging = cfg.staging.clone();
        let updates = Updates::start(cfg).unwrap();
        let view = wait_for(&updates, "ready", |s| s.ready_version().is_some()).await;
        let UpdateState::Ready {
            ref version,
            ref archive,
        } = view.state
        else {
            unreachable!()
        };
        assert_eq!(version.to_string(), "0.2.0");
        assert_eq!(*archive, staging.join("0.2.0").join(ASSET));
        assert_eq!(std::fs::read(archive).unwrap(), b"the app bytes");
        assert!(!staging.join("0.2.0").join(format!("{ASSET}.part")).exists());
        assert_eq!(view.notice(), Some("Update ready. Restart to install."));
    }

    #[tokio::test]
    async fn a_file_with_the_wrong_hash_is_deleted_and_never_ready() {
        let dir = tempfile::tempdir().unwrap();
        let (fake, base) = serve(b"tampered bytes", "v0.2.0").await;
        fake.lock().unwrap().listed_hash = sha256_hex(b"the real bytes");
        let cfg = config(&base, dir.path(), "0.1.0");
        let staging = cfg.staging.clone();
        let updates = Updates::start(cfg).unwrap();
        let view = wait_for(&updates, "failed", |s| {
            matches!(s, UpdateState::Failed { .. })
        })
        .await;
        assert!(
            view.summary().contains("does not match its checksum"),
            "{}",
            view.summary()
        );
        assert!(view.notice().is_none());
        let left: Vec<_> = std::fs::read_dir(staging.join("0.2.0"))
            .unwrap()
            .flatten()
            .collect();
        assert!(left.is_empty(), "no file stays: {left:?}");
    }

    #[tokio::test]
    async fn the_same_version_is_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let (_, base) = serve(b"x", "v0.1.0").await;
        let updates = Updates::start(config(&base, dir.path(), "0.1.0")).unwrap();
        let view = wait_for(&updates, "up to date", |s| {
            matches!(s, UpdateState::UpToDate { .. })
        })
        .await;
        assert!(view.summary().contains("is the newest"));
    }

    #[tokio::test]
    async fn a_repository_without_a_release_is_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let (fake, base) = serve(b"x", "v0.2.0").await;
        fake.lock().unwrap().has_release = false;
        let updates = Updates::start(config(&base, dir.path(), "0.1.0")).unwrap();
        wait_for(&updates, "up to date", |s| {
            matches!(s, UpdateState::UpToDate { .. })
        })
        .await;
    }

    #[tokio::test]
    async fn a_server_error_is_a_failure_with_a_next_step() {
        let dir = tempfile::tempdir().unwrap();
        let (fake, base) = serve(b"x", "v0.2.0").await;
        fake.lock().unwrap().release_status = 503;
        let updates = Updates::start(config(&base, dir.path(), "0.1.0")).unwrap();
        let view = wait_for(&updates, "failed", |s| {
            matches!(s, UpdateState::Failed { .. })
        })
        .await;
        assert_eq!(
            view.summary(),
            "The release server answered with 503. Consort tries again later"
        );
    }

    #[tokio::test]
    async fn an_unreachable_server_is_a_failure_not_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing listens on port 9 of the local host.
        let updates = Updates::start(config("http://127.0.0.1:9", dir.path(), "0.1.0")).unwrap();
        let view = wait_for(&updates, "failed", |s| {
            matches!(s, UpdateState::Failed { .. })
        })
        .await;
        assert!(
            view.summary()
                .starts_with("Consort could not reach the release server")
        );
    }

    #[tokio::test]
    async fn with_the_switch_off_the_update_waits_until_the_user_asks() {
        let dir = tempfile::tempdir().unwrap();
        let (fake, base) = serve(b"bytes", "v0.2.0").await;
        let cfg = config(&base, dir.path(), "0.1.0");
        std::fs::write(cfg.settings_path.as_ref().unwrap(), r#"{"auto":false}"#).unwrap();
        let updates = Updates::start(cfg).unwrap();
        assert!(!updates.view().auto);
        let view = wait_for(&updates, "available", |s| {
            matches!(s, UpdateState::Available { .. })
        })
        .await;
        assert!(view.notice().is_none());
        assert_eq!(fake.lock().unwrap().file_requests.load(Ordering::SeqCst), 0);
        updates.download_now();
        wait_for(&updates, "ready", |s| s.ready_version().is_some()).await;
        assert_eq!(fake.lock().unwrap().file_requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn turning_the_switch_on_downloads_the_waiting_update_and_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (_, base) = serve(b"bytes", "v0.2.0").await;
        let cfg = config(&base, dir.path(), "0.1.0");
        let settings = cfg.settings_path.clone().unwrap();
        std::fs::write(&settings, r#"{"auto":false}"#).unwrap();
        let updates = Updates::start(cfg).unwrap();
        wait_for(&updates, "available", |s| {
            matches!(s, UpdateState::Available { .. })
        })
        .await;
        updates.set_auto(true);
        let view = wait_for(&updates, "ready", |s| s.ready_version().is_some()).await;
        assert!(view.auto);
        assert!(load_settings(Some(&settings)).auto);
        updates.set_auto(false);
        let mut rx = updates.subscribe();
        while rx.borrow().auto {
            rx.changed().await.unwrap();
        }
        assert!(
            !load_settings(Some(&settings)).auto,
            "the switch is on disk"
        );
    }

    #[tokio::test]
    async fn a_staged_file_is_not_downloaded_again() {
        let dir = tempfile::tempdir().unwrap();
        let (fake, base) = serve(b"bytes", "v0.2.0").await;
        let cfg = config(&base, dir.path(), "0.1.0");
        let http = http_client(&cfg.current).unwrap();
        let candidate = check_release(&http, &cfg).await.unwrap().unwrap();
        let first = download(&http, &cfg, &candidate).await.unwrap();
        let second = download(&http, &cfg, &candidate).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(fake.lock().unwrap().file_requests.load(Ordering::SeqCst), 1);
        // A cut file does not pass the hash, so it downloads again.
        std::fs::write(&first, b"cut").unwrap();
        download(&http, &cfg, &candidate).await.unwrap();
        assert_eq!(fake.lock().unwrap().file_requests.load(Ordering::SeqCst), 2);
        assert_eq!(std::fs::read(&first).unwrap(), b"bytes");
    }

    #[tokio::test]
    async fn a_new_download_removes_the_older_staged_versions() {
        let dir = tempfile::tempdir().unwrap();
        let (_, base) = serve(b"bytes", "v0.3.0").await;
        let cfg = config(&base, dir.path(), "0.1.0");
        let old = cfg.staging.join("0.2.0");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(ASSET), b"old").unwrap();
        let http = http_client(&cfg.current).unwrap();
        let candidate = check_release(&http, &cfg).await.unwrap().unwrap();
        download(&http, &cfg, &candidate).await.unwrap();
        assert!(!old.exists());
        assert!(cfg.staging.join("0.3.0").join(ASSET).exists());
    }

    #[tokio::test]
    async fn a_release_without_a_checksum_line_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (_, base) = serve(b"bytes", "v0.2.0").await;
        let cfg = config(&base, dir.path(), "0.1.0");
        let http = http_client(&cfg.current).unwrap();
        let mut candidate = check_release(&http, &cfg).await.unwrap().unwrap();
        candidate.asset.name = "Other.zip".to_owned();
        let err = download(&http, &cfg, &candidate).await.unwrap_err();
        assert_eq!(err, UpdateError::MissingChecksum("Other.zip".to_owned()));
    }

    #[test]
    fn the_settings_survive_a_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("updates.json");
        assert!(load_settings(Some(&path)).auto, "no file: on");
        std::fs::write(&path, "not json").unwrap();
        assert!(load_settings(Some(&path)).auto, "a broken file: on");
        save_settings(&path, &UpdateSettings { auto: false }).unwrap();
        assert!(!load_settings(Some(&path)).auto);
        assert!(load_settings(None).auto);
    }

    #[tokio::test]
    async fn a_fixed_view_never_checks_and_follows_the_switch() {
        let updates = Updates::fixed(UpdateView {
            state: UpdateState::Ready {
                version: Version::parse("0.2.0").unwrap(),
                archive: PathBuf::new(),
            },
            auto: true,
            current: Version::parse("0.1.0").unwrap(),
            installable: true,
            release_protocols: None,
            hub: None,
        });
        updates.check_now();
        updates.set_auto(false);
        assert!(!updates.view().auto);
        assert!(updates.view().notice().is_some());
    }

    #[test]
    fn install_needs_a_ready_update() {
        let updates = Updates::fixed(UpdateView {
            state: UpdateState::Idle,
            auto: true,
            current: Version::parse("0.1.0").unwrap(),
            installable: true,
            release_protocols: None,
            hub: None,
        });
        assert_eq!(updates.install_and_relaunch(), Err(UpdateError::NotReady));
    }

    #[test]
    fn an_update_on_hold_is_not_installed_and_a_new_hub_ends_the_hold() {
        let updates = Updates::fixed(UpdateView {
            state: UpdateState::Ready {
                version: Version::parse("0.8.0").unwrap(),
                archive: PathBuf::new(),
            },
            auto: true,
            current: Version::parse("0.7.0").unwrap(),
            installable: true,
            release_protocols: Some(vec![2]),
            hub: None,
        });
        let hub = |version: &str, protocols: Vec<u32>| HubVersions {
            hub_version: version.to_owned(),
            protocol_versions: protocols,
            install: None,
        };
        updates.set_hub(Some(hub("0.6.2", vec![1])));
        assert_eq!(
            updates.install_and_relaunch(),
            Err(UpdateError::Held {
                version: Version::parse("0.8.0").unwrap(),
                hub_version: "0.6.2".to_owned(),
            })
        );
        updates.set_hub(Some(hub("0.8.0", vec![2])));
        assert_eq!(updates.view().hold(), None);
    }

    async fn serve_manifest(body: &'static str) -> String {
        let app = Router::new().route("/release.json", get(move || async move { body }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}/release.json")
    }

    fn candidate(manifest: Option<String>) -> Candidate {
        let asset = |name: &str, url: String| super::super::Asset {
            name: name.to_owned(),
            url,
            size: 1,
        };
        Candidate {
            version: Version::parse("0.8.0").unwrap(),
            asset: asset(ASSET, String::new()),
            checksums: asset("SHA256SUMS", String::new()),
            manifest: manifest.map(|url| asset("release.json", url)),
        }
    }

    #[tokio::test]
    async fn the_updater_reads_the_protocol_versions_of_a_release() {
        let http = reqwest::Client::new();
        let url = serve_manifest(r#"{"version":"0.8.0","protocol_versions":[2,3]}"#).await;
        assert_eq!(
            fetch_release_protocols(&http, &candidate(Some(url))).await,
            Some(vec![2, 3])
        );
    }

    #[tokio::test]
    async fn a_manifest_that_is_missing_or_wrong_holds_nothing() {
        let http = reqwest::Client::new();
        assert_eq!(fetch_release_protocols(&http, &candidate(None)).await, None);
        for body in [
            "not json",
            // The file of another release.
            r#"{"version":"0.7.0","protocol_versions":[2]}"#,
            r#"{"version":"0.8.0","protocol_versions":[]}"#,
        ] {
            let url = serve_manifest(body).await;
            assert_eq!(
                fetch_release_protocols(&http, &candidate(Some(url))).await,
                None,
                "{body}"
            );
        }
    }
}
