//! The hub is offline at start, in a headless window (Paper `ERQ-0`): the
//! pill under the mark, the queue hint, the dimmed answers and the sidebar.

use std::time::Duration;

use crate::app::ConsortApp;
use consort_client::{Client, Connection, ViewState};
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Entity, Modifiers, WindowOptions, point, px, size};

fn open(state: ViewState, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
    });
    let options = WindowOptions {
        window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(1440.), px(900.)),
        })),
        ..WindowOptions::default()
    };
    let (handle, app) = cx
        .update(|cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    ConsortApp::new(Client::mock_with(state).expect("mock"), true, window, cx)
                })
            })
        })
        .expect("open the window");
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    (handle, app)
}

fn settle(cx: &mut VisualTestContext, ms: u64) {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
        cx.run_until_parked();
    }
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn has(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

#[gpui::test]
fn the_offline_screen_shows_the_pill_the_queue_hint_and_no_runs_on(cx: &mut TestAppContext) {
    let (handle, _app) = open(consort_client::mock_offline::hub_offline(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(has(cx, "offline-pill"), "the pill under the mark");
    assert!(has(cx, "offline-try-again"), "Try again in the pill");
    assert!(has(cx, "queue-hint"), "messages wait here");
    assert!(
        !has(cx, "runs-on"),
        "nothing runs while the hub is down, so the composer does not say where"
    );
    // The line above the composer stays out: the pill says it.
    assert!(!has(cx, "link-try-again"));
    // Try again asks the client to connect. The screen stays whole.
    let bounds = cx.debug_bounds("offline-try-again").expect("Try again");
    cx.simulate_click(bounds.center(), Modifiers::default());
    settle(cx, 50);
    assert!(has(cx, "offline-pill"));
}

#[gpui::test]
fn an_online_screen_has_no_pill_and_says_where_it_runs(cx: &mut TestAppContext) {
    let (handle, _app) = open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert!(!has(cx, "offline-pill"));
    assert!(!has(cx, "queue-hint"));
    assert!(has(cx, "runs-on"));
}

#[gpui::test]
fn an_answer_does_not_go_out_while_the_hub_is_offline(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock_offline::hub_offline(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    let approval = app.read_with(cx, |a, _| {
        a.state
            .timeline(&a.state.tutti().expect("Tutti").id)
            .expect("timeline")
            .items
            .iter()
            .find_map(|item| match item {
                consort_client::TimelineItem::TaskCard(card) => {
                    card.approval.as_ref().map(|a| a.id.clone())
                }
                _ => None,
            })
            .expect("an approval waits")
    });
    assert_eq!(
        app.read_with(cx, |a, _| a.answers_blocked()),
        Some(consort_client::offline::WAITING_FOR_HUB)
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.answer_approval(
                approval.clone(),
                consort_client::ApprovalChoice::AllowOnce,
                window,
                cx,
            )
        })
    });
    settle(cx, 200);
    // The user sees why, and the approval still waits.
    let notice = app.read_with(cx, |a, _| {
        a.notices.visible().last().map(|n| n.message.clone())
    });
    assert_eq!(
        notice.as_deref(),
        Some(consort_client::offline::WAITING_FOR_HUB)
    );
    let still_waiting = app.read_with(cx, |a, _| {
        a.state
            .timeline(&a.state.tutti().expect("Tutti").id)
            .expect("timeline")
            .items
            .iter()
            .any(|item| {
                matches!(item, consort_client::TimelineItem::TaskCard(card)
                    if card.approval.as_ref().is_some_and(|a| a.id == approval
                        && a.state == consort_client::RequestState::Pending))
            })
    });
    assert!(still_waiting);
}

#[gpui::test]
fn the_answers_work_again_when_the_hub_is_back(cx: &mut TestAppContext) {
    let mut state = consort_client::mock_offline::hub_offline();
    state.connection = Connection::Demo;
    state.saved_data = None;
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    settle(cx, 100);
    assert_eq!(app.read_with(cx, |a, _| a.answers_blocked()), None);
}

#[test]
fn a_waiting_message_says_when_it_goes_out() {
    assert_eq!(
        crate::ui::timeline::pending_text(true),
        "Sends when the hub is back"
    );
    assert_eq!(crate::ui::timeline::pending_text(false), "Sending");
}
