//! What the app adds to the first conversation (Paper T3, T4 and T8):
//!
//! - The worker card. The app makes the pairing code, so the assistant never
//!   sees it. The card shows the install command with the reach of the hub.
//! - The line above the composer of a hub whose accounts all signed out.
//! - The one offer of the notification permission, at the close of the guide.

use consort_client::{
    AddWorkerCardView, ApprovalView, GuideState, HubCardView, LoadState, Os, RequestState,
    TimelineItem, ViewState,
};
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled, div,
};

use crate::{
    app::ConsortApp,
    notify_platform::{self, Permission},
    ui::{
        add_worker::{default_os, install_command, shell_name, system_card, time_left},
        hub_blocked::needs_hub,
        parts::{ButtonKind, button, error_box, status_glyph, text},
    },
};

/// The page where a user makes a Composio project key.
pub const COMPOSIO_KEY_PAGE: &str = "https://platform.composio.dev";

/// The title of a worker card.
pub fn worker_card_title(this_computer: bool, os: Os) -> &'static str {
    if !this_computer {
        return "Add a worker";
    }
    match os {
        Os::MacOs => "Add this Mac as a worker",
        Os::Windows => "Add this PC as a worker",
        Os::Linux => "Add this computer as a worker",
    }
}

/// The line under the title of a worker card.
pub fn worker_card_line(this_computer: bool, os: Os) -> String {
    let place = match os {
        Os::MacOs if this_computer => "Terminal",
        _ => shell_name(os),
    };
    if this_computer {
        format!(
            "Run this in {place}. It installs the worker for your user, and it does not need admin rights."
        )
    } else {
        format!(
            "Run this in {place} on the machine. It installs the worker for your user, and it does not need admin rights."
        )
    }
}

/// The line of the status row of a worker card. `left_ms` is the time that the
/// code has left. `None` means that the hub has made no code yet.
pub fn worker_card_status(expires_at: Option<i64>, now: i64) -> (String, bool) {
    match expires_at {
        None => ("Making a code".to_owned(), true),
        Some(end) if end <= now => (
            "The code expired. Make a new code to add the worker.".to_owned(),
            false,
        ),
        Some(end) => (
            format!(
                "Waiting for the worker to connect. The code works once, and it expires in {}.",
                time_left(end, now)
            ),
            true,
        ),
    }
}

/// The first pending hub card of a channel that `wanted` accepts.
pub fn pending_card(
    state: &ViewState,
    channel_id: &str,
    wanted: impl Fn(&HubCardView) -> bool,
) -> Option<ApprovalView> {
    let timeline = state.timeline(channel_id)?;
    timeline.items.iter().find_map(|item| match item {
        TimelineItem::Approval(card)
            if card.approval.state == RequestState::Pending
                && card.approval.card.as_deref().is_some_and(&wanted) =>
        {
            Some(card.approval.clone())
        }
        _ => None,
    })
}

/// Whether a connect card must ask for the Composio key: the hub answered, and
/// it has no key.
pub fn card_needs_key(state: &ViewState) -> bool {
    state.composio.load == LoadState::Loaded && !state.composio.key_present
}

/// Whether Tutti offers the system permission for notifications. It offers one
/// time, on this client, after the guided start closed, and only when the
/// system would ask.
pub fn shows_permission_offer(guide: GuideState, offered: bool, permission: &Permission) -> bool {
    guide == GuideState::Closed && !offered && *permission == Permission::NotAsked
}

impl ConsortApp {
    /// Follows the cards of the open channel. A worker card gets one code when
    /// it shows. A connect card needs to know whether the hub has a key.
    pub fn follow_first_run(&mut self, cx: &mut Context<Self>) {
        let channel = self.channel_id.clone();
        match pending_card(&self.state, &channel, |c| {
            matches!(c, HubCardView::AddWorker(_))
        }) {
            Some(card) if self.ext.worker_card.as_deref() != Some(card.id.as_str()) => {
                self.ext.worker_card = Some(card.id);
                self.make_worker_card_code(cx);
            }
            Some(_) => {}
            None => {
                self.ext.worker_card = None;
                self.ext.worker_card_error = None;
            }
        }
        let connecting = pending_card(&self.state, &channel, |c| {
            matches!(c, HubCardView::ConnectService(_))
        })
        .is_some();
        if connecting && self.state.composio.load == LoadState::NotLoaded && !self.ext.key_asked {
            self.ext.key_asked = true;
            let client = self.client.clone();
            self.spawn_command(cx, async move {
                client
                    .run(consort_client::Command::LoadComposioStatus)
                    .await
            });
        }
    }

