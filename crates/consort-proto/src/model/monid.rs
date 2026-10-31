//! Monid: paid tools behind the hub (ADR 0016).
//!
//! Money is in whole millionths of a dollar, so that a long sum does not
//! drift. Prices can be as small as $0.00015.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ApprovalRule, BlobRef, UseIn};
use crate::ids::{ApprovalId, ChannelId, MessageId, MonidCallId, SessionId, TaskId};
use crate::time::Timestamp;

/// The notice kind of a Monid call row in a timeline or a task thread.
pub const MONID_CALL_NOTICE: &str = "monid_call";
/// The option of a Monid card that allows the tool for the task.
pub const MONID_CARD_ALLOW_TASK: &str = "allow_task";
/// The option of a Monid card that allows the tool in the channel from now
/// on.
pub const MONID_CARD_ALWAYS: &str = "always";
/// The option of a Monid card that blocks the tool in the channel.
pub const MONID_CARD_NEVER: &str = "never";

/// What a routine or a proactive turn may do with Monid. No user is there to
/// watch these turns, so they have their own setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonidUnattended {
    /// The turn cannot call Monid.
    Off,
    /// The turn asks, and a run waits in Needs input.
    Ask,
    /// The turn runs without a card.
    RunWithoutAsking,
}

/// The limits that the user set. `None` is no limit, the default. Over a
/// limit the hub asks, in every mode. A limit does not end the work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MonidLimits {
    /// The most for one call.
    #[serde(default)]
    pub per_call_micros: Option<i64>,
    /// The most for all calls of one task.
    #[serde(default)]
    pub per_task_micros: Option<i64>,
    /// The most for one channel in a month.
    #[serde(default)]
    pub per_channel_month_micros: Option<i64>,
    /// The most for the hub in a month.
    #[serde(default)]
    pub per_hub_month_micros: Option<i64>,
}

fn default_approval() -> ApprovalRule {
    ApprovalRule::AskEveryTime
}

fn default_routines() -> MonidUnattended {
    MonidUnattended::Ask
}

fn default_proactive() -> MonidUnattended {
    MonidUnattended::Off
}

/// The Monid settings of the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidSettings {
    /// The channels that have Monid.
    #[serde(default)]
    pub use_in: UseIn,
    /// The rule for each call, unless a channel changes it.
    #[serde(default = "default_approval")]
    pub approval: ApprovalRule,
    /// The setting of routines, unless a routine changes it.
    #[serde(default = "default_routines")]
    pub routines: MonidUnattended,
    /// The setting of proactive turns.
    #[serde(default = "default_proactive")]
    pub proactive: MonidUnattended,
    /// The limits.
    #[serde(default)]
    pub limits: MonidLimits,
}

impl Default for MonidSettings {
    fn default() -> Self {
        Self {
            use_in: UseIn::default(),
            approval: default_approval(),
            routines: default_routines(),
            proactive: default_proactive(),
            limits: MonidLimits::default(),
        }
    }
}

/// A provider or one tool of a provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MonidToolRef {
    /// The provider, for example `apify`.
    pub provider: String,
    /// The endpoint. `None` means every tool of the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// The name to show, for example "ElevenLabs text to speech".
    #[serde(default)]
    pub name: String,
}

impl MonidToolRef {
    /// Whether this entry covers the tool.
    pub fn covers(&self, provider: &str, endpoint: &str) -> bool {
        self.provider.eq_ignore_ascii_case(provider)
            && self.endpoint.as_deref().is_none_or(|e| e == endpoint)
    }
}

/// One entry of a channel's list of allowed tools.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MonidAllowedTool {
    /// The provider or the tool.
    #[serde(flatten)]
    pub tool: MonidToolRef,
    /// The user chose "Always allow this tool here". Calls of it have no
    /// card, unless they are over a limit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub run_without_asking: bool,
}

/// What one channel may do with Monid. The default changes nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MonidChannelAccess {
    /// The channel has Monid off, as an exception to "Use in".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// The channel's own rule for each call. `None` uses the hub's rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalRule>,
    /// True is "Only these tools": a tool that is not in `allowed` is
    /// refused. False is "All tools".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub only_allowed: bool,
    /// The allowed tools. With "All tools", an entry can still say
    /// `run_without_asking`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<MonidAllowedTool>,
    /// The tools that this channel never uses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<MonidToolRef>,
}

