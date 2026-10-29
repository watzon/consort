//! The Connectors and skills dialog of a channel, and the account settings
//! dialog with the provider extras toggle.

use consort_client::{ChannelItemView, channel_extensions};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Field},
    ui::{
        add_account::AccountDialogParts,
        dialogs::input_box,
        ext_parts::{hint, service_logo},
        menu::MenuAction,
        parts::{ButtonKind, button, detail, focusable, provider_logo, text},
        settings::switch,
    },
};

/// The site where a provider's extras are set up, for the account dialog.
fn extras_site(harness: &str, provider: &str) -> String {
    match harness {
        "claude" => "claude.ai".to_owned(),
        "codex" => "chatgpt.com".to_owned(),
        "gemini" => "gemini.google.com".to_owned(),
        _ => format!("the {provider} website"),
    }
}

/// The short name of a provider's account, such as Claude.
fn account_kind(harness: &str, provider: &str) -> String {
    match harness {
        "claude" => "Claude".to_owned(),
        "codex" => "ChatGPT".to_owned(),
        "gemini" => "Google".to_owned(),
        _ => provider.to_owned(),
    }
}

impl ConsortApp {
    /// One row of the channel dialog: the logo, the name, the line, and a
    /// switch for an item the scope gives, or Change for one it does not.
    fn channel_item_row(
        &mut self,
        channel_id: &str,
        item: &ChannelItemView,
        connector: bool,
        last: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let id = item.id.clone();
        let channel = channel_id.to_owned();
        let enabled = item.enabled;
        let control: AnyElement = if item.given {
            switch(&format!("channel-item-{}", item.id), enabled, t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_channel_item(channel.clone(), id.clone(), connector, !enabled, cx)
                }))
                .into_any_element()
        } else {
            button(
                SharedString::from(format!("channel-item-change-{}", item.id)),
                "Change",
                ButtonKind::Quiet,
                t,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.show_item_scope(id.clone(), connector, window, cx)
            }))
            .into_any_element()
        };
        let name_color = if item.given {
            c.text_primary
        } else {
            c.text_secondary
        };
        div()
            .flex()
            .items_center()
            .gap(t.space.md)
            .py(t.space.sm + t.space.xxs)
            .px(t.space.md + t.space.xxs)
            .when(!last, |d| {
                d.border_b(t.border_width).border_color(c.border_subtle)
            })
            .children(item.logo.as_ref().map(|logo| {
                div()
                    .when(!item.given, |d| d.opacity(t.opacity.answered_dim))
                    .child(service_logo(logo, t.component.provider_logo, t))
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(if connector {
                        text(item.name.clone(), t.text.ui_strong, name_color)
                    } else {
                        div()
                            .type_style(t.text.ui)
                            .font_family(t.font_mono.clone())
                            .text_color(name_color)
                            .child(item.name.clone())
                    })
                    .child(text(item.detail.clone(), detail(t), c.text_tertiary)),
            )
            .child(div().flex_none().child(control))
    }

    /// The Connectors and skills dialog of a channel.
    pub fn channel_extensions_dialog(
        &mut self,
        channel_id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let c = &t.colors;
        let Some(set) = channel_extensions(&self.state, channel_id) else {
            return (
                "This channel is gone".to_owned(),
                "It may have been deleted in another client. Close this dialog.".to_owned(),
                t.layout.dialog_width,
                div(),
                div(),
                None,
            );
        };
        let frame = || {
            div()
                .flex()
                .flex_col()
                .rounded(t.radius.lg)
                .border(t.border_width)
                .border_color(c.border_subtle)
                .bg(c.surface_card)
        };
        let mut connectors = frame();
        let n = set.connectors.len();
        for (i, item) in set.connectors.iter().enumerate() {
            connectors =
                connectors.child(self.channel_item_row(channel_id, item, true, i + 1 == n, t, cx));
        }
        let mut skills = frame();
        let n = set.skills.len();
        for (i, item) in set.skills.iter().enumerate() {
            skills =
                skills.child(self.channel_item_row(channel_id, item, false, i + 1 == n, t, cx));
        }
        let section = |title: &str, list: Div, empty: &str, has: bool| {
            div()
                .flex()
                .flex_col()
                .gap(t.space.sm)
                .child(text(title.to_owned(), t.text.label, c.text_secondary))
                .child(if has {
                    list
                } else {
                    div().child(hint(empty.to_owned(), t))
                })
        };
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.lg)
            .child(section(
                "Connectors",
                connectors,
                "No connectors yet. Add one in Settings > Connectors.",
                !set.connectors.is_empty(),
            ))
            .child(section(
                "Skills",
                skills,
                "No skills yet. Add one in Settings > Skills.",
                !set.skills.is_empty(),
            ));
        let actions = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                text(
                    format!("Changes apply to the next session in {}.", set.channel_name),
                    detail(t),
                    c.text_tertiary,
                )
                .flex_1(),
            )
            .child(
                button("dialog-confirm", "Done", ButtonKind::Primary, t)
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            );
        (
            format!("Connectors and skills in {}", set.channel_name),
            "These come from the \"Use in\" setting of each one. Turn one off to keep it out of this channel only.".to_owned(),
            t.layout.dialog_account,
            body,
            actions,
            None,
        )
    }

    /// The settings of one account: its name and the provider extras.
    pub fn account_settings_dialog(
        &mut self,
        worker_id: &str,
        account_id: &str,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let c = &t.colors;
        let worker = self.state.worker(worker_id).cloned();
        let account = worker.as_ref().and_then(|w| w.account(account_id)).cloned();
        let (Some(worker), Some(account)) = (worker, account) else {
            return (
                "This account is gone".to_owned(),
                "It may have been removed in another client. Close this dialog.".to_owned(),
                t.layout.dialog_wide,
                div(),
                div(),
                None,
            );
        };
        let focused = {
            use gpui_kit::Focusable as _;
            self.inputs
                .name
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        };
        let kind = account_kind(&account.harness, &account.provider);
        let site = extras_site(&account.harness, &account.provider);
        let on = account.extras_enabled;
        let (w, a) = (worker_id.to_owned(), account_id.to_owned());
        let toggle =
            switch("account-extras", on, t).on_click(cx.listener(move |this, _, _, cx| {
                let client = this.client.clone();
                let (worker_id, account_id) = (w.clone(), a.clone());
                this.spawn_command(cx, async move {
                    client
                        .run(consort_client::Command::SetAccountExtras {
                            worker_id,
                            account_id,
                            enabled: !on,
                        })
                        .await
                });
            }));
        let mut extras = div()
            .flex()
            .flex_col()
            .rounded(t.radius.lg)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .bg(c.surface_card)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(t.space.md)
                    .p(t.space.lg - t.space.xxs)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(t.space.xs)
                            .child(text(
                                format!("Use connectors, plugins and skills from this {kind} account"),
                                t.text.ui_strong,
                                c.text_primary,
                            ))
                            .child(text(
                                if on {
                                    format!("{kind} adds the ones set up on {site} to every session of this account. Consort does not see them, so its approvals and activity log do not cover them.")
                                } else {
                                    format!("Off. Sessions of this account start without the ones set up on {site}. They use only the connectors and skills of Consort.")
                                },
                                detail(t),
                                c.text_secondary,
                            )),
                    )
                    .child(div().flex_none().child(toggle)),
            );
        match &account.extras {
            Some(found) => {
                let list_open = self.ext.extras_list_open;
                let count = |n: u32, word: &str| {
                    div()
                        .flex()
                        .gap(t.space.xs)
                        .child(text(n.to_string(), t.text.ui_strong, c.text_primary))
                        .child(text(word.to_owned(), t.text.ui, c.text_secondary))
                };
                let mut panel = div()
                    .flex()
                    .flex_col()
                    .gap(t.space.sm)
                    .p(t.space.lg - t.space.xxs)
                    .border_t(t.border_width)
                    .border_color(c.border_subtle)
                    .bg(c.surface_sidebar)
                    .rounded_b(t.radius.lg)
                    .child(text(
                        format!("Synced from {site} at the last session"),
                        detail(t),
                        c.text_tertiary,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.lg)
                            .child(count(found.mcp_servers.count, "connectors"))
                            .child(count(found.plugins.count, "plugins"))
                            .child(count(found.skills.count, "skills"))
                            .child(div().flex_1())
                            .child(
                                focusable(div().id("extras-list-toggle"), t)
                                    .rounded(t.radius.sm)
                                    .cursor_pointer()
                                    .type_style(t.text.ui)
                                    .text_color(c.text_secondary)
                                    .hover(|s| s.text_color(c.text_primary))
                                    .child(if list_open {
                                        "Hide the list"
                                    } else {
                                        "Show the list"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.ext.extras_list_open = !this.ext.extras_list_open;
                                        cx.notify();
                                    })),
                            ),
                    );
                let names = found.names_line(if list_open { usize::MAX } else { 6 });
                if let Some(names) = names {
                    panel = panel.child(text(names, detail(t), c.text_secondary));
                }
                if !found.cannot_turn_off.is_empty() {
                    panel = panel.child(text(
                        format!(
                            "Consort cannot turn these off: {}.",
                            consort_client::join_and(&found.cannot_turn_off)
                        ),
                        detail(t),
                        c.text_tertiary,
                    ));
                }
                extras = extras.child(panel);
            }
            None => {
                extras = extras.child(
                    div()
                        .p(t.space.lg - t.space.xxs)
                        .border_t(t.border_width)
                        .border_color(c.border_subtle)
                        .child(text(
                            format!("{} has not reported what this account brings yet. It does after the next session.", worker.name),
                            detail(t),
                            c.text_tertiary,
                        )),
                );
            }
        }
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.lg)
            .child(crate::ui::dialogs::field_block(
                "Name",
                input_box(&self.inputs.name, focused, t),
                None,
                t,
            ))
            .child(extras);
        let (w, a) = (worker_id.to_owned(), account_id.to_owned());
        let (rw, ra) = (worker_id.to_owned(), account_id.to_owned());
        let (dw, da) = (worker_id.to_owned(), account_id.to_owned());
        let old_label = account.label.clone();
        let can_sign_out = account.auth.works();
        let done: crate::app::DialogConfirm = std::rc::Rc::new(move |this, window, cx| {
            let name = this.field(Field::Name, cx);
            if name.is_empty() {
                this.dialog_error = Some("Enter a name for the account.".into());
                cx.notify();
                return;
            }
            if name == old_label {
                this.close_dialog(window, cx);
                return;
            }
            let client = this.client.clone();
            let (w, a) = (dw.clone(), da.clone());
            this.submit_dialog(
                async move { client.rename_account(w, a, name).await },
                window,
                cx,
            );
        });
        self.dialog_confirm.replace(Some(done.clone()));
        let actions = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .when(can_sign_out, |d| {
                d.child(
                    button("account-sign-out", "Sign out", ButtonKind::Quiet, t).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.run_menu(
                                MenuAction::SignOutAccount(w.clone(), a.clone()),
                                window,
                                cx,
                            );
                            this.close_dialog(window, cx);
                        }),
                    ),
                )
            })
            .child(
                button("account-remove", "Remove account", ButtonKind::Danger, t).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.run_menu(
                            MenuAction::RemoveAccount(rw.clone(), ra.clone()),
                            window,
                            cx,
                        )
                    }),
                ),
            )
            .child(div().flex_1())
            .child(
                button("dialog-confirm", "Done", ButtonKind::Primary, t)
                    .on_click(cx.listener(move |this, _, window, cx| done(this, window, cx))),
            );
        let lead = div()
            .size(t.component.sign_in_tile)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(t.radius.lg)
            .border(t.border_width)
            .border_color(c.border_strong)
            .bg(c.surface_card)
            .child(provider_logo(
                &account.logo,
                t.component.sign_in_logo,
                c.text_primary,
            ))
            .into_any_element();
        (
            format!("{} on {}", account.label, worker.name),
            match &account.email {
                Some(email) => format!("Signed in as {email}. {}", account.detail),
                None => account.detail.clone(),
            },
            t.layout.dialog_wide,
            body,
            actions,
            Some(lead),
        )
    }
}
