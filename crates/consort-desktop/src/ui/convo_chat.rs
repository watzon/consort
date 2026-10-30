//! Reactions and threads on messages (Paper `9I9-0` and `95W-0`): the quick
//! emoji at the top of the message menu, the emoji picker behind More, the
//! chips and the reply line under a message, and the thread panel.

use consort_client::{
    MessageView, Speaker,
    convo::{EMOJI_GROUPS, QUICK_EMOJI, ReactionView, reply_count_label},
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Div, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window,
    div, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Panel},
    convo::{ConvoAction, ReactionBar},
    ui::{
        menu::{MenuAction, MenuItem, OpenMenu, floating},
        parts::{detail, focusable, icon, icon_button, line, text},
        timeline::{message_menu, open_context_menu},
    },
};

/// The quick emoji row at the top of a message menu: six emoji and More. An
/// emoji that you chose already has the accent ring, and a click on it
/// removes it.
pub fn render_reaction_bar<V: 'static>(
    bar: &ReactionBar,
    t: &Tokens,
    cx: &mut Context<V>,
    on_pick: impl Fn(&mut V, MenuAction, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let c = &t.colors;
    let v = t.component.convo;
    let mut row = div()
        .flex()
        .items_center()
        .gap(t.space.xxs)
        .px(t.space.xxs)
        .pt(t.space.xxs)
        .pb(t.space.xs);
    for (i, emoji) in QUICK_EMOJI.iter().enumerate() {
        let chosen = bar.chosen.iter().any(|e| e == emoji);
        let action = MenuAction::Convo(ConvoAction::React {
            message_id: bar.message_id.clone(),
            emoji: (*emoji).to_owned(),
            remove: chosen,
        });
        let pick = on_pick.clone();
        row = row.child(
            div()
                .id(("quick-emoji", i))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(v.quick_emoji)
                .rounded(t.radius.md)
                .cursor_pointer()
                .when(chosen, |d| {
                    d.bg(c.tint_accent)
                        .border(t.border_width)
                        .border_color(c.accent_text)
                })
                .when(!chosen, |d| d.hover(|s| s.bg(c.surface_selected)))
                .type_style(v.quick_emoji_text)
                .child(*emoji)
                .on_click(
                    cx.listener(move |this, _, window, cx| pick(this, action.clone(), window, cx)),
                ),
        );
    }
    let more = MenuAction::Convo(ConvoAction::MoreEmoji(bar.message_id.clone()));
    row.child(
        div()
            .id("quick-emoji-more")
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .h(v.quick_emoji)
            .rounded(t.radius.md)
            .cursor_pointer()
            .hover(|s| s.bg(c.surface_selected))
            .type_style(t.text.ui_strong)
            .text_color(c.text_secondary)
            .child("More")
            .on_click(
                cx.listener(move |this, _, window, cx| on_pick(this, more.clone(), window, cx)),
            ),
    )
    .into_any_element()
}

/// The message menu with the quick emoji, at the pointer.
pub fn open_message_menu(
    this: &mut ConsortApp,
    event: &MouseDownEvent,
    message: &MessageView,
    with_thread: bool,
    window: &mut Window,
    cx: &mut Context<ConsortApp>,
) {
    let mut items = message_menu(message);
    if !with_thread {
        items.retain(|i| !matches!(i.action, MenuAction::ReplyInThread(_)));
    }
    open_context_menu(this, event, items, window, cx);
    // A menu for selected text has no Copy text item, and no emoji.
    let bar = (!message.pending).then(|| this.reaction_bar(&message.id));
    if let Some(menu) = this.menu.as_mut()
        && let Some(bar) = bar
        && menu.items.iter().any(|i| i.label.as_ref() == "Copy text")
    {
        menu.reactions = Some(bar);
        this.menu_anchor = Some(menu.clone());
    }
}