impl MonidChannelAccess {
    /// Whether a block or the channel's list refuses the tool.
    pub fn refuses(&self, provider: &str, endpoint: &str) -> bool {
        self.blocked.iter().any(|b| b.covers(provider, endpoint))
            || (self.only_allowed
                && !self
                    .allowed
                    .iter()
                    .any(|a| a.tool.covers(provider, endpoint)))
    }

    /// Whether the user always allows the tool in this channel.
    pub fn runs_without_asking(&self, provider: &str, endpoint: &str) -> bool {
        self.allowed
            .iter()
            .any(|a| a.run_without_asking && a.tool.covers(provider, endpoint))
    }
}

/// The spend of one channel in the current month.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidChannelSpend {
    /// The channel.
    pub channel_id: ChannelId,
    /// How many calls ran.
    pub calls: u32,
    /// What they cost.
    pub cost_micros: i64,
}

/// The Monid state of the hub, without the key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidStatus {
    /// Whether the hub has a key.
    pub key_present: bool,
    /// The last 4 characters of the key.
    #[serde(default)]
    pub key_hint: Option<String>,
    /// The balance on Monid, when the hub could read it.
    #[serde(default)]
    pub balance_micros: Option<i64>,
    /// What the hub could not read, with what to do next.
    #[serde(default)]
    pub error: Option<String>,
    /// The settings.
    #[serde(default)]
    pub settings: MonidSettings,
    /// The total of the hub this month.
    #[serde(default)]
    pub spent_month_micros: i64,
    /// The spend per channel this month, the largest first.
    #[serde(default)]
    pub spend: Vec<MonidChannelSpend>,
}

/// Where a Monid call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonidCallState {
    /// The run has started.
    Running,
    /// The tool ran and gave a result.
    Completed,
    /// The provider answered with an error.
    ProviderError,
    /// The run failed, or the hub could not start it.
    Failed,
    /// The user denied the call.
    Denied,
    /// The channel does not allow the tool, or the setting is off.
    Blocked,
    /// A balance or a budget on Monid's site stopped the run.
    StoppedByMonid,
    /// The user stopped the run.
    Stopped,
    /// The run went over its time limit on Monid.
    TimedOut,
}

impl MonidCallState {
    /// Whether the call has ended.
    pub fn is_final(self) -> bool {
        self != Self::Running
    }
}

/// One Monid call, with its full input and output. The hub keeps it, because
/// the user asked to see them (ADR 0016).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonidCall {
    /// The ID.
    pub id: MonidCallId,
    /// The channel.
    pub channel_id: ChannelId,
    /// The session that called.
    pub session_id: SessionId,
    /// The task, for a call in a task.
    #[serde(default)]
    pub task_id: Option<TaskId>,
    /// The row of this call.
    #[serde(default)]
    pub message_id: Option<MessageId>,
    /// The card, when the call asked.
    #[serde(default)]
    pub approval_id: Option<ApprovalId>,
    /// The provider, for example `elevenlabs`.
    pub provider: String,
    /// The endpoint, for example `/text-to-speech`.
    pub endpoint: String,
    /// The name to show.
    pub name: String,
    /// The state.
    pub state: MonidCallState,
    /// The rate, for example "$0.08 per 1,000 characters".
    #[serde(default)]
    pub rate: String,
    /// What Monid charged.
    #[serde(default)]
    pub cost_micros: i64,
    /// Monid's run, when one started.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Whether Monid can stop the run now.
    #[serde(default)]
    pub stoppable: bool,
    /// The full input.
    #[serde(default)]
    pub input: Value,
    /// The output, up to the size that a client gets at once.
    #[serde(default)]
    pub output: Value,
    /// The whole output, when it is too large for `output`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_blob: Option<BlobRef>,
    /// What went wrong, with what to do next.
    #[serde(default)]
    pub error: Option<String>,
    /// The files that the hub saved in the channel files.
    #[serde(default)]
    pub files: Vec<BlobRef>,
    /// How long the run took.
    #[serde(default)]
    pub duration_ms: u64,
    /// When the call started.
    pub at: Timestamp,
}

