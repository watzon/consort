//! The chat redesign in a headless window: the bubbles, the composer, the
//! top actions, the Three voices mark, Reduce motion and the icon shimmer.

use std::{sync::Arc, time::Duration};

use crate::app::{ConsortApp, Panel};
use crate::ui::{mark::MarkPhase, motion::FrameClock};
use consort_client::{
    Client, TaskStatus, ViewState,
    activity::{ActivityView, ChannelActivity, MarkState},
    chat::{ChatRow, Lane},
};
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Entity, Modifiers, WindowOptions, point, px, size};

/// One stop-motion frame: `motion.frame`.
const FRAME: Duration = Duration::from_millis(125);

fn open(state: ViewState, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    // The mock client runs its own thread.
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

fn mock() -> ViewState {
    consort_client::mock::view_state()
}

/// The mock with nothing that moves: no task works or needs input, Tutti
/// has no messages, and every assistant is idle.
fn quiet() -> ViewState {
    let mut state = mock();
    for task in &mut state.tasks {
        task.status = TaskStatus::Resolved;
    }
    let tutti = consort_client::mock::tutti_id();
    for timeline in &mut state.timelines {
        if timeline.channel_id == tutti {
            timeline.items.clear();
        }
        // No approval or decision waits.
        timeline.items.retain(|item| {
            !matches!(
                item,
                consort_client::TimelineItem::Approval(_)
                    | consort_client::TimelineItem::Decision(_)
            )
        });
    }
    state.activity.clear();
    consort_client::derive::refresh(&mut state);
    state
}

fn with_tutti(mut state: ViewState, mark: MarkState) -> ViewState {
    let tutti = consort_client::mock::tutti_id();
    state.activity.retain(|a| a.channel_id != tutti);
    state.activity.push(ChannelActivity {
        channel_id: tutti.into(),
        activity: ActivityView {
            state: mark,
            line: (mark == MarkState::Working).then(|| "Searching the web".into()),
        },
    });
    state
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

#[gpui::test]
fn tutti_draws_bubbles_in_two_lanes_with_time_separators(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    app.read_with(cx, |app, _| {
        let labels: Vec<&str> = app
            .chat_rows
            .iter()
            .filter_map(|r| match r {
                ChatRow::Separator { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(labels, ["Today 8:00 AM", "9:12 AM"]);
        let lanes: Vec<Lane> = app
            .chat_rows
            .iter()
            .filter_map(|r| match r {
                ChatRow::Bubble(b) => Some(b.lane),
                _ => None,
            })
            .collect();
        // The brief (title and text), your question, and the reply. The
        // reply has two paragraphs and is still one bubble.
        assert_eq!(
            lanes,
            [Lane::Assistant, Lane::Assistant, Lane::You, Lane::Assistant]
        );
        assert!(matches!(app.chat_rows.last(), Some(ChatRow::Card(_))));
    });
    // The reply sits at the left, your bubble at the right, and no bubble is
    // wider than the token. A message is one bubble (DECISIONS 731), so the
    // reply has no second bubble.
    let reply = cx
        .debug_bounds("bubble-msg_tutti_reply.p0")
        .expect("the reply draws");
    assert!(
        cx.debug_bounds("bubble-msg_tutti_reply.p1").is_none(),
        "two paragraphs are one bubble"
    );
    let yours = cx
        .debug_bounds("bubble-msg_tutti_you.p0")
        .expect("your bubble draws");
    assert!(yours.right() > reply.right(), "your lane is at the right");
    assert!(yours.origin.x > reply.origin.x);
    let max = app.read_with(cx, |_, cx| ConsortApp::tokens(cx).layout.bubble_max_width);
    assert!(reply.size.width <= max && yours.size.width <= max);
}

#[gpui::test]
fn the_composer_has_one_field_with_send_inside_and_tools_under_it(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.composer_placeholder(), "Message Tutti");
    });
    let field = cx.debug_bounds("composer").expect("the field");
    let send = cx.debug_bounds("send").expect("the send key");
    let tools = cx.debug_bounds("composer-tools").expect("the tools row");
    let runs_on = cx.debug_bounds("runs-on").expect("Runs on hub");
    assert!(field.contains(&send.center()), "send is inside the field");
    assert!(send.right() > field.center().x, "at the right");
    assert!(
        tools.origin.y >= field.bottom(),
        "the tools sit under the field"
    );
    assert!(
        runs_on.right() > tools.center().x,
        "Runs on sits at the right"
    );
    // Another channel: its name in the placeholder.
    let sayso = app.read_with(cx, |app, _| {
        app.state
            .channels
            .iter()
            .find(|c| c.name == "sayso")
            .map(|c| c.id.clone())
            .expect("sayso")
    });
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_channel(sayso, window, cx)));
    app.read_with(cx, |app, _| {
        assert_eq!(app.composer_placeholder(), "Message sayso")
    });
}

