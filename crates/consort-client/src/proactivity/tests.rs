use consort_proto::client_api::{EntityPayload, FeedChange, FeedEntry};
use consort_proto::ids::{ChannelId, MessageId};
use consort_proto::model::{
    Author, ProactiveMark, ProactiveOutcome, ProactiveWake, QuietHours, Suggestion,
};
use consort_proto::time::Timestamp;

use super::*;
use crate::notify::{
    AttentionView, Focus, IncomingMessage, NotificationKind, NotificationSettings, Notifier,
};

const NOW: i64 = 1_793_300_000_000;

fn feed(seq: u64, payload: EntityPayload) -> FeedEntry {
    FeedEntry {
        seq,
        at: Timestamp::from_millis(NOW),
        channel_id: payload.channel_id(),
        entity_id: payload.entity_id(),
        change: FeedChange::Upsert(payload),
    }
}

fn message(channel: ChannelId, author: Author, at: i64, body: &str) -> model::Message {
    model::Message {
        id: MessageId::new(),
        channel_id: channel,
        thread_root_id: None,
        author,
        author_session_id: None,
        author_task_id: None,
        author_routine_id: None,
        kind: model::MessageKind::Text,
        body: body.into(),
        attachments: Vec::new(),
        in_timeline: false,
        client_message_id: None,
        reply_count: 0,
        last_reply_at: None,
        reactions: Vec::new(),
        edited_at: None,
        deleted_at: None,
        created_at: Timestamp::from_millis(at),
        suggestions: Vec::new(),
        proactive: None,
        source_message_id: None,
    }
}

/// A cache with a channel whose whole timeline is loaded.
fn cache_with(messages: Vec<model::Message>) -> (Cache, ChannelId) {
    let id = ChannelId::new();
    let channel = crate::test_entities::channel(id, "reading");
    let mut cache = Cache::default();
    cache.channels.insert(id.to_string(), channel);
    cache.channels_loaded.insert(id.to_string());
    for m in messages {
        cache.messages.insert(m.id.to_string(), m);
    }
    (cache, id)
}

fn view(cache: &Cache) -> ViewState {
    crate::reduce::view_state(
        cache,
        crate::view::Connection::default(),
        crate::view::UserView::default(),
        NOW,
    )
}

fn timeline(cache: &Cache, id: ChannelId) -> TimelineView {
    let state = view(cache);
    state.timeline(&id.to_string()).cloned().unwrap()
}

fn messages_of(timeline: &TimelineView) -> Vec<&crate::view::MessageView> {
    timeline
        .items
        .iter()
        .filter_map(|i| match i {
            TimelineItem::Message(m) => Some(m),
            _ => None,
        })
        .collect()
}

#[test]
fn suggestions_show_only_under_the_newest_assistant_message_and_hide_on_reply() {
    let channel = ChannelId::new();
    let mut older = message(channel, Author::Assistant, NOW - 5_000, "Hello.");
    older.suggestions = vec![Suggestion::send("Old")];
    let mut newest = message(channel, Author::Assistant, NOW - 4_000, "Set it up?");
    newest.suggestions = vec![
        Suggestion::send("Yes"),
        Suggestion::type_it("I will type it"),
    ];
    let (mut cache, id) = cache_with(Vec::new());
    for m in [&mut older, &mut newest] {
        m.channel_id = id;
    }
    cache.messages.insert(older.id.to_string(), older.clone());
    cache.messages.insert(newest.id.to_string(), newest.clone());
    let tl = timeline(&cache, id);
    let shown = messages_of(&tl);
    assert!(shown[0].suggestions.is_empty(), "only the newest has chips");
    assert_eq!(
        shown[1].suggestions,
        vec![
            SuggestionView {
                label: "Yes".into(),
                type_it: false
            },
            SuggestionView {
                label: "I will type it".into(),
                type_it: true
            },
        ]
    );
    // The line "You made the channel reading." starts the history.
    assert!(matches!(
        tl.items.iter().find(|i| !matches!(i, TimelineItem::Day(_))),
        Some(TimelineItem::Notice(n)) if n.text == "You made the channel reading."
    ));

    // Any reply from the user hides them.
    let mut reply = message(id, Author::User, NOW - 3_000, "Yes");
    reply.channel_id = id;
    cache.messages.insert(reply.id.to_string(), reply);
    let tl = timeline(&cache, id);
    assert!(messages_of(&tl).iter().all(|m| m.suggestions.is_empty()));
}