/// The body of a Monid call row: all a client needs to draw the closed row.
/// The input and the output come with `monid.call`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidCallNotice {
    /// Always [`MONID_CALL_NOTICE`].
    pub kind: String,
    /// The call.
    pub call_id: MonidCallId,
    /// The name to show.
    pub name: String,
    /// The state.
    pub state: MonidCallState,
    /// What Monid charged.
    #[serde(default)]
    pub cost_micros: i64,
    /// Whether Monid can stop the run now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stoppable: bool,
}

impl MonidCallNotice {
    /// The row of a call.
    pub fn of(call: &MonidCall) -> Self {
        Self {
            kind: MONID_CALL_NOTICE.to_owned(),
            call_id: call.id,
            name: call.name.clone(),
            state: call.state,
            cost_micros: call.cost_micros,
            stoppable: call.stoppable,
        }
    }
}

/// Why a Monid card asks, beyond the channel's rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidLimitNote {
    /// The sentence for the card, for example "This task has spent $4.98 of
    /// its $5.00 limit."
    pub text: String,
}

/// The card of a Monid call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonidCard {
    /// The provider.
    pub provider: String,
    /// The endpoint.
    pub endpoint: String,
    /// The name to show.
    pub name: String,
    /// The rate.
    pub rate: String,
    /// The estimate for this input. `None` when the input does not fix the
    /// cost, and then the card shows only the rate.
    #[serde(default)]
    pub estimate_micros: Option<i64>,
    /// The exact input that runs when the user allows the call.
    #[serde(default)]
    pub input: Value,
    /// Set when the call is over a limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over_limit: Option<MonidLimitNote>,
    /// The rule that made the card ask.
    #[serde(default)]
    pub ask_rule: Option<ApprovalRule>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(provider: &str, endpoint: Option<&str>) -> MonidToolRef {
        MonidToolRef {
            provider: provider.to_owned(),
            endpoint: endpoint.map(str::to_owned),
            name: String::new(),
        }
    }

    #[test]
    fn a_new_hub_asks_with_no_limits() {
        let settings: MonidSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, MonidSettings::default());
        assert_eq!(settings.approval, ApprovalRule::AskEveryTime);
        assert_eq!(settings.routines, MonidUnattended::Ask);
        assert_eq!(settings.proactive, MonidUnattended::Off);
        assert_eq!(settings.limits, MonidLimits::default());
        assert_eq!(settings.limits.per_call_micros, None);
    }

    #[test]
    fn a_new_channel_allows_all_tools() {
        let access = MonidChannelAccess::default();
        assert!(!access.refuses("apify", "/anything"));
        assert!(!access.runs_without_asking("apify", "/anything"));
    }

    #[test]
    fn only_these_tools_refuses_the_rest() {
        let access = MonidChannelAccess {
            only_allowed: true,
            allowed: vec![
                MonidAllowedTool {
                    tool: tool("apify", None),
                    run_without_asking: false,
                },
                MonidAllowedTool {
                    tool: tool("minimax", Some("/v1/image_generation")),
                    run_without_asking: true,
                },
            ],
            ..Default::default()
        };
        assert!(!access.refuses("apify", "/any/actor"));
        assert!(!access.refuses("minimax", "/v1/image_generation"));
        assert!(access.refuses("minimax", "/v1/other"));
        assert!(access.refuses("openai", "/v1/image/gpt-image-2"));
        assert!(access.runs_without_asking("minimax", "/v1/image_generation"));
        assert!(!access.runs_without_asking("apify", "/any/actor"));
    }

    #[test]
    fn a_block_wins_over_a_provider_on_the_list() {
        let access = MonidChannelAccess {
            only_allowed: true,
            allowed: vec![MonidAllowedTool {
                tool: tool("apify", None),
                run_without_asking: false,
            }],
            blocked: vec![tool("apify", Some("/google-maps"))],
            ..Default::default()
        };
        assert!(access.refuses("apify", "/google-maps"));
        assert!(!access.refuses("apify", "/instagram"));
    }
}
