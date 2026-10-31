//! The task item view (Paper `CA5-0` and `CPJ-0`): one command's whole
//! output, or one file change as a diff, in the task panel widened to
//! 560 px. Output starts at the end, because the end of a command matters
//! most. A worker that is offline cannot give the output; the panel says so
//! and offers Try again.

use std::sync::Arc;

use consort_client::convo::{
    DiffLine, ItemDetailState, ItemDetailView, ItemKind, diff_lines, output_caption, output_lines,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, InteractiveElement, IntoElement, ParentElement,
    ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled, UniformListScrollHandle,
    Window, div, prelude::FluentBuilder as _, uniform_list,
};

use crate::{
    app::ConsortApp,
    ui::parts::{
        ButtonKind, button, detail, icon, icon_button, icon_text_button, line, status_glyph, text,
    },
};

/// The lines of the open item, split once.
pub enum ItemLines {
    Output(Vec<String>),
    Diff(Vec<DiffLine>),
}

/// The split lines of the open item and the text they came from, so a new
/// text splits again and the same text does not.
#[derive(Default)]
pub struct ItemCache {
    pub item_id: String,
    pub text: Option<Arc<String>>,
    pub lines: Option<Arc<ItemLines>>,
    pub scroll: UniformListScrollHandle,
}

impl ItemLines {
    fn len(&self) -> usize {
        match self {
            ItemLines::Output(l) => l.len(),
            ItemLines::Diff(l) => l.len(),
        }
    }
}

/// One row of a diff: the old and the new line numbers, the mark, and the
/// text, on the tint of its kind. The item view and the skill review use it.
pub fn diff_row(line: &DiffLine, gutter: gpui_kit::Pixels, t: &Tokens) -> gpui_kit::Div {
    let c = &t.colors;
    let number = |n: Option<u32>| {
        div()
            .flex_none()
            .w(gutter)
            .pr(t.space.sm)
            .text_right()
            .text_color(c.text_tertiary)
            .child(n.map(|n| n.to_string()).unwrap_or_default())
    };
    let row = div()
        .flex()
        .w_full()
        .px(t.space.sm)
        .font_family(t.font_mono.clone())
        .type_style(t.text.mono)
        .whitespace_nowrap();
    let mark = |m: &'static str, color| div().w(t.space.lg).flex_none().text_color(color).child(m);
    match line {
        DiffLine::File(path) => row
            .bg(c.surface_selected)
            .text_color(c.text_secondary)
            .child(div().w(gutter * 2.0 + t.space.lg).flex_none())
            .child(path.clone()),
        DiffLine::Hunk(h) => row
            .bg(c.tint_scheduled)
            .text_color(c.text_tertiary)
            .child(div().w(gutter * 2.0 + t.space.lg).flex_none())
            .child(h.clone()),
        DiffLine::Context { old, new, text } => row
            .child(number(Some(*old)))
            .child(number(Some(*new)))
            .child(mark(" ", c.text_tertiary))
            .child(div().text_color(c.text_primary).child(text.clone())),
        DiffLine::Added { new, text } => row
            .bg(c.tint_done)
            .child(number(None))
            .child(number(Some(*new)))
            .child(mark("+", c.status_done))
            .child(div().text_color(c.text_primary).child(text.clone())),
        DiffLine::Removed { old, text } => row
            .bg(c.tint_failed)
            .child(number(Some(*old)))
            .child(number(None))
            .child(mark("-", c.status_failed))
            .child(div().text_color(c.text_primary).child(text.clone())),
    }
}

impl ConsortApp {
    /// The split lines of the open item. A new item or a new text splits
    /// again, and the list scrolls to its end.
    fn item_lines(&mut self, item: &ItemDetailView) -> Option<Arc<ItemLines>> {
        let ItemDetailState::Loaded { text } = &item.state else {
            return None;
        };
        let cache = &mut self.convo.item_cache;
        let same = cache.item_id == item.item_id
            && cache.text.as_ref().is_some_and(|t| Arc::ptr_eq(t, text));
        if !same {
            let lines = match item.kind {
                ItemKind::Output => {
                    ItemLines::Output(output_lines(text).into_iter().map(str::to_owned).collect())
                }
                ItemKind::Diff => ItemLines::Diff(diff_lines(text)),
            };
            let count = lines.len();
            cache.item_id = item.item_id.clone();
            cache.text = Some(text.clone());
            cache.lines = Some(Arc::new(lines));
            cache.scroll = UniformListScrollHandle::new();
            // Output starts at the end. A diff starts at its top.
            if item.kind == ItemKind::Output && count > 0 {
                cache
                    .scroll
                    .scroll_to_item(count - 1, ScrollStrategy::Bottom);
            }
        }
        cache.lines.clone()
    }

