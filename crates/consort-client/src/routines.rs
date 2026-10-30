//! Routines in the client: the views of routines and their runs, the form of
//! the routine dialog with its schedule picker, and the words that the rows
//! show.
//!
//! The schedule picker has six presets: Every day, Weekdays, Weekly,
//! Interval, Once and Cron. Every day, Weekdays and Weekly make a cron
//! expression, and the picker shows it, so an expert can learn it. Interval
//! runs count from the time when the user saves the routine, and have no
//! time zone. Times show on a 24-hour clock, as in the designs.

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use consort_proto::model::ROUTINE_MIN_INTERVAL_SECONDS;
pub use consort_proto::model::{RoutineRunStatus, RoutineSchedule};
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};

use crate::view::SelectionView;

/// A routine, as the Routines tab and Settings > Routines show it.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutineView {
    pub id: String,
    pub channel_id: String,
    pub name: String,
    pub enabled: bool,
    pub schedule: RoutineSchedule,
    /// The prompt of "Ask the assistant", or the brief of "Start a task".
    pub prompt: String,
    pub action: RoutineActionView,
    pub next_run_at: Option<i64>,
    pub last_run_at: Option<i64>,
    pub last_run_status: Option<RoutineRunStatus>,
    /// The newest runs first. Empty until they load
    /// ([`crate::Command::LoadRoutineRuns`]).
    pub runs: Vec<RoutineRunView>,
    pub runs_loaded: bool,
    /// What the routine is for: a check-in or a follow-up shows apart.
    pub kind: consort_proto::model::RoutineKind,
    pub origin: consort_proto::model::RoutineOrigin,
    /// When it was made, in Unix milliseconds.
    pub created_at: Option<i64>,
}

/// What a routine does when it runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum RoutineActionView {
    /// Ask the assistant: the channel's coordinator gets the prompt, with
    /// the channel's model.
    #[default]
    Ask,
    /// Start a task with its own title, model and effort.
    Task {
        title: String,
        /// `None` uses the channel's task model.
        selection: Option<SelectionView>,
    },
}

/// One run of a routine.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutineRunView {
    pub id: u64,
    pub routine_id: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub status: RoutineRunStatus,
    /// The user started it with Run now.
    pub manual: bool,
    pub message_id: Option<String>,
    pub task_id: Option<String>,
    pub error: Option<String>,
}

// The schedule picker -------------------------------------------------------

/// The presets of the schedule picker, in the order the picker shows them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Preset {
    #[default]
    EveryDay,
    Weekdays,
    Weekly,
    Interval,
    Once,
    Cron,
}

impl Preset {
    pub const ALL: [Preset; 6] = [
        Preset::EveryDay,
        Preset::Weekdays,
        Preset::Weekly,
        Preset::Interval,
        Preset::Once,
        Preset::Cron,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Preset::EveryDay => "Every day",
            Preset::Weekdays => "Weekdays",
            Preset::Weekly => "Weekly",
            Preset::Interval => "Interval",
            Preset::Once => "Once",
            Preset::Cron => "Cron",
        }
    }

    /// Whether the preset has a time of day and a time zone.
    pub fn has_time(self) -> bool {
        matches!(
            self,
            Preset::EveryDay | Preset::Weekdays | Preset::Weekly | Preset::Once
        )
    }
}

/// The unit of an interval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IntervalUnit {
    #[default]
    Minutes,
    Hours,
}

impl IntervalUnit {
    pub const ALL: [IntervalUnit; 2] = [IntervalUnit::Minutes, IntervalUnit::Hours];

    pub fn label(self) -> &'static str {
        match self {
            IntervalUnit::Minutes => "minutes",
            IntervalUnit::Hours => "hours",
        }
    }

    fn seconds(self) -> u64 {
        match self {
            IntervalUnit::Minutes => 60,
            IntervalUnit::Hours => 3_600,
        }
    }
}

/// The day names of the Weekly preset, Monday first.
pub const DAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const DAY_PLURALS: [&str; 7] = [
    "Mondays",
    "Tuesdays",
    "Wednesdays",
    "Thursdays",
    "Fridays",
    "Saturdays",
    "Sundays",
];
const DAY_FULL: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// The field of the routine dialog that has an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FormField {
    Name,
    Channel,
    Prompt,
    Title,
    Time,
    Days,
    Every,
    Date,
    Cron,
    TimeZone,
}

/// An error under one field. The text says what to change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub field: FormField,
    pub message: String,
}

impl FieldError {
    fn new(field: FormField, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

/// The schedule field of the routine dialog, as the user edits it. Each
/// preset keeps its own inputs, so a switch between presets loses nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct ScheduleForm {
    pub preset: Preset,
    /// The time of day, such as "8:00" or "17:30".
    pub time: String,
    /// The days of the Weekly preset, Monday first.
    pub days: [bool; 7],
    /// The number of the Interval preset.
    pub every: String,
    pub unit: IntervalUnit,
    /// The date of the Once preset, such as "Tue, Nov 3, 2026" or
    /// "2026-11-03".
    pub date: String,
    /// The expression of the Cron preset.
    pub cron: String,
    /// An IANA time zone name, such as "Europe/Berlin".
    pub time_zone: String,
}

impl ScheduleForm {
    /// A new schedule: every day at 9:00 in `time_zone`. The Once date is
    /// tomorrow.
    pub fn new(time_zone: &str, now_ms: i64) -> Self {
        let zone = parse_zone(time_zone).unwrap_or(Tz::UTC);
        let tomorrow = in_zone(now_ms, zone)
            .map(|now| now.date_naive() + Duration::days(1))
            .unwrap_or_default();
        Self {
            preset: Preset::EveryDay,
            time: "9:00".to_owned(),
            days: [true, false, false, false, false, false, false],
            every: "30".to_owned(),
            unit: IntervalUnit::Minutes,
            date: date_text(tomorrow),
            cron: "0 9 * * *".to_owned(),
            time_zone: time_zone.to_owned(),
        }
    }

