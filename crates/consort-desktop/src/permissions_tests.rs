//! The Permissions dialog, its rule dialog and its tool picker, in a
//! headless window on the mock. The form logic has unit tests in
//! `permissions.rs`.

use consort_client::{ApprovalRuleView, ChannelKind, ToolKindView, mock_extensions as mx};
use consort_proto::model::{CommandRuleResult, CommandRuleSource};
use gpui::{TestAppContext, VisualTestContext};

use crate::{
    app::{ConsortApp, Dialog},
    convo_tests::{draw, mock, open, until},
    permissions::{PermissionsAction, RULE_SELECT, rule_option},
    test_support::{click, has},
    ui::menu::MenuAction,
};

/// Whether an element with this debug selector is on screen. The selector
/// leaks, which is fine in a test.
fn shown(cx: &mut VisualTestContext, selector: &str) -> bool {
    let selector: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    cx.debug_bounds(selector).is_some()
}

fn sayso(app: &gpui_kit::Entity<ConsortApp>, cx: &VisualTestContext) -> String {
    app.read_with(cx, |app, _| {
        app.state
            .channels
            .iter()
            .find(|c| c.name == "sayso")
            .expect("sayso")
            .id
            .clone()
    })
}

fn act(app: &gpui_kit::Entity<ConsortApp>, cx: &mut VisualTestContext, action: PermissionsAction) {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.run_permissions_action(action, window, cx))
    });
    draw(cx);
}

fn open_permissions(app: &gpui_kit::Entity<ConsortApp>, cx: &mut VisualTestContext, channel: &str) {
    act(app, cx, PermissionsAction::Open(channel.to_owned()));
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::Permissions(channel.to_owned())))
    });
}

#[gpui::test]
fn the_access_section_opens_the_permissions_dialog(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = sayso(&app, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(Dialog::ChannelSettings(sayso.clone()), window, cx)
        })
    });
    draw(cx);
    assert!(has(cx, "settings-access"));
    assert!(has(cx, "settings-permissions"));
    assert!(has(cx, "settings-extensions"));
    app.read_with(cx, |app, _| {
        // Own: 3 rules. From Tutti: rm -rf. One exception on GitHub.
        assert_eq!(
            crate::permissions::access_detail(&app.state, &sayso),
            "4 command rules, 1 tool exception"
        );
    });
    click(cx, "settings-permissions");
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::Permissions(sayso.clone())));
    });
    assert!(has(cx, "permissions-add-rule"));
}

#[gpui::test]
fn the_dialog_shows_the_groups_the_connections_and_the_exceptions(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = sayso(&app, cx);
    open_permissions(&app, cx, &sayso);
    for selector in [
        "permissions-rule-own-git push",
        "permissions-rule-own-gh pr create",
        "permissions-rule-own-gh pr merge",
        "permissions-rule-tutti-rm -rf",
        "permissions-connection-con_mock_github",
        "permissions-exception-con_mock_github-GITHUB_MERGE_A_PULL_REQUEST",
        "permissions-add-exception-con_mock_github",
        "permissions-rule-con_mock_github-Read",
    ] {
        assert!(shown(cx, selector), "{selector}");
    }
    // A connection that the channel does not have has no card.
    assert!(!shown(cx, "permissions-connection-con_mock_linear-x"));
}

#[gpui::test]
fn tutti_has_no_group_of_its_own_rules_and_the_empty_state_points_to_the_assistant(
    cx: &mut TestAppContext,
) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let tutti = app.read_with(cx, |app, _| app.state.tutti().unwrap().id.clone());
    open_permissions(&app, cx, &tutti);
    assert!(shown(cx, "permissions-rule-own-rm -rf"));
    assert!(!shown(cx, "permissions-rule-tutti-rm -rf"));
    // A channel with no rule of its own still shows the rules of Tutti.
    let lights = app.read_with(cx, |app, _| {
        app.state
            .channels
            .iter()
            .find(|c| c.kind == ChannelKind::Standard && c.command_rules.is_empty())
            .unwrap()
            .id
            .clone()
    });
    open_permissions(&app, cx, &lights);
    assert!(shown(cx, "permissions-rule-tutti-rm -rf"));
    assert!(!has(cx, "permissions-empty"));
    // With no rule at all, the dialog says to tell the assistant.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.convo.permissions.form.rules.clear();
            for channel in &mut std::sync::Arc::make_mut(&mut app.state).channels {
                channel.command_rules.clear();
            }
            cx.notify();
        })
    });
    draw(cx);
    assert!(has(cx, "permissions-empty"));
    assert!(has(cx, "permissions-add-rule-empty"));
    assert!(!has(cx, "permissions-add-rule"));
}

