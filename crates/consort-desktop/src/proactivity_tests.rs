//! Phase 4b in a headless window: Settings > Proactivity, the quick-reply
//! chips (send and type), the writing bubble, the marks and the feedback
//! actions, the offer card, and the Follow-ups section of the Routines tab.

use std::time::Duration;

use crate::app::{ChannelTab, ConsortApp, Page, SettingsSection};
use consort_client::mock_proactivity::{
    CHECK_IN, CHECK_IN_MESSAGE, FOLLOW_UP_QUOTE, FOLLOW_UP_SWITCH, HOMELAB, LIGHTS, READING, TUTTI,
};
use consort_client::proactivity::{ProactiveRating, ProactivityLevel, SnoozeOption};
use consort_client::{Client, TimelineItem, ViewState};
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{
    AppContext as _, Entity, Focusable as _, Modifiers, WindowOptions, point, px, size,
};

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

fn state(screen: &str) -> ViewState {
    consort_client::mock_proactivity::screen_state(screen).expect("the phase 4b mock")
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn settle(cx: &mut VisualTestContext, ms: u64) {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
        cx.run_until_parked();
    }
    draw(cx);
}

fn until(
    cx: &mut VisualTestContext,
    app: &Entity<ConsortApp>,
    what: &str,
    ready: impl Fn(&ConsortApp) -> bool,
) {
    let end = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if app.read_with(cx, |app, _| ready(app)) {
            return;
        }
        assert!(std::time::Instant::now() < end, "timed out: {what}");
        settle(cx, 50);
    }
}

fn has(cx: &mut VisualTestContext, selector: &str) -> bool {
    let name: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    cx.debug_bounds(name).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let name: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    let at = cx
        .debug_bounds(name)
        .unwrap_or_else(|| panic!("{selector} draws"))
        .center();
    cx.simulate_click(at, Modifiers::default());
    draw(cx);
}

fn screen(app: &Entity<ConsortApp>, name: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_screen(name, window, cx)));
    draw(cx);
}

/// Settings > Proactivity: Pause all, snooze, a level, and Cancel all
/// follow-ups change the state, and the page draws its parts.
#[gpui::test]
fn the_settings_page_pauses_snoozes_sets_a_level_and_cancels_follow_ups(cx: &mut TestAppContext) {
    let (handle, app) = open(state("proactivity"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "proactivity", cx);
    assert!(app.read_with(cx, |app, _| app.settings_section
        == SettingsSection::Proactivity));
    assert!(has(cx, "proactivity-pause"));
    assert!(has(cx, "settings-Proactivity"));

    click(cx, "proactivity-pause");
    until(cx, &app, "paused", |app| app.state.proactivity.paused);

    cx.update(|_, cx| app.update(cx, |app, cx| app.snooze_proactivity(SnoozeOption::Week, cx)));
    until(cx, &app, "snoozed for a week", |app| {
        app.state.proactivity.snooze == SnoozeOption::Week
    });

    // A pick in the level select of homelab (row 3).
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let option = crate::proactivity::level_option(HOMELAB, Some(ProactivityLevel::Normal));
            assert!(app.pick_proactivity_select(crate::proactivity::LEVEL_SELECTS[3], &option, cx));
        })
    });
    until(cx, &app, "homelab is Normal", |app| {
        app.state
            .proactivity
            .channels
            .iter()
            .any(|c| c.channel_id == HOMELAB && c.effective == Some(ProactivityLevel::Normal))
    });

    // The log filters change what the log shows.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            assert!(app.pick_proactivity_select(
                crate::proactivity::LOG_CHANNEL_SELECT,
                LIGHTS,
                cx
            ));
        })
    });
    assert!(
        app.read_with(cx, |app, _| app.proactivity.log_channel.as_deref()
            == Some(LIGHTS))
    );

    cx.update(|_, cx| app.update(cx, |app, cx| app.cancel_all_follow_ups(None, cx)));
    until(cx, &app, "no follow-up waits", |app| {
        app.state.proactivity.follow_ups.is_empty()
    });
}

