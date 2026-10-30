//! What the headless window tests share: open the app on the mock client at
//! a size, draw, wait for the mock client, and read the bounds of an element
//! that has a debug selector.

use std::time::{Duration, Instant};

use crate::app::ConsortApp;
use consort_client::{Client, ViewState};
use gpui::{AnyWindowHandle, Bounds, Pixels, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Entity, Modifiers, WindowOptions, point, px, size};

/// Opens the app at the default window size, 1440 by 900 px.
pub fn open(state: ViewState, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    open_sized(state, 1440., 900., cx)
}

pub fn open_sized(
    state: ViewState,
    width: f32,
    height: f32,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<ConsortApp>) {
    // The mock client runs its own thread.
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
    });
    let options = WindowOptions {
        window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(width), px(height)),
        })),
        ..WindowOptions::default()
    };
    let (handle, app) = cx
        .update(|cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    ConsortApp::new(Client::mock_with(state).expect("mock"), true, window, cx)
                })
            })
        })
        .expect("open the window");
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    (handle, app)
}

pub fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

/// Lets the mock client's own thread work for `ms`, then draws.
pub fn settle(cx: &mut VisualTestContext, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
        cx.run_until_parked();
    }
    draw(cx);
}

/// Waits (at most 5 s) until the app is in the state that `ready` tests.
pub fn until(
    cx: &mut VisualTestContext,
    app: &Entity<ConsortApp>,
    what: &str,
    ready: impl Fn(&ConsortApp) -> bool,
) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if app.read_with(cx, |app, _| ready(app)) {
            return;
        }
        assert!(Instant::now() < end, "timed out: {what}");
        settle(cx, 50);
    }
}

/// The bounds of the element with this debug selector.
pub fn bounds(cx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
    maybe_bounds(cx, selector).unwrap_or_else(|| panic!("{selector} draws"))
}

pub fn maybe_bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    let name: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    cx.debug_bounds(name)
}

/// The height of the title bar that Windows and Linux can draw above the app.
pub fn title_bar(cx: &mut VisualTestContext) -> Pixels {
    if has(cx, "window-title-bar") {
        bounds(cx, "window-title-bar").size.height
    } else {
        px(0.)
    }
}

pub fn has(cx: &mut VisualTestContext, selector: &str) -> bool {
    maybe_bounds(cx, selector).is_some()
}

/// Clicks the center of an element, then lets the app answer.
pub fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).center();
    cx.simulate_click(at, Modifiers::default());
    settle(cx, 100);
}
