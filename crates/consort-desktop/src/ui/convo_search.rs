//! The search palette (Paper `A0N-0` and `AFG-0`): the scope filter, Go to
//! (channels and tasks, at once), then messages, memory notes and tasks
//! from the hub, with skeleton rows while the hub searches and an empty
//! state that asks Tutti to search.

use consort_client::convo::{
    SearchHitView, SearchLoad, SearchScope, SearchSection, highlight, query_words, search_footer,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    ui::{
        parts::{ButtonKind, button, detail, icon, line, status_color, status_glyph, text},
        switcher::Hit,
    },
};

/// Text with the query words in bold `text.primary`, the rest in `color`.
fn marked(
    text_value: &str,
    words: &[String],
    style: consort_theme::Type,
    color: gpui_kit::Hsla,
    t: &Tokens,
) -> Div {
    let mut row = div()
        .flex()
        .flex_wrap()
        .min_w_0()
        .type_style(style)
        .text_color(color);
    for (run, bold) in highlight(text_value, words) {
        let run = run.replace(' ', "\u{a0}");
        row = row.child(if bold {
            div()
                .font_weight(t.text.title.weight)
                .text_color(t.colors.text_primary)
                .child(run)
        } else {
            div().child(run)
        });
    }
    row
}

fn section_label(label: &str, t: &Tokens) -> Div {
    div()
        .px(t.space.sm)
        .pt(t.space.md)
        .pb(t.space.xs)
        .child(text(label.to_owned(), t.text.label, t.colors.text_tertiary))
}

fn divider(t: &Tokens) -> Div {
    div()
        .mx(t.space.sm)
        .mt(t.space.sm)
        .h(t.border_width)
        .bg(t.colors.border_subtle)
}

/// A skeleton row while the hub searches: a dot and two bars.
fn skeleton(t: &Tokens, wide: f32) -> Div {
    let c = &t.colors;
    let v = t.component.convo;
    div()
        .flex()
        .gap(t.space.md)
        .px(t.space.sm)
        .py(t.space.sm)
        .child(
            div()
                .size(v.skeleton_dot)
                .rounded(t.radius.full)
                .bg(c.surface_card),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(t.space.sm)
                .child(
                    div()
                        .w(v.skeleton_title)
                        .h(v.skeleton_bar)
                        .rounded(t.radius.full)
                        .bg(c.surface_card),
                )
                .child(
                    div()
                        .w(v.skeleton_line * wide)
                        .h(v.skeleton_bar)
                        .rounded(t.radius.full)
                        .bg(c.surface_card),
                ),
        )
}

