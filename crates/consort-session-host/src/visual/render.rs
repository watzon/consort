//! The steps of one render and of one height table, on one page target.
//!
//! The page loads from `http://consort-visual.localhost/page.html`. That
//! request never leaves the browser: the `Fetch` domain answers it with the
//! document. Any other document request in the main frame gets an empty 204,
//! so the page cannot navigate away and the frame keeps the page. A
//! `.localhost` origin is a secure context, and the page may load resources
//! over HTTP. Its network requests go through the proxy of the browser.
//!
//! The console output has the console calls and the page errors of the
//! `Runtime` domain, and the entries of the `Log` domain: blocked loads and
//! network errors, such as a request that the proxy refused.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use consort_proto::model::{
    VISUAL_CONSOLE_MAX_CHARS, VISUAL_CONSOLE_MAX_MESSAGES, VISUAL_FIT_MAX, VisualHeight, VisualMode,
};
use consort_proto::tools::VisualConsoleMessage;
use consort_visual::{MEASURE_EXPRESSION, SETTLE_EXPRESSION};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::cdp::{Cdp, CdpError, Event};
use super::{RenderError, Rendered};

/// The origin of the page.
pub(crate) const PAGE_ORIGIN: &str = "http://consort-visual.localhost";
/// The address of the page.
pub(crate) const PAGE_URL: &str = "http://consort-visual.localhost/page.html";
/// What console messages call the page instead of [`PAGE_URL`].
const PAGE_NAME: &str = "page.html";
/// The height of the viewport. The page fills the width, and the capture
/// goes beyond the viewport to the measured height.
const VIEWPORT_HEIGHT: u32 = 800;
/// Waits for two animation frames, so a new width has its layout.
const FRAMES_EXPRESSION: &str = "new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true))))";
/// Removes WebRTC before any page script runs. Its ICE servers can make the
/// browser resolve names and send UDP outside the proxy.
const NO_WEBRTC: &str = "delete window.RTCPeerConnection; delete window.webkitRTCPeerConnection;";

/// Renders the page at one width and scale. The image stops at
/// `max_height` CSS pixels.
pub(crate) async fn render(
    cdp: &Cdp,
    document: &str,
    width: u32,
    scale: u8,
    mode: VisualMode,
    max_height: u32,
) -> Result<Rendered, RenderError> {
    let page = Page::open(cdp, document, mode).await?;
    page.set_metrics(width, scale).await?;
    page.load().await?;
    let height = page.measure().await?;
    let png = page.screenshot(width, height.min(max_height)).await?;
    let (console, omitted) = page.console();
    page.close().await;
    Ok(Rendered {
        png,
        height,
        console,
        omitted,
    })
}

/// Loads the page once and measures it at each width.
pub(crate) async fn measure(
    cdp: &Cdp,
    document: &str,
    widths: &[u32],
    mode: VisualMode,
) -> Result<Vec<VisualHeight>, RenderError> {
    let Some(&first) = widths.first() else {
        return Ok(Vec::new());
    };
    let page = Page::open(cdp, document, mode).await?;
    page.set_metrics(first, 1).await?;
    page.load().await?;
    let mut heights = Vec::with_capacity(widths.len());
    for &width in widths {
        page.set_metrics(width, 1).await?;
        // A failed wait leaves the old layout; the measure still runs.
        let _ = page.evaluate(FRAMES_EXPRESSION, true).await;
        heights.push(VisualHeight {
            width,
            height: page.measure().await?,
        });
    }
    page.close().await;
    Ok(heights)
}

/// What the event pump of a page shares with the render.
#[derive(Debug, Default)]
struct PageState {
    /// The sequence number of the last event that ended a load of the main
    /// frame: `Page.loadEventFired`, or `Page.frameStoppedLoading` of the
    /// main frame. A script that navigates while the page parses stops the
    /// load, and then only the second one comes.
    loaded: u64,
    /// Why the page is gone, once it is.
    gone: Option<String>,
}

/// One page target with its session and its event pump. Dropping it closes
/// the target, even when a timeout cut the render off.
struct Page {
    cdp: Cdp,
    target_id: String,
    session: Option<String>,
    state: watch::Receiver<PageState>,
    console: Arc<Mutex<Console>>,
    pump: Option<JoinHandle<()>>,
    closed: bool,
}