fn type_rule(
    app: &gpui_kit::Entity<ConsortApp>,
    cx: &mut VisualTestContext,
    pattern: &str,
    except: &str,
    description: &str,
) {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let inputs = &app.convo_inputs;
            inputs
                .rule_pattern
                .update(cx, |i, cx| i.set_value(pattern.to_owned(), window, cx));
            inputs
                .rule_except
                .update(cx, |i, cx| i.set_value(except.to_owned(), window, cx));
            inputs
                .rule_description
                .update(cx, |i, cx| i.set_value(description.to_owned(), window, cx));
        })
    });
}

#[gpui::test]
fn a_new_rule_is_a_draft_until_save_changes(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = sayso(&app, cx);
    open_permissions(&app, cx, &sayso);
    click(cx, "permissions-add-rule");
    assert!(has(cx, "permissions-rule-dialog"));
    // An empty pattern is an error in the dialog, which stays open.
    click(cx, "rule-dialog-confirm");
    app.read_with(cx, |app, _| {
        let error = app.convo.permissions.rule.as_ref().unwrap().error.clone();
        assert!(error.unwrap().contains("needs a pattern"));
    });
    type_rule(
        &app,
        cx,
        "  cargo   test ",
        "--release, -p",
        "Run the tests",
    );
    click(cx, "rule-result-ask");
    click(cx, "rule-dialog-confirm");
    app.read_with(cx, |app, _| {
        assert!(app.convo.permissions.rule.is_none());
        let rule = app.convo.permissions.form.rule("cargo test").unwrap();
        assert_eq!(rule.except, ["--release", "-p"]);
        assert_eq!(rule.result, CommandRuleResult::Ask);
        assert_eq!(rule.source, CommandRuleSource::User);
        // The hub has nothing yet.
        let saved = &app.state.channel(&sayso).unwrap().command_rules;
        assert_eq!(saved.len(), 3);
    });
    assert!(shown(cx, "permissions-rule-own-cargo test"));
    click(cx, "dialog-confirm");
    until(cx, &app, "the saved rules", |app| {
        app.state.channel(&sayso).unwrap().command_rules.len() == 4
    });
    app.read_with(cx, |app, _| assert!(app.dialog.is_none()));
}

#[gpui::test]
fn cancel_drops_the_draft(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = sayso(&app, cx);
    open_permissions(&app, cx, &sayso);
    act(&app, cx, PermissionsAction::RemoveRule("git push".into()));
    assert!(!shown(cx, "permissions-rule-own-git push"));
    click(cx, "dialog-cancel");
    app.read_with(cx, |app, _| {
        assert!(app.dialog.is_none());
        assert_eq!(app.state.channel(&sayso).unwrap().command_rules.len(), 3);
    });
    // Opening it again starts from the saved rules.
    open_permissions(&app, cx, &sayso);
    assert!(shown(cx, "permissions-rule-own-git push"));
}

#[gpui::test]
fn a_rule_of_tutti_is_overridden_here_and_the_menus_open(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = sayso(&app, cx);
    open_permissions(&app, cx, &sayso);
    // The right click on a rule of Tutti has Override here only.
    let row = cx
        .debug_bounds("permissions-rule-tutti-rm -rf")
        .expect("the row");
    cx.simulate_event(gpui_kit::MouseDownEvent {
        button: gpui_kit::MouseButton::Right,
        position: row.center(),
        modifiers: gpui_kit::Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    draw(cx);
    app.read_with(cx, |app, _| {
        let items = &app.menu.as_ref().expect("a menu").items;
        let labels: Vec<_> = items.iter().map(|i| i.label.to_string()).collect();
        assert_eq!(labels, ["Override here"]);
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let action = app.menu.as_ref().unwrap().items[0].action.clone();
            app.run_menu(action, window, cx)
        })
    });
    draw(cx);
    app.read_with(cx, |app, cx| {
        let dialog = app.convo.permissions.rule.as_ref().expect("the dialog");
        assert!(dialog.overriding);
        assert_eq!(dialog.confirm_label(), "Save rule");
        assert_eq!(
            app.convo_inputs.rule_pattern.read(cx).value().to_string(),
            "rm -rf"
        );
    });
    // Run without asking here. The rule of Tutti goes from the list.
    click(cx, "rule-result-allow");
    click(cx, "rule-dialog-confirm");
    app.read_with(cx, |app, _| {
        let rule = app.convo.permissions.form.rule("rm -rf").unwrap();
        assert_eq!(rule.result, CommandRuleResult::Allow);
        assert_eq!(rule.source, CommandRuleSource::User);
    });
    assert!(shown(cx, "permissions-rule-own-rm -rf"));
    assert!(!shown(cx, "permissions-rule-tutti-rm -rf"));
    // Edit opens the rule of the channel with its fields.
    act(&app, cx, PermissionsAction::EditRule("git push".into()));
    app.read_with(cx, |app, cx| {
        assert_eq!(
            app.convo_inputs.rule_except.read(cx).value().to_string(),
            "--force, -f, --delete, main"
        );
        let dialog = app.convo.permissions.rule.as_ref().unwrap();
        assert_eq!(dialog.title(), "Edit rule");
    });
}

