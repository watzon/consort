//! The draft card in a headless window: the card that waits (its parts, its
//! tick and its Send), Edit in place, the line after the answer, and the
//! card that `show_draft` posts (Edit and Copy, no Send).

use crate::app::ConsortApp;
use crate::test_support::{bounds, click, has, maybe_bounds, open, settle, until};
use consort_client::{
    ApprovalCardView, ApprovalView, DraftOutcomeView, HubCardView, NoticeView, PostBody,
    RequestState, ShownDraftView, Speaker, TimelineItem, ViewState,
    mock::{self},
    mock_voice::{DRAFT_APPROVAL, DRAFT_BODY, REPLY_TASK, draft_approval},
};
use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext};
use gpui_kit::px;

const ANSWER_LINE: f32 = 36.0;

fn contains(outer: Bounds<Pixels>, inner: Bounds<Pixels>) -> bool {
    inner.left() >= outer.left() - px(0.5)
        && inner.right() <= outer.right() + px(0.5)
        && inner.top() >= outer.top() - px(0.5)
        && inner.bottom() <= outer.bottom() + px(0.5)
}

fn near(a: Pixels, b: Pixels) -> bool {
    (a - b).abs() < px(1.0)
}

/// The draft approval with no flags, so Send is on.
fn clean_draft() -> ApprovalView {
    let mut approval = draft_approval();
    if let Some(card) = approval.card.as_mut()
        && let HubCardView::Connector(card) = card.as_mut()
        && let Some(draft) = card.draft.as_mut()
    {
        draft.flags.clear();
    }
    approval
}

/// A draft that the user answered, with the outcome text.
fn closed_draft(
    id: &str,
    outcome: &str,
    final_text: Option<&str>,
    state: RequestState,
) -> ApprovalView {
    let mut approval = clean_draft();
    approval.id = id.into();
    approval.state = state;
    if let Some(card) = approval.card.as_mut()
        && let HubCardView::Connector(card) = card.as_mut()
        && let Some(draft) = card.draft.as_mut()
    {
        draft.outcome = Some(DraftOutcomeView {
            text: outcome.into(),
            final_text: final_text.map(str::to_owned),
        });
    }
    approval
}

/// The mock with these items as the only items of the Tutti timeline.
fn timeline(items: Vec<TimelineItem>) -> ViewState {
    let mut state = mock::view_state();
    for task in &mut state.tasks {
        task.status = consort_client::TaskStatus::Resolved;
    }
    let tutti = mock::tutti_id();
    for tl in &mut state.timelines {
        if tl.channel_id == tutti {
            tl.items = items.clone();
        }
    }
    state.activity.clear();
    consort_client::derive::refresh(&mut state);
    state
}

fn card(message_id: &str, at: i64, approval: ApprovalView) -> TimelineItem {
    TimelineItem::Approval(ApprovalCardView {
        message_id: message_id.into(),
        speaker: Speaker::Consort,
        time_label: "9:00 AM".into(),
        created_at: at,
        approval,
    })
}

fn shown_notice(message_id: &str, at: i64) -> TimelineItem {
    TimelineItem::Notice(NoticeView {
        message_id: message_id.into(),
        time_label: "9:00 AM".into(),
        created_at: at,
        kind: "draft".into(),
        text: "Draft: Water heater claim for iBex".into(),
        draft: Some(shown_draft(message_id)),
        ..NoticeView::default()
    })
}

fn shown_draft(message_id: &str) -> ShownDraftView {
    ShownDraftView {
        message_id: message_id.into(),
        title: "Water heater claim for iBex".into(),
        to: Some("iBex, in your claim form or on the phone".into()),
        body: "I would like to open a claim for my water heater. It is an older tank unit.".into(),
        edited: false,
        original_body: None,
    }
}

fn waiting_state() -> ViewState {
    timeline(vec![card("m_draft", 1_000, draft_approval())])
}