#[gpui::test]
fn the_tasks_button_opens_the_panel_and_has_a_gold_dot_when_something_needs_you(
    cx: &mut TestAppContext,
) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert!(
        cx.debug_bounds("tasks-needs-you").is_some(),
        "two tasks need you"
    );
    assert!(
        cx.debug_bounds("new-task").is_none(),
        "the Tasks panel has it"
    );
    let tasks = cx.debug_bounds("tasks-button").expect("the Tasks button");
    cx.simulate_click(tasks.center(), Modifiers::default());
    cx.run_until_parked();
    app.read_with(cx, |app, _| assert_eq!(app.panel, Panel::Tasks));
    let tasks = cx.debug_bounds("tasks-button").expect("the Tasks button");
    cx.simulate_click(tasks.center(), Modifiers::default());
    cx.run_until_parked();
    app.read_with(cx, |app, _| assert_eq!(app.panel, Panel::None));
}

#[gpui::test]
fn no_gold_dot_when_nothing_needs_you(cx: &mut TestAppContext) {
    let (handle, _app) = open(quiet(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert!(cx.debug_bounds("tasks-button").is_some());
    assert!(cx.debug_bounds("tasks-needs-you").is_none());
}

#[gpui::test]
fn the_mark_follows_the_assistant_and_holds_done_when_work_ends(cx: &mut TestAppContext) {
    let (handle, app) = open(with_tutti(quiet(), MarkState::Working), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.mark_phase(cx), MarkPhase::Working)
    });
    assert!(cx.debug_bounds("activity-pill").is_some(), "the pill shows");
    // Work ends: Done for 1.5 s, then idle.
    let idle = Arc::new(with_tutti(quiet(), MarkState::Idle));
    cx.update(|_, cx| app.update(cx, |app, cx| app.set_state(idle, cx)));
    draw(cx);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.mark_phase(cx), MarkPhase::Done)
    });
    assert!(
        cx.debug_bounds("activity-pill").is_none(),
        "no pill after work"
    );
    cx.executor().advance_clock(Duration::from_millis(1400));
    draw(cx);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.mark_phase(cx), MarkPhase::Done)
    });
    cx.executor().advance_clock(Duration::from_millis(200));
    draw(cx);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.mark_phase(cx), MarkPhase::Idle)
    });
    // Something needs you.
    let needs = Arc::new(with_tutti(quiet(), MarkState::NeedsYou));
    cx.update(|_, cx| app.update(cx, |app, cx| app.set_state(needs, cx)));
    draw(cx);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.mark_phase(cx), MarkPhase::NeedsYou)
    });
}

#[gpui::test]
fn the_mark_plays_at_eight_frames_a_second_and_stops_under_reduce_motion(cx: &mut TestAppContext) {
    let (handle, app) = open(with_tutti(quiet(), MarkState::Working), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| app.update(cx, |app, _| app.window_active = true));
    draw(cx);
    let tick = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.global::<FrameClock>().tick());
    let start = tick(cx);
    for _ in 0..8 {
        cx.executor().advance_clock(FRAME);
        cx.run_until_parked();
    }
    assert_eq!(tick(cx) - start, 8, "8 frames in one second");
    // Reduce motion: the clock stops and the mark holds frame 0.
    cx.update(|_, cx| cx.set_reduce_motion(true));
    draw(cx);
    cx.executor().advance_clock(FRAME);
    cx.run_until_parked();
    let held = tick(cx);
    for _ in 0..8 {
        cx.executor().advance_clock(FRAME);
        cx.run_until_parked();
    }
    assert_eq!(tick(cx), held, "nothing moves under Reduce motion");
    app.update(cx, |app, _| {
        assert_eq!(app.mark.frame(MarkPhase::Working, held + 5, true), 0)
    });
    // A state change still changes the pose.
    let needs = Arc::new(with_tutti(quiet(), MarkState::NeedsYou));
    cx.update(|_, cx| app.update(cx, |app, cx| app.set_state(needs, cx)));
    draw(cx);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.mark_phase(cx), MarkPhase::Done)
    });
    cx.update(|_, cx| cx.set_reduce_motion(false));
}

