//! The task line and the task thread in a headless window: a task that runs
//! is one line in the chat, Resolve is in the thread, the plan is a
//! checklist card under the newest message, and a message that the assistant
//! passed on to the task has "Show in channel" under it.

use crate::app::ConsortApp;
use crate::test_support::{bounds, click, has, open, settle};
use consort_client::{
    ApprovalView, PostBody, RequestState, Speaker, TaskCardView, TaskStatus, ThreadPost,
    TimelineItem, ViewState,
    mock::{self, T_SAYSO},
    plan::{PlanStepState, PlanStepView, PlanView},
};
use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext};
use gpui_kit::{Entity, px};

fn contains(outer: Bounds<Pixels>, inner: Bounds<Pixels>) -> bool {
    inner.left() >= outer.left() - px(0.5)
        && inner.right() <= outer.right() + px(0.5)
        && inner.top() >= outer.top() - px(0.5)
        && inner.bottom() <= outer.bottom() + px(0.5)
}

fn task_card(message_id: &str, at: i64, status: TaskStatus) -> TimelineItem {
    TimelineItem::TaskCard(TaskCardView {
        message_id: message_id.into(),
        task_id: T_SAYSO.into(),
        time_label: "9:00 AM".into(),
        created_at: at,
        title: "List what your home warranty covers".into(),
        status,
        latest: Some("I read two files so far.".into()),
        summary: String::new(),
        ..TaskCardView::default()
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

/// The mock with one card of the task in the Tutti timeline, and the task
/// in this status. The card takes the status of its task.
fn state_with_task(message_id: &str, status: TaskStatus) -> ViewState {
    let mut state = timeline(vec![task_card(message_id, 1_000, TaskStatus::Working)]);
    for task in &mut state.tasks {
        if task.id == T_SAYSO {
            task.status = status;
        }
    }
    for thread in &mut state.threads {
        if thread.task_id == T_SAYSO {
            thread.status = status;
        }
    }
    consort_client::derive::refresh(&mut state);
    state
}

#[gpui::test]
fn a_task_that_runs_is_one_line_in_the_chat(cx: &mut TestAppContext) {
    let (handle, _app) = open(state_with_task("m_working", TaskStatus::Working), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(has(cx, "task-line-m_working"));
    assert!(
        !has(cx, "card-m_working"),
        "a running task has no card body"
    );
    // The line is one line of text high.
    let line = bounds(cx, "task-line-m_working");
    assert!(line.size.height <= px(24.0), "{:?}", line.size.height);
}

#[gpui::test]
fn a_task_in_review_is_one_line_and_has_no_resolve_in_the_chat(cx: &mut TestAppContext) {
    let (handle, _app) = open(state_with_task("m_review", TaskStatus::Review), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(has(cx, "task-line-m_review"));
    assert!(
        !has(cx, "card-m_review"),
        "a task in review has no card body"
    );
    assert!(!has(cx, "resolve-m_review"), "Resolve is in the thread");
}

#[gpui::test]
fn a_failed_task_keeps_the_reason_in_its_card(cx: &mut TestAppContext) {
    let (handle, _app) = open(state_with_task("m_failed", TaskStatus::Failed), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(has(cx, "task-line-m_failed"));
    assert!(
        has(cx, "card-m_failed"),
        "the reason of a failed task stays"
    );
}

#[gpui::test]
fn a_task_with_an_approval_that_waits_keeps_its_card(cx: &mut TestAppContext) {
    let mut card = TaskCardView {
        message_id: "m_wait".into(),
        task_id: T_SAYSO.into(),
        created_at: 1_000,
        title: "Fix the crash".into(),
        status: TaskStatus::NeedsInput,
        ..TaskCardView::default()
    };
    card.approval = Some(ApprovalView {
        id: "apr_wait".into(),
        summary: "Run the tests".into(),
        detail: Some("cargo test".into()),
        state: RequestState::Pending,
        choices: vec![consort_client::ApprovalChoice::AllowOnce],
        ..ApprovalView::default()
    });
    let (handle, _app) = open(timeline(vec![TimelineItem::TaskCard(card)]), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(has(cx, "card-m_wait"), "the approval is in the chat card");
}

#[gpui::test]
fn the_thread_has_resolve_for_a_task_in_review(cx: &mut TestAppContext) {
    let (handle, app) = open(state_with_task("m_review", TaskStatus::Review), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    // The line opens the thread.
    click(cx, "task-line-m_review");
    app.read_with(cx, |app, _| {
        assert_eq!(app.panel, crate::app::Panel::Thread(T_SAYSO.into()));
    });
    assert!(has(cx, "review-action"), "Resolve is in the thread");
}

#[gpui::test]
fn the_task_line_uses_the_accent_only_for_the_two_waiting_statuses(cx: &mut TestAppContext) {
    let (_handle, _app) = open(mock::view_state(), cx);
    cx.update(|cx| {
        let t = ConsortApp::tokens(cx);
        let color = |s| crate::ui::timeline::task_line_color(s, &t);
        assert_eq!(color(TaskStatus::Working), t.colors.text_tertiary);
        assert_eq!(color(TaskStatus::NeedsInput), t.colors.accent_text);
        assert_eq!(color(TaskStatus::Review), t.colors.accent_text);
        assert_ne!(color(TaskStatus::Working), t.colors.accent_text);
    });
}

fn plan(states: &[(&str, PlanStepState)]) -> PlanView {
    PlanView {
        steps: states
            .iter()
            .map(|(text, state)| PlanStepView {
                text: (*text).into(),
                state: *state,
            })
            .collect(),
    }
}

fn with_plan(state: &mut ViewState, plan: Option<PlanView>) {
    for thread in &mut state.threads {
        if thread.task_id == T_SAYSO {
            thread.plan = plan.clone();
        }
    }
}

fn open_thread(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_thread(T_SAYSO.into(), window, cx)));
    settle(cx, 200);
}

#[gpui::test]
fn the_plan_is_a_checklist_card_under_the_newest_message(cx: &mut TestAppContext) {
    let mut state = mock::view_state();
    with_plan(
        &mut state,
        Some(plan(&[
            ("Find the warranty documents", PlanStepState::Done),
            ("Read the plan and the contract", PlanStepState::Current),
            ("Write the summary as a document", PlanStepState::Next),
        ])),
    );
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    let panel = bounds(cx, "thread-panel");
    let card = bounds(cx, "thread-plan");
    assert!(contains(panel, card), "the card is inside the panel");
    let rows: Vec<_> = (0..3)
        .map(|i| bounds(cx, &format!("plan-step-row-{i}")))
        .collect();
    for row in &rows {
        assert!(contains(card, *row), "a step is inside the card");
    }
    assert!(rows[0].bottom() <= rows[1].top() && rows[1].bottom() <= rows[2].top());
    // The plan is the row under the posts, and the composer is under it.
    // (The mock task is not Working, so no writing bubble follows it.)
    assert!(card.top() > bounds(cx, "thread-panel").top());
    assert!(card.bottom() <= bounds(cx, "reply-box").top());
    app.read_with(cx, |app, _| {
        assert!(
            app.thread_keys
                .last()
                .is_some_and(|k| k.starts_with("plan-")),
            "{:?}",
            app.thread_keys
        );
        // The plan is not a post.
        assert!(
            app.thread_keys
                .iter()
                .filter(|k| k.starts_with("plan-"))
                .count()
                == 1
        );
    });
}

#[gpui::test]
fn a_task_with_no_plan_has_no_card_and_a_new_step_state_makes_a_new_row_key(
    cx: &mut TestAppContext,
) {
    let (handle, app) = open(mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    assert!(!has(cx, "thread-plan"));
    let before = app.read_with(cx, |app, _| app.thread_keys.clone());
    assert!(!before.iter().any(|k| k.starts_with("plan-")));
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let mut state = (*app.state).clone();
            with_plan(
                &mut state,
                Some(plan(&[
                    ("One", PlanStepState::Current),
                    ("Two", PlanStepState::Next),
                ])),
            );
            app.state = state.into();
            app.sync_lists();
            cx.notify();
        })
    });
    settle(cx, 100);
    assert!(has(cx, "thread-plan"));
    let first = app.read_with(cx, |app, _| app.thread_keys.last().cloned());
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let mut state = (*app.state).clone();
            with_plan(
                &mut state,
                Some(plan(&[
                    ("One", PlanStepState::Done),
                    ("Two", PlanStepState::Current),
                ])),
            );
            app.state = state.into();
            app.sync_lists();
            cx.notify();
        })
    });
    let second = app.read_with(cx, |app, _| app.thread_keys.last().cloned());
    assert_ne!(first, second, "a step that moved measures the card again");
}

