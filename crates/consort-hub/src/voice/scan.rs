//! The commitment scan: dates, times, weekdays, money amounts and agreement
//! phrases in a draft that the request did not contain, and text that
//! breaks a hard rule of the style guide that a simple pattern can check.
//!
//! The scan works on ASCII-lowercase text, which keeps every byte offset,
//! so a flag's `start` and `end` point into the field value as it is.

use consort_proto::model::{CommitmentFlag, FlagKind, VoiceRule};

/// A phrase that the scan found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: FlagKind,
}

const WEEKDAYS: &[&str] = &[
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];
const RELATIVE_DAYS: &[&str] = &[
    "tomorrow",
    "tonight",
    "today",
    "next week",
    "this week",
    "next month",
    "end of day",
    "end of the day",
    "end of the week",
    "end of the month",
    "eod",
    "eow",
];
const MONTHS: &[&str] = &[
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "jan",
    "feb",
    "mar",
    "apr",
    "jun",
    "jul",
    "aug",
    "sep",
    "sept",
    "oct",
    "nov",
    "dec",
];
const AGREEMENTS: &[&str] = &[
    "works for me",
    "work for me",
    "i agree",
    "agreed",
    "confirmed",
    "i confirm",
    "see you",
    "sounds good",
    "it's a deal",
    "it is a deal",
    "deal",
    "i accept",
    "count me in",
    "i'll be there",
    "i will be there",
    "let's do it",
    "let us do it",
    "we have a deal",
];
const CURRENCY_WORDS: &[&str] = &[
    "usd", "eur", "gbp", "dollars", "dollar", "euros", "euro", "pounds", "bucks", "cad", "aud",
];
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

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'\''
}

/// Whether `lower[start..end]` stands as whole words.
fn whole_word(lower: &[u8], start: usize, end: usize) -> bool {
    let before = start == 0 || !is_word_byte(lower[start - 1]);
    let after = end >= lower.len() || !is_word_byte(lower[end]);
    before && after
}

/// Every whole-word place of `needle` in `lower`.
fn find_words(lower: &str, needle: &str) -> Vec<(usize, usize)> {
    let bytes = lower.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find(needle) {
        let start = from + at;
        let end = start + needle.len();
        if whole_word(bytes, start, end) {
            out.push((start, end));
        }
        from = start + needle.len().max(1);
        while from < lower.len() && !lower.is_char_boundary(from) {
            from += 1;
        }
    }
    out
}

/// The end of a run of ASCII digits from `at`.
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

/// A day number with an optional ordinal: `14`, `14th`, `1st`.
fn day_number(bytes: &[u8], at: usize) -> Option<usize> {
    let end = digits_end(bytes, at);
    if end == at || end - at > 2 {
        return None;
    }
    let mut end = end;
    for suffix in [b"st", b"nd", b"rd", b"th"] {
        if bytes.len() >= end + 2 && &bytes[end..end + 2] == suffix {
            end += 2;
            break;
        }
    }
    (end >= bytes.len() || !bytes[end].is_ascii_alphanumeric()).then_some(end)
}