impl Page {
    /// Makes a target, attaches to it, and prepares it for the document.
    async fn open(cdp: &Cdp, document: &str, mode: VisualMode) -> Result<Self, RenderError> {
        let created = cdp
            .call("Target.createTarget", json!({ "url": "about:blank" }), None)
            .await?;
        let target_id = string_field(&created, "targetId", "Target.createTarget")?;
        let (state_tx, state) = watch::channel(PageState::default());
        let mut page = Self {
            cdp: cdp.clone(),
            target_id: target_id.clone(),
            session: None,
            state,
            console: Arc::new(Mutex::new(Console::default())),
            pump: None,
            closed: false,
        };
        let attached = cdp
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session = string_field(&attached, "sessionId", "Target.attachToTarget")?;
        let events = cdp.events(&session);
        page.session = Some(session.clone());
        let body = base64::engine::general_purpose::STANDARD.encode(document.as_bytes());
        page.pump = Some(tokio::spawn(pump(
            cdp.clone(),
            session,
            // The ID of a page target is the ID of its main frame.
            target_id,
            body,
            events,
            page.console.clone(),
            state_tx,
        )));

        page.call("Page.enable", json!({})).await?;
        page.call("Runtime.enable", json!({})).await?;
        page.call("Log.enable", json!({})).await?;
        page.call(
            "Emulation.setEmulatedMedia",
            json!({ "features": [{ "name": "prefers-color-scheme", "value": mode.as_str() }] }),
        )
        .await?;
        page.call(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": NO_WEBRTC, "runImmediately": true }),
        )
        .await?;
        page.call(
            "Fetch.enable",
            json!({ "patterns": [
                { "urlPattern": format!("{PAGE_ORIGIN}/*") },
                { "urlPattern": "*", "resourceType": "Document" },
            ] }),
        )
        .await?;
        Ok(page)
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RenderError> {
        Ok(self
            .cdp
            .call(method, params, self.session.as_deref())
            .await?)
    }

    async fn set_metrics(&self, width: u32, scale: u8) -> Result<(), RenderError> {
        self.call(
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": width,
                "height": VIEWPORT_HEIGHT,
                "deviceScaleFactor": scale,
                "mobile": false,
            }),
        )
        .await?;
        Ok(())
    }

    /// Navigates to the page, waits for the end of its load, then for its
    /// fonts and two animation frames.
    async fn load(&self) -> Result<(), RenderError> {
        let mut state = self.state.clone();
        let (navigated, committed) = self
            .cdp
            .call_ordered(
                "Page.navigate",
                json!({ "url": PAGE_URL }),
                self.session.as_deref(),
            )
            .await?;
        if let Some(error) = navigated.get("errorText").and_then(Value::as_str)
            && !error.is_empty()
        {
            return Err(RenderError::PageNotLoaded(error.to_owned()));
        }
        // The reply comes when the navigation commits, and the end of the
        // load comes after it. An older event, such as the end of the load
        // of `about:blank`, has a smaller number.
        // The guard of the state ends with this statement, so the future
        // stays `Send`.
        let gone = state
            .wait_for(|s| s.loaded > committed || s.gone.is_some())
            .await
            .map_err(|_| RenderError::Closed("the page events stopped".to_owned()))?
            .gone
            .clone();
        if let Some(reason) = gone {
            if self.cdp.is_closed() {
                return Err(RenderError::Closed(reason));
            }
            return Err(RenderError::PageNotLoaded(reason));
        }
        // A page that breaks the fonts or the animation frames still
        // renders, a little earlier.
        if let Err(err) = self.evaluate(SETTLE_EXPRESSION, true).await {
            tracing::debug!(error = %err, "the visual did not settle");
        }
        Ok(())
    }

    /// Runs an expression in the page and returns its value.
    async fn evaluate(&self, expression: &str, await_promise: bool) -> Result<Value, RenderError> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": await_promise,
                    "returnByValue": true,
                }),
            )
            .await?;
        if let Some(details) = result.get("exceptionDetails") {
            let text = details
                .pointer("/exception/description")
                .or_else(|| details.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("a script error");
            return Err(RenderError::PageNotLoaded(
                text.replace(PAGE_URL, PAGE_NAME),
            ));
        }
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// The height of the content, clamped to `1..=VISUAL_FIT_MAX`.
    async fn measure(&self) -> Result<u32, RenderError> {
        let value = self.evaluate(MEASURE_EXPRESSION, false).await?;
        let Some(height) = value.as_f64() else {
            return Err(RenderError::PageNotLoaded(
                "the height of the page could not be read".to_owned(),
            ));
        };
        Ok(clamp_height(height))
    }

    async fn screenshot(&self, width: u32, height: u32) -> Result<Vec<u8>, RenderError> {
        let shot = self
            .call(
                "Page.captureScreenshot",
                json!({
                    "format": "png",
                    "captureBeyondViewport": true,
                    "clip": { "x": 0, "y": 0, "width": width, "height": height, "scale": 1 },
                }),
            )
            .await?;
        let data = string_field(&shot, "data", "Page.captureScreenshot")?;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| RenderError::Step {
                method: "Page.captureScreenshot".to_owned(),
                message: format!("the image is not base64: {e}"),
            })
    }

    fn console(&self) -> (Vec<VisualConsoleMessage>, u32) {
        let console = match self.console.lock() {
            Ok(c) => c,
            Err(p) => p.into_inner(),
        };
        (console.messages.clone(), console.omitted)
    }

    /// Closes the target and waits for the reply.
    async fn close(mut self) {
        self.closed = true;
        self.stop_events();
        let _ = self
            .cdp
            .call(
                "Target.closeTarget",
                json!({ "targetId": self.target_id }),
                None,
            )
            .await;
    }

    fn stop_events(&mut self) {
        if let Some(pump) = self.pump.take() {
            pump.abort();
        }
        if let Some(session) = &self.session {
            self.cdp.forget(session);
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        self.stop_events();
        if self.closed {
            return;
        }
        // A render that failed or timed out leaves its target. Close it in
        // the background.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let cdp = self.cdp.clone();
            let target_id = std::mem::take(&mut self.target_id);
            runtime.spawn(async move {
                let _ = cdp
                    .call("Target.closeTarget", json!({ "targetId": target_id }), None)
                    .await;
            });
        }
    }
}

