//! The commitment scan: dates, times, weekdays, money amounts and agreement
//! phrases in a draft that the request did not contain, and text that
//! breaks a hard rule of the style guide that a simple pattern can check.
//!
//! The scan first normalizes the text: curly quotes and apostrophes become
//! straight ones, non-breaking and thin spaces become one space, and
//! letters become lowercase. A map keeps the byte offsets of the original
//! text, so a flag's `start` and `end` point into the field value as it is.
//!
//! Each match has a value: a time is minutes after midnight, a date is a
//! month and a day, an amount is a number of cents with its currency, an
//! agreement is its phrase. The scan reads the request the same way and
//! compares values, never substrings: `$50` is flagged when the request
//! says `$500`, and `Deal` is flagged when the request says `ideal`.

use consort_proto::model::{CommitmentFlag, FlagKind, VoiceRule};

// Normalized text ----------------------------------------------------------

/// A text in the scan's form, with the original byte range of each byte.
struct Norm {
    text: String,
    /// For each byte of `text`: the start of its original character.
    starts: Vec<usize>,
    /// For each byte of `text`: the end of its original character.
    ends: Vec<usize>,
}

impl Norm {
    fn new(original: &str) -> Self {
        let mut text = String::with_capacity(original.len());
        let mut starts = Vec::with_capacity(original.len());
        let mut ends = Vec::with_capacity(original.len());
        for (i, c) in original.char_indices() {
            let end = i + c.len_utf8();
            let mapped: String = match c {
                '\u{2018}' | '\u{2019}' | '\u{201B}' | '\u{2032}' | '\u{02BC}' | '`' => "'".into(),
                '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => "\"".into(),
                '\n' => "\n".into(),
                c if c.is_whitespace() => {
                    // A run of spaces becomes one space.
                    if text.ends_with(' ') {
                        if let Some(last) = ends.last_mut() {
                            *last = end;
                        }
                        continue;
                    }
                    " ".into()
                }
                c => c.to_lowercase().collect(),
            };
            for _ in 0..mapped.len() {
                starts.push(i);
                ends.push(end);
            }
            text.push_str(&mapped);
        }
        Self { text, starts, ends }
    }

    /// The original byte range of `self.text[start..end]`.
    fn original(&self, start: usize, end: usize) -> (usize, usize) {
        if start >= end || end > self.text.len() {
            return (0, 0);
        }
        (self.starts[start], self.ends[end - 1])
    }
}

// Values -------------------------------------------------------------------

/// The value of a match, for comparing a draft with the request.
#[derive(Debug, Clone, PartialEq)]
enum Val {
    /// Minutes after midnight.
    Time(u16),
    /// The end of the working day ("EOD", "close of business").
    EndOfDay,
    /// A calendar day. The year is there only when the text has one.
    Day {
        month: u8,
        day: u8,
        year: Option<u16>,
    },
    /// A day of the month alone ("the 14th").
    DayOfMonth(u8),
    /// A weekday, 1 for Monday. `qual` is 0 alone, 1 for "this", 2 for
    /// "next".
    Weekday { day: u8, qual: u8 },
    /// A day relative to today, such as "tomorrow".
    Relative(&'static str),
    /// An amount in cents, with the currency when the text names one.
    Amount {
        cents: i128,
        currency: Option<&'static str>,
    },
    /// An agreement phrase, in its canonical form.
    Phrase(String),
}

/// The currency group of a code: `$` and the dollar words match USD, CAD
/// and AUD; the yen sign matches JPY and CNY.
fn currency_matches(a: &str, b: &str) -> bool {
    const DOLLARS: &[&str] = &["dollar", "usd", "cad", "aud", "nzd", "sgd", "hkd"];
    const YEN: &[&str] = &["yen", "jpy", "cny"];
    // A sign or word matches every code of its group; two different codes
    // do not match.
    a == b
        || (a == "dollar" && DOLLARS.contains(&b))
        || (b == "dollar" && DOLLARS.contains(&a))
        || (a == "yen" && YEN.contains(&b))
        || (b == "yen" && YEN.contains(&a))
}

fn same(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (
            Val::Day {
                month: m1,
                day: d1,
                year: y1,
            },
            Val::Day {
                month: m2,
                day: d2,
                year: y2,
            },
        ) => m1 == m2 && d1 == d2 && (y1.is_none() || y2.is_none() || y1 == y2),
        (Val::DayOfMonth(d), Val::Day { day, .. }) | (Val::Day { day, .. }, Val::DayOfMonth(d)) => {
            d == day
        }
        (Val::Weekday { day: d1, qual: q1 }, Val::Weekday { day: d2, qual: q2 }) => {
            d1 == d2 && (*q1 == 0 || *q2 == 0 || q1 == q2)
        }
        (
            Val::Amount {
                cents: c1,
                currency: k1,
            },
            Val::Amount {
                cents: c2,
                currency: k2,
            },
        ) => {
            c1 == c2
                && match (k1, k2) {
                    (Some(a), Some(b)) => currency_matches(a, b),
                    _ => true,
                }
        }
        _ => a == b,
    }
}

/// A phrase that the scan found, in normalized offsets, with its value
/// parts. Each part lists the values it can mean; the request must hold
/// one of them for every part.
#[derive(Debug, Clone)]
struct Found {
    start: usize,
    end: usize,
    kind: FlagKind,
    parts: Vec<Vec<Val>>,
}

