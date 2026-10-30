//! The hub tools with which the assistant works on its own: follow-ups that
//! it sets, a quieter level and snooze, and a silent finding.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{ChannelId, RoutineId};
use crate::model::{ProactivityLevel, SnoozeChoice};
use crate::time::Timestamp;

/// `no_reply_needed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoReplyNeededArgs {
    /// In a turn that you started on your own: one short line about what
    /// you saw and chose not to report. It goes into the proactivity log
    /// and into the next check-in, so you do not report it twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding: Option<String>,
}

/// `follow_up_later`. Give `after_minutes` or `at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FollowUpLaterArgs {
    /// Wake after this many minutes. At least 15, at most 30 days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_minutes: Option<u64>,
    /// Wake at this time. At least 15 minutes and at most 30 days from now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
    /// Why, in one line, for example "Check whether Brightline replied to
    /// the quote." You get it back when you wake, and the user sees it.
    pub reason: String,
    /// A short key. A new follow-up with the same key in this channel
    /// replaces the old one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// `cancel_follow_up`. Give `routine_id` or `key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelFollowUpArgs {
    /// The follow-up routine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine_id: Option<RoutineId>,
    /// The key that you gave when you set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// `set_proactivity`. Give `level`, `snooze` or both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetProactivityArgs {
    /// A quieter level: `off`, `quiet` or `normal`. The hub refuses a
    /// louder level. Only the user can raise it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<ProactivityLevel>,
    /// In Tutti: the channel to change. Leave it out for this channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<ChannelId>,
    /// Snooze everything that starts on its own in every channel:
    /// `two_hours`, `until_tomorrow` or `week`. `off` ends a snooze.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze: Option<SnoozeChoice>,
}

/// The result of `follow_up_later`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUpResult {
    /// The follow-up routine.
    pub routine_id: RoutineId,
    /// When it wakes you.
    pub at: Timestamp,
    /// The follow-up that this one replaced, for the same key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced: Option<RoutineId>,
}

/// The result of `set_proactivity`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetProactivityResult {
    /// The channel's level now.
    pub level: ProactivityLevel,
    /// The end of the snooze, when one is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<Timestamp>,
}