/// Handles the events of one page: answers paused requests, keeps console
/// messages, and notes the end of each load.
async fn pump(
    cdp: Cdp,
    session: String,
    main_frame: String,
    body: String,
    mut events: mpsc::UnboundedReceiver<Event>,
    console: Arc<Mutex<Console>>,
    state: watch::Sender<PageState>,
) {
    while let Some(event) = events.recv().await {
        match event.method.as_str() {
            "Page.loadEventFired" => state.send_modify(|s| s.loaded = s.loaded.max(event.seq)),
            "Page.frameStoppedLoading"
                if event.params.get("frameId").and_then(Value::as_str)
                    == Some(main_frame.as_str()) =>
            {
                state.send_modify(|s| s.loaded = s.loaded.max(event.seq));
            }
            "Fetch.requestPaused" => {
                if let Err(err) =
                    answer_paused(&cdp, &session, &main_frame, &body, &event.params).await
                {
                    tracing::debug!(error = %err, "could not answer a paused request of a visual");
                }
            }
            "Runtime.consoleAPICalled" | "Runtime.exceptionThrown" | "Log.entryAdded" => {
                if let Some(message) = console_message(&event.method, &event.params) {
                    let mut console = match console.lock() {
                        Ok(c) => c,
                        Err(p) => p.into_inner(),
                    };
                    console.push(message);
                }
            }
            "Target.detachedFromTarget" => {
                state.send_modify(|s| {
                    s.gone.get_or_insert_with(|| "the page closed".to_owned());
                });
            }
            "Inspector.targetCrashed" => {
                state.send_modify(|s| {
                    s.gone.get_or_insert_with(|| "the page crashed".to_owned());
                });
            }
            _ => {}
        }
    }
    state.send_modify(|s| {
        s.gone
            .get_or_insert_with(|| "the connection to the browser closed".to_owned());
    });
}

/// What to do with a paused request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Paused {
    /// Answer with the document.
    Fulfill,
    /// Let it go to the network, through the proxy.
    Continue,
    /// Answer with an empty 204. A navigation that gets 204 stops, and the
    /// frame keeps its document. A failed navigation would show the error
    /// page of the browser in place of the visual.
    Stay,
    /// Fail it with `AccessDenied`.
    Fail,
}

