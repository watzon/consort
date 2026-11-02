//! The visual item in a headless window (Paper G1 to G5, docs/
//! VISUALS-PLAN.md "Desktop"): the item in the timeline and in a task
//! thread, its states, its height, its menu and footer, what each action
//! does, the presence that the cover rule gives each item from the state of
//! the app, and the visual in the light box.
//!
//! A headless window cannot make a webview, so `VisualUi::live` is off in
//! tests. The probes still apply the cover rule and record what it said.

use std::{path::PathBuf, time::Duration};

use crate::{
    app::{ConsortApp, Panel},
    test_support::{bounds, click, draw, has, open, open_sized, settle, until},
    ui::{lightbox::LightboxItem, menu::MenuAction, visual_cover::Presence},
    visual::{Place, VisualAction, browser_file, render_width, visual_menu},
};
use consort_client::{
    ClientError, MessageView, PostBody, Speaker, TaskStatus, ThreadPost, TimelineItem, ViewState,
    VisualView, mock,
    mock_visual::{CHANNEL, COPY_SPEED, READ_SPEED},
};
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{Entity, Modifiers, MouseButton, px};

/// A visual that the mock does not know: its render is `not_found`.
const UNKNOWN: &str = "msg_visual_unknown";

/// The mock state with filler messages before the visuals of mac-ext4, so
/// the timeline can scroll them out, and a third visual that the mock
/// cannot draw at the end.
fn ext4_state() -> ViewState {
    let mut state = mock::view_state();
    let timeline = state
        .timelines
        .iter_mut()
        .find(|t| t.channel_id == CHANNEL)
        .expect("mac-ext4");
    let mut unknown = timeline
        .items
        .iter()
        .find_map(|item| match item {
            TimelineItem::Notice(n) if n.message_id == READ_SPEED => Some(n.clone()),
            _ => None,
        })
        .expect("the read speed visual");
    unknown.message_id = UNKNOWN.into();
    if let Some(visual) = unknown.visual.as_mut() {
        visual.message_id = UNKNOWN.into();
        visual.title = "A visual the hub cannot draw".into();
    }
    for ix in 0..30 {
        timeline.items.insert(
            0,
            TimelineItem::Message(MessageView {
                id: format!("msg_filler_{ix}"),
                channel_id: CHANNEL.into(),
                speaker: Speaker::You,
                time_label: "9:00 AM".into(),
                body: format!("Filler message {ix}, so the timeline scrolls."),
                ..MessageView::default()
            }),
        );
    }
    timeline.items.push(TimelineItem::Notice(unknown));
    state
}

/// Opens the app on mac-ext4 in a window `height` px high, with the window
/// active.
fn open_ext4(
    state: ViewState,
    height: f32,
    cx: &mut TestAppContext,
) -> (Entity<ConsortApp>, VisualTestContext) {
    let (handle, app) = open_sized(state, 1440., height, cx);
    let mut cx = VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.window_active = true;
            app.open_channel(CHANNEL.into(), window, cx);
        })
    });
    draw(&mut cx);
    draw(&mut cx);
    (app, cx)
}

fn view_of(app: &Entity<ConsortApp>, cx: &mut VisualTestContext, id: &str) -> VisualView {
    app.read_with(cx, |app, _| {
        app.state
            .timeline(CHANNEL)
            .into_iter()
            .flat_map(|t| t.items.iter())
            .find_map(|item| match item {
                TimelineItem::Notice(n) if n.message_id == id => n.visual.clone(),
                _ => None,
            })
            .expect("the visual")
    })
}

fn presence(app: &Entity<ConsortApp>, cx: &mut VisualTestContext, id: &str) -> Presence {
    app.read_with(cx, |app, _| app.visuals.frame.presence(Place::Timeline, id))
}

fn run(app: &Entity<ConsortApp>, cx: &mut VisualTestContext, action: VisualAction) {
    cx.update(|window, cx| app.update(cx, |app, cx| app.run_visual_action(action, window, cx)));
    draw(cx);
}

fn right_click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).center();
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    draw(cx);
}

