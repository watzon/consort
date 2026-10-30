//! The assistant on its own, as a client shows it: Settings > Proactivity,
//! the proactive marks and quick replies in the timeline, the writing
//! bubble, and the rules for quiet hours.
//!
//! The hub decides what runs (see `docs/PROACTIVITY-PLAN.md`). The client
//! shows the settings, the level of each channel, today's numbers and the
//! log, and marks the messages that a proactive turn posted.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use consort_proto::model::{
    self, Channel, ChannelKind, MessageKind, ProactiveOutcome, ProactiveWake, ProactivitySettings,
    SnoozeChoice, SuggestionAction,
};

pub use consort_proto::model::{
    CHECK_IN_NOTE_PATH, ChannelKind as LevelKind, ProactiveKind, ProactiveRating, ProactivityLevel,
    RoutineKind,
};

use crate::cache::Cache;
use crate::connectors::LoadState;
use crate::view::{NoticeView, Speaker, TimelineItem, TimelineView, ViewState};

/// How many log entries the client asks for at start and for the page.
pub const LOG_PAGE: u32 = 200;

// Cache ----------------------------------------------------------------------

/// What the cache keeps about proactivity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProactivityCache {
    /// The settings, from the `all` snapshot and the feed. Empty from an
    /// older hub.
    pub settings: Option<ProactivitySettings>,
    /// The log entries that the client loaded or that the feed brought.
    pub wakes: BTreeMap<u64, ProactiveWake>,
    pub log_load: LoadState,
    /// Older entries exist on the hub.
    pub log_has_more: bool,
    /// What went wrong with the last change.
    pub error: Option<String>,
}

impl ProactivityCache {
    /// Applies settings from the feed. Returns whether they changed.
    pub fn apply_settings(&mut self, settings: ProactivitySettings) -> bool {
        if self.settings.as_ref() == Some(&settings) {
            return false;
        }
        self.settings = Some(settings);
        true
    }

    /// Applies one log entry. Returns whether it changed.
    pub fn apply_wake(&mut self, wake: ProactiveWake) -> bool {
        if self.wakes.get(&wake.id) == Some(&wake) {
            return false;
        }
        self.wakes.insert(wake.id, wake);
        true
    }

    /// Takes a page of the log.
    pub fn load_page(&mut self, wakes: Vec<ProactiveWake>, has_more: bool) {
        for wake in wakes {
            self.wakes.insert(wake.id, wake);
        }
        self.log_has_more = has_more;
        self.log_load = LoadState::Loaded;
    }

    /// Drops the log of a deleted channel.
    pub fn drop_channel(&mut self, channel_id: &str) {
        self.wakes
            .retain(|_, w| w.channel_id.to_string() != channel_id);
    }
}

/// A change of the settings. Fields that are `None` do not change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProactivityChanges {
    pub paused: Option<bool>,
    /// `HH:MM`.
    pub quiet_start: Option<String>,
    /// `HH:MM`.
    pub quiet_end: Option<String>,
    /// An IANA time zone.
    pub time_zone: Option<String>,
}

/// The `proactivity.set` request of a change, with the quiet hours whole.
pub fn set_request(
    changes: &ProactivityChanges,
    current: Option<&ProactivitySettings>,
) -> consort_proto::client_api::ProactivitySet {
    let current_hours = current.map(|s| s.quiet_hours.clone()).unwrap_or_default();
    let quiet_hours =
        (changes.quiet_start.is_some() || changes.quiet_end.is_some()).then(|| model::QuietHours {
            start: changes
                .quiet_start
                .clone()
                .unwrap_or(current_hours.start.clone()),
            end: changes
                .quiet_end
                .clone()
                .unwrap_or(current_hours.end.clone()),
        });
    consort_proto::client_api::ProactivitySet {
        paused: changes.paused,
        quiet_hours,
        time_zone: changes.time_zone.clone(),
        ..Default::default()
    }
}

// Time -----------------------------------------------------------------------

