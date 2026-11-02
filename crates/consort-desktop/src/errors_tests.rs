//! The in-chat error and the refused message in a headless window (Paper E1
//! and E2): the card in the assistant lane at the bubble width, its actions
//! and Details, the quiet line, the same card in the task thread, and a
//! message that the hub refused in the lane of the user.

use crate::app::{ConsortApp, Dialog};
use crate::test_support::{bounds, click, has, maybe_bounds, open, settle};
use consort_client::{
    ErrorCardView, MessageView, NoticeView, PostBody, RefusedView, SendFailureView, Speaker,
    TaskStatus, ThreadPost, TimelineItem, TurnErrorKindView, ViewState,
    mock::{self, T_SAYSO},
    turn_errors::{TryAgainTarget, TurnFailed, error_card},
};
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{Entity, px};

const NOW: i64 = 1_000_000;

fn failed(kind: TurnErrorKindView, account: &str) -> TurnFailed {
    TurnFailed {
        kind,
        resets_at: None,
        account: Some(account.into()),
        path: Vec::new(),
        raw: "Internal error: You've hit your weekly limit, resets 4am (America/Denver): {\n  \"errorKind\": \"rate_limit\"\n}".into(),
    }
}

fn resend() -> Option<TryAgainTarget> {
    Some(TryAgainTarget::ResendMessage {
        channel_id: mock::tutti_id().into(),
        body: "OK, go ahead and start the build.".into(),
    })
}

fn card(kind: TurnErrorKindView, quiet: bool) -> ErrorCardView {
    error_card(&failed(kind, "Claude Lab"), resend(), quiet, NOW)
}

fn user_message(at: i64) -> TimelineItem {
    TimelineItem::Message(MessageView {
        id: "m_you".into(),
        channel_id: mock::tutti_id().into(),
        speaker: Speaker::You,
        time_label: "11:45 AM".into(),
        created_at: at,
        body: "OK, go ahead and start the build.".into(),
        ..MessageView::default()
    })
}

fn failed_notice(card: ErrorCardView) -> TimelineItem {
    TimelineItem::Notice(NoticeView {
        message_id: "m_failed".into(),
        time_label: "11:45 AM".into(),
        created_at: 2_000,
        kind: "turn_failed".into(),
        text: "The turn failed.".into(),
        error: Some(card),
        ..NoticeView::default()
    })
}

