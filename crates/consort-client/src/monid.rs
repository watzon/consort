//! Monid in the view state: paid tools with one wallet (ADR 0016).
//!
//! This file holds the view records of Monid, the conversions to and from
//! the wire types, the words that the cards and pages use, and the pure
//! functions that the desktop shares. Money is whole millionths of a
//! dollar, so a long sum does not drift.

use std::collections::BTreeMap;

use consort_proto::{
    client_api::MonidCatalogTool,
    model::{
        MonidAllowedTool, MonidCall, MonidCallNotice, MonidCallState, MonidCard,
        MonidChannelAccess, MonidLimits, MonidSettings, MonidStatus, MonidToolRef, MonidUnattended,
    },
};
use serde_json::Value;

use crate::{
    ClientError,
    connectors::{ApprovalRuleView, LoadState, UseInView, use_in_matches, use_in_short},
    extensions_view::{rule_proto, rule_view, use_in_proto, use_in_view},
    files::AttachmentView,
    format,
    view::{ChannelView, ViewState},
};

/// The link that adds money. Monid has no API for it.
pub const MONID_WALLET_URL: &str = "https://app.monid.ai/wallet";
/// Where the user makes a key.
pub const MONID_SITE: &str = "app.monid.ai";

/// The kind of a Monid call row, as the notice view names it.
pub const MONID_CALL_KIND: &str = "monid_call";

const MICROS_PER_DOLLAR: i64 = 1_000_000;

// Money ------------------------------------------------------------------------

/// Millionths of a dollar as text: "$0.0035", "$1.24", "$0". It is the same
/// format as the hub's.
pub fn money_text(micros: i64) -> String {
    if micros == 0 {
        return "$0".to_owned();
    }
    let sign = if micros < 0 { "-" } else { "" };
    let abs = micros.unsigned_abs();
    let whole = abs / MICROS_PER_DOLLAR as u64;
    let frac = abs % MICROS_PER_DOLLAR as u64;
    if frac.is_multiple_of(10_000) {
        return format!("{sign}${whole}.{:02}", frac / 10_000);
    }
    let digits = format!("{frac:06}");
    format!("{sign}${whole}.{}", digits.trim_end_matches('0'))
}

/// The amount in a limit field as millionths of a dollar. An empty field is
/// `Ok(None)`: no limit. "$5", "5.00" and "1,250.5" work. A text that is
/// not an amount above zero is an error that says what to type.
pub fn parse_limit(text: &str) -> Result<Option<i64>, String> {
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '$' && *c != ',')
        .collect();
    if cleaned.is_empty() {
        return Ok(None);
    }
    let bad =
        || "Type an amount in dollars, such as 5 or 0.50. Leave it empty for no limit.".to_owned();
    let (whole, frac) = match cleaned.split_once('.') {
        Some((w, f)) => (w, f),
        None => (cleaned.as_str(), ""),
    };
    if (whole.is_empty() && frac.is_empty())
        || !whole.chars().all(|c| c.is_ascii_digit())
        || !frac.chars().all(|c| c.is_ascii_digit())
        || frac.len() > 6
        || whole.len() > 9
    {
        return Err(bad());
    }
    let whole: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| bad())?
    };
    let frac_micros: i64 = if frac.is_empty() {
        0
    } else {
        let padded = format!("{frac:0<6}");
        padded.parse().map_err(|_| bad())?
    };
    let micros = whole * MICROS_PER_DOLLAR + frac_micros;
    if micros <= 0 {
        return Err("A limit must be more than $0. Leave it empty for no limit.".to_owned());
    }
    Ok(Some(micros))
}

/// The text of a limit in its field: "5" or "0.50". Empty for no limit.
pub fn limit_field_text(micros: Option<i64>) -> String {
    match micros {
        None => String::new(),
        Some(m) => money_text(m).trim_start_matches('$').to_owned(),
    }
}

// Settings ---------------------------------------------------------------------

/// What a routine or a proactive turn may do with Monid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MonidUnattendedView {
    Off,
    #[default]
    Ask,
    RunWithoutAsking,
}

impl MonidUnattendedView {
    pub const ALL: [MonidUnattendedView; 3] = [
        MonidUnattendedView::Off,
        MonidUnattendedView::Ask,
        MonidUnattendedView::RunWithoutAsking,
    ];

    /// The words of the segment.
    pub fn label(self) -> &'static str {
        match self {
            MonidUnattendedView::Off => "Off",
            MonidUnattendedView::Ask => "Ask",
            MonidUnattendedView::RunWithoutAsking => "Run without asking",
        }
    }

    /// A stable key for a select.
    pub fn key(self) -> &'static str {
        match self {
            MonidUnattendedView::Off => "off",
            MonidUnattendedView::Ask => "ask",
            MonidUnattendedView::RunWithoutAsking => "run_without_asking",
        }
    }

    pub fn from_key(key: &str) -> Option<MonidUnattendedView> {
        MonidUnattendedView::ALL
            .into_iter()
            .find(|v| v.key() == key)
    }

    pub fn to_proto(self) -> MonidUnattended {
        match self {
            MonidUnattendedView::Off => MonidUnattended::Off,
            MonidUnattendedView::Ask => MonidUnattended::Ask,
            MonidUnattendedView::RunWithoutAsking => MonidUnattended::RunWithoutAsking,
        }
    }

    pub fn from_proto(value: MonidUnattended) -> MonidUnattendedView {
        match value {
            MonidUnattended::Off => MonidUnattendedView::Off,
            MonidUnattended::Ask => MonidUnattendedView::Ask,
            MonidUnattended::RunWithoutAsking => MonidUnattendedView::RunWithoutAsking,
        }
    }
}

/// The limits that the user set. `None` is no limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MonidLimitsView {
    pub per_call_micros: Option<i64>,
    pub per_task_micros: Option<i64>,
    pub per_channel_month_micros: Option<i64>,
    pub per_hub_month_micros: Option<i64>,
}

impl MonidLimitsView {
    /// Whether the user set any limit.
    pub fn any(&self) -> bool {
        self.per_call_micros.is_some()
            || self.per_task_micros.is_some()
            || self.per_channel_month_micros.is_some()
            || self.per_hub_month_micros.is_some()
    }

    /// The limits in a plain sentence part, such as "$1 for each call and
    /// $20 for the hub each month". Empty when no limit is set.
    pub fn sentence(&self) -> String {
        let mut parts = Vec::new();
        if let Some(m) = self.per_call_micros {
            parts.push(format!("{} for each call", money_text(m)));
        }
        if let Some(m) = self.per_task_micros {
            parts.push(format!("{} for each task", money_text(m)));
        }
        if let Some(m) = self.per_channel_month_micros {
            parts.push(format!("{} for each channel each month", money_text(m)));
        }
        if let Some(m) = self.per_hub_month_micros {
            parts.push(format!("{} for the hub each month", money_text(m)));
        }
        crate::connectors::join_and(&parts)
    }
}

