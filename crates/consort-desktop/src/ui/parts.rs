//! Small parts that every surface uses: icons, status glyphs, buttons and
//! text styles. Sizes and colors come from the theme tokens only.

use consort_client::TaskStatus;
use consort_theme::{Tokens, Type, TypeExt as _};
use std::{cell::Cell, time::Duration};

use gpui_kit::{
    AnyElement, App, Div, ElementId, Global, Hsla, InteractiveElement, IntoElement, ParentElement,
    Pixels, SharedString, Stateful, StatefulInteractiveElement as _, Styled, Svg, Transformation,
    base::SelectableText, div, percentage, prelude::FluentBuilder as _, svg,
};

/// A Lucide icon from `assets/icons`.
pub fn icon(name: &str, size: Pixels, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(size)
        .flex_none()
        .text_color(color)
}

/// A Consort glyph from `assets/glyphs`.
pub fn glyph(name: &str, size: Pixels, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("glyphs/{name}.svg")))
        .size(size)
        .flex_none()
        .text_color(color)
}

/// The color of a status glyph.
pub fn status_color(status: TaskStatus, t: &Tokens) -> Hsla {
    let c = &t.colors;
    match status {
        TaskStatus::UpNext => c.text_tertiary,
        TaskStatus::Scheduled => c.status_scheduled,
        TaskStatus::Working => c.status_working,
        TaskStatus::NeedsInput | TaskStatus::Review => c.accent_text,
        TaskStatus::Failed => c.status_failed,
        TaskStatus::Resolved => c.status_done,
    }
}

/// How many times a second the Working glyph moves. The arc turns in steps,
/// so one timer redraws the window about 15 times a second while a Working
/// glyph shows. An animation element would ask for a new frame at every
/// display refresh, and GPUI draws the whole window for each one.
pub const SPIN_STEPS_PER_SECOND: u32 = 15;

/// The turn of every Working glyph, shared. All glyphs show the same turn,
/// so one timer moves them all. The app sets it as a global.
#[derive(Default)]
pub struct Spin {
    /// The turn, from 0.0 to 1.0.
    turn: Cell<f32>,
    /// A Working glyph was drawn since the last `begin_frame`.
    used: Cell<bool>,
}

impl Global for Spin {}

impl Spin {
    /// Starts a frame: no Working glyph has been drawn yet.
    pub fn begin_frame(&self) {
        self.used.set(false);
    }

    /// True when the frame drew a Working glyph, so the timer must run.
    pub fn used(&self) -> bool {
        self.used.get()
    }

    /// Moves the turn on by one step. `period` is the time of one full turn.
    pub fn advance(&self, period: Duration) {
        let steps = steps_per_turn(period) as f32;
        self.turn.set((self.turn.get() + 1.0 / steps).fract());
    }

    pub fn turn(&self) -> f32 {
        self.turn.get()
    }
}

/// The steps in one full turn of the glyph.
pub fn steps_per_turn(period: Duration) -> u32 {
    ((period.as_secs_f32() * SPIN_STEPS_PER_SECOND as f32).round() as u32).max(4)
}

/// The time between two steps.
pub fn step_interval(period: Duration) -> Duration {
    period / steps_per_turn(period)
}

/// True when the spin timer must run: a Working glyph is on screen, the
/// window has the focus, and the system does not ask for less motion. A
/// window that sits behind others, or a screen with no Working glyph, draws
/// nothing on a timer.
pub fn spin_runs(glyph_shown: bool, window_active: bool, reduce_motion: bool) -> bool {
    glyph_shown && window_active && !reduce_motion
}

/// The glyph of a task status. The Working glyph is two layers: a still
/// ring and an arc that turns once every `motion.spin`, in steps that one
/// shared timer makes (see [`Spin`]). It stands still when the system asks
/// for less motion.
pub fn status_glyph(
    id: impl Into<ElementId>,
    status: TaskStatus,
    size: Pixels,
    t: &Tokens,
    cx: &App,
) -> AnyElement {
    let color = status_color(status, t);
    let name = match status {
        TaskStatus::UpNext => "status-up-next",
        TaskStatus::Scheduled => "status-scheduled",
        TaskStatus::NeedsInput => "status-needs-input",
        TaskStatus::Review => "status-review",
        TaskStatus::Failed => "status-failed",
        TaskStatus::Resolved => "status-resolved",
        TaskStatus::Working => {
            let _ = id;
            let arc = glyph("status-working-arc", size, color)
                .absolute()
                .top_0()
                .left_0();
            let spin = cx.try_global::<Spin>();
            if let Some(spin) = spin {
                spin.used.set(true);
            }
            let arc: AnyElement = match spin {
                Some(spin) if !cx.reduce_motion() => arc
                    .with_transformation(Transformation::rotate(percentage(spin.turn())))
                    .into_any_element(),
                _ => arc.into_any_element(),
            };
            return div()
                .relative()
                .flex_none()
                .size(size)
                .child(glyph("status-working-track", size, t.colors.border_strong))
                .child(arc)
                .into_any_element();
        }
    };
    glyph(name, size, color).into_any_element()
}

