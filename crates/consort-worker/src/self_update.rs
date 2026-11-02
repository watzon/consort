//! A worker follows the version of its hub. The hub names a version in the
//! `worker.update` command or in a `protocol_mismatch` error. The hub never
//! sends a program or an address.
//!
//! The worker downloads that release from the fixed repository, checks its
//! SHA-256, waits until no work runs, swaps its program in one step, and asks
//! the program around it to restart. It never moves to an older version. A
//! program in a cargo `target` folder or in an app bundle never replaces
//! itself.
//!
//! The network and the file replacement are the seams in `consort_link::update`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use consort_link::update::{
    GithubHost, ProgramSwap, ReleaseHost, RenameSwap, fetch_program, is_build_or_bundle,
    remove_old_program,
};
use consort_proto::model::WorkerUpdateStatus;
use consort_proto::release::{Version, program_asset};
use tokio::sync::watch;

/// How often the worker looks whether its work has ended.
const WORK_POLL: Duration = Duration::from_secs(5);
/// How long a failed update waits before the next try.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

/// What a worker needs to replace itself. A worker without this cannot
/// update, and says so.
#[derive(Clone)]
pub struct SelfUpdate {
    /// The program to replace.
    pub exe: PathBuf,
    /// The release file for this system, such as `consort-linux-x86_64.tar.gz`.
    pub asset: String,
    /// The version of this program.
    pub current: Version,
    /// Where the release comes from.
    pub host: Arc<dyn ReleaseHost>,
    /// How the program is replaced.
    pub swap: Arc<dyn ProgramSwap>,
    /// How often the worker looks whether its work has ended.
    pub work_poll: Duration,
    /// How long a failed update waits before the next try.
    pub retry_after: Duration,
}

impl std::fmt::Debug for SelfUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelfUpdate")
            .field("exe", &self.exe)
            .field("asset", &self.asset)
            .field("current", &self.current)
            .finish_non_exhaustive()
    }
}

impl SelfUpdate {
    /// The update of this running program. `None` for a development build,
    /// for a program in a build folder or an app bundle, and for a system
    /// with no release file. Take this at the start: on Linux, the path of
    /// the program changes after it is replaced.
    pub fn for_this_program() -> Option<Self> {
        if cfg!(debug_assertions) {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        if is_build_or_bundle(&exe) {
            return None;
        }
        remove_old_program(&exe);
        let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
        let host = GithubHost::new(&format!("consort-worker/{current}")).ok()?;
        Some(Self {
            exe,
            asset: program_asset(std::env::consts::OS, std::env::consts::ARCH)?,
            current,
            host: Arc::new(host),
            swap: Arc::new(RenameSwap),
            work_poll: WORK_POLL,
            retry_after: RETRY_AFTER,
        })
    }
}

/// What the worker tells the rest of itself during an update.
pub(crate) trait UpdateEvents: Send + Sync + 'static {
    /// True while a process runs or a request of the hub is being answered.
    fn busy(&self) -> bool;
    /// The status changed. The worker sends a new inventory.
    fn changed(&self);
}

struct State {
    status: Option<WorkerUpdateStatus>,
    /// The version that the current run follows.
    target: Option<Version>,
    /// A newer run ends an older one with this number.
    run: u64,
    /// When the update of a version failed.
    failed: Option<(Version, Instant)>,
}

/// The update of one worker.
pub(crate) struct Updater {
    config: Option<SelfUpdate>,
    /// The folder for the download. It holds nothing else.
    work: PathBuf,
    state: Mutex<State>,
    restart: watch::Sender<bool>,
}