#[gpui::test]
fn a_bubble_with_a_list_takes_the_full_width(cx: &mut TestAppContext) {
    // A list cannot report its natural width, so its bubble must not shrink
    // to the width of a short heading and wrap the list word by word.
    let mut state = mock();
    let tutti = consort_client::mock::tutti_id();
    for timeline in &mut state.timelines {
        if timeline.channel_id != tutti {
            continue;
        }
        let Some(consort_client::TimelineItem::Message(reply)) = timeline
            .items
            .iter()
            .find(|i| i.key() == "msg_tutti_reply")
            .cloned()
        else {
            panic!("the mock has the reply");
        };
        let mut list = reply.clone();
        list.id = "msg_tutti_list".to_owned();
        list.body = "**Your accounts**\n\n- Gmail (your personal and work accounts): search, read, draft, reply, label.\n- Google Calendar".to_owned();
        let mut short = reply;
        short.id = "msg_tutti_short".to_owned();
        short.body = "Done.".to_owned();
        timeline.items.retain(|i| i.key() != "msg_tutti_reply");
        timeline
            .items
            .push(consort_client::TimelineItem::Message(list));
        timeline
            .items
            .push(consort_client::TimelineItem::Message(short));
    }
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    draw(cx);
    let max = app.read_with(cx, |_, cx| ConsortApp::tokens(cx).layout.bubble_max_width);
    let list = cx
        .debug_bounds("bubble-msg_tutti_list.p0")
        .expect("the list bubble draws");
    assert!(
        (list.size.width - max).abs() < px(1.),
        "the list bubble is {:?}, the bubble width is {max:?}",
        list.size.width
    );
    // A short plain message still fits its text.
    let short = cx
        .debug_bounds("bubble-msg_tutti_short.p0")
        .expect("the short bubble draws");
    assert!(short.size.width < max / 2., "{:?}", short.size.width);
}

/// The user often watches the work from another app.
#[gpui::test]
fn work_moves_in_a_window_without_the_focus_and_the_idle_mark_rests(cx: &mut TestAppContext) {
    let (handle, app) = open(with_tutti(quiet(), MarkState::Working), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| app.update(cx, |app, _| app.window_active = false));
    draw(cx);
    let tick = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.global::<FrameClock>().tick());
    let run = |cx: &mut VisualTestContext| {
        for _ in 0..8 {
            cx.executor().advance_clock(FRAME);
            cx.run_until_parked();
        }
    };
    let start = tick(cx);
    run(cx);
    assert!(tick(cx) > start, "the work moves");

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.state = with_tutti(quiet(), MarkState::Idle).into();
            app.mark_forced = Some(MarkPhase::Idle);
            cx.notify();
        })
    });
    draw(cx);
    run(cx);
    let rested = tick(cx);
    run(cx);
    assert_eq!(tick(cx), rested, "nothing runs at rest");
}