fn zone(settings: &ProactivitySettings) -> Tz {
    settings
        .time_zone
        .as_deref()
        .and_then(|z| z.parse::<Tz>().ok())
        .unwrap_or(Tz::UTC)
}

fn in_zone(ms: i64, tz: Tz) -> DateTime<Tz> {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        .with_timezone(&tz)
}

/// Whether `now` is in the quiet hours or a snooze: nothing notifies then.
pub fn quiet_now(settings: Option<&ProactivitySettings>, now_ms: i64) -> bool {
    let Some(settings) = settings else {
        return false;
    };
    if settings.is_snoozed(consort_proto::time::Timestamp::from_millis(now_ms)) {
        return true;
    }
    let local = in_zone(now_ms, zone(settings));
    settings
        .quiet_hours
        .contains_minute(local.hour() * 60 + local.minute())
}

/// "1:45 PM" on this computer's clock.
fn clock(ms: i64) -> String {
    crate::format::time_label(ms)
}

/// The local midnight of the user's day, in the hub's time zone for the
/// user. "Today" in Settings > Proactivity counts from here.
pub fn start_of_day(settings: Option<&ProactivitySettings>, now_ms: i64) -> i64 {
    let tz = settings.map(zone).unwrap_or(Tz::UTC);
    let date = in_zone(now_ms, tz).date_naive();
    date.and_hms_opt(0, 0, 0)
        .and_then(|naive| tz.from_local_datetime(&naive).earliest())
        .map(|t| t.timestamp_millis())
        .unwrap_or(now_ms)
}

// Words ----------------------------------------------------------------------

/// The line under a channel in Settings > Proactivity: what its level does.
pub fn level_meaning(level: ProactivityLevel, kind: ChannelKind, check_in_hours: u64) -> String {
    match (level, kind) {
        (ProactivityLevel::Off, _) => "The assistant only answers here.".to_owned(),
        (ProactivityLevel::Quiet, _) => {
            "Follow-ups it set, nudges for stale tasks, the start of a new channel.".to_owned()
        }
        (ProactivityLevel::Normal, ChannelKind::Tutti) => {
            format!("Quiet, plus a check-in every {check_in_hours} hours and the morning brief.")
        }
        (ProactivityLevel::Normal, ChannelKind::Standard) => {
            format!("Quiet, plus a check-in every {check_in_hours} hours.")
        }
        (ProactivityLevel::Eager, _) => "Normal, plus a daily research pass and offers.".to_owned(),
    }
}

/// The helper text under the Proactivity select of the channel settings
/// dialog.
pub fn level_helper(level: ProactivityLevel, kind: ChannelKind) -> String {
    let what = match (level, kind) {
        (ProactivityLevel::Off, _) => "Off: the assistant only answers here.",
        (ProactivityLevel::Quiet, _) => {
            "Quiet: follow-ups it set, nudges for stale tasks and the start of a new channel."
        }
        (ProactivityLevel::Normal, ChannelKind::Tutti) => {
            "Normal: Quiet, plus check-ins and the morning brief."
        }
        (ProactivityLevel::Normal, ChannelKind::Standard) => "Normal: Quiet, plus check-ins.",
        (ProactivityLevel::Eager, _) => {
            "Eager: Normal, plus a daily read-only research pass and offers."
        }
    };
    format!("{what} Only you can raise the level.")
}

/// The icon of a proactive mark: `eye` for a check-in, `sun` for the
/// morning brief, `clock` for a follow-up, `compose` for an offer.
pub fn mark_icon(kind: ProactiveKind, offer: bool) -> &'static str {
    if offer {
        return "compose";
    }
    match kind {
        ProactiveKind::CheckIn => "eye",
        ProactiveKind::Brief => "sun",
        ProactiveKind::FollowUp => "clock",
        ProactiveKind::StaleTask => "needs-input",
        ProactiveKind::Research => "search",
        ProactiveKind::ChannelCreated => "channel",
        ProactiveKind::Feedback => "message",
        ProactiveKind::Voice => "quote",
    }
}

