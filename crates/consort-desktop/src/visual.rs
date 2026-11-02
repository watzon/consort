//! Visuals in the desktop app (docs/VISUALS-PLAN.md, "Desktop").
//!
//! A visual draws in three places: the timeline, a task thread and the
//! light box. In each frame the cover rule of `ui/visual_cover.rs` says
//! whether it is live (macOS only), an image, or nothing.
//!
//! This module keeps:
//! - for each visual: its page (fetched once from the hub), the images that
//!   the app asked for at each width, the failures, and the height that the
//!   live page reported;
//! - the frame record. A probe in each item writes where the item drew and
//!   what the cover rule said, while GPUI prepaints the frame. So the rule
//!   reads the bounds of this frame, and the probe shows or hides the live
//!   page before the page takes its place. A sweep at the end of the frame
//!   hides each page whose item did not draw, because a native view stays
//!   where it was when its element is gone;
//! - the live pages: at most [`PAGES_MAX`]. A page goes away
//!   [`DROP_AFTER`] after its item left the screen.
//!
//! The views are in `ui/visual_item.rs` and `ui/lightbox.rs`.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use consort_client::{AttachmentView, Client, ClientError, VisualImageFile, VisualView};
use consort_proto::model::{
    VISUAL_RENDER_WIDTH_MAX, VISUAL_RENDER_WIDTH_MIN, VISUAL_SCALE_MAX, VisualMode, VisualNotice,
    VisualStyle,
};
use consort_theme::Tokens;
use consort_visual::Host;
use gpui_kit::{
    App, AsyncApp, Bounds, ClipboardEntry, ClipboardItem, Context, ImageFormat, IntoElement,
    Pixels, Styled, Window, canvas,
};

use crate::{
    app::ConsortApp,
    ui::{
        lightbox::LightboxItem,
        menu::{MenuAction, MenuItem},
        visual_cover::{Presence, TimelineArea, visual_presence},
    },
};

/// The most live pages at once. Each one is a WebContent process.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const PAGES_MAX: usize = 4;
/// How long a live page stays after its item left the screen.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const DROP_AFTER: Duration = Duration::from_secs(30);
/// The failed tries in a row of an error that passes by itself, before the
/// item says that the visual did not load. The app goes on trying.
pub const SHOW_FAILED_AFTER: u32 = 3;
/// The wait before the first new try after an error that passes by itself.
/// Each next wait is twice as long, up to [`RETRY_MAX`].
const RETRY_FIRST: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(30);
/// A new width waits this long before the app asks for an image at it, so a
/// resize of the window asks once and not on each frame.
pub const SETTLE: Duration = Duration::from_millis(300);
/// The least time between two links that a page opens.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const OPEN_GAP: Duration = Duration::from_secs(1);

/// The script that asks a live page for its height at the width that it
/// has now, two frames after a move. The size reporter of the bootstrap
/// sends a height only when it changes, so a page that moves to a new width
/// with the same height would not say it again.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn size_script() -> String {
    format!(
        "requestAnimationFrame(function(){{requestAnimationFrame(function(){{try{{window.ipc.postMessage(JSON.stringify({{type:\"size\",height:Math.ceil({})}}))}}catch(_){{}}}})}});",
        consort_visual::MEASURE_EXPRESSION
    )
}

/// The text of the loading state (Paper G3).
pub const LOADING_TEXT: &str = "Loading the visual";
/// The title of the failed state (Paper G3).
pub const FAILED_TITLE: &str = "The visual did not load";
/// What the failed state says when the page did not come (Paper G3).
pub const PAGE_FAILED: &str =
    "The hub did not send the page. Check that the hub is online, then try again.";

/// Where a visual draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    Timeline,
    Thread,
    Lightbox,
}

impl Place {
    /// The part of the element IDs and debug selectors of this place.
    pub fn key(self) -> &'static str {
        match self {
            Place::Timeline => "timeline",
            Place::Thread => "thread",
            Place::Lightbox => "lightbox",
        }
    }
}

/// The height that the live page reported, and the width that it had then,
/// in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveHeight {
    pub width: u32,
    pub height: u32,
}

/// The height of the page of a visual at `width` CSS pixels:
///
/// 1. the height that the live page reported at this width;
/// 2. else the height of the image that shows, scaled to this width;
/// 3. else the height table of the notice, and then the height that the
///    assistant gave.
pub fn page_height(
    width: u32,
    live: Option<LiveHeight>,
    image: Option<&VisualImageFile>,
    notice: &VisualNotice,
) -> u32 {
    if let Some(live) = live.filter(|l| l.width == width && l.height > 0) {
        return live.height;
    }
    if let Some(image) = image.filter(|i| i.width > 0 && i.height > 0) {
        return scaled_height(image, width);
    }
    notice.height_at(width)
}

/// The height of an image drawn `width` CSS pixels wide, rounded. The mock
/// always answers with its image at 720 px, so the app scales it.
pub fn scaled_height(image: &VisualImageFile, width: u32) -> u32 {
    if image.width == 0 {
        return image.height;
    }
    let (h, w, iw) = (
        u64::from(image.height),
        u64::from(width),
        u64::from(image.width),
    );
    u32::try_from((h * w + iw / 2) / iw).unwrap_or(u32::MAX)
}

/// The width of a request for an image: the width of the item in whole CSS
/// pixels, in the range that the hub takes.
pub fn render_width(width: Pixels) -> u32 {
    let whole = f32::from(width).floor();
    if !whole.is_finite() || whole < VISUAL_RENDER_WIDTH_MIN as f32 {
        return VISUAL_RENDER_WIDTH_MIN;
    }
    if whole > VISUAL_RENDER_WIDTH_MAX as f32 {
        return VISUAL_RENDER_WIDTH_MAX;
    }
    whole as u32
}

/// The scale of a request for an image: the scale of the window rounded up,
/// from 1 to 3.
pub fn render_scale(scale_factor: f32) -> u8 {
    let up = scale_factor.ceil();
    if !up.is_finite() || up < 1. {
        return 1;
    }
    up.min(f32::from(VISUAL_SCALE_MAX)) as u8
}

/// Whether an error can pass by itself: no session host is online, the
/// browser for visuals is installing, the hub is not connected or did not
/// answer in time. The app tries again after a wait.
pub fn passes(err: &ClientError) -> bool {
    matches!(
        err,
        ClientError::Unavailable { .. } | ClientError::NotConnected | ClientError::TimedOut
    )
}

/// The wait before the next try after `tries` failed tries in a row.
pub fn retry_delay(tries: u32) -> Duration {
    let doublings = tries.saturating_sub(1).min(5);
    (RETRY_FIRST * 2u32.pow(doublings)).min(RETRY_MAX)
}

/// An error as a sentence: the client errors have no full stop.
pub fn sentence(message: &str) -> String {
    let message = message.trim();
    if message.ends_with(['.', '!', '?']) {
        message.to_owned()
    } else {
        format!("{message}.")
    }
}

/// A request that failed.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub message: String,
    /// The failed tries in a row.
    pub tries: u32,
    /// The error does not pass by itself, so the app does not try again
    /// until the user asks.
    pub hard: bool,
    /// When the app tries again by itself.
    pub retry_at: Option<Instant>,
}

impl Failure {
    /// The failure of try number `tries` with `err`, at `now`.
    pub fn after(err: &ClientError, tries: u32, now: Instant) -> Self {
        let hard = !passes(err);
        Self {
            message: sentence(&err.to_string()),
            tries,
            hard,
            retry_at: (!hard).then(|| now + retry_delay(tries)),
        }
    }

    /// Whether the item says that the visual did not load: at once for a
    /// hard error, after a few tries for an error that passes.
    pub fn shown(&self) -> bool {
        self.hard || self.tries >= SHOW_FAILED_AFTER
    }

