//! The main column: the timeline as bubbles, the Three voices mark and the
//! top actions over it, and the composer.

use std::time::Instant;

use consort_client::{
    ApprovalChoice, ApprovalView, ChannelKind, MessageView, RequestState, Speaker, TaskCardView,
    TaskStatus, TimelineItem,
    chat::{BubbleRow, ChatRow, Lane, Spacing},
};
use consort_theme::{Tokens, Type, TypeExt as _};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, Hsla, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement as _,
    Styled, Window, base::SelectableText, component::text::TextView, div, img, linear_color_stop,
    linear_gradient, list, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Panel},
    countdown,
    ui::window_controls::DragArea as _,
    ui::{
        links::click_action,
        mark::{MarkPhase, frame_path as mark_frame},
        markdown::{Segment, exact_selection, segments},
        menu::{MenuAction, MenuItem, OpenMenu},
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
            MenuAction::ReplyInThread(message.id.clone()),
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
/// is for a failed task, and for a task in Ready for review or Resolved
/// whose session ended. Stop is for a working task, and Archive for a failed
/// or a resolved one. Delete is last. The thread panel leaves out Open
/// thread, because the thread is open.
pub fn task_menu(
    task_id: &str,
    status: TaskStatus,
    session_ended: bool,
    with_open_thread: bool,
) -> Vec<MenuItem> {
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
    if consort_client::convo::can_retry(status, session_ended) {
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
    items.push(MenuItem::new("Copy task ID", MenuAction::Copy(id.clone())).separated());
    items.push(
        MenuItem::new(
            "Delete",
            MenuAction::Convo(crate::convo::ConvoAction::DeleteTask(id)),
        )
        .danger(),
    );
    items
}

/// Markdown as the reader sees it, for "Copy text". Code keeps every
/// character.
pub(crate) fn plain_text(markdown: &str) -> String {
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
        self.layout.window_width = window.viewport_size().width;
        let frame = self.layout_frame(t);
        self.follow_frame(frame, t, cx);
        let sidebar_width = if frame.sidebar {
            t.layout.sidebar_width
        } else {
            gpui_kit::Pixels::ZERO
        };
        let beside = if frame.panel_over {
            gpui_kit::Pixels::ZERO
        } else {
            gpui_kit::px(frame.panel)
        };
        let column = (self.layout.window_width - sidebar_width - beside - t.space.xl * 2.0)
            .min(t.layout.timeline_max_width);
        let narrow = column < t.layout.wide_timeline;
        if narrow != self.narrow {
            self.narrow = narrow;
            self.timeline.remeasure();
        }
        let sidebar = (frame.sidebar || self.layout.sliding || self.layout.drawer_open)
            .then(|| self.render_sidebar(t, window, cx));
        let (slot, drawer) = if self.layout.drawer_open {
            (None, sidebar)
        } else {
            (sidebar, None)
        };
        let slot = self.render_sidebar_slot(slot, frame, t);
        let buttons = self.render_sidebar_buttons(frame, t, cx);
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
            .child(self.render_composer(t, window, cx))
            .child(self.render_top_overlay(frame, t, window, cx));
        let panel = match self.panel.clone() {
            Panel::None => None,
            Panel::Tasks => Some(self.render_tasks_panel(t, window, cx)),
            Panel::Thread(task_id) => Some(self.render_thread_panel(&task_id, t, window, cx)),
            Panel::Channel => Some(self.render_channel_panel(t, window, cx)),
            Panel::MessageThread(root) => {
                Some(self.render_message_thread_panel(&root, t, window, cx))
            }
            Panel::Document => Some(self.render_document_panel(t, window, cx)),
        };
        let (panel_beside, panel_over) = match panel {
            Some(panel) if frame.panel_over => (None, Some(self.render_panel_over(panel, t, cx))),
            Some(panel) => (
                Some(
                    div()
                        .relative()
                        .flex_none()
                        .h_full()
                        .child(panel)
                        .child(self.render_resize_handle(t, cx)),
                ),
                None,
            ),
            None => (None, None),
        };
        let drawer = drawer.map(|sidebar| self.render_drawer(sidebar, t, cx));
        // A drawer has its buttons on top of it, and a full panel covers them.
        div()
            .id("main-page")
            .relative()
            .flex()
            .size_full()
            .on_drag_move::<crate::ui::window_layout::PanelResize>(cx.listener(
                |this,
                 event: &gpui_kit::DragMoveEvent<crate::ui::window_layout::PanelResize>,
                 _,
                 cx| {
                    let t = ConsortApp::tokens(cx);
                    this.drag_panel_edge(event.event.position.x, &t, cx);
                },
            ))
            .child(slot)
            .child(column)
            .children(panel_beside)
            .children(drawer)
            .children(panel_over.is_none().then_some(buttons).flatten())
            .children(panel_over)
            .into_any_element()
    }

    /// The top of the column, over the timeline: a strip that moves the
    /// window, the Three voices mark with its activity pill at the center,
    /// and the Channel and Tasks toggles at the right. The whole zone belongs
    /// to the window, not to the timeline: it takes the clicks, has the arrow
    /// cursor and moves the window. In the compact layout it is the top bar.
    fn render_top_overlay(
        &mut self,
        shape: crate::layout::Frame,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let chat = t.component.chat;
        let state = self.state.clone();
        let activity = state.activity(&self.channel_id);
        // While the hub is offline, the screen shows saved data. The mark
        // rests and fades, and its pill says so (Paper `ERQ-0`).
        let saved = state.saved_data.clone();
        let phase = if saved.is_some() {
            MarkPhase::Idle
        } else {
            self.mark_phase(cx)
        };
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
        let line = (phase == MarkPhase::Working && !shape.compact).then(|| {
            activity
                .line
                .clone()
                .filter(|l| !l.trim().is_empty())
                .unwrap_or_else(|| "Working".to_owned())
        });
        // The compact top bar: the channel name under the mark opens the
        // Channel panel.
        let channel_name = shape.compact.then(|| {
            let channel = state.channel(&self.channel_id);
            let name = channel.map(|ch| ch.name.clone()).unwrap_or_default();
            if channel.is_some_and(|ch| ch.kind == ChannelKind::Tutti) {
                name
            } else {
                format!("# {name}")
            }
        });
        let no_account = self.no_account_in_tutti();
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
                    .child(if no_account {
                        crate::ui::mark::setup_mark(consort_client::setup::Dabs::Hub, chat.mark, t)
                    } else {
                        img(mark_frame(phase, frame, t.is_dark(), false))
                            .size(chat.mark)
                            .when(saved.is_some(), |i| i.opacity(t.opacity.offline_mark))
                            .into_any_element()
                    }),
            )
            .when_some(saved, |d, saved| {
                d.child(
                    div()
                        .id("offline-pill")
                        .debug_selector(|| "offline-pill".into())
                        .flex()
                        .items_center()
                        .gap(chat.pill_pad_x)
                        .py(chat.pill_pad_y)
                        .pl(chat.pill_pad_x)
                        .pr(t.space.xs)
                        .rounded(t.radius.full)
                        .bg(c.surface_card)
                        .border(t.border_width)
                        .border_color(c.border_subtle)
                        .child(
                            text(
                                if matches!(
                                    state.connection,
                                    consort_client::Connection::Incompatible { .. }
                                ) {
                                    saved.incompatible_pill_text()
                                } else {
                                    saved.pill_text()
                                },
                                detail(t),
                                c.text_secondary,
                            )
                            .whitespace_nowrap(),
                        )
                        .child(
                            div()
                                .id("offline-try-again")
                                .debug_selector(|| "offline-try-again".into())
                                .occlude()
                                .px(t.space.sm)
                                .py(t.border_width)
                                .rounded(t.radius.full)
                                .bg(c.surface_selected)
                                .cursor_pointer()
                                .hover(|s| s.bg(c.hover_raised))
                                .child(text(
                                    "Try again",
                                    Type {
                                        weight: t.text.ui_strong.weight,
                                        ..detail(t)
                                    },
                                    c.text_primary,
                                ))
                                .on_click(cx.listener(|this, _, _, cx| this.try_again(cx))),
                        ),
                )
            })
            .when_some(channel_name, |d, name| {
                d.child(
                    focusable(div().id("channel-pill"), t)
                        .debug_selector(|| "channel-pill".into())
                        .occlude()
                        .py(chat.pill_pad_y)
                        .px(chat.pill_pad_x)
                        .rounded(t.radius.full)
                        .bg(c.surface_card)
                        .border(t.border_width)
                        .border_color(c.border_subtle)
                        .cursor_pointer()
                        .hover(|s| s.bg(c.hover_card))
                        .child(text(name, detail(t), c.text_secondary).whitespace_nowrap())
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_channel_panel(cx))),
                )
            })
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
        let actions = div()
            .absolute()
            .top(t.space.md)
            .right(t.space.md)
            .flex()
            .items_center()
            .gap(t.space.xs)
            .occlude();
        let actions = if shape.compact {
            actions
                .child(
                    focusable(div().id("compact-avatar"), t)
                        .debug_selector(|| "compact-avatar".into())
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(t.component.sidebar.avatar)
                        .rounded(t.radius.full)
                        .bg(c.surface_selected)
                        .cursor_pointer()
                        .child(
                            div()
                                .type_style(t.text.caption)
                                .font_weight(t.text.title.weight)
                                .text_color(c.text_primary)
                                .child(state.user.initials.clone()),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_settings(crate::app::SettingsSection::Appearance, cx);
                            this.layout.settings_list = true;
                        })),
                )
                .child(tasks)
        } else {
            actions.child(channel_button).child(tasks)
        };
        let menu_button = shape.compact.then(|| {
            div()
                .absolute()
                .top(crate::ui::window_layout::top_on_light_line(
                    chat.top_action,
                    t,
                ))
                .left(crate::ui::window_layout::closed_left(t))
                .occlude()
                .child(
                    action("compact-menu", "menu", self, cx)
                        .debug_selector(|| "compact-menu".into())
                        .tooltip(|_, cx| {
                            cx.new(|_| crate::ui::convo_sidebar::Tip("Menu".into()))
                                .into()
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
                )
        });
        div()
            .id("top-zone")
            .debug_selector(|| "top-zone".into())
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(t.layout.content_top)
            // The zone takes the clicks and the text cursor, not the timeline
            // under it. A wheel over it still scrolls the timeline.
            .block_mouse_except_scroll()
            .cursor_default()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .cursor_default()
                    .drag_zone(),
            )
            .child(mark)
            .child(actions)
            .children(menu_button)
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
        } else if empty && self.no_account_in_tutti() {
            crate::ui::setup::no_account_state(t, cx).into_any_element()
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
                    // The pointer on any row of a message counts for the
                    // footer of its group.
                    let hover_item = match &row {
                        ChatRow::Bubble(b) => Some(b.item),
                        ChatRow::Attachment(a) => Some(a.item),
                        ChatRow::Footer { item, .. } | ChatRow::Actions { item, .. } => Some(*item),
                        _ => None,
                    };
                    div()
                        .id(("chat-row", ix))
                        .when_some(hover_item, |d, item| {
                            d.on_hover(cx.listener(move |this, on: &bool, _, cx| {
                                if *on {
                                    this.chat_hover = Some((ix, item));
                                } else if this.chat_hover.is_some_and(|(row, _)| row == ix) {
                                    this.chat_hover = None;
                                }
                                cx.notify();
                            }))
                        })
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
        // The clear part of the timeline for the cover rule of visuals. It
        // comes before the list, so it prepaints before the items.
        let area = {
            let t = t.clone();
            self.visuals
                .frame
                .area_probe(crate::visual::Place::Timeline, move |viewport| {
                    crate::ui::visual_cover::TimelineArea::of_timeline(viewport, &t, at_end)
                })
        };
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .w_full()
            .child(area)
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
            ChatRow::Footer { item, lane } => (
                t.component.convo.reaction_gap,
                self.render_message_footer(*item, *lane == Lane::You, t, cx),
            ),
            ChatRow::Actions { first, item, lane } => (
                gpui_kit::Pixels::ZERO,
                self.render_group_footer(*first, *item, *lane == Lane::You, t, cx),
            ),
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
                        // A visual is a timeline item of its own.
                        Some(n) if n.visual.is_some() => match &n.visual {
                            Some(visual) => self.render_visual_item(
                                visual,
                                crate::visual::Place::Timeline,
                                t,
                                window,
                                cx,
                            ),
                            None => div().into_any_element(),
                        },
                        Some(n) => self.render_notice_line(&n, t, window, cx),
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
        &mut self,
        notice: &consort_client::NoticeView,
        t: &Tokens,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The text that `show_draft` shows is a card, left, at the bubble
        // width.
        if let Some(draft) = &notice.draft {
            let key = format!("{}-shown", notice.message_id);
            let card = self.render_shown_draft(draft, &key, true, t, cx);
            return div()
                .flex()
                .w_full()
                .max_w(t.layout.bubble_max_width)
                .child(card)
                .into_any_element();
        }
        // A failed turn is a card in the assistant lane at the bubble width.
        if let Some(card) = &notice.error {
            let key = format!("err-{}", notice.message_id);
            let place = crate::ui::errors::ErrorPlace::Channel(self.channel_id.clone());
            let shows_card = !card.quiet || self.errors_open.contains(&key);
            let element = self.render_error_card(card, &key, place, t, cx);
            let time_id = format!("error-time-{key}");
            return div()
                .flex()
                .flex_col()
                .items_start()
                .w_full()
                .child(element)
                // The footer line keeps its time, under the right end of the
                // card. A quiet line has none.
                .when(shows_card, |d| {
                    d.child(
                        div()
                            .flex()
                            .justify_end()
                            .w_full()
                            .max_w(t.layout.bubble_max_width)
                            .pt(t.space.xxs)
                            .child(div().debug_selector(move || time_id).child(text(
                                notice.time_label.clone(),
                                t.text.caption,
                                t.colors.text_tertiary,
                            ))),
                    )
                })
                .into_any_element();
        }
        if let Some(row) = &notice.monid {
            return self.render_monid_call(row, "chat", t, cx);
        }
        if let Some(memory) = &notice.memory {
            return self.render_memory_saved_line(notice, memory, t, cx);
        }
        if let Some(fallback) = &notice.fallback {
            let channel_id = self.channel_id.clone();
            return self.render_fallback_notice(
                fallback,
                &notice.message_id,
                &notice.time_label,
                &channel_id,
                false,
                t,
                cx,
            );
        }
        let c = &t.colors;
        // A line of the assistant's own work (Paper T5), in its lane.
        if notice.kind == "channel_made" {
            return div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .child(crate::ui::parts::icon("check", t.icon.dense, c.status_done))
                .child(
                    selectable(
                        SharedString::from(format!("notice-line-{}", notice.message_id)),
                        notice.text.clone(),
                        t.text.ui,
                        c.text_secondary,
                    )
                    .flex_1()
                    .min_w_0(),
                )
                .into_any_element();
        }
        let copy = notice.text.clone();
        // The time shares the baseline of the first line of the text. The
        // two boxes start at the same top, and the time moves down by the
        // difference of the two baselines (see `baseline_lead`).
        let lead = crate::ui::parts::baseline_lead(window, t.text.ui, t.text.caption, t);
        let time_id = format!("notice-time-{}", notice.message_id);
        let text_id = format!("notice-text-{}", notice.message_id);
        div()
            .id(SharedString::from(format!("notice-{}", notice.message_id)))
            .flex()
            .justify_center()
            .items_start()
            .gap(t.space.sm)
            .child(
                selectable(
                    SharedString::from(format!("notice-line-{}", notice.message_id)),
                    notice.text.clone(),
                    t.text.ui,
                    c.text_tertiary,
                )
                .min_w_0()
                .debug_selector(move || text_id),
            )
            // The line that starts a channel sits under its time separator.
            .when(
                !matches!(notice.kind.as_str(), "channel_created" | "signed_in"),
                |d| {
                    d.child(
                        div().flex_none().pt(lead).child(
                            text(notice.time_label.clone(), t.text.caption, c.text_tertiary)
                                .whitespace_nowrap()
                                .debug_selector(move || time_id),
                        ),
                    )
                },
            )
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
        let hub_down = self.state.connection.is_hub_down();
        let queued = message.pending && hub_down;
        let selector = format!("bubble-{key}");
        // While the thread of this message is open, its bubbles have the
        // accent ring, as an open task card does.
        let ring = crate::ui::convo_chat::thread_open_for(&self.panel, &message.id);
        let menu_message = message.clone();
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
            .when(ring, |d| {
                d.border(t.component.convo.root_ring)
                    .border_color(c.accent_text)
            })
            .py(chat.bubble_pad_y)
            .px(chat.bubble_pad_x)
            .type_style(t.text.body)
            .text_color(c.text_primary)
            // A message that waits for the hub is a little faded, and one
            // that the hub refused is dimmed.
            .when(queued, |d| d.opacity(t.opacity.queued))
            .when(message.refused.is_some(), |d| {
                d.opacity(t.opacity.answered_dim)
            })
            // The message menu with the quick emoji, before the text's own.
            .capture_any_mouse_down(
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if event.button == MouseButton::Right {
                        crate::ui::convo_chat::open_message_menu(
                            this,
                            event,
                            &menu_message,
                            true,
                            window,
                            cx,
                        );
                        cx.stop_propagation();
                    }
                }),
            )
            .child(content);
        // A message on its way, or one that you edited, says so under its
        // last bubble. The times are in the separators.
        let last_part = row.last_in_group
            || self.chat_rows.iter().all(|r| match r {
                ChatRow::Bubble(b) => b.item != row.item || b.part <= row.part,
                _ => true,
            });
        let refused = message
            .refused
            .clone()
            .filter(|_| last_part)
            .map(|refused| self.render_refused_note(&refused, &key, t, cx));
        let note = if !last_part || refused.is_some() {
            None
        } else if message.pending {
            Some(pending_text(hub_down).to_owned())
        } else if message.edited {
            Some("Edited".to_owned())
        } else {
            None
        };
        // A message that the assistant posted on its own: its mark above the
        // first bubble, and quick replies under the newest message. Useful
        // and Too noisy are in the footer of its group.
        let caption = message
            .proactive
            .as_ref()
            .filter(|m| row.part == 0 && m.show_caption)
            .map(|m| self.render_mark_caption(m, t));
        // Chips show only under the newest message: any reply hides them.
        let newest = self.state.timeline(&self.channel_id).and_then(|tl| {
            tl.items
                .iter()
                .rposition(|i| matches!(i, TimelineItem::Message(_)))
        });
        let chips = (last_part && !message.suggestions.is_empty() && newest == Some(row.item))
            .then(|| self.render_chips(&message.suggestions, t, cx));
        let body = lane
            .when_some(caption, |d, caption| d.child(caption))
            .child(bubble)
            .when_some(note, |d, note| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.xs + t.space.xxs)
                        .pt(t.space.xxs)
                        .px(chat.bubble_pad_x)
                        // The glyph of a waiting message, in place of the
                        // plain note.
                        .when(queued, |row| {
                            row.child(crate::ui::parts::icon(
                                "working",
                                t.icon.dense,
                                c.text_tertiary,
                            ))
                        })
                        .child(text(note, t.text.caption, c.text_tertiary)),
                )
            })
            .when_some(refused, |d, refused| d.child(refused))
            .when_some(chips, |d, chips| d.child(chips));
        div()
            .id(SharedString::from(format!("message-{key}")))
            .w_full()
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
            // An approval has no task line: its own card says "Needs you",
            // and the line after the answer says the answer.
            TimelineItem::Approval(card) => {
                let element =
                    self.render_approval_card(&card.approval, &card.message_id, true, t, cx);
                div()
                    .flex()
                    .flex_col()
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
                            // A link to a task (`task:<id>`) opens its thread.
                            if let Some(task) = url.strip_prefix("task:") {
                                let task = task.to_owned();
                                let _ = weak
                                    .update(cx, |this, cx| this.open_task_link(task, window, cx));
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
                        text("Enter saves. Escape cancels.", detail(t), c.text_tertiary)
                            .flex_1()
                            .min_w_0(),
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
        // While the task runs, waits for review or is resolved, the chat has
        // the line alone. The thread holds the rest, Resolve too.
        let collapsed = card.is_line_only();
        let open = self.panel == Panel::Thread(card.task_id.clone());
        let status = card.status;
        let age = self
            .state
            .task(&card.task_id)
            .map(|task| task.age_label.clone())
            .unwrap_or_default();
        let state = if matches!(status, TaskStatus::Resolved) && !card.summary.is_empty() {
            card.summary.clone()
        } else {
            consort_client::plan::line_state(status, &age)
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
                task_line_color(status, t),
                place,
                t,
                cx,
            )
            .id(SharedString::from(format!("task-line-{}", card.message_id)))
            .debug_selector({
                let id = format!("task-line-{}", card.message_id);
                move || id.clone()
            })
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, _, window, cx| {
                    this.open_thread(task_id.clone(), window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    let ended = this.state.task(&menu_task).is_some_and(|t| t.session_ended);
                    open_context_menu(
                        this,
                        event,
                        task_menu(&menu_task, status, ended, true),
                        window,
                        cx,
                    )
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
            .debug_selector({
                let id = format!("card-{}", card.message_id);
                move || id.clone()
            })
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
                    let ended = this.state.task(&menu_task).is_some_and(|t| t.session_ended);
                    open_context_menu(
                        this,
                        event,
                        task_menu(&menu_task, status, ended, true),
                        window,
                        cx,
                    )
                }),
            );
        let pending = card
            .approval
            .clone()
            .filter(|a| a.state != RequestState::Answered);
        if let Some(approval) = pending.clone()
            && approval.card.is_some()
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
                        .child(div().flex_1().min_w_0()),
                ))
                .child(self.render_approval_card(&approval, &key, false, t, cx))
                .child(
                    div()
                        .flex()
                        .child(div().flex_1().min_w_0())
                        .child(self.open_thread_link(&card.task_id, open, t, cx)),
                );
        } else if let Some(approval) = pending {
            // What asks, then the folder and the command, then the answers.
            let summary = selectable(
                SharedString::from(format!("{}.summary", approval.id)),
                approval.title(),
                t.text.ui,
                c.text_secondary,
            )
            .flex_1()
            .min_w_0();
            let key = format!("{}-approval", card.message_id);
            let width = self.approval_card_width(true, t) - chat.bubble_pad_x * 2.0;
            let block = self.render_approval_block(&approval, &key, width, true, t, cx);
            body = body
                .child(place_chips(
                    div().flex().items_center().gap(t.space.sm).child(summary),
                ))
                .child(block)
                .child(self.render_approval_buttons(&approval, t, cx))
                .child(
                    div()
                        .flex()
                        .child(div().flex_1().min_w_0())
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
            // The Monid total of the task, while it makes calls: the user
            // sees runaway calls and can stop them.
            if let Some(total) = consort_client::monid::task_line(card.monid_calls) {
                body = body.child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(t.space.sm)
                        .child(
                            selectable(
                                SharedString::from(format!("card-monid-{}", card.message_id)),
                                total,
                                t.text.ui,
                                c.text_secondary,
                            )
                            .flex_1()
                            .min_w_0(),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family(t.font_mono.clone())
                                .type_style(t.text.mono)
                                .text_color(c.text_primary)
                                .child(consort_client::money_text(card.monid_cost_micros)),
                        ),
                );
            }
            // The voice task: its finished steps, and what stays in it.
            if !card.steps.is_empty() {
                body = body.child(self.render_task_steps(&card.message_id, &card.steps, t));
            }
            if let Some(note) = card.note.clone() {
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.sm)
                        .child(text(note, detail(t), c.text_tertiary).flex_1().min_w_0())
                        .child(self.open_thread_link(&card.task_id, open, t, cx)),
                );
                return div()
                    .flex()
                    .flex_col()
                    .child(line)
                    .child(body)
                    .into_any_element();
            }
            // Open thread sits at the right. Resolve is in the thread.
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(div().flex_1().min_w_0())
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

    /// The composer of the chat: the shared component (`ui/composer.rs`),
    /// with the state of the link above it.
    fn render_composer(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        // The line above the composer: the state of the link, and for an
        // offline link the reason and a countdown that ticks once a second.
        // With saved data on screen, the pill under the mark says that the
        // hub is down, so this line stays out.
        let link: Option<(String, Option<String>, bool)> = match &state.connection {
            consort_client::Connection::Offline { .. } if state.saved_data.is_some() => None,
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
        let above = link.map(|(line_text, reason, offline)| {
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
                            // The reason is an error detail, so the user can
                            // select it and copy it.
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
                })
                .into_any_element()
        });
        // The hub state is unknown while the link is down, so the line of
        // the assistant shows only when the link line does not.
        // A hub or an app that is too old has its card here.
        // A hub on the tailnet says what to do, in place of the link line.
        // A hub on this computer that stopped has its own card and its own button.
        let above = self
            .local_stopped_card(t, cx)
            .or_else(|| self.tailnet_card(t))
            .or(above)
            .or_else(|| self.incompatible_card(t, cx))
            .or_else(|| self.no_account_card(t, cx))
            .or_else(|| self.permission_offer_card(t, cx))
            .or_else(|| self.render_assistant_status(t, cx));
        let column = self.chat_column_width(t);
        self.render_composer_box(
            crate::ui::composer::ComposerTarget::Channel,
            column,
            above,
            t,
            window,
            cx,
        )
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

