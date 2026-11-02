//! One headless browser process: its flags, its temp profile, its proxy and
//! its DevTools connection.
//!
//! The browser opens its DevTools port on `127.0.0.1` at a free port and
//! writes the port and the path to `<user-data-dir>/DevToolsActivePort`.
//! A pipe would need file descriptors 3 and 4 in the child, and that needs
//! `unsafe` code.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use consort_harness::process::{HarnessProcess, ProcessSpec, WatcherHandle};
use serde_json::json;
use tokio::io::AsyncReadExt;

use super::RenderError;
use super::cdp::Cdp;
use super::proxy::Proxy;

/// The file in the profile where the browser writes its DevTools address.
pub(crate) const ACTIVE_PORT_FILE: &str = "DevToolsActivePort";
/// How often the launcher looks for [`ACTIVE_PORT_FILE`].
const ACTIVE_PORT_POLL: Duration = Duration::from_millis(25);
/// How long the browser has to quit when it is asked to.
const QUIT_GRACE: Duration = Duration::from_secs(2);
/// The most characters of the browser's output in an error message.
const STDERR_IN_ERROR: usize = 1_000;

/// How to start a browser.
#[derive(Debug, Clone)]
pub(crate) struct LaunchOptions {
    /// The `chrome-headless-shell` program.
    pub program: PathBuf,
    /// How long the browser has to write its DevTools address.
    pub launch_timeout: Duration,
    /// How long one DevTools call may wait for its reply.
    pub call_timeout: Duration,
    /// Learns the process group of the browser, so that a session host
    /// that was killed can end it at the next start.
    pub watcher: Option<WatcherHandle>,
    /// Whether the operator turned off the sandbox of the browser
    /// (`CONSORT_VISUAL_NO_SANDBOX=1`).
    pub no_sandbox: bool,
}

/// A running browser. Dropping it kills the process group and deletes the
/// profile; [`Browser::stop`] asks the browser to quit first.
pub(crate) struct Browser {
    // The fields drop in this order: the process first, then its profile,
    // then the proxy.
    process: tokio::sync::Mutex<HarnessProcess>,
    profile: Profile,
    cdp: Cdp,
    pid: Option<u32>,
    _proxy: Proxy,
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser")
            .field("pid", &self.pid)
            .field("profile", &self.profile.path)
            .finish_non_exhaustive()
    }
}

impl Browser {
    /// Starts the proxy and the browser, and connects to it.
    pub(crate) async fn launch(options: &LaunchOptions) -> Result<Self, RenderError> {
        let no_sandbox = sandbox_off(is_root(), options.no_sandbox)?;
        let proxy = Proxy::start()
            .await
            .map_err(|e| RenderError::Start(format!("the network proxy could not start: {e}")))?;
        let profile = Profile::create().map_err(|e| {
            RenderError::Start(format!("the temporary profile could not be made: {e}"))
        })?;
        let spec = ProcessSpec {
            program: options.program.clone(),
            args: flags(
                &profile.path,
                proxy.port(),
                no_sandbox,
                cfg!(target_os = "linux"),
            ),
            watcher: options.watcher.clone(),
            ..ProcessSpec::default()
        };
        let mut process = consort_harness::process::spawn(&spec)
            .map_err(|e| RenderError::Start(format!("the program could not run: {e}")))?;
        drop(process.take_stdin());
        // Read stdout, so a full pipe never blocks the browser.
        if let Some(mut stdout) = process.take_stdout() {
            tokio::spawn(async move {
                let mut sink = [0u8; 4096];
                while matches!(stdout.read(&mut sink).await, Ok(n) if n > 0) {}
            });
        }
        let pid = process.id();
        let url = wait_for_active_port(&mut process, &profile.path, options.launch_timeout).await?;
        let cdp = Cdp::connect(&url, options.call_timeout)
            .await
            .map_err(|e| {
                start_failure(
                    &process.stderr_tail(),
                    format!("its DevTools port did not answer: {e}"),
                )
            })?;
        Ok(Self {
            process: tokio::sync::Mutex::new(process),
            profile,
            cdp,
            pid,
            _proxy: proxy,
        })
    }

