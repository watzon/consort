//! The main column: header, timeline and composer.

use consort_client::{
    ApprovalChoice, ApprovalView, ChannelKind, MessageView, RequestState, Speaker, TaskCardView,
    TaskStatus, TimelineItem,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, App, Context, Div, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window,
    base::SelectableText, component::text::TextView, div, linear_color_stop, linear_gradient, list,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Panel},
    ui::sizes,
    ui::window_controls::DragArea as _,
    ui::{
        markdown::{Segment, segments},
        menu::{MenuAction, MenuItem, OpenMenu},
        parts::{
            ButtonKind, button, detail, icon, icon_button, line, status_glyph, text, worker_pill,
        },
    },
};

/// What a right click on a message offers.
pub fn message_menu(message: &MessageView) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::new("Copy text", MenuAction::Copy(plain_text(&message.body))),
        MenuItem::new(
            "Copy as Markdown",
            MenuAction::CopyMarkdown(message.body.clone()),
        ),
        MenuItem::new(
            "Reply in thread",
            MenuAction::ReplyInThread(message.body.clone()),
        ),
        MenuItem::new(
            "Quote in reply",
            MenuAction::QuoteInReply(message.body.clone()),
        ),
    ];
    if message.speaker == Speaker::You {
        items.push(
            MenuItem::new(
                "Edit",
                MenuAction::EditMessage(message.id.clone(), message.body.clone()),
            )
            .separated(),
        );
        items.push(MenuItem::new("Delete", MenuAction::DeleteMessage(message.id.clone())).danger());
    }
    items
}

/// What a right click on a post in a task thread offers.
pub fn post_menu(body: &str) -> Vec<MenuItem> {
    vec![
        MenuItem::new("Copy text", MenuAction::Copy(plain_text(body))),
        MenuItem::new(
            "Copy as Markdown",
            MenuAction::CopyMarkdown(body.to_owned()),
        ),
        MenuItem::new("Quote in reply", MenuAction::QuoteInReply(body.to_owned())),
    ]
}

/// What a right click on selected text offers.
pub fn selection_menu(selected: String) -> Vec<MenuItem> {
    vec![
        MenuItem::new("Copy", MenuAction::Copy(selected.clone())),
        MenuItem::new("Quote in reply", MenuAction::QuoteInReply(selected.clone())),
        MenuItem::new("Ask Consort about this", MenuAction::AskConsort(selected)),
    ]
}

/// What a right click on a task card or a task row offers.
pub fn task_menu(task_id: &str, status: TaskStatus) -> Vec<MenuItem> {
    let id = task_id.to_owned();
    let mut items = vec![MenuItem::new(
        "Open thread",
        MenuAction::OpenThread(id.clone()),
    )];
    if status == TaskStatus::Resolved {
        items.push(MenuItem::new(
            "Not done",
            MenuAction::ReopenTask(id.clone()),
        ));
    } else {
        items.push(MenuItem::new(
            "Resolve",
            MenuAction::ResolveTask(id.clone()),
        ));
    }
    if status == TaskStatus::Failed {
        items.push(MenuItem::new("Retry", MenuAction::RetryTask(id.clone())));
    }
    if status == TaskStatus::Working {
        items.push(MenuItem::new("Stop", MenuAction::StopTask(id.clone())));
    }
    items.push(MenuItem::new("Copy task ID", MenuAction::Copy(id)).separated());
    items
}

