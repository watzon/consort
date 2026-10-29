//! Key presses in the text inputs of the app, in a headless window.

use crate::app::{ConsortApp, Dialog, Field, Page, SettingsSection};
use crate::ui::{
    menu::MenuAnchor,
    model_picker::{PickerFocus, PickerTarget},
};
use consort_client::{Client, SelectionSource};
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Entity, WindowOptions};

fn select_all_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    }
}

/// The app on mock content, with the New channel dialog open and "general" in
/// its Name field.
fn app_with_a_dialog(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    // The mock client runs its own thread.
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
    });
    let (handle, app) = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| ConsortApp::new(Client::mock().expect("mock client"), true, window, cx))
            })
        })
        .expect("open the window");
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(Dialog::NewChannel, window, cx);
            app.set_field(Field::Name, "general", window, cx);
        });
    });
    cx.run_until_parked();
    (handle, app)
}

fn selected_name(app: &Entity<ConsortApp>, cx: &VisualTestContext) -> String {
    app.read_with(cx, |app, cx| {
        app.inputs.name.read(cx).selected_text().to_string()
    })
}

#[gpui::test]
fn select_all_selects_the_text_of_a_dialog_field(cx: &mut TestAppContext) {
    let (handle, app) = app_with_a_dialog(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.simulate_keystrokes(select_all_key());
    assert_eq!(selected_name(&app, cx), "general");
}

/// A key press that the key map of the input does not take reaches the root
/// key handler of the app, which sends the edit action to the focused input.
#[gpui::test]
fn select_all_works_when_the_key_map_of_the_input_does_not_take_the_key(cx: &mut TestAppContext) {
    let (handle, app) = app_with_a_dialog(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| cx.clear_key_bindings());
    cx.simulate_keystrokes(select_all_key());
    assert_eq!(selected_name(&app, cx), "general");
    // Cut takes the selected text out of the field.
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-x"
    } else {
        "ctrl-x"
    });
    app.read_with(cx, |app, cx| {
        assert_eq!(app.inputs.name.read(cx).value(), "");
    });
}

/// Opens Settings > Workers with the Add tag field of the first worker open
/// and "wor" typed in it.
fn app_with_a_tag_field(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    let (handle, app) = app_with_a_dialog(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.close_dialog(window, cx);
            app.open_settings(SettingsSection::Workers, cx);
            app.tag_entry = app.settings_worker.clone();
            app.inputs.tag.update(cx, |input, cx| {
                input.set_value("wor", window, cx);
                input.focus(window, cx);
            });
        });
    });
    cx.run_until_parked();
    (handle, app)
}

#[gpui::test]
fn escape_in_the_tag_field_closes_only_the_field(cx: &mut TestAppContext) {
    let (handle, app) = app_with_a_tag_field(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.simulate_keystrokes("escape");
    app.read_with(cx, |app, cx| {
        assert_eq!(app.page, Page::Settings, "Settings stays open");
        assert_eq!(app.tag_entry, None, "the tag field is closed");
        assert_eq!(app.inputs.tag.read(cx).value(), "", "the text is cleared");
    });
    // The next Escape leaves Settings.
    cx.simulate_keystrokes("escape");
    app.read_with(cx, |app, _| assert_eq!(app.page, Page::Main));
}

/// The app on mock content in Everything, with the model picker of the
/// composer open.
fn app_with_the_picker(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    let (handle, app) = app_with_a_dialog(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.close_dialog(window, cx);
            let target = PickerTarget::Assistant(app.channel_id.clone());
            app.open_picker(
                target,
                MenuAnchor::Point(gpui_kit::point(gpui_kit::px(600.), gpui_kit::px(800.))),
                window,
                cx,
            );
        });
    });
    cx.run_until_parked();
    (handle, app)
}

