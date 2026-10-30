//! The main column: the timeline as bubbles, the Three voices mark and the
//! top actions over it, and the composer.

use std::time::Instant;

use consort_client::{
    ApprovalChoice, ApprovalView, ChannelKind, MessageView, RequestState, Speaker, TaskCardView,
    TaskStatus, TimelineItem,
    chat::{BubbleRow, ChatRow, Lane, Spacing},
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, App, Context, Div, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window,
    base::SelectableText, component::text::TextView, div, img, linear_color_stop, linear_gradient,
    list, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Panel},
    countdown,
    ui::window_controls::DragArea as _,
    ui::{
        links::click_action,
        mark::{MarkPhase, frame_path as mark_frame},
        markdown::{Segment, exact_selection, segments},
        menu::{MenuAction, MenuItem, OpenMenu, TextTarget, text_edit_menu},
        model_picker::{PickerTarget, TriggerStyle},
        motion::FrameClock,
        parts::{
            ButtonKind, button, detail, focusable, icon_button, line, selectable, status_glyph,
            text, worker_pill,
        },
    },
};

/// The progressive tint at the top of the timeline: one gradient layer
/// between each pair of stops of `layout.top_tint`, because a GPUI gradient
/// has two stops. Each layer is the canvas color at the opacity of its
/// stops, so the tint gets lighter from the window top to clear.
pub fn top_tint(t: &Tokens) -> Vec<Div> {
    let stops = t.layout.top_tint;
    let canvas = t.colors.surface_canvas;
    stops
        .windows(2)
        .enumerate()
        .map(|(ix, pair)| {
            let (from, to) = (pair[0], pair[1]);
            div()
                .debug_selector(move || format!("top-tint-{ix}"))
                .absolute()
                .top(from.at)
                .left_0()
                .right_0()
                .h(to.at - from.at)
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(canvas.opacity(from.alpha), 0.),
                    linear_color_stop(canvas.opacity(to.alpha), 1.),
                ))
        })
        .collect()
}

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
        // No header: the timeline fills the column, and the Three voices
        // mark and the top actions float over its top fade.
        // A drop anywhere over the chat attaches the files to the composer.
        // The composer is the only place that shows it.
        let column = div()
            .id("chat-column")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .on_drag_move::<gpui_kit::ExternalPaths>(cx.listener(
                |this, event: &gpui_kit::DragMoveEvent<gpui_kit::ExternalPaths>, _, cx| {
                    let inside = event.bounds.contains(&event.event.position);
                    let count = event.drag(cx).paths().len();
                    this.drag_files_over(crate::app::DropOver::Chat, inside, count, cx);
                },
            ))
            .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, _, cx| {
                this.drop_files(crate::app::DropOver::Chat, paths, cx);
            }))
            .child(self.render_timeline(t, window, cx))
            .child(self.render_composer(t, cx))
            .child(self.render_top_overlay(t, window, cx));
        let panel = match self.panel.clone() {
            Panel::None => None,
            Panel::Tasks => Some(self.render_tasks_panel(t, window, cx)),
            Panel::Thread(task_id) => Some(self.render_thread_panel(&task_id, t, window, cx)),
            Panel::Channel => Some(self.render_channel_panel(t, window, cx)),
        };
        div()
            .flex()
            .size_full()
            .child(sidebar)
            .child(column)
            .children(panel)
            .into_any_element()
    }

    /// The top of the column, over the timeline: a strip that moves the
    /// window, the Three voices mark with its activity pill at the center,
    /// and the Tasks, New task and Connectors buttons at the right.
    fn render_top_overlay(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let chat = t.component.chat;
        let state = self.state.clone();
        let activity = state.activity(&self.channel_id);
        let phase = self.mark_phase(cx);
        let tick = cx.global::<FrameClock>().tick();
        let reduce = cx.reduce_motion();
        let frame = self.mark.frame(phase, tick, reduce);
        if phase.animates() && !reduce {
            if phase.ticks_per_frame() > 1 {
                cx.global::<FrameClock>().mark_used_slow();
            } else {
                cx.global::<FrameClock>().mark_used();
            }
            // Load the frames of the loop before they show, so a frame
            // never draws empty while its image loads.
            for n in 0..phase.frames() {
                let path = mark_frame(phase, n, t.is_dark(), false);
                let _ = window.use_asset::<gpui_kit::ImgResourceLoader>(
                    &gpui_kit::Resource::Embedded(path),
                    cx,
                );
            }
        }
        // The pill shows only while the assistant works.
        let line = (phase == MarkPhase::Working).then(|| {
            activity
                .line
                .clone()
                .filter(|l| !l.trim().is_empty())
                .unwrap_or_else(|| "Working".to_owned())
        });
        let mark = div()
            .absolute()
            .top(chat.mark_top)
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(t.space.xxs)
            .child(
                div()
                    .id("three-voices")
                    .debug_selector(|| "three-voices".into())
                    .child(img(mark_frame(phase, frame, t.is_dark(), false)).size(chat.mark)),
            )
            .when_some(line, |d, line| {
                d.child(
                    div()
                        .id("activity-pill")
                        .debug_selector(|| "activity-pill".into())
                        .py(chat.pill_pad_y)
                        .px(chat.pill_pad_x)
                        .rounded(t.radius.full)
                        .bg(c.surface_card)
                        .border(t.border_width)
                        .border_color(c.border_subtle)
                        .child(text(line, detail(t), c.text_secondary).whitespace_nowrap()),
                )
            });
        // The top actions.
        let action = |id: &'static str, name: &str, this: &Self, cx: &App| {
            let _ = this;
            icon_button(
                id,
                name,
                chat.top_action,
                chat.top_action_icon,
                c.text_secondary,
                c.hover_canvas,
                t,
                cx,
            )
        };
        let pressed = self.panel == Panel::Tasks;
        let need_you = state.tasks_panel.need_you > 0;
        let tasks = action("tasks-button", "tasks", self, cx)
            .debug_selector(|| "tasks-button".into())
            .relative()
            .when(pressed, |d| d.bg(c.surface_selected))
            // A gold dot when something needs you.
            .when(need_you, |d| {
                d.child(
                    div()
                        .id("tasks-needs-you")
                        .debug_selector(|| "tasks-needs-you".into())
                        .absolute()
                        .top(t.space.sm - t.space.xxs)
                        .right(t.space.sm - t.space.xxs)
                        .size(t.component.state_dot)
                        .rounded(t.radius.full)
                        .bg(c.accent_fill),
                )
            })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_tasks(cx)));
        // The Channel panel: files now, routines and memory in phase 2.
        let channel_open = self.panel == Panel::Channel;
        let channel_button = action("channel-button", "channel", self, cx)
            .debug_selector(|| "channel-button".into())
            .when(channel_open, |d| d.bg(c.surface_selected))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_channel_panel(cx)));
        let new_task = action("new-task", "compose", self, cx)
            .debug_selector(|| "new-task".into())
            .on_click(cx.listener(|this, _, window, cx| this.start_task_draft(window, cx)));
        // The connectors and skills of the channel, for a channel that has
        // some, with the count of those that its sessions get.
        let enabled = consort_client::channel_extensions(&state, &self.channel_id)
            .map(|set| set.enabled_connector_ids().len() + set.enabled_skill_ids().len())
            .unwrap_or(0);
        let extensions_id = self.channel_id.clone();
        let extensions = (enabled > 0).then(|| {
            let id: gpui_kit::ElementId = "channel-extensions".into();
            let hovered = crate::ui::motion::is_hovered(&id, cx);
            let version = crate::ui::motion::shimmer_version(hovered, cx);
            let hover_id = id.clone();
            focusable(div().id(id), t)
                .flex()
                .flex_none()
                .items_center()
                .gap(t.space.xs)
                .h(chat.top_action)
                .px(t.space.sm)
                .rounded(t.radius.md)
                .cursor_pointer()
                .hover(|s| s.bg(c.hover_canvas))
                .on_hover(move |on, window, cx| {
                    if let Some(clock) = cx.try_global::<FrameClock>() {
                        clock.set_hovered(hover_id.clone(), *on);
                    }
                    window.refresh();
                })
                .child(crate::ui::parts::icon_version(
                    "connector",
                    version,
                    chat.top_action_icon,
                    c.text_secondary,
                ))
                .child(text(enabled.to_string(), t.text.ui, c.text_secondary))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_dialog(
                        crate::app::Dialog::ChannelExtensions(extensions_id.clone()),
                        window,
                        cx,
                    )
                }))
        });
        let actions = div()
            .absolute()
            .top(t.space.md)
            .right(t.space.md)
            .flex()
            .items_center()
            .gap(t.space.xs)
            .children(extensions)
            .child(channel_button)
            .child(tasks)
            .child(new_task);
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(t.layout.content_top)
            // The overlay must not take the clicks and the scroll of the
            // timeline under it, apart from its own parts.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(t.space.page)
                    .drag_area(),
            )
            .child(mark)
            .child(actions)
            .into_any_element()
    }

    fn render_timeline(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let empty = self.chat_rows.is_empty();
        // At the live edge nothing is cut below, so the bottom fade shows only
        // while the reader is scrolled up.
        let at_end = empty || self.timeline.is_scrolled_to_end().unwrap_or(true);
        let fade_bottom = t.layout.fade_bottom;
        let unloaded = self.channel_unloaded();
        let body = if let Some(waiting) = self.waiting() {
            // No channel has loaded. It is not a channel with no messages.
            self.render_waiting_timeline(&waiting, t, cx)
        } else if empty && unloaded {
            self.render_unloaded_timeline(t).into_any_element()
        } else if empty {
            self.render_empty_timeline(t).into_any_element()
        } else {
            let count = self.timeline_keys.len();
            let max_width = t.layout.timeline_max_width;
            let inset = t.space.xl;
            // The writing bubble follows the last row while a turn owes a
            // reply (`consort_client::proactivity::is_writing`).
            let writing = consort_client::proactivity::is_writing(&self.state, &self.channel_id);
            list(
                self.timeline.clone(),
                cx.processor(move |this, ix: usize, window, cx| {
                    let t = ConsortApp::tokens(cx);
                    let Some(row) = this.chat_rows.get(ix).cloned() else {
                        return div().into_any_element();
                    };
                    let (top, content) = this.render_chat_row(&row, &t, window, cx);
                    let bubble =
                        (writing && ix + 1 == count).then(|| this.render_writing_bubble(&t, cx));
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .px(inset)
                        .pt(top)
                        .when(ix + 1 == count, |d| d.pb(t.space.sm))
                        .child(
                            div()
                                .w_full()
                                .max_w(max_width)
                                .child(content)
                                .when_some(bubble, |d, bubble| {
                                    d.child(div().flex().pt(t.space.lg).child(bubble))
                                }),
                        )
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
                        .top(t.layout.content_top)
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
            // The chat never has a hard edge. At the top a progressive tint
            // darkens it under the mark, the pill and the top actions, so
            // content that moves up stays faintly visible. Above the
            // composer it fades into the canvas.
            .children(top_tint(t))
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
                    .flex()
                    .flex_col()
                    .items_center()
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

    /// One row of the chat: the space above it and its content.
    fn render_chat_row(
        &mut self,
        row: &ChatRow,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (gpui_kit::Pixels, AnyElement) {
        let spacing = |spacing: Spacing| match spacing {
            Spacing::Start => t.layout.content_top,
            Spacing::InGroup => t.space.xs,
            Spacing::NewGroup => t.space.lg,
            Spacing::AfterSeparator => gpui_kit::Pixels::ZERO,
        };
        match row {
            ChatRow::Separator { label, .. } => {
                let first = self.chat_rows.first().is_some_and(
                    |r| matches!(r, ChatRow::Separator { label: l, .. } if l == label),
                );
                (
                    if first {
                        t.layout.content_top
                    } else {
                        t.space.lg
                    },
                    div()
                        .flex()
                        .justify_center()
                        .pb(t.space.md)
                        .child(text(label.clone(), detail(t), t.colors.text_tertiary))
                        .into_any_element(),
                )
            }
            ChatRow::Bubble(bubble) => (
                spacing(bubble.spacing),
                self.render_bubble(bubble, t, window, cx),
            ),
            ChatRow::Card(card) => (
                spacing(card.spacing),
                self.render_card_row(card.item, t, cx),
            ),
            ChatRow::Attachment(row) => {
                (spacing(row.spacing), self.render_attachment_row(row, t, cx))
            }
            ChatRow::Notice { item, spacing: s } => {
                let notice = self
                    .state
                    .timeline(&self.channel_id)
                    .and_then(|tl| tl.items.get(*item))
                    .and_then(|i| match i {
                        TimelineItem::Notice(n) => Some(n.clone()),
                        _ => None,
                    });
                (
                    spacing(*s),
                    match notice {
                        Some(n) => self.render_notice_line(&n, t, cx),
                        None => div().into_any_element(),
                    },
                )
            }
        }
    }

    /// A system notice: one quiet centered line, with no speaker. The text
    /// is selectable. The hub writes the kind and the text; only the text
    /// shows.
    fn render_notice_line(
        &self,
        notice: &consort_client::NoticeView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(memory) = &notice.memory {
            return self.render_memory_saved_line(notice, memory, t, cx);
        }
        let c = &t.colors;
        let copy = notice.text.clone();
        div()
            .id(SharedString::from(format!("notice-{}", notice.message_id)))
            .flex()
            .justify_center()
            .items_baseline()
            .gap(t.space.sm)
            .child(
                selectable(
                    SharedString::from(format!("notice-line-{}", notice.message_id)),
                    notice.text.clone(),
                    t.text.ui,
                    c.text_tertiary,
                )
                .min_w_0(),
            )
            // The line that starts a channel sits under its time separator.
            .when(notice.kind != "channel_created", |d| {
                d.child(text(
                    notice.time_label.clone(),
                    t.text.caption,
                    c.text_tertiary,
                ))
            })
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

    /// One bubble: a part of a message. The assistant's bubbles sit at the
    /// left on `surface.card`, yours at the right on `accent.surface`. The
    /// last bubble of a group has the small corner on the speaker's side.
    fn render_bubble(
        &mut self,
        row: &BubbleRow,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let chat = t.component.chat;
        let Some(TimelineItem::Message(message)) = self
            .state
            .timeline(&self.channel_id)
            .and_then(|tl| tl.items.get(row.item))
            .cloned()
        else {
            return div().into_any_element();
        };
        let you = row.lane == Lane::You;
        let editing = you && self.editing.as_deref() == Some(message.id.as_str());
        let lane = div()
            .flex()
            .flex_col()
            .w_full()
            .when(you, |d| d.items_end())
            .when(!you, |d| d.items_start());
        if editing {
            let edit = self.render_edit_box(t, cx);
            return lane
                .child(div().w_full().max_w(t.layout.bubble_max_width).child(edit))
                .into_any_element();
        }
        let menu = message_menu(&message);
        let key = format!("{}.p{}", message.id, row.part);
        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(t.space.sm + t.space.xxs);
        if row.title {
            let title = message.title.clone().unwrap_or_default();
            let items = menu.clone();
            content = content.child(
                div()
                    .id(SharedString::from(format!("{key}.title")))
                    .font_weight(t.text.title.weight)
                    .child(SelectableText::new(
                        SharedString::from(format!("{key}.title.text")),
                        title,
                    ))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            open_context_menu(this, event, items.clone(), window, cx)
                        }),
                    ),
            );
        } else if let Some(brief) = (!you && consort_client::proactivity::is_brief(&message))
            .then(|| consort_client::proactivity::parse_brief(&message.body))
            .flatten()
        {
            content = content.child(self.render_brief(&key, &brief, t, cx));
        } else if !row.markdown.is_empty() {
            let code = if you {
                c.surface_card
            } else {
                c.surface_raised
            };
            content = content.child(self.render_markdown(&key, &row.markdown, menu, code, t, cx));
        }
        if row.with_items {
            content = content.child(self.render_brief_items(&message, t, cx));
        }
        let tail = chat.bubble_tail;
        let selector = format!("bubble-{key}");
        let bubble = div()
            .id(SharedString::from(format!("bubble-{key}")))
            .debug_selector(move || selector)
            .max_w(t.layout.bubble_max_width)
            .min_w_0()
            // Lists, tables, quotes and code cannot report a natural width,
            // so their bubble takes the full lane, up to the bubble width
            // (`needs_full_width`).
            .when(row.wide, |d| d.w_full())
            .bg(if you {
                c.accent_surface
            } else {
                c.surface_card
            })
            .rounded(chat.bubble_radius)
            .when(row.last_in_group && you, |d| d.rounded_br(tail))
            .when(row.last_in_group && !you, |d| d.rounded_bl(tail))
            .py(chat.bubble_pad_y)
            .px(chat.bubble_pad_x)
            .type_style(t.text.body)
            .text_color(c.text_primary)
            .child(content);
        // A message on its way, or one that you edited, says so under its
        // last bubble. The times are in the separators.
        let last_part = row.last_in_group
            || self.chat_rows.iter().all(|r| match r {
                ChatRow::Bubble(b) => b.item != row.item || b.part <= row.part,
                _ => true,
            });
        let note = if !last_part {
            None
        } else if message.pending {
            Some(time_text("", true).to_owned())
        } else if message.edited {
            Some("Edited".to_owned())
        } else {
            None
        };
        // A message that the assistant posted on its own: its mark above the
        // first bubble, Useful and Too noisy under the last one on hover,
        // and quick replies under the newest message.
        let caption = message
            .proactive
            .as_ref()
            .filter(|m| row.part == 0 && m.show_caption)
            .map(|m| self.render_mark_caption(m, t));
        let hovered = self.proactivity.hovered.as_deref() == Some(message.id.as_str());
        let feedback = message
            .proactive
            .as_ref()
            .filter(|m| last_part && (hovered || m.rating.is_some()))
            .map(|m| self.render_feedback(&message.id, m.rating, t, cx));
        // Chips show only under the newest message: any reply hides them.
        let newest = self.state.timeline(&self.channel_id).and_then(|tl| {
            tl.items
                .iter()
                .rposition(|i| matches!(i, TimelineItem::Message(_)))
        });
        let chips = (last_part && !message.suggestions.is_empty() && newest == Some(row.item))
            .then(|| self.render_chips(&message.suggestions, t, cx));
        let hover_id = message.id.clone();
        let proactive = message.proactive.is_some();
        let body = lane
            .when_some(caption, |d, caption| d.child(caption))
            .child(bubble)
            .when_some(note, |d, note| {
                d.child(
                    text(note, t.text.caption, c.text_tertiary)
                        .pt(t.space.xxs)
                        .px(chat.bubble_pad_x),
                )
            })
            .when_some(feedback, |d, feedback| d.child(feedback.pt(t.space.xs)))
            .when_some(chips, |d, chips| d.child(chips));
        // The pointer over a proactive message shows its feedback actions,
        // which sit under the bubble, inside this region.
        div()
            .id(SharedString::from(format!("message-{key}")))
            .w_full()
            .when(proactive, |d| {
                d.on_hover(cx.listener(move |this, on: &bool, _, cx| {
                    if *on {
                        this.proactivity.hovered = Some(hover_id.clone());
                    } else if this.proactivity.hovered.as_deref() == Some(hover_id.as_str()) {
                        this.proactivity.hovered = None;
                    }
                    cx.notify();
                }))
            })
            .child(body)
            .into_any_element()
    }

    /// The task references of a brief, as a numbered list in its bubble:
    /// the status and the task name in a chip that opens the thread, the
    /// channel, and the line about the task.
    fn render_brief_items(
        &mut self,
        message: &MessageView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let mut list = div().flex().flex_col().gap(t.space.sm + t.space.xxs);
        for (i, item) in message.items.iter().enumerate() {
            let task_id = item.task_id.clone();
            let name = task_id
                .as_deref()
                .and_then(|id| self.state.task(id))
                .map(|task| task.title.clone())
                .unwrap_or_else(|| item.channel_name.clone());
            let chip = div()
                .id(SharedString::from(format!("{}-item-{i}", message.id)))
                .flex()
                .flex_none()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .py(t.space.xxs)
                .pl(t.space.xs + t.space.xxs)
                .pr(t.space.sm)
                .rounded(t.radius.md)
                .bg(c.surface_raised)
                .when(task_id.is_some(), |d| {
                    d.cursor_pointer().hover(|s| s.bg(c.hover_raised))
                })
                .child(status_glyph(
                    SharedString::from(format!("{}-glyph-{i}", message.id)),
                    item.status,
                    t.icon.dense,
                    t,
                    cx,
                ))
                .child(text(name, t.text.ui_strong, c.text_primary).whitespace_nowrap())
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(id) = task_id.clone() {
                        this.open_thread(id, window, cx);
                    }
                }));
            list = list.child(
                div()
                    .flex()
                    .items_start()
                    .gap(t.space.sm)
                    .child(
                        text(format!("{}.", i + 1), t.text.body, c.text_tertiary)
                            .w(t.component.chat.brief_number)
                            .flex_none(),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(t.space.xs)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(t.space.sm)
                                    .child(chip)
                                    .child(
                                        text(item.channel_name.clone(), detail(t), c.text_tertiary)
                                            .whitespace_nowrap(),
                                    ),
                            )
                            .child(selectable(
                                SharedString::from(format!("{}-item-text-{i}", message.id)),
                                item.text.clone(),
                                t.text.body,
                                c.text_primary,
                            )),
                    ),
            );
        }
        list
    }

    /// The line above a card in the timeline: a curved connector, the
    /// status, the name and the state.
    #[allow(clippy::too_many_arguments)]
    fn task_line(
        &self,
        key: &str,
        status: TaskStatus,
        name: String,
        state: String,
        state_color: Hsla,
        place: Option<String>,
        t: &Tokens,
        cx: &App,
    ) -> Div {
        let c = &t.colors;
        div()
            .flex()
            .items_end()
            .gap(t.space.xs + t.space.xxs)
            .pl(t.space.sm + t.space.xxs)
            .child(crate::ui::parts::glyph(
                "task-connector",
                t.component.chat.task_connector,
                c.border_strong,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .gap(t.space.xs + t.space.xxs)
                    .child(status_glyph(
                        SharedString::from(format!("{key}-line-glyph")),
                        status,
                        t.icon.dense,
                        t,
                        cx,
                    ))
                    .child(line(name, t.text.ui_strong, c.text_primary))
                    .child(text(state, t.text.ui, state_color).whitespace_nowrap())
                    .when_some(place, |d, place| {
                        d.child(text(place, t.text.ui, c.text_tertiary).whitespace_nowrap())
                    }),
            )
    }

    /// A card in the assistant lane, under its task line: a task card, or
    /// an approval or a decision of the channel's assistant.
    fn render_card_row(&mut self, item: usize, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let Some(item) = self
            .state
            .timeline(&self.channel_id)
            .and_then(|tl| tl.items.get(item))
            .cloned()
        else {
            return div().into_any_element();
        };
        let request_state = |state: RequestState| match state {
            RequestState::Pending | RequestState::Sending => {
                ("Needs you".to_owned(), c.accent_text)
            }
            RequestState::Answered => ("Answered".to_owned(), c.text_tertiary),
            RequestState::Expired | RequestState::Closed => ("Closed".to_owned(), c.text_tertiary),
        };
        let card_frame = |content: AnyElement| {
            div()
                .mt(t.space.xs + t.space.xxs)
                .w_full()
                .max_w(t.layout.bubble_max_width)
                .flex()
                .child(content)
        };
        match item {
            TimelineItem::TaskCard(card) => self.render_task_card_row(&card, t, cx),
            TimelineItem::Approval(card) => {
                let (state, color) = request_state(card.approval.state);
                let element =
                    self.render_approval_card(&card.approval, &card.message_id, true, t, cx);
                div()
                    .flex()
                    .flex_col()
                    .child(self.task_line(
                        &card.message_id,
                        crate::ui::panels::request_glyph(card.approval.state),
                        "Approval".to_owned(),
                        state,
                        color,
                        None,
                        t,
                        cx,
                    ))
                    .child(card_frame(element))
                    .into_any_element()
            }
            TimelineItem::Decision(card) if card.proactive.as_ref().is_some_and(|m| m.offer) => {
                let mark = card.proactive.clone().unwrap_or_default();
                let element = self.render_offer_card(&card.decision, t, cx);
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .child(self.render_mark_caption(&mark, t))
                    .child(element)
                    .into_any_element()
            }
            TimelineItem::Decision(card) => {
                let (state, color) = request_state(card.decision.state);
                let element = self.render_decision(&card.decision, &card.message_id, false, t, cx);
                div()
                    .flex()
                    .flex_col()
                    .child(self.task_line(
                        &card.message_id,
                        crate::ui::panels::request_glyph(card.decision.state),
                        "Decision".to_owned(),
                        state,
                        color,
                        None,
                        t,
                        cx,
                    ))
                    .child(card_frame(element))
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
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

    /// A task card under its task line. The line names the task and its
    /// state, so the card has no header row of its own. A resolved task
    /// with nothing pending is the line alone, with its summary.
    fn render_task_card_row(
        &mut self,
        card: &TaskCardView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let chat = t.component.chat;
        let collapsed = card
            .approval
            .as_ref()
            .is_none_or(|a| a.state == RequestState::Answered)
            && matches!(card.status, TaskStatus::Resolved);
        let open = self.panel == Panel::Thread(card.task_id.clone());
        let status = card.status;
        let state = if collapsed && !card.summary.is_empty() {
            card.summary.clone()
        } else if status == TaskStatus::NeedsInput {
            "Needs you".to_owned()
        } else {
            status.label().to_owned()
        };
        let place = card.channel_name.clone().map(|name| format!("in {name}"));
        let task_id = card.task_id.clone();
        let menu_task = card.task_id.clone();
        let line = self
            .task_line(
                &card.message_id,
                status,
                card.title.clone(),
                state,
                crate::ui::parts::status_color(status, t),
                place,
                t,
                cx,
            )
            .id(SharedString::from(format!("task-line-{}", card.message_id)))
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, _, window, cx| {
                    this.open_thread(task_id.clone(), window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    open_context_menu(this, event, task_menu(&menu_task, status, true), window, cx)
                }),
            );
        if collapsed {
            return line.into_any_element();
        }
        let menu_task = card.task_id.clone();
        // The worker and the approval mode sit at the right of the first row.
        let place_chips = |row: Div| {
            row.child(
                text(card.approval_mode.tag(), detail(t), c.text_tertiary).whitespace_nowrap(),
            )
            .when_some(card.worker.clone(), |d, w| {
                d.child(worker_pill(&w.name, &w.account, w.online, t))
            })
        };
        let mut body = div()
            .id(SharedString::from(format!("card-{}", card.message_id)))
            .mt(t.space.xs + t.space.xxs)
            .w_full()
            .max_w(t.layout.bubble_max_width)
            .flex()
            .flex_col()
            .gap(t.space.md)
            .pt(t.space.md)
            .pb(chat.bubble_pad_x)
            .px(chat.bubble_pad_x)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(if open { c.accent_text } else { c.border_subtle })
            .rounded(chat.bubble_radius)
            .rounded_tl(chat.bubble_tail)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    open_context_menu(this, event, task_menu(&menu_task, status, true), window, cx)
                }),
            );
        let pending = card
            .approval
            .clone()
            .filter(|a| a.state != RequestState::Answered);
        if let Some(approval) = pending.clone()
            && let Some(hub_card) = approval.card.clone()
        {
            // A connector approval from the task: the hub card, inside the
            // task card.
            let key = format!("{}-hub-card", card.message_id);
            body = body
                .child(place_chips(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.sm)
                        .child(div().flex_1()),
                ))
                .child(self.render_hub_card(&approval, &hub_card, &key, false, t, cx))
                .child(
                    div()
                        .flex()
                        .child(div().flex_1())
                        .child(self.open_thread_link(&card.task_id, open, t, cx)),
                );
        } else if let Some(approval) = pending {
            // What asks, then the folder and the command, then the answers.
            let summary = selectable(
                SharedString::from(format!("{}.summary", approval.id)),
                approval.summary.clone(),
                t.text.ui,
                c.text_secondary,
            )
            .flex_1()
            .min_w_0();
            body = body
                .child(place_chips(
                    div().flex().items_center().gap(t.space.sm).child(summary),
                ))
                .child(self.render_approval_detail(&approval, false, t, cx))
                .child(
                    self.render_approval_buttons(&approval, t, cx)
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
            let (summary, color) = if summary.is_empty() {
                (card.summary.clone(), c.text_tertiary)
            } else {
                (summary, c.text_secondary)
            };
            // The text is selectable: it holds the reason for a failed task
            // or the review summary, and the user may copy it.
            body = body.child(place_chips(
                div().flex().items_start().gap(t.space.sm).child(
                    selectable(
                        SharedString::from(format!("card-latest-{}", card.message_id)),
                        summary,
                        t.text.ui,
                        color,
                    )
                    .flex_1()
                    .min_w_0()
                    .line_clamp(3)
                    .text_ellipsis(),
                ),
            ));
            // The actions sit in one row at the bottom, as on an approval:
            // Resolve for a task in review, and Open thread at the right.
            let task_id = card.task_id.clone();
            body = body.child(
                div()
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
        div()
            .flex()
            .flex_col()
            .child(line)
            .child(body)
            .into_any_element()
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

    /// The composer: one rounded field with the send key inside at the
    /// right, and under it a quiet row of tools: add, voice, the model and
    /// effort chips, and where the assistant runs.
    fn render_composer(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let chat = t.component.chat;
        let state = self.state.clone();
        let channel = state.channel(&self.channel_id);
        // A channel without its own model says that it uses Tutti's.
        let inherited = channel.is_some_and(|ch| {
            ch.kind != ChannelKind::Tutti
                && ch
                    .coordinator_selection
                    .as_ref()
                    .is_some_and(|e| e.source == consort_client::SelectionSource::Tutti)
        });
        let runs_on = channel
            .and_then(|ch| ch.coordinator_worker.clone())
            .filter(|w| !w.trim().is_empty());
        let target = PickerTarget::Assistant(self.channel_id.clone());
        let model_chip = channel.map(|_| {
            self.render_model_trigger("composer-model", target.clone(), TriggerStyle::Chip, t, cx)
        });
        let effort_chip = channel.and_then(|_| {
            self.render_effort_trigger("composer-effort", target.clone(), TriggerStyle::Chip, t, cx)
        });
        let input = crate::ui::parts::textarea(&self.inputs.composer, t.text.body);
        let target = consort_client::UploadTarget::Composer {
            channel_id: self.channel_id.clone(),
        };
        let chips = self.render_upload_chips(&target, t, cx);
        let dropping = self
            .drop_hover
            .filter(|(over, _)| *over == crate::app::DropOver::Chat)
            .map(|(_, count)| count)
            .filter(|_| self.waiting().is_none());
        let drop_zone = dropping.map(|count| self.render_drop_zone(count, t));
        let paste_target = target.clone();
        let add_target = target.clone();
        let send = icon_button(
            "send",
            "send",
            chat.send_button,
            t.icon.size,
            c.text_tertiary,
            c.hover_card,
            t,
            cx,
        )
        .rounded(t.radius.lg)
        .debug_selector(|| "send".into())
        .on_click(cx.listener(|this, _, window, cx| {
            this.send_message(window, cx);
            // The button takes the focus. The composer gets it back, so
            // the user can go on typing.
            this.inputs
                .composer
                .update(cx, |input, cx| input.focus(window, cx));
        }));
        let tool = |id: &'static str, name: &str, cx: &App| {
            icon_button(
                id,
                name,
                t.component.composer_chip,
                t.icon.size,
                c.text_secondary,
                c.hover_canvas,
                t,
                cx,
            )
        };
        // The line above the composer: the state of the link, and for an
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
            .gap(t.space.xs + t.space.xxs)
            .px(t.space.xl)
            .pt(t.space.md)
            .pb(t.space.lg)
            .when_some(link, |d, (line_text, reason, offline)| {
                d.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(t.space.sm)
                        .w_full()
                        .max_w(t.layout.timeline_max_width)
                        .pb(t.space.xs)
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
                    .debug_selector(|| "composer".into())
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .w_full()
                    .max_w(t.layout.timeline_max_width)
                    // No channel has loaded: the box takes no text.
                    .when(self.waiting().is_some(), |d| d.opacity(t.opacity.busy))
                    .p(t.space.sm)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .rounded(chat.composer_radius)
                    // Files dragged over the chat: the field is the drop
                    // zone, with a dashed accent border and a soft fill.
                    .when(dropping.is_some(), |d| {
                        d.border(t.stroke)
                            .border_dashed()
                            .border_color(c.accent_text)
                            .bg(c.accent_fill.opacity(t.opacity.drop_fill))
                            .pb(t.space.md + t.space.xxs)
                    })
                    // A picture or a copied file on the clipboard attaches.
                    // Text pastes as usual.
                    .capture_action(cx.listener(
                        move |this, _: &gpui_kit::base::input::Paste, _, cx| {
                            if this.paste_files(paste_target.clone(), cx) {
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .children(chips)
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap(t.space.sm)
                            .pl(t.space.sm)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .py(t.space.xs)
                                    .type_style(t.text.body)
                                    .text_color(c.text_primary)
                                    .child(input),
                            )
                            .child(send),
                    )
                    .children(drop_zone)
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            let items = text_edit_menu(TextTarget::Composer);
                            this.open_menu(OpenMenu::at(event.position, items), cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .id("composer-tools")
                    .debug_selector(|| "composer-tools".into())
                    .flex()
                    .items_center()
                    .gap(t.space.xxs)
                    .w_full()
                    .max_w(t.layout.timeline_max_width)
                    .px(t.space.xs)
                    .child(
                        tool("attach", "plus", cx)
                            .debug_selector(|| "attach".into())
                            .relative()
                            .child(self.triggers.track("composer-attach"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_add_menu("composer-attach", add_target.clone(), cx)
                            })),
                    )
                    .child(
                        tool("voice", "voice", cx).on_click(cx.listener(|this, _, _, cx| {
                            this.show_notice("Voice input comes in a later build.".into(), cx)
                        })),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(t.border_width)
                            .h(t.icon.size)
                            .mx(t.space.xs + t.space.xxs)
                            .bg(c.border_strong),
                    )
                    .when(inherited, |d| {
                        d.child(
                            text(
                                format!("Same as {}", crate::ui::TUTTI),
                                detail(t),
                                c.text_tertiary,
                            )
                            .flex_none()
                            .pr(t.space.xs),
                        )
                    })
                    .children(model_chip)
                    .children(effort_chip)
                    .child(div().flex_1())
                    .when_some(runs_on, |d, worker| {
                        d.child(
                            text(format!("Runs on {worker}"), detail(t), c.text_tertiary)
                                .whitespace_nowrap()
                                .id("runs-on")
                                .debug_selector(|| "runs-on".into()),
                        )
                    }),
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