/// The caption of a proactive mark.
pub fn mark_label(kind: ProactiveKind, offer: bool) -> &'static str {
    if offer { "Offer" } else { kind.label() }
}

// Timeline -------------------------------------------------------------------

/// The proactive mark of a message in the timeline.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProactiveMarkView {
    pub kind: Option<ProactiveKind>,
    /// "Check-in", "Morning brief", "Follow-up" or "Offer".
    pub label: String,
    pub icon: String,
    /// For a follow-up: why the assistant set it.
    pub reason: Option<String>,
    pub offer: bool,
    pub notify: bool,
    /// The user's rating.
    pub rating: Option<ProactiveRating>,
    /// True on the first bubble of a group: the caption shows only there.
    pub show_caption: bool,
}

/// A quick reply under the newest assistant message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SuggestionView {
    pub label: String,
    /// True for "I will type it": a click only focuses the composer.
    pub type_it: bool,
}

/// The mark of a message, from its proactive record.
fn mark_view(mark: &model::ProactiveMark) -> ProactiveMarkView {
    ProactiveMarkView {
        kind: Some(mark.kind),
        label: mark_label(mark.kind, mark.offer).to_owned(),
        icon: mark_icon(mark.kind, mark.offer).to_owned(),
        reason: mark.reason.clone(),
        offer: mark.offer,
        notify: mark.notify,
        rating: mark.rating,
        show_caption: true,
    }
}