/// A send chip sends its label as your message, and then the chips go.
#[gpui::test]
fn a_send_chip_sends_its_label_and_the_chips_go(cx: &mut TestAppContext) {
    let (handle, app) = open(state("channel-greeting"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "channel-greeting", cx);
    assert!(has(cx, "chip-0") && has(cx, "chip-2"));
    click(cx, "chip-0");
    until(cx, &app, "Yes is sent", |app| {
        app.state.timeline(READING).is_some_and(|t| {
            t.items.iter().any(|i| {
                matches!(i, TimelineItem::Message(m) if m.body == "Yes" && m.speaker == consort_client::Speaker::You)
            })
        })
    });
    // Your reply hides the chips.
    settle(cx, 100);
    assert!(!has(cx, "chip-0"));
}

/// "I will type it" sends nothing and moves the focus to the composer.
#[gpui::test]
fn the_type_chip_only_focuses_the_composer(cx: &mut TestAppContext) {
    let (handle, app) = open(state("channel-question"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "channel-question", cx);
    let before = app.read_with(cx, |app, _| {
        app.state.timeline(READING).map_or(0, |t| t.items.len())
    });
    assert!(has(cx, "chip-3"));
    click(cx, "chip-3");
    settle(cx, 100);
    let after = app.read_with(cx, |app, _| {
        app.state.timeline(READING).map_or(0, |t| t.items.len())
    });
    assert_eq!(before, after, "nothing was sent");
    let focused = cx.update(|window, cx| {
        app.read(cx)
            .inputs
            .composer
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    });
    assert!(focused, "the composer has the focus");
}

/// The writing bubble shows while a reply is owed, and not after it came.
#[gpui::test]
fn the_writing_bubble_shows_while_the_reply_is_owed(cx: &mut TestAppContext) {
    let (handle, app) = open(state("channel-writing"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "channel-writing", cx);
    assert!(has(cx, "writing-bubble"));
    screen(&app, "channel-question", cx);
    let (handle2, app2) = open(state("channel-question"), cx);
    let cx2 = &mut VisualTestContext::from_window(handle2, cx);
    screen(&app2, "channel-question", cx2);
    assert!(!has(cx2, "writing-bubble"));
}

/// The dabs of the writing bubble rise in turn on the clock. The bubble keeps
/// its size, and under Reduce motion the dabs stay still.
#[gpui::test]
fn the_writing_bubble_moves_in_a_wave_and_keeps_its_size(cx: &mut TestAppContext) {
    let (handle, app) = open(state("channel-writing"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "channel-writing", cx);
    cx.update(|_, cx| app.update(cx, |app, _| app.window_active = true));
    draw(cx);
    let tops = |cx: &mut VisualTestContext| {
        ["writing-dab-0", "writing-dab-1", "writing-dab-2"]
            .map(|dab| cx.debug_bounds(dab).expect("the dab draws").origin.y)
    };
    let bubble = cx.debug_bounds("writing-bubble").expect("the bubble");
    let mut seen = vec![tops(cx)];
    for _ in 0..8 {
        cx.executor().advance_clock(Duration::from_millis(125));
        cx.run_until_parked();
        draw(cx);
        seen.push(tops(cx));
        assert_eq!(cx.debug_bounds("writing-bubble"), Some(bubble));
    }
    for dab in 0..3 {
        assert!(
            seen.iter().any(|tops| tops[dab] != seen[0][dab]),
            "dab {dab} moves"
        );
    }
    assert_ne!(seen[2], seen[4], "the dabs do not rise together");

    cx.update(|_, cx| cx.set_reduce_motion(true));
    draw(cx);
    let held = tops(cx);
    for _ in 0..8 {
        cx.executor().advance_clock(Duration::from_millis(125));
        cx.run_until_parked();
        draw(cx);
        assert_eq!(tops(cx), held, "nothing moves under Reduce motion");
    }
    cx.update(|_, cx| cx.set_reduce_motion(false));
}

/// A proactive message has its mark, Useful and Too noisy on hover, and a
/// click records the rating. The offer card draws with its three buttons.
#[gpui::test]
fn proactive_messages_show_their_marks_feedback_and_the_offer(cx: &mut TestAppContext) {
    let (handle, app) = open(state("proactive-messages"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "proactive-messages", cx);
    assert!(app.read_with(cx, |app, _| app.channel_id == TUTTI));
    // The offer is the first card; the list opens at its end.
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.timeline.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            })
        })
    });
    draw(cx);
    assert!(has(cx, "offer-card"));
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            // The footer of the check-in.
            let items = app
                .state
                .timeline(TUTTI)
                .map(|t| t.items.clone())
                .unwrap_or_default();
            let row = app
                .chat_rows
                .iter()
                .position(|r| {
                    matches!(r, consort_client::chat::ChatRow::Actions { item, .. }
                        if matches!(items.get(*item), Some(TimelineItem::Message(m)) if m.id == CHECK_IN_MESSAGE))
                })
                .expect("the check-in row");
            app.timeline.scroll_to_reveal_item(row)
        })
    });
    draw(cx);
    assert!(has(cx, "feedback-useful") && has(cx, "feedback-too_noisy"));
    click(cx, "feedback-too_noisy");
    until(cx, &app, "the rating is kept", |app| {
        app.state.timeline(TUTTI).is_some_and(|t| {
            t.items.iter().any(|i| {
                matches!(i, TimelineItem::Message(m) if m.id == CHECK_IN_MESSAGE
                    && m.proactive.as_ref().is_some_and(|p| p.rating == Some(ProactiveRating::TooNoisy)))
            })
        })
    });
    // The choice stays as one chip, and a click on it clears the choice.
    draw(cx);
    assert!(has(cx, "feedback-chip") && !has(cx, "feedback-pair"));
    click(cx, "feedback-chip");
    until(cx, &app, "the rating is gone", |app| {
        app.state.timeline(TUTTI).is_some_and(|t| {
            t.items.iter().any(|i| {
                matches!(i, TimelineItem::Message(m) if m.id == CHECK_IN_MESSAGE
                    && m.proactive.as_ref().is_some_and(|p| p.rating.is_none()))
            })
        })
    });
}