#[test]
fn a_proactive_message_gets_its_mark_with_one_caption_per_group() {
    let (mut cache, id) = cache_with(Vec::new());
    let mut first = message(id, Author::Assistant, NOW - 5_000, "Your dentist moved.");
    first.proactive = Some(ProactiveMark::new(ProactiveKind::CheckIn));
    let mut second = message(id, Author::Assistant, NOW - 4_900, "I changed nothing.");
    second.proactive = Some(ProactiveMark::new(ProactiveKind::CheckIn));
    let mut follow = message(id, Author::Assistant, NOW - 4_000, "No reply yet.");
    follow.proactive = Some(ProactiveMark {
        reason: Some("You asked me to check the vendor reply today.".into()),
        ..ProactiveMark::new(ProactiveKind::FollowUp)
    });
    for m in [first, second, follow] {
        cache.messages.insert(m.id.to_string(), m);
    }
    let tl = timeline(&cache, id);
    let marks: Vec<_> = messages_of(&tl)
        .iter()
        .map(|m| m.proactive.clone().unwrap())
        .collect();
    assert_eq!(marks[0].label, "Check-in");
    assert_eq!(marks[0].icon, "eye");
    assert!(marks[0].show_caption);
    assert!(!marks[1].show_caption, "the second bubble of the check-in");
    assert_eq!(marks[2].label, "Follow-up");
    assert_eq!(marks[2].icon, "clock");
    assert_eq!(
        marks[2].reason.as_deref(),
        Some("You asked me to check the vendor reply today.")
    );
    assert_eq!(mark_label(ProactiveKind::Research, true), "Offer");
    assert_eq!(mark_icon(ProactiveKind::Research, true), "compose");
    assert_eq!(mark_icon(ProactiveKind::Brief, false), "sun");
}

fn incoming(id: &str, at: i64, proactive: Option<bool>) -> IncomingMessage {
    IncomingMessage {
        id: id.into(),
        channel_id: "chn_1".into(),
        channel_name: "house-lights".into(),
        routine: false,
        text: "Brightline replied.".into(),
        created_at: at,
        proactive,
    }
}

#[test]
fn a_proactive_message_notifies_only_with_notify_and_never_as_a_message() {
    let settings = NotificationSettings::default();
    assert!(settings.attention, "on by default");
    let focus = Focus::default();
    let mut notifier = Notifier::new();
    let mut view = AttentionView {
        loaded: true,
        ..AttentionView::default()
    };
    assert!(notifier.observe(&view, &focus, &settings).is_empty());
    view.messages = vec![
        incoming("plain", NOW + 1, None),
        incoming("quiet", NOW + 2, Some(false)),
        incoming("loud", NOW + 3, Some(true)),
    ];
    let out = notifier.observe(&view, &focus, &settings);
    let kinds: Vec<_> = out.iter().map(|n| (n.tag.clone(), n.kind)).collect();
    assert_eq!(
        kinds,
        vec![
            ("consort:message:plain".into(), NotificationKind::Message),
            ("consort:message:loud".into(), NotificationKind::Attention),
        ],
        "a proactive message without notify never notifies"
    );
    assert_eq!(out[1].body, "The assistant needs your attention.");

    // The switch "When the assistant needs your attention" turns them off,
    // and the switch for messages does not turn them on.
    let off = NotificationSettings {
        attention: false,
        ..NotificationSettings::default()
    };
    view.messages.push(incoming("loud-2", NOW + 4, Some(true)));
    assert!(notifier.observe(&view, &focus, &off).is_empty());
}

#[test]
fn nothing_notifies_in_quiet_hours_or_while_snoozed() {
    let settings = NotificationSettings::default();
    let mut notifier = Notifier::new();
    let mut view = AttentionView {
        loaded: true,
        ..AttentionView::default()
    };
    notifier.observe(&view, &Focus::default(), &settings);
    view.quiet = true;
    view.messages = vec![
        incoming("loud", NOW + 3, Some(true)),
        incoming("plain", NOW + 4, None),
    ];
    assert!(
        notifier
            .observe(&view, &Focus::default(), &settings)
            .is_empty()
    );

    // The quiet rule reads the hub's settings in the user's time zone.
    let mut hub = ProactivitySettings {
        time_zone: Some("America/Chicago".into()),
        quiet_hours: QuietHours {
            start: "21:00".into(),
            end: "07:00".into(),
        },
        ..ProactivitySettings::default()
    };
    let at = |s: &str| Timestamp::parse_rfc3339(s).unwrap().as_millis();
    assert!(
        quiet_now(Some(&hub), at("2026-10-30T03:30:00Z")),
        "22:30 in Chicago"
    );
    assert!(!quiet_now(Some(&hub), at("2026-10-30T17:00:00Z")), "noon");
    hub.snoozed_until = Some(Timestamp::from_millis(at("2026-10-30T18:00:00Z")));
    assert!(quiet_now(Some(&hub), at("2026-10-30T17:00:00Z")), "snoozed");
    assert!(!quiet_now(None, at("2026-10-30T03:30:00Z")), "an older hub");
}

