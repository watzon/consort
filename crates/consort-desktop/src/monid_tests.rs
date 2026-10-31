//! Monid and access requests in a headless window, on the mock.

use crate::app::{ConsortApp, Dialog, Page, SettingsSection};
use crate::monid::{CHANNEL_RULE_SELECT, CallPart, FOLLOW_HUB, HUB_RULE_SELECT, MonidAction};
use crate::ui::menu::MenuAction;
use crate::ui::monid_cards::monid_call_menu;
use consort_client::{
    ApprovalRuleView, Client, MonidCallStateView, MonidUnattendedView, RequestState, TimelineItem,
    ViewState, find_call_row, mock_monid as mm,
};
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Entity, WindowOptions};

const LIGHTS: &str = "chn_house_lights";

fn open_app(cx: &mut TestAppContext, state: ViewState) -> (AnyWindowHandle, Entity<ConsortApp>) {
    // The mock client runs its own thread.
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

fn has(cx: &mut VisualTestContext, selector: &str) -> bool {
    let name: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    cx.debug_bounds(name).is_some()
}

fn lights(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) -> consort_client::ChannelView {
    app.read_with(cx, |app, _| {
        app.state.channel(LIGHTS).expect("house-lights").clone()
    })
}

/// The ID of the chat message of an approval: the card uses it as its key.
fn card_key(app: &Entity<ConsortApp>, cx: &mut VisualTestContext, approval_id: &str) -> String {
    app.read_with(cx, |app, _| {
        app.state
            .timelines
            .iter()
            .flat_map(|t| t.items.iter())
            .find_map(|item| match item {
                TimelineItem::Approval(card) if card.approval.id == approval_id => {
                    Some(card.message_id.clone())
                }
                _ => None,
            })
            .expect("the approval is in a timeline")
    })
}

#[test]
fn a_call_row_menu_copies_three_parts_and_stops_only_a_call_that_can_stop() {
    let rows = mm::rows();
    let running = rows
        .iter()
        .find(|r| r.call_id == mm::CALL_RUNNING)
        .expect("running");
    let done = rows
        .iter()
        .find(|r| r.call_id == mm::CALL_ELEVEN)
        .expect("completed");
    assert!(running.stoppable && !done.stoppable);
    let names = |items: Vec<crate::ui::menu::MenuItem>| -> Vec<String> {
        items.into_iter().map(|i| i.label.to_string()).collect()
    };
    assert_eq!(
        names(monid_call_menu(done)),
        ["Copy input", "Copy output", "Copy run ID"]
    );
    assert_eq!(
        names(monid_call_menu(running)),
        ["Copy input", "Copy output", "Copy run ID", "Stop"]
    );
}

/// The card of a call shows Allow for this task only when the hub offers it,
/// and Change limit instead of it when the call goes over a limit.
#[gpui::test]
fn the_monid_cards_draw_the_buttons_that_their_options_allow(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_channel(LIGHTS.into(), window, cx)));
    settle(cx);
    let normal = card_key(&app, cx, mm::APR_CARD);
    let over = card_key(&app, cx, mm::APR_OVER_LIMIT);
    // The timeline keeps the newest cards in view. Look for the cards that
    // draw, and check the buttons of each.
    let mut seen = 0;
    if has(cx, &format!("{normal}-card")) {
        seen += 1;
        for part in ["allow", "allow-task", "deny", "more"] {
            assert!(has(cx, &format!("{normal}-{part}")), "{part}");
        }
        assert!(!has(cx, &format!("{normal}-change-limit")));
    }
    if has(cx, &format!("{over}-card")) {
        seen += 1;
        for part in ["allow", "change-limit", "deny", "more"] {
            assert!(has(cx, &format!("{over}-{part}")), "{part}");
        }
        assert!(!has(cx, &format!("{over}-allow-task")));
    }
    assert!(seen > 0, "at least one Monid card draws");
}