/// Markdown with its marks taken out, for "Copy text".
fn plain_text(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut chars = markdown.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' | '_' | '`' => {}
            '[' => {
                let mut label = String::new();
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    label.push(c);
                }
                if chars.peek() == Some(&'(') {
                    for c in chars.by_ref() {
                        if c == ')' {
                            break;
                        }
                    }
                }
                out.push_str(&label);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Opens the selection menu when text is selected, else the given menu.
pub fn open_context_menu(
    this: &mut ConsortApp,
    event: &MouseDownEvent,
    items: Vec<MenuItem>,
    window: &mut Window,
    cx: &mut Context<ConsortApp>,
) {
    let selected = gpui_kit::base::TextSelection::selected_text(window, cx);
    let items = if selected.trim().is_empty() {
        items
    } else {
        selection_menu(selected)
    };
    this.open_menu(OpenMenu::at(event.position, items), cx);
    cx.stop_propagation();
}

impl ConsortApp {
    pub fn render_main(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panel_width = if self.panel == Panel::None {
            gpui_kit::Pixels::ZERO
        } else {
            t.layout.panel_width
        };
        let column = (window.viewport_size().width
            - t.layout.sidebar_width
            - panel_width
            - t.space.xl * 2.0)
            .min(t.layout.timeline_max_width);
        let narrow = column < sizes::WIDE_TIMELINE;
        if narrow != self.narrow {
            self.narrow = narrow;
            self.timeline.remeasure();
        }
        let sidebar = self.render_sidebar(t, window, cx);
        let column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_header(t, cx))
            .child(self.render_timeline(t, window, cx))
            .child(self.render_composer(t, cx));
        let panel = match self.panel.clone() {
            Panel::None => None,
            Panel::Tasks => Some(self.render_tasks_panel(t, window, cx)),
            Panel::Thread(task_id) => Some(self.render_thread_panel(&task_id, t, window, cx)),
        };
        div()
            .flex()
            .size_full()
            .child(sidebar)
            .child(column)
            .children(panel)
            .into_any_element()
    }

    fn render_header(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;

        let state = self.state.clone();
        let channel = state.channel(&self.channel_id);
        let name = channel.map(|c| c.name.clone()).unwrap_or_default();
        let purpose = channel.map(|c| c.purpose.clone()).unwrap_or_default();
        let panel = &state.tasks_panel;
        let pressed = self.panel == Panel::Tasks;
        let tasks_button = div()
            .id("tasks-button")
            .flex()
            .flex_none()
            .items_center()
            .gap(t.space.sm)
            .py(sizes::BUTTON_PAD_Y)
            .px(t.space.md - t.space.xxs)
            .rounded(t.radius.md)
            .border(t.border_width)
            .cursor_pointer()
            .when(pressed, |d| {
                d.bg(c.surface_selected).border_color(c.surface_selected)
            })
            .when(!pressed, |d| {
                d.border_color(c.border_strong)
                    .hover(|s| s.bg(c.hover_canvas))
            })
            .when(panel.need_you > 0, |d| {
                d.child(status_glyph(
                    "header-needs-input",
                    TaskStatus::NeedsInput,
                    t.icon.size,
                    t,
                    cx,
                ))
                .child(
                    text(
                        format!("{} need you", panel.need_you),
                        t.text.ui_strong,
                        c.text_primary,
                    )
                    .whitespace_nowrap(),
                )
                .child(
                    text(
                        format!("of {} tasks", panel.active_count),
                        t.text.ui,
                        if pressed {
                            c.text_secondary
                        } else {
                            c.text_tertiary
                        },
                    )
                    .whitespace_nowrap(),
                )
            })
            .when(panel.need_you == 0, |d| {
                d.child(text("Tasks", t.text.ui_strong, c.text_primary))
                    .child(text(
                        format!("{} active", panel.active_count),
                        t.text.ui,
                        if pressed {
                            c.text_secondary
                        } else {
                            c.text_tertiary
                        },
                    ))
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_tasks(cx)));
        let new_task = icon_button(
            "new-task",
            "square-pen",
            t.space.xxl,
            t.icon.size,
            c.text_secondary,
            c.hover_canvas,
            t,
        )
        .on_click(cx.listener(|this, _, window, cx| {
            this.inputs.composer.update(cx, |input, cx| {
                input.set_value("Start a task to ", window, cx);
                input.focus(window, cx);
            });
        }));
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(t.space.md)
            .h(t.space.page + t.space.xs)
            .pl(t.space.xl)
            .pr(t.space.lg)
            .border_b(t.border_width)
            .border_color(c.border_subtle)
            .drag_area()
            .child(
                div()
                    .type_style(t.text.title)
                    .line_height(t.text.body.line)
                    .text_color(c.text_primary)
                    .whitespace_nowrap()
                    .child(name),
            )
            .child(line(purpose, t.text.ui, c.text_tertiary).flex_1())
            .child(tasks_button)
            .child(new_task)
            .into_any_element()
    }

    fn render_timeline(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let empty = self
            .state
            .timeline(&self.channel_id)
            .is_none_or(|tl| tl.items.is_empty());
        let fade_top = t.space.xl;
        // At the live edge nothing is cut below, so the bottom fade shows only
        // while the reader is scrolled up.
        let at_end = empty || self.timeline.is_scrolled_to_end().unwrap_or(true);
        let fade_bottom = t.space.xxl;
        let body = if empty {
            self.render_empty_timeline(t).into_any_element()
        } else {
            let gap = t.space.xl;
            let count = self.timeline_keys.len();
            let max_width = t.layout.timeline_max_width;
            let inset = t.space.xl;
            list(
                self.timeline.clone(),
                cx.processor(move |this, ix: usize, window, cx| {
                    let t = ConsortApp::tokens(cx);
                    let row = this.render_timeline_item(ix, &t, window, cx);
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .px(inset)
                        .when(ix == 0, |d| d.pt(t.space.xl))
                        .when(ix + 1 < count, |d| d.pb(gap))
                        .child(div().w_full().max_w(max_width).child(row))
                        .into_any_element()
                }),
            )
            .size_full()
            .into_any_element()
        };
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(body)
            // The chat never has a hard edge: it fades into the canvas at the
            // top, and above the composer.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(fade_top)
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(c.surface_canvas, 0.),
                        linear_color_stop(c.canvas_clear, 1.),
                    )),
            )
            .when(!at_end, |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom(-t.space.lg)
                        .left_0()
                        .right_0()
                        .h(fade_bottom)
                        .bg(linear_gradient(
                            0.,
                            linear_color_stop(c.surface_canvas, 0.),
                            linear_color_stop(c.canvas_clear, 1.),
                        )),
                )
            })
            .into_any_element()
    }

    fn render_empty_timeline(&self, t: &Tokens) -> Div {
        let c = &t.colors;
        let name = self
            .state
            .channel(&self.channel_id)
            .map(|ch| ch.name.clone())
            .unwrap_or_default();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .pb(t.space.xxl)
            .child(
                div()
                    .w_full()
                    .max_w(t.layout.timeline_max_width)
                    .px(t.space.xl)
                    .pl(t.layout.speaker_gutter + t.space.lg + t.space.xl)
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .child(text(
                        format!("{name} has no messages yet"),
                        t.text.title,
                        c.text_primary,
                    ))
                    .child(text(
                        "Write what this channel is for, or ask Consort to start a task.",
                        t.text.body,
                        c.text_secondary,
                    )),
            )
    }

    fn render_timeline_item(
        &mut self,
        ix: usize,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(item) = self
            .state
            .timeline(&self.channel_id)
            .and_then(|tl| tl.items.get(ix))
            .cloned()
        else {
            return div().into_any_element();
        };
        match item {
            TimelineItem::Day(day) => div()
                .when(!self.narrow, |d| d.pl(t.layout.speaker_gutter + t.space.lg))
                .child(text(day.label, t.text.label, t.colors.text_tertiary))
                .into_any_element(),
            TimelineItem::Message(message) => self.render_message(&message, t, window, cx),
            TimelineItem::TaskCard(card) => self.render_card_row(&card, t, cx),
        }
    }

    /// One timeline row: the speaker in the gutter at 600 px and wider, or on
    /// one line above the content when the column is narrower.
    fn speaker_row(
        &self,
        speaker: Speaker,
        time: &str,
        edited: bool,
        top: gpui_kit::Pixels,
        content: impl IntoElement,
        t: &Tokens,
    ) -> AnyElement {
        let c = &t.colors;
        if !self.narrow {
            return div()
                .flex()
                .gap(t.space.lg)
                .child(self.gutter_with(speaker, time, edited, top, t))
                .child(content)
                .into_any_element();
        }
        let color = if speaker == Speaker::You {
            c.accent_text
        } else {
            c.text_secondary
        };
        let mut label = time.to_owned();
        if edited {
            label.push_str(", edited");
        }
        div()
            .flex()
            .flex_col()
            .gap(t.space.xs + t.space.xxs)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(t.space.sm)
                    .child(text(speaker.label(), t.text.label, color))
                    .child(text(label, t.text.caption, c.text_tertiary)),
            )
            .child(content)
            .into_any_element()
    }

    fn gutter_with(
        &self,
        speaker: Speaker,
        time: &str,
        edited: bool,
        top: gpui_kit::Pixels,
        t: &Tokens,
    ) -> Div {
        let c = &t.colors;
        let color = if speaker == Speaker::You {
            c.accent_text
        } else {
            c.text_secondary
        };
        let speaker_line = if speaker == Speaker::Task {
            t.text.label.line
        } else {
            t.text.body.line
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(t.layout.speaker_gutter)
            .pt(top)
            .child(
                div()
                    .type_style(t.text.label)
                    .line_height(speaker_line)
                    .text_color(color)
                    .child(speaker.label()),
            )
            .child(text(time.to_owned(), t.text.caption, c.text_tertiary))
            .when(edited, |d| {
                d.child(text("Edited", t.text.caption, c.text_tertiary))
            })
    }

    /// Message text, split into prose and code blocks.
    pub fn render_markdown(
        &mut self,
        key: &str,
        markdown: &str,
        menu: Vec<MenuItem>,
        code_surface: Hsla,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let mut out = div().flex().flex_col().gap(t.space.md).min_w_0();
        for (i, segment) in segments(markdown).into_iter().enumerate() {
            let seg_key = format!("{key}.{i}");
            match segment {
                Segment::Prose(prose) => {
                    let state = self.texts.get(&seg_key, &prose, cx);
                    let items = menu.clone();
                    out = out.child(
                        div()
                            .id(SharedString::from(seg_key))
                            .min_w_0()
                            .child(TextView::new(&state))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    open_context_menu(this, event, items.clone(), window, cx)
                                }),
                            ),
                    );
                }
                Segment::Code { code, .. } => {
                    let copy = code.clone();
                    out = out.child(
                        div()
                            .id(SharedString::from(seg_key.clone()))
                            .min_w_0()
                            .rounded(t.radius.md)
                            .bg(code_surface)
                            .border(t.border_width)
                            .border_color(c.border_subtle)
                            .px(t.space.md)
                            .py(t.space.sm)
                            .font_family(t.font_mono.clone())
                            .type_style(t.text.mono)
                            .text_color(c.text_primary)
                            .child(SelectableText::new(
                                SharedString::from(format!("{seg_key}.code")),
                                code,
                            ))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    open_context_menu(
                                        this,
                                        event,
                                        vec![MenuItem::new(
                                            "Copy code",
                                            MenuAction::Copy(copy.clone()),
                                        )],
                                        window,
                                        cx,
                                    )
                                }),
                            ),
                    );
                }
            }
        }
        out
    }

    fn render_message(
        &mut self,
        message: &MessageView,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let you = message.speaker == Speaker::You;
        let top = if you {
            t.space.xs + t.space.xxs
        } else {
            t.space.xxs
        };
        let menu = message_menu(message);
        let editing = you && self.editing.as_deref() == Some(message.id.as_str());
        let body = if editing {
            self.render_edit_box(t, cx)
        } else {
            self.render_markdown(&message.id, &message.body, menu, c.surface_card, t, cx)
        };
        let content = if editing {
            div().flex().flex_col().flex_1().min_w_0().child(body)
        } else if you {
            div()
                .flex()
                .flex_col()
                .items_start()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .max_w_full()
                        .bg(c.accent_surface)
                        .rounded(t.radius.lg)
                        .py(t.space.xs + t.space.xxs)
                        .px(t.space.md)
                        .type_style(t.text.body)
                        .text_color(c.text_primary)
                        .child(body),
                )
        } else {
            let mut content = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(t.space.sm)
                .type_style(t.text.body)
                .text_color(c.text_primary);
            if let Some(title) = &message.title {
                content = content.child(
                    div()
                        .type_style(t.text.title)
                        .line_height(t.text.body.line)
                        .child(title.clone()),
                );
            }
            content = content.child(body);
            if !message.items.is_empty() {
                let mut rows = div()
                    .flex()
                    .flex_col()
                    .pt(t.space.xs)
                    .border_t(t.border_width)
                    .border_color(c.border_subtle);
                let last = message.items.len() - 1;
                for (i, item) in message.items.iter().enumerate() {
                    let task_id = item.task_id.clone();
                    rows = rows.child(
                        div()
                            .id(SharedString::from(format!("{}-item-{i}", message.id)))
                            .flex()
                            .items_center()
                            .gap(t.space.md)
                            .py(t.space.sm)
                            .when(i < last, |d| {
                                d.border_b(t.border_width).border_color(c.border_subtle)
                            })
                            .when(task_id.is_some(), |d| d.cursor_pointer())
                            .child(status_glyph(
                                SharedString::from(format!("{}-glyph-{i}", message.id)),
                                item.status,
                                t.icon.size,
                                t,
                                cx,
                            ))
                            .child(
                                text(item.text.clone(), t.text.ui, c.text_primary)
                                    .flex_1()
                                    .min_w_0(),
                            )
                            .child(
                                text(item.channel_name.clone(), t.text.ui, c.text_tertiary)
                                    .w(t.layout.speaker_gutter + t.space.page - t.space.sm)
                                    .flex_none()
                                    .text_right(),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(id) = task_id.clone() {
                                    this.open_thread(id, cx);
                                }
                            })),
                    );
                }
                content = content.child(rows);
            }
            content
        };
        self.speaker_row(
            message.speaker,
            &message.time_label,
            message.edited,
            top,
            content,
            t,
        )
    }

    /// The text box that edits one of the user's messages in place. Enter
    /// saves and Escape cancels.
    fn render_edit_box(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .child(
                div()
                    .id("edit-message")
                    .w_full()
                    .py(t.space.xs + t.space.xxs)
                    .px(t.space.md)
                    .bg(c.accent_surface)
                    .border(t.border_width)
                    .border_color(c.accent_text)
                    .rounded(t.radius.lg)
                    .text_color(c.text_primary)
                    .child(crate::ui::parts::textarea(&self.inputs.edit, t.text.body))
                    .capture_key_down(cx.listener(
                        |this, event: &gpui_kit::KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape" {
                                this.cancel_edit(window, cx);
                                cx.stop_propagation();
                            }
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(
                        text("Enter saves. Escape cancels.", detail(t), c.text_tertiary).flex_1(),
                    )
                    .child(
                        button("edit-cancel", "Cancel", ButtonKind::Quiet, t).on_click(
                            cx.listener(|this, _, window, cx| this.cancel_edit(window, cx)),
                        ),
                    )
                    .child(
                        button("edit-save", "Save", ButtonKind::Primary, t).on_click(
                            cx.listener(|this, _, window, cx| this.save_edit(window, cx)),
                        ),
                    ),
            )
    }

    fn render_card_row(
        &mut self,
        card: &TaskCardView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = card
            .approval
            .as_ref()
            .is_none_or(|a| a.state == RequestState::Answered)
            && matches!(card.status, TaskStatus::Resolved);
        let top = if collapsed {
            t.space.sm + t.space.xxs
        } else {
            t.space.md
        };
        let card_element = self.render_task_card(card, collapsed, t, cx);
        self.speaker_row(Speaker::Task, &card.time_label, false, top, card_element, t)
    }

    pub fn render_task_card(
        &mut self,
        card: &TaskCardView,
        collapsed: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let open = self.panel == Panel::Thread(card.task_id.clone());
        let task_id = card.task_id.clone();
        let menu_task = card.task_id.clone();
        let status = card.status;
        let glyph = status_glyph(
            SharedString::from(format!("card-{}", card.message_id)),
            card.status,
            t.icon.size,
            t,
            cx,
        );
        let base = div()
            .id(SharedString::from(format!("card-{}", card.message_id)))
            .flex()
            .flex_1()
            .min_w_0()
            .rounded(t.radius.lg)
            .border(t.border_width)
            .border_color(if open { c.accent_text } else { c.border_subtle })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    open_context_menu(this, event, task_menu(&menu_task, status), window, cx)
                }),
            );
        if collapsed {
            return base
                .items_center()
                .gap(t.space.sm)
                .py(t.space.sm + t.space.xxs)
                .px(t.space.md + t.space.xxs)
                .cursor_pointer()
                .hover(|s| s.bg(c.hover_canvas))
                .child(glyph)
                .child(line(card.title.clone(), t.text.ui_strong, c.text_secondary).flex_1())
                .child(text(card.summary.clone(), detail(t), c.text_tertiary).whitespace_nowrap())
                .on_click(cx.listener(move |this, _, _, cx| this.open_thread(task_id.clone(), cx)))
                .into_any_element();
        }
        let inset = t.icon.size + t.space.sm;
        let header = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .child(glyph)
            .child(line(card.title.clone(), t.text.ui_strong, c.text_primary).flex_1())
            .when_some(card.channel_name.clone(), |d, name| {
                d.child(text(name, detail(t), c.text_tertiary).whitespace_nowrap())
            })
            .when_some(card.worker.clone(), |d, w| {
                d.child(worker_pill(&w.name, &w.account, w.online, t))
            });
        let mut body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(t.space.md)
            .pt(t.space.md)
            .pb(t.space.md + t.space.xxs)
            .px(t.space.md + t.space.xxs)
            .bg(c.surface_card)
            .rounded(t.radius.lg)
            .child(header);
        let pending = card
            .approval
            .clone()
            .filter(|a| a.state != RequestState::Answered);
        if let Some(approval) = pending {
            body = body
                .child(
                    self.render_approval_detail(&approval, true, t, cx)
                        .pl(inset),
                )
                .child(
                    self.render_approval_buttons(&approval, t, cx)
                        .pl(inset)
                        .child(div().flex_1())
                        .child(self.open_thread_link(&card.task_id, open, t, cx)),
                );
        } else {
            let summary = match &card.approval {
                Some(a) => a
                    .answer
                    .map(|choice| answered_label(choice).to_owned())
                    .unwrap_or_default(),
                None => card.latest.as_deref().map(plain_text).unwrap_or_default(),
            };
            body = body.child(
                div()
                    .pl(inset)
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .when(!summary.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .type_style(t.text.ui)
                                .text_color(c.text_secondary)
                                .child(summary),
                        )
                    })
                    .when(card.latest.is_none() && card.approval.is_none(), |d| {
                        d.child(div().flex_1())
                    })
                    .child(self.open_thread_link(&card.task_id, open, t, cx)),
            );
        }
        base.child(body).into_any_element()
    }

    fn open_thread_link(
        &mut self,
        task_id: &str,
        open: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let task_id = task_id.to_owned();
        div()
            .id(SharedString::from(format!("open-thread-{task_id}")))
            .flex_none()
            .type_style(detail(t))
            .text_color(if open {
                c.accent_text
            } else {
                c.text_secondary
            })
            .cursor_pointer()
            .when(!open, |d| d.hover(|s| s.text_color(c.text_primary)))
            .child(if open { "Thread open" } else { "Open thread" })
            .on_click(cx.listener(move |this, _, _, cx| {
                if open {
                    this.close_panel(cx);
                } else {
                    this.open_thread(task_id.clone(), cx);
                }
            }))
            .into_any_element()
    }

    /// "Codex wants to run a command in C:\Code\sayso", then the command.
    pub fn render_approval_detail(
        &mut self,
        approval: &ApprovalView,
        with_folder: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let mut detail_box = div().flex().flex_col().gap(t.space.sm).min_w_0();
        if with_folder {
            let mut summary = div()
                .flex()
                .flex_wrap()
                .type_style(t.text.ui)
                .text_color(c.text_secondary)
                .child(approval.summary.clone());
            if let Some(folder) = &approval.folder {
                summary = summary
                    .child(div().whitespace_nowrap().child(" in "))
                    .child(
                        div()
                            .font_family(t.font_mono.clone())
                            .text_color(c.text_primary)
                            .child(folder.clone()),
                    );
            }
            detail_box = detail_box.child(summary);
        }
        if let Some(command) = approval.detail.clone() {
            detail_box = detail_box.child(
                div()
                    .id(SharedString::from(format!("{}-command", approval.id)))
                    .min_w_0()
                    .bg(c.surface_raised)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .rounded(t.radius.md)
                    .py(t.space.sm)
                    .px(t.space.md)
                    .font_family(t.font_mono.clone())
                    .type_style(t.text.mono)
                    .text_color(c.text_primary)
                    .child(SelectableText::new(
                        SharedString::from(format!("{}.command", approval.id)),
                        command.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            open_context_menu(
                                this,
                                event,
                                vec![MenuItem::new(
                                    "Copy code",
                                    MenuAction::Copy(command.clone()),
                                )],
                                window,
                                cx,
                            )
                        }),
                    ),
            );
        }
        detail_box
    }

    pub fn render_approval_buttons(
        &mut self,
        approval: &ApprovalView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut row = div().flex().flex_wrap().items_center().gap(t.space.sm);
        for (i, choice) in approval.choices.iter().enumerate() {
            let kind = match choice {
                ApprovalChoice::AllowOnce => ButtonKind::Primary,
                ApprovalChoice::AllowForTask => ButtonKind::Secondary,
                ApprovalChoice::Deny => ButtonKind::Quiet,
            };
            let id = approval.id.clone();
            let choice = *choice;
            row = row.child(
                button(
                    SharedString::from(format!("{}-{i}", approval.id)),
                    choice.label(),
                    kind,
                    t,
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.answer_approval(id.clone(), choice, cx)),
                ),
            );
        }
        row
    }

    fn render_composer(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let channel = state.channel(&self.channel_id);
        let hint = channel
            .and_then(
                |ch| match (&ch.coordinator_worker, &ch.coordinator_account) {
                    (Some(w), Some(a)) => Some(format!("Runs on {w} with {a}")),
                    (Some(w), None) => Some(format!("Runs on {w}")),
                    _ => None,
                },
            )
            .unwrap_or_default();
        let is_everything = channel.is_some_and(|ch| ch.kind == ChannelKind::Everything);
        let _ = is_everything;
        let input = crate::ui::parts::textarea(&self.inputs.composer, t.text.body);
        let send = div()
            .id("send")
            .size(t.space.xl + t.space.xs)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(t.radius.md)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_strong)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_raised))
            .child(icon("corner-down-left", t.icon.dense, c.text_primary))
            .on_click(cx.listener(|this, _, window, cx| this.send_message(window, cx)));
        let tool = |id: &'static str, name: &str| {
            icon_button(
                id,
                name,
                t.space.xl + t.space.xs,
                t.icon.size,
                c.text_secondary,
                c.hover_card,
                t,
            )
        };
        let link = match &state.connection {
            consort_client::Connection::Offline { retry_in_ms, .. } => Some(format!(
                "Consort lost the connection to the hub. It tries again in {} s.",
                retry_in_ms.div_ceil(1000)
            )),
            consort_client::Connection::Connecting => Some("Connecting to the hub".to_owned()),
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(t.space.sm)
            .px(t.space.xl)
            .pt(t.space.lg)
            .pb(t.space.xl)
            .when_some(link, |d, line_text| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.sm)
                        .w_full()
                        .max_w(t.layout.timeline_max_width)
                        .child(status_glyph(
                            "link-state",
                            TaskStatus::Working,
                            t.icon.dense,
                            t,
                            cx,
                        ))
                        .child(text(line_text, detail(t), c.text_secondary)),
                )
            })
            .child(
                div()
                    .id("composer")
                    .flex()
                    .flex_col()
                    .w_full()
                    .max_w(t.layout.timeline_max_width)
                    .gap(t.space.sm + t.space.xxs)
                    .pt(t.space.md)
                    .pb(t.space.sm + t.space.xxs)
                    .pl(t.space.lg)
                    .pr(t.space.md)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .rounded(t.radius.lg)
                    .child(
                        div()
                            .type_style(t.text.body)
                            .text_color(c.text_primary)
                            .child(input),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.xs)
                            .ml(-t.space.sm)
                            .child(tool("attach", "plus").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.show_notice(
                                        "Attachments come in a later build.".into(),
                                        cx,
                                    )
                                },
                            )))
                            .child(tool("voice", "mic").on_click(cx.listener(|this, _, _, cx| {
                                this.show_notice("Voice input comes in a later build.".into(), cx)
                            })))
                            .child(div().flex_1())
                            .child(text(hint, detail(t), c.text_tertiary).pr(t.space.sm))
                            .child(send),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            let items = vec![
                                MenuItem::new("Cut", MenuAction::ComposerCut),
                                MenuItem::new("Copy", MenuAction::ComposerCopy),
                                MenuItem::new("Paste", MenuAction::ComposerPaste),
                                MenuItem::new("Select all", MenuAction::ComposerSelectAll)
                                    .separated(),
                            ];
                            this.open_menu(OpenMenu::at(event.position, items), cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .into_any_element()
    }
}

fn answered_label(choice: ApprovalChoice) -> &'static str {
    match choice {
        ApprovalChoice::AllowOnce => "Allowed once.",
        ApprovalChoice::AllowForTask => "Allowed for this task.",
        ApprovalChoice::Deny => "Denied.",
    }
}

/// Keeps `App` in the import list on every platform.
#[allow(dead_code)]
fn _app(_: &App) {}