fn timeline(items: Vec<TimelineItem>) -> ViewState {
    let mut state = mock::view_state();
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

fn with_card(card: ErrorCardView) -> ViewState {
    timeline(vec![user_message(1_000), failed_notice(card)])
}

fn near(a: gpui::Pixels, b: gpui::Pixels) -> bool {
    (a - b).abs() < px(1.0)
}

#[gpui::test]
fn the_card_is_in_the_assistant_lane_at_the_bubble_width_with_its_parts(cx: &mut TestAppContext) {
    let (handle, app) = open(with_card(card(TurnErrorKindView::RateLimit, false)), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let t = app.read_with(cx, |_, cx| ConsortApp::tokens(cx));
    let frame = bounds(cx, "error-err-m_failed");
    let composer = bounds(cx, "composer");
    assert!(
        near(frame.size.width, t.layout.bubble_max_width),
        "560 px wide: {frame:?}"
    );
    assert!(
        near(frame.left(), composer.left()),
        "in the assistant lane, at the left of the column"
    );
    // The buttons are inside the card, and Details is at the right end of
    // their row.
    let choose = bounds(cx, "error-action-err-m_failed-ChooseAnotherModel");
    let again = bounds(cx, "error-action-err-m_failed-TryAgain");
    let details = bounds(cx, "error-details-err-m_failed");
    for part in [choose, again, details] {
        assert!(
            frame.contains(&part.center()),
            "{part:?} is inside {frame:?}"
        );
    }
    assert!(choose.right() < again.left(), "in order");
    assert!(near(choose.center().y, again.center().y));
    assert!(near(details.center().y, again.center().y), "one row");
    assert!(
        details.right() > frame.right() - t.space.xl,
        "Details is at the right end"
    );
    // The footer keeps its time, under the card.
    let time = bounds(cx, "error-time-err-m_failed");
    assert!(time.top() >= frame.bottom());
    assert!(time.right() <= frame.right() + px(0.5));
    // The writing bubble does not show with the card.
    assert!(!has(cx, "writing-bubble"));
}

#[gpui::test]
fn each_kind_has_the_actions_of_its_row(cx: &mut TestAppContext) {
    for (kind, actions) in [
        (
            TurnErrorKindView::RateLimit,
            vec!["ChooseAnotherModel", "TryAgain"],
        ),
        (
            TurnErrorKindView::ExpiredSignIn,
            vec!["SignInAgain", "ChooseAnotherModel", "TryAgain"],
        ),
        (TurnErrorKindView::WorkerRestarted, vec!["TryAgain"]),
        (TurnErrorKindView::Unknown, vec!["TryAgain"]),
    ] {
        let (handle, _app) = open(with_card(card(kind, false)), cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        settle(cx, 50);
        for name in ["SignInAgain", "ChooseAnotherModel", "TryAgain"] {
            assert_eq!(
                has(cx, &format!("error-action-err-m_failed-{name}")),
                actions.contains(&name),
                "{kind:?} {name}"
            );
        }
    }
}

#[gpui::test]
fn a_routine_run_or_a_check_in_has_no_try_again(cx: &mut TestAppContext) {
    // No message of the user started the turn, so there is nothing to send
    // again (`error_card` drops the button when `try_again` is `None`).
    let no_retry = error_card(
        &failed(TurnErrorKindView::RateLimit, "Claude Lab"),
        None,
        false,
        NOW,
    );
    let (handle, _app) = open(timeline(vec![failed_notice(no_retry)]), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 50);
    assert!(has(cx, "error-action-err-m_failed-ChooseAnotherModel"));
    assert!(!has(cx, "error-action-err-m_failed-TryAgain"));
}

#[gpui::test]
fn details_open_the_raw_text_as_selectable_mono_text(cx: &mut TestAppContext) {
    let (handle, app) = open(with_card(card(TurnErrorKindView::RateLimit, false)), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 50);
    assert!(!has(cx, "error-raw-err-m_failed"), "closed at first");
    let before = bounds(cx, "error-err-m_failed").size.height;
    click(cx, "error-details-err-m_failed");
    let raw = bounds(cx, "error-raw-err-m_failed");
    let frame = bounds(cx, "error-err-m_failed");
    assert!(frame.size.height > before, "the card grows for the text");
    assert!(frame.contains(&raw.center()) && raw.size.width > px(300.));
    app.read_with(cx, |app, _| {
        assert!(app.errors_open.contains("err-m_failed-details"));
    });
    // A second click closes it.
    click(cx, "error-details-err-m_failed");
    assert!(!has(cx, "error-raw-err-m_failed"));
}

#[gpui::test]
fn try_again_sends_the_last_message_again(cx: &mut TestAppContext) {
    let (handle, app) = open(with_card(card(TurnErrorKindView::Unknown, false)), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 50);
    let count = |app: &Entity<ConsortApp>, cx: &mut VisualTestContext| {
        app.read_with(cx, |app, _| {
            app.state
                .timeline(&app.channel_id)
                .map(|tl| {
                    tl.items
                        .iter()
                        .filter(|i| {
                            matches!(i, TimelineItem::Message(m)
                                if m.speaker == Speaker::You
                                    && m.body == "OK, go ahead and start the build.")
                        })
                        .count()
                })
                .unwrap_or_default()
        })
    };
    assert_eq!(count(&app, cx), 1);
    click(cx, "error-action-err-m_failed-TryAgain");
    settle(cx, 300);
    assert_eq!(count(&app, cx), 2, "the message shows twice");
}

#[gpui::test]
fn choose_another_model_opens_the_composer_picker_for_the_channel(cx: &mut TestAppContext) {
    let (handle, app) = open(with_card(card(TurnErrorKindView::RateLimit, false)), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 50);
    click(cx, "error-action-err-m_failed-ChooseAnotherModel");
    app.read_with(cx, |app, _| {
        let picker = app.picker.as_ref().expect("the model picker is open");
        assert_eq!(
            picker.target,
            crate::ui::model_picker::PickerTarget::Assistant(mock::tutti_id().into())
        );
    });
}

#[gpui::test]
fn sign_in_again_opens_the_sign_in_of_the_account(cx: &mut TestAppContext) {
    let mut c = card(TurnErrorKindView::ExpiredSignIn, false);
    c.account = Some("Claude Personal".into());
    let (handle, app) = open(with_card(c), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 50);
    click(cx, "error-action-err-m_failed-SignInAgain");
    app.read_with(cx, |app, _| {
        assert!(
            matches!(app.dialog, Some(Dialog::AddAccount(_))),
            "the sign-in dialog of the account opens: {:?}",
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
    let mut c = card(TurnErrorKindView::ExpiredSignIn, false);
    c.account = Some("An account that no worker has".into());
    let (handle, app) = open(with_card(c), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 50);
    click(cx, "error-action-err-m_failed-SignInAgain");
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, crate::app::Page::Settings);
        assert_eq!(app.settings_section, crate::app::SettingsSection::Providers);
    });
}

#[gpui::test]
fn a_quiet_card_is_one_line_with_no_buttons_and_a_click_opens_it(cx: &mut TestAppContext) {
    let (handle, app) = open(with_card(card(TurnErrorKindView::RateLimit, true)), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 50);
    let t = app.read_with(cx, |_, cx| ConsortApp::tokens(cx));
    let line = bounds(cx, "error-line-err-m_failed");
    assert!(!has(cx, "error-err-m_failed"), "no card");
    assert!(!has(cx, "error-action-err-m_failed-TryAgain"), "no buttons");
    assert!(!has(cx, "error-time-err-m_failed"));
    assert!(near(line.size.height, t.component.chat.footer_line));
    // A click opens the card, still with no buttons.
    click(cx, "error-line-err-m_failed");
    assert!(has(cx, "error-err-m_failed"));
    assert!(!has(cx, "error-line-err-m_failed"));
    for name in ["ChooseAnotherModel", "TryAgain"] {
        assert!(
            !has(cx, &format!("error-action-err-m_failed-{name}")),
            "old buttons do not come back: {name}"
        );
    }
    assert!(has(cx, "error-details-err-m_failed"), "Details stays");
}

fn thread_with(posts: Vec<ThreadPost>, status: TaskStatus) -> ViewState {
    let mut state = mock::view_state();
    for task in &mut state.tasks {
        if task.id == T_SAYSO {
            task.status = status;
        }
    }
    for thread in &mut state.threads {
        if thread.task_id == T_SAYSO {
            thread.posts = posts.clone();
            thread.status = status;
        }
    }
    state.activity.clear();
    consort_client::derive::refresh(&mut state);
    state
}

fn error_post(card: ErrorCardView) -> ThreadPost {
    ThreadPost {
        id: "pst_failed".into(),
        speaker: Speaker::System,
        time_label: "9:00 AM".into(),
        bodies: vec![PostBody::Notice {
            kind: "turn_failed".into(),
            text: "The turn failed.".into(),
            error: Some(Box::new(card)),
            draft: None,
            fallback: None,
            monid: None,
            visual: None,
        }],
        pending: false,
        source_message_id: None,
        refused: None,
    }
}

#[gpui::test]
fn the_task_thread_shows_the_same_card_and_try_again_retries_the_task(cx: &mut TestAppContext) {
    let c = error_card(
        &failed(TurnErrorKindView::WorkerRestarted, "Claude Lab"),
        Some(TryAgainTarget::Task {
            task_id: T_SAYSO.into(),
        }),
        false,
        NOW,
    );
    let (handle, app) = open(thread_with(vec![error_post(c)], TaskStatus::Failed), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_thread(T_SAYSO.into(), window, cx)));
    settle(cx, 200);
    let key = "err-pst_failed.0";
    let frame = bounds(cx, &format!("error-{key}"));
    let panel = bounds(cx, "thread-panel");
    assert!(panel.contains(&frame.center()), "the card is in the panel");
    assert!(frame.size.width <= panel.size.width);
    assert!(has(cx, &format!("error-action-{key}-TryAgain")));
    // The raw text opens here too.
    click(cx, &format!("error-details-{key}"));
    assert!(has(cx, &format!("error-raw-{key}")));
    // Try again retries the task: it leaves Failed, so the card turns quiet.
    click(cx, &format!("error-action-{key}-TryAgain"));
    settle(cx, 300);
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.state.task(T_SAYSO).map(|t| t.status),
            Some(TaskStatus::Working)
        );
    });
}

