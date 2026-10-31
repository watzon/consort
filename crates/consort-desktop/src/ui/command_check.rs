//! The API endpoints of Settings > Providers and the command check of
//! Settings > Models (designs P1, P2 and P3).

use consort_client::{
    CONFIDENCE_LIMITS, CheckEndpointChoice, CheckSourceKind, Command, CommandCheckChoice,
    CommandOutput, EndpointView, TestResultView,
};
use consort_theme::Tokens;
use gpui_kit::{
    AppContext as _, Context, Div, Entity, Focusable as _, InteractiveElement as _, MouseButton,
    MouseDownEvent, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window,
    component::input::{InputEvent, InputState},
    div,
    prelude::FluentBuilder as _,
};

use crate::app::{ConsortApp, Dialog, SettingsSection};
use crate::ui::admin_parts::{hint, row, row_label, rows, section_title, select};
use crate::ui::dialogs::{field_block, input_box};
use crate::ui::menu::{OpenMenu, endpoint_menu};
use crate::ui::model_picker::{PickerTarget, TriggerStyle};
use crate::ui::parts::{ButtonKind, button, detail, error_box, selectable, status_glyph, text};
use crate::ui::settings_providers::dots_button;
use crate::ui::workers::{row_button, row_list};

pub const CHECK_SOURCE_SELECT: &str = "check-source";
pub const CHECK_ENDPOINT_SELECT: &str = "check-endpoint";
pub const CHECK_LIMIT_SELECT: &str = "check-limit";

/// The fields of the endpoint dialog and the model names of the check.
pub struct CheckInputs {
    pub endpoint_name: Entity<InputState>,
    pub endpoint_url: Entity<InputState>,
    pub endpoint_key: Entity<InputState>,
    pub decision_model: Entity<InputState>,
    pub chat_model: Entity<InputState>,
}

impl CheckInputs {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<ConsortApp>,
    ) -> (CheckInputs, Vec<Subscription>) {
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<ConsortApp>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let inputs = CheckInputs {
            endpoint_name: field("OpenRouter", window, cx),
            endpoint_url: field("https://openrouter.ai/api", window, cx),
            endpoint_key: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Paste the key")
                    .masked(true)
            }),
            decision_model: field("typesafe/jev-1.14", window, cx),
            chat_model: field("openai/gpt-5.5-mini", window, cx),
        };
        // A model name saves when the user leaves the field or presses Enter.
        let mut subscriptions = Vec::new();
        for input in [&inputs.decision_model, &inputs.chat_model] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        this.save_check_model_names(cx);
                    }
                },
            ));
        }
        for input in [
            &inputs.endpoint_name,
            &inputs.endpoint_url,
            &inputs.endpoint_key,
        ] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit_on_enter(window, cx);
                    }
                },
            ));
        }
        (inputs, subscriptions)
    }

    pub fn all(&self) -> [&Entity<InputState>; 5] {
        [
            &self.endpoint_name,
            &self.endpoint_url,
            &self.endpoint_key,
            &self.decision_model,
            &self.chat_model,
        ]
    }
}

/// What the two pages keep while the user works.
#[derive(Default)]
pub struct CheckUi {
    pub testing: bool,
    pub test: Option<TestResultView>,
    pub endpoint_testing: bool,
    pub endpoint_test: Option<TestResultView>,
}

/// The state of an endpoint row.
pub fn endpoint_state(endpoint: &EndpointView) -> &'static str {
    if endpoint.in_use {
        "In use"
    } else {
        "Not used"
    }
}

/// The choice on an endpoint for a kind. A new one takes the first endpoint.
fn choice_of<'a>(
    choice: &'a mut CommandCheckChoice,
    kind: CheckSourceKind,
    first_endpoint: Option<&str>,
) -> Option<&'a mut CheckEndpointChoice> {
    let slot = match kind {
        CheckSourceKind::Account => return None,
        CheckSourceKind::Decision => &mut choice.decision,
        CheckSourceKind::Chat => &mut choice.chat,
    };
    Some(slot.get_or_insert_with(|| CheckEndpointChoice {
        endpoint_id: first_endpoint.unwrap_or_default().to_owned(),
        model: String::new(),
        confidence_limit: consort_client::DEFAULT_CONFIDENCE_LIMIT,
    }))
}

