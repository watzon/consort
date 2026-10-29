//! The window that opens when the client cannot start. It says what
//! happened and what to do next, so the app never closes with no window.

use consort_theme::{ActiveTokens as _, TypeExt as _};
use gpui_kit::{
    Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement as _, Styled, Window, div,
};

use crate::ui::{
    parts::{ButtonKind, button, error_box, glyph, text},
    window_controls::{self, DragArea as _},
};

/// What the user does next. A start error is not something the app can fix
/// by itself.
pub const NEXT_STEP: &str =
    "Close Consort and open it again. If this error stays, restart your computer.";

pub struct StartupError {
    message: String,
}

impl StartupError {
    pub fn new(message: String) -> StartupError {
        StartupError { message }
    }
}

impl Render for StartupError {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.tokens().clone();
        let c = &t.colors;
        let mark_color = if t.is_dark() {
            c.accent_fill
        } else {
            c.accent_text
        };
        let form = div()
            .flex()
            .flex_col()
            .gap(t.space.xl)
            .w(t.layout.panel_width + t.space.page)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.text.heading.size * 0.35)
                    .child(glyph("mark", t.text.heading.size * 1.2, mark_color))
                    .child(
                        div()
                            .text_size(t.text.heading.size)
                            .font_weight(t.text.heading.weight)
                            .text_color(c.text_primary)
                            .child("consort"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.sm)
                    .child(text("Consort cannot start", t.text.display, c.text_primary))
                    .child(text(NEXT_STEP, t.text.body, c.text_secondary)),
            )
            .child(error_box("startup-error", self.message.clone(), &t))
            .child(
                div().flex().child(
                    button("quit", "Quit Consort", ButtonKind::Primary, &t)
                        .px(t.space.xl)
                        .on_click(cx.listener(|_, _, _, cx| cx.quit())),
                ),
            );
        div()
            .id("startup-error-window")
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface_canvas)
            .text_color(c.text_primary)
            .font_family(t.font_sans.clone())
            .type_style(t.text.ui)
            .children(window_controls::title_bar(&t, window))
            .child(
                div()
                    .h(t.space.xxl + t.space.md)
                    .w_full()
                    .flex_none()
                    .drag_area(),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .pb(t.space.page)
                    .child(form),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::NEXT_STEP;

    #[test]
    fn the_next_step_is_one_instruction_per_sentence() {
        assert_eq!(NEXT_STEP.matches('.').count(), 2);
        assert!(NEXT_STEP.starts_with("Close Consort"));
    }
}
