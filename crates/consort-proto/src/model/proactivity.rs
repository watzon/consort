//! Proactivity: what the assistant may start on its own, and the record of
//! what it did.
//!
//! The hub decides before a model runs: the level of the channel, quiet
//! hours, snooze, Pause all and the daily budget. See
//! `docs/PROACTIVITY-PLAN.md` and ADR 0013.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::ValidationError;
use super::conversation::ChannelKind;
use crate::ids::{ChannelId, MessageId, RoutineId, TaskId};
use crate::time::Timestamp;

/// The feed `entity_id` of the one proactivity settings record.
pub const PROACTIVITY_ENTITY_ID: &str = "proactivity";

/// The path of the check-in note in a memory scope.
pub const CHECK_IN_NOTE_PATH: &str = "check-in.md";
/// The most bytes of the check-in note that a check-in gets. The hub cuts a
/// longer note and says so.
pub const CHECK_IN_NOTE_MAX_BYTES: usize = 2 * 1024;
/// The front matter line that says that a check-in note watches an outside
/// source, such as mail or a calendar. A check-in with this line runs also
/// when nothing changed in the hub.
pub const CHECK_IN_WATCH_OUTSIDE: &str = "watch: outside";

/// The shortest interval of a check-in, in seconds.
pub const CHECK_IN_MIN_SECONDS: u64 = 30 * 60;
/// The interval of Tutti's check-in when the hub creates it.
pub const CHECK_IN_TUTTI_SECONDS: u64 = 3 * 60 * 60;
/// The interval of another channel's check-in when the hub creates it.
pub const CHECK_IN_CHANNEL_SECONDS: u64 = 6 * 60 * 60;
/// The longest interval that back-off gives a check-in.
pub const CHECK_IN_BACKOFF_MAX_SECONDS: u64 = 12 * 60 * 60;
/// After this many silent check-ins in a row, the hub doubles the interval.
pub const SILENT_CHECK_INS_BEFORE_BACKOFF: u32 = 6;

/// The shortest time from now to a follow-up, in minutes.
pub const FOLLOW_UP_MIN_MINUTES: u64 = 15;
/// The longest time from now to a follow-up, in days.
pub const FOLLOW_UP_MAX_DAYS: u64 = 30;
/// The most follow-ups that wait in one channel.
pub const FOLLOW_UPS_MAX_PER_CHANNEL: usize = 20;
/// The most follow-ups that run in one channel in a day. The hub skips the
/// next ones that day (`skipped_budget`).
pub const FOLLOW_UPS_MAX_RUNS_PER_DAY: u32 = 10;
/// The shortest time between two runs of the morning brief or the research
/// pass, in seconds: 12 hours.
pub const BRIEF_RESEARCH_MIN_GAP_SECONDS: u64 = 12 * 60 * 60;
/// The most characters in the reason of a follow-up.
pub const FOLLOW_UP_REASON_MAX_CHARS: usize = 300;

/// The most notifying proactive messages in one day across the hub, by
/// default.
pub const NOTIFY_CAP_DEFAULT: u32 = 3;
/// The daily budget of proactive turns in Tutti.
pub const TURN_BUDGET_TUTTI: u32 = 8;
/// The daily budget of proactive turns in another channel.
pub const TURN_BUDGET_CHANNEL: u32 = 3;
/// The most offers that the research pass makes in one day.
pub const RESEARCH_OFFERS_MAX_PER_DAY: u32 = 3;
/// The memory folder that the research pass may write.
pub const RESEARCH_MEMORY_FOLDER: &str = "research/";

/// A task in Needs input for this long gets one nudge.
pub const STALE_NEEDS_INPUT_MS: i64 = 24 * 60 * 60 * 1000;
/// A task in Ready for review for this long gets one nudge.
pub const STALE_REVIEW_MS: i64 = 72 * 60 * 60 * 1000;

/// The hub keeps the proactivity log for this many days.
pub const PROACTIVE_LOG_KEEP_DAYS: i64 = 30;
/// The most characters in a finding.
pub const FINDING_MAX_CHARS: usize = 300;

/// The most suggestions on one message.
pub const SUGGESTIONS_MAX: usize = 4;
/// The most characters in the label of one suggestion.
pub const SUGGESTION_LABEL_MAX_CHARS: usize = 40;