impl Found {
    fn one(start: usize, end: usize, kind: FlagKind, value: Val) -> Self {
        Self {
            start,
            end,
            kind,
            parts: vec![vec![value]],
        }
    }
}

// Helpers on normalized bytes ---------------------------------------------

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'\''
}

/// Whether `text[start..end]` stands as whole words.
fn whole_word(bytes: &[u8], start: usize, end: usize) -> bool {
    let before = start == 0 || !is_word_byte(bytes[start - 1]);
    let after = end >= bytes.len() || !is_word_byte(bytes[end]);
    before && after
}

/// Every whole-word place of `needle` in `text`.
fn find_words(text: &str, needle: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(needle) {
        let start = from + at;
        let end = start + needle.len();
        if whole_word(bytes, start, end) {
            out.push((start, end));
        }
        from = start + 1;
        while from < text.len() && !text.is_char_boundary(from) {
            from += 1;
        }
    }
    out
}

fn digits_end(bytes: &[u8], at: usize) -> usize {
    let mut i = at;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    i
}

fn skip_spaces(bytes: &[u8], at: usize) -> usize {
    let mut i = at;
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    i
}

fn number(bytes: &[u8], start: usize, end: usize) -> Option<u32> {
    std::str::from_utf8(&bytes[start..end]).ok()?.parse().ok()
}

/// The text before `at`, without spaces at its end.
fn head(text: &str, at: usize) -> &str {
    text[..at].trim_end_matches(' ')
}

// Dates ----------------------------------------------------------------------

const WEEKDAYS: &[(&str, u8)] = &[
    ("monday", 1),
    ("tuesday", 2),
    ("wednesday", 3),
    ("thursday", 4),
    ("friday", 5),
    ("saturday", 6),
    ("sunday", 7),
    ("mon", 1),
    ("tue", 2),
    ("tues", 2),
    ("wed", 3),
    ("thu", 4),
    ("thur", 4),
    ("thurs", 4),
    ("fri", 5),
];
/// "sat" and "sun" are also words, so they count only next to a date sign.
const WEEKEND_ABBREVIATIONS: &[(&str, u8)] = &[("sat", 6), ("sun", 7)];

const RELATIVE_DAYS: &[(&str, &str)] = &[
    ("the day after tomorrow", "day after tomorrow"),
    ("day after tomorrow", "day after tomorrow"),
    ("tomorrow", "tomorrow"),
    ("tmrw", "tomorrow"),
    ("tonight", "tonight"),
    ("this evening", "tonight"),
    ("today", "today"),
    ("next week", "next week"),
    ("this week", "this week"),
    ("next month", "next month"),
    ("this month", "this month"),
    ("this weekend", "weekend"),
    ("next weekend", "next weekend"),
    ("end of the week", "end of week"),
    ("end of week", "end of week"),
    ("eow", "end of week"),
    ("end of the month", "end of month"),
    ("end of month", "end of month"),
    ("eom", "end of month"),
];

const END_OF_DAY: &[&str] = &[
    "eod",
    "end of day",
    "end of the day",
    "end of business",
    "close of business",
    "cob",
    "eob",
];

