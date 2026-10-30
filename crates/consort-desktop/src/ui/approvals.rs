//! Approvals have two views.
//!
//! While it waits, an approval is a card at the bubble width with the accent
//! border: the title (what the action is), "Needs you", the exact command or
//! target once, the folder when there is one, and the buttons in one row of
//! equal widths. A long command stops at five lines until the user taps
//! "Show all". All text wraps inside the box.
//!
//! After the answer, it is one line of `chat.answered_line` in a box: a
//! glyph, the action cut to one line, the answer and a chevron. A click opens
//! the box in place with the full command and the folder. Nothing else in the
//! timeline moves.
//!
//! The open state is local to the app: one flag for each approval and each
//! place (the key says where), and the same for "Show all".

use consort_client::{
    ApprovalChoice, ApprovalView, HubCardView, RequestState, TaskStatus,
    approval_text::COMMAND_LINES,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, App, Context, Div, IndentAdjustment, InteractiveElement, IntoElement, LineFragment,
    MouseButton, MouseDownEvent, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement as _, Styled, base::SelectableText, div, font,
    prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    ui::{
        menu::{MenuAction, MenuItem},
        parts::{ButtonKind, detail, focusable, icon, line, status_glyph, text, wrapping_button},
        timeline::{DetailPart, detail_parts, is_open_click, open_context_menu},
    },
};

/// Whether the approval still needs the user.
fn is_waiting(state: RequestState) -> bool {
    matches!(state, RequestState::Pending | RequestState::Sending)
}

/// The glyph of the line after the answer: done, failed for Denied, and the
/// quiet ring for a request that closed with no answer.
pub(crate) fn answered_status(approval: &ApprovalView) -> TaskStatus {
    match (approval.state, approval.answer) {
        (RequestState::Answered, Some(ApprovalChoice::Deny)) => TaskStatus::Failed,
        (RequestState::Answered, _) => TaskStatus::Resolved,
        _ => TaskStatus::UpNext,
    }
}

/// How many lines a mono text takes when it wraps at `width`.
pub(crate) fn wrapped_lines(cx: &App, t: &Tokens, content: &str, width: Pixels) -> usize {
    let system = cx.text_system();
    let mut wrapper = system.line_wrapper(font(t.font_mono.clone()), t.text.mono.size);
    content
        .split('\n')
        .map(|row| {
            1 + wrapper
                .wrap_line(
                    &[LineFragment::text(row)],
                    width,
                    IndentAdjustment::NoIndent,
                )
                .count()
        })
        .sum()
}

impl ConsortApp {
    /// The width that an approval card has in a place: the bubble width, or
    /// less in a narrow column. `chat` is the timeline, else a task thread.
    pub(crate) fn approval_card_width(&self, chat: bool, t: &Tokens) -> Pixels {
        let room = if chat {
            self.chat_column_width(t) - t.space.xl * 2.0
        } else {
            self.right_panel_width(t) - t.space.lg * 2.0 - t.border_width
        };
        room.min(t.layout.bubble_max_width).max(Pixels::ZERO)
    }

    /// An approval: the card while it waits, and the line after the answer.
    /// `key` is unique for the approval and the place, so each place keeps
    /// its own open state.
    pub fn render_approval_card(
        &mut self,
        approval: &ApprovalView,
        key: &str,
        chat: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A draft is a card of its own: see `ui/drafts.rs`.
        if let Some(HubCardView::Connector(card)) = approval.card.as_deref()
            && let Some(draft) = card.draft.clone()
        {
            let card = card.clone();
            return self.render_draft_approval(approval, &card, &draft, key, chat, t, cx);
        }
        if let Some(hub_card) = approval.card.clone() {
            // A hub card keeps its own body while it waits. After the answer
            // it shrinks to the same line, unless its outcome is still
            // moving (a sign-in in a browser): then the user needs the
            // buttons of the outcome, so the whole card stays.
            return if is_waiting(approval.state) || self.hub_outcome_is_live(approval, &hub_card) {
                self.render_hub_card(approval, &hub_card, key, chat, t, cx)
            } else {
                self.render_answered_line(approval, Some(&hub_card), key, chat, t, cx)
            };
        }
        if is_waiting(approval.state) {
            self.render_waiting_approval(approval, key, chat, t, cx)
        } else {
            self.render_answered_line(approval, None, key, chat, t, cx)
        }
    }

