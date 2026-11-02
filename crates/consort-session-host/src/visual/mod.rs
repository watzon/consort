//! The renderer of visuals: one headless browser over the DevTools
//! protocol, behind a proxy that reaches only public addresses. The plan is
//! `docs/VISUALS-PLAN.md`, "Renderer (session host)".
//!
//! A [`Renderer`] starts its browser on the first render and stops it after
//! a time with no work ([`RendererConfig::idle_stop`]). A browser that
//! crashed starts again on the next render. At most
//! [`RendererConfig::max_pages`] pages render at once.
//!
//! [`Renderer::render`] gives the PNG of a page at one width and scale, its
//! height and its console output. [`Renderer::measure`] loads a page once
//! and gives its height at each width: the height table of a visual.
//!
//! The document must be complete: the caller adds the bootstrap of
//! `consort-visual` first.
//!
//! The rest of the session host uses the renderer through [`VisualEngine`],
//! so its tests can use a fake. `setup` makes the renderer of the session
//! host, and `command` answers `visual.render` from the hub.

mod browser;
mod cdp;
pub(crate) mod command;
mod proxy;
mod render;
pub(crate) mod setup;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use consort_harness::process::WatcherHandle;
use consort_proto::model::{
    VISUAL_FIT_MAX, VISUAL_RENDER_WIDTH_MAX, VISUAL_SCALE_MAX, VisualHeight, VisualMode,
};
use consort_proto::tools::{PREVIEW_VISUAL_INSTALLING, VisualConsoleMessage};
use tokio::sync::Semaphore;

use self::browser::{Browser, LaunchOptions};
use self::cdp::CdpError;

/// The time with no work after which the browser stops.
pub const DEFAULT_IDLE_STOP: Duration = Duration::from_secs(5 * 60);
/// The most pages that render at once.
pub const DEFAULT_MAX_PAGES: usize = 2;
/// The longest that one render or one height table may take.
pub const DEFAULT_RENDER_TIMEOUT: Duration = Duration::from_secs(20);
/// The longest that the browser may take to start.
pub const DEFAULT_LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the browser program is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChromeSource {
    /// The `chrome-headless-shell` program.
    Path(PathBuf),
    /// The managed browser is installing.
    Installing,
    /// The managed browser is not available, for this reason.
    Failed(String),
}

/// Finds the browser program when the renderer starts a browser.
pub type ChromeFinder = Arc<dyn Fn() -> ChromeSource + Send + Sync>;

/// The settings of a [`Renderer`].
#[derive(Clone)]
pub struct RendererConfig {
    /// Finds the browser program. It runs each time a browser starts.
    pub chrome: ChromeFinder,
    /// The time with no work after which the browser stops.
    pub idle_stop: Duration,
    /// The most pages that render at once.
    pub max_pages: usize,
    /// The longest that one render or one height table may take, from the
    /// new page to its close. The wait for a free page and the browser
    /// start are not in it.
    pub render_timeout: Duration,
    /// The longest that the browser may take to start.
    pub launch_timeout: Duration,
    /// Learns the process group of each browser, so that a session host
    /// that was killed can end it at the next start.
    pub watcher: Option<WatcherHandle>,
    /// Whether the operator turned off the sandbox of the browser with
    /// `CONSORT_VISUAL_NO_SANDBOX=1`. The renderer never turns it off by
    /// itself. With this off, a session host that runs as root on Linux
    /// cannot start the browser.
    pub no_sandbox: bool,
}

impl std::fmt::Debug for RendererConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RendererConfig")
            .field("idle_stop", &self.idle_stop)
            .field("max_pages", &self.max_pages)
            .field("render_timeout", &self.render_timeout)
            .field("launch_timeout", &self.launch_timeout)
            .field("no_sandbox", &self.no_sandbox)
            .finish_non_exhaustive()
    }
}

impl RendererConfig {
    /// The default settings, with a function that finds the browser.
    pub fn new(chrome: impl Fn() -> ChromeSource + Send + Sync + 'static) -> Self {
        Self {
            chrome: Arc::new(chrome),
            idle_stop: DEFAULT_IDLE_STOP,
            max_pages: DEFAULT_MAX_PAGES,
            render_timeout: DEFAULT_RENDER_TIMEOUT,
            launch_timeout: DEFAULT_LAUNCH_TIMEOUT,
            watcher: None,
            no_sandbox: false,
        }
    }