/// The Monid settings of the hub.
#[derive(Clone, Debug, PartialEq)]
pub struct MonidSettingsView {
    pub use_in: UseInView,
    /// The rule for each call, unless a channel changes it.
    pub approval: ApprovalRuleView,
    pub routines: MonidUnattendedView,
    pub proactive: MonidUnattendedView,
    pub limits: MonidLimitsView,
}

impl Default for MonidSettingsView {
    /// The hub's defaults: ask every time, routines ask, proactive turns off,
    /// no limits.
    fn default() -> Self {
        MonidSettingsView {
            use_in: UseInView::EveryChannel,
            approval: ApprovalRuleView::AskEveryTime,
            routines: MonidUnattendedView::Ask,
            proactive: MonidUnattendedView::Off,
            limits: MonidLimitsView::default(),
        }
    }
}

impl MonidSettingsView {
    pub fn from_proto(settings: &MonidSettings) -> MonidSettingsView {
        MonidSettingsView {
            use_in: use_in_view(&settings.use_in),
            approval: rule_view(settings.approval),
            routines: MonidUnattendedView::from_proto(settings.routines),
            proactive: MonidUnattendedView::from_proto(settings.proactive),
            limits: MonidLimitsView {
                per_call_micros: settings.limits.per_call_micros,
                per_task_micros: settings.limits.per_task_micros,
                per_channel_month_micros: settings.limits.per_channel_month_micros,
                per_hub_month_micros: settings.limits.per_hub_month_micros,
            },
        }
    }

    pub fn to_proto(&self) -> Result<MonidSettings, ClientError> {
        Ok(MonidSettings {
            use_in: use_in_proto(&self.use_in)?,
            approval: rule_proto(self.approval),
            routines: self.routines.to_proto(),
            proactive: self.proactive.to_proto(),
            limits: MonidLimits {
                per_call_micros: self.limits.per_call_micros,
                per_task_micros: self.limits.per_task_micros,
                per_channel_month_micros: self.limits.per_channel_month_micros,
                per_hub_month_micros: self.limits.per_hub_month_micros,
            },
        })
    }
}

/// What one channel spent this month.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonidSpendView {
    pub channel_id: String,
    pub channel_name: String,
    pub calls: u32,
    pub cost_micros: i64,
}

impl MonidSpendView {
    /// "83 calls" or "1 call".
    pub fn calls_label(&self) -> String {
        calls_label(self.calls)
    }
}

/// "1 call", "83 calls".
pub fn calls_label(calls: u32) -> String {
    if calls == 1 {
        "1 call".to_owned()
    } else {
        format!("{calls} calls")
    }
}

// Channel access -----------------------------------------------------------------

/// A provider or one tool of a provider.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MonidToolView {
    pub provider: String,
    /// `None` is every tool of the provider.
    pub endpoint: Option<String>,
    /// The name to show, such as "ElevenLabs text to speech".
    pub name: String,
}

impl MonidToolView {
    /// The row title: a provider reads "All Apify tools".
    pub fn title(&self) -> String {
        match self.endpoint {
            Some(_) => self.name.clone(),
            None => format!("All {} tools", self.name_or_provider()),
        }
    }

    fn name_or_provider(&self) -> String {
        if self.name.trim().is_empty() {
            self.provider.clone()
        } else {
            self.name.clone()
        }
    }

    pub fn is_provider(&self) -> bool {
        self.endpoint.is_none()
    }

    fn same(&self, other: &MonidToolView) -> bool {
        self.provider.eq_ignore_ascii_case(&other.provider) && self.endpoint == other.endpoint
    }

    fn from_proto(tool: &MonidToolRef) -> MonidToolView {
        MonidToolView {
            provider: tool.provider.clone(),
            endpoint: tool.endpoint.clone(),
            name: tool.name.clone(),
        }
    }

    fn to_proto(&self) -> MonidToolRef {
        MonidToolRef {
            provider: self.provider.clone(),
            endpoint: self.endpoint.clone(),
            name: self.name.clone(),
        }
    }
}

/// One entry of a channel's list of allowed tools.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MonidAllowedView {
    pub tool: MonidToolView,
    /// The user chose "Always allow this tool here".
    pub run_without_asking: bool,
}

/// What one channel may do with Monid. The default changes nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonidAccessView {
    /// Monid is off in this channel, as an exception to "Use in".
    pub disabled: bool,
    /// The channel's own rule. `None` uses the hub's rule.
    pub approval: Option<ApprovalRuleView>,
    /// "Only these tools".
    pub only_allowed: bool,
    pub allowed: Vec<MonidAllowedView>,
    pub blocked: Vec<MonidToolView>,
}

impl MonidAccessView {
    pub fn from_proto(access: &MonidChannelAccess) -> MonidAccessView {
        MonidAccessView {
            disabled: access.disabled,
            approval: access.approval.map(rule_view),
            only_allowed: access.only_allowed,
            allowed: access
                .allowed
                .iter()
                .map(|a| MonidAllowedView {
                    tool: MonidToolView::from_proto(&a.tool),
                    run_without_asking: a.run_without_asking,
                })
                .collect(),
            blocked: access
                .blocked
                .iter()
                .map(MonidToolView::from_proto)
                .collect(),
        }
    }

    pub fn to_proto(&self) -> MonidChannelAccess {
        MonidChannelAccess {
            disabled: self.disabled,
            approval: self.approval.map(rule_proto),
            only_allowed: self.only_allowed,
            allowed: self
                .allowed
                .iter()
                .map(|a| MonidAllowedTool {
                    tool: a.tool.to_proto(),
                    run_without_asking: a.run_without_asking,
                })
                .collect(),
            blocked: self.blocked.iter().map(MonidToolView::to_proto).collect(),
        }
    }

    /// The access with a tool or a provider added to the list. A block of
    /// the same entry goes away, because the user now wants it.
    pub fn with_allowed(&self, tool: MonidToolView) -> MonidAccessView {
        let mut next = self.clone();
        next.blocked.retain(|b| !b.same(&tool));
        if !next.allowed.iter().any(|a| a.tool.same(&tool)) {
            next.allowed.push(MonidAllowedView {
                tool,
                run_without_asking: false,
            });
        }
        next
    }

