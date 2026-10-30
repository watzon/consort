//! The one composer of the chat and the task thread, in a headless window:
//! the two fields line up, the tool row gives up its labels in a narrow
//! panel and never overflows, and the model chips of a task change the model
//! of the task.

use crate::app::ConsortApp;
use crate::test_support::{bounds, click, draw, has, open, open_sized, settle, until};
use consort_client::{
    SelectionView, TaskModelOption, ViewState,
    mock::{self, T_SAYSO},
};
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{Entity, px};

/// The mock, with a task that can switch to Claude Work. That pick starts a
/// new session, so it has a note.
fn state() -> ViewState {
    let mut state = mock::view_state();
    let thread = state
        .threads
        .iter_mut()
        .find(|t| t.task_id == T_SAYSO)
        .expect("the Sayso thread");
    for model in [
        ("claude-opus-5-5", "Opus 5.5"),
        ("claude-sonnet-5-5", "Sonnet 5.5"),
    ] {
        thread.model_options.push(TaskModelOption {
            account_label: "Claude Work".into(),
            model_id: model.0.into(),
            model_name: model.1.into(),
            starts_new_session: true,
            note: Some(consort_client::task_model::NEW_SESSION_NOTE.into()),
        });
    }
    state
}

fn open_thread(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_thread(T_SAYSO.into(), window, cx)));
    settle(cx, 200);
}

fn set_panel_width(app: &Entity<ConsortApp>, width: f32, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.layout.settings.panel_width = Some(width);
            cx.notify();
        })
    });
    draw(cx);
}

fn thread_selection(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) -> SelectionView {
    app.read_with(cx, |app, _| {
        app.state
            .thread(T_SAYSO)
            .and_then(|t| t.selection.clone())
            .expect("the task has a model")
    })
}

#[gpui::test]
fn the_chat_and_the_thread_composers_line_up_at_the_bottom(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    let field = bounds(cx, "composer");
    let reply = bounds(cx, "reply-box");
    assert_eq!(field.origin.y, reply.origin.y, "the same top");
    assert_eq!(field.bottom(), reply.bottom(), "the same bottom");
    assert_eq!(field.size.height, reply.size.height, "the same height");
    let tools = bounds(cx, "composer-tools");
    let reply_tools = bounds(cx, "reply-tools");
    assert_eq!(
        tools.origin.y, reply_tools.origin.y,
        "the tool rows line up"
    );
    assert_eq!(tools.size.height, reply_tools.size.height);
    assert_eq!(tools.bottom(), reply_tools.bottom());
    // The same parts: Send is inside the field, the tools sit under it.
    let send = bounds(cx, "reply-send");
    assert!(reply.contains(&send.center()), "Send is inside the field");
    assert!(reply_tools.origin.y >= reply.bottom());
    assert_eq!(
        bounds(cx, "send").size,
        send.size,
        "the same Send key in both"
    );
    for part in ["reply-attach", "reply-model", "reply-effort"] {
        assert!(has(cx, part), "{part} is under the thread field");
    }
}

#[gpui::test]
fn the_thread_composer_says_where_the_task_runs_when_there_is_room(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    set_panel_width(&app, 560., cx);
    // The line "Runs with" is gone. "Runs on" has the worker of the task, at
    // the right end of the row.
    let reply_tools = bounds(cx, "reply-tools");
    let runs_on = bounds(cx, "reply-runs-on");
    assert!(runs_on.right() > reply_tools.center().x);
    assert!(runs_on.right() <= reply_tools.right());
}

#[gpui::test]
fn the_tool_row_drops_its_labels_in_a_narrow_panel_and_never_overflows(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    // A wide panel has room for the account name and Runs on.
    set_panel_width(&app, 560., cx);
    assert!(has(cx, "reply-runs-on"), "Runs on shows when there is room");
    let wide_model = bounds(cx, "reply-model").size.width;
    // The least width of a panel: first Runs on goes, then the account.
    set_panel_width(&app, 320., cx);
    let panel = bounds(cx, "thread-panel");
    assert_eq!(panel.size.width, px(320.));
    assert!(!has(cx, "reply-runs-on"), "Runs on goes first");
    let narrow_model = bounds(cx, "reply-model");
    assert!(
        narrow_model.size.width < wide_model,
        "the account name goes next: {:?} against {wide_model:?}",
        narrow_model.size.width
    );
    for part in [
        "reply-box",
        "reply-tools",
        "reply-send",
        "reply-attach",
        "reply-model",
        "reply-effort",
    ] {
        let b = bounds(cx, part);
        assert!(
            b.left() >= panel.left() && b.right() <= panel.right(),
            "{part} stays inside the panel: {b:?} in {panel:?}"
        );
    }
    // Every chip is on the one line of the row.
    let tools = bounds(cx, "reply-tools");
    for part in ["reply-attach", "reply-model", "reply-effort"] {
        let b = bounds(cx, part);
        assert!(
            b.top() >= tools.top() && b.bottom() <= tools.bottom(),
            "{part} does not wrap out of the row"
        );
    }
}

