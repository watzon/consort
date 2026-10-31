//! The gate of the dictation plan, step 3: a message box on `EditorState`.

use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::base::input::{Editor, EditorState};
use gpui_kit::component::input::{Paste, Redo, Undo};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, EntityInputHandler as _, Focusable as _,
    IntoElement, Render, Window, div, prelude::*, px, size,
};

struct Box_ {
    state: Entity<EditorState>,
}
impl Render for Box_ {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(600.))
            .h(px(200.))
            .child(Editor::new(&self.state))
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<EditorState>, VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
    });
    let (root, vcx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(false)
                .folding(false)
                .indent_guides(false)
                .searchable(false)
                .auto_close(false)
                .smart_indent(false)
                .soft_wrap(true)
                .scroll_beyond_last_line(Some(0))
                .submit_on_enter(true)
        });
        Box_ { state }
    });
    let state = root.read_with(vcx, |r, _| r.state.clone());
    vcx.simulate_resize(size(px(800.), px(600.)));
    vcx.run_until_parked();
    (state, vcx.clone())
}

fn text(state: &Entity<EditorState>, cx: &mut VisualTestContext) -> String {
    state.read_with(cx, |s, _| s.value().to_string())
}

fn act(state: &Entity<EditorState>, action: &dyn gpui_kit::Action, cx: &mut VisualTestContext) {
    let handle = state.read_with(cx, |s, cx| s.focus_handle(cx).clone());
    cx.update(|window, cx| handle.dispatch_action(action, window, cx));
    cx.run_until_parked();
}

fn typed(state: &Entity<EditorState>, t: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| state.update(cx, |s, cx| s.replace_text_in_range(None, t, window, cx)));
    cx.run_until_parked();
}

fn partial(state: &Entity<EditorState>, live: &mut bool, t: &str, cx: &mut VisualTestContext) {
    if *live {
        act(state, &Undo, cx);
    }
    cx.update(|window, cx| state.update(cx, |s, cx| s.insert(t.to_string(), window, cx)));
    *live = true;
    cx.run_until_parked();
}

#[gpui::test]
fn a_run_of_partials_is_one_undo_step(cx: &mut TestAppContext) {
    let (state, mut vcx) = open(cx);
    let cx = &mut vcx;
    typed(&state, "Please ", cx);
    typed(&state, "build. Thanks.", cx);
    // The caret goes before " Thanks."; moves break the coalescing.
    cx.update(|_, cx| state.update(cx, |s, cx| s.set_selected_range(13..13, cx)));
    let mut live = false;
    for p in ["then", "then push", "then pushed the", "then push the fix"] {
        partial(&state, &mut live, p, cx);
        assert_eq!(text(&state, cx), format!("Please build.{p} Thanks."));
    }
    let caret = state.read_with(cx, |s, _| s.cursor());
    assert_eq!(caret, 13 + "then push the fix".len());
    act(&state, &Undo, cx);
    assert_eq!(text(&state, cx), "Please build. Thanks.");
    assert_eq!(state.read_with(cx, |s, _| s.cursor()), 13);
    act(&state, &Redo, cx);
    assert_eq!(text(&state, cx), "Please build.then push the fix Thanks.");
    act(&state, &Undo, cx);
    // The draft has its own undo history under the dictated words.
    act(&state, &Undo, cx);
    assert_eq!(text(&state, cx), "");
}

#[gpui::test]
fn ime_composes_and_commits_as_one_step(cx: &mut TestAppContext) {
    let (state, mut vcx) = open(cx);
    let cx = &mut vcx;
    typed(&state, "a ", cx);
    for preedit in ["n", "ni", "你"] {
        cx.update(|window, cx| {
            state.update(cx, |s, cx| {
                s.replace_and_mark_text_in_range(None, preedit, None, window, cx)
            })
        });
    }
    let marked = cx.update(|window, cx| state.update(cx, |s, cx| s.marked_text_range(window, cx)));
    assert_eq!(marked, Some(2..3));
    cx.update(|window, cx| {
        state.update(cx, |s, cx| {
            s.replace_text_in_range(None, "你好", window, cx)
        })
    });
    assert_eq!(text(&state, cx), "a 你好");
    act(&state, &Undo, cx);
    assert_eq!(text(&state, cx), "a ");
}

#[gpui::test]
fn paste_inserts_plain_text_as_one_step(cx: &mut TestAppContext) {
    let (state, mut vcx) = open(cx);
    let cx = &mut vcx;
    typed(&state, "x", cx);
    cx.update(|_, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("one\ntwo\n    three".into()))
    });
    act(&state, &Paste, cx);
    assert_eq!(text(&state, cx), "xone\ntwo\n    three");
    act(&state, &Undo, cx);
    assert_eq!(text(&state, cx), "x");
}

/// The deepest that a message box scrolls with this text, in pixels.
fn deepest_scroll(state: &Entity<EditorState>, lines: usize, cx: &mut VisualTestContext) -> f32 {
    cx.update(|window, cx| {
        state.update(cx, |s, cx| {
            s.set_value(vec!["line"; lines].join("\n"), window, cx);
            s.set_scroll_offset(gpui_kit::point(px(0.), px(-100_000.)), cx);
        });
        window.refresh();
    });
    cx.run_until_parked();
    let y = state.read_with(cx, |s, _| s.scroll_offset().y);
    -f32::from(y)
}

#[gpui::test]
fn a_message_box_stops_scrolling_at_its_last_line(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
    });
    let (root, vcx) = cx.add_window_view(|window, cx| Box_ {
        state: cx.new(|cx| crate::ui::parts::message_box("Message Consort", window, cx)),
    });
    let state = root.read_with(vcx, |r, _| r.state.clone());
    vcx.simulate_resize(size(px(800.), px(600.)));
    vcx.run_until_parked();

    let at_40 = deepest_scroll(&state, 40, vcx);
    let at_80 = deepest_scroll(&state, 80, vcx);
    // 40 more lines give the height of 40 lines. The last line ends at the
    // bottom of the 200 px box, so nothing is left below it.
    let forty_lines = at_80 - at_40;
    assert!(forty_lines > 0., "the box did not scroll");
    assert!(
        (at_40 - (forty_lines - 200.)).abs() < 1.,
        "scrolled {at_40} px, and 40 lines are {forty_lines} px"
    );
}