/// The labels of the three options of an offer.
pub const OFFER_DO_IT: &str = "Do it";
/// See [`OFFER_DO_IT`].
pub const OFFER_NOT_NOW: &str = "Not now";
/// See [`OFFER_DO_IT`].
pub const OFFER_NEVER: &str = "Never about this";

/// How much the assistant starts on its own in one channel.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ProactivityLevel {
    /// Nothing. The assistant only answers.
    Off,
    /// Follow-ups that it set, stale-task nudges and the start of a new
    /// channel.
    Quiet,
    /// Quiet, plus check-ins and (in Tutti) the morning brief.
    Normal,
    /// Normal, plus the read-only research pass and offers.
    Eager,
}

impl ProactivityLevel {
    /// Every level, from the quietest to the loudest.
    pub const ALL: [ProactivityLevel; 4] = [Self::Off, Self::Quiet, Self::Normal, Self::Eager];

    /// The level of a channel that has none set: Normal in Tutti, Quiet in
    /// other channels.
    pub fn default_for(kind: ChannelKind) -> Self {
        match kind {
            ChannelKind::Tutti => Self::Normal,
            ChannelKind::Standard => Self::Quiet,
        }
    }

    /// The wire text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Quiet => "quiet",
            Self::Normal => "normal",
            Self::Eager => "eager",
        }
    }

    /// Reads the wire text.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.as_str() == text)
    }

    /// The name in the UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Quiet => "Quiet",
            Self::Normal => "Normal",
            Self::Eager => "Eager",
        }
    }

    /// Whether this level lets a wake of `kind` run.
    pub fn allows(self, kind: ProactiveKind) -> bool {
        self >= kind.min_level()
    }
}

/// What woke the assistant on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProactiveKind {
    /// A check-in routine.
    CheckIn,
    /// The morning brief.
    Brief,
    /// A follow-up that the assistant set.
    FollowUp,
    /// A task that waited too long in Needs input or Ready for review.
    StaleTask,
    /// The read-only research pass.
    Research,
    /// A new channel: the guided start.
    ChannelCreated,
    /// The user rated a proactive message as too noisy.
    Feedback,
    /// An offer about the style guide of "Write in my voice". Only the hub
    /// posts it. The guide from "Learn" answers the user's own click, so no
    /// level stops it; the hub offers a rule learned from edits
    /// (`propose_voice_rule`) only when Tutti's level is not Off.
    Voice,
}

impl ProactiveKind {
    /// Every kind.
    pub const ALL: [ProactiveKind; 8] = [
        Self::CheckIn,
        Self::Brief,
        Self::FollowUp,
        Self::StaleTask,
        Self::Research,
        Self::ChannelCreated,
        Self::Feedback,
        Self::Voice,
    ];

    /// The wire text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CheckIn => "check_in",
            Self::Brief => "brief",
            Self::FollowUp => "follow_up",
            Self::StaleTask => "stale_task",
            Self::Research => "research",
            Self::ChannelCreated => "channel_created",
            Self::Feedback => "feedback",
            Self::Voice => "voice",
        }
    }

    /// Reads the wire text.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == text)
    }

    /// The name in the UI, for example "Check-in".
    pub fn label(self) -> &'static str {
        match self {
            Self::CheckIn => "Check-in",
            Self::Brief => "Morning brief",
            Self::FollowUp => "Follow-up",
            Self::StaleTask => "Stale task",
            Self::Research => "Research",
            Self::ChannelCreated => "New channel",
            Self::Feedback => "Feedback",
            Self::Voice => "Voice",
        }
    }

    /// The quietest level that lets this kind run. Feedback comes from the
    /// user, so every level lets it run.
    pub fn min_level(self) -> ProactivityLevel {
        match self {
            Self::FollowUp | Self::StaleTask | Self::ChannelCreated => ProactivityLevel::Quiet,
            Self::CheckIn | Self::Brief => ProactivityLevel::Normal,
            Self::Research => ProactivityLevel::Eager,
            Self::Feedback | Self::Voice => ProactivityLevel::Off,
        }
    }

    /// Whether the daily budget can stop a wake of this kind. Follow-ups and
    /// the brief count, but the budget never stops them. A new channel and
    /// feedback come from the user.
    pub fn budget_can_skip(self) -> bool {
        matches!(self, Self::CheckIn | Self::StaleTask | Self::Research)
    }
}

