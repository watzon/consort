//! A hub on this computer: the screens W2, W3 and W4, the card for a hub that
//! stopped, and the restart with tasks.

use consort_client::local_hub::{HubRun, LocalHub, SwapState};
use consort_client::mock_local::{screen_state, screen_view};
use consort_client::setup::Gate;
use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::app::{ConsortApp, Dialog, Page, SettingsSection};
use crate::test_support::{click, draw, has, open, settle};
use crate::ui::local_hub::LocalUi;

fn set_local(app: &Entity<ConsortApp>, screen: &str, cx: &mut VisualTestContext) -> LocalHub {
    let hub = LocalHub::fixed(screen_view(screen).expect("a view"));
    let handle = hub.clone();
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.local = LocalUi::with(hub);
            app.local.seen_swap = app.local.hub.view().swap;
        })
    });
    draw(cx);
    handle
}

fn open_hub(
    screen: &str,
    cx: &mut TestAppContext,
) -> (VisualTestContext, Entity<ConsortApp>, LocalHub) {
    let (handle, app) = open(screen_state(screen).expect("a state"), cx);
    let mut cx = VisualTestContext::from_window(handle, cx);
    settle(&mut cx, 100);
    let hub = set_local(&app, screen, &mut cx);
    (cx, app, hub)
}

fn setup_page(screen: &str, cx: &mut TestAppContext) -> VisualTestContext {
    let (mut cx, app, _) = open_hub(screen, cx);
    let gate = if screen == "local-found" {
        Gate::LocalFound
    } else {
        Gate::Where
    };
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.page = Page::Setup;
            app.setup.gate = gate;
        })
    });
    draw(&mut cx);
    cx
}

#[gpui::test]
fn the_install_screen_has_the_four_steps(cx: &mut TestAppContext) {
    let mut cx = setup_page("local-install", cx);
    assert!(has(&mut cx, "local-install"));
    for step in ["checked", "starting_hub", "adding_worker", "connecting"] {
        assert!(has(&mut cx, &format!("local-step-{step}")), "{step}");
    }
}

#[gpui::test]
fn each_problem_has_its_card(cx: &mut TestAppContext) {
    let mut one = setup_page("local-windows", cx);
    assert!(has(&mut one, "local-windows"));
    assert!(!has(&mut one, "local-install"));
}

#[gpui::test]
fn bubblewrap_shows_the_command_and_try_again(cx: &mut TestAppContext) {
    let mut cx = setup_page("local-bubblewrap", cx);
    assert!(has(&mut cx, "local-bubblewrap-command"));
    assert!(has(&mut cx, "local-try-again"));
}

#[gpui::test]
fn a_failed_install_offers_the_log(cx: &mut TestAppContext) {
    let mut cx = setup_page("local-failed", cx);
    assert!(has(&mut cx, "local-failed"));
    assert!(has(&mut cx, "local-log"));
    assert!(has(&mut cx, "local-try-again"));
}

#[gpui::test]
fn a_hub_on_this_computer_offers_connect(cx: &mut TestAppContext) {
    let mut cx = setup_page("local-found", cx);
    assert!(has(&mut cx, "local-connect"));
}

#[gpui::test]
fn a_stopped_hub_has_its_card_and_start_brings_it_back(cx: &mut TestAppContext) {
    let (mut cx, _app, hub) = open_hub("local-stopped", cx);
    assert!(has(&mut cx, "local-stopped-card"));
    assert!(has(&mut cx, "local-log"));
    click(&mut cx, "local-start");
    assert_eq!(hub.view().run, HubRun::Running);
    assert!(!has(&mut cx, "local-stopped-card"));
}

#[gpui::test]
fn another_offline_hub_has_no_stopped_card(cx: &mut TestAppContext) {
    let mut state = screen_state("local-stopped").unwrap();
    state.hub_url = "https://hub.example.com".to_owned();
    let (handle, app) = open(state, cx);
    let mut cx = VisualTestContext::from_window(handle, cx);
    settle(&mut cx, 100);
    set_local(&app, "local-stopped", &mut cx);
    assert!(!has(&mut cx, "local-stopped-card"));
}

