//! The API endpoints of Settings > Providers, the command check of Settings >
//! Models, and what the check shows in a task thread and an approval.

use consort_client::{CheckEndpointChoice, CheckSourceKind, CommandCheckChoice, mock};
use gpui::{TestAppContext, VisualTestContext};

use crate::app::{ConsortApp, Dialog, SettingsSection};
use crate::test_support::{click, has, open_sized, settle, until};
use crate::ui::command_check::{
    CHECK_ENDPOINT_SELECT, CHECK_LIMIT_SELECT, CHECK_SOURCE_SELECT, apply_check_pick,
};
use crate::ui::menu::MenuAction;

fn open_state(
    state: consort_client::ViewState,
    section: SettingsSection,
    cx: &mut TestAppContext,
) -> (gpui::AnyWindowHandle, gpui_kit::Entity<ConsortApp>) {
    // Tall enough for the whole page, so a click reaches the last section.
    let (handle, app) = open_sized(state, 1440., 3000., cx);
    let vcx = &mut VisualTestContext::from_window(handle, cx);
    vcx.update(|_, cx| app.update(cx, |app, cx| app.open_settings(section, cx)));
    settle(vcx, 100);
    (handle, app)
}

fn open_section(
    section: SettingsSection,
    cx: &mut TestAppContext,
) -> (gpui::AnyWindowHandle, gpui_kit::Entity<ConsortApp>) {
    open_state(mock::view_state(), section, cx)
}

#[test]
fn a_pick_of_a_source_keeps_the_choices_of_the_other_kinds() {
    let mut choice = CommandCheckChoice {
        chat: Some(CheckEndpointChoice {
            endpoint_id: "b".into(),
            model: "chat".into(),
            confidence_limit: 80,
        }),
        ..CommandCheckChoice::default()
    };
    assert!(apply_check_pick(
        &mut choice,
        CHECK_SOURCE_SELECT,
        "Decision model",
        Some("a")
    ));
    assert_eq!(choice.kind, CheckSourceKind::Decision);
    let decision = choice.decision.clone().expect("a new choice");
    assert_eq!(decision.endpoint_id, "a");
    assert_eq!(decision.confidence_limit, 80);
    assert_eq!(choice.chat.as_ref().expect("kept").model, "chat");
    assert!(apply_check_pick(
        &mut choice,
        CHECK_LIMIT_SELECT,
        "90",
        Some("a")
    ));
    assert_eq!(choice.decision.as_ref().expect("set").confidence_limit, 90);
    assert!(apply_check_pick(
        &mut choice,
        CHECK_ENDPOINT_SELECT,
        "b",
        Some("a")
    ));
    assert_eq!(choice.decision.as_ref().expect("set").endpoint_id, "b");
    assert!(!apply_check_pick(
        &mut choice,
        CHECK_ENDPOINT_SELECT,
        "b",
        Some("a")
    ));
}

#[gpui::test]
fn the_providers_page_lists_endpoints_and_the_dialog_adds_one(cx: &mut TestAppContext) {
    let (handle, app) = open_section(SettingsSection::Providers, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert!(has(cx, "add-endpoint"));
    assert!(has(cx, "endpoint-row-end_1"));
    assert!(has(cx, "endpoint-dots-end_1"));
    click(cx, "add-endpoint");
    settle(cx, 100);
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::Endpoint(None)))
    });
    click(cx, "endpoint-test");
    until(cx, &app, "the test to end", |app| {
        app.check_ui.endpoint_test.is_some()
    });
    app.update_in(cx, |app, window, cx| {
        for (input, value) in [
            (app.check_inputs.endpoint_name.clone(), "Gateway"),
            (
                app.check_inputs.endpoint_url.clone(),
                "https://gateway.test",
            ),
            (app.check_inputs.endpoint_key.clone(), "secret-ab12"),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
    });
    click(cx, "dialog-confirm");
    until(cx, &app, "the endpoint to show", |app| {
        app.state.endpoints.iter().any(|e| e.name == "Gateway")
    });
    app.read_with(cx, |app, _| {
        let added = app
            .state
            .endpoints
            .iter()
            .find(|e| e.name == "Gateway")
            .expect("added");
        assert_eq!(added.key_last_four, "ab12");
        assert!(!added.in_use);
    });
}