    /// The access with a tool or a provider that this channel never uses.
    /// The same entry leaves the list of allowed tools.
    pub fn with_blocked(&self, tool: MonidToolView) -> MonidAccessView {
        let mut next = self.clone();
        next.allowed.retain(|a| !a.tool.same(&tool));
        if !next.blocked.iter().any(|b| b.same(&tool)) {
            next.blocked.push(tool);
        }
        next
    }

    /// The access without the entry at `index` of the allowed list.
    pub fn without_allowed(&self, index: usize) -> MonidAccessView {
        let mut next = self.clone();
        if index < next.allowed.len() {
            next.allowed.remove(index);
        }
        next
    }

    /// The access without the entry at `index` of the blocked list.
    pub fn without_blocked(&self, index: usize) -> MonidAccessView {
        let mut next = self.clone();
        if index < next.blocked.len() {
            next.blocked.remove(index);
        }
        next
    }

    pub fn with_only_allowed(&self, only: bool) -> MonidAccessView {
        MonidAccessView {
            only_allowed: only,
            ..self.clone()
        }
    }

    pub fn with_approval(&self, rule: Option<ApprovalRuleView>) -> MonidAccessView {
        MonidAccessView {
            approval: rule,
            ..self.clone()
        }
    }

    pub fn with_disabled(&self, disabled: bool) -> MonidAccessView {
        MonidAccessView {
            disabled,
            ..self.clone()
        }
    }
}

// Hub state ------------------------------------------------------------------------

/// One tool found by a search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonidSearchHit {
    pub provider: String,
    pub provider_name: String,
    pub endpoint: String,
    pub name: String,
    pub rate: String,
}

impl MonidSearchHit {
    pub fn from_proto(tool: &MonidCatalogTool) -> MonidSearchHit {
        MonidSearchHit {
            provider: tool.provider.clone(),
            provider_name: tool.provider_name.clone(),
            endpoint: tool.endpoint.clone(),
            name: tool.name.clone(),
            rate: tool.rate.clone(),
        }
    }

    /// The tool, to add to a list.
    pub fn tool(&self) -> MonidToolView {
        MonidToolView {
            provider: self.provider.clone(),
            endpoint: Some(self.endpoint.clone()),
            name: self.name.clone(),
        }
    }

    /// The whole provider, to add to a list.
    pub fn provider_tool(&self) -> MonidToolView {
        MonidToolView {
            provider: self.provider.clone(),
            endpoint: None,
            name: self.provider_name.clone(),
        }
    }
}

/// The search of a channel's list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonidSearchView {
    pub query: String,
    pub hits: Vec<MonidSearchHit>,
    pub load: LoadState,
}

/// One call in the list of recent calls of a channel.
#[derive(Clone, Debug, PartialEq)]
pub struct MonidRecentCallView {
    pub call_id: String,
    pub name: String,
    pub state: MonidCallStateView,
    pub cost_micros: i64,
    pub time_label: String,
    /// "$0.0496", or the words of a call that did not charge.
    pub summary: String,
}

/// The recent calls of one channel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MonidCallsView {
    pub calls: Vec<MonidRecentCallView>,
    pub load: LoadState,
}

/// The Monid state of the hub, as the client loads it with
/// [`crate::Command::LoadMonidStatus`]. The key never comes back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MonidView {
    pub load: LoadState,
    pub key_present: bool,
    /// The last characters of the key, such as "3f9a".
    pub key_hint: Option<String>,
    pub balance_micros: Option<i64>,
    /// What the hub could not read, with what to do next.
    pub error: Option<String>,
    pub settings: MonidSettingsView,
    pub spent_month_micros: i64,
    pub spend: Vec<MonidSpendView>,
    /// The search of the list in the Channel panel.
    pub search: MonidSearchView,
    /// The recent calls of each channel, by channel ID.
    pub calls: BTreeMap<String, MonidCallsView>,
}

impl MonidView {
    /// Fills the status fields from the hub's answer.
    pub fn set_status(&mut self, status: MonidStatus, channels: &[(String, String)]) {
        self.load = LoadState::Loaded;
        self.key_present = status.key_present;
        self.key_hint = status.key_hint;
        self.balance_micros = status.balance_micros;
        self.error = status.error;
        self.settings = MonidSettingsView::from_proto(&status.settings);
        self.spent_month_micros = status.spent_month_micros;
        self.spend = status
            .spend
            .iter()
            .map(|s| {
                let id = s.channel_id.to_string();
                MonidSpendView {
                    channel_name: channels
                        .iter()
                        .find(|(cid, _)| *cid == id)
                        .map(|(_, name)| name.clone())
                        .unwrap_or_else(|| "A removed channel".to_owned()),
                    channel_id: id,
                    calls: s.calls,
                    cost_micros: s.cost_micros,
                }
            })
            .collect();
    }

    /// "Balance $49.34." Empty when the hub could not read it.
    pub fn balance_text(&self) -> Option<String> {
        self.balance_micros.map(money_text)
    }

    /// What one channel spent this month, in millionths of a dollar.
    pub fn spent_in(&self, channel_id: &str) -> i64 {
        self.spend
            .iter()
            .find(|s| s.channel_id == channel_id)
            .map_or(0, |s| s.cost_micros)
    }

    /// The row of Settings > Connectors under the list: "$49.34 left.
    /// $3.87 spent this month."
    pub fn footer_detail(&self) -> String {
        let spent = format!("{} spent this month.", money_text(self.spent_month_micros));
        match self.balance_micros {
            Some(b) => format!("{} left. {spent}", money_text(b)),
            None => spent,
        }
    }

    /// The text under the Monid title: "Balance $49.34. Paid tools from many
    /// providers, in one wallet."
    pub fn page_subtitle(&self) -> String {
        match self.balance_micros {
            Some(b) => format!(
                "Balance {}. Paid tools from many providers, in one wallet.",
                money_text(b)
            ),
            None => "Paid tools from many providers, in one wallet.".to_owned(),
        }
    }

    /// "Ends in 3f9a".
    pub fn key_text(&self) -> String {
        match self.key_hint.as_deref().filter(|h| !h.is_empty()) {
            Some(hint) => format!("Ends in {}", hint.trim_start_matches('…')),
            None => "Saved on the hub".to_owned(),
        }
    }
}

// The row of the Connectors tab ------------------------------------------------------

/// "Asks before each call", "Asks once per task" or "Runs without asking".
pub fn rule_phrase(rule: ApprovalRuleView) -> &'static str {
    match rule {
        ApprovalRuleView::AskEveryTime => "Asks before each call",
        ApprovalRuleView::AskOncePerTask => "Asks once per task",
        ApprovalRuleView::RunWithoutAsking => "Runs without asking",
    }
}

/// The rule that a channel follows: its own, or the hub's.
pub fn effective_rule(settings: &MonidSettingsView, access: &MonidAccessView) -> ApprovalRuleView {
    access.approval.unwrap_or(settings.approval)
}