fn menu_labels(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) -> Vec<String> {
    app.read_with(cx, |app, _| {
        app.menu
            .as_ref()
            .map(|m| m.items.iter().map(|i| i.label.to_string()).collect())
            .unwrap_or_default()
    })
}

/// A file in a folder of this test run.
fn temp_file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("consort-visual-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the temp folder");
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path
}

fn wait_for_file(cx: &mut VisualTestContext, path: &std::path::Path) -> Vec<u8> {
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while !path.is_file() {
        assert!(
            std::time::Instant::now() < end,
            "{} is written",
            path.display()
        );
        settle(cx, 50);
    }
    // The file is complete when the write returns.
    settle(cx, 50);
    let bytes = std::fs::read(path).expect("the file");
    // Leave nothing in the temp folder. Each test runs in its own process,
    // so the folder of this run holds only this file.
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent().filter(|d| {
        d.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("consort-visual-test-"))
    }) {
        let _ = std::fs::remove_dir(dir);
    }
    bytes
}

#[gpui::test]
fn each_visual_has_its_title_its_page_and_the_height_of_its_image(cx: &mut TestAppContext) {
    let (app, mut cx) = open_ext4(ext4_state(), 1400., cx);
    let cx = &mut cx;
    until(cx, &app, "the images", |app| {
        [READ_SPEED, COPY_SPEED]
            .iter()
            .all(|id| app.visuals.shown(id, 720).image.is_some())
    });
    draw(cx);
    for id in [READ_SPEED, COPY_SPEED] {
        let item = bounds(cx, &format!("visual-timeline-{id}"));
        let page = bounds(cx, &format!("visual-page-timeline-{id}"));
        assert!(has(cx, &format!("visual-image-timeline-{id}")), "{id}");
        // The page is the full width of the item, under the title line.
        assert_eq!(page.size.width, item.size.width, "{id}");
        assert!(page.top() > item.top(), "{id}: the title line is above");
        // The mock image is 720 px wide: the height follows it at the
        // width of the column.
        let view = view_of(&app, cx, id);
        let width = render_width(page.size.width);
        let image_height = if id == READ_SPEED { 264 } else { 260 };
        let expected = (image_height * width + 360) / 720;
        assert_eq!(page.size.height, px(expected as f32), "{id}");
        assert_eq!(view.notice.height_at(720), image_height, "{id}");
    }
}

#[gpui::test]
fn a_visual_with_no_image_loads_then_says_it_did_not_load_and_try_again_asks_again(
    cx: &mut TestAppContext,
) {
    let (handle, app) = open_sized(ext4_state(), 1440., 1400., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    // Before mac-ext4 opens, the image at the width of the column failed
    // once, with an error that passes. The next try is not due yet.
    let unavailable = ClientError::Unavailable {
        message: "The browser for visuals is installing. Try again in a minute.".into(),
    };
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.visuals.fail_image(UNKNOWN, 720, &unavailable, 1)
        })
    });
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_channel(CHANNEL.into(), window, cx)));
    draw(cx);
    draw(cx);
    let key = format!("timeline-{UNKNOWN}");
    assert!(
        has(cx, &format!("visual-loading-{key}")),
        "one try: loading"
    );
    assert!(!has(cx, &format!("visual-failed-{key}")));

    // After a few tries the item says so, with what to do next.
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.visuals.fail_image(UNKNOWN, 720, &unavailable, 3)
        })
    });
    draw(cx);
    assert!(has(cx, &format!("visual-failed-{key}")));
    assert!(has(cx, &format!("visual-try-again-{key}")));

    // Try again asks at once. The mock does not know this visual, so the
    // hub answers `not_found`: a hard error, shown at once.
    let asks = app.read_with(cx, |app, _| app.visuals.image_asks);
    click(cx, &format!("visual-try-again-{key}"));
    draw(cx);
    assert!(
        app.read_with(cx, |app, _| app.visuals.image_asks) > asks,
        "Try again asks for the image again"
    );
    until(cx, &app, "the answer of the hub", |app| {
        app.visuals
            .shown(UNKNOWN, 720)
            .image_failure
            .is_some_and(|f| f.hard && f.message.contains("This visual is gone"))
    });
    draw(cx);
    assert!(has(cx, &format!("visual-failed-{key}")));

    // When the page did not come either, the item says so in the words of
    // Paper G3.
    let offline = ClientError::Hub {
        code: "not_found".into(),
        message: "This file is not on the hub.".into(),
    };
    cx.update(|_, cx| app.update(cx, |app, _| app.visuals.fail_page(UNKNOWN, &offline, 1)));
    draw(cx);
    let text = cx
        .debug_bounds("visual-failed-timeline-msg_visual_unknown")
        .map(|_| ());
    assert!(text.is_some());
    app.read_with(cx, |app, _| {
        let shown = app.visuals.shown(UNKNOWN, 720);
        assert_eq!(
            crate::visual::body(
                shown.image.as_ref(),
                shown.image_failure.as_ref(),
                shown.page_failure.as_ref()
            ),
            crate::visual::Body::Failed(crate::visual::PAGE_FAILED.into())
        );
    });
}

