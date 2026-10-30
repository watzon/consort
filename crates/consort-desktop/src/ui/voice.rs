//! Write in my voice, drawn: Settings > Voice off (Paper `H3V-0`) and on
//! (`H7T-0`), the voice task card and the "Open the full guide" link of the
//! offer (`HL7-0`), the card of a draft in your voice with its flags, the
//! tick, Edit and "Not how I write" (`HTB-0`), and the voice dialogs.

use consort_client::connectors::ConnectorCardView;
use consort_client::voice::{
    Disclosure, DraftCardView, GuideEdit, GuideSectionKind, GuideSectionView, VoiceSampleState,
    VoiceSourceKind, VoiceSourceRow,
};
use consort_client::{ApprovalChoice, ApprovalView, RequestState};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, Focusable as _, HighlightStyle, InteractiveElement as _, IntoElement,
    ParentElement, SharedString, Stateful, StatefulInteractiveElement as _, Styled, StyledText,
    UnderlineStyle, Window, div, prelude::FluentBuilder as _,
};

use crate::app::ConsortApp;
use crate::ui::admin_parts::{column, header, page, rows, section_title};
use crate::ui::convo_dialogs::DialogParts;
use crate::ui::dialogs::{field_block, input_box};
use crate::ui::hub_blocked::needs_hub;
use crate::ui::parts::{
    ButtonKind, button, detail, focusable, icon, icon_text_button, selectable, text, textarea,
};
use crate::ui::settings::switch;
use crate::voice::{VoiceDialog, example_key, rule_key};

/// A checkbox: the accent fill with a check when ticked.
pub fn checkbox(id: impl Into<SharedString>, on: bool, t: &Tokens) -> Stateful<Div> {
    let c = &t.colors;
    let v = t.component.voice;
    focusable(div().id(gpui_kit::ElementId::Name(id.into())), t)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(v.check_box)
        .rounded(t.radius.sm)
        .cursor_pointer()
        .when(on, |d| {
            d.bg(c.accent_fill)
                .child(icon("check", v.check_icon, c.accent_on_fill))
        })
        .when(!on, |d| {
            d.bg(c.surface_raised)
                .border(t.border_width)
                .border_color(c.border_strong)
        })
}

/// A radio button: an `accent.text` ring with a dot when chosen.
fn radio(on: bool, t: &Tokens) -> Div {
    let c = &t.colors;
    let v = t.component.voice;
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(v.check_box)
        .mt(t.space.xxs)
        .rounded(t.radius.full)
        .bg(c.surface_raised)
        .border(t.border_width)
        .border_color(if on { c.accent_text } else { c.border_strong })
        .when(on, |d| {
            d.child(
                div()
                    .size(v.radio_dot)
                    .rounded(t.radius.full)
                    .bg(c.accent_text),
            )
        })
}

/// The tile of a source: a monogram or an icon on `surface.selected`.
fn tile(content: impl IntoElement, t: &Tokens) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(t.component.voice.source_tile)
        .rounded(t.radius.md)
        .bg(t.colors.surface_selected)
        .child(content)
}

/// A title and the lines under it.
fn titled(title: &str, lines: Vec<(String, gpui_kit::Hsla)>, t: &Tokens) -> Div {
    let mut d = div().flex().flex_col().flex_1().min_w_0().child(text(
        title.to_owned(),
        t.text.ui_strong,
        t.colors.text_primary,
    ));
    for (line, color) in lines {
        d = d.child(text(line, detail(t), color));
    }
    d
}

/// A section title with a hint under it.
fn section_head(title: &str, hint: &str, t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(t.space.xxs)
        .child(section_title(title, t))
        .child(text(hint.to_owned(), detail(t), t.colors.text_tertiary))
}

/// A row of a bordered group with a line under it.
fn group_row(last: bool, t: &Tokens) -> Div {
    div()
        .flex()
        .items_start()
        .gap(t.space.md)
        .py(t.space.md)
        .px(t.space.lg)
        .when(!last, |d| {
            d.border_b(t.border_width)
                .border_color(t.colors.border_subtle)
        })
}

/// Body text with the flagged phrases underlined in `accent.text`. The
/// colored text lies under a selectable copy with clear text, so the user
/// can still select and copy the body.
pub fn flagged_text(
    id: impl Into<SharedString>,
    value: &str,
    ranges: &[(usize, usize)],
    t: &Tokens,
) -> Div {
    let c = &t.colors;
    let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = ranges
        .iter()
        .filter(|(s, e)| {
            s < e && *e <= value.len() && value.is_char_boundary(*s) && value.is_char_boundary(*e)
        })
        .map(|(s, e)| {
            (
                *s..*e,
                HighlightStyle {
                    color: Some(c.accent_text),
                    underline: Some(UnderlineStyle {
                        thickness: t.border_width,
                        color: Some(c.accent_text),
                        wavy: false,
                    }),
                    ..HighlightStyle::default()
                },
            )
        })
        .collect();
    let id = id.into();
    if highlights.is_empty() {
        return selectable(id, value.to_owned(), t.text.ui, c.text_primary);
    }
    div()
        .relative()
        .type_style(t.text.ui)
        .text_color(c.text_primary)
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .child(StyledText::new(value.to_owned()).with_highlights(highlights)),
        )
        .child(
            div()
                .text_color(gpui_kit::transparent_black())
                .child(gpui_kit::base::SelectableText::new(id, value.to_owned())),
        )
}