/// Puts the proactive marks and the quick replies on the timelines, and the
/// line "You made the channel X." at the start of a channel whose whole
/// history is loaded. The reducer calls it after it builds the timelines.
pub fn decorate_timelines(cache: &Cache, timelines: &mut [TimelineView]) {
    for timeline in timelines.iter_mut() {
        let mut last_mark: Option<(Option<ProactiveKind>, bool)> = None;
        for item in &mut timeline.items {
            match item {
                TimelineItem::Message(m) => {
                    let record = cache.messages.get(&m.id);
                    let mark = record.and_then(|r| r.proactive.as_ref()).map(mark_view);
                    m.proactive = mark.map(|mut mark| {
                        // One caption per group of bubbles from one wake.
                        let key = (mark.kind, mark.offer);
                        mark.show_caption = last_mark != Some(key) || mark.reason.is_some();
                        last_mark = Some(key);
                        mark
                    });
                    if m.proactive.is_none() {
                        last_mark = None;
                    }
                    m.suggestions = record
                        .map(|r| {
                            r.suggestions
                                .iter()
                                .map(|s| SuggestionView {
                                    label: s.label.clone(),
                                    type_it: s.action == SuggestionAction::Type,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                }
                TimelineItem::Decision(d) => {
                    d.proactive = cache
                        .messages
                        .get(&d.message_id)
                        .and_then(|r| r.proactive.as_ref())
                        .map(mark_view);
                    last_mark = None;
                }
                TimelineItem::Day(_) => {}
                _ => last_mark = None,
            }
        }
        keep_newest_suggestions(timeline);
        if timeline.loaded
            && !timeline.has_older
            && let Some(channel) = cache.channels.get(&timeline.channel_id)
            && !channel.is_tutti()
        {
            add_made_line(timeline, channel);
        }
    }
}

/// Quick replies show only under the newest assistant message, and only
/// while the user has not answered after it.
fn keep_newest_suggestions(timeline: &mut TimelineView) {
    let newest = timeline
        .items
        .iter()
        .rposition(|item| matches!(item, TimelineItem::Message(_)));
    for (index, item) in timeline.items.iter_mut().enumerate() {
        if let TimelineItem::Message(m) = item
            && (Some(index) != newest || m.speaker != Speaker::Consort)
        {
            m.suggestions.clear();
        }
    }
}

/// The key of the line "You made the channel X.".
pub fn made_line_key(channel_id: &str) -> String {
    format!("made-{channel_id}")
}

fn add_made_line(timeline: &mut TimelineView, channel: &Channel) {
    let at = channel.created_at.as_millis();
    let line = TimelineItem::Notice(NoticeView {
        message_id: made_line_key(&channel.id.to_string()),
        time_label: crate::format::time_label(at),
        created_at: at,
        kind: "channel_created".to_owned(),
        text: format!("You made the channel {}.", channel.name),
        memory: None,
    });
    // After the day line of the first day, before every message.
    let at_index = timeline
        .items
        .iter()
        .position(|item| !matches!(item, TimelineItem::Day(_)))
        .unwrap_or(timeline.items.len());
    timeline.items.insert(at_index, line);
}

/// Whether the writing bubble shows in a channel: a coordinator turn runs
/// and has not posted yet. That is when the user's message is the newest
/// one and reached the hub, or when a new channel has no message yet and
/// its assistant works on the greeting. A proactive turn in a channel with
/// messages shows no bubble, because it may end in silence.
pub fn is_writing(view: &ViewState, channel_id: &str) -> bool {
    let Some(timeline) = view.timeline(channel_id) else {
        return false;
    };
    let newest = timeline.items.iter().rev().find(|item| {
        matches!(
            item,
            TimelineItem::Message(_)
                | TimelineItem::Decision(_)
                | TimelineItem::TaskCard(_)
                | TimelineItem::Approval(_)
        )
    });
    match newest {
        Some(TimelineItem::Message(m)) => m.speaker == Speaker::You && !m.pending,
        Some(_) => false,
        None => {
            timeline.loaded
                && view.activity(channel_id).state == crate::activity::MarkState::Working
        }
    }
}

// The morning brief ---------------------------------------------------------

/// The three groups of the morning brief, in order.
pub const BRIEF_GROUPS: [&str; 3] = ["Needs you", "Ready for review", "Done since yesterday"];

/// One item of the brief: its number, the linked name and the rest of the
/// line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BriefLine {
    pub number: u32,
    pub name: String,
    /// `task:<id>`, `message:<id>` or a web address. Empty without a link.
    pub target: String,
    pub rest: String,
}

/// One group of the brief.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BriefGroup {
    pub label: String,
    /// True for the groups that are the user's turn: their label is in the
    /// accent color.
    pub your_turn: bool,
    pub lines: Vec<BriefLine>,
}

/// A morning brief, read from the Markdown that the coordinator posted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BriefView {
    pub intro: String,
    pub groups: Vec<BriefGroup>,
}

/// Reads a brief: a first paragraph, then lines that name a group, each
/// followed by numbered items such as `1. [Name.](task:tsk_1) The rest.`.
/// `None` when the text has no group, so it shows as plain Markdown.
pub fn parse_brief(body: &str) -> Option<BriefView> {
    let mut view = BriefView::default();
    let mut intro: Vec<&str> = Vec::new();
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let label = line.trim_matches(|c| c == '*' || c == '#' || c == ' ' || c == ':');
        if let Some(group) = BRIEF_GROUPS.iter().find(|g| g.eq_ignore_ascii_case(label)) {
            view.groups.push(BriefGroup {
                label: (*group).to_owned(),
                your_turn: *group != BRIEF_GROUPS[2],
                lines: Vec::new(),
            });
            continue;
        }
        match view.groups.last_mut() {
            None => intro.push(line),
            Some(group) => {
                let (number, text) = match line.split_once(". ") {
                    Some((n, rest)) if n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty() => {
                        (n.parse().unwrap_or(0), rest)
                    }
                    _ => (0, line.trim_start_matches(['-', '*', ' '])),
                };
                let (name, target, rest) = match text
                    .strip_prefix('[')
                    .and_then(|t| t.split_once("]("))
                    .and_then(|(name, t)| {
                        t.split_once(')').map(|(target, rest)| (name, target, rest))
                    }) {
                    Some((name, target, rest)) => (name, target, rest.trim()),
                    None => ("", "", text),
                };
                group.lines.push(BriefLine {
                    number,
                    name: name.to_owned(),
                    target: target.to_owned(),
                    rest: rest.to_owned(),
                });
            }
        }
    }
    if view.groups.is_empty() {
        return None;
    }
    view.intro = intro.join(" ");
    Some(view)
}