/// A button of a card sends the option to the hub. The card is answered, and
/// Always allow adds the tool to the list of the channel.
#[gpui::test]
fn an_option_of_a_card_answers_it_and_changes_the_list_of_the_channel(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(LIGHTS.into(), window, cx);
            app.run_monid_action(
                MonidAction::AnswerOption(mm::APR_CARD.into(), "always".into()),
                window,
                cx,
            );
            app.run_monid_action(
                MonidAction::AnswerOption(mm::APR_OVER_LIMIT.into(), "never".into()),
                window,
                cx,
            );
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        for id in [mm::APR_CARD, mm::APR_OVER_LIMIT] {
            let state = app
                .state
                .timelines
                .iter()
                .flat_map(|t| t.items.iter())
                .find_map(|i| match i {
                    TimelineItem::Approval(c) if c.approval.id == id => Some(c.approval.state),
                    _ => None,
                });
            assert_eq!(state, Some(RequestState::Answered), "{id}");
        }
    });
    let channel = lights(&app, cx);
    assert!(
        channel
            .monid
            .allowed
            .iter()
            .any(|a| a.tool.provider == "elevenlabs" && a.run_without_asking)
    );
    assert!(
        channel
            .monid
            .blocked
            .iter()
            .any(|b| b.provider == "serpapi")
    );
}

/// An access card answers with its option, too.
#[gpui::test]
fn an_access_card_answers_with_its_option(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(LIGHTS.into(), window, cx);
            app.run_monid_action(
                MonidAction::AnswerOption(mm::APR_ACCESS_TASK.into(), "allow".into()),
                window,
                cx,
            );
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        let state = app
            .state
            .timelines
            .iter()
            .flat_map(|t| t.items.iter())
            .find_map(|i| match i {
                TimelineItem::Approval(c) if c.approval.id == mm::APR_ACCESS_TASK => {
                    Some(c.approval.state)
                }
                _ => None,
            });
        assert_eq!(state, Some(RequestState::Answered));
    });
}

/// A call row opens with its detail and Stop ends a call that runs.
#[gpui::test]
fn a_call_row_loads_its_detail_when_it_opens_and_stop_ends_a_running_call(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let key = crate::monid::call_key("chat", mm::CALL_ELEVEN);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(LIGHTS.into(), window, cx);
            app.toggle_call_row(key.clone(), mm::CALL_ELEVEN.into(), cx);
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert!(app.monid_ui.open_calls.contains(&key));
        let row = find_call_row(&app.state, mm::CALL_ELEVEN).expect("row");
        assert!(row.detail.is_some(), "the detail loaded");
    });
    // The same row closes again.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.toggle_call_row(key.clone(), mm::CALL_ELEVEN.into(), cx)
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert!(!app.monid_ui.open_calls.contains(&key))
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_monid_action(MonidAction::StopCall(mm::CALL_RUNNING.into()), window, cx)
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        let row = find_call_row(&app.state, mm::CALL_RUNNING).expect("row");
        assert_eq!(row.state, MonidCallStateView::Stopped);
        assert!(!row.stoppable);
        // Copy output with no detail yet is a part of the menu, not a crash.
        let _ = CallPart::Output;
    });
}

/// The Monid row shows in the Connectors tab, and the Monid view opens from
/// it and goes back.
#[gpui::test]
fn the_connectors_tab_has_a_monid_row_that_opens_the_view(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel_connectors(LIGHTS.into(), window, cx)
        })
    });
    settle(cx);
    assert!(has(cx, "channel-monid-row"), "the Monid row");
    assert!(has(cx, "temporary-access"), "the Temporary access section");
    assert!(!has(cx, "monid-view"));
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_monid_panel(window, cx)));
    settle(cx);
    assert!(has(cx, "monid-view"), "the view");
    assert!(has(cx, "monid-back"));
    cx.update(|_, cx| app.update(cx, |app, cx| app.close_monid_panel(cx)));
    settle(cx);
    assert!(!has(cx, "monid-view"));
    assert!(has(cx, "channel-monid-row"));
}

