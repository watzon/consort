//! The app side of Monid, the paid tools with one wallet (ADR 0016) and the
//! access requests that go with it (ADR 0017): the state of its page, its
//! panel view and its dialog, its text inputs, its menu actions, and the
//! commands that they send. The views are in `ui/monid_cards.rs`,
//! `ui/monid_panel.rs` and `ui/monid_settings.rs`.

use std::collections::BTreeSet;

use consort_client::{
    ApprovalRuleView, Command, UseInView,
    monid::{
        MonidAccessView, MonidSettingsView, MonidToolView, MonidUnattendedView, find_call_row,
        parse_limit,
    },
};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, Window,
    component::input::{InputEvent, InputState},
};

use crate::app::{ConsortApp, Dialog, SettingsSection};

/// The select of "Each call" in the Monid view of a channel. Its options are
/// the keys of the rules, and `hub` for the hub's rule.
pub const CHANNEL_RULE_SELECT: &str = "monid-channel-rule";
/// The select of "Each call" on the Monid page of Settings.
pub const HUB_RULE_SELECT: &str = "monid-hub-rule";
/// The select of "Approvals start as" in the Add Monid dialog.
pub const ADD_RULE_SELECT: &str = "monid-add-rule";

/// The option of the channel select that follows the hub.
pub const FOLLOW_HUB: &str = "hub";

/// The part of a call that a menu item copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallPart {
    Input,
    Output,
    RunId,
}

/// What a Monid menu item, or a button of a Monid card, does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonidAction {
    /// Copies a part of a call. The call loads first when it is not open.
    CopyPart(String, CallPart),
    /// Asks Monid to stop a run.
    StopCall(String),
    /// Answers a card with one of its options: the approval ID and the
    /// option ID.
    AnswerOption(String, String),
    /// Remove a tool from the list of allowed tools of a channel: the
    /// channel ID and the position.
    RemoveAllowed(String, usize),
    /// Remove a tool from the list of tools that a channel never uses.
    RemoveBlocked(String, usize),
    /// Ends a temporary access: the channel ID and the grant ID.
    EndAccess(String, String),
    /// Adds a search result to the allowed list of a channel.
    AddAllowed(String, MonidToolView),
    /// Adds a search result to the list of tools that a channel never uses.
    AddBlocked(String, MonidToolView),
}

/// The Add Monid dialog.
#[derive(Clone, Debug, Default)]
pub struct AddForm {
    pub use_in: UseInView,
    /// "Approvals start as".
    pub approval: ApprovalRuleView,
    /// The dialog opened from the Add connector dialog, so Back goes there.
    pub from_catalog: bool,
}

/// Everything the app keeps about Monid between draws.
#[derive(Default)]
pub struct MonidUi {
    /// Settings > Connectors shows the Monid page.
    pub page: bool,
    /// The Connectors tab of the Channel panel shows the Monid view.
    pub panel: bool,
    pub add: AddForm,
    /// The call rows that the user opened. The key is the call ID and the
    /// place.
    pub open_calls: BTreeSet<String>,
    /// Why the limits were not saved.
    pub limits_error: Option<String>,
    /// Why the key was not saved.
    pub key_error: Option<String>,
    /// Counts the searches typed, so a search runs after the typing stops.
    pub search_serial: u64,
}

/// The text inputs of Monid.
pub struct MonidInputs {
    /// The API key. The field masks it.
    pub key: Entity<InputState>,
    /// The search of the Monid view in the panel.
    pub search: Entity<InputState>,
    pub per_call: Entity<InputState>,
    pub per_task: Entity<InputState>,
    pub per_channel: Entity<InputState>,
    pub per_hub: Entity<InputState>,
}

