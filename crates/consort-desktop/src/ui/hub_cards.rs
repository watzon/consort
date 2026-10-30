//! The cards that the hub makes itself, in the timeline: a connector
//! approval, a management approval, a connect card, a sign-in card and an
//! install card. Each one is answered with `approval.answer`, with the two
//! options that the hub gave it.

use consort_client::{
    ApprovalChoice, ApprovalView, ConnectionStatusView, HubCardView, RequestState, TaskStatus,
    approvals_sentence, audit_summary, use_in_sentence,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    ui::{
        ext_parts::service_logo,
        hub_blocked::needs_hub,
        parts::{
            ButtonKind, button, detail, error_box, icon, provider_logo, selectable, status_glyph,
            text,
        },
    },
};

/// The glyph of a card: Needs input while it waits.
fn glyph_status(state: RequestState) -> TaskStatus {
    match state {
        RequestState::Pending | RequestState::Sending => TaskStatus::NeedsInput,
        RequestState::Answered => TaskStatus::Resolved,
        RequestState::Expired | RequestState::Closed => TaskStatus::UpNext,
    }
}

/// A long value, such as the body of an email, shows as a paragraph and not
/// in the label column.
fn is_paragraph(label: &str, value: &str) -> bool {
    label.eq_ignore_ascii_case("body")
        || label.eq_ignore_ascii_case("message")
        || value.contains('\n')
        || value.chars().count() > 90
}

impl ConsortApp {
    /// A hub card. `chat` gives it the corners of a card in the chat:
    /// `bubble_radius`, with the small corner at the top left under its task
    /// line. A hub card inside a task card keeps `lg` corners.
    #[allow(clippy::too_many_arguments)]
    pub fn render_hub_card(
        &mut self,
        approval: &ApprovalView,
        card: &HubCardView,
        key: &str,
        chat: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_hub_card_with(approval, card, key, Some(chat), t, cx)
    }

    /// The body of a hub card with no frame of its own, for the box of an
    /// answered line that the user opened.
    pub fn render_hub_card_body(
        &mut self,
        approval: &ApprovalView,
        card: &HubCardView,
        key: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_hub_card_with(approval, card, key, None, t, cx)
    }