/// Whether the hub gives Monid to the channel: it has a key and "Use in"
/// matches, or the channel has an exception. A channel with an exception
/// shows the row so the user can turn it on again.
pub fn available_in(monid: &MonidView, channel: &ChannelView) -> bool {
    (monid.key_present && use_in_matches(&monid.settings.use_in, channel)) || channel.monid.disabled
}

/// Whether the channel can call Monid now: it is available and the user did
/// not turn it off here.
pub fn active_in(monid: &MonidView, channel: &ChannelView) -> bool {
    monid.key_present && use_in_matches(&monid.settings.use_in, channel) && !channel.monid.disabled
}

/// The Monid row of the Connectors tab of a channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelMonidRow {
    /// The line under the name.
    pub detail: String,
    /// The switch. Off sets `access.disabled`.
    pub enabled: bool,
}

/// The row, or `None` when the channel does not show Monid.
pub fn channel_row(state: &ViewState, channel_id: &str) -> Option<ChannelMonidRow> {
    let channel = state.channel(channel_id)?;
    if !available_in(&state.monid, channel) {
        return None;
    }
    let enabled = !channel.monid.disabled;
    let detail = if enabled {
        format!(
            "Paid tools. {}. {} this month.",
            rule_phrase(effective_rule(&state.monid.settings, &channel.monid)),
            money_text(state.monid.spent_in(channel_id))
        )
    } else {
        "Paid tools. Off in this channel.".to_owned()
    };
    Some(ChannelMonidRow { detail, enabled })
}

/// The line under the title of the Monid view in the panel: "$2.41 in this
/// channel this month. Balance $49.34."
pub fn channel_subtitle(monid: &MonidView, channel_id: &str) -> String {
    let spent = format!(
        "{} in this channel this month.",
        money_text(monid.spent_in(channel_id))
    );
    match monid.balance_text() {
        Some(balance) => format!("{spent} Balance {balance}."),
        None => spent,
    }
}

/// The words of the "Each call" select. `None` is the hub's rule.
pub fn each_call_choices(settings: &MonidSettingsView) -> Vec<(Option<ApprovalRuleView>, String)> {
    let mut out: Vec<(Option<ApprovalRuleView>, String)> = ApprovalRuleView::ALL
        .iter()
        .rev()
        .map(|r| (Some(*r), r.label().to_owned()))
        .collect();
    out.push((
        None,
        format!(
            "Use the hub's rule ({})",
            settings.approval.label().to_lowercase()
        ),
    ));
    out
}

/// The words of the "Each call" select for the current choice.
pub fn each_call_label(settings: &MonidSettingsView, current: Option<ApprovalRuleView>) -> String {
    match current {
        Some(rule) => rule.label().to_owned(),
        None => format!(
            "Use the hub's rule ({})",
            settings.approval.label().to_lowercase()
        ),
    }
}

/// "Use in" in a few words, for the Settings page.
pub fn use_in_text(settings: &MonidSettingsView, channels: &[ChannelView]) -> String {
    use_in_short(&settings.use_in, channels)
}

// The Full access note ----------------------------------------------------------------

/// The note of the Full access dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullAccessNote {
    pub title: &'static str,
    pub body: String,
}

/// The note that says Full access includes spending. `None` when the channel
/// does not have Monid.
pub fn full_access_note(monid: &MonidView, channel: &ChannelView) -> Option<FullAccessNote> {
    if !active_in(monid, channel) {
        return None;
    }
    let limits = &monid.settings.limits;
    let last = if limits.any() {
        format!("Your limits are {}.", limits.sentence())
    } else {
        "No limits are set.".to_owned()
    };
    Some(FullAccessNote {
        title: "Full access includes spending",
        body: format!(
            "This channel can spend your Monid balance with no approvals. Only the limits that you set will stop it to ask. {last}"
        ),
    })
}

// The card ---------------------------------------------------------------------------------

/// One option of the more menu of a Monid card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonidCardOption {
    /// The option ID that the answer sends.
    pub id: String,
    pub label: String,
}

/// A paid tool call that waits for the user.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MonidCardView {
    pub provider: String,
    pub endpoint: String,
    pub name: String,
    pub rate: String,
    pub estimate_micros: Option<i64>,
    /// The exact input, one `key: value` line for each key.
    pub input: String,
    /// The sentence of a limit that the call goes over.
    pub over_limit: Option<String>,
    /// The card has "Allow for this task".
    pub allow_task: bool,
    /// "Always allow this tool here" and "Never use this tool here", as the
    /// hub sent them.
    pub more: Vec<MonidCardOption>,
}

impl MonidCardView {
    pub fn title(&self) -> String {
        format!("Run a paid tool: {}", self.name)
    }

    /// The state at the right of the title.
    pub fn state_label(&self) -> &'static str {
        if self.over_limit.is_some() {
            "Over your limit"
        } else {
            "Needs you"
        }
    }

    /// The Price line: "$0.08 per 1,000 characters. About $0.05 for this
    /// call." Without an estimate, only the rate.
    pub fn price_line(&self) -> String {
        let rate = self.rate.trim().trim_end_matches('.');
        match self.estimate_micros {
            Some(m) => format!("{rate}. About {} for this call.", money_text(m)),
            None => format!("{rate}."),
        }
    }
}