/// Whether a message is a morning brief: it shows as one bubble.
pub fn is_brief(message: &crate::view::MessageView) -> bool {
    message
        .proactive
        .as_ref()
        .is_some_and(|m| m.kind == Some(ProactiveKind::Brief))
}

// Settings > Proactivity -----------------------------------------------------

/// A snooze choice in the segmented control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SnoozeOption {
    #[default]
    Off,
    TwoHours,
    UntilTomorrow,
    Week,
}

impl SnoozeOption {
    pub const ALL: [SnoozeOption; 4] = [Self::Off, Self::TwoHours, Self::UntilTomorrow, Self::Week];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::TwoHours => "2 hours",
            Self::UntilTomorrow => "Until tomorrow",
            Self::Week => "A week",
        }
    }

    pub fn choice(self) -> SnoozeChoice {
        match self {
            Self::Off => SnoozeChoice::Off,
            Self::TwoHours => SnoozeChoice::TwoHours,
            Self::UntilTomorrow => SnoozeChoice::UntilTomorrow,
            Self::Week => SnoozeChoice::Week,
        }
    }
}

/// One channel in "Each channel".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelLevelView {
    pub channel_id: String,
    pub name: String,
    pub icon: String,
    pub tutti: bool,
    pub level: Option<ProactivityLevel>,
    /// The level that applies: the channel's own or the default.
    pub effective: Option<ProactivityLevel>,
    /// True when the channel has no level of its own.
    pub is_default: bool,
    pub meaning: String,
}

/// The tone of a result in the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ResultTone {
    /// It asked for you: `accent.text`.
    Notified,
    /// `status.failed`.
    Failed,
    /// `text.primary`.
    Posted,
    /// Skipped and silent: `text.tertiary`.
    #[default]
    Quiet,
}

/// One row of the log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogRowView {
    pub id: u64,
    pub channel_id: String,
    pub channel_name: String,
    pub kind: Option<ProactiveKind>,
    pub kind_label: String,
    pub time_label: String,
    pub at: i64,
    pub result: String,
    pub tone: ResultTone,
    pub detail: String,
    /// The message that "Open message" opens, and its thread root when it
    /// is a reply.
    pub message: Option<(String, Option<String>)>,
}

/// A group of log rows under "Today", "Yesterday" or a date.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogGroupView {
    pub label: String,
    pub rows: Vec<LogRowView>,
}

/// A follow-up that waits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FollowUpView {
    pub routine_id: String,
    pub channel_id: String,
    /// The reason, as the title.
    pub reason: String,
    /// "Today at 15:00, set this morning".
    pub when: String,
    pub at: i64,
}

/// Settings > Proactivity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProactivityView {
    /// False until the hub sent the settings. An older hub never does.
    pub available: bool,
    pub paused: bool,
    pub quiet_start: String,
    pub quiet_end: String,
    pub time_zone: String,
    pub snooze: SnoozeOption,
    /// "Now: snoozed until 1:45 PM. ..." or what a snooze does.
    pub snooze_line: String,
    pub follow_ups: Vec<FollowUpView>,
    /// "2 are waiting, both in house-lights. ..."
    pub follow_ups_line: String,
    pub channels: Vec<ChannelLevelView>,
    /// Proactive turns today.
    pub turns_today: u32,
    pub turns_line: String,
    pub notified_today: u32,
    pub notify_cap: u32,
    pub notify_line: String,
    /// The morning brief routine and its line.
    pub brief: Option<(String, String)>,
    pub log: Vec<LogGroupView>,
    pub log_load: LoadState,
    pub log_has_more: bool,
    pub error: Option<String>,
}

