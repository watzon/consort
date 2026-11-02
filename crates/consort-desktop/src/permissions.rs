//! Permissions in the desktop app (ADR 0020): the Access section of the
//! channel settings, the Permissions dialog, its rule dialog and its
//! exception picker.
//!
//! The form logic is pure and has no GPUI: the draft, the override of a rule
//! from Tutti, the counts and the diff that Save changes sends. The views
//! are in `ui/permissions.rs`. Every edit is a draft until Save changes.

use consort_client::{
    ApprovalRuleView, ChannelChanges, ChannelKind, Command, ConnectionChanges, ConnectionView,
    ConnectorToolView, ServiceToolsView, ToolApprovalsView, ToolExceptionView, ToolKindView,
    ViewState, channel_extensions, matching_channels,
};
use consort_proto::model::{
    CommandRule, CommandRuleInput, CommandRuleResult, CommandRuleSource, tool_display_name,
    validate_command_rules,
};
use gpui_kit::{Context, Window};

use crate::{
    app::{ConsortApp, Dialog},
    convo::set_input,
};

/// The ID of the select of a tool kind of a connection, for
/// `MenuAction::Select`.
pub const RULE_SELECT: &str = "permissions-rule";
/// The ID of the select of a tool exception.
pub const EXCEPTION_SELECT: &str = "permissions-exception";

/// What a control of the Permissions dialog does.
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionsAction {
    /// Open the Permissions dialog of a channel.
    Open(String),
    /// Open the rule dialog with no rule.
    AddRule,
    /// Edit the rule of the channel with this pattern.
    EditRule(String),
    RemoveRule(String),
    /// Copy the rule of Tutti with this pattern to this channel, for the
    /// user to change.
    OverrideRule(String),
    /// A connection ID and a tool slug.
    RemoveException(String, String),
    /// Open the tool picker of a connection.
    AddException(String),
}

/// One connection in the draft: its rule for each tool kind and its tool
/// exceptions.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionDraft {
    pub id: String,
    pub approvals: ToolApprovalsView,
    pub exceptions: Vec<ToolExceptionView>,
}

impl ConnectionDraft {
    fn of(connection: &ConnectionView) -> ConnectionDraft {
        ConnectionDraft {
            id: connection.id.clone(),
            approvals: connection.approvals,
            exceptions: connection.tool_exceptions.clone(),
        }
    }

    /// Gives a tool its own rule, or changes the rule that it has.
    pub fn set_exception(&mut self, tool: &str, name: &str, rule: ApprovalRuleView) {
        match self
            .exceptions
            .iter_mut()
            .find(|e| e.tool.eq_ignore_ascii_case(tool))
        {
            Some(existing) => existing.rule = rule,
            None => self.exceptions.push(ToolExceptionView {
                tool: tool.to_owned(),
                name: name.to_owned(),
                rule,
            }),
        }
    }

    pub fn remove_exception(&mut self, tool: &str) {
        self.exceptions.retain(|e| e.tool != tool);
    }
}

/// The draft of the Permissions dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PermissionsForm {
    pub channel_id: String,
    /// The rules of this channel. Not the rules of Tutti.
    pub rules: Vec<CommandRule>,
    /// The connections that the channel has, in the order of the state.
    pub connections: Vec<ConnectionDraft>,
}

/// What Save changes sends.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PermissionsSave {
    /// The whole list, when it changed.
    pub command_rules: Option<Vec<CommandRule>>,
    /// The connections with a change.
    pub connections: Vec<(String, ConnectionChanges)>,
}

impl PermissionsSave {
    pub fn is_empty(&self) -> bool {
        self.command_rules.is_none() && self.connections.is_empty()
    }
}

/// The connections that a channel has: its scope gives them, and the channel
/// has not turned them off. A connection that is still connecting has no
/// tools yet, so it is left out.
pub fn available_connections<'a>(
    state: &'a ViewState,
    channel_id: &str,
) -> Vec<&'a ConnectionView> {
    let Some(view) = channel_extensions(state, channel_id) else {
        return Vec::new();
    };
    let ids = view.enabled_connector_ids();
    state
        .connections
        .iter()
        .filter(|c| ids.contains(&c.id.as_str()))
        .filter(|c| {
            !matches!(
                c.status,
                consort_client::ConnectionStatusView::Connecting { .. }
            )
        })
        .collect()
}

/// The rules of Tutti. They apply in every channel.
pub fn tutti_rules(state: &ViewState) -> &[CommandRule] {
    state.tutti().map_or(&[], |c| c.command_rules.as_slice())
}

/// The rules of Tutti that a channel does not override: the rules of Tutti
/// with a pattern that the channel has no rule for. Tutti has none.
pub fn inherited_rules(
    rules: &[CommandRule],
    tutti: &[CommandRule],
    is_tutti: bool,
) -> Vec<CommandRule> {
    if is_tutti {
        return Vec::new();
    }
    tutti
        .iter()
        .filter(|t| !rules.iter().any(|r| r.pattern == t.pattern))
        .cloned()
        .collect()
}

