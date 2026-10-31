//! The status line above the composer when the assistant cannot answer, in a
//! headless window: where it sits, that it shows only with a status, and its
//! Sign in again action.

use crate::app::{ConsortApp, Dialog};
use crate::test_support::{bounds, click, has, open, settle};
use consort_client::{AssistantStatusView, ViewState, mock};
use gpui::{TestAppContext, VisualTestContext};

fn with_status(status: Option<AssistantStatusView>) -> ViewState {
    let mut state = mock::view_state();
    let tutti = mock::tutti_id();
    for channel in &mut state.channels {
        if channel.id == tutti {
            channel.assistant_status = status.clone();
        }
    }
    state
}

fn waiting() -> AssistantStatusView {
    AssistantStatusView {
        text: "Waiting for a worker to come online.".into(),
        account: None,
    }
}

fn signed_out(account: &str) -> AssistantStatusView {
    AssistantStatusView {
        text: format!("{account} is signed out, so the assistant cannot answer."),
        account: Some(account.into()),
    }
}

#[gpui::test]
fn the_line_sits_just_above_the_composer_and_has_no_button_without_an_account(
    cx: &mut TestAppContext,
) {
    let (handle, app) = open(with_status(Some(waiting())), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let t = app.read_with(cx, |_, cx| ConsortApp::tokens(cx));
    let line = bounds(cx, "assistant-status");
    let composer = bounds(cx, "composer");
    assert!(
        line.bottom() <= composer.top() + t.space.xs,
        "above the composer: {line:?} {composer:?}"
    );
    assert!(
        composer.top() - line.bottom() < t.space.xl,
        "just above it: {line:?} {composer:?}"
    );
    assert!(has(cx, "assistant-status-text"));
    assert!(!has(cx, "assistant-status-sign-in"));
}

#[gpui::test]
fn without_a_status_the_line_is_not_there(cx: &mut TestAppContext) {
    let (handle, _app) = open(with_status(None), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(!has(cx, "assistant-status"));
}

#[gpui::test]
fn the_line_goes_away_when_the_state_is_cleared(cx: &mut TestAppContext) {
    let (handle, app) = open(with_status(Some(waiting())), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(has(cx, "assistant-status"));
    app.update(cx, |app, cx| {
        let mut state = (*app.state).clone();
        for channel in &mut state.channels {
            channel.assistant_status = None;
        }
        app.state = state.into();
        cx.notify();
    });
    settle(cx, 100);
    assert!(!has(cx, "assistant-status"));
}

#[gpui::test]
fn sign_in_again_opens_the_sign_in_of_the_account(cx: &mut TestAppContext) {
    let (handle, app) = open(with_status(Some(signed_out("Claude Personal"))), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    click(cx, "assistant-status-sign-in");
    app.read_with(cx, |app, _| {
        assert!(
            matches!(app.dialog, Some(Dialog::AddAccount(_))),
            "the sign-in dialog opens: {:?}",
            app.dialog
        );
        assert_eq!(
            app.add_account.account_id.as_deref().and_then(|id| {
                app.state
                    .accounts()
                    .find(|(_, a)| a.id == id)
                    .map(|(_, a)| a.label.clone())
            }),
            Some("Claude Personal".to_owned())
        );
    });
}

#[gpui::test]
fn sign_in_again_with_an_unknown_account_opens_the_providers_page(cx: &mut TestAppContext) {
    let (handle, app) = open(with_status(Some(signed_out("No worker has this"))), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    click(cx, "assistant-status-sign-in");
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, crate::app::Page::Settings);
        assert_eq!(app.settings_section, crate::app::SettingsSection::Providers);
    });
}

#[gpui::test]
fn choose_another_model_opens_the_picker_of_the_assistant(cx: &mut TestAppContext) {
    let (handle, app) = open(with_status(Some(waiting())), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    click(cx, "assistant-status-model");
    app.read_with(cx, |app, _| {
        let picker = app.picker.as_ref().expect("the model picker is open");
        assert_eq!(
            picker.target,
            crate::ui::model_picker::PickerTarget::Assistant(mock::tutti_id().into())
        );
    });
}

#[gpui::test]
fn a_short_reason_lines_up_with_the_buttons(cx: &mut TestAppContext) {
    let (handle, _app) = open(with_status(Some(signed_out("Claude Personal"))), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let text = bounds(cx, "assistant-status-text");
    let button = bounds(cx, "assistant-status-sign-in");
    let middle = |b: gpui::Bounds<gpui::Pixels>| b.top() + b.size.height / 2.0;
    let apart = (middle(text) - middle(button)).abs();
    assert!(apart < gpui::px(1.0), "{text:?} {button:?}");
    assert!(has(cx, "assistant-status-model"));
}