/// The palette.
pub fn render_search(app: &mut ConsortApp, t: &Tokens, cx: &mut Context<ConsortApp>) -> AnyElement {
    let c = &t.colors;
    let v = t.component.convo;
    let query = app.inputs.search.read(cx).value().trim().to_owned();
    let words = query_words(&query);
    let local = app.switcher_hits(cx);
    let hub = app.hub_hits(cx);
    let load = if words.is_empty() {
        SearchLoad::Idle
    } else if app.state.convo.search.query.trim() != query
        || app.state.convo.search.scope != app.convo.search_scope
    {
        // The key press waits for its pause: the hub search is on its way.
        SearchLoad::Loading
    } else {
        app.state.convo.search.load.clone()
    };
    let total = local.len() + hub.len();
    let selected = app.switcher.unwrap_or(0).min(total.saturating_sub(1));

    // The scope filter.
    let mut scopes = div().flex().items_center().gap(t.space.xs);
    for scope in SearchScope::ALL {
        let active = scope == app.convo.search_scope;
        scopes = scopes.child(
            div()
                .id(SharedString::from(format!("scope-{}", scope.label())))
                .px(t.space.md)
                .py(t.space.xs)
                .rounded(t.radius.md)
                .cursor_pointer()
                .type_style(if active { t.text.ui_strong } else { t.text.ui })
                .text_color(if active {
                    c.text_primary
                } else {
                    c.text_secondary
                })
                .when(active, |d| d.bg(c.surface_selected))
                .when(!active, |d| d.hover(|s| s.bg(c.hover_raised)))
                .child(scope.label())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.convo.search_scope = scope;
                    this.switcher = Some(0);
                    this.run_search(cx);
                    cx.notify();
                })),
        );
    }

    let mut list = div().flex().flex_col().px(t.space.sm).pb(t.space.sm);
    let mut row_index = 0usize;
    if !local.is_empty() {
        list = list.child(section_label("Go to", t));
        for hit in &local {
            let is_selected = row_index == selected;
            let pick = hit.clone();
            let quiet = if is_selected {
                c.text_secondary
            } else {
                c.text_tertiary
            };
            let (leading, name, detail_line, right): (AnyElement, String, Option<String>, Div) =
                match hit {
                    Hit::Channel {
                        name,
                        icon: icon_name,
                        tutti,
                        detail: d,
                        ..
                    } => (
                        if *tutti {
                            crate::ui::mark::still_mark(t.icon.size, t)
                        } else {
                            icon(icon_name, t.icon.size, c.text_secondary).into_any_element()
                        },
                        name.clone(),
                        Some(d.clone()),
                        div().child(text("Channel", detail(t), quiet)),
                    ),
                    Hit::Task {
                        id,
                        title,
                        status,
                        detail: d,
                        ..
                    } => (
                        status_glyph(
                            SharedString::from(format!("search-{id}")),
                            *status,
                            t.icon.size,
                            t,
                            cx,
                        ),
                        title.clone(),
                        None,
                        div()
                            .flex()
                            .gap(t.space.xs)
                            .child(text(format!("Task in {d},"), detail(t), quiet))
                            .child(text(
                                if status.is_your_turn() {
                                    "Needs you".to_owned()
                                } else {
                                    status.label().to_owned()
                                },
                                detail(t),
                                if status.is_your_turn() {
                                    c.accent_text
                                } else {
                                    status_color(*status, t)
                                },
                            )),
                    ),
                };
            list =
                list.child(
                    div()
                        .id(("search-local", row_index))
                        .flex()
                        .items_center()
                        .gap(t.space.md)
                        .h(v.search_row)
                        .px(t.space.sm)
                        .rounded(t.radius.md)
                        .cursor_pointer()
                        .when(is_selected, |d| d.bg(c.surface_selected))
                        .when(!is_selected, |d| d.hover(|s| s.bg(c.hover_raised)))
                        .child(leading)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(t.space.sm)
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .child(
                                    marked(&name, &words, t.text.ui_strong, c.text_primary, t)
                                        .flex_none(),
                                )
                                .when_some(detail_line, |d, line_text| {
                                    d.child(
                                        marked(&line_text, &words, t.text.ui, quiet, t)
                                            .overflow_hidden(),
                                    )
                                }),
                        )
                        .child(right.flex_none())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.go_to(pick.clone(), window, cx)
                        })),
                );
            row_index += 1;
        }
    }

    match &load {
        SearchLoad::Loading => {
            if !local.is_empty() {
                list = list.child(divider(t));
            }
            list = list
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.sm)
                        .px(t.space.sm)
                        .pt(t.space.md)
                        .pb(t.space.xs)
                        .child(status_glyph(
                            "search-spinner",
                            consort_client::TaskStatus::Working,
                            t.icon.dense,
                            t,
                            cx,
                        ))
                        .child(text(
                            "Searching messages, tasks and memory on the hub",
                            detail(t),
                            c.text_secondary,
                        )),
                )
                .child(skeleton(t, 1.0))
                .child(skeleton(t, 0.86))
                .child(skeleton(t, 0.92));
        }
        SearchLoad::Failed(message) => {
            list = list.child(div().px(t.space.sm).pt(t.space.md).child(text(
                message.clone(),
                t.text.ui,
                c.status_failed,
            )));
        }
        _ => {}
    }

    let mut last_section: Option<SearchSection> = None;
    for hit in &hub {
        if last_section != Some(hit.section) {
            if last_section.is_some() || !local.is_empty() {
                list = list.child(divider(t));
            }
            list = list.child(section_label(hit.section.label(), t));
            last_section = Some(hit.section);
        }
        let is_selected = row_index == selected;
        list = list.child(hub_row(hit, row_index, is_selected, &words, t, cx));
        row_index += 1;
    }

    let nothing = !words.is_empty() && load == SearchLoad::Done && total == 0;
    let body: AnyElement = if nothing {
        let ask = query.clone();
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(t.space.sm)
            .px(t.space.xl)
            .py(t.space.xl)
            .child(text(
                format!("Nothing matches \u{201C}{query}\u{201D}"),
                t.text.title,
                c.text_primary,
            ))
            .child(
                text(
                    "No channel, task, message or memory note has these words. Try fewer words, or ask the assistant to look for you.",
                    t.text.ui,
                    c.text_secondary,
                )
                .text_center(),
            )
            .child(
                button("search-ask-tutti", "Ask Tutti to search", ButtonKind::Secondary, t)
                    .mt(t.space.sm)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.ask_tutti_to_search(ask.clone(), window, cx)
                    })),
            )
            .into_any_element()
    } else {
        div()
            .id("search-results")
            .max_h(v.search_results_max)
            .overflow_y_scroll()
            .child(list)
            .into_any_element()
    };
    let footer_left = match &load {
        SearchLoad::Loading => "Channels and tasks show at once. The hub adds the rest.",
        _ => "Enter opens the result in place. Up and Down move.",
    };
    let footer_right = if words.is_empty() {
        String::new()
    } else if matches!(load, SearchLoad::Loading) {
        "Searching the hub".to_owned()
    } else {
        search_footer(&app.state.convo.search)
    };

    let panel = div()
        .id("switcher")
        .debug_selector(|| "search-palette".into())
        .occlude()
        .flex()
        .flex_col()
        .w(v.search_width)
        .bg(c.surface_raised)
        .border(t.border_width)
        .border_color(c.border_strong)
        .rounded(t.radius.xl)
        .shadow(t.shadow_float())
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .key_context(crate::convo::SEARCH_CONTEXT)
        .on_action(
            cx.listener(|this, _: &crate::convo::ScopeNext, _, cx| this.step_scope(false, cx)),
        )
        .on_action(
            cx.listener(|this, _: &crate::convo::ScopePrev, _, cx| this.step_scope(true, cx)),
        )
        .capture_key_down(cx.listener(ConsortApp::switcher_key))
        .child(
            div()
                .flex()
                .items_center()
                .gap(t.space.md)
                .px(t.space.lg)
                .py(t.space.md)
                .child(icon("search", t.icon.size, c.text_secondary))
                .child(
                    div()
                        .flex_1()
                        .text_color(c.text_primary)
                        .child(crate::ui::parts::input(&app.inputs.search, t.text.body)),
                )
                .child(
                    div()
                        .px(t.space.xs + t.space.xxs)
                        .rounded(t.radius.sm)
                        .border(t.border_width)
                        .border_color(c.border_strong)
                        .type_style(detail(t))
                        .text_color(c.text_secondary)
                        .child("esc"),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(t.space.lg)
                .pb(t.space.md)
                .border_b(t.border_width)
                .border_color(c.border_subtle)
                .child(scopes)
                .child(text("Tab changes the scope", detail(t), c.text_tertiary)),
        )
        .child(body)
        .when(!nothing, |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(t.space.lg)
                    .py(t.space.sm)
                    .border_t(t.border_width)
                    .border_color(c.border_subtle)
                    .bg(c.surface_card)
                    .child(text(footer_left, detail(t), c.text_tertiary))
                    .child(text(footer_right, detail(t), c.text_tertiary)),
            )
        });
    div()
        .id("switcher-scrim")
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .flex()
        .items_start()
        .justify_center()
        .pt(t.layout.switcher_top)
        .bg(c.scrim)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.close_switcher(window, cx)),
        )
        .child(panel)
        .into_any_element()
}