/// The user's rating of a proactive message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProactiveRating {
    /// The message helped. The hub only records it.
    Useful,
    /// The message was noise. The assistant adds a skip line to the
    /// check-in note and confirms in one reply.
    TooNoisy,
}

impl ProactiveRating {
    /// The wire text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Useful => "useful",
            Self::TooNoisy => "too_noisy",
        }
    }
}

/// The mark on a message that a proactive turn posted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactiveMark {
    /// What woke the assistant.
    pub kind: ProactiveKind,
    /// Whether the message asks for an OS notification. The hub clears it
    /// in quiet hours, while snoozed and over the daily limit.
    #[serde(default)]
    pub notify: bool,
    /// For a follow-up: why the assistant set it, as one line under the
    /// mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// True for an offer: a decision with Do it, Not now and Never about
    /// this.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub offer: bool,
    /// The user's rating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<ProactiveRating>,
}

impl ProactiveMark {
    /// A mark of this kind with nothing else set.
    pub fn new(kind: ProactiveKind) -> Self {
        Self {
            kind,
            notify: false,
            reason: None,
            offer: false,
            rating: None,
        }
    }
}

/// What a quick reply does when the user clicks it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionAction {
    /// Send the label as the user's message.
    #[default]
    Send,
    /// Move the focus to the composer and send nothing, for "I will type
    /// it".
    Type,
}

/// A quick reply under an assistant message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Suggestion {
    /// The text on the chip. At most 40 characters.
    pub label: String,
    /// `send` (the default) sends the label as the user's message. `type`
    /// only moves the focus to the composer.
    #[serde(default)]
    pub action: SuggestionAction,
}

impl Suggestion {
    /// A suggestion that sends its label.
    pub fn send(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: SuggestionAction::Send,
        }
    }

    /// A suggestion that only focuses the composer.
    pub fn type_it(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: SuggestionAction::Type,
        }
    }
}

/// Checks the suggestions of a message: at most four, each with a label of
/// one to 40 characters, and no label twice.
pub fn validate_suggestions(suggestions: &[Suggestion]) -> Result<(), ValidationError> {
    if suggestions.len() > SUGGESTIONS_MAX {
        return Err(ValidationError::new(
            "suggestions",
            format!("has more than {SUGGESTIONS_MAX} entries"),
        ));
    }
    for (i, s) in suggestions.iter().enumerate() {
        let field = format!("suggestions[{i}].label");
        if s.label.trim().is_empty() {
            return Err(ValidationError::new(field, "is empty"));
        }
        if s.label.chars().count() > SUGGESTION_LABEL_MAX_CHARS {
            return Err(ValidationError::new(
                field,
                format!("is longer than {SUGGESTION_LABEL_MAX_CHARS} characters"),
            ));
        }
        if suggestions[..i]
            .iter()
            .any(|o| o.label.trim() == s.label.trim())
        {
            return Err(ValidationError::new(field, "is a copy of an earlier label"));
        }
    }
    Ok(())
}

/// What a task that a proactive turn starts may do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProactiveTasks {
    /// Draft mode: the task runs in Ask, so every outward action waits for
    /// the user. The default.
    #[default]
    Drafts,
    /// The task gets the channel's default approval mode.
    ChannelDefault,
}

/// The hours when the assistant starts nothing and sends no notification.
/// Times are `HH:MM` in the user's time zone. A range can cross midnight.
/// A range whose start and end are the same is empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHours {
    /// The start, for example `21:00`.
    pub start: String,
    /// The end, for example `07:00`.
    pub end: String,
}

impl Default for QuietHours {
    fn default() -> Self {
        Self {
            start: "21:00".to_owned(),
            end: "07:00".to_owned(),
        }
    }
}

impl QuietHours {
    /// The start and the end as minutes after midnight. `None` when one of
    /// them is not `HH:MM`.
    pub fn minutes(&self) -> Option<(u32, u32)> {
        let read = |t: &str| crate::client_api::parse_time(t).map(|(h, m)| h * 60 + m);
        Some((read(&self.start)?, read(&self.end)?))
    }

