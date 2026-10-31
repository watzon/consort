//! The system notice row in a headless window: the time shares the baseline
//! of the first line of the text, for a notice of one line and for a notice
//! that wraps.

use crate::app::ConsortApp;
use crate::test_support::{bounds, open, settle};
use consort_client::{
    NoticeView, TimelineItem, ViewState,
    mock::{self},
};
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::px;

fn with_notice(text: &str) -> ViewState {
    let mut state = mock::view_state();
    let tutti = mock::tutti_id();
    for tl in &mut state.timelines {
        if tl.channel_id == tutti {
            tl.items = vec![TimelineItem::Notice(NoticeView {
                message_id: "m_notice".into(),
                time_label: "1:45 PM".into(),
                created_at: 2_000,
                kind: "x".into(),
                text: text.into(),
                ..NoticeView::default()
            })];
        }
    }
    state.activity.clear();
    consort_client::derive::refresh(&mut state);
    state
}

/// The baseline of the first line of the text and the baseline of the time,
/// as the window paints them, and the height of the text.
fn baselines(
    cx: &mut VisualTestContext,
    app: &gpui_kit::Entity<ConsortApp>,
) -> (gpui::Pixels, gpui::Pixels, gpui::Pixels) {
    let t = app.read_with(cx, |_, cx| ConsortApp::tokens(cx));
    let text = bounds(cx, "notice-text-m_notice");
    let time = bounds(cx, "notice-time-m_notice");
    let (ui, caption) = cx.update(|window, _| {
        (
            crate::ui::parts::baseline_in_line(window, t.text.ui, &t),
            crate::ui::parts::baseline_in_line(window, t.text.caption, &t),
        )
    });
    (text.top() + ui, time.top() + caption, text.size.height)
}

#[gpui::test]
fn the_time_shares_the_baseline_of_a_one_line_notice(cx: &mut TestAppContext) {
    let (handle, app) = open(with_notice("Waiting for a worker to come online."), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let t = app.read_with(cx, |_, cx| ConsortApp::tokens(cx));
    let (text, time, height) = baselines(cx, &app);
    assert!(
        height <= t.text.ui.line + px(0.5),
        "the notice is one line: {height:?}"
    );
    assert!(
        (text - time).abs() < px(0.5),
        "the baselines differ: text {text:?}, time {time:?}"
    );
    // The time is to the right of the text, on the same row.
    let (text_box, time_box) = (
        bounds(cx, "notice-text-m_notice"),
        bounds(cx, "notice-time-m_notice"),
    );
    assert!(time_box.left() >= text_box.right());
}

#[gpui::test]
fn the_time_shares_the_baseline_of_the_first_line_of_a_wrapped_notice(cx: &mut TestAppContext) {
    let long = "The account Claude Code (Personal 2) is signed out. Sign in to Claude Code (Personal 2) in Settings, and the assistant answers your waiting messages. ".repeat(4);
    let (handle, app) = open(with_notice(&long), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let t = app.read_with(cx, |_, cx| ConsortApp::tokens(cx));
    let (text, time, height) = baselines(cx, &app);
    assert!(
        height > t.text.ui.line * 1.5,
        "the notice wraps: {height:?}"
    );
    assert!(
        (text - time).abs() < px(0.5),
        "the baselines differ: text {text:?}, time {time:?}"
    );
}

/// The old row aligned the bottoms of the two boxes. With the type styles of
/// the app, that puts the time lower than the baseline of the text. This
/// test names the size of the old error, so the fix stays a fix.
#[gpui::test]
fn the_old_bottom_alignment_would_leave_the_time_too_low(cx: &mut TestAppContext) {
    let (handle, app) = open(with_notice("Waiting for a worker to come online."), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 50);
    let t = app.read_with(cx, |_, cx| ConsortApp::tokens(cx));
    let (ui, caption) = cx.update(|window, _| {
        (
            crate::ui::parts::baseline_in_line(window, t.text.ui, &t),
            crate::ui::parts::baseline_in_line(window, t.text.caption, &t),
        )
    });
    // Bottoms aligned: the distance from the baseline to the bottom of each
    // box is not the same, so the baselines are apart by this much.
    let apart = (t.text.ui.line - ui) - (t.text.caption.line - caption);
    assert!(apart.abs() > px(0.5), "{apart:?}");
}
