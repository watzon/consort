//! The hub gate: pure checks that decide whether a proactive wake may run,
//! before a model runs. No I/O and no clock: every function takes `now`.

use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Timelike};
use chrono_tz::Tz;
use consort_proto::ids::RoutineId;
use consort_proto::model::{
    ChannelKind, ProactiveKind, ProactiveOutcome, ProactivityLevel, ProactivitySettings,
    SnoozeChoice, TURN_BUDGET_CHANNEL, TURN_BUDGET_TUTTI,
};
use consort_proto::time::Timestamp;

/// The most a check-in or the research pass starts after its time.
pub(crate) const JITTER_MAX_MS: i64 = 5 * 60 * 1000;

/// The user's time zone, or UTC while the hub has none.
pub(crate) fn zone(settings: &ProactivitySettings) -> Tz {
    settings
        .time_zone
        .as_deref()
        .and_then(|name| name.trim().parse::<Tz>().ok())
        .unwrap_or(Tz::UTC)
}

/// The name of the time zone that the hub uses for the user.
pub(crate) fn zone_name(settings: &ProactivitySettings) -> String {
    zone(settings).name().to_owned()
}

fn local(now: Timestamp, tz: Tz) -> DateTime<Tz> {
    DateTime::<chrono::Utc>::from_timestamp_millis(now.as_millis())
        .unwrap_or(DateTime::<chrono::Utc>::UNIX_EPOCH)
        .with_timezone(&tz)
}

/// The local time at `date` `hour`:`minute`. A time in a daylight saving
/// gap is the first real time after the gap. A time that happens twice is
/// the first one.
fn local_at(tz: Tz, date: NaiveDate, hour: u32, minute: u32) -> Option<Timestamp> {
    let naive = date.and_hms_opt(hour, minute, 0)?;
    let mapped = tz.from_local_datetime(&naive);
    let at = match mapped.earliest() {
        Some(at) => at,
        None => {
            // A gap: try each minute after the time until one exists.
            let mut probe = naive;
            loop {
                probe += ChronoDuration::minutes(1);
                if let Some(at) = tz.from_local_datetime(&probe).earliest() {
                    break at;
                }
                if probe - naive > ChronoDuration::hours(3) {
                    return None;
                }
            }
        }
    };
    Some(Timestamp::from_millis(at.timestamp_millis()))
}

/// The local time of day as minutes after midnight.
pub(crate) fn local_minute(now: Timestamp, tz: Tz) -> u32 {
    let at = local(now, tz);
    at.hour() * 60 + at.minute()
}

/// Whether `now` is inside the quiet hours.
pub(crate) fn in_quiet_hours(settings: &ProactivitySettings, now: Timestamp) -> bool {
    settings
        .quiet_hours
        .contains_minute(local_minute(now, zone(settings)))
}

/// The next end of the quiet hours after `now`.
pub(crate) fn quiet_hours_end(settings: &ProactivitySettings, now: Timestamp) -> Option<Timestamp> {
    let (_, end) = settings.quiet_hours.minutes()?;
    let tz = zone(settings);
    let today = local(now, tz).date_naive();
    for days in 0..3 {
        let date = today.checked_add_days(chrono::Days::new(days))?;
        if let Some(at) = local_at(tz, date, end / 60, end % 60)
            && at > now
        {
            return Some(at);
        }
    }
    None
}

/// The local midnight that starts the day of `now`. The daily budget and
/// the notification limit count from here.
pub(crate) fn start_of_day(settings: &ProactivitySettings, now: Timestamp) -> Timestamp {
    let tz = zone(settings);
    let date = local(now, tz).date_naive();
    local_at(tz, date, 0, 0).unwrap_or(now)
}

/// The end of a snooze that starts at `now`. `Off` is `None`.
pub(crate) fn snooze_until(
    settings: &ProactivitySettings,
    choice: SnoozeChoice,
    now: Timestamp,
) -> Option<Timestamp> {
    const HOUR_MS: i64 = 60 * 60 * 1000;
    match choice {
        SnoozeChoice::Off => None,
        SnoozeChoice::TwoHours => Some(now.plus_millis(2 * HOUR_MS)),
        SnoozeChoice::Week => Some(now.plus_millis(7 * 24 * HOUR_MS)),
        SnoozeChoice::UntilTomorrow => {
            // Tomorrow at the end of the quiet hours, or at 07:00 when the
            // quiet hours are not valid.
            let tz = zone(settings);
            let (hour, minute) = settings
                .quiet_hours
                .minutes()
                .map_or((7, 0), |(_, end)| (end / 60, end % 60));
            let tomorrow = local(now, tz)
                .date_naive()
                .checked_add_days(chrono::Days::new(1))?;
            local_at(tz, tomorrow, hour, minute).or(Some(now.plus_millis(24 * HOUR_MS)))
        }
    }
}