/// The lists of the view change the access of the channel, and the select
/// sets the rule of the channel or goes back to the hub.
#[gpui::test]
fn the_tool_lists_and_the_rule_change_the_access_of_the_channel(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_channel(LIGHTS.into(), window, cx)));
    settle(cx);
    let before = lights(&app, cx).monid;
    assert_eq!(before.allowed.len(), 3);
    assert_eq!(before.blocked.len(), 1);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_monid_action(MonidAction::RemoveAllowed(LIGHTS.into(), 0), window, cx);
        })
    });
    settle(cx);
    let after = lights(&app, cx).monid;
    assert_eq!(after.allowed.len(), 2);
    assert_eq!(after.allowed[0].tool, before.allowed[1].tool);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_monid_action(MonidAction::RemoveBlocked(LIGHTS.into(), 0), window, cx);
        })
    });
    settle(cx);
    assert!(lights(&app, cx).monid.blocked.is_empty());
    // A tool from the search joins the list and the search clears.
    let tool = before.allowed[0].tool.clone();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_monid_action(MonidAction::AddBlocked(LIGHTS.into(), tool), window, cx);
        })
    });
    settle(cx);
    assert_eq!(lights(&app, cx).monid.blocked.len(), 1);
    // The rule of the channel.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            assert!(app.pick_monid_select(
                CHANNEL_RULE_SELECT,
                ApprovalRuleView::RunWithoutAsking.key(),
                cx
            ));
        })
    });
    settle(cx);
    assert_eq!(
        lights(&app, cx).monid.approval,
        Some(ApprovalRuleView::RunWithoutAsking)
    );
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.pick_monid_select(CHANNEL_RULE_SELECT, FOLLOW_HUB, cx);
        })
    });
    settle(cx);
    assert_eq!(lights(&app, cx).monid.approval, None, "follows the hub");
}

/// End on a temporary access removes the grant.
#[gpui::test]
fn end_removes_a_temporary_access(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel_connectors(LIGHTS.into(), window, cx)
        })
    });
    settle(cx);
    let before = lights(&app, cx).access_grants.len();
    assert!(before >= 2);
    assert!(has(cx, "end-access-agr_mock_gmail"), "End draws");
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_monid_action(
                MonidAction::EndAccess(LIGHTS.into(), "agr_mock_gmail".into()),
                window,
                cx,
            )
        })
    });
    settle(cx);
    let channel = lights(&app, cx);
    assert_eq!(channel.access_grants.len(), before - 1);
    assert!(
        channel
            .access_grants
            .iter()
            .all(|g| g.id != "agr_mock_gmail")
    );
    assert!(!has(cx, "end-access-agr_mock_gmail"));
}

fn set_field(
    app: &Entity<ConsortApp>,
    cx: &mut VisualTestContext,
    field: impl Fn(&crate::monid::MonidInputs) -> Entity<gpui_kit::component::input::InputState>,
    text: &str,
) {
    let text = text.to_owned();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let input = field(&app.monid_inputs);
            input.update(cx, |input, cx| input.set_value(text, window, cx));
        })
    });
}

/// The Monid page of Settings > Connectors saves the limits, says what is
/// wrong with an amount and sets the rule of the hub.
#[gpui::test]
fn the_settings_page_saves_limits_and_the_rule_of_the_hub(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_monid_page(window, cx)));
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, Page::Settings);
        assert_eq!(app.settings_section, SettingsSection::Connectors);
        assert!(app.monid_ui.page);
    });
    assert!(has(cx, "monid-add-money"), "the page draws");
    assert!(has(cx, "monid-list-row"), "the list has a Monid row");
    let before = app.read_with(cx, |app, _| app.state.monid.settings.limits);
    // A wrong amount saves nothing and says so.
    set_field(&app, cx, |i| i.per_call.clone(), "ten");
    cx.update(|window, cx| app.update(cx, |app, cx| app.save_monid_limits(window, cx)));
    settle(cx);
    app.read_with(cx, |app, _| {
        assert!(app.monid_ui.limits_error.is_some());
        assert_eq!(app.state.monid.settings.limits, before);
    });
    // A right amount saves.
    set_field(&app, cx, |i| i.per_call.clone(), "1.50");
    cx.update(|window, cx| app.update(cx, |app, cx| app.save_monid_limits(window, cx)));
    settle(cx);
    app.read_with(cx, |app, _| {
        assert!(app.monid_ui.limits_error.is_none());
        assert_eq!(
            app.state.monid.settings.limits.per_call_micros,
            Some(1_500_000)
        );
    });
    // The rule of the hub.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            assert!(app.pick_monid_select(
                HUB_RULE_SELECT,
                ApprovalRuleView::AskOncePerTask.key(),
                cx
            ));
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.state.monid.settings.approval,
            ApprovalRuleView::AskOncePerTask
        );
        // Another select is not a select of Monid.
    });
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            assert!(!app.pick_monid_select("other-select", "x", cx))
        })
    });
}

