//! Window controls on Windows and Linux. macOS draws its own traffic lights
//! in the sidebar, so it gets none of this.
//!
//! On Windows the buttons only mark their areas: Windows hit-tests them as
//! the caption buttons, so snap layouts and the native behavior work. On
//! Linux, with client-side decorations, the buttons act on click.

use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Div, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
    StatefulInteractiveElement as _, Styled, Window, WindowControlArea, div,
    prelude::FluentBuilder as _,
};

use crate::ui::parts::icon;

/// Whether this window draws its own controls.
pub fn has_controls(window: &Window) -> bool {
    if cfg!(target_os = "macos") {
        return false;
    }
    #[cfg(target_os = "linux")]
    if !matches!(
        window.window_decorations(),
        gpui_kit::Decorations::Client { .. }
    ) {
        return false;
    }
    let _ = window;
    true
}

/// The width of one control button.
fn button_width(t: &Tokens) -> Pixels {
    t.space.page
}

/// The height of the title bar.
pub fn bar_height(t: &Tokens) -> Pixels {
    t.space.xxl
}

#[derive(Clone, Copy)]
enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Control {
    fn icon(self) -> &'static str {
        match self {
            Control::Minimize => "minus",
            Control::Maximize => "square",
            Control::Restore => "restore",
            Control::Close => "x",
        }
    }

    fn area(self) -> WindowControlArea {
        match self {
            Control::Minimize => WindowControlArea::Min,
            Control::Maximize | Control::Restore => WindowControlArea::Max,
            Control::Close => WindowControlArea::Close,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Control::Minimize => "window-minimize",
            Control::Maximize => "window-maximize",
            Control::Restore => "window-restore",
            Control::Close => "window-close",
        }
    }
}

fn control(kind: Control, t: &Tokens) -> impl IntoElement {
    let c = &t.colors;
    let (hover_bg, hover_fg) = match kind {
        Control::Close => (c.status_failed, c.on_failed),
        _ => (c.surface_selected, c.text_primary),
    };
    let fg = c.text_secondary;
    div()
        .id(kind.id())
        .group(kind.id())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(button_width(t))
        .h_full()
        .text_color(fg)
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .when(cfg!(target_os = "windows"), |d| {
            d.window_control_area(kind.area())
        })
        .when(cfg!(target_os = "linux"), |d| {
            d.on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                match kind {
                    Control::Minimize => window.minimize_window(),
                    Control::Maximize | Control::Restore => window.zoom_window(),
                    Control::Close => window.remove_window(),
                }
            })
        })
        .child(
            icon(kind.icon(), t.icon.dense, fg)
                .group_hover(kind.id(), move |s| s.text_color(hover_fg)),
        )
}

/// The title bar on Windows and Linux: the wordmark, a drag area and the
/// window controls. `None` on macOS and under server-side decorations.
pub fn title_bar(t: &Tokens, window: &Window) -> Option<AnyElement> {
    if !has_controls(window) {
        return None;
    }
    let c = &t.colors;
    let supported = window.window_controls();
    let maximized = window.is_maximized();
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(bar_height(t))
            .bg(c.surface_sidebar)
            .border_b(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .gap(t.space.sm)
                    .px(t.space.lg)
                    .drag_area()
                    .child(crate::ui::mark::still_mark(t.icon.size, t))
                    .child(
                        div()
                            .type_style(t.text.ui)
                            .font_weight(t.text.title.weight)
                            .text_color(c.text_secondary)
                            .child("consort"),
                    ),
            )
            .when(supported.minimize, |d| {
                d.child(control(Control::Minimize, t))
            })
            .when(supported.maximize, |d| {
                d.child(control(
                    if maximized {
                        Control::Restore
                    } else {
                        Control::Maximize
                    },
                    t,
                ))
            })
            .child(control(Control::Close, t))
            .into_any_element(),
    )
}

/// A region that moves the window when dragged: the empty parts of the
/// title strip and the headers.
pub trait DragArea: Styled + InteractiveElement + Sized {
    fn drag_area(self) -> Self {
        let this = self.window_control_area(WindowControlArea::Drag);
        #[cfg(target_os = "linux")]
        let this = this.on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count >= 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        });
        this
    }
}

impl DragArea for Div {}