impl ConsortApp {
    /// Settings > Voice.
    pub fn render_voice(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let view = self.state.voice.clone();
        let line = text(
            "How drafts that go out in your name sound. Consort learns your style only from sources that you choose.",
            t.text.ui,
            c.text_secondary,
        );
        let mut col = column(t)
            .gap(t.space.xl + t.space.sm)
            .child(header("Voice", line, None, t));
        if !view.available {
            return page("voice-page", t)
                .child(col.child(text(
                    "This hub is older than the app and cannot write in your voice yet. Update the hub, then open this page again.",
                    t.text.ui,
                    c.text_secondary,
                )))
                .into_any_element();
        }
        if let Some(error) = view.error.clone() {
            col = col.child(crate::ui::parts::error_box("voice-error", error, t));
        }
        let col = if view.enabled {
            self.voice_on_page(col, t, window, cx)
        } else {
            self.voice_off_page(col, t, cx)
        };
        page("voice-page", t).child(col).into_any_element()
    }

    /// The switch row.
    fn voice_switch_row(
        &mut self,
        on: bool,
        last: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        crate::ui::admin_parts::row(last, t)
            .child(titled(
                "Write in my voice",
                vec![(
                    "Drafts that go out in your name will sound like you. Consort learns from text you wrote and keeps a style guide that you can read, edit and reset.".to_owned(),
                    t.colors.text_tertiary,
                )],
                t,
            ))
            .child(
                switch("voice-switch", on, t)
                    .debug_selector(|| "voice-switch".into())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_voice_enabled(!on, cx))),
            )
    }

    /// The source rows: mail and chat connections, documents and edits.
    fn voice_sources(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let rows_view: Vec<VoiceSourceRow> = self.voice_source_rows();
        let documents = self.voice_documents();
        let mut group = rows(t);
        if rows_view.is_empty() {
            group = group.child(group_row(false, t).child(titled(
                "No mail or chat connection",
                vec![(
                        "Connect Gmail or Slack in Settings > Connectors to learn from them."
                            .to_owned(),
                        c.text_tertiary,
                    )],
                t,
            )));
        }
        for row in rows_view {
            let id = row.connection_id.clone();
            let tick = {
                let id = id.clone();
                checkbox(format!("voice-source-{id}"), row.ticked, t)
                    .mt(t.space.xxs)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_voice_source(&id, cx)))
            };
            let mono = tile(
                text(row.monogram.clone(), t.text.ui_strong, c.text_secondary),
                t,
            );
            let mut info = div().flex().flex_col().flex_1().min_w_0().gap(t.space.sm);
            match row.kind {
                Some(VoiceSourceKind::Mail) => {
                    info = info.child(titled(
                        &row.title,
                        vec![
                            (row.detail.clone(), c.text_secondary),
                            (
                                "Only mail that you sent, at most 300 messages. Threads about HR, legal, medical, salary, offers and passwords are skipped.".to_owned(),
                                c.text_tertiary,
                            ),
                        ],
                        t,
                    ));
                }
                _ => {
                    let public =
                        {
                            let id = id.clone();
                            checkbox(format!("voice-public-{id}"), row.ticked, t)
                                .mt(t.space.xxs)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_voice_source(&id, cx)
                                }))
                        };
                    let private =
                        {
                            let id = id.clone();
                            checkbox(format!("voice-private-{id}"), row.include_private, t)
                                .mt(t.space.xxs)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_voice_private(&id, cx)
                                }))
                        };
                    info = info
                        .child(titled(
                            &row.title,
                            vec![(row.detail.clone(), c.text_tertiary)],
                            t,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(t.space.sm)
                                .child(
                                    div()
                                        .flex()
                                        .items_start()
                                        .gap(t.space.sm + t.space.xxs)
                                        .child(public)
                                        .child(text("Public channels", t.text.ui, c.text_primary)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_start()
                                        .gap(t.space.sm + t.space.xxs)
                                        .child(private)
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .child(text(
                                                    "Direct messages",
                                                    t.text.ui,
                                                    c.text_primary,
                                                ))
                                                .child(text(
                                                    "Private channels too. Consort reads them only when you tick this.",
                                                    detail(t),
                                                    c.text_tertiary,
                                                )),
                                        ),
                                ),
                        );
                }
            }
            group = group.child(group_row(false, t).child(tick).child(mono).child(info));
        }
        // Documents you pick.
        let docs_line = if documents.is_empty() {
            "None yet. Choose files from a channel or a path on a worker. Your own documents are the cleanest source.".to_owned()
        } else {
            format!(
                "{} picked. Consort reads them the next time it learns.",
                if documents.len() == 1 {
                    "1 document".to_owned()
                } else {
                    format!("{} documents", documents.len())
                }
            )
        };
        let mut doc_info = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(t.space.xs)
            .child(titled(
                "Documents you pick",
                vec![(docs_line, c.text_tertiary)],
                t,
            ));
        for (i, path) in documents.iter().enumerate() {
            let remove = path.clone();
            doc_info = doc_info.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(div().font_family(t.font_mono.clone()).child(selectable(
                        SharedString::from(format!("voice-doc-{i}")),
                        path.clone(),
                        t.text.mono,
                        c.text_primary,
                    )))
                    .child(
                        button(
                            SharedString::from(format!("voice-doc-remove-{i}")),
                            "Remove",
                            ButtonKind::Quiet,
                            t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove_voice_document(&remove, cx)
                        })),
                    ),
            );
        }
        group = group.child(
            group_row(false, t)
                .items_center()
                .child(div().size(t.component.voice.check_box).flex_none())
                .child(tile(icon("file", t.icon.size, c.text_secondary), t))
                .child(doc_info)
                .child(
                    icon_text_button("voice-add-document", "plus", "Add", t).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.open_voice_dialog(VoiceDialog::AddDocument, window, cx)
                        },
                    )),
                ),
        );
        // Your edits to drafts.
        let edits = self.state.voice.learn_from_edits;
        group.child(
            group_row(true, t)
                .child(
                    checkbox("voice-edits", edits, t)
                        .mt(t.space.sm)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_voice_edits(cx))),
                )
                .child(tile(icon("edit", t.icon.size, c.text_secondary), t))
                .child(titled(
                    "Your edits to drafts",
                    vec![(
                        "When you change a draft before you approve it, Consort can learn from the change. One edit changes nothing. Several edits that agree make an offer.".to_owned(),
                        c.text_tertiary,
                    )],
                    t,
                )),
        )
    }

    /// The page with the voice off: the sources, the promises, and Turn on
    /// and learn.
    fn voice_off_page(&mut self, col: Div, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let switch_card = rows(t).child(self.voice_switch_row(false, true, t, cx));
        let sources = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section_head(
                "Where Consort learns from",
                "Consort reads only what you tick, and only after you turn the switch on.",
                t,
            ))
            .child(self.voice_sources(t, cx));
        let promise = |title: &str, line: &str, last: bool| {
            crate::ui::admin_parts::row(last, t).child(titled(
                title,
                vec![(line.to_owned(), c.text_tertiary)],
                t,
            ))
        };
        let never = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section_title("What the voice never changes", t))
            .child(
                rows(t)
                    .child(promise(
                        "Wording only",
                        "The voice adds no date, price, promise or agreement that you did not give. Consort marks any that appear.",
                        false,
                    ))
                    .child(promise(
                        "Approval",
                        "While the voice is on, every send asks you first, also when the connection may run without asking. A draft follows the connection's rule.",
                        false,
                    ))
                    .child(promise(
                        "Where it applies",
                        "Only text that goes out in your name. Not the assistant's replies to you, task briefs, code or commits.",
                        true,
                    )),
            );
        let rows_now = self.voice_source_rows();
        let can = rows_now.iter().any(|r| r.ticked) || !self.voice_documents().is_empty();
        let start = button("voice-turn-on", "Turn on and learn", ButtonKind::Primary, t)
            .debug_selector(|| "voice-turn-on".into())
            .when(!can, |b| b.opacity(t.opacity.busy))
            .on_click(cx.listener(|this, _, _, cx| this.set_voice_enabled(true, cx)));
        let offline = self.answers_blocked();
        col.child(switch_card)
            .child(sources)
            .child(never)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.lg)
                    .child(needs_hub(start, offline, t))
                    .child(
                        text(
                            "Consort reads the ticked sources on one of your workers. It shows you the style guide before it saves anything.",
                            detail(t),
                            c.text_tertiary,
                        )
                        .flex_1(),
                    ),
            )
    }

    /// The page with the voice on: what it learned, the style guide, Try
    /// it, Disclosure, Turn off and Reset.
    fn voice_on_page(
        &mut self,
        col: Div,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let view = self.state.voice.clone();
        // What it learned from, or the learning state.
        let learned = match &view.learning {
            Some(learning) => {
                let task = learning.task_id.clone();
                crate::ui::admin_parts::row(true, t)
                    .child(titled(
                        &learning.line,
                        vec![(
                            "Consort shows you the style guide in Tutti before it saves anything."
                                .to_owned(),
                            c.text_tertiary,
                        )],
                        t,
                    ))
                    .child(
                        button("voice-open-task", "Open the task", ButtonKind::Quiet, t).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.page = crate::app::Page::Main;
                                this.open_thread(task.clone(), window, cx);
                            }),
                        ),
                    )
            }
            None => crate::ui::admin_parts::row(true, t)
                .child(titled(
                    view.learned_line
                        .as_deref()
                        .unwrap_or("Consort has not learned your voice yet"),
                    vec![(view.sources_line.clone(), c.text_tertiary)],
                    t,
                ))
                .child(
                    button(
                        "voice-change-sources",
                        "Change sources",
                        ButtonKind::Quiet,
                        t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.voice.changing_sources = !this.voice.changing_sources;
                        cx.notify();
                    })),
                )
                .child(needs_hub(
                    button("voice-learn-again", "Learn again", ButtonKind::Secondary, t)
                        .debug_selector(|| "voice-change-sources".into())
                        .debug_selector(|| "voice-learn-again".into())
                        .on_click(cx.listener(|this, _, _, cx| this.learn_voice_again(cx))),
                    self.answers_blocked(),
                    t,
                )),
        };
        let mut col = col.child(
            rows(t)
                .child(self.voice_switch_row(true, false, t, cx))
                .child(learned),
        );
        if self.voice.changing_sources {
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.md)
                    .child(section_head(
                        "Where Consort learns from",
                        "A change applies the next time Consort learns. Learn again to use it now.",
                        t,
                    ))
                    .child(self.voice_sources(t, cx)),
            );
        }
        col.child(self.voice_guide(&view.guide, &view.guide_caption, t, window, cx))
            .child(self.voice_try_it(t, window, cx))
            .child(self.voice_disclosure(view.disclosure, t, cx))
            .child(
                rows(t)
                    .child(
                        crate::ui::admin_parts::row(false, t)
                            .child(titled(
                                "Turn off the voice",
                                vec![(
                                    "Drafts sound neutral again. Consort keeps the style guide, and you can turn the voice on later.".to_owned(),
                                    c.text_tertiary,
                                )],
                                t,
                            ))
                            .child(
                                button("voice-turn-off", "Turn off", ButtonKind::Secondary, t)
                        .debug_selector(|| "voice-turn-off".into())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_voice_enabled(false, cx)
                                    })),
                            ),
                    )
                    .child(
                        crate::ui::admin_parts::row(true, t)
                            .child(titled(
                                "Reset the voice",
                                vec![(
                                    "Deletes the style guide and what Consort learned. The old version stays for 30 days, so you can restore it.".to_owned(),
                                    c.text_tertiary,
                                )],
                                t,
                            ))
                            .child(
                                button("voice-reset", "Reset", ButtonKind::Secondary, t)
                        .debug_selector(|| "voice-reset".into())
                                    .text_color(c.status_failed)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_voice_dialog(VoiceDialog::ConfirmReset, window, cx)
                                    })),
                            ),
                    ),
            )
    }

    /// The Remove slot at the right of a rule or an example: it shows on
    /// hover, in a fixed lane.
    fn remove_slot(
        &mut self,
        key: &str,
        hovered: bool,
        edit: GuideEdit,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let slot = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_end()
            .gap(t.space.xs + t.space.xxs)
            .w(t.component.voice.remove_slot)
            .h(t.text.ui.line);
        if !hovered {
            return slot;
        }
        slot.child(
            focusable(
                div().id(SharedString::from(format!("voice-remove-{key}"))),
                t,
            )
            .flex()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .cursor_pointer()
            .child(icon("close", t.icon.dense, c.text_secondary))
            .child(text("Remove", detail(t), c.text_secondary).font_weight(t.text.ui_strong.weight))
            .on_click(cx.listener(move |this, _, _, cx| this.edit_voice_guide(edit.clone(), cx))),
        )
    }

    /// One section card of the style guide.
    fn guide_section(
        &mut self,
        section: &GuideSectionView,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let v = t.component.voice;
        let mut card = rows(t).child(
            div()
                .flex()
                .items_baseline()
                .gap(t.space.sm)
                .py(t.space.md)
                .px(t.space.lg)
                .border_b(t.border_width)
                .border_color(c.border_subtle)
                .child(text(
                    section.title.clone(),
                    t.text.ui_strong,
                    c.text_primary,
                ))
                .child(text(section.subtitle.clone(), detail(t), c.text_tertiary).flex_1())
                .when_some(section.learned.clone(), |d, learned| {
                    d.child(text(learned, detail(t), c.text_tertiary))
                }),
        );
        if let Some(note) = &section.note
            && section.rules.is_empty()
            && section.examples.is_empty()
        {
            card = card.child(div().py(t.space.md).px(t.space.lg).child(text(
                note.clone(),
                detail(t),
                c.text_tertiary,
            )));
        }
        let rule_count = section.rules.len();
        let last_is_rule = section.examples.is_empty() && section.kind != GuideSectionKind::Hard;
        for (i, rule) in section.rules.iter().enumerate() {
            let key = rule_key(&section.key, i);
            let hovered = self.voice.hovered.as_deref() == Some(key.as_str());
            let hover_key = key.clone();
            let remove = self.remove_slot(
                &key,
                hovered,
                GuideEdit::RemoveRule {
                    section: section.key.clone(),
                    index: i,
                },
                t,
                cx,
            );
            let last = last_is_rule && i + 1 == rule_count;
            card = card.child(
                div()
                    .id(SharedString::from(format!("voice-rule-{key}")))
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .pt(v.rule_pad_y)
                    .pb(if last { t.space.md } else { v.rule_pad_y })
                    .px(t.space.lg)
                    .when(hovered, |d| d.bg(c.surface_card))
                    .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                        set_hover(this, &hover_key, *on, cx)
                    }))
                    .child(
                        div()
                            .size(v.rule_dot)
                            .flex_none()
                            .rounded(t.radius.full)
                            .bg(c.text_tertiary),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .flex_1()
                            .min_w_0()
                            .gap_x(t.space.xs)
                            .child(selectable(
                                SharedString::from(format!("voice-rule-text-{key}")),
                                rule.text.clone(),
                                t.text.ui,
                                c.text_primary,
                            ))
                            .when(rule.by_user, |d| {
                                d.child(text("(you)", t.text.ui, c.accent_text))
                            }),
                    )
                    .child(remove),
            );
        }
        if !section.examples.is_empty() {
            card = card.child(div().pt(t.space.sm).pb(t.space.xs).px(t.space.lg).child(
                text("Examples", detail(t), c.text_tertiary).font_weight(t.text.ui_strong.weight),
            ));
            let mut list = div()
                .flex()
                .flex_col()
                .gap(t.space.sm)
                .pt(t.space.xs)
                .pb(t.space.lg)
                .px(t.space.lg);
            for (i, example) in section.examples.iter().enumerate() {
                let key = example_key(&section.key, i);
                let hovered = self.voice.hovered.as_deref() == Some(key.as_str());
                let hover_key = key.clone();
                let remove = self.remove_slot(
                    &key,
                    hovered,
                    GuideEdit::RemoveExample {
                        section: section.key.clone(),
                        index: i,
                    },
                    t,
                    cx,
                );
                list = list.child(
                    div()
                        .id(SharedString::from(format!("voice-example-{key}")))
                        .flex()
                        .items_start()
                        .gap(t.space.md)
                        .py(v.example_pad_y)
                        .px(t.space.md)
                        .rounded(t.radius.md)
                        .bg(if hovered {
                            c.surface_selected
                        } else {
                            c.surface_card
                        })
                        .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                            set_hover(this, &hover_key, *on, cx)
                        }))
                        .child(
                            selectable(
                                SharedString::from(format!("voice-example-text-{key}")),
                                example.clone(),
                                t.text.ui,
                                c.text_primary,
                            )
                            .flex_1()
                            .min_w_0(),
                        )
                        .child(remove),
                );
            }
            card = card.child(list);
        }
        if section.kind == GuideSectionKind::Hard {
            card = card.child(self.add_rule_row(t, window, cx));
        }
        card
    }

    /// "Add a rule" at the end of the hard rules.
    fn add_rule_row(&mut self, t: &Tokens, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        if self.voice.adding_rule {
            let focused = self
                .voice_inputs
                .rule
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
            return div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .pt(t.space.sm)
                .pb(t.space.md)
                .px(t.space.lg)
                .child(
                    div()
                        .flex_1()
                        .child(input_box(&self.voice_inputs.rule, focused, t)),
                )
                .child(
                    button("voice-rule-save", "Add rule", ButtonKind::Primary, t).on_click(
                        cx.listener(|this, _, window, cx| this.add_voice_rule(window, cx)),
                    ),
                )
                .child(
                    button("voice-rule-cancel", "Cancel", ButtonKind::Quiet, t).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.voice.adding_rule = false;
                            cx.notify();
                        }),
                    ),
                );
        }
        div().pt(t.space.sm).pb(t.space.md).px(t.space.lg).child(
            focusable(div().id("voice-add-rule"), t)
                .flex()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .cursor_pointer()
                .child(icon("plus", t.icon.dense, c.text_secondary))
                .child(text("Add a rule", t.text.ui_strong, c.text_secondary))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.voice.adding_rule = true;
                    this.voice_inputs
                        .rule
                        .update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                })),
        )
    }

    /// The style guide: its caption, Edit as markdown, and the sections.
    fn voice_guide(
        &mut self,
        guide: &[GuideSectionView],
        caption: &str,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let has_note = self.state.voice.note.is_some();
        let mut d = div().flex().flex_col().gap(t.space.lg).child(
            div()
                .flex()
                .items_start()
                .gap(t.space.lg)
                .child(
                    section_head(
                        "Style guide",
                        if caption.is_empty() {
                            "Consort has no style guide yet. It shows the guide to you as an offer in Tutti when it finishes learning."
                        } else {
                            caption
                        },
                        t,
                    )
                    .flex_1(),
                )
                .when(has_note, |d| {
                    d.child(
                        icon_text_button("voice-edit-markdown", "edit", "Edit as markdown", t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_voice_dialog(VoiceDialog::Markdown, window, cx)
                            })),
                    )
                }),
        );
        for section in guide {
            d = d.child(self.guide_section(section, t, window, cx));
        }
        d
    }

    /// Try it: a request, Write a sample, and the sample.
    fn voice_try_it(&mut self, t: &Tokens, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let focused = self
            .voice_inputs
            .request
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let mut group = rows(t).child(
            div()
                .flex()
                .items_end()
                .gap(t.space.sm)
                .py(t.space.md)
                .px(t.space.lg)
                .border_b(t.border_width)
                .border_color(c.border_subtle)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .gap(t.space.xs)
                        .child(
                            text("Request", detail(t), c.text_secondary)
                                .font_weight(t.text.ui_strong.weight),
                        )
                        .child(input_box(&self.voice_inputs.request, focused, t)),
                )
                .child(needs_hub(
                    button("voice-try", "Write a sample", ButtonKind::Secondary, t)
                        .debug_selector(|| "voice-try".into())
                        .on_click(cx.listener(|this, _, _, cx| this.try_voice(cx))),
                    self.answers_blocked(),
                    t,
                )),
        );
        let sample = self.state.voice.sample.clone();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .py(t.space.md)
            .px(t.space.lg);
        match sample {
            None => {
                body = body.child(text(
                    "No sample yet. Write a request, then Write a sample.",
                    detail(t),
                    c.text_tertiary,
                ));
            }
            Some(sample) => {
                body = body.child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(t.space.sm)
                        .child(
                            text("Sample draft", detail(t), c.text_secondary)
                                .font_weight(t.text.ui_strong.weight),
                        )
                        .when(sample.state == VoiceSampleState::Ready, |d| {
                            d.child(text(sample.voice_line.clone(), detail(t), c.text_tertiary))
                        }),
                );
                body = match (sample.state, sample.draft.clone()) {
                    (VoiceSampleState::Ready, Some(draft)) => body.child(
                        div()
                            .py(t.component.voice.example_pad_y)
                            .px(t.space.md)
                            .rounded(t.radius.md)
                            .bg(c.surface_card)
                            .child(selectable("voice-sample", draft, t.text.ui, c.text_primary)),
                    ),
                    (VoiceSampleState::Failed, _) => body.child(text(
                        "The sample did not come back. Write a sample again.",
                        t.text.ui,
                        c.status_failed,
                    )),
                    _ => body.child(text(
                        "Consort is writing the sample.",
                        t.text.ui,
                        c.text_secondary,
                    )),
                };
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.sm)
                        .child(
                            div()
                                .size(t.component.voice.sample_dot)
                                .flex_none()
                                .rounded(t.radius.full)
                                .bg(c.status_done),
                        )
                        .child(text(
                            "Nothing is sent. Consort does not save this sample.",
                            detail(t),
                            c.text_tertiary,
                        )),
                );
            }
        }
        group = group.child(body);
        div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section_head(
                "Try it",
                "Write a request. Consort shows how a draft would sound with this style guide.",
                t,
            ))
            .child(group)
    }

    /// Disclosure: three radio options.
    fn voice_disclosure(&mut self, current: Disclosure, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let options = [
            (
                Disclosure::None,
                "Add nothing",
                "The draft has only your words.",
            ),
            (
                Disclosure::Footer,
                "Add a line: Drafted with Consort",
                "The line goes at the end of every draft.",
            ),
            (
                Disclosure::FooterNewContacts,
                "Add the line only for new contacts",
                "A new contact is a person you have not written to before.",
            ),
        ];
        let mut group = rows(t);
        for (i, (value, title, line)) in options.into_iter().enumerate() {
            group = group.child(
                focusable(
                    div().id(SharedString::from(format!("voice-disclosure-{i}"))),
                    t,
                )
                .flex()
                .items_start()
                .gap(t.space.md)
                .py(t.space.md)
                .px(t.space.lg)
                .cursor_pointer()
                .when(i < 2, |d| {
                    d.border_b(t.border_width).border_color(c.border_subtle)
                })
                .child(radio(value == current, t))
                .child(titled(title, vec![(line.to_owned(), c.text_tertiary)], t))
                .on_click(cx.listener(move |this, _, _, cx| this.set_voice_disclosure(value, cx))),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section_head(
                "Disclosure",
                "Say that Consort helped write a message. Consort adds the line to the draft text, so you see it on the approval card.",
                t,
            ))
            .child(group)
    }

    // The draft card -------------------------------------------------------

    /// The fields, the voice line, the flags, the tick and the buttons of a
    /// draft card, or its fields and Save changes in Edit.
    pub fn render_draft_card(
        &mut self,
        approval: &ApprovalView,
        card: &ConnectorCardView,
        draft: &DraftCardView,
        key: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let v = t.component.voice;
        let pending = approval.state == RequestState::Pending;
        let editing = pending
            && self
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
        // The fields.
        let mut fields = div()
            .flex()
            .flex_col()
            .rounded(t.radius.md)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .bg(c.surface_raised);
        let count = card.fields.len();
        for (i, field) in card.fields.iter().enumerate() {
            let label = text(field.label.clone(), detail(t), c.text_tertiary)
                .line_height(t.text.ui.line)
                .w(v.field_label)
                .flex_none();
            let is_subject = subject_key.is_some() && field.label == "Subject";
            let is_body = body_field.as_ref().is_some_and(|b| b.label == field.label);
            let value: AnyElement = if editing && is_subject {
                let focused = false;
                div()
                    .flex_1()
                    .child(
                        input_box(&self.voice_inputs.subject, focused, t)
                            .bg(c.surface_canvas)
                            .min_h(v.subject_box),
                    )
                    .into_any_element()
            } else if editing && is_body {
                div()
                    .flex_1()
                    .min_h(v.body_box)
                    .py(t.space.sm)
                    .px(t.space.sm + t.space.xxs)
                    .rounded(t.radius.md)
                    .bg(c.surface_canvas)
                    .border(t.border_width)
                    .border_color(c.accent_text)
                    .child(textarea(&self.voice_inputs.body, t.text.ui))
                    .into_any_element()
            } else {
                flagged_text(
                    format!("{key}-draft-field-{i}"),
                    &field.value,
                    &draft.ranges(&field.label),
                    t,
                )
                .flex_1()
                .min_w_0()
                .into_any_element()
            };
            fields = fields.child(
                div()
                    .flex()
                    .items_start()
                    .gap(t.space.sm)
                    .py(t.space.sm)
                    .px(t.space.md)
                    .when(i + 1 < count, |d| {
                        d.border_b(t.border_width).border_color(c.border_subtle)
                    })
                    .child(label)
                    .child(value),
            );
        }
        let voice_line = div()
            .flex()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .child(icon("quote", v.line_icon, c.text_tertiary))
            .child(text(draft.voice_line.clone(), detail(t), c.text_tertiary));
        let mut out = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(fields)
            .child(voice_line);
        if editing {
            let id = approval.id.clone();
            let body_key = body_field.map(|b| b.key);
            return out
                .child(text(
                    "Consort checks the text again when you save. It marks any date, price or agreement that is not in your request.",
                    detail(t),
                    c.text_tertiary,
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.sm)
                        .child(
                            button(
                                SharedString::from(format!("{key}-save")),
                                "Save changes",
                                ButtonKind::Primary,
                                t,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.save_draft_edit(
                                    id.clone(),
                                    subject_key.clone(),
                                    body_key.clone(),
                                    cx,
                                )
                            })),
                        )
                        .child(
                            button(
                                SharedString::from(format!("{key}-cancel")),
                                "Cancel",
                                ButtonKind::Secondary,
                                t,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_draft_edit(cx))),
                        ),
                );
        }
        // The flags.
        if pending && !draft.flags.is_empty() {
            let mut list = div().flex().flex_col().gap(t.space.sm);
            for flag in &draft.flags {
                list = list.child(
                    div()
                        .flex()
                        .gap(t.space.sm + t.space.xxs)
                        .child(
                            div()
                                .w(v.flag_bar)
                                .flex_none()
                                .rounded(t.radius.full)
                                .bg(c.accent_text),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .gap(t.space.xs + t.space.xxs)
                                        .child(text(
                                            flag.text.clone(),
                                            t.text.ui_strong,
                                            c.text_primary,
                                        ))
                                        .child(text(
                                            flag.kind_label.clone(),
                                            detail(t),
                                            c.text_tertiary,
                                        )),
                                )
                                .child(text(flag.note.clone(), detail(t), c.accent_text)),
                        ),
                );
            }
            let hash = draft.review_hash.clone();
            let checked = self.draft_checked(&approval.id, &hash);
            let id = approval.id.clone();
            out = out.child(list).child(
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
                    .child(text(
                        "I checked the marked lines",
                        t.text.ui,
                        c.text_primary,
                    )),
            );
        }
        if !pending {
            return out.child(self.card_outcome_or_buttons(
                approval,
                &consort_client::HubCardView::Connector(card.clone()),
                key,
                t,
                cx,
            ));
        }
        // The buttons: Allow stays off until the tick, Edit, Don't, and
        // "Not how I write" at the right.
        let checked = self.draft_checked(&approval.id, &draft.review_hash);
        let blocked = draft.allow_blocked(checked);
        let checked_hash = checked.then(|| draft.review_hash.clone());
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
        let edit = {
            let id = approval.id.clone();
            let card_key = key.to_owned();
            let subject = draft
                .editable
                .iter()
                .find(|f| f.key == "subject")
                .map(|f| f.value.clone());
            let body = draft
                .editable
                .iter()
                .find(|f| f.key != "subject")
                .map(|f| f.value.clone());
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
                    subject.clone(),
                    body.clone(),
                    window,
                    cx,
                )
            }))
        };
        let deny = {
            let id = approval.id.clone();
            needs_hub(
                button(
                    SharedString::from(format!("{key}-deny")),
                    approval.choice_label(ApprovalChoice::Deny),
                    ButtonKind::Quiet,
                    t,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.answer_approval(id.clone(), ApprovalChoice::Deny, window, cx)
                })),
                offline,
                t,
            )
        };
        let noted = draft.not_my_voice || self.voice.feedback.contains(&approval.id);
        let feedback: AnyElement = if noted {
            text(
                "Noted. Consort asks what is wrong.",
                detail(t),
                c.text_tertiary,
            )
            .into_any_element()
        } else {
            let id = approval.id.clone();
            focusable(
                div().id(SharedString::from(format!("{key}-not-my-voice"))),
                t,
            )
            .debug_selector(|| "not-my-voice".into())
            .cursor_pointer()
            .type_style(detail(t))
            .text_color(c.text_tertiary)
            .hover(|s| s.text_color(c.text_secondary))
            .child("Not how I write")
            .on_click(cx.listener(move |this, _, _, cx| this.not_my_voice(id.clone(), cx)))
            .into_any_element()
        };
        out.child(
            div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .child(allow)
                .child(edit)
                .child(deny)
                .child(div().flex_1())
                .when(draft.in_voice || !draft.flags.is_empty(), |d| {
                    d.child(feedback)
                }),
        )
    }

    // The voice task card and the offer --------------------------------------

    /// The finished steps of the voice task's card.
    pub fn render_task_steps(&self, key: &str, steps: &[String], t: &Tokens) -> Div {
        let c = &t.colors;
        let mut list = div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .py(t.space.sm + t.space.xxs)
            .px(t.space.md)
            .rounded(t.radius.md)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_subtle);
        for (i, step) in steps.iter().enumerate() {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm + t.space.xxs)
                    .child(icon("check", t.icon.dense, c.status_done))
                    .child(selectable(
                        SharedString::from(format!("{key}-step-{i}")),
                        step.clone(),
                        t.text.ui,
                        c.text_primary,
                    )),
            );
        }
        list
    }

    /// "Open the full guide" under the offer "Save this style guide?".
    pub fn render_open_guide_link(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Stateful<Div> {
        let c = &t.colors;
        focusable(div().id("voice-open-guide"), t)
            .debug_selector(|| "voice-open-guide".into())
            .flex()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .cursor_pointer()
            .child(icon("external-link", t.icon.dense, c.text_secondary))
            .child(
                div()
                    .type_style(t.text.ui)
                    .text_color(c.text_primary)
                    .underline()
                    .child("Open the full guide"),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_voice_dialog(VoiceDialog::Proposal, window, cx)
            }))
    }

    // Dialogs ------------------------------------------------------------------

    /// The parts of a voice dialog.
    pub fn voice_dialog_parts(
        &mut self,
        dialog: &VoiceDialog,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DialogParts {
        let c = &t.colors;
        match dialog {
            VoiceDialog::ConfirmReset => (
                "Reset the voice?".to_owned(),
                "Consort deletes the style guide and turns the voice off. Drafts sound neutral again. The old version stays in Recently deleted for 30 days, so you can restore it in Settings > Memory.".to_owned(),
                t.layout.dialog_width,
                div(),
                self.dialog_actions_kind(
                    "Reset the voice",
                    ButtonKind::Danger,
                    t,
                    cx,
                    |this, window, cx| {
                        let client = this.client.clone();
                        this.submit_dialog(
                            async move { client.run(consort_client::Command::ResetVoice).await },
                            window,
                            cx,
                        );
                    },
                ),
                None,
            ),
            VoiceDialog::Proposal => {
                let body = match self.state.voice.proposal.clone() {
                    Some(text_) => div()
                        .p(t.space.md)
                        .rounded(t.radius.md)
                        .bg(c.surface_card)
                        .border(t.border_width)
                        .border_color(c.border_subtle)
                        .font_family(t.font_mono.clone())
                        .child(selectable("voice-proposal", text_, t.text.mono, c.text_primary)),
                    None => text(
                        "No style guide waits now. It may be saved or dropped already.",
                        t.text.ui,
                        c.text_secondary,
                    ),
                };
                (
                    "The style guide that waits".to_owned(),
                    "Consort saves it as voice.md only after you say Do it on the offer in Tutti."
                        .to_owned(),
                    t.component.convo.dialog_settings,
                    body,
                    self.dialog_actions("Close", t, cx, |this, window, cx| {
                        this.close_dialog(window, cx)
                    }),
                    None,
                )
            }
            VoiceDialog::Markdown => (
                "Edit the style guide".to_owned(),
                "Keep the voice.md format. A rule that you add or change gets the mark (you), and a learned change never replaces it.".to_owned(),
                t.component.convo.dialog_settings,
                div()
                    .p(t.space.md)
                    .rounded(t.radius.md)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .font_family(t.font_mono.clone())
                    .child(textarea(&self.voice_inputs.markdown, t.text.mono)),
                self.dialog_actions("Save", t, cx, |this, window, cx| {
                    let content = this.voice_inputs.markdown.read(cx).value().to_string();
                    let client = this.client.clone();
                    this.submit_dialog(
                        async move {
                            client
                                .run(consort_client::Command::EditVoiceGuide(
                                    GuideEdit::Markdown { content },
                                ))
                                .await
                        },
                        window,
                        cx,
                    );
                }),
                None,
            ),
            VoiceDialog::AddDocument => {
                let focused = self
                    .voice_inputs
                    .document
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
                (
                    "Add a document".to_owned(),
                    "Give a channel file as <channel>/<path>, or a path on a worker. Consort reads it the next time it learns.".to_owned(),
                    t.layout.dialog_width,
                    field_block(
                        "Path",
                        input_box(&self.voice_inputs.document, focused, t),
                        Some("Your own writing, such as posts or letters, teaches the voice best."),
                        t,
                    ),
                    self.dialog_actions("Add document", t, cx, |this, window, cx| {
                        let path = this.voice_inputs.document.read(cx).value().to_string();
                        if path.trim().is_empty() {
                            this.dialog_error = Some("Enter a path.".into());
                            cx.notify();
                            return;
                        }
                        this.add_voice_document(path, cx);
                        this.close_dialog(window, cx);
                    }),
                    None,
                )
            }
        }
    }
}

/// Keeps the hovered rule or example.
fn set_hover(app: &mut ConsortApp, key: &str, on: bool, cx: &mut Context<ConsortApp>) {
    if on {
        app.voice.hovered = Some(key.to_owned());
    } else if app.voice.hovered.as_deref() == Some(key) {
        app.voice.hovered = None;
    }
    cx.notify();
}
