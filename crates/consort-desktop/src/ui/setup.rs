//! The setup screens: W1, W2 and W3 (Paper `TE9-0`, `TEA-0`, `TEB-0`), S2, S3,
//! S4 and S5 (`S73-0`, `S8J-0`, `S7T-0`, `T75-0`). S1 is in `onboarding.rs`.
//!
//! The gate in `consort_client::setup` picks what opens. The sign-in is the
//! flow of the Add account dialog, in `add_account.rs`, on a screen.

use consort_client::Connection;
use consort_client::local_hub::{
    InstallPhase, InstallStep, LocalSetupStage, StepState, step_label,
};
use consort_client::setup::{
    Dabs, Gate, LocalPresence, ProviderChoice, ToolStage, default_tutti_selection, gate,
    provider_choices, provider_short_name, tool_of, tool_stage,
};
use consort_client::{ChannelChanges, NewAccount, SecretEnv, SignInStep, TaskStatus};
use consort_theme::{Tokens, Type, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled, Window, div, linear_color_stop,
    linear_gradient, prelude::FluentBuilder as _, relative,
};

use crate::app::{AccountStep, AddAccountForm, ConsortApp, Dialog, Page, ProviderPick};
use crate::ui::parts::{
    ButtonKind, button, detail, focusable, icon, provider_logo, selectable, status_glyph, text,
};
use crate::ui::window_controls::DragArea as _;

/// Where the user is inside what the gate opened. The gate holds the rest, so
/// this is not a saved step: it ends with the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SetupStep {
    /// What the gate says: W1 or S3.
    #[default]
    Start,
    /// S2: Start a hub on your server.
    Server,
    /// S4: the sign-in of the chosen provider.
    SignIn,
}

/// The state of the setup screens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetupUi {
    /// What the gate said last.
    pub gate: Gate,
    pub step: SetupStep,
    /// T8 opened S3. The gate alone would keep T8.
    pub open_provider: bool,
    /// Do this later is on its way to the hub.
    pub later_sent: bool,
    /// Tutti gets its default model when an account signs in.
    pub pending_model: bool,
}

impl SetupUi {
    /// The gate of the screen: T8 can open the provider step.
    fn screen_gate(&self) -> Gate {
        if self.open_provider && self.gate == Gate::NoAccount {
            Gate::Provider
        } else {
            self.gate
        }
    }
}

impl ConsortApp {
    /// The answer of the gate now.
    pub fn setup_gate(&self) -> Gate {
        let mut hub = self.state.hub_setup;
        hub.later |= self.setup.later_sent;
        gate(
            &self.state.connection,
            LocalPresence::from(&self.local.hub.view()),
            &hub,
        )
    }

    /// Follows the gate. The hub state and this computer pick the page, so a
    /// restart comes back to the same place. Mock Demo mode is not gated.
    pub fn follow_setup(&mut self, cx: &mut Context<Self>) {
        if self.demo {
            return;
        }
        let next = self.setup_gate();
        if next != self.setup.gate {
            if self.setup.step == SetupStep::SignIn && next != Gate::Provider {
                self.end_add_account(cx);
            }
            if next != Gate::Provider && next != Gate::NoAccount {
                self.setup.step = SetupStep::Start;
            }
            self.setup.gate = next;
            self.placeholder_stale = true;
        }
        if matches!(next, Gate::Main) {
            self.setup.open_provider = false;
            self.setup.step = SetupStep::Start;
        }
        if matches!(next, Gate::Provider | Gate::NoAccount) {
            self.setup.pending_model = true;
        }
        self.page = setup_page(next, self.page, self.setup.open_provider);
        self.set_tutti_model(cx);
        self.follow_first_run(cx);
    }

