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

/// A wide window, so the Channel panel has its full width.
fn open_wide(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
    });
    let options = WindowOptions {
        window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
            origin: gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
            size: gpui_kit::size(gpui_kit::px(1440.), gpui_kit::px(900.)),
        })),
        ..WindowOptions::default()
    };
    let (handle, app) = cx
        .update(|cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    ConsortApp::new(
                        Client::mock_with(consort_client::mock::view_state()).expect("mock"),
                        true,
                        window,
                        cx,
                    )
                })
            })
        })
        .expect("open the window");
    (handle, app)
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let name: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    cx.debug_bounds(name)
        .unwrap_or_else(|| panic!("{selector} draws"))
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).center();
    cx.simulate_click(at, gpui_kit::Modifiers::default());
    settle(cx);
}

fn enabled_connectors(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) -> Vec<String> {
    app.read_with(cx, |app, _| {
        consort_client::channel_extensions(&app.state, mx::ACME)
            .expect("acme")
            .enabled_connector_ids()
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    })
}

/// A switch in the Connectors tab turns a connector on in the channel, and
/// the effective set follows.
#[gpui::test]
fn the_channel_switch_turns_a_connector_on_in_the_channel(cx: &mut TestAppContext) {
    let (handle, app) = open_wide(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert!(!enabled_connectors(&app, cx).contains(&mx::CON_GMAIL_PERSONAL.to_owned()));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel_connectors(mx::ACME.into(), window, cx);
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
    assert!(enabled_connectors(&app, cx).contains(&mx::CON_GMAIL_PERSONAL.to_owned()));
}

/// The Connectors tab opens from the channel row menu, lists the connectors
/// and skills of the channel, turns one off with its switch, and Manage all
/// in Settings opens Settings > Connectors.
#[gpui::test]
fn the_connectors_tab_lists_the_items_and_the_switch_turns_one_off(cx: &mut TestAppContext) {
    let (handle, app) = open_wide(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    // The menu item opens the channel, the panel and the tab.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                crate::ui::menu::MenuAction::ChannelExtensions(mx::ACME.into()),
                window,
                cx,
            )
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, Page::Main);
        assert_eq!(app.channel_id, mx::ACME);
        assert_eq!(app.panel, crate::app::Panel::Channel);
        assert_eq!(app.channel_tab, crate::app::ChannelTab::Connectors);
        assert!(app.dialog.is_none());
    });
    let on = enabled_connectors(&app, cx);
    assert!(!on.is_empty(), "the channel has a connector on");
    let skills_on = app.read_with(cx, |app, _| {
        consort_client::channel_extensions(&app.state, mx::ACME)
            .expect("acme")
            .enabled_skill_ids()
            .len()
    });
    // The tab lists the items, each with its switch.
    for id in &on {
        bounds(cx, &format!("channel-item-{id}"));
    }
    app.read_with(cx, |app, _| {
        assert_eq!(app.connectors_tab_count(mx::ACME), on.len() + skills_on)
    });
    bounds(cx, "channel-add-connector");
    // The rows stay inside the panel.
    let panel = bounds(cx, "channel-panel");
    let first = bounds(cx, &format!("channel-item-{}", on[0]));
    assert!(first.right() <= panel.right());
    // A click on the switch turns the connector off in this channel.
    click(cx, &format!("channel-item-{}", on[0]));
    assert!(!enabled_connectors(&app, cx).contains(&on[0]));
    assert_eq!(enabled_connectors(&app, cx).len(), on.len() - 1);
    // Manage all in Settings opens Settings > Connectors.
    click(cx, "channel-manage-connectors");
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, Page::Settings);
        assert_eq!(app.settings_section, SettingsSection::Connectors);
    });
}

