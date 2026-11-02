//! A hub on this computer: the card for a hub that stopped (Paper `TEB-0`),
//! Settings > Hub in W4 (`TEC-0`), and the restart with tasks in V2 (`TED-0`).
//! The install screens are in `setup.rs`.
//!
//! The logic is in `consort_client::local_hub`. This file never spawns anything
//! and never touches the network.

use consort_client::Command;
use consort_client::admin::{HubStatusView, LastBackupView};
use consort_client::local_hub::{LocalConfig, LocalHub, LocalHubView, SwapState, tasks_title};
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, ParentElement, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div,
};

use crate::app::{ConsortApp, Dialog};
use crate::ui::admin_parts::{hint, row, rows, status_dot};
use crate::ui::parts::{ButtonKind, button, detail, error_box, selectable, text};
use crate::ui::settings::switch;

/// The body of a dialog: title, description, width, body and actions.
pub type LocalDialogParts = (String, String, Pixels, Div, Div);

/// The local hub of the app, and what the app still has to do with it.
pub struct LocalUi {
    pub hub: LocalHub,
    /// The swap belongs to an update, so the app restarts after it.
    pub restart_after_swap: bool,
    /// The last swap state that the app acted on.
    pub seen_swap: SwapState,
}

impl LocalUi {
    /// Starts the service. The mock mode and a test have no hub on this computer.
    pub fn start(demo: bool) -> LocalUi {
        let hub = (!demo)
            .then(LocalConfig::desktop)
            .flatten()
            .and_then(|config| LocalHub::start(config).ok())
            .unwrap_or_else(|| LocalHub::fixed(LocalHubView::none()));
        LocalUi::with(hub)
    }

    pub fn with(hub: LocalHub) -> LocalUi {
        LocalUi {
            hub,
            restart_after_swap: false,
            seen_swap: SwapState::Idle,
        }
    }
}