fn dates(lower: &str, out: &mut Vec<Span>) {
    let bytes = lower.as_bytes();
    for day in RELATIVE_DAYS {
        for (start, end) in find_words(lower, day) {
            out.push(Span {
                start,
                end,
                kind: FlagKind::Date,
            });
        }
    }
    for day in WEEKDAYS {
        for (start, end) in find_words(lower, day) {
            out.push(Span {
                start,
                end,
                kind: FlagKind::Weekday,
            });
        }
    }
    // A month with a day after it ("March 3", "March the 3rd") or before
    // it ("3 March", "3rd of March").
    for month in MONTHS {
        for (start, end) in find_words(lower, month) {
            let after = skip_spaces(bytes, end);
            let after = if lower[after..].starts_with("the ") {
                after + 4
            } else {
                after
            };
            if let Some(day_end) = day_number(bytes, after) {
                out.push(Span {
                    start,
                    end: day_end,
                    kind: FlagKind::Date,
                });
                continue;
            }
            // Before: "<n> " or "<n>th of ".
            let head = lower[..start].trim_end();
            let head = head.strip_suffix(" of").unwrap_or(head).trim_end();
            let head_bytes = head.as_bytes();
            let mut i = head_bytes.len();
            while i > 0 && head_bytes[i - 1].is_ascii_alphanumeric() {
                i -= 1;
            }
            if i < head_bytes.len()
                && head_bytes[i].is_ascii_digit()
                && day_number(head_bytes, i) == Some(head_bytes.len())
            {
                out.push(Span {
                    start: i,
                    end,
                    kind: FlagKind::Date,
                });
            }
        }
    }
    // "the 14th".
    for (start, _) in find_words(lower, "the") {
        let at = start + 4;
        if at < bytes.len()
            && bytes[at].is_ascii_digit()
            && let Some(end) = day_number(bytes, at)
            && end > digits_end(bytes, at)
        {
            out.push(Span {
                start,
                end,
                kind: FlagKind::Date,
            });
        }
    }
    // Numeric dates: 3/14, 3/14/2026, 2026-03-14.
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() || (i > 0 && bytes[i - 1].is_ascii_alphanumeric()) {
            i += 1;
            continue;
        }
        let first = digits_end(bytes, i);
        let sep = bytes.get(first).copied();
        if matches!(sep, Some(b'/') | Some(b'-'))
            && bytes.get(first + 1).is_some_and(u8::is_ascii_digit)
        {
            let second = digits_end(bytes, first + 1);
            let mut end = second;
            if bytes.get(second) == sep.as_ref()
                && bytes.get(second + 1).is_some_and(u8::is_ascii_digit)
            {
                end = digits_end(bytes, second + 1);
            }
            let a = first - i;
            let b = second - first - 1;
            let plausible = (a <= 2 && b <= 2) || (a == 4 && b <= 2 && end > second);
            if plausible && (end >= bytes.len() || !bytes[end].is_ascii_alphanumeric()) {
                out.push(Span {
                    start: i,
                    end,
                    kind: FlagKind::Date,
                });
                i = end;
                continue;
            }
        }
        i = first.max(i + 1);
    }
}

fn times(lower: &str, out: &mut Vec<Span>) {
    let bytes = lower.as_bytes();
    for word in ["noon", "midnight"] {
        for (start, end) in find_words(lower, word) {
            out.push(Span {
                start,
                end,
                kind: FlagKind::Time,
            });
        }
    }
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit()
            || (i > 0
                && (bytes[i - 1].is_ascii_alphanumeric()
                    || bytes[i - 1] == b':'
                    || bytes[i - 1] == b'$'))
        {
            i += 1;
            continue;
        }
        let hour_end = digits_end(bytes, i);
        if hour_end - i > 2 {
            i = hour_end;
            continue;
        }
        let mut end = hour_end;
        let mut has_minutes = false;
        if bytes.get(end) == Some(&b':')
            && bytes.get(end + 1).is_some_and(u8::is_ascii_digit)
            && digits_end(bytes, end + 1) == end + 3
        {
            end += 3;
            has_minutes = true;
        }
        let after = skip_spaces(bytes, end);
        let rest = &lower[after..];
        let meridiem = ["a.m.", "p.m.", "am", "pm"].iter().find(|m| {
            rest.starts_with(**m) && (m.ends_with('.') || whole_word(bytes, after, after + m.len()))
        });
        if let Some(m) = meridiem {
            out.push(Span {
                start: i,
                end: after + m.len(),
                kind: FlagKind::Time,
            });
            i = after + m.len();
            continue;
        }
        if has_minutes && (end >= bytes.len() || !bytes[end].is_ascii_alphanumeric()) {
            out.push(Span {
                start: i,
                end,
                kind: FlagKind::Time,
            });
            i = end;
            continue;
        }
        i = end.max(i + 1);
    }
}