/// Waits until the mock client's thread has answered and the app has the
/// new state, then reads the model of a channel's assistant.
fn assistant_model(
    app: &Entity<ConsortApp>,
    cx: &mut VisualTestContext,
    channel: &str,
) -> (String, Option<String>, SelectionSource) {
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    app.read_with(cx, |app, _| {
        let chosen = app
            .state
            .channels
            .iter()
            .find(|c| c.name == channel)
            .and_then(|c| c.coordinator_selection.clone())
            .expect("a model");
        (
            chosen.selection.model_id.clone(),
            chosen.selection.effort().map(str::to_owned),
            chosen.source,
        )
    })
}

/// The picker opens, the search filters it, the keyboard chooses, and the
/// choice goes to the hub as `channel.update` for Everything. The effort
/// stays, because the new model offers it. A channel without its own model
/// follows, and a channel with its own keeps it.
#[gpui::test]
fn the_picker_filters_chooses_and_updates_the_channel(cx: &mut TestAppContext) {
    let (handle, app) = app_with_the_picker(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert_eq!(
        assistant_model(&app, cx, "Everything"),
        (
            "claude-opus-5-5".into(),
            Some("high".into()),
            SelectionSource::Channel
        )
    );
    cx.simulate_input("sonnet personal");
    app.read_with(cx, |app, cx| {
        let query = app.inputs.model_search.read(cx).value().to_string();
        let found = consort_client::picker::filter(&app.state.model_groups, &query);
        assert_eq!(
            found.len(),
            1,
            "only Claude Personal has a Sonnet that matches"
        );
        assert_eq!(found[0].models.len(), 1);
        assert_eq!(app.picker.as_ref().and_then(|p| p.highlighted), Some(0));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    app.read_with(cx, |app, _| {
        assert!(app.picker.is_none(), "a choice closes the picker")
    });
    assert_eq!(
        assistant_model(&app, cx, "Everything"),
        (
            "claude-sonnet-5-5".into(),
            Some("high".into()),
            SelectionSource::Channel
        )
    );
    assert_eq!(
        assistant_model(&app, cx, "homelab"),
        (
            "claude-sonnet-5-5".into(),
            Some("high".into()),
            SelectionSource::Everything
        )
    );
    // sayso has its own model, Sonnet 5.5 of Claude Work, and keeps it.
    let (model, _, source) = assistant_model(&app, cx, "sayso");
    assert_eq!(
        (model.as_str(), source),
        ("claude-sonnet-5-5", SelectionSource::Channel)
    );
    let account = app.read_with(cx, |app, _| {
        app.state
            .channels
            .iter()
            .find(|c| c.name == "sayso")
            .and_then(|c| c.coordinator_selection.clone())
            .map(|e| e.selection.account_label)
    });
    assert_eq!(account.as_deref(), Some("Claude Work"));
}

/// A model without the chosen effort level falls back to its default: the
/// selection has no effort.
#[gpui::test]
fn a_model_without_the_effort_level_falls_back_to_its_default(cx: &mut TestAppContext) {
    let (handle, app) = app_with_the_picker(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    // Codex offers Low to Extra high, and the effort is High: it stays.
    cx.simulate_input("gpt-5.5");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        assistant_model(&app, cx, "Everything"),
        (
            "gpt-5.5".into(),
            Some("high".into()),
            SelectionSource::Channel
        )
    );
    // Haiku 4.5 has no effort levels at all.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let target = PickerTarget::Assistant(app.channel_id.clone());
            app.open_picker(
                target,
                MenuAnchor::Point(gpui_kit::point(gpui_kit::px(600.), gpui_kit::px(800.))),
                window,
                cx,
            );
        })
    });
    cx.simulate_input("haiku 4.5");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        assistant_model(&app, cx, "Everything"),
        ("claude-haiku-4-5".into(), None, SelectionSource::Channel)
    );
}

