//! The fallback path in the app: the edits of the steps, the picker that
//! adds one, the confirm dialog for another harness, the marker on the model
//! chip, and the notices.

use consort_client::{
    ChannelFallbackView, FallbackCauseView, FallbackNoticeKind, FallbackNoticeView,
    FallbackScopeView, SelectionView, TimelineItem, mock,
};
use gpui::{TestAppContext, VisualTestContext};

use crate::app::{ConsortApp, Dialog, Page, SettingsSection};
use crate::fallback::{AddOutcome, Direction, add_outcome, dropped, moved, without};
use crate::test_support::{has, open, settle, until};
use crate::ui::{menu::MenuAction, model_picker::PickerTarget, models::step_menu};

fn step(label: &str, model: &str) -> SelectionView {
    SelectionView {
        account_label: label.into(),
        model_id: model.into(),
        model_name: model.into(),
        options: Vec::new(),
    }
}

fn names(steps: &[SelectionView]) -> Vec<&str> {
    steps.iter().map(|s| s.model_id.as_str()).collect()
}

#[test]
fn a_step_moves_one_place_and_stays_at_the_ends() {
    let steps = vec![step("A", "a"), step("A", "b"), step("A", "c")];
    assert_eq!(
        names(&moved(steps.clone(), 1, Direction::Up)),
        ["b", "a", "c"]
    );
    assert_eq!(
        names(&moved(steps.clone(), 1, Direction::Down)),
        ["a", "c", "b"]
    );
    assert_eq!(
        names(&moved(steps.clone(), 0, Direction::Up)),
        ["a", "b", "c"]
    );
    assert_eq!(
        names(&moved(steps.clone(), 2, Direction::Down)),
        ["a", "b", "c"]
    );
}

#[test]
fn a_dragged_step_lands_at_the_place_of_the_row_it_drops_on() {
    let steps = vec![step("A", "a"), step("A", "b"), step("A", "c")];
    assert_eq!(names(&dropped(steps.clone(), 0, 2)), ["b", "c", "a"]);
    assert_eq!(names(&dropped(steps.clone(), 2, 0)), ["c", "a", "b"]);
    assert_eq!(names(&dropped(steps.clone(), 1, 1)), ["a", "b", "c"]);
    assert_eq!(names(&without(steps, 1)), ["a", "c"]);
}

#[test]
fn the_step_menu_dims_move_up_on_the_first_step_and_move_down_on_the_last() {
    let first = step_menu(0, 3);
    let labels: Vec<&str> = first.iter().map(|i| i.label.as_ref()).collect();
    assert_eq!(labels, ["Move up", "Move down", "Remove"]);
    assert!(first[0].disabled && !first[1].disabled && !first[2].disabled);
    let last = step_menu(2, 3);
    assert!(!last[0].disabled && last[1].disabled);
    assert!(matches!(last[2].action, MenuAction::Fallback(_)));
    let only = step_menu(0, 1);
    assert!(only[0].disabled && only[1].disabled && !only[2].disabled);
}

#[test]
fn the_mock_path_asks_for_no_dialog_because_it_has_a_step_on_another_harness() {
    let state = mock::view_state();
    let path = &state.fallback_path;
    assert!(path.has_other_harness_step());
    // A model the path has shows its step and is not added again.
    let again = step("Claude Work", "claude-sonnet-5-5");
    assert_eq!(
        add_outcome(path, &state.model_groups, &again),
        AddOutcome::Repeat(1)
    );
    let other = step("Codex Personal", "gpt-6-astra");
    assert_eq!(
        add_outcome(path, &state.model_groups, &other),
        AddOutcome::Add
    );
    // With no other-harness step, the first one asks once.
    let mut empty = path.clone();
    empty.steps.clear();
    assert_eq!(
        add_outcome(&empty, &state.model_groups, &other),
        AddOutcome::Confirm
    );
    // A step on the harness of the first choice never asks.
    let same = step("Claude Work", "claude-opus-5-5");
    assert_eq!(
        add_outcome(&empty, &state.model_groups, &same),
        AddOutcome::Add
    );
}

fn open_models(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, gpui_kit::Entity<ConsortApp>) {
    let (handle, app) = open(mock::view_state(), cx);
    let vcx = &mut VisualTestContext::from_window(handle, cx);
    vcx.update(|_, cx| app.update(cx, |app, cx| app.open_settings(SettingsSection::Models, cx)));
    settle(vcx, 100);
    (handle, app)
}