    /// Whether the app tries again now.
    pub fn due(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|at| now >= at)
    }
}

/// What the page area of an item draws under the live page.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// The image of the page.
    Image(VisualImageFile),
    /// "Loading the visual": no image yet, and no failure to show.
    Loading,
    /// "The visual did not load", with what to do next.
    Failed(String),
}

/// What the page area draws. The image when there is one. Else the failure
/// of the image, once it shows: with the text of Paper G3 when the page did
/// not come either, else with the error of the image. Else the loading
/// state. A failed page alone shows no failure, because the image can still
/// come.
pub fn body(
    image: Option<&VisualImageFile>,
    image_failure: Option<&Failure>,
    page_failure: Option<&Failure>,
) -> Body {
    if let Some(image) = image {
        return Body::Image(image.clone());
    }
    match image_failure.filter(|f| f.shown()) {
        Some(failure) => Body::Failed(match page_failure.filter(|f| f.shown()) {
            Some(_) => PAGE_FAILED.to_owned(),
            None => failure.message.clone(),
        }),
        None => Body::Loading,
    }
}

/// Whether the app takes a height from the page: only while the page shows
/// and has a width. A hidden page first reports its height at width 0.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn accept_size(shown: bool, width: Pixels) -> bool {
    shown && width > Pixels::ZERO
}

/// Whether a page may open a link now: at most one each [`OPEN_GAP`].
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn may_open(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|at| now.duration_since(at) >= OPEN_GAP)
}

/// The file name of the image of a visual: `Read speed on a 10 GB file.png`.
pub fn image_name(title: &str) -> String {
    let page = consort_visual::page_name(title);
    let stem = page.strip_suffix(".html").unwrap_or(&page);
    format!("{stem}.png")
}

/// The document of Open in browser and Save as HTML: the page with the
/// theme of the app, the fonts and the ink kit, and no script of the app.
pub fn browser_document(page: &str, style: &VisualStyle) -> Result<String, ClientError> {
    consort_visual::document(page, style, Host::Renderer).map_err(|err| {
        ClientError::Other(format!(
            "Consort could not prepare the page of the visual: {err}. Try again"
        ))
    })
}

/// Where Open in browser writes the document: next to the page in the
/// cache of the client, `browser/<page hash>/<page name>`.
pub fn browser_file(page_path: &Path, sha256: &str, title: &str) -> PathBuf {
    page_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
        .join("browser")
        .join(sha256)
        .join(consort_visual::page_name(title))
}

/// The `file:` URL of a path, with each byte that a URL path cannot hold
/// encoded: `file:///tmp/Read%20speed.html`. A Windows path gets forward
/// slashes and a slash before its drive.
pub fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut url = String::from("file://");
    if !text.starts_with('/') {
        url.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                url.push(char::from(byte))
            }
            other => url.push_str(&format!("%{other:02X}")),
        }
    }
    url
}

/// The style of the pages and images: the colors of the active theme in
/// its mode. With no theme library (some tests), the built-in Spruce.
pub fn current_style(t: &Tokens, cx: &App) -> VisualStyle {
    let mode = if t.mode.is_dark() {
        VisualMode::Dark
    } else {
        VisualMode::Light
    };
    cx.try_global::<consort_theme::ThemeLibrary>()
        .and_then(|library| library.active())
        .map(|theme| consort_visual::theme_style(theme.file.mode(t.mode), mode))
        .unwrap_or_else(|| consort_visual::default_style(mode))
}

/// The menu of a visual (Paper G2): Open full screen, Open in browser, then
/// Copy image, Save image and Save as HTML. The light box shows the visual
/// full screen already, so it has no Open full screen.
pub fn visual_menu(view: &VisualView, in_lightbox: bool) -> Vec<MenuItem> {
    let view = Box::new(view.clone());
    let item = |label: &str, action: VisualAction| MenuItem::new(label, MenuAction::Visual(action));
    let mut items = Vec::new();
    if !in_lightbox {
        items.push(item(
            "Open full screen",
            VisualAction::OpenFullScreen(view.clone()),
        ));
    }
    items.push(item(
        "Open in browser",
        VisualAction::OpenInBrowser(view.clone()),
    ));
    items.push(item("Copy image", VisualAction::CopyImage(view.clone())).separated());
    items.push(item("Save image", VisualAction::SaveImage(view.clone())));
    items.push(item("Save as HTML", VisualAction::SaveHtml(view)));
    items
}

/// What a menu item or a button of a visual does.
#[derive(Clone, Debug, PartialEq)]
pub enum VisualAction {
    /// The light box with the visual live on a sheet.
    OpenFullScreen(Box<VisualView>),
    /// The page with the theme in the system browser.
    OpenInBrowser(Box<VisualView>),
    /// The image on the clipboard.
    CopyImage(Box<VisualView>),
    /// The save dialog, then the image there.
    SaveImage(Box<VisualView>),
    /// The save dialog, then the page with the theme there.
    SaveHtml(Box<VisualView>),
}

/// A live page of a visual. Only macOS has one.
#[cfg(target_os = "macos")]
pub type PageHandle = gpui_kit::Entity<crate::ui::live_page::LivePage>;
/// A live page of a visual. Only macOS has one, so no value exists here.
#[cfg(not(target_os = "macos"))]
pub type PageHandle = std::convert::Infallible;

/// Shows or hides a live page.
fn set_page_shown(page: &PageHandle, shown: bool, cx: &mut App) {
    #[cfg(target_os = "macos")]
    page.update(cx, |page, cx| {
        if shown {
            page.show(cx);
        } else {
            page.hide(cx);
        }
    });
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (shown, cx);
        match *page {}
    }
}

/// The element of a live page, which fills its parent.
pub fn page_element(page: Option<PageHandle>) -> Option<gpui_kit::AnyElement> {
    #[cfg(target_os = "macos")]
    {
        use gpui_kit::ParentElement as _;
        page.map(|page| {
            gpui_kit::div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(page)
                .into_any_element()
        })
    }
    #[cfg(not(target_os = "macos"))]
    page.map(|page| match page {})
}

/// Where an item drew in a frame, and what the cover rule said.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    /// The page area of the item, in window coordinates.
    pub bounds: Bounds<Pixels>,
    pub presence: Presence,
    /// The frame.
    pub serial: u64,
}

/// What the probes write while GPUI prepaints a frame, and what the next
/// frame reads.
#[derive(Default)]
pub struct FrameRecord {
    /// Counts the frames. Each render of the app starts one.
    serial: Cell<u64>,
    /// The area of each place in the last frame that drew it.
    areas: RefCell<HashMap<Place, TimelineArea>>,
    /// Each item that drew, by place and message ID.
    seen: RefCell<HashMap<(Place, String), Seen>>,
    /// When each visual was last on screen, by message ID.
    on_screen: RefCell<HashMap<String, Instant>>,
    /// The visuals whose page shows in this frame.
    claimed: RefCell<HashSet<String>>,
    /// The visuals whose page showed in the last frame.
    last_claimed: RefCell<HashSet<String>>,
    /// The live pages, for the sweep.
    pages: RefCell<Vec<(String, PageHandle)>>,
}

impl FrameRecord {
    pub fn serial(&self) -> u64 {
        self.serial.get()
    }

    /// Where the item of `message_id` drew in `place` in the last frame
    /// that drew it.
    pub fn seen(&self, place: Place, message_id: &str) -> Option<Seen> {
        self.seen
            .borrow()
            .get(&(place, message_id.to_owned()))
            .copied()
    }

