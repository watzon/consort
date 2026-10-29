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

/// Markdown as one run of plain text, for a short summary on a card: code
/// blocks, headings and list marks go, links keep their words, and every
/// blank line and line break becomes one space. The card clamps it.
pub fn card_summary(markdown: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || trimmed.is_empty() {
            continue;
        }
        let mut text = trimmed.trim_start_matches('#').trim_start();
        for mark in ["- ", "* ", "+ ", "> "] {
            if let Some(rest) = text.strip_prefix(mark) {
                text = rest;
            }
        }
        if let Some((number, rest)) = text.split_once(". ")
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit())
        {
            text = rest;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&strip_marks(text));
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Inline Markdown without its marks: emphasis and code marks go, and a
/// link keeps only its words. An underscore inside a word (`new_members`)
/// is part of the word, so it stays.
pub fn strip_marks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '_' if prev.is_some_and(char::is_alphanumeric)
                && chars.peek().is_some_and(|c| c.is_alphanumeric()) =>
            {
                out.push(ch);
            }
            '*' | '_' | '`' => {}
            '[' => {
                let mut label = String::new();
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    label.push(c);
                }
                if chars.peek() == Some(&'(') {
                    for c in chars.by_ref() {
                        if c == ')' {
                            break;
                        }
                    }
                }
                out.push_str(&label);
            }
            _ => out.push(ch),
        }
        prev = Some(ch);
    }
    out
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
    fn card_summaries_are_clean_text() {
        let md = "Wrote **hello.txt** in the workspace.\n\n```sh\ncat hello.txt\n```\n\n- It says [hi](https://x.y).\n- Tests pass.";
        assert_eq!(
            card_summary(md),
            "Wrote hello.txt in the workspace. It says hi. Tests pass."
        );
        assert_eq!(card_summary("## Done\n1. One"), "Done One");
        assert_eq!(
            card_summary("Edit `rules/new_members.toml`, _not_ __this__."),
            "Edit rules/new_members.toml, not this."
        );
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
