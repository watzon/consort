//! Phase 5 in a headless window: the three steps of the Add worker dialog
//! and the update notice in the sidebar.

use std::time::Duration;

use crate::app::{ConsortApp, Dialog};
use crate::ui::add_worker::AddWorkerStep;
use consort_client::{
    Client, Os, ViewState,
    update::{UpdateState, UpdateView, Updates, Version},
};
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Entity, Modifiers, WindowOptions, point, px, size};

fn open(state: ViewState, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
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
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    (handle, app)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn settle(cx: &mut VisualTestContext, ms: u64) {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
        cx.run_until_parked();
    }
    draw(cx);
}

fn has(cx: &mut VisualTestContext, selector: &str) -> bool {
    let name: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    cx.debug_bounds(name).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let name: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    let bounds = cx
        .debug_bounds(name)
        .unwrap_or_else(|| panic!("{selector} draws"));
    cx.simulate_click(bounds.center(), Modifiers::default());
    settle(cx, 50);
}

/// Opens the Add worker dialog the way the app does: it asks the mock hub
/// for a code.
fn open_add_worker(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.open_dialog(Dialog::AddWorker, window, cx))
    });
    settle(cx, 300);
}

fn version(text: &str) -> Version {
    Version::parse(text).expect("a version")
}

#[gpui::test]
fn the_add_worker_dialog_goes_from_the_system_to_the_install_command(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_add_worker(&app, cx);
    // Step 1: three systems and Continue. There is no command yet.
    for selector in [
        "add-worker-os-macos",
        "add-worker-os-windows",
        "add-worker-os-linux",
        "add-worker-continue",
    ] {
        assert!(has(cx, selector), "{selector} draws in step 1");
    }
    assert!(!has(cx, "copy-install-command"));
    click(cx, "add-worker-os-macos");
    assert_eq!(app.read_with(cx, |a, _| a.add_worker_os), Os::MacOs);
    click(cx, "add-worker-continue");
    assert_eq!(
        app.read_with(cx, |a, _| a.add_worker_step),
        AddWorkerStep::Install
    );
    // Step 2: the command, its Copy button and New code.
    assert!(has(cx, "copy-install-command"));
    assert!(has(cx, "add-worker-new-code"));
    assert!(has(cx, "add-worker-back"));
    assert!(!has(cx, "add-worker-continue"));
    let command = app.read_with(cx, |a, _| {
        crate::ui::add_worker::install_command(a.add_worker_os, a.state.worker_pairing.as_ref())
    });
    let command = command.expect("the mock hub made a code");
    assert!(command.contains("worker.sh"), "{command}");
    assert!(command.contains("--code KQ7M-4XPD"), "{command}");
    // Back returns to step 1, and the chosen system stays.
    click(cx, "add-worker-back");
    assert!(has(cx, "add-worker-continue"));
    assert_eq!(app.read_with(cx, |a, _| a.add_worker_os), Os::MacOs);
}

#[gpui::test]
fn a_new_code_makes_a_second_code(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_add_worker(&app, cx);
    click(cx, "add-worker-continue");
    let before = app.read_with(cx, |a, _| a.state.worker_pairing.clone());
    assert!(before.is_some());
    // The mock gives the same text, but the expiry moves on.
    std::thread::sleep(Duration::from_millis(30));
    click(cx, "add-worker-new-code");
    settle(cx, 200);
    let after = app.read_with(cx, |a, _| a.state.worker_pairing.clone());
    assert!(
        after.as_ref().map(|p| p.expires_at) > before.as_ref().map(|p| p.expires_at),
        "a new code has a later expiry"
    );
}