/// Add a connector opens the Add connector dialog, and the dialog of the
/// channel settings leads to the tab.
#[gpui::test]
fn the_connectors_tab_opens_the_add_connector_dialog(cx: &mut TestAppContext) {
    let (handle, app) = open_wide(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel_connectors(mx::ACME.into(), window, cx)
        })
    });
    settle(cx);
    click(cx, "channel-add-connector");
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog.as_ref(), Some(&Dialog::AddConnector));
        assert_eq!(app.page, Page::Main);
    });
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

/// Whether an element with this debug selector is on screen. The selector
/// leaks, which is fine in a test.
fn has_bounds(cx: &mut VisualTestContext, selector: String) -> bool {
    let selector: &'static str = Box::leak(selector.into_boxed_str());
    cx.debug_bounds(selector).is_some()
}

fn labels(items: &[crate::ui::menu::MenuItem]) -> Vec<&str> {
    items.iter().map(|i| i.label.as_ref()).collect()
}

/// Settings > Skills lists the sources of the skill directory. Only a source
/// that the user added has a row menu, and its one item is Remove. skills.sh
/// is a link and not a source.
#[gpui::test]
fn settings_skills_lists_the_sources_and_only_added_ones_can_be_removed(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| app.update(cx, |app, cx| app.open_settings(SettingsSection::Skills, cx)));
    settle(cx);
    let sources = app.read_with(cx, |app, _| app.state.skill_directory.sources.clone());
    assert_eq!(
        sources.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["smithery", "anthropic", "src_acme"]
    );
    for id in ["smithery", "anthropic", "src_acme"] {
        assert!(
            has_bounds(cx, format!("skill-source-{id}")),
            "the row of {id}"
        );
    }
    assert!(cx.debug_bounds("add-skill-source").is_some());
    assert!(cx.debug_bounds("skills-sh-link").is_some(), "skills.sh");
    for source in &sources {
        let menu = crate::ui::menu::skill_source_menu(source);
        if source.removable {
            let items = menu.expect("an added source has a menu");
            assert_eq!(labels(&items), ["Remove"]);
            assert_eq!(
                items[0].action,
                crate::ui::menu::MenuAction::RemoveSkillSource(source.id.clone())
            );
        } else {
            assert!(menu.is_none(), "{} cannot be removed", source.name);
        }
    }
}

/// Add source asks for one URL, says what to do when it is empty, calls
/// `skill_directory.add_source`, and closes. Remove calls
/// `skill_directory.remove_source`.
#[gpui::test]
fn add_source_and_remove_call_the_directory_requests(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| app.update(cx, |app, cx| app.open_settings(SettingsSection::Skills, cx)));
    settle(cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(Dialog::AddSkillSource, window, cx)
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::AddSkillSource));
    });
    // An empty field says what to paste.
    cx.update(|window, cx| app.update(cx, |app, cx| app.add_skill_source(window, cx)));
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.dialog_error.as_deref(),
            Some("Paste the URL of a git repo or a marketplace.json file.")
        );
        assert_eq!(app.dialog, Some(Dialog::AddSkillSource), "the dialog stays");
        assert_eq!(app.state.skill_directory.sources.len(), 3);
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.ext_inputs.source_url.update(cx, |input, cx| {
                input.set_value("https://github.com/owner/repo", window, cx)
            });
            app.add_skill_source(window, cx);
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert!(app.dialog.is_none(), "the dialog closes");
        let added = app
            .state
            .skill_directory
            .sources
            .iter()
            .find(|s| s.url == "https://github.com/owner/repo")
            .expect("the new source");
        assert!(added.removable);
        assert_eq!(added.name, "owner/repo");
        assert_eq!(app.state.skill_directory.sources.len(), 4);
    });
    // Remove takes an added source away. The built-in ones stay.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                crate::ui::menu::MenuAction::RemoveSkillSource("src_acme".into()),
                window,
                cx,
            )
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        let ids: Vec<&str> = app
            .state
            .skill_directory
            .sources
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert!(!ids.contains(&"src_acme"));
        assert!(ids.contains(&"smithery") && ids.contains(&"anthropic"));
        assert_eq!(ids.len(), 3);
    });
}

