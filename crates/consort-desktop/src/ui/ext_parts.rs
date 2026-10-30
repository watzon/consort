//! Parts that the connector and skill pages, their dialogs and the hub cards
//! share: service logos, the section and row frames, the Use in control,
//! the approval rule select, and a list that scrolls with fades.

use consort_client::{ApprovalRuleView, ServiceLogo, ToolKindView, UseInView};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels,
    ScrollHandle, SharedString, Stateful, StatefulInteractiveElement as _, Styled, div, img,
    linear_color_stop, linear_gradient, prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    extensions::{UseInPicker, UseInTarget},
    ui::{
        menu::{MenuAction, MenuItem, OpenMenu},
        parts::{detail, focusable, icon, text},
    },
};

/// A service logo: the image that the client loaded, or a monogram tile
/// until then (and when it does not load).
pub fn service_logo(logo: &ServiceLogo, size: Pixels, t: &Tokens) -> AnyElement {
    let c = &t.colors;
    match &logo.image {
        Some(image) => img(crate::assets::remote_image(&image.key, &image.bytes))
            .size(size)
            .flex_none()
            .into_any_element(),
        None => div()
            .size(size)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(t.radius.sm)
            .bg(c.surface_selected)
            .type_style(consort_theme::Type {
                size: size * 0.6,
                line: size,
                ..t.text.label
            })
            .text_color(c.text_secondary)
            .child(logo.monogram.clone())
            .into_any_element(),
    }
}

/// The tile around a service logo at the top of a page or a dialog.
pub fn logo_tile(logo: &ServiceLogo, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .size(t.component.connector_tile)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(t.radius.lg)
        .border(t.border_width)
        .border_color(c.border_strong)
        .bg(c.surface_card)
        .child(service_logo(logo, t.component.sign_in_logo, t))
}

/// A section of a settings page: a title, a line under it, and the body.
pub fn page_section(title: &str, description: &str, body: impl IntoElement, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .flex_col()
        .gap(t.space.md + t.space.xxs)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(t.space.xxs)
                .child(
                    div()
                        .type_style(t.text.body)
                        .font_weight(t.text.title.weight)
                        .text_color(c.text_primary)
                        .child(title.to_owned()),
                )
                .child(text(description.to_owned(), t.text.ui, c.text_tertiary)),
        )
        .child(body)
}

/// The frame of a list of rows: a `border.subtle` line, `lg` corners.
pub fn rows_frame(t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(t.radius.lg)
        .border(t.border_width)
        .border_color(t.colors.border_subtle)
}

/// One row of a settings frame: a title and a hint, then the control.
pub fn setting_row(
    title: &str,
    hint: impl IntoElement,
    control: impl IntoElement,
    last: bool,
    t: &Tokens,
) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .items_center()
        .gap(t.space.lg)
        .py(t.space.md)
        .px(t.space.lg)
        .when(!last, |d| {
            d.border_b(t.border_width).border_color(c.border_subtle)
        })
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(text(title.to_owned(), t.text.ui_strong, c.text_primary))
                .child(hint),
        )
        .child(div().flex_none().child(control))
}

/// A hint under a row title, in `text.tertiary`.
pub fn hint(content: impl Into<SharedString>, t: &Tokens) -> Div {
    text(content, detail(t), t.colors.text_tertiary)
}

/// A small chip with a tag, in the accent: the tags of a Use in.
pub fn scope_tag(tag: &str, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex_none()
        .px(t.space.xs + t.space.xxs)
        .rounded(t.radius.sm)
        .border(t.border_width)
        .border_color(c.accent_fill)
        .type_style(t.text.label)
        .font_weight(t.text.caption.weight)
        .text_color(c.accent_text)
        .child(tag.to_owned())
}

/// The trigger key of a Use in control.
fn use_in_key(target: &UseInTarget) -> &'static str {
    match target {
        UseInTarget::Connection(_) => "use-in-connection",
        UseInTarget::Skill(_) => "use-in-skill",
        UseInTarget::ConnectDialog => "use-in-connect",
        UseInTarget::AddSkillDialog => "use-in-add-skill",
    }
}

