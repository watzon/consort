//! Labels made from times, in the user's time zone.

use chrono::{DateTime, Datelike, Local, TimeZone};

fn local(ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

/// "9:12 AM".
pub fn time_label(ms: i64) -> String {
    local(ms)
        .map(|t| t.format("%-I:%M %p").to_string())
        .unwrap_or_default()
}

/// "Today", "Yesterday", "Tuesday" within a week, then "October 12".
pub fn day_label(ms: i64, now_ms: i64) -> String {
    let (Some(day), Some(now)) = (local(ms), local(now_ms)) else {
        return String::new();
    };
    let days = now
        .date_naive()
        .signed_duration_since(day.date_naive())
        .num_days();
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => day.format("%A").to_string(),
        _ if day.year() == now.year() => day.format("%B %-d").to_string(),
        _ => day.format("%B %-d, %Y").to_string(),
    }
}

/// The local day of a time, to group messages under day labels.
pub fn day_key(ms: i64) -> String {
    local(ms)
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// "Now", "2 min", "3 h", "4 d".
pub fn age_label(ms: i64, now_ms: i64) -> String {
    let minutes = (now_ms - ms).max(0) / 60_000;
    match minutes {
        0 => "Now".into(),
        1..=59 => format!("{minutes} min"),
        60..=1439 => format!("{} h", minutes / 60),
        _ => format!("{} d", minutes / 1440),
    }
}

/// The time now, in Unix milliseconds.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_round_down() {
        let now = 1_000_000_000_000;
        assert_eq!(age_label(now - 30_000, now), "Now");
        assert_eq!(age_label(now - 2 * 60_000, now), "2 min");
        assert_eq!(age_label(now - 3 * 3_600_000 - 5, now), "3 h");
        assert_eq!(age_label(now - 50 * 3_600_000, now), "2 d");
    }

    #[test]
    fn days_are_named() {
        let now = Local
            .with_ymd_and_hms(2026, 10, 29, 12, 0, 0)
            .single()
            .expect("valid")
            .timestamp_millis();
        assert_eq!(day_label(now - 3_600_000, now), "Today");
        assert_eq!(day_label(now - 24 * 3_600_000, now), "Yesterday");
        assert_eq!(day_label(now - 40 * 24 * 3_600_000, now), "September 19");
    }
}