fn money(lower: &str, out: &mut Vec<Span>) {
    let bytes = lower.as_bytes();
    let amount_end = |at: usize| -> usize {
        let mut j = at;
        while j < bytes.len()
            && (bytes[j].is_ascii_digit()
                || ((bytes[j] == b',' || bytes[j] == b'.')
                    && bytes.get(j + 1).is_some_and(u8::is_ascii_digit)))
        {
            j += 1;
        }
        if j < bytes.len()
            && matches!(bytes[j], b'k' | b'm')
            && !bytes.get(j + 1).is_some_and(u8::is_ascii_alphanumeric)
        {
            j += 1;
        }
        j
    };
    for symbol in ["$", "€", "£", "¥"] {
        let mut from = 0;
        while let Some(at) = lower[from..].find(symbol) {
            let start = from + at;
            let digits = skip_spaces(bytes, start + symbol.len());
            if bytes.get(digits).is_some_and(u8::is_ascii_digit) {
                let end = amount_end(digits);
                out.push(Span {
                    start,
                    end,
                    kind: FlagKind::Price,
                });
                from = end;
            } else {
                from = start + symbol.len();
            }
        }
    }
    for word in CURRENCY_WORDS {
        for (start, end) in find_words(lower, word) {
            let head = lower[..start].trim_end();
            let head_bytes = head.as_bytes();
            let mut i = head_bytes.len();
            while i > 0
                && (head_bytes[i - 1].is_ascii_digit()
                    || head_bytes[i - 1] == b','
                    || head_bytes[i - 1] == b'.'
                    || head_bytes[i - 1] == b'k')
            {
                i -= 1;
            }
            if i < head_bytes.len() && head_bytes[i..].iter().any(u8::is_ascii_digit) {
                out.push(Span {
                    start: i,
                    end,
                    kind: FlagKind::Price,
                });
            }
        }
    }
}

fn agreements(lower: &str, out: &mut Vec<Span>) {
    for phrase in AGREEMENTS {
        for (start, end) in find_words(lower, phrase) {
            out.push(Span {
                start,
                end,
                kind: FlagKind::Agreement,
            });
        }
    }
}

/// Spans that cover others are kept; a span inside another goes.
fn without_nested(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by_key(|s| (s.start, std::cmp::Reverse(s.end)));
    let mut out: Vec<Span> = Vec::new();
    for span in spans {
        if out
            .iter()
            .any(|o| o.start <= span.start && span.end <= o.end)
        {
            continue;
        }
        out.push(span);
    }
    out
}

/// Joins a weekday or date with a time next to it ("Friday at 3 PM") into
/// one date.
fn merged(lower: &str, spans: Vec<Span>) -> Vec<Span> {
    let joins = [
        "", " ", ", ", " at ", " on ", " by ", " the ", " @ ", ",", " around ",
    ];
    let timeish = |k: FlagKind| matches!(k, FlagKind::Date | FlagKind::Weekday | FlagKind::Time);
    let mut out: Vec<Span> = Vec::new();
    for span in spans {
        if let Some(last) = out.last_mut()
            && timeish(last.kind)
            && timeish(span.kind)
            && span.start >= last.end
            && joins.contains(&&lower[last.end..span.start])
        {
            let kinds = [last.kind, span.kind];
            last.end = span.end;
            last.kind = if kinds.contains(&FlagKind::Date)
                || (kinds.contains(&FlagKind::Weekday) && kinds.contains(&FlagKind::Time))
            {
                FlagKind::Date
            } else {
                kinds[0]
            };
            continue;
        }
        out.push(span);
    }
    out
}

/// Every commitment phrase in a text: dates, times, weekdays, money and
/// agreements.
pub(crate) fn commitments(text: &str) -> Vec<Span> {
    let lower = text.to_ascii_lowercase();
    let mut spans = Vec::new();
    dates(&lower, &mut spans);
    times(&lower, &mut spans);
    money(&lower, &mut spans);
    agreements(&lower, &mut spans);
    merged(&lower, without_nested(spans))
}

