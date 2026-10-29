//! Key presses in the text inputs of the app, in a headless window.

use crate::app::{ConsortApp, Dialog, Field, Page, SettingsSection};
use consort_client::Client;
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
