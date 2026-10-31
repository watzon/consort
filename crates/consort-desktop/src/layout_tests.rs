//! The redesign of 2026-10-30 in a headless window: the frame of the main
//! page, the sidebar rows, the top zone and the footer of a message group.

use crate::{
    app::{ConsortApp, Panel},
    test_support::title_bar,
};
use consort_client::Client;
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{
    AppContext as _, Bounds, Entity, Modifiers, MouseButton, Pixels, WindowOptions, point, px, size,
};

fn open(width: f32, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    // The mock client runs its own thread.
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
    });
    let options = WindowOptions {
        window_bounds: Some(gpui_kit::WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(width), px(900.)),
        })),
        ..WindowOptions::default()
    };
    let (handle, app) = cx
        .update(|cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    ConsortApp::new(
                        Client::mock_with(consort_client::mock::view_state()).expect("mock"),
                        true,
                        window,
                        cx,
                    )
                })
            })
        })
        .expect("open the window");
    let cx = &mut VisualTestContext::from_window(handle, cx);
    draw(cx);
    (handle, app)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("no element {selector}"))
}

fn has(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = bounds(cx, selector).center();
    cx.simulate_click(at, Modifiers::default());
    draw(cx);
}

/// The slide of the sidebar ends.
fn end_slide(cx: &mut VisualTestContext) {
    let t = cx.update(|_, cx| ConsortApp::tokens(cx));
    cx.executor().advance_clock(t.motion.normal * 2);
    draw(cx);
}

#[gpui::test]
fn the_buttons_move_to_the_traffic_lights_when_the_sidebar_hides(cx: &mut TestAppContext) {
    let (handle, app) = open(1440., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let t = cx.update(|_, cx| ConsortApp::tokens(cx));
    assert!(has(cx, "sidebar"));
    let open_at = bounds(cx, "sidebar-buttons");
    assert!(
        open_at.right() <= t.layout.sidebar_width,
        "in the title bar"
    );
    click(cx, "sidebar-toggle");
    end_slide(cx);
    assert!(!has(cx, "sidebar"), "no rail");
    app.read_with(cx, |app, _| assert!(app.layout.settings.sidebar_hidden));
    let closed_at = bounds(cx, "sidebar-buttons");
    assert_eq!(
        closed_at.left(),
        crate::ui::window_layout::closed_left(&t),
        "next to the traffic lights"
    );
    assert_eq!(closed_at.size, open_at.size);
    assert_eq!(closed_at.top(), open_at.top(), "the same center line");
    click(cx, "sidebar-toggle");
    end_slide(cx);
    assert!(has(cx, "sidebar"));
    assert_eq!(bounds(cx, "sidebar-buttons").left(), open_at.left());
}

#[gpui::test]
fn the_shortcut_hides_and_shows_the_sidebar(cx: &mut TestAppContext) {
    let (handle, app) = open(1440., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        let focus = app.read(cx).focus.clone();
        focus.focus(window, cx);
    });
    let key = if cfg!(target_os = "macos") {
        "cmd-\\"
    } else {
        "ctrl-\\"
    };
    cx.simulate_keystrokes(key);
    draw(cx);
    end_slide(cx);
    assert!(!has(cx, "sidebar"));
    cx.simulate_keystrokes(key);
    draw(cx);
    end_slide(cx);
    assert!(has(cx, "sidebar"));
}

