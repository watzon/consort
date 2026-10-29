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

/// Markdown as plain text for a short summary on a card, one block to a
/// line. Headings and list marks go, emphasis and code marks go, and a link
/// keeps its words. A paragraph is one line, even when the source wraps it.
/// A heading, a list item, a quote, and each line of a code block (fenced or
/// indented) is a line of its own, so the lines of a code block never run
/// together with the text around them. The card clamps the lines.
pub fn card_summary(markdown: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut paragraph = String::new();
    let mut in_fence = false;
    let mut in_indented_code = false;
    let mut after_blank = true;
    fn flush(paragraph: &mut String, lines: &mut Vec<String>) {
        let text = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            lines.push(text);
        }
        paragraph.clear();
    }
    for raw in markdown.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut paragraph, &mut lines);
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            // Code is text as it is: no marks come off.
            if !trimmed.is_empty() {
                lines.push(trimmed.to_owned());
            }
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut lines);
            in_indented_code = false;
            after_blank = true;
            continue;
        }
        let indented = raw.starts_with("    ") || raw.starts_with('\t');
        if indented && (after_blank || in_indented_code) {
            flush(&mut paragraph, &mut lines);
            in_indented_code = true;
            after_blank = false;
            lines.push(trimmed.to_owned());
            continue;
        }
        in_indented_code = false;
        after_blank = false;
        let mut text = trimmed;
        let mut new_block = false;
        if text.starts_with('#') {
            text = text.trim_start_matches('#').trim_start();
            new_block = true;
        }
        for mark in ["- ", "* ", "+ ", "> "] {
            if let Some(rest) = text.strip_prefix(mark) {
                text = rest;
                new_block = true;
            }
        }
        if let Some((number, rest)) = text.split_once(". ")
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit())
        {
            text = rest;
            new_block = true;
        }
        if new_block {
            flush(&mut paragraph, &mut lines);
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(&strip_marks(text));
        if new_block {
            // A heading or a list item is a line by itself.
            flush(&mut paragraph, &mut lines);
        }
    }
    flush(&mut paragraph, &mut lines);
    lines.join("\n")
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
    fn card_summaries_are_clean_text_with_one_block_to_a_line() {
        let md = "Wrote **hello.txt** in the workspace.\n\n```sh\ncat hello.txt\n```\n\n- It says [hi](https://x.y).\n- Tests pass.";
        assert_eq!(
            card_summary(md),
            "Wrote hello.txt in the workspace.\ncat hello.txt\nIt says hi.\nTests pass."
        );
        assert_eq!(card_summary("## Done\n1. One"), "Done\nOne");
        assert_eq!(
            card_summary("Edit `rules/new_members.toml`, _not_ __this__."),
            "Edit rules/new_members.toml, not this."
        );
        assert_eq!(card_summary("  \n\n"), "");
    }

    /// Finding 16 of review cycle 2: "...were changed. uname -s date -u %Y"
    /// ran the lines of a code block together with the text before it.
    #[test]
    fn the_lines_of_a_code_block_do_not_run_together() {
        let indented =
            "Three files were changed.\n\n    uname -s\n    date -u +%Y\n\nNo other files changed.";
        assert_eq!(
            card_summary(indented),
            "Three files were changed.\nuname -s\ndate -u +%Y\nNo other files changed."
        );
        let fenced = "Three files were changed.\n```\nuname -s\ndate -u +%Y\n```\nDone.";
        assert_eq!(
            card_summary(fenced),
            "Three files were changed.\nuname -s\ndate -u +%Y\nDone."
        );
        // Marks in code stay: they are not Markdown there.
        assert_eq!(card_summary("```\nls *_x\n```"), "ls *_x");
    }

    #[test]
    fn a_paragraph_that_wraps_in_the_source_stays_one_line() {
        let md = "The first line of the\nsame paragraph.\n\nA second paragraph.";
        assert_eq!(
            card_summary(md),
            "The first line of the same paragraph.\nA second paragraph."
        );
        // An indented line inside a paragraph is not code.
        assert_eq!(card_summary("Text\n    still text"), "Text still text");
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