#[gpui::test]
fn the_main_composer_in_a_narrow_window_never_overflows(cx: &mut TestAppContext) {
    // The compact layout: one column, 420 px wide.
    let (handle, _app) = open_sized(state(), 420., 700., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    draw(cx);
    let tools = bounds(cx, "composer-tools");
    assert!(
        tools.right() <= px(420.),
        "the tool row stays in the window"
    );
    for part in ["composer-attach", "composer-model", "composer-effort"] {
        let b = bounds(cx, part);
        assert!(b.right() <= tools.right(), "{part} stays in the row");
        assert!(b.bottom() <= tools.bottom(), "{part} stays on one line");
    }
    assert!(!has(cx, "runs-on"), "Runs on has no room");
}

#[gpui::test]
fn a_task_picker_choice_sends_the_update_and_shows_the_note(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    assert_eq!(thread_selection(&app, cx).account_label, "Codex Personal");
    click(cx, "reply-model");
    app.read_with(cx, |app, _| {
        let open = app.picker.as_ref().expect("the picker opens");
        assert_eq!(
            open.target,
            crate::ui::model_picker::PickerTarget::Task(T_SAYSO.into())
        );
    });
    // Only the models of the worker of the task are offered.
    assert!(has(cx, "model-row-Claude Work-claude-opus-5-5"));
    assert!(has(cx, "model-row-Codex Personal-gpt-6.1-sol"));
    assert!(
        !has(cx, "model-row-OpenCode with Fireworks-fireworks/glm-5p3"),
        "a model that the worker of the task does not have is not offered"
    );
    assert!(!has(cx, "model-row-Claude Personal-claude-opus-5-5"));
    // The pick of another account has the note. The pick of the same
    // account has none.
    let notes = (0..8)
        .filter(|n| has(cx, &format!("pick-note-{n}")))
        .count();
    assert_eq!(notes, 1, "one row has the new session note");
    app.read_with(cx, |app, _| {
        let target = crate::ui::model_picker::PickerTarget::Task(T_SAYSO.into());
        assert_eq!(
            app.task_note(&target, "Claude Work", "claude-opus-5-5")
                .as_deref(),
            Some("This starts a new session in the same folder.")
        );
        assert_eq!(
            app.task_note(&target, "Codex Personal", "gpt-6.1-sol"),
            None
        );
    });
    click(cx, "model-row-Claude Work-claude-opus-5-5");
    until(cx, &app, "the task takes the model", |app| {
        app.state
            .thread(T_SAYSO)
            .and_then(|t| t.selection.as_ref())
            .is_some_and(|s| s.account_label == "Claude Work")
    });
    let chosen = thread_selection(&app, cx);
    assert_eq!(chosen.model_id, "claude-opus-5-5");
    app.read_with(cx, |app, _| {
        assert!(app.picker.is_none(), "the picker closes")
    });
}

#[gpui::test]
fn the_effort_chip_of_a_task_changes_its_effort(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    click(cx, "reply-effort");
    app.read_with(cx, |app, _| {
        let open = app.effort_menu.as_ref().expect("the effort menu opens");
        assert_eq!(
            open.target,
            crate::ui::model_picker::PickerTarget::Task(T_SAYSO.into())
        );
    });
    cx.simulate_keystrokes("up enter");
    until(cx, &app, "the task takes the effort", |app| {
        app.state
            .thread(T_SAYSO)
            .and_then(|t| t.selection.as_ref())
            .is_some_and(|s| s.effort() != Some("high"))
    });
    assert_eq!(thread_selection(&app, cx).account_label, "Codex Personal");
}

#[gpui::test]
fn a_task_with_no_model_has_no_divider_after_the_mic(cx: &mut TestAppContext) {
    let mut state = state();
    let thread = state
        .threads
        .iter_mut()
        .find(|t| t.task_id == T_SAYSO)
        .expect("the Sayso thread");
    thread.selection = None;
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    assert!(has(cx, "reply-voice"), "the mic stays");
    assert!(!has(cx, "reply-divider"), "no divider without chips");
    assert!(has(cx, "composer-divider"), "the channel keeps its divider");
}

#[gpui::test]
fn chips_of_a_task_that_cannot_change_show_but_do_not_open(cx: &mut TestAppContext) {
    let mut state = state();
    state
        .threads
        .iter_mut()
        .find(|t| t.task_id == T_SAYSO)
        .expect("the thread")
        .can_change_model = false;
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    assert!(
        has(cx, "reply-model") && has(cx, "reply-effort"),
        "they show"
    );
    click(cx, "reply-model");
    click(cx, "reply-effort");
    app.read_with(cx, |app, _| {
        assert!(app.picker.is_none(), "the model chip does not open");
        assert!(app.effort_menu.is_none(), "the effort chip does not open");
    });
}
