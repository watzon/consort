//! Small parts that every surface uses: icons, status glyphs, buttons and
//! text styles. Sizes and colors come from the theme tokens only.

use consort_client::{ProviderLogo, TaskStatus};
use consort_theme::{Tokens, Type, TypeExt as _};

use gpui_kit::{
    AnyElement, App, Div, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled, Svg, base::SelectableText,
    div, img, prelude::FluentBuilder as _, svg,
};

/// An icon from `assets/icons`, in version 0. An old Lucide name gives its
/// nearest icon (see [`crate::ui::icons::resolve`]).
pub fn icon(name: &str, size: Pixels, color: Hsla) -> Svg {
    icon_version(name, 0, size, color)
}

/// An icon in one of its three hand-drawn versions, for the shimmer.
pub fn icon_version(name: &str, version: usize, size: Pixels, color: Hsla) -> Svg {
    svg()
        .path(crate::ui::icons::path(name, version))
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

/// The logo of a provider. Claude Code and Gemini CLI keep their own
/// colors. Codex, OpenCode and the ACP registry icons draw in `tint`, like
/// an icon. A provider with no logo (or a registry icon that has not
/// loaded) gets the neutral bot glyph, so a provider never shows as text
/// alone.
pub fn provider_logo(logo: &ProviderLogo, size: Pixels, tint: Hsla) -> AnyElement {
    match logo {
        ProviderLogo::ClaudeCode => img("providers/claude-code.svg")
            .size(size)
            .flex_none()
            .into_any_element(),
        ProviderLogo::GeminiCli => img("providers/gemini-cli.svg")
            .size(size)
            .flex_none()
            .into_any_element(),
        ProviderLogo::Codex => mono_logo("providers/codex.svg".into(), size, tint),
        ProviderLogo::OpenCode => mono_logo("providers/opencode.svg".into(), size, tint),
        ProviderLogo::Registry { id, svg: Some(svg) } => {
            mono_logo(crate::assets::remote_icon(id, svg), size, tint)
        }
        ProviderLogo::Registry { svg: None, .. } | ProviderLogo::Generic => {
            icon("bot", size, tint).into_any_element()
        }
    }
}

fn mono_logo(path: SharedString, size: Pixels, tint: Hsla) -> AnyElement {
    svg()
        .path(path)
        .size(size)
        .flex_none()
        .text_color(tint)
        .into_any_element()
}

/// A worker tag as a small chip: `label` text, `sm` corners. A tag that a
/// channel requires (`strong`) is in the accent, the way the designs mark
/// the tags that decide where work runs.
pub fn tag_chip(tag: &str, strong: bool, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(t.space.xs)
        .py(t.space.xxs)
        .pl(t.space.sm)
        .pr(t.space.sm)
        .rounded(t.radius.sm)
        .border(t.border_width)
        .when(strong, |d| d.bg(c.tint_accent).border_color(c.accent_text))
        .when(!strong, |d| d.border_color(c.border_strong))
        .child(
            div()
                .type_style(if strong { t.text.label } else { detail(t) })
                .text_color(if strong {
                    c.accent_text
                } else {
                    c.text_secondary
                })
                .whitespace_nowrap()
                .child(tag.to_owned()),
        )
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

/// The glyph of a task status. Working, Needs input, Ready for review,
/// Failed and Scheduled use the hand-drawn icons of the set. Up next (a
/// dashed ring) and Resolved (a solid disc) keep Consort's own glyphs,
/// because the set has nothing like them. A live status (Working or Needs
/// input) shimmers through its three versions at 8 frames a second (see
/// [`crate::ui::motion`]). It stands still under Reduce motion.
pub fn status_glyph(
    id: impl Into<ElementId>,
    status: TaskStatus,
    size: Pixels,
    t: &Tokens,
    cx: &App,
) -> AnyElement {
    let _ = id.into();
    let color = status_color(status, t);
    let live = matches!(status, TaskStatus::Working | TaskStatus::NeedsInput);
    let Some(name) = status_icon_name(status) else {
        let glyph_name = match status {
            TaskStatus::Resolved => "status-resolved",
            _ => "status-up-next",
        };
        return glyph(glyph_name, size, color).into_any_element();
    };
    let version = crate::ui::motion::shimmer_version(live, cx);
    let glyph = icon_version(name, version, size, color);
    if status == TaskStatus::Working {
        // The shimmer is too small to see at this size. The turn shows the work.
        let angle = crate::ui::motion::spinner_angle(live, cx);
        return glyph
            .with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(angle)))
            .into_any_element();
    }
    glyph.into_any_element()
}

