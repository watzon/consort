//! The Add account dialog: step 1 chooses the provider (the four built-in
//! providers, an ACP registry agent or a custom command), step 2 names the
//! account, and step 3 signs in. The account is made when the user leaves
//! step 2. "Sign in" and "Sign in again" on an account row open step 3.

use consort_client::setup::PROVIDERS;
use consort_client::{
    AcpSpec, HarnessAccountView, NewAccount, ProviderLogo, RegistryLoad, SecretEnv, SignInChoice,
    SignInStep, profile_slug,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Pixels, SharedString, StatefulInteractiveElement as _, Styled,
    Window, component::input::InputState, div, prelude::FluentBuilder as _,
};

use crate::{
    app::{AccountStep, AddAccountForm, ConsortApp, Dialog, ProviderPick},
    ui::{
        dialogs::input_box,
        parts::{
            ButtonKind, button, detail, error_box, focusable, icon, provider_logo, selectable,
            status_glyph, text,
        },
        settings::segmented,
    },
};

/// What the dialog builds: the title, the line under it, the width, the
/// body, the footer and the logo tile before the title.
pub type AccountDialogParts = (String, String, Pixels, Div, Div, Option<AnyElement>);

/// Only web pages open from a sign-in. A worker that sends any other kind
/// of address gets an error.
fn is_web_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

impl ConsortApp {
    /// Opens the dialog at step 3 for an account that exists: "Sign in" and
    /// "Sign in again" on an account row.
    pub fn open_sign_in(
        &mut self,
        worker_id: String,
        account_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let provider = self
            .state
            .worker(&worker_id)
            .and_then(|w| w.account(&account_id))
            .map(|a| self.pick_of(a));
        self.open_dialog(Dialog::AddAccount(worker_id.clone()), window, cx);
        self.add_account = AddAccountForm {
            worker_id,
            step: AccountStep::SignIn,
            provider,
            account_id: Some(account_id),
            ..AddAccountForm::default()
        };
        self.start_sign_in_method(window, cx);
    }

    /// Opens the dialog at step 3 on a sign-in that runs already, such as
    /// one that the hub started for a sign-in card.
    pub fn open_running_sign_in(
        &mut self,
        worker_id: String,
        account_id: String,
        sign_in_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let account = self
            .state
            .worker(&worker_id)
            .and_then(|w| w.account(&account_id))
            .cloned();
        let provider = account.as_ref().map(|a| self.pick_of(a));
        let method = account
            .as_ref()
            .and_then(|a| {
                let running = self.state.sign_in(&sign_in_id)?.method.clone()?;
                a.sign_in_methods.iter().position(|m| *m == running)
            })
            .unwrap_or(0);
        self.open_dialog(Dialog::AddAccount(worker_id.clone()), window, cx);
        self.add_account = AddAccountForm {
            worker_id,
            step: AccountStep::SignIn,
            provider,
            account_id: Some(account_id),
            method,
            sign_in_id: Some(sign_in_id),
            auto_started: true,
            ..AddAccountForm::default()
        };
        cx.notify();
    }

    /// The provider of an account that exists.
    fn pick_of(&self, account: &HarnessAccountView) -> ProviderPick {
        if let Some(provider) = PROVIDERS.iter().find(|p| p.harness == account.harness) {
            return ProviderPick::Builtin(provider.harness);
        }
        match account.harness.strip_prefix("acp:") {
            Some(id) => ProviderPick::Registry {
                id: id.to_owned(),
                name: account.provider.clone(),
            },
            None => ProviderPick::Command,
        }
    }

    /// The name and the logo of the chosen provider.
    pub(crate) fn chosen_provider(&self) -> (String, ProviderLogo) {
        match &self.add_account.provider {
            Some(ProviderPick::Builtin(harness)) => PROVIDERS
                .iter()
                .find(|p| p.harness == *harness)
                .map(|p| (p.name.to_owned(), p.logo.clone()))
                .unwrap_or_default(),
            Some(ProviderPick::Registry { id, name }) => {
                (name.clone(), self.state.acp_registry.logo(id))
            }
            Some(ProviderPick::Command) => ("Custom command".to_owned(), ProviderLogo::Generic),
            None => (String::new(), ProviderLogo::Generic),
        }
    }

    /// The account that the dialog signs in, once the worker reports it.
    pub(crate) fn dialog_account(&self) -> Option<HarnessAccountView> {
        let form = &self.add_account;
        self.state
            .worker(&form.worker_id)
            .and_then(|w| w.account(form.account_id.as_deref()?))
            .cloned()
    }