/// The arrow keys move through the rows and wrap, and the list scrolls
/// inside the picker to keep the row in view. A search for "e" matches more
/// rows than the list can show.
#[gpui::test]
fn the_arrow_keys_wrap_and_scroll_the_list(cx: &mut TestAppContext) {
    let (handle, app) = app_with_the_picker(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.simulate_input("e");
    let total = app.read_with(cx, |app, cx| app.picker_rows(cx).0.len());
    assert!(total > 10, "{total} rows");
    app.read_with(cx, |app, _| {
        assert_eq!(app.picker.as_ref().and_then(|p| p.highlighted), Some(0))
    });
    cx.simulate_keystrokes("up");
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.picker.as_ref().and_then(|p| p.highlighted),
            Some(total - 1)
        )
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let offset = app.read_with(cx, |app, _| {
        app.picker.as_ref().map(|p| p.scroll.offset().y)
    });
    assert!(
        offset.is_some_and(|y| y < gpui_kit::px(0.)),
        "the last row scrolled into view: {offset:?}"
    );
    cx.simulate_keystrokes("down");
    app.read_with(cx, |app, _| {
        assert_eq!(app.picker.as_ref().and_then(|p| p.highlighted), Some(0))
    });
}

/// The picker opens on Favorites, because the mock has favorites. Left
/// moves the keyboard to the rail, Down shows the first account, Right goes
/// back to its rows, and typing searches every account with the rail
/// hidden.
#[gpui::test]
fn the_rail_takes_the_arrow_keys_and_typing_searches(cx: &mut TestAppContext) {
    use consort_client::picker::PickerView;
    let (handle, app) = app_with_the_picker(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    app.read_with(cx, |app, _| {
        let open = app.picker.as_ref().expect("open");
        assert_eq!(open.view, PickerView::Favorites);
        assert_eq!(open.focus, PickerFocus::List);
    });
    cx.simulate_keystrokes("left down");
    app.read_with(cx, |app, cx| {
        let open = app.picker.as_ref().expect("open");
        assert_eq!(open.view, PickerView::Account("Claude Personal".into()));
        assert_eq!(open.focus, PickerFocus::Rail { ring: true });
        let (rows, searching) = app.picker_rows(cx);
        assert!(!searching);
        assert_eq!(rows.len(), 4, "the models of Claude Personal");
        // The current model, Opus 5.5 of Claude Personal, has the keyboard.
        assert_eq!(open.highlighted, Some(0));
    });
    cx.simulate_keystrokes("down");
    app.read_with(cx, |app, _| {
        let open = app.picker.as_ref().expect("open");
        assert_eq!(open.view, PickerView::Account("Claude Work".into()));
    });
    cx.simulate_keystrokes("right");
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.picker.as_ref().map(|p| p.focus),
            Some(PickerFocus::List)
        )
    });
    cx.simulate_keystrokes("left");
    cx.simulate_input("gpt");
    app.read_with(cx, |app, cx| {
        let open = app.picker.as_ref().expect("open");
        assert_eq!(open.focus, PickerFocus::List, "typing leaves the rail");
        let (rows, searching) = app.picker_rows(cx);
        assert!(searching);
        let accounts: Vec<&str> = rows
            .iter()
            .map(|r| app.state.model_groups[r.group].account_label.as_str())
            .collect();
        assert_eq!(accounts, ["Codex Personal"; 3]);
    });
    // Escape clears the search first, and the account comes back.
    cx.simulate_keystrokes("escape");
    app.read_with(cx, |app, cx| {
        assert_eq!(app.inputs.model_search.read(cx).value(), "");
        let open = app.picker.as_ref().expect("still open");
        assert_eq!(open.view, PickerView::Account("Claude Work".into()));
    });
    cx.simulate_keystrokes("escape");
    app.read_with(cx, |app, _| assert!(app.picker.is_none()));
}