/// The icon of a status, or `None` for the two statuses that have a Consort
/// glyph (Up next and Resolved). Scheduled work is routine work: it uses the
/// `routine` metronome in the scheduled color. The `clock` icon is for plain
/// times.
pub fn status_icon_name(status: TaskStatus) -> Option<&'static str> {
    match status {
        TaskStatus::UpNext | TaskStatus::Resolved => None,
        TaskStatus::Scheduled => Some("routine"),
        TaskStatus::Working => Some("working"),
        TaskStatus::NeedsInput => Some("needs-input"),
        TaskStatus::Review => Some("done"),
        TaskStatus::Failed => Some("failed"),
    }
}

/// The corners of a card. A card in the chat (`chat`) has
/// `chat.bubble_radius` corners, with `chat.bubble_tail` at the top left
/// where its task line meets it. Other cards have `lg` corners.
pub fn card_corners(card: Div, chat: bool, t: &Tokens) -> Div {
    if chat {
        card.rounded(t.component.chat.bubble_radius)
            .rounded_tl(t.component.chat.bubble_tail)
    } else {
        card.rounded(t.radius.lg)
    }
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
    /// `status.failed` fill: the confirm button of a dialog that destroys
    /// something.
    DangerFill,
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
    button_shell(id, kind, t)
        .whitespace_nowrap()
        .child(label.into())
}

/// A button whose label wraps inside it, for a label that comes from outside,
/// such as a harness's own words. The caller gives it a width.
pub fn wrapping_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    t: &Tokens,
) -> Stateful<Div> {
    button_shell(id, kind, t)
        .whitespace_normal()
        .text_center()
        .child(div().w_full().min_w_0().child(label.into()))
}

fn button_shell(id: impl Into<ElementId>, kind: ButtonKind, t: &Tokens) -> Stateful<Div> {
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
        .cursor_pointer();
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
        ButtonKind::DangerFill => {
            let fill = c.status_failed;
            let dim = t.opacity.danger_hover;
            base.py(pad)
                .bg(fill)
                .text_color(c.on_failed)
                .hover(move |s| s.bg(fill).opacity(dim))
        }
    }
}

/// A link that leaves the place where the user is, such as "Show in channel"
/// or "Show task": an underlined label and the `external-link` icon. Every
/// link that leaves Settings uses it, so the user can tell it from a control
/// that stays. The caller adds the click handler. The keyboard can focus it.
pub fn leaving_link(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    t: &Tokens,
) -> Stateful<Div> {
    let c = &t.colors;
    focusable(div().id(id), t)
        .flex()
        .flex_none()
        .items_center()
        .gap(t.space.xs + t.space.xxs)
        .cursor_pointer()
        .whitespace_nowrap()
        .child(
            div()
                .type_style(t.text.ui)
                .text_color(c.text_primary)
                .underline()
                .child(label.into()),
        )
        .child(icon("external-link", t.icon.dense, c.text_secondary))
}