fn snooze_option(settings: &ProactivitySettings, now_ms: i64) -> SnoozeOption {
    let Some(until) = settings
        .snoozed_until
        .filter(|u| u.as_millis() > now_ms)
        .map(|u| u.as_millis())
    else {
        return SnoozeOption::Off;
    };
    let left = until - now_ms;
    const HOUR: i64 = 3_600_000;
    if left <= 2 * HOUR + 60_000 {
        SnoozeOption::TwoHours
    } else if left <= 36 * HOUR {
        SnoozeOption::UntilTomorrow
    } else {
        SnoozeOption::Week
    }
}

fn day_words(at: i64, now_ms: i64) -> String {
    let (Some(day), Some(now)) = (
        Local.timestamp_millis_opt(at).single(),
        Local.timestamp_millis_opt(now_ms).single(),
    ) else {
        return String::new();
    };
    match day
        .date_naive()
        .signed_duration_since(now.date_naive())
        .num_days()
    {
        0 => "Today".to_owned(),
        1 => "Tomorrow".to_owned(),
        -1 => "Yesterday".to_owned(),
        _ if day.year() == now.year() => day.format("%b %-d").to_string(),
        _ => day.format("%b %-d, %Y").to_string(),
    }
}

/// "this morning", "yesterday", "on Oct 27".
fn set_words(at: i64, now_ms: i64) -> String {
    let (Some(day), Some(now)) = (
        Local.timestamp_millis_opt(at).single(),
        Local.timestamp_millis_opt(now_ms).single(),
    ) else {
        return String::new();
    };
    match now
        .date_naive()
        .signed_duration_since(day.date_naive())
        .num_days()
    {
        0 if day.hour() < 12 => "set this morning".to_owned(),
        0 if day.hour() < 18 => "set this afternoon".to_owned(),
        0 => "set this evening".to_owned(),
        1 => "set yesterday".to_owned(),
        _ => format!("set on {}", day.format("%b %-d")),
    }
}

/// The time line of a follow-up row: "Today at 15:00, set this morning".
pub fn follow_up_when(at: i64, set_at: Option<i64>, now_ms: i64) -> String {
    let when = format!(
        "{} at {}",
        day_words(at, now_ms),
        crate::format::clock_24(at)
    );
    match set_at {
        Some(set) => format!("{when}, {}", set_words(set, now_ms)),
        None => when,
    }
}

fn channel_name(cache: &Cache, id: &str) -> String {
    cache
        .channels
        .get(id)
        .map(|c| {
            if c.is_tutti() {
                crate::view::TUTTI.to_owned()
            } else {
                c.name.clone()
            }
        })
        .unwrap_or_default()
}

fn outcome_tone(outcome: ProactiveOutcome) -> ResultTone {
    match outcome {
        ProactiveOutcome::Notified => ResultTone::Notified,
        ProactiveOutcome::Failed => ResultTone::Failed,
        ProactiveOutcome::Posted => ResultTone::Posted,
        _ => ResultTone::Quiet,
    }
}

/// The log rows, newest first, grouped by day.
pub fn log_groups(
    cache: &Cache,
    channel: Option<&str>,
    kind: Option<ProactiveKind>,
    now_ms: i64,
) -> Vec<LogGroupView> {
    let mut groups: Vec<LogGroupView> = Vec::new();
    for wake in cache.proactivity.wakes.values().rev() {
        let channel_id = wake.channel_id.to_string();
        if channel.is_some_and(|c| c != channel_id) || kind.is_some_and(|k| k != wake.kind) {
            continue;
        }
        let at = wake.at.as_millis();
        let message = wake
            .message_ids
            .first()
            .filter(|_| {
                matches!(
                    wake.outcome,
                    ProactiveOutcome::Posted | ProactiveOutcome::Notified
                )
            })
            .map(|id| {
                let id = id.to_string();
                let root = cache
                    .messages
                    .get(&id)
                    .and_then(|m| m.thread_root_id)
                    .map(|r| r.to_string());
                (id, root)
            });
        let row = LogRowView {
            id: wake.id,
            channel_name: channel_name(cache, &channel_id),
            channel_id,
            kind: Some(wake.kind),
            kind_label: wake.kind.label().to_owned(),
            time_label: clock(at),
            at,
            result: wake.outcome.label().to_owned(),
            tone: outcome_tone(wake.outcome),
            detail: wake.detail.clone().unwrap_or_default(),
            message,
        };
        let label = crate::format::day_label(at, now_ms);
        match groups.last_mut() {
            Some(group) if group.label == label => group.rows.push(row),
            _ => groups.push(LogGroupView {
                label,
                rows: vec![row],
            }),
        }
    }
    groups
}