    /// Whether a local time, as minutes after midnight, is inside the range.
    /// A range that is not valid contains nothing.
    pub fn contains_minute(&self, minute: u32) -> bool {
        let Some((start, end)) = self.minutes() else {
            return false;
        };
        match start.cmp(&end) {
            std::cmp::Ordering::Equal => false,
            std::cmp::Ordering::Less => (start..end).contains(&minute),
            std::cmp::Ordering::Greater => minute >= start || minute < end,
        }
    }

    /// Checks both times.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if crate::client_api::parse_time(&self.start).is_none() {
            return Err(ValidationError::new(
                "quiet_hours.start",
                "must be HH:MM from 00:00 to 23:59",
            ));
        }
        if crate::client_api::parse_time(&self.end).is_none() {
            return Err(ValidationError::new(
                "quiet_hours.end",
                "must be HH:MM from 00:00 to 23:59",
            ));
        }
        Ok(())
    }
}

/// The proactivity settings of the hub. Every client reads the same
/// record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProactivitySettings {
    /// One range for every channel.
    pub quiet_hours: QuietHours,
    /// The user's IANA time zone, for quiet hours, the brief and the
    /// research pass. Empty until a client sets it; the hub uses UTC until
    /// then.
    pub time_zone: Option<String>,
    /// While set and in the future, check-ins, nudges and the research pass
    /// do not run, and nothing notifies.
    pub snoozed_until: Option<Timestamp>,
    /// Pause all: nothing starts on its own in any channel.
    pub paused: bool,
    /// The most notifying proactive messages in one day.
    pub notify_cap_per_day: u32,
}

impl Default for ProactivitySettings {
    fn default() -> Self {
        Self {
            quiet_hours: QuietHours::default(),
            time_zone: None,
            snoozed_until: None,
            paused: false,
            notify_cap_per_day: NOTIFY_CAP_DEFAULT,
        }
    }
}

impl ProactivitySettings {
    /// Whether a snooze is on at `now`.
    pub fn is_snoozed(&self, now: Timestamp) -> bool {
        self.snoozed_until.is_some_and(|until| until > now)
    }
}

/// A snooze choice in Settings > Proactivity and in `set_proactivity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnoozeChoice {
    /// End the snooze.
    Off,
    /// For 2 hours.
    TwoHours,
    /// Until the end of quiet hours tomorrow.
    UntilTomorrow,
    /// For 7 days.
    Week,
}

/// What came of one proactive wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProactiveOutcome {
    /// The wake waits for its turn, or its turn runs.
    Pending,
    /// The turn posted at least one message, with no notification.
    Posted,
    /// The turn posted a message that notified.
    Notified,
    /// The turn posted nothing.
    Silent,
    /// The hub skipped the wake: nothing changed since the last check-in.
    SkippedNoChange,
    /// The hub skipped the wake: quiet hours.
    SkippedQuietHours,
    /// The hub skipped the wake: snoozed.
    SkippedSnoozed,
    /// The hub skipped the wake: Pause all.
    SkippedPaused,
    /// The hub skipped the wake: the level of the channel. The hub writes
    /// no row for this outcome; the value exists for older records and
    /// tools.
    SkippedLevel,
    /// The hub skipped the wake: the daily turn limit of the channel.
    SkippedBudget,
    /// The hub skipped the wake: the last one still waited for a turn.
    SkippedBusy,
    /// The turn failed.
    Failed,
}

impl ProactiveOutcome {
    /// Every outcome.
    pub const ALL: [ProactiveOutcome; 12] = [
        Self::Pending,
        Self::Posted,
        Self::Notified,
        Self::Silent,
        Self::SkippedNoChange,
        Self::SkippedQuietHours,
        Self::SkippedSnoozed,
        Self::SkippedPaused,
        Self::SkippedLevel,
        Self::SkippedBudget,
        Self::SkippedBusy,
        Self::Failed,
    ];

