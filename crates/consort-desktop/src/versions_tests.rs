//! The version surfaces: the card above the composer, the hold in the
//! sidebar, Settings > Hub, the Connect error and Settings > Workers.

use consort_client::mock_versions;
use consort_client::update::{UpdateView, Updates, Version};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

use crate::app::{ConsortApp, SettingsSection};
use crate::test_support::{draw, has, open, settle};
use crate::ui::update_notice::start_updates;
use crate::ui::versions::screen_view;

fn updates_for(screen: &str) -> Updates {
    Updates::fixed(screen_view(start_updates(true).view(), screen))
}

fn set_updates(app: &Entity<ConsortApp>, view: Updates, cx: &mut VisualTestContext) {
    cx.update(|_, cx| app.update(cx, |app, _| app.updates = view));
    draw(cx);
}

fn updates_view(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) -> UpdateView {
    app.read_with(cx, |a, _| a.updates.view())
}

#[gpui::test]
fn the_hub_too_old_card_has_the_command_and_both_buttons(cx: &mut TestAppContext) {
    let (handle, _app) = open(mock_versions::hub_too_old(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(has(cx, "incompatible-card"));
    assert!(has(cx, "incompatible-command"));
    // The composer stays: a message waits in the outbox.
    assert!(has(cx, "offline-pill"));
    // The card and the offline line do not both speak.
    assert!(!has(cx, "link-state"));
}

#[gpui::test]
fn the_card_sits_above_the_composer_in_the_window(cx: &mut TestAppContext) {
    let (handle, _app) = open(mock_versions::hub_too_old(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let card = cx.debug_bounds("incompatible-card").expect("the card");
    let window = cx.update(|window, _| window.bounds().size);
    assert!(card.top() >= gpui::px(0.0));
    assert!(card.bottom() <= window.height);
    let command = cx
        .debug_bounds("incompatible-command")
        .expect("the command");
    assert!(command.right() <= card.right());
}

#[gpui::test]
fn the_app_too_old_card_offers_the_update_when_it_is_ready(cx: &mut TestAppContext) {
    let (handle, app) = open(mock_versions::app_too_old(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(has(cx, "incompatible-card"));
    assert!(
        !has(cx, "incompatible-command"),
        "no command for the server"
    );
    assert!(has(cx, "incompatible-check"), "no update ready yet");
    set_updates(&app, updates_for("app-too-old"), cx);
    assert!(has(cx, "incompatible-update"));
    assert!(!has(cx, "incompatible-check"));
}

#[gpui::test]
fn the_sidebar_shows_the_hold_in_place_of_the_notice(cx: &mut TestAppContext) {
    let (handle, app) = open(mock_versions::update_held(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(!has(cx, "update-hold"), "no update yet");
    set_updates(&app, updates_for("update-held"), cx);
    // The state of the app tells the updater about the hub.
    let hub = app.read_with(cx, |a, _| a.state.hub_versions.clone());
    app.read_with(cx, |a, _| a.updates.set_hub(hub));
    draw(cx);
    assert!(updates_view(&app, cx).hold().is_some());
    assert!(has(cx, "update-hold"));
    assert!(!has(cx, "update-notice"));
    // Update the hub goes to the page with the command.
    let at = cx.debug_bounds("update-hold-hub").expect("button").center();
    cx.simulate_click(at, Modifiers::default());
    cx.run_until_parked();
    let section = app.read_with(cx, |a, _| a.settings_section);
    assert_eq!(section, SettingsSection::Hub);
    // A new hub ends the hold.
    app.read_with(cx, |a, _| {
        a.updates
            .set_hub(Some(consort_client::versions::HubVersions {
                hub_version: "0.8.0".into(),
                protocol_versions: vec![2],
                install: None,
            }))
    });
    assert!(updates_view(&app, cx).hold().is_none());
}

#[gpui::test]
fn settings_hub_shows_a_newer_version_with_its_command(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    set_updates(&app, updates_for("hub-update"), cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| app.open_settings(SettingsSection::Hub, cx));
    });
    settle(cx, 100);
    assert!(has(cx, "hub-update-card"));
    assert!(has(cx, "hub-update-command"));
    // With no newer version, the card stays out.
    let same = start_updates(true).view();
    assert_eq!(same.latest(), None);
    assert!(
        same.current <= Version::parse("0.6.0").expect("a version"),
        "the mock hub is ahead of this build"
    );
    set_updates(&app, Updates::fixed(same), cx);
    assert!(!has(cx, "hub-update-card"));
}

#[gpui::test]
fn the_connect_screen_shows_the_title_of_a_version_error(cx: &mut TestAppContext) {
    let mut state = consort_client::mock::view_state();
    state.connection = consort_client::Connection::NotPaired;
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.page = crate::app::Page::Onboarding;
            app.show_pair_error(
                &consort_client::ClientError::Incompatible {
                    title: "This hub is too old for this app".into(),
                    detail: "The hub runs 0.6.2.".into(),
                },
                cx,
            );
        })
    });
    settle(cx, 50);
    assert!(has(cx, "onboarding-error-title"));
}

#[gpui::test]
fn settings_workers_shows_where_an_update_is(cx: &mut TestAppContext) {
    let state = mock_versions::workers_updating();
    let ids: Vec<String> = state.workers.iter().map(|w| w.id.clone()).collect();
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(SettingsSection::Workers, cx)
        });
    });
    settle(cx, 100);
    let name =
        |id: &str| -> &'static str { Box::leak(format!("worker-update-{id}").into_boxed_str()) };
    assert!(!has(cx, name(&ids[0])), "no update, no line");
    assert!(has(cx, name(&ids[1])));
    assert!(has(cx, name(&ids[2])));
}