const MONTHS: &[(&str, u8)] = &[
    ("january", 1),
    ("february", 2),
    ("march", 3),
    ("april", 4),
    ("may", 5),
    ("june", 6),
    ("july", 7),
    ("august", 8),
    ("september", 9),
    ("october", 10),
    ("november", 11),
    ("december", 12),
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("sept", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];

/// A day number with an optional ordinal at `at`: `14`, `14th`, `1st`.
/// Returns the day and the end.
fn day_number(bytes: &[u8], at: usize) -> Option<(u8, usize)> {
    let end = digits_end(bytes, at);
    if end == at || end - at > 2 {
        return None;
    }
    let day = u8::try_from(number(bytes, at, end)?).ok()?;
    let mut end = end;
    for suffix in [b"st", b"nd", b"rd", b"th"] {
        if bytes.len() >= end + 2 && &bytes[end..end + 2] == suffix {
            end += 2;
            break;
        }
    }
    ((1..=31).contains(&day) && (end >= bytes.len() || !bytes[end].is_ascii_alphanumeric()))
        .then_some((day, end))
}

/// An optional year after a date: `, 2026` or ` 2026`. Returns the year and
/// the end.
fn year_after(bytes: &[u8], at: usize) -> Option<(u16, usize)> {
    let mut i = at;
    if bytes.get(i) == Some(&b',') {
        i += 1;
    }
    let i = skip_spaces(bytes, i);
    if i == at {
        return None;
    }
    let end = digits_end(bytes, i);
    if end - i != 4 || bytes.get(end).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    let year = u16::try_from(number(bytes, i, end)?).ok()?;
    (1900..=2200).contains(&year).then_some((year, end))
}

fn full_year(year: u32, digits: usize) -> Option<u16> {
    let year = if digits == 2 { 2000 + year } else { year };
    u16::try_from(year)
        .ok()
        .filter(|y| (1900..=2200).contains(y))
}

fn day(month: u32, day: u32, year: Option<u16>) -> Option<Val> {
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then(|| Val::Day {
        month: u8::try_from(month).unwrap_or(0),
        day: u8::try_from(day).unwrap_or(0),
        year,
    })
}

fn weekdays(text: &str, out: &mut Vec<Found>) {
    let bytes = text.as_bytes();
    let mut add = |start: usize, end: usize, day: u8| {
        let mut start = start;
        let mut end = end;
        // "Fri." keeps its period.
        if end - start <= 5 && bytes.get(end) == Some(&b'.') {
            end += 1;
        }
        let before = head(text, start);
        let qual = if before.ends_with("this") || before.ends_with("coming") {
            1
        } else if before.ends_with("next") {
            2
        } else {
            0
        };
        if qual > 0 {
            let word = if before.ends_with("this") {
                "this"
            } else if before.ends_with("next") {
                "next"
            } else {
                "coming"
            };
            let at = before.len() - word.len();
            if whole_word(bytes, at, before.len()) {
                start = at;
            }
        }
        out.push(Found::one(
            start,
            end,
            FlagKind::Weekday,
            Val::Weekday { day, qual },
        ));
    };
    for (word, n) in WEEKDAYS {
        for (start, end) in find_words(text, word) {
            add(start, end, *n);
        }
    }
    for (word, n) in WEEKEND_ABBREVIATIONS {
        for (start, end) in find_words(text, word) {
            let next = bytes.get(end).copied();
            let after = skip_spaces(bytes, end);
            let before = head(text, start);
            let date_sign = matches!(next, Some(b'.' | b','))
                || (after > end && bytes.get(after).is_some_and(u8::is_ascii_digit))
                || ["on", "this", "next", "until", "by"]
                    .iter()
                    .any(|w| before.ends_with(w));
            if date_sign {
                add(start, end, *n);
            }
        }
    }
}

fn relative_days(text: &str, out: &mut Vec<Found>) {
    for (phrase, value) in RELATIVE_DAYS {
        for (start, end) in find_words(text, phrase) {
            out.push(Found::one(start, end, FlagKind::Date, Val::Relative(value)));
        }
    }
    for phrase in END_OF_DAY {
        for (start, end) in find_words(text, phrase) {
            out.push(Found::one(start, end, FlagKind::Time, Val::EndOfDay));
        }
    }
}

/// "Nov 14", "Nov. 14, 2026", "March the 3rd", "14 November", "3rd of
/// March 2026", and "the 14th".
fn month_dates(text: &str, out: &mut Vec<Found>) {
    let bytes = text.as_bytes();
    for (word, month) in MONTHS {
        for (start, end) in find_words(text, word) {
            let mut after = end;
            if bytes.get(after) == Some(&b'.') {
                after += 1;
            }
            let after = skip_spaces(bytes, after);
            let after = if text[after..].starts_with("the ") {
                after + 4
            } else {
                after
            };
            if let Some((d, day_end)) = day_number(bytes, after) {
                let (year, end) = match year_after(bytes, day_end) {
                    Some((y, e)) => (Some(y), e),
                    None => (None, day_end),
                };
                if let Some(value) = day(u32::from(*month), u32::from(d), year) {
                    out.push(Found::one(start, end, FlagKind::Date, value));
                }
                continue;
            }
            // Before: "<n> " or "<n>th of ".
            let before = head(text, start);
            let before = before.strip_suffix(" of").unwrap_or(before);
            let before = before.trim_end_matches(' ');
            let b = before.as_bytes();
            let mut i = b.len();
            while i > 0 && b[i - 1].is_ascii_alphanumeric() {
                i -= 1;
            }
            if i < b.len()
                && b[i].is_ascii_digit()
                && (i == 0 || !is_word_byte(b[i - 1]))
                && let Some((d, day_end)) = day_number(b, i)
                && day_end == b.len()
            {
                let mut month_end = end;
                if bytes.get(month_end) == Some(&b'.') {
                    month_end += 1;
                }
                let (year, end) = match year_after(bytes, month_end) {
                    Some((y, e)) => (Some(y), e),
                    None => (None, end),
                };
                if let Some(value) = day(u32::from(*month), u32::from(d), year) {
                    out.push(Found::one(i, end, FlagKind::Date, value));
                }
            }
        }
    }
    // "the 14th".
    for (start, _) in find_words(text, "the") {
        let at = start + 4;
        if at < bytes.len()
            && bytes[at].is_ascii_digit()
            && let Some((d, end)) = day_number(bytes, at)
            && end > digits_end(bytes, at)
        {
            out.push(Found::one(start, end, FlagKind::Date, Val::DayOfMonth(d)));
        }
    }
}

/// Numeric dates: `11/14`, `11/14/2026`, `2026-11-14`, `14.11.2026`,
/// `14-11-2026`. A date that can be read both ways (`3/4`) has both
/// values.
fn numeric_dates(text: &str, out: &mut Vec<Found>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let starts_here = bytes[i].is_ascii_digit()
            && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'.'));
        if !starts_here {
            i += 1;
            continue;
        }
        let mut groups = vec![(i, digits_end(bytes, i))];
        let mut sep = None;
        while groups.len() < 3 {
            let at = groups[groups.len() - 1].1;
            let Some(&c) = bytes.get(at) else { break };
            if !matches!(c, b'/' | b'-' | b'.') || sep.is_some_and(|s| s != c) {
                break;
            }
            if !bytes.get(at + 1).is_some_and(u8::is_ascii_digit) {
                break;
            }
            sep = Some(c);
            groups.push((at + 1, digits_end(bytes, at + 1)));
        }
        let end = groups[groups.len() - 1].1;
        let next_ok = end >= bytes.len()
            || !(bytes[end].is_ascii_alphanumeric()
                || (matches!(bytes[end], b'/' | b'-' | b'.')
                    && bytes.get(end + 1).is_some_and(u8::is_ascii_digit)));
        let lens: Vec<usize> = groups.iter().map(|(s, e)| e - s).collect();
        let nums: Vec<u32> = groups
            .iter()
            .map(|(s, e)| number(bytes, *s, *e).unwrap_or(0))
            .collect();
        let mut values = Vec::new();
        match (groups.len(), sep) {
            (2, Some(b'/')) if lens[0] <= 2 && lens[1] <= 2 => {
                values.extend(day(nums[0], nums[1], None));
                values.extend(day(nums[1], nums[0], None));
            }
            (3, Some(_)) if lens[0] == 4 && lens[1] <= 2 && lens[2] <= 2 => {
                values.extend(day(nums[1], nums[2], full_year(nums[0], 4)));
            }
            (3, Some(s)) if lens[0] <= 2 && lens[1] <= 2 && matches!(lens[2], 2 | 4) => {
                let year = full_year(nums[2], lens[2]);
                if s == b'/' {
                    values.extend(day(nums[0], nums[1], year));
                    values.extend(day(nums[1], nums[0], year));
                } else {
                    values.extend(day(nums[1], nums[0], year));
                    values.extend(day(nums[0], nums[1], year));
                }
            }
            _ => {}
        }
        values.dedup();
        if next_ok && !values.is_empty() {
            out.push(Found {
                start: i,
                end,
                kind: FlagKind::Date,
                parts: vec![values],
            });
            i = end;
            continue;
        }
        i = groups[0].1.max(i + 1);
    }
}

