//! The draft card (Paper D1 and D2).
//!
//! A draft card shows text that goes out in the name of the user. It has
//! three views:
//!
//! - It waits. A card with the accent border: the title with the message
//!   icon, Copy and Edit as icon buttons, the To and Subject lines, the text
//!   with the flagged phrases underlined, and one footer. The footer has a
//!   hint and "Not now" and Send. When the text has flags, the tick "I
//!   checked the marked line" takes the hint's place and Send stays off
//!   until the tick.
//! - Edit in place. The Subject and the text become fields in the same card.
//!   Cancel and Save are in the footer.
//! - After the answer. One line of `chat.answered_line` with the outcome
//!   ("Sent to Dana", "Not sent", "Replaced by a new version"). A click opens
//!   the text that went out.
//!
//! A card that `show_draft` posted has a normal border and no Send, because
//! Consort cannot send it. It has Edit and Copy.

use consort_client::{
    ApprovalChoice, ApprovalView, RequestState, ShownDraftView, TaskStatus,
    connectors::ConnectorCardView, voice::DraftCardView,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    ui::{
        hub_blocked::needs_hub,
        parts::{
            ButtonKind, button, card_corners, detail, focusable, icon, icon_button, input,
            selectable, text, textarea,
        },
        voice::{checkbox, flagged_text},
    },
};

/// The padding at the left and the right of the card.
fn side_pad(t: &Tokens) -> gpui_kit::Pixels {
    t.space.md + t.space.xxs
}

/// The title of the card: "Draft: <title>".
fn card_title(title: &str) -> String {
    format!("Draft: {title}")
}

/// The line under the title of a card that waits: "To <to>, from
/// <connection>". A card with no To value says only where it comes from.
pub(crate) fn from_line(draft: &DraftCardView, connection: &str) -> String {
    match draft.to.as_deref() {
        Some(to) if draft.draft_only => format!("To {to}, saved in {connection}"),
        Some(to) => format!("To {to}, from {connection}"),
        None if draft.draft_only => format!("Saved in {connection}"),
        None => format!("From {connection}"),
    }
}