#[gpui::test]
fn the_idle_mark_shimmers_at_half_speed(cx: &mut TestAppContext) {
    let (handle, app) = open(quiet(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| app.update(cx, |app, _| app.window_active = true));
    draw(cx);
    app.read_with(cx, |app, cx| {
        assert_eq!(app.mark_phase(cx), MarkPhase::Idle)
    });
    let mut frames = Vec::new();
    for _ in 0..6 {
        // Two frame periods per idle frame: 4 a second.
        cx.executor().advance_clock(FRAME * 2);
        cx.run_until_parked();
        frames.push(cx.update(|_, cx| {
            let tick = cx.global::<FrameClock>().tick();
            app.update(cx, |app, _| app.mark.frame(MarkPhase::Idle, tick, false))
        }));
        let fast = cx.update(|_, cx| cx.global::<FrameClock>().fast());
        assert!(!fast, "only the idle mark moves, so the clock runs slow");
    }
    let mut seen = frames.clone();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen, [0, 1, 2], "the three edges show in turn: {frames:?}");
    // Under Reduce motion the idle mark holds still.
    cx.update(|_, cx| cx.set_reduce_motion(true));
    let tick = cx.update(|_, cx| cx.global::<FrameClock>().tick());
    app.update(cx, |app, _| {
        assert_eq!(app.mark.frame(MarkPhase::Idle, tick + 3, true), 0)
    });
    cx.update(|_, cx| cx.set_reduce_motion(false));
}

#[gpui::test]
fn an_icon_button_shimmers_only_while_hovered(cx: &mut TestAppContext) {
    let (handle, app) = open(quiet(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    // The idle mark shimmers too; hold it on its still Done frame, so only
    // the button can move here.
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.window_active = true;
            app.mark_forced = Some(MarkPhase::Done);
        })
    });
    draw(cx);
    let tick = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.global::<FrameClock>().tick());
    // Nothing moves: the clock stands still.
    let start = tick(cx);
    for _ in 0..4 {
        cx.executor().advance_clock(FRAME);
        cx.run_until_parked();
    }
    assert_eq!(tick(cx), start, "an idle screen costs nothing");
    // The pointer on the Channel button: three versions in turn, 8 a second.
    let button = cx
        .debug_bounds("channel-button")
        .expect("the Channel button");
    cx.simulate_mouse_move(button.center(), None, Modifiers::default());
    cx.run_until_parked();
    let hovered = cx.update(|_, cx| cx.global::<FrameClock>().hovered());
    assert_eq!(hovered, Some("channel-button".into()));
    let from = tick(cx);
    let mut versions = Vec::new();
    for _ in 0..3 {
        cx.executor().advance_clock(FRAME);
        cx.run_until_parked();
        versions.push(cx.update(|_, cx| {
            let id = gpui_kit::ElementId::from("channel-button");
            let live = crate::ui::motion::is_hovered(&id, cx);
            crate::ui::motion::shimmer_version(live, cx)
        }));
    }
    assert_eq!(tick(cx) - from, 3);
    versions.sort_unstable();
    assert_eq!(
        versions,
        [0, 1, 2],
        "each version shows once in three frames"
    );
    // The pointer leaves: the clock stops after the next frame.
    cx.simulate_mouse_move(point(px(700.), px(500.)), None, Modifiers::default());
    cx.run_until_parked();
    let hovered = cx.update(|_, cx| cx.global::<FrameClock>().hovered());
    assert_eq!(hovered, None, "the pointer left the button");
    cx.executor().advance_clock(FRAME);
    cx.run_until_parked();
    let left = tick(cx);
    for _ in 0..4 {
        cx.executor().advance_clock(FRAME);
        cx.run_until_parked();
    }
    assert_eq!(tick(cx), left);
    let version = cx.update(|_, cx| {
        let id = gpui_kit::ElementId::from("channel-button");
        crate::ui::motion::shimmer_version(crate::ui::motion::is_hovered(&id, cx), cx)
    });
    assert_eq!(version, 0, "an icon that is not hovered shows version 0");
}

