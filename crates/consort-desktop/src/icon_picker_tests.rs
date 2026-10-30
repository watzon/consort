//! Window tests of the All icons dialog (`docs/notes/design-icon-picker.md`).

use crate::app::{ConsortApp, Dialog};
use crate::convo::{ConvoAction, IconTarget};
use crate::convo_tests::{draw, mock, open, until};
use crate::ui::menu::MenuAction;
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::Entity;

/// Opens the New channel dialog and clicks All icons.
fn new_channel_with_picker(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(MenuAction::Convo(ConvoAction::NewChannel(None)), window, cx)
        })
    });
    draw(cx);
    let more = cx.debug_bounds("icon-choice-more").expect("All icons");
    cx.simulate_click(more.center(), gpui_kit::Modifiers::default());
    draw(cx);
}

fn search(app: &Entity<ConsortApp>, word: &str, cx: &mut VisualTestContext) {
    let word = word.to_owned();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.convo_inputs
                .icon_search
                .update(cx, |input, cx| input.set_value(&word, window, cx))
        })
    });
    draw(cx);
}

#[gpui::test]
fn all_icons_opens_over_new_channel_and_a_pick_takes_the_last_quick_place(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    new_channel_with_picker(&app, cx);
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.dialog,
            Some(Dialog::NewChannel),
            "New channel stays open"
        );
        assert_eq!(
            app.convo.icon_picker.as_ref().map(|p| p.target.clone()),
            Some(IconTarget::NewChannel)
        );
    });
    assert!(cx.debug_bounds("icon-picker").is_some());
    // Every icon is in the list before a search.
    assert!(cx.debug_bounds("icon-tile-code").is_some());

    // A word keeps only the icons that match it.
    search(&app, "moon", cx);
    assert!(cx.debug_bounds("icon-tile-moon").is_some());
    assert!(cx.debug_bounds("icon-tile-server").is_none());

    let moon = cx.debug_bounds("icon-tile-moon").unwrap();
    cx.simulate_click(moon.center(), gpui_kit::Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, cx| {
        assert!(app.convo.icon_picker.is_none(), "a pick closes All icons");
        assert_eq!(app.dialog, Some(Dialog::NewChannel));
        assert_eq!(app.convo.new_channel_icon, "moon");
        assert!(
            app.convo_inputs.icon_search.read(cx).value().is_empty(),
            "the search is empty for the next time"
        );
    });
    // The picked icon takes the place of the last quick icon.
    assert!(cx.debug_bounds("icon-choice-moon").is_some());
    assert!(cx.debug_bounds("icon-choice-star").is_none());
    assert!(cx.debug_bounds("icon-picker").is_none());

    // A quick icon brings the last place back.
    let code = cx.debug_bounds("icon-choice-code").unwrap();
    cx.simulate_click(code.center(), gpui_kit::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds("icon-choice-star").is_some());
    app.read_with(cx, |app, _| assert_eq!(app.convo.new_channel_icon, "code"));
}

#[gpui::test]
fn escape_closes_only_all_icons(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    new_channel_with_picker(&app, cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.convo.icon_picker.is_none());
        assert_eq!(
            app.dialog,
            Some(Dialog::NewChannel),
            "New channel stays open"
        );
    });
    // A second Escape closes New channel.
    cx.simulate_keystrokes("escape");
    draw(cx);
    app.read_with(cx, |app, _| assert!(app.dialog.is_none()));
}

#[gpui::test]
fn the_arrow_keys_move_through_the_list_and_enter_picks(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    new_channel_with_picker(&app, cx);
    // Down leaves the search field for the first icon, Right moves on one.
    cx.simulate_keystrokes("down right");
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.convo.icon_picker.as_ref().unwrap().focus, Some(1));
    });
    cx.simulate_keystrokes("down");
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.convo.icon_picker.as_ref().unwrap().focus, Some(12));
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.convo.icon_picker.is_none());
        // The 13th icon of the list: Work and code, second row, second place.
        assert_eq!(app.convo.new_channel_icon, "clock");
        assert_eq!(app.dialog, Some(Dialog::NewChannel));
    });
}

#[gpui::test]
fn enter_in_the_search_picks_the_first_result(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    new_channel_with_picker(&app, cx);
    search(&app, "serv", cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.convo.icon_picker.is_none());
        assert_eq!(app.convo.new_channel_icon, "server");
    });
}

