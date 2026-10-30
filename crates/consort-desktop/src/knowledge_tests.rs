//! Phase 2 in a headless window: the Routines tab, a routine with its run
//! history, the routine dialog and its errors, Settings > Routines, the
//! Memory tab, the note editor with the stale-write message, Settings >
//! Memory with the version history and Recently deleted, and the memory
//! saved line in the timeline.

use std::time::Duration;

use crate::app::{ChannelTab, ConsortApp, Dialog, Panel, SettingsSection};
use crate::knowledge::{KnowledgeAction, Place, note_menu, routine_menu};
use consort_client::{
    Client, Command, ViewState,
    memory::GLOBAL,
    mock_routines::{CLIPS, LIGHTS, PRICE_WATCH, ROOFLINE, SUNSET},
};
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

fn state() -> ViewState {
    consort_client::mock_routines::screen_state("routines").expect("the phase 2 mock")
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

fn bounds(cx: &mut VisualTestContext, selector: &str) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let name: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    cx.debug_bounds(name)
        .unwrap_or_else(|| panic!("{selector} draws"))
}

fn has(cx: &mut VisualTestContext, selector: &str) -> bool {
    let name: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    cx.debug_bounds(name).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).center();
    cx.simulate_click(at, Modifiers::default());
    draw(cx);
}

fn open_tab(app: &Entity<ConsortApp>, tab: ChannelTab, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(LIGHTS.into(), window, cx);
            app.open_channel_tab(tab, cx);
        })
    });
    draw(cx);
}

fn routine(app: &ConsortApp, id: &str) -> consort_client::routines::RoutineView {
    app.state
        .routines
        .iter()
        .find(|r| r.id == id)
        .cloned()
        .expect("the routine")
}

/// The Routines tab lists the rows of the channel, and the switch turns a
/// routine off.
#[gpui::test]
fn the_routines_tab_lists_the_rows_and_the_switch_turns_one_off(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_tab(&app, ChannelTab::Routines, cx);
    assert!(has(cx, "routines-summary") && has(cx, "routine-new"));
    let sunset = bounds(cx, &format!("routine-row-{SUNSET}"));
    let price = bounds(cx, &format!("routine-row-{PRICE_WATCH}"));
    let clips = bounds(cx, &format!("routine-row-{CLIPS}"));
    assert!(sunset.origin.y < price.origin.y && price.origin.y < clips.origin.y);
    // The hover actions show only on the row under the pointer.
    assert!(!has(cx, &format!("routine-run-{SUNSET}")));
    cx.update(|_, cx| app.update(cx, |app, _| app.knowledge.hover = Some(SUNSET.into())));
    draw(cx);
    assert!(has(cx, &format!("routine-run-{SUNSET}")));
    assert!(has(cx, &format!("routine-history-{SUNSET}")));

    click(cx, &format!("routine-switch-{SUNSET}"));
    until(cx, &app, "the routine is off", |app| {
        !routine(app, SUNSET).enabled
    });
    // Every tab of the Channel panel takes a click now.
    click(cx, "channel-tab-Memory");
    app.read_with(cx, |app, _| assert_eq!(app.channel_tab, ChannelTab::Memory));
}

/// A routine opens in the panel with its summary and its runs. Run now adds
/// a run.
#[gpui::test]
fn a_routine_opens_with_its_run_history(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_tab(&app, ChannelTab::Routines, cx);
    click(cx, &format!("routine-row-{SUNSET}"));
    app.read_with(cx, |app, _| {
        assert_eq!(app.knowledge.routine_open.as_deref(), Some(SUNSET));
        assert_eq!(app.panel, Panel::Channel);
    });
    assert!(has(cx, "routine-runs-label"));
    for run in 1..=5 {
        assert!(has(cx, &format!("run-{run}")), "run {run} draws");
    }
    // A run is a row: the newest first.
    assert!(bounds(cx, "run-5").origin.y < bounds(cx, "run-1").origin.y);
    click(cx, "routine-page-run");
    until(cx, &app, "a new run", |app| {
        routine(app, SUNSET).runs.len() == 6
    });
    let newest = app.read_with(cx, |app, _| routine(app, SUNSET).runs[0].clone());
    assert!(newest.manual);
    click(cx, "routine-back");
    app.read_with(cx, |app, _| assert!(app.knowledge.routine_open.is_none()));
}