/// The chips of a message: yours in the accent, the assistant's neutral,
/// with a count only when both of you chose the same emoji. A click on a
/// chip adds your reaction, or takes it away.
pub fn reaction_chips(
    message_id: &str,
    reactions: &[ReactionView],
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> Div {
    let c = &t.colors;
    let v = t.component.convo;
    let mut row = div().flex().flex_wrap().items_center().gap(v.reaction_gap);
    for (i, reaction) in reactions.iter().enumerate() {
        let action = ConvoAction::React {
            message_id: message_id.to_owned(),
            emoji: reaction.emoji.clone(),
            remove: reaction.mine,
        };
        let count = reaction.count_label();
        row = row.child(
            div()
                .id(SharedString::from(format!("reaction-{message_id}-{i}")))
                .flex()
                .items_center()
                .gap(t.space.xs)
                .h(v.reaction_chip)
                .px(v.reaction_pad_x)
                .rounded(t.radius.full)
                .border(t.border_width)
                .cursor_pointer()
                .when(reaction.mine, |d| {
                    d.bg(c.tint_accent).border_color(c.accent_text)
                })
                .when(!reaction.mine, |d| {
                    d.bg(c.surface_card)
                        .border_color(c.border_subtle)
                        .hover(|s| s.bg(c.hover_card))
                })
                .child(
                    div()
                        .type_style(v.reaction_emoji)
                        .child(reaction.emoji.clone()),
                )
                .when_some(count, |d, n| d.child(text(n, t.text.label, c.accent_text)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_convo(action.clone(), window, cx)
                })),
        );
    }
    row
}

/// The reply line under a message with replies: "4 replies" and "Last reply
/// 9:31 AM". A click opens the thread.
pub fn reply_line(message: &MessageView, t: &Tokens, cx: &mut Context<ConsortApp>) -> AnyElement {
    let c = &t.colors;
    let root = message.id.clone();
    focusable(
        div().id(SharedString::from(format!("replies-{}", message.id))),
        t,
    )
    .flex()
    .items_center()
    .gap(t.space.xs + t.space.xxs)
    .h(t.component.convo.reaction_chip)
    .px(t.space.sm)
    .rounded(t.radius.md)
    .bg(c.surface_card)
    .cursor_pointer()
    .hover(|s| s.bg(c.hover_card))
    .child(icon("reply", t.icon.dense, c.text_secondary))
    .child(text(
        reply_count_label(message.reply_count),
        t.text.label,
        c.text_primary,
    ))
    .when_some(message.last_reply_label.clone(), |d, last| {
        d.child(text(last, detail(t), c.text_tertiary))
    })
    .on_click(
        cx.listener(move |this, _, window, cx| this.open_message_thread(root.clone(), window, cx)),
    )
    .into_any_element()
}

impl ConsortApp {
    /// The footer row of a message in the timeline: its reaction chips, and
    /// its reply line.
    pub fn render_message_footer(
        &mut self,
        item: usize,
        you: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(consort_client::TimelineItem::Message(message)) = self
            .state
            .timeline(&self.channel_id)
            .and_then(|tl| tl.items.get(item))
            .cloned()
        else {
            return div().into_any_element();
        };
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(t.component.convo.reaction_gap)
            .pt(t.space.xxs)
            .when(you, |d| d.justify_end());
        if !message.reactions.is_empty() {
            row = row.child(reaction_chips(&message.id, &message.reactions, t, cx));
        }
        if message.reply_count > 0 {
            row = row.child(reply_line(&message, t, cx));
        }
        div()
            .flex()
            .w_full()
            .when(you, |d| d.justify_end())
            .child(row)
            .into_any_element()
    }

    /// The line under a message group. It is always there, so a hover never
    /// moves the timeline. On hover it has React, Reply in thread and Copy
    /// at the edge of the bubble, then the time. A proactive message also has
    /// Useful and Too noisy at the other edge, and a choice stays at rest.
    pub fn render_group_footer(
        &mut self,
        first: usize,
        item: usize,
        you: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chat = t.component.chat;
        let line_row = div()
            .flex()
            .items_center()
            .w_full()
            .max_w(t.layout.bubble_max_width)
            .h(chat.footer_line)
            .gap(t.space.xxs);
        let lane = div().flex().w_full().when(you, |d| d.justify_end());
        let Some(consort_client::TimelineItem::Message(message)) = self
            .state
            .timeline(&self.channel_id)
            .and_then(|tl| tl.items.get(item))
            .cloned()
        else {
            return lane.child(line_row).into_any_element();
        };
        let hovered = self
            .chat_hover
            .is_some_and(|(_, hover)| (first..=item).contains(&hover))
            || self.proactivity.hovered.as_deref() == Some(message.id.as_str());
        let rating = message.proactive.as_ref().and_then(|m| m.rating);
        let feedback = message.proactive.as_ref().and_then(|_| match rating {
            Some(rating) => Some(self.render_feedback_chip(&message.id, rating, t, cx)),
            None if hovered => Some(self.render_feedback_pair(&message.id, t, cx)),
            None => None,
        });
        let mut row = line_row;
        // A message on its way has no ID that the actions could use.
        if hovered && !message.pending {
            let actions = self.footer_actions(&message, t, cx);
            let time = text(
                message.time_label.clone(),
                detail(t),
                t.colors.text_tertiary,
            )
            .flex_none()
            .px(t.space.xs);
            if you {
                row = row.justify_end().child(time);
                for action in actions.into_iter().rev() {
                    row = row.child(action);
                }
            } else {
                for action in actions {
                    row = row.child(action);
                }
                row = row.child(time);
            }
        }
        if let Some(feedback) = feedback {
            row = row.child(div().flex_1()).child(feedback);
        }
        lane.child(row).into_any_element()
    }