/// The Use in control: "Tutti only", "Every channel", and a button that opens
/// a dialog to choose channels or tags. The button shows the choice.
/// `wide` makes the segments fill the width, as in a dialog.
pub fn use_in_control(
    target: UseInTarget,
    current: &UseInView,
    wide: bool,
    t: &Tokens,
    app: &ConsortApp,
    cx: &mut Context<ConsortApp>,
) -> Div {
    let c = &t.colors;
    let key = use_in_key(&target);
    let tutti_id = app.state.sidebar.tutti.id.clone();
    let tutti = consort_client::use_in_is_only(current, &tutti_id);
    let every = matches!(current, UseInView::EveryChannel);
    let chosen = !tutti && !every;
    let tags: Vec<String> = match current {
        UseInView::Tags(tags) => tags.clone(),
        _ => Vec::new(),
    };
    let segment = |id: &'static str, active: bool| {
        let selector = format!("{key}-{id}");
        focusable(div().id(SharedString::from(selector.clone())), t)
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .justify_center()
            .gap(t.space.xs + t.space.xxs)
            .py(t.space.xs)
            .px(t.space.sm + t.space.xxs)
            .rounded(t.radius.sm)
            .cursor_pointer()
            .whitespace_nowrap()
            .when(wide, |d| d.flex_1())
            .when(active, |d| d.bg(c.surface_selected))
            .when(!active, |d| d.hover(|s| s.bg(c.hover_card)))
    };
    let label = |content: String, active: bool| {
        text(
            content,
            if active { t.text.ui_strong } else { t.text.ui },
            if active {
                c.text_primary
            } else {
                c.text_secondary
            },
        )
    };
    let tutti_target = target.clone();
    let every_target = target.clone();
    let choose_target = target.clone();
    let choose_label = if chosen {
        consort_client::use_in_choice_label(current, &app.state.channels)
    } else {
        "Choose channels or tags".to_owned()
    };
    div()
        .relative()
        .flex()
        .gap(t.space.xxs)
        .p(t.space.xxs)
        .bg(c.surface_card)
        .rounded(t.radius.md)
        .when(wide, |d| d.w_full())
        .child(
            segment("tutti", tutti)
                .child(label("Tutti only".to_owned(), tutti))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let id = this.state.sidebar.tutti.id.clone();
                    if !id.is_empty() {
                        this.set_use_in(tutti_target.clone(), UseInView::Channels(vec![id]), cx);
                    }
                })),
        )
        .child(
            segment("every", every)
                .child(label("Every channel".to_owned(), every))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_use_in(every_target.clone(), UseInView::EveryChannel, cx)
                })),
        )
        .child(
            segment("choose", chosen)
                .child(label(choose_label, chosen))
                .children(tags.iter().map(|tag| scope_tag(tag, t)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let current = this.use_in_of(&choose_target);
                    this.ext.use_in_picker =
                        Some(UseInPicker::new(choose_target.clone(), &current));
                    this.ext.use_in_scroll = gpui_kit::ScrollHandle::new();
                    cx.notify();
                })),
        )
}

/// The trigger key of an approval rule select.
fn rule_key(kind: ToolKindView, dialog: bool) -> &'static str {
    match (kind, dialog) {
        (ToolKindView::Read, false) => "rule-read",
        (ToolKindView::Change, false) => "rule-change",
        (ToolKindView::Delete, false) => "rule-delete",
        (ToolKindView::Read, true) => "connect-rule-read",
        (ToolKindView::Change, true) => "connect-rule-change",
        (ToolKindView::Delete, true) => "connect-rule-delete",
    }
}

