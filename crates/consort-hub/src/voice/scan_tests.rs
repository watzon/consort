//! Tests of the commitment scan: one table for each format, and the
//! comparison by value.

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

/// The flagged phrases of `draft` against `request`.
fn flagged(draft: &str, request: &str) -> Vec<String> {
    scan(&body(draft), request, &[])
        .into_iter()
        .map(|f| f.text)
        .collect()
}

/// Each row: a text, and the one phrase and kind that the scan finds.
fn check_table(kind: FlagKind, rows: &[(&str, &str)]) {
    for (text, want) in rows {
        let found = texts(text);
        assert_eq!(
            found,
            vec![((*want).to_owned(), kind)],
            "in {text:?}, want {want:?}"
        );
    }
}

#[test]
fn it_finds_every_time_format() {
    check_table(
        FlagKind::Time,
        &[
            ("Call me at 3pm.", "3pm"),
            ("Call me at 3 PM.", "3 PM"),
            ("Call me at 3\u{202F}PM.", "3\u{202F}PM"),
            ("Call me at 3\u{a0}pm.", "3\u{a0}pm"),
            ("Call me at 3:30pm.", "3:30pm"),
            ("Call me at 3:30 p.m. please", "3:30 p.m."),
            ("Call me at 15:30.", "15:30"),
            ("Call me at 15.30.", "15.30"),
            ("Call me at 9:30", "9:30"),
            ("Lunch at noon.", "noon"),
            ("Up until midnight.", "midnight"),
            ("I send it by EOD.", "EOD"),
            ("I send it by end of day.", "end of day"),
            ("Back at 3 o'clock.", "3 o'clock"),
            ("Back at 3 o\u{2019}clock.", "3 o\u{2019}clock"),
        ],
    );
}

#[test]
fn it_finds_every_date_format() {
    check_table(
        FlagKind::Date,
        &[
            ("Due Nov 14.", "Nov 14"),
            ("Due Nov. 14.", "Nov. 14"),
            ("Due November 14th, 2026.", "November 14th, 2026"),
            ("Due 14 November.", "14 November"),
            ("Due 14th of Nov. 2026 then.", "14th of Nov. 2026"),
            ("Due March the 3rd.", "March the 3rd"),
            ("Due 11/14.", "11/14"),
            ("Due 11/14/2026.", "11/14/2026"),
            ("Due 2026-11-14.", "2026-11-14"),
            ("Due 14.11.2026.", "14.11.2026"),
            ("Due on the 14th.", "the 14th"),
            ("I send it today.", "today"),
            ("I send it tomorrow.", "tomorrow"),
            ("I send it tonight.", "tonight"),
            ("I send it next week.", "next week"),
            ("Friday at 3 PM works.", "Friday at 3 PM"),
            ("Nov 14 at 15:30 then.", "Nov 14 at 15:30"),
        ],
    );
    check_table(
        FlagKind::Weekday,
        &[
            ("See it Tuesday.", "Tuesday"),
            ("See it this Friday.", "this Friday"),
            ("See it next Monday.", "next Monday"),
            ("See it Fri.", "Fri."),
            ("See it Thu or later.", "Thu"),
            ("See it on Sat.", "Sat."),
        ],
    );
    // Words that look like dates but are not.
    for text in [
        "You may want version 2 of the 10 files.",
        "I sat down. The sun is out.",
        "Pi is 3.14 and the ratio is 50/50.",
        "Call 555-123-4567.",
    ] {
        assert_eq!(texts(text), vec![], "{text}");
    }
}

#[test]
fn it_finds_every_money_format() {
    check_table(
        FlagKind::Price,
        &[
            ("It is $500.", "$500"),
            ("It is 500$.", "500$"),
            ("It is €500.", "€500"),
            ("It is 500€.", "500€"),
            ("It is 500 €.", "500 €"),
            ("It is USD 500.", "USD 500"),
            ("It is 500 USD.", "500 USD"),
            ("It is EUR 300.", "EUR 300"),
            ("It is CHF 900.", "CHF 900"),
            ("It is ₹5000.", "₹5000"),
            ("It is £1,200.50.", "£1,200.50"),
            ("It is 300 euros.", "300 euros"),
            ("It is 25k.", "25k"),
            ("It is 1.5M.", "1.5M"),
            ("It is $2 million.", "$2 million"),
            ("It is 2bn dollars.", "2bn dollars"),
            ("It is five hundred dollars.", "five hundred dollars"),
            (
                "It is twenty-five thousand euros.",
                "twenty-five thousand euros",
            ),
            ("It is a hundred bucks.", "a hundred bucks"),
        ],
    );
    assert_eq!(texts("Pick option 2b on page 3."), vec![]);
}