#[test]
fn the_writing_bubble_shows_while_the_reply_is_owed() {
    let (mut cache, id) = cache_with(Vec::new());
    let mut state = view(&cache);
    // A new channel with no message: the bubble shows while its assistant
    // works on the greeting.
    assert!(!is_writing(&state, &id.to_string()));
    state.activity.retain(|a| a.channel_id != id.to_string());
    state.activity.push(crate::activity::ChannelActivity {
        channel_id: id.to_string(),
        activity: crate::activity::ActivityView {
            state: crate::activity::MarkState::Working,
            line: Some("Writing".into()),
        },
    });
    assert!(is_writing(&state, &id.to_string()));
    // The user's message reached the hub: the bubble shows.
    let mine = message(id, Author::User, NOW - 1_000, "Yes");
    cache.messages.insert(mine.id.to_string(), mine);
    let state = view(&cache);
    assert!(is_writing(&state, &id.to_string()));
    // The assistant posted: no bubble.
    let answer = message(id, Author::Assistant, NOW - 500, "Good.");
    cache.messages.insert(answer.id.to_string(), answer);
    let state = view(&cache);
    assert!(!is_writing(&state, &id.to_string()));
}

#[test]
fn a_failed_turn_ends_the_writing_bubble_and_other_notices_do_not() {
    let (mut cache, id) = cache_with(Vec::new());
    let notice = |kind: &str, at: i64| {
        let body = serde_json::json!({ "kind": kind, "text": "Some text" }).to_string();
        let mut m = message(id, Author::System, at, &body);
        m.kind = consort_proto::model::MessageKind::Notice;
        m
    };
    let mine = message(id, Author::User, NOW - 4_000, "Yes");
    cache.messages.insert(mine.id.to_string(), mine);
    // A notice during the turn: the reply is still owed.
    let saved = notice("memory_saved", NOW - 3_000);
    cache.messages.insert(saved.id.to_string(), saved);
    assert!(is_writing(&view(&cache), &id.to_string()));
    // The turn failed: nothing more comes.
    let failed = notice("turn_failed", NOW - 2_000);
    cache.messages.insert(failed.id.to_string(), failed);
    assert!(!is_writing(&view(&cache), &id.to_string()));
    // The user tries again: a new reply is owed.
    let again = message(id, Author::User, NOW - 1_000, "Try again");
    cache.messages.insert(again.id.to_string(), again);
    assert!(is_writing(&view(&cache), &id.to_string()));
}

fn thread_of(
    status: crate::view::TaskStatus,
    posts: Vec<crate::view::ThreadPost>,
) -> crate::view::TaskThreadView {
    crate::view::TaskThreadView {
        task_id: "tsk_1".into(),
        status,
        posts,
        ..crate::view::TaskThreadView::default()
    }
}

fn notice_post(quiet: bool) -> crate::view::ThreadPost {
    let card = crate::turn_errors::ErrorCardView {
        quiet,
        ..crate::turn_errors::ErrorCardView::default()
    };
    crate::view::ThreadPost {
        id: "pst_failed".into(),
        speaker: Speaker::System,
        time_label: "9:00 AM".into(),
        bodies: vec![crate::view::PostBody::Notice {
            kind: "turn_failed".into(),
            text: "The turn failed.".into(),
            error: Some(Box::new(card)),
            draft: None,
            fallback: None,
            monid: None,
        }],
        pending: false,
        source_message_id: None,
        refused: None,
    }
}

fn text_post(speaker: Speaker) -> crate::view::ThreadPost {
    crate::view::ThreadPost {
        id: "pst_text".into(),
        speaker,
        time_label: "9:00 AM".into(),
        bodies: vec![crate::view::PostBody::Text("Reading the plan.".into())],
        pending: false,
        source_message_id: None,
        refused: None,
    }
}