impl ConsortApp {
    /// The draft card of an approval: the card while it waits, and the line
    /// after the answer. `key` is unique for the approval and the place.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_draft_approval(
        &mut self,
        approval: &ApprovalView,
        card: &ConnectorCardView,
        draft: &DraftCardView,
        key: &str,
        chat: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let waiting = draft.outcome.is_none()
            && matches!(
                approval.state,
                RequestState::Pending | RequestState::Sending
            );
        if waiting {
            self.render_waiting_draft(approval, card, draft, key, chat, t, cx)
        } else {
            self.render_closed_draft(approval, card, draft, key, t, cx)
        }
    }

    /// The frame of a card: the accent border while it needs the user.
    fn draft_frame(&self, key: &str, accent: bool, chat: bool, t: &Tokens) -> Div {
        let c = &t.colors;
        card_corners(div(), chat, t)
            .debug_selector({
                let id = format!("{key}-draft-card");
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
            .border_color(if accent {
                c.accent_text
            } else {
                c.border_subtle
            })
    }

    /// The title row and the lines under it. `right` sits at the end of the
    /// title row, and `aside` at the end of the first line.
    fn draft_header(
        &self,
        key: &str,
        title: &str,
        lines: Vec<String>,
        right: Vec<AnyElement>,
        aside: Option<AnyElement>,
        t: &Tokens,
    ) -> Div {
        let c = &t.colors;
        let mut head = div()
            .flex()
            .flex_col()
            .gap(t.space.xs)
            .py(t.space.md)
            .px(side_pad(t))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(icon("message", t.icon.dense, c.text_secondary).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .type_style(t.text.ui_strong)
                            .text_color(c.text_primary)
                            .debug_selector({
                                let id = format!("{key}-draft-title");
                                move || id.clone()
                            })
                            .child(gpui_kit::base::SelectableText::new(
                                SharedString::from(format!("{key}.draft-title")),
                                card_title(title),
                            )),
                    )
                    .when(!right.is_empty(), |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(t.space.sm)
                                .children(right),
                        )
                    }),
            );
        let mut aside = aside;
        for (i, line) in lines.into_iter().enumerate() {
            let row = div()
                .flex()
                .items_baseline()
                .gap(t.space.md)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(text(line, t.text.ui, c.text_tertiary)),
                )
                .when(i == 0, |d| d.children(aside.take()));
            head = head.child(row.debug_selector({
                let id = format!("{key}-draft-line-{i}");
                move || id.clone()
            }));
        }
        head
    }

    /// The text of the card under a line at the top.
    fn draft_body(&self, key: &str, value: &str, ranges: &[(usize, usize)], t: &Tokens) -> Div {
        let c = &t.colors;
        div()
            .py(t.space.md)
            .px(side_pad(t))
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                flagged_text(format!("{key}-draft-text"), value, ranges, t.text.body, t)
                    .w_full()
                    .min_w_0()
                    .debug_selector({
                        let id = format!("{key}-draft-body");
                        move || id.clone()
                    }),
            )
    }

    /// The footer: `left` takes the room, the buttons keep their size.
    fn draft_footer(
        &self,
        key: &str,
        left: AnyElement,
        buttons: Vec<AnyElement>,
        t: &Tokens,
    ) -> Div {
        div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pb(t.space.md)
            .px(side_pad(t))
            .debug_selector({
                let id = format!("{key}-draft-footer");
                move || id.clone()
            })
            .child(div().flex_1().min_w_0().child(left))
            .children(buttons)
    }

    /// Copy puts the text on the clipboard and says so.
    fn copy_draft_button(
        &mut self,
        key: &str,
        value: String,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        icon_button(
            SharedString::from(format!("{key}-draft-copy")),
            "copy",
            t.component.chat.footer_button,
            t.icon.dense,
            c.text_secondary,
            c.hover_card,
            t,
            cx,
        )
        .debug_selector(|| "draft-copy".into())
        .on_click(cx.listener(move |this, _, _, cx| this.copy_draft_text(value.clone(), cx)))
        .into_any_element()
    }

    /// Puts the text of a draft on the clipboard and says so.
    pub(crate) fn copy_draft_text(&mut self, value: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.show_notice("Copied the text.".into(), cx);
    }

    /// The card of a draft that waits, or that is in Edit.
    #[allow(clippy::too_many_arguments)]
    fn render_waiting_draft(
        &mut self,
        approval: &ApprovalView,
        card: &ConnectorCardView,
        draft: &DraftCardView,
        key: &str,
        chat: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let editing = self
            .voice
            .editing
            .as_ref()
            .is_some_and(|(id, k)| id == &approval.id && k == key);
        let subject_key = draft
            .editable
            .iter()
            .find(|f| f.key == "subject")
            .map(|f| f.key.clone());
        let body_field = draft.editable.iter().find(|f| f.key != "subject").cloned();
        let frame = self.draft_frame(key, true, chat, t);
        // The Subject is a line of the header. In Edit it is a field.
        let mut lines = vec![from_line(draft, &card.connection_name)];
        if !editing && let Some(subject) = &draft.subject {
            lines.push(format!("Subject: {subject}"));
        }
        let noted = draft.not_my_voice || self.voice.feedback.contains(&approval.id);
        let aside: Option<AnyElement> = (!editing && (draft.in_voice || !draft.flags.is_empty()))
            .then(|| {
                if noted {
                    text("Noted.", detail(t), c.text_tertiary)
                        .flex_none()
                        .into_any_element()
                } else {
                    let id = approval.id.clone();
                    focusable(
                        div().id(SharedString::from(format!("{key}-not-my-voice"))),
                        t,
                    )
                    .debug_selector(|| "not-my-voice".into())
                    .flex_none()
                    .cursor_pointer()
                    .whitespace_nowrap()
                    .type_style(detail(t))
                    .text_color(c.text_tertiary)
                    .hover(|s| s.text_color(c.text_secondary))
                    .child("Not how I write")
                    .on_click(cx.listener(move |this, _, _, cx| this.not_my_voice(id.clone(), cx)))
                    .into_any_element()
                }
            });
        let mut right: Vec<AnyElement> = Vec::new();
        if !editing {
            if draft.in_voice {
                right.push(
                    text("In your voice", detail(t), c.text_tertiary)
                        .flex_none()
                        .whitespace_nowrap()
                        .into_any_element(),
                );
            }
            right.push(self.copy_draft_button(key, draft.body.clone(), t, cx));
            if !draft.editable.is_empty() {
                let id = approval.id.clone();
                let card_key = key.to_owned();
                let subject = draft
                    .editable
                    .iter()
                    .find(|f| f.key == "subject")
                    .map(|f| f.value.clone());
                let body = body_field.as_ref().map(|f| f.value.clone());
                right.push(
                    icon_button(
                        SharedString::from(format!("{key}-draft-edit")),
                        "edit",
                        t.component.chat.footer_button,
                        t.icon.dense,
                        c.text_secondary,
                        c.hover_card,
                        t,
                        cx,
                    )
                    .debug_selector(|| "draft-edit".into())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_draft_edit(
                            id.clone(),
                            card_key.clone(),
                            subject.clone(),
                            body.clone(),
                            window,
                            cx,
                        )
                    }))
                    .into_any_element(),
                );
            }
        }
        let head = self.draft_header(key, &draft.title, lines, right, aside, t);
        if editing {
            let id = approval.id.clone();
            let body_key = body_field.map(|b| b.key);
            let fields = self.draft_edit_fields(subject_key.is_some(), t);
            let footer = self.draft_footer(
                key,
                text(
                    "Consort checks the text again when you save.",
                    detail(t),
                    c.text_tertiary,
                )
                .into_any_element(),
                vec![
                    button(
                        SharedString::from(format!("{key}-cancel")),
                        "Cancel",
                        ButtonKind::Secondary,
                        t,
                    )
                    .debug_selector(|| "draft-cancel".into())
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_draft_edit(cx)))
                    .into_any_element(),
                    button(
                        SharedString::from(format!("{key}-save")),
                        "Save",
                        ButtonKind::Primary,
                        t,
                    )
                    .debug_selector(|| "draft-save".into())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.save_draft_edit(id.clone(), subject_key.clone(), body_key.clone(), cx)
                    }))
                    .into_any_element(),
                ],
                t,
            );
            return frame
                .child(head)
                .child(fields)
                .child(footer)
                .into_any_element();
        }
        // The text, with the flagged phrases underlined. The ranges are
        // offsets in the value of the field, so the value is the field's.
        let label = body_field.as_ref().map(|f| f.label.clone());
        let value = label
            .as_deref()
            .and_then(|l| card.fields.iter().find(|f| f.label == l))
            .map(|f| f.value.clone())
            .unwrap_or_else(|| draft.body.clone());
        let ranges = label
            .as_deref()
            .map(|l| draft.ranges(l))
            .unwrap_or_default();
        let mut out = frame
            .child(head)
            .child(self.draft_body(key, &value, &ranges, t));
        // The flags, each with its words.
        if !draft.flags.is_empty() {
            let mut list = div()
                .flex()
                .flex_col()
                .gap(t.space.sm)
                .px(side_pad(t))
                .pb(t.space.md);
            for flag in &draft.flags {
                list = list.child(
                    div()
                        .flex()
                        .gap(t.space.sm + t.space.xxs)
                        .child(
                            div()
                                .w(t.component.voice.flag_bar)
                                .flex_none()
                                .rounded(t.radius.full)
                                .bg(c.accent_text),
                        )
                        .child(div().flex_1().min_w_0().child(text(
                            flag.line(),
                            t.text.ui,
                            c.accent_text,
                        ))),
                );
            }
            out = out.child(list);
        }
        // The footer: the hint, or the tick when there are flags.
        let checked = self.draft_checked(&approval.id, &draft.review_hash);
        let blocked = draft.allow_blocked(checked);
        let left: AnyElement = if draft.flags.is_empty() {
            text(
                "Edit it here, or tell me what to change.",
                detail(t),
                c.text_tertiary,
            )
            .into_any_element()
        } else {
            let hash = draft.review_hash.clone();
            let id = approval.id.clone();
            div()
                .flex()
                .items_center()
                .gap(t.space.sm + t.space.xxs)
                .child(
                    checkbox(format!("{key}-checked"), checked, t)
                        .debug_selector(|| "draft-checked".into())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_draft_checked(&id, &hash, cx)
                        })),
                )
                .child(text(draft.check_label(), t.text.ui, c.text_primary))
                .into_any_element()
        };
        let offline = self.answers_blocked();
        let allow_label = approval.choice_label(ApprovalChoice::AllowOnce);
        let allow: AnyElement = if blocked {
            div()
                .id(SharedString::from(format!("{key}-allow")))
                .debug_selector(|| "draft-allow-off".into())
                .flex()
                .flex_none()
                .items_center()
                .px(t.space.md)
                .py(t.component.button_pad_y)
                .rounded(t.radius.md)
                .bg(c.surface_selected)
                .type_style(t.text.ui_strong)
                .text_color(c.text_tertiary)
                .child(allow_label)
                .into_any_element()
        } else {
            let id = approval.id.clone();
            let checked_hash = checked.then(|| draft.review_hash.clone());
            needs_hub(
                button(
                    SharedString::from(format!("{key}-allow")),
                    allow_label,
                    ButtonKind::Primary,
                    t,
                )
                .debug_selector(|| "draft-allow".into())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.answer_draft_card(
                        id.clone(),
                        ApprovalChoice::AllowOnce,
                        checked_hash.clone(),
                        window,
                        cx,
                    )
                })),
                offline,
                t,
            )
            .into_any_element()
        };
        let deny = {
            let id = approval.id.clone();
            needs_hub(
                button(
                    SharedString::from(format!("{key}-deny")),
                    approval.choice_label(ApprovalChoice::Deny),
                    ButtonKind::Secondary,
                    t,
                )
                .debug_selector(|| "draft-deny".into())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.answer_approval(id.clone(), ApprovalChoice::Deny, window, cx)
                })),
                offline,
                t,
            )
            .into_any_element()
        };
        out.child(self.draft_footer(key, left, vec![deny, allow], t))
            .into_any_element()
    }

    /// The Subject and the text as fields, in one box with the accent border.
    fn draft_edit_fields(&self, with_subject: bool, t: &Tokens) -> Div {
        let c = &t.colors;
        let v = t.component.voice;
        div()
            .mx(t.space.md)
            .mb(t.space.md)
            .flex()
            .flex_col()
            .rounded(t.radius.md)
            .border(t.border_width)
            .border_color(c.accent_text)
            .bg(c.surface_canvas)
            .when(with_subject, |d| {
                d.child(
                    div()
                        .min_h(v.subject_box)
                        .py(t.space.sm)
                        .px(t.space.sm + t.space.xxs)
                        .border_b(t.border_width)
                        .border_color(c.border_subtle)
                        .child(input(&self.voice_inputs.subject, t.text.ui)),
                )
            })
            .child(
                div()
                    .min_h(v.body_box)
                    .py(t.space.sm)
                    .px(t.space.sm + t.space.xxs)
                    .child(textarea(&self.voice_inputs.body, t.text.body)),
            )
    }

    /// The line after the answer. Open, it shows the text that went out (or
    /// the text that waited, for a card that did not run).
    fn render_closed_draft(
        &mut self,
        approval: &ApprovalView,
        card: &ConnectorCardView,
        draft: &DraftCardView,
        key: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let ran = draft
            .outcome
            .as_ref()
            .is_some_and(|o| o.final_text.is_some());
        let answer = draft
            .outcome
            .as_ref()
            .map(|o| o.text.clone())
            .unwrap_or_else(|| approval.answer_label());
        let open = self.approvals_open.contains(key);
        let body = open.then(|| {
            let shown = draft
                .outcome
                .as_ref()
                .and_then(|o| o.final_text.clone())
                .unwrap_or_else(|| draft.body.clone());
            let mut body = div()
                .flex()
                .flex_col()
                .gap(t.space.sm)
                .px(t.space.md)
                .pb(t.space.md);
            body = body.child(text(
                from_line(draft, &card.connection_name),
                t.text.ui,
                c.text_tertiary,
            ));
            if let Some(subject) = &draft.subject {
                body = body.child(text(
                    format!("Subject: {subject}"),
                    t.text.ui,
                    c.text_tertiary,
                ));
            }
            body.child(
                div()
                    .debug_selector({
                        let id = format!("{key}-draft-final");
                        move || id.clone()
                    })
                    .child(selectable(
                        SharedString::from(format!("{key}-draft-final-text")),
                        shown,
                        t.text.ui,
                        c.text_primary,
                    )),
            )
        });
        self.render_closed_line(
            key,
            if ran {
                TaskStatus::Resolved
            } else {
                TaskStatus::UpNext
            },
            card_title(&draft.title),
            false,
            answer,
            body,
            t,
            cx,
        )
    }

    // The card that `show_draft` posts --------------------------------------

    /// A card from `show_draft`: it has a normal border and says that
    /// Consort cannot send it. Edit changes the title and the text in place.
    /// Copy puts the text on the clipboard.
    pub(crate) fn render_shown_draft(
        &mut self,
        draft: &ShownDraftView,
        key: &str,
        chat: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let editing = self
            .voice
            .editing
            .as_ref()
            .is_some_and(|(id, k)| id == &draft.message_id && k == key);
        let frame = self.draft_frame(key, false, chat, t);
        let lines: Vec<String> = draft
            .to
            .as_deref()
            .map(|to| vec![format!("To {to}")])
            .unwrap_or_default();
        let aside = (draft.edited && !editing).then(|| {
            text("Edited", detail(t), c.text_tertiary)
                .flex_none()
                .into_any_element()
        });
        let head = self.draft_header(key, &draft.title, lines, Vec::new(), aside, t);
        let id = draft.message_id.clone();
        if editing {
            let footer =
                self.draft_footer(
                    key,
                    text(
                        "Your changes stay in this card.",
                        detail(t),
                        c.text_tertiary,
                    )
                    .into_any_element(),
                    vec![
                        button(
                            SharedString::from(format!("{key}-cancel")),
                            "Cancel",
                            ButtonKind::Secondary,
                            t,
                        )
                        .debug_selector(|| "draft-cancel".into())
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_draft_edit(cx)))
                        .into_any_element(),
                        button(
                            SharedString::from(format!("{key}-save")),
                            "Save",
                            ButtonKind::Primary,
                            t,
                        )
                        .debug_selector(|| "draft-save".into())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.save_shown_draft_edit(id.clone(), cx)
                        }))
                        .into_any_element(),
                    ],
                    t,
                );
            let fields = self.draft_edit_fields(true, t);
            return frame
                .child(head)
                .child(fields)
                .child(footer)
                .into_any_element();
        }
        let edit = {
            let id = draft.message_id.clone();
            let card_key = key.to_owned();
            let title = draft.title.clone();
            let body = draft.body.clone();
            button(
                SharedString::from(format!("{key}-edit")),
                "Edit",
                ButtonKind::Secondary,
                t,
            )
            .debug_selector(|| "draft-edit".into())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.start_draft_edit(
                    id.clone(),
                    card_key.clone(),
                    Some(title.clone()),
                    Some(body.clone()),
                    window,
                    cx,
                )
            }))
            .into_any_element()
        };
        let copy = {
            let body = draft.body.clone();
            button(
                SharedString::from(format!("{key}-copy")),
                "Copy",
                ButtonKind::Secondary,
                t,
            )
            .debug_selector(|| "draft-copy".into())
            .on_click(cx.listener(move |this, _, _, cx| this.copy_draft_text(body.clone(), cx)))
            .into_any_element()
        };
        let footer = self.draft_footer(
            key,
            text(
                "Consort cannot send this one for you.",
                detail(t),
                c.text_tertiary,
            )
            .into_any_element(),
            vec![edit, copy],
            t,
        );
        frame
            .child(head)
            .child(self.draft_body(key, &draft.body, &[], t))
            .child(footer)
            .into_any_element()
    }

    /// Save on a card from `show_draft`: the title and the text go to the
    /// hub. The card shows the new text when the hub answers.
    pub(crate) fn save_shown_draft_edit(&mut self, message_id: String, cx: &mut Context<Self>) {
        let title = self.voice_inputs.subject.read(cx).value().to_string();
        let body = self.voice_inputs.body.read(cx).value().to_string();
        self.voice.editing = None;
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.edit_draft(message_id, Some(title), Some(body)).await
        });
        cx.notify();
    }
}