#[gpui::test]
fn the_cover_rule_follows_the_state_of_the_app(cx: &mut TestAppContext) {
    let (app, mut cx) = open_ext4(ext4_state(), 1400., cx);
    let cx = &mut cx;
    draw(cx);
    // The copy speed visual is all in the clear part of the timeline.
    assert_eq!(presence(&app, cx, COPY_SPEED), Presence::Live);

    // A right click opens the menu of G2, and the item shows its image.
    right_click(cx, &format!("visual-page-timeline-{COPY_SPEED}"));
    assert_eq!(
        menu_labels(&app, cx),
        [
            "Open full screen",
            "Open in browser",
            "Copy image",
            "Save image",
            "Save as HTML"
        ]
    );
    draw(cx);
    assert_eq!(presence(&app, cx, COPY_SPEED), Presence::Image);
    cx.update(|_, cx| app.update(cx, |app, cx| app.close_menu(cx)));
    draw(cx);
    assert_eq!(presence(&app, cx, COPY_SPEED), Presence::Live);

    // A window that is not active shows the image.
    cx.update(|_, cx| app.update(cx, |app, _| app.window_active = false));
    draw(cx);
    assert_eq!(presence(&app, cx, COPY_SPEED), Presence::Image);
    cx.update(|_, cx| app.update(cx, |app, _| app.window_active = true));

    // Scrolled out of the list, the item is not drawn at all.
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.timeline.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            })
        })
    });
    draw(cx);
    draw(cx);
    assert_eq!(presence(&app, cx, COPY_SPEED), Presence::Hidden);
    assert!(!has(cx, &format!("visual-timeline-{COPY_SPEED}")));
}

#[gpui::test]
fn an_item_under_the_top_tint_shows_its_image(cx: &mut TestAppContext) {
    let (app, mut cx) = open_ext4(ext4_state(), 900., cx);
    let cx = &mut cx;
    // Put the copy speed visual at the top of the list, under the tint.
    let row = app.read_with(cx, |app, _| {
        app.chat_rows
            .iter()
            .position(|row| {
                matches!(row, consort_client::chat::ChatRow::Notice { item, .. }
                if app.state.timeline(CHANNEL).and_then(|t| t.items.get(*item)).is_some_and(
                    |i| matches!(i, TimelineItem::Notice(n) if n.message_id == COPY_SPEED)
                ))
            })
            .expect("the row of the visual")
    });
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.timeline.scroll_to(gpui::ListOffset {
                item_ix: row,
                offset_in_item: px(40.),
            })
        })
    });
    draw(cx);
    draw(cx);
    assert_eq!(presence(&app, cx, COPY_SPEED), Presence::Image);
}