fn with_thread_status(state: &mut ViewState, status: TaskStatus) {
    for task in &mut state.tasks {
        if task.id == T_SAYSO {
            task.status = status;
        }
    }
    for thread in &mut state.threads {
        if thread.task_id == T_SAYSO {
            thread.status = status;
        }
    }
}

#[gpui::test]
fn the_writing_bubble_is_under_the_plan_while_the_task_works(cx: &mut TestAppContext) {
    let mut state = mock::view_state();
    with_thread_status(&mut state, TaskStatus::Working);
    with_plan(
        &mut state,
        Some(plan(&[
            ("Find the warranty documents", PlanStepState::Done),
            ("Read the plan and the contract", PlanStepState::Current),
        ])),
    );
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    let panel = bounds(cx, "thread-panel");
    let card = bounds(cx, "thread-plan");
    let bubble = bounds(cx, "writing-bubble");
    assert!(contains(panel, bubble), "the bubble is inside the panel");
    assert!(
        bubble.top() >= card.bottom(),
        "the bubble is under the plan card"
    );
    assert!(bubble.bottom() <= bounds(cx, "reply-box").top());
    // The three dabs of the timeline's bubble.
    for i in 0..3 {
        assert!(has(cx, &format!("writing-dab-{i}")), "dab {i}");
    }
    app.read_with(cx, |app, _| {
        let keys = &app.thread_keys;
        assert!(keys.last().is_some_and(|k| k.starts_with("writing-")));
        assert!(keys[keys.len() - 2].starts_with("plan-"), "{keys:?}");
    });
}