/// The interval of a channel's check-in in hours, for its meaning line.
fn check_in_hours(cache: &Cache, channel_id: &str) -> u64 {
    cache
        .routines
        .values()
        .find(|r| r.kind == RoutineKind::CheckIn && r.channel_id.to_string() == channel_id)
        .and_then(|r| match r.schedule {
            model::RoutineSchedule::Interval { seconds } => Some(seconds.div_ceil(3600)),
            _ => None,
        })
        .unwrap_or(
            if cache
                .channels
                .get(channel_id)
                .is_some_and(Channel::is_tutti)
            {
                3
            } else {
                6
            },
        )
}

/// Builds Settings > Proactivity from the cache.
pub fn proactivity_view(cache: &Cache, now_ms: i64) -> ProactivityView {
    let Some(settings) = cache.proactivity.settings.as_ref() else {
        return ProactivityView {
            log_load: cache.proactivity.log_load.clone(),
            error: cache.proactivity.error.clone(),
            ..ProactivityView::default()
        };
    };
    let snooze = snooze_option(settings, now_ms);
    let snooze_line = match settings.snoozed_until.filter(|u| u.as_millis() > now_ms) {
        Some(until) => format!(
            "Now: snoozed until {}. Follow-ups that the assistant set still run, but they do not notify.",
            clock(until.as_millis())
        ),
        None => "Check-ins, nudges and the research pass wait. Follow-ups and the brief still run, but they do not notify.".to_owned(),
    };

    let mut follow_ups: Vec<FollowUpView> = cache
        .routines
        .values()
        .filter(|r| r.kind == RoutineKind::FollowUp && r.enabled)
        .map(|r| {
            let at = r.next_run_at.map(|t| t.as_millis()).unwrap_or_default();
            FollowUpView {
                routine_id: r.id.to_string(),
                channel_id: r.channel_id.to_string(),
                reason: r.prompt.clone(),
                when: follow_up_when(at, r.created_at.map(|t| t.as_millis()), now_ms),
                at,
            }
        })
        .collect();
    follow_ups.sort_by_key(|f| (f.at, f.routine_id.clone()));
    let follow_ups_line = follow_ups_line(cache, &follow_ups);

    let mut channels: Vec<&Channel> = cache
        .channels
        .values()
        .filter(|c| c.archived_at.is_none())
        .collect();
    channels.sort_by_key(|c| (!c.is_tutti(), c.name.to_lowercase()));
    let channels = channels
        .into_iter()
        .map(|c| {
            let id = c.id.to_string();
            let effective = c.proactivity_level();
            ChannelLevelView {
                name: channel_name(cache, &id),
                icon: c.icon.clone().unwrap_or_else(|| "channel".to_owned()),
                tutti: c.is_tutti(),
                level: c.proactivity,
                effective: Some(effective),
                is_default: c.proactivity.is_none(),
                meaning: level_meaning(effective, c.kind, check_in_hours(cache, &id)),
                channel_id: id,
            }
        })
        .collect();

    let day = start_of_day(Some(settings), now_ms);
    let today: Vec<&ProactiveWake> = cache
        .proactivity
        .wakes
        .values()
        .filter(|w| w.at.as_millis() >= day)
        .collect();
    let turns_today = u32::try_from(today.len()).unwrap_or(u32::MAX);
    let skipped = today.iter().filter(|w| w.outcome.is_skipped()).count();
    let ran = today.len() - skipped;
    let notified_today = u32::try_from(
        today
            .iter()
            .filter(|w| w.outcome == ProactiveOutcome::Notified)
            .count(),
    )
    .unwrap_or(u32::MAX);
    let turns_line = format!(
        "{ran} ran a model. {skipped} {} skipped before a model ran.",
        if skipped == 1 { "was" } else { "were" }
    );
    let cap = settings.notify_cap_per_day;
    let notify_line = format!("After {cap}, the assistant posts without a notification.");

    let brief = cache
        .routines
        .values()
        .find(|r| r.kind == RoutineKind::Brief)
        .map(|r| {
            let schedule = crate::routines::schedule_words(&r.schedule);
            let came = today
                .iter()
                .find(|w| w.kind == ProactiveKind::Brief && !w.outcome.is_skipped())
                .map(|w| format!(" Today's brief came at {}.", clock(w.at.as_millis())))
                .unwrap_or_default();
            let line = if r.enabled {
                format!("{schedule} in Tutti.{came} Change the time or the days in the routine.")
            } else {
                "Off. Turn it on in the routine.".to_owned()
            };
            (r.id.to_string(), line)
        });

    ProactivityView {
        available: true,
        paused: settings.paused,
        quiet_start: settings.quiet_hours.start.clone(),
        quiet_end: settings.quiet_hours.end.clone(),
        time_zone: settings
            .time_zone
            .clone()
            .unwrap_or_else(|| "UTC".to_owned()),
        snooze,
        snooze_line,
        follow_ups,
        follow_ups_line,
        channels,
        turns_today,
        turns_line,
        notified_today,
        notify_cap: cap,
        notify_line,
        brief,
        log: log_groups(cache, None, None, now_ms),
        log_load: cache.proactivity.log_load.clone(),
        log_has_more: cache.proactivity.log_has_more,
        error: cache.proactivity.error.clone(),
    }
}