    /// Tutti gets a default model after the first sign-in. The user can change
    /// it in the composer chip.
    fn set_tutti_model(&mut self, cx: &mut Context<Self>) {
        if !self.setup.pending_model || !self.state.hub_setup.signed_in {
            return;
        }
        let Some(tutti) = self.state.tutti() else {
            return;
        };
        if tutti.coordinator_selection.is_some() {
            self.setup.pending_model = false;
            return;
        }
        let Some(selection) = default_tutti_selection(&self.state.model_groups) else {
            return;
        };
        self.setup.pending_model = false;
        let id = tutti.id.clone();
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client
                .update_channel(
                    id,
                    ChannelChanges {
                        coordinator_selection: Some(Some(selection)),
                        ..ChannelChanges::default()
                    },
                )
                .await
        });
    }

    /// Do this later: the hub keeps the choice, and Tutti opens.
    fn setup_later(&mut self, cx: &mut Context<Self>) {
        self.setup.later_sent = true;
        self.setup.open_provider = false;
        self.setup.step = SetupStep::Start;
        if self.page == Page::Setup {
            self.page = Page::Main;
        }
        self.follow_setup(cx);
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client.set_setup_later(true).await;
            let _ = this.update(cx, |this, cx| {
                if let Err(err) = result {
                    this.setup.later_sent = false;
                    this.follow_setup(cx);
                    this.show_error(err.to_string(), cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// T8: Sign in to a provider opens S3.
    pub fn open_provider_step(&mut self, cx: &mut Context<Self>) {
        self.setup.open_provider = true;
        self.setup.step = SetupStep::Start;
        self.setup.pending_model = true;
        self.page = Page::Setup;
        cx.notify();
    }

    /// S3: a provider row. An account of this provider that is there from an
    /// earlier try goes on. A new account has the short name of the provider.
    pub fn choose_setup_provider(
        &mut self,
        choice: &ProviderChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.state.session_host.clone() else {
            return;
        };
        self.end_add_account(cx);
        self.dialog_error = None;
        let existing = host
            .accounts
            .iter()
            .find(|a| a.harness == choice.harness)
            .map(|a| a.id.clone());
        self.add_account = AddAccountForm {
            worker_id: host.id.clone(),
            step: AccountStep::SignIn,
            provider: Some(ProviderPick::Builtin(choice.harness)),
            account_id: existing.clone(),
            ..AddAccountForm::default()
        };
        self.setup.step = SetupStep::SignIn;
        if existing.is_some() {
            self.start_sign_in_method(window, cx);
        } else {
            let account = NewAccount {
                worker_id: host.id,
                harness: choice.harness.to_owned(),
                label: provider_short_name(choice.name).to_owned(),
                acp: None,
                env: SecretEnv::new(),
            };
            self.create_account_and_sign_in(account, window, cx);
        }
        cx.notify();
    }

    /// S4 and S5: Back and Choose another provider. The account stays, and a
    /// later try goes on with it.
    fn leave_sign_in(&mut self, cx: &mut Context<Self>) {
        self.end_add_account(cx);
        self.dialog_error = None;
        self.setup.step = SetupStep::Start;
        cx.notify();
    }

    /// The page for `Page::Setup`.
    pub fn render_setup(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let gate = self.setup.screen_gate();
        let install_phase = self.local.hub.view().install.phase;
        let installing = matches!(install_phase, InstallPhase::Running)
            || (matches!(install_phase, InstallPhase::Done) && gate == Gate::Neutral);
        let (selector, dabs, content): (&'static str, Dabs, Div) = match (gate, self.setup.step) {
            (Gate::Provider, SetupStep::SignIn) => (
                "setup-sign-in",
                Dabs::Account,
                self.render_sign_in_screen(t, window, cx),
            ),
            (Gate::Provider, _) => ("setup-provider", Dabs::Account, self.render_provider(t, cx)),
            (Gate::Where | Gate::LocalFound, SetupStep::Server) => {
                ("setup-server", Dabs::Hub, self.render_server(t, cx))
            }
            _ if installing => ("local-setup", Dabs::Hub, self.render_install(t, cx)),
            (Gate::Where | Gate::LocalFound, _) => {
                ("setup-where", Dabs::None, self.render_where(t, cx))
            }
            (_, _) => (
                "setup-neutral",
                match self.state.connection {
                    Connection::NotPaired => Dabs::None,
                    _ => Dabs::Hub,
                },
                div(),
            ),
        };
        setup_frame("setup", selector, dabs, content, t)
    }

    /// W1, with the W3 card when this computer has something to say.
    fn render_where(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let view = self.local.hub.view();
        let found = self.setup.screen_gate() == Gate::LocalFound;
        let first = match view.setup_stage() {
            LocalSetupStage::Found | LocalSetupStage::Choose if found => setup_card(
                "local-found",
                "A hub is on this computer",
                "It has your channels and your memory. This app can connect to it with no setup code.",
                t,
            )
            .child(
                div().flex().child(
                    setup_button("local-connect", "Connect", ButtonKind::Primary, t)
                        .debug_selector(|| "local-connect".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            let hub = this.local.hub.clone();
                            let client = this.client.clone();
                            this.spawn_command(cx, async move { hub.connect(client).await });
                        })),
                ),
            ),
            LocalSetupStage::NotOnThisSystem => setup_card(
                "local-windows",
                "On this computer",
                "Not on Windows yet. Consort cannot keep an agent inside its limits here. Run the hub on a Mac or a Linux computer, and add this computer as a worker.",
                t,
            ),
            LocalSetupStage::NeedsBubblewrap { command } => setup_card(
                "local-bubblewrap",
                "This computer needs bubblewrap",
                "Consort runs each agent inside it. Install it, and then try again.",
                t,
            )
            .child(self.command_block("local-bubblewrap", command, t, cx))
            .child(
                actions(t)
                    .child(
                        button("local-try-again", "Try again", ButtonKind::Secondary, t)
                            .debug_selector(|| "local-try-again".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let hub = this.local.hub.clone();
                                this.spawn_command(cx, async move { hub.recheck().await });
                            })),
                    )
                    .child(connect_button(ButtonKind::Secondary, false, t, cx)),
            ),
            LocalSetupStage::Failed { message } => failure_card(
                "local-failed",
                "The hub did not start".to_owned(),
                "Nothing else changed on this computer. The log says what went wrong.".to_owned(),
                Some(message),
                t,
            )
            .child(
                actions(t)
                    .child(self.try_install_button(t, cx))
                    .child(self.show_log_button(t, cx)),
            ),
            LocalSetupStage::Found | LocalSetupStage::Choose | LocalSetupStage::Install => {
                setup_card(
                    "setup-this-computer",
                    "On this computer",
                    "No server and no setup code. It runs in the background and starts when you sign in. Your assistant works only while this computer is on and awake.",
                    t,
                )
                .child(
                    div().flex().child(
                        setup_button("local-install", "Install here", ButtonKind::Secondary, t)
                            .debug_selector(|| "local-install-here".into())
                            .on_click(cx.listener(|this, _, _, cx| this.install_local_hub(cx))),
                    ),
                )
            }
        };
        let server = setup_card(
            "setup-on-server",
            "On a server",
            "Your assistant keeps working when this computer sleeps, and your phone can reach it. You need a hub that runs, or a Linux server to start one.",
            t,
        )
        .child(div().flex().child(connect_button(ButtonKind::Secondary, true, t, cx)));
        div()
            .flex()
            .flex_col()
            .child(setup_heading(
                "Where should Consort run?",
                "The hub keeps your assistant, channels and memory. You can move it later.",
                t,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.md)
                    .pt(t.component.setup_gap)
                    .child(first)
                    .child(server),
            )
    }

    /// W2: four steps with no input.
    fn render_install(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let view = self.local.hub.view();
        let line = |state: StepState, step: InstallStep, cx: &mut Context<Self>| {
            let (style, color) = match state {
                StepState::Done => (t.text.ui, c.text_secondary),
                StepState::Working => (t.text.ui_strong, c.text_primary),
                StepState::Waiting => (t.text.ui, c.text_tertiary),
            };
            let marker = match state {
                StepState::Done => icon("check", t.icon.size, c.status_done).into_any_element(),
                StepState::Working => status_glyph(
                    SharedString::from(format!("local-step-spin-{}", step.as_str())),
                    TaskStatus::Working,
                    t.icon.size,
                    t,
                    cx,
                ),
                StepState::Waiting => div()
                    .size(t.space.md - t.space.xxs + t.space.xxs)
                    .rounded(t.radius.full)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .into_any_element(),
            };
            let id = SharedString::from(format!("local-step-{}", step.as_str()));
            div()
                .debug_selector(move || id.to_string())
                .flex()
                .items_center()
                .gap(t.space.sm + t.space.xxs)
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .size(t.icon.size)
                        .items_center()
                        .justify_center()
                        .child(marker),
                )
                .child(
                    text(step_label(step, state), style, color)
                        .flex_1()
                        .min_w_0(),
                )
        };
        let mut steps = div()
            .flex()
            .flex_col()
            .gap(t.space.md + t.space.xxs)
            .pt(t.component.setup_gap);
        for (step, state) in InstallStep::ALL.iter().zip(view.install.steps) {
            steps = steps.child(line(state, *step, cx));
        }
        div()
            .debug_selector(|| "local-install".into())
            .flex()
            .flex_col()
            .child(setup_heading(
                "Installing the hub",
                "This takes less than a minute. You do not need to do anything.",
                t,
            ))
            .child(steps)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs + t.space.xxs)
                    .mt(t.component.setup_gap)
                    .pt(t.space.lg)
                    .border_t(t.border_width)
                    .border_color(c.border_subtle)
                    .child(text(
                        if cfg!(target_os = "macos") {
                            "The hub keeps running after you quit the app. macOS can say that a background item was added."
                        } else {
                            "The hub keeps running after you quit the app."
                        },
                        detail(t),
                        c.text_tertiary,
                    ))
                    .child(div().font_family(t.font_mono.clone()).child(selectable(
                        "local-folder",
                        view.folder.clone(),
                        detail(t),
                        c.text_tertiary,
                    ))),
            )
    }

    /// S2: one command and the setup code.
    fn render_server(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let number = |n: &'static str| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(t.component.step_marker)
                .rounded(t.radius.full)
                .border(t.border_width)
                .border_color(c.border_strong)
                .child(text(n, detail(t), c.text_secondary))
        };
        let step = |n, title: &'static str, line: &'static str| {
            div().flex().gap(t.space.md).child(number(n)).child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.sm)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(t.space.xxs)
                            .child(text(title, t.text.ui_strong, c.text_primary))
                            .child(text(line, detail(t), c.text_tertiary)),
                    ),
            )
        };
        let command = self.command_block(
            "setup-hub",
            consort_client::update::hub_install_command(),
            t,
            cx,
        );
        let run = step(
            "1",
            "Run this on your server",
            "It installs the hub and the session host, and starts both. With Tailscale, the server needs no DNS name and no open ports.",
        );
        div()
            .flex()
            .flex_col()
            .child(setup_heading(
                "Start a hub on your server",
                "A server stays on, so your assistant works while this computer sleeps, and your phone can reach it.",
                t,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.lg + t.space.xs)
                    .pt(t.space.xxl)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(t.space.sm)
                            .child(run)
                            .child(div().pl(t.component.step_marker + t.space.md).child(command)),
                    )
                    .child(step(
                        "2",
                        "Enter the setup code",
                        "The script prints it. It works once and expires after 10 minutes.",
                    )),
            )
            .child(
                actions(t)
                    .pt(t.component.setup_gap)
                    .child(
                        setup_button(
                            "setup-have-code",
                            "I have a setup code",
                            ButtonKind::Primary,
                            t,
                        )
                        .debug_selector(|| "setup-have-code".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.setup.step = SetupStep::Start;
                            this.page = Page::Onboarding;
                            cx.notify();
                        })),
                    )
                    .child(
                        setup_button(
                            "setup-server-guide",
                            "Open the server guide",
                            ButtonKind::Secondary,
                            t,
                        )
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.open_url(&consort_client::update::server_guide_url());
                        })),
                    ),
            )
    }

    /// S3: one provider for the first account.
    fn render_provider(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let host = self.state.session_host.clone().filter(|h| h.online);
        let body: Div = match &host {
            None => self.render_no_host(t, cx),
            Some(host) => {
                let mut list = div().flex().flex_col().gap(t.space.xs + t.space.xxs);
                for choice in provider_choices(host) {
                    let logo = choice.logo.clone();
                    let (name, line) = (choice.name, choice.line);
                    let harness = choice.harness;
                    list = list.child(
                        focusable(
                            div().id(SharedString::from(format!("provider-{harness}"))),
                            t,
                        )
                        .debug_selector(move || format!("setup-provider-{harness}"))
                        .flex()
                        .items_center()
                        .gap(t.space.md)
                        .py(t.space.md)
                        .px(t.space.md + t.space.xxs)
                        .bg(c.surface_card)
                        .border(t.border_width)
                        .border_color(c.border_subtle)
                        .rounded(t.radius.lg)
                        .cursor_pointer()
                        .hover(|s| s.bg(c.hover_card))
                        .child(provider_logo(
                            &logo,
                            t.component.sign_in_logo,
                            c.text_primary,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(text(name, t.text.ui_strong, c.text_primary))
                                .child(text(line, detail(t), c.text_tertiary)),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.choose_setup_provider(&choice, window, cx);
                            },
                        )),
                    );
                }
                let another = host.id.clone();
                list.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap(t.space.sm)
                        .pt(t.space.md + t.space.xs - t.space.xxs)
                        .child(
                            link_button("setup-another-agent", "Choose another agent", t).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.open_dialog(
                                        Dialog::AddAccount(another.clone()),
                                        window,
                                        cx,
                                    );
                                }),
                            ),
                        )
                        .child(
                            link_button("setup-later", "Do this later", t)
                                .debug_selector(|| "setup-later".into())
                                .on_click(cx.listener(|this, _, _, cx| this.setup_later(cx))),
                        ),
                )
            }
        };
        let stopped = self.local_stopped_card(t, cx);
        div()
            .flex()
            .flex_col()
            .child(setup_heading(
                "Sign in to a provider",
                "The assistant needs a model to think with. Choose a plan that you already pay for. The login stays on the hub machine.",
                t,
            ))
            .when_some(stopped, |d, card| d.child(div().pt(t.component.setup_gap).child(card)))
            .child(div().pt(t.component.setup_gap).child(body))
    }

    /// S5: the hub has no session host, or it is away. The screen goes on by
    /// itself when the session host connects.
    fn render_no_host(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        div()
            .debug_selector(|| "setup-no-host".into())
            .flex()
            .flex_col()
            .gap(t.space.sm + t.space.xxs)
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
                    .child(text(
                        "The hub machine cannot run agents yet",
                        Type {
                            weight: t.text.title.weight,
                            ..t.text.ui
                        },
                        c.text_primary,
                    ))
                    .child(text(
                        "The hub has no session host. Start it on the hub machine with a code from this command.",
                        detail(t),
                        c.text_secondary,
                    )),
            )
            .child(self.command_block(
                "setup-session-host",
                "consort hub session-host-code".to_owned(),
                t,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(status_glyph(
                        "setup-host-waiting",
                        TaskStatus::Working,
                        t.icon.dense,
                        t,
                        cx,
                    ))
                    .child(
                        text(
                            "Waiting for the session host to connect",
                            detail(t),
                            c.text_secondary,
                        )
                        .flex_1()
                        .min_w_0(),
                    ),
            )
    }

    /// S4 and the sign-in states of S5.
    fn render_sign_in_screen(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        self.dialog_confirm.replace(None);
        let account = self.dialog_account();
        self.auto_start_sign_in(window, cx);
        let (provider, _) = self.chosen_provider();
        let short = provider_short_name(&provider).to_owned();
        let stage = tool_stage(
            self.state
                .session_host
                .as_ref()
                .and_then(|host| tool_of(host, &provider)),
        );
        let step = self.running_sign_in_step();
        let mut column = div().flex().flex_col().child(setup_heading(
            &format!("Sign in to {short}"),
            &format!(
                "The hub machine runs {provider}'s own sign-in. Consort does not see your password."
            ),
            t,
        ));
        let block = |content: Div| div().pt(t.space.lg + t.space.xs).child(content);
        match &stage {
            ToolStage::Installing => {
                column = column.child(block(install_card(&provider, t, cx)));
            }
            ToolStage::Failed { detail: why } => {
                column = column.child(block(self.install_failed_card(
                    &provider,
                    why.clone(),
                    t,
                    cx,
                )));
            }
            ToolStage::Ready { version } => {
                if let Some(version) = version {
                    column = column.child(
                        div()
                            .pt(t.space.lg + t.space.xs)
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .child(icon("check", t.icon.dense, c.status_done))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .flex_1()
                                    .min_w_0()
                                    .child(text(
                                        format!(
                                            "{provider} is installed on the hub machine, version "
                                        ),
                                        detail(t),
                                        c.text_secondary,
                                    ))
                                    .child(
                                        text(version.clone(), detail(t), c.text_secondary)
                                            .font_family(t.font_mono.clone()),
                                    )
                                    .child(text(".", detail(t), c.text_secondary)),
                            ),
                    );
                }
                if let Some(tabs) = self.method_tabs(t, cx) {
                    column = column.child(div().pt(t.space.lg + t.space.xs).child(tabs));
                }
                let failed = self.sign_in_failure(&step, &short);
                let card = match failed {
                    Some((title, line, more)) => {
                        failure_card("setup-code-failed", title, line, more, t).child(
                            div().flex().child(
                                button(
                                    "setup-start-again",
                                    "Start again",
                                    ButtonKind::Secondary,
                                    t,
                                )
                                .debug_selector(|| "setup-start-again".into())
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.start_sign_in_method(window, cx),
                                )),
                            ),
                        )
                    }
                    None => self.sign_in_flow_card(t, window, cx),
                };
                column = column.child(div().pt(t.space.md).child(card));
            }
        }
        if let Some(err) = self.dialog_error.clone() {
            column = column.child(div().pt(t.space.md).child(crate::ui::parts::error_box(
                SharedString::from(format!("setup-error-{err}")),
                err,
                t,
            )));
        }
        let note = match (&step, account.as_ref()) {
            (Some(SignInStep::NeedsCode { .. } | SignInStep::DeviceCode { .. }), _) => {
                Some("The code works once. If it expires, start again.")
            }
            _ => None,
        };
        column.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(t.space.lg)
                .pt(t.space.lg)
                .child(
                    button("setup-back", "Back", ButtonKind::Secondary, t)
                        .h(t.space.xxl)
                        .px(t.space.md + t.space.xxs)
                        .rounded(t.radius.lg)
                        .debug_selector(|| "setup-back".into())
                        .on_click(cx.listener(|this, _, _, cx| this.leave_sign_in(cx))),
                )
                .when_some(note, |d, note| {
                    d.child(
                        text(note, detail(t), c.text_tertiary)
                            .flex_1()
                            .min_w_0()
                            .text_right(),
                    )
                }),
        )
    }

    /// The text of S5 for a sign-in that ended with no login: the title, the
    /// line and the details. `None` while it can still go on.
    fn sign_in_failure(
        &self,
        step: &Option<SignInStep>,
        short: &str,
    ) -> Option<(String, String, Option<String>)> {
        let refused = || {
            (
                format!("{short} did not accept this code"),
                "A code works once. Start again for a new sign-in page.".to_owned(),
            )
        };
        match step {
            Some(SignInStep::Failed { message }) if self.add_account.code_sent => {
                let (title, line) = refused();
                Some((title, line, Some(message.clone())))
            }
            Some(SignInStep::Expired) if self.add_account.code_sent => {
                let (title, line) = refused();
                Some((title, line, None))
            }
            Some(SignInStep::Failed { message }) => Some((
                "The sign-in failed".to_owned(),
                "Start again for a new sign-in page.".to_owned(),
                Some(message.clone()),
            )),
            Some(SignInStep::Expired) => Some((
                "The sign-in expired".to_owned(),
                "Nothing happened for 15 minutes. Start again for a new sign-in page.".to_owned(),
                None,
            )),
            Some(SignInStep::Cancelled) => Some((
                "The sign-in stopped".to_owned(),
                "It was cancelled. Start again to sign in.".to_owned(),
                None,
            )),
            _ => None,
        }
    }

    fn install_failed_card(
        &mut self,
        provider: &str,
        why: Option<String>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let host = self.state.session_host.as_ref().map(|h| h.id.clone());
        let tool = self
            .state
            .session_host
            .as_ref()
            .and_then(|h| tool_of(h, provider))
            .map(|tool| tool.id.clone());
        failure_card(
            "setup-install-failed",
            format!("{provider} did not install"),
            "The download stopped on the hub machine. Check its network, and then try again."
                .to_owned(),
            why,
            t,
        )
        .child(
            actions(t)
                .child(
                    button("setup-install-again", "Try again", ButtonKind::Secondary, t)
                        .debug_selector(|| "setup-install-again".into())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let (Some(host), Some(tool)) = (host.clone(), tool.clone()) else {
                                return;
                            };
                            // The sign-in starts again when the install ends.
                            this.add_account.sign_in_id = None;
                            this.add_account.auto_started = false;
                            let client = this.client.clone();
                            this.spawn_command(
                                cx,
                                async move { client.install_tool(host, tool).await },
                            );
                        })),
                )
                .child(
                    button(
                        "setup-another-provider",
                        "Choose another provider",
                        ButtonKind::Secondary,
                        t,
                    )
                    .debug_selector(|| "setup-another-provider".into())
                    .on_click(cx.listener(|this, _, _, cx| this.leave_sign_in(cx))),
                ),
        )
    }

    /// Opens a setup screen of the designs, for `--screen`.
    pub fn open_setup_screen(&mut self, screen: &str, window: &mut Window, cx: &mut Context<Self>) {
        use consort_client::mock_local::{SETUP_ACCOUNT, SETUP_SIGN_IN, screen_view};
        if let Some(view) = screen_view(screen) {
            self.local = crate::ui::local_hub::LocalUi::with(
                consort_client::local_hub::LocalHub::fixed(view),
            );
        }
        let host = self
            .state
            .session_host
            .as_ref()
            .map(|h| h.id.clone())
            .unwrap_or_default();
        self.page = Page::Setup;
        self.setup = SetupUi {
            gate: Gate::Where,
            ..SetupUi::default()
        };
        match screen {
            "setup-found" => self.setup.gate = Gate::LocalFound,
            "setup-server" => self.setup.step = SetupStep::Server,
            "setup-neutral" => self.setup.gate = Gate::Neutral,
            "setup-connect"
            | "setup-connect-unreachable"
            | "setup-connect-refused"
            | "setup-connect-busy" => {
                self.page = Page::Onboarding;
                self.inputs
                    .hub_url
                    .update(cx, |input, cx| input.focus(window, cx));
                self.onboarding.busy = screen == "setup-connect-busy";
                let error = match screen {
                    "setup-connect-unreachable" => Some(consort_client::ClientError::Unreachable {
                        url: "https://hub.example.com".to_owned(),
                    }),
                    "setup-connect-refused" => Some(consort_client::ClientError::BadCode),
                    _ => None,
                };
                if let Some(error) = error {
                    self.show_pair_error(&error, cx);
                }
            }
            "setup-provider" | "setup-no-host" => self.setup.gate = Gate::Provider,
            "setup-no-account" => {
                self.setup.gate = Gate::NoAccount;
                self.page = Page::Main;
                if let Some(tutti) = self.state.tutti().map(|tutti| tutti.id.clone()) {
                    self.open_channel(tutti, window, cx);
                }
            }
            "setup-sign-in"
            | "setup-tool-installing"
            | "setup-tool-failed"
            | "setup-code-refused" => {
                self.setup.gate = Gate::Provider;
                self.setup.step = SetupStep::SignIn;
                self.add_account = AddAccountForm {
                    worker_id: host.clone(),
                    step: AccountStep::SignIn,
                    provider: Some(ProviderPick::Builtin("claude")),
                    account_id: Some(SETUP_ACCOUNT.to_owned()),
                    auto_started: true,
                    ..AddAccountForm::default()
                };
                if screen == "setup-code-refused" {
                    self.add_account.sign_in_id = Some(SETUP_SIGN_IN.to_owned());
                    self.add_account.code_sent = true;
                }
                if screen == "setup-sign-in" {
                    let started = self
                        .client
                        .run_blocking(consort_client::Command::StartSignIn {
                            worker_id: host,
                            account_id: SETUP_ACCOUNT.to_owned(),
                            method: consort_client::SignInChoice::ClaudePlan,
                        });
                    if let Ok(consort_client::CommandOutput::SignInStarted(id)) = started {
                        self.add_account.sign_in_id = Some(id);
                    }
                    self.state = self.client.state();
                }
            }
            _ => {}
        }
        cx.notify();
    }
}