    /// The form for a stored schedule. A cron expression that a preset makes
    /// opens in that preset.
    pub fn from_schedule(schedule: &RoutineSchedule, local_zone: &str, now_ms: i64) -> Self {
        let mut form = Self::new(local_zone, now_ms);
        match schedule {
            RoutineSchedule::Cron {
                expression,
                time_zone,
            } => {
                form.time_zone = time_zone.clone();
                form.cron = expression.clone();
                form.preset = Preset::Cron;
                if let Some((preset, time, days)) = preset_of_cron(expression) {
                    form.preset = preset;
                    form.time = time;
                    if let Some(days) = days {
                        form.days = days;
                    }
                }
            }
            RoutineSchedule::Interval { seconds } => {
                form.preset = Preset::Interval;
                if seconds.is_multiple_of(3_600) {
                    form.unit = IntervalUnit::Hours;
                    form.every = (seconds / 3_600).to_string();
                } else {
                    form.unit = IntervalUnit::Minutes;
                    form.every = (seconds / 60).max(1).to_string();
                }
            }
            RoutineSchedule::Once { at } => {
                form.preset = Preset::Once;
                if let Some(local) =
                    in_zone(at.as_millis(), parse_zone(local_zone).unwrap_or(Tz::UTC))
                {
                    form.date = date_text(local.date_naive());
                    form.time = clock(local.hour(), local.minute());
                }
            }
        }
        form
    }

    /// Picks a preset. The cron of the Cron preset starts as the cron of the
    /// preset that was on, so an expert can change it.
    pub fn set_preset(&mut self, preset: Preset) {
        if preset == Preset::Cron
            && self.preset != Preset::Cron
            && let Some(cron) = self.cron_expression()
        {
            self.cron = cron;
        }
        self.preset = preset;
    }

    /// The cron expression that the form makes, for the presets that make
    /// one. For Cron, the text as the user wrote it.
    pub fn cron_expression(&self) -> Option<String> {
        let (hour, minute) = parse_time(&self.time)?;
        match self.preset {
            Preset::EveryDay => Some(format!("{minute} {hour} * * *")),
            Preset::Weekdays => Some(format!("{minute} {hour} * * 1-5")),
            Preset::Weekly => {
                let days: Vec<String> = (0..7)
                    .filter(|d| self.days[*d])
                    .map(|d| ((d + 1) % 7).to_string())
                    .collect();
                if days.is_empty() {
                    return None;
                }
                Some(format!("{minute} {hour} * * {}", days.join(",")))
            }
            Preset::Cron => Some(normalize_cron(&self.cron)),
            Preset::Interval | Preset::Once => None,
        }
    }

    /// The errors of the form, one per field, in the order of the fields.
    pub fn errors(&self, now_ms: i64) -> Vec<FieldError> {
        let mut out = Vec::new();
        let zone = if self.preset.has_time() || self.preset == Preset::Cron {
            match parse_zone(&self.time_zone) {
                Some(zone) => Some(zone),
                None => {
                    out.push(FieldError::new(
                        FormField::TimeZone,
                        "This time zone is not known. Choose one from the list, such as Europe/Berlin.",
                    ));
                    None
                }
            }
        } else {
            None
        };
        match self.preset {
            Preset::EveryDay | Preset::Weekdays | Preset::Weekly => {
                if parse_time(&self.time).is_none() {
                    out.push(time_error());
                }
                if self.preset == Preset::Weekly && !self.days.iter().any(|d| *d) {
                    out.push(FieldError::new(FormField::Days, "Choose at least one day."));
                }
            }
            Preset::Interval => {
                if let Err(message) = self.interval_seconds() {
                    out.push(FieldError::new(FormField::Every, message));
                }
            }
            Preset::Once => {
                let date = parse_date(&self.date);
                if date.is_none() {
                    out.push(FieldError::new(
                        FormField::Date,
                        "Enter a date such as 2026-11-03 or Nov 3, 2026.",
                    ));
                }
                let time = parse_time(&self.time);
                if time.is_none() {
                    out.push(time_error());
                }
                if let (Some(date), Some((hour, minute)), Some(zone)) = (date, time, zone)
                    && let Some(at) = local_millis(date, hour, minute, zone)
                    && at <= now_ms
                {
                    out.push(FieldError::new(
                        FormField::Date,
                        "This time is in the past. Choose a later date or time.",
                    ));
                }
            }
            Preset::Cron => {
                if let Err(message) = check_cron(&self.cron) {
                    out.push(FieldError::new(FormField::Cron, message));
                }
            }
        }
        out
    }

    /// The schedule for the hub, or the errors.
    pub fn schedule(&self, now_ms: i64) -> Result<RoutineSchedule, Vec<FieldError>> {
        let errors = self.errors(now_ms);
        if !errors.is_empty() {
            return Err(errors);
        }
        match self.preset {
            Preset::Interval => {
                let seconds = self
                    .interval_seconds()
                    .map_err(|m| vec![FieldError::new(FormField::Every, m)])?;
                Ok(RoutineSchedule::Interval { seconds })
            }
            Preset::Once => {
                let at = self
                    .once_at()
                    .ok_or_else(|| vec![FieldError::new(FormField::Date, "Enter a date.")])?;
                Ok(RoutineSchedule::Once {
                    at: consort_proto::time::Timestamp::from_millis(at),
                })
            }
            _ => {
                let expression = self.cron_expression().ok_or_else(|| vec![time_error()])?;
                Ok(RoutineSchedule::Cron {
                    expression,
                    time_zone: self.time_zone.trim().to_owned(),
                })
            }
        }
    }

    fn interval_seconds(&self) -> Result<u64, String> {
        let unit = self.unit.label();
        let number: u64 = self
            .every
            .trim()
            .parse()
            .map_err(|_| format!("Enter a whole number of {unit}, 1 or more."))?;
        let seconds = number
            .checked_mul(self.unit.seconds())
            .filter(|s| *s <= 366 * 24 * 3_600)
            .ok_or_else(|| "Use an interval of one year or less.".to_owned())?;
        if seconds < ROUTINE_MIN_INTERVAL_SECONDS {
            return Err(format!("Enter a whole number of {unit}, 1 or more."));
        }
        Ok(seconds)
    }

    /// The time of the Once preset, in Unix milliseconds.
    pub fn once_at(&self) -> Option<i64> {
        let date = parse_date(&self.date)?;
        let (hour, minute) = parse_time(&self.time)?;
        local_millis(date, hour, minute, parse_zone(&self.time_zone)?)
    }

    /// The schedule in words, such as "Weekdays at 8:00" or "Every 30
    /// minutes". Empty when the form has an error.
    pub fn words(&self) -> String {
        match self.preset {
            Preset::Interval => match self.interval_seconds() {
                Ok(seconds) => interval_words(seconds),
                Err(_) => String::new(),
            },
            Preset::Once => match (parse_date(&self.date), parse_time(&self.time)) {
                (Some(date), Some((h, m))) => {
                    format!("Once on {} at {}", date.format("%b %-d"), clock(h, m))
                }
                _ => String::new(),
            },
            _ => self
                .cron_expression()
                .and_then(|cron| cron_words(&cron))
                .unwrap_or_default(),
        }
    }