#[gpui::test]
fn the_models_page_shows_the_path_and_removes_a_step(cx: &mut TestAppContext) {
    let (handle, app) = open_models(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    for selector in [
        "fallback-path",
        "fallback-other-accounts",
        "fallback-step-0",
        "fallback-step-2",
        "fallback-add",
    ] {
        assert!(has(cx, selector), "{selector} draws");
    }
    app.read_with(cx, |app, _| {
        assert_eq!(app.state.fallback_path.steps.len(), 3)
    });
    crate::test_support::click(cx, "fallback-remove-1");
    until(cx, &app, "the step to go", |app| {
        app.state.fallback_path.steps.len() == 2
    });
    app.read_with(cx, |app, _| {
        let steps = &app.state.fallback_path.steps;
        assert_eq!(steps[0].selection.model_id, "claude-sonnet-5-5");
        assert_eq!(steps[1].selection.model_id, "gpt-5.5-mini");
    });
}

#[gpui::test]
fn the_add_step_picker_adds_a_step_and_marks_the_steps_that_the_path_has(cx: &mut TestAppContext) {
    let (handle, app) = open_models(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    crate::test_support::click(cx, "fallback-add");
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.picker.as_ref().map(|p| p.target.clone()),
            Some(PickerTarget::FallbackStep)
        );
        // The path has Sonnet of Claude Work as step 1.
        assert_eq!(
            app.task_note(
                &PickerTarget::FallbackStep,
                "Claude Work",
                "claude-sonnet-5-5"
            )
            .as_deref(),
            Some("Step 1")
        );
    });
    cx.simulate_input("haiku 5.5");
    cx.simulate_keystrokes("enter");
    until(cx, &app, "the step to join", |app| {
        app.state.fallback_path.steps.len() == 4
    });
    app.read_with(cx, |app, _| {
        assert!(app.picker.is_none());
        let last = app.state.fallback_path.steps.last().expect("a step");
        assert_eq!(last.selection.model_id, "claude-haiku-5-5");
        assert!(app.dialog.is_none(), "the same harness asks nothing");
    });
}

#[gpui::test]
fn the_first_step_on_another_harness_asks_once_and_cancel_adds_nothing(cx: &mut TestAppContext) {
    let (handle, app) = open_models(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    // Clear the steps, so no step is on another harness.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.save_fallback_path(true, Vec::new(), cx);
            let _ = window;
        })
    });
    until(cx, &app, "an empty path", |app| {
        app.state.fallback_path.steps.is_empty()
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.add_fallback_step(step_for("Codex Personal", "gpt-5.5", app), window, cx)
        })
    });
    settle(cx, 100);
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::ConfirmOtherHarnessStep));
        assert!(app.fallback_ui.pending.is_some());
    });
    assert!(has(cx, "dialog-confirm"));
    cx.update(|window, cx| app.update(cx, |app, cx| app.close_dialog(window, cx)));
    settle(cx, 100);
    app.read_with(cx, |app, _| {
        assert!(app.dialog.is_none());
        assert!(app.state.fallback_path.steps.is_empty());
    });
    // Ask again and confirm.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.add_fallback_step(step_for("Codex Personal", "gpt-5.5", app), window, cx)
        })
    });
    settle(cx, 100);
    crate::test_support::click(cx, "dialog-confirm");
    until(cx, &app, "the step to join", |app| {
        app.state.fallback_path.steps.len() == 1
    });
    app.read_with(cx, |app, _| {
        assert!(app.dialog.is_none());
        assert!(app.state.fallback_path.steps[0].other_harness);
    });
}

fn step_for(label: &str, model: &str, app: &ConsortApp) -> SelectionView {
    let name = app
        .state
        .model_groups
        .iter()
        .find(|g| g.account_label == label)
        .and_then(|g| g.models.iter().find(|m| m.id == model))
        .map(|m| m.name.clone())
        .unwrap_or_else(|| model.to_owned());
    SelectionView {
        model_name: name,
        ..step(label, model)
    }
}

/// A state where the assistant of Tutti runs on a fallback.
fn on_a_fallback(returns_at: i64) -> consort_client::ViewState {
    let mut state = mock::view_state();
    let tutti = state
        .channels
        .iter_mut()
        .find(|c| c.id == mock::tutti_id())
        .expect("Tutti");
    tutti.fallback = Some(ChannelFallbackView {
        from: SelectionView {
            model_name: "Opus 5.5".into(),
            ..step("Claude Personal", "claude-opus-5-5")
        },
        to: SelectionView {
            model_name: "GPT-5.5".into(),
            ..step("Codex Personal", "gpt-5.5")
        },
        cause: FallbackCauseView::UsageLimit,
        returns_at: Some(returns_at),
        available_label: (returns_at > mock::MOCK_NOW).then(|| "Available at 4:10 PM".to_owned()),
    });
    state
}

