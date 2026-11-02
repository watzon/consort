//! The renderer of the session host: where its browser comes from, and the
//! settings of the operator.
//!
//! - `CONSORT_VISUAL_CHROME` names a browser program. It wins over the
//!   managed browser. Use it on a platform with no Chrome for Testing build
//!   (musl Linux), or for a system Chromium.
//! - `CONSORT_VISUAL_NO_SANDBOX=1` turns off the sandbox of the browser.
//!   The renderer never turns it off by itself. docs/deploy.md says when an
//!   operator may need it.
//!
//! Without `CONSORT_VISUAL_CHROME`, the renderer uses the managed
//! `chrome-headless-shell`. The first render that finds it missing starts
//! the install in the background and fails with
//! [`RenderError::Installing`](super::RenderError::Installing), so no tool
//! call waits for a download.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use consort_harness::process::WatcherHandle;
use consort_tools::ToolId;

use super::{ChromeSource, Renderer, RendererConfig};
use crate::tools::Tools;

/// The environment variable with the path of a browser program.
pub const CHROME_VAR: &str = "CONSORT_VISUAL_CHROME";
/// The environment variable that turns off the sandbox of the browser when
/// it is `1`.
pub const NO_SANDBOX_VAR: &str = "CONSORT_VISUAL_NO_SANDBOX";

/// The settings of the operator, from the environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Settings {
    /// The browser program of `CONSORT_VISUAL_CHROME`.
    pub chrome: Option<PathBuf>,
    /// Whether `CONSORT_VISUAL_NO_SANDBOX` is `1`.
    pub no_sandbox: bool,
}

impl Settings {
    /// Reads the settings from the values of the two variables.
    pub(crate) fn from_values(chrome: Option<&str>, no_sandbox: Option<&str>) -> Self {
        Self {
            chrome: chrome
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            no_sandbox: no_sandbox.map(str::trim) == Some("1"),
        }
    }

    /// Reads the settings from the environment of the session host.
    pub(crate) fn from_env() -> Self {
        let chrome = std::env::var(CHROME_VAR).ok();
        let no_sandbox = std::env::var(NO_SANDBOX_VAR).ok();
        Self::from_values(chrome.as_deref(), no_sandbox.as_deref())
    }
}

/// Where the browser is now, or `None` when the managed browser is not
/// installed, no install runs, and no install failed.
///
/// The program of `chrome` wins. An installed managed browser comes next,
/// also while an update of it runs.
pub(crate) fn chrome_source(tools: &Tools, chrome: Option<&Path>) -> Option<ChromeSource> {
    if let Some(path) = chrome {
        return Some(ChromeSource::Path(path.to_path_buf()));
    }
    let tool = ToolId::ChromeHeadlessShell;
    if tools.store().installed(&tool).is_some() {
        return Some(match tools.store().launch(&tool) {
            Ok(spec) => ChromeSource::Path(spec.program),
            Err(err) => ChromeSource::Failed(err.to_string()),
        });
    }
    if tools.installing(&tool) {
        return Some(ChromeSource::Installing);
    }
    tools.install_failure(&tool).map(ChromeSource::Failed)
}

/// The function that the renderer calls each time it starts a browser. A
/// missing managed browser starts to install in the background, once, and
/// the function says [`ChromeSource::Installing`]. `changed` runs when the
/// install starts and when it ends, so the hub learns the new tool state.
pub(crate) fn finder(
    tools: Arc<Tools>,
    chrome: Option<PathBuf>,
    changed: Arc<dyn Fn() + Send + Sync>,
) -> impl Fn() -> ChromeSource + Send + Sync + 'static {
    // True from the start of an install task until it ends. It keeps a
    // second render from starting a second task before the store marks the
    // tool as installing.
    let starting = Arc::new(AtomicBool::new(false));
    move || {
        if let Some(source) = chrome_source(&tools, chrome.as_deref()) {
            return source;
        }
        if starting.swap(true, Ordering::AcqRel) {
            return ChromeSource::Installing;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            starting.store(false, Ordering::Release);
            return ChromeSource::Failed(
                "the session host could not start the install of the preview browser".to_owned(),
            );
        };
        tracing::info!("installing the preview browser for visuals");
        let tools = tools.clone();
        let starting = starting.clone();
        let changed = changed.clone();
        runtime.spawn(async move {
            changed();
            match tools.ensure(&ToolId::ChromeHeadlessShell).await {
                Ok(_) => tracing::info!("installed the preview browser"),
                Err(err) => {
                    tracing::warn!(error = %err, "could not install the preview browser");
                }
            }
            starting.store(false, Ordering::Release);
            changed();
        });
        ChromeSource::Installing
    }
}