/// Applies a pick of a select of the command check to `choice`. False when
/// the pick changes nothing.
pub fn apply_check_pick(
    choice: &mut CommandCheckChoice,
    select: &str,
    option: &str,
    first_endpoint: Option<&str>,
) -> bool {
    let before = choice.clone();
    let kind = choice.kind;
    match select {
        CHECK_SOURCE_SELECT => {
            if let Some(kind) = CheckSourceKind::ALL.iter().find(|k| k.label() == option) {
                choice.kind = *kind;
                choice_of(choice, *kind, first_endpoint);
            }
        }
        CHECK_ENDPOINT_SELECT => {
            if let Some(slot) = choice_of(choice, kind, first_endpoint) {
                option.clone_into(&mut slot.endpoint_id);
            }
        }
        CHECK_LIMIT_SELECT => {
            if let (Ok(limit), Some(slot)) = (
                option.parse::<u8>(),
                choice_of(choice, kind, first_endpoint),
            ) {
                slot.confidence_limit = limit;
            }
        }
        _ => return false,
    }
    *choice != before
}

impl ConsortApp {
    /// The section at the end of Settings > Providers.
    pub(crate) fn endpoints_section(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let endpoints = self.state.endpoints.clone();
        let add = row_button("add-endpoint", "Add endpoint".to_owned(), t)
            .debug_selector(|| "add-endpoint".into())
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_dialog(Dialog::Endpoint(None), window, cx)
            }));
        let mut list = row_list(t);
        let count = endpoints.len();
        if count == 0 {
            list = list.child(
                div().py(t.space.md).px(t.space.lg).child(hint(
                    "No endpoint yet. An endpoint is a base URL and a key for the command check. It cannot run a task.",
                    t,
                )),
            );
        }
        for (index, endpoint) in endpoints.iter().enumerate() {
            list = list.child(self.endpoint_row(endpoint, index + 1 == count, t, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(text(
                        "API endpoints".to_owned(),
                        t.text.title,
                        t.colors.text_primary,
                    ))
                    .child(add),
            )
            .child(list)
    }

    fn endpoint_row(
        &mut self,
        endpoint: &EndpointView,
        last: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let c = &t.colors;
        let items = endpoint_menu(&endpoint.id, &endpoint.base_url);
        let right_items = items.clone();
        crate::ui::workers::list_row(last, t)
            .id(SharedString::from(format!("endpoint-row-{}", endpoint.id)))
            .debug_selector({
                let id = format!("endpoint-row-{}", endpoint.id);
                move || id.clone()
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_menu(OpenMenu::at(event.position, right_items.clone()), cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(text(
                        endpoint.name.clone(),
                        t.text.ui_strong,
                        c.text_primary,
                    ))
                    .child(selectable(
                        gpui_kit::ElementId::Name(format!("endpoint-url-{}", endpoint.id).into()),
                        endpoint.base_url.clone(),
                        detail(t),
                        c.text_tertiary,
                    )),
            )
            .child(
                text(
                    format!("Key ending {}", endpoint.key_last_four),
                    detail(t),
                    c.text_tertiary,
                )
                .flex_none(),
            )
            .child(
                text(
                    endpoint_state(endpoint),
                    detail(t),
                    if endpoint.in_use {
                        c.text_secondary
                    } else {
                        c.text_tertiary
                    },
                )
                .flex_none()
                .w(t.component.account_state),
            )
            .child(dots_button(
                format!("endpoint-dots-{}", endpoint.id),
                items,
                t,
                cx,
            ))
    }

    /// Fills the fields of the endpoint dialog.
    pub(crate) fn prepare_endpoint_dialog(
        &mut self,
        id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.check_ui.endpoint_test = None;
        self.check_ui.endpoint_testing = false;
        let endpoint = id.and_then(|id| self.state.endpoints.iter().find(|e| e.id == id));
        let (name, url) =
            endpoint.map_or_else(Default::default, |e| (e.name.clone(), e.base_url.clone()));
        self.check_inputs
            .endpoint_name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.check_inputs
            .endpoint_url
            .update(cx, |input, cx| input.set_value(url, window, cx));
        self.check_inputs
            .endpoint_key
            .update(cx, |input, cx| input.set_value("", window, cx));
        let name = self.check_inputs.endpoint_name.read(cx).focus_handle(cx);
        window.focus(&name, cx);
    }

    /// The dialog "Add an endpoint". Save does not wait for a test, because
    /// an endpoint can be down for a short time.
    pub(crate) fn endpoint_dialog(
        &mut self,
        id: Option<String>,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (String, String, gpui_kit::Pixels, Div, Div) {
        let field = |input: &Entity<InputState>, window: &Window, cx: &Context<Self>| {
            input_box(input, input.read(cx).focus_handle(cx).is_focused(window), t)
        };
        let name = field(&self.check_inputs.endpoint_name, window, cx);
        let url = field(&self.check_inputs.endpoint_url, window, cx);
        let key = field(&self.check_inputs.endpoint_key, window, cx);
        let editing = id.is_some();
        let test_id = id.clone();
        let test_line: Option<Div> = if self.check_ui.endpoint_testing {
            Some(hint("Testing the endpoint.", t))
        } else {
            self.check_ui
                .endpoint_test
                .as_ref()
                .map(|result| test_result_lines("endpoint-test", result, t))
        };
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(field_block("Name", name, None, t))
            .child(field_block("Base URL", url, None, t))
            .child(field_block(
                "API key",
                key,
                Some(if editing {
                    "Leave this empty to keep the key. The app cannot read the key again."
                } else {
                    "After you save, the app shows only its last four characters."
                }),
                t,
            ))
            .when_some(test_line, |d, line| d.child(line));
        let test = button("endpoint-test", "Test", ButtonKind::Secondary, t)
            .debug_selector(|| "endpoint-test".into())
            .on_click(
                cx.listener(move |this, _, _, cx| this.test_endpoint_form(test_id.clone(), cx)),
            );
        let actions = self.dialog_actions("Save", t, cx, move |this, window, cx| {
            let client = this.client.clone();
            let id = id.clone();
            let name = this.field_text(&this.check_inputs.endpoint_name, cx);
            let url = this.field_text(&this.check_inputs.endpoint_url, cx);
            let key = this.field_text(&this.check_inputs.endpoint_key, cx);
            let key = (!key.is_empty()).then_some(key);
            this.submit_dialog(
                async move { client.save_endpoint(id, name, url, key).await },
                window,
                cx,
            );
        });
        (
            if editing {
                "Edit the endpoint".to_owned()
            } else {
                "Add an endpoint".to_owned()
            },
            "The command check uses it. The key stays on the hub.".to_owned(),
            t.layout.dialog_width,
            body,
            div()
                .flex()
                .flex_col()
                .gap(t.space.sm)
                .child(div().flex().justify_start().child(test))
                .child(actions),
        )
    }

    fn test_endpoint_form(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        let url = self.field_text(&self.check_inputs.endpoint_url, cx);
        let key = self.field_text(&self.check_inputs.endpoint_key, cx);
        self.check_ui.endpoint_testing = true;
        self.check_ui.endpoint_test = None;
        cx.notify();
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client
                .run_for(Command::TestEndpoint {
                    id,
                    base_url: url,
                    api_key: (!key.is_empty()).then_some(key),
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.check_ui.endpoint_testing = false;
                this.check_ui.endpoint_test = Some(test_output(result));
                cx.notify();
            });
        })
        .detach();
    }

    /// Test connection in the menu of an endpoint row: a notice with the
    /// result.
    pub(crate) fn test_saved_endpoint(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(endpoint) = self.state.endpoints.iter().find(|e| e.id == id) else {
            return;
        };
        let (name, base_url) = (endpoint.name.clone(), endpoint.base_url.clone());
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client
                .run_for(Command::TestEndpoint {
                    id: Some(id),
                    base_url,
                    api_key: None,
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let result = test_output(result);
                let line = format!("{name}: {}", result.message);
                if result.ok {
                    this.show_notice(line, cx);
                } else {
                    this.show_error(line, cx);
                }
            });
        })
        .detach();
    }

    /// The dialog that removes an endpoint.
    pub(crate) fn confirm_remove_endpoint_dialog(
        &mut self,
        id: String,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> (String, String, gpui_kit::Pixels, Div, Div) {
        let endpoint = self.state.endpoints.iter().find(|e| e.id == id);
        let name = endpoint.map(|e| e.name.clone()).unwrap_or_default();
        let in_use = endpoint.is_some_and(|e| e.in_use);
        (
            format!("Remove {name}?"),
            if in_use {
                "The command check uses this endpoint. It goes back to a model of an account. The hub deletes the key.".to_owned()
            } else {
                "The hub deletes the key. You cannot undo this.".to_owned()
            },
            t.layout.dialog_width,
            div(),
            self.dialog_actions_kind(
                "Remove endpoint",
                ButtonKind::Danger,
                t,
                cx,
                move |this, window, cx| {
                    let client = this.client.clone();
                    let id = id.clone();
                    this.submit_dialog(async move { client.remove_endpoint(id).await }, window, cx);
                },
            ),
        )
    }

    /// Saves a new choice. A new choice makes the last test old.
    fn set_check_choice(&mut self, choice: CommandCheckChoice, cx: &mut Context<Self>) {
        self.check_ui.test = None;
        let client = self.client.clone();
        self.spawn_command(cx, async move { client.set_command_check(choice).await });
        cx.notify();
    }

    /// A pick of a select of the section. True when the select is one of them.
    pub(crate) fn pick_check_select(
        &mut self,
        select: &str,
        option: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if ![
            CHECK_SOURCE_SELECT,
            CHECK_ENDPOINT_SELECT,
            CHECK_LIMIT_SELECT,
        ]
        .contains(&select)
        {
            return false;
        }
        let mut choice = self.state.command_check.choice.clone();
        let first = self.state.endpoints.first().map(|e| e.id.clone());
        if apply_check_pick(&mut choice, select, option, first.as_deref()) {
            self.set_check_choice(choice, cx);
        }
        true
    }

    /// A model from the picker, or `None` for Choose for me.
    pub(crate) fn set_check_account_model(
        &mut self,
        selection: Option<consort_client::SelectionView>,
        cx: &mut Context<Self>,
    ) {
        let mut choice = self.state.command_check.choice.clone();
        choice.account_model = selection;
        self.set_check_choice(choice, cx);
    }

    /// Saves the model names that changed.
    pub(crate) fn save_check_model_names(&mut self, cx: &mut Context<Self>) {
        let mut choice = self.state.command_check.choice.clone();
        let first = self.state.endpoints.first().map(|e| e.id.clone());
        let decision = self.field_text(&self.check_inputs.decision_model, cx);
        let chat = self.field_text(&self.check_inputs.chat_model, cx);
        if (choice.decision.is_some() || !decision.is_empty())
            && let Some(slot) = choice_of(&mut choice, CheckSourceKind::Decision, first.as_deref())
        {
            slot.model = decision;
        }
        if (choice.chat.is_some() || !chat.is_empty())
            && let Some(slot) = choice_of(&mut choice, CheckSourceKind::Chat, first.as_deref())
        {
            slot.model = chat;
        }
        if choice != self.state.command_check.choice {
            self.set_check_choice(choice, cx);
        }
    }

    /// Fills a model name field from the hub while the user is not in it.
    fn sync_check_model_names(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choice = self.state.command_check.choice.clone();
        for (input, value) in [
            (
                self.check_inputs.decision_model.clone(),
                choice.decision.as_ref().map(|c| c.model.clone()),
            ),
            (
                self.check_inputs.chat_model.clone(),
                choice.chat.as_ref().map(|c| c.model.clone()),
            ),
        ] {
            let value = value.unwrap_or_default();
            let (focused, current) = {
                let state = input.read(cx);
                (
                    state.focus_handle(cx).is_focused(window),
                    state.value().to_string(),
                )
            };
            if !focused && current != value {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
        }
    }

    fn test_check(&mut self, cx: &mut Context<Self>) {
        let model = match self.state.command_check.choice.kind {
            CheckSourceKind::Chat => self.field_text(&self.check_inputs.chat_model, cx),
            _ => self.field_text(&self.check_inputs.decision_model, cx),
        };
        let any_endpoint = !self.state.endpoints.is_empty();
        if let Some(reason) = self.state.command_check.test_blocked(&model, any_endpoint) {
            self.check_ui.test = Some(TestResultView {
                ok: false,
                message: reason.to_owned(),
                details: None,
            });
            cx.notify();
            return;
        }
        self.save_check_model_names(cx);
        self.check_ui.testing = true;
        self.check_ui.test = None;
        cx.notify();
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client.run_for(Command::TestCommandCheck).await;
            let _ = this.update(cx, |this, cx| {
                this.check_ui.testing = false;
                this.check_ui.test = Some(test_output(result));
                cx.notify();
            });
        })
        .detach();
    }

    /// The section "Command check" of Settings > Models.
    pub(crate) fn command_check_section(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        self.sync_check_model_names(window, cx);
        let view = self.state.command_check.clone();
        let kind = view.choice.kind;
        let options = CheckSourceKind::ALL
            .iter()
            .map(|k| (k.label().to_owned(), k.label().to_owned(), *k == kind))
            .collect();
        let source = select(
            CHECK_SOURCE_SELECT,
            kind.label().to_owned(),
            t.component.dictation_cleanup_select,
            options,
            self,
            t,
            cx,
        );
        let mut list = rows(t).child(
            row(false, t).child(row_label("Source", t)).child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.xxs)
                    .child(source)
                    .child(hint(kind.description(), t)),
            ),
        );
        list = match kind {
            CheckSourceKind::Account => list.child(self.check_account_row(t, cx)),
            CheckSourceKind::Decision | CheckSourceKind::Chat => {
                self.check_endpoint_rows(list, kind, t, window, cx)
            }
        };
        list = list.child(self.check_test_row(t, cx));
        div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section_title("Command check", t))
            .child(hint(
                "In Auto mode, it decides whether a shell command can run without asking.",
                t,
            ))
            .child(list)
            .child(how_the_check_works(t))
    }

    fn check_account_row(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let view = self.state.command_check.clone();
        let chosen_by_consort = view.choice.account_model.is_none();
        if view.account_model().is_none() {
            return row(false, t)
                .child(row_label("Model", t))
                .child(
                    hint(
                        "No model yet. Add an account in Providers, or choose another source.",
                        t,
                    )
                    .flex_1()
                    .min_w_0(),
                )
                .child(open_providers_button("check-open-providers", t, cx));
        }
        row(false, t).child(row_label("Model", t)).child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(t.space.xxs)
                .child(div().debug_selector(|| "check-model".into()).child(
                    self.render_model_trigger(
                        "check-model",
                        PickerTarget::CommandCheck,
                        TriggerStyle::Field,
                        t,
                        cx,
                    ),
                ))
                .when(chosen_by_consort, |d| {
                    d.child(hint(
                        "Chosen for you. It is the smallest model of your accounts.",
                        t,
                    ))
                }),
        )
    }

    fn check_endpoint_rows(
        &mut self,
        mut list: Div,
        kind: CheckSourceKind,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let choice = match kind {
            CheckSourceKind::Decision => self.state.command_check.choice.decision.clone(),
            _ => self.state.command_check.choice.chat.clone(),
        };
        let endpoints = self.state.endpoints.clone();
        if endpoints.is_empty() {
            return list.child(
                row(false, t)
                    .child(row_label("Endpoint", t))
                    .child(
                        hint("No endpoint yet. Add one in Providers.", t)
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(open_providers_button("check-open-providers", t, cx)),
            );
        }
        let current = choice.as_ref().map(|c| c.endpoint_id.clone());
        let label = endpoints
            .iter()
            .find(|e| Some(&e.id) == current.as_ref())
            .map_or_else(|| "Choose an endpoint".to_owned(), |e| e.name.clone());
        let options = endpoints
            .iter()
            .map(|e| {
                (
                    e.name.clone(),
                    e.id.clone(),
                    Some(&e.id) == current.as_ref(),
                )
            })
            .collect();
        let endpoint = select(
            CHECK_ENDPOINT_SELECT,
            label,
            t.component.dictation_cleanup_select,
            options,
            self,
            t,
            cx,
        );
        list = list.child(
            row(false, t)
                .child(row_label("Endpoint", t))
                .child(endpoint),
        );
        let input = match kind {
            CheckSourceKind::Decision => self.check_inputs.decision_model.clone(),
            _ => self.check_inputs.chat_model.clone(),
        };
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        list = list.child(
            row(false, t).child(row_label("Model name", t)).child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(input_box(&input, focused, t)),
            ),
        );
        match kind {
            CheckSourceKind::Decision => {
                let limit = choice
                    .as_ref()
                    .map_or(consort_client::DEFAULT_CONFIDENCE_LIMIT, |c| {
                        c.confidence_limit
                    });
                let options = CONFIDENCE_LIMITS
                    .iter()
                    .map(|l| (format!("{l} percent"), l.to_string(), *l == limit))
                    .collect();
                let limit_box = select(
                    CHECK_LIMIT_SELECT,
                    format!("{limit} percent"),
                    t.component.dictation_cleanup_select,
                    options,
                    self,
                    t,
                    cx,
                );
                list.child(
                    row(false, t)
                        .child(row_label("Confidence limit", t))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(t.space.xxs)
                                .child(limit_box)
                                .child(hint(
                                    "An answer below the limit counts as unsure. Then Consort asks you.",
                                    t,
                                )),
                        ),
                )
            }
            _ => list.child(
                row(false, t).child(row_label("Cost", t)).child(
                    hint(
                        "Each check costs money, and the endpoint sees the commands.",
                        t,
                    )
                    .flex_1()
                    .min_w_0(),
                ),
            ),
        }
    }

    fn check_test_row(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let testing = self.check_ui.testing;
        let enabled = !testing;
        let test = button("check-test", "Test", ButtonKind::Secondary, t)
            .debug_selector(|| "check-test".into())
            .when(!enabled, |b| b.opacity(t.opacity.busy).cursor_default())
            .when(enabled, |b| {
                b.on_click(cx.listener(|this, _, _, cx| this.test_check(cx)))
            });
        let result: Div = if testing {
            div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .child(status_glyph(
                    "check-testing",
                    consort_client::TaskStatus::Working,
                    t.icon.dense,
                    t,
                    cx,
                ))
                .child(text(
                    "Testing the command check.",
                    t.text.ui,
                    c.text_secondary,
                ))
        } else {
            match self.check_ui.test.clone() {
                Some(result) => {
                    let failed = !result.ok;
                    test_result_lines("check-test-result", &result, t).when(failed, |d| {
                        d.child(open_providers_button("check-test-providers", t, cx))
                    })
                }
                None => div(),
            }
        };
        row(true, t)
            .items_start()
            .child(row_label("Test", t))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.sm)
                    .child(test)
                    .child(result),
            )
    }
}