/// The input of a call as lines of `key: value`. A string shows without
/// quotes. A value that is not text shows as compact JSON. A body that is not
/// an object shows as JSON.
pub fn input_lines(input: &Value) -> String {
    match input {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| match v {
                Value::String(s) => format!("{k}: {s}"),
                other => format!("{k}: {other}"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The view of a Monid card. `options` are the approval's options, so the
/// menu has the ones that the hub offers.
pub fn card_view(
    card: &MonidCard,
    options: &[consort_proto::model::RequestOption],
) -> MonidCardView {
    use consort_proto::model::{MONID_CARD_ALLOW_TASK, MONID_CARD_ALWAYS, MONID_CARD_NEVER};
    let has = |id: &str| options.iter().any(|o| o.id == id);
    let more = options
        .iter()
        .filter(|o| o.id == MONID_CARD_ALWAYS || o.id == MONID_CARD_NEVER)
        .map(|o| MonidCardOption {
            id: o.id.clone(),
            label: o.label.clone(),
        })
        .collect();
    MonidCardView {
        provider: card.provider.clone(),
        endpoint: card.endpoint.clone(),
        name: card.name.clone(),
        rate: card.rate.clone(),
        estimate_micros: card.estimate_micros,
        input: input_lines(&card.input),
        over_limit: card.over_limit.as_ref().map(|n| n.text.clone()),
        allow_task: if options.is_empty() {
            card.over_limit.is_none()
        } else {
            has(MONID_CARD_ALLOW_TASK)
        },
        more,
    }
}

// The call row -------------------------------------------------------------------------------

/// Where a call is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MonidCallStateView {
    #[default]
    Running,
    Completed,
    ProviderError,
    Failed,
    Denied,
    Blocked,
    StoppedByMonid,
    Stopped,
    TimedOut,
}

impl MonidCallStateView {
    pub fn from_proto(state: MonidCallState) -> MonidCallStateView {
        match state {
            MonidCallState::Running => MonidCallStateView::Running,
            MonidCallState::Completed => MonidCallStateView::Completed,
            MonidCallState::ProviderError => MonidCallStateView::ProviderError,
            MonidCallState::Failed => MonidCallStateView::Failed,
            MonidCallState::Denied => MonidCallStateView::Denied,
            MonidCallState::Blocked => MonidCallStateView::Blocked,
            MonidCallState::StoppedByMonid => MonidCallStateView::StoppedByMonid,
            MonidCallState::Stopped => MonidCallStateView::Stopped,
            MonidCallState::TimedOut => MonidCallStateView::TimedOut,
        }
    }

    /// The call has ended.
    pub fn is_final(self) -> bool {
        self != MonidCallStateView::Running
    }

    /// The call went wrong in a way that the row marks in the error color.
    pub fn is_error(self) -> bool {
        matches!(
            self,
            MonidCallStateView::ProviderError
                | MonidCallStateView::Failed
                | MonidCallStateView::StoppedByMonid
                | MonidCallStateView::TimedOut
        )
    }
}

/// The text at the right of a closed row, from the state and the cost.
pub fn call_summary(state: MonidCallStateView, cost_micros: i64) -> String {
    match state {
        MonidCallStateView::Running => "Running".to_owned(),
        MonidCallStateView::Completed => money_text(cost_micros),
        MonidCallStateView::ProviderError if cost_micros == 0 => {
            "Provider error, not charged".to_owned()
        }
        MonidCallStateView::ProviderError => money_text(cost_micros),
        MonidCallStateView::Failed => "Failed".to_owned(),
        MonidCallStateView::Denied => "Denied, not charged".to_owned(),
        MonidCallStateView::Blocked => "Blocked here, not charged".to_owned(),
        MonidCallStateView::StoppedByMonid => "Stopped by Monid, not charged".to_owned(),
        MonidCallStateView::Stopped => "Stopped".to_owned(),
        MonidCallStateView::TimedOut => "Timed out".to_owned(),
    }
}

/// The text of a call that has no text of its own for its state, when the
/// user opens the row.
pub fn call_note(state: MonidCallStateView, error: Option<&str>) -> Option<String> {
    let error = error.map(str::trim).filter(|e| !e.is_empty());
    match state {
        MonidCallStateView::Blocked => Some(error.map(str::to_owned).unwrap_or_else(|| {
            "This channel does not allow this tool. To allow it, open the Channel panel, then Connectors, then Monid."
                .to_owned()
        })),
        MonidCallStateView::StoppedByMonid => Some(error.map(str::to_owned).unwrap_or_else(|| {
            "The Monid balance or a budget on Monid's site stopped this run. Add money or change the budget on Monid's site, then ask again."
                .to_owned()
        })),
        MonidCallStateView::Failed
        | MonidCallStateView::ProviderError
        | MonidCallStateView::TimedOut
        | MonidCallStateView::Stopped
        | MonidCallStateView::Denied => error.map(str::to_owned),
        _ => None,
    }
}

/// "2.1 s", "340 ms", "1 min 5 s".
pub fn duration_label(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms} ms");
    }
    if ms < 60_000 {
        return format!("{:.1} s", ms as f64 / 1_000.0);
    }
    let secs = ms / 1_000;
    format!("{} min {} s", secs / 60, secs % 60)
}

/// The Cost line: "$0.0496, 620 characters at $0.08 per 1,000". When the
/// rate has no count that fits the cost, "$0.0496 at $0.08 per 1,000
/// characters". Without a rate, only the cost.
pub fn cost_line(cost_micros: i64, rate: &str) -> String {
    let cost = money_text(cost_micros);
    let rate = rate.trim().trim_end_matches('.');
    if rate.is_empty() {
        return cost;
    }
    if let Some((quantity, short_rate)) = quantity_and_rate(cost_micros, rate) {
        return format!("{cost}, {quantity} at {short_rate}");
    }
    format!("{cost} at {rate}")
}

/// For a rate like "$0.08 per 1,000 characters": the count of units that the
/// cost buys ("620 characters") and the rate without the unit ("$0.08 per
/// 1,000"). `None` when the rate has another shape or the count is not whole.
fn quantity_and_rate(cost_micros: i64, rate: &str) -> Option<(String, String)> {
    let (price_text, per) = rate.split_once(" per ")?;
    let price_micros = parse_limit(price_text).ok().flatten()?;
    let mut words = per.splitn(2, ' ');
    let first = words.next()?;
    let (per_count, unit) = match first.replace(',', "").parse::<i64>() {
        Ok(n) if n > 0 => (n, words.next()?.trim()),
        _ => (1, per.trim()),
    };
    if unit.is_empty() || cost_micros <= 0 {
        return None;
    }
    // quantity = cost / price * per_count, in whole units.
    let numerator = cost_micros.checked_mul(per_count)?;
    if numerator % price_micros != 0 {
        return None;
    }
    let quantity = numerator / price_micros;
    let short_rate = if per_count > 1 {
        format!(
            "{price_text} per {}",
            crate::connectors::thousands(per_count as u64)
        )
    } else {
        format!("{price_text} per {unit}")
    };
    Some((
        format!("{} {unit}", crate::connectors::thousands(quantity as u64)),
        short_rate,
    ))
}

/// What `monid.call` loaded for one call, for the open row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MonidCallDetailView {
    pub load: LoadState,
    /// The full input as JSON.
    pub input: String,
    /// The output as JSON or text, up to what the hub sent.
    pub output: String,
    /// The output is cut. The whole output is the file `output_file`.
    pub output_cut: bool,
    pub output_file: Option<AttachmentView>,
    /// "$0.0496, 620 characters at $0.08 per 1,000".
    pub cost_line: Option<String>,
    /// "01KQ7M4T9XG2ZB8H3VN6RCY5PD, 2.1 s".
    pub run_line: Option<String>,
    /// The words that the state needs, such as what to do after a block.
    pub note: Option<String>,
}

/// The pretty text of a JSON value for a box: a string shows as it is.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