    /// The card of an approval that waits.
    fn render_waiting_approval(
        &mut self,
        approval: &ApprovalView,
        key: &str,
        chat: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let pad = t.space.md + t.space.xxs;
        let width = self.approval_card_width(chat, t);
        let block = self.render_approval_block(approval, key, width - pad * 2.0, true, t, cx);
        let buttons = self.render_approval_buttons(approval, t, cx);
        crate::ui::parts::card_corners(div(), chat, t)
            .id(SharedString::from(format!("{key}-card")))
            .debug_selector({
                let id = format!("{key}-card");
                move || id.clone()
            })
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .max_w(t.layout.bubble_max_width)
            .overflow_hidden()
            .gap(t.space.sm + t.space.xxs)
            .py(t.space.md)
            .px(pad)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.accent_text)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(t.space.sm + t.space.xxs)
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(t.text.ui.line)
                            .child(status_glyph(
                                SharedString::from(format!("{key}-glyph")),
                                TaskStatus::NeedsInput,
                                t.icon.size,
                                t,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .type_style(t.text.ui_strong)
                            .text_color(c.text_primary)
                            .child(SelectableText::new(
                                SharedString::from(format!("{key}-title")),
                                approval.title(),
                            )),
                    )
                    .child(
                        text("Needs you", t.text.ui, c.accent_text)
                            .flex_none()
                            .whitespace_nowrap(),
                    ),
            )
            .child(block)
            .child(buttons)
            .into_any_element()
    }

    /// The block of an approval: the command or the target, in mono, and the
    /// folder when the agent can choose it. `width` is the room for the
    /// card's content. With `clamp`, a command that wraps to more than five
    /// lines stops there, with "Show all" (and "Show less") under it. All
    /// text wraps, so a long address with no spaces breaks at the edge.
    pub fn render_approval_block(
        &mut self,
        approval: &ApprovalView,
        key: &str,
        width: Pixels,
        clamp: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let mut block = div().flex().flex_col().gap(t.space.sm).w_full().min_w_0();
        for part in detail_parts(approval, false) {
            block = match part {
                DetailPart::Summary(_) => block,
                DetailPart::Folder(folder) => block.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(t.space.sm)
                        .w_full()
                        .min_w_0()
                        .child(
                            // GPUI's baseline does not line up a 16 px label
                            // with a 20 px mono line, so both get the mono
                            // line height.
                            text("Folder", detail(t), c.text_tertiary)
                                .flex_none()
                                .line_height(t.text.mono.line),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(t.font_mono.clone())
                                .type_style(t.text.mono)
                                .text_color(c.text_primary)
                                .debug_selector({
                                    let id = format!("{key}-folder");
                                    move || id.clone()
                                })
                                .child(SelectableText::new(
                                    SharedString::from(format!("{key}.folder")),
                                    folder,
                                )),
                        ),
                ),
                DetailPart::Command(command) => {
                    block.child(self.render_command_box(key, command, width, clamp, t, cx))
                }
            };
        }
        block
    }

    /// The command box.
    fn render_command_box(
        &mut self,
        key: &str,
        command: String,
        width: Pixels,
        clamp: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let c = &t.colors;
        // The text has the box's padding and border on each side.
        let text_width = width - t.space.md * 2.0 - t.border_width * 2.0 - t.space.xs;
        let long = clamp && wrapped_lines(cx, t, &command, text_width) > COMMAND_LINES;
        let all = self.approvals_all.contains(key);
        let stopped = long && !all;
        let copy = command.clone();
        let toggle_key = key.to_owned();
        div()
            .id(SharedString::from(format!("{key}-command")))
            .flex()
            .flex_col()
            .gap(t.space.xs)
            .w_full()
            .min_w_0()
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .rounded(t.radius.md)
            .py(t.space.sm)
            .px(t.space.md)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    // Whole lines only: the box is as high as five lines.
                    .when(stopped, |d| {
                        d.max_h(t.text.mono.line * COMMAND_LINES as f32)
                            .overflow_hidden()
                    })
                    .font_family(t.font_mono.clone())
                    .type_style(t.text.mono)
                    .text_color(c.text_primary)
                    .debug_selector({
                        let id = format!("{key}-command-text");
                        move || id.clone()
                    })
                    .child(SelectableText::new(
                        SharedString::from(format!("{key}.command")),
                        command,
                    )),
            )
            .when(long, |d| {
                d.child(
                    focusable(div().id(SharedString::from(format!("{key}-show-all"))), t)
                        .debug_selector({
                            let id = format!("{key}-show-all");
                            move || id.clone()
                        })
                        .flex_none()
                        .self_start()
                        .type_style(detail(t))
                        .text_color(c.text_secondary)
                        .cursor_pointer()
                        .hover(|s| s.text_color(c.text_primary))
                        .child(if all { "Show less" } else { "Show all" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.approvals_all.remove(&toggle_key) {
                                this.approvals_all.insert(toggle_key.clone());
                            }
                            cx.notify();
                        })),
                )
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    open_context_menu(
                        this,
                        event,
                        vec![MenuItem::new("Copy code", MenuAction::Copy(copy.clone()))],
                        window,
                        cx,
                    )
                }),
            )
    }

    /// The buttons of an approval: the choices in one row of equal widths.
    /// The wide choice shows the harness's own words when it gave some. They
    /// wrap, so the user reads the whole scope, and the row stays as high as
    /// its tallest button.
    pub fn render_approval_buttons(
        &mut self,
        approval: &ApprovalView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let blocked = self.answers_blocked();
        let mut row = div().flex().items_stretch().w_full().gap(t.space.sm);
        // A long label gets its own row under the others.
        let mut long_rows = Vec::new();
        for (i, choice) in approval.choices.iter().enumerate() {
            let kind = match choice {
                ApprovalChoice::AllowOnce => ButtonKind::Primary,
                ApprovalChoice::AllowForTask => ButtonKind::Secondary,
                ApprovalChoice::Deny => ButtonKind::Secondary,
            };
            let id = approval.id.clone();
            let choice = *choice;
            let button_id = format!("{}-{i}", approval.id);
            let long =
                consort_client::approval_text::label_needs_own_row(&approval.choice_label(choice));
            let control = crate::ui::hub_blocked::needs_hub(
                wrapping_button(
                    SharedString::from(button_id.clone()),
                    approval.choice_label(choice),
                    kind,
                    t,
                )
                .debug_selector(move || button_id.clone())
                // The three buttons have one width and one height: the
                // primary one gets the border of the others, in its own
                // color, so the row is equal and no button is taller.
                .when(kind == ButtonKind::Primary, |b| {
                    b.py(t.space.xs)
                        .border(t.border_width)
                        .border_color(t.colors.accent_fill)
                })
                .flex_1()
                .min_w_0()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.answer_approval(id.clone(), choice, window, cx)
                })),
                blocked,
                t,
            );
            if long {
                long_rows.push(div().flex().w_full().child(control));
            } else {
                row = row.child(control);
            }
        }
        if long_rows.is_empty() {
            return row;
        }
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(t.space.sm)
            .child(row)
            .children(long_rows)
    }

    /// The line after the answer. It is `chat.answered_line` high with a
    /// glyph, the action cut to one line, the answer and a chevron. A click
    /// opens it in place: the full command and the folder (or the whole hub
    /// card) show under the same line.
    fn render_answered_line(
        &mut self,
        approval: &ApprovalView,
        hub: Option<&HubCardView>,
        key: &str,
        chat: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let open = self.approvals_open.contains(key);
        let width = self.approval_card_width(chat, t);
        let pad = t.space.md;
        // Closed: the action. Open: what the action was.
        let (headline, mono) = if open {
            (approval.title_done(), false)
        } else {
            (approval.action_line(), true)
        };
        let body =
            open.then(|| match hub {
                // The hub card's own body, with its outcome line.
                Some(card) => div()
                    .px(pad)
                    .pb(pad)
                    .child(self.render_hub_card_body(approval, card, key, t, cx)),
                None => {
                    let block =
                        self.render_approval_block(approval, key, width - pad * 2.0, false, t, cx);
                    div()
                        .flex()
                        .flex_col()
                        .gap(t.space.sm)
                        .px(pad)
                        .pb(pad)
                        .child(block)
                        .children(approval.scope_words().map(|words| {
                            text(words, t.text.ui, c.text_secondary).into_any_element()
                        }))
                }
            });
        self.render_closed_line(
            key,
            answered_status(approval),
            headline,
            mono,
            approval.answer_label(),
            body,
            t,
            cx,
        )
    }

    /// The box of a request that is closed: one line of
    /// `chat.answered_line` with a glyph, `headline`, the answer and a
    /// chevron, and `body` under it when the line is open. A click toggles
    /// the open state for `key`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_closed_line(
        &mut self,
        key: &str,
        status: TaskStatus,
        headline: String,
        mono: bool,
        answer: String,
        body: Option<Div>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let open = self.approvals_open.contains(key);
        let toggle_key = key.to_owned();
        let header = focusable(div().id(SharedString::from(format!("{key}-line"))), t)
            .debug_selector({
                let id = format!("{key}-line");
                move || id.clone()
            })
            .flex()
            .items_center()
            .gap(t.space.sm + t.space.xxs)
            .h(t.component.chat.answered_line)
            .px(t.space.md)
            .w_full()
            .cursor_pointer()
            .child(status_glyph(
                SharedString::from(format!("{key}-glyph")),
                status,
                t.icon.size,
                t,
                cx,
            ))
            .child(
                line(headline, t.text.ui, c.text_primary)
                    .flex_1()
                    .when(mono, |d| d.font_family(t.font_mono.clone())),
            )
            .child(
                text(answer, t.text.ui, c.text_secondary)
                    .flex_none()
                    .whitespace_nowrap(),
            )
            .child(icon(
                if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                t.icon.dense,
                c.text_tertiary,
            ))
            .on_click(cx.listener(move |this, event, _, cx| {
                if !is_open_click(event) {
                    return;
                }
                if !this.approvals_open.remove(&toggle_key) {
                    this.approvals_open.insert(toggle_key.clone());
                }
                cx.notify();
            }));
        div()
            .id(SharedString::from(format!("{key}-answered")))
            .debug_selector({
                let id = format!("{key}-answered");
                move || id.clone()
            })
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .max_w(t.layout.bubble_max_width)
            .overflow_hidden()
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            // The line is a box of its own, not a bubble.
            .rounded(t.radius.lg)
            .child(header)
            .children(body)
            .into_any_element()
    }
}