/// The menus of a routine and of a memory note keep their names and order.
#[test]
fn the_routine_and_note_menus_keep_their_names_and_order() {
    let all = consort_client::mock_routines::routines();
    let labels = |items: Vec<crate::ui::menu::MenuItem>| -> Vec<String> {
        items.iter().map(|i| i.label.to_string()).collect()
    };
    assert_eq!(
        labels(routine_menu(&all[0])),
        ["Run now", "Run history", "Edit", "Turn off", "Delete"]
    );
    let off = all.iter().find(|r| r.id == CLIPS).expect("clips");
    assert_eq!(labels(routine_menu(off))[3], "Turn on");
    assert!(routine_menu(off)[4].danger && routine_menu(off)[4].separator_before);
    assert_eq!(
        labels(note_menu(GLOBAL, "people/kevin.md")),
        ["Version history", "Copy path", "Rename", "Delete"]
    );
    assert!(note_menu(GLOBAL, "a.md")[3].danger);
}

/// Save stays off while a field has an error. The error sits under its
/// field. A fixed form saves.
#[gpui::test]
fn the_routine_dialog_saves_only_when_no_field_has_an_error(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_tab(&app, ChannelTab::Routines, cx);
    click(cx, "routine-new");
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::Routine(None)))
    });
    let before = app.read_with(cx, |app, _| app.state.routines.len());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let k = &app.knowledge_inputs;
            k.name
                .update(cx, |i, cx| i.set_value("Frost check", window, cx));
            k.prompt.update(cx, |i, cx| {
                i.set_value("Check the frost forecast.", window, cx)
            });
            app.pick_preset(consort_client::routines::Preset::Cron, window, cx);
            app.knowledge_inputs
                .cron
                .update(cx, |i, cx| i.set_value("0 8 * * MON 1", window, cx));
        })
    });
    draw(cx);
    assert!(has(cx, "field-error"));
    assert!(has(cx, "schedule-preview"));
    click(cx, "dialog-confirm");
    settle(cx, 150);
    app.read_with(cx, |app, _| {
        assert!(app.dialog.is_some(), "Save is off");
        assert_eq!(app.state.routines.len(), before);
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.knowledge_inputs
                .cron
                .update(cx, |i, cx| i.set_value("0 6 * * *", window, cx));
        })
    });
    draw(cx);
    assert!(!has(cx, "field-error"));
    click(cx, "dialog-confirm");
    until(cx, &app, "the new routine", |app| {
        app.state.routines.len() == before + 1 && app.dialog.is_none()
    });
    let saved = app.read_with(cx, |app, _| {
        app.state
            .routines
            .iter()
            .find(|r| r.name == "Frost check")
            .cloned()
            .expect("saved")
    });
    assert_eq!(
        consort_client::routines::summary_line(&saved),
        "Every day at 6:00, asks the assistant"
    );
}

/// The Weekly preset changes the days, and the preview shows the cron.
#[gpui::test]
fn the_weekly_preset_shows_its_days_and_its_cron(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_tab(&app, ChannelTab::Routines, cx);
    click(cx, "routine-new");
    click(cx, "preset-Weekly");
    click(cx, "day-Thu");
    let form = app.read_with(cx, |app, cx| app.routine_form(cx).expect("the form"));
    assert_eq!(
        form.schedule.days,
        [true, false, false, true, false, false, false]
    );
    assert_eq!(
        form.schedule.cron_expression().as_deref(),
        Some("0 9 * * 1,4")
    );
    assert!(has(cx, "schedule-cron"));
    // A Once routine has the calendar of its date field.
    click(cx, "preset-Once");
    click(cx, "schedule-calendar");
    assert!(has(cx, "schedule-calendar-popover"));
}

/// Settings > Routines groups by channel and filters by status.
#[gpui::test]
fn settings_routines_groups_by_channel_and_filters(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(SettingsSection::Routines, cx)
        })
    });
    draw(cx);
    assert!(has(cx, "settings-routines"));
    assert!(has(cx, "routine-group-house-lights"));
    assert!(has(cx, "routine-group-homelab"));
    click(cx, "routine-filter-Off");
    assert!(has(cx, &format!("routine-row-{CLIPS}")));
    assert!(!has(cx, &format!("routine-row-{SUNSET}")));
    click(cx, "routine-filter-Last run failed");
    assert!(has(cx, &format!("routine-row-{PRICE_WATCH}")));
    assert!(!has(cx, &format!("routine-row-{CLIPS}")));
    assert!(!has(cx, "routine-group-homelab"));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_knowledge_menu(
                KnowledgeAction::FilterChannel(Some("chn_homelab".into())),
                window,
                cx,
            )
        })
    });
    draw(cx);
    assert!(!has(cx, "routine-group-house-lights"));
}

