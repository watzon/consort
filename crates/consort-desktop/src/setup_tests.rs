//! The setup screens: W1 to W3, S1 to S5, and how the gate moves the app.

use consort_client::Connection;
use consort_client::local_hub::LocalHub;
use consort_client::mock_local::{SETUP_SCREENS, screen_state, screen_view};
use consort_client::setup::{Gate, History, HubFacts};
use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::app::{ConsortApp, Page};
use crate::test_support::{bounds, click, draw, has, open_sized, settle, until};
use crate::ui::setup::SetupStep;

fn open_setup(
    screen: &str,
    width: f32,
    height: f32,
    cx: &mut TestAppContext,
) -> (VisualTestContext, Entity<ConsortApp>) {
    let state = screen_state(screen).expect("a state");
    let (handle, app) = open_sized(state, width, height, cx);
    let mut cx = VisualTestContext::from_window(handle, cx);
    settle(&mut cx, 50);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.open_screen(screen, window, cx));
    });
    settle(&mut cx, 50);
    (cx, app)
}

/// Opens the app with the gate on, as in the real app. The mock client keeps
/// these facts when its state changes.
fn gated(
    screen: &str,
    connection: Connection,
    hub: HubFacts,
    cx: &mut TestAppContext,
) -> (VisualTestContext, Entity<ConsortApp>) {
    gated_with(screen, connection, hub, |_| {}, cx)
}

fn gated_with(
    screen: &str,
    connection: Connection,
    hub: HubFacts,
    change: impl FnOnce(&mut consort_client::ViewState),
    cx: &mut TestAppContext,
) -> (VisualTestContext, Entity<ConsortApp>) {
    let mut state = screen_state(screen).expect("a state");
    change(&mut state);
    state.connection = connection;
    state.hub_setup = hub;
    let (handle, app) = open_sized(state, 1440., 900., cx);
    let mut cx = VisualTestContext::from_window(handle, cx);
    settle(&mut cx, 50);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.local = crate::ui::local_hub::LocalUi::with(LocalHub::fixed(
                screen_view(screen).expect("a view"),
            ));
            app.demo = false;
            app.follow_setup(cx);
        })
    });
    draw(&mut cx);
    (cx, app)
}

fn online() -> Connection {
    Connection::Online
}

fn new_hub() -> HubFacts {
    HubFacts {
        loaded: true,
        history: History::None,
        ..HubFacts::default()
    }
}

#[gpui::test]
fn each_screen_shows_what_it_is_for(cx: &mut TestAppContext) {
    for (screen, selector) in [
        ("setup-where", "local-install-here"),
        ("setup-where", "setup-mark"),
        ("setup-windows", "local-windows"),
        ("setup-bubblewrap", "local-bubblewrap"),
        ("setup-found", "local-connect"),
        ("setup-install", "local-step-checked"),
        ("setup-failed", "local-failed"),
        ("setup-connect", "connect-back"),
        ("setup-connect-unreachable", "onboarding-error-title"),
        ("setup-connect-refused", "onboarding-error-title"),
        ("setup-server", "setup-have-code"),
        ("setup-provider", "setup-provider-claude"),
        ("setup-provider", "setup-later"),
        ("setup-no-host", "setup-no-host"),
        ("setup-sign-in", "setup-back"),
        ("setup-tool-installing", "setup-installing"),
        ("setup-tool-installing", "setup-install-bar"),
        ("setup-tool-failed", "setup-install-failed"),
        ("setup-code-refused", "setup-code-failed"),
        ("setup-no-account", "no-account-state"),
    ] {
        let (mut cx, _) = open_setup(screen, 1440., 900., cx);
        assert!(has(&mut cx, selector), "{screen} shows {selector}");
    }
}