/// A click on the star of a row changes the favorite only: the picker stays
/// open, and the channel keeps its model.
#[gpui::test]
fn the_star_toggles_a_favorite_without_choosing_the_model(cx: &mut TestAppContext) {
    let (handle, app) = app_with_the_picker(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let before = assistant_model(&app, cx, "Everything");
    // Down to Sonnet 5.5 of Claude Personal, a favorite in the mock.
    cx.simulate_keystrokes("down");
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let star = app
        .read_with(cx, |app, _| app.triggers.get("picker-star"))
        .expect("the star of the highlighted row has bounds");
    assert!(app.read_with(cx, |app, _| {
        app.state
            .is_favorite("Claude Personal", "claude-sonnet-5-5")
    }));
    cx.simulate_click(star.center(), gpui_kit::Modifiers::default());
    let after = assistant_model(&app, cx, "Everything");
    app.read_with(cx, |app, _| {
        assert!(app.picker.is_some(), "the picker stays open");
        assert!(
            !app.state
                .is_favorite("Claude Personal", "claude-sonnet-5-5"),
            "the star is off"
        );
    });
    assert_eq!(after, before, "the model did not change");
}

/// The pointer on a rail button shows a tooltip with the account name.
#[gpui::test]
fn a_rail_button_shows_a_tooltip(cx: &mut TestAppContext) {
    let (handle, app) = app_with_the_picker(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    // The first account is the third child of the rail, after Favorites
    // and the divider.
    let button = app
        .read_with(cx, |app, _| {
            app.picker
                .as_ref()
                .and_then(|p| p.rail_scroll.bounds_for_item(2))
        })
        .expect("the rail has bounds");
    cx.simulate_mouse_move(button.center(), None, gpui_kit::Modifiers::default());
    cx.run_until_parked();
    app.read_with(cx, |app, cx| {
        assert_eq!(app.picker.as_ref().and_then(|p| p.rail_hover), Some(1));
        let t = ConsortApp::tokens(cx);
        assert!(app.render_picker_tooltip(&t).is_some());
    });
}

/// Escape in the picker of the New channel dialog closes only the picker.
#[gpui::test]
fn escape_in_the_picker_closes_only_the_picker(cx: &mut TestAppContext) {
    let (handle, app) = app_with_a_dialog(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_picker(
                PickerTarget::NewChannelTasks,
                MenuAnchor::Point(gpui_kit::point(gpui_kit::px(400.), gpui_kit::px(300.))),
                window,
                cx,
            );
        })
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    app.read_with(cx, |app, _| {
        assert!(app.picker.is_none(), "the picker is closed");
        assert_eq!(app.dialog, Some(Dialog::NewChannel), "the dialog stays");
        assert_eq!(app.new_channel.task_selection, None, "nothing was chosen");
    });
    // A choice in the dialog changes only the form.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_picker(
                PickerTarget::NewChannelTasks,
                MenuAnchor::Point(gpui_kit::point(gpui_kit::px(400.), gpui_kit::px(300.))),
                window,
                cx,
            );
        })
    });
    cx.simulate_input("codex");
    cx.simulate_keystrokes("enter");
    app.read_with(cx, |app, _| {
        let chosen = app
            .new_channel
            .task_selection
            .clone()
            .expect("a model for tasks");
        assert_eq!(chosen.account_label, "Codex Personal");
        assert_eq!(
            app.new_channel.coordinator_selection, None,
            "Same as Everything"
        );
        assert_eq!(app.dialog, Some(Dialog::NewChannel));
    });
}

/// The effort menu chooses a level for the channel's assistant.
#[gpui::test]
fn the_effort_menu_chooses_a_level(cx: &mut TestAppContext) {
    let (handle, app) = app_with_a_dialog(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.close_dialog(window, cx)));
    // The composer draws once, so the chip has bounds.
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let target = PickerTarget::Assistant(app.channel_id.clone());
            app.open_effort("composer-effort", target, window, cx);
        })
    });
    app.read_with(cx, |app, _| {
        assert!(app.effort_menu.is_some(), "the menu is open")
    });
    // Low, Medium (Default), High (chosen), Max: down goes to Max.
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(
        assistant_model(&app, cx, "Everything"),
        (
            "claude-opus-5-5".into(),
            Some("max".into()),
            SelectionSource::Channel
        )
    );
    app.read_with(cx, |app, _| assert!(app.effort_menu.is_none()));
}