// Times ----------------------------------------------------------------------

fn times(text: &str, out: &mut Vec<Found>) {
    let bytes = text.as_bytes();
    for (word, minutes) in [("noon", 720), ("midday", 720), ("midnight", 0)] {
        for (start, end) in find_words(text, word) {
            out.push(Found::one(start, end, FlagKind::Time, Val::Time(minutes)));
        }
    }
    let mut i = 0;
    while i < bytes.len() {
        let starts_here = bytes[i].is_ascii_digit()
            && (i == 0
                || !(bytes[i - 1].is_ascii_alphanumeric()
                    || matches!(bytes[i - 1], b':' | b'.' | b',')));
        if !starts_here {
            i += 1;
            continue;
        }
        let hour_end = digits_end(bytes, i);
        let hour_len = hour_end - i;
        if hour_len > 2 {
            i = hour_end;
            continue;
        }
        let hour = number(bytes, i, hour_end).unwrap_or(99);
        let mut end = hour_end;
        let mut minutes = None;
        // Minutes, unless more number parts follow (`14.11.2026`).
        if matches!(bytes.get(end), Some(b':' | b'.'))
            && digits_end(bytes, end + 1) == end + 3
            && !(matches!(bytes.get(end + 3), Some(b':' | b'.' | b'/' | b'-'))
                && bytes.get(end + 4).is_some_and(u8::is_ascii_digit))
        {
            let m = number(bytes, end + 1, end + 3).unwrap_or(99);
            if m <= 59 {
                minutes = Some((m, bytes[end] == b'.'));
                end += 3;
            }
        }
        let after = skip_spaces(bytes, end);
        let after = if after - end > 1 { end } else { after };
        let rest = &text[after..];
        let meridiem = ["a.m.", "p.m.", "am", "pm", "a.m", "p.m"].iter().find(|m| {
            rest.starts_with(**m)
                && (m.ends_with('.')
                    || !bytes.get(after + m.len()).is_some_and(|b| is_word_byte(*b)))
        });
        let m = minutes.map_or(0, |(m, _)| m);
        if let Some(meridiem) = meridiem
            && hour <= 23
        {
            let pm = meridiem.starts_with('p');
            let h = if hour > 12 {
                hour
            } else if pm {
                hour % 12 + 12
            } else {
                hour % 12
            };
            let value = u16::try_from(h * 60 + m).unwrap_or(0);
            let end = after + meridiem.len();
            out.push(Found::one(i, end, FlagKind::Time, Val::Time(value)));
            i = end;
            continue;
        }
        // "3 o'clock".
        if minutes.is_none() && (1..=12).contains(&hour) && text[after..].starts_with("o'clock") {
            let end = after + "o'clock".len();
            let a = u16::try_from(hour % 12 * 60).unwrap_or(0);
            out.push(Found {
                start: i,
                end,
                kind: FlagKind::Time,
                parts: vec![vec![Val::Time(a), Val::Time(a + 720)]],
            });
            i = end;
            continue;
        }
        // "15:30", "9:30", and "15.30" with a two-digit hour.
        if let Some((m, dot)) = minutes
            && hour <= 23
            && (!dot || hour_len == 2)
            && (end >= bytes.len() || !bytes[end].is_ascii_alphanumeric())
        {
            let exact = u16::try_from(hour * 60 + m).unwrap_or(0);
            let mut values = vec![Val::Time(exact)];
            if (1..=12).contains(&hour) {
                values.push(Val::Time(
                    u16::try_from((hour + 12) % 24 * 60 + m).unwrap_or(0),
                ));
            }
            out.push(Found {
                start: i,
                end,
                kind: FlagKind::Time,
                parts: vec![values],
            });
            i = end;
            continue;
        }
        i = end.max(i + 1);
    }
}