/// A time as the user reads it, for example "7:00 AM", or "Mon 7:00 AM"
/// when it is not on the day of `now`.
pub(crate) fn time_label(settings: &ProactivitySettings, at: Timestamp, now: Timestamp) -> String {
    let tz = zone(settings);
    let when = local(at, tz);
    let (pm, hour) = when.hour12();
    let text = format!(
        "{hour}:{:02} {}",
        when.minute(),
        if pm { "PM" } else { "AM" }
    );
    if when.date_naive() == local(now, tz).date_naive() {
        text
    } else {
        let day = match when.weekday() {
            chrono::Weekday::Mon => "Mon",
            chrono::Weekday::Tue => "Tue",
            chrono::Weekday::Wed => "Wed",
            chrono::Weekday::Thu => "Thu",
            chrono::Weekday::Fri => "Fri",
            chrono::Weekday::Sat => "Sat",
            chrono::Weekday::Sun => "Sun",
        };
        format!("{day} {text}")
    }
}

/// A delay of 0 to 5 minutes that stays the same for one routine, so
/// routines that share a minute do not all start at once.
pub(crate) fn jitter_ms(routine: RoutineId) -> i64 {
    let digest = crate::util::sha256(routine.to_string().as_bytes()).to_string();
    let seed = u64::from_str_radix(digest.get(..12).unwrap_or("0"), 16).unwrap_or(0);
    let max = u64::try_from(JITTER_MAX_MS).unwrap_or(1);
    i64::try_from(seed % max).unwrap_or(0)
}

/// The daily budget of proactive turns in a channel of this kind.
pub(crate) fn turn_budget(kind: ChannelKind) -> u32 {
    match kind {
        ChannelKind::Tutti => TURN_BUDGET_TUTTI,
        ChannelKind::Standard => TURN_BUDGET_CHANNEL,
    }
}

/// What the gate decided about one wake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Gate {
    /// The wake runs.
    Run,
    /// The wake waits, with no log row: the level stops it, or it waits for
    /// the end of quiet hours (a stale-task nudge).
    Quiet,
    /// The hub skips the wake and writes a log row with the outcome and why.
    Skip(ProactiveOutcome, String),
}

/// What the gate knows about one wake.
#[derive(Debug, Clone)]
pub(crate) struct GateInput<'a> {
    pub kind: ProactiveKind,
    pub level: ProactivityLevel,
    pub channel_name: &'a str,
    pub channel_kind: ChannelKind,
    pub settings: &'a ProactivitySettings,
    pub now: Timestamp,
    /// Proactive turns of the channel today.
    pub turns_today: u32,
}

/// Decides whether a scheduled proactive wake runs. The order: the level,
/// Pause all, snooze, quiet hours, then the daily budget. A check-in that
/// passes still needs the no-change check, which needs the store.
pub(crate) fn decide(input: &GateInput<'_>) -> Gate {
    use ProactiveKind as K;
    let GateInput {
        kind,
        level,
        settings,
        now,
        ..
    } = *input;
    if !level.allows(kind) {
        return Gate::Quiet;
    }
    // Feedback and a new channel come from the user.
    if matches!(kind, K::Feedback) {
        return Gate::Run;
    }
    if settings.paused {
        return Gate::Skip(
            ProactiveOutcome::SkippedPaused,
            "Pause all is on.".to_owned(),
        );
    }
    if matches!(kind, K::ChannelCreated) {
        return Gate::Run;
    }
    let snoozed = settings.is_snoozed(now);
    if snoozed && matches!(kind, K::CheckIn | K::StaleTask | K::Research) {
        let until = settings
            .snoozed_until
            .map(|until| time_label(settings, until, now))
            .unwrap_or_default();
        return Gate::Skip(
            ProactiveOutcome::SkippedSnoozed,
            format!("Snoozed until {until}."),
        );
    }
    if in_quiet_hours(settings, now) {
        match kind {
            K::CheckIn => {
                let until = quiet_hours_end(settings, now)
                    .map(|end| format!("Quiet hours until {}.", time_label(settings, end, now)))
                    .unwrap_or_else(|| "Quiet hours.".to_owned());
                return Gate::Skip(ProactiveOutcome::SkippedQuietHours, until);
            }
            K::StaleTask => return Gate::Quiet,
            _ => {}
        }
    }
    let budget = turn_budget(input.channel_kind);
    if kind.budget_can_skip() && input.turns_today >= budget {
        return Gate::Skip(
            ProactiveOutcome::SkippedBudget,
            format!(
                "{budget} of {budget} proactive turns used in {}.",
                input.channel_name
            ),
        );
    }
    Gate::Run
}