/// A dot for a worker: solid in `status.done` when online, an open ring in
/// `text.tertiary` when offline. Centered in a 16 px slot.
pub fn worker_dot(online: bool, dot: Pixels, slot: Pixels, t: &Tokens) -> Div {
    let inner = div().size(dot).rounded(t.radius.full).flex_none();
    let inner = if online {
        inner.bg(t.colors.status_done)
    } else {
        inner.border(t.stroke).border_color(t.colors.text_tertiary)
    };
    div()
        .size(slot)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(inner)
}

/// A line of text in one style and color.
pub fn text(content: impl Into<SharedString>, style: Type, color: Hsla) -> Div {
    div()
        .type_style(style)
        .text_color(color)
        .child(content.into())
}

/// A line of text in one style and color that the user can select and copy.
/// `id` must be unique and stable, because the selection keeps its state by
/// it.
pub fn selectable(
    id: impl Into<ElementId>,
    content: impl Into<SharedString>,
    style: Type,
    color: Hsla,
) -> Div {
    div()
        .type_style(style)
        .text_color(color)
        .child(SelectableText::new(id, content))
}

/// An error on a tinted ground, with the text selectable so the user can
/// copy the details.
pub fn error_box(id: impl Into<ElementId>, message: impl Into<SharedString>, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .gap(t.space.sm)
        .p(t.space.md)
        .rounded(t.radius.md)
        .bg(c.tint_failed)
        .child(icon("circle-alert", t.icon.size, c.status_failed))
        .child(
            selectable(id, message, t.text.ui, c.text_primary)
                .flex_1()
                .min_w_0(),
        )
}

/// A text that ends with an ellipsis when it does not fit.
pub fn line(content: impl Into<SharedString>, style: Type, color: Hsla) -> Div {
    text(content, style, color).min_w_0().truncate()
}

/// The kinds of text button.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// `accent.fill`: the answer that the user most likely gives.
    Primary,
    /// `surface.raised` with a `border.strong` border.
    Secondary,
    /// Text only, in `text.secondary`.
    Quiet,
    /// Text only, in `status.failed`.
    Danger,
}

/// Makes a control reachable by the keyboard. It joins the Tab order, Enter
/// and Space click it, and it shows the focus ring (`accent.text`, drawn
/// outside the control) while the keyboard has the focus. A click with the
/// mouse shows no ring.
pub fn focusable(control: Stateful<Div>, t: &Tokens) -> Stateful<Div> {
    let ring = t.focus_ring();
    control
        .focusable()
        .tab_stop(true)
        .focus_visible(move |style| style.shadow(ring))
}

/// A text button: UI strong text, `md` corners, 30 px high. The keyboard can
/// focus it.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    t: &Tokens,
) -> Stateful<Div> {
    let c = &t.colors;
    let base = focusable(div().id(id), t)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(t.space.xs + t.space.xxs)
        .px(t.space.md)
        .rounded(t.radius.md)
        .type_style(t.text.ui_strong)
        .cursor_pointer()
        .whitespace_nowrap()
        .child(label.into());
    // Every kind is 30 px high: 20 px of text plus 5 px above and below. The
    // secondary button's border takes 1 px of each.
    let pad = t.component.button_pad_y;
    match kind {
        ButtonKind::Primary => base
            .py(pad)
            .bg(c.accent_fill)
            .text_color(c.accent_on_fill)
            .hover(|s| s.bg(c.hover_accent_fill))
            .active(|s| s.bg(c.pressed_accent_fill)),
        ButtonKind::Secondary => base
            .py(t.space.xs)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_strong)
            .text_color(c.text_primary)
            .hover(|s| s.bg(c.hover_raised))
            .active(|s| s.bg(c.pressed_raised)),
        ButtonKind::Quiet => base
            .py(pad)
            .text_color(c.text_secondary)
            .hover(|s| s.bg(c.surface_selected).text_color(c.text_primary)),
        ButtonKind::Danger => base
            .py(pad)
            .text_color(c.status_failed)
            .hover(|s| s.bg(c.tint_failed)),
    }
}