    /// The task panel while it shows one item in full.
    pub fn render_item_view(
        &mut self,
        item: ItemDetailView,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let v = t.component.convo;
        let lines = self.item_lines(&item);
        let copy_label = match item.kind {
            ItemKind::Output => "Copy output",
            ItemKind::Diff => "Copy diff",
        };
        let copy_text = match &item.state {
            ItemDetailState::Loaded { text } => Some(text.clone()),
            _ => None,
        };
        let subtitle: AnyElement = match item.kind {
            ItemKind::Diff if item.total > 0 || item.lines_added + item.lines_removed > 0 => {
                let rest = item
                    .subtitle
                    .split_once(" removed")
                    .map(|(_, r)| r.to_owned())
                    .unwrap_or_default();
                let added = if item.lines_added == 1 {
                    "line"
                } else {
                    "lines"
                };
                div()
                    .flex()
                    .type_style(detail(t))
                    .text_color(c.text_secondary)
                    .child(
                        div()
                            .text_color(c.status_done)
                            .child(format!("{} {added} added", item.lines_added)),
                    )
                    .child(", ")
                    .child(
                        div()
                            .text_color(c.status_failed)
                            .child(format!("{} removed", item.lines_removed)),
                    )
                    .child(rest)
                    .into_any_element()
            }
            _ => div()
                .flex()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .when(item.subtitle.starts_with("Exit code"), |d| {
                    let ok = item.subtitle.starts_with("Exit code 0");
                    d.child(
                        div()
                            .size(t.space.xs + t.space.xxs)
                            .rounded(t.radius.full)
                            .bg(if ok { c.status_done } else { c.status_failed }),
                    )
                })
                .child(line(item.subtitle.clone(), detail(t), c.text_secondary))
                .into_any_element(),
        };
        let head = crate::ui::parts::panel_header(t)
            .pl(t.space.sm)
            .child(
                icon_button(
                    "item-back",
                    "chevron-left",
                    t.space.xxl,
                    t.icon.size,
                    c.text_secondary,
                    c.hover_sidebar,
                    t,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.close_task_item(cx))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        line(item.title.clone(), t.text.mono, c.text_primary)
                            .font_family(t.font_mono.clone()),
                    )
                    .child(subtitle),
            )
            .when_some(copy_text, |d, text| {
                d.child(
                    icon_text_button("item-copy", "copy", copy_label, t).on_click(
                        move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()))
                        },
                    ),
                )
            })
            .child(
                icon_button(
                    "item-close",
                    "x",
                    t.space.xxl,
                    t.icon.size,
                    c.text_secondary,
                    c.hover_sidebar,
                    t,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_task_item(cx);
                    this.close_panel(cx);
                })),
            );

        let worker = item.worker_name.clone();
        let state_box = |glyph: AnyElement,
                         title: String,
                         body: String,
                         retry: bool,
                         cx: &mut Context<Self>| {
            let reopen = item.clone();
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(t.space.sm)
                .size_full()
                .px(t.space.xl)
                .child(glyph)
                .child(text(title, t.text.ui_strong, c.text_primary))
                .child(text(body, detail(t), c.text_secondary).text_center())
                .when(retry, |d| {
                    d.child(
                        button("item-retry", "Try again", ButtonKind::Secondary, t)
                            .mt(t.space.sm)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_task_item(
                                    reopen.task_id.clone(),
                                    reopen.session_id.clone(),
                                    reopen.item_id.clone(),
                                    cx,
                                )
                            })),
                    )
                })
                .into_any_element()
        };
        let body: AnyElement = match (&item.state, lines) {
            (ItemDetailState::Loaded { .. }, Some(lines)) => {
                let count = lines.len();
                let gutter = v.line_number_col;
                let list_lines = lines.clone();
                let tokens = t.clone();
                uniform_list("item-lines", count, move |range, _, _| {
                    let t = &tokens;
                    let c = &t.colors;
                    range
                        .map(|ix| {
                            let number = |n: Option<u32>| {
                                div()
                                    .flex_none()
                                    .w(gutter)
                                    .pr(t.space.sm)
                                    .text_right()
                                    .text_color(c.text_tertiary)
                                    .child(n.map(|n| n.to_string()).unwrap_or_default())
                            };
                            let row = div()
                                .flex()
                                .w_full()
                                .px(t.space.sm)
                                .font_family(t.font_mono.clone())
                                .type_style(t.text.mono)
                                .whitespace_nowrap();
                            match list_lines.as_ref() {
                                ItemLines::Output(lines) => {
                                    row.child(number(u32::try_from(ix + 1).ok())).child(
                                        div().text_color(c.text_primary).child(lines[ix].clone()),
                                    )
                                }
                                ItemLines::Diff(lines) => diff_row(&lines[ix], gutter, t),
                            }
                        })
                        .collect()
                })
                .track_scroll(&self.convo.item_cache.scroll)
                .size_full()
                .py(t.space.sm)
                .into_any_element()
            }
            (ItemDetailState::Offline, _) => state_box(
                status_glyph(
                    SharedString::from("item-offline"),
                    consort_client::TaskStatus::UpNext,
                    t.icon.size,
                    t,
                    cx,
                ),
                format!("{worker} is offline"),
                format!(
                    "The full output stays on {worker}. It shows here when the worker is back online. The summary in the thread is still there."
                ),
                true,
                cx,
            ),
            (ItemDetailState::Failed(message), _) => state_box(
                icon("warning", t.icon.size, c.status_failed).into_any_element(),
                "Consort could not show the full text".to_owned(),
                message.clone(),
                true,
                cx,
            ),
            _ => state_box(
                status_glyph(
                    SharedString::from("item-loading"),
                    consort_client::TaskStatus::Working,
                    t.icon.size,
                    t,
                    cx,
                ),
                format!("Getting the output from {worker}"),
                "The worker keeps the full output. The hub keeps only the summary.".to_owned(),
                false,
                cx,
            ),
        };
        let caption = match (&item.state, item.kind) {
            (ItemDetailState::Loaded { .. }, ItemKind::Output) => output_caption(
                self.convo
                    .item_cache
                    .lines
                    .as_ref()
                    .map(|l| l.len())
                    .unwrap_or(0),
                &worker,
            ),
            (ItemDetailState::Loaded { .. }, ItemKind::Diff) => {
                format!("The diff comes from {worker}.")
            }
            _ => String::new(),
        };
        div()
            .id("item-view")
            .debug_selector(|| "item-view".into())
            .flex()
            .flex_col()
            .flex_none()
            .w(v.panel_wide)
            .h_full()
            .bg(c.surface_sidebar)
            .border_l(t.border_width)
            .border_color(c.border_subtle)
            .child(head)
            .when_some(item.check.as_ref().map(|c| check_rows(c, t)), |d, rows| {
                d.child(rows)
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .m(t.space.lg)
                    .rounded(t.radius.lg)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .bg(c.surface_canvas)
                    .overflow_hidden()
                    .child(body),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(t.space.md)
                    .px(t.space.lg)
                    .pb(t.space.md)
                    .child(line(caption, detail(t), c.text_tertiary).flex_1().min_w_0())
                    .children(
                        item.position_label()
                            .map(|p| text(p, detail(t), c.text_tertiary)),
                    ),
            )
            .into_any_element()
    }
}