/// Decides a paused request. The page itself gets the document. A document
/// of another site in a subframe may load, as any embed would. Any other
/// document stays where it is: a navigation of the main frame, or a
/// subframe on the page origin. Any other request on the page origin
/// fails.
pub(crate) fn decide(
    url: &str,
    resource_type: Option<&str>,
    frame_id: Option<&str>,
    main_frame: &str,
) -> Paused {
    let url = url.split('#').next().unwrap_or(url);
    if url == PAGE_URL {
        return Paused::Fulfill;
    }
    let on_page_origin = url.starts_with(&format!("{PAGE_ORIGIN}/")) || url == PAGE_ORIGIN;
    let subframe = frame_id.is_some_and(|f| f != main_frame);
    match resource_type {
        Some("Document") if subframe && !on_page_origin => Paused::Continue,
        Some("Document") => Paused::Stay,
        _ => Paused::Fail,
    }
}

async fn answer_paused(
    cdp: &Cdp,
    session: &str,
    main_frame: &str,
    body: &str,
    params: &Value,
) -> Result<(), CdpError> {
    let Some(request_id) = params.get("requestId").and_then(Value::as_str) else {
        return Ok(());
    };
    let url = params
        .pointer("/request/url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let resource_type = params.get("resourceType").and_then(Value::as_str);
    let frame_id = params.get("frameId").and_then(Value::as_str);
    let (method, params) = match decide(url, resource_type, frame_id, main_frame) {
        Paused::Fulfill => (
            "Fetch.fulfillRequest",
            json!({
                "requestId": request_id,
                "responseCode": 200,
                "responseHeaders": [{ "name": "Content-Type", "value": "text/html; charset=utf-8" }],
                "body": body,
            }),
        ),
        Paused::Continue => ("Fetch.continueRequest", json!({ "requestId": request_id })),
        Paused::Stay => (
            "Fetch.fulfillRequest",
            json!({ "requestId": request_id, "responseCode": 204, "body": "" }),
        ),
        Paused::Fail => (
            "Fetch.failRequest",
            json!({ "requestId": request_id, "errorReason": "AccessDenied" }),
        ),
    };
    cdp.call(method, params, Some(session)).await.map(|_| ())
}

/// The console messages and page errors of one page.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Console {
    /// At most `VISUAL_CONSOLE_MAX_MESSAGES`.
    pub messages: Vec<VisualConsoleMessage>,
    /// How many more came.
    pub omitted: u32,
}

impl Console {
    /// Keeps a message, cut to `VISUAL_CONSOLE_MAX_CHARS`, or counts it when
    /// the list is full.
    ///
    /// The address of the page becomes its file name, and an address on
    /// the page origin becomes its path: `chart.png` for a relative
    /// reference that did not load.
    pub(crate) fn push(&mut self, mut message: VisualConsoleMessage) {
        if self.messages.len() >= VISUAL_CONSOLE_MAX_MESSAGES {
            self.omitted = self.omitted.saturating_add(1);
            return;
        }
        let text = message
            .text
            .replace(PAGE_URL, PAGE_NAME)
            .replace(&format!("{PAGE_ORIGIN}/"), "");
        message.text = cut(&text, VISUAL_CONSOLE_MAX_CHARS);
        self.messages.push(message);
    }
}