    /// The default settings, with a browser at a known path.
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self::new(move || ChromeSource::Path(path.clone()))
    }
}

/// One render of a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The PNG. It is `width * scale` pixels wide and `height * scale`
    /// pixels tall.
    pub png: Vec<u8>,
    /// The height of the content in CSS pixels, clamped to
    /// `1..=VISUAL_FIT_MAX`. The image has this height, or the
    /// `max_height` of [`Renderer::render_within`] when the page is taller.
    pub height: u32,
    /// The console messages and page errors, at most
    /// `VISUAL_CONSOLE_MAX_MESSAGES`, each cut to
    /// `VISUAL_CONSOLE_MAX_CHARS`.
    pub console: Vec<VisualConsoleMessage>,
    /// How many console messages did not fit.
    pub omitted: u32,
}

/// Why a render failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    /// The managed browser is installing.
    #[error("{}", PREVIEW_VISUAL_INSTALLING)]
    Installing,
    /// The managed browser is not available.
    #[error("The preview browser is not available: {0}")]
    Unavailable(String),
    /// The browser did not start.
    #[error(
        "The preview browser could not start: {0}. Try again. If it fails again, read the log of the session host."
    )]
    Start(String),
    /// The browser did not start, because a shared library is missing.
    #[error(
        "The preview browser could not start, because the library {0} is missing. Install the libraries that docs/deploy.md lists for the preview browser, then try again."
    )]
    MissingLibrary(String),
    /// The session host runs as root on Linux, and the operator did not
    /// turn off the sandbox of the browser.
    #[error(
        "The preview browser does not run as root without its sandbox. Run the session host as another user, or set CONSORT_VISUAL_NO_SANDBOX=1 (read docs/deploy.md first)."
    )]
    RootWithoutSandbox,
    /// The browser could not start its sandbox: the system does not allow
    /// the user namespaces or the system calls that it needs.
    #[error(
        "The preview browser could not start its sandbox, because this system does not allow it. Follow the steps for the preview browser in docs/deploy.md, then try again."
    )]
    NoUsableSandbox,
    /// The render took too long.
    #[error(
        "The page did not finish in {} s. Remove scripts that do not end, or make the page smaller, then try again.",
        .0.as_secs()
    )]
    Timeout(Duration),
    /// The page did not load or could not be measured.
    #[error("The page did not load: {0}. Check the HTML, then try again.")]
    PageNotLoaded(String),
    /// The connection to the browser closed.
    #[error(
        "The connection to the preview browser closed: {0}. Try again. The browser starts again by itself."
    )]
    Closed(String),
    /// The browser refused a step.
    #[error("The preview browser could not do {method}: {message}. Try again.")]
    Step {
        /// The DevTools method.
        method: String,
        /// What the browser said.
        message: String,
    },
}

impl From<CdpError> for RenderError {
    fn from(err: CdpError) -> Self {
        match err {
            CdpError::Connect(reason) => {
                Self::Start(format!("its DevTools port did not answer: {reason}"))
            }
            CdpError::Closed(reason) => Self::Closed(reason),
            CdpError::Timeout { after, .. } => Self::Timeout(after),
            CdpError::Protocol { method, message } => Self::Step { method, message },
        }
    }
}

/// The renderer of a session host. Clones share one browser.
#[derive(Clone)]
pub struct Renderer {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

struct Inner {
    config: RendererConfig,
    pages: Semaphore,
    slot: tokio::sync::Mutex<Slot>,
    activity: Mutex<Activity>,
}

#[derive(Default)]
struct Slot {
    browser: Option<Arc<Browser>>,
    /// Counts the browsers that started. The idle task of an older browser
    /// sees a newer number and ends.
    generation: u64,
}

struct Activity {
    /// The renders that use the browser now.
    active: usize,
    /// When the last render ended.
    last_used: Instant,
}

impl Inner {
    fn activity(&self) -> std::sync::MutexGuard<'_, Activity> {
        match self.activity.lock() {
            Ok(a) => a,
            Err(p) => p.into_inner(),
        }
    }
}