    /// The next `count` runs, in Unix milliseconds. Empty when the form has
    /// an error. An interval counts from `now`, as if the user saved now.
    pub fn next_runs(&self, now_ms: i64, count: usize) -> Vec<i64> {
        if !self.errors(now_ms).is_empty() {
            return Vec::new();
        }
        match self.preset {
            Preset::Interval => {
                let Ok(seconds) = self.interval_seconds() else {
                    return Vec::new();
                };
                let step = i64::try_from(seconds).unwrap_or(i64::MAX / 2_000) * 1_000;
                (1..=count as i64).map(|k| now_ms + k * step).collect()
            }
            Preset::Once => self.once_at().into_iter().collect(),
            _ => {
                let (Some(cron), Some(zone)) =
                    (self.cron_expression(), parse_zone(&self.time_zone))
                else {
                    return Vec::new();
                };
                cron_runs(&cron, zone, now_ms, count)
            }
        }
    }

    /// The preview under the schedule: the next runs and a line that
    /// explains the schedule.
    pub fn preview(&self, now_ms: i64) -> SchedulePreview {
        let errors = self.errors(now_ms);
        let zone = parse_zone(&self.time_zone).unwrap_or(Tz::UTC);
        if !errors.is_empty() {
            let (headline, detail) = match self.preset {
                Preset::Cron => (
                    "Fix the cron to see the next runs.".to_owned(),
                    "Example: 0 8 * * 1 runs on Mondays at 8:00.".to_owned(),
                ),
                Preset::Once => (
                    "Choose a later time to see the run.".to_owned(),
                    "Once routines need a time in the future.".to_owned(),
                ),
                _ => (
                    "Fix the schedule to see the next runs.".to_owned(),
                    String::new(),
                ),
            };
            return SchedulePreview {
                headline,
                detail,
                cron: None,
                valid: false,
            };
        }
        let runs = self.next_runs(now_ms, if self.preset == Preset::Once { 1 } else { 5 });
        // An interval has no time zone: its runs show in this computer's.
        let shown_zone = if self.preset == Preset::Interval {
            local_zone()
        } else {
            zone
        };
        let labels: Vec<String> = runs
            .iter()
            .map(|at| run_label(*at, now_ms, shown_zone))
            .collect();
        let words = self.words();
        let (headline, detail, cron) = match self.preset {
            Preset::Once => (
                format!(
                    "Runs once: {}",
                    self.once_at()
                        .and_then(|at| in_zone(at, zone))
                        .map(|t| format!(
                            "{} at {}",
                            t.format("%a, %b %-d"),
                            clock(t.hour(), t.minute())
                        ))
                        .unwrap_or_default()
                ),
                "After the run, the routine turns off. It stays in the list.".to_owned(),
                None,
            ),
            Preset::Interval => (
                format!("Next runs: {}", labels.join(", ")),
                format!("{words}, counted from when you save it."),
                None,
            ),
            _ => {
                let cron = self.cron_expression();
                let detail = if words.is_empty() {
                    "As cron:".to_owned()
                } else {
                    format!("{words}. As cron:")
                };
                (format!("Next runs: {}", labels.join(", ")), detail, cron)
            }
        };
        SchedulePreview {
            headline,
            detail,
            cron,
            valid: true,
        }
    }
}

/// What the preview box under the schedule says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulePreview {
    /// "Next runs: Fri 8:00, Mon 8:00" or what to fix.
    pub headline: String,
    /// The schedule in words, or an example.
    pub detail: String,
    /// The cron expression to show after `detail`, in mono.
    pub cron: Option<String>,
    /// False when the form has an error.
    pub valid: bool,
}

fn time_error() -> FieldError {
    FieldError::new(FormField::Time, "Enter a time such as 8:00 or 17:30.")
}

/// Reads "8:00", "08:00" or "17:30".
pub fn parse_time(text: &str) -> Option<(u32, u32)> {
    let (h, m) = text.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let hour: u32 = h.parse().ok()?;
    let minute: u32 = m.parse().ok()?;
    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// Reads "2026-11-03", "Nov 3, 2026" or "Tue, Nov 3, 2026".
pub fn parse_date(text: &str) -> Option<NaiveDate> {
    let text = text.trim();
    for format in [
        "%Y-%m-%d",
        "%a, %b %d, %Y",
        "%b %d, %Y",
        "%a %b %d %Y",
        "%b %d %Y",
    ] {
        if let Ok(date) = NaiveDate::parse_from_str(text, format) {
            return Some(date);
        }
    }
    None
}

/// The local date of a time, as the date field shows it.
pub fn date_label_of(ms: i64) -> String {
    in_zone(ms, local_zone())
        .map(|t| date_text(t.date_naive()))
        .unwrap_or_default()
}

/// "Tue, Nov 3, 2026", as the date field shows a date.
pub fn date_text(date: NaiveDate) -> String {
    date.format("%a, %b %-d, %Y").to_string()
}

/// "8:00" or "17:30".
fn clock(hour: u32, minute: u32) -> String {
    format!("{hour}:{minute:02}")
}

fn parse_zone(name: &str) -> Option<Tz> {
    name.trim().parse::<Tz>().ok()
}

fn in_zone(ms: i64, zone: Tz) -> Option<DateTime<Tz>> {
    DateTime::<Utc>::from_timestamp_millis(ms).map(|t| t.with_timezone(&zone))
}

/// The time of a local date and time in a zone. A time in a daylight saving
/// gap moves to the first real time after it.
fn local_millis(date: NaiveDate, hour: u32, minute: u32, zone: Tz) -> Option<i64> {
    let naive = date.and_time(NaiveTime::from_hms_opt(hour, minute, 0)?);
    for shift in 0..=120 {
        let candidate = naive + Duration::minutes(shift);
        if let Some(t) = zone.from_local_datetime(&candidate).earliest() {
            return Some(t.timestamp_millis());
        }
    }
    None
}

/// The time zone of this computer, as an IANA name. UTC when the system
/// does not say.
pub fn local_time_zone() -> String {
    iana_time_zone::get_timezone()
        .ok()
        .filter(|name| parse_zone(name).is_some())
        .unwrap_or_else(|| "UTC".to_owned())
}

/// The zone in which a routine's times show: the time zone of a cron
/// schedule, as its words do, else this computer's.
pub fn display_zone(schedule: &RoutineSchedule) -> Tz {
    match schedule {
        RoutineSchedule::Cron { time_zone, .. } => parse_zone(time_zone).unwrap_or_else(local_zone),
        _ => local_zone(),
    }
}

fn local_zone() -> Tz {
    parse_zone(&local_time_zone()).unwrap_or(Tz::UTC)
}

/// Every IANA time zone name that the picker offers, sorted.
pub fn time_zones() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = chrono_tz::TZ_VARIANTS.iter().map(|z| z.name()).collect();
    names.sort_unstable();
    names
}