// Money ----------------------------------------------------------------------

/// Currency signs, before or after the number.
const CURRENCY_SIGNS: &[(&str, &str)] = &[
    ("$", "dollar"),
    ("€", "eur"),
    ("£", "gbp"),
    ("¥", "yen"),
    ("₹", "inr"),
    ("₩", "krw"),
    ("₽", "rub"),
    ("₺", "try"),
    ("₪", "ils"),
    ("₫", "vnd"),
    ("฿", "thb"),
    ("₦", "ngn"),
    ("₱", "php"),
];

/// Currency codes and words, before or after the number.
const CURRENCY_WORDS: &[(&str, &str)] = &[
    ("usd", "usd"),
    ("eur", "eur"),
    ("gbp", "gbp"),
    ("chf", "chf"),
    ("jpy", "jpy"),
    ("cny", "cny"),
    ("inr", "inr"),
    ("cad", "cad"),
    ("aud", "aud"),
    ("nzd", "nzd"),
    ("sgd", "sgd"),
    ("hkd", "hkd"),
    ("sek", "sek"),
    ("nok", "nok"),
    ("dkk", "dkk"),
    ("pln", "pln"),
    ("mxn", "mxn"),
    ("brl", "brl"),
    ("zar", "zar"),
    ("dollars", "dollar"),
    ("dollar", "dollar"),
    ("bucks", "dollar"),
    ("euros", "eur"),
    ("euro", "eur"),
    ("pounds", "gbp"),
    ("pound", "gbp"),
    ("quid", "gbp"),
    ("francs", "chf"),
    ("franc", "chf"),
    ("yen", "yen"),
    ("rupees", "inr"),
    ("rupee", "inr"),
];

/// Words after a number that multiply it.
const SCALE_WORDS: &[(&str, u64)] = &[
    ("thousand", 1_000),
    ("grand", 1_000),
    ("million", 1_000_000),
    ("mn", 1_000_000),
    ("billion", 1_000_000_000),
    ("bn", 1_000_000_000),
    ("k", 1_000),
    ("m", 1_000_000),
    ("mm", 1_000_000),
];

/// The value of number text such as `1,200.50`, `12,50` or `1.5`. A comma
/// with three digits after it groups thousands; a single comma with one or
/// two digits after it is a decimal comma.
fn amount_value(raw: &str) -> Option<f64> {
    let last_dot = raw.rfind('.');
    let last_comma = raw.rfind(',');
    let cleaned: String = match (last_dot, last_comma) {
        (Some(d), Some(c)) if c > d => raw.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => raw.replace(',', ""),
        (None, Some(c)) if raw.len() - c - 1 != 3 && raw.matches(',').count() == 1 => {
            raw.replace(',', ".")
        }
        (None, Some(_)) => raw.replace(',', ""),
        _ => raw.to_owned(),
    };
    cleaned.parse().ok()
}

fn cents(value: f64) -> i128 {
    // Amounts in drafts are far below the range of i128.
    #[allow(clippy::cast_possible_truncation)]
    let c = (value * 100.0).round() as i128;
    c
}

/// A scale word at `at` (after optional spaces). Returns the factor and the
/// end. Single letters (`k`, `m`, `b`) count only right after the digits.
fn scale_at(text: &str, at: usize) -> Option<(u64, usize)> {
    let bytes = text.as_bytes();
    let start = skip_spaces(bytes, at);
    let spaced = start != at;
    SCALE_WORDS.iter().find_map(|(word, factor)| {
        let end = start + word.len();
        let fits =
            text[start..].starts_with(word) && !bytes.get(end).is_some_and(|b| is_word_byte(*b));
        let attached_only = word.len() <= 2 && !matches!(*word, "bn" | "mn");
        (fits && !(attached_only && spaced)).then_some((*factor, end))
    })
}

fn money(text: &str, out: &mut Vec<Found>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let starts_here = bytes[i].is_ascii_digit()
            && (i == 0 || !(bytes[i - 1].is_ascii_digit() || bytes[i - 1].is_ascii_alphabetic()));
        if !starts_here {
            i += 1;
            continue;
        }
        // The number: digits with `,` or `.` between digits.
        let mut j = i;
        while j < bytes.len()
            && (bytes[j].is_ascii_digit()
                || (matches!(bytes[j], b',' | b'.')
                    && bytes.get(j + 1).is_some_and(u8::is_ascii_digit)))
        {
            j += 1;
        }
        let Some(mut value) = amount_value(&text[i..j]) else {
            i = j;
            continue;
        };
        let mut end = j;
        let attached_scale = scale_at(text, j).filter(|(_, e)| !text[j..*e].starts_with(' '));
        if let Some((factor, e)) = scale_at(text, j) {
            #[allow(clippy::cast_precision_loss)]
            let factor = factor as f64;
            value *= factor;
            end = e;
        }
        // A currency before: a sign, or a code with a space.
        let before = head(text, i);
        let mut start = i;
        let mut currency = None;
        if let Some((sign, code)) = CURRENCY_SIGNS.iter().find(|(s, _)| before.ends_with(s)) {
            currency = Some(*code);
            start = before.len() - sign.len();
        } else if let Some((word, code)) =
            CURRENCY_WORDS
                .iter()
                .filter(|(w, _)| w.len() == 3)
                .find(|(w, _)| {
                    before.ends_with(w) && whole_word(bytes, before.len() - w.len(), before.len())
                })
        {
            currency = Some(*code);
            start = before.len() - word.len();
        }
        // A currency after: a sign, or a code or word.
        if currency.is_none() {
            let after = skip_spaces(bytes, end);
            if let Some((sign, code)) = CURRENCY_SIGNS
                .iter()
                .find(|(s, _)| text[after..].starts_with(s))
            {
                currency = Some(*code);
                end = after + sign.len();
            } else if let Some((word, code)) = CURRENCY_WORDS.iter().find(|(w, _)| {
                text[after..].starts_with(w) && whole_word(bytes, after, after + w.len())
            }) {
                currency = Some(*code);
                end = after + word.len();
            }
        }
        if currency.is_some() || attached_scale.is_some() {
            out.push(Found::one(
                start,
                end,
                FlagKind::Price,
                Val::Amount {
                    cents: cents(value),
                    currency,
                },
            ));
            i = end.max(i + 1);
            continue;
        }
        i = j.max(i + 1);
    }
    word_amounts(text, out);
}