    /// The wire text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Posted => "posted",
            Self::Notified => "notified",
            Self::Silent => "silent",
            Self::SkippedNoChange => "skipped_no_change",
            Self::SkippedQuietHours => "skipped_quiet_hours",
            Self::SkippedSnoozed => "skipped_snoozed",
            Self::SkippedPaused => "skipped_paused",
            Self::SkippedLevel => "skipped_level",
            Self::SkippedBudget => "skipped_budget",
            Self::SkippedBusy => "skipped_busy",
            Self::Failed => "failed",
        }
    }

    /// Reads the wire text.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == text)
    }

    /// The result in words, for the log in Settings > Proactivity.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Running",
            Self::Posted => "Posted",
            Self::Notified => "Notified",
            Self::Silent => "Stayed silent",
            Self::SkippedNoChange => "Skipped: nothing changed",
            Self::SkippedQuietHours => "Skipped: quiet hours",
            Self::SkippedSnoozed => "Skipped: snoozed",
            Self::SkippedPaused => "Skipped: paused",
            Self::SkippedLevel => "Skipped: level",
            Self::SkippedBudget => "Skipped: over the turn limit",
            Self::SkippedBusy => "Skipped: a turn was running",
            Self::Failed => "Failed",
        }
    }

    /// Whether the hub stopped the wake before a model ran.
    pub fn is_skipped(self) -> bool {
        matches!(
            self,
            Self::SkippedNoChange
                | Self::SkippedQuietHours
                | Self::SkippedSnoozed
                | Self::SkippedPaused
                | Self::SkippedLevel
                | Self::SkippedBudget
                | Self::SkippedBusy
        )
    }
}

/// One proactive wake in the proactivity log. It is a feed entity, so a
/// client sees a wake start and end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactiveWake {
    /// The number. It is unique in the hub and grows with each wake.
    pub id: u64,
    /// The channel.
    pub channel_id: ChannelId,
    /// What woke the assistant.
    pub kind: ProactiveKind,
    /// When the wake was due.
    pub at: Timestamp,
    /// What came of it.
    pub outcome: ProactiveOutcome,
    /// One line: the finding of a silent turn, or why the hub skipped the
    /// wake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The messages that the turn posted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_ids: Vec<MessageId>,
    /// The task that the turn started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    /// The routine that woke the assistant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine_id: Option<RoutineId>,
    /// True when the turn asked to notify and the hub posted without a
    /// notification: quiet hours, snooze, or over the notification limit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notify_held: bool,
    /// When the turn ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
}