    /// The DevTools connection.
    pub(crate) fn cdp(&self) -> &Cdp {
        &self.cdp
    }

    /// The process ID of the browser.
    pub(crate) fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The temp profile of the browser.
    #[cfg(test)]
    pub(crate) fn profile_dir(&self) -> &Path {
        &self.profile.path
    }

    /// Whether the browser still runs and its connection is open.
    pub(crate) fn is_alive(&self) -> bool {
        if self.cdp.is_closed() {
            return false;
        }
        // A render that stops the browser holds the lock. Count that
        // browser as alive; the caller sees the closed connection next.
        match self.process.try_lock() {
            Ok(mut process) => !process.has_exited(),
            Err(_) => true,
        }
    }

    /// Asks the browser to quit, kills its process group, and deletes the
    /// profile.
    pub(crate) async fn stop(&self) {
        // A confined browser (AppArmor in LXC, for example) can miss a
        // signal, so ask it first.
        if !self.cdp.is_closed() {
            let _ = self
                .cdp
                .call_with_timeout("Browser.close", json!({}), None, QUIT_GRACE)
                .await;
        }
        self.process.lock().await.shutdown(QUIT_GRACE).await;
        self.profile.remove().await;
    }
}

/// The browser flags. `no_sandbox` turns off the sandbox of the browser;
/// only the operator asks for it ([`sandbox_off`]). `linux` adds the flags
/// How many times to try to delete a profile, and the wait between tries.
const PROFILE_REMOVE_TRIES: usize = 10;
const PROFILE_REMOVE_GAP: Duration = Duration::from_millis(100);

/// for Linux.
pub(crate) fn flags(profile: &Path, proxy_port: u16, no_sandbox: bool, linux: bool) -> Vec<String> {
    let mut flags = vec![
        "--headless=new".to_owned(),
        "--remote-debugging-port=0".to_owned(),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        "--disable-gpu".to_owned(),
        "--hide-scrollbars".to_owned(),
        "--mute-audio".to_owned(),
        "--block-new-web-contents".to_owned(),
        "--disable-quic".to_owned(),
        format!("--proxy-server=socks5://127.0.0.1:{proxy_port}"),
        // Loopback skips the proxy unless this rule removes it from the
        // bypass list.
        "--proxy-bypass-list=<-loopback>".to_owned(),
        // WebRTC sends UDP, which no proxy carries.
        "--force-webrtc-ip-handling-policy=disable_non_proxied_udp".to_owned(),
    ];
    if linux {
        // Docker gives a container 64 MB of `/dev/shm`, and a tall page
        // needs more. The browser then uses the temp folder.
        flags.push("--disable-dev-shm-usage".to_owned());
    }
    if no_sandbox {
        flags.push("--no-sandbox".to_owned());
    }
    flags.push("about:blank".to_owned());
    flags
}

/// Whether the browser starts with `--no-sandbox`. The renderer never turns
/// the sandbox off by itself: only the operator does, with
/// `CONSORT_VISUAL_NO_SANDBOX=1` (`allowed`). The browser refuses to run as
/// root with its sandbox, so a root session host without that setting gets
/// [`RenderError::RootWithoutSandbox`] before the browser starts.
pub(crate) fn sandbox_off(root: bool, allowed: bool) -> Result<bool, RenderError> {
    match (root, allowed) {
        (_, true) => Ok(true),
        (true, false) => Err(RenderError::RootWithoutSandbox),
        (false, false) => Ok(false),
    }
}