/// A test result: a line, or an error with details that the user can select.
fn test_result_lines(key: &'static str, result: &TestResultView, t: &Tokens) -> Div {
    let c = &t.colors;
    if result.ok {
        return div()
            .flex()
            .flex_col()
            .debug_selector(move || key.to_owned())
            .child(text(result.message.clone(), t.text.ui, c.text_secondary));
    }
    let mut lines = div()
        .flex()
        .flex_col()
        .items_start()
        .gap(t.space.sm)
        .w_full()
        .debug_selector(move || key.to_owned())
        .child(error_box(key, result.message.clone(), t));
    if let Some(details) = &result.details {
        lines = lines.child(div().font_family(t.font_mono.clone()).child(selectable(
            gpui_kit::ElementId::Name(format!("{key}-details").into()),
            details.clone(),
            t.text.mono,
            c.text_secondary,
        )));
    }
    lines
}

fn test_output(result: Result<CommandOutput, consort_client::ClientError>) -> TestResultView {
    match result {
        Ok(CommandOutput::Tested(result)) => result,
        Ok(_) => TestResultView {
            ok: false,
            message: "The hub gave no test result. Try again.".to_owned(),
            details: None,
        },
        Err(err) => TestResultView {
            ok: false,
            message: err.to_string(),
            details: None,
        },
    }
}