/// Collapses the spaces of a cron expression.
fn normalize_cron(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Checks a cron expression as the hub does: five fields, no seconds, no
/// year.
pub fn check_cron(text: &str) -> Result<Cron, String> {
    let fields = text.split_whitespace().count();
    if fields == 0 {
        return Err(
            "Enter a cron with 5 fields: minute, hour, day of month, month and day of week."
                .to_owned(),
        );
    }
    if fields != 5 {
        let verb = if fields > 5 { "Remove" } else { "Add" };
        let count = if fields.abs_diff(5) == 1 {
            "one"
        } else {
            "some"
        };
        let change = format!("{verb} {count}");
        let plural = if fields == 1 { "field" } else { "fields" };
        return Err(format!(
            "This cron has {fields} {plural}. {change} so it has 5: minute, hour, day of month, month and day of week."
        ));
    }
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(&normalize_cron(text))
        .map_err(|err| format!("This cron is not valid: {err}. Check each field, then try again."))
}

fn cron_runs(expression: &str, zone: Tz, now_ms: i64, count: usize) -> Vec<i64> {
    let Ok(cron) = check_cron(expression) else {
        return Vec::new();
    };
    let Some(mut after) = in_zone(now_ms, zone) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while out.len() < count {
        match cron.find_next_occurrence(&after, false) {
            Ok(next) => {
                out.push(next.timestamp_millis());
                after = next;
            }
            Err(_) => break,
        }
    }
    out
}

/// The preset, the time and the days of a cron expression that a preset
/// makes. `None` for any other expression.
fn preset_of_cron(expression: &str) -> Option<(Preset, String, Option<[bool; 7]>)> {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    let [minute, hour, "*", "*", dow] = fields.as_slice() else {
        return None;
    };
    let minute: u32 = minute.parse().ok().filter(|m| *m < 60)?;
    let hour: u32 = hour.parse().ok().filter(|h| *h < 24)?;
    let time = clock(hour, minute);
    match *dow {
        "*" => Some((Preset::EveryDay, time, None)),
        "1-5" => Some((Preset::Weekdays, time, None)),
        list => {
            let mut days = [false; 7];
            for part in list.split(',') {
                let number: usize = part.parse().ok().filter(|n| *n <= 7)?;
                // Cron counts Sunday as 0 or 7. The picker starts on Monday.
                days[(number + 6) % 7] = true;
            }
            Some((Preset::Weekly, time, Some(days)))
        }
    }
}

/// A cron expression in words, for the expressions that read well:
/// "Every day at 17:30", "Weekdays at 8:00", "Mondays and Thursdays at
/// 8:00", "Every 2 hours, Monday to Friday". `None` for others.
pub fn cron_words(expression: &str) -> Option<String> {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    let [minute, hour, dom, month, dow] = fields.as_slice() else {
        return None;
    };
    if *dom != "*" || *month != "*" {
        return None;
    }
    let days = days_words(dow)?;
    let at_time = match (minute.parse::<u32>(), hour.parse::<u32>()) {
        (Ok(m), Ok(h)) if m < 60 && h < 24 => Some(clock(h, m)),
        _ => None,
    };
    if let Some(time) = at_time {
        return Some(match days {
            DaysWords::Every => format!("Every day at {time}"),
            DaysWords::Weekdays => format!("Weekdays at {time}"),
            DaysWords::Plural(names) => format!("{names} at {time}"),
            DaysWords::Range(range) => format!("Every day at {time}, {range}"),
        });
    }
    let rhythm = if *minute == "*" && *hour == "*" {
        "Every minute".to_owned()
    } else if let Some(step) = minute.strip_prefix("*/")
        && *hour == "*"
    {
        format!("Every {step} minutes")
    } else if let Some(step) = hour.strip_prefix("*/")
        && minute.parse::<u32>().is_ok_and(|m| m == 0)
    {
        if step == "1" {
            "Every hour".to_owned()
        } else {
            format!("Every {step} hours")
        }
    } else if *hour == "*"
        && let Ok(m) = minute.parse::<u32>()
    {
        format!("Every hour at :{m:02}")
    } else {
        return None;
    };
    Some(match days {
        DaysWords::Every => rhythm,
        DaysWords::Weekdays => format!("{rhythm}, Monday to Friday"),
        DaysWords::Plural(names) => format!("{rhythm}, on {names}"),
        DaysWords::Range(range) => format!("{rhythm}, {range}"),
    })
}

enum DaysWords {
    Every,
    Weekdays,
    Plural(String),
    Range(String),
}

fn days_words(dow: &str) -> Option<DaysWords> {
    if dow == "*" {
        return Some(DaysWords::Every);
    }
    if dow == "1-5" {
        return Some(DaysWords::Weekdays);
    }
    let index = |part: &str| -> Option<usize> {
        let n: usize = part.parse().ok().filter(|n| *n <= 7)?;
        Some((n + 6) % 7)
    };
    if let Some((from, to)) = dow.split_once('-') {
        let (from, to) = (index(from)?, index(to)?);
        return Some(DaysWords::Range(format!(
            "{} to {}",
            DAY_FULL[from], DAY_FULL[to]
        )));
    }
    let mut days: Vec<usize> = dow.split(',').map(index).collect::<Option<_>>()?;
    days.sort_unstable();
    days.dedup();
    let names: Vec<&str> = days.iter().map(|d| DAY_PLURALS[*d]).collect();
    Some(DaysWords::Plural(join_and(&names)))
}

/// "A", "A and B", "A, B and C".
fn join_and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn interval_words(seconds: u64) -> String {
    if seconds.is_multiple_of(3_600) {
        match seconds / 3_600 {
            1 => "Every hour".to_owned(),
            n => format!("Every {n} hours"),
        }
    } else {
        match seconds / 60 {
            1 => "Every minute".to_owned(),
            n => format!("Every {n} minutes"),
        }
    }
}

/// A run in the preview list: "Today 17:30", "Fri 8:00", "Nov 12 8:00".
fn run_label(at: i64, now_ms: i64, zone: Tz) -> String {
    let (Some(t), Some(now)) = (in_zone(at, zone), in_zone(now_ms, zone)) else {
        return String::new();
    };
    let days = t
        .date_naive()
        .signed_duration_since(now.date_naive())
        .num_days();
    let day = match days {
        0 => "Today".to_owned(),
        // A week ahead still reads well as a day name, as the designs show.
        1..=7 => t.format("%a").to_string(),
        _ => t.format("%b %-d").to_string(),
    };
    format!("{day} {}", clock(t.hour(), t.minute()))
}

// The calendar of the date field -------------------------------------------

/// One day of the calendar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarDay {
    /// The day of the month.
    pub day: u32,
    /// `2026-11-03`, a stable key.
    pub key: String,
    /// The date as the date field shows it.
    pub text: String,
    /// The day is in the month that the calendar shows.
    pub in_month: bool,
    /// The day is before today: a Once routine cannot run then.
    pub past: bool,
    /// The day is the date in the field.
    pub chosen: bool,
}

