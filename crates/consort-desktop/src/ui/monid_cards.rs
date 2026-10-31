//! Monid in the timeline: the card that asks before a paid tool runs, the
//! card that asks for access (ADR 0017), and the row of a call.
//!
//! A Monid card is the normal waiting card of an approval, with the accent
//! border. It names the tool, shows the exact input in a box that the user
//! can select, gives the price or the limit, and has the buttons. A call row
//! is one quiet line with the cost at the right. A click opens the full
//! input, the output and the cost under it. The open state is local to the
//! app, like the details of an error card.

use consort_client::{
    ApprovalView, MonidCallRowView, MonidCallStateView, MonidCardView, TaskStatus,
    access::AccessCardView,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement as _, Styled, base::SelectableText,
    div, prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    monid::{CallPart, MonidAction, call_key},
    ui::{
        hub_blocked::needs_hub,
        menu::{MenuAction, MenuItem, OpenMenu},
        parts::{
            ButtonKind, button, detail, focusable, icon, icon_button, line, selectable,
            status_glyph, text, wrapping_button,
        },
        timeline::{is_open_click, open_context_menu},
    },
};

/// The input of a call, in a box that the user can select.
fn input_box(id: SharedString, content: String, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .w_full()
        .min_w_0()
        .py(t.space.sm)
        .px(t.space.md)
        .bg(c.surface_raised)
        .border(t.border_width)
        .border_color(c.border_subtle)
        .rounded(t.radius.md)
        .font_family(t.font_mono.clone())
        .type_style(t.text.mono)
        .text_color(c.text_primary)
        .child(SelectableText::new(id, content))
}

/// A label and a text: "Price" and "$0.08 per 1,000 characters."
fn labeled_line(label: &'static str, id: SharedString, content: String, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .items_start()
        .gap(t.space.sm)
        .w_full()
        .min_w_0()
        .child(text(label, detail(t), c.text_tertiary).flex_none())
        .child(
            selectable(id, content, detail(t), c.text_secondary)
                .flex_1()
                .min_w_0(),
        )
}