#[gpui::test]
fn every_screen_opens_at_the_compact_width_inside_the_window(cx: &mut TestAppContext) {
    for screen in SETUP_SCREENS {
        let (mut cx, _) = open_setup(screen, 420., 560., cx);
        if !has(&mut cx, "setup-column") {
            assert_eq!(screen, "setup-no-account", "{screen} has the setup column");
            continue;
        }
        let column = bounds(&mut cx, "setup-column");
        assert!(
            f32::from(column.origin.x) >= 0. && f32::from(column.right()) <= 420.,
            "{screen} stays inside the window: {column:?}"
        );
    }
}

#[gpui::test]
fn the_column_has_the_width_of_the_design(cx: &mut TestAppContext) {
    let (mut cx, _) = open_setup("setup-where", 1440., 900., cx);
    let column = bounds(&mut cx, "setup-column");
    let t = cx.update(|_, cx| ConsortApp::tokens(cx));
    assert_eq!(
        column.size.width,
        t.layout.setup_column + t.space.xl * 2.0,
        "the padding is outside the 448 px"
    );
}

#[gpui::test]
fn windows_has_no_install_and_a_way_to_a_server(cx: &mut TestAppContext) {
    let (mut cx, _) = open_setup("setup-windows", 1440., 900., cx);
    assert!(has(&mut cx, "local-windows"));
    assert!(!has(&mut cx, "local-install-here"));
    assert!(has(&mut cx, "local-connect-elsewhere"));
}

#[gpui::test]
fn no_bubblewrap_has_try_again_and_connect(cx: &mut TestAppContext) {
    let (mut cx, _) = open_setup("setup-bubblewrap", 1440., 900., cx);
    assert!(has(&mut cx, "local-try-again"));
    assert!(has(&mut cx, "local-connect-elsewhere"));
    assert!(!has(&mut cx, "local-install-here"));
}

#[gpui::test]
fn a_failed_install_has_try_again_and_show_log(cx: &mut TestAppContext) {
    let (mut cx, _) = open_setup("setup-failed", 1440., 900., cx);
    assert!(has(&mut cx, "local-try-again"));
    assert!(has(&mut cx, "local-log"));
    assert!(has(&mut cx, "local-connect-elsewhere"), "not a trap");
}

#[gpui::test]
fn connect_goes_to_s1_back_to_w1_and_on_to_s2(cx: &mut TestAppContext) {
    let (mut cx, app) = open_setup("setup-where", 1440., 900., cx);
    click(&mut cx, "local-connect-elsewhere");
    draw(&mut cx);
    app.read_with(&cx, |app, _| assert_eq!(app.page, Page::Onboarding));
    click(&mut cx, "connect-back");
    draw(&mut cx);
    assert!(has(&mut cx, "local-install-here"), "Back goes to W1");
    click(&mut cx, "local-connect-elsewhere");
    draw(&mut cx);
    click(&mut cx, "connect-no-hub");
    draw(&mut cx);
    app.read_with(&cx, |app, _| assert_eq!(app.setup.step, SetupStep::Server));
    assert!(has(&mut cx, "setup-have-code"));
    click(&mut cx, "setup-have-code");
    draw(&mut cx);
    app.read_with(&cx, |app, _| assert_eq!(app.page, Page::Onboarding));
}

#[gpui::test]
fn the_gate_takes_the_page_from_s3_to_main(cx: &mut TestAppContext) {
    let (mut cx, app) = gated("setup-provider", online(), new_hub(), cx);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.setup.gate, Gate::Provider);
        assert_eq!(app.page, Page::Setup);
    });
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            std::sync::Arc::make_mut(&mut app.state).hub_setup.signed_in = true;
            app.follow_setup(cx);
        })
    });
    app.read_with(&cx, |app, _| {
        assert_eq!(app.setup.gate, Gate::Main);
        assert_eq!(app.page, Page::Main);
    });
}