/// The rows that say what the command check did with this command. The words
/// are selectable, and the labels are not.
fn check_rows(check: &consort_client::CheckDetailView, t: &Tokens) -> Div {
    let c = &t.colors;
    let row = |key: &'static str, label: &'static str, value: String| {
        div()
            .flex()
            .items_start()
            .gap(t.space.md)
            .child(
                text(label, detail(t), c.text_tertiary)
                    .flex_none()
                    .w(t.component.settings_label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .type_style(detail(t))
                    .text_color(c.text_secondary)
                    .child(gpui_kit::base::SelectableText::new(
                        SharedString::from(format!("item-check.{key}")),
                        value,
                    )),
            )
    };
    let mut rows = div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(t.space.xs)
        .px(t.space.lg)
        .pt(t.space.md)
        .debug_selector(|| "item-check".into())
        .child(row("check", "Check", check.check.clone()));
    if let Some(reason) = &check.reason {
        rows = rows.child(row("reason", "Reason", reason.clone()));
    }
    if let Some(rule) = &check.rule {
        rows = rows.child(row("rule", "Rule tested", rule.clone()));
    }
    if let Some(error) = &check.error {
        rows = rows.child(row("error", "Error", error.clone()));
    }
    if check.blocked {
        rows = rows.child(text(
            "To allow this command, tell the assistant in the chat.",
            detail(t),
            c.text_tertiary,
        ));
    }
    rows
}
