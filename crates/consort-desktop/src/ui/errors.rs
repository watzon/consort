//! The in-chat error (Paper E1 and E2) and the message that the hub refused.
//!
//! A failed turn is a card in the assistant lane at the bubble width, on
//! `surface.card`, with a 1 px border of `status.failed` at 40 percent and
//! the tail corner at the bottom left. It has the `failed` icon, a title in
//! Body semibold, the text in Body `text.secondary` and the actions, with
//! Details at the right end of their row. Details opens the raw text of the
//! harness as selectable mono text. When the user tries again or sends
//! another message, the client marks the card quiet. A quiet card is one line
//! with the icon, the title and a chevron, and a click opens it, without
//! buttons, so old buttons never stay in the history.
//!
//! A message that the hub refused stays in the lane of the user, dimmed, with
//! the reason under it and "Put back in the message box". It has no toast.
//!
//! The open state is local to the app: one flag for each card and each place
//! (the key says where) in `errors_open`.

use consort_client::{ErrorAction, ErrorCardView, RefusedView, TryAgainTarget};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement as _, Styled, Window, base::SelectableText, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    ui::{
        menu::MenuAnchor,
        model_picker::PickerTarget,
        parts::{ButtonKind, button, detail, focusable, icon, line, text},
        timeline::is_open_click,
    },
};

/// Where a card is: the actions need the channel or the task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorPlace {
    /// The timeline of this channel.
    Channel(String),
    /// The thread of this task.
    Task(String),
}

/// The ID of the model chip of the composer that belongs to a place.
fn model_chip(place: &ErrorPlace) -> &'static str {
    match place {
        ErrorPlace::Channel(_) => "composer-model",
        ErrorPlace::Task(_) => "reply-model",
    }
}

impl ConsortApp {
    /// The card of a failed turn. `key` says where it is, so a card has its
    /// own open state in each place. `place` says which composer the actions
    /// use.
    pub fn render_error_card(
        &mut self,
        card: &ErrorCardView,
        key: &str,
        place: ErrorPlace,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let open = self.errors_open.contains(key);
        let details_key = format!("{key}-details");
        let details_open = self.errors_open.contains(&details_key);
        let selector = format!("error-{key}");

        // A quiet card is one line until the user opens it.
        if card.quiet && !open {
            let toggle = key.to_owned();
            return focusable(div().id(SharedString::from(format!("error-line-{key}"))), t)
                .debug_selector({
                    let id = format!("error-line-{key}");
                    move || id.clone()
                })
                .flex()
                .items_center()
                .gap(t.space.sm + t.space.xxs)
                .h(t.component.chat.footer_line)
                .cursor_pointer()
                .child(icon("failed", t.icon.size, c.status_failed))
                .child(text(card.summary.clone(), t.text.ui, c.text_secondary))
                .child(icon("chevron-right", t.icon.dense, c.text_tertiary))
                .on_click(cx.listener(move |this, event, _, cx| {
                    if is_open_click(event) {
                        this.errors_open.insert(toggle.clone());
                        cx.notify();
                    }
                }))
                .into_any_element();
        }

        let title = div()
            .type_style(t.text.body)
            .font_weight(t.text.ui_strong.weight)
            .text_color(c.text_primary)
            .child(SelectableText::new(
                SharedString::from(format!("{key}.title")),
                card.title.clone(),
            ));
        let explanation = div()
            .type_style(t.text.body)
            .text_color(c.text_secondary)
            .child(SelectableText::new(
                SharedString::from(format!("{key}.text")),
                card.explanation.clone(),
            ));
        let head = div()
            .flex()
            .items_start()
            .gap(t.space.sm + t.space.xxs)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(t.space.xl)
                    .child(icon("failed", t.icon.size, c.status_failed)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.xxs)
                    .child(title)
                    .child(explanation),
            );

