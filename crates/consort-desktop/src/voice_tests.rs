//! Phase 4c in a headless window: Settings > Voice off and on, Reset with
//! its confirm dialog, the offer's "Open the full guide", and the card of a
//! draft in your voice: every allow off until the tick, Edit and Save
//! changes, and "Not how I write".

use std::time::Duration;

use crate::app::{ConsortApp, Dialog, Page, Panel, SettingsSection};
use crate::voice::VoiceDialog;
use consort_client::mock_voice::{DRAFT_APPROVAL, GMAIL, MOCK_REVIEW_HASH, REPLY_TASK, SLACK};
use consort_client::voice::{Disclosure, GuideEdit};
use consort_client::{Client, ViewState};
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
            size: size(px(1440.), px(1400.)),
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
    consort_client::mock_voice::screen_state(screen).expect("the phase 4c mock")
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

/// The page with the voice off: the sources, and Turn on and learn, which
/// needs a ticked source.
#[gpui::test]
fn the_off_page_turns_on_only_with_a_ticked_source(cx: &mut TestAppContext) {
    let (handle, app) = open(state("voice-off"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "voice-off", cx);
    assert!(app.read_with(cx, |app, _| app.settings_section == SettingsSection::Voice));
    assert!(has(cx, "settings-Voice"));
    assert!(has(cx, "voice-switch") && has(cx, "voice-turn-on"));

    // Untick both sources: the ticks stay in the app, and nothing turns on.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.toggle_voice_source(GMAIL, cx);
            app.toggle_voice_source(SLACK, cx);
        })
    });
    click(cx, "voice-turn-on");
    settle(cx, 100);
    assert!(app.read_with(cx, |app, _| !app.state.voice.enabled));
    assert!(app.read_with(cx, |app, _| {
        app.voice_source_rows().iter().all(|r| !r.ticked)
    }));

    // Tick Gmail and the direct messages of Slack, then turn on.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.toggle_voice_source(GMAIL, cx);
            app.toggle_voice_private(SLACK, cx);
        })
    });
    click(cx, "voice-turn-on");
    until(cx, &app, "the voice is on", |app| app.state.voice.enabled);
    let rows = app.read_with(cx, |app, _| app.state.voice.sources.clone());
    assert!(rows.iter().all(|r| r.ticked));
    assert!(
        rows.iter()
            .any(|r| r.connection_id == SLACK && r.include_private)
    );
}

/// The page with the voice on: Remove a rule, pick a disclosure, write a
/// sample, and Reset, which asks first.
#[gpui::test]
fn the_on_page_edits_the_guide_and_reset_asks_first(cx: &mut TestAppContext) {
    let (handle, app) = open(state("voice-on"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "voice-on", cx);
    let core_rules = |app: &ConsortApp| app.state.voice.guide[0].rules.len();
    assert_eq!(app.read_with(cx, |app, _| core_rules(app)), 3);
    // The design's hover: Remove shows on the second core rule.
    assert!(has(cx, "voice-learn-again"));
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.edit_voice_guide(
                GuideEdit::RemoveRule {
                    section: "core".into(),
                    index: 1,
                },
                cx,
            )
        })
    });
    until(cx, &app, "the rule is gone", |app| core_rules(app) == 2);

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_voice_disclosure(Disclosure::Footer, cx)
        })
    });
    until(cx, &app, "the footer is chosen", |app| {
        app.state.voice.disclosure == Disclosure::Footer
    });

    // Reset is at the end of the long page: open it as its click does.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_voice_dialog(VoiceDialog::ConfirmReset, window, cx)
        })
    });
    draw(cx);
    assert!(app.read_with(cx, |app, _| app.dialog
        == Some(Dialog::Voice(VoiceDialog::ConfirmReset))));
    // Nothing changed before the confirm.
    assert!(app.read_with(cx, |app, _| app.state.voice.enabled));
    click(cx, "dialog-confirm");
    until(cx, &app, "the voice is reset", |app| {
        !app.state.voice.enabled && app.state.voice.guide.is_empty()
    });
}