#[gpui::test]
fn the_gate_does_not_show_s3_before_the_inventory(cx: &mut TestAppContext) {
    let hub = HubFacts {
        inventory_pending: true,
        ..new_hub()
    };
    let (mut cx, _) = gated("setup-provider", online(), hub, cx);
    assert!(has(&mut cx, "setup-neutral"));
    assert!(!has(&mut cx, "setup-provider-claude"));
}

#[gpui::test]
fn no_key_and_no_local_hub_opens_w1(cx: &mut TestAppContext) {
    let (mut cx, app) = gated(
        "setup-where",
        Connection::NotPaired,
        HubFacts::default(),
        cx,
    );
    app.read_with(&cx, |app, _| assert_eq!(app.page, Page::Setup));
    assert!(has(&mut cx, "local-install-here"));
}

#[gpui::test]
fn a_local_hub_with_no_key_opens_its_card(cx: &mut TestAppContext) {
    let (mut cx, _) = gated(
        "setup-found",
        Connection::NotPaired,
        HubFacts::default(),
        cx,
    );
    assert!(has(&mut cx, "local-found"));
    assert!(has(&mut cx, "local-connect"));
}

#[gpui::test]
fn a_new_paired_client_on_a_set_up_hub_opens_main(cx: &mut TestAppContext) {
    let hub = HubFacts {
        signed_in: true,
        ..new_hub()
    };
    let (cx, app) = gated("setup-provider", online(), hub, cx);
    app.read_with(&cx, |app, _| assert_eq!(app.page, Page::Main));
}

#[gpui::test]
fn demo_mode_is_not_gated(cx: &mut TestAppContext) {
    let (mut cx, app) = open_setup("setup-no-account", 1440., 900., cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            std::sync::Arc::make_mut(&mut app.state).connection = Connection::NotPaired;
            app.page = Page::Main;
            app.follow_setup(cx);
        })
    });
    app.read_with(&cx, |app, _| assert_eq!(app.page, Page::Main));
}

#[gpui::test]
fn do_this_later_opens_tutti_and_t8_can_come_back(cx: &mut TestAppContext) {
    let (mut cx, app) = gated("setup-provider", online(), new_hub(), cx);
    click(&mut cx, "setup-later");
    settle(&mut cx, 50);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.setup.gate, Gate::NoAccount);
        assert_eq!(app.page, Page::Main);
        assert!(app.state.hub_setup.later, "the choice is in the hub state");
    });
    cx.update(|_, cx| app.update(cx, |app, cx| app.open_provider_step(cx)));
    draw(&mut cx);
    assert!(has(&mut cx, "setup-provider-claude"), "T8 opens S3");
    app.read_with(&cx, |app, _| assert_eq!(app.page, Page::Setup));
}

#[gpui::test]
fn a_provider_makes_an_account_with_its_short_name(cx: &mut TestAppContext) {
    let (mut cx, app) = gated("setup-provider", online(), new_hub(), cx);
    click(&mut cx, "setup-provider-claude");
    until(&mut cx, &app, "the account", |app| {
        app.state
            .session_host
            .as_ref()
            .is_some_and(|host| host.accounts.iter().any(|a| a.harness == "claude"))
    });
    app.read_with(&cx, |app, _| {
        let host = app.state.session_host.as_ref().expect("a host");
        let account = host
            .accounts
            .iter()
            .find(|a| a.harness == "claude")
            .expect("an account");
        assert_eq!(account.label, "Claude");
        assert_eq!(app.setup.step, SetupStep::SignIn);
    });
    draw(&mut cx);
    assert!(has(&mut cx, "setup-back"));
    click(&mut cx, "setup-back");
    draw(&mut cx);
    assert!(has(&mut cx, "setup-provider-claude"));
}

#[gpui::test]
fn a_tool_that_installs_holds_the_sign_in(cx: &mut TestAppContext) {
    let (mut cx, app) = open_setup("setup-tool-installing", 1440., 900., cx);
    settle(&mut cx, 50);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.add_account.sign_in_id, None, "the sign-in waits");
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.auto_start_sign_in(window, cx));
    });
    settle(&mut cx, 50);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.add_account.sign_in_id, None, "it still waits");
    });
}