/// The page for a gate. Settings stays open while a client is paired. A client
/// that has left its hub, or that the hub refused, cannot stay there.
fn setup_page(gate: Gate, page: Page, open_provider: bool) -> Page {
    match gate {
        Gate::PairAgain => Page::Onboarding,
        Gate::Where | Gate::LocalFound if page == Page::Onboarding => Page::Onboarding,
        Gate::Where | Gate::LocalFound => Page::Setup,
        Gate::Main if page == Page::Settings => Page::Settings,
        Gate::NoAccount if page == Page::Settings => Page::Settings,
        Gate::NoAccount if open_provider => Page::Setup,
        Gate::Main | Gate::NoAccount => Page::Main,
        Gate::Neutral | Gate::Provider if page == Page::Settings => Page::Settings,
        Gate::Neutral | Gate::Provider => Page::Setup,
    }
}

/// The frame of a setup screen: the mark with the dabs that are lit, and a
/// column that scrolls and fades at its edges.
pub fn setup_frame(
    id: &'static str,
    selector: &'static str,
    dabs: Dabs,
    content: Div,
    t: &Tokens,
) -> AnyElement {
    let c = &t.colors;
    let fade = |angle: f32| {
        linear_gradient(
            angle,
            linear_color_stop(c.surface_canvas, 0.),
            linear_color_stop(c.canvas_clear, 1.),
        )
    };
    div()
        .id(id)
        .debug_selector(move || selector.to_owned())
        .relative()
        .size_full()
        .bg(c.surface_canvas)
        .child(
            div()
                .id("setup-scroll")
                .size_full()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .items_center()
                .type_style(t.text.ui)
                .child(
                    div()
                        .debug_selector(|| "setup-column".into())
                        .flex()
                        .flex_col()
                        .flex_none()
                        .w_full()
                        .max_w(t.layout.setup_column + t.space.xl * 2.0)
                        .px(t.space.xl)
                        .pt(t.layout.setup_top)
                        .pb(t.space.page)
                        .child(crate::ui::mark::setup_mark(dabs, t.component.setup_mark, t))
                        .child(content),
                ),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(t.layout.fade_bottom)
                .bg(fade(180.)),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(t.layout.fade_bottom)
                .bg(fade(0.)),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(t.space.xxl + t.space.md)
                .drag_area(),
        )
        .into_any_element()
}