/// The renderer of a session host.
pub(crate) fn session_renderer(
    tools: Arc<Tools>,
    settings: Settings,
    watcher: Option<WatcherHandle>,
    changed: Arc<dyn Fn() + Send + Sync>,
) -> Renderer {
    if let Some(path) = &settings.chrome {
        tracing::info!(path = %path.display(), "visuals use the browser of {CHROME_VAR}");
    }
    if settings.no_sandbox {
        tracing::warn!(
            "{NO_SANDBOX_VAR}=1: the preview browser runs without its sandbox. Read docs/deploy.md."
        );
    }
    let mut config = RendererConfig::new(finder(tools, settings.chrome, changed));
    config.watcher = watcher;
    config.no_sandbox = settings.no_sandbox;
    Renderer::new(config)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use consort_tools::{ToolStore, ToolStoreOptions};

    use super::*;

    /// A store whose browser downloads fail at once: nothing listens on
    /// port 1.
    fn offline_tools(dir: &Path) -> Arc<Tools> {
        let mut options = ToolStoreOptions::detect().unwrap();
        options.sources.chrome_for_testing = "http://127.0.0.1:1".to_owned();
        let store = ToolStore::with_options(dir.join("tools"), options).unwrap();
        Arc::new(Tools::with_store(store, false))
    }

    /// Makes the managed browser look installed.
    fn fake_install(dir: &Path) {
        let tool_dir = dir
            .join("tools")
            .join(ToolId::ChromeHeadlessShell.dir_name());
        std::fs::create_dir_all(tool_dir.join("156.0.8078.80")).unwrap();
        std::fs::write(tool_dir.join("current"), "156.0.8078.80").unwrap();
    }

    #[test]
    fn visual_settings_read_the_environment_values() {
        assert_eq!(Settings::from_values(None, None), Settings::default());
        let set = Settings::from_values(Some(" /usr/bin/chromium "), Some("1"));
        assert_eq!(set.chrome, Some(PathBuf::from("/usr/bin/chromium")));
        assert!(set.no_sandbox);
        // Only `1` turns the sandbox off.
        for value in ["0", "true", "yes", ""] {
            assert!(
                !Settings::from_values(None, Some(value)).no_sandbox,
                "{value}"
            );
        }
        assert_eq!(Settings::from_values(Some("  "), None).chrome, None);
    }

    #[test]
    fn visual_finder_prefers_the_program_of_the_operator() {
        let dir = tempfile::tempdir().unwrap();
        let tools = offline_tools(dir.path());
        assert_eq!(chrome_source(&tools, None), None);
        let program = Path::new("/usr/bin/chromium");
        assert_eq!(
            chrome_source(&tools, Some(program)),
            Some(ChromeSource::Path(program.to_path_buf()))
        );
        fake_install(dir.path());
        assert_eq!(
            chrome_source(&tools, Some(program)),
            Some(ChromeSource::Path(program.to_path_buf()))
        );
        let Some(ChromeSource::Path(managed)) = chrome_source(&tools, None) else {
            panic!("the managed browser is installed");
        };
        assert!(managed.starts_with(dir.path().join("tools")), "{managed:?}");
        assert!(
            managed.to_string_lossy().contains("chrome-headless-shell"),
            "{managed:?}"
        );
    }

    #[tokio::test]
    async fn visual_finder_starts_one_install_and_then_reports_its_failure() {
        let dir = tempfile::tempdir().unwrap();
        let tools = offline_tools(dir.path());
        let changes = Arc::new(AtomicUsize::new(0));
        let counter = changes.clone();
        let find = finder(
            tools.clone(),
            None,
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        );
        assert_eq!(find(), ChromeSource::Installing);
        // A second render while the first install runs starts no other.
        assert_eq!(find(), ChromeSource::Installing);
        // The start and the end of one install.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while changes.load(Ordering::SeqCst) < 2 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the install did not end"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let ChromeSource::Failed(reason) = find() else {
            panic!("the failed install is not reported");
        };
        assert!(!reason.is_empty());
        // No new install starts after a failure.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(changes.load(Ordering::SeqCst), 2);
        assert_eq!(
            chrome_source(&tools, None),
            Some(ChromeSource::Failed(reason))
        );
    }
}