/// The Routines tab shows the check-in row with Open note, and the
/// Follow-ups section, where Cancel deletes one follow-up.
#[gpui::test]
fn the_routines_tab_shows_the_check_in_and_cancels_a_follow_up(cx: &mut TestAppContext) {
    let (handle, app) = open(state("routines-follow-ups"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "routines-follow-ups", cx);
    assert!(app.read_with(cx, |app, _| app.channel_id == LIGHTS));
    cx.update(|_, cx| app.update(cx, |app, cx| app.open_channel_tab(ChannelTab::Routines, cx)));
    draw(cx);
    assert!(has(cx, &format!("routine-row-{CHECK_IN}")));
    assert!(has(cx, &format!("routine-note-{CHECK_IN}")));
    assert!(has(cx, &format!("follow-up-{FOLLOW_UP_QUOTE}")));
    assert!(has(cx, &format!("follow-up-{FOLLOW_UP_SWITCH}")));
    // The check-in menu has no Edit.
    let check_in = app.read_with(cx, |app, _| {
        app.state
            .routines
            .iter()
            .find(|r| r.id == CHECK_IN)
            .cloned()
            .expect("the check-in")
    });
    let labels: Vec<String> = crate::knowledge::routine_menu(&check_in)
        .iter()
        .map(|i| i.label.to_string())
        .collect();
    assert!(!labels.contains(&"Edit".to_owned()), "{labels:?}");
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.cancel_follow_up(FOLLOW_UP_QUOTE.into(), cx)
        })
    });
    until(cx, &app, "the follow-up is gone", |app| {
        app.state.routines.iter().all(|r| r.id != FOLLOW_UP_QUOTE)
    });
}

/// The channel settings dialog has the Proactivity row after Model and
/// effort. A pick waits in the form until Save changes sends it with the
/// other fields.
#[gpui::test]
fn the_channel_settings_dialog_saves_the_level_with_save_changes(cx: &mut TestAppContext) {
    use crate::app::Dialog;
    use crate::proactivity::{CHANNEL_SETTINGS_LEVEL_SELECT, level_option};
    let (handle, app) = open(state("proactivity"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    // The mock gives homelab its own level: Off.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(Dialog::ChannelSettings(HOMELAB.into()), window, cx)
        })
    });
    draw(cx);
    assert!(has(cx, CHANNEL_SETTINGS_LEVEL_SELECT));
    app.read_with(cx, |app, cx| {
        assert_eq!(app.convo.settings.proactivity, Some(ProactivityLevel::Off));
        let (_, changes) = app.settings_changes(cx).expect("the form");
        assert_eq!(changes, consort_client::ChannelChanges::default());
    });

    // A pick changes the form and sends nothing yet.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let option = level_option(HOMELAB, Some(ProactivityLevel::Eager));
            app.pick_select(CHANNEL_SETTINGS_LEVEL_SELECT, &option, window, cx);
        })
    });
    draw(cx);
    app.read_with(cx, |app, cx| {
        assert_eq!(
            app.convo.settings.proactivity,
            Some(ProactivityLevel::Eager)
        );
        let own = |app: &ConsortApp| {
            app.state
                .proactivity
                .channels
                .iter()
                .find(|c| c.channel_id == HOMELAB)
                .and_then(|c| c.level)
        };
        assert_eq!(own(app), Some(ProactivityLevel::Off), "not sent yet");
        let (id, changes) = app.settings_changes(cx).expect("the form");
        assert_eq!(id, HOMELAB);
        assert_eq!(
            changes,
            consort_client::ChannelChanges {
                proactivity: Some(Some(ProactivityLevel::Eager)),
                ..Default::default()
            }
        );
    });

    // Save changes sends it.
    click(cx, "dialog-confirm");
    until(cx, &app, "homelab is Eager", |app| {
        app.state
            .proactivity
            .channels
            .iter()
            .any(|c| c.channel_id == HOMELAB && c.level == Some(ProactivityLevel::Eager))
    });
    app.read_with(cx, |app, _| assert!(app.dialog.is_none()));

    // "Default for this channel" goes back to the default, also with Save
    // changes.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(Dialog::ChannelSettings(HOMELAB.into()), window, cx);
            let option = level_option(HOMELAB, None);
            app.pick_select(CHANNEL_SETTINGS_LEVEL_SELECT, &option, window, cx);
        })
    });
    draw(cx);
    click(cx, "dialog-confirm");
    until(cx, &app, "homelab is on its default", |app| {
        app.state
            .proactivity
            .channels
            .iter()
            .any(|c| c.channel_id == HOMELAB && c.level.is_none() && c.is_default)
    });
}