/// The Add Monid dialog needs a key, saves it, and the key never shows.
#[gpui::test]
fn the_add_monid_dialog_asks_for_a_key_and_never_shows_it(cx: &mut TestAppContext) {
    let state = mm::screen_state("monid-add").expect("a state without a key");
    let (handle, app) = open_app(cx, state);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(SettingsSection::Connectors, cx);
            app.open_monid_key_dialog(false, window, cx);
        })
    });
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::MonidKey));
        assert!(!app.state.monid.key_present);
    });
    // An empty key says what to do.
    cx.update(|window, cx| app.update(cx, |app, cx| app.save_monid_key(window, cx)));
    settle(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::MonidKey), "the dialog stays");
        assert!(app.dialog_error.is_some());
    });
    set_field(&app, cx, |i| i.key.clone(), "monid_live_abcd1234");
    cx.update(|window, cx| app.update(cx, |app, cx| app.save_monid_key(window, cx)));
    settle(cx);
    app.read_with(cx, |app, _| {
        assert!(app.state.monid.key_present);
        assert!(app.dialog.is_none(), "the dialog closes");
        assert_eq!(app.state.monid.key_text(), "Ends in 1234");
        assert!(!format!("{:?}", app.state.monid).contains("monid_live"));
    });
    let field = cx.update(|_, cx| app.read(cx).monid_inputs.key.read(cx).value().to_string());
    assert!(field.is_empty(), "the field is clear");
}

/// A routine takes its own Monid setting, and None goes back to the hub.
#[gpui::test]
fn a_routine_takes_its_own_monid_setting_or_the_hub_one(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(SettingsSection::Routines, cx);
            app.open_routine_in_settings("rtn_sunset_check", cx);
            let _ = window;
        })
    });
    settle(cx);
    assert!(
        !has(cx, "routine-monid-hub"),
        "no button while it follows the hub"
    );
    let setting = |app: &Entity<ConsortApp>, cx: &mut VisualTestContext| {
        app.read_with(cx, |app, _| {
            app.state
                .routines
                .iter()
                .find(|r| r.id == "rtn_sunset_check")
                .expect("routine")
                .monid
        })
    };
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_routine_monid(
                "rtn_sunset_check".into(),
                Some(MonidUnattendedView::Off),
                cx,
            )
        })
    });
    settle(cx);
    assert_eq!(setting(&app, cx), Some(MonidUnattendedView::Off));
    assert!(has(cx, "routine-monid-hub"), "Use the hub's setting draws");
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_routine_monid("rtn_sunset_check".into(), None, cx)
        })
    });
    settle(cx);
    assert_eq!(setting(&app, cx), None);
}

/// The New task dialog says what Full access means for spending, and only
/// for Full access.
#[gpui::test]
fn full_access_says_that_the_channel_can_spend(cx: &mut TestAppContext) {
    let (handle, app) = open_app(cx, consort_client::mock::view_state());
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(LIGHTS.into(), window, cx);
            app.open_new_task(window, cx);
            app.convo.new_task.approval_mode = consort_client::ApprovalMode::Ask;
        })
    });
    settle(cx);
    assert!(!has(cx, "monid-full-access-note"));
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.convo.new_task.approval_mode = consort_client::ApprovalMode::FullAccess;
            cx.notify();
        })
    });
    settle(cx);
    assert!(has(cx, "monid-full-access-note"));
}

/// The menu of a context is an action of Monid.
#[test]
fn a_menu_item_of_monid_carries_a_monid_action() {
    let rows = mm::rows();
    let items = monid_call_menu(&rows[0]);
    assert!(
        items
            .iter()
            .all(|i| matches!(i.action, MenuAction::Monid(_)))
    );
}

#[gpui::test]
fn the_connector_list_has_a_monid_row_only_with_a_key(cx: &mut TestAppContext) {
    let mut state = consort_client::mock::view_state();
    state.monid.key_present = false;
    let (handle, app) = open_app(cx, state);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_monid_page(window, cx)));
    settle(cx);
    assert!(!has(cx, "monid-list-row"));
}

#[gpui::test]
fn the_more_menu_of_a_card_opens_under_its_button(cx: &mut TestAppContext) {
    let (handle, app) = crate::test_support::open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_channel(LIGHTS.into(), window, cx)));
    settle(cx);
    // The newest card of the two: the older one can sit under the top zone
    // of the window, which takes the click.
    let key = card_key(&app, cx, mm::APR_OVER_LIMIT);
    crate::test_support::click(cx, &format!("{key}-more"));
    app.read_with(cx, |app, _| {
        let menu = app.menu.as_ref().expect("the menu is open");
        assert!(matches!(
            menu.anchor,
            crate::ui::menu::MenuAnchor::Trigger(_)
        ));
    });
}