/// One month of the calendar: six weeks, Monday first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarMonth {
    /// "November 2026".
    pub title: String,
    pub prev: (i32, u32),
    pub next: (i32, u32),
    pub weeks: Vec<Vec<CalendarDay>>,
}

/// The month to open the calendar on: the month of the date in the field,
/// else this month.
pub fn calendar_start(date: &str, now_ms: i64) -> (i32, u32) {
    use chrono::Datelike;
    let day = parse_date(date)
        .or_else(|| in_zone(now_ms, local_zone()).map(|t| t.date_naive()))
        .unwrap_or_default();
    (day.year(), day.month())
}

/// The calendar of one month. `None` for a month that does not exist.
pub fn calendar_month(year: i32, month: u32, date: &str, now_ms: i64) -> Option<CalendarMonth> {
    use chrono::Datelike;
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let today = in_zone(now_ms, local_zone())?.date_naive();
    let chosen = parse_date(date);
    let prev = if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    };
    let next = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let start = first - Duration::days(i64::from(first.weekday().num_days_from_monday()));
    let weeks = (0..6)
        .map(|week| {
            (0..7)
                .map(|col| {
                    let day = start + Duration::days(week * 7 + col);
                    CalendarDay {
                        day: day.day(),
                        key: day.format("%Y-%m-%d").to_string(),
                        text: date_text(day),
                        in_month: day.month() == month,
                        past: day < today,
                        chosen: chosen == Some(day),
                    }
                })
                .collect()
        })
        .collect();
    Some(CalendarMonth {
        title: first.format("%B %Y").to_string(),
        prev,
        next,
        weeks,
    })
}

// Words for the rows ---------------------------------------------------------

/// The schedule of a stored routine in words. Times of a cron schedule are
/// in its own time zone. A Once time shows in this computer's time zone.
pub fn schedule_words(schedule: &RoutineSchedule) -> String {
    match schedule {
        RoutineSchedule::Cron { expression, .. } => {
            cron_words(expression).unwrap_or_else(|| format!("Cron {expression}"))
        }
        RoutineSchedule::Interval { seconds } => interval_words(*seconds),
        RoutineSchedule::Once { at } => in_zone(at.as_millis(), local_zone())
            .map(|t| {
                format!(
                    "Once on {} at {}",
                    t.format("%b %-d"),
                    clock(t.hour(), t.minute())
                )
            })
            .unwrap_or_default(),
    }
}

/// The second line of a routine row: "Every day at 17:30, asks the
/// assistant".
pub fn summary_line(routine: &RoutineView) -> String {
    let action = match routine.action {
        RoutineActionView::Ask => "asks the assistant",
        RoutineActionView::Task { .. } => "starts a task",
    };
    format!("{}, {action}", schedule_words(&routine.schedule))
}

/// The tone of the dot in front of the third line of a routine row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    /// A run worked.
    Ok,
    /// A run failed.
    Failed,
    /// A run was skipped, or the routine has not run yet.
    Quiet,
    /// A run runs now.
    Running,
    /// The routine is off.
    Off,
}

/// The third line of a routine row, in two parts: the last result (in the
/// tone's color) and the next run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusLine {
    pub tone: StatusTone,
    pub result: String,
    pub next: String,
}

/// The off text of a routine row.
pub const OFF_TEXT: &str = "Off. It does not run until you turn it on.";

/// The third line of a routine row.
pub fn status_line(routine: &RoutineView, now_ms: i64) -> StatusLine {
    if !routine.enabled {
        return StatusLine {
            tone: StatusTone::Off,
            result: OFF_TEXT.to_owned(),
            next: String::new(),
        };
    }
    let (tone, result) = match (routine.last_run_status, routine.last_run_at) {
        (Some(RoutineRunStatus::Running), _) => (StatusTone::Running, "Running now.".to_owned()),
        (Some(status), Some(at)) => {
            let when = past_day(at, now_ms, display_zone(&routine.schedule));
            match status {
                RoutineRunStatus::Ok => (StatusTone::Ok, format!("Succeeded {when}.")),
                RoutineRunStatus::Error => (StatusTone::Failed, format!("Failed {when}.")),
                RoutineRunStatus::Skipped => (StatusTone::Quiet, format!("Skipped {when}.")),
                RoutineRunStatus::Running => (StatusTone::Running, "Running now.".to_owned()),
            }
        }
        _ => (StatusTone::Quiet, "Has not run yet.".to_owned()),
    };
    let next = routine
        .next_run_at
        .map(|at| {
            format!(
                "Next run {}.",
                next_time(at, now_ms, display_zone(&routine.schedule))
            )
        })
        .unwrap_or_default();
    StatusLine { tone, result, next }
}

/// "today", "yesterday", "on Monday", "on Oct 12".
fn past_day(ms: i64, now_ms: i64, zone: Tz) -> String {
    let (Some(t), Some(now)) = (in_zone(ms, zone), in_zone(now_ms, zone)) else {
        return String::new();
    };
    match now
        .date_naive()
        .signed_duration_since(t.date_naive())
        .num_days()
    {
        i64::MIN..=0 => "today".to_owned(),
        1 => "yesterday".to_owned(),
        2..=6 => format!("on {}", t.format("%A")),
        _ => format!("on {}", t.format("%b %-d")),
    }
}

/// "today at 17:30", "tomorrow at 2:00", "Mon 8:00", "Nov 12 at 9:00".
fn next_time(ms: i64, now_ms: i64, zone: Tz) -> String {
    let (Some(t), Some(now)) = (in_zone(ms, zone), in_zone(now_ms, zone)) else {
        return String::new();
    };
    let time = clock(t.hour(), t.minute());
    match t
        .date_naive()
        .signed_duration_since(now.date_naive())
        .num_days()
    {
        i64::MIN..=0 => format!("today at {time}"),
        1 => format!("tomorrow at {time}"),
        2..=6 => format!("{} {time}", t.format("%a")),
        _ => format!("{} at {time}", t.format("%b %-d")),
    }
}