/// Marks one render as active while it lives.
struct Active {
    inner: Arc<Inner>,
}

impl Drop for Active {
    fn drop(&mut self) {
        let mut activity = self.inner.activity();
        activity.active = activity.active.saturating_sub(1);
        activity.last_used = Instant::now();
    }
}

impl Renderer {
    /// A renderer. No browser starts until the first render.
    pub fn new(config: RendererConfig) -> Self {
        let pages = Semaphore::new(config.max_pages.max(1));
        Self {
            inner: Arc::new(Inner {
                config,
                pages,
                slot: tokio::sync::Mutex::new(Slot::default()),
                activity: Mutex::new(Activity {
                    active: 0,
                    last_used: Instant::now(),
                }),
            }),
        }
    }

    /// Renders a complete document at `width` CSS pixels and `scale`
    /// device pixels for each CSS pixel. The caller checks the ranges of
    /// its request; here `width` is clamped to `1..=VISUAL_RENDER_WIDTH_MAX`
    /// and `scale` to `1..=VISUAL_SCALE_MAX`.
    pub async fn render(
        &self,
        document: String,
        width: u32,
        scale: u8,
        mode: VisualMode,
    ) -> Result<Rendered, RenderError> {
        self.render_within(document, width, scale, mode, VISUAL_FIT_MAX)
            .await
    }

    /// Like [`Renderer::render`], but the image stops at `max_height` CSS
    /// pixels. [`Rendered::height`] is still the height of the whole page.
    pub async fn render_within(
        &self,
        document: String,
        width: u32,
        scale: u8,
        mode: VisualMode,
        max_height: u32,
    ) -> Result<Rendered, RenderError> {
        let width = width.clamp(1, VISUAL_RENDER_WIDTH_MAX);
        let scale = scale.clamp(1, VISUAL_SCALE_MAX);
        let max_height = max_height.clamp(1, VISUAL_FIT_MAX);
        self.with_page(|cdp| async move {
            render::render(&cdp, &document, width, scale, mode, max_height).await
        })
        .await
    }

    /// Loads a complete document once and measures its height at each
    /// width, in the order of `widths`.
    pub async fn measure(
        &self,
        document: String,
        widths: &[u32],
        mode: VisualMode,
    ) -> Result<Vec<VisualHeight>, RenderError> {
        let widths: Vec<u32> = widths
            .iter()
            .map(|w| (*w).clamp(1, VISUAL_RENDER_WIDTH_MAX))
            .collect();
        self.with_page(|cdp| async move { render::measure(&cdp, &document, &widths, mode).await })
            .await
    }

    /// Whether a browser runs now.
    pub async fn is_running(&self) -> bool {
        self.inner
            .slot
            .lock()
            .await
            .browser
            .as_ref()
            .is_some_and(|b| b.is_alive())
    }

    /// Stops the browser now, if one runs. The next render starts a new
    /// one.
    pub async fn stop(&self) {
        let browser = self.inner.slot.lock().await.browser.take();
        if let Some(browser) = browser {
            browser.stop().await;
        }
    }