    /// `chat` is `None` for a body with no frame.
    fn render_hub_card_with(
        &mut self,
        approval: &ApprovalView,
        card: &HubCardView,
        key: &str,
        chat: Option<bool>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let pending = approval.state == RequestState::Pending;
        let whole = card;
        let mut frame = div()
            .debug_selector({
                let id = format!("{key}-hub");
                move || id.clone()
            })
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(t.space.md);
        if let Some(chat) = chat {
            frame = crate::ui::parts::card_corners(
                frame
                    .p(t.space.lg)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_subtle),
                chat,
                t,
            );
        }
        let glyph = status_glyph(
            SharedString::from(format!("{key}-glyph")),
            glyph_status(approval.state),
            t.icon.size,
            t,
            cx,
        );
        let summary = selectable(
            SharedString::from(format!("{key}-summary")),
            approval.summary.clone(),
            t.text.ui_strong,
            c.text_primary,
        )
        .flex_1()
        .min_w_0();
        match card {
            HubCardView::Connector(card) => {
                frame = frame.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.sm)
                        .child(glyph)
                        .child(service_logo(&card.logo, t.icon.dense, t))
                        .child(summary)
                        .child(text(
                            card.rule_note.clone(),
                            t.text.caption,
                            c.text_tertiary,
                        )),
                );
                // A draft card is a card of its own (`ui/drafts.rs`).
                {
                    let mut fields = div()
                        .flex()
                        .flex_col()
                        .gap(t.space.xs)
                        .py(t.space.md - t.space.xxs)
                        .px(t.space.md)
                        .rounded(t.radius.md)
                        .border(t.border_width)
                        .border_color(c.border_subtle)
                        .bg(c.surface_canvas);
                    let mut paragraphs = Vec::new();
                    for (i, field) in card.fields.iter().enumerate() {
                        if is_paragraph(&field.label, &field.value) {
                            paragraphs.push((i, field.clone()));
                            continue;
                        }
                        fields = fields.child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(t.space.md)
                                .child(
                                    text(field.label.clone(), detail(t), c.text_tertiary)
                                        .w(t.layout.speaker_gutter)
                                        .flex_none(),
                                )
                                .child(
                                    selectable(
                                        SharedString::from(format!("{key}-field-{i}")),
                                        field.value.clone(),
                                        t.text.ui,
                                        c.text_primary,
                                    )
                                    .flex_1()
                                    .min_w_0(),
                                ),
                        );
                    }
                    for (i, field) in paragraphs {
                        fields = fields.child(div().pt(t.space.sm).child(selectable(
                            SharedString::from(format!("{key}-field-{i}")),
                            field.value.clone(),
                            t.text.ui,
                            c.text_secondary,
                        )));
                    }
                    if card.fields.is_empty() {
                        fields = fields.child(text(
                            "The call has no arguments to show.",
                            t.text.ui,
                            c.text_tertiary,
                        ));
                    }
                    frame = frame.child(fields);
                    let tool = card.tool.clone();
                    frame = frame.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .child(self.card_outcome_or_buttons(approval, whole, key, t, cx))
                            .child(div().flex_1())
                            .child(div().font_family(t.font_mono.clone()).child(selectable(
                                SharedString::from(format!("{key}-tool")),
                                tool,
                                t.text.caption,
                                c.text_tertiary,
                            ))),
                    );
                }
            }
            HubCardView::Management(card) => {
                frame = frame
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .child(glyph)
                            .child(summary),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(t.space.xs)
                            .child(selectable(
                                SharedString::from(format!("{key}-change")),
                                card.change.clone(),
                                t.text.ui,
                                c.text_primary,
                            ))
                            .child(selectable(
                                SharedString::from(format!("{key}-effect")),
                                card.effect.clone(),
                                t.text.ui,
                                c.text_secondary,
                            )),
                    )
                    .child(self.card_outcome_or_buttons(approval, whole, key, t, cx));
            }
            HubCardView::ConnectService(connect) => {
                let title = format!("Connect {} as {}", connect.service_name, connect.name);
                frame = frame.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(t.space.md)
                        .child(
                            div()
                                .h(t.text.ui.line)
                                .flex()
                                .items_center()
                                .child(service_logo(&connect.logo, t.component.provider_logo, t)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(selectable(
                                    SharedString::from(format!("{key}-summary")),
                                    title,
                                    t.text.ui_strong,
                                    c.text_primary,
                                ))
                                .child(text(
                                    format!(
                                        "{} {}",
                                        use_in_sentence(&connect.use_in, &self.state.channels)
                                            .replace("Used in", "Use in"),
                                        approvals_sentence(&connect.approvals)
                                    ),
                                    detail(t),
                                    c.text_secondary,
                                )),
                        ),
                );
                frame = frame.child(self.card_outcome_or_buttons(approval, whole, key, t, cx));
                if pending {
                    frame = frame.child(text(
                        format!(
                            "You sign in on {}'s own page. The assistant never sees your login.",
                            connect.service_name
                        ),
                        t.text.caption,
                        c.text_tertiary,
                    ));
                }
            }
            HubCardView::SignIn(sign_in) => {
                let worker = self
                    .state
                    .worker(&sign_in.worker_id)
                    .map(|w| w.name.clone())
                    .unwrap_or_else(|| "a worker".to_owned());
                frame = frame.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(t.space.md)
                        .child(
                            div()
                                .h(t.text.ui.line)
                                .flex()
                                .items_center()
                                .child(provider_logo(
                                    &sign_in.logo,
                                    t.component.provider_logo,
                                    c.text_primary,
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(text(
                                    format!("Sign in to {} on {worker}", sign_in.account_label),
                                    t.text.ui_strong,
                                    c.text_primary,
                                ))
                                .child(text(
                                    format!(
                                        "The sign-in for {} runs in the sign-in dialog, the same as in Settings > Workers. The assistant never sees your login.",
                                        sign_in.provider
                                    ),
                                    detail(t),
                                    c.text_secondary,
                                )),
                        ),
                );
                let waiting = self
                    .ext
                    .sign_in_card
                    .as_ref()
                    .is_some_and(|w| w.approval_id == approval.id);
                if pending && waiting {
                    frame = frame.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .child(status_glyph(
                                SharedString::from(format!("{key}-waiting")),
                                TaskStatus::Working,
                                t.icon.size,
                                t,
                                cx,
                            ))
                            .child(text(
                                "Waiting for the sign-in to finish.",
                                t.text.ui,
                                c.text_secondary,
                            )),
                    );
                }
                frame = frame.child(self.card_outcome_or_buttons(approval, whole, key, t, cx));
            }
            HubCardView::SkillInstall(install) => {
                let (_, audits) = audit_summary(&install.audits);
                let mut facts = vec![
                    (
                        "Source",
                        match &install.commit {
                            Some(commit) => format!("{} at {commit}", install.source),
                            None => install.source.clone(),
                        },
                    ),
                    (
                        "License",
                        install
                            .license
                            .clone()
                            .unwrap_or_else(|| "Not given".into()),
                    ),
                    (
                        "Scripts",
                        if install.scripts.is_empty() {
                            "None".to_owned()
                        } else {
                            format!(
                                "{}. It stays off until you turn it on.",
                                consort_client::join_and(&install.scripts)
                            )
                        },
                    ),
                    ("Audits", audits),
                ];
                facts.push((
                    "Use in",
                    use_in_sentence(&install.use_in, &self.state.channels)
                        .trim_start_matches("Used in ")
                        .trim_end_matches('.')
                        .to_owned(),
                ));
                let mut rows = div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .py(t.space.md - t.space.xxs)
                    .px(t.space.md)
                    .rounded(t.radius.md)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .bg(c.surface_canvas);
                for (i, (label, value)) in facts.into_iter().enumerate() {
                    rows = rows.child(
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
                                selectable(
                                    SharedString::from(format!("{key}-fact-{i}")),
                                    value,
                                    t.text.ui,
                                    c.text_primary,
                                )
                                .flex_1()
                                .min_w_0(),
                            ),
                    );
                }
                frame = frame
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .child(glyph)
                            .child(icon("box", t.icon.dense, c.text_secondary))
                            .child(
                                div()
                                    .type_style(t.text.ui_strong)
                                    .font_family(t.font_mono.clone())
                                    .text_color(c.text_primary)
                                    .child(install.name.clone()),
                            )
                            .child(
                                text(install.description.clone(), t.text.ui, c.text_secondary)
                                    .flex_1()
                                    .min_w_0()
                                    .truncate(),
                            ),
                    )
                    .child(rows)
                    .children(install.blocked_reason.clone().map(|reason| {
                        error_box(SharedString::from(format!("{key}-blocked")), reason, t)
                    }))
                    .child(self.card_outcome_or_buttons(approval, whole, key, t, cx));
            }
        }
        frame.into_any_element()
    }

    /// The buttons of a card that waits, or the line that says how it was
    /// answered.
    pub(crate) fn card_outcome_or_buttons(
        &mut self,
        approval: &ApprovalView,
        card: &HubCardView,
        key: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        match approval.state {
            RequestState::Pending | RequestState::Sending => {}
            RequestState::Expired => {
                return div().child(text(
                    "This request closed before anyone answered it.",
                    t.text.ui,
                    c.text_secondary,
                ));
            }
            RequestState::Closed => {
                return div().child(text("This request is closed.", t.text.ui, c.text_secondary));
            }
            RequestState::Answered => {
                let allowed = approval.answer != Some(ApprovalChoice::Deny);
                return div().child(self.card_outcome(card, allowed, key, t, cx));
            }
        }
        let id = approval.id.clone();
        let busy = self
            .in_flight
            .is_running(&format!("approval:{}", approval.id));
        let allow_label = approval.choice_label(ApprovalChoice::AllowOnce);
        let deny_label = approval.choice_label(ApprovalChoice::Deny);
        let mut row = div().flex().flex_wrap().items_center().gap(t.space.sm);
        let blocked = matches!(card, HubCardView::SkillInstall(i) if i.blocked_reason.is_some());
        let offline = self.answers_blocked();
        let allow = {
            let id = id.clone();
            let card = card.clone();
            needs_hub(
                button(
                    SharedString::from(format!("{key}-allow")),
                    allow_label,
                    ButtonKind::Primary,
                    t,
                )
                .when(busy, |b| b.opacity(t.opacity.busy))
                .when(matches!(card, HubCardView::ConnectService(_)), |b| {
                    b.child(icon("external-link", t.icon.dense, c.accent_on_fill))
                })
                .on_click(cx.listener(move |this, _, window, cx| match &card {
                    HubCardView::ConnectService(_) => this.continue_connect_card(id.clone(), cx),
                    HubCardView::SignIn(_) => this.sign_in_from_card(id.clone(), window, cx),
                    _ => this.answer_approval(id.clone(), ApprovalChoice::AllowOnce, window, cx),
                })),
                offline,
                t,
            )
        };
        if !blocked {
            row = row.child(allow);
        }
        if matches!(card, HubCardView::ConnectService(_)) {
            let id = id.clone();
            row = row.child(needs_hub(
                button(
                    SharedString::from(format!("{key}-change")),
                    "Change settings",
                    ButtonKind::Secondary,
                    t,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.change_connect_card(id.clone(), window, cx)
                })),
                offline,
                t,
            ));
        }
        row.child(needs_hub(
            button(
                SharedString::from(format!("{key}-deny")),
                deny_label,
                ButtonKind::Quiet,
                t,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.answer_approval(id.clone(), ApprovalChoice::Deny, window, cx)
            })),
            offline,
            t,
        ))
    }

    /// Whether the outcome of an answered card still moves: the browser sign-in
    /// of a connection that is connecting, and a sign-in that runs. Its
    /// buttons must stay in sight, so the card does not shrink to a line.
    pub(crate) fn hub_outcome_is_live(&self, approval: &ApprovalView, card: &HubCardView) -> bool {
        if approval.state != RequestState::Answered || approval.answer == Some(ApprovalChoice::Deny)
        {
            return false;
        }
        match card {
            HubCardView::ConnectService(connect) => connect
                .connection_id
                .as_ref()
                .and_then(|id| self.state.connection(id))
                .is_some_and(|connection| {
                    matches!(connection.status, ConnectionStatusView::Connecting { .. })
                }),
            HubCardView::SignIn(sign_in) => sign_in
                .sign_in_id
                .as_ref()
                .and_then(|id| self.state.sign_in(id))
                .is_some_and(|running| {
                    !matches!(
                        running.step,
                        consort_client::SignInStep::Succeeded { .. }
                            | consort_client::SignInStep::Failed { .. }
                            | consort_client::SignInStep::Cancelled
                            | consort_client::SignInStep::Expired
                    )
                }),
            _ => false,
        }
    }

    /// The line of an answered card.
    fn card_outcome(
        &mut self,
        card: &HubCardView,
        allowed: bool,
        key: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let line = |s: String| text(s, t.text.ui, c.text_secondary).into_any_element();
        match (card, allowed) {
            (HubCardView::Connector(_), true) => line("You allowed it. The call ran.".into()),
            (HubCardView::Connector(_), false) => line("You said no. The call did not run.".into()),
            (HubCardView::Management(_), true) => {
                line("You allowed it. The change is made.".into())
            }
            (HubCardView::Management(_), false) => line("You said no. Nothing changed.".into()),
            (HubCardView::ConnectService(card), true) => {
                let connection = card
                    .connection_id
                    .as_ref()
                    .and_then(|id| self.state.connection(id).cloned());
                match connection {
                    Some(connection) => match &connection.status {
                        ConnectionStatusView::Active => {
                            line(format!("{} is connected.", connection.name))
                        }
                        ConnectionStatusView::Connecting { redirect_url, .. } => {
                            let url = redirect_url.clone();
                            let id = connection.id.clone();
                            div()
                                .flex()
                                .items_center()
                                .gap(t.space.sm)
                                .child(status_glyph(
                                    SharedString::from(format!("{key}-connecting")),
                                    TaskStatus::Working,
                                    t.icon.size,
                                    t,
                                    cx,
                                ))
                                .child(text(
                                    "Waiting for you to finish the sign-in in your browser.",
                                    t.text.ui,
                                    c.text_secondary,
                                ))
                                .when_some(url, |d, url| {
                                    d.child(
                                        button(
                                            SharedString::from(format!("{key}-reopen")),
                                            "Open the page again",
                                            ButtonKind::Quiet,
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.ext.opened.retain(|o| o != &id);
                                                this.open_sign_in_page(&id, &url, cx);
                                            }),
                                        ),
                                    )
                                })
                                .into_any_element()
                        }
                        ConnectionStatusView::NeedsSignIn { .. }
                        | ConnectionStatusView::Failed { .. } => line(format!(
                            "The sign-in of {} did not finish. Reconnect it in Settings > Connectors.",
                            connection.name
                        )),
                    },
                    None => line("You continued. The hub is starting the sign-in.".into()),
                }
            }
            (HubCardView::ConnectService(_), false) => line("Not connected.".into()),
            (HubCardView::SignIn(card), true) => {
                let running = card
                    .sign_in_id
                    .as_ref()
                    .and_then(|id| self.state.sign_in(id).cloned());
                match running {
                    Some(sign_in) => match &sign_in.step {
                        consort_client::SignInStep::Succeeded { .. } => {
                            line(format!("{} is signed in.", card.account_label))
                        }
                        consort_client::SignInStep::Failed { message } => line(format!(
                            "The sign-in failed. {message} Ask the assistant to try again."
                        )),
                        consort_client::SignInStep::Cancelled
                        | consort_client::SignInStep::Expired => line(
                            "The sign-in stopped before it finished. Ask the assistant to try again."
                                .into(),
                        ),
                        _ => {
                            let (w, a, id) = (
                                card.worker_id.clone(),
                                card.account_id.clone(),
                                sign_in.id.clone(),
                            );
                            div()
                                .flex()
                                .items_center()
                                .gap(t.space.sm)
                                .child(status_glyph(
                                    SharedString::from(format!("{key}-signing-in")),
                                    TaskStatus::Working,
                                    t.icon.size,
                                    t,
                                    cx,
                                ))
                                .child(text(
                                    "The sign-in runs. Finish its steps in the sign-in dialog.",
                                    t.text.ui,
                                    c.text_secondary,
                                ))
                                .child(
                                    button(
                                        SharedString::from(format!("{key}-open-sign-in")),
                                        "Open the sign-in",
                                        ButtonKind::Quiet,
                                        t,
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_running_sign_in(
                                            w.clone(),
                                            a.clone(),
                                            id.clone(),
                                            window,
                                            cx,
                                        )
                                    })),
                                )
                                .into_any_element()
                        }
                    },
                    None if card.sign_in_id.is_some() => {
                        line("The sign-in started in another client, or it ended.".into())
                    }
                    None => line("You allowed it. The hub is starting the sign-in.".into()),
                }
            }
            (HubCardView::SignIn(_), false) => line("Not signed in.".into()),
            (HubCardView::SkillInstall(install), true) => line(if install.skill_id.is_some() {
                format!("Added {}.", install.name)
            } else {
                "You allowed it. The hub is adding the skill.".to_owned()
            }),
            (HubCardView::SkillInstall(_), false) => line("Not added.".into()),
        }
    }
}