impl ConsortApp {
    /// The frame of a card that waits: the accent border, the glyph, the
    /// title and the state at the right.
    #[allow(clippy::too_many_arguments)]
    fn waiting_frame(
        &mut self,
        key: &str,
        chat: bool,
        title: String,
        state: &'static str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let pad = t.space.md + t.space.xxs;
        let header = div()
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
                selectable(
                    SharedString::from(format!("{key}-title")),
                    title,
                    t.text.ui_strong,
                    c.text_primary,
                )
                .flex_1()
                .min_w_0(),
            )
            .child(
                text(state, t.text.ui, c.accent_text)
                    .flex_none()
                    .whitespace_nowrap(),
            );
        crate::ui::parts::card_corners(div(), chat, t)
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
            .child(header)
    }

    /// A button of a card, as wide as the others in its row. The first one
    /// is the accent button.
    fn card_button(
        &mut self,
        id: String,
        label: String,
        kind: ButtonKind,
        action: impl Fn(&mut ConsortApp, &mut gpui_kit::Window, &mut Context<ConsortApp>) + 'static,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let blocked = self.answers_blocked();
        let selector = id.clone();
        needs_hub(
            wrapping_button(SharedString::from(id), label, kind, t)
                .debug_selector(move || selector.clone())
                .when(kind == ButtonKind::Primary, |b| {
                    b.py(t.space.xs)
                        .border(t.border_width)
                        .border_color(t.colors.accent_fill)
                })
                .flex_1()
                .min_w_0()
                .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx))),
            blocked,
            t,
        )
    }

    /// The card of a paid tool that waits for the user: the exact input,
    /// the price or the limit, and the buttons.
    pub fn render_monid_card(
        &mut self,
        approval: &ApprovalView,
        card: &MonidCardView,
        key: &str,
        chat: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let over = card.over_limit.is_some();
        let id = approval.id.clone();
        let busy = self
            .in_flight
            .is_running(&format!("approval:{}", approval.id));
        let allow = {
            let id = id.clone();
            self.card_button(
                format!("{key}-allow"),
                "Allow once".to_owned(),
                ButtonKind::Primary,
                move |this, window, cx| {
                    this.answer_option(id.clone(), "allow".to_owned(), window, cx)
                },
                t,
                cx,
            )
        };
        let middle = if over {
            Some(self.card_button(
                format!("{key}-change-limit"),
                "Change limit".to_owned(),
                ButtonKind::Secondary,
                |this, window, cx| this.open_monid_page(window, cx),
                t,
                cx,
            ))
        } else if card.allow_task {
            let id = id.clone();
            Some(self.card_button(
                format!("{key}-allow-task"),
                "Allow for this task".to_owned(),
                ButtonKind::Secondary,
                move |this, window, cx| {
                    this.answer_option(id.clone(), "allow_task".to_owned(), window, cx)
                },
                t,
                cx,
            ))
        } else {
            None
        };
        let deny = {
            let id = id.clone();
            self.card_button(
                format!("{key}-deny"),
                "Deny".to_owned(),
                ButtonKind::Secondary,
                move |this, window, cx| {
                    this.answer_option(id.clone(), "deny".to_owned(), window, cx)
                },
                t,
                cx,
            )
        };
        let more_key = format!("{key}-more");
        let more = (!card.more.is_empty()).then(|| {
            let items: Vec<MenuItem> = card
                .more
                .iter()
                .map(|o| {
                    MenuItem::new(
                        o.label.clone(),
                        MenuAction::Monid(MonidAction::AnswerOption(id.clone(), o.id.clone())),
                    )
                })
                .collect();
            let selector = more_key.clone();
            let trigger = more_key.clone();
            div().flex_none().child(
                icon_button(
                    SharedString::from(more_key.clone()),
                    "ellipsis",
                    t.space.xxl,
                    t.icon.size,
                    c.text_secondary,
                    c.hover_raised,
                    t,
                    cx,
                )
                .debug_selector(move || selector.clone())
                .border(t.border_width)
                .border_color(c.border_strong)
                .bg(c.surface_raised)
                .relative()
                .child(self.triggers.track(trigger.clone()))
                .on_click(cx.listener(
                    move |this, event: &gpui_kit::ClickEvent, _, cx| {
                        let menu = match this.triggers.get(&trigger) {
                            Some(bounds) => OpenMenu::under(bounds, items.clone(), false),
                            None => OpenMenu::at(event.position(), items.clone()),
                        };
                        this.open_menu(menu, cx);
                    },
                )),
            )
        });
        let facts = if let Some(sentence) = &card.over_limit {
            labeled_line(
                "Limit",
                SharedString::from(format!("{key}-limit")),
                sentence.clone(),
                t,
            )
        } else {
            labeled_line(
                "Price",
                SharedString::from(format!("{key}-price")),
                card.price_line(),
                t,
            )
        };
        let copy = card.input.clone();
        let input = input_box(
            SharedString::from(format!("{key}-input")),
            card.input.clone(),
            t,
        )
        .debug_selector({
            let id = format!("{key}-input");
            move || id.clone()
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
        );
        let buttons = div()
            .flex()
            .items_stretch()
            .w_full()
            .gap(t.space.sm)
            .when(busy, |d| d.opacity(t.opacity.busy))
            .child(allow)
            .children(middle)
            .child(deny)
            .children(more);
        self.waiting_frame(key, chat, card.title(), card.state_label(), t, cx)
            .child(input)
            .child(facts)
            .child(buttons)
            .into_any_element()
    }

    /// The card that asks for access to a connection or to Monid. The
    /// buttons are the options of the approval, in the order that the hub
    /// sent them. The first one is the accent button.
    pub fn render_access_card(
        &mut self,
        approval: &ApprovalView,
        card: &AccessCardView,
        key: &str,
        chat: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self
            .in_flight
            .is_running(&format!("approval:{}", approval.id));
        let mut buttons = div()
            .flex()
            .items_stretch()
            .w_full()
            .gap(t.space.sm)
            .when(busy, |d| d.opacity(t.opacity.busy));
        for (i, option) in card.options.iter().enumerate() {
            let id = approval.id.clone();
            let option_id = option.id.clone();
            let kind = if i == 0 {
                ButtonKind::Primary
            } else {
                ButtonKind::Secondary
            };
            buttons = buttons.child(self.card_button(
                format!("{key}-option-{}", option.id),
                option.label.clone(),
                kind,
                move |this, window, cx| {
                    this.answer_option(id.clone(), option_id.clone(), window, cx)
                },
                t,
                cx,
            ));
        }
        let mut frame = self.waiting_frame(key, chat, card.title.clone(), "Needs you", t, cx);
        if let Some(reason) = &card.reason {
            frame = frame.child(access_line(
                "Why",
                SharedString::from(format!("{key}-why")),
                reason.clone(),
                t,
            ));
        }
        frame
            .child(access_line(
                "Access",
                SharedString::from(format!("{key}-effect")),
                card.effect.clone(),
                t,
            ))
            .child(buttons)
            .into_any_element()
    }

    /// The line of an access card after the answer: "Gmail Work: access for
    /// this task" and "Allowed" or "Denied". It has nothing to open.
    pub fn render_access_answered(
        &mut self,
        card: &AccessCardView,
        key: &str,
        t: &Tokens,
    ) -> AnyElement {
        let c = &t.colors;
        let answer = if card.allowed() { "Allowed" } else { "Denied" };
        div()
            .debug_selector({
                let id = format!("{key}-answered");
                move || id.clone()
            })
            .flex()
            .items_center()
            .gap(t.space.sm + t.space.xxs)
            .w_full()
            .min_w_0()
            .max_w(t.layout.bubble_max_width)
            .h(t.component.chat.answered_line)
            .px(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .rounded(t.radius.lg)
            .child(
                selectable(
                    SharedString::from(format!("{key}-line")),
                    card.answered_line(),
                    t.text.ui,
                    c.text_primary,
                )
                .flex_1()
                .min_w_0(),
            )
            .child(
                text(answer, t.text.ui, c.text_secondary)
                    .flex_none()
                    .whitespace_nowrap(),
            )
            .into_any_element()
    }

    /// The body of a Monid card that was answered, for the box of an
    /// answered line that the user opened: the input and the price.
    pub fn render_monid_card_body(
        &mut self,
        card: &MonidCardView,
        key: &str,
        t: &Tokens,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .w_full()
            .min_w_0()
            .child(input_box(
                SharedString::from(format!("{key}-input")),
                card.input.clone(),
                t,
            ))
            .child(labeled_line(
                "Price",
                SharedString::from(format!("{key}-price")),
                card.price_line(),
                t,
            ))
            .into_any_element()
    }

    /// The body of an access card that was answered, when the user opens it.
    pub fn render_access_card_body(
        &mut self,
        card: &AccessCardView,
        key: &str,
        t: &Tokens,
    ) -> AnyElement {
        let mut body = div().flex().flex_col().gap(t.space.sm).w_full().min_w_0();
        if let Some(reason) = &card.reason {
            body = body.child(access_line(
                "Why",
                SharedString::from(format!("{key}-why")),
                reason.clone(),
                t,
            ));
        }
        body.child(access_line(
            "Access",
            SharedString::from(format!("{key}-effect")),
            card.effect.clone(),
            t,
        ))
        .into_any_element()
    }

    /// The row of a paid call. `place` says where the row shows (the
    /// channel or a task thread), so each place keeps its own open state.
    /// `time` is the time label of the notice.
    pub fn render_monid_call(
        &mut self,
        row: &MonidCallRowView,
        place: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let key = call_key(place, &row.call_id);
        let open = row.can_open() && self.monid_ui.open_calls.contains(&key);
        let state = row.state;
        let (marker, summary_color): (AnyElement, _) = match state {
            MonidCallStateView::Running => (
                status_glyph(
                    SharedString::from(format!("monid-glyph-{key}")),
                    TaskStatus::Working,
                    t.icon.size,
                    t,
                    cx,
                ),
                c.text_secondary,
            ),
            MonidCallStateView::Completed => (
                status_glyph(
                    SharedString::from(format!("monid-glyph-{key}")),
                    TaskStatus::Resolved,
                    t.icon.size,
                    t,
                    cx,
                ),
                c.text_secondary,
            ),
            s if s.is_error() => (
                icon("failed", t.icon.size, c.status_failed).into_any_element(),
                c.text_secondary,
            ),
            MonidCallStateView::Blocked => (
                icon("lock", t.icon.size, c.text_tertiary).into_any_element(),
                c.text_secondary,
            ),
            _ => (
                icon("failed", t.icon.size, c.text_tertiary).into_any_element(),
                c.text_secondary,
            ),
        };
        let mono_summary = matches!(state, MonidCallStateView::Completed)
            || (state == MonidCallStateView::ProviderError && row.summary.starts_with('$'));
        let toggle_key = key.clone();
        let call_id = row.call_id.clone();
        let name = row.name.clone();
        let stop_id = row.call_id.clone();
        let can_open = row.can_open();
        let header = focusable(div().id(SharedString::from(format!("monid-call-{key}"))), t)
            .debug_selector({
                let id = format!("monid-call-{key}");
                move || id.clone()
            })
            .flex()
            .items_center()
            .gap(t.space.sm + t.space.xxs)
            .min_h(t.component.chat.answered_line)
            .px(t.space.md)
            .w_full()
            .when(can_open, |d| d.cursor_pointer())
            .child(marker)
            .child(line(name, t.text.ui, c.text_primary).flex_1().min_w_0())
            .child(
                text(row.summary.clone(), t.text.ui, summary_color)
                    .flex_none()
                    .whitespace_nowrap()
                    .when(mono_summary, |d| {
                        d.font_family(t.font_mono.clone()).type_style(t.text.mono)
                    }),
            )
            .when(row.stoppable, |d| {
                d.child(
                    button(
                        SharedString::from(format!("monid-stop-{key}")),
                        "Stop",
                        ButtonKind::Quiet,
                        t,
                    )
                    .debug_selector({
                        let id = format!("monid-stop-{key}");
                        move || id.clone()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_monid_action(MonidAction::StopCall(stop_id.clone()), window, cx);
                        cx.stop_propagation();
                    })),
                )
            })
            .when(can_open, |d| {
                d.child(icon(
                    if open {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                    t.icon.dense,
                    c.text_tertiary,
                ))
            })
            .when(can_open, |d| {
                d.on_click(cx.listener(move |this, event, _, cx| {
                    if !is_open_click(event) {
                        return;
                    }
                    this.toggle_call_row(toggle_key.clone(), call_id.clone(), cx);
                }))
            });
        let body = open.then(|| self.monid_call_body(row, &key, t, cx));
        let menu = monid_call_menu(row);
        let attachments = row.attachments.clone();
        let mut column = div()
            .flex()
            .flex_col()
            .items_start()
            .w_full()
            .min_w_0()
            .max_w(t.layout.bubble_max_width)
            .child(
                div()
                    .id(SharedString::from(format!("monid-box-{key}")))
                    .flex()
                    .flex_col()
                    .w_full()
                    .min_w_0()
                    .overflow_hidden()
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .rounded(t.radius.lg)
                    .child(header)
                    .children(body)
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            open_context_menu(this, event, menu.clone(), window, cx)
                        }),
                    ),
            );
        if !attachments.is_empty() {
            let list = self.render_post_attachments(&format!("monid-{key}"), &attachments, t, cx);
            column = column.child(list);
        }
        column.into_any_element()
    }

    /// What shows under an open call row: the input, the output, the cost
    /// and the run, or the words that the state needs.
    fn monid_call_body(
        &mut self,
        row: &MonidCallRowView,
        key: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let pad = t.space.md;
        let mut body = div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .w_full()
            .min_w_0()
            .px(pad)
            .pb(pad);
        let Some(detail_view) = &row.detail else {
            return body.child(text("Loading the call.", t.text.ui, c.text_tertiary));
        };
        if detail_view.load.is_loading() && detail_view.input.is_empty() {
            return body.child(text("Loading the call.", t.text.ui, c.text_tertiary));
        }
        if let Some(message) = detail_view.load.error() {
            return body.child(crate::ui::parts::error_box(
                SharedString::from(format!("{key}-load-error")),
                message.to_owned(),
                t,
            ));
        }
        let labeled_box = |label: &'static str, id: String, content: String| {
            div()
                .flex()
                .flex_col()
                .gap(t.space.xs)
                .w_full()
                .min_w_0()
                .child(text(label, detail(t), c.text_tertiary))
                .child(input_box(SharedString::from(id), content, t))
        };
        if let Some(note) = &detail_view.note {
            body = body.child(selectable(
                SharedString::from(format!("{key}-note")),
                note.clone(),
                t.text.ui,
                c.text_secondary,
            ));
        }
        if !detail_view.input.is_empty() {
            body = body.child(labeled_box(
                "Input",
                format!("{key}-input"),
                detail_view.input.clone(),
            ));
        }
        if !detail_view.output.is_empty() {
            let mut output = labeled_box(
                "Output",
                format!("{key}-output"),
                detail_view.output.clone(),
            );
            if detail_view.output_cut
                && let Some(file) = detail_view.output_file.clone()
            {
                output = output
                    .child(text("The output is cut here.", detail(t), c.text_tertiary))
                    .child(
                        div().flex().child(
                            button(
                                SharedString::from(format!("{key}-open-output")),
                                "Open the whole output",
                                ButtonKind::Secondary,
                                t,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    // The whole output is a file in the channel
                                    // files. The app opens it as it opens any
                                    // attachment.
                                    this.run_menu(
                                        MenuAction::OpenAttachment(file.clone()),
                                        window,
                                        cx,
                                    )
                                },
                            )),
                        ),
                    );
            }
            body = body.child(output);
        }
        if let Some(cost) = &detail_view.cost_line {
            body = body.child(fact_row(
                "Cost",
                SharedString::from(format!("{key}-cost")),
                cost.clone(),
                t,
            ));
        }
        if let Some(run) = &detail_view.run_line {
            body = body.child(fact_row(
                "Run",
                SharedString::from(format!("{key}-run")),
                run.clone(),
                t,
            ));
        }
        body
    }
}