impl ProactiveWake {
    /// The feed `entity_id`: the number.
    pub fn entity_id(&self) -> String {
        self.id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn levels_order_from_off_to_eager_and_gate_each_kind() {
        use ProactiveKind as K;
        use ProactivityLevel as L;
        assert!(L::Off < L::Quiet && L::Quiet < L::Normal && L::Normal < L::Eager);
        assert_eq!(L::default_for(ChannelKind::Tutti), L::Normal);
        assert_eq!(L::default_for(ChannelKind::Standard), L::Quiet);
        assert!(!L::Off.allows(K::FollowUp));
        assert!(L::Off.allows(K::Feedback));
        assert!(L::Quiet.allows(K::StaleTask) && L::Quiet.allows(K::ChannelCreated));
        assert!(!L::Quiet.allows(K::CheckIn));
        assert!(L::Normal.allows(K::Brief) && !L::Normal.allows(K::Research));
        assert!(L::Eager.allows(K::Research));
        for level in L::ALL {
            assert_eq!(L::parse(level.as_str()), Some(level));
            assert_eq!(serde_json::to_value(level).unwrap(), json!(level.as_str()));
        }
        for kind in K::ALL {
            assert_eq!(K::parse(kind.as_str()), Some(kind));
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
        }
    }

    #[test]
    fn quiet_hours_cross_midnight_and_an_empty_range_contains_nothing() {
        let night = QuietHours::default();
        assert!(night.contains_minute(21 * 60));
        assert!(night.contains_minute(23 * 60 + 59));
        assert!(night.contains_minute(0));
        assert!(night.contains_minute(6 * 60 + 59));
        assert!(!night.contains_minute(7 * 60));
        assert!(!night.contains_minute(12 * 60));
        let day = QuietHours {
            start: "12:00".into(),
            end: "13:30".into(),
        };
        assert!(day.contains_minute(12 * 60 + 15));
        assert!(!day.contains_minute(13 * 60 + 30));
        let empty = QuietHours {
            start: "08:00".into(),
            end: "08:00".into(),
        };
        assert!(!empty.contains_minute(8 * 60));
        let broken = QuietHours {
            start: "25:00".into(),
            end: "08:00".into(),
        };
        assert!(!broken.contains_minute(1));
        assert!(broken.validate().is_err());
        assert!(night.validate().is_ok());
    }

    #[test]
    fn suggestions_have_a_limit_of_four_short_labels() {
        let ok = vec![
            Suggestion::send("Yes"),
            Suggestion::type_it("I will type it"),
        ];
        assert!(validate_suggestions(&ok).is_ok());
        let five: Vec<_> = (0..5).map(|i| Suggestion::send(format!("{i}"))).collect();
        assert!(validate_suggestions(&five).is_err());
        let long = vec![Suggestion::send("x".repeat(41))];
        assert!(validate_suggestions(&long).is_err());
        let copy = vec![Suggestion::send("Yes"), Suggestion::send("Yes")];
        assert!(validate_suggestions(&copy).is_err());
        let blank = vec![Suggestion::send(" ")];
        assert!(validate_suggestions(&blank).is_err());
        // A suggestion without an action sends.
        let read: Suggestion = serde_json::from_value(json!({"label": "Yes"})).unwrap();
        assert_eq!(read.action, SuggestionAction::Send);
        assert_eq!(
            serde_json::to_value(Suggestion::type_it("I will type it")).unwrap(),
            json!({"label": "I will type it", "action": "type"})
        );
    }

    #[test]
    fn settings_and_wakes_round_trip_and_old_records_read() {
        let settings = ProactivitySettings {
            time_zone: Some("America/Chicago".into()),
            snoozed_until: Some(Timestamp::from_millis(10)),
            ..ProactivitySettings::default()
        };
        let value = serde_json::to_value(&settings).unwrap();
        assert_eq!(value["quiet_hours"]["start"], "21:00");
        assert_eq!(value["notify_cap_per_day"], 3);
        assert_eq!(
            serde_json::from_value::<ProactivitySettings>(value).unwrap(),
            settings
        );
        let old: ProactivitySettings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(old, ProactivitySettings::default());
        assert!(settings.is_snoozed(Timestamp::from_millis(9)));
        assert!(!settings.is_snoozed(Timestamp::from_millis(10)));

        let wake = ProactiveWake {
            id: 4,
            channel_id: "chn_00000000-0000-0000-0000-000000000001".parse().unwrap(),
            kind: ProactiveKind::CheckIn,
            at: Timestamp::from_millis(1_000),
            outcome: ProactiveOutcome::SkippedNoChange,
            detail: Some("No change since the brief.".into()),
            message_ids: Vec::new(),
            task_id: None,
            routine_id: None,
            notify_held: false,
            ended_at: None,
        };
        let value = serde_json::to_value(&wake).unwrap();
        assert_eq!(value["outcome"], "skipped_no_change");
        assert!(value.get("message_ids").is_none());
        assert_eq!(
            serde_json::from_value::<ProactiveWake>(value).unwrap(),
            wake
        );
        for outcome in ProactiveOutcome::ALL {
            assert_eq!(ProactiveOutcome::parse(outcome.as_str()), Some(outcome));
        }
        assert_eq!(
            ProactiveOutcome::SkippedBudget.label(),
            "Skipped: over the turn limit"
        );
        assert_eq!(
            ProactiveOutcome::SkippedBusy.label(),
            "Skipped: a turn was running"
        );
    }

    #[test]
    fn a_mark_leaves_out_what_is_not_set() {
        let mark = ProactiveMark::new(ProactiveKind::CheckIn);
        assert_eq!(
            serde_json::to_value(&mark).unwrap(),
            json!({"kind": "check_in", "notify": false})
        );
        let full = ProactiveMark {
            notify: true,
            reason: Some("You asked me to check the vendor reply today.".into()),
            offer: true,
            rating: Some(ProactiveRating::TooNoisy),
            ..ProactiveMark::new(ProactiveKind::FollowUp)
        };
        let value = serde_json::to_value(&full).unwrap();
        assert_eq!(value["rating"], "too_noisy");
        assert_eq!(
            serde_json::from_value::<ProactiveMark>(value).unwrap(),
            full
        );
    }
}