impl MonidInputs {
    pub fn new(window: &mut Window, cx: &mut Context<ConsortApp>) -> MonidInputs {
        let limit = |window: &mut Window, cx: &mut Context<ConsortApp>| {
            cx.new(|cx| InputState::new(window, cx).placeholder("No limit"))
        };
        MonidInputs {
            key: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Paste the API key")
                    .masked(true)
            }),
            search: cx
                .new(|cx| InputState::new(window, cx).placeholder("Add a tool or a provider")),
            per_call: limit(window, cx),
            per_task: limit(window, cx),
            per_channel: limit(window, cx),
            per_hub: limit(window, cx),
        }
    }

    pub fn fields(&self) -> [&Entity<InputState>; 6] {
        [
            &self.key,
            &self.search,
            &self.per_call,
            &self.per_task,
            &self.per_channel,
            &self.per_hub,
        ]
    }

    pub fn limits(&self) -> [&Entity<InputState>; 4] {
        [
            &self.per_call,
            &self.per_task,
            &self.per_channel,
            &self.per_hub,
        ]
    }
}

/// The key of the open state of a call row in a place.
pub fn call_key(place: &str, call_id: &str) -> String {
    format!("{place}:{call_id}")
}

/// What the select "Each call" of a channel means, from its option.
pub fn channel_rule_from_option(option: &str) -> Option<Option<ApprovalRuleView>> {
    if option == FOLLOW_HUB {
        return Some(None);
    }
    ApprovalRuleView::from_key(option).map(Some)
}

/// The access of a channel after a tool joins the allowed list. A tool that
/// is on the other list leaves it.
pub fn access_with_allowed(access: &MonidAccessView, tool: MonidToolView) -> MonidAccessView {
    access.with_allowed(tool)
}

/// The access of a channel after a tool joins the list of tools that it
/// never uses.
pub fn access_with_blocked(access: &MonidAccessView, tool: MonidToolView) -> MonidAccessView {
    access.with_blocked(tool)
}

/// Reads the four limit fields. `Err` has the words for the user.
pub fn read_limits(fields: [&str; 4]) -> Result<consort_client::monid::MonidLimitsView, String> {
    let mut values = [None; 4];
    for (value, text) in values.iter_mut().zip(fields) {
        *value = parse_limit(text)?;
    }
    Ok(consort_client::monid::MonidLimitsView {
        per_call_micros: values[0],
        per_task_micros: values[1],
        per_channel_month_micros: values[2],
        per_hub_month_micros: values[3],
    })
}

