//! Approvals in a headless window: the card that waits (its wrapping, its
//! five lines and its row of equal buttons), the line after the answer (its
//! height, its open state, per approval and per place), and the hub cards.

use crate::test_support::{bounds, click, has, open, open_sized, settle, until};
use consort_client::{
    ApprovalCardView, ApprovalChoice, ApprovalView, PostBody, RequestState, Speaker, ThreadPost,
    TimelineItem, ViewState,
    approval_text::ApprovalKind,
    mock::{self, T_SAYSO},
    mock_extensions as mx,
};
use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext};
use gpui_kit::px;

const ANSWER_LINE: f32 = 36.0;

/// A command that wraps to more than five lines at the bubble width, with a
/// URL that has no spaces in it.
const LONG_COMMAND: &str = "cd /tmp/homedocs.cG4jpD && curl -sS -o closing.pdf 'https://temp.4d4f16c61d89ec64e760039c4ec50717.r2.cloudflarestorage.com/project_scope/ok_0OJWUtSO5sTh/pr_V6pJfaqsSwrO/files/mke6kmw4n0vl.pdf?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=7685aa30fee07632b83ea58979a398d0%2F20261030%2Fauto%2Fs3%2Faws4_request&X-Amz-Date=20261030T154500Z&X-Amz-Expires=3600&X-Amz-Signature=bb1b0a64d0d5a3f4c9e7a1d2c3b4a5968778695a4b3c2d1e0f'\ncurl -sS -o second.pdf 'https://example.com/a-very-long-address-with-no-spaces-in-it-at-all/and/more/and/more/and/more/and/more.pdf'\npdftotext -layout closing.pdf closing.txt";

/// A folder with one long word in it.
fn long_folder() -> String {
    format!(
        "/Users/chris/.consort-dev/hub/worker/workspaces/{}",
        "a1b2c3d4".repeat(24)
    )
}

fn approval(
    id: &str,
    summary: &str,
    detail: &str,
    folder: Option<String>,
    choices: &[ApprovalChoice],
) -> ApprovalView {
    ApprovalView {
        id: id.into(),
        task_id: Some("tsk_test".into()),
        summary: summary.into(),
        folder,
        detail: Some(detail.into()),
        kind: ApprovalKind::Command,
        choices: choices.to_vec(),
        state: RequestState::Pending,
        ..ApprovalView::default()
    }
}

fn answered(mut view: ApprovalView, answer: Option<ApprovalChoice>) -> ApprovalView {
    view.state = match answer {
        Some(_) => RequestState::Answered,
        None => RequestState::Closed,
    };
    view.answer = answer;
    view
}

const THREE: [ApprovalChoice; 3] = [
    ApprovalChoice::AllowOnce,
    ApprovalChoice::AllowForTask,
    ApprovalChoice::Deny,
];

/// The mock with the approvals as the only items of the Tutti timeline.
fn timeline_state(approvals: Vec<(&str, ApprovalView)>) -> ViewState {
    let mut state = mock::view_state();
    for task in &mut state.tasks {
        task.status = consort_client::TaskStatus::Resolved;
    }
    let tutti = mock::tutti_id();
    for timeline in &mut state.timelines {
        if timeline.channel_id == tutti {
            timeline.items = approvals
                .iter()
                .enumerate()
                .map(|(i, (message_id, approval))| {
                    TimelineItem::Approval(ApprovalCardView {
                        message_id: (*message_id).into(),
                        speaker: Speaker::Consort,
                        time_label: "9:00 AM".into(),
                        created_at: 1_000 + i as i64,
                        approval: approval.clone(),
                    })
                })
                .collect();
        }
    }
    state.activity.clear();
    consort_client::derive::refresh(&mut state);
    state
}

fn contains(outer: Bounds<Pixels>, inner: Bounds<Pixels>) -> bool {
    inner.left() >= outer.left() - px(0.5)
        && inner.right() <= outer.right() + px(0.5)
        && inner.top() >= outer.top() - px(0.5)
        && inner.bottom() <= outer.bottom() + px(0.5)
}