/// The title and the line of a setup screen.
pub fn setup_heading(title: &str, line: &str, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .flex_col()
        .gap(t.space.sm)
        .pt(t.component.setup_gap)
        .child(text(title.to_owned(), t.text.display, c.text_primary))
        .child(text(line.to_owned(), t.text.body, c.text_secondary))
}

/// A button of a setup screen.
pub fn setup_button(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    kind: ButtonKind,
    t: &Tokens,
) -> Stateful<Div> {
    button(id, label, kind, t)
        .h(t.component.setup_control)
        .px(if kind == ButtonKind::Primary {
            t.space.xl
        } else {
            t.space.lg
        })
        .rounded(t.radius.lg)
}

/// A quiet action in text, such as Do this later.
pub fn link_button(id: &'static str, label: &'static str, t: &Tokens) -> Stateful<Div> {
    let c = &t.colors;
    focusable(div().id(id), t)
        .flex_none()
        .cursor_pointer()
        .rounded(t.radius.sm)
        .type_style(t.text.ui_strong)
        .text_color(c.text_secondary)
        .hover(|s| s.text_color(c.text_primary))
        .child(label)
}

/// A card of W1 and W3. W3 draws the card about bubblewrap in smaller text.
fn setup_card(id: &'static str, title: &'static str, body: &'static str, t: &Tokens) -> Div {
    let c = &t.colors;
    let quiet = id == "local-windows";
    let compact = id == "local-bubblewrap";
    div()
        .debug_selector(move || id.to_owned())
        .flex()
        .flex_col()
        .gap(t.space.md)
        .p(t.space.lg)
        .bg(c.surface_card)
        .border(t.border_width)
        .border_color(if quiet || compact {
            c.border_subtle
        } else {
            c.border_strong
        })
        .rounded(if compact { t.radius.lg } else { t.radius.xl })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(t.space.xxs)
                .child(text(
                    title,
                    Type {
                        weight: t.text.title.weight,
                        ..if compact { t.text.ui } else { t.text.body }
                    },
                    if quiet {
                        c.text_secondary
                    } else {
                        c.text_primary
                    },
                ))
                .child(text(
                    body,
                    if compact { detail(t) } else { t.text.ui },
                    c.text_secondary,
                )),
        )
}