/// The time of a run in the run history: "Today 17:30", "Yesterday 17:30",
/// "Tue 17:30", "Oct 12 17:30".
pub fn run_time_label(ms: i64, now_ms: i64, schedule: &RoutineSchedule) -> String {
    let zone = display_zone(schedule);
    let (Some(t), Some(now)) = (in_zone(ms, zone), in_zone(now_ms, zone)) else {
        return String::new();
    };
    let time = clock(t.hour(), t.minute());
    match now
        .date_naive()
        .signed_duration_since(t.date_naive())
        .num_days()
    {
        i64::MIN..=0 => format!("Today {time}"),
        1 => format!("Yesterday {time}"),
        2..=6 => format!("{} {time}", t.format("%a")),
        _ => format!("{} {time}", t.format("%b %-d")),
    }
}

/// "17:30": a time of a routine on a 24-hour clock, in its display zone.
pub fn routine_clock(ms: i64, schedule: &RoutineSchedule) -> String {
    in_zone(ms, display_zone(schedule))
        .map(|t| clock(t.hour(), t.minute()))
        .unwrap_or_default()
}

/// The status of a run in words: "Succeeded", "Failed", "Skipped",
/// "Running".
pub fn run_status_label(status: RoutineRunStatus) -> &'static str {
    match status {
        RoutineRunStatus::Ok => "Succeeded",
        RoutineRunStatus::Error => "Failed",
        RoutineRunStatus::Skipped => "Skipped",
        RoutineRunStatus::Running => "Running",
    }
}

/// How long a run took: "3 s", "48 s", "1 min 4 s", "2 h 5 min". Empty for
/// a skipped run and for a run that still runs.
pub fn run_duration_label(run: &RoutineRunView) -> String {
    if run.status == RoutineRunStatus::Skipped {
        return String::new();
    }
    let Some(ended) = run.ended_at else {
        return String::new();
    };
    let seconds = (ended - run.started_at).max(0) / 1_000;
    match seconds {
        0..=59 => format!("{seconds} s"),
        60..=3_599 => {
            let (m, s) = (seconds / 60, seconds % 60);
            if s == 0 {
                format!("{m} min")
            } else {
                format!("{m} min {s} s")
            }
        }
        _ => {
            let (h, m) = (seconds / 3_600, (seconds % 3_600) / 60);
            if m == 0 {
                format!("{h} h")
            } else {
                format!("{h} h {m} min")
            }
        }
    }
}

/// The run history shows the runs of the last 30 days.
pub const RUN_HISTORY_DAYS: i64 = 30;

/// The runs that the run history shows: the last 30 days, newest first.
pub fn recent_runs(routine: &RoutineView, now_ms: i64) -> Vec<&RoutineRunView> {
    let since = now_ms - RUN_HISTORY_DAYS * 24 * 3_600_000;
    routine
        .runs
        .iter()
        .filter(|run| run.started_at >= since)
        .collect()
}

/// The next runs of a stored routine, for the routine summary: "Today
/// 17:30, Fri 17:30, Sat 17:30". Empty for a routine that is off.
pub fn next_runs_label(routine: &RoutineView, now_ms: i64) -> String {
    if !routine.enabled {
        return String::new();
    }
    let zone = display_zone(&routine.schedule);
    let runs: Vec<i64> = match &routine.schedule {
        RoutineSchedule::Cron {
            expression,
            time_zone,
        } => cron_runs(expression, parse_zone(time_zone).unwrap_or(zone), now_ms, 3),
        RoutineSchedule::Interval { seconds } => {
            let step = i64::try_from(*seconds).unwrap_or(3_600) * 1_000;
            let first = routine.next_run_at.unwrap_or(now_ms + step);
            (0..3).map(|k| first + k * step).collect()
        }
        RoutineSchedule::Once { .. } => routine.next_run_at.into_iter().collect(),
    };
    runs.iter()
        .map(|at| run_label(*at, now_ms, zone))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The schedule of a stored routine as the summary shows it: "Every day at
/// 17:30, Europe/Berlin".
pub fn schedule_detail(schedule: &RoutineSchedule) -> String {
    let words = schedule_words(schedule);
    match schedule {
        RoutineSchedule::Cron { time_zone, .. } => format!("{words}, {time_zone}"),
        RoutineSchedule::Interval { .. } => format!("{words}, counted from the save"),
        RoutineSchedule::Once { .. } => words,
    }
}

// The routine dialog ------------------------------------------------------

/// The fields of the routine dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutineForm {
    pub name: String,
    pub channel_id: String,
    /// True for "Start a task", false for "Ask the assistant".
    pub starts_task: bool,
    /// The prompt, or the brief of the task.
    pub prompt: String,
    /// The task title, for "Start a task".
    pub title: String,
    /// The model and effort of the task, for "Start a task". `None` uses
    /// the channel's task model.
    pub selection: Option<SelectionView>,
    pub schedule: ScheduleForm,
    /// Whether the routine runs. A new routine starts on.
    pub enabled: bool,
}

impl RoutineForm {
    /// An empty form for a new routine in a channel.
    pub fn new(channel_id: &str, now_ms: i64) -> Self {
        Self {
            name: String::new(),
            channel_id: channel_id.to_owned(),
            starts_task: false,
            prompt: String::new(),
            title: String::new(),
            selection: None,
            schedule: ScheduleForm::new(&local_time_zone(), now_ms),
            enabled: true,
        }
    }

    /// The form of a stored routine.
    pub fn of(routine: &RoutineView, now_ms: i64) -> Self {
        let (starts_task, title, selection) = match &routine.action {
            RoutineActionView::Ask => (false, String::new(), None),
            RoutineActionView::Task { title, selection } => {
                (true, title.clone(), selection.clone())
            }
        };
        Self {
            name: routine.name.clone(),
            channel_id: routine.channel_id.clone(),
            starts_task,
            prompt: routine.prompt.clone(),
            title,
            selection,
            schedule: ScheduleForm::from_schedule(&routine.schedule, &local_time_zone(), now_ms),
            enabled: routine.enabled,
        }
    }