fn normalize(text: &str) -> String {
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

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
    let normalized = rule.replace(['“', '”'], "\"").replace(['‘', '’'], "'");
    let mut parts = normalized.split('"');
    parts.next();
    while let Some(inside) = parts.next() {
        let phrase = inside.trim().trim_end_matches([',', '.']).to_lowercase();
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
    let lower = rule.to_lowercase();
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

/// The spans of one text that break one check.
fn breaks(text: &str, check: &RuleCheck, found: &[Span]) -> Vec<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    match check {
        RuleCheck::Dates => found
            .iter()
            .filter(|s| matches!(s.kind, FlagKind::Date | FlagKind::Weekday | FlagKind::Time))
            .filter(|s| {
                let sentence = format!(" {} ", sentence_of(&lower, s.start, s.end));
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
                let sentence = sentence_of(&lower, s.start, s.end);
                ["meet", "call", "calendar", "invite", "see you"]
                    .iter()
                    .any(|w| sentence.contains(w))
            })
            .map(|s| (s.start, s.end))
            .collect(),
        RuleCheck::Emoji => text
            .char_indices()
            .filter(|(_, c)| is_emoji(*c))
            .map(|(i, c)| (i, i + c.len_utf8()))
            .collect(),
        RuleCheck::Exclamations => text
            .char_indices()
            .filter(|(_, c)| *c == '!')
            .map(|(i, _)| (i, i + 1))
            .collect(),
        RuleCheck::Phrases(phrases) => phrases
            .iter()
            .flat_map(|p| {
                let ascii = p.is_ascii();
                if ascii {
                    lower
                        .match_indices(p.as_str())
                        .map(|(i, m)| (i, i + m.len()))
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                }
            })
            .collect(),
    }
}

/// One field of a draft that the scan reads.
pub(crate) struct ScanField<'a> {
    /// The argument key.
    pub key: &'a str,
    /// The label on the card.
    pub label: &'a str,
    /// The text.
    pub value: &'a str,
}