/// The offer "Save this style guide?" opens the full guide.
#[gpui::test]
fn the_offer_opens_the_full_guide(cx: &mut TestAppContext) {
    let (handle, app) = open(state("voice-offer"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "voice-offer", cx);
    assert!(has(cx, "voice-open-guide"));
    click(cx, "voice-open-guide");
    assert!(app.read_with(cx, |app, _| app.dialog
        == Some(Dialog::Voice(VoiceDialog::Proposal))));
}

/// The id of the offer in Tutti.
fn offer_id(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) -> String {
    app.read_with(cx, |app, _| {
        app.state
            .timelines
            .iter()
            .flat_map(|t| &t.items)
            .find_map(|item| match item {
                consort_client::TimelineItem::Decision(card) => Some(card.decision.id.clone()),
                _ => None,
            })
            .expect("the offer")
    })
}

/// After Do it, the guide is a note. The answers go, the card says what
/// the user chose, and the link opens the saved note.
#[gpui::test]
fn do_it_replaces_the_answers_and_opens_the_saved_guide(cx: &mut TestAppContext) {
    let (handle, app) = open(state("voice-offer"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "voice-offer", cx);
    let id = offer_id(&app, cx);
    assert!(has(cx, &format!("offer-{id}-0")));
    assert!(!has(cx, "offer-outcome"));
    click(cx, &format!("offer-{id}-0"));
    until(cx, &app, "the offer is answered", |app| {
        app.state.timelines.iter().flat_map(|t| &t.items).any(|i| {
            matches!(i, consort_client::TimelineItem::Decision(c) if c.decision.chosen == Some(0))
        })
    });
    draw(cx);
    assert!(has(cx, "offer-outcome"));
    assert!(!has(cx, "voice-open-guide"));
    for i in 0..3 {
        assert!(!has(cx, &format!("offer-{id}-{i}")));
    }
    assert!(has(cx, "voice-saved-guide"));
    click(cx, "voice-saved-guide");
    assert!(app.read_with(cx, |app, _| app.dialog.is_none()));
}

/// After another answer, the card has no link: the proposal is gone.
#[gpui::test]
fn not_now_leaves_no_link_to_the_guide(cx: &mut TestAppContext) {
    let (handle, app) = open(state("voice-offer"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "voice-offer", cx);
    let id = offer_id(&app, cx);
    click(cx, &format!("offer-{id}-1"));
    until(cx, &app, "the offer is answered", |app| {
        app.state.timelines.iter().flat_map(|t| &t.items).any(|i| {
            matches!(i, consort_client::TimelineItem::Decision(c) if c.decision.chosen == Some(1))
        })
    });
    draw(cx);
    assert!(has(cx, "offer-outcome"));
    assert!(!has(cx, "voice-open-guide") && !has(cx, "voice-saved-guide"));
}

/// A draft with marked lines: Allow stays off until the tick. Edit and
/// Save changes check the text again and clear the tick. "Not how I
/// write" records the feedback.
#[gpui::test]
fn a_draft_card_keeps_allow_off_until_the_tick(cx: &mut TestAppContext) {
    let (handle, app) = open(state("voice-approval"), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "voice-approval", cx);
    assert!(has(cx, "draft-allow-off"));
    assert!(!has(cx, "draft-allow"));
    click(cx, "draft-checked");
    assert!(app.read_with(cx, |app, _| {
        app.draft_checked(DRAFT_APPROVAL, MOCK_REVIEW_HASH)
    }));
    // The tick holds only for the hash of the card that the user saw.
    assert!(!app.read_with(cx, |app, _| {
        app.draft_checked(DRAFT_APPROVAL, "another scan")
    }));
    assert!(has(cx, "draft-allow"));

    // Edit, then Save changes: the hub checks again and the tick clears.
    click(cx, "draft-edit");
    assert!(app.read_with(cx, |app, _| app.voice.editing.is_some()));
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.save_draft_edit(
                DRAFT_APPROVAL.into(),
                Some("subject".into()),
                Some("body".into()),
                cx,
            )
        })
    });
    until(cx, &app, "the flags are gone", |app| {
        !app.draft_checked(DRAFT_APPROVAL, MOCK_REVIEW_HASH) && app.voice.editing.is_none()
    });
    settle(cx, 100);
    assert!(has(cx, "draft-allow"), "no flags: allow is on");

    click(cx, "not-my-voice");
    assert!(app.read_with(cx, |app, _| app.voice.feedback.contains(DRAFT_APPROVAL)));
}

/// The voice task link in Settings > Voice says "Show task". It leaves
/// Settings on purpose and opens the task thread.
#[gpui::test]
fn show_task_leaves_settings_for_the_voice_task(cx: &mut TestAppContext) {
    let mut view = state("voice-approval");
    view.voice.learning = Some(consort_client::voice::VoiceLearningView {
        task_id: REPLY_TASK.into(),
        title: "Learn my voice".into(),
        line: "Learning from your sources".into(),
        status: consort_client::TaskStatus::Working,
    });
    let (handle, app) = open(view, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| app.update(cx, |app, cx| app.open_settings(SettingsSection::Voice, cx)));
    draw(cx);
    app.read_with(cx, |app, _| assert_eq!(app.page, Page::Settings));
    click(cx, "voice-open-task");
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, Page::Main);
        assert_eq!(app.panel, Panel::Thread(REPLY_TASK.into()));
    });
}