/// A card of S5 that says what failed and what to do. All of its text can be
/// selected, because a user may copy it.
pub fn failure_card(
    id: &'static str,
    title: String,
    line: String,
    more: Option<String>,
    t: &Tokens,
) -> Div {
    let c = &t.colors;
    div()
        .debug_selector(move || id.to_owned())
        .flex()
        .flex_col()
        .gap(t.space.sm + t.space.xxs)
        .p(t.space.md)
        .bg(c.tint_failed)
        .rounded(t.radius.lg)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(t.space.xxs)
                .child(selectable(
                    SharedString::from(format!("{id}-title")),
                    title,
                    t.text.ui_strong,
                    c.text_primary,
                ))
                .child(selectable(
                    SharedString::from(format!("{id}-line")),
                    line,
                    detail(t),
                    c.text_secondary,
                ))
                .when_some(more, |d, more| {
                    d.child(selectable(
                        SharedString::from(format!("{id}-more")),
                        more,
                        detail(t),
                        c.text_tertiary,
                    ))
                }),
        )
}

/// S5: the tool installs before the sign-in. The tool reports no percent, so
/// the bar moves with no end.
fn install_card(provider: &str, t: &Tokens, cx: &mut Context<ConsortApp>) -> Div {
    let c = &t.colors;
    let turn = crate::ui::motion::spinner_angle(true, cx) / std::f32::consts::TAU;
    div()
        .debug_selector(|| "setup-installing".into())
        .flex()
        .flex_col()
        .gap(t.space.sm)
        .p(t.space.lg)
        .bg(c.surface_card)
        .border(t.border_width)
        .border_color(c.border_subtle)
        .rounded(t.radius.lg)
        .child(text(
            format!("Installing {provider} on the hub machine"),
            t.text.ui_strong,
            c.text_primary,
        ))
        .child(
            div()
                .debug_selector(|| "setup-install-bar".into())
                .relative()
                .w_full()
                .h(t.component.usage_bar)
                .overflow_hidden()
                .rounded(t.radius.full)
                .bg(c.surface_selected)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(turn * 1.45 - 0.45))
                        .w(relative(0.45))
                        .rounded(t.radius.full)
                        .bg(c.status_working),
                ),
        )
        .child(text(
            "Sign-in starts when this ends. You do not need to do anything.",
            detail(t),
            c.text_tertiary,
        ))
}