/// One hub result: two lines for a message or a note, one for a task.
fn hub_row(
    hit: &SearchHitView,
    index: usize,
    selected: bool,
    words: &[String],
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> AnyElement {
    let c = &t.colors;
    let quiet = if selected {
        c.text_secondary
    } else {
        c.text_tertiary
    };
    let leading: AnyElement = if hit.tutti {
        crate::ui::mark::still_mark(t.icon.size, t)
    } else if let Some(status) = hit.status {
        status_glyph(
            SharedString::from(format!("search-hub-{index}")),
            status,
            t.icon.size,
            t,
            cx,
        )
    } else {
        icon(
            hit.icon.as_deref().unwrap_or("channel"),
            t.icon.size,
            c.text_secondary,
        )
        .into_any_element()
    };
    let title = if hit.section == SearchSection::Memory {
        line(hit.title.clone(), t.text.mono, c.text_primary).font_family(t.font_mono.clone())
    } else if hit.section == SearchSection::Tasks {
        marked(&hit.title, words, t.text.ui, c.text_primary, t)
    } else {
        line(hit.title.clone(), t.text.ui_strong, c.text_primary)
    };
    let pick = hit.clone();
    div()
        .id(("search-hub", index))
        .flex()
        .gap(t.space.md)
        .px(t.space.sm)
        .py(t.space.sm)
        .rounded(t.radius.md)
        .cursor_pointer()
        .when(selected, |d| d.bg(c.surface_selected))
        .when(!selected, |d| d.hover(|s| s.bg(c.hover_raised)))
        .child(div().flex().items_center().h(t.text.ui.line).child(leading))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(t.space.xxs)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.xs)
                        .child(title)
                        .when_some(hit.title_note.clone(), |d, note| {
                            d.child(text(note, t.text.ui, quiet))
                        })
                        .child(div().flex_1())
                        .child(text(hit.detail.clone(), detail(t), quiet)),
                )
                .when(!hit.snippet.is_empty(), |d| {
                    d.child(marked(&hit.snippet, words, t.text.ui, c.text_secondary, t))
                }),
        )
        .on_click(cx.listener(move |this, _, window, cx| this.open_hit(pick.clone(), window, cx)))
        .into_any_element()
}