/// The command rules that apply in a channel, as the saved state has them.
pub fn effective_rule_count(state: &ViewState, channel_id: &str) -> usize {
    let Some(channel) = state.channel(channel_id) else {
        return 0;
    };
    let is_tutti = channel.kind == ChannelKind::Tutti;
    channel.command_rules.len()
        + inherited_rules(&channel.command_rules, tutti_rules(state), is_tutti).len()
}

/// The tool exceptions of the connections that a channel has.
pub fn exception_count(state: &ViewState, channel_id: &str) -> usize {
    available_connections(state, channel_id)
        .iter()
        .map(|c| c.tool_exceptions.len())
        .sum()
}

/// The line under Permissions in the Access section, such as "4 command
/// rules, 1 tool exception".
pub fn access_detail(state: &ViewState, channel_id: &str) -> String {
    let count = |n: usize, one: &str, many: &str| match n {
        0 => format!("no {many}"),
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    };
    let line = format!(
        "{}, {}",
        count(
            effective_rule_count(state, channel_id),
            "command rule",
            "command rules"
        ),
        count(
            exception_count(state, channel_id),
            "tool exception",
            "tool exceptions"
        ),
    );
    let mut chars = line.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => line,
    }
}

/// The groups of the commands column.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuleGroups {
    pub allow: Vec<CommandRule>,
    pub ask: Vec<CommandRule>,
    pub tutti: Vec<CommandRule>,
}

impl RuleGroups {
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.ask.is_empty() && self.tutti.is_empty()
    }
}

/// Splits the draft rules by result, and adds the rules of Tutti that stay.
pub fn rule_groups(rules: &[CommandRule], tutti: &[CommandRule], is_tutti: bool) -> RuleGroups {
    RuleGroups {
        allow: rules
            .iter()
            .filter(|r| r.result == CommandRuleResult::Allow)
            .cloned()
            .collect(),
        ask: rules
            .iter()
            .filter(|r| r.result == CommandRuleResult::Ask)
            .cloned()
            .collect(),
        tutti: inherited_rules(rules, tutti, is_tutti),
    }
}

/// The words after a rule description: "Except a, b." and "Added by the
/// assistant.". `None` when the rule has neither.
pub fn rule_note(rule: &CommandRule) -> Option<String> {
    let mut parts = Vec::new();
    if !rule.except.is_empty() {
        parts.push(format!("Except {}.", rule.except.join(", ")));
    }
    if rule.source == CommandRuleSource::Assistant {
        parts.push("Added by the assistant.".to_owned());
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// What a rule does, in the words of the group heading.
pub fn result_words(result: CommandRuleResult) -> &'static str {
    match result {
        CommandRuleResult::Allow => "Runs without asking.",
        CommandRuleResult::Ask => "Always asks.",
    }
}

/// The exceptions of a rule as the user types them: "--force, main".
pub fn except_text(except: &[String]) -> String {
    except.join(", ")
}

/// The exceptions that the user typed, split at the commas.
pub fn parse_except(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A rule of the user from the fields of the rule dialog. The error is the
/// text for the dialog.
pub fn build_rule(
    pattern: &str,
    except: &str,
    result: CommandRuleResult,
    description: &str,
) -> Result<CommandRule, String> {
    CommandRuleInput {
        pattern: pattern.to_owned(),
        except: parse_except(except),
        result,
        description: description.to_owned(),
    }
    .into_rule(CommandRuleSource::User)
    .map_err(|e| e.to_string())
}

/// The rule that a new exception starts with: the opposite of the rule of
/// its tool kind, because the user adds an exception to differ from it.
pub fn default_exception_rule(kind_rule: ApprovalRuleView) -> ApprovalRuleView {
    match kind_rule {
        ApprovalRuleView::RunWithoutAsking => ApprovalRuleView::AskEveryTime,
        _ => ApprovalRuleView::RunWithoutAsking,
    }
}

/// The options of the select of an exception: it cannot ask once per task.
pub const EXCEPTION_RULES: [ApprovalRuleView; 2] = [
    ApprovalRuleView::RunWithoutAsking,
    ApprovalRuleView::AskEveryTime,
];

/// The tools that the picker offers: the tools of the service with no
/// exception yet, that match the search. A word matches the name, the slug
/// and the description, with no regard to case.
pub fn exception_candidates<'a>(
    tools: &'a ServiceToolsView,
    draft: &ConnectionDraft,
    query: &str,
    service: &str,
) -> Vec<&'a ConnectorToolView> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    tools
        .tools
        .iter()
        .filter(|t| {
            !draft
                .exceptions
                .iter()
                .any(|e| e.tool.eq_ignore_ascii_case(&t.slug))
        })
        .filter(|t| {
            let haystack = format!(
                "{} {} {}",
                tool_display_name(service, &t.slug),
                t.slug,
                t.description
            )
            .to_lowercase();
            words.iter().all(|w| haystack.contains(w))
        })
        .collect()
}