/// A square icon button with `md` corners. `hover` is the hover fill for the
/// surface it sits on. The keyboard can focus it.
pub fn icon_button(
    id: impl Into<ElementId>,
    name: &str,
    box_size: Pixels,
    icon_size: Pixels,
    color: Hsla,
    hover: Hsla,
    t: &Tokens,
) -> Stateful<Div> {
    let primary = t.colors.text_primary;
    focusable(div().id(id), t)
        .size(box_size)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(t.radius.md)
        .cursor_pointer()
        .hover(move |s| s.bg(hover).text_color(primary))
        .child(icon(name, icon_size, color))
}

/// A small pill with a dot and a label: the worker of a task.
pub fn worker_pill(name: &str, account: &str, online: bool, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(t.space.xs + t.space.xxs)
        .py(t.space.xxs)
        .px(t.space.sm)
        .rounded(t.radius.full)
        .border(t.border_width)
        .border_color(c.border_subtle)
        .child(
            div()
                .size(t.space.xs + t.space.xxs)
                .rounded(t.radius.full)
                .when(online, |d| d.bg(c.status_done))
                .when(!online, |d| {
                    d.border(t.border_width).border_color(c.text_tertiary)
                }),
        )
        .child(
            div()
                .type_style(detail(t))
                .text_color(c.text_secondary)
                .whitespace_nowrap()
                .child(if account.is_empty() {
                    name.to_owned()
                } else {
                    format!("{name}, {account}")
                }),
        )
}

/// Label size with regular weight: details, meta lines and pills.
pub fn detail(t: &Tokens) -> Type {
    Type {
        weight: t.text.caption.weight,
        ..t.text.label
    }
}

/// A multi-line text input with no frame, in one text style. The frame is the
/// caller's.
pub fn textarea(
    state: &gpui_kit::Entity<gpui_kit::component::input::TextareaState>,
    style: Type,
) -> Div {
    div()
        .w_full()
        .type_style(style)
        .child(gpui_kit::base::input::Textarea::new(state))
}

/// A one-line text input with no frame, in one text style.
pub fn input(state: &gpui_kit::Entity<gpui_kit::component::input::InputState>, style: Type) -> Div {
    div()
        .w_full()
        .type_style(style)
        .child(gpui_kit::base::input::Input::new(state))
}

/// The editor colors of every text input: `text.tertiary` placeholders,
/// the `accent.text` caret and the theme's selection.
pub fn editor_style(t: &Tokens) -> gpui_kit::base::input::InputEditorStyle {
    let c = &t.colors;
    gpui_kit::base::input::InputEditorStyle {
        foreground: c.text_primary,
        muted_foreground: c.text_tertiary,
        selection: c.selection,
        caret: c.accent_text,
        border: c.border_strong,
        background: c.canvas_clear,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review cycle 3, finding 9: the Working glyph asked for a new frame at
    /// every display refresh, also while the window sat behind other windows.
    #[test]
    fn the_spin_timer_runs_only_for_a_shown_glyph_in_an_active_window() {
        assert!(spin_runs(true, true, false));
        assert!(!spin_runs(false, true, false), "no Working glyph on screen");
        assert!(!spin_runs(true, false, false), "the window is not active");
        assert!(
            !spin_runs(true, true, true),
            "the system asks for less motion"
        );
    }

    #[test]
    fn the_glyph_turns_in_steps_that_one_timer_makes() {
        let period = Duration::from_millis(1200);
        assert_eq!(steps_per_turn(period), 18);
        // 15 steps a second, not a frame at every display refresh.
        let interval = step_interval(period);
        assert!(interval > Duration::from_millis(60) && interval < Duration::from_millis(70));
        let spin = Spin::default();
        for _ in 0..18 {
            spin.advance(period);
        }
        assert!(
            spin.turn() < 1e-3 || (1.0 - spin.turn()) < 1e-3,
            "{}",
            spin.turn()
        );
        spin.advance(period);
        assert!((spin.turn() - 1.0 / 18.0).abs() < 1e-3);
        // A very short period still takes a few steps.
        assert_eq!(steps_per_turn(Duration::from_millis(10)), 4);
    }

    #[test]
    fn a_frame_notes_whether_it_drew_a_working_glyph() {
        let spin = Spin::default();
        assert!(!spin.used());
        spin.used.set(true);
        assert!(spin.used());
        spin.begin_frame();
        assert!(!spin.used());
    }
}