/// Whether a proactive message may notify now: not in quiet hours, not
/// while snoozed or paused.
pub(crate) fn may_notify(settings: &ProactivitySettings, now: Timestamp) -> bool {
    !settings.paused && !settings.is_snoozed(now) && !in_quiet_hours(settings, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::model::QuietHours;

    fn at(rfc3339: &str) -> Timestamp {
        Timestamp::parse_rfc3339(rfc3339).unwrap()
    }

    fn settings(tz: &str, start: &str, end: &str) -> ProactivitySettings {
        ProactivitySettings {
            time_zone: Some(tz.to_owned()),
            quiet_hours: QuietHours {
                start: start.to_owned(),
                end: end.to_owned(),
            },
            ..ProactivitySettings::default()
        }
    }

    fn input<'a>(
        kind: ProactiveKind,
        level: ProactivityLevel,
        settings: &'a ProactivitySettings,
        now: Timestamp,
        turns_today: u32,
    ) -> GateInput<'a> {
        GateInput {
            kind,
            level,
            channel_name: "sayso",
            channel_kind: ChannelKind::Standard,
            settings,
            now,
            turns_today,
        }
    }

    #[test]
    fn quiet_hours_cross_midnight_in_the_users_time_zone() {
        let s = settings("America/Chicago", "21:00", "07:00");
        // 03:30 UTC on Oct 30 is 22:30 in Chicago (CDT, UTC-5).
        assert!(in_quiet_hours(&s, at("2026-10-30T03:30:00Z")));
        // 11:30 UTC is 06:30 in Chicago.
        assert!(in_quiet_hours(&s, at("2026-10-30T11:30:00Z")));
        // 12:00 UTC is 07:00: the end is not inside.
        assert!(!in_quiet_hours(&s, at("2026-10-30T12:00:00Z")));
        // 17:00 UTC is noon.
        assert!(!in_quiet_hours(&s, at("2026-10-30T17:00:00Z")));
        // Without a time zone the hub uses UTC.
        let utc = ProactivitySettings::default();
        assert!(in_quiet_hours(&utc, at("2026-10-30T22:00:00Z")));
        assert!(!in_quiet_hours(&utc, at("2026-10-30T12:00:00Z")));
    }

    #[test]
    fn quiet_hours_follow_daylight_saving_time() {
        let s = settings("America/Chicago", "21:00", "07:00");
        // Before the change on Nov 1, 2026, Chicago is UTC-5: 11:30 UTC is
        // 06:30, inside. After it, Chicago is UTC-6: 12:30 UTC is 06:30,
        // still inside, and 13:00 UTC is 07:00, outside.
        assert!(in_quiet_hours(&s, at("2026-10-31T11:30:00Z")));
        assert!(!in_quiet_hours(&s, at("2026-10-31T12:30:00Z")));
        assert!(in_quiet_hours(&s, at("2026-11-02T12:30:00Z")));
        assert!(!in_quiet_hours(&s, at("2026-11-02T13:00:00Z")));
        // The end of quiet hours on the night of the change is 07:00 local,
        // 13:00 UTC.
        assert_eq!(
            quiet_hours_end(&s, at("2026-11-01T05:00:00Z")),
            Some(at("2026-11-01T13:00:00Z"))
        );
        // A quiet hour range that starts in the spring gap (02:30 does not
        // exist on Mar 8, 2026) still works.
        let gap = settings("America/Chicago", "01:00", "02:30");
        assert_eq!(
            quiet_hours_end(&gap, at("2026-03-08T07:30:00Z")),
            Some(at("2026-03-08T08:00:00Z")),
            "the end in the gap is the first real minute after it (03:00 CDT)"
        );
    }

    #[test]
    fn snooze_ends_and_the_start_of_the_day() {
        let s = settings("America/Chicago", "21:00", "07:00");
        let now = at("2026-10-30T16:50:00Z"); // 11:50 in Chicago
        assert_eq!(
            snooze_until(&s, SnoozeChoice::TwoHours, now),
            Some(at("2026-10-30T18:50:00Z"))
        );
        assert_eq!(
            snooze_until(&s, SnoozeChoice::UntilTomorrow, now),
            Some(at("2026-10-31T12:00:00Z"))
        );
        assert_eq!(snooze_until(&s, SnoozeChoice::Off, now), None);
        assert_eq!(start_of_day(&s, now), at("2026-10-30T05:00:00Z"));
        assert_eq!(time_label(&s, at("2026-10-30T18:45:00Z"), now), "1:45 PM");
        assert_eq!(
            time_label(&s, at("2026-10-31T12:00:00Z"), now),
            "Sat 7:00 AM"
        );
    }

    #[test]
    fn the_gate_checks_level_pause_snooze_quiet_hours_and_budget_in_order() {
        use ProactiveKind as K;
        use ProactivityLevel as L;
        let mut s = settings("UTC", "21:00", "07:00");
        let noon = at("2026-10-30T12:00:00Z");
        let night = at("2026-10-30T23:00:00Z");
        assert_eq!(
            decide(&input(K::CheckIn, L::Quiet, &s, noon, 0)),
            Gate::Quiet
        );
        assert_eq!(
            decide(&input(K::FollowUp, L::Off, &s, noon, 0)),
            Gate::Quiet
        );
        assert_eq!(
            decide(&input(K::CheckIn, L::Normal, &s, noon, 0)),
            Gate::Run
        );
        // Quiet hours skip a check-in, delay a nudge, and let a follow-up,
        // the brief and the research pass run.
        assert!(matches!(
            decide(&input(K::CheckIn, L::Normal, &s, night, 0)),
            Gate::Skip(ProactiveOutcome::SkippedQuietHours, text) if text == "Quiet hours until Sat 7:00 AM."
        ));
        assert_eq!(
            decide(&input(K::StaleTask, L::Quiet, &s, night, 0)),
            Gate::Quiet
        );
        assert_eq!(
            decide(&input(K::FollowUp, L::Quiet, &s, night, 0)),
            Gate::Run
        );
        assert_eq!(decide(&input(K::Brief, L::Normal, &s, night, 0)), Gate::Run);
        assert_eq!(
            decide(&input(K::Research, L::Eager, &s, night, 0)),
            Gate::Run
        );
        // The budget skips check-ins, nudges and research, not follow-ups.
        assert!(matches!(
            decide(&input(K::CheckIn, L::Normal, &s, noon, 3)),
            Gate::Skip(ProactiveOutcome::SkippedBudget, text) if text == "3 of 3 proactive turns used in sayso."
        ));
        assert_eq!(
            decide(&input(K::FollowUp, L::Quiet, &s, noon, 9)),
            Gate::Run
        );
        assert_eq!(decide(&input(K::Brief, L::Normal, &s, noon, 9)), Gate::Run);
        // Snooze skips check-ins, nudges and research; follow-ups and the
        // brief run.
        s.snoozed_until = Some(at("2026-10-30T13:45:00Z"));
        assert!(matches!(
            decide(&input(K::StaleTask, L::Quiet, &s, noon, 0)),
            Gate::Skip(ProactiveOutcome::SkippedSnoozed, text) if text == "Snoozed until 1:45 PM."
        ));
        assert_eq!(
            decide(&input(K::FollowUp, L::Quiet, &s, noon, 0)),
            Gate::Run
        );
        assert_eq!(decide(&input(K::Brief, L::Normal, &s, noon, 0)), Gate::Run);
        assert!(!may_notify(&s, noon));
        // Pause all stops everything that starts on its own, also follow-ups
        // and the brief. Feedback still runs.
        s.paused = true;
        for kind in [K::FollowUp, K::Brief, K::CheckIn, K::Research] {
            assert!(matches!(
                decide(&input(kind, L::Eager, &s, noon, 0)),
                Gate::Skip(ProactiveOutcome::SkippedPaused, _)
            ));
        }
        assert_eq!(decide(&input(K::Feedback, L::Off, &s, noon, 0)), Gate::Run);
    }

    #[test]
    fn notifications_wait_for_quiet_hours_and_snooze() {
        let mut s = settings("UTC", "21:00", "07:00");
        assert!(may_notify(&s, at("2026-10-30T12:00:00Z")));
        assert!(!may_notify(&s, at("2026-10-30T22:00:00Z")));
        s.snoozed_until = Some(at("2026-10-30T13:00:00Z"));
        assert!(!may_notify(&s, at("2026-10-30T12:00:00Z")));
        assert!(may_notify(&s, at("2026-10-30T13:00:00Z")));
    }

    #[test]
    fn jitter_is_stable_and_at_most_five_minutes() {
        let id = RoutineId::new();
        assert_eq!(jitter_ms(id), jitter_ms(id));
        for _ in 0..50 {
            let j = jitter_ms(RoutineId::new());
            assert!((0..JITTER_MAX_MS).contains(&j));
        }
    }
}