/// The flags of a draft: commitments that `request` does not contain, and
/// text that breaks a checkable hard rule.
pub(crate) fn scan(
    fields: &[ScanField<'_>],
    request: &str,
    hard_rules: &[VoiceRule],
) -> Vec<CommitmentFlag> {
    let request = normalize(request);
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
        let found = commitments(field.value);
        let mut taken: Vec<(usize, usize)> = Vec::new();
        for (rule, check) in &checks {
            for (start, end) in breaks(field.value, check, &found) {
                if taken.iter().any(|(s, e)| *s < end && start < *e) {
                    continue;
                }
                taken.push((start, end));
                flags.push(CommitmentFlag {
                    field: field.key.to_owned(),
                    label: field.label.to_owned(),
                    text: field.value[start..end].to_owned(),
                    kind: FlagKind::HardRule,
                    start,
                    end,
                    rule: Some(rule.clone()),
                });
            }
        }
        for span in found {
            if taken.iter().any(|(s, e)| *s < span.end && span.start < *e) {
                continue;
            }
            let phrase = &field.value[span.start..span.end];
            if request.contains(&normalize(phrase)) {
                continue;
            }
            flags.push(CommitmentFlag {
                field: field.key.to_owned(),
                label: field.label.to_owned(),
                text: phrase.to_owned(),
                kind: span.kind,
                start: span.start,
                end: span.end,
                rule: None,
            });
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
mod tests {
    use super::*;

    fn texts(text: &str) -> Vec<(String, FlagKind)> {
        commitments(text)
            .into_iter()
            .map(|s| (text[s.start..s.end].to_owned(), s.kind))
            .collect()
    }

    fn body(value: &str) -> Vec<ScanField<'_>> {
        vec![ScanField {
            key: "body",
            label: "Body",
            value,
        }]
    }

    #[test]
    fn it_finds_dates_times_weekdays_money_and_agreements() {
        assert_eq!(
            texts("Thanks for the update. Friday at 3 PM works for me."),
            vec![
                ("Friday at 3 PM".to_owned(), FlagKind::Date),
                ("works for me".to_owned(), FlagKind::Agreement),
            ]
        );
        assert_eq!(
            texts("Call me Tuesday."),
            vec![("Tuesday".to_owned(), FlagKind::Weekday)]
        );
        assert_eq!(
            texts("at 10:30 or noon"),
            vec![
                ("10:30".to_owned(), FlagKind::Time),
                ("noon".to_owned(), FlagKind::Time)
            ]
        );
        assert_eq!(
            texts("On March 3rd and 14 May."),
            vec![
                ("March 3rd".to_owned(), FlagKind::Date),
                ("14 May".to_owned(), FlagKind::Date)
            ]
        );
        assert_eq!(
            texts("Due 3/14 or 2026-11-02."),
            vec![
                ("3/14".to_owned(), FlagKind::Date),
                ("2026-11-02".to_owned(), FlagKind::Date)
            ]
        );
        assert_eq!(
            texts("It costs $1,200.50 or 300 euros."),
            vec![
                ("$1,200.50".to_owned(), FlagKind::Price),
                ("300 euros".to_owned(), FlagKind::Price)
            ]
        );
        assert_eq!(
            texts("I agree. Sounds good, see you."),
            vec![
                ("I agree".to_owned(), FlagKind::Agreement),
                ("Sounds good".to_owned(), FlagKind::Agreement),
                ("see you".to_owned(), FlagKind::Agreement)
            ]
        );
        assert_eq!(texts("You may want version 2 of the 10 files."), vec![]);
        assert_eq!(texts("The ideal plan is ready."), vec![]);
    }

    #[test]
    fn a_phrase_in_the_request_is_not_flagged() {
        let flags = scan(
            &body("Hi Dana,\nFriday at 3 PM works for me. It is $500."),
            "Reply to Dana. Say Friday at 3 pm is fine.",
            &[],
        );
        let texts: Vec<&str> = flags.iter().map(|f| f.text.as_str()).collect();
        assert_eq!(texts, vec!["works for me", "$500"]);
        assert_eq!(flags[0].note(), "Not in your request. Check it.");
        let start = flags[1].start;
        assert_eq!(
            &"Hi Dana,\nFriday at 3 PM works for me. It is $500."[start..flags[1].end],
            "$500"
        );
    }

    #[test]
    fn the_hard_rule_never_promise_a_date_flags_a_promise() {
        let rule = VoiceRule {
            text: "Never promise a date or a price.".into(),
            by_user: true,
        };
        let flags = scan(
            &body("I will send it by Friday."),
            "Send the file by Friday",
            std::slice::from_ref(&rule),
        );
        assert_eq!(flags.len(), 1, "{flags:?}");
        assert_eq!(flags[0].kind, FlagKind::HardRule);
        assert_eq!(flags[0].text, "Friday");
        assert_eq!(
            flags[0].note(),
            "Breaks your rule: Never promise a date or a price."
        );
        // A date that is not a promise breaks no rule, and the request has it.
        let flags = scan(&body("Thanks for Friday."), "about Friday", &[rule]);
        assert!(flags.is_empty(), "{flags:?}");
    }

    #[test]
    fn hard_rules_on_phrases_emoji_and_meetings() {
        let rules = vec![
            VoiceRule {
                text: "Do not use \"circle back\".".into(),
                by_user: false,
            },
            VoiceRule {
                text: "Do not use emoji in mail.".into(),
                by_user: false,
            },
            VoiceRule {
                text: "Never agree to a meeting. Offer to check the calendar.".into(),
                by_user: true,
            },
            VoiceRule {
                text: "Sign with your first name.".into(),
                by_user: false,
            },
        ];
        let flags = scan(
            &body("Let me circle back 🙂. Sounds good, let's meet."),
            "",
            &rules,
        );
        let kinds: Vec<(&str, FlagKind)> =
            flags.iter().map(|f| (f.text.as_str(), f.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("circle back", FlagKind::HardRule),
                ("🙂", FlagKind::HardRule),
                ("Sounds good", FlagKind::HardRule),
            ]
        );
    }

    #[test]
    fn flags_follow_the_field_order() {
        let fields = vec![
            ScanField {
                key: "subject",
                label: "Subject",
                value: "Meeting Monday",
            },
            ScanField {
                key: "body",
                label: "Body",
                value: "Agreed.",
            },
        ];
        let flags = scan(&fields, "", &[]);
        assert_eq!(flags[0].field, "subject");
        assert_eq!(flags[1].field, "body");
        assert_eq!(flags[1].kind, FlagKind::Agreement);
    }
}