    /// Every error of the form, in the order of the fields.
    pub fn errors(&self, now_ms: i64) -> Vec<FieldError> {
        let mut out = Vec::new();
        if self.name.trim().is_empty() {
            out.push(FieldError::new(FormField::Name, "Give the routine a name."));
        } else if self.name.trim().chars().count() > 120 {
            out.push(FieldError::new(
                FormField::Name,
                "Use a name of 120 characters or less.",
            ));
        }
        if self.channel_id.trim().is_empty() {
            out.push(FieldError::new(FormField::Channel, "Choose a channel."));
        }
        if self.starts_task && self.title.trim().is_empty() {
            out.push(FieldError::new(FormField::Title, "Give the task a title."));
        }
        if self.prompt.trim().is_empty() {
            out.push(FieldError::new(
                FormField::Prompt,
                if self.starts_task {
                    "Write what the task must do."
                } else {
                    "Write what the assistant must do."
                },
            ));
        }
        out.extend(self.schedule.errors(now_ms));
        out
    }

    /// The error of one field, if it has one.
    pub fn error(&self, field: FormField, now_ms: i64) -> Option<String> {
        self.errors(now_ms)
            .into_iter()
            .find(|e| e.field == field)
            .map(|e| e.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::time::Timestamp;

    fn at(rfc3339: &str) -> i64 {
        Timestamp::parse_rfc3339(rfc3339).unwrap().as_millis()
    }

    fn form(preset: Preset) -> ScheduleForm {
        let mut form = ScheduleForm::new("Europe/Berlin", at("2026-10-29T12:00:00Z"));
        form.preset = preset;
        form
    }

    #[test]
    fn each_preset_makes_its_cron() {
        let mut every = form(Preset::EveryDay);
        every.time = "17:30".into();
        assert_eq!(every.cron_expression().as_deref(), Some("30 17 * * *"));
        let mut weekdays = form(Preset::Weekdays);
        weekdays.time = "8:00".into();
        assert_eq!(weekdays.cron_expression().as_deref(), Some("0 8 * * 1-5"));
        let mut weekly = form(Preset::Weekly);
        weekly.time = "8:00".into();
        weekly.days = [true, false, false, true, false, false, true];
        assert_eq!(weekly.cron_expression().as_deref(), Some("0 8 * * 1,4,0"));
        assert_eq!(weekly.words(), "Mondays, Thursdays and Sundays at 8:00");
        assert_eq!(every.words(), "Every day at 17:30");
        assert_eq!(weekdays.words(), "Weekdays at 8:00");
    }

    #[test]
    fn a_stored_cron_opens_in_the_preset_that_makes_it() {
        let now = at("2026-10-29T12:00:00Z");
        let cron = |e: &str| RoutineSchedule::Cron {
            expression: e.into(),
            time_zone: "Europe/Berlin".into(),
        };
        let weekdays = ScheduleForm::from_schedule(&cron("0 8 * * 1-5"), "UTC", now);
        assert_eq!(
            (weekdays.preset, weekdays.time.as_str()),
            (Preset::Weekdays, "8:00")
        );
        assert_eq!(weekdays.time_zone, "Europe/Berlin");
        let weekly = ScheduleForm::from_schedule(&cron("0 8 * * 1,4"), "UTC", now);
        assert_eq!(weekly.preset, Preset::Weekly);
        assert_eq!(weekly.days, [true, false, false, true, false, false, false]);
        let other = ScheduleForm::from_schedule(&cron("0 */2 * * 1-5"), "UTC", now);
        assert_eq!(other.preset, Preset::Cron);
        assert_eq!(other.cron, "0 */2 * * 1-5");
        let interval =
            ScheduleForm::from_schedule(&RoutineSchedule::Interval { seconds: 7_200 }, "UTC", now);
        assert_eq!(
            (interval.preset, interval.every.as_str(), interval.unit),
            (Preset::Interval, "2", IntervalUnit::Hours)
        );
    }

    #[test]
    fn the_preview_lists_the_next_runs_in_the_time_zone() {
        // Thursday Oct 29 2026, 12:00 UTC is 13:00 in Berlin (CET).
        let now = at("2026-10-29T12:00:00Z");
        let mut weekdays = form(Preset::Weekdays);
        weekdays.time = "8:00".into();
        let preview = weekdays.preview(now);
        assert!(preview.valid);
        assert_eq!(
            preview.headline,
            "Next runs: Fri 8:00, Mon 8:00, Tue 8:00, Wed 8:00, Thu 8:00"
        );
        assert_eq!(preview.detail, "Weekdays at 8:00. As cron:");
        assert_eq!(preview.cron.as_deref(), Some("0 8 * * 1-5"));
        let mut daily = form(Preset::EveryDay);
        daily.time = "17:30".into();
        assert!(
            daily
                .preview(now)
                .headline
                .starts_with("Next runs: Today 17:30, Fri 17:30")
        );
    }

    #[test]
    fn an_interval_counts_from_the_save_and_needs_a_minute() {
        let now = at("2026-10-29T12:00:00Z");
        let mut interval = form(Preset::Interval);
        interval.every = "30".into();
        assert_eq!(
            interval.next_runs(now, 2),
            vec![now + 30 * 60_000, now + 60 * 60_000]
        );
        assert_eq!(
            interval.schedule(now),
            Ok(RoutineSchedule::Interval { seconds: 1_800 })
        );
        assert_eq!(interval.words(), "Every 30 minutes");
        assert!(
            interval
                .preview(now)
                .detail
                .ends_with("counted from when you save it.")
        );
        interval.every = "0".into();
        let errors = interval.errors(now);
        assert_eq!(errors[0].field, FormField::Every);
        interval.every = "1".into();
        interval.unit = IntervalUnit::Hours;
        assert_eq!(
            interval.schedule(now),
            Ok(RoutineSchedule::Interval { seconds: 3_600 })
        );
    }

    #[test]
    fn a_once_time_in_the_past_is_an_error_under_the_date() {
        let now = at("2026-10-29T12:00:00Z");
        let mut once = form(Preset::Once);
        once.date = "Wed, Oct 28, 2026".into();
        once.time = "9:00".into();
        let errors = once.errors(now);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, FormField::Date);
        assert_eq!(
            errors[0].message,
            "This time is in the past. Choose a later date or time."
        );
        let preview = once.preview(now);
        assert!(!preview.valid);
        assert_eq!(preview.headline, "Choose a later time to see the run.");
        once.date = "2026-11-03".into();
        assert!(once.errors(now).is_empty());
        // 9:00 in Berlin is 8:00 UTC in November.
        assert_eq!(
            once.schedule(now),
            Ok(RoutineSchedule::Once {
                at: Timestamp::from_millis(at("2026-11-03T08:00:00Z"))
            })
        );
        assert_eq!(once.preview(now).headline, "Runs once: Tue, Nov 3 at 9:00");
        assert_eq!(once.words(), "Once on Nov 3 at 9:00");
    }

    #[test]
    fn a_cron_with_the_wrong_field_count_says_what_to_change() {
        let now = at("2026-10-29T12:00:00Z");
        let mut cron = form(Preset::Cron);
        cron.cron = "0 8 * * MON 1".into();
        let errors = cron.errors(now);
        assert_eq!(
            errors[0].message,
            "This cron has 6 fields. Remove one so it has 5: minute, hour, day of month, month and day of week."
        );
        assert_eq!(
            cron.preview(now).headline,
            "Fix the cron to see the next runs."
        );
        cron.cron = "0 8 *".into();
        assert!(
            cron.errors(now)[0]
                .message
                .starts_with("This cron has 3 fields. Add some")
        );
        cron.cron = "0 */2 * * 1-5".into();
        assert!(cron.errors(now).is_empty());
        assert_eq!(cron.words(), "Every 2 hours, Monday to Friday");
        cron.cron = "61 8 * * *".into();
        assert!(
            cron.errors(now)[0]
                .message
                .starts_with("This cron is not valid")
        );
    }

    #[test]
    fn a_time_zone_that_is_not_known_is_an_error() {
        let mut daily = form(Preset::EveryDay);
        daily.time_zone = "Mars/Olympus".into();
        assert_eq!(daily.errors(0)[0].field, FormField::TimeZone);
        // An interval has no time zone.
        daily.preset = Preset::Interval;
        assert!(daily.errors(0).is_empty());
    }

    #[test]
    fn times_and_dates_parse_in_the_forms_the_fields_show() {
        assert_eq!(parse_time("8:00"), Some((8, 0)));
        assert_eq!(parse_time("17:30"), Some((17, 30)));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("8"), None);
        let date = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        assert_eq!(parse_date("Tue, Nov 3, 2026"), Some(date));
        assert_eq!(parse_date("2026-11-03"), Some(date));
        assert_eq!(parse_date("Nov 3, 2026"), Some(date));
        assert_eq!(date_text(date), "Tue, Nov 3, 2026");
    }