fn near(a: Pixels, b: Pixels) -> bool {
    (a - b).abs() < px(1.0)
}

fn waiting_state() -> ViewState {
    timeline_state(vec![(
        "msg_wait",
        approval(
            "apr_wait",
            "cd /tmp/homedocs.cG4jpD && curl -sS -o closing.pdf 'https://temp.4d4f16c61d89ec64e760039c4ec50717.r2.cloudflarestorage.com/…",
            LONG_COMMAND,
            Some(long_folder()),
            &THREE,
        ),
    )])
}

#[gpui::test]
fn a_waiting_card_wraps_long_text_inside_its_box(cx: &mut TestAppContext) {
    let (handle, _app) = open(waiting_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let card = bounds(cx, "msg_wait-card");
    assert!(
        card.size.width <= px(560.0) + px(0.5),
        "the card is at most the bubble width: {:?}",
        card.size.width
    );
    for part in ["msg_wait-command-text", "msg_wait-folder"] {
        let text = bounds(cx, part);
        assert!(
            contains(card, text),
            "{part} stays inside the card: {text:?} in {card:?}"
        );
    }
    // The folder is one long word. It breaks, so it takes more than a line.
    assert!(bounds(cx, "msg_wait-folder").size.height > px(20.0));
}

#[gpui::test]
fn long_text_wraps_inside_the_card_in_a_narrow_thread_panel(cx: &mut TestAppContext) {
    let mut state = mock::view_state();
    let thread = state
        .threads
        .iter_mut()
        .find(|t| t.task_id == T_SAYSO)
        .expect("the thread");
    thread.posts = vec![ThreadPost {
        id: "pst_wait".into(),
        speaker: Speaker::Task,
        time_label: "9:00 AM".into(),
        bodies: vec![PostBody::Approval(approval(
            "apr_thread",
            "",
            LONG_COMMAND,
            Some(long_folder()),
            &THREE,
        ))],
        pending: false,
        source_message_id: None,
        refused: None,
    }];
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.layout.settings.panel_width = Some(320.0);
            app.open_thread(T_SAYSO.into(), window, cx)
        })
    });
    settle(cx, 200);
    let panel = bounds(cx, "thread-panel");
    let card = bounds(cx, "pst_wait.0-card");
    assert!(contains(panel, card), "the card is inside the panel");
    for part in ["pst_wait.0-command-text", "pst_wait.0-folder"] {
        let text = bounds(cx, part);
        assert!(
            contains(card, text),
            "{part} stays inside the card: {text:?} in {card:?}"
        );
    }
    for i in 0..3 {
        let button = bounds(cx, &format!("apr_thread-{i}"));
        assert!(contains(card, button), "button {i} stays inside the card");
    }
}

