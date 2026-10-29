//! Connectors, skills and the hub cards in a headless window, on the mock.

use crate::app::{ConsortApp, Dialog, Page, SettingsSection};
use consort_client::{
    Client, ConnectionStatusView, RequestState, TimelineItem, UseInView, ViewState,
    mock_extensions as mx,
};
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Entity, WindowOptions};

fn open_app(cx: &mut TestAppContext, state: ViewState) -> (AnyWindowHandle, Entity<ConsortApp>) {
    // The mock client runs its own thread.
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
    });
    let (handle, app) = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    ConsortApp::new(Client::mock_with(state).expect("mock"), true, window, cx)
                })
            })
        })
        .expect("open the window");
    (handle, app)
}

/// Lets the mock client answer and the window draw.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

#[gpui::test]
fn every_connector_and_skill_page_and_dialog_opens(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    for section in [SettingsSection::Connectors, SettingsSection::Skills] {
        cx.update(|_, cx| app.update(cx, |app, cx| app.open_settings(section, cx)));
        settle(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.page, Page::Settings);
            assert_eq!(app.settings_section, section);
        });
    }
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.ext.skill.as_deref(),
            Some(mx::SKL_RESEARCH),
            "the first skill opens"
        );
    });
    let dialogs = [
        Dialog::ComposioKey,
        Dialog::AddConnector,
        Dialog::ConfirmDisconnect(mx::CON_GITHUB.into()),
        Dialog::AddSkill,
        Dialog::ConfirmRemoveSkill(mx::SKL_PDF.into()),
        Dialog::ChannelExtensions(mx::ACME.into()),
        Dialog::AccountSettings("wrk_bigboi".into(), "acc_bigboi_claude".into()),
    ];
    for dialog in dialogs {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_dialog(dialog.clone(), window, cx))
        });
        settle(cx);
        app.read_with(cx, |app, _| assert_eq!(app.dialog.as_ref(), Some(&dialog)));
    }
    // The Add connector dialog searched the catalog, and Add skill the
    // directories.
    app.read_with(cx, |app, _| {
        assert!(!app.state.connector_catalog.services.is_empty());
        assert!(!app.state.skill_directory.entries.is_empty());
    });
    // The files dialog loads the files and opens SKILL.md.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_skill_files(mx::SKL_RESEARCH.into(), window, cx)
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.ext.files.files.len(), 3);
        assert_eq!(
            app.ext.files.open.as_ref().map(|f| f.path.as_str()),
            Some("SKILL.md")
        );
    });
}

/// The Connect dialog starts a connection, waits for the browser, and
/// closes when the connection turns active.
#[gpui::test]
fn the_connect_dialog_follows_the_connection_until_it_is_active(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(SettingsSection::Connectors, cx);
            app.open_dialog(Dialog::AddConnector, window, cx);
        })
    });
    settle(cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let github = app
                .state
                .connector_catalog
                .services
                .iter()
                .find(|s| s.slug == "github")
                .cloned()
                .expect("GitHub in the catalog");
            app.open_connect(github, None, true, window, cx);
        })
    });
    settle(cx);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.dialog, Some(Dialog::Connect));
        // GitHub is connected once already, under that name.
        assert_eq!(app.ext_inputs.connect_name.read(cx).value(), "");
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.ext_inputs
                .connect_name
                .update(cx, |input, cx| input.set_value("GitHub Work", window, cx));
            app.ext.connect.use_in = UseInView::Tags(vec!["work".into()]);
            app.submit_connect(window, cx);
        })
    });
    settle(cx);
    let id = app.read_with(cx, |app, _| {
        let id = app
            .ext
            .connect
            .attempt
            .clone()
            .expect("the attempt started");
        let connection = app.state.connection(&id).expect("the connection");
        assert!(matches!(
            connection.status,
            ConnectionStatusView::Connecting { .. }
        ));
        assert_eq!(connection.handle, "github_work");
        assert_eq!(app.dialog, Some(Dialog::Connect), "the dialog waits");
        id
    });
    // The mock finishes the sign-in a moment later.
    for _ in 0..80 {
        settle(cx);
        if app.read_with(cx, |app, _| app.dialog.is_none()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    app.read_with(cx, |app, _| {
        assert!(
            app.dialog.is_none(),
            "an active connection closes the dialog"
        );
        assert!(
            app.state
                .connection(&id)
                .is_some_and(|c| c.status.is_active())
        );
        assert_eq!(app.ext.connection.as_deref(), Some(id.as_str()));
    });
}

fn card_state(state: &ViewState, approval_id: &str) -> Option<RequestState> {
    state.timelines.iter().find_map(|t| {
        t.items.iter().find_map(|i| match i {
            TimelineItem::Approval(card) if card.approval.id == approval_id => {
                Some(card.approval.state)
            }
            _ => None,
        })
    })
}

/// Allow on a connector card answers it, and Continue in browser on a
/// connect card answers it and connects with the card's settings.
#[gpui::test]
fn hub_cards_answer_with_their_options(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(mx::ACME.into(), window, cx);
            app.answer_approval(
                mx::APR_SEND_EMAIL.into(),
                consort_client::ApprovalChoice::AllowOnce,
                window,
                cx,
            );
            app.continue_connect_card(mx::APR_CONNECT_DRIVE.into(), cx);
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(
            card_state(&app.state, mx::APR_SEND_EMAIL),
            Some(RequestState::Answered)
        );
        assert_eq!(
            card_state(&app.state, mx::APR_CONNECT_DRIVE),
            Some(RequestState::Answered)
        );
        let drive = app
            .state
            .connections
            .iter()
            .find(|c| c.name == "Drive Work")
            .expect("the card connected Drive Work");
        assert_eq!(drive.use_in, UseInView::Channels(vec![mx::HIRING.into()]));
        // The demo does not open a browser, but it noted the page as opened.
        assert!(app.ext.opened.contains(&drive.id));
        assert!(app.ext.browser_cards.is_empty());
    });
}