/// The Activity item of a connection opens a dialog with the calls: the
/// tool, the decision, the outcome, the time and the size. Show more loads
/// the older page.
#[gpui::test]
fn the_activity_dialog_lists_the_calls_and_pages(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(SettingsSection::Connectors, cx)
        })
    });
    settle(cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                crate::ui::menu::MenuAction::ConnectionActivity(mx::CON_GITHUB.into()),
                window,
                cx,
            )
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.dialog,
            Some(Dialog::ConnectionActivity(mx::CON_GITHUB.into()))
        );
        let calls = &app.ext.activity.calls;
        assert_eq!(calls.len(), 3, "the newest page");
        assert!(app.ext.activity.has_more);
        let sent = &calls[1];
        assert_eq!(sent.tool, "GMAIL_SEND_EMAIL");
        assert_eq!(sent.decision, "Allowed by you");
        assert!(sent.succeeded);
        assert_eq!(sent.size_label, "412 B");
        let denied = &calls[2];
        assert!(!denied.succeeded);
        assert_eq!(denied.error.as_deref(), Some("You said no."));
    });
    for id in ["ccl_mock_3", "ccl_mock_2", "ccl_mock_1"] {
        assert!(has_bounds(cx, format!("call-{id}")), "{id}");
    }
    assert!(cx.debug_bounds("call-ccl_mock_0").is_none());
    assert!(cx.debug_bounds("activity-more").is_some(), "Show more");
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.load_activity(mx::CON_GITHUB.into(), true, cx)
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.ext.activity.calls.len(), 4);
        assert!(!app.ext.activity.has_more);
        assert_eq!(app.ext.activity.calls[3].id, "ccl_mock_0");
    });
    assert!(cx.debug_bounds("call-ccl_mock_0").is_some());
    assert!(cx.debug_bounds("activity-more").is_none(), "no more pages");
}

/// A page that fails says what happened and what to do next, and Try again
/// loads the first page again.
#[gpui::test]
fn the_activity_dialog_says_when_the_calls_do_not_load(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(
                Dialog::ConnectionActivity(mx::CON_GITHUB.into()),
                window,
                cx,
            )
        })
    });
    settle(cx);
    cx.update(|window, cx| {
        app.update(cx, |app, _| {
            app.ext.activity.calls.clear();
            app.ext.activity.has_more = false;
            app.ext.activity.error = Some(
                "Consort could not load the activity: the hub does not answer. Try again.".into(),
            );
        });
        window.refresh();
    });
    settle(cx);
    assert!(cx.debug_bounds("activity-retry").is_some(), "Try again");
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.load_activity(mx::CON_GITHUB.into(), false, cx)
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert!(app.ext.activity.error.is_none());
        assert_eq!(app.ext.activity.calls.len(), 3);
    });
    assert!(cx.debug_bounds("activity-retry").is_none());
}

/// The Use in that the first skill of the mock has now.
fn skill_use_in(cx: &mut VisualTestContext, app: &Entity<ConsortApp>) -> UseInView {
    app.read_with(cx, |app, _| {
        app.state
            .skill(mx::SKL_RESEARCH)
            .expect("the skill")
            .use_in
            .clone()
    })
}

fn picker_open(cx: &mut VisualTestContext, app: &Entity<ConsortApp>) -> bool {
    app.read_with(cx, |app, _| app.ext.use_in_picker.is_some())
}