fn number_word(word: &str) -> Option<u64> {
    const UNITS: &[&str] = &[
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    const TENS: &[&str] = &[
        "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];
    if let Some(n) = UNITS.iter().position(|w| *w == word) {
        return u64::try_from(n).ok();
    }
    TENS.iter()
        .position(|w| *w == word)
        .and_then(|n| u64::try_from(n * 10 + 20).ok())
}

/// "five hundred dollars", "a thousand euros", "two million dollars",
/// "twenty-five thousand USD".
fn word_amounts(text: &str, out: &mut Vec<Found>) {
    // The words with their byte ranges; a hyphen splits words too.
    let mut words: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c.is_ascii_alphanumeric() {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            words.push((s, i));
        }
    }
    if let Some(s) = start {
        words.push((s, text.len()));
    }
    for (k, (ws, we)) in words.iter().enumerate() {
        let Some((_, code)) = CURRENCY_WORDS.iter().find(|(w, _)| *w == &text[*ws..*we]) else {
            continue;
        };
        // Walk back over number words.
        let mut first = k;
        while first > 0 {
            let (s, e) = words[first - 1];
            let w = &text[s..e];
            let joined_ok = text[e..words[first].0]
                .chars()
                .all(|c| c == ' ' || c == '-');
            let is_number = number_word(w).is_some()
                || [
                    "hundred", "thousand", "million", "billion", "and", "a", "an",
                ]
                .contains(&w);
            if !is_number || !joined_ok {
                break;
            }
            first -= 1;
        }
        // "and" at the start is not part of the number.
        while first < k && &text[words[first].0..words[first].1] == "and" {
            first += 1;
        }
        if first == k {
            continue;
        }
        let mut total: u64 = 0;
        let mut current: u64 = 0;
        let mut any = false;
        for (s, e) in &words[first..k] {
            let w = &text[*s..*e];
            match w {
                "a" | "an" => current = current.max(1),
                "and" => {}
                "hundred" => {
                    current = current.max(1).saturating_mul(100);
                    any = true;
                }
                "thousand" | "million" | "billion" => {
                    let scale = match w {
                        "thousand" => 1_000,
                        "million" => 1_000_000,
                        _ => 1_000_000_000,
                    };
                    total = total.saturating_add(current.max(1).saturating_mul(scale));
                    current = 0;
                    any = true;
                }
                other => {
                    if let Some(n) = number_word(other) {
                        current = current.saturating_add(n);
                        any = true;
                    }
                }
            }
        }
        if !any {
            continue;
        }
        let amount = total.saturating_add(current);
        out.push(Found::one(
            words[first].0,
            *we,
            FlagKind::Price,
            Val::Amount {
                cents: i128::from(amount) * 100,
                currency: Some(code),
            },
        ));
    }
}

// Agreements -----------------------------------------------------------------

/// Phrases that agree, accept or confirm in the user's name. The scan
/// compares their canonical form (see [`canonical_phrase`]).
const AGREEMENTS: &[&str] = &[
    "yes",
    "works for me",
    "work for me",
    "works for us",
    "that works",
    "this works",
    "i agree",
    "we agree",
    "agreed",
    "confirmed",
    "i confirm",
    "we confirm",
    "approved",
    "i approve",
    "go ahead",
    "see you",
    "see you there",
    "sounds good",
    "sounds great",
    "sounds perfect",
    "it's a deal",
    "we have a deal",
    "you have a deal",
    "deal",
    "i accept",
    "we accept",
    "accepted",
    "count me in",
    "count us in",
    "i'll be there",
    "we'll be there",
    "i'm in",
    "we're in",
    "let's do it",
    "happy to",
    "glad to",
    "consider it done",
    "will do",
    "no problem",
    "of course",
    "absolutely",
    "i'll pay",
    "we'll pay",
    "i'll sign",
    "we'll sign",
    "booked",
];

/// The canonical form of an agreement: long forms become contractions, so
/// "I will be there" and "I'll be there" are the same.
fn canonical_phrase(text: &str) -> String {
    let mut out = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (long, short) in [
        ("i will", "i'll"),
        ("we will", "we'll"),
        ("let us", "let's"),
        ("it is", "it's"),
        ("we are", "we're"),
        ("i am", "i'm"),
        ("that is", "that's"),
    ] {
        out = out.replace(long, short);
    }
    out
}

fn agreements(text: &str, out: &mut Vec<Found>) {
    let canonical: Vec<String> = AGREEMENTS.iter().map(|p| canonical_phrase(p)).collect();
    let long_forms = [
        ("i'll", "i will"),
        ("we'll", "we will"),
        ("let's", "let us"),
        ("it's", "it is"),
        ("we're", "we are"),
        ("i'm", "i am"),
    ];
    for phrase in &canonical {
        let mut forms = vec![phrase.clone()];
        for (short, long) in long_forms {
            if phrase.contains(short) {
                forms.push(phrase.replace(short, long));
            }
        }
        for form in forms {
            for (start, end) in find_words(text, &form) {
                out.push(Found::one(
                    start,
                    end,
                    FlagKind::Agreement,
                    Val::Phrase(phrase.clone()),
                ));
            }
        }
    }
}

// Joining ----------------------------------------------------------------------

/// Matches that cover others are kept; a match inside another goes.
fn without_nested(mut found: Vec<Found>) -> Vec<Found> {
    found.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
    let mut out: Vec<Found> = Vec::new();
    for f in found {
        if out.iter().any(|o| o.start <= f.start && f.end <= o.end) {
            continue;
        }
        // Overlaps without nesting: the earlier match wins.
        if out.iter().any(|o| o.start < f.end && f.start < o.end) {
            continue;
        }
        out.push(f);
    }
    out
}

/// Joins a weekday or date with a time next to it ("Friday at 3 PM") into
/// one date.
fn merged(text: &str, found: Vec<Found>) -> Vec<Found> {
    let joins = [
        "", " ", ", ", " at ", " on ", " by ", " the ", " @ ", ",", " around ", " from ",
    ];
    let timeish = |k: FlagKind| matches!(k, FlagKind::Date | FlagKind::Weekday | FlagKind::Time);
    let mut out: Vec<Found> = Vec::new();
    for f in found {
        if let Some(last) = out.last_mut()
            && timeish(last.kind)
            && timeish(f.kind)
            && f.start >= last.end
            && joins.contains(&&text[last.end..f.start])
        {
            let kinds = [last.kind, f.kind];
            last.end = f.end;
            last.kind = if kinds.contains(&FlagKind::Date)
                || (kinds.contains(&FlagKind::Weekday) && kinds.contains(&FlagKind::Time))
            {
                FlagKind::Date
            } else {
                kinds[0]
            };
            last.parts.extend(f.parts);
            continue;
        }
        out.push(f);
    }
    out
}

/// Every commitment in a normalized text, in normalized offsets.
fn find_all(text: &str) -> Vec<Found> {
    let mut found = Vec::new();
    weekdays(text, &mut found);
    relative_days(text, &mut found);
    month_dates(text, &mut found);
    numeric_dates(text, &mut found);
    money(text, &mut found);
    times(text, &mut found);
    agreements(text, &mut found);
    merged(text, without_nested(found))
}

/// Every value in a request, for comparing.
fn request_values(request: &str) -> Vec<Val> {
    let norm = Norm::new(request);
    find_all(&norm.text)
        .into_iter()
        .flat_map(|f| f.parts.into_iter().flatten())
        .collect()
}

/// A phrase that the scan found in an original text.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: FlagKind,
}