#[gpui::test]
fn the_buttons_are_one_row_of_equal_widths(cx: &mut TestAppContext) {
    let (handle, _app) = open(waiting_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let card = bounds(cx, "msg_wait-card");
    let buttons: Vec<_> = (0..3)
        .map(|i| bounds(cx, &format!("apr_wait-{i}")))
        .collect();
    for b in &buttons {
        assert!(contains(card, *b), "{b:?} is inside the card");
        assert!(near(b.top(), buttons[0].top()), "one row");
        assert!(near(b.size.width, buttons[0].size.width), "equal widths");
        assert!(near(b.size.height, buttons[0].size.height), "equal heights");
    }
    assert!(buttons[0].right() <= buttons[1].left());
    assert!(buttons[1].right() <= buttons[2].left());
}

#[gpui::test]
fn a_long_button_label_gets_its_own_row_and_wraps_inside_it(cx: &mut TestAppContext) {
    let mut long = approval(
        "apr_long",
        "Run a command",
        "cargo build --release",
        None,
        &THREE,
    );
    long.allow_always_label = Some("Yes, and do not ask again for this command ".repeat(4));
    let state = timeline_state(vec![("msg_long", long)]);
    let (handle, _app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let card = bounds(cx, "msg_long-card");
    let (once, wide, deny) = (
        bounds(cx, "apr_long-0"),
        bounds(cx, "apr_long-1"),
        bounds(cx, "apr_long-2"),
    );
    for b in [once, wide, deny] {
        assert!(contains(card, b), "{b:?} is inside the card");
    }
    assert!(near(once.top(), deny.top()), "the short ones share a row");
    assert!(near(once.size.width, deny.size.width));
    assert!(wide.top() >= once.bottom(), "the long one goes under them");
    assert!(near(wide.left(), once.left()) && near(wide.right(), deny.right()));
    // The text wraps: the button grows, and the text does not run out of it.
    assert!(wide.size.height > once.size.height + px(10.0));
}

#[gpui::test]
fn a_card_with_no_task_choice_has_no_allow_for_this_task(cx: &mut TestAppContext) {
    let state = timeline_state(vec![(
        "msg_two",
        approval(
            "apr_two",
            "Send an email with Gmail Work",
            "To: support@ibexhw.com",
            None,
            &[ApprovalChoice::AllowOnce, ApprovalChoice::Deny],
        ),
    )]);
    let (handle, _app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let card = bounds(cx, "msg_two-card");
    let (a, b) = (bounds(cx, "apr_two-0"), bounds(cx, "apr_two-1"));
    assert!(!has(cx, "apr_two-2"), "two buttons only");
    assert!(near(a.size.width, b.size.width), "two equal widths");
    assert!(near(
        a.size.width * 2.0 + px(8.0),
        card.size.width - px(2.0) - px(28.0)
    ));
}

#[gpui::test]
fn a_long_command_stops_at_five_lines_and_show_all_opens_the_rest(cx: &mut TestAppContext) {
    let (handle, app) = open(waiting_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let mono_line = px(20.0);
    let stopped = bounds(cx, "msg_wait-command-text");
    assert!(
        near(stopped.size.height, mono_line * 5.0),
        "five lines of the mono line: {:?}",
        stopped.size.height
    );
    assert!(has(cx, "msg_wait-show-all"), "Show all is there");
    click(cx, "msg_wait-show-all");
    let all = bounds(cx, "msg_wait-command-text");
    assert!(
        all.size.height > mono_line * 5.0 + px(1.0),
        "the rest opens in place: {:?}",
        all.size.height
    );
    app.read_with(cx, |app, _| assert!(app.approvals_all.contains("msg_wait")));
    // Shorter text has no Show all.
    click(cx, "msg_wait-show-all");
    assert!(near(
        bounds(cx, "msg_wait-command-text").size.height,
        mono_line * 5.0
    ));
}

#[gpui::test]
fn a_short_command_has_no_show_all(cx: &mut TestAppContext) {
    let state = timeline_state(vec![(
        "msg_short",
        approval(
            "apr_short",
            "Delete files outside the workspace",
            "rm -rf ~/Documents/home-docs/old-scans",
            Some("/Users/chris/work".into()),
            &THREE,
        ),
    )]);
    let (handle, _app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(has(cx, "msg_short-card"));
    assert!(!has(cx, "msg_short-show-all"));
    assert!(near(
        bounds(cx, "msg_short-command-text").size.height,
        px(20.0)
    ));
}

fn answered_state() -> ViewState {
    let base = approval(
        "apr_x",
        "cd /tmp/ibex && curl -sSL -o terms.pdf https://ibexhw.com/files/terms.pdf",
        "cd /tmp/ibex.4Fwn9k && curl -sSL -A 'Mozilla/5.0' -o terms.pdf\nhttps://ibexhw.com/wasatchfront/files/ibex-wasatchfront.pdf && pdftotext -layout terms.pdf terms.txt",
        Some("/Users/chris/.consort-dev/hub/worker/workspaces/ses_01a18dec".into()),
        &THREE,
    );
    let mut once = answered(base.clone(), Some(ApprovalChoice::AllowOnce));
    once.id = "apr_once".into();
    let mut task = answered(base.clone(), Some(ApprovalChoice::AllowForTask));
    task.id = "apr_task".into();
    let mut deny = answered(base.clone(), Some(ApprovalChoice::Deny));
    deny.id = "apr_deny".into();
    let mut closed = answered(base, None);
    closed.id = "apr_closed".into();
    timeline_state(vec![
        ("msg_once", once),
        ("msg_task", task),
        ("msg_deny", deny),
        ("msg_closed", closed),
    ])
}

#[gpui::test]
fn the_line_after_the_answer_is_36_px_and_opens_in_place(cx: &mut TestAppContext) {
    let (handle, app) = open(answered_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    for key in ["msg_once", "msg_task", "msg_deny", "msg_closed"] {
        let line = bounds(cx, &format!("{key}-line"));
        assert!(
            near(line.size.height, px(ANSWER_LINE)),
            "{key}: the line is 36 px, not {:?}",
            line.size.height
        );
        let boxed = bounds(cx, &format!("{key}-answered"));
        assert!(
            near(boxed.size.height, px(ANSWER_LINE) + px(2.0)),
            "{key}: one line in a box"
        );
        assert!(boxed.size.width <= px(560.5));
    }
    // Open the second one. Only its box grows. The others keep their size,
    // and the gaps between the boxes stay as they are.
    let before: Vec<_> = ["msg_once", "msg_task", "msg_deny", "msg_closed"]
        .iter()
        .map(|k| bounds(cx, &format!("{k}-answered")))
        .collect();
    click(cx, "msg_task-line");
    app.read_with(cx, |app, _| {
        assert!(app.approvals_open.contains("msg_task"));
        assert_eq!(app.approvals_open.len(), 1, "one place, one approval");
    });
    let after: Vec<_> = ["msg_once", "msg_task", "msg_deny", "msg_closed"]
        .iter()
        .map(|k| bounds(cx, &format!("{k}-answered")))
        .collect();
    let growth = after[1].size.height - before[1].size.height;
    assert!(growth > px(20.0), "the open box grows: {growth:?}");
    for i in [0, 2, 3] {
        assert!(
            near(after[i].size.height, before[i].size.height),
            "box {i} keeps its height"
        );
    }
    // The gap between the boxes after the open one is unchanged.
    let gap = |b: &[Bounds<Pixels>], i: usize| b[i + 1].top() - b[i].bottom();
    assert!(near(gap(&before, 2), gap(&after, 2)));
    assert!(near(gap(&before, 1), gap(&after, 1)));
    // The open box has the full command and the folder.
    assert!(has(cx, "msg_task-command-text"));
    assert!(has(cx, "msg_task-folder"));
    assert!(contains(after[1], bounds(cx, "msg_task-command-text")));
    // The command shows in full: more than two lines of the long text.
    assert!(bounds(cx, "msg_task-command-text").size.height >= px(40.0));
    // A click on the line closes it again.
    click(cx, "msg_task-line");
    app.read_with(cx, |app, _| assert!(app.approvals_open.is_empty()));
    assert!(!has(cx, "msg_task-command-text"));
}

#[gpui::test]
fn the_open_state_is_for_one_approval_in_one_place(cx: &mut TestAppContext) {
    let mut state = answered_state();
    // The same approval, answered, in the thread of a task.
    let mut once = answered(
        approval("apr_once", "", "ls -la /tmp", Some("/tmp".into()), &THREE),
        Some(ApprovalChoice::AllowOnce),
    );
    once.id = "apr_once".into();
    let thread = state
        .threads
        .iter_mut()
        .find(|t| t.task_id == T_SAYSO)
        .expect("the thread");
    thread.posts = vec![ThreadPost {
        id: "pst_once".into(),
        speaker: Speaker::Task,
        time_label: "9:00 AM".into(),
        bodies: vec![PostBody::Approval(once)],
        pending: false,
        source_message_id: None,
        refused: None,
    }];
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_thread(T_SAYSO.into(), window, cx)));
    settle(cx, 200);
    // Both places show the line. Open the one in the thread.
    assert!(has(cx, "msg_once-line") && has(cx, "pst_once.0-line"));
    click(cx, "pst_once.0-line");
    app.read_with(cx, |app, _| {
        assert!(app.approvals_open.contains("pst_once.0"));
        assert!(!app.approvals_open.contains("msg_once"));
    });
    assert!(has(cx, "pst_once.0-folder"), "the thread's line is open");
    assert!(
        !has(cx, "msg_once-folder"),
        "the timeline's line stays closed"
    );
    // The open line of a thread shows the title in the past.
    assert!(near(
        bounds(cx, "pst_once.0-line").size.height,
        px(ANSWER_LINE)
    ));
}

#[gpui::test]
fn a_request_with_a_scope_keeps_the_words_of_the_harness_when_opened(cx: &mut TestAppContext) {
    let mut view = answered(
        approval("apr_scope", "", "cargo build --release", None, &THREE),
        Some(ApprovalChoice::AllowForTask),
    );
    view.allow_always_label = Some("Yes, and don't ask again for cargo build commands".into());
    let (handle, app) = open(timeline_state(vec![("msg_scope", view)]), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let closed = bounds(cx, "msg_scope-answered").size.height;
    click(cx, "msg_scope-line");
    // The words show under the command, so the box is taller than command
    // and folder alone: command box (20 + 2 + 16) plus the words (20).
    let open_height = bounds(cx, "msg_scope-answered").size.height;
    assert!(open_height > closed + px(20.0 + 20.0));
    app.read_with(cx, |app, _| {
        assert!(app.approvals_open.contains("msg_scope"))
    });
}

fn hub_key(state: &ViewState, approval_id: &str) -> String {
    state
        .timelines
        .iter()
        .flat_map(|t| &t.items)
        .find_map(|item| match item {
            TimelineItem::Approval(card) if card.approval.id == approval_id => {
                Some(card.message_id.clone())
            }
            _ => None,
        })
        .expect("the hub card")
}

#[gpui::test]
fn a_hub_card_keeps_its_body_while_it_waits_and_shrinks_to_the_line_after(cx: &mut TestAppContext) {
    let state = mock::view_state();
    let key = hub_key(&state, mx::APR_ACCESS);
    let (handle, app) = open_sized(state, 1440., 900., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(mx::HIRING.into(), window, cx)
        })
    });
    settle(cx, 200);
    assert!(
        has(cx, &format!("{key}-hub")),
        "the card keeps its own body"
    );
    assert!(!has(cx, &format!("{key}-line")), "it is not a line yet");
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.answer_approval(mx::APR_ACCESS.into(), ApprovalChoice::AllowOnce, window, cx)
        })
    });
    until(cx, &app, "the card is answered", |app| {
        app.state.timelines.iter().flat_map(|t| &t.items).any(|i| {
            matches!(i, TimelineItem::Approval(c)
                if c.approval.id == mx::APR_ACCESS && c.approval.state == RequestState::Answered)
        })
    });
    settle(cx, 100);
    assert!(near(
        bounds(cx, &format!("{key}-line")).size.height,
        px(ANSWER_LINE)
    ));
    assert!(
        !has(cx, &format!("{key}-hub")),
        "the body is hidden after the answer"
    );
    // Opened, it shows the whole card again, so no fact is lost.
    click(cx, &format!("{key}-line"));
    assert!(has(cx, &format!("{key}-hub")), "the body shows when opened");
    let boxed = bounds(cx, &format!("{key}-answered"));
    assert!(contains(boxed, bounds(cx, &format!("{key}-hub"))));
}