/// A row of buttons that wraps at the compact width.
fn actions(t: &Tokens) -> Div {
    div().flex().flex_wrap().gap(t.space.sm)
}

/// Connect to a hub: opens S1.
fn connect_button(
    kind: ButtonKind,
    tall: bool,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> Stateful<Div> {
    let base = if tall {
        setup_button("local-connect-elsewhere", "Connect to a hub", kind, t)
    } else {
        button("local-connect-elsewhere-short", "Connect to a hub", kind, t)
    };
    base.debug_selector(|| "local-connect-elsewhere".into())
        .on_click(cx.listener(|this, _, _, cx| {
            this.page = Page::Onboarding;
            cx.notify();
        }))
}

/// The empty state of Tutti with no signed-in account (T8).
pub fn no_account_state(t: &Tokens, cx: &mut Context<ConsortApp>) -> Div {
    let c = &t.colors;
    div()
        .debug_selector(|| "no-account-state".into())
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(t.space.xs + t.space.xxs)
        .px(t.space.xl)
        .child(text(
            "The assistant cannot answer yet",
            t.text.display,
            c.text_primary,
        ))
        .child(
            text(
                "It needs one signed-in account. When you sign in, it says hello here. You can look around first.",
                t.text.body,
                c.text_secondary,
            )
            .text_center()
            .max_w(t.layout.setup_column),
        )
        .child(
            div().pt(t.space.sm + t.space.xxs).child(
                setup_button(
                    "no-account-sign-in",
                    "Sign in to a provider",
                    ButtonKind::Primary,
                    t,
                )
                .debug_selector(|| "no-account-sign-in".into())
                .on_click(cx.listener(|this, _, _, cx| this.open_provider_step(cx))),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_moves_the_page_and_leaves_settings_open() {
        assert_eq!(setup_page(Gate::Where, Page::Main, false), Page::Setup);
        assert_eq!(setup_page(Gate::Where, Page::Settings, false), Page::Setup);
        assert_eq!(
            setup_page(Gate::Where, Page::Onboarding, false),
            Page::Onboarding,
            "S1 is a step of W1"
        );
        assert_eq!(
            setup_page(Gate::PairAgain, Page::Main, false),
            Page::Onboarding
        );
        assert_eq!(setup_page(Gate::Provider, Page::Main, false), Page::Setup);
        assert_eq!(
            setup_page(Gate::Provider, Page::Settings, false),
            Page::Settings
        );
        assert_eq!(setup_page(Gate::Neutral, Page::Setup, false), Page::Setup);
        assert_eq!(setup_page(Gate::Main, Page::Setup, false), Page::Main);
        assert_eq!(
            setup_page(Gate::Main, Page::Settings, false),
            Page::Settings
        );
        assert_eq!(setup_page(Gate::NoAccount, Page::Setup, false), Page::Main);
        assert_eq!(setup_page(Gate::NoAccount, Page::Setup, true), Page::Setup);
    }
}