/// What a message that is not on the hub yet says: "Sends when the hub is
/// back" while the link is down, else "Sending".
pub fn pending_text(hub_down: bool) -> &'static str {
    if hub_down {
        "Sends when the hub is back"
    } else {
        "Sending"
    }
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

/// The color of the state words on a task line. Only the two statuses that
/// wait for the user use the accent. Working is quiet, in `text.tertiary`.
pub(crate) fn task_line_color(status: TaskStatus, t: &Tokens) -> Hsla {
    match status {
        TaskStatus::Working => t.colors.text_tertiary,
        other => crate::ui::parts::status_color(other, t),
    }
}

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
            let items = task_menu("t1", status, false, true);
            assert!(
                !labels(&items).contains(&"Resolve"),
                "{status:?} offers Resolve"
            );
            assert!(!labels(&items).contains(&"Not done"));
        }
        let review = task_menu("t1", TaskStatus::Review, false, true);
        assert_eq!(
            labels(&review),
            ["Open thread", "Resolve", "Copy task ID", "Delete"]
        );
        let resolved = task_menu("t1", TaskStatus::Resolved, false, true);
        assert_eq!(
            labels(&resolved),
            [
                "Open thread",
                "Not done",
                "Archive",
                "Copy task ID",
                "Delete"
            ]
        );
    }

    #[test]
    fn retry_and_stop_match_the_status() {
        let failed = task_menu("t1", TaskStatus::Failed, false, true);
        assert_eq!(
            labels(&failed),
            ["Open thread", "Retry", "Archive", "Copy task ID", "Delete"]
        );
        let working = task_menu("t1", TaskStatus::Working, false, true);
        assert_eq!(
            labels(&working),
            ["Open thread", "Stop", "Copy task ID", "Delete"]
        );
        // A task in review or resolved whose session ended can only retry.
        for status in [TaskStatus::Review, TaskStatus::Resolved] {
            assert!(labels(&task_menu("t1", status, true, true)).contains(&"Retry"));
            assert!(!labels(&task_menu("t1", status, false, true)).contains(&"Retry"));
        }
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
            let items = task_menu("t1", status, false, true);
            assert!(
                !labels(&items).contains(&"Archive"),
                "{status:?} offers Archive"
            );
        }
        for status in [TaskStatus::Failed, TaskStatus::Resolved] {
            let items = task_menu("t1", status, false, false);
            let archive = items
                .iter()
                .find(|i| i.label == "Archive")
                .expect("Archive");
            assert_eq!(archive.action, MenuAction::ArchiveTask("t1".into()));
        }
    }

    #[test]
    fn the_thread_menu_leaves_out_open_thread_by_a_flag_not_by_its_label() {
        let items = task_menu("t1", TaskStatus::Working, false, false);
        assert_eq!(labels(&items), ["Stop", "Copy task ID", "Delete"]);
        assert!(
            items
                .iter()
                .all(|i| !matches!(i.action, MenuAction::OpenThread(_)))
        );
    }
}
