//! Settings > Hub > Reach from other devices in a headless window, with the
//! notices that send the user to it.

use std::time::Duration;

use crate::app::{ConsortApp, Dialog};
use consort_client::{Client, ViewState};
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Entity, WindowOptions, point, px, size};

fn open(state: ViewState, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
        cx.set_app_identity("dev.consort.test", "Consort");
    });
    let options = WindowOptions {
        window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(1440.), px(900.)),
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
    let vcx = &mut VisualTestContext::from_window(handle, cx);
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    (handle, app)
}

fn settle(cx: &mut VisualTestContext, ms: u64) {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
        cx.run_until_parked();
    }
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn has(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn screen(app: &Entity<ConsortApp>, name: &str, cx: &mut VisualTestContext) {
    let name = name.to_owned();
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_screen(&name, window, cx)));
    settle(cx, 150);
}

fn mock_state(name: &str) -> ViewState {
    consort_client::mock_reach::screen_state(name).expect("a reach screen")
}

#[gpui::test]
fn turning_on_tailscale_shows_add_a_device_with_a_code(cx: &mut TestAppContext) {
    let (handle, app) = open(mock_state("reach-off"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "reach-off", cx);
    assert!(has(cx, "reach-tailscale"));
    assert!(has(cx, "reach-network"));
    assert!(has(cx, "reach-tailscale-turn-on"));
    assert!(!has(cx, "reach-add-device"));

    crate::test_support::click(cx, "reach-tailscale-turn-on");
    settle(cx, 200);
    // The page asks for a code at its next second. The clock of a test does
    // not run, so this asks now.
    cx.update(|_, cx| app.update(cx, |app, cx| app.open_reach(cx)));
    settle(cx, 200);
    assert!(has(cx, "reach-add-device"));
    assert!(has(cx, "reach-tailscale-turn-off"));
    app.read_with(cx, |app, _| {
        assert_eq!(app.state.admin.reach.routes.len(), 1);
        assert!(app.state.admin.pairing.is_some(), "the page makes a code");
    });
}

#[gpui::test]
fn back_to_hub_forgets_the_code(cx: &mut TestAppContext) {
    let (handle, app) = open(mock_state("reach-on"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "reach-on", cx);
    assert!(has(cx, "reach-add-device"));
    crate::test_support::click(cx, "reach-back");
    settle(cx, 200);
    assert!(!has(cx, "reach-add-device"));
    assert!(has(cx, "hub-reach"));
    app.read_with(cx, |app, _| {
        assert!(!app.admin.reach_open);
        assert!(app.state.admin.pairing.is_none());
    });
}

#[gpui::test]
fn the_hub_page_opens_the_reach_page(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "hub", cx);
    crate::test_support::click(cx, "hub-reach");
    settle(cx, 150);
    assert!(has(cx, "reach-back"));
}

#[gpui::test]
fn the_list_of_states_has_a_card_for_each(cx: &mut TestAppContext) {
    let (handle, app) = open(mock_state("reach-states"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "reach-states", cx);
    // A window that is 900 px high draws the first cards.
    assert!(has(cx, "reach-state-0"));
    assert!(has(cx, "reach-state-1"));
    app.read_with(cx, |app, _| assert!(app.admin.reach_gallery));
}

#[gpui::test]
fn a_pairing_code_on_loopback_sends_the_user_to_reach(cx: &mut TestAppContext) {
    let (handle, app) = open(mock_state("pair-client-loopback"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "pair-client-loopback", cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::PairClient));
    });
    crate::test_support::click(cx, "open-reach");
    settle(cx, 150);
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, None);
        assert!(app.admin.reach_open);
    });
    assert!(has(cx, "reach-tailscale"));
}

#[gpui::test]
fn add_a_worker_on_loopback_sends_the_user_to_reach(cx: &mut TestAppContext) {
    let (handle, app) = open(mock_state("add-worker-loopback"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "add-worker-loopback", cx);
    assert!(has(cx, "open-reach"));
}

#[gpui::test]
fn a_hub_on_the_tailnet_shows_its_error_above_the_composer(cx: &mut TestAppContext) {
    let (handle, _app) = open(mock_state("reach-unreachable"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 150);
    assert!(has(cx, "tailnet-notice"));
}

#[gpui::test]
fn the_connect_screen_takes_a_pasted_link_with_no_code(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let link = "consort://pair?hub=https%3A%2F%2Fmac.local%3A7601&code=ABCD-EFGH";
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.inputs
                .hub_url
                .update(cx, |input, cx| input.set_value(link, window, cx));
            app.connect(window, cx);
        })
    });
    settle(cx, 100);
    app.read_with(cx, |app, _| {
        assert_eq!(app.onboarding.error, None);
    });
}
