//! Client requests for proactivity: the settings (`proactivity.get`,
//! `proactivity.set`, `proactivity.snooze`), the log (`proactivity.log`),
//! Cancel all follow-ups (`proactivity.cancel_follow_ups`), and the
//! Useful and Too noisy ratings of a proactive message (`message.rate`).

use serde::{Deserialize, Serialize};

use crate::ids::{ChannelId, MessageId};
use crate::model::{
    ProactiveKind, ProactiveRating, ProactiveWake, ProactivitySettings, QuietHours, SnoozeChoice,
};

/// The most log entries in one `proactivity.log` page.
pub const PROACTIVITY_LOG_PAGE_MAX: u32 = 500;

/// `proactivity.get`: the settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactivityGet {}

/// `proactivity.set`. Fields that you leave out do not change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactivitySet {
    /// Pause all, or end it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    /// The quiet hours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_hours: Option<QuietHours>,
    /// The user's IANA time zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// Set the time zone only when the hub has none. A client sends this
    /// at start with the system time zone, so a choice that the user made
    /// stays.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub time_zone_if_unset: bool,
    /// The most notifying proactive messages in one day, 0 to 20.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_cap_per_day: Option<u32>,
}

/// `proactivity.snooze`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactivitySnooze {
    /// The choice. `off` ends a snooze.
    pub choice: SnoozeChoice,
}

/// The response to `proactivity.get`, `proactivity.set` and
/// `proactivity.snooze`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactivityResult {
    /// The settings after the change.
    pub settings: ProactivitySettings,
}

/// `proactivity.log`: a page of the proactivity log, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactivityLog {
    /// Only this channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<ChannelId>,
    /// Only this kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ProactiveKind>,
    /// Only entries older than this entry number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<u64>,
    /// The most entries. The default is 100 and the limit is 500.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// The response to `proactivity.log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactivityLogResult {
    /// The entries, newest first.
    pub wakes: Vec<ProactiveWake>,
    /// True when older entries exist.
    pub has_more: bool,
}

/// `proactivity.cancel_follow_ups`: delete the follow-ups that the
/// assistant set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactivityCancelFollowUps {
    /// Only in this channel. Leave it out for every channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<ChannelId>,
}

/// The response to `proactivity.cancel_follow_ups`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactivityCancelFollowUpsResult {
    /// How many follow-ups the hub deleted.
    pub canceled: u32,
}

/// `message.rate`: rate a proactive message. Too noisy wakes the
/// assistant, which adds a skip line to the check-in note and confirms in
/// one reply. Useful is only recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageRate {
    /// The proactive message.
    pub message_id: MessageId,
    /// The rating. `null` takes it back.
    pub rating: Option<ProactiveRating>,
}