/// A fact of an open call: "Cost" and the line, in mono.
fn fact_row(label: &'static str, id: SharedString, content: String, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .items_baseline()
        .gap(t.space.md)
        .child(
            text(label, detail(t), c.text_tertiary)
                .w(t.layout.speaker_gutter)
                .flex_none(),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(t.font_mono.clone())
                .child(selectable(id, content, t.text.mono, c.text_primary)),
        )
}

/// A "Why" or "Access" line of an access card.
fn access_line(label: &'static str, id: SharedString, content: String, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .items_start()
        .gap(t.space.md)
        .w_full()
        .min_w_0()
        .child(
            text(label, detail(t), c.text_tertiary)
                .w(t.layout.speaker_gutter)
                .flex_none()
                .line_height(t.text.ui.line),
        )
        .child(
            selectable(id, content, t.text.ui, c.text_primary)
                .flex_1()
                .min_w_0(),
        )
}

/// The menu of a Monid call row: Copy input, Copy output, Copy run ID, and
/// Stop for a call that Monid can stop.
pub fn monid_call_menu(row: &MonidCallRowView) -> Vec<MenuItem> {
    let id = row.call_id.clone();
    let mut items = vec![
        MenuItem::new(
            "Copy input",
            MenuAction::Monid(MonidAction::CopyPart(id.clone(), CallPart::Input)),
        ),
        MenuItem::new(
            "Copy output",
            MenuAction::Monid(MonidAction::CopyPart(id.clone(), CallPart::Output)),
        ),
        MenuItem::new(
            "Copy run ID",
            MenuAction::Monid(MonidAction::CopyPart(id.clone(), CallPart::RunId)),
        ),
    ];
    if row.stoppable {
        items.push(MenuItem::new("Stop", MenuAction::Monid(MonidAction::StopCall(id))).separated());
    }
    items
}