impl ConsortApp {
    /// Acts on each change of the local hub.
    pub fn follow_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut rx = self.local.hub.subscribe();
        cx.spawn_in(window, async move |this, cx| {
            while rx.changed().await.is_ok() {
                if this
                    .update_in(cx, |this, window, cx| this.local_changed(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// The hub moved to another port, or a swap reached a new state.
    pub fn local_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.local.hub.view();
        self.follow_moved_hub(&view, cx);
        self.follow_setup(cx);
        if view.swap != self.local.seen_swap {
            self.local.seen_swap = view.swap.clone();
            match view.swap {
                SwapState::NeedsChoice { .. } => {
                    self.open_dialog(Dialog::RestartWithTasks, window, cx);
                }
                SwapState::Done if self.local.restart_after_swap => {
                    self.local.restart_after_swap = false;
                    self.relaunch(cx);
                }
                SwapState::Failed { message } => {
                    self.local.restart_after_swap = false;
                    crate::ui::update_notice::allow_restart();
                    self.show_error(message, cx);
                }
                _ => {}
            }
        }
        cx.notify();
    }

    /// The hub of this client took another port: its old one was in use.
    pub fn follow_moved_hub(&mut self, view: &LocalHubView, cx: &mut Context<Self>) {
        if let Some(to) = view.moved_address(&self.state.hub_url) {
            let client = self.client.clone();
            let command = Command::MoveHub {
                from: self.state.hub_url.clone(),
                to: to.to_owned(),
            };
            self.spawn_command(cx, async move { client.run(command).await });
        }
    }

    /// Starts the install and pairs this app. W1 calls this.
    pub fn install_local_hub(&mut self, cx: &mut Context<Self>) {
        let hub = self.local.hub.clone();
        let client = self.client.clone();
        // The view shows a failure, so this needs no notice.
        cx.spawn(async move |_, _| {
            let _ = hub.install_and_connect(client).await;
        })
        .detach();
        cx.notify();
    }

    /// The card above the composer when the hub on this computer stopped.
    pub fn local_stopped_card(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = self.local.hub.view();
        if !view.stopped_for(&self.state.hub_url, &self.state.connection) {
            return None;
        }
        Some(
            self.local_card(
                "local-stopped-card",
                "The hub on this computer has stopped",
                "Your assistant cannot answer until it starts. Your messages wait here.",
                t,
            )
            .max_w(t.layout.timeline_max_width)
            .mb(t.space.sm)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(t.space.sm)
                    .child(self.start_hub_button(t, cx))
                    .child(self.show_log_button(t, cx)),
            )
            .into_any_element(),
        )
    }

    fn local_card(&self, id: &'static str, title: &str, body: &str, t: &Tokens) -> Div {
        let c = &t.colors;
        div()
            .debug_selector(move || id.to_owned())
            .flex()
            .flex_col()
            .gap(t.space.md)
            .w_full()
            .p(t.space.lg)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .rounded(t.radius.lg)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xxs)
                    .child(text(title.to_owned(), t.text.ui_strong, c.text_primary))
                    .child(text(body.to_owned(), t.text.ui, c.text_secondary)),
            )
    }

    fn start_hub_button(&self, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        button("local-start", "Start the hub", ButtonKind::Primary, t)
            .debug_selector(|| "local-start".into())
            .on_click(cx.listener(|this, _, _, cx| {
                let hub = this.local.hub.clone();
                this.spawn_command(cx, async move { hub.start_hub().await });
                cx.notify();
            }))
    }

    pub(crate) fn show_log_button(&self, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        button("local-log", "Show log", ButtonKind::Secondary, t)
            .debug_selector(|| "local-log".into())
            .on_click(cx.listener(|this, _, _, cx| {
                let hub = this.local.hub.clone();
                this.spawn_command(cx, async move { hub.show_log().await });
            }))
    }

    pub(crate) fn try_install_button(
        &self,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        button("local-try-again", "Try again", ButtonKind::Secondary, t)
            .debug_selector(|| "local-try-again".into())
            .on_click(cx.listener(|this, _, _, cx| this.install_local_hub(cx)))
    }

    /// Settings > Hub for a hub on this computer: the header and the five rows.
    pub fn local_hub_section(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let view = self.local.hub.view();
        let reach = &self.state.admin.reach;
        let reach_on = !reach.routes.is_empty();
        let reach_text = if reach_on {
            reach.summary.clone()
        } else {
            "Off. Only this computer can reach the hub. Turn this on to add a phone or a worker through your network or Tailscale.".to_owned()
        };
        let login = switch("local-login", view.start_at_login, t)
            .flex_none()
            .debug_selector(|| "local-login".into())
            .on_click(cx.listener(|this, _, _, cx| {
                let hub = this.local.hub.clone();
                let on = !hub.view().start_at_login;
                this.spawn_command(cx, async move { hub.set_start_at_login(on).await });
            }));
        let local_button = |id: &'static str, label: &'static str, kind: ButtonKind| {
            button(id, label, kind, t)
                .flex_none()
                .debug_selector(move || id.to_owned())
        };
        rows(t)
            .debug_selector(|| "local-rows".into())
            .child(local_row(
                "Start when I sign in",
                "The hub keeps running after you quit the app. While this computer sleeps, routines and check-ins wait.".to_owned(),
                login.into_any_element(),
                false,
                t,
            ))
            .child(local_row(
                "Reach from other devices",
                reach_text,
                local_button(
                    "local-reach",
                    if reach_on { "Manage" } else { "Turn on" },
                    ButtonKind::Secondary,
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_reach(cx)))
                .into_any_element(),
                false,
                t,
            ))
            .child(local_row(
                "Move to a server",
                "Makes a backup that you restore on the server, and then connects this app there. You sign in to your accounts again on the server.".to_owned(),
                local_button("local-move", "Start the move", ButtonKind::Secondary)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_dialog(Dialog::LocalMove, window, cx);
                        this.back_up_now(cx);
                    }))
                    .into_any_element(),
                false,
                t,
            ))
            .child(local_row(
                "Connect to another hub",
                "This app leaves this hub. The hub stays on this computer and keeps your data.".to_owned(),
                local_button("local-connect-other", "Connect", ButtonKind::Secondary)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_dialog(Dialog::LocalConnectOther, window, cx);
                    }))
                    .into_any_element(),
                false,
                t,
            ))
            .child(local_row(
                "Remove from this computer",
                "Stops the hub and removes its service. The app asks whether to keep your data.".to_owned(),
                local_button("local-remove", "Remove", ButtonKind::Danger)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_dialog(Dialog::LocalRemove, window, cx);
                    }))
                    .into_any_element(),
                true,
                t,
            ))
    }

    /// The header line of Settings > Hub for a hub on this computer.
    pub fn local_hub_line(hub: &HubStatusView, t: &Tokens) -> Div {
        div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .child(status_dot(Some(hub.online).filter(|o| *o), t))
            .child(
                text(
                    format!(
                        "This hub runs on this computer. Version {}. It updates with the app.",
                        hub.hub_version
                    ),
                    t.text.ui,
                    t.colors.text_secondary,
                )
                .flex_1()
                .min_w_0(),
            )
    }

    /// The dialogs of a hub on this computer.
    pub fn local_dialog(
        &mut self,
        dialog: &Dialog,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> LocalDialogParts {
        match dialog {
            Dialog::RestartWithTasks => self.restart_tasks_dialog(t, cx),
            Dialog::LocalRemove => self.remove_dialog(t, cx),
            Dialog::LocalMove => self.move_dialog(t, cx),
            _ => self.connect_other_dialog(t, cx),
        }
    }

    fn restart_tasks_dialog(&mut self, t: &Tokens, cx: &mut Context<Self>) -> LocalDialogParts {
        let working = match self.local.hub.view().swap {
            SwapState::NeedsChoice { working } | SwapState::Waiting { working } => working,
            _ => 0,
        };
        let choose = |this: &mut ConsortApp,
                      wait: bool,
                      window: &mut Window,
                      cx: &mut Context<ConsortApp>| {
            let hub = this.local.hub.clone();
            this.submit_dialog(async move { hub.choose_swap(wait).await }, window, cx);
        };
        let actions = two_buttons(
            button("restart-now", "Restart now", ButtonKind::Secondary, t)
                .debug_selector(|| "restart-now".into())
                .on_click(cx.listener(move |this, _, window, cx| choose(this, false, window, cx))),
            button(
                "restart-when-done",
                "Restart when they finish",
                ButtonKind::Primary,
                t,
            )
            .debug_selector(|| "restart-when-done".into())
            .on_click(cx.listener(move |this, _, window, cx| choose(this, true, window, cx))),
            t,
        );
        (
            tasks_title(working),
            "The update restarts the hub and its agents. The tasks stop, and you can retry them. Consort makes a backup first.".to_owned(),
            t.layout.dialog_width,
            div(),
            actions,
        )
    }

    fn remove_dialog(&mut self, t: &Tokens, cx: &mut Context<Self>) -> LocalDialogParts {
        let remove = |this: &mut ConsortApp,
                      keep_data: bool,
                      window: &mut Window,
                      cx: &mut Context<ConsortApp>| {
            let hub = this.local.hub.clone();
            let client = this.client.clone();
            this.submit_dialog(
                async move {
                    hub.remove(keep_data).await?;
                    client.unpair().await
                },
                window,
                cx,
            );
            // The app has no hub after this, so it asks where to connect.
            this.leave_after_dialog(cx);
        };
        let actions = two_buttons(
            button(
                "local-remove-delete",
                "Delete the data",
                ButtonKind::Danger,
                t,
            )
            .debug_selector(|| "local-remove-delete".into())
            .on_click(cx.listener(move |this, _, window, cx| remove(this, false, window, cx))),
            button("local-remove-keep", "Keep the data", ButtonKind::Primary, t)
                .debug_selector(|| "local-remove-keep".into())
                .on_click(cx.listener(move |this, _, window, cx| remove(this, true, window, cx))),
            t,
        );
        let folder = self.local.hub.view().folder;
        (
            "Remove the hub from this computer?".to_owned(),
            "This stops the hub and removes its service. If you keep the data, you can install the hub again and continue. If you delete the data, your channels and your memory on this hub are gone.".to_owned(),
            t.layout.dialog_width,
            div().child(selectable(
                "local-remove-folder",
                folder,
                detail(t),
                t.colors.text_tertiary,
            )),
            actions,
        )
    }

    fn move_dialog(&mut self, t: &Tokens, cx: &mut Context<Self>) -> LocalDialogParts {
        let c = &t.colors;
        let backup = self.state.admin.hub.backup.clone();
        let status: Div = match &backup.last {
            LastBackupView::Done { file_name, .. } if !backup.busy => div()
                .debug_selector(|| "local-move-file".into())
                .flex()
                .flex_col()
                .gap(t.space.xxs)
                .child(text(
                    "The backup is ready.",
                    t.text.ui_strong,
                    c.text_primary,
                ))
                .child(div().font_family(t.font_mono.clone()).child(selectable(
                    "local-move-path",
                    format!("{}/{file_name}", backup.folder),
                    t.text.mono,
                    c.text_primary,
                ))),
            LastBackupView::Failed { .. } => error_box(
                "local-move-error",
                backup
                    .error
                    .clone()
                    .unwrap_or_else(|| "The backup did not finish. Try again.".to_owned()),
                t,
            ),
            _ => text("Making a backup.", t.text.ui, c.text_secondary),
        };
        let steps = [
            "Copy the backup file to the server.".to_owned(),
            "Run hub.sh on the server. It installs the hub.".to_owned(),
            format!("Run {} on the server.", consort_client::admin::RESTORE_COMMAND),
            "Connect this app to the hub on the server.".to_owned(),
            "Sign in to your accounts again on the server. The accounts do not move, because Consort never reads credential files.".to_owned(),
        ];
        let body = div().flex().flex_col().gap(t.space.md).child(status).child(
            div()
                .flex()
                .flex_col()
                .gap(t.space.xs)
                .children(steps.into_iter().enumerate().map(|(index, step)| {
                    div()
                        .flex()
                        .gap(t.space.sm)
                        .child(
                            text(format!("{}.", index + 1), t.text.ui, c.text_tertiary).flex_none(),
                        )
                        .child(
                            selectable(
                                SharedString::from(format!("local-move-step-{index}")),
                                step,
                                t.text.ui,
                                c.text_secondary,
                            )
                            .flex_1()
                            .min_w_0(),
                        )
                })),
        );
        (
            "Move to a server".to_owned(),
            "The hub stays on this computer until you remove it.".to_owned(),
            t.layout.dialog_width,
            body,
            self.dialog_actions("Connect to the server", t, cx, |this, window, cx| {
                this.leave_this_hub(window, cx);
            }),
        )
    }

    fn connect_other_dialog(&mut self, t: &Tokens, cx: &mut Context<Self>) -> LocalDialogParts {
        (
            "Connect to another hub?".to_owned(),
            "This app leaves this hub. The hub stays on this computer and keeps your data."
                .to_owned(),
            t.layout.dialog_width,
            div(),
            self.dialog_actions("Connect to another hub", t, cx, |this, window, cx| {
                this.leave_this_hub(window, cx);
            }),
        )
    }

    /// Forgets the pairing and goes to the first screen. The hub keeps running.
    fn leave_this_hub(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.submit_dialog(async move { client.unpair().await }, window, cx);
        self.leave_after_dialog(cx);
    }

    /// Goes to the first screen when the unpair of the dialog ends.
    fn leave_after_dialog(&mut self, cx: &mut Context<Self>) {
        self.leaving_hub = true;
        cx.notify();
    }
}

/// One row of W4: the name, what it does and its control.
fn local_row(title: &str, body: String, action: AnyElement, last: bool, t: &Tokens) -> Div {
    let c = &t.colors;
    row(last, t)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(t.space.xxs)
                .child(text(title.to_owned(), t.text.ui_strong, c.text_primary))
                .child(hint(body, t)),
        )
        .child(action)
}

/// The row of a dialog with two choices. Escape closes the dialog with no choice.
fn two_buttons(first: impl IntoElement, second: impl IntoElement, t: &Tokens) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_end()
        .gap(t.space.sm)
        .pt(t.space.lg)
        .border_t(t.border_width)
        .border_color(t.colors.border_subtle)
        .child(first)
        .child(second)
}