#[gpui::test]
fn the_footer_shows_on_hover_and_its_buttons_act(cx: &mut TestAppContext) {
    let (app, mut cx) = open_ext4(ext4_state(), 1400., cx);
    let cx = &mut cx;
    let key = format!("timeline-{COPY_SPEED}");
    assert!(
        !has(cx, &format!("visual-open-{key}")),
        "no hover, no buttons"
    );
    let footer = bounds(cx, &format!("visual-footer-{key}"));
    cx.simulate_mouse_move(footer.center(), None, Modifiers::default());
    draw(cx);
    for button in ["open", "copy", "save"] {
        assert!(has(cx, &format!("visual-{button}-{key}")), "{button}");
    }
    // Open full screen opens the light box with this visual.
    click(cx, &format!("visual-open-{key}"));
    app.read_with(cx, |app, _| {
        let lightbox = app.lightbox.as_ref().expect("the light box");
        assert!(matches!(
            lightbox.current(),
            Some(LightboxItem::Visual(v)) if v.message_id == COPY_SPEED
        ));
        assert_eq!(lightbox.caption, "Visual from mac-ext4 at 9:44 AM.");
    });
    cx.simulate_keystrokes("escape");
    draw(cx);

    // Save image asks where, then writes the PNG there.
    cx.simulate_mouse_move(footer.center(), None, Modifiers::default());
    draw(cx);
    assert!(!cx.did_prompt_for_new_path());
    click(cx, &format!("visual-save-{key}"));
    assert!(
        cx.did_prompt_for_new_path(),
        "Save image opens the save dialog"
    );
    let to = temp_file("copy-speed.png");
    let chosen = to.clone();
    cx.simulate_new_path_selection(move |_| Some(chosen));
    let png = wait_for_file(cx, &to);
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "a PNG");
}

#[gpui::test]
fn copy_image_puts_the_png_on_the_clipboard(cx: &mut TestAppContext) {
    let (app, mut cx) = open_ext4(ext4_state(), 1400., cx);
    let cx = &mut cx;
    let view = view_of(&app, cx, COPY_SPEED);
    run(&app, cx, VisualAction::CopyImage(Box::new(view)));
    let end = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let image = cx.update(|_, cx| {
            cx.read_from_clipboard().and_then(|item| {
                item.entries.into_iter().find_map(|entry| match entry {
                    gpui_kit::ClipboardEntry::Image(image) => Some(image),
                    _ => None,
                })
            })
        });
        if let Some(image) = image {
            assert_eq!(image.format, gpui_kit::ImageFormat::Png);
            assert!(image.bytes.starts_with(b"\x89PNG"));
            break;
        }
        assert!(std::time::Instant::now() < end, "the image is copied");
        settle(cx, 50);
    }
}

#[gpui::test]
fn save_as_html_and_open_in_browser_write_the_page_with_the_theme_and_no_script(
    cx: &mut TestAppContext,
) {
    let (app, mut cx) = open_ext4(ext4_state(), 1400., cx);
    let cx = &mut cx;
    let view = view_of(&app, cx, READ_SPEED);
    let check = |html: &str| {
        assert!(html.contains("id=\"consort-visual-theme\""), "the theme");
        assert!(html.contains("--chart-1:"), "the chart roles");
        assert!(!html.contains("consort-visual-bootstrap"), "no app script");
        assert!(!html.contains("ipc.postMessage"));
    };

    run(&app, cx, VisualAction::SaveHtml(Box::new(view.clone())));
    assert!(cx.did_prompt_for_new_path(), "Save as HTML asks where");
    let to = temp_file("read-speed.html");
    let chosen = to.clone();
    cx.simulate_new_path_selection(move |_| Some(chosen));
    let saved = String::from_utf8(wait_for_file(cx, &to)).expect("text");
    check(&saved);
    assert!(saved.contains("Read speed"), "the page");

    let page = PathBuf::from(view.page.local_path.clone().expect("the mock page file"));
    let file = browser_file(&page, &view.page.sha256, &view.title);
    let _ = std::fs::remove_file(&file);
    run(&app, cx, VisualAction::OpenInBrowser(Box::new(view)));
    let written = String::from_utf8(wait_for_file(cx, &file)).expect("text");
    check(&written);
    assert_eq!(written, saved, "the same document");
    // The browser of the system opens the file.
    let url = crate::visual::file_url(&file);
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while cx.opened_url().as_deref() != Some(url.as_str()) {
        assert!(std::time::Instant::now() < end, "{url} opens");
        settle(cx, 50);
    }
}