/// A switch in the channel dialog turns a connector on in the channel, and
/// the effective set follows.
#[gpui::test]
fn the_channel_switch_turns_a_connector_on_in_the_channel(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let enabled = |app: &Entity<ConsortApp>, cx: &mut VisualTestContext| {
        app.read_with(cx, |app, _| {
            consort_client::channel_extensions(&app.state, mx::ACME)
                .expect("acme")
                .enabled_connector_ids()
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>()
        })
    };
    assert!(!enabled(&app, cx).contains(&mx::CON_GMAIL_PERSONAL.to_owned()));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(Dialog::ChannelExtensions(mx::ACME.into()), window, cx);
            app.set_channel_item(
                mx::ACME.into(),
                mx::CON_GMAIL_PERSONAL.into(),
                true,
                true,
                cx,
            );
        })
    });
    settle(cx);
    assert!(enabled(&app, cx).contains(&mx::CON_GMAIL_PERSONAL.to_owned()));
}

/// The account dialog turns the provider extras off.
#[gpui::test]
fn the_account_dialog_turns_the_extras_off(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let client = app.client.clone();
            app.spawn_command(cx, async move {
                client
                    .run(consort_client::Command::SetAccountExtras {
                        worker_id: "wrk_bigboi".into(),
                        account_id: "acc_bigboi_claude".into(),
                        enabled: false,
                    })
                    .await
            });
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        let account = app
            .state
            .worker("wrk_bigboi")
            .and_then(|w| w.account("acc_bigboi_claude"))
            .expect("the account");
        assert!(!account.extras_enabled);
    });
}

/// A hub without the skill pipeline answers `unsupported`: the dialog shows
/// that in place of results, and a check says it as an error.
#[gpui::test]
fn the_add_skill_dialog_shows_an_unsupported_hub(cx: &mut TestAppContext) {
    let mut state = consort_client::mock::view_state();
    state.skill_directory.load = consort_client::LoadState::Unsupported(
        "This hub cannot search skill directories yet. Update the hub, then try again.".into(),
    );
    let (handle, app) = open_app(cx, state);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(SettingsSection::Skills, cx);
            app.open_dialog(Dialog::AddSkill, window, cx);
            app.ext.add_skill.tab = crate::extensions::AddSkillTab::Git;
            app.ext_inputs.git_url.update(cx, |input, cx| {
                input.set_value("https://github.com/anthropics/skills", window, cx)
            });
            app.check_git_skill(cx);
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert!(matches!(
            app.state.skill_directory.load,
            consort_client::LoadState::Unsupported(_)
        ));
        assert_eq!(
            app.dialog_error.as_deref(),
            Some("This hub cannot add skills yet. Update the hub, then try again.")
        );
        assert!(app.ext.add_skill.preview.is_none());
    });
}

/// Sign in on a sign-in card with a method: Allow makes the hub start the
/// sign-in, and the sign-in dialog opens on it.
#[gpui::test]
fn the_sign_in_card_opens_the_sign_in_that_the_hub_started(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(mx::HIRING.into(), window, cx);
            app.sign_in_from_card(mx::APR_SIGN_IN.into(), window, cx);
        })
    });
    settle(cx);
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(
            card_state(&app.state, mx::APR_SIGN_IN),
            Some(RequestState::Answered)
        );
        assert_eq!(
            app.dialog,
            Some(Dialog::AddAccount("wrk_bigboi".into())),
            "the sign-in dialog opens"
        );
        let id = app
            .add_account
            .sign_in_id
            .clone()
            .expect("on the hub's sign-in");
        assert!(app.state.sign_in(&id).is_some());
        assert_eq!(app.add_account.step, crate::app::AccountStep::SignIn);
    });
}