#[gpui::test]
fn the_dialog_shows_step_3_when_the_new_worker_is_online(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state_with_long_detail(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_add_worker(&app, cx);
    click(cx, "add-worker-continue");
    assert!(!has(cx, "add-worker-add-account"));
    // As if bigboi had just paired and connected.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.add_worker_before.retain(|id| {
                app.state
                    .workers
                    .iter()
                    .any(|w| &w.id == id && w.name != "bigboi")
            });
            app.apply_state(app.state.clone(), cx);
        })
    });
    draw(cx);
    assert!(has(cx, "add-worker-add-account"), "step 3 draws");
    assert!(has(cx, "add-worker-done"));
    assert!(!has(cx, "copy-install-command"));
    // Add an account closes this dialog and opens the account dialog of the
    // new worker.
    click(cx, "add-worker-add-account");
    let dialog = app.read_with(cx, |a, _| a.dialog.clone());
    assert!(
        matches!(dialog, Some(Dialog::AddAccount(_))),
        "the account dialog opens: {dialog:?}"
    );
}

fn ready(state: UpdateState) -> Updates {
    Updates::fixed(UpdateView {
        state,
        auto: true,
        current: version("0.1.0"),
        installable: true,
        release_protocols: None,
        hub: None,
    })
}

#[gpui::test]
fn an_expired_account_shows_a_notice_that_opens_its_sign_in(cx: &mut TestAppContext) {
    let state = consort_client::mock::view_state();
    let (worker_id, account_id) = state
        .expired_accounts()
        .next()
        .map(|(worker, account)| (worker.id.clone(), account.id.clone()))
        .expect("the mock has an expired account");
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let notice = format!("sign-in-notice-{account_id}");
    let button = format!("sign-in-again-{account_id}");
    let notice: &'static str = Box::leak(notice.into_boxed_str());
    let button: &'static str = Box::leak(button.into_boxed_str());
    assert!(has(cx, notice));
    let at = cx
        .debug_bounds(button)
        .expect("the button of the notice")
        .center();
    cx.simulate_click(at, Modifiers::default());
    cx.run_until_parked();
    let form = cx.update(|_, cx| {
        let app = app.read(cx);
        (
            matches!(app.dialog, Some(Dialog::AddAccount(_))),
            app.add_account.worker_id.clone(),
            app.add_account.account_id.clone(),
        )
    });
    assert_eq!(form, (true, worker_id, Some(account_id)));
}

#[gpui::test]
fn the_update_notice_shows_only_when_an_update_is_ready(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert!(!has(cx, "update-notice"), "no update, no notice");
    for state in [
        UpdateState::Checking,
        UpdateState::Available {
            version: version("0.2.0"),
        },
        UpdateState::Downloading {
            version: version("0.2.0"),
        },
        UpdateState::Failed {
            message: "No network.".into(),
        },
    ] {
        cx.update(|_, cx| app.update(cx, |app, _| app.updates = ready(state)));
        draw(cx);
        assert!(!has(cx, "update-notice"));
    }
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.updates = ready(UpdateState::Ready {
                version: version("0.2.0"),
                archive: std::path::PathBuf::new(),
            })
        })
    });
    draw(cx);
    assert!(has(cx, "update-notice"));
    assert!(has(cx, "update-restart"));
    // The notice sits above the account row, inside the sidebar.
    let notice = cx.debug_bounds("update-notice").expect("notice");
    let avatar = cx.debug_bounds("sidebar-avatar").expect("avatar");
    let sidebar = cx.debug_bounds("sidebar").expect("sidebar");
    assert!(notice.bottom() <= avatar.top(), "above the account row");
    assert!(notice.left() >= sidebar.left() && notice.right() <= sidebar.right());
}

#[gpui::test]
fn restart_says_why_when_the_app_cannot_replace_itself(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.updates = ready(UpdateState::Ready {
                version: version("0.2.0"),
                archive: std::path::PathBuf::new(),
            })
        })
    });
    draw(cx);
    // The test program runs from a build folder, so the install is refused.
    click(cx, "update-restart");
    settle(cx, 300);
    let message = app.read_with(cx, |a, _| {
        a.notices.visible().last().map(|n| n.message.clone())
    });
    let message = message.expect("an error notice");
    assert!(message.contains("cannot replace itself"), "{message}");
    // A second try works the same: the click is not stuck.
    click(cx, "update-restart");
    settle(cx, 300);
    assert!(app.read_with(cx, |a, _| a.notices.visible().last().is_some()));
}