#[gpui::test]
fn escape_closes_the_rule_dialog_and_not_the_dialog_under_it(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = sayso(&app, cx);
    open_permissions(&app, cx, &sayso);
    act(&app, cx, PermissionsAction::AddRule);
    let closed =
        cx.update(|window, cx| app.update(cx, |app, cx| app.permissions_key("escape", window, cx)));
    assert!(closed);
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.convo.permissions.rule.is_none());
        assert_eq!(app.dialog, Some(Dialog::Permissions(sayso.clone())));
    });
    // With nothing open on top, the key is not for this layer.
    let handled =
        cx.update(|window, cx| app.update(cx, |app, cx| app.permissions_key("escape", window, cx)));
    assert!(!handled);
}

#[gpui::test]
fn a_connection_select_and_an_exception_save_to_the_connection(cx: &mut TestAppContext) {
    // One connection, so its card is in view with no scrolling.
    let mut state = mock();
    state.connections.retain(|c| c.id == mx::CON_GITHUB);
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = sayso(&app, cx);
    open_permissions(&app, cx, &sayso);
    // The select flips its menu through the shared menu layer.
    click(cx, "permissions-rule-con_mock_github-Delete");
    app.read_with(cx, |app, _| {
        let menu = app.menu.as_ref().expect("the menu");
        let labels: Vec<_> = menu.items.iter().map(|i| i.label.to_string()).collect();
        assert_eq!(
            labels,
            ["Run without asking", "Ask once per task", "Ask every time"]
        );
        assert!(menu.items[2].checked);
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                MenuAction::Select(
                    RULE_SELECT,
                    rule_option(
                        mx::CON_GITHUB,
                        ToolKindView::Delete,
                        ApprovalRuleView::AskOncePerTask,
                    ),
                ),
                window,
                cx,
            )
        })
    });
    // The exception can only run without asking or ask every time.
    click(cx, "permissions-add-exception-con_mock_github");
    until(cx, &app, "the tools", |app| {
        app.state.tools_of("github").is_some()
    });
    assert!(shown(cx, "permissions-picker"));
    assert!(shown(cx, "permissions-tool-GITHUB_CREATE_A_PULL_REQUEST"));
    // The tool with an exception is not offered.
    assert!(!shown(cx, "permissions-tool-GITHUB_MERGE_A_PULL_REQUEST"));
    // The search narrows the list to the tool.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.convo_inputs
                .exception_search
                .update(cx, |i, cx| i.set_value("create a pull", window, cx))
        })
    });
    draw(cx);
    assert!(!shown(cx, "permissions-tool-GITHUB_GET_0"));
    click(cx, "permissions-tool-GITHUB_CREATE_A_PULL_REQUEST");
    app.read_with(cx, |app, _| {
        assert!(app.convo.permissions.picker.is_none());
        let draft = app
            .convo
            .permissions
            .form
            .connection(mx::CON_GITHUB)
            .unwrap();
        assert_eq!(draft.exceptions.len(), 2);
        // The rule of Change is "Ask every time", so the exception differs.
        assert_eq!(draft.exceptions[1].rule, ApprovalRuleView::RunWithoutAsking);
        assert_eq!(draft.exceptions[1].name, "Create a pull request");
    });
    // Remove one with its x button.
    click(
        cx,
        "permissions-exception-con_mock_github-GITHUB_MERGE_A_PULL_REQUEST-remove",
    );
    click(cx, "dialog-confirm");
    until(cx, &app, "the saved connection", |app| {
        let c = app.state.connection(mx::CON_GITHUB).unwrap();
        c.approvals.delete == ApprovalRuleView::AskOncePerTask
            && c.tool_exceptions.len() == 1
            && c.tool_exceptions[0].tool == "GITHUB_CREATE_A_PULL_REQUEST"
    });
    app.read_with(cx, |app, _| {
        assert!(app.dialog.is_none());
        // Nothing changed in the rules, so the channel is as it was.
        let rules = &app.state.channel(&sayso_id(app)).unwrap().command_rules;
        assert_eq!(rules.len(), 3);
    });
}