#[gpui::test]
fn a_draft_that_waits_has_its_parts_in_one_card_at_the_bubble_width(cx: &mut TestAppContext) {
    let (handle, _app) = open(waiting_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let frame = bounds(cx, "m_draft-draft-card");
    assert!(
        frame.size.width <= px(560.0) + px(0.5),
        "at most the bubble width"
    );
    for part in [
        "m_draft-draft-title",
        "m_draft-draft-line-0",
        "m_draft-draft-line-1",
        "m_draft-draft-body",
        "m_draft-draft-footer",
        "draft-copy",
        "draft-edit",
        "draft-deny",
        "draft-allow-off",
        "draft-checked",
        "not-my-voice",
    ] {
        let b = bounds(cx, part);
        assert!(
            contains(frame, b),
            "{part} is inside the card: {b:?} in {frame:?}"
        );
    }
    // The order: title, To line, Subject line, text, footer.
    let ys: Vec<Pixels> = [
        "m_draft-draft-title",
        "m_draft-draft-line-0",
        "m_draft-draft-line-1",
        "m_draft-draft-body",
        "m_draft-draft-footer",
    ]
    .iter()
    .map(|p| bounds(cx, p).top())
    .collect();
    assert!(ys.windows(2).all(|w| w[0] < w[1]), "{ys:?}");
    // Copy and Edit sit in the title row, at its right end.
    let title = bounds(cx, "m_draft-draft-title");
    for icon in ["draft-copy", "draft-edit"] {
        let b = bounds(cx, icon);
        assert!(
            b.left() >= title.right() - px(0.5),
            "{icon} is at the right of the title"
        );
        assert!(b.top() < bounds(cx, "m_draft-draft-line-0").top());
    }
    // The tick sits in the footer, where the hint is when there are no flags.
    let footer = bounds(cx, "m_draft-draft-footer");
    assert!(contains(footer, bounds(cx, "draft-checked")));
    // The buttons keep their place at the right of the footer.
    assert!(bounds(cx, "draft-deny").right() <= bounds(cx, "draft-allow-off").left());
    // The footer has 14 px of padding at each side.
    assert!(near(
        bounds(cx, "draft-allow-off").right(),
        footer.right() - px(14.0)
    ));
}

#[gpui::test]
fn a_draft_with_no_flags_has_a_hint_and_send_is_on(cx: &mut TestAppContext) {
    let (handle, _app) = open(timeline(vec![card("m_clean", 1_000, clean_draft())]), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(!has(cx, "draft-checked"), "no tick without flags");
    assert!(has(cx, "draft-allow") && !has(cx, "draft-allow-off"));
    assert!(has(cx, "draft-edit") && has(cx, "draft-copy"));
}

#[gpui::test]
fn the_tick_takes_the_place_of_the_hint_and_turns_send_on(cx: &mut TestAppContext) {
    let (handle, app) = open(waiting_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(has(cx, "draft-allow-off") && !has(cx, "draft-allow"));
    let off = bounds(cx, "draft-allow-off");
    click(cx, "draft-checked");
    assert!(app.read_with(cx, |app, _| {
        app.draft_checked(DRAFT_APPROVAL, "mock-review-hash")
    }));
    let on = bounds(cx, "draft-allow");
    assert!(near(on.right(), off.right()), "Send stays where it was");
    assert!(!has(cx, "draft-allow-off"));
}

#[gpui::test]
fn edit_in_place_swaps_the_text_for_fields_and_cancel_brings_it_back(cx: &mut TestAppContext) {
    let (handle, app) = open(waiting_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let before = bounds(cx, "m_draft-draft-card");
    click(cx, "draft-edit");
    app.read_with(cx, |app, _| assert!(app.voice.editing.is_some()));
    let frame = bounds(cx, "m_draft-draft-card");
    assert!(has(cx, "draft-save") && has(cx, "draft-cancel"));
    assert!(
        !has(cx, "draft-allow") && !has(cx, "draft-allow-off") && !has(cx, "draft-deny"),
        "Send and Not now wait while the text is in Edit"
    );
    assert!(!has(cx, "m_draft-draft-body"), "the text became fields");
    assert!(
        !has(cx, "m_draft-draft-line-1"),
        "the Subject is a field in Edit"
    );
    for part in ["draft-save", "draft-cancel"] {
        assert!(
            contains(frame, bounds(cx, part)),
            "{part} is inside the card"
        );
    }
    assert!(bounds(cx, "draft-cancel").right() <= bounds(cx, "draft-save").left());
    click(cx, "draft-cancel");
    app.read_with(cx, |app, _| assert!(app.voice.editing.is_none()));
    assert!(has(cx, "m_draft-draft-body") && has(cx, "draft-allow-off"));
    assert_eq!(
        bounds(cx, "m_draft-draft-card").size.width,
        before.size.width
    );
}

#[gpui::test]
fn save_sends_the_new_text_and_the_card_checks_again(cx: &mut TestAppContext) {
    let (handle, app) = open(waiting_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    click(cx, "draft-checked");
    click(cx, "draft-edit");
    click(cx, "draft-save");
    // The mock checks the text again and finds no flags. The tick clears.
    until(cx, &app, "the card has no flags", |app| {
        app.voice.editing.is_none() && !app.draft_checked(DRAFT_APPROVAL, "mock-review-hash")
    });
    settle(cx, 150);
    assert!(!has(cx, "draft-checked"));
    assert!(has(cx, "draft-allow"));
}

#[gpui::test]
fn a_draft_card_stays_inside_a_narrow_thread_panel(cx: &mut TestAppContext) {
    let (handle, app) = open(mock_voice_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.layout.settings.panel_width = Some(320.0);
            app.open_thread(REPLY_TASK.into(), window, cx)
        })
    });
    settle(cx, 200);
    let panel = bounds(cx, "thread-panel");
    let frame = bounds(cx, "pst_reply_dana_2.0-draft-card");
    assert!(contains(panel, frame), "the card is inside the panel");
    for part in [
        "pst_reply_dana_2.0-draft-title",
        "pst_reply_dana_2.0-draft-body",
        "pst_reply_dana_2.0-draft-footer",
        "draft-copy",
        "draft-edit",
        "draft-deny",
        "draft-allow-off",
        "draft-checked",
    ] {
        let b = bounds(cx, part);
        assert!(
            contains(frame, b),
            "{part} stays in the card: {b:?} in {frame:?}"
        );
    }
}

fn mock_voice_state() -> ViewState {
    consort_client::mock_voice::screen_state("voice-approval").expect("the voice mock")
}

#[gpui::test]
fn the_line_after_the_answer_is_36_px_with_the_outcome_and_opens_the_final_text(
    cx: &mut TestAppContext,
) {
    let state = timeline(vec![
        card(
            "m_sent",
            1_000,
            closed_draft(
                "a_sent",
                "Sent to Dana Reyes",
                Some(DRAFT_BODY),
                RequestState::Answered,
            ),
        ),
        card(
            "m_replaced",
            2_000,
            closed_draft(
                "a_rep",
                "Replaced by a new version",
                None,
                RequestState::Closed,
            ),
        ),
        card(
            "m_not",
            3_000,
            closed_draft("a_not", "Not sent", None, RequestState::Answered),
        ),
    ]);
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    for key in ["m_sent", "m_replaced", "m_not"] {
        let line = bounds(cx, &format!("{key}-line"));
        assert!(
            near(line.size.height, px(ANSWER_LINE)),
            "{key}: {:?}",
            line.size.height
        );
        assert!(
            !has(cx, &format!("{key}-draft-card")),
            "no waiting card for {key}"
        );
    }
    assert!(!has(cx, "draft-allow") && !has(cx, "draft-allow-off"));
    // One click opens one line. The others stay shut.
    click(cx, "m_sent-line");
    app.read_with(cx, |app, _| {
        assert!(app.approvals_open.contains("m_sent"));
        assert!(!app.approvals_open.contains("m_not"));
    });
    let text = bounds(cx, "m_sent-draft-final");
    let open = bounds(cx, "m_sent-answered");
    assert!(contains(open, text), "the final text is in the open box");
    assert!(near(bounds(cx, "m_sent-line").size.height, px(ANSWER_LINE)));
    assert!(near(bounds(cx, "m_not-line").size.height, px(ANSWER_LINE)));
    // A card that did not run still shows the text that waited.
    click(cx, "m_not-line");
    assert!(has(cx, "m_not-draft-final"));
    click(cx, "m_sent-line");
    assert!(!has(cx, "m_sent-draft-final"), "a second click shuts it");
}

#[gpui::test]
fn a_shown_draft_has_edit_and_copy_and_no_send(cx: &mut TestAppContext) {
    let (handle, app) = open(timeline(vec![shown_notice("m_shown", 1_000)]), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let frame = bounds(cx, "m_shown-shown-draft-card");
    assert!(frame.size.width <= px(560.0) + px(0.5));
    for part in [
        "m_shown-shown-draft-title",
        "m_shown-shown-draft-line-0",
        "m_shown-shown-draft-body",
        "m_shown-shown-draft-footer",
        "draft-edit",
        "draft-copy",
    ] {
        assert!(
            contains(frame, bounds(cx, part)),
            "{part} is inside the card"
        );
    }
    for part in [
        "draft-allow",
        "draft-allow-off",
        "draft-deny",
        "draft-checked",
        "not-my-voice",
    ] {
        assert!(
            !has(cx, part),
            "{part} has no place on a card that cannot send"
        );
    }
    // Edit and Copy are buttons in the footer, side by side.
    assert!(bounds(cx, "draft-edit").right() <= bounds(cx, "draft-copy").left());
    // Copy puts the text on the clipboard and says so.
    click(cx, "draft-copy");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(
        copied.as_deref(),
        Some("I would like to open a claim for my water heater. It is an older tank unit.")
    );
    app.read_with(cx, |app, _| {
        assert!(
            app.notices
                .visible()
                .last()
                .is_some_and(|n| n.message == "Copied the text."),
            "Copy says so"
        );
    });
}

#[gpui::test]
fn edit_on_a_shown_draft_saves_the_new_text_through_the_client(cx: &mut TestAppContext) {
    let (handle, app) = open(timeline(vec![shown_notice("m_shown", 1_000)]), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    click(cx, "draft-edit");
    assert!(has(cx, "draft-save") && has(cx, "draft-cancel"));
    assert!(
        !has(cx, "draft-copy"),
        "Copy waits while the text is in Edit"
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.voice_inputs.body.update(cx, |body, cx| {
                body.set_value("A new text.".to_owned(), window, cx)
            });
        })
    });
    click(cx, "draft-save");
    until(cx, &app, "the card has the new text", |app| {
        shown_in(app).is_some_and(|d| d.body == "A new text." && d.edited)
    });
    settle(cx, 100);
    assert!(
        has(cx, "draft-edit") && has(cx, "draft-copy"),
        "the card is whole again"
    );
    assert!(maybe_bounds(cx, "draft-save").is_none());
}

fn shown_in(app: &ConsortApp) -> Option<ShownDraftView> {
    app.state
        .timeline(mock::tutti_id())?
        .items
        .iter()
        .find_map(|item| match item {
            TimelineItem::Notice(n) => n.draft.clone(),
            _ => None,
        })
}

#[gpui::test]
fn a_shown_draft_in_a_thread_panel_stays_inside_it(cx: &mut TestAppContext) {
    let mut state = mock::view_state();
    let thread = state
        .threads
        .iter_mut()
        .find(|t| t.task_id == mock::T_SAYSO)
        .expect("the thread");
    thread.posts = vec![consort_client::ThreadPost {
        id: "pst_shown".into(),
        speaker: Speaker::Task,
        time_label: "9:00 AM".into(),
        bodies: vec![PostBody::Notice {
            kind: "draft".into(),
            text: "Draft".into(),
            error: None,
            draft: Some(shown_draft("m_thread_shown")),
        }],
        pending: false,
        source_message_id: None,
        refused: None,
    }];
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.layout.settings.panel_width = Some(320.0);
            app.open_thread(mock::T_SAYSO.into(), window, cx)
        })
    });
    settle(cx, 200);
    let panel = bounds(cx, "thread-panel");
    let frame = bounds(cx, "pst_shown.0-shown-draft-card");
    assert!(contains(panel, frame));
    for part in ["draft-edit", "draft-copy", "pst_shown.0-shown-draft-footer"] {
        assert!(
            contains(frame, bounds(cx, part)),
            "{part} stays in the card"
        );
    }
}