impl ConsortApp {
    /// Subscribes to the inputs of Monid.
    pub fn subscribe_monid_inputs(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui_kit::Subscription> {
        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(
            &self.monid_inputs.key,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.save_monid_key(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        ));
        subs.push(cx.subscribe_in(
            &self.monid_inputs.search,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.search_monid_soon(cx);
                }
            },
        ));
        for field in self.monid_inputs.limits() {
            subs.push(
                cx.subscribe_in(
                    field,
                    window,
                    |this, _, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } | InputEvent::Blur => {
                            this.save_monid_limits(window, cx)
                        }
                        _ => {}
                    },
                ),
            );
        }
        subs
    }

    /// Asks the hub for the key state, the balance, the settings and the
    /// spend of this month.
    pub fn load_monid_status(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(
            cx,
            async move { client.run(Command::LoadMonidStatus).await },
        );
    }

    /// Runs a menu item or a button of Monid.
    pub fn run_monid_action(
        &mut self,
        action: MonidAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            MonidAction::CopyPart(call_id, part) => self.copy_call_part(call_id, part, cx),
            MonidAction::StopCall(call_id) => {
                let client = self.client.clone();
                self.spawn_command(cx, async move {
                    client.run(Command::StopMonidCall { call_id }).await
                });
            }
            MonidAction::AnswerOption(approval_id, option_id) => {
                self.answer_option(approval_id, option_id, window, cx)
            }
            MonidAction::RemoveAllowed(channel_id, at) => {
                self.update_monid_access(&channel_id, cx, move |access| access.without_allowed(at))
            }
            MonidAction::RemoveBlocked(channel_id, at) => {
                self.update_monid_access(&channel_id, cx, move |access| access.without_blocked(at))
            }
            MonidAction::EndAccess(channel_id, grant_id) => {
                let client = self.client.clone();
                self.spawn_command(cx, async move {
                    client
                        .run(Command::EndAccess {
                            channel_id,
                            grant_id,
                        })
                        .await
                });
            }
            MonidAction::AddAllowed(channel_id, tool) => {
                self.clear_monid_search(window, cx);
                self.update_monid_access(&channel_id, cx, move |access| {
                    access_with_allowed(access, tool)
                });
            }
            MonidAction::AddBlocked(channel_id, tool) => {
                self.clear_monid_search(window, cx);
                self.update_monid_access(&channel_id, cx, move |access| {
                    access_with_blocked(access, tool)
                });
            }
        }
    }

    /// Answers an approval with the option that has this ID.
    pub fn answer_option(
        &mut self,
        approval_id: String,
        option_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.blocked_by_hub(cx) {
            return;
        }
        let client = self.client.clone();
        let id = approval_id.clone();
        self.run_guarded(
            format!("approval:{approval_id}"),
            async move {
                client
                    .run(Command::AnswerApprovalOption {
                        approval_id: id,
                        option_id,
                    })
                    .await
                    .map(|_| ())
            },
            crate::drafts::Kept::Request("The request still waits for your answer."),
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// Copies the input, the output or the run ID of a call. The call loads
    /// first when its detail is not here yet.
    fn copy_call_part(&mut self, call_id: String, part: CallPart, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let loaded = this
                .update(cx, |this, _| {
                    find_call_row(&this.state, &call_id)
                        .and_then(|r| r.detail.as_ref())
                        .is_some_and(|d| d.load == consort_client::LoadState::Loaded)
                })
                .unwrap_or(false);
            if !loaded {
                let id = call_id.clone();
                if let Err(err) = client.run(Command::LoadMonidCall { call_id: id }).await {
                    let _ = this.update(cx, |this, cx| this.show_error(err.to_string(), cx));
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                let text = find_call_row(&this.state, &call_id)
                    .and_then(|r| r.detail.as_ref())
                    .map(|d| match part {
                        CallPart::Input => d.input.clone(),
                        CallPart::Output => d.output.clone(),
                        CallPart::RunId => d
                            .run_line
                            .as_deref()
                            .and_then(|l| l.split(", ").next())
                            .unwrap_or_default()
                            .to_owned(),
                    })
                    .unwrap_or_default();
                if text.is_empty() {
                    this.show_notice("The call has nothing to copy.".into(), cx);
                } else {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            });
        })
        .detach();
    }

    /// Opens or closes a call row. The detail loads when the row opens.
    pub fn toggle_call_row(&mut self, key: String, call_id: String, cx: &mut Context<Self>) {
        if !self.monid_ui.open_calls.remove(&key) {
            self.monid_ui.open_calls.insert(key);
            let loaded = find_call_row(&self.state, &call_id)
                .and_then(|r| r.detail.as_ref())
                .is_some_and(|d| d.load == consort_client::LoadState::Loaded);
            if !loaded {
                let client = self.client.clone();
                self.spawn_command(cx, async move {
                    client.run(Command::LoadMonidCall { call_id }).await
                });
            }
        }
        self.timeline.remeasure();
        cx.notify();
    }

    /// Settings > Connectors with the Monid page open.
    pub fn open_monid_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(SettingsSection::Connectors, cx);
        self.select_monid_page(window, cx);
    }

    /// Shows the Monid page in Settings > Connectors.
    pub fn select_monid_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.monid_ui.page = true;
        self.monid_ui.limits_error = None;
        self.sync_monid_limit_inputs(window, cx);
        self.load_monid_status(cx);
        cx.notify();
    }

    /// Puts the limits of the hub in their fields.
    pub fn sync_monid_limit_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let limits = self.state.monid.settings.limits;
        let values = [
            limits.per_call_micros,
            limits.per_task_micros,
            limits.per_channel_month_micros,
            limits.per_hub_month_micros,
        ];
        for (input, micros) in self.monid_inputs.limits().into_iter().zip(values) {
            let text = consort_client::monid::limit_field_text(micros);
            input.update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }

    /// Saves the four limit fields. A wrong amount shows under the rows, and
    /// nothing is saved.
    pub fn save_monid_limits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.monid_ui.page {
            return;
        }
        let texts: Vec<String> = self
            .monid_inputs
            .limits()
            .into_iter()
            .map(|f| f.read(cx).value().to_string())
            .collect();
        let limits = match read_limits([&texts[0], &texts[1], &texts[2], &texts[3]]) {
            Ok(limits) => limits,
            Err(message) => {
                self.monid_ui.limits_error = Some(message);
                cx.notify();
                return;
            }
        };
        self.monid_ui.limits_error = None;
        if limits == self.state.monid.settings.limits {
            cx.notify();
            return;
        }
        let _ = window;
        self.update_monid_settings(cx, move |settings| settings.limits = limits);
    }

    /// Sends the settings of the hub with a change.
    pub fn update_monid_settings(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut MonidSettingsView),
    ) {
        let mut settings = self.state.monid.settings.clone();
        change(&mut settings);
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.run(Command::UpdateMonidSettings { settings }).await
        });
    }

    /// Sends the Monid access of a channel with a change.
    pub fn update_monid_access(
        &mut self,
        channel_id: &str,
        cx: &mut Context<Self>,
        change: impl FnOnce(&MonidAccessView) -> MonidAccessView,
    ) {
        let Some(channel) = self.state.channel(channel_id) else {
            return;
        };
        let access = change(&channel.monid);
        let client = self.client.clone();
        let channel_id = channel_id.to_owned();
        self.spawn_command(cx, async move {
            client
                .run(Command::SetChannelMonid { channel_id, access })
                .await
        });
    }

    /// Sets what routines or proactive turns may do with Monid on the page.
    pub fn set_monid_unattended(
        &mut self,
        routines: bool,
        value: MonidUnattendedView,
        cx: &mut Context<Self>,
    ) {
        self.update_monid_settings(cx, move |settings| {
            if routines {
                settings.routines = value;
            } else {
                settings.proactive = value;
            }
        });
    }

    /// Sets what one routine may do with Monid. `None` uses the hub's
    /// setting.
    pub fn set_routine_monid(
        &mut self,
        routine_id: String,
        monid: Option<MonidUnattendedView>,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client
                .run(Command::SetRoutineMonid { routine_id, monid })
                .await
        });
    }

    /// A pick from one of the Monid selects. Returns whether the select was
    /// one of them.
    pub fn pick_monid_select(
        &mut self,
        select: &'static str,
        option: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        match select {
            CHANNEL_RULE_SELECT => {
                if let Some(rule) = channel_rule_from_option(option) {
                    let channel_id = self.channel_id.clone();
                    self.update_monid_access(&channel_id, cx, move |access| {
                        access.with_approval(rule)
                    });
                }
                true
            }
            HUB_RULE_SELECT => {
                if let Some(rule) = ApprovalRuleView::from_key(option) {
                    self.update_monid_settings(cx, move |settings| settings.approval = rule);
                }
                true
            }
            ADD_RULE_SELECT => {
                if let Some(rule) = ApprovalRuleView::from_key(option) {
                    self.monid_ui.add.approval = rule;
                }
                cx.notify();
                true
            }
            _ => false,
        }
    }

    /// Opens the Add Monid dialog, or the key dialog when the hub has a key.
    pub fn open_monid_key_dialog(
        &mut self,
        from_catalog: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.monid_ui.add = AddForm {
            use_in: crate::extensions::default_use_in(&self.state, &self.channel_id),
            approval: ApprovalRuleView::AskEveryTime,
            from_catalog,
        };
        self.monid_ui.key_error = None;
        self.open_dialog(Dialog::MonidKey, window, cx);
    }

    /// Saves the key of the key field, with the settings of the Add Monid
    /// dialog when the hub has no key yet.
    pub fn save_monid_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog != Some(Dialog::MonidKey) {
            return;
        }
        let key = self.monid_inputs.key.read(cx).value().trim().to_owned();
        if key.is_empty() {
            self.dialog_error = Some("Paste your Monid API key. Make one at app.monid.ai.".into());
            cx.notify();
            return;
        }
        let settings = (!self.state.monid.key_present).then(|| MonidSettingsView {
            use_in: self.monid_ui.add.use_in.clone(),
            approval: self.monid_ui.add.approval,
            ..MonidSettingsView::default()
        });
        let client = self.client.clone();
        let input = self.monid_inputs.key.clone();
        self.submit_dialog(
            async move {
                client
                    .run(Command::SetMonidKey {
                        api_key: consort_client::SecretText(key),
                        settings,
                    })
                    .await
            },
            window,
            cx,
        );
        input.update(cx, |input, cx| input.set_value("", window, cx));
    }

    /// Removes the key from the hub.
    pub fn remove_monid_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(cx, async move { client.run(Command::RemoveMonidKey).await });
        self.close_dialog(window, cx);
    }

    /// Runs the search a moment after the typing stops.
    fn search_monid_soon(&mut self, cx: &mut Context<Self>) {
        self.monid_ui.search_serial += 1;
        let serial = self.monid_ui.search_serial;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.monid_ui.search_serial == serial {
                    let query = this.monid_inputs.search.read(cx).value().to_string();
                    let client = this.client.clone();
                    this.spawn_command(cx, async move {
                        client.run(Command::SearchMonid { query }).await
                    });
                }
            });
        })
        .detach();
    }

    fn clear_monid_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.monid_inputs
            .search
            .update(cx, |input, cx| input.set_value("", window, cx));
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client
                .run(Command::SearchMonid {
                    query: String::new(),
                })
                .await
        });
    }

    /// Opens the Monid view in the Connectors tab of the Channel panel.
    pub fn open_monid_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.monid_ui.panel = true;
        self.monid_inputs
            .search
            .update(cx, |input, cx| input.set_value("", window, cx));
        let channel_id = self.channel_id.clone();
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.run(Command::LoadMonidStatus).await?;
            client.run(Command::LoadMonidCalls { channel_id }).await
        });
        cx.notify();
    }

    pub fn close_monid_panel(&mut self, cx: &mut Context<Self>) {
        self.monid_ui.panel = false;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_rule_select_knows_the_hub_and_each_rule() {
        assert_eq!(channel_rule_from_option("hub"), Some(None));
        assert_eq!(
            channel_rule_from_option("ask_every_time"),
            Some(Some(ApprovalRuleView::AskEveryTime))
        );
        assert_eq!(channel_rule_from_option("nonsense"), None);
    }

    #[test]
    fn the_limit_fields_read_amounts_and_empty_means_no_limit() {
        let limits = read_limits(["1", "", "20.50", " "]).expect("valid");
        assert_eq!(limits.per_call_micros, Some(1_000_000));
        assert_eq!(limits.per_task_micros, None);
        assert_eq!(limits.per_channel_month_micros, Some(20_500_000));
        assert_eq!(limits.per_hub_month_micros, None);
    }

    #[test]
    fn a_wrong_amount_gives_words_and_saves_nothing() {
        assert!(read_limits(["ten", "", "", ""]).is_err());
        assert!(read_limits(["-3", "", "", ""]).is_err());
    }

    #[test]
    fn a_call_row_has_one_open_state_for_each_place() {
        assert_ne!(call_key("chat", "c1"), call_key("thread", "c1"));
    }
}