/// The select of one approval rule. `connection` is `None` for the
/// Connect dialog.
pub fn rule_select(
    connection: Option<String>,
    kind: ToolKindView,
    rule: ApprovalRuleView,
    t: &Tokens,
    app: &ConsortApp,
    cx: &mut Context<ConsortApp>,
) -> Stateful<Div> {
    let c = &t.colors;
    let key = rule_key(kind, connection.is_none());
    focusable(div().id(key), t)
        .relative()
        .flex()
        .items_center()
        .justify_between()
        .w(t.component.group_select)
        .py(t.space.xs + t.space.xxs)
        .pl(t.space.md - t.space.xxs)
        .pr(t.space.sm + t.space.xxs)
        .rounded(t.radius.md)
        .border(t.border_width)
        .border_color(c.border_strong)
        .bg(c.surface_raised)
        .cursor_pointer()
        .hover(|s| s.border_color(c.text_tertiary))
        .child(app.triggers.track(key))
        .child(text(rule.label(), t.text.ui, c.text_primary))
        .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary))
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(bounds) = this.triggers.get(key) {
                let items = ApprovalRuleView::ALL
                    .iter()
                    .map(|r| {
                        MenuItem::new(r.label(), MenuAction::SetRule(connection.clone(), kind, *r))
                            .checked(*r == rule)
                    })
                    .collect();
                this.open_menu(OpenMenu::under(bounds, items, true), cx);
            }
        }))
}

/// A list that scrolls inside a fixed height and fades into `ground` at
/// the top and the bottom while more rows are there (AGENTS.md rule 3).
#[allow(clippy::too_many_arguments)]
pub fn faded_scroll(
    id: &'static str,
    handle: &ScrollHandle,
    height: Pixels,
    content: impl IntoElement,
    ground: Hsla,
    clear: Hsla,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> Div {
    faded_scroll_rows(
        id,
        handle,
        height,
        vec![content.into_any_element()],
        ground,
        clear,
        t,
        cx,
    )
}

/// [`faded_scroll`] with each row as its own child of the scroll, so
/// `ScrollHandle::scroll_to_item` can bring one row into view.
#[allow(clippy::too_many_arguments)]
pub fn faded_scroll_rows(
    id: &'static str,
    handle: &ScrollHandle,
    height: Pixels,
    rows: Vec<gpui_kit::AnyElement>,
    ground: Hsla,
    clear: Hsla,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> Div {
    let offset = handle.offset();
    let max = handle.max_offset();
    let above = offset.y < -Pixels::from(0.5);
    let below = -offset.y + Pixels::from(0.5) < max.y;
    let fade = t.space.xl;
    div()
        .relative()
        .h(height)
        .flex_none()
        .overflow_hidden()
        .child(
            div()
                .id(id)
                .size_full()
                .overflow_y_scroll()
                .track_scroll(handle)
                // The fades follow the scroll position.
                .on_scroll(cx.listener(|_, _, _, cx| cx.notify()))
                .children(rows),
        )
        // The list knows its height only after its first layout. When that
        // shows more rows below than this draw assumed, draw again, so the
        // bottom fade is there before the first scroll.
        .child({
            let handle = handle.clone();
            gpui_kit::canvas(
                move |_, window, _| {
                    let now_below = -handle.offset().y + Pixels::from(0.5) < handle.max_offset().y;
                    if now_below != below {
                        window.refresh();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_0()
        })
        .when(above, |d| {
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(fade)
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(ground, 0.),
                        linear_color_stop(clear, 1.),
                    )),
            )
        })
        .when(below, |d| {
            d.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(fade)
                    .bg(linear_gradient(
                        0.,
                        linear_color_stop(ground, 0.),
                        linear_color_stop(clear, 1.),
                    )),
            )
        })
}

/// A status dot of `component.worker_dot`.
pub fn dot(color: Hsla, t: &Tokens) -> Div {
    div()
        .size(t.component.worker_dot)
        .flex_none()
        .rounded(t.radius.full)
        .bg(color)
}

/// An open ring of `component.worker_dot`, for something that is off.
pub fn ring(color: Hsla, t: &Tokens) -> Div {
    div()
        .size(t.component.worker_dot)
        .flex_none()
        .rounded(t.radius.full)
        .border(t.border_width)
        .border_color(color)
}

/// A mono block that the user can select, for context text and code.
pub fn mono_block(id: impl Into<gpui_kit::ElementId>, content: String, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .py(t.space.md + t.space.xxs)
        .px(t.space.lg - t.space.xxs)
        .rounded(t.radius.lg)
        .border(t.border_width)
        .border_color(c.border_subtle)
        .bg(c.surface_card)
        .font_family(t.font_mono.clone())
        .child(crate::ui::parts::selectable(
            id,
            content,
            t.text.mono,
            c.text_secondary,
        ))
}