#[gpui::test]
fn the_session_host_that_connects_continues_s5(cx: &mut TestAppContext) {
    let (mut cx, app) = gated("setup-no-host", online(), new_hub(), cx);
    assert!(has(&mut cx, "setup-no-host"));
    let host = screen_state("setup-provider").and_then(|s| s.session_host);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            std::sync::Arc::make_mut(&mut app.state).session_host = host;
            app.follow_setup(cx);
        })
    });
    draw(&mut cx);
    assert!(!has(&mut cx, "setup-no-host"));
    assert!(has(&mut cx, "setup-provider-claude"));
}

fn set_tutti_selection(
    state: &mut consort_client::ViewState,
    selection: Option<consort_client::EffectiveSelection>,
) {
    let tutti = state.tutti().map(|t| t.id.clone()).expect("Tutti");
    if let Some(channel) = state.channels.iter_mut().find(|c| c.id == tutti) {
        channel.coordinator_selection = selection;
    }
}

fn sign_in(cx: &mut VisualTestContext, app: &Entity<ConsortApp>) {
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            std::sync::Arc::make_mut(&mut app.state).hub_setup.signed_in = true;
            app.follow_setup(cx);
        })
    });
    settle(cx, 50);
}

#[gpui::test]
fn the_first_sign_in_gives_tutti_the_first_model(cx: &mut TestAppContext) {
    let (mut cx, app) = gated_with(
        "setup-provider",
        online(),
        new_hub(),
        |state| {
            set_tutti_selection(state, None);
            state.model_groups = consort_client::mock::view_state().model_groups;
        },
        cx,
    );
    let expected = app.read_with(&cx, |app, _| {
        assert!(
            app.setup.pending_model,
            "the provider step waits for a model"
        );
        consort_client::setup::default_tutti_selection(&app.state.model_groups)
            .expect("the mock has models")
    });
    sign_in(&mut cx, &app);
    app.read_with(&cx, |app, _| {
        let tutti = app.state.tutti().expect("Tutti");
        assert_eq!(
            tutti
                .coordinator_selection
                .as_ref()
                .map(|s| &s.selection.model_id),
            Some(&expected.model_id)
        );
    });
    // The mock client keeps its own facts, so its next state says "signed out"
    // and the step waits again. With a model on Tutti, the wait ends.
    sign_in(&mut cx, &app);
    app.read_with(&cx, |app, _| assert!(!app.setup.pending_model));
}

#[gpui::test]
fn the_first_sign_in_keeps_a_model_that_tutti_has(cx: &mut TestAppContext) {
    let (mut cx, app) = gated("setup-provider", online(), new_hub(), cx);
    let before = app.read_with(&cx, |app, _| {
        app.state
            .tutti()
            .and_then(|t| t.coordinator_selection.clone())
            .expect("the mock Tutti has a model")
    });
    sign_in(&mut cx, &app);
    app.read_with(&cx, |app, _| {
        assert!(!app.setup.pending_model);
        assert_eq!(
            app.state
                .tutti()
                .and_then(|t| t.coordinator_selection.clone()),
            Some(before)
        );
    });
}

#[gpui::test]
fn a_hub_that_was_signed_in_gets_no_model_from_the_app(cx: &mut TestAppContext) {
    let hub = HubFacts {
        signed_in: true,
        ..new_hub()
    };
    let (mut cx, app) = gated_with(
        "setup-provider",
        online(),
        hub,
        |state| set_tutti_selection(state, None),
        cx,
    );
    settle(&mut cx, 50);
    app.read_with(&cx, |app, _| {
        assert!(!app.setup.pending_model);
        assert_eq!(
            app.state
                .tutti()
                .and_then(|t| t.coordinator_selection.clone()),
            None,
            "the user removed the model, or chose none: the app leaves it"
        );
    });
}