/// Whether the effective user is root. Only Linux counts: the sandbox rule
/// of the browser for root is a Linux rule.
fn is_root() -> bool {
    #[cfg(target_os = "linux")]
    {
        rustix::process::geteuid().is_root()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Reads the DevTools address from the text of [`ACTIVE_PORT_FILE`]: the
/// port on the first line and the browser path on the second.
pub(crate) fn parse_active_port(text: &str) -> Option<String> {
    let mut lines = text.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim();
    if port == 0 || !path.starts_with("/devtools/browser/") {
        return None;
    }
    Some(format!("ws://127.0.0.1:{port}{path}"))
}

/// The library in a "error while loading shared libraries" line of the
/// browser's output.
pub(crate) fn missing_library(stderr: &str) -> Option<String> {
    const MARK: &str = "error while loading shared libraries: ";
    let start = stderr.find(MARK)? + MARK.len();
    let rest = &stderr[start..];
    let name = rest.split([':', '\n']).next()?.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Whether the browser's output says that its sandbox could not start. The
/// Linux sandbox writes "No usable sandbox!" when the system refuses user
/// namespaces (AppArmor, seccomp or a container), and "Running as root
/// without --no-sandbox is not supported" for root.
pub(crate) fn sandbox_failure(stderr: &str) -> Option<RenderError> {
    if stderr.contains("No usable sandbox") {
        return Some(RenderError::NoUsableSandbox);
    }
    if stderr.contains("Running as root without --no-sandbox") {
        return Some(RenderError::RootWithoutSandbox);
    }
    None
}

/// The error for a browser that did not start, with the end of its output.
fn start_failure(stderr: &str, reason: String) -> RenderError {
    if let Some(library) = missing_library(stderr) {
        return RenderError::MissingLibrary(library);
    }
    if let Some(err) = sandbox_failure(stderr) {
        return err;
    }
    let stderr = stderr.trim();
    if stderr.is_empty() {
        return RenderError::Start(reason);
    }
    let (tail, _) = consort_proto::model::tail_of(stderr, STDERR_IN_ERROR);
    RenderError::Start(format!("{reason}. The browser wrote: {}", tail.trim()))
}

/// Waits until the browser writes its DevTools address, exits, or the time
/// runs out.
async fn wait_for_active_port(
    process: &mut HarnessProcess,
    profile: &Path,
    timeout: Duration,
) -> Result<String, RenderError> {
    let file = profile.join(ACTIVE_PORT_FILE);
    let started = Instant::now();
    loop {
        // The browser may not have finished the file, so a part that does
        // not parse waits for the next poll.
        if let Ok(text) = tokio::fs::read_to_string(&file).await
            && let Some(url) = parse_active_port(&text)
        {
            return Ok(url);
        }
        if process.has_exited() {
            // Let the stderr reader take the last lines.
            tokio::time::sleep(Duration::from_millis(200)).await;
            return Err(start_failure(
                &process.stderr_tail(),
                "the browser exited before it opened its DevTools port".to_owned(),
            ));
        }
        if started.elapsed() >= timeout {
            let err = start_failure(
                &process.stderr_tail(),
                format!(
                    "the browser did not open its DevTools port in {} s",
                    timeout.as_secs()
                ),
            );
            process.kill().await;
            return Err(err);
        }
        tokio::time::sleep(ACTIVE_PORT_POLL).await;
    }
}

/// The temp profile of one browser. It is deleted when the browser stops,
/// or when this value drops.
struct Profile {
    path: PathBuf,
    removed: AtomicBool,
}

impl Profile {
    fn create() -> std::io::Result<Self> {
        // A short name: the browser makes socket files in the profile, and
        // a macOS socket path has at most 104 bytes.
        let path =
            std::env::temp_dir().join(format!("consort-visual-{:016x}", rand::random::<u64>()));
        crate::config::create_private_dir(&path)?;
        Ok(Self {
            path,
            removed: AtomicBool::new(false),
        })
    }

    async fn remove(&self) {
        if self.removed.swap(true, Ordering::AcqRel) {
            return;
        }
        // A helper process of the browser can still write to the profile for
        // a short time after the browser quits, so try again a few times.
        let mut last = None;
        for _ in 0..PROFILE_REMOVE_TRIES {
            let path = self.path.clone();
            match tokio::task::spawn_blocking(move || std::fs::remove_dir_all(path)).await {
                Ok(Ok(())) => return,
                Ok(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => return,
                Ok(Err(err)) => last = Some(err.to_string()),
                Err(err) => last = Some(err.to_string()),
            }
            tokio::time::sleep(PROFILE_REMOVE_GAP).await;
        }
        if let Some(err) = last {
            tracing::debug!(error = %err, path = %self.path.display(), "could not delete the profile of the preview browser");
        }
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        if !*self.removed.get_mut() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_browser_reads_the_active_port_file() {
        assert_eq!(
            parse_active_port("41235\n/devtools/browser/0b8c-41d2\n").as_deref(),
            Some("ws://127.0.0.1:41235/devtools/browser/0b8c-41d2")
        );
        assert_eq!(
            parse_active_port("9222\r\n/devtools/browser/abc").as_deref(),
            Some("ws://127.0.0.1:9222/devtools/browser/abc")
        );
        // A file that the browser has not finished.
        assert_eq!(parse_active_port(""), None);
        assert_eq!(parse_active_port("41235"), None);
        assert_eq!(parse_active_port("41235\n"), None);
        assert_eq!(parse_active_port("412"), None);
        // Bad content.
        assert_eq!(parse_active_port("0\n/devtools/browser/x"), None);
        assert_eq!(parse_active_port("port\n/devtools/browser/x"), None);
        assert_eq!(parse_active_port("70000\n/devtools/browser/x"), None);
        assert_eq!(parse_active_port("9222\n/json/version"), None);
    }

    #[test]
    fn visual_browser_names_a_missing_library() {
        let stderr = "[0101/000000.000:INFO] starting\n/opt/chrome-headless-shell: error while loading shared libraries: libnss3.so: cannot open shared object file: No such file or directory\n";
        assert_eq!(missing_library(stderr).as_deref(), Some("libnss3.so"));
        let err = start_failure(stderr, "the browser exited".to_owned());
        assert!(matches!(&err, RenderError::MissingLibrary(lib) if lib == "libnss3.so"));
        let text = err.to_string();
        assert!(text.contains("libnss3.so"), "{text}");
        assert!(text.contains("docs/deploy.md"), "{text}");
        assert_eq!(missing_library("all good\n"), None);
    }

    #[test]
    fn visual_browser_start_failure_has_the_output_tail() {
        let err = start_failure(
            "line one\nFatal: no display\n",
            "the browser exited".to_owned(),
        );
        let text = err.to_string();
        assert!(matches!(err, RenderError::Start(_)));
        assert!(text.contains("the browser exited"), "{text}");
        assert!(text.contains("Fatal: no display"), "{text}");
        let long = "x".repeat(5_000);
        let text = start_failure(&long, "failed".to_owned()).to_string();
        assert!(text.len() < 1_400, "{}", text.len());
        assert!(
            matches!(start_failure("  ", "failed".to_owned()), RenderError::Start(r) if r == "failed")
        );
    }

    #[test]
    fn visual_browser_flags() {
        let profile = Path::new("/tmp/consort-visual-test");
        let flags = flags(profile, 4567, false, false);
        for expected in [
            "--headless=new",
            "--remote-debugging-port=0",
            "--user-data-dir=/tmp/consort-visual-test",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            "--hide-scrollbars",
            "--mute-audio",
            "--block-new-web-contents",
            "--disable-quic",
            "--proxy-server=socks5://127.0.0.1:4567",
            "--proxy-bypass-list=<-loopback>",
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
        ] {
            assert!(
                flags.iter().any(|f| f == expected),
                "no {expected} in {flags:?}"
            );
        }
        assert!(!flags.iter().any(|f| f == "--no-sandbox"));
        assert!(!flags.iter().any(|f| f == "--disable-dev-shm-usage"));
        assert_eq!(flags.last().map(String::as_str), Some("about:blank"));

        let no_sandbox = super::flags(profile, 4567, true, false);
        assert!(no_sandbox.iter().any(|f| f == "--no-sandbox"));
        assert_eq!(no_sandbox.len(), flags.len() + 1);
        assert_eq!(no_sandbox.last().map(String::as_str), Some("about:blank"));

        // On Linux the browser does not use the small `/dev/shm` of Docker,
        // and it keeps its sandbox.
        let linux = super::flags(profile, 4567, false, true);
        assert!(linux.iter().any(|f| f == "--disable-dev-shm-usage"));
        assert!(!linux.iter().any(|f| f == "--no-sandbox"));
        assert_eq!(linux.len(), flags.len() + 1);
        assert_eq!(linux.last().map(String::as_str), Some("about:blank"));
    }

    #[test]
    fn visual_browser_never_turns_off_the_sandbox_by_itself() {
        // A normal user keeps the sandbox.
        assert_eq!(sandbox_off(false, false), Ok(false));
        // Root without the setting of the operator does not start.
        let err = sandbox_off(true, false).unwrap_err();
        assert_eq!(err, RenderError::RootWithoutSandbox);
        let text = err.to_string();
        assert!(text.contains("CONSORT_VISUAL_NO_SANDBOX=1"), "{text}");
        assert!(text.contains("docs/deploy.md"), "{text}");
        assert!(text.contains("another user"), "{text}");
        // The operator turns it off, for root or for another user.
        assert_eq!(sandbox_off(true, true), Ok(true));
        assert_eq!(sandbox_off(false, true), Ok(true));
        #[cfg(not(target_os = "linux"))]
        assert!(!is_root());
    }

    #[test]
    fn visual_browser_reads_a_sandbox_failure() {
        let stderr = "[1101/101010.000:FATAL:zygote_host_impl_linux.cc(127)] No usable sandbox! Update your kernel or see https://chromium.googlesource.com/chromium/src/+/main/docs/linux/suid_sandbox_development.md for more information on developing with the SUID sandbox.\n";
        let err = start_failure(stderr, "the browser exited".to_owned());
        assert_eq!(err, RenderError::NoUsableSandbox);
        let text = err.to_string();
        assert!(text.contains("docs/deploy.md"), "{text}");
        assert!(text.contains("sandbox"), "{text}");
        let root = "[1101/101010.000:ERROR:zygote_host_impl_linux.cc(100)] Running as root without --no-sandbox is not supported. See https://crbug.com/638180.\n";
        assert_eq!(
            start_failure(root, "the browser exited".to_owned()),
            RenderError::RootWithoutSandbox
        );
        assert_eq!(sandbox_failure("all good\n"), None);
        // A missing library comes first: the sandbox never started.
        let both =
            "error while loading shared libraries: libnss3.so: cannot open\nNo usable sandbox!";
        assert!(matches!(
            start_failure(both, "x".to_owned()),
            RenderError::MissingLibrary(_)
        ));
    }

    #[tokio::test]
    async fn visual_browser_profile_is_private_and_deleted() {
        let profile = Profile::create().unwrap();
        let path = profile.path.clone();
        assert!(path.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        std::fs::write(path.join(ACTIVE_PORT_FILE), "1\n/devtools/browser/x").unwrap();
        profile.remove().await;
        assert!(!path.exists());

        let dropped = Profile::create().unwrap();
        let path = dropped.path.clone();
        drop(dropped);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn visual_browser_reports_a_program_that_exits() {
        // `false` exits at once with no DevTools port.
        let Some(program) = consort_harness::process::resolve_program("false") else {
            return;
        };
        let options = LaunchOptions {
            program,
            launch_timeout: Duration::from_secs(5),
            call_timeout: Duration::from_secs(5),
            watcher: None,
            no_sandbox: false,
        };
        let err = Browser::launch(&options).await.unwrap_err();
        assert!(matches!(err, RenderError::Start(_)), "{err:?}");
        assert!(err.to_string().contains("exited"), "{err}");
    }

    #[tokio::test]
    async fn visual_browser_reports_a_program_that_does_not_exist() {
        let options = LaunchOptions {
            program: PathBuf::from("/nonexistent/chrome-headless-shell"),
            launch_timeout: Duration::from_secs(1),
            call_timeout: Duration::from_secs(1),
            watcher: None,
            no_sandbox: false,
        };
        let err = Browser::launch(&options).await.unwrap_err();
        assert!(matches!(err, RenderError::Start(_)), "{err:?}");
    }
}