#[gpui::test]
fn reduce_motion_stops_the_shimmer_of_a_hovered_button(cx: &mut TestAppContext) {
    let (handle, app) = open(quiet(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| {
        cx.set_reduce_motion(true);
        app.update(cx, |app, _| app.window_active = true);
    });
    draw(cx);
    let button = cx
        .debug_bounds("channel-button")
        .expect("the Channel button");
    cx.simulate_mouse_move(button.center(), None, Modifiers::default());
    cx.run_until_parked();
    let start = cx.update(|_, cx| cx.global::<FrameClock>().tick());
    for _ in 0..4 {
        cx.executor().advance_clock(FRAME);
        cx.run_until_parked();
    }
    let (tick, version) = cx.update(|_, cx| {
        let id = gpui_kit::ElementId::from("channel-button");
        (
            cx.global::<FrameClock>().tick(),
            crate::ui::motion::shimmer_version(crate::ui::motion::is_hovered(&id, cx), cx),
        )
    });
    assert_eq!(tick, start);
    assert_eq!(version, 0);
    cx.update(|_, cx| cx.set_reduce_motion(false));
}

/// The top of the chat is a progressive tint, not a solid zone: layers from
/// the canvas at about 94 percent to clear. The mark, the pill and the top
/// actions sit on the tint. At rest the first item starts below the pill.
#[gpui::test]
fn the_top_tint_lies_under_the_mark_and_the_first_bubble_starts_below_the_pill(
    cx: &mut TestAppContext,
) {
    for mark in [MarkState::Working, MarkState::Idle] {
        // Tutti with one short message, so the chat is at rest at the top.
        let mut state = with_tutti(mock(), mark);
        let tutti = consort_client::mock::tutti_id();
        for timeline in &mut state.timelines {
            if timeline.channel_id == tutti {
                timeline.items.retain(|i| i.key() == "msg_tutti_reply");
            }
        }
        let (handle, app) = open(state, cx);
        let cx = &mut VisualTestContext::from_window(handle, cx);
        let (content_top, stops) = app.read_with(cx, |_, cx| {
            let layout = ConsortApp::tokens(cx).layout;
            (layout.content_top, layout.top_tint)
        });
        let first = app.read_with(cx, |app, _| {
            let items = &app.state.timeline(&app.channel_id).expect("Tutti").items;
            app.chat_rows
                .iter()
                .find_map(|row| match row {
                    ChatRow::Bubble(b) => {
                        Some(format!("bubble-{}.p{}", items[b.item].key(), b.part))
                    }
                    _ => None,
                })
                .expect("a bubble")
        });
        let first: &'static str = Box::leak(first.into_boxed_str());
        draw(cx);
        let bubble = cx
            .debug_bounds(first)
            .unwrap_or_else(|| panic!("{first} draws"));
        let voices = cx.debug_bounds("three-voices").expect("the mark");
        // No solid zone any more.
        assert!(cx.debug_bounds("fade-top-solid").is_none());
        // One layer between each pair of stops, end to end, from the top of
        // the timeline to the clear stop.
        let layers: Vec<_> = (0..stops.len() - 1)
            .map(|ix| {
                let name: &'static str = Box::leak(format!("top-tint-{ix}").into_boxed_str());
                cx.debug_bounds(name)
                    .unwrap_or_else(|| panic!("{name} draws"))
            })
            .collect();
        let top = layers[0].origin.y;
        for (ix, layer) in layers.iter().enumerate() {
            assert_eq!(layer.origin.y - top, stops[ix].at, "layer {ix} starts");
            assert_eq!(layer.size.height, stops[ix + 1].at - stops[ix].at);
        }
        let tint_bottom = layers[layers.len() - 1].bottom();
        assert_eq!(tint_bottom - top, px(108.));
        // The mark and the top actions sit on the tint.
        assert!(voices.bottom() < tint_bottom, "the mark");
        let tasks = cx.debug_bounds("tasks-button").expect("the Tasks button");
        assert!(tasks.bottom() < tint_bottom, "the top actions");
        if mark == MarkState::Working {
            let pill = cx.debug_bounds("activity-pill").expect("the pill shows");
            assert!(pill.bottom() < tint_bottom, "the pill sits on the tint");
            assert!(
                bubble.origin.y >= pill.bottom(),
                "the first bubble ({:?}) starts at or below the pill ({:?})",
                bubble.origin.y,
                pill.bottom()
            );
        } else {
            assert!(cx.debug_bounds("activity-pill").is_none());
        }
        // The same place with and without the pill, so the chat never
        // jumps.
        assert!(
            bubble.origin.y >= top + content_top - px(1.),
            "{:?} < {:?}",
            bubble.origin.y,
            top + content_top
        );
    }
}
