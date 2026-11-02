//! The light box of Paper G4 in a headless window: a click on the dark area
//! closes it and a click on the picture does not, Escape closes it, and the
//! top buttons are icon buttons with tooltips that do what Save, Open and
//! Close did.

use std::time::Duration;

use crate::{
    app::ConsortApp,
    test_support::{bounds, click, draw, has, open, settle, until},
    ui::lightbox::LightboxItem,
};
use consort_client::AttachmentView;
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{Entity, Modifiers, point, px};

/// A picture of the mock client. With no `local_path`, the light box shows
/// "Loading the picture" in the frame of the picture.
fn picture(name: &str, local_path: Option<String>) -> AttachmentView {
    AttachmentView {
        // No file of the mock client has this hash.
        sha256: "ab".repeat(32),
        name: name.into(),
        mime: "image/png".into(),
        size: 2048,
        image: true,
        detail: "PNG, 2 KB".into(),
        size_label: "2 KB".into(),
        local_path,
        artifact: None,
    }
}

/// A PNG three times as tall as it is wide, in a folder of its own.
fn tall_png() -> String {
    let dir = std::env::temp_dir().join(format!("consort-lightbox-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the temp folder");
    let path = dir.join("tall.png");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(100, 300).expect("a pixmap");
    pixmap.fill(resvg::tiny_skia::Color::from_rgba8(200, 160, 80, 255));
    std::fs::write(&path, pixmap.encode_png().expect("a PNG")).expect("the PNG file");
    path.display().to_string()
}

fn open_with(
    images: Vec<AttachmentView>,
    cx: &mut TestAppContext,
) -> (Entity<ConsortApp>, VisualTestContext) {
    let (handle, app) = open(consort_client::mock::view_state_with_files(), cx);
    let mut cx = VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_lightbox(
                images,
                0,
                "You sent it today at 7:40 PM.".into(),
                window,
                cx,
            )
        })
    });
    draw(&mut cx);
    (app, cx)
}

fn is_open(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) -> bool {
    app.read_with(cx, |app, _| app.lightbox.is_some())
}

fn at(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_click(point(px(x), px(y)), Modifiers::default());
    draw(cx);
}

#[gpui::test]
fn a_click_on_the_dark_area_closes_the_light_box(cx: &mut TestAppContext) {
    let (app, mut cx) = open_with(vec![picture("a.png", None)], cx);
    let cx = &mut cx;
    let lightbox = bounds(cx, "lightbox");
    // The bottom left corner, beside "1 of 1".
    at(
        cx,
        f32::from(lightbox.left()) + 8.,
        f32::from(lightbox.bottom()) - 8.,
    );
    assert!(!is_open(&app, cx));
    assert!(!has(cx, "lightbox"));

    // The top bar is dark area too, outside its buttons.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_lightbox(vec![picture("a.png", None)], 0, String::new(), window, cx)
        })
    });
    draw(cx);
    let details = bounds(cx, "lightbox-details");
    at(
        cx,
        f32::from(details.left()) + 4.,
        f32::from(details.center().y),
    );
    assert!(!is_open(&app, cx));
}

#[gpui::test]
fn a_click_on_the_picture_an_arrow_or_a_button_does_not_close(cx: &mut TestAppContext) {
    let (app, mut cx) = open_with(vec![picture("a.png", None), picture("b.png", None)], cx);
    let cx = &mut cx;
    let frame = bounds(cx, "lightbox-image");
    at(cx, frame.center().x.into(), frame.center().y.into());
    assert!(is_open(&app, cx), "the picture");
    // Previous is off on the first picture, and still not the dark area.
    click(cx, "lightbox-previous");
    assert!(is_open(&app, cx), "an arrow that is off");
    click(cx, "lightbox-next");
    app.read_with(cx, |app, _| {
        assert_eq!(app.lightbox.as_ref().map(|l| l.index), Some(1))
    });
    // The button closes no more than its own action does.
    click(cx, "lightbox-save");
    assert!(is_open(&app, cx), "a button");
    cx.simulate_new_path_selection(|_| None);
    settle(cx, 50);
}

/// A picture of another shape than its frame leaves dark bands, and a click
/// there closes the light box.
#[gpui::test]
fn a_click_beside_a_tall_picture_closes_the_light_box(cx: &mut TestAppContext) {
    let (app, mut cx) = open_with(vec![picture("tall.png", Some(tall_png()))], cx);
    let cx = &mut cx;
    // The picture loads on the background executor.
    settle(cx, 200);
    let frame = bounds(cx, "lightbox-image");
    // The picture is a third of the height wide, at the center.
    let band = f32::from(frame.left()) + 4.;
    let on_picture = f32::from(frame.center().x) + f32::from(frame.size.height) / 6. - 4.;
    let y = f32::from(frame.center().y);
    at(cx, on_picture, y);
    assert!(is_open(&app, cx), "the edge of the picture");
    at(cx, band, y);
    assert!(!is_open(&app, cx), "the band beside the picture");
}

#[gpui::test]
fn escape_closes_the_light_box(cx: &mut TestAppContext) {
    let (app, mut cx) = open_with(vec![picture("a.png", None)], cx);
    let cx = &mut cx;
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(!is_open(&app, cx));
}

/// Save asks where to save the file, Open asks the client for the file to
/// open it (the mock has no copy, so an error says so), and Close closes.
#[gpui::test]
fn the_icon_buttons_save_open_and_close(cx: &mut TestAppContext) {
    let (app, mut cx) = open_with(vec![picture("a.png", None)], cx);
    let cx = &mut cx;
    assert!(!cx.did_prompt_for_new_path());
    click(cx, "lightbox-save");
    assert!(cx.did_prompt_for_new_path(), "Save opens the save dialog");
    cx.simulate_new_path_selection(|_| None);
    settle(cx, 50);

    click(cx, "lightbox-open");
    until(cx, &app, "the error of Open", |app| {
        app.notices
            .visible()
            .any(|n| n.title().contains("This demo file has no copy"))
    });
    assert!(is_open(&app, cx));

    click(cx, "lightbox-close");
    assert!(!is_open(&app, cx));
}

/// The pointer on a top button shows its name in a tooltip.
#[gpui::test]
fn the_icon_buttons_have_tooltips(cx: &mut TestAppContext) {
    let (_app, mut cx) = open_with(vec![picture("a.png", None)], cx);
    let cx = &mut cx;
    for (button, tip) in [
        ("lightbox-save", "tip-Save"),
        ("lightbox-open", "tip-Open"),
        ("lightbox-close", "tip-Close"),
    ] {
        let center = bounds(cx, button).center();
        cx.simulate_mouse_move(center, None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(600));
        draw(cx);
        assert!(has(cx, tip), "{button} shows {tip}");
    }
}

#[gpui::test]
fn the_light_box_takes_a_list_of_items(cx: &mut TestAppContext) {
    let (app, mut cx) = open_with(Vec::new(), cx);
    let cx = &mut cx;
    assert!(!is_open(&app, cx), "no items, no light box");
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let items = vec![
                LightboxItem::Picture(picture("a.png", None)),
                LightboxItem::Picture(picture("b.png", None)),
            ];
            app.open_lightbox_items(items, 1, String::new(), window, cx)
        })
    });
    draw(cx);
    assert!(has(cx, "lightbox-count"));
    app.read_with(cx, |app, _| {
        let lightbox = app.lightbox.as_ref().expect("open");
        assert_eq!(lightbox.current().map(LightboxItem::name), Some("b.png"));
    });
}
