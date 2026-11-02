//! The live page of a visual: one native webview from `gpui-webview`, on
//! macOS only (docs/VISUALS-PLAN.md, "Live or image").
//!
//! The native view draws above all GPUI content in its bounds and does not
//! clip. It also stays where it was last drawn when its element is not in
//! the frame, for example when a `list` row scrolls out. So the owner must
//! call [`LivePage::hide`] each time the cover rule (`ui/visual_cover.rs`)
//! does not say `Live`, and [`LivePage::show`] when it does.
//!
//! The page:
//! - has a data store that is not kept (incognito), and no developer tools;
//! - loads only what the app gives it: the first page, and one more page for
//!   each [`LivePage::load_html`]. Every other navigation is refused;
//! - opens no windows;
//! - sends each `window.ipc.postMessage(..)` string to the owner as
//!   [`PageEvent::Message`];
//! - stays hidden until its first load finishes, so the white of an empty
//!   webview never shows;
//! - shows no menu of its own on a right click. It sends a
//!   [`MenuRequest`] instead, so the app opens its planned menu (GPUI
//!   rule 2).
//!
//! This file depends on nothing else in the crate, so the spike in
//! `examples/live_page.rs` can use it as it is.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, IntoElement, Render, Window};
use gpui_webview::WebView;
use lb_wry::{NewWindowResponse, PageLoadEvent, Theme, WebViewBuilder};

/// What a live page tells its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageEvent {
    /// The page sent this string with `window.ipc.postMessage`. The visual
    /// item reads it with `consort_visual::parse_message`.
    Message(String),
    /// The first load of the page finished. From now on it shows while its
    /// owner wants it.
    Loaded,
    /// The page tried to go to this address, and the live page refused it.
    Refused(String),
}

/// The script that each load of the page runs first. A right click sends
/// `menu:<x>,<y>:<selected text>` with the place of the click in the page,
/// and the webview shows no menu of its own.
const MENU_SCRIPT: &str = r#"document.addEventListener("contextmenu",function(event){event.preventDefault();try{var s=String(window.getSelection()||"");window.ipc.postMessage("menu:"+Math.round(event.clientX)+","+Math.round(event.clientY)+":"+s);}catch(_){}},true);"#;

/// A right click in the page: where it was, in CSS pixels from the top left
/// corner of the page, and the text that the user selected in the page.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuRequest {
    pub x: f32,
    pub y: f32,
    pub selection: String,
}

impl MenuRequest {
    /// Reads a message that [`MENU_SCRIPT`] sent. Any other message gives
    /// `None`.
    pub fn parse(message: &str) -> Option<Self> {
        let rest = message.strip_prefix("menu:")?;
        let (place, selection) = rest.split_once(':')?;
        let (x, y) = place.split_once(',')?;
        let x: f32 = x.trim().parse().ok()?;
        let y: f32 = y.trim().parse().ok()?;
        (x.is_finite() && y.is_finite()).then(|| Self {
            x,
            y,
            selection: selection.to_owned(),
        })
    }
}

/// Counts the loads that the app allowed. A navigation that the app did not
/// start finds no load left and is refused.
#[derive(Clone, Debug)]
pub struct NavigationGate(Rc<Cell<u32>>);

impl NavigationGate {
    /// A gate that allows the first load of the page.
    pub fn new() -> Self {
        Self(Rc::new(Cell::new(1)))
    }

    /// Allows one more load: the app is about to load a page.
    pub fn allow_one(&self) {
        self.0.set(self.0.get().saturating_add(1));
    }

    /// Whether a navigation may start. Each navigation that may start uses
    /// one load.
    pub fn pass(&self) -> bool {
        match self.0.get() {
            0 => false,
            left => {
                self.0.set(left - 1);
                true
            }
        }
    }
}

impl Default for NavigationGate {
    fn default() -> Self {
        Self::new()
    }
}

/// One live page and its native webview.
pub struct LivePage {
    view: Entity<WebView>,
    gate: NavigationGate,
    /// The owner wants the page on screen.
    wanted: bool,
    /// The first load finished.
    loaded: bool,
}

impl EventEmitter<PageEvent> for LivePage {}

impl LivePage {
    /// Makes the live page of `html` in `window`. The page is hidden until
    /// its first load finishes and its owner calls [`LivePage::show`].
    pub fn build(
        html: String,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Entity<Self>, lb_wry::Error> {
        // The handlers exist before the entity does, so they find it here.
        let slot: Rc<RefCell<Option<gpui_kit::WeakEntity<LivePage>>>> = Rc::default();
        let send = sender(slot.clone(), cx);
        let gate = NavigationGate::new();
        let builder = WebViewBuilder::new()
            .with_html(html)
            .with_incognito(true)
            .with_devtools(false)
            .with_visible(false)
            .with_initialization_script(MENU_SCRIPT)
            .with_navigation_handler({
                let gate = gate.clone();
                let send = send.clone();
                move |url| {
                    let pass = gate.pass();
                    if !pass {
                        send(PageEvent::Refused(url));
                    }
                    pass
                }
            })
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_ipc_handler({
                let send = send.clone();
                move |request| send(PageEvent::Message(request.into_body()))
            })
            .with_on_page_load_handler(move |event, _| {
                if matches!(event, PageLoadEvent::Finished) {
                    send(PageEvent::Loaded);
                }
            });
        let mut view = WebView::build(builder, window, cx)?;
        // `WebView` starts visible and shows at its first paint. It waits
        // for the load instead.
        view.hide();
        let view = cx.new(|_| view);
        let page = cx.new(|_| Self {
            view,
            gate,
            wanted: false,
            loaded: false,
        });
        *slot.borrow_mut() = Some(page.downgrade());
        Ok(page)
    }

    /// Shows the page in the bounds of its element, once it has loaded.
    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.wanted = true;
        self.sync(cx);
    }