fn open_settings(cx: &mut TestAppContext) -> (VisualTestContext, Entity<ConsortApp>, LocalHub) {
    let (mut cx, app, hub) = open_hub("hub-local", cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(SettingsSection::Hub, cx);
        })
    });
    settle(&mut cx, 100);
    (cx, app, hub)
}

#[gpui::test]
fn settings_show_the_five_rows_for_a_local_hub(cx: &mut TestAppContext) {
    let (mut cx, _app, _hub) = open_settings(cx);
    for id in [
        "local-rows",
        "local-login",
        "local-reach",
        "local-move",
        "local-connect-other",
        "local-remove",
    ] {
        assert!(has(&mut cx, id), "{id}");
    }
    assert!(has(&mut cx, "back-up-now"), "the backups stay on the page");
}

#[gpui::test]
fn the_switch_turns_the_start_at_login_off(cx: &mut TestAppContext) {
    let (mut cx, _app, hub) = open_settings(cx);
    assert!(hub.view().start_at_login);
    click(&mut cx, "local-login");
    assert!(!hub.view().start_at_login);
}

#[gpui::test]
fn a_hub_on_a_server_has_no_local_rows(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state(), cx);
    let mut cx = VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| app.update(cx, |app, cx| app.open_settings(SettingsSection::Hub, cx)));
    settle(&mut cx, 100);
    assert!(!has(&mut cx, "local-rows"));
}

#[gpui::test]
fn remove_asks_about_the_data_and_ends_at_the_connect_screen(cx: &mut TestAppContext) {
    let (mut cx, app, hub) = open_settings(cx);
    click(&mut cx, "local-remove");
    assert!(has(&mut cx, "local-remove-keep"));
    assert!(has(&mut cx, "local-remove-delete"));
    assert!(hub.view().installed, "nothing happens before the choice");
    click(&mut cx, "local-remove-keep");
    settle(&mut cx, 100);
    assert!(!hub.view().installed);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.page, Page::Setup);
        assert_eq!(app.dialog, None);
    });
}

#[gpui::test]
fn connect_to_another_hub_leaves_the_hub_installed(cx: &mut TestAppContext) {
    let (mut cx, app, hub) = open_settings(cx);
    click(&mut cx, "local-connect-other");
    click(&mut cx, "dialog-confirm");
    settle(&mut cx, 100);
    assert!(hub.view().installed);
    app.read_with(&cx, |app, _| assert_eq!(app.page, Page::Setup));
}

#[gpui::test]
fn move_shows_the_backup_file_and_the_steps(cx: &mut TestAppContext) {
    let (mut cx, app, _hub) = open_hub("hub-local-move", cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(SettingsSection::Hub, cx);
            app.open_dialog(Dialog::LocalMove, window, cx);
        })
    });
    settle(&mut cx, 100);
    assert!(has(&mut cx, "local-move-file"));
}

fn restart_dialog(cx: &mut TestAppContext) -> (VisualTestContext, Entity<ConsortApp>, LocalHub) {
    let (mut cx, app, hub) = open_hub("restart-tasks", cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(Dialog::RestartWithTasks, window, cx);
        })
    });
    settle(&mut cx, 50);
    (cx, app, hub)
}

#[gpui::test]
fn restart_with_tasks_offers_both_choices(cx: &mut TestAppContext) {
    let (mut cx, _app, hub) = restart_dialog(cx);
    assert!(has(&mut cx, "restart-now"));
    assert!(has(&mut cx, "restart-when-done"));
    click(&mut cx, "restart-when-done");
    assert_eq!(hub.view().swap, SwapState::Waiting { working: 2 });
}

#[gpui::test]
fn closing_the_restart_dialog_cancels_the_swap(cx: &mut TestAppContext) {
    let (mut cx, app, hub) = restart_dialog(cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.close_dialog(window, cx)));
    settle(&mut cx, 50);
    assert_eq!(hub.view().swap, SwapState::Idle);
}

#[gpui::test]
fn a_swap_that_needs_a_choice_opens_the_dialog(cx: &mut TestAppContext) {
    let (mut cx, app, _hub) = open_hub("restart-tasks", cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.local.seen_swap = SwapState::Idle;
            app.local_changed(window, cx);
        })
    });
    draw(&mut cx);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::RestartWithTasks));
    });
}