impl MonidCallDetailView {
    pub fn from_call(call: &MonidCall) -> MonidCallDetailView {
        let state = MonidCallStateView::from_proto(call.state);
        let run_line = match (call.run_id.as_deref(), call.duration_ms) {
            (Some(id), 0) => Some(id.to_owned()),
            (Some(id), ms) => Some(format!("{id}, {}", duration_label(ms))),
            (None, 0) => None,
            (None, ms) => Some(duration_label(ms)),
        };
        let charged = state == MonidCallStateView::Completed
            || (state == MonidCallStateView::ProviderError && call.cost_micros > 0)
            || call.cost_micros > 0;
        MonidCallDetailView {
            load: LoadState::Loaded,
            input: value_text(&call.input),
            output: value_text(&call.output),
            output_cut: call.output_blob.is_some(),
            output_file: call
                .output_blob
                .as_ref()
                .map(|b| crate::files::attachment_view(b, None, None)),
            cost_line: charged.then(|| cost_line(call.cost_micros, &call.rate)),
            run_line,
            note: call_note(state, call.error.as_deref()),
        }
    }
}

/// A Monid call row in a timeline or a task thread.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MonidCallRowView {
    pub call_id: String,
    pub name: String,
    pub state: MonidCallStateView,
    pub cost_micros: i64,
    /// Monid can stop the run now: the row has Stop.
    pub stoppable: bool,
    /// The text at the right of the closed row.
    pub summary: String,
    /// The files that the hub saved in the channel files.
    pub attachments: Vec<AttachmentView>,
    /// What `monid.call` loaded. `None` until the user opens the row.
    pub detail: Option<MonidCallDetailView>,
}

impl MonidCallRowView {
    /// The row can open: a call that runs has nothing to show yet.
    pub fn can_open(&self) -> bool {
        self.state.is_final()
    }

    /// The row of a notice body.
    pub fn of_notice(
        notice: &MonidCallNotice,
        attachments: Vec<AttachmentView>,
        detail: Option<MonidCallDetailView>,
    ) -> MonidCallRowView {
        let state = MonidCallStateView::from_proto(notice.state);
        MonidCallRowView {
            call_id: notice.call_id.to_string(),
            name: notice.name.clone(),
            state,
            cost_micros: notice.cost_micros,
            stoppable: notice.stoppable && !state.is_final(),
            summary: call_summary(state, notice.cost_micros),
            attachments,
            detail,
        }
    }
}

/// The row of a call, wherever the app shows it: a channel timeline or a task
/// thread.
pub fn find_call_row<'a>(state: &'a ViewState, call_id: &str) -> Option<&'a MonidCallRowView> {
    use crate::view::{PostBody, TimelineItem};
    let in_timelines = state
        .timelines
        .iter()
        .flat_map(|t| t.items.iter())
        .find_map(|i| match i {
            TimelineItem::Notice(n) => n.monid.as_deref().filter(|r| r.call_id == call_id),
            _ => None,
        });
    in_timelines.or_else(|| {
        state
            .threads
            .iter()
            .flat_map(|t| t.posts.iter())
            .flat_map(|p| p.bodies.iter())
            .find_map(|b| match b {
                PostBody::Notice {
                    monid: Some(row), ..
                } if row.call_id == call_id => Some(row.as_ref()),
                _ => None,
            })
    })
}

/// Reads a notice body. `None` when the kind is not a Monid call or the body
/// is not the JSON of one.
pub fn parse_call_notice(body: &str) -> Option<MonidCallNotice> {
    let notice: MonidCallNotice = serde_json::from_str(body).ok()?;
    (notice.kind == consort_proto::model::MONID_CALL_NOTICE).then_some(notice)
}

/// The row of a call in the list of recent calls.
pub fn recent_call(call: &MonidCall) -> MonidRecentCallView {
    let state = MonidCallStateView::from_proto(call.state);
    MonidRecentCallView {
        call_id: call.id.to_string(),
        name: call.name.clone(),
        state,
        cost_micros: call.cost_micros,
        time_label: format::time_label(call.at.as_millis()),
        summary: call_summary(state, call.cost_micros),
    }
}

// The task card -----------------------------------------------------------------------------------

