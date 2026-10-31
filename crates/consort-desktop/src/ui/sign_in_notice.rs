//! The sign-in notices in the sidebar, above the update notice: one for each
//! account whose login expired, with Sign in again. A notice stays until the
//! sign-in works, or until the user removes the account in Settings >
//! Providers. It is the same box as the update notice.

use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div,
};

use crate::{
    app::ConsortApp,
    ui::{
        parts::{icon, text},
        workers::row_button,
    },
};

impl ConsortApp {
    pub fn sign_in_notices(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = &t.colors;
        let n = t.component.update_notice;
        let expired: Vec<(String, String, String)> = self
            .state
            .expired_accounts()
            .map(|(worker, account)| (worker.id.clone(), account.id.clone(), account.label.clone()))
            .collect();
        expired
            .into_iter()
            .map(|(worker_id, account_id, label)| {
                let selector = format!("sign-in-notice-{account_id}");
                let button = format!("sign-in-again-{account_id}");
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector)
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(n.gap)
                    .py(n.pad_y)
                    .pl(n.pad_left)
                    .pr(n.pad_right)
                    .mb(n.margin_bottom)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .rounded(t.radius.lg)
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(n.gap)
                            .w_full()
                            .child(icon("circle-alert", t.icon.size, c.status_failed))
                            .child(
                                text(
                                    format!("{label} needs a new sign-in."),
                                    t.text.ui,
                                    c.text_secondary,
                                )
                                .flex_1()
                                .min_w_0(),
                            ),
                    )
                    .child(
                        row_button(button.clone(), "Sign in again".into(), t)
                            .debug_selector(move || button)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_sign_in(
                                    worker_id.clone(),
                                    account_id.clone(),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .into_any_element()
            })
            .collect()
    }
}