#[gpui::test]
fn a_visual_in_a_task_thread_draws_in_the_thread(cx: &mut TestAppContext) {
    let mut state = mock::view_state();
    let view = state
        .timeline(CHANNEL)
        .into_iter()
        .flat_map(|t| t.items.iter())
        .find_map(|item| match item {
            TimelineItem::Notice(n) if n.message_id == READ_SPEED => n.visual.clone(),
            _ => None,
        })
        .expect("the visual");
    let post = ThreadPost {
        id: "pst_visual".into(),
        speaker: Speaker::Consort,
        time_label: "9:40 AM".into(),
        bodies: vec![PostBody::Notice {
            kind: "visual".into(),
            text: view.title.clone(),
            error: None,
            draft: None,
            fallback: None,
            monid: None,
            visual: Some(Box::new(view)),
        }],
        pending: false,
        source_message_id: None,
        refused: None,
    };
    for thread in &mut state.threads {
        if thread.task_id == mock::T_SAYSO {
            thread.posts = vec![post.clone()];
            thread.status = TaskStatus::Working;
        }
    }
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.window_active = true;
            app.open_thread(mock::T_SAYSO.into(), window, cx)
        })
    });
    draw(cx);
    draw(cx);
    let key = format!("thread-{READ_SPEED}");
    let item = bounds(cx, &format!("visual-{key}"));
    let panel = bounds(cx, "thread-panel");
    assert!(panel.contains(&item.center()), "the item is in the panel");
    assert!(item.size.width <= panel.size.width);
    until(cx, &app, "the image", |app| {
        app.visuals
            .width(Place::Thread, READ_SPEED)
            .is_some_and(|w| {
                app.visuals
                    .shown(READ_SPEED, render_width(w))
                    .image
                    .is_some()
            })
    });
    draw(cx);
    assert!(has(cx, &format!("visual-image-{key}")));
    assert_ne!(
        app.read_with(cx, |app, _| app
            .visuals
            .frame
            .presence(Place::Thread, READ_SPEED)),
        Presence::Hidden
    );
    // The right click opens the same menu.
    right_click(cx, &format!("visual-page-{key}"));
    assert_eq!(
        menu_labels(&app, cx).first().map(String::as_str),
        Some("Open full screen")
    );
}

#[gpui::test]
fn a_panel_over_the_window_covers_the_timeline_but_not_the_thread(cx: &mut TestAppContext) {
    let (handle, app) = open_sized(mock::view_state(), 600., 800., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_thread(mock::T_SAYSO.into(), window, cx)
        })
    });
    draw(cx);
    app.read_with(cx, |app, cx| {
        let t = ConsortApp::tokens(cx);
        assert!(app.covers_timeline(&t));
        assert!(!app.covers_thread());
        assert!(!app.covers_lightbox());
    });
    cx.update(|_, cx| app.update(cx, |app, cx| app.show_notice("Saved a.png.".into(), cx)));
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.covers_thread(), "a notice is over the thread");
        assert!(app.covers_lightbox(), "and over the light box");
    });
}

/// Opens the copy speed visual full screen.
fn open_full_screen(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) -> VisualView {
    let view = view_of(app, cx, COPY_SPEED);
    run(
        app,
        cx,
        VisualAction::OpenFullScreen(Box::new(view.clone())),
    );
    draw(cx);
    view
}

#[gpui::test]
fn the_light_box_shows_a_visual_on_a_sheet(cx: &mut TestAppContext) {
    let (app, mut cx) = open_ext4(ext4_state(), 900., cx);
    let cx = &mut cx;
    open_full_screen(&app, cx);
    let sheet = bounds(cx, "lightbox-sheet");
    let page = bounds(cx, &format!("visual-page-lightbox-{COPY_SPEED}"));
    assert!(sheet.contains(&page.center()));
    assert!(page.size.width > px(720.), "wider than in the timeline");
    // A visual opens alone: no arrows and no count.
    assert!(!has(cx, "lightbox-count"));
    assert!(!has(cx, "lightbox-previous"));
    // The sheet is live, and the item in the timeline shows its image.
    assert_eq!(
        app.read_with(cx, |app, _| app
            .visuals
            .frame
            .presence(Place::Lightbox, COPY_SPEED)),
        Presence::Live
    );
    assert_eq!(presence(&app, cx, COPY_SPEED), Presence::Image);
    // The page area is no taller than the stage.
    let lightbox = bounds(cx, "lightbox");
    assert!(page.bottom() <= lightbox.bottom());
    // A right click on the sheet has no Open full screen.
    right_click(cx, "lightbox-sheet");
    assert_eq!(
        menu_labels(&app, cx),
        [
            "Open in browser",
            "Copy image",
            "Save image",
            "Save as HTML"
        ]
    );
}

