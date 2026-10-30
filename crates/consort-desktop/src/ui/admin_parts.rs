//! The parts that Settings > Hub, Settings > Clients and Settings >
//! Notifications share: one 760 px column with a header, section titles and
//! bordered row groups, as in the phase 3 designs.

use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, InteractiveElement as _, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _,
};

use crate::app::ConsortApp;
use crate::ui::menu::{MenuAction, MenuItem, OpenMenu};
use crate::ui::parts::{detail, icon, icon_button, selectable, text};

/// The scrolling page: 44 px above, the speaker gutter at the sides, and a
/// column of the settings width with 32 px between sections.
pub fn page(id: &'static str, t: &Tokens) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .h_full()
        .min_w_0()
        .overflow_y_scroll()
        .pt(t.space.page - t.space.xs)
        .px(t.layout.speaker_gutter)
        .pb(t.space.page)
}

/// The column inside the page.
pub fn column(t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(t.space.xxl)
        .w_full()
        .max_w(t.component.settings_detail_max)
}

/// The page title and its line, with a button at the right.
pub fn header(title: &str, line: Div, action: Option<AnyElement>, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .items_start()
        .gap(t.space.lg)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(t.space.xs + t.space.xxs)
                .child(text(title.to_owned(), t.text.heading, c.text_primary))
                .child(line),
        )
        .children(action.map(|a| div().mt(t.space.xxs).child(a)))
}

/// A section title: body size, title weight.
pub fn section_title(title: &str, t: &Tokens) -> Div {
    div()
        .type_style(t.text.body)
        .font_weight(t.text.title.weight)
        .text_color(t.colors.text_primary)
        .child(title.to_owned())
}

/// A bordered group of rows.
pub fn rows(t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .border(t.border_width)
        .border_color(t.colors.border_subtle)
        .rounded(t.radius.lg)
        .overflow_hidden()
}

/// One row of a group: 12 px above and below, 16 px at the sides. Every row
/// but the last has a line under it.
pub fn row(last: bool, t: &Tokens) -> Div {
    div()
        .flex()
        .items_center()
        .gap(t.space.lg)
        .py(t.space.md)
        .px(t.space.lg)
        .when(!last, |d| {
            d.border_b(t.border_width)
                .border_color(t.colors.border_subtle)
        })
}

/// The label column of a row.
pub fn row_label(label: &str, t: &Tokens) -> Div {
    text(label.to_owned(), t.text.ui_strong, t.colors.text_primary)
        .flex_none()
        .w(t.component.settings_label)
}

/// A value in mono that the user can select, such as a path or an address.
pub fn mono_value(id: impl Into<SharedString>, value: String, t: &Tokens) -> Div {
    div().font_family(t.font_mono.clone()).child(selectable(
        gpui_kit::ElementId::Name(id.into()),
        value,
        t.text.mono,
        t.colors.text_primary,
    ))
}

/// A hint under a value.
pub fn hint(content: impl Into<SharedString>, t: &Tokens) -> Div {
    text(content, detail(t), t.colors.text_tertiary)
}

/// A dot before a status line: solid when `ok`, an open ring otherwise.
pub fn status_dot(ok: Option<bool>, t: &Tokens) -> Div {
    let c = &t.colors;
    let dot = div()
        .size(t.component.worker_dot)
        .flex_none()
        .rounded(t.radius.full);
    match ok {
        Some(true) => dot.bg(c.status_done),
        Some(false) => dot.bg(c.status_failed),
        None => dot.border(t.border_width).border_color(c.text_tertiary),
    }
}

/// A copy button that puts `value` on the clipboard and says so.
pub fn copy_button(
    id: &'static str,
    value: String,
    what: &'static str,
    size: gpui_kit::Pixels,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    icon_button(
        id,
        "copy",
        size,
        t.icon.dense,
        c.text_secondary,
        c.hover_card,
        t,
        cx,
    )
    .on_click(cx.listener(move |this, _, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
        this.show_notice(format!("Copied the {what}."), cx);
    }))
}

/// A box that opens a select menu under it. `options` are `(label, option,
/// checked)`, and a pick is `MenuAction::Select(id, option)`.
pub fn select(
    id: &'static str,
    label: String,
    width: gpui_kit::Pixels,
    options: Vec<(String, String, bool)>,
    app: &ConsortApp,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .w(width)
        .py(t.space.xs)
        .pl(t.space.sm + t.space.xxs)
        .pr(t.space.sm)
        .rounded(t.radius.md)
        .border(t.border_width)
        .border_color(c.border_strong)
        .bg(c.surface_raised)
        .cursor_pointer()
        .hover(|s| s.bg(c.hover_raised))
        .child(app.triggers.track(id))
        .child(text(label, t.text.ui, c.text_primary).truncate())
        .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary))
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(bounds) = this.triggers.get(id) {
                let items = options
                    .iter()
                    .map(|(label, option, checked)| {
                        MenuItem::new(label.clone(), MenuAction::Select(id, option.clone()))
                            .checked(*checked)
                    })
                    .collect();
                this.open_menu(OpenMenu::under(bounds, items, false), cx);
            }
        }))
}