#[gpui::test]
fn a_word_with_no_icon_says_so(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    new_channel_with_picker(&app, cx);
    search(&app, "xyz", cx);
    assert!(cx.debug_bounds("icon-tile-code").is_none());
    assert!(cx.debug_bounds("icon-list").is_some());
    app.read_with(cx, |app, _| {
        assert!(app.convo.icon_picker.is_some(), "the dialog stays open");
    });
}

#[gpui::test]
fn change_icon_from_the_channel_menu_saves_the_pick(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = app.read_with(cx, |app, _| {
        app.state
            .channels
            .iter()
            .find(|c| c.name == "sayso")
            .unwrap()
            .id
            .clone()
    });
    let id = sayso.clone();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(MenuAction::ChangeIcon(id), window, cx)
        })
    });
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(
            app.dialog.is_none(),
            "it opens alone, with no dialog under it"
        );
        assert_eq!(
            app.convo.icon_picker.as_ref().map(|p| p.target.clone()),
            Some(IconTarget::Channel(sayso.clone()))
        );
    });
    search(&app, "moon", cx);
    let moon = cx.debug_bounds("icon-tile-moon").unwrap();
    cx.simulate_click(moon.center(), gpui_kit::Modifiers::default());
    until(cx, &app, "the new icon of sayso", |app| {
        app.state.channel(&sayso).is_some_and(|c| c.icon == "moon")
    });
}

#[gpui::test]
fn channel_settings_opens_all_icons_and_keeps_the_pick_until_save(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = app.read_with(cx, |app, _| {
        app.state
            .channels
            .iter()
            .find(|c| c.name == "sayso")
            .unwrap()
            .id
            .clone()
    });
    let id = sayso.clone();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(Dialog::ChannelSettings(id), window, cx)
        })
    });
    draw(cx);
    let button = cx.debug_bounds("settings-icon").expect("the icon button");
    cx.simulate_click(button.center(), gpui_kit::Modifiers::default());
    draw(cx);
    search(&app, "moon", cx);
    let moon = cx.debug_bounds("icon-tile-moon").unwrap();
    cx.simulate_click(moon.center(), gpui_kit::Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.convo.settings.icon, "moon");
        assert!(matches!(app.dialog, Some(Dialog::ChannelSettings(_))));
        assert_ne!(
            app.state.channel(&sayso).unwrap().icon,
            "moon",
            "nothing saves before Save changes"
        );
    });
}

#[gpui::test]
fn a_tile_under_the_pointer_shimmers(cx: &mut TestAppContext) {
    use crate::ui::motion::FrameClock;
    let frame = std::time::Duration::from_millis(125);
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    // The test window is not the key window, so say it is; and hold the
    // mark still, so only the tile moves here.
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.window_active = true;
            app.mark_forced = Some(crate::ui::mark::MarkPhase::Done);
        })
    });
    new_channel_with_picker(&app, cx);
    let tick = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.global::<FrameClock>().tick());
    // Machines is in view when the list opens.
    let moon = cx
        .debug_bounds("icon-tile-server")
        .expect("the server tile");
    cx.simulate_mouse_move(moon.center(), None, gpui_kit::Modifiers::default());
    cx.run_until_parked();
    let hovered = cx.update(|_, cx| cx.global::<FrameClock>().hovered());
    assert_eq!(hovered, Some("icon-tile-server".into()));
    // The clock runs: the hovered icon plays its three versions in turn.
    let from = tick(cx);
    let mut versions = Vec::new();
    for _ in 0..3 {
        cx.executor().advance_clock(frame);
        cx.run_until_parked();
        versions.push(cx.update(|_, cx| {
            let id = gpui_kit::ElementId::from("icon-tile-server");
            let live = crate::ui::motion::is_hovered(&id, cx);
            crate::ui::motion::shimmer_version(live, cx)
        }));
    }
    assert_eq!(tick(cx) - from, 3);
    versions.sort_unstable();
    assert_eq!(versions, [0, 1, 2]);
    // The quick row of New channel shimmers the same way.
    cx.simulate_keystrokes("escape");
    draw(cx);
    let globe = cx.debug_bounds("icon-choice-globe").expect("a quick icon");
    cx.simulate_mouse_move(globe.center(), None, gpui_kit::Modifiers::default());
    cx.run_until_parked();
    let hovered = cx.update(|_, cx| cx.global::<FrameClock>().hovered());
    assert_eq!(hovered, Some("icon-choice-globe".into()));
}