/// The Memory tab shows the notes as a tree. A note opens in place. A save
/// after another writer saved shows the stale-write message, which keeps
/// the user's text. Save my version saves it.
#[gpui::test]
fn a_stale_save_keeps_the_text_and_saves_it_on_request(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_tab(&app, ChannelTab::Memory, cx);
    let path = "decisions/roofline.md";
    assert!(has(cx, "memory-summary"));
    assert!(has(cx, &format!("memory-folder-{LIGHTS}-decisions")));
    // A folder closes and opens.
    click(cx, &format!("memory-folder-{LIGHTS}-decisions"));
    assert!(!has(cx, &format!("memory-note-{LIGHTS}-{path}")));
    click(cx, &format!("memory-folder-{LIGHTS}-decisions"));
    click(cx, &format!("memory-note-{LIGHTS}-{path}"));
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.knowledge.panel_note,
            Some((LIGHTS.to_owned(), path.to_owned()))
        )
    });
    assert!(has(cx, "note-text"));
    click(cx, "note-edit");
    app.read_with(cx, |app, _| {
        assert!(
            app.knowledge
                .editor
                .as_ref()
                .is_some_and(|e| e.place == Place::Panel)
        )
    });
    // Another writer saves first.
    let (client, sha) = app.read_with(cx, |app, _| {
        let note = app.note(LIGHTS, path).expect("the note");
        (app.client.clone(), note.sha256)
    });
    let other = format!("{ROOFLINE}\n- Lumen Co said yes.");
    let expected = other.clone();
    cx.update(|_, cx| {
        cx.spawn(async move |_| {
            client
                .run(Command::WriteMemoryNote {
                    scope: LIGHTS.into(),
                    path: path.into(),
                    content: other,
                    expected_sha256: Some(sha),
                })
                .await
                .expect("the other write")
        })
        .detach()
    });
    until(cx, &app, "the other write", |app| {
        app.note(LIGHTS, path)
            .and_then(|n| n.content)
            .is_some_and(|c| c == expected)
    });
    settle(cx, 100);
    let mine = "# Roofline lights\n\nMy own text.";
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.knowledge_inputs
                .note
                .update(cx, |i, cx| i.set_value(mine, window, cx))
        })
    });
    click(cx, "note-save");
    until(cx, &app, "the stale-write message", |app| {
        app.knowledge
            .editor
            .as_ref()
            .is_some_and(|e| e.conflict.is_some())
    });
    draw(cx);
    assert!(has(cx, "note-conflict"));
    // The user's text stays in the editor, and Save waits for a choice.
    app.read_with(cx, |app, cx| {
        assert_eq!(app.knowledge_inputs.note.read(cx).value().to_string(), mine)
    });
    click(cx, "note-save");
    settle(cx, 100);
    app.read_with(cx, |app, _| assert!(app.knowledge.editor.is_some()));
    click(cx, "note-save-mine");
    until(cx, &app, "my version is saved", |app| {
        app.knowledge.editor.is_none()
            && app
                .note(LIGHTS, path)
                .and_then(|n| n.content)
                .is_some_and(|c| c == mine)
    });
}

/// Load the new version replaces the user's text with the newer version.
#[gpui::test]
fn load_the_new_version_takes_the_newer_text(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_tab(&app, ChannelTab::Memory, cx);
    let path = "decisions/roofline.md";
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_note_in_panel(LIGHTS, path, window, cx);
            app.edit_note(Place::Panel, LIGHTS, path, window, cx);
            let mut newer = app.note(LIGHTS, path).expect("the note");
            newer.version = 5;
            newer.content = Some("Newer text.".into());
            if let Some(editor) = app.knowledge.editor.as_mut() {
                editor.conflict = Some(newer);
            }
        })
    });
    draw(cx);
    click(cx, "note-load-new");
    app.read_with(cx, |app, cx| {
        let editor = app.knowledge.editor.as_ref().expect("the editor");
        assert!(editor.conflict.is_none());
        assert_eq!(editor.base_version, 5);
        assert_eq!(
            app.knowledge_inputs.note.read(cx).value().to_string(),
            "Newer text."
        );
    });
}

