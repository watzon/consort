//! The switcher (Command K): go to a channel or a task. It searches the view
//! state on this client, so it works offline and answers at once.

use consort_client::{ChannelKind, TaskStatus, ViewState};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    ui::parts::{detail, icon, line, status_glyph, text},
};

/// The most rows the switcher shows.
const MAX_ROWS: usize = 10;

/// One result.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Channel {
        id: String,
        name: String,
        icon: String,
        everything: bool,
        detail: String,
    },
    Task {
        id: String,
        channel_id: String,
        title: String,
        status: TaskStatus,
        detail: String,
    },
}

/// How well `text` matches `query`: 3 for a start, 2 for a word start, 1
/// anywhere, 0 for no match. Both are lowercase.
fn score(text: &str, query: &str) -> u8 {
    let text = text.to_lowercase();
    if text.starts_with(query) {
        3
    } else if text
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(query))
    {
        2
    } else if text.contains(query) {
        1
    } else {
        0
    }
}

/// The results for `query`: channels first, then tasks, best match first.
/// With no query: every channel, then the tasks that need the user.
pub fn search(state: &ViewState, query: &str) -> Vec<Hit> {
    let query = query.trim().to_lowercase();
    let mut channels: Vec<(u8, usize, Hit)> = state
        .channels
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let s = if query.is_empty() {
                1
            } else {
                score(&c.name, &query).max(score(&c.purpose, &query).saturating_sub(1))
            };
            (s > 0).then(|| {
                (
                    s,
                    i,
                    Hit::Channel {
                        id: c.id.clone(),
                        name: c.name.clone(),
                        icon: c.icon.clone(),
                        everything: c.kind == ChannelKind::Everything,
                        detail: c.purpose.clone(),
                    },
                )
            })
        })
        .collect();
    let mut tasks: Vec<(u8, usize, Hit)> = state
        .tasks
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let s = if query.is_empty() {
                u8::from(t.status.is_your_turn())
            } else {
                score(&t.title, &query).max(score(&t.channel_name, &query).saturating_sub(1))
            };
            // A resolved task ranks below an open one with the same match.
            let s = if t.status == TaskStatus::Resolved {
                s.min(1)
            } else {
                s
            };
            (s > 0).then(|| {
                (
                    s,
                    i,
                    Hit::Task {
                        id: t.id.clone(),
                        channel_id: t.channel_id.clone(),
                        title: t.title.clone(),
                        status: t.status,
                        detail: t.channel_name.clone(),
                    },
                )
            })
        })
        .collect();
    channels.sort_by_key(|(s, i, _)| (std::cmp::Reverse(*s), *i));
    tasks.sort_by_key(|(s, i, _)| (std::cmp::Reverse(*s), *i));
    let mut out: Vec<Hit> = channels.into_iter().map(|(_, _, h)| h).collect();
    out.truncate(if query.is_empty() {
        MAX_ROWS
    } else {
        MAX_ROWS / 2 + 1
    });
    let room = MAX_ROWS.saturating_sub(out.len());
    out.extend(tasks.into_iter().take(room).map(|(_, _, h)| h));
    out
}

impl ConsortApp {
    pub fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.switcher = Some(0);
        self.inputs.search.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    pub fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.switcher = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn switcher_hits(&self, cx: &gpui_kit::App) -> Vec<Hit> {
        let query = self.inputs.search.read(cx).value().to_string();
        search(&self.state, &query)
    }

    fn go_to(&mut self, hit: Hit, window: &mut Window, cx: &mut Context<Self>) {
        self.close_switcher(window, cx);
        match hit {
            Hit::Channel { id, .. } => self.open_channel(id, window, cx),
            Hit::Task { id, channel_id, .. } => {
                self.open_channel(channel_id, window, cx);
                self.open_thread(id, window, cx);
            }
        }
    }