/// A secondary button with an icon before its label, such as Upload or
/// Save.
pub fn icon_text_button(
    id: impl Into<ElementId>,
    icon_name: &str,
    label: impl Into<SharedString>,
    t: &Tokens,
) -> Stateful<Div> {
    let c = &t.colors;
    focusable(div().id(id), t)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(t.space.xs + t.space.xxs)
        .py(t.space.xs)
        .px(t.space.md)
        .rounded(t.radius.md)
        .bg(c.surface_raised)
        .border(t.border_width)
        .border_color(c.border_strong)
        .type_style(t.text.ui_strong)
        .text_color(c.text_primary)
        .cursor_pointer()
        .whitespace_nowrap()
        .hover(|s| s.bg(c.hover_raised))
        .active(|s| s.bg(c.pressed_raised))
        .child(icon(icon_name, t.icon.dense, c.text_secondary))
        .child(label.into())
}

/// A square icon button with `md` corners. `hover` is the hover fill for the
/// surface it sits on. The keyboard can focus it.
/// An icon in a square button. While the pointer is on it, the icon
/// shimmers through its three versions (not under Reduce motion).
#[allow(clippy::too_many_arguments)]
pub fn icon_button(
    id: impl Into<ElementId>,
    name: &str,
    box_size: Pixels,
    icon_size: Pixels,
    color: Hsla,
    hover: Hsla,
    t: &Tokens,
    cx: &App,
) -> Stateful<Div> {
    let primary = t.colors.text_primary;
    let id: ElementId = id.into();
    let hovered = crate::ui::motion::is_hovered(&id, cx);
    let version = crate::ui::motion::shimmer_version(hovered, cx);
    let hover_id = id.clone();
    focusable(div().id(id), t)
        .on_hover(move |on, window, cx| {
            if let Some(clock) = cx.try_global::<crate::ui::motion::FrameClock>() {
                clock.set_hovered(hover_id.clone(), *on);
            }
            window.refresh();
        })
        .size(box_size)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(t.radius.md)
        .cursor_pointer()
        .hover(move |s| s.bg(hover).text_color(primary))
        .child(icon_version(name, version, icon_size, color))
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

/// The header of every right panel: one height, the same insets and a line
/// under it. A header with Back at its left sets a smaller left inset.
pub fn panel_header(t: &Tokens) -> Div {
    use crate::ui::window_controls::DragArea as _;
    div()
        .debug_selector(|| "panel-header".into())
        .flex()
        .flex_none()
        .items_center()
        .gap(t.space.xs)
        .h(t.component.sidebar.title_bar)
        .pl(t.space.lg)
        .pr(t.space.sm)
        .border_b(t.border_width)
        .border_color(t.colors.border_subtle)
        .drag_area()
}

/// The row of tabs under a panel header.
pub fn panel_tabs(tabs: Div, t: &Tokens) -> Div {
    div()
        .debug_selector(|| "panel-tabs".into())
        .flex()
        .flex_none()
        .px(t.space.lg)
        .pt(t.space.md)
        .pb(t.space.xs)
        .child(tabs)
}

#[cfg(test)]
mod tests {

    #[test]
    fn scheduled_work_uses_the_routine_icon_and_not_the_clock() {
        use consort_client::TaskStatus;
        assert_eq!(
            super::status_icon_name(TaskStatus::Scheduled),
            Some("routine")
        );
        assert_eq!(super::status_icon_name(TaskStatus::UpNext), None);
        assert_eq!(super::status_icon_name(TaskStatus::Resolved), None);
        for status in [
            TaskStatus::Working,
            TaskStatus::NeedsInput,
            TaskStatus::Review,
            TaskStatus::Failed,
        ] {
            assert_ne!(super::status_icon_name(status), Some("clock"));
        }
    }

    #[test]
    fn a_status_glyph_uses_an_icon_of_the_set_or_a_consort_glyph() {
        use gpui_kit::AssetSource as _;
        let assets = crate::assets::Assets;
        for name in ["routine", "working", "needs-input", "done", "failed"] {
            for v in 0..3 {
                let p = crate::ui::icons::path(name, v);
                assert!(assets.load(&p).ok().flatten().is_some(), "{p}");
            }
        }
        for glyph in ["glyphs/status-up-next.svg", "glyphs/status-resolved.svg"] {
            assert!(assets.load(glyph).ok().flatten().is_some(), "{glyph}");
        }
    }
}