#[test]
fn it_finds_every_agreement_with_any_apostrophe() {
    check_table(
        FlagKind::Agreement,
        &[
            ("I'll be there.", "I'll be there"),
            ("I\u{2019}ll be there.", "I\u{2019}ll be there"),
            ("I will be there.", "I will be there"),
            ("Count me in.", "Count me in"),
            ("Sounds good.", "Sounds good"),
            ("Deal.", "Deal"),
            ("Confirmed.", "Confirmed"),
            ("Happy to.", "Happy to"),
            ("I accept.", "I accept"),
            ("We\u{2019}re in.", "We\u{2019}re in"),
            ("Yes.", "Yes"),
            ("Approved.", "Approved"),
            ("Go ahead.", "Go ahead"),
            ("It works for me.", "works for me"),
        ],
    );
    assert_eq!(texts("The ideal plan is ready."), vec![]);
}

#[test]
fn the_request_is_compared_by_value() {
    // A different amount, day or time is flagged, even when the request
    // holds the draft's text as a substring.
    assert_eq!(flagged("It is $50.", "Offer $500."), vec!["$50"]);
    assert_eq!(flagged("It is $1.", "Offer $1,000."), vec!["$1"]);
    assert_eq!(flagged("Due March 1.", "Due March 14."), vec!["March 1"]);
    assert_eq!(flagged("At 3:00 pm.", "At 13:00 pm."), vec!["3:00 pm"]);
    assert_eq!(flagged("Deal.", "That is the ideal plan."), vec!["Deal"]);
    assert_eq!(
        flagged("See you.", "Did you see your mail?"),
        vec!["See you"]
    );
    // The same value in another form is not flagged.
    for (draft, request) in [
        ("It is $500.", "Offer 500 USD."),
        ("It is 25k.", "Offer $25,000."),
        ("It is 1.5M EUR.", "Offer €1,500,000."),
        ("It is five hundred dollars.", "Offer $500."),
        (
            "Friday at 3 PM works for me.",
            "Say Friday at 15:00 works for me.",
        ),
        ("At 3pm.", "At 3 o'clock."),
        ("Due Nov. 14.", "Due 11/14."),
        ("Due 2026-11-14.", "Due 14 November."),
        ("Due on the 14th.", "Due Nov 14."),
        ("See you this Friday.", "see you Friday"),
        ("I\u{2019}ll be there.", "Tell them I will be there."),
        ("Yes, sounds good.", "Say yes, sounds good."),
        ("By EOD.", "by close of business"),
    ] {
        assert_eq!(
            flagged(draft, request),
            Vec::<String>::new(),
            "{draft} / {request}"
        );
    }
    // Another currency is another amount.
    assert_eq!(flagged("It is €500.", "Offer $500."), vec!["€500"]);
    // "next Friday" is not "this Friday".
    assert_eq!(
        flagged("See you next Friday.", "see you this Friday"),
        vec!["next Friday"]
    );
}

#[test]
fn an_empty_request_flags_every_match() {
    let flags = flagged(
        "I will send the signed contract at 3pm and pay USD 4,000. Confirmed.",
        "",
    );
    assert_eq!(flags, vec!["3pm", "USD 4,000", "Confirmed"]);
}

#[test]
fn a_phrase_in_the_request_is_not_flagged() {
    let text = "Hi Dana,\nFriday at 3 PM works for me. It is $500.";
    let flags = scan(
        &body(text),
        "Reply to Dana. Say Friday at 3 pm is fine.",
        &[],
    );
    let found: Vec<&str> = flags.iter().map(|f| f.text.as_str()).collect();
    assert_eq!(found, vec!["works for me", "$500"]);
    assert_eq!(flags[0].note(), "Not in your request. Check it.");
    assert_eq!(&text[flags[1].start..flags[1].end], "$500");
}

#[test]
fn offsets_point_into_the_original_text() {
    let text = "Caf\u{e9}  \u{201C}ok\u{201D}: I\u{2019}ll be there at 3\u{a0}PM.";
    let flags = scan(&body(text), "", &[]);
    let found: Vec<&str> = flags.iter().map(|f| &text[f.start..f.end]).collect();
    assert_eq!(found, vec!["I\u{2019}ll be there", "3\u{a0}PM"]);
    assert!(flags.iter().all(|f| text[f.start..f.end] == f.text));
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
            text: "Do not use \u{201C}circle back\u{201D}.".into(),
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
        &body("Let me Circle back 🙂. Sounds good, let's meet."),
        "",
        &rules,
    );
    let kinds: Vec<(&str, FlagKind)> = flags.iter().map(|f| (f.text.as_str(), f.kind)).collect();
    assert_eq!(
        kinds,
        vec![
            ("Circle back", FlagKind::HardRule),
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