fn sayso_id(app: &ConsortApp) -> String {
    app.state
        .channels
        .iter()
        .find(|c| c.name == "sayso")
        .unwrap()
        .id
        .clone()
}

#[gpui::test]
fn the_channel_menu_opens_the_permissions_dialog(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = sayso(&app, cx);
    let items = crate::ui::menu::channel_menu(&sayso);
    let item = items.iter().find(|i| i.label == "Permissions").unwrap();
    let action = item.action.clone();
    cx.update(|window, cx| app.update(cx, |app, cx| app.run_menu(action, window, cx)));
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::Permissions(sayso.clone())))
    });
}

#[gpui::test]
fn an_owned_connection_says_so_in_the_dialog(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = sayso(&app, cx);
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            let state = std::sync::Arc::make_mut(&mut app.state);
            let github = state
                .connections
                .iter_mut()
                .find(|c| c.id == mx::CON_GITHUB);
            github.unwrap().assistant_owned = true;
        })
    });
    open_permissions(&app, cx, &sayso);
    assert!(shown(cx, "permissions-connection-con_mock_github"));
}

/// A permissions card shows its groups and rows, and after the answer it is
/// one line with the place and the number of changes.
#[gpui::test]
fn a_permissions_card_shows_its_rows_and_then_one_line(cx: &mut TestAppContext) {
    // The hiring timeline keeps only this card, so it is in view.
    let mut state = mock();
    for timeline in &mut state.timelines {
        let has_card = timeline
            .items
            .iter()
            .any(|i| matches!(i, consort_client::TimelineItem::Approval(c) if c.message_id == KEY));
        if has_card {
            timeline.items.retain(
                |i| matches!(i, consort_client::TimelineItem::Approval(c) if c.message_id == KEY),
            );
        }
    }
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(mx::HIRING.into(), window, cx)
        })
    });
    draw(cx);
    assert!(shown(cx, &format!("{KEY}-hub")), "the card");
    assert!(shown(cx, &format!("{KEY}-perm-0-0")), "the first row");
    assert!(shown(cx, &format!("{KEY}-perm-1-0")), "the connection row");
    // Allow answers it, and the card becomes one line.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.answer_approval(
                mx::APR_PERMISSIONS.into(),
                consort_client::ApprovalChoice::AllowOnce,
                window,
                cx,
            )
        })
    });
    until(cx, &app, "the line", |app| {
        app.state.timelines.iter().any(|t| {
            t.items.iter().any(|i| {
                matches!(i, consort_client::TimelineItem::Approval(c)
                    if c.approval.id == mx::APR_PERMISSIONS
                        && c.approval.state == consort_client::RequestState::Answered)
            })
        })
    });
    assert!(shown(cx, &format!("{KEY}-line")), "the answered line");
    assert!(!shown(cx, &format!("{KEY}-hub")), "no card any more");
}

const KEY: &str = "msg_hiring_permissions";

/// The switch on the connection page makes the account the assistant's, and
/// the risk shows in words while it is on.
#[gpui::test]
fn the_connection_page_has_the_owner_switch_with_the_risk(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(crate::app::SettingsSection::Connectors, cx);
            app.select_connection(Some(mx::CON_GITHUB.into()), Some(window), cx);
        })
    });
    draw(cx);
    assert!(has(cx, "connection-assistant-owned"));
    assert!(!has(cx, "connection-owned-risk"), "off: no warning yet");
    click(cx, "connection-assistant-owned");
    until(cx, &app, "the owner", |app| {
        app.state
            .connection(mx::CON_GITHUB)
            .is_some_and(|c| c.assistant_owned)
    });
    assert!(has(cx, "connection-owned-risk"));
    click(cx, "connection-assistant-owned");
    until(cx, &app, "the owner off", |app| {
        app.state
            .connection(mx::CON_GITHUB)
            .is_some_and(|c| !c.assistant_owned)
    });
    assert!(!has(cx, "connection-owned-risk"));
}