fn kind_key(kind: ToolKindView) -> &'static str {
    match kind {
        ToolKindView::Read => "read",
        ToolKindView::Change => "change",
        ToolKindView::Delete => "delete",
    }
}

/// The option of a pick in a select of a tool kind.
pub fn rule_option(connection: &str, kind: ToolKindView, rule: ApprovalRuleView) -> String {
    format!("{connection}\u{1f}{}\u{1f}{}", kind_key(kind), rule.key())
}

/// The pick that [`rule_option`] made.
pub fn parse_rule_option(option: &str) -> Option<(String, ToolKindView, ApprovalRuleView)> {
    let mut parts = option.split('\u{1f}');
    let connection = parts.next()?.to_owned();
    let kind_word = parts.next()?;
    let kind = ToolKindView::ALL
        .into_iter()
        .find(|k| kind_key(*k) == kind_word)?;
    let rule = ApprovalRuleView::from_key(parts.next()?)?;
    parts.next().is_none().then_some((connection, kind, rule))
}

/// The option of a pick in a select of an exception.
pub fn exception_option(connection: &str, tool: &str, rule: ApprovalRuleView) -> String {
    format!("{connection}\u{1f}{tool}\u{1f}{}", rule.key())
}

/// The pick that [`exception_option`] made.
pub fn parse_exception_option(option: &str) -> Option<(String, String, ApprovalRuleView)> {
    let mut parts = option.split('\u{1f}');
    let connection = parts.next()?.to_owned();
    let tool = parts.next()?.to_owned();
    let rule = ApprovalRuleView::from_key(parts.next()?)?;
    parts.next().is_none().then_some((connection, tool, rule))
}

/// "Permissions of consort-app: 5 changes", from the title of the card
/// ("Change what consort-app can do") and the number of its rows.
pub fn answered_headline(change: &str, count: usize) -> String {
    let place = change
        .strip_prefix("Change what ")
        .and_then(|rest| rest.strip_suffix(" can do"))
        .map(str::to_owned);
    let changes = if count == 1 {
        "1 change".to_owned()
    } else {
        format!("{count} changes")
    };
    match place {
        Some(place) => format!("Permissions of {place}: {changes}"),
        None => format!("Permissions: {changes}"),
    }
}

impl PermissionsForm {
    /// The draft of a channel from the saved state. `None` when the channel
    /// is gone.
    pub fn open(state: &ViewState, channel_id: &str) -> Option<PermissionsForm> {
        let channel = state.channel(channel_id)?;
        Some(PermissionsForm {
            channel_id: channel.id.clone(),
            rules: channel.command_rules.clone(),
            connections: available_connections(state, channel_id)
                .into_iter()
                .map(ConnectionDraft::of)
                .collect(),
        })
    }

    pub fn connection_mut(&mut self, id: &str) -> Option<&mut ConnectionDraft> {
        self.connections.iter_mut().find(|c| c.id == id)
    }

    pub fn connection(&self, id: &str) -> Option<&ConnectionDraft> {
        self.connections.iter().find(|c| c.id == id)
    }

    pub fn rule(&self, pattern: &str) -> Option<&CommandRule> {
        self.rules.iter().find(|r| r.pattern == pattern)
    }

    pub fn remove_rule(&mut self, pattern: &str) {
        self.rules.retain(|r| r.pattern != pattern);
    }

    /// Adds a rule, or replaces the rule with the pattern `editing`. The
    /// draft keeps its list when the new list is not valid: the error is for
    /// the rule dialog.
    pub fn save_rule(&mut self, editing: Option<&str>, rule: CommandRule) -> Result<(), String> {
        let mut rules = self.rules.clone();
        match editing.and_then(|p| rules.iter().position(|r| r.pattern == p)) {
            Some(at) => rules[at] = rule,
            None => rules.push(rule),
        }
        validate_command_rules(&rules).map_err(|e| e.to_string())?;
        self.rules = rules;
        Ok(())
    }

    /// What Save changes sends: only what differs from the saved state.
    pub fn changes(&self, state: &ViewState) -> PermissionsSave {
        let mut save = PermissionsSave::default();
        if let Some(channel) = state.channel(&self.channel_id)
            && channel.command_rules != self.rules
        {
            save.command_rules = Some(self.rules.clone());
        }
        for draft in &self.connections {
            let Some(saved) = state.connection(&draft.id) else {
                continue;
            };
            let changes = ConnectionChanges {
                approvals: (draft.approvals != saved.approvals).then_some(draft.approvals),
                tool_exceptions: (draft.exceptions != saved.tool_exceptions)
                    .then(|| draft.exceptions.clone()),
                ..ConnectionChanges::default()
            };
            if changes != ConnectionChanges::default() {
                save.connections.push((draft.id.clone(), changes));
            }
        }
        save
    }
}