        // The actions at the left, Details at the right end of their row.
        let has_details = !card.details.trim().is_empty();
        let mut row = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pl(t.icon.size + t.space.sm + t.space.xxs);
        for action in &card.actions {
            row = row.child(self.render_error_action(*action, card, key, &place, t, cx));
        }
        row = row.child(div().flex_1());
        if has_details {
            let toggle = details_key.clone();
            row = row.child(
                focusable(
                    div().id(SharedString::from(format!("error-details-{key}"))),
                    t,
                )
                .debug_selector({
                    let id = format!("error-details-{key}");
                    move || id.clone()
                })
                .flex()
                .flex_none()
                .items_center()
                .gap(t.space.xxs)
                .h(t.space.xxl - t.space.xxs)
                .pl(t.space.sm)
                .pr(t.space.xs)
                .rounded(t.radius.md)
                .cursor_pointer()
                .hover(|s| s.bg(c.hover_card))
                .child(text("Details", t.text.ui, c.text_secondary))
                .child(icon(
                    if details_open {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                    t.icon.dense,
                    c.text_secondary,
                ))
                .on_click(cx.listener(move |this, event, _, cx| {
                    if !is_open_click(event) {
                        return;
                    }
                    if !this.errors_open.remove(&toggle) {
                        this.errors_open.insert(toggle.clone());
                    }
                    cx.notify();
                })),
            );
        }
        let show_row = !card.actions.is_empty() || has_details;

        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .flex()
            .flex_col()
            .gap(t.space.md)
            .w_full()
            .min_w_0()
            .max_w(t.layout.bubble_max_width)
            .pt(t.component.chat.bubble_pad_x)
            .px(t.component.chat.bubble_pad_x)
            .pb(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.status_failed.opacity(t.opacity.error_border))
            .rounded(t.component.chat.bubble_radius)
            .rounded_bl(t.component.chat.bubble_tail)
            .child(head)
            .when(show_row, |d| d.child(row))
            .when(has_details && details_open, |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!("error-raw-{key}")))
                        .debug_selector({
                            let id = format!("error-raw-{key}");
                            move || id.clone()
                        })
                        .w_full()
                        .min_w_0()
                        .max_h(t.layout.error_text_max_height)
                        .overflow_y_scroll()
                        .py(t.space.sm + t.space.xxs)
                        .px(t.space.md)
                        .rounded(t.radius.md)
                        .bg(c.surface_canvas)
                        .font_family(t.font_mono.clone())
                        .type_style(t.text.mono)
                        .text_color(c.text_secondary)
                        .child(SelectableText::new(
                            SharedString::from(format!("{key}.raw")),
                            card.details.clone(),
                        )),
                )
            })
            .into_any_element()
    }

    /// One button of a card.
    fn render_error_action(
        &mut self,
        action: ErrorAction,
        card: &ErrorCardView,
        key: &str,
        place: &ErrorPlace,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("error-action-{key}-{action:?}");
        let card = card.clone();
        let place = place.clone();
        button(
            SharedString::from(id.clone()),
            action.label(),
            ButtonKind::Secondary,
            t,
        )
        .debug_selector(move || id.clone())
        .on_click(
            cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                this.run_error_action(action, &card, &place, event.position(), window, cx)
            }),
        )
        .into_any_element()
    }

    /// What a button of a card does.
    ///
    /// - Try again: `Client::try_again` with the target of the card.
    /// - Choose another model: the model picker of the composer of the
    ///   place, under its chip (or at the click when the chip is not there).
    /// - Sign in again: the sign-in of the account of the turn. When no
    ///   worker has an account with this name, the Workers page of Settings
    ///   shows the accounts, so the click always leads somewhere.
    pub fn run_error_action(
        &mut self,
        action: ErrorAction,
        card: &ErrorCardView,
        place: &ErrorPlace,
        at: gpui_kit::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            ErrorAction::TryAgain => {
                if let Some(target) = card.try_again.clone() {
                    self.retry_turn(target, cx);
                }
            }
            ErrorAction::ChooseAnotherModel => {
                let target = match place {
                    ErrorPlace::Channel(id) => PickerTarget::Assistant(id.clone()),
                    ErrorPlace::Task(id) => PickerTarget::Task(id.clone()),
                };
                let anchor = self
                    .triggers
                    .get(model_chip(place))
                    .map(MenuAnchor::Trigger)
                    .unwrap_or(MenuAnchor::Point(at));
                self.open_picker(target, anchor, window, cx);
            }
            ErrorAction::SignInAgain => match self.account_of_error(card, place) {
                Some((worker_id, account_id)) => {
                    self.open_sign_in(worker_id, account_id, window, cx)
                }
                None => self.open_settings(crate::app::SettingsSection::Workers, cx),
            },
        }
    }

    /// Sends the last message again, or retries the task. The card turns
    /// quiet when the client sees the new message or the new status.
    pub fn retry_turn(&mut self, target: TryAgainTarget, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(cx, async move { client.try_again(target).await });
    }

    /// The worker and the account of the turn that failed. The card names
    /// the account. The worker of the channel or of the task goes first, and
    /// any other worker that has an account with this name follows.
    fn account_of_error(
        &self,
        card: &ErrorCardView,
        place: &ErrorPlace,
    ) -> Option<(String, String)> {
        let name = card.account.as_deref()?;
        let preferred = match place {
            ErrorPlace::Channel(id) => self
                .state
                .channel(id)
                .and_then(|ch| ch.coordinator_worker.clone()),
            ErrorPlace::Task(id) => self
                .state
                .thread(id)
                .and_then(|th| th.placement.as_ref().map(|p| p.worker_name.clone())),
        };
        let found = |worker: &consort_client::WorkerView| {
            worker
                .accounts
                .iter()
                .find(|a| a.label == name)
                .map(|a| (worker.id.clone(), a.id.clone()))
        };
        self.state
            .workers
            .iter()
            .filter(|w| Some(&w.name) == preferred.as_ref())
            .find_map(found)
            .or_else(|| self.state.workers.iter().find_map(found))
    }

    /// The reason and the action under a message that the hub refused. The
    /// reason is selectable, because it is an error detail. The action puts
    /// the text back in the message box it came from.
    pub fn render_refused_note(
        &mut self,
        refused: &RefusedView,
        key: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let failure = refused.failure_id.clone();
        div()
            .id(SharedString::from(format!("refused-{key}")))
            .debug_selector({
                let id = format!("refused-{key}");
                move || id
            })
            .flex()
            .items_center()
            .justify_end()
            .flex_wrap()
            .gap(t.space.sm)
            .pt(t.space.xs)
            .child(icon("failed", t.icon.dense, c.status_failed))
            .child(div().min_w_0().child(crate::ui::parts::selectable(
                SharedString::from(format!("refused-reason-{key}")),
                format!("Not sent. {}", refused.reason),
                detail(t),
                c.text_secondary,
            )))
            .child(
                focusable(
                    div().id(SharedString::from(format!("refused-put-back-{key}"))),
                    t,
                )
                .debug_selector({
                    let id = format!("refused-put-back-{key}");
                    move || id
                })
                .flex_none()
                .cursor_pointer()
                .whitespace_nowrap()
                .child(
                    line(refused.action.clone(), detail(t), c.text_primary)
                        .font_weight(t.text.ui_strong.weight),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.restore_failed_text(&failure, window, cx)
                })),
            )
            .into_any_element()
    }
}