#[gpui::test]
fn a_task_that_does_not_work_has_no_writing_bubble_and_the_bubble_ends_with_the_turn(
    cx: &mut TestAppContext,
) {
    let mut state = mock::view_state();
    with_thread_status(&mut state, TaskStatus::Review);
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    assert!(!has(cx, "writing-bubble"), "a task in review");
    // The task starts to work: the bubble comes.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let mut state = (*app.state).clone();
            with_thread_status(&mut state, TaskStatus::Working);
            app.state = state.into();
            app.sync_lists();
            cx.notify();
        })
    });
    settle(cx, 100);
    assert!(has(cx, "writing-bubble"), "a task that works");
    // The turn ends: the bubble goes.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let mut state = (*app.state).clone();
            with_thread_status(&mut state, TaskStatus::NeedsInput);
            app.state = state.into();
            app.sync_lists();
            cx.notify();
        })
    });
    settle(cx, 100);
    assert!(!has(cx, "writing-bubble"), "the turn ended");
}

fn user_post(id: &str, source: Option<&str>) -> ThreadPost {
    ThreadPost {
        id: id.into(),
        speaker: Speaker::You,
        time_label: "9:00 AM".into(),
        bodies: vec![PostBody::Text(
            "It is through iBex. I also want to know if it covers a replacement water heater."
                .into(),
        )],
        pending: false,
        source_message_id: source.map(str::to_owned),
        refused: None,
    }
}

#[gpui::test]
fn show_in_channel_is_under_a_user_post_that_came_from_the_chat(cx: &mut TestAppContext) {
    let mut state = mock::view_state();
    for thread in &mut state.threads {
        if thread.task_id == T_SAYSO {
            thread.posts = vec![
                user_post("pst_from_chat", Some("msg_from_chat")),
                user_post("pst_typed_here", None),
            ];
        }
    }
    let channel = state
        .tasks
        .iter()
        .find(|t| t.id == T_SAYSO)
        .map(|t| t.channel_id.clone())
        .expect("the task");
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_thread(&app, cx);
    assert!(has(cx, "show-in-channel-pst_from_chat"));
    assert!(
        !has(cx, "show-in-channel-pst_typed_here"),
        "a post that the user typed in the thread has no link"
    );
    let panel = bounds(cx, "thread-panel");
    let link = bounds(cx, "show-in-channel-pst_from_chat");
    assert!(contains(panel, link), "the link is inside the panel");
    // It sits at the right, in the lane of the user.
    assert!(link.center().x > panel.center().x);
    // Click: the channel opens and the message is the one to show.
    click(cx, "show-in-channel-pst_from_chat");
    app.read_with(cx, |app, _| {
        assert_eq!(app.channel_id, channel);
        assert_eq!(app.convo.scroll_to.as_deref(), Some("msg_from_chat"));
    });
}
