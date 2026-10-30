//! The corner notifications in a headless window (Paper E3 and E4): the
//! stack at the bottom right above the composer, the compact position, three
//! at most with the newest at the bottom, and the six seconds of information.

use std::time::Duration;

use crate::{
    app::ConsortApp,
    test_support::{bounds, click, draw, has, open, open_sized, settle},
};
use consort_client::mock;
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{Entity, px};

fn show_error(app: &Entity<ConsortApp>, text: &str, cx: &mut VisualTestContext) {
    let text = text.to_owned();
    cx.update(|_, cx| app.update(cx, |app, cx| app.show_error(text, cx)));
    draw(cx);
}

fn show_info(app: &Entity<ConsortApp>, text: &str, cx: &mut VisualTestContext) {
    let text = text.to_owned();
    cx.update(|_, cx| app.update(cx, |app, cx| app.show_notice(text, cx)));
    draw(cx);
}

fn serials(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) -> Vec<u64> {
    app.read_with(cx, |app, _| {
        app.notices.visible().map(|n| n.serial).collect()
    })
}

#[gpui::test]
fn the_stack_is_at_the_bottom_right_above_the_composer(cx: &mut TestAppContext) {
    let (handle, app) = open(mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    show_error(
        &app,
        "Could not upload budget-2026.xlsx. The file is larger than 100 MB. Choose a smaller file.",
        cx,
    );
    let (window, notice, composer) = (
        cx.update(|window, _| window.viewport_size()),
        bounds(cx, "notice-stack"),
        bounds(cx, "composer"),
    );
    let t = cx.update(|_, cx| ConsortApp::tokens(cx));
    assert_eq!(notice.size.width, t.layout.notice_width, "360 px wide");
    assert_eq!(
        window.width - notice.right(),
        t.space.lg,
        "16 px from the right edge"
    );
    assert_eq!(
        window.height - notice.bottom(),
        t.layout.notice_bottom,
        "124 px from the bottom"
    );
    assert!(
        notice.bottom() <= composer.top(),
        "the stack is above the composer, so it never covers Send"
    );
    let send = bounds(cx, "send");
    assert!(!notice.intersects(&send));
}

#[gpui::test]
fn an_error_has_an_icon_a_title_text_and_a_close_button_and_stays(cx: &mut TestAppContext) {
    let (handle, app) = open(mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    show_error(
        &app,
        "Could not save the channel settings. The hub did not answer in time.",
        cx,
    );
    let serial = serials(&app, cx)[0];
    assert!(has(cx, &format!("notice-{serial}")));
    // A long wait does not clear an error.
    cx.executor().advance_clock(Duration::from_secs(60));
    settle(cx, 50);
    assert!(has(cx, &format!("notice-{serial}")), "an error stays");
    // The close button dismisses it.
    click(cx, &format!("notice-close-{serial}"));
    assert!(!has(cx, &format!("notice-{serial}")));
    assert!(!has(cx, "notice-stack"));
}

#[gpui::test]
fn information_leaves_after_six_seconds_and_the_close_button_works_too(cx: &mut TestAppContext) {
    let (handle, app) = open(mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    show_info(&app, "Routine saved.", cx);
    let first = serials(&app, cx)[0];
    assert!(has(cx, &format!("notice-{first}")));
    cx.executor().advance_clock(Duration::from_secs(5));
    settle(cx, 50);
    assert!(has(cx, &format!("notice-{first}")), "still there at 5 s");
    cx.executor().advance_clock(Duration::from_secs(2));
    settle(cx, 50);
    assert!(!has(cx, &format!("notice-{first}")), "gone after 6 s");
    // Dismissed by hand before the timer.
    show_info(&app, "Copied the path.", cx);
    let second = serials(&app, cx)[0];
    click(cx, &format!("notice-close-{second}"));
    assert!(!has(cx, &format!("notice-{second}")));
}

#[gpui::test]
fn three_show_with_the_newest_at_the_bottom_and_the_others_wait(cx: &mut TestAppContext) {
    let (handle, app) = open(mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    show_info(&app, "Routine saved.", cx);
    show_error(&app, "Could not upload a. The file is too large.", cx);
    show_error(&app, "Could not save b. The hub did not answer.", cx);
    show_error(&app, "Could not save c. The hub did not answer.", cx);
    show_error(&app, "Could not save d. The hub did not answer.", cx);
    // The information took the third place, so it left for the third error.
    let shown = serials(&app, cx);
    assert_eq!(shown.len(), 3, "three at most");
    let boxes: Vec<_> = shown
        .iter()
        .map(|s| bounds(cx, &format!("notice-{s}")))
        .collect();
    assert!(
        boxes[0].bottom() < boxes[1].top() && boxes[1].bottom() < boxes[2].top(),
        "one above the other, the oldest at the top: {boxes:?}"
    );
    let t = cx.update(|_, cx| ConsortApp::tokens(cx));
    assert_eq!(
        boxes[1].top() - boxes[0].bottom(),
        t.space.sm,
        "8 px between cards"
    );
    app.read_with(cx, |app, _| {
        assert_eq!(app.notices.waiting(), 1, "the fourth error waits");
        let titles: Vec<_> = app
            .notices
            .visible()
            .map(|n| n.title().to_owned())
            .collect();
        assert_eq!(
            titles,
            ["Could not upload a", "Could not save b", "Could not save c"]
        );
    });
    assert!(cx.debug_bounds("notice-stack").is_some());
    // Dismiss the top one: the fourth error takes the bottom place.
    click(cx, &format!("notice-close-{}", shown[0]));
    app.read_with(cx, |app, _| {
        let titles: Vec<_> = app
            .notices
            .visible()
            .map(|n| n.title().to_owned())
            .collect();
        assert_eq!(
            titles,
            ["Could not save b", "Could not save c", "Could not save d"]
        );
        assert_eq!(app.notices.waiting(), 0);
    });
}

#[gpui::test]
fn the_stack_is_full_width_under_the_top_bar_in_the_compact_layout(cx: &mut TestAppContext) {
    let (handle, app) = open_sized(mock::view_state(), 600., 900., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    show_error(
        &app,
        "Could not upload budget-2026.xlsx. The file is larger than 100 MB. Choose a smaller file.",
        cx,
    );
    let t = cx.update(|_, cx| ConsortApp::tokens(cx));
    let stack = bounds(cx, "notice-stack");
    let window = cx.update(|window, _| window.viewport_size());
    assert_eq!(stack.left(), t.space.lg, "16 px margin at the left");
    assert_eq!(window.width - stack.right(), t.space.lg, "and at the right");
    assert_eq!(
        stack.top(),
        t.layout.notice_compact_top,
        "88 px from the top"
    );
    assert!(
        stack.top() > bounds(cx, "compact-menu").bottom(),
        "under the top bar"
    );
}

#[gpui::test]
fn a_window_of_760_to_1100_px_uses_the_compact_position(cx: &mut TestAppContext) {
    // 1000 px: the sidebar shows and the corner stack would cover the chat
    // column, so the stack takes the full width under the top bar.
    let (handle, app) = open_sized(mock::view_state(), 1000., 800., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    show_error(
        &app,
        "Could not save the channel settings. The hub did not answer.",
        cx,
    );
    let t = cx.update(|_, cx| ConsortApp::tokens(cx));
    let stack = bounds(cx, "notice-stack");
    assert_eq!(stack.top(), t.layout.notice_compact_top);
    assert_eq!(stack.left(), t.space.lg);
    assert!(
        stack.size.width > t.layout.notice_width,
        "wider than 360 px"
    );
    // The same window wider than the tokens ask keeps the corner.
    let (handle, app) = open_sized(mock::view_state(), 1200., 800., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    show_error(
        &app,
        "Could not save the channel settings. The hub did not answer.",
        cx,
    );
    let stack = bounds(cx, "notice-stack");
    assert_eq!(stack.size.width, t.layout.notice_width);
    assert!(stack.right() > px(1000.));
}