#[test]
fn the_writing_bubble_shows_in_a_task_thread_only_while_the_task_works() {
    use crate::view::TaskStatus as S;
    // A working task shows it, with posts or with none, under the newest
    // message of the task or of the user.
    assert!(is_task_writing(&thread_of(S::Working, Vec::new())));
    assert!(is_task_writing(&thread_of(
        S::Working,
        vec![text_post(Speaker::Task)]
    )));
    assert!(is_task_writing(&thread_of(
        S::Working,
        vec![text_post(Speaker::You)]
    )));
    // Every other status means that the turn ended or waits for the user.
    for status in [S::UpNext, S::NeedsInput, S::Review, S::Failed, S::Resolved] {
        assert!(
            !is_task_writing(&thread_of(status, Vec::new())),
            "{status:?} shows no bubble"
        );
    }
}

#[test]
fn a_live_failure_notice_ends_the_wait_and_a_quiet_one_does_not() {
    use crate::view::TaskStatus as S;
    // The notice came before the status moved to Failed: no more text comes.
    assert!(!is_task_writing(&thread_of(
        S::Working,
        vec![text_post(Speaker::Task), notice_post(false)]
    )));
    // The user tried again: the old notice is quiet and a new turn runs.
    assert!(is_task_writing(&thread_of(
        S::Working,
        vec![notice_post(true)]
    )));
    // A newer post after a live notice means that the turn went on.
    assert!(is_task_writing(&thread_of(
        S::Working,
        vec![notice_post(false), text_post(Speaker::Task)]
    )));
}

#[test]
fn the_cache_takes_settings_and_log_entries_from_the_feed() {
    let channel = ChannelId::new();
    let mut cache = Cache::default();
    let settings = ProactivitySettings {
        paused: true,
        ..ProactivitySettings::default()
    };
    assert!(cache.apply(feed(
        1,
        EntityPayload::ProactivitySettings(settings.clone())
    )));
    assert!(!cache.apply(feed(
        2,
        EntityPayload::ProactivitySettings(settings.clone())
    )));
    assert_eq!(cache.proactivity.settings, Some(settings));
    let wake = ProactiveWake {
        id: 3,
        channel_id: channel,
        kind: ProactiveKind::CheckIn,
        at: Timestamp::from_millis(NOW),
        outcome: ProactiveOutcome::Pending,
        detail: None,
        message_ids: Vec::new(),
        task_id: None,
        routine_id: None,
        notify_held: false,
        ended_at: None,
    };
    assert!(cache.apply(feed(3, EntityPayload::ProactiveWake(wake.clone()))));
    let silent = ProactiveWake {
        outcome: ProactiveOutcome::Silent,
        ..wake
    };
    assert!(cache.apply(feed(4, EntityPayload::ProactiveWake(silent))));
    assert_eq!(
        cache.proactivity.wakes[&3].outcome,
        ProactiveOutcome::Silent
    );
}

fn wake(
    id: u64,
    channel: ChannelId,
    kind: ProactiveKind,
    at: i64,
    outcome: ProactiveOutcome,
) -> ProactiveWake {
    ProactiveWake {
        id,
        channel_id: channel,
        kind,
        at: Timestamp::from_millis(at),
        outcome,
        detail: Some(format!("detail {id}")),
        message_ids: vec![MessageId::new()],
        task_id: None,
        routine_id: None,
        notify_held: false,
        ended_at: None,
    }
}