    /// Makes a pairing code for the worker card. Only this client sees it.
    pub fn make_worker_card_code(&mut self, cx: &mut Context<Self>) {
        if !self.in_flight.begin("worker-card-code") {
            return;
        }
        self.ext.worker_card_error = None;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client.worker_pairing_code().await;
            let _ = this.update(cx, |this, cx| {
                this.in_flight.finish("worker-card-code");
                if let Err(err) = result {
                    this.ext.worker_card_error = Some(err.to_string());
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The body of a pending worker card.
    pub fn render_worker_card(
        &mut self,
        approval: &ApprovalView,
        card: &AddWorkerCardView,
        key: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let os = if card.this_computer {
            default_os()
        } else {
            self.add_worker_os
        };
        let pairing = self.state.worker_pairing.clone();
        let now = consort_client::format::now_ms();
        let expired = pairing.as_ref().is_some_and(|p| p.expires_at <= now);
        let (status, waiting) = worker_card_status(pairing.as_ref().map(|p| p.expires_at), now);
        let command = install_command(os, pairing.as_ref())
            .filter(|_| !expired)
            .map(|command| self.command_box(command, t, cx));
        let loopback = pairing
            .as_ref()
            .is_some_and(|p| p.loopback_only)
            .then(|| self.loopback_notice(consort_client::reach::LOOPBACK_WORKER_NOTICE, t, cx));
        let systems = (!card.this_computer).then(|| {
            let mut row = div().flex().flex_wrap().gap(t.space.xs);
            for choice in [Os::MacOs, Os::Windows, Os::Linux] {
                let name = system_card(choice).0;
                row = row.child(
                    button(
                        SharedString::from(format!("{key}-os-{name}")),
                        name,
                        if choice == os {
                            ButtonKind::Secondary
                        } else {
                            ButtonKind::Quiet
                        },
                        t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.add_worker_os = choice;
                        cx.notify();
                    })),
                );
            }
            row
        });
        let failed = self.ext.worker_card_error.clone();
        let glyph = waiting.then(|| {
            status_glyph(
                SharedString::from(format!("{key}-waiting")),
                consort_client::TaskStatus::Working,
                t.icon.dense,
                t,
                cx,
            )
        });
        let offline = self.answers_blocked();
        let id = approval.id.clone();
        let not_now = needs_hub(
            button(
                SharedString::from(format!("{key}-deny")),
                approval.choice_label(consort_client::ApprovalChoice::Deny),
                ButtonKind::Quiet,
                t,
            )
            .flex_none()
            .debug_selector(|| "worker-card-not-now".into())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.answer_approval(id.clone(), consort_client::ApprovalChoice::Deny, window, cx)
            })),
            offline,
            t,
        );
        let new_code = (expired || failed.is_some()).then(|| {
            button(
                SharedString::from(format!("{key}-new-code")),
                "New code",
                ButtonKind::Secondary,
                t,
            )
            .flex_none()
            .debug_selector(|| "worker-card-new-code".into())
            .on_click(cx.listener(|this, _, _, cx| this.make_worker_card_code(cx)))
        });
        div()
            .debug_selector(|| "worker-card".into())
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xxs)
                    .child(text(
                        worker_card_title(card.this_computer, os),
                        t.text.ui_strong,
                        c.text_primary,
                    ))
                    .child(
                        text(
                            worker_card_line(card.this_computer, os),
                            t.text.label,
                            c.text_secondary,
                        )
                        .font_weight(t.text.ui.weight),
                    ),
            )
            .children(systems)
            .children(loopback)
            .children(command)
            .children(
                failed.map(|err| error_box(SharedString::from(format!("{key}-error")), err, t)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .children(glyph)
                    .child(
                        text(status, t.text.label, c.text_secondary)
                            .font_weight(t.text.ui.weight)
                            .flex_1()
                            .min_w_0(),
                    )
                    .children(new_code)
                    .child(not_now),
            )
    }

    /// Whether Tutti is open with no signed-in account (T8).
    pub fn no_account_in_tutti(&self) -> bool {
        self.setup.gate == consort_client::setup::Gate::NoAccount
            && self
                .state
                .tutti()
                .is_some_and(|tutti| tutti.id == self.channel_id)
    }

    /// The line above the composer when the timeline has messages and no
    /// account is signed in. An empty timeline has the T8 state in its place.
    pub fn no_account_card(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.no_account_in_tutti() || self.chat_rows.is_empty() {
            return None;
        }
        let c = &t.colors;
        Some(
            div()
                .debug_selector(|| "no-account-card".into())
                .flex()
                .items_center()
                .gap(t.space.md)
                .w_full()
                .max_w(t.layout.timeline_max_width)
                .mb(t.space.sm)
                .p(t.space.md)
                .rounded(t.radius.lg)
                .bg(c.surface_card)
                .child(
                    text(
                        "The assistant cannot answer. No account is signed in.",
                        t.text.ui,
                        c.text_secondary,
                    )
                    .flex_1()
                    .min_w_0(),
                )
                .child(
                    button(
                        "no-account-card-sign-in",
                        "Sign in to a provider",
                        ButtonKind::Primary,
                        t,
                    )
                    .flex_none()
                    .on_click(cx.listener(|this, _, _, cx| this.open_provider_step(cx))),
                )
                .into_any_element(),
        )
    }

    /// The one offer of the system permission for notifications.
    pub fn permission_offer_card(
        &mut self,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tutti = self
            .state
            .tutti()
            .is_some_and(|tutti| tutti.id == self.channel_id);
        if !tutti
            || !shows_permission_offer(
                self.state.hub_setup.guide,
                self.admin.settings.permission_offered,
                &self.admin.permission,
            )
        {
            return None;
        }
        let c = &t.colors;
        Some(
            div()
                .debug_selector(|| "permission-offer".into())
                .flex()
                .items_center()
                .gap(t.space.md)
                .w_full()
                .max_w(t.layout.timeline_max_width)
                .mb(t.space.sm)
                .p(t.space.md)
                .rounded(t.radius.lg)
                .bg(c.surface_card)
                .child(
                    text(
                        "Show notifications on this computer? Consort tells you when a task needs you.",
                        t.text.ui,
                        c.text_secondary,
                    )
                    .flex_1()
                    .min_w_0(),
                )
                .child(
                    button("permission-offer-not-now", "Not now", ButtonKind::Quiet, t)
                        .flex_none()
                        .debug_selector(|| "permission-offer-not-now".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_notification_setting(|s| s.permission_offered = true, cx);
                        })),
                )
                .child(
                    button("permission-offer-turn-on", "Turn on", ButtonKind::Primary, t)
                        .flex_none()
                        .debug_selector(|| "permission-offer-turn-on".into())
                        .on_click(cx.listener(|this, _, _, cx| this.ask_notification_permission(cx))),
                )
                .into_any_element(),
        )
    }

    /// Turn on: the system asks. The offer does not come back, whatever the
    /// answer is. Settings > Notifications says what to do after a no.
    pub fn ask_notification_permission(&mut self, cx: &mut Context<Self>) {
        self.set_notification_setting(|s| s.permission_offered = true, cx);
        cx.spawn(async move |this, cx| {
            let answered = notify_platform::request_permission().await;
            let _ = this.update(cx, |this, cx| {
                if let Err(message) = answered {
                    this.show_error(message, cx);
                }
                this.check_permission(cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_names_this_computer_only_when_the_user_chose_it() {
        assert_eq!(
            worker_card_title(true, Os::MacOs),
            "Add this Mac as a worker"
        );
        assert_eq!(
            worker_card_title(true, Os::Windows),
            "Add this PC as a worker"
        );
        assert_eq!(worker_card_title(false, Os::MacOs), "Add a worker");
    }

    #[test]
    fn the_line_names_the_program_of_the_system() {
        assert_eq!(
            worker_card_line(true, Os::MacOs),
            "Run this in Terminal. It installs the worker for your user, and it does not need admin rights."
        );
        assert!(worker_card_line(true, Os::Windows).starts_with("Run this in PowerShell."));
        assert!(worker_card_line(false, Os::Linux).contains("a terminal on the machine"));
    }

    #[test]
    fn the_status_counts_down_and_says_when_the_code_expired() {
        assert_eq!(
            worker_card_status(None, 0),
            ("Making a code".to_owned(), true)
        );
        let (line, waiting) = worker_card_status(Some(582_000), 0);
        assert!(waiting);
        assert_eq!(
            line,
            "Waiting for the worker to connect. The code works once, and it expires in 9:42."
        );
        let (line, waiting) = worker_card_status(Some(10), 10);
        assert!(!waiting);
        assert!(line.starts_with("The code expired."));
    }

    #[test]
    fn the_permission_offer_shows_once_after_the_guide_closed() {
        let not_asked = Permission::NotAsked;
        assert!(shows_permission_offer(
            GuideState::Closed,
            false,
            &not_asked
        ));
        assert!(!shows_permission_offer(
            GuideState::Running,
            false,
            &not_asked
        ));
        assert!(!shows_permission_offer(
            GuideState::Stopped,
            false,
            &not_asked
        ));
        assert!(!shows_permission_offer(
            GuideState::Closed,
            true,
            &not_asked
        ));
        assert!(!shows_permission_offer(
            GuideState::Closed,
            false,
            &Permission::Allowed
        ));
        assert!(!shows_permission_offer(
            GuideState::Closed,
            false,
            &Permission::NoBundle
        ));
    }
}