#[gpui::test]
fn a_group_row_keeps_its_height_on_hover_and_a_right_click_opens_its_menu(cx: &mut TestAppContext) {
    let (handle, app) = open(1440., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let rest = bounds(cx, "sidebar-group-label");
    cx.simulate_mouse_move(rest.center(), None, Modifiers::default());
    draw(cx);
    assert_eq!(bounds(cx, "sidebar-group-label"), rest, "nothing moves");
    cx.simulate_mouse_down(rest.center(), MouseButton::Right, Modifiers::default());
    cx.simulate_mouse_up(rest.center(), MouseButton::Right, Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| {
        let labels: Vec<&str> = app
            .menu
            .as_ref()
            .expect("the menu")
            .items
            .iter()
            .map(|i| i.label.as_ref())
            .collect();
        assert_eq!(labels, ["New channel", "Rename", "Delete group"]);
    });
}

#[gpui::test]
fn the_top_actions_are_two_toggles_and_a_panel_edge_resizes(cx: &mut TestAppContext) {
    let (handle, app) = open(1440., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert!(has(cx, "channel-button") && has(cx, "tasks-button"));
    assert!(!has(cx, "new-task"));
    click(cx, "channel-button");
    assert_eq!(bounds(cx, "channel-panel").size.width, px(400.));
    let edge = bounds(cx, "panel-resize");
    assert_eq!(edge.size.width, px(8.));
    assert_eq!(edge.center().x, px(1040.));
    // The pointer sets the width, down to the least width.
    cx.update(|_, cx| {
        let t = ConsortApp::tokens(cx);
        app.update(cx, |app, cx| {
            app.drag_panel_edge(px(900.), &t, cx);
        })
    });
    draw(cx);
    assert_eq!(bounds(cx, "channel-panel").size.width, px(540.));
    cx.update(|_, cx| {
        let t = ConsortApp::tokens(cx);
        app.update(cx, |app, cx| {
            app.drag_panel_edge(px(1400.), &t, cx);
        })
    });
    draw(cx);
    assert_eq!(bounds(cx, "channel-panel").size.width, px(320.));
    // The same button closes it.
    click(cx, "channel-button");
    app.read_with(cx, |app, _| assert_eq!(app.panel, Panel::None));
}

#[gpui::test]
fn a_small_window_has_the_compact_layout(cx: &mut TestAppContext) {
    let (handle, app) = open(700., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert!(!has(cx, "sidebar") && !has(cx, "sidebar-buttons"));
    assert!(has(cx, "compact-menu") && has(cx, "compact-avatar") && has(cx, "channel-pill"));
    assert!(has(cx, "tasks-button") && !has(cx, "channel-button"));
    // The drawer opens over the chat, and a channel closes it.
    click(cx, "compact-menu");
    assert!(has(cx, "sidebar-drawer") && has(cx, "sidebar"));
    let sayso = app.read_with(cx, |app, _| {
        app.state
            .channels
            .iter()
            .find(|c| c.name == "sayso")
            .map(|c| c.id.clone())
            .expect("sayso")
    });
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_channel(sayso.clone(), window, cx)));
    draw(cx);
    assert!(!has(cx, "sidebar-drawer"));
    // A panel covers the window, and Back closes it.
    click(cx, "channel-pill");
    assert_eq!(bounds(cx, "channel-panel").size.width, px(700.));
    assert!(has(cx, "panel-back"));
    click(cx, "panel-back");
    app.read_with(cx, |app, _| assert_eq!(app.panel, Panel::None));
}

#[gpui::test]
fn the_top_zone_takes_the_pointer_from_the_timeline(cx: &mut TestAppContext) {
    let (handle, _app) = open(1440., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let t = cx.update(|_, cx| ConsortApp::tokens(cx));
    let zone = bounds(cx, "top-zone");
    assert_eq!(zone.top(), title_bar(cx));
    assert_eq!(zone.size.height, t.layout.content_top);
    assert_eq!(zone.left(), t.layout.sidebar_width);
}

#[gpui::test]
fn a_hover_shows_the_footer_of_a_group_and_nothing_moves(cx: &mut TestAppContext) {
    let (handle, app) = open(1440., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let first_bubble = app.read_with(cx, |app, _| {
        app.chat_rows
            .iter()
            .rev()
            .find_map(|r| match r {
                consort_client::chat::ChatRow::Bubble(b) => Some(b.clone()),
                _ => None,
            })
            .expect("a bubble")
    });
    let id = app.read_with(cx, |app, _| {
        match app
            .state
            .timeline(&app.channel_id)
            .and_then(|t| t.items.get(first_bubble.item))
        {
            Some(consort_client::TimelineItem::Message(m)) => m.id.clone(),
            _ => panic!("a message"),
        }
    });
    let selector = format!("bubble-{id}.p{}", first_bubble.part);
    let at = cx
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .expect("the bubble");
    assert!(!has(cx, "footer-react"), "the line is empty at rest");
    cx.simulate_mouse_move(at.center(), None, Modifiers::default());
    draw(cx);
    assert!(has(cx, "footer-react") && has(cx, "footer-reply") && has(cx, "footer-copy"));
    let t = cx.update(|_, cx| ConsortApp::tokens(cx));
    assert_eq!(
        bounds(cx, "footer-react").size.height,
        t.component.chat.footer_button
    );
    let copy = bounds(cx, "footer-copy");
    let again = copy.origin;
    cx.simulate_mouse_move(copy.center(), None, Modifiers::default());
    draw(cx);
    assert_eq!(bounds(cx, "footer-copy").origin, again, "nothing moves");
}

#[gpui::test]
fn compact_settings_show_the_list_then_one_section(cx: &mut TestAppContext) {
    let (handle, app) = open(700., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    click(cx, "compact-avatar");
    app.read_with(cx, |app, _| {
        assert_eq!(app.page, crate::app::Page::Settings);
        assert!(app.layout.settings_list);
    });
    assert!(!has(cx, "settings-list-back"));
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(crate::app::SettingsSection::Workers, cx)
        })
    });
    draw(cx);
    assert!(has(cx, "settings-list-back"));
    click(cx, "settings-list-back");
    app.read_with(cx, |app, _| assert!(app.layout.settings_list));
}

#[gpui::test]
fn the_tasks_and_channel_panels_have_the_same_header_and_tabs(cx: &mut TestAppContext) {
    let (handle, _app) = open(1440., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    click(cx, "tasks-button");
    let (header, tabs) = (bounds(cx, "panel-header"), bounds(cx, "panel-tabs"));
    click(cx, "channel-button");
    assert_eq!(bounds(cx, "panel-header"), header);
    assert_eq!(bounds(cx, "panel-tabs"), tabs);
}

/// GPUI gives a flex item the full width of its text as its minimum, so a long
/// line pushes its neighbors out of the row. `min_w_0` lets the text wrap.
#[test]
fn every_flex_1_can_shrink_so_its_text_wraps() {
    fn in_chain<'a>(lines: impl Iterator<Item = &'a str>, indent: usize) -> bool {
        for line in lines {
            let own = line.len() - line.trim_start().len();
            if own != indent || !line.trim_start().starts_with('.') {
                if own <= indent && !line.trim_start().starts_with('.') {
                    return false;
                }
                continue;
            }
            // A label that never wraps keeps its width, as a tab does.
            if line.contains("min_w_0") || line.contains("whitespace_nowrap") {
                return true;
            }
        }
        false
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
    let mut bad = Vec::new();
    let mut folders = vec![root];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = source.lines().collect();
            for (at, line) in lines.iter().enumerate() {
                if !line.contains(".flex_1()")
                    || line.contains("min_w_0")
                    || line.contains("whitespace_nowrap")
                {
                    continue;
                }
                let indent = line.len() - line.trim_start().len();
                if !in_chain(lines[..at].iter().rev().copied(), indent)
                    && !in_chain(lines[at + 1..].iter().copied(), indent)
                {
                    bad.push(format!("{}:{}", path.display(), at + 1));
                }
            }
        }
    }
    assert!(bad.is_empty(), "flex_1 with no min_w_0: {bad:#?}");
}