#[test]
fn the_settings_page_counts_today_and_groups_the_log() {
    let (mut cache, id) = cache_with(Vec::new());
    cache.proactivity.settings = Some(ProactivitySettings {
        time_zone: Some("UTC".into()),
        ..ProactivitySettings::default()
    });
    let day = start_of_day(cache.proactivity.settings.as_ref(), NOW);
    for w in [
        wake(
            1,
            id,
            ProactiveKind::CheckIn,
            day - 3_600_000,
            ProactiveOutcome::Posted,
        ),
        wake(
            2,
            id,
            ProactiveKind::CheckIn,
            day + 1_000,
            ProactiveOutcome::SkippedNoChange,
        ),
        wake(
            3,
            id,
            ProactiveKind::Brief,
            day + 2_000,
            ProactiveOutcome::Notified,
        ),
        wake(
            4,
            id,
            ProactiveKind::FollowUp,
            day + 3_000,
            ProactiveOutcome::Silent,
        ),
    ] {
        cache.proactivity.wakes.insert(w.id, w);
    }
    let view = proactivity_view(&cache, NOW);
    assert!(view.available);
    assert_eq!(view.turns_today, 3);
    assert_eq!(
        view.turns_line,
        "2 ran a model. 1 was skipped before a model ran."
    );
    assert_eq!((view.notified_today, view.notify_cap), (1, 3));
    assert_eq!(
        view.channels[0].meaning,
        "Follow-ups it set, nudges for stale tasks, the start of a new channel."
    );
    assert!(view.channels[0].is_default);
    let rows: Vec<_> = view.log.iter().flat_map(|g| g.rows.iter()).collect();
    assert_eq!(rows[0].id, 4, "newest first");
    assert_eq!(rows[0].result, "Stayed silent");
    assert_eq!(rows[0].tone, ResultTone::Quiet);
    assert!(
        rows[0].message.is_none(),
        "a silent wake has no message to open"
    );
    assert_eq!(rows[1].tone, ResultTone::Notified);
    assert!(rows[1].message.is_some());
    assert_eq!(rows[2].result, "Skipped: nothing changed");
    // The filters.
    let briefs = log_groups(&cache, None, Some(ProactiveKind::Brief), NOW);
    assert_eq!(briefs.iter().map(|g| g.rows.len()).sum::<usize>(), 1);
    let none = log_groups(&cache, Some("chn_other"), None, NOW);
    assert!(none.is_empty());
    // An older hub sends no settings: the page says so.
    assert!(!proactivity_view(&Cache::default(), NOW).available);
}

#[test]
fn level_words_name_what_each_level_does() {
    assert_eq!(
        level_meaning(ProactivityLevel::Off, ChannelKind::Standard, 6),
        "The assistant only answers here."
    );
    assert_eq!(
        level_meaning(ProactivityLevel::Normal, ChannelKind::Standard, 6),
        "Quiet, plus a check-in every 6 hours."
    );
    assert_eq!(
        level_meaning(ProactivityLevel::Eager, ChannelKind::Tutti, 3),
        "Normal, plus a daily research pass and offers."
    );
    assert_eq!(
        level_helper(ProactivityLevel::Quiet, ChannelKind::Standard),
        "Quiet: follow-ups it set, nudges for stale tasks and the start of a new channel. Only you can raise the level."
    );
}

#[test]
fn a_brief_reads_as_groups_with_numbered_linked_items() {
    let brief = parse_brief(crate::mock_proactivity::BRIEF_BODY).unwrap();
    assert_eq!(brief.intro, "Five things for today. Two need you.");
    let labels: Vec<_> = brief
        .groups
        .iter()
        .map(|g| (g.label.as_str(), g.your_turn))
        .collect();
    assert_eq!(
        labels,
        [
            ("Needs you", true),
            ("Ready for review", true),
            ("Done since yesterday", false)
        ]
    );
    let first = &brief.groups[0].lines[0];
    assert_eq!(first.number, 1);
    assert_eq!(first.name, "Go through open Sayso issues.");
    assert_eq!(first.target, "task:tsk_0192f0c4-sayso-issues");
    assert_eq!(first.rest, "Codex needs your OK to build on bigboi.");
    assert_eq!(brief.groups[2].lines[0].number, 5);
    assert!(parse_brief("Just a message.\n\n1. One").is_none());
    // Bold or heading group names read too.
    let bold = parse_brief("Hi.\n\n**Needs you**\n- [A](task:t) b").unwrap();
    assert_eq!(bold.groups[0].lines[0].name, "A");
}

#[test]
fn a_change_of_quiet_hours_sends_both_times() {
    let current = ProactivitySettings::default();
    let request = set_request(
        &ProactivityChanges {
            quiet_end: Some("06:00".into()),
            ..ProactivityChanges::default()
        },
        Some(&current),
    );
    assert_eq!(
        request.quiet_hours,
        Some(QuietHours {
            start: "21:00".into(),
            end: "06:00".into()
        })
    );
    assert_eq!(request.paused, None);
    let pause = set_request(
        &ProactivityChanges {
            paused: Some(true),
            ..ProactivityChanges::default()
        },
        Some(&current),
    );
    assert_eq!(pause.quiet_hours, None);
    assert_eq!(pause.paused, Some(true));
}

#[test]
fn follow_up_rows_say_when_and_when_they_were_set() {
    let now = crate::format::now_ms();
    let line = follow_up_when(now + 3_600_000, Some(now - 60_000), now);
    assert!(line.contains(" at "), "{line}");
    assert!(line.contains("set this"), "{line}");
}