    /// Hides the page. The page keeps its state, and the focus goes back to
    /// the window.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.wanted = false;
        self.sync(cx);
    }

    /// Whether the native view is on screen now.
    pub fn is_shown(&self, cx: &App) -> bool {
        self.view.read(cx).visible()
    }

    /// Whether the first load finished.
    // The spike in `examples/live_page.rs` uses it.
    #[allow(dead_code)]
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Loads another page in the same webview, for example after a theme
    /// change.
    pub fn load_html(&mut self, html: &str, cx: &mut Context<Self>) -> Result<(), lb_wry::Error> {
        self.gate.allow_one();
        let result = self.view.read(cx).load_html(html);
        if result.is_err() {
            // The load did not start, so it must not stay allowed.
            self.gate.pass();
        }
        result
    }

    /// Gives the page a dark or a light appearance: its
    /// `prefers-color-scheme` and its native controls.
    pub fn set_dark(&self, dark: bool, cx: &App) -> Result<(), lb_wry::Error> {
        let theme = if dark { Theme::Dark } else { Theme::Light };
        self.view.read(cx).set_theme(theme)
    }

    /// Runs a script in the page. Its messages come back as
    /// [`PageEvent::Message`].
    pub fn run_script(&self, script: &str, cx: &App) -> Result<(), lb_wry::Error> {
        self.view.read(cx).evaluate_script(script)
    }

    /// Gives the keyboard back to the window, for example after Escape in
    /// the page. A focused page gets every key until then.
    pub fn release_focus(&self, cx: &App) -> Result<(), lb_wry::Error> {
        self.view.read(cx).focus_parent()
    }

    /// Reads the text that the user selected in the page. GPUI takes
    /// Cmd+C before the page does, so Copy reads the selection here.
    /// `done` gets the text as a JSON string, such as `"Disk use"`.
    // The spike in `examples/live_page.rs` uses it. The app reads the
    // selection from the right click of the page instead.
    #[allow(dead_code)]
    pub fn read_selection(
        &self,
        done: impl Fn(String) + Send + 'static,
        cx: &App,
    ) -> Result<(), lb_wry::Error> {
        self.view
            .read(cx)
            .evaluate_script_with_callback("String(getSelection())", done)
    }

    fn receive(&mut self, event: PageEvent, cx: &mut Context<Self>) {
        if event == PageEvent::Loaded {
            if self.loaded {
                // A later load: the page is on screen already.
                return;
            }
            self.loaded = true;
            self.sync(cx);
        }
        cx.emit(event);
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let visible = self.wanted && self.loaded;
        self.view.update(cx, |view, cx| {
            if view.visible() != visible {
                if visible {
                    view.show();
                } else {
                    view.hide();
                }
                cx.notify();
            }
        });
    }
}

impl Render for LivePage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.view.clone()
    }
}

/// Sends an event of the webview to its live page. The webview calls its
/// handlers from the run loop of the main thread, outside of any GPUI
/// update, so the event waits for a task on the main thread.
fn sender(
    slot: Rc<RefCell<Option<gpui_kit::WeakEntity<LivePage>>>>,
    cx: &App,
) -> Rc<dyn Fn(PageEvent)> {
    let executor = cx.foreground_executor().clone();
    let app = cx.to_async();
    Rc::new(move |event| {
        let Some(page) = slot.borrow().clone() else {
            return;
        };
        let mut app = app.clone();
        executor
            .spawn(async move {
                // The page is gone when this fails, and so is its owner.
                let _ = page.update(&mut app, |page, cx| page.receive(event, cx));
            })
            .detach();
    })
}

#[cfg(test)]
mod tests {
    use super::{MenuRequest, NavigationGate};

    #[test]
    fn a_right_click_in_the_page_gives_its_place_and_the_selection() {
        assert_eq!(
            MenuRequest::parse("menu:120,48:Read speed: 1,810 MB/s"),
            Some(MenuRequest {
                x: 120.,
                y: 48.,
                selection: "Read speed: 1,810 MB/s".into(),
            })
        );
        assert_eq!(
            MenuRequest::parse("menu:0,0:").map(|m| m.selection),
            Some(String::new())
        );
        for other in [
            r#"{"type":"size","height":264}"#,
            "menu:",
            "menu:1:",
            "menu:a,b:",
            "menu:NaN,1:",
            "menu:1,2",
        ] {
            assert_eq!(MenuRequest::parse(other), None, "{other}");
        }
    }

    #[test]
    fn the_gate_allows_only_the_loads_of_the_app() {
        let gate = NavigationGate::new();
        assert!(gate.pass(), "the first page");
        assert!(!gate.pass(), "a link or a script");
        assert!(!gate.pass());
        gate.allow_one();
        assert!(gate.pass(), "a page that the app loads");
        assert!(!gate.pass());
    }

    #[test]
    fn a_clone_shares_the_loads() {
        let gate = NavigationGate::new();
        let handler = gate.clone();
        assert!(handler.pass());
        gate.allow_one();
        assert!(handler.pass());
        assert!(!gate.pass());
    }
}
