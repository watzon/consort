//! The status line above the composer when the assistant cannot answer.
//!
//! It is live state of the channel, not a message: it shows while a message
//! of the user waits (or a turn cannot start), and it goes away when the hub
//! says that the assistant can answer. It has Choose another model, because
//! another model can often answer. When one account is signed out, it also
//! has Sign in again, with the same action as the sign-in notice in the
//! sidebar.

use consort_client::AssistantStatusView;
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, SettingsSection},
    ui::{
        menu::MenuAnchor,
        model_picker::PickerTarget,
        parts::{ButtonKind, button, detail, icon, selectable},
    },
};

impl ConsortApp {
    /// The line for the open channel, or `None` when the channel shows
    /// nothing.
    pub fn render_assistant_status(
        &mut self,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let channel = self.state.channel(&self.channel_id)?;
        let status = channel.assistant_status.clone()?;
        let worker = channel.coordinator_worker.clone();
        Some(self.assistant_status_line(&status, worker, t, cx))
    }

    fn assistant_status_line(
        &mut self,
        status: &AssistantStatusView,
        worker: Option<String>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let account = status.account.clone();
        let tint = if account.is_some() {
            c.status_failed
        } else {
            c.text_secondary
        };
        div()
            .id("assistant-status")
            .debug_selector(|| "assistant-status".to_owned())
            .flex()
            .items_center()
            .gap(t.space.sm)
            .w_full()
            .max_w(t.layout.timeline_max_width)
            .pb(t.space.xs)
            .child(
                // The icon stays with the first line when the text wraps.
                // The row centers this part on the buttons, which are taller
                // than one line.
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_start()
                    .gap(t.space.sm)
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(detail(t).line)
                            .child(icon("circle-alert", t.icon.dense, tint)),
                    )
                    .child(
                        // The reason is an error detail, so the user can
                        // select it and copy it.
                        selectable(
                            SharedString::from("assistant-status-text"),
                            status.text.clone(),
                            detail(t),
                            c.text_secondary,
                        )
                        .flex_1()
                        .min_w_0()
                        .debug_selector(|| "assistant-status-text".to_owned()),
                    ),
            )
            .when_some(account, |row, account| {
                row.child(
                    button(
                        "assistant-status-sign-in",
                        "Sign in again",
                        ButtonKind::Quiet,
                        t,
                    )
                    .flex_none()
                    .debug_selector(|| "assistant-status-sign-in".to_owned())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        match this.account_named(&account, worker.as_deref()) {
                            Some((worker_id, account_id)) => {
                                this.open_sign_in(worker_id, account_id, window, cx)
                            }
                            // No account has this name: the Providers page
                            // shows the accounts, so the click leads
                            // somewhere.
                            None => this.open_settings(SettingsSection::Providers, cx),
                        }
                    })),
                )
            })
            .child(
                button(
                    "assistant-status-model",
                    "Choose another model",
                    ButtonKind::Quiet,
                    t,
                )
                .flex_none()
                .debug_selector(|| "assistant-status-model".to_owned())
                .on_click(cx.listener(
                    move |this, event: &gpui_kit::ClickEvent, window, cx| {
                        let anchor = this
                            .triggers
                            .get("composer-model")
                            .map(MenuAnchor::Trigger)
                            .unwrap_or(MenuAnchor::Point(event.position()));
                        let target = PickerTarget::Assistant(this.channel_id.clone());
                        this.open_picker(target, anchor, window, cx);
                    },
                )),
            )
            .into_any_element()
    }
}