#[gpui::test]
fn the_tooltips_of_the_light_box_stay_above_the_sheet(cx: &mut TestAppContext) {
    let (_app, mut cx) = open_ext4(ext4_state(), 900., cx);
    let cx = &mut cx;
    open_full_screen(&_app, cx);
    let sheet = bounds(cx, "lightbox-sheet");
    for (button, tip) in [
        ("lightbox-save", "tip-Save image"),
        ("lightbox-open", "tip-Open in browser"),
        ("lightbox-close", "tip-Close"),
    ] {
        let edge = bounds(cx, button);
        // The pointer at the bottom edge of the button puts the tooltip as
        // low as it goes.
        let low = gpui_kit::point(edge.center().x, edge.bottom() - px(1.));
        cx.simulate_mouse_move(low, None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(600));
        draw(cx);
        let tip_bounds = bounds(cx, tip);
        assert!(
            tip_bounds.bottom() <= sheet.top(),
            "{tip} ends at {:?}, above the sheet at {:?}",
            tip_bounds.bottom(),
            sheet.top()
        );
    }
}

#[gpui::test]
fn a_click_outside_the_sheet_or_escape_closes_the_light_box(cx: &mut TestAppContext) {
    let (app, mut cx) = open_ext4(ext4_state(), 900., cx);
    let cx = &mut cx;
    open_full_screen(&app, cx);
    // A click on the sheet keeps it open.
    let sheet = bounds(cx, "lightbox-sheet");
    cx.simulate_click(
        gpui_kit::point(sheet.left() + px(4.), sheet.top() + px(4.)),
        Modifiers::default(),
    );
    draw(cx);
    assert!(
        app.read_with(cx, |app, _| app.lightbox.is_some()),
        "on the sheet"
    );
    // A click beside it closes.
    cx.simulate_click(
        gpui_kit::point(sheet.left() - px(8.), sheet.center().y),
        Modifiers::default(),
    );
    draw(cx);
    assert!(app.read_with(cx, |app, _| app.lightbox.is_none()), "beside");

    open_full_screen(&app, cx);
    assert!(app.read_with(cx, |app, _| app.lightbox.is_some()));
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(app.read_with(cx, |app, _| app.lightbox.is_none()), "Escape");
}

#[gpui::test]
fn the_menu_items_run_their_actions(cx: &mut TestAppContext) {
    let (app, mut cx) = open_ext4(ext4_state(), 1400., cx);
    let cx = &mut cx;
    let view = view_of(&app, cx, COPY_SPEED);
    let items = visual_menu(&view, false);
    // Open full screen through the menu, as a click on the item does.
    let first = items[0].action.clone();
    assert_eq!(
        first,
        MenuAction::Visual(VisualAction::OpenFullScreen(Box::new(view.clone())))
    );
    cx.update(|window, cx| app.update(cx, |app, cx| app.run_menu(first, window, cx)));
    draw(cx);
    assert!(app.read_with(cx, |app, _| app.menu.is_none() && app.lightbox.is_some()));
    // The buttons of the light box are named for a visual.
    let tips: Vec<_> = LightboxItem::Visual(view)
        .buttons()
        .iter()
        .map(|b| b.tip)
        .collect();
    assert_eq!(tips, ["Save image", "Open in browser", "Close"]);
    assert_eq!(
        app.read_with(cx, |app, _| app.panel.clone()),
        Panel::None,
        "nothing else opened"
    );
}