#[gpui::test]
fn the_chip_shows_the_marker_and_its_menu_offers_a_way_back(cx: &mut TestAppContext) {
    let (handle, app) = open(on_a_fallback(mock::MOCK_NOW + 3_600_000), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(has(cx, "fallback-marker"), "the marker draws on the chip");
    app.read_with(cx, |app, _| {
        let (name, account, _) = app.chip_texts(&PickerTarget::Assistant(app.channel_id.clone()));
        assert_eq!(
            (name.as_str(), account.as_str()),
            ("GPT-5.5", "Codex Personal")
        );
    });
    crate::test_support::click(cx, "composer-model");
    app.read_with(cx, |app, _| {
        assert!(app.picker.is_none(), "the chip opens its own menu");
        let menu = app.menu.as_ref().expect("the fallback menu");
        let header = menu.header.as_ref().expect("a header");
        assert_eq!(header.title.as_ref(), "On a fallback");
        let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(
            labels,
            ["Go back to Opus 5.5", "Choose another model", "Edit path"]
        );
        // Before the reset, Go back waits and says when.
        assert!(menu.items[0].disabled);
        assert_eq!(
            menu.items[0].trailing.as_deref(),
            Some("Available at 4:10 PM")
        );
    });
}

#[gpui::test]
fn after_the_reset_the_menu_goes_back_and_the_marker_leaves(cx: &mut TestAppContext) {
    let (handle, app) = open(on_a_fallback(mock::MOCK_NOW - 60_000), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    crate::test_support::click(cx, "composer-model");
    app.read_with(cx, |app, _| {
        let menu = app.menu.as_ref().expect("the fallback menu");
        assert!(!menu.items[0].disabled);
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let id = app.channel_id.clone();
            app.run_fallback(crate::fallback::FallbackAction::GoBack(id), window, cx)
        })
    });
    until(cx, &app, "the assistant to go back", |app| {
        app.state
            .channel(&app.channel_id)
            .is_some_and(|c| c.fallback.is_none())
    });
}

#[gpui::test]
fn the_models_page_is_in_the_settings_rail_and_edit_path_opens_it(cx: &mut TestAppContext) {
    let (handle, app) = open(mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_fallback(crate::fallback::FallbackAction::EditPath, window, cx)
        })
    });
    settle(cx, 100);
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, Page::Settings);
        assert_eq!(app.settings_section, SettingsSection::Models);
    });
    assert!(has(cx, "fallback-path"));
}

fn notice_in_timeline(
    state: &mut consort_client::ViewState,
    fallback: FallbackNoticeView,
) -> String {
    let timeline = state
        .timelines
        .iter_mut()
        .find(|t| t.channel_id == mock::tutti_id())
        .expect("the Tutti timeline");
    let id = "msg_fallback".to_owned();
    timeline
        .items
        .push(TimelineItem::Notice(consort_client::NoticeView {
            message_id: id.clone(),
            time_label: "3:53 PM".into(),
            created_at: mock::MOCK_NOW,
            kind: "account_fallback".into(),
            text: fallback.text.clone(),
            fallback: Some(fallback),
            ..consort_client::NoticeView::default()
        }));
    id
}

#[gpui::test]
fn a_move_notice_has_details_edit_path_and_go_back_now(cx: &mut TestAppContext) {
    let mut state = on_a_fallback(mock::MOCK_NOW - 60_000);
    let id = notice_in_timeline(
        &mut state,
        FallbackNoticeView {
            kind: FallbackNoticeKind::Moved,
            text: "Claude Personal reached its usage limit, so the assistant moved to Codex Personal, GPT-5.5. It returns at 4:10 PM.".into(),
            scope: FallbackScopeView::Assistant,
            from: Some("Claude Personal, Opus 5.5".into()),
            to: Some("Codex Personal, GPT-5.5".into()),
            provider: Some("OpenAI".into()),
            returns_label: Some("It returns at 4:10 PM".into()),
            returns_at: Some(mock::MOCK_NOW - 60_000),
            account_label: None,
            lines: consort_client::fallback::path_line_views(&[], 0, false),
            go_back: true,
        },
    );
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    app.read_with(cx, |app, _| assert_eq!(app.channel_id, mock::tutti_id()));
    let selector = |name: &str| format!("{name}-{id}");
    assert!(has(cx, &selector("fallback-notice")), "the notice draws");
    assert!(has(cx, &selector("fallback-go-back")), "Go back now draws");
    assert!(has(cx, &selector("fallback-edit")), "Edit path draws");
    crate::test_support::click(cx, &selector("fallback-go-back"));
    until(cx, &app, "the assistant to go back", |app| {
        app.state
            .channel(&app.channel_id)
            .is_some_and(|c| c.fallback.is_none())
    });
    crate::test_support::click(cx, &selector("fallback-edit"));
    app.read_with(cx, |app, _| {
        assert_eq!(app.settings_section, SettingsSection::Models)
    });
}