impl Updater {
    pub(crate) fn new(config: Option<SelfUpdate>, work: PathBuf) -> Self {
        Self {
            config,
            work,
            state: Mutex::new(State {
                status: None,
                target: None,
                run: 0,
                failed: None,
            }),
            restart: watch::channel(false).0,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Where the update is now.
    pub(crate) fn status(&self) -> Option<WorkerUpdateStatus> {
        self.state().status.clone()
    }

    /// Turns true when the worker has its new program and may restart.
    pub(crate) fn restart_requested(&self) -> watch::Receiver<bool> {
        self.restart.subscribe()
    }

    fn set_status(&self, status: Option<WorkerUpdateStatus>, events: &dyn UpdateEvents) {
        self.state().status = status;
        events.changed();
    }

    /// Follows the version that the hub names, and returns where the update
    /// is. A name that is not a version, or a version that is not newer than
    /// this program, changes nothing.
    pub(crate) fn follow(
        self: &Arc<Self>,
        named: &str,
        events: Arc<dyn UpdateEvents>,
    ) -> Option<WorkerUpdateStatus> {
        let Some(version) = Version::parse(named) else {
            tracing::warn!(
                version = named,
                "the hub named something that is not a version"
            );
            return self.status();
        };
        let Some(config) = self.config.clone() else {
            // The version of a build that cannot replace itself may be
            // anything, so the hub decided that it is older.
            let status = WorkerUpdateStatus::NotAvailable {
                version: version.to_string(),
            };
            if self.status().as_ref() != Some(&status) {
                self.set_status(Some(status), events.as_ref());
            }
            return self.status();
        };
        if version <= config.current {
            tracing::info!(%version, current = %config.current, "this worker does not move to an older version");
            return self.status();
        }
        let run = {
            let mut state = self.state();
            if state.target.as_ref().is_some_and(|t| *t >= version) {
                return state.status.clone();
            }
            let failed_lately = state.failed.as_ref().is_some_and(|(failed, at)| {
                *failed == version && at.elapsed() < config.retry_after
            });
            if failed_lately {
                return state.status.clone();
            }
            state.run += 1;
            state.target = Some(version.clone());
            state.run
        };
        let this = self.clone();
        tokio::spawn(async move {
            this.run(run, config, version, events).await;
        });
        self.status()
    }

    fn is_current(&self, run: u64) -> bool {
        self.state().run == run
    }

    async fn run(
        &self,
        run: u64,
        config: SelfUpdate,
        version: Version,
        events: Arc<dyn UpdateEvents>,
    ) {
        tracing::info!(%version, "this worker follows the hub to a new version");
        let name = version.to_string();
        self.set_status(
            Some(WorkerUpdateStatus::Downloading {
                version: name.clone(),
            }),
            events.as_ref(),
        );
        let work = self.work.join(&name);
        let program =
            match fetch_program(config.host.as_ref(), &version, &config.asset, &work).await {
                Ok(program) => program,
                Err(err) => {
                    self.fail(run, version, err.to_string(), events.as_ref());
                    return;
                }
            };
        // The old program stays in place until the work ends. So a worker
        // that stops for another reason still has a program that matches
        // what it ran before.
        let mut told_waiting = false;
        while events.busy() {
            if !self.is_current(run) {
                return;
            }
            if !told_waiting {
                told_waiting = true;
                self.set_status(
                    Some(WorkerUpdateStatus::WaitingForWork {
                        version: name.clone(),
                    }),
                    events.as_ref(),
                );
            }
            tokio::time::sleep(config.work_poll).await;
        }
        if !self.is_current(run) {
            return;
        }
        let swapped = {
            let swap = config.swap.clone();
            let exe = config.exe.clone();
            let program = program.clone();
            tokio::task::spawn_blocking(move || swap.swap(&program, &exe)).await
        };
        match swapped {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                self.fail(run, version, err.to_string(), events.as_ref());
                return;
            }
            Err(err) => {
                self.fail(run, version, err.to_string(), events.as_ref());
                return;
            }
        }
        let _ = std::fs::remove_dir_all(&self.work);
        self.set_status(
            Some(WorkerUpdateStatus::Restarting { version: name }),
            events.as_ref(),
        );
        tracing::info!(%version, "the worker has its new program and restarts");
        self.restart.send_replace(true);
    }

    fn fail(&self, run: u64, version: Version, message: String, events: &dyn UpdateEvents) {
        tracing::warn!(%version, error = %message, "the update of this worker failed");
        let _ = std::fs::remove_dir_all(&self.work);
        {
            let mut state = self.state();
            if state.run != run {
                return;
            }
            state.target = None;
            state.failed = Some((version.clone(), Instant::now()));
            state.status = Some(WorkerUpdateStatus::Failed {
                version: version.to_string(),
                message,
            });
        }
        events.changed();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use async_trait::async_trait;
    use consort_link::update::UpdateError;
    use consort_proto::release::{CHECKSUMS_ASSET, release_file_url};
    use sha2::{Digest as _, Sha256};

    use super::*;

    const ASSET: &str = "consort-linux-x86_64.tar.gz";

    struct FakeHost {
        files: Mutex<HashMap<String, Vec<u8>>>,
        downloads: AtomicUsize,
    }

    #[async_trait]
    impl ReleaseHost for FakeHost {
        async fn fetch(&self, url: &str, _limit: usize) -> Result<Vec<u8>, UpdateError> {
            self.files
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or(UpdateError::Status {
                    status: 404,
                    url: url.to_owned(),
                })
        }
        async fn download(&self, url: &str, dest: &Path, limit: u64) -> Result<(), UpdateError> {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            let bytes = self.fetch(url, limit as usize).await?;
            std::fs::write(dest, bytes).map_err(|e| UpdateError::Storage {
                path: dest.display().to_string(),
                message: e.to_string(),
            })
        }
    }

    #[derive(Default)]
    struct FakeSwap {
        swapped: Mutex<Option<(Vec<u8>, PathBuf)>>,
        fail: AtomicBool,
    }

    impl ProgramSwap for FakeSwap {
        fn swap(&self, new: &Path, target: &Path) -> Result<(), UpdateError> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(UpdateError::Storage {
                    path: target.display().to_string(),
                    message: "read only".into(),
                });
            }
            *self.swapped.lock().unwrap() =
                Some((std::fs::read(new).unwrap(), target.to_path_buf()));
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeEvents {
        busy: AtomicBool,
        changes: AtomicUsize,
    }

    impl UpdateEvents for FakeEvents {
        fn busy(&self) -> bool {
            self.busy.load(Ordering::SeqCst)
        }
        fn changed(&self) {
            self.changes.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn tarball(bytes: &[u8]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        {
            let mut tar = tar::Builder::new(&mut gz);
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, "consort", bytes).unwrap();
            tar.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    struct Rig {
        updater: Arc<Updater>,
        host: Arc<FakeHost>,
        swap: Arc<FakeSwap>,
        events: Arc<FakeEvents>,
        _dir: tempfile::TempDir,
    }

    /// A worker at 0.7.0. `version` is on the fake host, and `listed` is the
    /// hash that its `SHA256SUMS` holds.
    fn rig(version: &str, listed: Option<&str>) -> Rig {
        let v = Version::parse(version).unwrap();
        let bytes = tarball(b"new program");
        let hash = listed.map_or_else(|| sha(&bytes), str::to_owned);
        let mut files = HashMap::new();
        files.insert(
            release_file_url(&v, CHECKSUMS_ASSET),
            format!("{hash}  {ASSET}\n").into_bytes(),
        );
        files.insert(release_file_url(&v, ASSET), bytes);
        let host = Arc::new(FakeHost {
            files: Mutex::new(files),
            downloads: AtomicUsize::new(0),
        });
        let swap = Arc::new(FakeSwap::default());
        let dir = tempfile::tempdir().unwrap();
        let config = SelfUpdate {
            exe: PathBuf::from("/usr/local/bin/consort"),
            asset: ASSET.to_owned(),
            current: Version::parse("0.7.0").unwrap(),
            host: host.clone(),
            swap: swap.clone(),
            work_poll: Duration::from_millis(10),
            retry_after: Duration::from_secs(3600),
        };
        let updater = Arc::new(Updater::new(Some(config), dir.path().join("update")));
        Rig {
            updater,
            host,
            swap,
            events: Arc::new(FakeEvents::default()),
            _dir: dir,
        }
    }

    async fn wait_until(what: &str, done: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !done() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    fn restarting(updater: &Updater) -> bool {
        matches!(
            updater.status(),
            Some(WorkerUpdateStatus::Restarting { .. })
        )
    }

    #[tokio::test]
    async fn a_named_version_downloads_swaps_and_asks_for_a_restart() {
        let r = rig("0.8.0", None);
        r.updater.follow("0.8.0", r.events.clone());
        wait_until("the restart", || restarting(&r.updater)).await;
        let (bytes, target) = r.swap.swapped.lock().unwrap().clone().unwrap();
        assert_eq!(bytes, b"new program");
        assert_eq!(target, PathBuf::from("/usr/local/bin/consort"));
        assert!(*r.updater.restart_requested().borrow());
        assert!(r.events.changes.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn it_waits_for_the_work_to_end_before_it_swaps_and_restarts() {
        let r = rig("0.8.0", None);
        r.events.busy.store(true, Ordering::SeqCst);
        r.updater.follow("0.8.0", r.events.clone());
        wait_until("the wait", || {
            matches!(
                r.updater.status(),
                Some(WorkerUpdateStatus::WaitingForWork { .. })
            )
        })
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(r.swap.swapped.lock().unwrap().is_none(), "work still runs");
        assert!(!*r.updater.restart_requested().borrow());
        r.events.busy.store(false, Ordering::SeqCst);
        wait_until("the restart", || restarting(&r.updater)).await;
        assert!(r.swap.swapped.lock().unwrap().is_some());
    }

    #[tokio::test]
    async fn it_never_moves_to_an_older_or_the_same_version() {
        let r = rig("0.6.0", None);
        for version in ["0.6.0", "0.7.0"] {
            assert_eq!(r.updater.follow(version, r.events.clone()), None);
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(r.host.downloads.load(Ordering::SeqCst), 0);
        assert!(r.swap.swapped.lock().unwrap().is_none());
        assert_eq!(r.updater.status(), None);
    }

    #[tokio::test]
    async fn a_name_that_is_not_a_version_does_nothing() {
        let r = rig("0.8.0", None);
        for name in ["latest", "0.8.0-../../evil", "https://evil.test/consort"] {
            r.updater.follow(name, r.events.clone());
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(r.host.downloads.load(Ordering::SeqCst), 0);
        assert_eq!(r.updater.status(), None);
    }

    #[tokio::test]
    async fn a_wrong_checksum_keeps_the_old_program_and_is_not_tried_again_at_once() {
        let r = rig("0.8.0", Some(&sha(b"another file")));
        r.updater.follow("0.8.0", r.events.clone());
        wait_until("the failure", || {
            matches!(r.updater.status(), Some(WorkerUpdateStatus::Failed { .. }))
        })
        .await;
        let Some(WorkerUpdateStatus::Failed { message, .. }) = r.updater.status() else {
            unreachable!()
        };
        assert!(message.contains("does not match its checksum"), "{message}");
        assert!(r.swap.swapped.lock().unwrap().is_none());
        assert!(!*r.updater.restart_requested().borrow());
        // A hub that keeps refusing the link names the version again and
        // again. That does not start one download after the other.
        r.updater.follow("0.8.0", r.events.clone());
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(r.host.downloads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_failed_swap_reports_the_failure_and_does_not_restart() {
        let r = rig("0.8.0", None);
        r.swap.fail.store(true, Ordering::SeqCst);
        r.updater.follow("0.8.0", r.events.clone());
        wait_until("the failure", || {
            matches!(r.updater.status(), Some(WorkerUpdateStatus::Failed { .. }))
        })
        .await;
        assert!(!*r.updater.restart_requested().borrow());
    }

    #[tokio::test]
    async fn the_same_version_named_twice_runs_once() {
        let r = rig("0.8.0", None);
        r.events.busy.store(true, Ordering::SeqCst);
        r.updater.follow("0.8.0", r.events.clone());
        r.updater.follow("0.8.0", r.events.clone());
        wait_until("the wait", || {
            matches!(
                r.updater.status(),
                Some(WorkerUpdateStatus::WaitingForWork { .. })
            )
        })
        .await;
        assert_eq!(r.host.downloads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_newer_version_replaces_one_that_waits() {
        let r = rig("0.8.0", None);
        let newer = Version::parse("0.9.0").unwrap();
        let bytes = tarball(b"newer program");
        {
            let mut files = r.host.files.lock().unwrap();
            files.insert(
                release_file_url(&newer, CHECKSUMS_ASSET),
                format!("{}  {ASSET}\n", sha(&bytes)).into_bytes(),
            );
            files.insert(release_file_url(&newer, ASSET), bytes);
        }
        r.events.busy.store(true, Ordering::SeqCst);
        r.updater.follow("0.8.0", r.events.clone());
        wait_until("the wait", || {
            matches!(
                r.updater.status(),
                Some(WorkerUpdateStatus::WaitingForWork { .. })
            )
        })
        .await;
        r.updater.follow("0.9.0", r.events.clone());
        wait_until("the second download", || {
            r.host.downloads.load(Ordering::SeqCst) == 2
        })
        .await;
        r.events.busy.store(false, Ordering::SeqCst);
        wait_until("the restart", || {
            matches!(
                r.updater.status(),
                Some(WorkerUpdateStatus::Restarting { version }) if version == "0.9.0"
            )
        })
        .await;
        let (bytes, _) = r.swap.swapped.lock().unwrap().clone().unwrap();
        assert_eq!(bytes, b"newer program");
    }

    #[tokio::test]
    async fn a_build_that_cannot_replace_itself_says_so_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let updater = Arc::new(Updater::new(None, dir.path().join("update")));
        let events = Arc::new(FakeEvents::default());
        assert_eq!(
            updater.follow("0.8.0", events.clone()),
            Some(WorkerUpdateStatus::NotAvailable {
                version: "0.8.0".into()
            })
        );
        assert!(!*updater.restart_requested().borrow());
        assert!(!dir.path().join("update").exists());
    }

    /// `cargo test` is a debug build in a `target` folder.
    #[test]
    fn this_test_program_never_gets_a_self_update() {
        assert!(SelfUpdate::for_this_program().is_none());
    }
}