    /// Runs one page job with a free page, the browser, and the timeout.
    async fn with_page<T, F, Fut>(&self, job: F) -> Result<T, RenderError>
    where
        F: FnOnce(cdp::Cdp) -> Fut,
        Fut: std::future::Future<Output = Result<T, RenderError>>,
    {
        let _permit = self
            .inner
            .pages
            .acquire()
            .await
            .map_err(|_| RenderError::Closed("the renderer stopped".to_owned()))?;
        let (browser, _active) = self.browser().await?;
        let timeout = self.inner.config.render_timeout;
        let started = Instant::now();
        let result = match tokio::time::timeout(timeout, job(browser.cdp().clone())).await {
            Ok(result) => result,
            Err(_) => Err(RenderError::Timeout(timeout)),
        };
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            ok = result.is_ok(),
            "rendered a visual"
        );
        result
    }

    /// The running browser, or a new one. The returned guard marks the
    /// browser as in use.
    async fn browser(&self) -> Result<(Arc<Browser>, Active), RenderError> {
        let mut slot = self.inner.slot.lock().await;
        if let Some(browser) = &slot.browser {
            if browser.is_alive() {
                let browser = browser.clone();
                return Ok((browser, self.begin()));
            }
            tracing::info!("the preview browser stopped; starting it again");
        }
        if let Some(old) = slot.browser.take() {
            old.stop().await;
        }
        let program = match (self.inner.config.chrome)() {
            ChromeSource::Path(path) => path,
            ChromeSource::Installing => return Err(RenderError::Installing),
            ChromeSource::Failed(reason) => return Err(RenderError::Unavailable(reason)),
        };
        let options = LaunchOptions {
            program,
            launch_timeout: self.inner.config.launch_timeout,
            call_timeout: self.inner.config.render_timeout,
            watcher: self.inner.config.watcher.clone(),
            no_sandbox: self.inner.config.no_sandbox,
        };
        let started = Instant::now();
        let browser = Arc::new(Browser::launch(&options).await?);
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            pid = browser.pid(),
            "started the preview browser"
        );
        slot.generation += 1;
        slot.browser = Some(browser.clone());
        let active = self.begin();
        tokio::spawn(idle_stop(Arc::downgrade(&self.inner), slot.generation));
        Ok((browser, active))
    }

    fn begin(&self) -> Active {
        self.inner.activity().active += 1;
        Active {
            inner: self.inner.clone(),
        }
    }

    /// The running browser, for tests.
    #[cfg(test)]
    async fn current(&self) -> Option<Arc<Browser>> {
        self.inner.slot.lock().await.browser.clone()
    }
}

/// What the session host needs from a renderer. [`Renderer`] is the real
/// one; tests use a fake with no browser.
#[async_trait]
pub trait VisualEngine: Send + Sync {
    /// Renders a complete document, as [`Renderer::render_within`] does.
    async fn render(
        &self,
        document: String,
        width: u32,
        scale: u8,
        mode: VisualMode,
        max_height: u32,
    ) -> Result<Rendered, RenderError>;

    /// Measures a complete document at each width, as
    /// [`Renderer::measure`] does.
    async fn measure(
        &self,
        document: String,
        widths: &[u32],
        mode: VisualMode,
    ) -> Result<Vec<VisualHeight>, RenderError>;

    /// Stops the browser, if one runs. The session host calls it when it
    /// stops.
    async fn stop(&self) {}
}

#[async_trait]
impl VisualEngine for Renderer {
    async fn render(
        &self,
        document: String,
        width: u32,
        scale: u8,
        mode: VisualMode,
        max_height: u32,
    ) -> Result<Rendered, RenderError> {
        self.render_within(document, width, scale, mode, max_height)
            .await
    }

    async fn measure(
        &self,
        document: String,
        widths: &[u32],
        mode: VisualMode,
    ) -> Result<Vec<VisualHeight>, RenderError> {
        Renderer::measure(self, document, widths, mode).await
    }

    async fn stop(&self) {
        Renderer::stop(self).await;
    }
}

/// Stops the browser of `generation` after the idle time. Ends when that
/// browser is gone or the renderer drops.
async fn idle_stop(inner: Weak<Inner>, generation: u64) {
    loop {
        let wait = {
            let Some(inner) = inner.upgrade() else { return };
            let mut slot = inner.slot.lock().await;
            if slot.generation != generation || slot.browser.is_none() {
                return;
            }
            let idle = inner.config.idle_stop;
            let (active, idle_for) = {
                let activity = inner.activity();
                (activity.active, activity.last_used.elapsed())
            };
            if active == 0 && idle_for >= idle {
                let browser = slot.browser.take();
                drop(slot);
                if let Some(browser) = browser {
                    browser.stop().await;
                    tracing::info!(
                        "stopped the preview browser after {} s with no work",
                        idle.as_secs()
                    );
                }
                return;
            }
            if active > 0 { idle } else { idle - idle_for }
        };
        tokio::time::sleep(wait).await;
    }
}