fn open_providers_button(
    id: &'static str,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> gpui_kit::Stateful<Div> {
    button(id, "Open Providers", ButtonKind::Secondary, t)
        .debug_selector(move || id.to_owned())
        .flex_none()
        .on_click(cx.listener(|this, _, _, cx| this.open_settings(SettingsSection::Providers, cx)))
}

/// The table "How the check works".
fn how_the_check_works(t: &Tokens) -> Div {
    let lines = [
        (
            "It checks",
            "Shell commands and tools that Consort does not know, in Auto mode only. File changes in the workspace and hub tools do not need it.",
        ),
        (
            "It sees",
            "Your messages and the command. It does not see files, tool output or the words of the assistant.",
        ),
        (
            "It asks you when",
            "It does not answer in 10 s, it fails, or it is not sure. It also asks after 3 blocks in a row or 20 blocks in a turn.",
        ),
        (
            "It is not a lock",
            "A worker's own limit and the boundary around the session still hold. The check can be wrong.",
        ),
    ];
    let count = lines.len();
    let mut list = rows(t);
    for (index, (label, line)) in lines.into_iter().enumerate() {
        list = list.child(
            row(index + 1 == count, t)
                .items_start()
                .child(row_label(label, t))
                .child(
                    text(line, t.text.ui, t.colors.text_secondary)
                        .flex_1()
                        .min_w_0(),
                ),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap(t.space.md)
        .child(section_title("How the check works", t))
        .child(list)
}