/// Settings > Memory shows global memory and each channel's memory. The
/// version history marks the changed lines, and Restore saves the old text
/// as the newest version.
#[gpui::test]
fn settings_memory_restores_an_old_version(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| app.update(cx, |app, cx| app.open_settings(SettingsSection::Memory, cx)));
    draw(cx);
    assert!(has(cx, "settings-memory"));
    assert!(has(cx, "memory-group-Global memory"));
    assert!(has(cx, "memory-group-house-lights"));
    assert!(has(cx, "memory-group-sayso"));
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.open_history(GLOBAL, "people/kevin.md", cx)
        })
    });
    until(cx, &app, "the versions", |app| {
        app.knowledge
            .history
            .as_ref()
            .is_some_and(|h| h.versions.len() == 6)
    });
    draw(cx);
    assert!(has(cx, "version-list") && has(cx, "version-preview"));
    // Version 5 differs from version 6 in its title and its last line.
    assert!(has(cx, "changed-line-0"));
    assert!(has(cx, "line-2"));
    let old = app.read_with(cx, |app, _| {
        let h = app.knowledge.history.as_ref().expect("history");
        h.versions[h.selected].content.clone().expect("text")
    });
    click(cx, "restore-version");
    until(cx, &app, "the restored text", |app| {
        app.note(GLOBAL, "people/kevin.md")
            .and_then(|n| n.content)
            .is_some_and(|c| c == old)
            && app.knowledge.history.is_none()
    });
}

/// A deleted note waits in Recently deleted, and Restore brings it back.
#[gpui::test]
fn recently_deleted_brings_a_note_back(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| app.update(cx, |app, cx| app.open_settings(SettingsSection::Memory, cx)));
    draw(cx);
    assert!(has(cx, "memory-deleted-row"));
    cx.update(|_, cx| app.update(cx, |app, _| app.knowledge.settings_deleted = true));
    draw(cx);
    assert!(has(cx, "recently-deleted"));
    let key = format!("{GLOBAL}:people/old-landlord.md");
    assert!(has(cx, &format!("deleted-{key}")));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_knowledge_menu(
                KnowledgeAction::RestoreNote(GLOBAL.into(), "people/old-landlord.md".into()),
                window,
                cx,
            )
        })
    });
    until(cx, &app, "the restored note", |app| {
        app.note(GLOBAL, "people/old-landlord.md")
            .is_some_and(|n| n.deleted_at.is_none())
    });
}

/// The memory saved line sits in the timeline with the note path as a link
/// that opens the note in the Channel panel.
#[gpui::test]
fn the_memory_saved_line_opens_its_note(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_channel(LIGHTS.into(), window, cx)));
    draw(cx);
    // The line is the newest item, at the end of the timeline.
    cx.update(|_, cx| app.update(cx, |app, _| app.timeline.scroll_to_end()));
    draw(cx);
    assert!(has(cx, "memory-saved-msg_lights_memory_saved"));
    click(cx, "memory-saved-link");
    app.read_with(cx, |app, _| {
        assert_eq!(app.panel, Panel::Channel);
        assert_eq!(app.channel_tab, ChannelTab::Memory);
        assert_eq!(
            app.knowledge.panel_note,
            Some((LIGHTS.to_owned(), "decisions/roofline.md".to_owned()))
        );
    });
}

/// A memory note in the search results opens in phase 2's views: a channel
/// note in the Memory tab of the Channel panel, a global note in Settings >
/// Memory.
#[gpui::test]
fn a_memory_search_hit_opens_the_note_in_the_memory_views(cx: &mut TestAppContext) {
    use crate::app::Page;
    use consort_client::convo::{SearchHitView, SearchSection, SearchTarget};

    let hit = |channel_id: Option<&str>, path: &str| SearchHitView {
        section: SearchSection::Memory,
        target: SearchTarget::Memory {
            channel_id: channel_id.map(str::to_owned),
            path: path.to_owned(),
        },
        title: path.to_owned(),
        title_note: None,
        icon: Some("notebook".into()),
        tutti: false,
        status: None,
        snippet: String::new(),
        detail: String::new(),
    };
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_hit(hit(Some(LIGHTS), "decisions/roofline.md"), window, cx)
        })
    });
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, Page::Main);
        assert_eq!(app.channel_id, LIGHTS);
        assert_eq!(app.panel, Panel::Channel);
        assert_eq!(app.channel_tab, ChannelTab::Memory);
        assert_eq!(
            app.knowledge.panel_note,
            Some((LIGHTS.to_owned(), "decisions/roofline.md".to_owned()))
        );
    });

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_hit(hit(None, "people/kevin.md"), window, cx)
        })
    });
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, Page::Settings);
        assert_eq!(app.settings_section, SettingsSection::Memory);
        assert_eq!(
            app.knowledge.settings_note,
            Some((GLOBAL.to_owned(), "people/kevin.md".to_owned()))
        );
    });
    assert!(has(cx, "settings-memory"));
}