/// The rule dialog: Add a rule, Edit rule or Override here.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleDialog {
    /// The pattern of the draft rule that Save replaces. `None` adds one.
    pub editing: Option<String>,
    /// The rule comes from Tutti.
    pub overriding: bool,
    pub result: CommandRuleResult,
    pub error: Option<String>,
}

impl RuleDialog {
    pub fn title(&self) -> &'static str {
        if self.overriding {
            "Override a rule from Tutti"
        } else if self.editing.is_some() {
            "Edit rule"
        } else {
            "Add a rule"
        }
    }

    pub fn confirm_label(&self) -> &'static str {
        if self.editing.is_some() || self.overriding {
            "Save rule"
        } else {
            "Add rule"
        }
    }
}

/// The tool picker of a connection.
#[derive(Clone, Debug, PartialEq)]
pub struct ExceptionPicker {
    pub connection_id: String,
}

/// The state of the Permissions dialog that is not a text input.
#[derive(Default)]
pub struct PermissionsState {
    pub form: PermissionsForm,
    pub rule: Option<RuleDialog>,
    pub picker: Option<ExceptionPicker>,
    pub commands_scroll: gpui_kit::ScrollHandle,
    pub connections_scroll: gpui_kit::ScrollHandle,
    pub picker_scroll: gpui_kit::ScrollHandle,
}

impl ConsortApp {
    /// Starts the draft when the dialog opens.
    pub fn prepare_permissions_dialog(&mut self, channel_id: &str) {
        self.convo.permissions = PermissionsState {
            form: PermissionsForm::open(&self.state, channel_id).unwrap_or_default(),
            ..PermissionsState::default()
        };
    }