    /// Ends the dialog: a sign-in that still runs is cancelled, and the
    /// sign-in leaves the view state.
    pub fn end_add_account(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.add_account.sign_in_id.take() else {
            return;
        };
        let running = self.state.sign_in(&id).is_some_and(|s| !s.step.is_final());
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            if running {
                let _ = client.cancel_sign_in(id.clone()).await;
            }
            client
                .run(consort_client::Command::DismissSignIn { sign_in_id: id })
                .await
        });
    }

    /// Starts the sign-in of the chosen method. A sign-in that runs for
    /// another method is cancelled first. An API key and a terminal method
    /// have no flow, so nothing starts.
    pub fn start_sign_in_method(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.dialog_account() else {
            // The worker has not reported the account yet. The render
            // starts the sign-in when it does.
            return;
        };
        if self
            .sign_in_tool()
            .is_some_and(|tool| tool.state == consort_client::ToolState::Installing)
        {
            // The tool installs first. The render starts the sign-in when
            // that ends.
            return;
        }
        self.add_account.auto_started = true;
        self.add_account.code_sent = false;
        self.end_add_account(cx);
        self.dialog_error = None;
        let Some(method) = account
            .sign_in_methods
            .get(self.add_account.method)
            .cloned()
        else {
            return;
        };
        if !method.has_flow() {
            if matches!(method, SignInChoice::ApiKey { .. }) {
                self.inputs.api_key.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
            }
            cx.notify();
            return;
        }
        self.inputs
            .sign_in_code
            .update(cx, |input, cx| input.set_value("", window, cx));
        let client = self.client.clone();
        let worker_id = self.add_account.worker_id.clone();
        let account_id = account.id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = client.start_sign_in(worker_id, account_id, method).await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(id) => {
                        this.add_account.sign_in_id = Some(id);
                        this.inputs
                            .sign_in_code
                            .update(cx, |input, cx| input.focus(window, cx));
                    }
                    Err(err) => this.dialog_error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// True when a sign-in can start by itself. A sign-in installs a missing
    /// tool on the session host, so a missing tool does not hold it. An install
    /// that runs holds it until the end. A failed install waits for the user.
    fn sign_in_tool_ready(&self) -> bool {
        self.sign_in_tool().is_none_or(|tool| {
            !matches!(
                tool.state,
                consort_client::ToolState::Installing | consort_client::ToolState::Failed
            )
        })
    }

    /// Starts the sign-in once, when the worker has reported the account and
    /// its tool is ready. The dialog and the sign-in screen both call this.
    pub(crate) fn auto_start_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_account().is_some()
            && !self.add_account.auto_started
            && self.add_account.sign_in_id.is_none()
            && self.sign_in_tool_ready()
        {
            self.add_account.auto_started = true;
            cx.defer_in(window, |this, window, cx| {
                this.start_sign_in_method(window, cx)
            });
        }
    }

    /// The step of the running sign-in of the form.
    pub(crate) fn running_sign_in_step(&self) -> Option<SignInStep> {
        let id = self.add_account.sign_in_id.as_ref()?;
        self.state.sign_in(id).map(|s| s.step.clone())
    }

    /// Next on step 1 and step 2. Leaving step 2 makes the account.
    fn add_account_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog_error = None;
        match self.add_account.step {
            AccountStep::Provider => {
                let Some(provider) = self.add_account.provider.clone() else {
                    self.dialog_error = Some("Choose a provider, then click Next.".into());
                    cx.notify();
                    return;
                };
                if provider == ProviderPick::Command
                    && self.field_text(&self.inputs.program, cx).is_empty()
                {
                    self.dialog_error =
                        Some("Enter the program that runs the agent, then click Next.".into());
                    cx.notify();
                    return;
                }
                let (name, _) = self.chosen_provider();
                let current = self.field_text(&self.inputs.account_name, cx);
                if current.is_empty() {
                    self.inputs
                        .account_name
                        .update(cx, |input, cx| input.set_value(name, window, cx));
                }
                self.inputs
                    .account_name
                    .update(cx, |input, cx| input.focus(window, cx));
                self.add_account.step = AccountStep::Name;
                cx.notify();
            }
            AccountStep::Name => {
                let label = self.field_text(&self.inputs.account_name, cx);
                if label.is_empty() {
                    self.dialog_error = Some("Enter a name for the account.".into());
                    cx.notify();
                    return;
                }
                let worker_id = self.add_account.worker_id.clone();
                let taken = self
                    .state
                    .worker(&worker_id)
                    .is_some_and(|w| w.accounts.iter().any(|a| a.label == label));
                if taken {
                    self.dialog_error = Some(format!(
                        "An account named {label} is there already. Choose another name."
                    ));
                    cx.notify();
                    return;
                }
                let (harness, acp) = match self.add_account.provider.clone() {
                    Some(ProviderPick::Builtin(harness)) => (harness.to_owned(), None),
                    Some(ProviderPick::Registry { id, .. }) => {
                        ("acp".to_owned(), Some(AcpSpec::Registry { id }))
                    }
                    Some(ProviderPick::Command) | None => {
                        let program = self.field_text(&self.inputs.program, cx);
                        let args = self
                            .field_text(&self.inputs.program_args, cx)
                            .split_whitespace()
                            .map(str::to_owned)
                            .collect();
                        ("acp".to_owned(), Some(AcpSpec::Command { program, args }))
                    }
                };
                let account = NewAccount {
                    worker_id,
                    harness,
                    label,
                    acp,
                    env: SecretEnv::new(),
                };
                self.create_account_and_sign_in(account, window, cx);
            }
            AccountStep::SignIn => {}
        }
    }

    /// Makes the account, then moves to step 3 and starts the sign-in. The
    /// dialog stays busy while the hub answers, so a second click waits.
    pub(crate) fn create_account_and_sign_in(
        &mut self,
        account: NewAccount,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.in_flight.begin("add-account") {
            return;
        }
        let client = self.client.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = client.create_account(account).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.in_flight.finish("add-account");
                let on_screen = this.setup.step == crate::ui::setup::SetupStep::SignIn;
                if !on_screen && !matches!(this.dialog, Some(Dialog::AddAccount(_))) {
                    if let Err(err) = &result {
                        this.show_error(err.to_string(), cx);
                    }
                    return;
                }
                match result {
                    Ok(id) => {
                        this.add_account.account_id = Some(id);
                        this.add_account.step = AccountStep::SignIn;
                        this.add_account.method = 0;
                        this.start_sign_in_method(window, cx);
                    }
                    Err(err) => this.dialog_error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn field_text(&self, input: &Entity<InputState>, cx: &gpui_kit::App) -> String {
        input.read(cx).value().trim().to_owned()
    }

    fn is_focused(&self, input: &Entity<InputState>, window: &Window, cx: &gpui_kit::App) -> bool {
        input.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The Add account dialog, at its step.
    pub fn add_account_dialog(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        // Every account is on the hub machine. The dialog names no worker.
        let worker = "the hub machine".to_owned();
        match self.add_account.step {
            AccountStep::Provider => {
                let (body, note) = self.provider_step(t, window, cx);
                let actions = self.account_footer(note, "Cancel", "Next", true, t, cx);
                (
                    "Add an account".to_owned(),
                    "An account is one login to a provider. The hub machine keeps each one in its own profile folder, so one provider can have several accounts.".to_owned(),
                    t.layout.dialog_account,
                    body,
                    actions,
                    None,
                )
            }
            AccountStep::Name => {
                let body = self.name_step(t, window, cx);
                let actions = self.account_footer(
                    "You can change the name later.".into(),
                    "Back",
                    "Next",
                    true,
                    t,
                    cx,
                );
                (
                    "Add an account".to_owned(),
                    "An account is one login to a provider. The hub machine keeps each one in its own profile folder, so one provider can have several accounts.".to_owned(),
                    t.layout.dialog_account,
                    body,
                    actions,
                    None,
                )
            }
            AccountStep::SignIn => self.sign_in_step(&worker, t, window, cx),
        }
    }

    /// The bar of the three steps. A step that is done shows a check and
    /// what was chosen in it.
    fn steps_bar(&self, t: &Tokens) -> Div {
        let c = &t.colors;
        let (provider, _) = self.chosen_provider();
        let label = self
            .dialog_account()
            .map(|a| a.label)
            .unwrap_or_else(|| "Name".to_owned());
        let current = self.add_account.step;
        let steps = [
            (AccountStep::Provider, "1", "Provider", provider),
            (AccountStep::Name, "2", "Name", label),
            (
                AccountStep::SignIn,
                "3",
                if self.sign_in_tool().is_some_and(|tool| {
                    !matches!(
                        tool.state,
                        consort_client::ToolState::Ready
                            | consort_client::ToolState::UpdateAvailable
                    )
                }) {
                    "Install and sign in"
                } else {
                    "Sign in"
                },
                String::new(),
            ),
        ];
        let order = |s: AccountStep| match s {
            AccountStep::Provider => 0,
            AccountStep::Name => 1,
            AccountStep::SignIn => 2,
        };
        let mut bar = div()
            .flex()
            .gap(t.space.xxs)
            .p(t.space.xxs)
            .bg(c.surface_card)
            .rounded(t.radius.md);
        for (step, number, name, done_name) in steps {
            let active = step == current;
            let done = order(step) < order(current);
            let mut cell = div()
                .flex()
                .flex_1()
                .flex_basis(gpui_kit::relative(0.))
                .min_w_0()
                .items_center()
                .gap(t.space.sm)
                .py(t.space.xs)
                .px(t.space.sm + t.space.xxs)
                .rounded(t.radius.sm)
                .when(active, |d| d.bg(c.surface_selected));
            if done {
                cell = cell.child(icon("check", t.space.md, c.status_done)).child(
                    crate::ui::parts::line(done_name, t.text.ui, c.text_secondary),
                );
            } else {
                cell = cell
                    .child(
                        div()
                            .font_family(t.font_mono.clone())
                            .type_style(detail(t))
                            .text_color(if active {
                                c.text_primary
                            } else {
                                c.text_tertiary
                            })
                            .child(number),
                    )
                    .child(text(
                        name,
                        if active { t.text.ui_strong } else { t.text.ui },
                        if active {
                            c.text_primary
                        } else {
                            c.text_secondary
                        },
                    ));
            }
            bar = bar.child(cell);
        }
        bar
    }

    /// The footer: a note, a quiet way back and the main button.
    fn account_footer(
        &self,
        note: String,
        back: &str,
        next: &str,
        next_enabled: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let busy = self.in_flight.is_running("add-account");
        let step = self.add_account.step;
        let confirm: crate::app::DialogConfirm = std::rc::Rc::new(
            |this: &mut ConsortApp, window: &mut Window, cx: &mut Context<ConsortApp>| {
                this.add_account_next(window, cx)
            },
        );
        if next_enabled {
            self.dialog_confirm.replace(Some(confirm.clone()));
        }
        div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(text(note, detail(t), c.text_tertiary).flex_1().min_w_0())
            .child(
                button("account-back", back.to_owned(), ButtonKind::Secondary, t).on_click(
                    cx.listener(move |this, _, window, cx| {
                        if step == AccountStep::Name {
                            this.dialog_error = None;
                            this.add_account.step = AccountStep::Provider;
                            cx.notify();
                        } else {
                            this.close_dialog(window, cx);
                        }
                    }),
                ),
            )
            .child(
                button("account-next", next.to_owned(), ButtonKind::Primary, t)
                    .px(t.space.md + t.space.xxs)
                    .when(busy, |b| b.opacity(t.opacity.busy))
                    .on_click(cx.listener(move |this, _, window, cx| confirm(this, window, cx))),
            )
    }

    /// Step 1: the provider grid, the ACP registry and the custom command.
    fn provider_step(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Div, String) {
        let c = &t.colors;
        let chosen = self.add_account.provider.clone();
        let mut grid = div().flex().flex_col().gap(t.space.sm);
        for pair in PROVIDERS.chunks(2) {
            let mut row = div().flex().gap(t.space.sm);
            for provider in pair {
                let (harness, name, line, logo) = (
                    provider.harness,
                    provider.name,
                    provider.line,
                    &provider.logo,
                );
                let selected = chosen == Some(ProviderPick::Builtin(harness));
                let pick = ProviderPick::Builtin(harness);
                row = row.child(
                    focusable(
                        div().id(SharedString::from(format!("provider-{harness}"))),
                        t,
                    )
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .flex_basis(gpui_kit::relative(0.))
                    .gap(t.space.xxs)
                    .py(t.space.sm + t.space.xxs)
                    .px(t.space.md)
                    .rounded(t.radius.lg)
                    .border(t.border_width)
                    .cursor_pointer()
                    .when(selected, |d| {
                        d.bg(c.tint_accent).border_color(c.accent_text)
                    })
                    .when(!selected, |d| {
                        d.border_color(c.border_strong)
                            .hover(|s| s.bg(c.hover_raised))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(t.space.sm)
                                    .child(provider_logo(
                                        logo,
                                        t.component.provider_logo,
                                        c.text_primary,
                                    ))
                                    .child(text(name, t.text.ui_strong, c.text_primary)),
                            )
                            .when(selected, |d| d.child(chosen_mark(t))),
                    )
                    .child(text(line, detail(t), c.text_tertiary))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.add_account.provider = Some(pick.clone());
                        this.dialog_error = None;
                        cx.notify();
                    })),
                );
            }
            grid = grid.child(row);
        }
        let custom = chosen == Some(ProviderPick::Command);
        let search_focused = self.is_focused(&self.inputs.agent_search, window, cx);
        let search = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(t.space.sm)
            .py(t.space.xs + t.space.xxs)
            .px(t.space.sm + t.space.xxs)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(if search_focused {
                c.accent_text
            } else {
                c.border_strong
            })
            .rounded(t.radius.md)
            .child(icon("search", t.icon.dense, c.text_tertiary))
            .child(
                crate::ui::parts::input(&self.inputs.agent_search, t.text.ui)
                    .flex_1()
                    .min_w_0(),
            );
        let custom_button = button("custom-command", "Custom command", ButtonKind::Secondary, t)
            .when(custom, |b| b.border_color(c.accent_text))
            .on_click(cx.listener(|this, _, window, cx| {
                this.add_account.provider = Some(ProviderPick::Command);
                this.dialog_error = None;
                this.inputs
                    .program
                    .update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            }));
        let below: AnyElement = if custom {
            let program_focused = self.is_focused(&self.inputs.program, window, cx);
            let args_focused = self.is_focused(&self.inputs.program_args, window, cx);
            div()
                .flex()
                .gap(t.space.md)
                .child(
                    crate::ui::dialogs::field_block(
                        "Program",
                        input_box(&self.inputs.program, program_focused, t),
                        Some("The hub machine runs it as an ACP agent."),
                        t,
                    )
                    .flex_1()
                    .min_w_0(),
                )
                .child(
                    crate::ui::dialogs::field_block(
                        "Arguments",
                        input_box(&self.inputs.program_args, args_focused, t),
                        Some("Separate them with spaces."),
                        t,
                    )
                    .flex_1()
                    .min_w_0(),
                )
                .into_any_element()
        } else {
            self.registry_list(t, cx).into_any_element()
        };
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.lg + t.space.xxs)
            .child(self.steps_bar(t))
            .child(grid)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.sm + t.space.xxs)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.md)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h(t.border_width)
                                    .bg(c.border_strong),
                            )
                            .child(text(
                                "Or add an agent from the ACP registry",
                                detail(t),
                                c.text_tertiary,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h(t.border_width)
                                    .bg(c.border_strong),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(t.space.sm)
                            .child(search)
                            .child(custom_button),
                    )
                    .child(below),
            );
        let (name, _) = self.chosen_provider();
        let note = match &chosen {
            Some(ProviderPick::Builtin(_)) => {
                format!("Consort installs {name} on the hub machine and signs in.")
            }
            Some(ProviderPick::Registry { .. }) => {
                format!(
                    "Consort installs {name} from the ACP registry on the hub machine and signs in."
                )
            }
            Some(ProviderPick::Command) => {
                "The hub machine runs this command as it is. Consort does not install it."
                    .to_owned()
            }
            None => "Choose a provider.".to_owned(),
        };
        (body, note)
    }

    /// The agents of the ACP registry that match the search. An agent that a
    /// worker cannot install shows why, and cannot be chosen.
    fn registry_list(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let registry = self.state.acp_registry.clone();
        match &registry.load {
            RegistryLoad::NotLoaded | RegistryLoad::Loading => {
                return div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.sm)
                    .child(status_glyph(
                        "registry-loading",
                        consort_client::TaskStatus::Working,
                        t.icon.size,
                        t,
                        cx,
                    ))
                    .child(text(
                        "Loading the ACP registry",
                        t.text.ui,
                        c.text_secondary,
                    ))
                    .into_any_element();
            }
            RegistryLoad::Failed(message) => {
                return div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.sm)
                    .child(icon("circle-alert", t.icon.size, c.status_failed))
                    .child(
                        selectable(
                            "registry-error",
                            message.clone(),
                            detail(t),
                            c.text_secondary,
                        )
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        button("registry-retry", "Try again", ButtonKind::Secondary, t).on_click(
                            cx.listener(|this, _, _, cx| {
                                let client = this.client.clone();
                                this.spawn_command(
                                    cx,
                                    async move { client.load_acp_registry().await },
                                );
                            }),
                        ),
                    )
                    .into_any_element();
            }
            RegistryLoad::Loaded => {}
        }
        let query = self.inputs.agent_search.read(cx).value().to_string();
        let found = registry.search(&query);
        if found.is_empty() {
            return text(
                "No agent in the ACP registry matches. Try another word, or use a custom command.",
                t.text.ui,
                c.text_secondary,
            )
            .py(t.space.sm)
            .into_any_element();
        }
        let chosen = self.add_account.provider.clone();
        let mut list = div()
            .id("registry-list")
            .flex()
            .flex_col()
            .max_h(t.component.agent_list_max)
            .overflow_y_scroll();
        let last = found.len().saturating_sub(1);
        for (i, agent) in found.into_iter().enumerate() {
            let selected =
                matches!(&chosen, Some(ProviderPick::Registry { id, .. }) if *id == agent.id);
            let pick = ProviderPick::Registry {
                id: agent.id.clone(),
                name: agent.name.clone(),
            };
            let supported = agent.supported;
            let trailing: AnyElement = if selected {
                chosen_mark(t).into_any_element()
            } else if supported {
                text("Choose", t.text.ui, c.text_secondary).into_any_element()
            } else {
                text("Not available", t.text.ui, c.text_tertiary).into_any_element()
            };
            let description = if supported {
                agent.description.clone()
            } else {
                "Consort cannot install this agent yet. It needs a download that the hub machine cannot check.".to_owned()
            };
            list = list.child(
                div()
                    .id(SharedString::from(format!("agent-{}", agent.id)))
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .py(t.space.sm)
                    .when(i < last, |d| {
                        d.border_b(t.border_width).border_color(c.border_subtle)
                    })
                    .when(!supported, |d| d.opacity(t.opacity.answered_dim))
                    .when(supported, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(c.hover_raised))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.add_account.provider = Some(pick.clone());
                                this.dialog_error = None;
                                cx.notify();
                            }))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(t.component.agent_tile)
                            .rounded(t.radius.md)
                            .bg(c.surface_card)
                            .border(t.border_width)
                            .border_color(c.border_subtle)
                            .child(provider_logo(&agent.logo, t.icon.size, c.text_primary)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(text(agent.name.clone(), t.text.ui_strong, c.text_primary))
                            .child(crate::ui::parts::line(
                                description,
                                detail(t),
                                c.text_tertiary,
                            )),
                    )
                    .child(div().flex_none().child(trailing)),
            );
        }
        list.into_any_element()
    }

    /// Step 2: the account name and the profile folder.
    fn name_step(&mut self, t: &Tokens, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let (_, logo) = self.chosen_provider();
        let focused = self.is_focused(&self.inputs.account_name, window, cx);
        let name = self.field_text(&self.inputs.account_name, cx);
        let field = div()
            .flex()
            .items_center()
            .gap(t.space.sm + t.space.xxs)
            .py(t.space.xs + t.space.xxs)
            .px(t.space.sm + t.space.xxs)
            .bg(c.surface_canvas)
            .border(t.border_width)
            .border_color(if focused {
                c.accent_text
            } else {
                c.border_strong
            })
            .rounded(t.radius.md)
            .child(provider_logo(&logo, t.icon.size, c.text_primary))
            .child(
                crate::ui::parts::input(&self.inputs.account_name, t.text.body)
                    .flex_1()
                    .min_w_0(),
            );
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.lg + t.space.xxs)
            .child(self.steps_bar(t))
            .child(crate::ui::dialogs::field_block(
                "Account name",
                field,
                Some("Shown in the model picker and on task cards. Say whose login it is, such as Personal or Work."),
                t,
            ));
        let windows = self
            .state
            .worker(&self.add_account.worker_id)
            .is_some_and(|w| w.os == consort_client::Os::Windows);
        let slug = profile_slug(&name);
        let folder = if windows {
            format!(r"%USERPROFILE%\.consort\profiles\{slug}")
        } else {
            format!("~/.consort/profiles/{slug}")
        };
        body.child(
            div()
                .flex()
                .items_center()
                .gap(t.space.sm + t.space.xxs)
                .child(icon("folder", t.icon.dense, c.text_tertiary))
                .child(text(
                    "Profile folder on the hub machine:".to_owned(),
                    detail(t),
                    c.text_tertiary,
                ))
                .child(
                    selectable("profile-folder", folder, detail(t), c.text_secondary)
                        .font_family(t.font_mono.clone()),
                ),
        )
    }

    /// Step 3: the method tabs, then the flow of the sign-in.
    /// The command line tool of the provider that the dialog signs in to.
    fn sign_in_tool(&self) -> Option<consort_client::ToolView> {
        let (provider, _) = self.chosen_provider();
        let host = self.state.session_host.as_ref()?;
        consort_client::provider_sections(host)
            .into_iter()
            .find(|section| section.name == provider)
            .and_then(|section| section.tool)
    }

    /// The install in step 3, while the tool of the provider is not ready.
    /// The user can close the dialog, and the install goes on.
    fn install_block(&self, provider: &str, t: &Tokens, cx: &mut Context<Self>) -> Option<Div> {
        let c = &t.colors;
        let tool = self.sign_in_tool()?;
        let host = self.state.session_host.as_ref()?.id.clone();
        match tool.state {
            consort_client::ToolState::Failed => {
                let tool_id = tool.id.clone();
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(t.space.sm)
                        .debug_selector(|| "install-failed".into())
                        .child(error_box(
                            "install-error",
                            format!("{provider} did not install. Check that the hub machine can reach the network, then try again."),
                            t,
                        ))
                        .when_some(tool.detail.clone(), |d, details| {
                            d.child(selectable(
                                "install-details",
                                details,
                                detail(t),
                                c.text_secondary,
                            ))
                        })
                        .child(
                            button("install-again", "Try again", ButtonKind::Secondary, t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let client = this.client.clone();
                                    let (w, id) = (host.clone(), tool_id.clone());
                                    this.spawn_command(cx, async move {
                                        client.install_tool(w, id).await
                                    });
                                })),
                        ),
                )
            }
            consort_client::ToolState::Installing | consort_client::ToolState::Missing => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .debug_selector(|| "install-running".into())
                    .child(status_glyph(
                        "install-running",
                        consort_client::TaskStatus::Working,
                        t.icon.dense,
                        t,
                        cx,
                    ))
                    .child(
                        text(
                            format!("Installing {provider} on the hub machine. You can close this dialog. The install goes on."),
                            t.text.ui,
                            c.text_secondary,
                        )
                        .flex_1()
                        .min_w_0(),
                    ),
            ),
            consort_client::ToolState::Ready | consort_client::ToolState::UpdateAvailable => None,
        }
    }

    fn sign_in_step(
        &mut self,
        worker: &str,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let c = &t.colors;
        let account = self.dialog_account();
        let (provider, logo) = match &account {
            Some(a) => (a.provider.clone(), a.logo.clone()),
            None => self.chosen_provider(),
        };
        let label = account
            .as_ref()
            .map(|a| a.label.clone())
            .unwrap_or_else(|| self.field_text(&self.inputs.account_name, cx));
        // The worker reported the account after the dialog made it, or its
        // tool is ready now: start the sign-in.
        self.auto_start_sign_in(window, cx);
        let method = account
            .as_ref()
            .and_then(|a| a.sign_in_methods.get(self.add_account.method).cloned());
        let lead = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(t.component.sign_in_tile)
            .rounded(t.radius.lg)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_strong)
            .child(provider_logo(
                &logo,
                t.component.sign_in_logo,
                c.text_primary,
            ))
            .into_any_element();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(t.space.lg + t.space.xxs)
            .child(self.steps_bar(t));
        if let Some(block) = self.install_block(&provider, t, cx) {
            body = body.child(block);
        }
        if let Some(tabs) = self.method_tabs(t, cx) {
            body = body.child(crate::ui::dialogs::field_block(
                "Sign in with",
                tabs,
                None,
                t,
            ));
        }
        let step = self.running_sign_in_step();
        body = body.child(self.sign_in_flow_card(t, window, cx));
        let done = self.add_account.key_saved
            || matches!(step, Some(SignInStep::Succeeded { .. }))
            || matches!(method, Some(SignInChoice::Terminal { .. }));
        let note = match (&method, &step) {
            (_, Some(SignInStep::NeedsCode { .. }) | Some(SignInStep::DeviceCode { .. })) => {
                "The code works once. If it expires, start again.".to_owned()
            }
            (Some(SignInChoice::ApiKey { .. }), _) if !self.add_account.key_saved => {
                format!("Consort sends the key to {worker} once and does not keep it.")
            }
            _ => format!("The account stays on {worker} when you close this dialog."),
        };
        let confirm_done: crate::app::DialogConfirm = std::rc::Rc::new(
            |this: &mut ConsortApp, window: &mut Window, cx: &mut Context<ConsortApp>| {
                this.close_dialog(window, cx)
            },
        );
        if done {
            self.dialog_confirm.replace(Some(confirm_done.clone()));
        }
        let finish = if done {
            button("sign-in-done", "Done", ButtonKind::Primary, t)
                .px(t.space.md + t.space.xxs)
                .on_click(cx.listener(move |this, _, window, cx| confirm_done(this, window, cx)))
        } else {
            button("sign-in-done", "Done", ButtonKind::Quiet, t)
                .px(t.space.md + t.space.xxs)
                .border(t.border_width)
                .border_color(c.border_subtle)
                .text_color(c.text_tertiary)
                .cursor_default()
        };
        let actions = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(text(note, detail(t), c.text_tertiary).flex_1().min_w_0())
            .when(!done, |d| {
                d.child(
                    button("sign-in-cancel", "Cancel", ButtonKind::Secondary, t)
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                )
            })
            .child(finish);
        (
            format!("Sign in to {label}"),
            format!(
                "The hub machine runs {provider}'s own sign-in. The login stays in this account's profile folder on {worker}."
            ),
            t.layout.dialog_account,
            body,
            actions,
            Some(lead),
        )
    }

    /// The tabs of the ways to sign in, when the account has more than one.
    /// The dialog and the sign-in screen share them.
    pub(crate) fn method_tabs(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Option<Div> {
        let methods = self.dialog_account()?.sign_in_methods;
        if methods.len() < 2 {
            return None;
        }
        let options: Vec<(usize, String)> = methods
            .iter()
            .enumerate()
            .map(|(i, m)| (i, m.label()))
            .collect();
        let options: Vec<(usize, &str)> = options.iter().map(|(i, l)| (*i, l.as_str())).collect();
        Some(segmented(
            "sign-in-method",
            &options,
            self.add_account.method,
            t,
            cx,
            |_| t.text.ui,
            |this, value, window, cx| {
                if this.add_account.method != value {
                    this.add_account.method = value;
                    this.add_account.key_saved = false;
                    this.start_sign_in_method(window, cx);
                }
            },
        ))
    }

    /// The flow of the chosen method. The dialog and the sign-in screen share
    /// it.
    pub(crate) fn sign_in_flow_card(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let account = self.dialog_account();
        let provider = match &account {
            Some(a) => a.provider.clone(),
            None => self.chosen_provider().0,
        };
        let label = account
            .as_ref()
            .map(|a| a.label.clone())
            .unwrap_or_else(|| self.field_text(&self.inputs.account_name, cx));
        let method = account
            .as_ref()
            .and_then(|a| a.sign_in_methods.get(self.add_account.method).cloned());
        let step = self.running_sign_in_step();
        self.flow_card(
            "the hub machine",
            &provider,
            &label,
            method.as_ref(),
            step.as_ref(),
            account.is_some(),
            t,
            window,
            cx,
        )
    }

    /// The card with the flow of the chosen method, at the step that the
    /// worker reported.
    #[allow(clippy::too_many_arguments)]
    fn flow_card(
        &mut self,
        worker: &str,
        provider: &str,
        label: &str,
        method: Option<&SignInChoice>,
        step: Option<&SignInStep>,
        account_known: bool,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let card = div()
            .flex()
            .flex_col()
            .gap(t.space.lg)
            .p(t.space.lg)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .rounded(t.radius.lg);
        if !account_known {
            return card.child(waiting_row(
                format!("Waiting for {worker} to set up the account"),
                t,
                cx,
            ));
        }
        let Some(method) = method else {
            return card.child(text(
                format!("The hub machine offers no way to sign in to this account from the app. Check the setup of {provider} on {worker}."),
                t.text.ui,
                c.text_secondary,
            ));
        };
        match method {
            SignInChoice::ApiKey { variable } => {
                if self.add_account.key_saved {
                    return card.child(flow_step(
                        Marker::Done,
                        format!("API key saved on {worker}."),
                        Some(format!("Tasks on {worker} can use {label} now.")),
                        None,
                        t,
                    ));
                }
                let focused = self.is_focused(&self.inputs.api_key, window, cx);
                let variable = variable.clone();
                let save_variable = variable.clone();
                let save: crate::app::DialogConfirm = std::rc::Rc::new(
                    move |this: &mut ConsortApp,
                          window: &mut Window,
                          cx: &mut Context<ConsortApp>| {
                        this.save_api_key(save_variable.clone(), window, cx)
                    },
                );
                self.dialog_confirm.replace(Some(save.clone()));
                let row = div()
                    .flex()
                    .gap(t.space.sm)
                    .child(
                        code_field(&self.inputs.api_key, focused, t)
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        button("save-key", "Save", ButtonKind::Primary, t)
                            .px(t.space.md + t.space.xxs)
                            .when(self.in_flight.is_running("save-key"), |b| {
                                b.opacity(t.opacity.busy)
                            })
                            .on_click(
                                cx.listener(move |this, _, window, cx| save(this, window, cx)),
                            ),
                    );
                card.child(flow_step(
                    Marker::Number("1"),
                    "Paste the API key".to_owned(),
                    Some(format!("The hub machine saves it as {variable} in this account's profile folder. The hub does not keep it.")),
                    Some(row.into_any_element()),
                    t,
                ))
            }
            SignInChoice::Terminal { command } => {
                let copy = command.clone();
                let block = div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.sm)
                    .pl(t.space.md)
                    .pr(t.space.xs)
                    .bg(c.surface_canvas)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .rounded(t.radius.md)
                    .child(
                        selectable(
                            "terminal-command",
                            command.clone(),
                            t.text.mono,
                            c.text_primary,
                        )
                        .font_family(t.font_mono.clone())
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        crate::ui::parts::icon_button(
                            "copy-terminal-command",
                            "copy",
                            t.space.xl + t.space.xs,
                            t.icon.size,
                            c.text_secondary,
                            c.hover_card,
                            t,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            this.show_notice("Copied the command.".into(), cx);
                        })),
                    );
                card.child(flow_step(
                    Marker::Number("1"),
                    format!("Run this command on {worker}"),
                    Some(format!(
                        "{provider} signs in only in a terminal. Run the command on {worker}. {}",
                        if self.dialog.is_none() {
                            "This screen goes on by itself when the login works."
                        } else {
                            "Then close this dialog. The account row shows when the login works."
                        }
                    )),
                    Some(block.into_any_element()),
                    t,
                ))
            }
            _ => self.flow_steps(worker, provider, label, step, card, t, window, cx),
        }
    }

    /// The flow of a method that runs on the worker.
    #[allow(clippy::too_many_arguments)]
    fn flow_steps(
        &mut self,
        worker: &str,
        provider: &str,
        label: &str,
        step: Option<&SignInStep>,
        card: Div,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let open_buttons = |url: &str,
                            copy_label: &'static str,
                            copy_value: String,
                            cx: &mut Context<Self>| {
            let open = url.to_owned();
            div()
                .flex()
                .gap(t.space.sm)
                .child(
                    button("open-sign-in", "Open sign-in page", ButtonKind::Secondary, t)
                        .child(icon("external-link", t.icon.dense, c.text_primary))
                        .flex_row_reverse()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if is_web_url(&open) {
                                cx.open_url(&open);
                            } else {
                                this.show_error(
                                    "The worker sent a sign-in address that is not a web page, so Consort did not open it. Try again.".into(),
                                    cx,
                                );
                            }
                        })),
                )
                .child(
                    button(
                        SharedString::from(format!("copy-{copy_label}")),
                        copy_label,
                        ButtonKind::Quiet,
                        t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_value.clone()));
                        let what = if copy_label == "Copy code" { "code" } else { "link" };
                        this.show_notice(format!("Copied the {what}."), cx);
                    })),
                )
        };
        let goes_on = if self.dialog.is_none() {
            "This screen goes on by itself."
        } else {
            "The dialog goes on by itself."
        };
        let retry = |cx: &mut Context<Self>| {
            button("sign-in-retry", "Try again", ButtonKind::Secondary, t)
                .on_click(cx.listener(|this, _, window, cx| this.start_sign_in_method(window, cx)))
        };
        match step {
            None | Some(SignInStep::Starting) => card.child(waiting_row(
                format!("Starting the sign-in on {worker}"),
                t,
                cx,
            )),
            Some(SignInStep::NeedsCode { url }) => {
                let focused = self.is_focused(&self.inputs.sign_in_code, window, cx);
                let send: crate::app::DialogConfirm = std::rc::Rc::new(
                    |this: &mut ConsortApp, _window: &mut Window, cx: &mut Context<ConsortApp>| {
                        this.send_sign_in_code(cx)
                    },
                );
                self.dialog_confirm.replace(Some(send.clone()));
                let row = div()
                    .flex()
                    .gap(t.space.sm)
                    .child(
                        code_field(&self.inputs.sign_in_code, focused, t)
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        button("send-code", "Send code", ButtonKind::Primary, t)
                            .px(t.space.md + t.space.xxs)
                            .when(self.in_flight.is_running("send-code"), |b| {
                                b.opacity(t.opacity.busy)
                            })
                            .on_click(
                                cx.listener(move |this, _, window, cx| send(this, window, cx)),
                            ),
                    );
                let page = consort_client::setup::provider_short_name(provider).to_owned();
                card.child(flow_step(
                    Marker::Done,
                    format!("Open {page}'s sign-in page"),
                    Some(if self.dialog.is_none() {
                        format!("The hub machine started the sign-in of {provider}. Sign in with the {page} account that you want the assistant to use.")
                    } else {
                        format!("The hub machine started the sign-in of {provider} for this account. Sign in with the login that {label} stands for.")
                    }),
                    Some(open_buttons(url, "Copy link", url.clone(), cx).into_any_element()),
                    t,
                ))
                .child(flow_step(
                    Marker::Number("2"),
                    format!("Paste the code that {page} shows"),
                    Some(format!("Consort passes it to {provider} on {worker} and does not keep it.")),
                    Some(row.into_any_element()),
                    t,
                ))
            }
            Some(SignInStep::DeviceCode { url, user_code }) => {
                let code = div()
                    .flex()
                    .items_center()
                    .py(t.space.md)
                    .px(t.space.lg)
                    .bg(c.surface_canvas)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .rounded(t.radius.md)
                    .child(
                        div()
                            .font_family(t.font_mono.clone())
                            .text_size(t.text.display.size)
                            .line_height(t.text.display.line)
                            .text_color(c.text_primary)
                            .child(gpui_kit::base::SelectableText::new(
                                "device-code",
                                user_code.clone(),
                            )),
                    );
                card.child(flow_step(
                    Marker::Number("1"),
                    "Enter this code on the sign-in page".to_owned(),
                    Some(format!(
                        "{provider} on {worker} waits until you sign in. {goes_on}"
                    )),
                    Some(
                        div()
                            .flex()
                            .flex_col()
                            .gap(t.space.sm)
                            .child(code)
                            .child(open_buttons(url, "Copy code", user_code.clone(), cx))
                            .into_any_element(),
                    ),
                    t,
                ))
            }
            Some(SignInStep::OpenUrl { url }) => card.child(flow_step(
                Marker::Number("1"),
                "Open the sign-in page".to_owned(),
                Some(format!(
                    "Sign in on the page. {provider} on {worker} waits. {goes_on}"
                )),
                Some(open_buttons(url, "Copy link", url.clone(), cx).into_any_element()),
                t,
            )),
            Some(SignInStep::Verifying) => card.child(waiting_row(
                format!("Checking the sign-in on {worker}"),
                t,
                cx,
            )),
            Some(SignInStep::Succeeded { detail: who }) => card.child(flow_step(
                Marker::Done,
                match who {
                    Some(who) => format!("Signed in as {who}"),
                    None => "Signed in".to_owned(),
                },
                Some(format!("Tasks on {worker} can use {label} now.")),
                None,
                t,
            )),
            Some(SignInStep::Failed { message }) => card.child(flow_step(
                Marker::Failed,
                "The sign-in failed".to_owned(),
                Some(message.clone()),
                Some(retry(cx).into_any_element()),
                t,
            )),
            Some(SignInStep::Cancelled) => card.child(flow_step(
                Marker::Failed,
                "The sign-in stopped".to_owned(),
                Some("It was cancelled. Start it again to sign in.".to_owned()),
                Some(retry(cx).into_any_element()),
                t,
            )),
            Some(SignInStep::Expired) => card.child(flow_step(
                Marker::Failed,
                "The sign-in expired".to_owned(),
                Some("Nothing happened for 15 minutes. Start it again to sign in.".to_owned()),
                Some(retry(cx).into_any_element()),
                t,
            )),
        }
    }

    /// Sends the code of the running sign-in.
    fn send_sign_in_code(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.add_account.sign_in_id.clone() else {
            return;
        };
        let code = self.field_text(&self.inputs.sign_in_code, cx);
        if code.is_empty() {
            self.dialog_error = Some("Paste the code that the sign-in page shows.".into());
            cx.notify();
            return;
        }
        if !self.in_flight.begin("send-code") {
            return;
        }
        self.dialog_error = None;
        self.add_account.code_sent = true;
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client.submit_sign_in_code(id, code).await;
            let _ = this.update(cx, |this, cx| {
                this.in_flight.finish("send-code");
                if let Err(err) = result {
                    this.dialog_error = Some(err.to_string());
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Saves an API key in the account's secret variables.
    fn save_api_key(&mut self, variable: String, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.field_text(&self.inputs.api_key, cx);
        if key.is_empty() {
            self.dialog_error = Some("Paste the API key, then click Save.".into());
            cx.notify();
            return;
        }
        let Some(account_id) = self.add_account.account_id.clone() else {
            return;
        };
        if !self.in_flight.begin("save-key") {
            return;
        }
        self.dialog_error = None;
        let mut env = SecretEnv::new();
        env.insert(variable, key);
        let client = self.client.clone();
        let worker_id = self.add_account.worker_id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = client.set_account_secrets(worker_id, account_id, env).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.in_flight.finish("save-key");
                match result {
                    Ok(()) => {
                        this.add_account.key_saved = true;
                        this.inputs
                            .api_key
                            .update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    Err(err) => this.dialog_error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// The accent circle with a check that marks the chosen provider.
fn chosen_mark(t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(t.icon.size)
        .rounded(t.radius.full)
        .bg(c.accent_fill)
        .child(icon("check", t.space.sm + t.space.xxs, c.accent_on_fill))
}

/// The marker of a step in a sign-in flow.
enum Marker {
    Done,
    Number(&'static str),
    Failed,
}

/// One step of a sign-in flow: a marker, a title, a line and the controls.
fn flow_step(
    marker: Marker,
    title: String,
    line: Option<String>,
    controls: Option<AnyElement>,
    t: &Tokens,
) -> Div {
    let c = &t.colors;
    let base = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(t.component.step_marker)
        .rounded(t.radius.full);
    let marker = match marker {
        Marker::Done => base
            .bg(c.status_done)
            .child(icon("check", t.space.md, c.surface_canvas)),
        Marker::Number(n) => base
            .border(t.border_width)
            .border_color(c.accent_text)
            .child(text(n, t.text.label, c.accent_text)),
        Marker::Failed => base.child(icon(
            "circle-alert",
            t.component.step_marker,
            c.status_failed,
        )),
    };
    let id = SharedString::from(format!("flow-line-{title}"));
    div().flex().gap(t.space.md).child(marker).child(
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
                    .when_some(line, |d, line| {
                        d.child(selectable(id, line, detail(t), c.text_tertiary))
                    }),
            )
            .children(controls),
    )
}

/// A spinner and a line, while the worker works.
fn waiting_row(line: String, t: &Tokens, cx: &mut Context<ConsortApp>) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .items_center()
        .gap(t.space.md)
        .child(status_glyph(
            "sign-in-waiting",
            consort_client::TaskStatus::Working,
            t.icon.size,
            t,
            cx,
        ))
        .child(text(line, t.text.ui, c.text_secondary))
}

/// A one-line field for a code or a key: the canvas ground, mono text, and
/// the accent border while it has the focus.
fn code_field(input: &Entity<InputState>, focused: bool, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .py(t.component.button_pad_y)
        .px(t.space.sm + t.space.xxs)
        .bg(c.surface_canvas)
        .border(t.border_width)
        .border_color(if focused {
            c.accent_text
        } else {
            c.border_strong
        })
        .rounded(t.radius.md)
        .font_family(t.font_mono.clone())
        .child(crate::ui::parts::input(input, t.text.mono))
}