/// Cuts text to at most `max` characters. Cut text ends with "…".
pub(crate) fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The console message of a `Runtime.consoleAPICalled`, a
/// `Runtime.exceptionThrown` or a `Log.entryAdded` event.
///
/// A log entry of the level `verbose` is left out: it is a hint of the
/// browser, not a fault of the page, and it would fill the list.
pub(crate) fn console_message(method: &str, params: &Value) -> Option<VisualConsoleMessage> {
    match method {
        "Log.entryAdded" => {
            let entry = params.get("entry")?;
            let level = match entry.get("level").and_then(Value::as_str)? {
                "verbose" => return None,
                "info" => "info",
                "warning" => "warning",
                "error" => "error",
                _ => "log",
            };
            let text = entry.get("text").and_then(Value::as_str).unwrap_or("");
            // A network error names no address in its text.
            let text = match entry.get("url").and_then(Value::as_str) {
                Some(url) if !url.is_empty() && !text.contains(url) => format!("{text} ({url})"),
                _ => text.to_owned(),
            };
            Some(VisualConsoleMessage {
                level: level.to_owned(),
                text,
            })
        }
        "Runtime.consoleAPICalled" => {
            let level = match params.get("type").and_then(Value::as_str)? {
                "log" | "debug" | "dir" | "dirxml" | "table" | "trace" => "log",
                "info" => "info",
                "warning" => "warning",
                "error" | "assert" => "error",
                _ => return None,
            };
            let text = params
                .get("args")
                .and_then(Value::as_array)
                .map(|args| args.iter().map(remote_text).collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            Some(VisualConsoleMessage {
                level: level.to_owned(),
                text,
            })
        }
        "Runtime.exceptionThrown" => {
            let details = params.get("exceptionDetails")?;
            let text = details
                .pointer("/exception/description")
                .or_else(|| details.get("text"))
                .and_then(Value::as_str)?;
            Some(VisualConsoleMessage {
                level: "error".to_owned(),
                text: text.to_owned(),
            })
        }
        _ => None,
    }
}

/// The text of one argument of a console call.
fn remote_text(object: &Value) -> String {
    if let Some(text) = object.get("value").and_then(Value::as_str) {
        return text.to_owned();
    }
    if let Some(description) = object.get("description").and_then(Value::as_str) {
        return description.to_owned();
    }
    if let Some(value) = object.get("value") {
        return value.to_string();
    }
    object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("undefined")
        .to_owned()
}

/// Rounds a measured height up and clamps it to `1..=VISUAL_FIT_MAX`.
pub(crate) fn clamp_height(height: f64) -> u32 {
    if height.is_nan() || height < 1.0 {
        return 1;
    }
    let max = f64::from(VISUAL_FIT_MAX);
    // In 1..=VISUAL_FIT_MAX after the clamp, so the cast is exact.
    height.ceil().min(max) as u32
}

fn string_field(value: &Value, field: &str, method: &str) -> Result<String, RenderError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| RenderError::Step {
            method: method.to_owned(),
            message: format!("the reply has no {field}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(level: &str, text: &str) -> VisualConsoleMessage {
        VisualConsoleMessage {
            level: level.to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn visual_console_keeps_twenty_and_counts_the_rest() {
        let mut console = Console::default();
        for i in 0..25 {
            console.push(message("log", &format!("line {i}")));
        }
        assert_eq!(console.messages.len(), VISUAL_CONSOLE_MAX_MESSAGES);
        assert_eq!(console.omitted, 5);
        assert_eq!(console.messages[0].text, "line 0");
        assert_eq!(console.messages[19].text, "line 19");
    }

    #[test]
    fn visual_console_cuts_long_text() {
        let mut console = Console::default();
        console.push(message("error", &"é".repeat(600)));
        let text = &console.messages[0].text;
        assert_eq!(text.chars().count(), VISUAL_CONSOLE_MAX_CHARS);
        assert!(text.ends_with('…'));
        console.push(message("log", &"a".repeat(500)));
        assert_eq!(console.messages[1].text, "a".repeat(500));
        // The page address becomes its file name.
        console.push(message(
            "error",
            &format!("Error: x\n    at {PAGE_URL}:3:9"),
        ));
        assert_eq!(console.messages[2].text, "Error: x\n    at page.html:3:9");
    }

    #[test]
    fn visual_console_reads_console_calls() {
        let params = json!({
            "type": "warning",
            "args": [
                { "type": "string", "value": "count" },
                { "type": "number", "value": 3, "description": "3" },
                { "type": "object", "subtype": "null", "value": null },
                { "type": "undefined" },
                { "type": "object", "className": "Object", "description": "Object" },
            ],
        });
        assert_eq!(
            console_message("Runtime.consoleAPICalled", &params),
            Some(message("warning", "count 3 null undefined Object"))
        );
        let debug = json!({ "type": "debug", "args": [{ "type": "string", "value": "d" }] });
        assert_eq!(
            console_message("Runtime.consoleAPICalled", &debug),
            Some(message("log", "d"))
        );
        let assert = json!({ "type": "assert", "args": [] });
        assert_eq!(
            console_message("Runtime.consoleAPICalled", &assert),
            Some(message("error", ""))
        );
        let group = json!({ "type": "startGroup", "args": [] });
        assert_eq!(console_message("Runtime.consoleAPICalled", &group), None);
    }

    #[test]
    fn visual_console_reads_page_errors() {
        let thrown = json!({ "exceptionDetails": {
            "text": "Uncaught",
            "exception": { "type": "object", "description": "Error: boom\n    at page.html:1:7" },
        } });
        assert_eq!(
            console_message("Runtime.exceptionThrown", &thrown),
            Some(message("error", "Error: boom\n    at page.html:1:7"))
        );
        let plain = json!({ "exceptionDetails": { "text": "Uncaught SyntaxError" } });
        assert_eq!(
            console_message("Runtime.exceptionThrown", &plain),
            Some(message("error", "Uncaught SyntaxError"))
        );
        assert_eq!(console_message("Page.loadEventFired", &json!({})), None);
    }

    #[test]
    fn visual_console_reads_log_entries() {
        let blocked = json!({ "entry": {
            "source": "network",
            "level": "error",
            "text": "Failed to load resource: net::ERR_ACCESS_DENIED",
            "url": format!("{PAGE_ORIGIN}/chart.png"),
            "timestamp": 1.0,
        } });
        let entry = console_message("Log.entryAdded", &blocked).unwrap();
        assert_eq!(entry.level, "error");
        assert_eq!(
            entry.text,
            format!("Failed to load resource: net::ERR_ACCESS_DENIED ({PAGE_ORIGIN}/chart.png)")
        );
        // In the list, an address on the page origin becomes its path.
        let mut console = Console::default();
        console.push(entry);
        assert_eq!(
            console.messages[0].text,
            "Failed to load resource: net::ERR_ACCESS_DENIED (chart.png)"
        );

        let refused = json!({ "entry": {
            "source": "network",
            "level": "error",
            "text": "Failed to load resource: net::ERR_SOCKS_CONNECTION_FAILED",
            "url": "http://10.0.0.1/data.json",
        } });
        assert_eq!(
            console_message("Log.entryAdded", &refused).unwrap().text,
            "Failed to load resource: net::ERR_SOCKS_CONNECTION_FAILED (http://10.0.0.1/data.json)"
        );
        // A text that names its address keeps it once.
        let deprecation = json!({ "entry": {
            "source": "deprecation",
            "level": "warning",
            "text": "Something at https://example.com/a.js is old",
            "url": "https://example.com/a.js",
        } });
        assert_eq!(
            console_message("Log.entryAdded", &deprecation),
            Some(message(
                "warning",
                "Something at https://example.com/a.js is old"
            ))
        );
        let info = json!({ "entry": { "source": "other", "level": "info", "text": "note" } });
        assert_eq!(
            console_message("Log.entryAdded", &info),
            Some(message("info", "note"))
        );
        let verbose = json!({ "entry": { "source": "violation", "level": "verbose", "text": "[Violation] slow" } });
        assert_eq!(console_message("Log.entryAdded", &verbose), None);
        assert_eq!(console_message("Log.entryAdded", &json!({})), None);
    }

    #[test]
    fn visual_render_decides_paused_requests() {
        let main = "MAIN";
        assert_eq!(
            decide(PAGE_URL, Some("Document"), Some(main), main),
            Paused::Fulfill
        );
        assert_eq!(
            decide(
                &format!("{PAGE_URL}#dark"),
                Some("Document"),
                Some(main),
                main
            ),
            Paused::Fulfill
        );
        // The main frame stays on the page.
        assert_eq!(
            decide("https://example.com/", Some("Document"), Some(main), main),
            Paused::Stay
        );
        assert_eq!(
            decide(
                &format!("{PAGE_ORIGIN}/other.html"),
                Some("Document"),
                Some(main),
                main
            ),
            Paused::Stay
        );
        // A subframe may show another site, but not another address on the
        // page origin.
        assert_eq!(
            decide(
                "https://example.com/embed",
                Some("Document"),
                Some("SUB"),
                main
            ),
            Paused::Continue
        );
        assert_eq!(
            decide(
                &format!("{PAGE_ORIGIN}/other.html"),
                Some("Document"),
                Some("SUB"),
                main
            ),
            Paused::Stay
        );
        assert_eq!(
            decide(
                &format!("{PAGE_ORIGIN}/image.png"),
                Some("Image"),
                Some(main),
                main
            ),
            Paused::Fail
        );
    }

    #[test]
    fn visual_render_clamps_heights() {
        assert_eq!(clamp_height(0.0), 1);
        assert_eq!(clamp_height(-5.0), 1);
        assert_eq!(clamp_height(f64::NAN), 1);
        assert_eq!(clamp_height(f64::INFINITY), VISUAL_FIT_MAX);
        assert_eq!(clamp_height(300.0), 300);
        assert_eq!(clamp_height(300.2), 301);
        assert_eq!(clamp_height(25_000.0), VISUAL_FIT_MAX);
    }

    #[test]
    fn visual_render_cuts_by_characters() {
        assert_eq!(cut("abc", 3), "abc");
        assert_eq!(cut("abcd", 3), "ab…");
        assert_eq!(cut("", 3), "");
    }
}