    fn switcher_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selected) = self.switcher else {
            return;
        };
        let hits = self.switcher_hits(cx);
        match event.keystroke.key.as_str() {
            "escape" => self.close_switcher(window, cx),
            "down" if !hits.is_empty() => self.switcher = Some((selected + 1) % hits.len()),
            "up" if !hits.is_empty() => {
                self.switcher = Some((selected + hits.len() - 1) % hits.len())
            }
            "enter" => {
                if let Some(hit) = hits.get(selected).cloned() {
                    self.go_to(hit, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub fn render_switcher(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let hits = self.switcher_hits(cx);
        let selected = self.switcher.unwrap_or(0).min(hits.len().saturating_sub(1));
        let query = self.inputs.search.read(cx).value().trim().to_owned();
        let mut list = div().flex().flex_col().p(t.space.xs);
        let mut last_kind: Option<bool> = None;
        for (i, hit) in hits.iter().enumerate() {
            let is_task = matches!(hit, Hit::Task { .. });
            if last_kind != Some(is_task) {
                list = list.child(
                    div()
                        .px(t.space.sm)
                        .pt(if i == 0 { t.space.xs } else { t.space.sm })
                        .pb(t.space.xs)
                        .child(text(
                            if is_task { "Tasks" } else { "Channels" },
                            t.text.label,
                            c.text_tertiary,
                        )),
                );
                last_kind = Some(is_task);
            }
            let is_selected = i == selected;
            // `text.tertiary` never sits on `surface.selected`.
            let quiet = if is_selected {
                c.text_secondary
            } else {
                c.text_tertiary
            };
            let (leading, name, detail_text) = match hit {
                Hit::Channel {
                    name,
                    icon: icon_name,
                    everything,
                    detail: d,
                    ..
                } => (
                    if *everything {
                        crate::ui::mark::still_mark(t.icon.size, t)
                    } else {
                        icon(icon_name, t.icon.size, c.text_secondary).into_any_element()
                    },
                    name.clone(),
                    d.clone(),
                ),
                Hit::Task {
                    id,
                    title,
                    status,
                    detail: d,
                    ..
                } => (
                    status_glyph(
                        SharedString::from(format!("switcher-{id}")),
                        *status,
                        t.icon.size,
                        t,
                        cx,
                    ),
                    title.clone(),
                    d.clone(),
                ),
            };
            let pick = hit.clone();
            list =
                list.child(
                    div()
                        .id(("switcher-row", i))
                        .flex()
                        .items_center()
                        .gap(t.space.md)
                        .h(t.space.xxl + t.space.xs)
                        .px(t.space.sm)
                        .rounded(t.radius.md)
                        .cursor_pointer()
                        .when(is_selected, |d| d.bg(c.surface_selected))
                        .when(!is_selected, |d| d.hover(|s| s.bg(c.hover_raised)))
                        .child(leading)
                        .child(
                            line(name, t.text.ui, c.text_primary)
                                .flex_none()
                                .max_w(t.layout.switcher_name_max),
                        )
                        .child(line(detail_text, t.text.ui, quiet).flex_1())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.go_to(pick.clone(), window, cx)
                        })),
                );
        }
        if hits.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xxs)
                    .p(t.space.md)
                    .child(text(
                        format!("Nothing matches \u{201C}{query}\u{201D}"),
                        t.text.ui_strong,
                        c.text_primary,
                    ))
                    .child(text(
                        "Try a shorter word, or the name of a channel.",
                        t.text.ui,
                        c.text_secondary,
                    )),
            );
        }
        let panel = div()
            .id("switcher")
            .occlude()
            .flex()
            .flex_col()
            .w(t.layout.switcher_width)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.xl)
            .shadow(t.shadow_float())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(Self::switcher_key))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .px(t.space.lg)
                    .py(t.space.md)
                    .border_b(t.border_width)
                    .border_color(c.border_subtle)
                    .child(icon("search", t.icon.size, c.text_secondary))
                    .child(
                        div()
                            .flex_1()
                            .text_color(c.text_primary)
                            .child(crate::ui::parts::input(&self.inputs.search, t.text.body)),
                    )
                    .child(
                        div()
                            .px(t.space.xs + t.space.xxs)
                            .rounded(t.radius.sm)
                            .border(t.border_width)
                            .border_color(c.border_strong)
                            .type_style(detail(t))
                            .text_color(c.text_secondary)
                            .child("Esc"),
                    ),
            )
            .child(list);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_and_tasks_are_found_by_word() {
        let state = consort_client::mock::view_state();
        let hits = search(&state, "lucy");
        assert!(matches!(&hits[0], Hit::Channel { name, .. } if name == "miss-lucy-tg"));
        let hits = search(&state, "spam");
        assert!(
            hits.iter()
                .any(|h| matches!(h, Hit::Task { title, .. } if title.contains("spam rules")))
        );
        assert!(search(&state, "zzzz").is_empty());
    }

    #[test]
    fn an_empty_query_lists_channels_then_your_turn() {
        let state = consort_client::mock::view_state();
        let hits = search(&state, "");
        assert!(matches!(
            &hits[0],
            Hit::Channel {
                everything: true,
                ..
            }
        ));
        assert!(hits.len() <= MAX_ROWS);
        assert!(
            hits.iter()
                .filter_map(|h| match h {
                    Hit::Task { status, .. } => Some(*status),
                    _ => None,
                })
                .all(TaskStatus::is_your_turn)
        );
    }

    #[test]
    fn a_start_beats_a_word_start_beats_anywhere() {
        assert_eq!(score("sayso", "say"), 3);
        assert_eq!(score("miss-lucy-tg", "lucy"), 2);
        assert_eq!(score("homelab", "lab"), 1);
        assert_eq!(score("homelab", "x"), 0);
    }
}