    #[test]
    fn the_calendar_starts_on_monday_and_marks_the_chosen_day() {
        let now = at("2026-10-29T12:00:00Z");
        let month = calendar_month(2026, 11, "Tue, Nov 3, 2026", now).unwrap();
        assert_eq!(month.title, "November 2026");
        assert_eq!((month.prev, month.next), ((2026, 10), (2026, 12)));
        // November 1, 2026 is a Sunday, so the first week starts on Oct 26.
        assert_eq!(month.weeks[0][0].key, "2026-10-26");
        assert!(!month.weeks[0][0].in_month && month.weeks[0][0].past);
        assert!(month.weeks[1][1].chosen);
        assert_eq!(month.weeks[1][1].text, "Tue, Nov 3, 2026");
        assert_eq!(calendar_start("Tue, Nov 3, 2026", now), (2026, 11));
        assert_eq!(calendar_month(2026, 13, "", now), None);
    }

    #[test]
    fn switching_to_cron_keeps_the_cron_of_the_preset() {
        let mut weekdays = form(Preset::Weekdays);
        weekdays.time = "7:30".into();
        weekdays.set_preset(Preset::Cron);
        assert_eq!(weekdays.cron, "30 7 * * 1-5");
    }

    fn routine(enabled: bool, status: Option<RoutineRunStatus>, last: Option<i64>) -> RoutineView {
        RoutineView {
            id: "rtn_1".into(),
            channel_id: "chn_1".into(),
            name: "Sunset check".into(),
            enabled,
            schedule: RoutineSchedule::Cron {
                expression: "30 17 * * *".into(),
                time_zone: "Europe/Berlin".into(),
            },
            prompt: "Check.".into(),
            action: RoutineActionView::Ask,
            next_run_at: None,
            last_run_at: last,
            last_run_status: status,
            runs: Vec::new(),
            runs_loaded: false,
            created_at: None,
            kind: Default::default(),
            origin: Default::default(),
        }
    }

    #[test]
    fn the_row_lines_say_the_schedule_the_action_and_the_last_result() {
        let now = crate::format::now_ms();
        let on = routine(true, Some(RoutineRunStatus::Ok), Some(now));
        assert_eq!(summary_line(&on), "Every day at 17:30, asks the assistant");
        let line = status_line(&on, now);
        assert_eq!(
            (line.tone, line.result.as_str()),
            (StatusTone::Ok, "Succeeded today.")
        );
        let failed = routine(true, Some(RoutineRunStatus::Error), Some(now - 86_400_000));
        assert_eq!(status_line(&failed, now).result, "Failed yesterday.");
        let off = routine(false, Some(RoutineRunStatus::Ok), Some(now));
        assert_eq!(status_line(&off, now).result, OFF_TEXT);
        assert_eq!(status_line(&off, now).tone, StatusTone::Off);
        let mut task = routine(true, None, None);
        task.action = RoutineActionView::Task {
            title: "Check".into(),
            selection: None,
        };
        assert!(summary_line(&task).ends_with(", starts a task"));
        assert_eq!(status_line(&task, now).result, "Has not run yet.");
    }

    #[test]
    fn durations_read_as_the_run_history_shows_them() {
        let run = |ms: i64, status| RoutineRunView {
            id: 1,
            routine_id: "rtn_1".into(),
            started_at: 0,
            ended_at: Some(ms),
            status,
            manual: false,
            message_id: None,
            task_id: None,
            error: None,
        };
        assert_eq!(
            run_duration_label(&run(3_000, RoutineRunStatus::Error)),
            "3 s"
        );
        assert_eq!(
            run_duration_label(&run(64_000, RoutineRunStatus::Ok)),
            "1 min 4 s"
        );
        assert_eq!(
            run_duration_label(&run(7_500_000, RoutineRunStatus::Ok)),
            "2 h 5 min"
        );
        assert_eq!(run_duration_label(&run(0, RoutineRunStatus::Skipped)), "");
    }

    #[test]
    fn the_form_checks_its_text_fields() {
        let now = at("2026-10-29T12:00:00Z");
        let mut form = RoutineForm::new("chn_1", now);
        let fields: Vec<FormField> = form.errors(now).iter().map(|e| e.field).collect();
        assert_eq!(fields, vec![FormField::Name, FormField::Prompt]);
        form.starts_task = true;
        form.name = "Weekly price watch".into();
        form.prompt = "Check the prices.".into();
        assert_eq!(
            form.error(FormField::Title, now).as_deref(),
            Some("Give the task a title.")
        );
        form.title = "Check prices".into();
        assert!(form.errors(now).is_empty());
    }
}