    /// What the cover rule said for the item in the frame that drew last.
    /// An item that did not draw in it is not on screen.
    #[cfg(test)]
    pub fn presence(&self, place: Place, message_id: &str) -> Presence {
        match self.seen(place, message_id) {
            Some(seen) if seen.serial == self.serial.get() => seen.presence,
            _ => Presence::Hidden,
        }
    }

    /// The area of a place in the last frame that drew it.
    pub fn area(&self, place: Place) -> Option<TimelineArea> {
        self.areas.borrow().get(&place).copied()
    }

    /// Whether the page of a visual showed in the last frame.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn was_claimed(&self, message_id: &str) -> bool {
        self.last_claimed.borrow().contains(message_id)
    }

    /// An element that fills its parent and records the area of `place`
    /// from its bounds. It must come before the items of the place, so it
    /// prepaints first.
    pub fn area_probe(
        self: &Rc<Self>,
        place: Place,
        area: impl Fn(Bounds<Pixels>) -> TimelineArea + 'static,
    ) -> gpui_kit::AnyElement {
        let frame = self.clone();
        canvas(
            move |bounds, _, _| {
                frame.areas.borrow_mut().insert(place, area(bounds));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }

    /// An element that fills the page area of an item. When GPUI prepaints
    /// it, it applies the cover rule with the bounds of this frame, records
    /// the answer, and shows or hides `page`. It must come before the
    /// element of the page, so the page takes its place only when it shows.
    /// When the answer or the width changed, the app draws again, so it can
    /// ask for an image or make the page.
    pub fn probe(
        self: &Rc<Self>,
        place: Place,
        message_id: String,
        covered: bool,
        window_active: bool,
        page: Option<PageHandle>,
    ) -> gpui_kit::AnyElement {
        let frame = self.clone();
        canvas(
            move |bounds, window, cx| {
                let presence = match frame.area(place) {
                    Some(area) => visual_presence(
                        bounds,
                        area,
                        covered,
                        window_active,
                        area.fits(bounds.size.height),
                    ),
                    // The item draws, so it is on screen.
                    None => Presence::Image,
                };
                let serial = frame.serial.get();
                let before = frame.seen.borrow_mut().insert(
                    (place, message_id.clone()),
                    Seen {
                        bounds,
                        presence,
                        serial,
                    },
                );
                if presence != Presence::Hidden {
                    frame
                        .on_screen
                        .borrow_mut()
                        .insert(message_id.clone(), Instant::now());
                }
                if let Some(page) = &page {
                    let live = presence == Presence::Live;
                    set_page_shown(page, live, cx);
                    if live {
                        frame.claimed.borrow_mut().insert(message_id.clone());
                    }
                }
                let changed = before.is_none_or(|b| {
                    b.presence != presence
                        || b.serial + 1 < serial
                        || render_width(b.bounds.size.width) != render_width(bounds.size.width)
                });
                if changed {
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }

    /// The last element of the window. It hides each live page that no item
    /// showed in this frame, such as the page of an item that scrolled out
    /// of a list.
    pub fn sweep(self: &Rc<Self>) -> gpui_kit::AnyElement {
        let frame = self.clone();
        canvas(
            move |_, _, cx| {
                let claimed = std::mem::take(&mut *frame.claimed.borrow_mut());
                let pages = frame.pages.borrow().clone();
                for (id, page) in &pages {
                    if !claimed.contains(id) {
                        set_page_shown(page, false, cx);
                    }
                }
                *frame.last_claimed.borrow_mut() = claimed;
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_0()
        .into_any_element()
    }
}

/// An image that the app asked for: its width, its scale and its style.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ImageKey {
    width: u32,
    scale: u8,
    /// The style, by its number in [`VisualUi::style_id`].
    style: u64,
}

#[derive(Clone, Debug)]
enum ImageLoad {
    Asking,
    Ready(VisualImageFile),
    Failed(Failure),
}

#[derive(Clone, Debug, Default)]
enum PageLoad {
    #[default]
    None,
    Fetching,
    /// The HTML of the page, as the hub stored it.
    Ready(Rc<str>),
}

/// A width of a place, and since when it is that width. `None` once the app
/// asked for an image at it.
#[derive(Clone, Copy, Debug)]
struct Settle {
    width: u32,
    since: Option<Instant>,
}

/// What the app knows of one visual.
#[derive(Default)]
struct VisualState {
    page: PageLoad,
    page_failure: Option<Failure>,
    images: HashMap<ImageKey, ImageLoad>,
    /// The newest image, and the style it has. It shows while the image at
    /// a new width comes.
    latest: Option<(u64, VisualImageFile)>,
    live: Option<LiveHeight>,
    widths: HashMap<Place, Settle>,
    /// The width at which the app last asked the live page for its height.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    size_asked: Option<u32>,
}

/// What a page area shows: the image, the failures and the live height.
#[derive(Clone, Debug, Default)]
pub struct Shown {
    pub image: Option<VisualImageFile>,
    pub image_failure: Option<Failure>,
    pub page_failure: Option<Failure>,
    pub live: Option<LiveHeight>,
}

/// One live page.
#[cfg(target_os = "macos")]
struct PageSlot {
    view: VisualView,
    page: PageHandle,
    /// The style that the page loaded with.
    style: u64,
    created: Instant,
    _events: gpui_kit::Subscription,
}

/// Everything the app keeps about visuals.
pub struct VisualUi {
    /// The app makes live pages: on macOS, outside of tests, until a page
    /// cannot be made.
    pub live: bool,
    states: HashMap<String, VisualState>,
    pub frame: Rc<FrameRecord>,
    style: VisualStyle,
    /// Counts the styles, so an image of an old theme is not used.
    style_id: u64,
    /// The scale of the window, rounded up.
    scale: u8,
    /// The item under the pointer: its footer shows.
    pub hovered: Option<(Place, String)>,
    /// When a page last opened a link.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    last_open: Option<Instant>,
    /// A timer waits to drop a live page.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    drop_timer: bool,
    /// Counts the requests for images, for the tests.
    pub image_asks: u64,
    #[cfg(target_os = "macos")]
    pages: Vec<PageSlot>,
}

impl Default for VisualUi {
    fn default() -> Self {
        Self {
            live: cfg!(target_os = "macos") && !cfg!(test),
            states: HashMap::new(),
            frame: Rc::default(),
            style: VisualStyle::default(),
            style_id: 0,
            scale: 1,
            hovered: None,
            last_open: None,
            drop_timer: false,
            image_asks: 0,
            #[cfg(target_os = "macos")]
            pages: Vec::new(),
        }
    }
}

impl VisualUi {
    /// The width of the page area of an item in the last frame that drew
    /// it.
    pub fn width(&self, place: Place, message_id: &str) -> Option<Pixels> {
        self.frame
            .seen(place, message_id)
            .map(|seen| seen.bounds.size.width)
            .filter(|w| *w > Pixels::ZERO)
    }

    /// What the page area of a visual shows at `width` CSS pixels.
    pub fn shown(&self, message_id: &str, width: u32) -> Shown {
        let Some(state) = self.states.get(message_id) else {
            return Shown::default();
        };
        let key = self.key(width);
        let (exact, image_failure) = match state.images.get(&key) {
            Some(ImageLoad::Ready(file)) => (Some(file.clone()), None),
            Some(ImageLoad::Failed(failure)) => (None, Some(failure.clone())),
            _ => (None, None),
        };
        let image = exact.or_else(|| {
            state
                .latest
                .as_ref()
                .filter(|(style, _)| *style == self.style_id)
                .map(|(_, file)| file.clone())
        });
        Shown {
            image,
            image_failure,
            page_failure: state.page_failure.clone(),
            live: state.live,
        }
    }

    /// The HTML of the page, once the app has it.
    pub fn html(&self, message_id: &str) -> Option<Rc<str>> {
        match self.states.get(message_id).map(|s| &s.page) {
            Some(PageLoad::Ready(html)) => Some(html.clone()),
            _ => None,
        }
    }

    fn key(&self, width: u32) -> ImageKey {
        ImageKey {
            width,
            scale: self.scale,
            style: self.style_id,
        }
    }

    /// Takes a new style. Returns whether it changed.
    fn set_style(&mut self, style: VisualStyle) -> bool {
        if style == self.style {
            return false;
        }
        self.style = style;
        self.style_id += 1;
        true
    }

    /// Records the answer to a request for an image. Returns when to try
    /// again, for an error that passes by itself.
    fn finish_image(
        &mut self,
        message_id: &str,
        key: ImageKey,
        result: Result<VisualImageFile, ClientError>,
        tries: u32,
        now: Instant,
    ) -> Option<Instant> {
        let state = self.states.entry(message_id.to_owned()).or_default();
        match result {
            Ok(file) => {
                state.latest = Some((key.style, file.clone()));
                state.images.insert(key, ImageLoad::Ready(file));
                None
            }
            Err(err) => {
                let failure = Failure::after(&err, tries + 1, now);
                let retry = failure.retry_at;
                state.images.insert(key, ImageLoad::Failed(failure));
                retry
            }
        }
    }

    /// Records the answer to a fetch of the page. Returns when to try again,
    /// for an error that passes by itself.
    fn finish_page(
        &mut self,
        message_id: &str,
        result: Result<String, ClientError>,
        tries: u32,
        now: Instant,
    ) -> Option<Instant> {
        let state = self.states.entry(message_id.to_owned()).or_default();
        match result {
            Ok(html) => {
                state.page = PageLoad::Ready(Rc::from(html));
                state.page_failure = None;
                None
            }
            Err(err) => {
                let failure = Failure::after(&err, tries + 1, now);
                let retry = failure.retry_at;
                state.page = PageLoad::None;
                state.page_failure = Some(failure);
                retry
            }
        }
    }

    /// Forgets the failures of a visual, so the next frame asks again.
    pub fn forget_failures(&mut self, message_id: &str) {
        if let Some(state) = self.states.get_mut(message_id) {
            state.page_failure = None;
            state
                .images
                .retain(|_, load| !matches!(load, ImageLoad::Failed(_)));
        }
    }

    /// Records a height that the live page reported.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn set_live_height(&mut self, message_id: &str, live: LiveHeight) {
        self.states.entry(message_id.to_owned()).or_default().live = Some(live);
    }

    /// Records a failure of the image of a visual at `width`, as if the hub
    /// had answered with `err`. The tests use it.
    #[cfg(test)]
    pub fn fail_image(&mut self, message_id: &str, width: u32, err: &ClientError, tries: u32) {
        let key = self.key(width);
        let failure = Failure::after(err, tries, Instant::now());
        self.states
            .entry(message_id.to_owned())
            .or_default()
            .images
            .insert(key, ImageLoad::Failed(failure));
    }

    /// Records a failure of the page of a visual. The tests use it.
    #[cfg(test)]
    pub fn fail_page(&mut self, message_id: &str, err: &ClientError, tries: u32) {
        let state = self.states.entry(message_id.to_owned()).or_default();
        state.page = PageLoad::None;
        state.page_failure = Some(Failure::after(err, tries, Instant::now()));
    }

    /// The newest image of a visual in the style of now, if its file is
    /// still there.
    fn ready_image(&self, message_id: &str) -> Option<VisualImageFile> {
        self.states
            .get(message_id)
            .and_then(|s| s.latest.as_ref())
            .filter(|(style, file)| *style == self.style_id && Path::new(&file.path).is_file())
            .map(|(_, file)| file.clone())
    }

    /// The width that an action asks an image at: the last width of the
    /// item in the timeline or a thread, else the width of the timeline.
    fn action_width(&self, message_id: &str, t_width: Pixels) -> u32 {
        let width = self
            .width(Place::Timeline, message_id)
            .or_else(|| self.width(Place::Thread, message_id))
            .unwrap_or(t_width);
        render_width(width)
    }
}

/// The page of a visual: the file in the cache of the client and its HTML.
/// The client fetches it from the hub when it has no copy.
async fn read_page(
    client: Client,
    page: AttachmentView,
    cx: &mut AsyncApp,
) -> Result<(PathBuf, String), ClientError> {
    let path = match page.local_path.filter(|p| Path::new(p).is_file()) {
        Some(path) => path,
        None => client.fetch_blob(page.sha256, page.size, None).await?,
    };
    let path = PathBuf::from(path);
    cx.background_executor()
        .spawn(async move {
            let html = std::fs::read_to_string(&path).map_err(|err| storage(&path, &err))?;
            Ok((path, html))
        })
        .await
}

fn storage(path: &Path, err: &std::io::Error) -> ClientError {
    ClientError::Storage {
        path: path.display().to_string(),
        message: err.to_string(),
    }
}

/// Writes a file on the background executor, with its folder.
async fn write_file(path: PathBuf, bytes: Vec<u8>, cx: &mut AsyncApp) -> Result<(), ClientError> {
    cx.background_executor()
        .spawn(async move {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|err| storage(dir, &err))?;
            }
            std::fs::write(&path, bytes).map_err(|err| storage(&path, &err))
        })
        .await
}

/// Reads a file on the background executor.
async fn read_file(path: PathBuf, cx: &mut AsyncApp) -> Result<Vec<u8>, ClientError> {
    cx.background_executor()
        .spawn(async move { std::fs::read(&path).map_err(|err| storage(&path, &err)) })
        .await
}

impl ConsortApp {
    /// The start of each frame: the next frame number, the scale of the
    /// window, the style of the theme (a change loads the live pages again
    /// and asks for new images), and the live pages that must go.
    pub fn begin_visual_frame(&mut self, t: &Tokens, window: &Window, cx: &mut Context<Self>) {
        let frame = self.visuals.frame.clone();
        frame.serial.set(frame.serial.get() + 1);
        self.visuals.scale = render_scale(window.scale_factor());
        if self.visuals.set_style(current_style(t, cx)) {
            self.reload_pages(cx);
        }
        self.drop_old_pages(cx);
    }

    /// Whether something is over a place of visuals, so a visual there shows
    /// its image and not its live page.
    pub fn covers_place(&self, t: &Tokens, place: Place) -> bool {
        match place {
            Place::Timeline => self.covers_timeline(t),
            Place::Thread => self.covers_thread(),
            Place::Lightbox => self.covers_lightbox(),
        }
    }

    /// The light box shows this visual.
    pub fn lightbox_shows(&self, message_id: &str) -> bool {
        matches!(
            self.lightbox.as_ref().and_then(|l| l.current()),
            Some(LightboxItem::Visual(view)) if view.message_id == message_id
        )
    }

    /// What an item of a visual needs in this frame, from what its probe
    /// saw in the last one. An item on screen asks for its image at its
    /// width, and on macOS for its page. A live item gets its page.
    pub fn want_visual(
        &mut self,
        view: &VisualView,
        place: Place,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = view.message_id.clone();
        let serial = self.visuals.frame.serial();
        let Some(seen) = self
            .visuals
            .frame
            .seen(place, &id)
            .filter(|s| s.serial + 1 == serial && s.presence != Presence::Hidden)
        else {
            return;
        };
        let width = render_width(seen.bounds.size.width);
        let now = Instant::now();
        let state = self.visuals.states.entry(id.clone()).or_default();
        let has_image = state.latest.is_some();
        let settle = state
            .widths
            .entry(place)
            .or_insert(Settle { width, since: None });
        if settle.width != width {
            *settle = Settle {
                width,
                since: Some(now),
            };
        }
        let wait = match settle.since {
            Some(since) if has_image && now.duration_since(since) < SETTLE => Some(since + SETTLE),
            _ => {
                settle.since = None;
                None
            }
        };
        match wait {
            Some(at) => self.wake_at(at, cx),
            None => {
                let key = self.visuals.key(width);
                self.ask_image(&id, key, cx);
            }
        }
        if self.visuals.live {
            self.want_page(view, cx);
            if seen.presence == Presence::Live {
                self.ensure_page(view, window, cx);
                self.ask_page_size(&id, place, width, cx);
            }
        }
    }

    /// Asks a live page that shows at a width with no live height for its
    /// height, once for each width.
    fn ask_page_size(
        &mut self,
        message_id: &str,
        place: Place,
        width: u32,
        cx: &mut Context<Self>,
    ) {
        #[cfg(target_os = "macos")]
        {
            let Some(state) = self.visuals.states.get(message_id) else {
                return;
            };
            if state.live.is_some_and(|l| l.width == width) || state.size_asked == Some(width) {
                return;
            }
            let Some(page) = self.visual_page(message_id, place) else {
                return;
            };
            if !page.read(cx).is_shown(cx) {
                return;
            }
            let _ = page.read(cx).run_script(&size_script(), cx);
            if let Some(state) = self.visuals.states.get_mut(message_id) {
                state.size_asked = Some(width);
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (message_id, place, width, cx);
    }

    /// Asks the client for an image, unless it is here, on its way, or
    /// failed and not due.
    fn ask_image(&mut self, message_id: &str, key: ImageKey, cx: &mut Context<Self>) {
        let now = Instant::now();
        let state = self
            .visuals
            .states
            .entry(message_id.to_owned())
            .or_default();
        let tries = match state.images.get(&key) {
            None => 0,
            Some(ImageLoad::Failed(failure)) if failure.due(now) => failure.tries,
            Some(_) => return,
        };
        state.images.insert(key.clone(), ImageLoad::Asking);
        self.visuals.image_asks += 1;
        let client = self.client.clone();
        let style = self.visuals.style.clone();
        let id = message_id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = client
                .render_visual(id.clone(), key.width, key.scale, style)
                .await;
            let _ = this.update(cx, |this, cx| {
                let retry = this
                    .visuals
                    .finish_image(&id, key, result, tries, Instant::now());
                if let Some(at) = retry {
                    this.wake_at(at, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fetches the page of a visual and reads it, once, and again after an
    /// error that passes when it is due.
    fn want_page(&mut self, view: &VisualView, cx: &mut Context<Self>) {
        let now = Instant::now();
        let state = self
            .visuals
            .states
            .entry(view.message_id.clone())
            .or_default();
        if !matches!(state.page, PageLoad::None) {
            return;
        }
        let tries = match &state.page_failure {
            None => 0,
            Some(failure) if failure.due(now) => failure.tries,
            Some(_) => return,
        };
        state.page = PageLoad::Fetching;
        let client = self.client.clone();
        let page = view.page.clone();
        let id = view.message_id.clone();
        cx.spawn(async move |this, cx| {
            let result = read_page(client, page, cx).await.map(|(_, html)| html);
            let _ = this.update(cx, |this, cx| {
                let retry = this.visuals.finish_page(&id, result, tries, Instant::now());
                if let Some(at) = retry {
                    this.wake_at(at, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Draws the app again at `at`.
    fn wake_at(&self, at: Instant, cx: &mut Context<Self>) {
        let wait = at.saturating_duration_since(Instant::now());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// Try again after "The visual did not load": the next frame asks for
    /// the page and the image again.
    pub fn visual_try_again(&mut self, message_id: &str, cx: &mut Context<Self>) {
        self.visuals.forget_failures(message_id);
        cx.notify();
    }

    /// The live page of a visual for `place`, when the page exists and this
    /// place has it. The light box has it while it shows the visual, so
    /// the item in the timeline does not.
    pub fn visual_page(&self, message_id: &str, place: Place) -> Option<PageHandle> {
        if (place == Place::Lightbox) != self.lightbox_shows(message_id) {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            self.visuals
                .pages
                .iter()
                .find(|slot| slot.view.message_id == message_id)
                .map(|slot| slot.page.clone())
        }
        #[cfg(not(target_os = "macos"))]
        None
    }

    /// Runs a menu item or a button of a visual.
    pub fn run_visual_action(
        &mut self,
        action: VisualAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            VisualAction::OpenFullScreen(view) => self.open_visual_lightbox(*view, window, cx),
            VisualAction::OpenInBrowser(view) => self.open_visual_in_browser(&view, cx),
            VisualAction::CopyImage(view) => self.copy_visual_image(&view, cx),
            VisualAction::SaveImage(view) => self.save_visual_image(&view, cx),
            VisualAction::SaveHtml(view) => self.save_visual_html(&view, cx),
        }
    }

    /// Open full screen: the light box with the visual alone.
    pub fn open_visual_lightbox(
        &mut self,
        view: VisualView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let caption = match self.visual_channel_name(&view.message_id) {
            Some(channel) => format!("Visual from {channel} at {}.", view.time_label),
            None => format!("Visual at {}.", view.time_label),
        };
        self.hovered_clear();
        self.open_lightbox_items(vec![LightboxItem::Visual(view)], 0, caption, window, cx);
    }

    fn hovered_clear(&mut self) {
        self.visuals.hovered = None;
    }

    /// The name of the channel of a visual: the channel whose timeline has
    /// it, else the channel of the open task thread.
    fn visual_channel_name(&self, message_id: &str) -> Option<String> {
        let in_timeline = self.state.timelines.iter().find(|tl| {
            tl.items.iter().any(|item| {
                matches!(item, consort_client::TimelineItem::Notice(n) if n.message_id == message_id)
            })
        });
        let channel_id = match (in_timeline, &self.panel) {
            (Some(tl), _) => tl.channel_id.clone(),
            (None, crate::app::Panel::Thread(task)) => self.state.task(task)?.channel_id.clone(),
            (None, _) => return None,
        };
        self.state.channel(&channel_id).map(|c| c.name.clone())
    }

    /// The image of a visual for an action: the newest image, else a new
    /// request at the width of its item.
    fn visual_image_now(
        &self,
        view: &VisualView,
    ) -> impl Future<Output = Result<VisualImageFile, ClientError>> + 'static {
        let ready = self.visuals.ready_image(&view.message_id);
        let client = self.client.clone();
        let id = view.message_id.clone();
        let width = self
            .visuals
            .action_width(&view.message_id, gpui_kit::px(720.));
        let scale = self.visuals.scale;
        let style = self.visuals.style.clone();
        async move {
            match ready {
                Some(file) => Ok(file),
                None => client.render_visual(id, width, scale, style).await,
            }
        }
    }

    /// Copy image: the PNG on the clipboard.
    pub fn copy_visual_image(&mut self, view: &VisualView, cx: &mut Context<Self>) {
        let image = self.visual_image_now(view);
        cx.spawn(async move |this, cx| {
            let bytes = match image.await {
                Ok(file) => read_file(PathBuf::from(file.path), cx).await,
                Err(err) => Err(err),
            };
            let _ = this.update(cx, |this, cx| match bytes {
                Ok(bytes) => cx.write_to_clipboard(ClipboardItem {
                    entries: vec![ClipboardEntry::Image(gpui_kit::Image::from_bytes(
                        ImageFormat::Png,
                        bytes,
                    ))],
                }),
                Err(err) => this.show_error(sentence(&err.to_string()), cx),
            });
        })
        .detach();
    }

    /// Save image: the save dialog, then the PNG there.
    pub fn save_visual_image(&mut self, view: &VisualView, cx: &mut Context<Self>) {
        let name = image_name(&view.title);
        let chosen = cx.prompt_for_new_path(&crate::ui::attachments::save_folder(), Some(&name));
        let image = self.visual_image_now(view);
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(to))) = chosen.await else {
                return;
            };
            let result = match image.await {
                Ok(file) => match read_file(PathBuf::from(file.path), cx).await {
                    Ok(bytes) => write_file(to, bytes, cx).await,
                    Err(err) => Err(err),
                },
                Err(err) => Err(err),
            };
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.show_notice(format!("Saved {name}."), cx),
                Err(err) => this.show_error(sentence(&err.to_string()), cx),
            });
        })
        .detach();
    }

    /// Save as HTML: the save dialog, then the page with the theme there.
    pub fn save_visual_html(&mut self, view: &VisualView, cx: &mut Context<Self>) {
        let name = consort_visual::page_name(&view.title);
        let chosen = cx.prompt_for_new_path(&crate::ui::attachments::save_folder(), Some(&name));
        let client = self.client.clone();
        let page = view.page.clone();
        let style = self.visuals.style.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(to))) = chosen.await else {
                return;
            };
            let result = match read_page(client, page, cx).await {
                Ok((_, html)) => match browser_document(&html, &style) {
                    Ok(document) => write_file(to, document.into_bytes(), cx).await,
                    Err(err) => Err(err),
                },
                Err(err) => Err(err),
            };
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.show_notice(format!("Saved {name}."), cx),
                Err(err) => this.show_error(sentence(&err.to_string()), cx),
            });
        })
        .detach();
    }

    /// Open in browser: the page with the theme in a file in the cache of
    /// the client, opened in the system browser.
    pub fn open_visual_in_browser(&mut self, view: &VisualView, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let page = view.page.clone();
        let title = view.title.clone();
        let style = self.visuals.style.clone();
        cx.spawn(async move |this, cx| {
            let sha256 = page.sha256.clone();
            let result = match read_page(client, page, cx).await {
                Ok((path, html)) => match browser_document(&html, &style) {
                    Ok(document) => {
                        let file = browser_file(&path, &sha256, &title);
                        write_file(file.clone(), document.into_bytes(), cx)
                            .await
                            .map(|()| file)
                    }
                    Err(err) => Err(err),
                },
                Err(err) => Err(err),
            };
            let _ = this.update(cx, |this, cx| match result {
                // A `file:` URL opens in the browser of the system.
                Ok(file) => cx.open_url(&file_url(&file)),
                Err(err) => this.show_error(sentence(&err.to_string()), cx),
            });
        })
        .detach();
    }

    /// Makes the live page of a visual when it has none: from the page with
    /// the bootstrap of a webview and the style of now. With four pages,
    /// the oldest that did not show in the last frame goes first; when all
    /// four showed, the visual stays an image.
    #[cfg(target_os = "macos")]
    fn ensure_page(&mut self, view: &VisualView, window: &mut Window, cx: &mut Context<Self>) {
        use crate::ui::live_page::{LivePage, PageEvent};
        // wry brings the app to the front when it makes a webview.
        if !self.visuals.live || !self.window_active {
            return;
        }
        if self
            .visuals
            .pages
            .iter()
            .any(|slot| slot.view.message_id == view.message_id)
        {
            return;
        }
        let Some(html) = self.visuals.html(&view.message_id) else {
            return;
        };
        if self.visuals.pages.len() >= PAGES_MAX && !self.drop_oldest_hidden_page() {
            return;
        }
        let Ok(document) = consort_visual::document(&html, &self.visuals.style, Host::Webview)
        else {
            return;
        };
        match LivePage::build(document, window, cx) {
            Ok(page) => {
                let dark = self.visuals.style.mode == VisualMode::Dark;
                let _ = page.read(cx).set_dark(dark, cx);
                let id = view.message_id.clone();
                let events = cx.subscribe_in(
                    &page,
                    window,
                    move |this, _, event: &PageEvent, window, cx| {
                        this.on_page_event(&id, event, window, cx)
                    },
                );
                self.visuals.pages.push(PageSlot {
                    view: view.clone(),
                    page,
                    style: self.visuals.style_id,
                    created: Instant::now(),
                    _events: events,
                });
                self.sync_frame_pages();
            }
            Err(err) => {
                // No live page in this run: the visuals show their images.
                eprintln!("consort: a visual cannot show live: {err}");
                self.visuals.live = false;
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn ensure_page(&mut self, _: &VisualView, _: &mut Window, _: &mut Context<Self>) {}

    /// Gives the sweep the pages of now.
    #[cfg(target_os = "macos")]
    fn sync_frame_pages(&self) {
        *self.visuals.frame.pages.borrow_mut() = self
            .visuals
            .pages
            .iter()
            .map(|slot| (slot.view.message_id.clone(), slot.page.clone()))
            .collect();
    }

    /// Drops the page that did not show in the last frame and was on screen
    /// longest ago. Returns whether one went.
    #[cfg(target_os = "macos")]
    fn drop_oldest_hidden_page(&mut self) -> bool {
        let frame = self.visuals.frame.clone();
        let on_screen = frame.on_screen.borrow();
        let oldest = self
            .visuals
            .pages
            .iter()
            .enumerate()
            .filter(|(_, slot)| !frame.was_claimed(&slot.view.message_id))
            .min_by_key(|(_, slot)| {
                on_screen
                    .get(&slot.view.message_id)
                    .copied()
                    .unwrap_or(slot.created)
            })
            .map(|(ix, _)| ix);
        drop(on_screen);
        match oldest {
            Some(ix) => {
                self.visuals.pages.remove(ix);
                self.sync_frame_pages();
                true
            }
            None => false,
        }
    }

    /// Drops the pages whose item has been off screen for [`DROP_AFTER`],
    /// and wakes the app when the next one is due.
    fn drop_old_pages(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            let now = Instant::now();
            let frame = self.visuals.frame.clone();
            let serial = frame.serial();
            let on_screen_last = |id: &str| {
                frame.seen.borrow().iter().any(|((_, seen_id), seen)| {
                    seen_id == id && seen.serial + 1 == serial && seen.presence != Presence::Hidden
                })
            };
            let last_seen = |slot: &PageSlot| {
                frame
                    .on_screen
                    .borrow()
                    .get(&slot.view.message_id)
                    .copied()
                    .unwrap_or(slot.created)
            };
            let before = self.visuals.pages.len();
            self.visuals.pages.retain(|slot| {
                on_screen_last(&slot.view.message_id)
                    || now.duration_since(last_seen(slot)) < DROP_AFTER
            });
            if self.visuals.pages.len() != before {
                self.sync_frame_pages();
            }
            let next = self
                .visuals
                .pages
                .iter()
                .filter(|slot| !on_screen_last(&slot.view.message_id))
                .map(|slot| last_seen(slot) + DROP_AFTER)
                .min();
            if let Some(at) = next
                && !self.visuals.drop_timer
            {
                self.visuals.drop_timer = true;
                let wait = at.saturating_duration_since(now);
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |this, cx| {
                        this.visuals.drop_timer = false;
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
    }

    /// Loads each live page again with the style of now.
    fn reload_pages(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            let style_id = self.visuals.style_id;
            let dark = self.visuals.style.mode == VisualMode::Dark;
            for ix in 0..self.visuals.pages.len() {
                let id = self.visuals.pages[ix].view.message_id.clone();
                let Some(html) = self.visuals.html(&id) else {
                    continue;
                };
                let Ok(document) =
                    consort_visual::document(&html, &self.visuals.style, Host::Webview)
                else {
                    continue;
                };
                let page = self.visuals.pages[ix].page.clone();
                let _ = page.update(cx, |page, cx| page.load_html(&document, cx));
                let _ = page.read(cx).set_dark(dark, cx);
                self.visuals.pages[ix].style = style_id;
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
    }

    /// What a live page told the app.
    #[cfg(target_os = "macos")]
    fn on_page_event(
        &mut self,
        message_id: &str,
        event: &crate::ui::live_page::PageEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::ui::live_page::{MenuRequest, PageEvent};
        use consort_visual::PageMessage;
        let text = match event {
            PageEvent::Loaded => {
                cx.notify();
                return;
            }
            PageEvent::Refused(_) => return,
            PageEvent::Message(text) => text,
        };
        if let Some(request) = MenuRequest::parse(text) {
            self.open_page_menu(message_id, request, cx);
            return;
        }
        match consort_visual::parse_message(text) {
            Some(PageMessage::Size(height)) => self.take_live_height(message_id, height, cx),
            Some(PageMessage::Open(url)) => {
                let now = Instant::now();
                if may_open(self.visuals.last_open, now) {
                    self.visuals.last_open = Some(now);
                    cx.open_url(&url);
                }
            }
            Some(PageMessage::Escape) => self.page_escape(message_id, window, cx),
            None => {}
        }
    }

    /// The place that has the live page of a visual now.
    #[cfg(target_os = "macos")]
    fn page_place(&self, message_id: &str) -> Option<Place> {
        if self.lightbox_shows(message_id) {
            return Some(Place::Lightbox);
        }
        [Place::Timeline, Place::Thread]
            .into_iter()
            .filter_map(|place| Some((place, self.visuals.frame.seen(place, message_id)?)))
            .max_by_key(|(_, seen)| seen.serial)
            .map(|(place, _)| place)
    }

    /// A height from the live page, at the width that the page has.
    #[cfg(target_os = "macos")]
    fn take_live_height(&mut self, message_id: &str, height: u32, cx: &mut Context<Self>) {
        let Some(slot) = self
            .visuals
            .pages
            .iter()
            .find(|slot| slot.view.message_id == message_id)
        else {
            return;
        };
        let shown = slot.page.read(cx).is_shown(cx);
        let Some(seen) = self
            .page_place(message_id)
            .and_then(|place| self.visuals.frame.seen(place, message_id))
        else {
            return;
        };
        if !accept_size(shown, seen.bounds.size.width) {
            return;
        }
        let live = LiveHeight {
            width: render_width(seen.bounds.size.width),
            height,
        };
        if self
            .visuals
            .states
            .get(message_id)
            .and_then(|s| s.live)
            .is_none_or(|old| old != live)
        {
            self.visuals.set_live_height(message_id, live);
            cx.notify();
        }
    }

    /// A right click in a live page: the menu of the visual, or the menu of
    /// the text that the user selected in the page.
    #[cfg(target_os = "macos")]
    fn open_page_menu(
        &mut self,
        message_id: &str,
        request: crate::ui::live_page::MenuRequest,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self
            .visuals
            .pages
            .iter()
            .find(|slot| slot.view.message_id == message_id)
            .map(|slot| slot.view.clone())
        else {
            return;
        };
        let Some(place) = self.page_place(message_id) else {
            return;
        };
        let Some(seen) = self.visuals.frame.seen(place, message_id) else {
            return;
        };
        let at =
            seen.bounds.origin + gpui_kit::point(gpui_kit::px(request.x), gpui_kit::px(request.y));
        let items = if request.selection.trim().is_empty() {
            visual_menu(&view, place == Place::Lightbox)
        } else {
            crate::ui::timeline::selection_menu(request.selection)
        };
        self.open_menu(crate::ui::menu::OpenMenu::at(at, items), cx);
    }

    /// Escape in a live page: it closes the light box when the light box
    /// shows the page. Else the keyboard goes back to the composer.
    #[cfg(target_os = "macos")]
    fn page_escape(&mut self, message_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(slot) = self
            .visuals
            .pages
            .iter()
            .find(|slot| slot.view.message_id == message_id)
        {
            let _ = slot.page.read(cx).release_focus(cx);
        }
        if self.lightbox_shows(message_id) {
            self.close_lightbox(cx);
            self.focus.focus(window, cx);
        } else {
            self.inputs
                .composer
                .update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::model::VisualHeight;
    use gpui_kit::px;

    fn notice() -> VisualNotice {
        VisualNotice::new(
            "Read speed",
            Some(300),
            vec![
                VisualHeight {
                    width: 560,
                    height: 221,
                },
                VisualHeight {
                    width: 720,
                    height: 264,
                },
            ],
        )
    }

    fn image(width: u32, height: u32) -> VisualImageFile {
        VisualImageFile {
            path: "/tmp/a.png".into(),
            width,
            height,
            scale: 2,
        }
    }

    #[test]
    fn the_live_height_comes_first_at_its_own_width() {
        let live = Some(LiveHeight {
            width: 720,
            height: 280,
        });
        let img = image(720, 264);
        assert_eq!(page_height(720, live, Some(&img), &notice()), 280);
        // At another width the live height is old: the image counts.
        assert_eq!(page_height(640, live, Some(&img), &notice()), 235);
    }

    #[test]
    fn the_image_is_scaled_to_the_width() {
        let img = image(720, 264);
        assert_eq!(page_height(720, None, Some(&img), &notice()), 264);
        // The mock answers at 720 px whatever the width: the app scales it.
        assert_eq!(page_height(368, None, Some(&img), &notice()), 135);
        assert_eq!(page_height(1040, None, Some(&img), &notice()), 381);
    }

    #[test]
    fn with_no_live_page_and_no_image_the_table_or_the_given_height_counts() {
        assert_eq!(page_height(720, None, None, &notice()), 264);
        assert_eq!(page_height(640, None, None, &notice()), 243);
        let no_table = VisualNotice::new("Read speed", Some(300), Vec::new());
        assert_eq!(page_height(720, None, None, &no_table), 300);
        // An image with no size does not count.
        assert_eq!(page_height(720, None, Some(&image(0, 0)), &no_table), 300);
    }

    #[test]
    fn a_request_has_a_whole_width_and_a_scale_in_range() {
        assert_eq!(render_width(px(720.9)), 720);
        assert_eq!(render_width(px(20.)), VISUAL_RENDER_WIDTH_MIN);
        assert_eq!(render_width(px(9000.)), VISUAL_RENDER_WIDTH_MAX);
        assert_eq!(render_width(px(f32::NAN)), VISUAL_RENDER_WIDTH_MIN);
        assert_eq!(render_scale(1.), 1);
        assert_eq!(render_scale(1.5), 2);
        assert_eq!(render_scale(2.), 2);
        assert_eq!(render_scale(4.), 3);
        assert_eq!(render_scale(0.), 1);
    }

    #[test]
    fn an_error_that_passes_tries_again_later_and_shows_after_a_few_tries() {
        let now = Instant::now();
        let unavailable = ClientError::Unavailable {
            message: "The browser for visuals is installing. Try again in a minute.".into(),
        };
        let first = Failure::after(&unavailable, 1, now);
        assert!(!first.hard);
        assert!(!first.shown(), "one try is not enough");
        assert_eq!(first.retry_at, Some(now + Duration::from_secs(2)));
        assert!(!first.due(now));
        assert!(first.due(now + Duration::from_secs(2)));
        assert!(Failure::after(&unavailable, SHOW_FAILED_AFTER, now).shown());
        assert_eq!(retry_delay(2), Duration::from_secs(4));
        assert_eq!(retry_delay(3), Duration::from_secs(8));
        assert_eq!(retry_delay(20), RETRY_MAX);
        for passing in [ClientError::NotConnected, ClientError::TimedOut] {
            assert!(passes(&passing));
        }
    }

    #[test]
    fn a_hard_error_shows_at_once_and_does_not_try_again_by_itself() {
        let now = Instant::now();
        let gone = ClientError::Hub {
            code: "not_found".into(),
            message: "This visual is gone. It may have been deleted in another client.".into(),
        };
        let failure = Failure::after(&gone, 1, now);
        assert!(failure.hard && failure.shown());
        assert_eq!(failure.retry_at, None);
        assert!(!failure.due(now + Duration::from_secs(3600)));
        assert_eq!(
            failure.message,
            "This visual is gone. It may have been deleted in another client."
        );
        // The client errors have no full stop. The item adds one.
        assert_eq!(
            Failure::after(&ClientError::NotConnected, 1, now).message,
            "Consort is not connected to the hub. Wait until it reconnects, then try again."
        );
    }

    #[test]
    fn the_body_is_the_image_else_the_failure_that_shows_else_loading() {
        let now = Instant::now();
        let img = image(720, 264);
        let gone = Failure::after(
            &ClientError::Hub {
                code: "not_found".into(),
                message: "This visual is gone.".into(),
            },
            1,
            now,
        );
        let early = Failure::after(&ClientError::TimedOut, 1, now);
        assert_eq!(
            body(Some(&img), Some(&gone), None),
            Body::Image(img.clone())
        );
        assert_eq!(body(None, None, None), Body::Loading);
        assert_eq!(body(None, Some(&early), None), Body::Loading, "not yet");
        assert_eq!(
            body(None, Some(&gone), None),
            Body::Failed("This visual is gone.".into())
        );
        // The page did not come either: the text of Paper G3.
        assert_eq!(
            body(None, Some(&gone), Some(&gone)),
            Body::Failed(PAGE_FAILED.into())
        );
        // A failed page alone waits for the image.
        assert_eq!(body(None, None, Some(&gone)), Body::Loading);
    }

    #[test]
    fn a_size_counts_only_from_a_page_that_shows_with_a_width() {
        assert!(accept_size(true, px(720.)));
        assert!(!accept_size(false, px(720.)), "hidden");
        assert!(!accept_size(true, px(0.)), "width 0");
    }

    #[test]
    fn a_page_opens_at_most_one_link_each_second() {
        let now = Instant::now();
        assert!(may_open(None, now));
        assert!(!may_open(Some(now), now + Duration::from_millis(400)));
        assert!(may_open(Some(now), now + OPEN_GAP));
    }

    #[test]
    fn the_names_of_the_files_come_from_the_title() {
        assert_eq!(
            image_name("Read speed on a 10 GB file"),
            "Read speed on a 10 GB file.png"
        );
        assert_eq!(image_name("a/b: c?"), "a b c.png");
        assert_eq!(image_name("   "), "visual.png");
        let file = browser_file(
            Path::new("/cache/blobs/abc"),
            "abc",
            "Copy speed during a 60 GB copy",
        );
        assert_eq!(
            file,
            Path::new("/cache/blobs/browser/abc/Copy speed during a 60 GB copy.html")
        );
    }

    #[test]
    fn the_size_script_measures_as_the_bootstrap_does_after_two_frames() {
        let script = size_script();
        assert!(script.contains(consort_visual::MEASURE_EXPRESSION));
        assert!(script.contains("type:\"size\""));
        assert_eq!(script.matches("requestAnimationFrame").count(), 2);
        // The message that it sends is one that the app reads.
        assert_eq!(
            consort_visual::parse_message(r#"{"type":"size","height":260}"#),
            Some(consort_visual::PageMessage::Size(260))
        );
    }

    #[test]
    fn a_file_url_encodes_the_path() {
        assert_eq!(
            file_url(Path::new("/tmp/browser/ab/Read speed: 1,810 MB.html")),
            "file:///tmp/browser/ab/Read%20speed:%201%2C810%20MB.html"
        );
        assert_eq!(
            file_url(Path::new("C:\\Users\\me\\a b.html")),
            "file:///C:/Users/me/a%20b.html"
        );
        assert_eq!(
            file_url(Path::new("/tmp/é.html")),
            "file:///tmp/%C3%A9.html"
        );
    }

    #[test]
    fn the_document_for_a_browser_has_the_theme_and_no_script_of_the_app() {
        let style = consort_visual::default_style(VisualMode::Dark);
        let document =
            browser_document("<p>Read speed</p>", &style).expect("a document for the style");
        assert!(
            document.contains("id=\"consort-visual-theme\""),
            "the theme"
        );
        assert!(document.contains("--chart-1:"), "the chart roles");
        assert!(document.contains("<p>Read speed</p>"), "the page");
        assert!(
            !document.contains("consort-visual-bootstrap"),
            "no size reporter or link catcher"
        );
        assert!(!document.contains("ipc.postMessage"));
    }

    #[test]
    fn the_menu_has_the_items_of_g2_in_order() {
        let view = VisualView {
            message_id: "msg_1".into(),
            title: "Read speed".into(),
            notice: notice(),
            page: AttachmentView::default(),
            time_label: "9:40 AM".into(),
        };
        let labels = |items: Vec<MenuItem>| -> Vec<(String, bool)> {
            items
                .iter()
                .map(|i| (i.label.to_string(), i.separator_before))
                .collect()
        };
        assert_eq!(
            labels(visual_menu(&view, false)),
            [
                ("Open full screen".to_owned(), false),
                ("Open in browser".to_owned(), false),
                ("Copy image".to_owned(), true),
                ("Save image".to_owned(), false),
                ("Save as HTML".to_owned(), false),
            ]
        );
        let in_lightbox = labels(visual_menu(&view, true));
        assert_eq!(in_lightbox.len(), 4);
        assert_eq!(in_lightbox[0].0, "Open in browser");
        let boxed = Box::new(view.clone());
        assert_eq!(
            visual_menu(&view, false)
                .into_iter()
                .map(|i| i.action)
                .collect::<Vec<_>>(),
            [
                MenuAction::Visual(VisualAction::OpenFullScreen(boxed.clone())),
                MenuAction::Visual(VisualAction::OpenInBrowser(boxed.clone())),
                MenuAction::Visual(VisualAction::CopyImage(boxed.clone())),
                MenuAction::Visual(VisualAction::SaveImage(boxed.clone())),
                MenuAction::Visual(VisualAction::SaveHtml(boxed)),
            ]
        );
    }

    #[test]
    fn an_answer_keeps_the_image_and_a_failure_its_tries() {
        let mut ui = VisualUi::default();
        let key = ui.key(720);
        let now = Instant::now();
        let retry = ui.finish_image("msg_1", key.clone(), Err(ClientError::TimedOut), 1, now);
        assert_eq!(retry, Some(now + retry_delay(2)));
        let shown = ui.shown("msg_1", 720);
        assert_eq!(shown.image_failure.map(|f| f.tries), Some(2));
        assert_eq!(
            ui.finish_image("msg_1", key, Ok(image(720, 264)), 2, now),
            None
        );
        assert_eq!(ui.shown("msg_1", 720).image, Some(image(720, 264)));
        // At another width the newest image shows until its own comes.
        assert_eq!(ui.shown("msg_1", 640).image, Some(image(720, 264)));
        // A new theme does not show the image of the old one.
        ui.set_style(consort_visual::default_style(VisualMode::Light));
        assert_eq!(ui.shown("msg_1", 720).image, None);
        // Try again forgets the failures.
        ui.fail_image("msg_1", 720, &ClientError::TimedOut, 3);
        assert!(ui.shown("msg_1", 720).image_failure.is_some());
        ui.forget_failures("msg_1");
        assert!(ui.shown("msg_1", 720).image_failure.is_none());
    }
}