/// The line on a task card: "Monid: 83 calls so far". `None` when the task
/// made no call.
pub fn task_line(calls: u32) -> Option<String> {
    (calls > 0).then(|| format!("Monid: {} so far", calls_label(calls)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::{
        ids::{ChannelId, MonidCallId, SessionId},
        model::MonidLimitNote,
    };

    #[test]
    fn money_has_the_format_of_the_hub() {
        assert_eq!(money_text(3_500), "$0.0035");
        assert_eq!(money_text(1_240_000), "$1.24");
        assert_eq!(money_text(150), "$0.00015");
        assert_eq!(money_text(0), "$0");
        assert_eq!(money_text(5_000_000), "$5.00");
        assert_eq!(money_text(49_340_000), "$49.34");
        assert_eq!(money_text(49_960), "$0.04996");
        assert_eq!(money_text(-3_500), "-$0.0035");
    }

    #[test]
    fn a_limit_field_reads_dollars() {
        assert_eq!(parse_limit(""), Ok(None));
        assert_eq!(parse_limit("  "), Ok(None));
        assert_eq!(parse_limit("5"), Ok(Some(5_000_000)));
        assert_eq!(parse_limit("$0.50"), Ok(Some(500_000)));
        assert_eq!(parse_limit("1,250.5"), Ok(Some(1_250_500_000)));
        assert_eq!(parse_limit(".015"), Ok(Some(15_000)));
        assert!(parse_limit("five").is_err());
        assert!(parse_limit("0").is_err());
        assert!(parse_limit("1.0000001").is_err());
        assert!(parse_limit("-3").is_err());
        assert_eq!(limit_field_text(Some(500_000)), "0.50");
        assert_eq!(limit_field_text(None), "");
    }

    #[test]
    fn the_cost_line_names_the_count_when_the_rate_allows() {
        assert_eq!(
            cost_line(49_600, "$0.08 per 1,000 characters"),
            "$0.0496, 620 characters at $0.08 per 1,000"
        );
        assert_eq!(
            cost_line(3_500, "$0.0035 per call"),
            "$0.0035, 1 call at $0.0035 per call"
        );
        // A count that is not whole shows only the rate.
        assert_eq!(
            cost_line(50_000, "$0.0016 per result"),
            "$0.05 at $0.0016 per result"
        );
        assert_eq!(cost_line(3_500, "From $0.003"), "$0.0035 at From $0.003");
        assert_eq!(cost_line(3_500, ""), "$0.0035");
    }

    #[test]
    fn every_state_has_its_words() {
        use MonidCallStateView::*;
        assert_eq!(call_summary(Running, 0), "Running");
        assert_eq!(call_summary(Completed, 49_600), "$0.0496");
        assert_eq!(
            call_summary(ProviderError, 0),
            "Provider error, not charged"
        );
        assert_eq!(call_summary(ProviderError, 3_500), "$0.0035");
        assert_eq!(call_summary(Failed, 0), "Failed");
        assert_eq!(call_summary(Denied, 0), "Denied, not charged");
        assert_eq!(call_summary(Blocked, 0), "Blocked here, not charged");
        assert_eq!(
            call_summary(StoppedByMonid, 0),
            "Stopped by Monid, not charged"
        );
        assert_eq!(call_summary(Stopped, 0), "Stopped");
        assert_eq!(call_summary(TimedOut, 0), "Timed out");
        assert!(
            call_note(Blocked, None)
                .unwrap()
                .starts_with("This channel does not allow this tool.")
        );
        assert!(
            call_note(StoppedByMonid, None)
                .unwrap()
                .contains("Add money or change the budget")
        );
        assert_eq!(
            call_note(Failed, Some("The tool timed out.")).as_deref(),
            Some("The tool timed out.")
        );
        assert_eq!(call_note(Completed, Some("x")), None);
    }

    #[test]
    fn the_card_says_what_it_costs() {
        let mut card = MonidCardView {
            name: "ElevenLabs text to speech".into(),
            rate: "$0.08 per 1,000 characters".into(),
            estimate_micros: Some(50_000),
            ..MonidCardView::default()
        };
        assert_eq!(card.title(), "Run a paid tool: ElevenLabs text to speech");
        assert_eq!(card.state_label(), "Needs you");
        assert_eq!(
            card.price_line(),
            "$0.08 per 1,000 characters. About $0.05 for this call."
        );
        card.estimate_micros = None;
        assert_eq!(card.price_line(), "$0.08 per 1,000 characters.");
        card.over_limit = Some("This task has spent $4.99 of its $5.00 limit.".into());
        assert_eq!(card.state_label(), "Over your limit");
    }

    #[test]
    fn the_card_input_shows_each_key_on_a_line() {
        let input =
            serde_json::json!({"usernames": ["consortmusic"], "resultsLimit": 50, "q": "piano"});
        // The order of the keys is the order of the call, or sorted. Both
        // show one key on each line.
        let text = input_lines(&input);
        let mut lines: Vec<&str> = text.lines().collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            [
                "q: piano",
                "resultsLimit: 50",
                "usernames: [\"consortmusic\"]"
            ]
        );
    }

    fn option(id: &str, label: &str) -> consort_proto::model::RequestOption {
        consort_proto::model::RequestOption {
            id: id.into(),
            label: label.into(),
            kind: consort_proto::model::RequestOptionKind::AllowOnce,
            description: None,
        }
    }

    #[test]
    fn the_card_takes_its_menu_from_the_options_of_the_hub() {
        let wire = MonidCard {
            provider: "serpapi".into(),
            endpoint: "/instagram".into(),
            name: "SerpApi Instagram profile".into(),
            rate: "$0.015 per call".into(),
            estimate_micros: Some(15_000),
            input: serde_json::json!({"username": "consortmusic"}),
            over_limit: Some(MonidLimitNote {
                text: "This task has spent $4.99 of its $5.00 limit.".into(),
            }),
            ask_rule: None,
        };
        let over = consort_proto::model::ApprovalDetails::Monid(Box::new(wire.clone()))
            .hub_card_options()
            .unwrap();
        let view = card_view(&wire, &over);
        assert!(!view.allow_task);
        assert_eq!(
            view.more.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
            vec!["never"]
        );
        let mut open = wire;
        open.over_limit = None;
        let options = consort_proto::model::ApprovalDetails::Monid(Box::new(open.clone()))
            .hub_card_options()
            .unwrap();
        let view = card_view(&open, &options);
        assert!(view.allow_task);
        assert_eq!(
            view.more
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Always allow this tool here", "Never use this tool here"]
        );
        let _ = option("allow", "Allow once");
    }

    #[test]
    fn an_access_list_keeps_its_rules() {
        let apify = MonidToolView {
            provider: "apify".into(),
            endpoint: None,
            name: "Apify".into(),
        };
        let maps = MonidToolView {
            provider: "apify".into(),
            endpoint: Some("/google-maps".into()),
            name: "Apify Google Maps scraper".into(),
        };
        assert_eq!(apify.title(), "All Apify tools");
        assert_eq!(maps.title(), "Apify Google Maps scraper");
        let access = MonidAccessView::default()
            .with_only_allowed(true)
            .with_allowed(apify.clone())
            .with_allowed(apify.clone());
        assert_eq!(access.allowed.len(), 1);
        let blocked = access.with_blocked(maps.clone());
        assert_eq!(blocked.blocked, vec![maps.clone()]);
        // Allowing a blocked tool takes the block away.
        let again = blocked.with_allowed(maps.clone());
        assert!(again.blocked.is_empty());
        assert_eq!(again.allowed.len(), 2);
        // Blocking an allowed tool takes it off the list.
        let moved = again.with_blocked(maps);
        assert_eq!(moved.allowed.len(), 1);
        assert_eq!(moved.without_allowed(0).allowed.len(), 0);
        assert_eq!(moved.without_blocked(0).blocked.len(), 0);
        // The wire form goes back to the same view.
        assert_eq!(MonidAccessView::from_proto(&moved.to_proto()), moved);
    }

    #[test]
    fn a_hub_with_a_key_gives_the_row_to_the_channels_that_use_in_names() {
        let mut state = ViewState::default();
        let mut channel = ChannelView {
            id: "chn_1".into(),
            name: "house-lights".into(),
            ..ChannelView::default()
        };
        state.channels.push(channel.clone());
        // No key, no row.
        assert!(channel_row(&state, "chn_1").is_none());
        state.monid.key_present = true;
        state.monid.spend = vec![MonidSpendView {
            channel_id: "chn_1".into(),
            channel_name: "house-lights".into(),
            calls: 2,
            cost_micros: 2_410_000,
        }];
        let row = channel_row(&state, "chn_1").unwrap();
        assert!(row.enabled);
        assert_eq!(
            row.detail,
            "Paid tools. Asks before each call. $2.41 this month."
        );
        state.monid.settings.approval = ApprovalRuleView::AskOncePerTask;
        assert_eq!(
            channel_row(&state, "chn_1").unwrap().detail,
            "Paid tools. Asks once per task. $2.41 this month."
        );
        // The channel rule wins over the hub rule.
        channel.monid.approval = Some(ApprovalRuleView::RunWithoutAsking);
        state.channels[0] = channel.clone();
        assert!(
            channel_row(&state, "chn_1")
                .unwrap()
                .detail
                .starts_with("Paid tools. Runs without asking.")
        );
        // An exception keeps the row, with the switch off.
        channel.monid.disabled = true;
        state.channels[0] = channel;
        let row = channel_row(&state, "chn_1").unwrap();
        assert!(!row.enabled);
        assert_eq!(row.detail, "Paid tools. Off in this channel.");
        // A hub that lost its key keeps the row only for the exception.
        state.monid.key_present = false;
        assert!(channel_row(&state, "chn_1").is_some());
        state.channels[0].monid.disabled = false;
        assert!(channel_row(&state, "chn_1").is_none());
    }

    #[test]
    fn the_panel_title_has_the_spend_and_the_balance() {
        let mut monid = MonidView {
            balance_micros: Some(49_340_000),
            ..MonidView::default()
        };
        monid.spend = vec![MonidSpendView {
            channel_id: "chn_1".into(),
            channel_name: "a".into(),
            calls: 3,
            cost_micros: 2_410_000,
        }];
        assert_eq!(
            channel_subtitle(&monid, "chn_1"),
            "$2.41 in this channel this month. Balance $49.34."
        );
        assert_eq!(
            channel_subtitle(&monid, "chn_2"),
            "$0 in this channel this month. Balance $49.34."
        );
        monid.spent_month_micros = 3_870_000;
        assert_eq!(
            monid.footer_detail(),
            "$49.34 left. $3.87 spent this month."
        );
        assert_eq!(
            monid.page_subtitle(),
            "Balance $49.34. Paid tools from many providers, in one wallet."
        );
        monid.key_hint = Some("3f9a".into());
        assert_eq!(monid.key_text(), "Ends in 3f9a");
    }

    #[test]
    fn full_access_says_what_it_spends() {
        let mut monid = MonidView {
            key_present: true,
            ..MonidView::default()
        };
        let channel = ChannelView {
            id: "chn_1".into(),
            ..ChannelView::default()
        };
        let note = full_access_note(&monid, &channel).unwrap();
        assert_eq!(note.title, "Full access includes spending");
        assert_eq!(
            note.body,
            "This channel can spend your Monid balance with no approvals. Only the limits that you set will stop it to ask. No limits are set."
        );
        monid.settings.limits.per_task_micros = Some(5_000_000);
        monid.settings.limits.per_hub_month_micros = Some(100_000_000);
        let note = full_access_note(&monid, &channel).unwrap();
        assert!(
            note.body.ends_with(
                "Your limits are $5.00 for each task and $100.00 for the hub each month."
            )
        );
        // A channel without Monid has no note.
        let off = ChannelView {
            monid: MonidAccessView {
                disabled: true,
                ..MonidAccessView::default()
            },
            ..channel.clone()
        };
        assert!(full_access_note(&monid, &off).is_none());
        monid.key_present = false;
        assert!(full_access_note(&monid, &channel).is_none());
    }

    #[test]
    fn a_task_line_counts_calls() {
        assert_eq!(task_line(0), None);
        assert_eq!(task_line(1).as_deref(), Some("Monid: 1 call so far"));
        assert_eq!(task_line(83).as_deref(), Some("Monid: 83 calls so far"));
    }

    #[test]
    fn each_call_choices_include_the_hub_rule() {
        let settings = MonidSettingsView::default();
        let choices = each_call_choices(&settings);
        assert_eq!(choices.len(), 4);
        assert_eq!(choices[0].1, "Ask every time");
        assert_eq!(choices[3].0, None);
        assert_eq!(choices[3].1, "Use the hub's rule (ask every time)");
        assert_eq!(each_call_label(&settings, None), choices[3].1);
    }

    #[test]
    fn a_call_notice_reads_and_a_foreign_body_does_not() {
        let id = MonidCallId::new();
        let body = serde_json::json!({
            "kind": "monid_call", "call_id": id, "name": "MiniMax image generation",
            "state": "completed", "cost_micros": 3500,
        })
        .to_string();
        let notice = parse_call_notice(&body).unwrap();
        let row = MonidCallRowView::of_notice(&notice, Vec::new(), None);
        assert_eq!(row.summary, "$0.0035");
        assert!(row.can_open());
        assert!(!row.stoppable);
        assert!(parse_call_notice(r#"{"kind":"turn_failed","text":"x"}"#).is_none());
        assert!(parse_call_notice("not json").is_none());
        let running = serde_json::json!({
            "kind": "monid_call", "call_id": id, "name": "Apify", "state": "running", "stoppable": true,
        })
        .to_string();
        let row =
            MonidCallRowView::of_notice(&parse_call_notice(&running).unwrap(), Vec::new(), None);
        assert!(row.stoppable);
        assert!(!row.can_open());
        assert_eq!(row.summary, "Running");
        let _ = (ChannelId::new(), SessionId::new());
    }

    #[test]
    fn the_open_row_has_the_cost_and_the_run() {
        let call = MonidCall {
            id: MonidCallId::new(),
            channel_id: ChannelId::new(),
            session_id: SessionId::new(),
            task_id: None,
            message_id: None,
            approval_id: None,
            provider: "elevenlabs".into(),
            endpoint: "/text-to-speech".into(),
            name: "ElevenLabs text to speech".into(),
            state: MonidCallState::Completed,
            rate: "$0.08 per 1,000 characters".into(),
            cost_micros: 49_600,
            run_id: Some("01KQ7M4T9XG2ZB8H3VN6RCY5PD".into()),
            stoppable: false,
            input: serde_json::json!({"text": "The piano"}),
            output: serde_json::json!({"audio": {"format": "mp3"}}),
            output_blob: None,
            error: None,
            files: Vec::new(),
            duration_ms: 2_100,
            at: consort_proto::time::Timestamp::from_millis(0),
        };
        let detail = MonidCallDetailView::from_call(&call);
        assert_eq!(
            detail.cost_line.as_deref(),
            Some("$0.0496, 620 characters at $0.08 per 1,000")
        );
        assert_eq!(
            detail.run_line.as_deref(),
            Some("01KQ7M4T9XG2ZB8H3VN6RCY5PD, 2.1 s")
        );
        assert!(detail.input.contains("\"text\": \"The piano\""));
        assert!(!detail.output_cut);
        let mut denied = call;
        denied.state = MonidCallState::Denied;
        denied.cost_micros = 0;
        denied.run_id = None;
        denied.duration_ms = 0;
        let detail = MonidCallDetailView::from_call(&denied);
        assert_eq!(detail.cost_line, None);
        assert_eq!(detail.run_line, None);
    }
}