#[gpui::test]
fn use_in_has_tutti_only_every_channel_and_a_dialog_for_the_rest(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| app.update(cx, |app, cx| app.open_settings(SettingsSection::Skills, cx)));
    settle(cx);
    let (tutti, channel) = app.read_with(cx, |app, _| {
        let tutti = app.state.sidebar.tutti.id.clone();
        let channel = app
            .state
            .channels
            .iter()
            .find(|c| c.id != tutti)
            .expect("a channel")
            .id
            .clone();
        (tutti, channel)
    });

    // Tutti only: the list has Tutti and no other channel.
    click(cx, "use-in-skill-tutti");
    let only = skill_use_in(cx, &app);
    assert_eq!(only, UseInView::Channels(vec![tutti.clone()]));
    app.read_with(cx, |app, _| {
        let names = consort_client::matching_channels(&only, &app.state.channels);
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].id, tutti);
    });

    click(cx, "use-in-skill-every");
    assert_eq!(skill_use_in(cx, &app), UseInView::EveryChannel);

    // An empty choice has no Save.
    click(cx, "use-in-skill-choose");
    assert!(picker_open(cx, &app));
    click(cx, "use-in-save");
    assert!(picker_open(cx, &app), "nothing is chosen yet");
    assert_eq!(skill_use_in(cx, &app), UseInView::EveryChannel);

    // Channels, with Tutti in the list.
    let row = format!("use-in-row-{channel}");
    click(cx, &row);
    click(cx, &format!("use-in-row-{tutti}"));
    assert_eq!(
        skill_use_in(cx, &app),
        UseInView::EveryChannel,
        "nothing changes until Save"
    );
    click(cx, "use-in-save");
    assert!(!picker_open(cx, &app));
    assert_eq!(
        skill_use_in(cx, &app),
        UseInView::Channels(vec![channel.clone(), tutti.clone()])
    );

    // It opens on what is chosen. Cancel keeps the scope.
    click(cx, "use-in-skill-choose");
    click(cx, &row);
    click(cx, "use-in-cancel");
    assert!(!picker_open(cx, &app));
    assert_eq!(
        skill_use_in(cx, &app),
        UseInView::Channels(vec![channel.clone(), tutti.clone()])
    );

    // Tags replace the channels. The card keeps its place on either tab.
    click(cx, "use-in-skill-choose");
    let on_channels = bounds(cx, "use-in-picker");
    click(cx, "use-in-tab-tags");
    assert_eq!(bounds(cx, "use-in-picker"), on_channels);
    click(cx, "use-in-row-work");
    click(cx, "use-in-save");
    assert_eq!(
        skill_use_in(cx, &app),
        UseInView::Tags(vec!["work".to_owned()])
    );
    click(cx, "use-in-skill-choose");
    app.read_with(cx, |app, _| {
        let picker = app.ext.use_in_picker.as_ref().expect("open");
        assert_eq!(picker.tab, crate::extensions::UseInPickerTab::Tags);
        assert_eq!(picker.tags, ["work"]);
    });
}

#[gpui::test]
fn the_use_in_dialog_opens_over_the_connect_dialog_and_closes_alone(cx: &mut TestAppContext) {
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
    let connect_use_in =
        |cx: &mut VisualTestContext| app.read_with(cx, |app, _| app.ext.connect.use_in.clone());
    let before = connect_use_in(cx);

    click(cx, "use-in-connect-choose");
    assert!(picker_open(cx, &app));
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(!picker_open(cx, &app), "Escape closes the Use in dialog");
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.dialog,
            Some(Dialog::Connect),
            "the dialog under it stays"
        );
    });
    assert_eq!(connect_use_in(cx), before);

    // A click outside the card closes only the Use in dialog too.
    click(cx, "use-in-connect-choose");
    let card = bounds(cx, "use-in-picker");
    cx.simulate_click(
        gpui_kit::point(card.origin.x, card.origin.y)
            - gpui_kit::point(gpui_kit::px(8.), gpui_kit::px(8.)),
        gpui_kit::Modifiers::default(),
    );
    settle(cx);
    assert!(!picker_open(cx, &app));
    app.read_with(cx, |app, _| assert_eq!(app.dialog, Some(Dialog::Connect)));

    click(cx, "use-in-connect-tutti");
    let tutti = app.read_with(cx, |app, _| app.state.sidebar.tutti.id.clone());
    assert_eq!(connect_use_in(cx), UseInView::Channels(vec![tutti]));
}