/// A Test with no model name says what is missing. It is never a dead button.
#[gpui::test]
fn test_with_no_model_name_says_what_is_missing(cx: &mut TestAppContext) {
    let (handle, app) = open_section(SettingsSection::Models, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    app.update_in(cx, |app, window, cx| {
        app.run_menu(
            MenuAction::Select(CHECK_SOURCE_SELECT, "Decision model".to_owned()),
            window,
            cx,
        );
    });
    settle(cx, 100);
    click(cx, "check-test");
    settle(cx, 100);
    app.read_with(cx, |app, _| {
        let result = app.check_ui.test.clone().expect("a result");
        assert!(!result.ok);
        assert_eq!(result.message, "Type a model name, then test.");
    });
    assert!(has(cx, "check-test-result"));
}

#[gpui::test]
fn the_models_page_changes_the_source_and_tests_it(cx: &mut TestAppContext) {
    let (handle, app) = open_section(SettingsSection::Models, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert!(has(cx, CHECK_SOURCE_SELECT));
    assert!(has(cx, "check-model"));
    click(cx, "check-test");
    until(cx, &app, "the test to end", |app| {
        app.check_ui.test.is_some()
    });
    app.read_with(cx, |app, _| {
        let result = app.check_ui.test.clone().expect("a result");
        assert!(result.ok);
        assert!(result.message.starts_with("Test passed"));
    });
    assert!(has(cx, "check-test-result"));
    app.update_in(cx, |app, window, cx| {
        app.run_menu(
            MenuAction::Select(CHECK_SOURCE_SELECT, "Decision model".to_owned()),
            window,
            cx,
        );
    });
    until(cx, &app, "the source to change", |app| {
        app.state.command_check.choice.kind == CheckSourceKind::Decision
    });
    settle(cx, 100);
    assert!(has(cx, CHECK_ENDPOINT_SELECT));
    assert!(has(cx, CHECK_LIMIT_SELECT));
    // The new source has no model name, so the old test result is gone and the
    // test waits for one.
    app.read_with(cx, |app, _| {
        assert!(app.check_ui.test.is_none());
        assert!(app.state.command_check.test_blocked("", true).is_some());
        let endpoint = &app.state.endpoints[0];
        assert!(endpoint.in_use, "the first endpoint is in use now");
    });
}

#[gpui::test]
fn a_model_of_an_account_opens_the_picker_and_choose_for_me_gives_it_back(cx: &mut TestAppContext) {
    let (handle, app) = open_section(SettingsSection::Models, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    app.read_with(cx, |app, _| {
        let view = &app.state.command_check;
        assert!(view.choice.account_model.is_none());
        assert_eq!(
            view.account_model().expect("chosen").model_name,
            "Haiku 5.5"
        );
    });
    click(cx, "check-model");
    settle(cx, 100);
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.picker.as_ref().map(|p| p.target.clone()),
            Some(crate::ui::model_picker::PickerTarget::CommandCheck)
        );
    });
    let picked = app.read_with(cx, |app, _| {
        let group = app.state.model_groups.first().expect("a group");
        consort_client::SelectionView {
            account_label: group.account_label.clone(),
            model_id: group.models[0].id.clone(),
            model_name: group.models[0].name.clone(),
            options: Vec::new(),
        }
    });
    let want = picked.model_id.clone();
    app.update(cx, |app, cx| {
        app.set_check_account_model(Some(picked), cx);
    });
    until(cx, &app, "the model to change", |app| {
        app.state
            .command_check
            .choice
            .account_model
            .as_ref()
            .is_some_and(|m| m.model_id == want)
    });
    app.update(cx, |app, cx| app.set_check_account_model(None, cx));
    until(cx, &app, "the choice to go back", |app| {
        app.state.command_check.choice.account_model.is_none()
    });
}

#[gpui::test]
fn the_hub_page_warns_about_a_mac_sandbox(cx: &mut TestAppContext) {
    let mut state = mock::view_state();
    if let Some(host) = state.session_host.as_mut() {
        host.boundary = Some(consort_client::BoundaryView::MacSandbox);
    }
    let (handle, _app) = open_state(state, SettingsSection::Hub, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 300);
    assert!(has(cx, "hub-boundary-warning"));
}

#[gpui::test]
fn the_hub_page_has_no_warning_on_linux(cx: &mut TestAppContext) {
    let (handle, _app) = open_section(SettingsSection::Hub, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 300);
    assert!(!has(cx, "hub-boundary-warning"));
}