    pub fn run_permissions_action(
        &mut self,
        action: PermissionsAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            PermissionsAction::Open(id) => self.open_dialog(Dialog::Permissions(id), window, cx),
            PermissionsAction::AddRule => {
                self.open_rule_dialog(None, None, false, window, cx);
            }
            PermissionsAction::EditRule(pattern) => {
                if let Some(rule) = self.convo.permissions.form.rule(&pattern).cloned() {
                    self.open_rule_dialog(Some(pattern), Some(rule), false, window, cx);
                }
            }
            PermissionsAction::RemoveRule(pattern) => {
                self.convo.permissions.form.remove_rule(&pattern)
            }
            PermissionsAction::OverrideRule(pattern) => {
                let rule = tutti_rules(&self.state)
                    .iter()
                    .find(|r| r.pattern == pattern)
                    .cloned();
                if let Some(rule) = rule {
                    self.open_rule_dialog(None, Some(rule), true, window, cx);
                }
            }
            PermissionsAction::RemoveException(connection, tool) => {
                if let Some(draft) = self.convo.permissions.form.connection_mut(&connection) {
                    draft.remove_exception(&tool);
                }
            }
            PermissionsAction::AddException(connection) => {
                self.open_exception_picker(connection, window, cx)
            }
        }
        cx.notify();
    }

    fn open_rule_dialog(
        &mut self,
        editing: Option<String>,
        rule: Option<CommandRule>,
        overriding: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (pattern, except, description, result) = match &rule {
            Some(r) => (
                r.pattern.clone(),
                except_text(&r.except),
                r.description.clone(),
                r.result,
            ),
            None => (
                String::new(),
                String::new(),
                String::new(),
                CommandRuleResult::Allow,
            ),
        };
        self.convo.permissions.rule = Some(RuleDialog {
            editing,
            overriding,
            result,
            error: None,
        });
        set_input(&self.convo_inputs.rule_pattern, &pattern, window, cx);
        set_input(&self.convo_inputs.rule_except, &except, window, cx);
        set_input(
            &self.convo_inputs.rule_description,
            &description,
            window,
            cx,
        );
        let input = self.convo_inputs.rule_pattern.clone();
        input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Takes the rule dialog into the draft, or shows why it cannot.
    pub fn save_rule_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.convo.permissions.rule.clone() else {
            return;
        };
        let read = |input: &gpui_kit::Entity<gpui_kit::component::input::InputState>,
                    cx: &gpui_kit::App| input.read(cx).value().to_string();
        let result = build_rule(
            &read(&self.convo_inputs.rule_pattern, cx),
            &read(&self.convo_inputs.rule_except, cx),
            dialog.result,
            &read(&self.convo_inputs.rule_description, cx),
        )
        .and_then(|rule| {
            self.convo
                .permissions
                .form
                .save_rule(dialog.editing.as_deref(), rule)
        });
        match result {
            Ok(()) => {
                self.convo.permissions.rule = None;
                self.focus.focus(window, cx);
            }
            Err(error) => {
                if let Some(open) = self.convo.permissions.rule.as_mut() {
                    open.error = Some(error);
                }
            }
        }
        cx.notify();
    }

    fn open_exception_picker(
        &mut self,
        connection_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(service) = self
            .state
            .connection(&connection_id)
            .map(|c| c.service.clone())
        else {
            return;
        };
        if self.state.tools_of(&service).is_none() {
            let client = self.client.clone();
            self.spawn_command(cx, async move {
                client.run(Command::LoadServiceTools { service }).await
            });
        }
        self.convo.permissions.picker = Some(ExceptionPicker { connection_id });
        self.convo.permissions.picker_scroll = gpui_kit::ScrollHandle::new();
        set_input(&self.convo_inputs.exception_search, "", window, cx);
        let input = self.convo_inputs.exception_search.clone();
        input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Gives a tool of the open picker its own rule, and closes the picker.
    pub fn pick_exception_tool(&mut self, tool: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.convo.permissions.picker.take() else {
            return;
        };
        let service = self
            .state
            .connection(&picker.connection_id)
            .map(|c| c.service.clone())
            .unwrap_or_default();
        let kind = self
            .state
            .tools_of(&service)
            .and_then(|t| t.tools.iter().find(|x| x.slug == tool))
            .map(|t| t.kind)
            .unwrap_or_default();
        let name = tool_display_name(&service, tool);
        if let Some(draft) = self
            .convo
            .permissions
            .form
            .connection_mut(&picker.connection_id)
        {
            let rule = default_exception_rule(draft.approvals.rule_for(kind));
            draft.set_exception(tool, &name, rule);
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Escape closes only the rule dialog or the picker, whichever is on
    /// top. The keys do not reach the dialog under it.
    pub fn permissions_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let rule_open = self.convo.permissions.rule.is_some();
        let picker_open = self.convo.permissions.picker.is_some();
        if !rule_open && !picker_open {
            return false;
        }
        if key == "escape" {
            if picker_open {
                self.convo.permissions.picker = None;
            } else {
                self.convo.permissions.rule = None;
            }
            self.focus.focus(window, cx);
            cx.notify();
        }
        true
    }

    /// Handles a pick in a select of the Permissions dialog. Returns false
    /// when the select is not one of them.
    pub fn pick_permissions_select(
        &mut self,
        select: &str,
        option: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        match select {
            RULE_SELECT => {
                if let Some((connection, kind, rule)) = parse_rule_option(option)
                    && let Some(draft) = self.convo.permissions.form.connection_mut(&connection)
                {
                    draft.approvals = draft.approvals.with(kind, rule);
                }
            }
            EXCEPTION_SELECT => {
                if let Some((connection, tool, rule)) = parse_exception_option(option)
                    && let Some(draft) = self.convo.permissions.form.connection_mut(&connection)
                    && let Some(exception) = draft.exceptions.iter_mut().find(|e| e.tool == tool)
                {
                    exception.rule = rule;
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Sends the changes of the Permissions dialog: the rules of the
    /// channel, then each connection with a change.
    pub fn save_permissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = self.convo.permissions.form.clone();
        let save = form.changes(&self.state);
        if save.is_empty() {
            self.close_dialog(window, cx);
            return;
        }
        let client = self.client.clone();
        self.submit_dialog(
            async move {
                if let Some(rules) = save.command_rules {
                    client
                        .update_channel(
                            form.channel_id.clone(),
                            ChannelChanges {
                                command_rules: Some(rules),
                                ..ChannelChanges::default()
                            },
                        )
                        .await?;
                }
                for (connection_id, changes) in save.connections {
                    client
                        .run(Command::UpdateConnection {
                            connection_id,
                            changes,
                        })
                        .await?;
                }
                Ok(())
            },
            window,
            cx,
        );
    }

    /// The other channels where a connection works, for "Also in ...".
    pub fn other_channels_of(&self, connection: &ConnectionView, channel_id: &str) -> Vec<String> {
        matching_channels(&connection.use_in, &self.state.channels)
            .into_iter()
            .filter(|c| c.id != channel_id)
            .map(|c| c.name.clone())
            .collect()
    }
}

/// Where else a connection works. A long list of names would crowd the
/// name of the connection, so more than two become a count.
pub fn also_in(others: &[&str]) -> String {
    if others.len() > 2 {
        format!("Also in {} channels", others.len())
    } else {
        format!("Also in {}", consort_client::convo::join_names(others))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_client::{ChannelView, ConnectionStatusView, LoadState, UseInView};

    fn rule(pattern: &str, result: CommandRuleResult, source: CommandRuleSource) -> CommandRule {
        CommandRule {
            pattern: pattern.into(),
            except: Vec::new(),
            result,
            description: format!("Does {pattern}"),
            source,
        }
    }

    fn channel(id: &str, kind: ChannelKind, rules: Vec<CommandRule>) -> ChannelView {
        ChannelView {
            id: id.into(),
            kind,
            name: id.into(),
            command_rules: rules,
            ..ChannelView::default()
        }
    }

    fn connection(id: &str, use_in: UseInView) -> ConnectionView {
        ConnectionView {
            id: id.into(),
            name: id.into(),
            service: "github".into(),
            use_in,
            ..ConnectionView::default()
        }
    }

    fn state() -> ViewState {
        let user = CommandRuleSource::User;
        ViewState {
            channels: vec![
                channel(
                    "tutti",
                    ChannelKind::Tutti,
                    vec![
                        rule("rm -rf", CommandRuleResult::Ask, user),
                        rule("git push", CommandRuleResult::Ask, user),
                    ],
                ),
                channel(
                    "app",
                    ChannelKind::Standard,
                    vec![
                        rule("git push", CommandRuleResult::Allow, user),
                        rule(
                            "gh pr merge",
                            CommandRuleResult::Ask,
                            CommandRuleSource::Assistant,
                        ),
                    ],
                ),
                channel("empty", ChannelKind::Standard, Vec::new()),
            ],
            connections: vec![
                {
                    let mut c = connection("github", UseInView::EveryChannel);
                    c.tool_exceptions = vec![ToolExceptionView {
                        tool: "GITHUB_MERGE_A_PULL_REQUEST".into(),
                        name: "Merge a pull request".into(),
                        rule: ApprovalRuleView::AskEveryTime,
                    }];
                    c
                },
                connection("notion", UseInView::Channels(vec!["empty".into()])),
            ],
            ..ViewState::default()
        }
    }

    #[test]
    fn the_counts_include_the_rules_of_tutti_that_the_channel_does_not_override() {
        let state = state();
        // Own: git push, gh pr merge. From Tutti: rm -rf. The git push of
        // Tutti is overridden.
        assert_eq!(effective_rule_count(&state, "app"), 3);
        // Tutti counts only its own rules.
        assert_eq!(effective_rule_count(&state, "tutti"), 2);
        assert_eq!(effective_rule_count(&state, "empty"), 2);
        assert_eq!(effective_rule_count(&state, "gone"), 0);
        assert_eq!(
            access_detail(&state, "app"),
            "3 command rules, 1 tool exception"
        );
    }

    #[test]
    fn a_connection_counts_only_where_the_channel_has_it() {
        let state = state();
        assert_eq!(exception_count(&state, "empty"), 1);
        let ids: Vec<_> = available_connections(&state, "empty")
            .iter()
            .map(|c| c.id.clone())
            .collect();
        assert_eq!(ids, ["github", "notion"]);
        let ids: Vec<_> = available_connections(&state, "app")
            .iter()
            .map(|c| c.id.clone())
            .collect();
        assert_eq!(ids, ["github"]);
    }

    #[test]
    fn the_detail_line_says_no_when_there_is_nothing() {
        let state = ViewState {
            channels: vec![
                channel("tutti", ChannelKind::Tutti, Vec::new()),
                channel(
                    "a",
                    ChannelKind::Standard,
                    vec![rule(
                        "ls",
                        CommandRuleResult::Allow,
                        CommandRuleSource::User,
                    )],
                ),
            ],
            ..ViewState::default()
        };
        assert_eq!(
            access_detail(&state, "a"),
            "1 command rule, no tool exceptions"
        );
        assert_eq!(
            access_detail(&state, "tutti"),
            "No command rules, no tool exceptions"
        );
    }

    #[test]
    fn a_channel_shows_the_groups_and_tutti_shows_no_third_group() {
        let state = state();
        let app = state.channel("app").unwrap();
        let groups = rule_groups(&app.command_rules, tutti_rules(&state), false);
        assert_eq!(groups.allow.len(), 1);
        assert_eq!(groups.ask.len(), 1);
        let from_tutti: Vec<_> = groups.tutti.iter().map(|r| r.pattern.as_str()).collect();
        assert_eq!(from_tutti, ["rm -rf"]);
        let tutti = state.tutti().unwrap();
        let groups = rule_groups(&tutti.command_rules, tutti_rules(&state), true);
        assert!(groups.tutti.is_empty());
        assert_eq!(groups.ask.len(), 2);
        assert!(rule_groups(&[], &[], false).is_empty());
    }

    #[test]
    fn the_note_of_a_rule_names_its_exceptions_and_who_added_it() {
        let mut r = rule(
            "git push",
            CommandRuleResult::Allow,
            CommandRuleSource::Assistant,
        );
        assert_eq!(rule_note(&r).as_deref(), Some("Added by the assistant."));
        r.except = vec!["--force".into(), "main".into()];
        assert_eq!(
            rule_note(&r).as_deref(),
            Some("Except --force, main. Added by the assistant.")
        );
        r.source = CommandRuleSource::User;
        assert_eq!(rule_note(&r).as_deref(), Some("Except --force, main."));
        r.except.clear();
        assert_eq!(rule_note(&r), None);
    }

    #[test]
    fn the_except_field_splits_at_commas() {
        assert_eq!(
            parse_except(" --force, main ,, -f"),
            ["--force", "main", "-f"]
        );
        assert!(parse_except("  ").is_empty());
        assert_eq!(except_text(&parse_except("a,b")), "a, b");
    }

    #[test]
    fn a_rule_from_the_dialog_is_the_rule_of_the_user_or_says_why_not() {
        let rule = build_rule(
            " git   push ",
            "--force, main",
            CommandRuleResult::Allow,
            " Push ",
        )
        .unwrap();
        assert_eq!(rule.pattern, "git push");
        assert_eq!(rule.except, ["--force", "main"]);
        assert_eq!(rule.description, "Push");
        assert_eq!(rule.source, CommandRuleSource::User);
        let error = build_rule("", "", CommandRuleResult::Ask, "x").unwrap_err();
        assert!(error.contains("needs a pattern"), "{error}");
        let error = build_rule("git push; rm", "", CommandRuleResult::Ask, "x").unwrap_err();
        assert!(error.contains("shell text"), "{error}");
        assert!(build_rule("ls", "", CommandRuleResult::Ask, " ").is_err());
    }

    #[test]
    fn the_draft_adds_edits_and_removes_a_rule_and_keeps_its_list_on_an_error() {
        let state = state();
        let mut form = PermissionsForm::open(&state, "app").unwrap();
        let user = |p: &str| build_rule(p, "", CommandRuleResult::Allow, "A rule").unwrap();
        form.save_rule(None, user("ls")).unwrap();
        assert_eq!(form.rules.len(), 3);
        // A second rule for a pattern is an error, and the list stays.
        let error = form.save_rule(None, user("ls")).unwrap_err();
        assert!(error.contains("two rules"), "{error}");
        assert_eq!(form.rules.len(), 3);
        // An edit replaces the rule in its place and makes it the user's.
        form.save_rule(Some("gh pr merge"), user("gh pr close"))
            .unwrap();
        assert_eq!(form.rules[1].pattern, "gh pr close");
        assert_eq!(form.rules[1].source, CommandRuleSource::User);
        // An edit may keep its own pattern.
        form.save_rule(Some("ls"), user("ls")).unwrap();
        assert_eq!(form.rules.len(), 3);
        form.remove_rule("ls");
        assert_eq!(form.rules.len(), 2);
    }

    #[test]
    fn an_override_adds_a_rule_to_the_channel_and_hides_the_one_of_tutti() {
        let state = state();
        let mut form = PermissionsForm::open(&state, "empty").unwrap();
        let tutti = tutti_rules(&state).to_vec();
        assert_eq!(inherited_rules(&form.rules, &tutti, false).len(), 2);
        let mut copy = tutti
            .iter()
            .find(|r| r.pattern == "rm -rf")
            .unwrap()
            .clone();
        copy.result = CommandRuleResult::Allow;
        copy.source = CommandRuleSource::User;
        form.save_rule(None, copy).unwrap();
        let left: Vec<_> = inherited_rules(&form.rules, &tutti, false)
            .iter()
            .map(|r| r.pattern.clone())
            .collect();
        assert_eq!(left, ["git push"]);
        let save = form.changes(&state);
        assert_eq!(save.command_rules.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn save_sends_only_what_changed() {
        let state = state();
        let mut form = PermissionsForm::open(&state, "app").unwrap();
        assert!(form.changes(&state).is_empty());
        // A connection change goes to that connection only, with only the
        // fields that changed.
        form.connection_mut("github").unwrap().approvals = ToolApprovalsView::default()
            .with(ToolKindView::Delete, ApprovalRuleView::RunWithoutAsking);
        let save = form.changes(&state);
        assert_eq!(save.command_rules, None);
        assert_eq!(save.connections.len(), 1);
        let (id, changes) = &save.connections[0];
        assert_eq!(id, "github");
        assert!(changes.approvals.is_some());
        assert_eq!(changes.tool_exceptions, None);
        // Put it back and add an exception: only the exceptions go.
        form.connection_mut("github").unwrap().approvals =
            state.connection("github").unwrap().approvals;
        form.connection_mut("github").unwrap().set_exception(
            "GITHUB_CREATE_A_PULL_REQUEST",
            "Create a pull request",
            ApprovalRuleView::RunWithoutAsking,
        );
        let save = form.changes(&state);
        let (_, changes) = &save.connections[0];
        assert_eq!(changes.approvals, None);
        assert_eq!(changes.tool_exceptions.as_ref().map(Vec::len), Some(2));
        // The rules go as a whole list.
        form.remove_rule("git push");
        assert_eq!(
            form.changes(&state).command_rules.as_ref().map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn an_exception_is_added_changed_and_removed() {
        let state = state();
        let mut form = PermissionsForm::open(&state, "app").unwrap();
        let draft = form.connection_mut("github").unwrap();
        draft.set_exception(
            "github_merge_a_pull_request",
            "Merge a pull request",
            ApprovalRuleView::RunWithoutAsking,
        );
        assert_eq!(draft.exceptions.len(), 1, "a slug has no case");
        assert_eq!(draft.exceptions[0].rule, ApprovalRuleView::RunWithoutAsking);
        draft.remove_exception("GITHUB_MERGE_A_PULL_REQUEST");
        assert!(draft.exceptions.is_empty());
        assert_eq!(
            default_exception_rule(ApprovalRuleView::RunWithoutAsking),
            ApprovalRuleView::AskEveryTime
        );
        assert_eq!(
            default_exception_rule(ApprovalRuleView::AskOncePerTask),
            ApprovalRuleView::RunWithoutAsking
        );
        assert!(!EXCEPTION_RULES.contains(&ApprovalRuleView::AskOncePerTask));
    }

    #[test]
    fn the_picker_searches_the_tools_and_skips_the_ones_with_an_exception() {
        let tool = |slug: &str, description: &str| ConnectorToolView {
            slug: slug.into(),
            description: description.into(),
            kind: ToolKindView::Change,
        };
        let tools = ServiceToolsView {
            service: "github".into(),
            tools: vec![
                tool("GITHUB_MERGE_A_PULL_REQUEST", ""),
                tool("GITHUB_CREATE_A_PULL_REQUEST", "Open a PR"),
                tool("GITHUB_DELETE_A_BRANCH", ""),
            ],
            load: LoadState::Loaded,
        };
        let state = state();
        let form = PermissionsForm::open(&state, "app").unwrap();
        let draft = form.connection("github").unwrap();
        let names = |q: &str| -> Vec<String> {
            exception_candidates(&tools, draft, q, "github")
                .iter()
                .map(|t| t.slug.clone())
                .collect()
        };
        // The merge tool has an exception already.
        assert_eq!(names("").len(), 2);
        assert_eq!(names("PULL request"), ["GITHUB_CREATE_A_PULL_REQUEST"]);
        assert_eq!(names("open a pr"), ["GITHUB_CREATE_A_PULL_REQUEST"]);
        assert_eq!(names("branch"), ["GITHUB_DELETE_A_BRANCH"]);
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn the_options_of_the_selects_read_back() {
        let option = rule_option(
            "con_1",
            ToolKindView::Delete,
            ApprovalRuleView::AskOncePerTask,
        );
        assert_eq!(
            parse_rule_option(&option),
            Some((
                "con_1".to_owned(),
                ToolKindView::Delete,
                ApprovalRuleView::AskOncePerTask
            ))
        );
        let option = exception_option("con_1", "GITHUB_X", ApprovalRuleView::AskEveryTime);
        assert_eq!(
            parse_exception_option(&option),
            Some((
                "con_1".to_owned(),
                "GITHUB_X".to_owned(),
                ApprovalRuleView::AskEveryTime
            ))
        );
        assert_eq!(parse_rule_option("nonsense"), None);
        assert_eq!(parse_exception_option("a\u{1f}b"), None);
    }

    #[test]
    fn the_answered_line_names_the_place_and_counts_the_changes() {
        assert_eq!(
            answered_headline("Change what consort-app can do", 5),
            "Permissions of consort-app: 5 changes"
        );
        assert_eq!(
            answered_headline("Change what every channel can do", 1),
            "Permissions of every channel: 1 change"
        );
        assert_eq!(answered_headline("Make it so", 2), "Permissions: 2 changes");
    }

    #[test]
    fn a_connection_that_is_still_connecting_has_no_row() {
        let mut state = state();
        state.connections[1].status = ConnectionStatusView::Connecting {
            redirect_url: None,
            expires_at: 0,
        };
        assert_eq!(available_connections(&state, "empty").len(), 1);
    }

    #[test]
    fn the_rule_dialog_has_its_words() {
        let mut dialog = RuleDialog {
            editing: None,
            overriding: false,
            result: CommandRuleResult::Allow,
            error: None,
        };
        assert_eq!(
            (dialog.title(), dialog.confirm_label()),
            ("Add a rule", "Add rule")
        );
        dialog.editing = Some("ls".into());
        assert_eq!(
            (dialog.title(), dialog.confirm_label()),
            ("Edit rule", "Save rule")
        );
        dialog.editing = None;
        dialog.overriding = true;
        assert_eq!(dialog.confirm_label(), "Save rule");
    }

    #[test]
    fn a_long_list_of_other_channels_becomes_a_count() {
        assert_eq!(also_in(&["acme"]), "Also in acme");
        assert_eq!(also_in(&["acme", "hiring"]), "Also in acme and hiring");
        assert_eq!(also_in(&["a", "b", "c"]), "Also in 3 channels");
    }
}