/// Gives the app the state as a state event from the client does.
fn announce(app: &Entity<ConsortApp>, state: ViewState, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.apply_state(std::sync::Arc::new(state), cx)
        })
    });
    settle(cx, 100);
}

fn refused_view() -> RefusedView {
    RefusedView {
        failure_id: "c-1".into(),
        reason: "The message is too long for the hub.".into(),
        action: RefusedView::ACTION.into(),
    }
}

fn refused_state() -> ViewState {
    let mut message = match user_message(1_000) {
        TimelineItem::Message(m) => m,
        _ => unreachable!(),
    };
    message.refused = Some(refused_view());
    let mut state = timeline(vec![TimelineItem::Message(message)]);
    state.send_failures = vec![SendFailureView {
        id: "c-1".into(),
        channel_id: mock::tutti_id().into(),
        task_id: None,
        channel_name: "Tutti".into(),
        body: "OK, go ahead and start the build.".into(),
        message: "The message is too long for the hub. Put your text back in the message box, then send it again.".into(),
        reason: "The message is too long for the hub.".into(),
        action: RefusedView::ACTION.into(),
        in_lane: true,
    }];
    state
}

#[gpui::test]
fn a_refused_message_stays_in_the_lane_of_the_user_with_its_reason(cx: &mut TestAppContext) {
    let state = refused_state();
    let (handle, app) = open(state.clone(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    announce(&app, state, cx);
    let bubble = bounds(cx, "bubble-m_you.p0");
    let note = bounds(cx, "refused-m_you.p0");
    let composer = bounds(cx, "composer");
    assert!(bubble.right() > composer.center().x, "the lane of the user");
    assert!(
        note.top() >= bubble.bottom(),
        "the reason is under the bubble"
    );
    assert!(near(note.right(), bubble.right()), "at the right edge");
    assert!(has(cx, "refused-put-back-m_you.p0"));
    // No toast: the lane shows it.
    app.read_with(cx, |app, _| {
        assert!(app.notices.visible().next().is_none(), "no notice");
    });
    assert!(!has(cx, "notice-stack"));
}

#[gpui::test]
fn put_back_in_the_message_box_puts_the_text_in_the_composer(cx: &mut TestAppContext) {
    let (handle, app) = open(refused_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    click(cx, "refused-put-back-m_you.p0");
    app.read_with(cx, |app, cx| {
        assert_eq!(
            app.inputs.composer.read(cx).value().to_string(),
            "OK, go ahead and start the build."
        );
    });
}

#[gpui::test]
fn a_failure_with_no_lane_still_shows_as_a_corner_notice(cx: &mut TestAppContext) {
    let mut state = refused_state();
    // The channel is gone: no lane shows the message.
    state.send_failures[0].in_lane = false;
    state.send_failures[0].channel_id = "chn_gone".into();
    let (handle, app) = open(state.clone(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    announce(&app, state, cx);
    app.read_with(cx, |app, _| {
        let shown: Vec<_> = app.notices.visible().collect();
        assert_eq!(shown.len(), 1, "one notice");
        assert_eq!(shown[0].failure.as_deref(), Some("c-1"));
    });
    assert!(has(cx, "notice-stack"));
}

/// A long message scrolls in the notice. Its button stays in the card.
#[gpui::test]
fn a_long_failure_keeps_its_button_inside_the_notice(cx: &mut TestAppContext) {
    let mut state = refused_state();
    state.send_failures[0].in_lane = false;
    state.send_failures[0].channel_id = "chn_gone".into();
    state.send_failures[0].body = "Look at the failing build and say why it fails. ".repeat(40);
    let (handle, app) = open(state.clone(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    announce(&app, state, cx);
    let serial = app.read_with(cx, |app, _| {
        app.notices.visible().next().expect("the notice").serial
    });
    let card = bounds(cx, &format!("notice-{serial}"));
    let row = bounds(cx, &format!("notice-action-row-{serial}"));
    assert!(row.size.height > px(0.), "the button shows");
    assert!(
        row.bottom() <= card.bottom(),
        "the button is inside the card: {row:?} in {card:?}"
    );
}

#[test]
fn a_failure_reason_shows_without_code_marks() {
    assert_eq!(
        crate::ui::panels::reason_text("No identity matches `Developer ID`. Install it."),
        "No identity matches Developer ID. Install it."
    );
}

#[gpui::test]
fn a_refused_reply_in_a_task_thread_stays_in_the_lane_of_the_user(cx: &mut TestAppContext) {
    let post = ThreadPost {
        id: "pst_refused".into(),
        speaker: Speaker::You,
        time_label: "9:00 AM".into(),
        bodies: vec![PostBody::Text("Try the other branch.".into())],
        pending: false,
        source_message_id: None,
        refused: Some(refused_view()),
    };
    let mut state = thread_with(vec![post], TaskStatus::Review);
    state.send_failures = refused_state().send_failures;
    state.send_failures[0].task_id = Some(T_SAYSO.into());
    let (handle, app) = open(state.clone(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_thread(T_SAYSO.into(), window, cx)));
    announce(&app, state, cx);
    let note = bounds(cx, "refused-pst_refused");
    assert!(maybe_bounds(cx, "refused-put-back-pst_refused").is_some());
    let panel = bounds(cx, "thread-panel");
    assert!(panel.contains(&note.center()));
    assert!(note.right() > panel.center().x, "at the right");
    assert!(!has(cx, "notice-stack"), "no toast");
    // The action puts the text in the reply box.
    click(cx, "refused-put-back-pst_refused");
    app.read_with(cx, |app, cx| {
        assert_eq!(
            app.inputs.reply.read(cx).value().to_string(),
            "OK, go ahead and start the build."
        );
    });
}

#[gpui::test]
fn a_card_of_a_failed_path_lists_the_steps_and_edit_path_opens_the_models_page(
    cx: &mut TestAppContext,
) {
    let body = r#"{"kind":"turn_failed","text":"x","error_kind":"rate_limit","account":"Claude Lab","path":[{"place":"first","account_label":"Claude Lab","model":"opus","model_name":"Opus 5.5","outcome":"failed","cause":"usage_limit"}]}"#;
    let path_failed = consort_client::turn_errors::parse_turn_failed(body).expect("a notice");
    let card = error_card(&path_failed, resend(), false, NOW);
    assert_eq!(card.title, "No model in the path could answer");
    let (handle, app) = open(with_card(card), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(crate::test_support::has(cx, "error-path-err-m_failed"));
    crate::test_support::click(cx, "error-action-err-m_failed-EditPath");
    app.read_with(cx, |app, _| {
        assert_eq!(app.settings_section, crate::app::SettingsSection::Models)
    });
}