    /// React, Reply in thread and Copy, in this order.
    fn footer_actions(
        &self,
        message: &MessageView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let c = &t.colors;
        let chat = t.component.chat;
        let button = |id: &'static str, name: &str, tip: &'static str, cx: &mut Context<Self>| {
            let element: gpui_kit::ElementId = id.into();
            let color = if crate::ui::motion::is_hovered(&element, cx) {
                c.text_primary
            } else {
                c.text_tertiary
            };
            icon_button(
                id,
                name,
                chat.footer_button,
                t.icon.size,
                color,
                c.surface_selected,
                t,
                cx,
            )
            .debug_selector(move || id.to_owned())
            .tooltip(move |_, cx| cx.new(|_| crate::ui::convo_sidebar::Tip(tip.into())).into())
        };
        let react_id = message.id.clone();
        let reply_id = message.id.clone();
        let body = message.body.clone();
        vec![
            button("footer-react", "smile", "React", cx)
                .relative()
                .child(self.triggers.track("footer-react"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(bounds) = this.triggers.get("footer-react") {
                        let anchor = OpenMenu::under(bounds, Vec::new(), false).anchor;
                        this.convo.emoji_picker = Some((react_id.clone(), anchor));
                        cx.notify();
                    }
                }))
                .into_any_element(),
            button("footer-reply", "message", "Reply in thread", cx)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_message_thread(reply_id.clone(), window, cx)
                }))
                .into_any_element(),
            button("footer-copy", "copy", "Copy", cx)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_menu(
                        MenuAction::Copy(crate::ui::timeline::plain_text(&body)),
                        window,
                        cx,
                    )
                }))
                .into_any_element(),
        ]
    }

    /// The emoji picker behind More: every emoji by group. A click adds the
    /// reaction, or takes away one that you chose already.
    pub fn render_emoji_picker(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (message_id, anchor) = self.convo.emoji_picker.clone()?;
        let c = &t.colors;
        let v = t.component.convo;
        let chosen = self.reaction_bar(&message_id).chosen;
        let mut body = div().flex().flex_col().gap(t.space.sm);
        for (group, emojis) in EMOJI_GROUPS {
            let mut grid = div().flex().flex_wrap();
            for emoji in emojis.iter() {
                let mine = chosen.iter().any(|e| e == emoji);
                let action = ConvoAction::React {
                    message_id: message_id.clone(),
                    emoji: (*emoji).to_owned(),
                    remove: mine,
                };
                grid = grid.child(
                    div()
                        .id(SharedString::from(format!("emoji-{emoji}")))
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(v.emoji_cell)
                        .rounded(t.radius.md)
                        .cursor_pointer()
                        .when(mine, |d| {
                            d.bg(c.tint_accent)
                                .border(t.border_width)
                                .border_color(c.accent_text)
                        })
                        .when(!mine, |d| d.hover(|s| s.bg(c.surface_selected)))
                        .type_style(v.emoji_cell_text)
                        .child(*emoji)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run_convo(action.clone(), window, cx)
                        })),
                );
            }
            body = body
                .child(text(*group, t.text.label, c.text_tertiary).px(t.space.xs))
                .child(grid);
        }
        let content = div()
            .id("emoji-picker")
            .debug_selector(|| "emoji-picker".into())
            .occlude()
            .w(v.emoji_picker_width)
            .h(v.emoji_picker_height)
            .overflow_y_scroll()
            .p(t.space.sm)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .rounded(t.radius.lg)
            .shadow(t.shadow_float())
            .child(body)
            .into_any_element();
        Some(floating(
            anchor,
            content,
            t,
            window,
            cx,
            |this: &mut Self, _, cx| {
                this.convo.emoji_picker = None;
                cx.notify();
            },
        ))
    }

    /// One message of the thread panel: its bubbles in its lane, then its
    /// chips. The root has the accent ring, as it has in the timeline.
    fn thread_message(
        &mut self,
        message: &MessageView,
        root: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let chat = t.component.chat;
        let you = message.speaker == Speaker::You;
        // A message is one bubble, as it is in the timeline (DECISIONS 731).
        let parts: Vec<String> = vec![message.body.trim().to_owned()];
        let max = self.right_panel_width(t) - t.space.page;
        let mut col = div()
            .flex()
            .flex_col()
            .gap(t.space.xs)
            .w_full()
            .when(you, |d| d.items_end())
            .when(!you, |d| d.items_start());
        let count = parts.len();
        for (i, markdown) in parts.into_iter().enumerate() {
            let key = format!("thread-{}.p{i}", message.id);
            let mut menu: Vec<MenuItem> = message_menu(message);
            menu.retain(|i| !matches!(i.action, MenuAction::ReplyInThread(_)));
            let md = self.render_markdown(&key, &markdown, menu, c.surface_raised, t, cx);
            let menu_message = message.clone();
            col = col.child(
                div()
                    .id(SharedString::from(format!("thread-bubble-{key}")))
                    .max_w(max)
                    .min_w_0()
                    .py(chat.bubble_pad_y)
                    .px(chat.bubble_pad_x)
                    .rounded(chat.bubble_radius)
                    .when(i + 1 == count && you, |d| d.rounded_br(chat.bubble_tail))
                    .when(i + 1 == count && !you, |d| d.rounded_bl(chat.bubble_tail))
                    .bg(if you {
                        c.accent_surface
                    } else {
                        c.surface_card
                    })
                    .when(root, |d| {
                        d.border(t.component.convo.root_ring)
                            .border_color(c.accent_text)
                    })
                    .type_style(t.text.body)
                    .text_color(c.text_primary)
                    .child(md)
                    // Before the text's own menu: the message menu with the
                    // quick emoji.
                    .capture_any_mouse_down(cx.listener(
                        move |this, event: &MouseDownEvent, window, cx| {
                            if event.button == MouseButton::Right {
                                open_message_menu(this, event, &menu_message, false, window, cx);
                                cx.stop_propagation();
                            }
                        },
                    )),
            );
        }
        if !message.attachments.is_empty() {
            let key = format!("thread-{}", message.id);
            col = col.child(self.render_post_attachments(&key, &message.attachments, t, cx));
        }
        if !message.reactions.is_empty() {
            col = col.child(reaction_chips(&message.id, &message.reactions, t, cx));
        }
        if message.pending {
            col = col.child(text(
                crate::ui::timeline::time_text("", true).to_owned(),
                t.text.caption,
                c.text_tertiary,
            ));
        }
        col
    }

    /// The thread panel of a message (`95W-0`): the root, the replies as
    /// bubbles, and a reply box with attachments.
    pub fn render_message_thread_panel(
        &mut self,
        root_id: &str,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let thread = state.convo.thread(root_id).cloned();
        let root = thread
            .as_ref()
            .map(|t| t.root.clone())
            .or_else(|| self.find_message(root_id));
        let subtitle = thread
            .as_ref()
            .map(|t| t.subtitle.clone())
            .unwrap_or_else(|| "Loading the replies".to_owned());
        let menu_root = root.clone();
        let head = crate::ui::parts::panel_header(t)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(line("Thread", t.text.ui_strong, c.text_primary))
                    .child(line(subtitle, detail(t), c.text_tertiary)),
            )
            .child(
                div()
                    .relative()
                    .child(self.triggers.track("message-thread-more"))
                    .child(
                        icon_button(
                            "message-thread-more",
                            "ellipsis",
                            t.space.xxl,
                            t.icon.size,
                            c.text_secondary,
                            c.hover_sidebar,
                            t,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let (Some(bounds), Some(root)) =
                                (this.triggers.get("message-thread-more"), menu_root.clone())
                            {
                                let mut items = message_menu(&root);
                                items.retain(|i| !matches!(i.action, MenuAction::ReplyInThread(_)));
                                this.open_menu(OpenMenu::under(bounds, items, false), cx);
                            }
                        })),
                    ),
            )
            .child(
                icon_button(
                    "message-thread-close",
                    "x",
                    t.space.xxl,
                    t.icon.size,
                    c.text_secondary,
                    c.hover_sidebar,
                    t,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx))),
            );

        let mut body = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .px(t.space.lg)
            .py(t.space.lg);
        if let Some(root) = &root {
            body = body
                .child(text(root.time_label.clone(), detail(t), c.text_tertiary))
                .child(self.thread_message(root, true, t, cx));
            let count = thread
                .as_ref()
                .map(|t| t.replies.len() as u32)
                .unwrap_or(root.reply_count)
                .max(root.reply_count);
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(text(reply_count_label(count), detail(t), c.text_tertiary))
                    .child(div().flex_1().h(t.border_width).bg(c.border_subtle)),
            );
        }
        let replies = thread.map(|t| t.replies).unwrap_or_default();
        for reply in &replies {
            body = body.child(self.thread_message(reply, false, t, cx));
        }

        let target = consort_client::UploadTarget::Thread {
            root_id: root_id.to_owned(),
        };
        let chips = self.render_upload_chips(&target, t, cx);
        let paste_target = target.clone();
        let add_target = target.clone();
        let reply = div()
            .flex_none()
            .px(t.space.lg)
            .pb(t.space.lg)
            .child(
                div()
                    .id("thread-reply-box")
                    .debug_selector(|| "thread-reply-box".into())
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .py(t.space.sm)
                    .px(t.space.sm)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .rounded(t.radius.lg)
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
                            .gap(t.space.xs)
                            .child(div().flex_1().min_w_0().px(t.space.xs + t.space.xxs).child(
                                crate::ui::parts::textarea(
                                    &self.convo_inputs.thread_reply,
                                    t.text.ui,
                                ),
                            ))
                            .child(
                                icon_button(
                                    "thread-reply-attach",
                                    "plus",
                                    t.space.xl + t.space.xs,
                                    t.icon.size,
                                    c.text_secondary,
                                    c.hover_card,
                                    t,
                                    cx,
                                )
                                .relative()
                                .child(self.triggers.track("thread-reply-attach"))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.open_add_menu(
                                            "thread-reply-attach",
                                            add_target.clone(),
                                            cx,
                                        )
                                    },
                                )),
                            )
                            .child(
                                icon_button(
                                    "thread-reply-send",
                                    "corner-down-left",
                                    t.space.xl + t.space.xs,
                                    t.icon.size,
                                    c.text_secondary,
                                    c.hover_card,
                                    t,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.send_thread_reply(window, cx),
                                )),
                            ),
                    ),
            )
            .child(
                text(
                    "The assistant reads the whole thread before it answers.",
                    detail(t),
                    c.text_tertiary,
                )
                .pt(t.space.sm)
                .px(t.space.xs),
            );

        div()
            .id("message-thread-panel")
            .debug_selector(|| "message-thread-panel".into())
            .flex()
            .flex_col()
            .flex_none()
            .w(self.right_panel_width(t))
            .h_full()
            .bg(c.surface_sidebar)
            .border_l(t.border_width)
            .border_color(c.border_subtle)
            .on_drag_move::<gpui_kit::ExternalPaths>(cx.listener(
                |this, event: &gpui_kit::DragMoveEvent<gpui_kit::ExternalPaths>, _, cx| {
                    let inside = event.bounds.contains(&event.event.position);
                    let count = event.drag(cx).paths().len();
                    this.drag_files_over(crate::app::DropOver::Reply, inside, count, cx);
                },
            ))
            .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, _, cx| {
                this.drop_files(crate::app::DropOver::Reply, paths, cx);
            }))
            .child(head)
            .child(
                div()
                    .id("message-thread-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .child(body),
            )
            .child(reply)
            .into_any_element()
    }
}

/// Whether the panel shows the thread of this message.
pub fn thread_open_for(panel: &Panel, message_id: &str) -> bool {
    matches!(panel, Panel::MessageThread(root) if root == message_id)
}