/// "Show in channel" in the log opens the channel and scrolls to the message,
/// as a search result does. The channel has many newer messages, so the
/// message is far from the end where a channel opens.
#[gpui::test]
fn open_message_in_the_log_scrolls_to_the_message(cx: &mut TestAppContext) {
    use consort_client::view::{MessageView, Speaker};
    let mut state = state("proactivity");
    let timeline = state
        .timelines
        .iter_mut()
        .find(|t| t.channel_id == TUTTI)
        .expect("the Tutti timeline");
    for n in 0..60 {
        timeline.items.push(TimelineItem::Message(MessageView {
            id: format!("msg_filler_{n}"),
            channel_id: TUTTI.into(),
            speaker: Speaker::You,
            body: format!("A newer message, number {n}."),
            created_at: 1_793_300_000_000 + n,
            ..MessageView::default()
        }));
    }
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let shown = |cx: &mut VisualTestContext| {
        (0..4).any(|part| has(cx, &format!("bubble-{CHECK_IN_MESSAGE}.p{part}")))
    };

    // Tutti opens at its end: the check-in message is far above.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(TUTTI.into(), window, cx);
            app.timeline.scroll_to_end();
        })
    });
    draw(cx);
    assert!(!shown(cx), "the message starts out of view");

    // Open the log from another channel, then use "Show in channel" of the
    // check-in (log entry 11).
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(LIGHTS.into(), window, cx);
            app.open_screen("proactivity", window, cx);
        })
    });
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.settings_section == SettingsSection::Proactivity)
    });
    // The log is below the fold: scroll the page until the link shows.
    for _ in 0..20 {
        if cx
            .debug_bounds("log-open-11")
            .is_some_and(|b| b.center().y < px(850.) && b.center().y > px(0.))
        {
            break;
        }
        cx.simulate_event(gpui_kit::ScrollEvent {
            position: point(px(900.), px(500.)),
            delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-300.))),
            ..Default::default()
        });
        draw(cx);
    }
    click(cx, "log-open-11");
    until(cx, &app, "the channel and the scroll", |app| {
        app.channel_id == TUTTI && app.convo.scroll_to.is_none()
    });
    // The link says where it goes, so it leaves Settings.
    app.read_with(cx, |app, _| assert_eq!(app.page, Page::Main));
    draw(cx);
    assert!(shown(cx), "the message is in view");
}

/// Scrolls Settings > Proactivity until a control shows.
fn scroll_to(cx: &mut VisualTestContext, selector: &str) {
    for _ in 0..20 {
        if cx
            .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
            .is_some_and(|b| b.center().y < px(850.) && b.center().y > px(0.))
        {
            return;
        }
        cx.simulate_event(gpui_kit::ScrollEvent {
            position: point(px(900.), px(500.)),
            delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-300.))),
            ..Default::default()
        });
        draw(cx);
    }
    panic!("{selector} does not scroll into view");
}

/// "Edit check-in.md" stays in Settings: it opens the note in Settings >
/// Memory.
#[gpui::test]
fn edit_check_in_opens_the_note_in_settings_memory(cx: &mut TestAppContext) {
    let (handle, app) = open(state("proactivity"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "proactivity", cx);
    app.read_with(cx, |app, _| assert_eq!(app.page, Page::Settings));
    scroll_to(cx, "proactivity-edit-note");
    click(cx, "proactivity-edit-note");
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, Page::Settings);
        assert_eq!(app.settings_section, SettingsSection::Memory);
        assert_eq!(
            app.knowledge.settings_note,
            Some((
                consort_client::memory::GLOBAL.to_owned(),
                consort_client::proactivity::CHECK_IN_NOTE_PATH.to_owned()
            ))
        );
    });
}

/// The brief's "Open routine" opens the routine in Settings > Routines.
#[gpui::test]
fn open_routine_of_the_brief_opens_in_settings_routines(cx: &mut TestAppContext) {
    let (handle, app) = open(state("proactivity"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "proactivity", cx);
    scroll_to(cx, "proactivity-open-brief");
    click(cx, "proactivity-open-brief");
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, Page::Settings);
        assert_eq!(app.settings_section, SettingsSection::Routines);
        assert_eq!(
            app.knowledge.settings_routine.as_deref(),
            Some(consort_client::mock_proactivity::BRIEF_ROUTINE)
        );
    });
    assert!(has(cx, "routine-back"));
}
