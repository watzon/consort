//! `ConsortApp::covers_timeline` in a headless window. Each overlay that a
//! visual must not draw over makes it true, one at a time, and the main page
//! with nothing open makes it false.

use crate::{
    app::{ConsortApp, Dialog, DropOver, Page, Panel},
    convo::{IconPicker, IconTarget},
    extensions::{UseInPicker, UseInTarget},
    permissions::{ExceptionPicker, RuleDialog},
    test_support::{draw, open, open_sized},
    ui::{
        menu::{MenuAction, MenuAnchor, MenuItem, OpenMenu},
        model_picker::{OpenEffort, PickerTarget},
    },
};
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{Context, Entity, Window, point, px};

fn covers(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) -> bool {
    draw(cx);
    app.read_with(cx, |app, cx| app.covers_timeline(&ConsortApp::tokens(cx)))
}

/// Opens an overlay with `open`, checks that it covers the timeline, closes
/// it with `close`, and checks that nothing covers it again.
fn check(
    app: &Entity<ConsortApp>,
    cx: &mut VisualTestContext,
    what: &str,
    open: impl FnOnce(&mut ConsortApp, &mut Window, &mut Context<ConsortApp>),
    close: impl FnOnce(&mut ConsortApp, &mut Window, &mut Context<ConsortApp>),
) {
    cx.update(|window, cx| app.update(cx, |app, cx| open(app, window, cx)));
    assert!(covers(app, cx), "{what} covers the timeline");
    cx.update(|window, cx| app.update(cx, |app, cx| close(app, window, cx)));
    assert!(!covers(app, cx), "nothing covers the timeline after {what}");
}

#[gpui::test]
fn each_overlay_covers_the_timeline(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert!(!covers(&app, cx), "the main page with nothing open");
    let anchor = MenuAnchor::Point(point(px(400.), px(300.)));

    check(
        &app,
        cx,
        "a menu",
        |app, _, cx| {
            let items = vec![MenuItem::new("Copy text", MenuAction::Copy("Hello".into()))];
            app.open_menu(OpenMenu::at(point(px(400.), px(300.)), items), cx)
        },
        |app, _, cx| app.close_menu(cx),
    );
    check(
        &app,
        cx,
        "a dialog",
        |app, window, cx| app.open_dialog(Dialog::About, window, cx),
        |app, window, cx| app.close_dialog(window, cx),
    );
    check(
        &app,
        cx,
        "the light box",
        |app, window, cx| {
            let image = consort_client::AttachmentView {
                name: "a.png".into(),
                ..Default::default()
            };
            app.open_lightbox(vec![image], 0, String::new(), window, cx)
        },
        |app, _, cx| app.close_lightbox(cx),
    );
    check(
        &app,
        cx,
        "the switcher",
        |app, window, cx| app.open_switcher(window, cx),
        |app, window, cx| app.close_switcher(window, cx),
    );
    check(
        &app,
        cx,
        "a notice",
        |app, _, cx| app.show_notice("Saved a.png.".into(), cx),
        |app, _, cx| {
            let serials: Vec<_> = app.notices.visible().map(|n| n.serial).collect();
            for serial in serials {
                app.notices.dismiss(serial);
            }
            cx.notify();
        },
    );
    check(
        &app,
        cx,
        "the drop state",
        |app, _, _| {
            // A frame with no drag clears the drop state, unless a screen
            // holds it.
            app.hold_drop = true;
            app.drop_hover = Some((DropOver::Chat, 2));
        },
        |app, _, _| {
            app.hold_drop = false;
            app.drop_hover = None;
        },
    );
    check(
        &app,
        cx,
        "Settings",
        |app, _, _| app.page = Page::Settings,
        |app, _, _| app.page = Page::Main,
    );
    let channel = app.read_with(cx, |app, _| app.channel_id.clone());
    check(
        &app,
        cx,
        "the model picker",
        {
            let channel = channel.clone();
            move |app, window, cx| {
                app.open_picker(PickerTarget::Assistant(channel), anchor, window, cx)
            }
        },
        |app, _, _| app.picker = None,
    );
    check(
        &app,
        cx,
        "the effort menu",
        move |app, _, _| {
            app.effort_menu = Some(OpenEffort {
                target: PickerTarget::Assistant(channel),
                anchor,
                highlighted: None,
            })
        },
        |app, _, _| app.effort_menu = None,
    );
    check(
        &app,
        cx,
        "the emoji picker",
        |app, _, _| app.convo.emoji_picker = Some(("msg_1".into(), anchor)),
        |app, _, _| app.convo.emoji_picker = None,
    );
    check(
        &app,
        cx,
        "All icons",
        |app, _, _| {
            app.convo.icon_picker = Some(IconPicker {
                target: IconTarget::NewChannel,
                focus: None,
            })
        },
        |app, _, _| app.convo.icon_picker = None,
    );
    check(
        &app,
        cx,
        "the Use in dialog",
        |app, _, _| {
            let current = consort_client::connectors::UseInView::EveryChannel;
            app.ext.use_in_picker = Some(UseInPicker::new(UseInTarget::ConnectDialog, &current))
        },
        |app, _, _| app.ext.use_in_picker = None,
    );
    check(
        &app,
        cx,
        "the rule dialog",
        |app, _, _| {
            app.convo.permissions.rule = Some(RuleDialog {
                editing: None,
                overriding: false,
                result: consort_proto::model::CommandRuleResult::Ask,
                error: None,
            })
        },
        |app, _, _| app.convo.permissions.rule = None,
    );
    check(
        &app,
        cx,
        "the tool picker",
        |app, _, _| {
            app.convo.permissions.picker = Some(ExceptionPicker {
                connection_id: "con_1".into(),
            })
        },
        |app, _, _| app.convo.permissions.picker = None,
    );
    check(
        &app,
        cx,
        "a dictation notice",
        |app, _, _| app.dictation.notice = Some(crate::dictation::DictationNotice::NoMicrophone),
        |app, _, _| app.dictation.notice = None,
    );
}

/// In a narrow window the sidebar is a drawer over the timeline, and a
/// panel covers the whole window. Both cover the timeline.
#[gpui::test]
fn the_drawer_and_a_panel_over_the_window_cover_the_timeline(cx: &mut TestAppContext) {
    let (handle, app) = open_sized(consort_client::mock::view_state(), 600., 800., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert!(!covers(&app, cx));
    // The drawer opens over the timeline in the compact layout.
    check(
        &app,
        cx,
        "the drawer",
        |app, _, _| app.layout.drawer_open = true,
        |app, _, _| app.layout.drawer_open = false,
    );
    cx.update(|_, cx| app.update(cx, |app, _| app.panel = Panel::Tasks));
    assert!(covers(&app, cx));
}

/// In a wide window a panel sits beside the timeline.
#[gpui::test]
fn a_panel_beside_the_timeline_does_not_cover_it(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| app.update(cx, |app, _| app.panel = Panel::Tasks));
    assert!(!covers(&app, cx));
}