fn follow_ups_line(cache: &Cache, follow_ups: &[FollowUpView]) -> String {
    let tail = "You can cancel one in the Routines tab of its channel.";
    match follow_ups.len() {
        0 => {
            "None are waiting. The assistant sets one when it says it will check again.".to_owned()
        }
        n => {
            let mut places: Vec<String> = follow_ups
                .iter()
                .map(|f| channel_name(cache, &f.channel_id))
                .collect();
            places.dedup();
            let place = match (n, places.as_slice()) {
                (1, [one]) => format!("1 is waiting, in {one}."),
                (2, [one]) => format!("2 are waiting, both in {one}."),
                (_, [one]) => format!("{n} are waiting, all in {one}."),
                _ => format!("{n} are waiting, in {} channels.", places.len()),
            };
            format!("{place} {tail}")
        }
    }
}

/// The result line of a check-in row in the Routines tab, from the log in
/// the view: "Stayed silent at 7:25." on the 24-hour clock of routines.
pub fn check_in_line(view: &ProactivityView, channel_id: &str) -> Option<String> {
    let row = view
        .log
        .iter()
        .flat_map(|g| g.rows.iter())
        .find(|r| r.kind == Some(ProactiveKind::CheckIn) && r.channel_id == channel_id)?;
    let at = crate::format::clock_24(row.at);
    Some(match row.result.as_str() {
        "Running" => "Running now.".to_owned(),
        "Notified" => format!("Notified you at {at}."),
        result => format!("{result} at {at}."),
    })
}

/// Whether a message is a proactive message, for the hover actions.
pub fn is_proactive(cache: &Cache, message_id: &str) -> bool {
    cache.messages.get(message_id).is_some_and(|m| {
        m.proactive.is_some() && matches!(m.kind, MessageKind::Text | MessageKind::Decision)
    })
}

#[cfg(test)]
mod tests;
