//! The main column: header, timeline and composer.

use std::time::Instant;

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
    countdown,
    ui::window_controls::DragArea as _,
    ui::{
        links::click_action,
        markdown::{Segment, exact_selection, segments},
        menu::{MenuAction, MenuItem, OpenMenu, TextTarget, text_edit_menu},
        model_picker::{PickerTarget, TriggerStyle},
        parts::{
            ButtonKind, button, detail, focusable, icon, icon_button, line, selectable,
            status_glyph, text, worker_pill,
        },
    },
};

/// What the timeline says when the open channel did not load.
const UNLOADED_TEXT: &str = "Cannot load this channel. Retrying.";

/// What a right click on a message offers. A message that is still on its
/// way to the hub has a made-up ID, so it offers only what works on its text:
/// no Reply in thread, Edit or Delete.
pub fn message_menu(message: &MessageView) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::new("Copy text", MenuAction::Copy(plain_text(&message.body))),
        MenuItem::new(
            "Copy as Markdown",
            MenuAction::CopyMarkdown(message.body.clone()),
        ),
    ];
    if !message.pending {
        items.push(MenuItem::new(
            "Reply in thread",
            MenuAction::ReplyInThread(message.body.clone()),
        ));
    }
    items.push(MenuItem::new(
        "Quote in reply",
        MenuAction::QuoteInReply(message.body.clone()),
    ));
    if message.speaker == Speaker::You && !message.pending {
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

/// What a right click on a task card or a task row offers. Resolve is for a
/// task that is Ready for review, and Not done is for a resolved task. Retry
/// is for a failed task, Stop for a working one, and Archive for a failed or
/// a resolved one. The thread panel leaves out Open thread, because the
/// thread is open.
pub fn task_menu(task_id: &str, status: TaskStatus, with_open_thread: bool) -> Vec<MenuItem> {
    let id = task_id.to_owned();
    let mut items = Vec::new();
    if with_open_thread {
        items.push(MenuItem::new(
            "Open thread",
            MenuAction::OpenThread(id.clone()),
        ));
    }
    match status {
        TaskStatus::Resolved => items.push(MenuItem::new(
            "Not done",
            MenuAction::ReopenTask(id.clone()),
        )),
        TaskStatus::Review => items.push(MenuItem::new(
            "Resolve",
            MenuAction::ResolveTask(id.clone()),
        )),
        _ => {}
    }
    if status == TaskStatus::Failed {
        items.push(MenuItem::new("Retry", MenuAction::RetryTask(id.clone())));
    }
    if status == TaskStatus::Working {
        items.push(MenuItem::new("Stop", MenuAction::StopTask(id.clone())));
    }
    // A failed task would stay in Active for ever, and a resolved task stays
    // in the lists. Archive hides either one.
    if matches!(status, TaskStatus::Failed | TaskStatus::Resolved) {
        items.push(MenuItem::new(
            "Archive",
            MenuAction::ArchiveTask(id.clone()),
        ));
    }
    items.push(MenuItem::new("Copy task ID", MenuAction::Copy(id)).separated());
    items
}

/// Markdown as the reader sees it, for "Copy text". Code keeps every
/// character.
fn plain_text(markdown: &str) -> String {
    consort_client::format::plain_text(markdown)
}

/// Opens the selection menu when text is selected, else the given menu.
pub fn open_context_menu(
    this: &mut ConsortApp,
    event: &MouseDownEvent,
    items: Vec<MenuItem>,
    window: &mut Window,
    cx: &mut Context<ConsortApp>,
) {
    let selected = exact_selection(&gpui_kit::base::TextSelection::selected_text(window, cx));
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
        let narrow = column < t.layout.wide_timeline;
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
        let tags = channel.map(|c| c.tags.clone()).unwrap_or_default();
        let panel = &state.tasks_panel;
        let pressed = self.panel == Panel::Tasks;
        let tasks_button = focusable(div().id("tasks-button"), t)
            .flex()
            .flex_none()
            .items_center()
            .gap(t.space.sm)
            .py(t.component.button_pad_y)
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
        .on_click(cx.listener(|this, _, window, cx| this.start_task_draft(window, cx)));
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
            .when(!tags.is_empty(), |d| {
                // The channel's worker tags: a click opens the Worker tags
                // dialog.
                let id = self.channel_id.clone();
                let mut chips = div()
                    .id("channel-tags")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(t.space.xs)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_dialog(crate::app::Dialog::ChannelTags(id.clone()), window, cx)
                    }));
                for tag in &tags {
                    chips = chips.child(crate::ui::parts::tag_chip(tag, false, t));
                }
                d.child(chips)
            })
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
        let unloaded = self.channel_unloaded();
        let body = if let Some(waiting) = self.waiting() {
            // No channel has loaded. It is not a channel with no messages.
            self.render_waiting_timeline(&waiting, t, cx)
        } else if empty && unloaded {
            self.render_unloaded_timeline(t).into_any_element()
        } else if empty {
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
            // A channel with messages that did not load shows what it has, and
            // says that the rest is missing.
            .when(unloaded && !empty, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(t.space.sm)
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .py(t.space.xs + t.space.xxs)
                                .px(t.space.md)
                                .rounded(t.radius.md)
                                .bg(c.surface_raised)
                                .child(text(UNLOADED_TEXT, t.text.ui, c.text_secondary)),
                        ),
                )
            })
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

    /// A channel that did not load shows no messages, which does not mean it
    /// has none. It says so, and the client asks the hub again by itself.
    fn render_unloaded_timeline(&self, t: &Tokens) -> Div {
        let c = &t.colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(t.space.xs)
            .child(text(UNLOADED_TEXT, t.text.title, c.text_primary))
            .child(text(
                "Messages show here when it loads.",
                t.text.body,
                c.text_secondary,
            ))
    }

    /// The column while no channel has loaded: "Connecting to the hub", or
    /// why the hub is not there, with a button that tries again at once.
    fn render_waiting_timeline(
        &self,
        waiting: &crate::waiting::Waiting,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let connecting = !waiting.can_retry;
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(t.space.sm)
            .px(t.space.xl)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .when(connecting, |d| {
                        d.child(status_glyph(
                            "waiting-for-hub",
                            TaskStatus::Working,
                            t.icon.size,
                            t,
                            cx,
                        ))
                    })
                    .child(text(waiting.title, t.text.title, c.text_primary)),
            )
            .child(text(waiting.next, t.text.body, c.text_secondary))
            .when_some(waiting.reason.clone(), |d, reason| {
                // The reason is an error detail: the user can copy it.
                d.child(div().max_w(t.layout.timeline_max_width).child(selectable(
                    SharedString::from(format!("waiting-reason-{reason}")),
                    reason,
                    detail(t),
                    c.text_tertiary,
                )))
            })
            .when(waiting.can_retry, |d| {
                d.child(
                    button("waiting-try-again", "Try again", ButtonKind::Secondary, t)
                        .on_click(cx.listener(|this, _, _, cx| this.try_again(cx))),
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
            TimelineItem::Approval(card) => {
                let element = self.render_approval_card(&card.approval, &card.message_id, t, cx);
                self.speaker_row(
                    card.speaker,
                    &card.time_label,
                    false,
                    t.space.md,
                    element,
                    t,
                )
            }
            TimelineItem::Decision(card) => {
                let element = self.render_decision(&card.decision, &card.message_id, false, t, cx);
                self.speaker_row(
                    card.speaker,
                    &card.time_label,
                    false,
                    t.space.md,
                    element,
                    t,
                )
            }
            TimelineItem::Notice(notice) => self.render_notice_line(&notice, t, cx),
        }
    }

    /// A system notice: one quiet line in the column of the message text,
    /// with no speaker. The text is selectable. The hub writes the kind and
    /// the text; only the text shows.
    fn render_notice_line(
        &self,
        notice: &consort_client::NoticeView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let copy = notice.text.clone();
        div()
            .id(SharedString::from(format!("notice-{}", notice.message_id)))
            .flex()
            .items_baseline()
            .gap(t.space.sm)
            .when(!self.narrow, |d| d.pl(t.layout.speaker_gutter + t.space.lg))
            .child(
                selectable(
                    SharedString::from(format!("notice-line-{}", notice.message_id)),
                    notice.text.clone(),
                    t.text.ui,
                    c.text_tertiary,
                )
                .min_w_0(),
            )
            .child(text(
                notice.time_label.clone(),
                t.text.caption,
                c.text_tertiary,
            ))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    open_context_menu(
                        this,
                        event,
                        vec![MenuItem::new("Copy text", MenuAction::Copy(copy.clone()))],
                        window,
                        cx,
                    )
                }),
            )
            .into_any_element()
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
                    // A click on a link never opens it by itself. The link
                    // policy decides: web and email links open, a link whose
                    // words differ from its address asks first, and every
                    // other scheme is refused.
                    let weak = cx.weak_entity();
                    let source = prose.clone();
                    let view =
                        TextView::new(&state).on_link_click(move |url, event, window, cx| {
                            if !is_open_click(event) {
                                return;
                            }
                            let action = click_action(&source, url);
                            let _ =
                                weak.update(cx, |this, cx| this.follow_link(action, window, cx));
                        });
                    out = out.child(
                        div()
                            .id(SharedString::from(seg_key))
                            .min_w_0()
                            .child(view)
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(id) = task_id.clone() {
                                    this.open_thread(id, window, cx);
                                }
                            })),
                    );
                }
                content = content.child(rows);
            }
            content
        };
        let time = time_text(&message.time_label, message.pending);
        self.speaker_row(message.speaker, time, message.edited, top, content, t)
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
                    open_context_menu(this, event, task_menu(&menu_task, status, true), window, cx)
                }),
            );
        if collapsed {
            return focusable(base, t)
                .items_center()
                .gap(t.space.sm)
                .py(t.space.sm + t.space.xxs)
                .px(t.space.md + t.space.xxs)
                .cursor_pointer()
                .hover(|s| s.bg(c.hover_canvas))
                .child(glyph)
                .child(line(card.title.clone(), t.text.ui_strong, c.text_secondary).flex_1())
                .child(text(card.summary.clone(), detail(t), c.text_tertiary).whitespace_nowrap())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_thread(task_id.clone(), window, cx)
                }))
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
            // How much the task may do without asking. A worker can cap it.
            .child(text(card.approval_mode.tag(), detail(t), c.text_tertiary).whitespace_nowrap())
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
            // The newest update, as clean text of at most three lines: a
            // review summary is Markdown with paragraphs and code, and each
            // block is a line of its own.
            let summary = match &card.approval {
                Some(a) => a
                    .answer
                    .map(|choice| answered_label(a, choice))
                    .unwrap_or_default(),
                None => card
                    .latest
                    .as_deref()
                    .map(consort_client::format::card_summary)
                    .unwrap_or_default(),
            };
            if !summary.is_empty() {
                // The text is selectable: it holds the reason for a failed
                // task or the review summary, and the user may copy it.
                body = body.child(
                    selectable(
                        SharedString::from(format!("card-latest-{}", card.message_id)),
                        summary,
                        t.text.ui,
                        c.text_secondary,
                    )
                    .pl(inset)
                    .min_w_0()
                    .line_clamp(3)
                    .text_ellipsis(),
                );
            }
            // The actions sit in one row at the bottom, as on an approval:
            // Resolve for a task in review, and Open thread at the right.
            let task_id = card.task_id.clone();
            body = body.child(
                div()
                    .pl(inset)
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .when(card.status == TaskStatus::Review, |d| {
                        d.child(
                            button(
                                SharedString::from(format!("resolve-{}", card.message_id)),
                                "Resolve",
                                ButtonKind::Primary,
                                t,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.run_menu(
                                        MenuAction::ResolveTask(task_id.clone()),
                                        window,
                                        cx,
                                    )
                                },
                            )),
                        )
                    })
                    .child(div().flex_1())
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
        focusable(
            div().id(SharedString::from(format!("open-thread-{task_id}"))),
            t,
        )
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
        .on_click(cx.listener(move |this, _, window, cx| {
            if open {
                this.close_panel(cx);
            } else {
                this.open_thread(task_id.clone(), window, cx);
            }
        }))
        .into_any_element()
    }

    /// What an approval asks for: the summary line (when `with_summary`),
    /// the folder it runs in, and the command, the files or the tool call in
    /// a block. The folder always shows, in full, because the agent writes
    /// the summary and can choose the folder (Codex lets the model do it).
    pub fn render_approval_detail(
        &mut self,
        approval: &ApprovalView,
        with_summary: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let mut detail_box = div().flex().flex_col().gap(t.space.sm).min_w_0();
        for part in detail_parts(approval, with_summary) {
            detail_box = match part {
                DetailPart::Summary(summary) => detail_box.child(
                    div()
                        .flex()
                        .min_w_0()
                        .type_style(t.text.ui)
                        .text_color(c.text_secondary)
                        .child(div().max_w_full().child(SelectableText::new(
                            SharedString::from(format!("{}.summary", approval.id)),
                            summary,
                        ))),
                ),
                DetailPart::Folder(folder) => detail_box.child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(t.space.sm)
                        .min_w_0()
                        .child(text("Folder", detail(t), c.text_tertiary).flex_none())
                        .child(
                            div()
                                .min_w_0()
                                .font_family(t.font_mono.clone())
                                .type_style(t.text.mono)
                                .text_color(c.text_primary)
                                .child(SelectableText::new(
                                    SharedString::from(format!("{}.folder", approval.id)),
                                    folder,
                                )),
                        ),
                ),
                DetailPart::Command(command) => detail_box.child(
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
                ),
            };
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
            // The wide choice shows the harness's own words, which can be
            // long. They wrap, so the user reads the whole scope.
            let wide = choice == ApprovalChoice::AllowForTask;
            row = row.child(
                button(
                    SharedString::from(format!("{}-{i}", approval.id)),
                    approval.choice_label(choice),
                    kind,
                    t,
                )
                .when(wide, |b| b.whitespace_normal().max_w_full().text_left())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.answer_approval(id.clone(), choice, window, cx)
                })),
            );
        }
        row
    }

    fn render_composer(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let channel = state.channel(&self.channel_id);
        // A channel without its own model says that it uses Everything's.
        let inherited = channel.is_some_and(|ch| {
            ch.kind != ChannelKind::Everything
                && ch
                    .coordinator_selection
                    .as_ref()
                    .is_some_and(|e| e.source == consort_client::SelectionSource::Everything)
        });
        let target = PickerTarget::Assistant(self.channel_id.clone());
        let model_chip = channel.map(|_| {
            self.render_model_trigger("composer-model", target.clone(), TriggerStyle::Chip, t, cx)
        });
        let effort_chip = channel.and_then(|_| {
            self.render_effort_trigger("composer-effort", target.clone(), TriggerStyle::Chip, t, cx)
        });
        let input = crate::ui::parts::textarea(&self.inputs.composer, t.text.body);
        let send = focusable(div().id("send"), t)
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
            .on_click(cx.listener(|this, _, window, cx| {
                this.send_message(window, cx);
                // The button takes the focus. The composer gets it back, so
                // the user can go on typing.
                this.inputs
                    .composer
                    .update(cx, |input, cx| input.focus(window, cx));
            }));
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
        // The line under the composer: the state of the link, and for an
        // offline link the reason and a countdown that ticks once a second.
        let link: Option<(String, Option<String>, bool)> = match &state.connection {
            consort_client::Connection::Offline {
                retry_in_ms,
                reason,
            } => {
                let left = self.offline.as_ref().map_or_else(
                    || countdown::seconds_left(*retry_in_ms, std::time::Duration::ZERO),
                    |o| o.seconds_left(Instant::now()),
                );
                let reason = Some(reason.trim().to_owned()).filter(|r| !r.is_empty());
                Some((countdown::offline_line(left), reason, true))
            }
            consort_client::Connection::Connecting => {
                Some(("Connecting to the hub".to_owned(), None, false))
            }
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
            .when_some(link, |d, (line_text, reason, offline)| {
                d.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(t.space.sm)
                        .w_full()
                        .max_w(t.layout.timeline_max_width)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .h(detail(t).line)
                                .child(status_glyph(
                                    "link-state",
                                    TaskStatus::Working,
                                    t.icon.dense,
                                    t,
                                    cx,
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(text(line_text, detail(t), c.text_secondary))
                                .when_some(reason, |col, reason| {
                                    // The reason is an error detail, so the
                                    // user can select it and copy it.
                                    col.child(selectable(
                                        SharedString::from(format!("link-reason-{reason}")),
                                        reason,
                                        detail(t),
                                        c.text_tertiary,
                                    ))
                                }),
                        )
                        // Do not wait for the countdown.
                        .when(offline, |row| {
                            row.child(
                                button("link-try-again", "Try again", ButtonKind::Quiet, t)
                                    .on_click(cx.listener(|this, _, _, cx| this.try_again(cx))),
                            )
                        }),
                )
            })
            .child(
                div()
                    .id("composer")
                    .flex()
                    .flex_col()
                    .w_full()
                    .max_w(t.layout.timeline_max_width)
                    // No channel has loaded: the box takes no text.
                    .when(self.waiting().is_some(), |d| d.opacity(t.opacity.busy))
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
                            .when(inherited, |d| {
                                d.child(
                                    text("Same as Everything", detail(t), c.text_tertiary)
                                        .flex_none()
                                        .pr(t.space.xs),
                                )
                            })
                            .children(model_chip)
                            .children(effort_chip)
                            .child(div().w(t.space.xs).flex_none())
                            .child(send),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            let items = text_edit_menu(TextTarget::Composer);
                            this.open_menu(OpenMenu::at(event.position, items), cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .into_any_element()
    }
}

/// One part of the detail block of an approval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailPart {
    /// The line that says what asks.
    Summary(String),
    /// The folder the command runs in.
    Folder(String),
    /// The command, the files or the tool call.
    Command(String),
}

/// The parts of an approval's detail block, in the order they show. The
/// summary is the agent's own words, so the folder and the command must
/// always be there to check it against: a card never leaves out the folder.
pub fn detail_parts(approval: &ApprovalView, with_summary: bool) -> Vec<DetailPart> {
    let mut parts = Vec::new();
    if with_summary {
        parts.push(DetailPart::Summary(approval.summary.clone()));
    }
    if let Some(folder) = approval.folder.clone().filter(|f| !f.trim().is_empty()) {
        parts.push(DetailPart::Folder(folder));
    }
    if let Some(command) = approval.detail.clone() {
        parts.push(DetailPart::Command(command));
    }
    parts
}

/// The time under a speaker. A message or a post that is still on its way to
/// the hub says "Sending" where its time goes.
pub fn time_text(label: &str, pending: bool) -> &str {
    if pending { "Sending" } else { label }
}

/// True for the clicks that open a link: the main or middle mouse button, a
/// key press, or a tap. A right click or a long press does not.
pub(crate) fn is_open_click(event: &gpui_kit::ClickEvent) -> bool {
    match event {
        gpui_kit::ClickEvent::Mouse(click) => {
            matches!(click.up.button, MouseButton::Left | MouseButton::Middle)
        }
        gpui_kit::ClickEvent::Keyboard(_) => true,
        gpui_kit::ClickEvent::Touch(click) => !click.long_press,
    }
}

/// What a card says after the user answered an approval. The wide choice
/// repeats the harness's own words, so the card keeps the scope that the
/// user saw on the button.
fn answered_label(approval: &ApprovalView, choice: ApprovalChoice) -> String {
    match choice {
        ApprovalChoice::AllowOnce => "Allowed once.".to_owned(),
        ApprovalChoice::AllowForTask => match approval.allow_always_label.as_deref() {
            Some(label) if !label.trim().is_empty() => format!("Allowed: {}", label.trim()),
            _ => "Allowed for this task.".to_owned(),
        },
        ApprovalChoice::Deny => "Denied.".to_owned(),
    }
}

/// Keeps `App` in the import list on every platform.
#[allow(dead_code)]
fn _app(_: &App) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(speaker: Speaker, pending: bool) -> MessageView {
        MessageView {
            id: if pending {
                "pending-abc".into()
            } else {
                "m1".into()
            },
            speaker,
            body: "Hello".into(),
            pending,
            ..MessageView::default()
        }
    }

    fn labels(items: &[MenuItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_ref()).collect()
    }

    fn approval_in(folder: Option<&str>) -> ApprovalView {
        ApprovalView {
            id: "apr_1".into(),
            task_id: Some("tsk_1".into()),
            summary: "Codex wants to run a command".into(),
            folder: folder.map(str::to_owned),
            detail: Some("rm -rf build".into()),
            ..ApprovalView::default()
        }
    }

    /// Review cycle 3, S6: task-thread and coordinator approval cards never
    /// showed the folder, and Codex lets the model pick it.
    #[test]
    fn the_folder_shows_with_or_without_the_summary_line() {
        let approval = approval_in(Some("/etc"));
        for with_summary in [true, false] {
            let parts = detail_parts(&approval, with_summary);
            assert!(
                parts.contains(&DetailPart::Folder("/etc".into())),
                "with_summary {with_summary}: {parts:?}"
            );
            assert!(parts.contains(&DetailPart::Command("rm -rf build".into())));
            assert_eq!(
                parts.contains(&DetailPart::Summary("Codex wants to run a command".into())),
                with_summary
            );
        }
        // The folder comes before the command.
        let parts = detail_parts(&approval, false);
        assert_eq!(
            parts,
            [
                DetailPart::Folder("/etc".into()),
                DetailPart::Command("rm -rf build".into())
            ]
        );
        // No folder, no row.
        assert_eq!(
            detail_parts(&approval_in(None), false),
            [DetailPart::Command("rm -rf build".into())]
        );
    }

    #[test]
    fn your_own_sent_message_can_be_edited_and_deleted() {
        let items = message_menu(&message(Speaker::You, false));
        assert_eq!(
            labels(&items),
            [
                "Copy text",
                "Copy as Markdown",
                "Reply in thread",
                "Quote in reply",
                "Edit",
                "Delete"
            ]
        );
    }

    #[test]
    fn a_pending_message_has_no_actions_that_need_a_real_id() {
        let items = message_menu(&message(Speaker::You, true));
        assert_eq!(
            labels(&items),
            ["Copy text", "Copy as Markdown", "Quote in reply"]
        );
        // Nothing in the menu carries the made-up ID.
        assert!(items.iter().all(|i| !matches!(
            &i.action,
            MenuAction::EditMessage(..) | MenuAction::DeleteMessage(..)
        )));
    }

    #[test]
    fn a_pending_message_or_post_shows_sending_in_place_of_its_time() {
        assert_eq!(time_text("9:12 AM", true), "Sending");
        assert_eq!(time_text("9:12 AM", false), "9:12 AM");
    }

    #[test]
    fn a_message_from_consort_has_no_edit_or_delete() {
        let items = message_menu(&message(Speaker::Consort, false));
        assert_eq!(
            labels(&items),
            [
                "Copy text",
                "Copy as Markdown",
                "Reply in thread",
                "Quote in reply"
            ]
        );
    }

    #[test]
    fn resolve_is_only_for_a_task_that_is_ready_for_review() {
        for status in [
            TaskStatus::Scheduled,
            TaskStatus::UpNext,
            TaskStatus::Working,
            TaskStatus::NeedsInput,
            TaskStatus::Failed,
        ] {
            let items = task_menu("t1", status, true);
            assert!(
                !labels(&items).contains(&"Resolve"),
                "{status:?} offers Resolve"
            );
            assert!(!labels(&items).contains(&"Not done"));
        }
        let review = task_menu("t1", TaskStatus::Review, true);
        assert_eq!(labels(&review), ["Open thread", "Resolve", "Copy task ID"]);
        let resolved = task_menu("t1", TaskStatus::Resolved, true);
        assert_eq!(
            labels(&resolved),
            ["Open thread", "Not done", "Archive", "Copy task ID"]
        );
    }

    #[test]
    fn retry_and_stop_match_the_status() {
        let failed = task_menu("t1", TaskStatus::Failed, true);
        assert_eq!(
            labels(&failed),
            ["Open thread", "Retry", "Archive", "Copy task ID"]
        );
        let working = task_menu("t1", TaskStatus::Working, true);
        assert_eq!(labels(&working), ["Open thread", "Stop", "Copy task ID"]);
    }

    /// Review cycle 3, finding 7: a failed task stayed in Active for ever.
    #[test]
    fn archive_is_for_a_failed_or_a_resolved_task_only() {
        for status in [
            TaskStatus::Scheduled,
            TaskStatus::UpNext,
            TaskStatus::Working,
            TaskStatus::NeedsInput,
            TaskStatus::Review,
        ] {
            let items = task_menu("t1", status, true);
            assert!(
                !labels(&items).contains(&"Archive"),
                "{status:?} offers Archive"
            );
        }
        for status in [TaskStatus::Failed, TaskStatus::Resolved] {
            let items = task_menu("t1", status, false);
            let archive = items
                .iter()
                .find(|i| i.label == "Archive")
                .expect("Archive");
            assert_eq!(archive.action, MenuAction::ArchiveTask("t1".into()));
        }
    }

    #[test]
    fn the_thread_menu_leaves_out_open_thread_by_a_flag_not_by_its_label() {
        let items = task_menu("t1", TaskStatus::Working, false);
        assert_eq!(labels(&items), ["Stop", "Copy task ID"]);
        assert!(
            items
                .iter()
                .all(|i| !matches!(i.action, MenuAction::OpenThread(_)))
        );
    }
}