/// Every commitment phrase in a text, in original offsets: dates, times,
/// weekdays, money and agreements.
#[cfg(test)]
pub(crate) fn commitments(text: &str) -> Vec<Span> {
    let norm = Norm::new(text);
    find_all(&norm.text)
        .into_iter()
        .map(|f| {
            let (start, end) = norm.original(f.start, f.end);
            Span {
                start,
                end,
                kind: f.kind,
            }
        })
        .collect()
}

// Hard rules -------------------------------------------------------------------

const PROMISE_WORDS: &[&str] = &[
    " will ",
    "'ll ",
    " by ",
    "promise",
    "guarantee",
    " until ",
    " before ",
    " no later than ",
];

/// What a hard rule checks.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RuleCheck {
    /// No promise of a date or a time.
    Dates,
    /// No price.
    Prices,
    /// No agreement to a meeting.
    Meetings,
    /// No emoji.
    Emoji,
    /// No exclamation marks.
    Exclamations,
    /// None of these phrases.
    Phrases(Vec<String>),
}

fn quoted_phrases(rule: &str) -> Vec<String> {
    let mut out = Vec::new();
    let normalized = Norm::new(rule).text;
    let mut parts = normalized.split('"');
    parts.next();
    while let Some(inside) = parts.next() {
        let phrase = inside.trim().trim_end_matches([',', '.']).to_owned();
        if phrase.chars().count() >= 2 {
            out.push(phrase);
        }
        parts.next();
    }
    out
}

/// The checks of one hard rule. A rule that no simple pattern can check
/// gives none.
fn checks_of(rule: &str) -> Vec<RuleCheck> {
    let lower = Norm::new(rule).text;
    let negative = ["never", "do not", "don't", "no ", "avoid", "not "]
        .iter()
        .any(|w| lower.contains(w));
    if !negative {
        return Vec::new();
    }
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let has = |list: &[&str]| words.iter().any(|w| list.contains(w));
    let mut out = Vec::new();
    if has(&[
        "date",
        "dates",
        "deadline",
        "deadlines",
        "time",
        "times",
        "day",
        "days",
    ]) {
        out.push(RuleCheck::Dates);
    }
    if has(&[
        "price", "prices", "cost", "costs", "money", "amount", "amounts", "fee", "fees",
    ]) {
        out.push(RuleCheck::Prices);
    }
    if has(&["meeting", "meetings", "meet"]) {
        out.push(RuleCheck::Meetings);
    }
    if lower.contains("emoji") {
        out.push(RuleCheck::Emoji);
    }
    if lower.contains("exclamation") {
        out.push(RuleCheck::Exclamations);
    }
    let phrases = quoted_phrases(rule);
    if !phrases.is_empty() {
        out.push(RuleCheck::Phrases(phrases));
    }
    out
}

/// The sentence around a byte range: from the last `.`, `!`, `?` or line
/// break before it to the next one after it.
fn sentence_of(text: &str, start: usize, end: usize) -> &str {
    let is_stop = |c: char| matches!(c, '.' | '!' | '?' | '\n');
    let from = text[..start].rfind(is_stop).map_or(0, |i| i + 1);
    let to = text[end..].find(is_stop).map_or(text.len(), |i| end + i);
    &text[from..to]
}

fn is_emoji(c: char) -> bool {
    matches!(u32::from(c), 0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0x1F000..=0x1F2FF)
}

/// The ranges of one normalized text that break one check, in normalized
/// offsets.
fn breaks(norm: &str, check: &RuleCheck, found: &[Found]) -> Vec<(usize, usize)> {
    match check {
        RuleCheck::Dates => found
            .iter()
            .filter(|s| matches!(s.kind, FlagKind::Date | FlagKind::Weekday | FlagKind::Time))
            .filter(|s| {
                let sentence = format!(" {} ", sentence_of(norm, s.start, s.end));
                PROMISE_WORDS.iter().any(|w| sentence.contains(w))
            })
            .map(|s| (s.start, s.end))
            .collect(),
        RuleCheck::Prices => found
            .iter()
            .filter(|s| s.kind == FlagKind::Price)
            .map(|s| (s.start, s.end))
            .collect(),
        RuleCheck::Meetings => found
            .iter()
            .filter(|s| s.kind == FlagKind::Agreement)
            .filter(|s| {
                let sentence = sentence_of(norm, s.start, s.end);
                ["meet", "call", "calendar", "invite", "see you"]
                    .iter()
                    .any(|w| sentence.contains(w))
            })
            .map(|s| (s.start, s.end))
            .collect(),
        RuleCheck::Emoji => norm
            .char_indices()
            .filter(|(_, c)| is_emoji(*c))
            .map(|(i, c)| (i, i + c.len_utf8()))
            .collect(),
        RuleCheck::Exclamations => norm
            .char_indices()
            .filter(|(_, c)| *c == '!')
            .map(|(i, _)| (i, i + 1))
            .collect(),
        RuleCheck::Phrases(phrases) => phrases.iter().flat_map(|p| find_words(norm, p)).collect(),
    }
}

// The scan ---------------------------------------------------------------------

/// One field of a draft that the scan reads.
pub(crate) struct ScanField<'a> {
    /// The argument key, or the path of a nested value such as
    /// `blocks.0.text.text`.
    pub key: &'a str,
    /// The label on the card.
    pub label: &'a str,
    /// The text.
    pub value: &'a str,
}

/// The flags of a draft: commitments whose value `request` does not hold,
/// and text that breaks a checkable hard rule.
pub(crate) fn scan(
    fields: &[ScanField<'_>],
    request: &str,
    hard_rules: &[VoiceRule],
) -> Vec<CommitmentFlag> {
    let wanted = request_values(request);
    let checks: Vec<(String, RuleCheck)> = hard_rules
        .iter()
        .flat_map(|r| {
            checks_of(&r.text)
                .into_iter()
                .map(|c| (r.text.clone(), c))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut flags = Vec::new();
    for field in fields {
        let norm = Norm::new(field.value);
        let found = find_all(&norm.text);
        let mut taken: Vec<(usize, usize)> = Vec::new();
        let mut flag = |start: usize, end: usize, kind: FlagKind, rule: Option<String>| {
            let (start, end) = norm.original(start, end);
            if start >= end {
                return;
            }
            flags.push(CommitmentFlag {
                field: field.key.to_owned(),
                label: field.label.to_owned(),
                text: field.value[start..end].to_owned(),
                kind,
                start,
                end,
                rule,
            });
        };
        for (rule, check) in &checks {
            for (start, end) in breaks(&norm.text, check, &found) {
                if taken.iter().any(|(s, e)| *s < end && start < *e) {
                    continue;
                }
                taken.push((start, end));
                flag(start, end, FlagKind::HardRule, Some(rule.clone()));
            }
        }
        for f in found {
            if taken.iter().any(|(s, e)| *s < f.end && f.start < *e) {
                continue;
            }
            let in_request = f.parts.iter().all(|alternatives| {
                alternatives
                    .iter()
                    .any(|v| wanted.iter().any(|w| same(v, w)))
            });
            if in_request {
                continue;
            }
            flag(f.start, f.end, f.kind, None);
        }
    }
    flags.sort_by_key(|f| {
        (
            fields
                .iter()
                .position(|x| x.key == f.field)
                .unwrap_or(usize::MAX),
            f.start,
        )
    });
    flags
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
