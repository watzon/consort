#![allow(clippy::unwrap_used)]

use consort_proto::client_api::{EntityPayload, FeedChange, FeedEntry};
use consort_proto::ids::{ConnectionId, DecisionId};
use consort_proto::model::{
    CommitmentFlag, DisplayField, MemoryAuthor, MemoryNote, ToolKind, UseIn, VoiceUse,
};
use consort_proto::time::Timestamp;
use serde_json::json;

use super::*;

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

fn cache_with_connections() -> (Cache, ConnectionId, ConnectionId) {
    let mut cache = Cache::default();
    let gmail = crate::test_entities::connection("Gmail Work", UseIn::EveryChannel, NOW);
    let mut slack = crate::test_entities::connection("Slack Work", UseIn::EveryChannel, NOW);
    slack.service = "slack".into();
    slack.service_name = "Slack".into();
    slack.identity = None;
    let github = {
        let mut c = crate::test_entities::connection("GitHub", UseIn::EveryChannel, NOW);
        c.service = "github".into();
        c
    };
    let (g, s) = (gmail.id, slack.id);
    for c in [gmail, slack, github] {
        cache.connections.insert(c.id.to_string(), c);
    }
    (cache, g, s)
}

const GUIDE: &str = "# Voice\nlearned: 214 sent mails\n\n## Core\n- Short sentences.\n- No exclamation marks.\n\n## email.formal (Clients and new contacts)\nlearned from: 96 mails\n- Greet with \"Hi {name},\".\n- Examples:\n  1. \"Hi {name},\\nThanks.\"\n  2. \"Hi {name},\\nThe invoice is attached.\"\n\n## Hard rules\n- Never promise a date. (you)\n";

#[test]
fn the_page_is_off_with_the_default_sources_until_the_hub_has_settings() {
    let (mut cache, gmail, slack) = cache_with_connections();
    assert!(!voice_view(&cache).available);
    cache.apply(feed(
        1,
        EntityPayload::VoiceSettings(VoiceSettings::default()),
    ));
    let view = voice_view(&cache);
    assert!(view.available && !view.enabled);
    // Mail and the chat's public channels are ticked; GitHub is no source.
    assert_eq!(view.sources.len(), 2);
    assert_eq!(view.sources[0].title, "Gmail, chris@work.example");
    assert_eq!(view.sources[0].detail, "Sent mail, last 90 days");
    assert_eq!(view.sources[1].title, "Slack, Work");
    assert!(view.sources.iter().all(|s| s.ticked && !s.include_private));
    assert!(view.can_turn_on());
    let list = view.source_list();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].connection_id, Some(gmail));
    assert_eq!(list[1].kind, VoiceSourceKind::Chat);
    assert_eq!(list[1].connection_id, Some(slack));
    assert!(view.guide.is_empty());
}

#[test]
fn the_page_shows_the_learned_line_the_guide_and_the_learning_state() {
    let (mut cache, gmail, _) = cache_with_connections();
    let settings = VoiceSettings {
        enabled: true,
        sources: vec![VoiceSource {
            kind: VoiceSourceKind::Mail,
            connection_id: Some(gmail),
            include_private: false,
            paths: Vec::new(),
        }],
        last_learned_at: Some(Timestamp::from_millis(NOW)),
        learned_summary: Some("214 sent mails".into()),
        ..VoiceSettings::default()
    };
    cache.apply(feed(1, EntityPayload::VoiceSettings(settings)));
    cache.apply(feed(
        2,
        EntityPayload::MemoryNote(MemoryNote {
            scope: MemoryScope::Global,
            path: VOICE_NOTE_PATH.into(),
            content: GUIDE.into(),
            sha256: consort_proto::model::Sha256Hash::from_bytes([7; 32]),
            version: 1,
            updated_by: MemoryAuthor::User,
            updated_at: Timestamp::from_millis(NOW),
            deleted_at: None,
            suggested_by: None,
        }),
    ));
    let view = voice_view(&cache);
    assert!(view.enabled);
    let line = view.learned_line.clone().unwrap();
    assert!(
        line.starts_with("Learned from 214 sent mails on "),
        "{line}"
    );
    assert_eq!(
        view.sources_line,
        "Gmail, chris@work.example (sent mail, last 90 days). Your edits to drafts also teach it."
    );
    // Only the chosen source is ticked now.
    assert_eq!(view.sources.iter().filter(|s| s.ticked).count(), 1);
    let titles: Vec<&str> = view.guide.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(
        titles,
        vec![
            "Core rules",
            "Formal email",
            "Close email",
            "Chat",
            "Public posts",
            "Hard rules"
        ]
    );
    let formal = &view.guide[1];
    assert_eq!(formal.learned.as_deref(), Some("Learned from 96 mails"));
    assert_eq!(formal.examples.len(), 2);
    assert_eq!(
        view.guide[4].note.as_deref(),
        Some("Not enough text yet. Add documents to learn this context.")
    );
    assert!(view.guide[5].rules[0].by_user);
    assert!(view.note.is_some());
    assert!(view.learning.is_none());
}

#[test]
fn a_new_offer_drops_the_old_proposal_text() {
    let mut cache = VoiceCache {
        proposal: Some("# Voice".into()),
        ..VoiceCache::default()
    };
    assert!(cache.apply_settings(VoiceSettings {
        proposal_decision_id: Some(DecisionId::new()),
        ..VoiceSettings::default()
    }));
    assert!(cache.proposal.is_some());
    assert!(cache.apply_settings(VoiceSettings::default()));
    assert!(cache.proposal.is_none());
}

fn card(voice: Option<&str>, flags: Vec<CommitmentFlag>) -> ConnectorCard {
    let body = "Hi Dana,\nFriday at 3 PM works for me.";
    ConnectorCard {
        connection_id: ConnectionId::new(),
        connection_name: "Gmail Work".into(),
        service_name: "Gmail".into(),
        tool: "GMAIL_SEND_EMAIL".into(),
        tool_kind: ToolKind::Change,
        fields: vec![
            DisplayField {
                label: "To".into(),
                value: "dana@example.com".into(),
            },
            DisplayField {
                label: "Subject".into(),
                value: "Venue".into(),
            },
            DisplayField {
                label: "Body".into(),
                value: body.into(),
            },
        ],
        arguments: json!({"to": "dana@example.com", "subject": "Venue", "body": body}),
        outgoing: Some(OutgoingKind::Send),
        editable: vec!["subject".into(), "body".into()],
        voice: voice.map(|c| VoiceUse { context: c.into() }),
        flags,
        draft_arguments: None,
        not_my_voice: false,
        ask_rule: None,
    }
}

#[test]
fn a_draft_card_has_the_voice_line_the_flags_and_its_fields_for_edit() {
    let flag = CommitmentFlag {
        field: "body".into(),
        label: "Body".into(),
        text: "Friday at 3 PM".into(),
        kind: FlagKind::Date,
        start: 9,
        end: 23,
        rule: None,
    };
    let view = draft_card(&card(Some("email.formal"), vec![flag])).unwrap();
    assert_eq!(view.voice_line, "Written in your voice (Formal email)");
    assert!(view.in_voice);
    assert_eq!(view.flags[0].kind_label, "Date");
    assert_eq!(view.flags[0].note, "Not in your request. Check it.");
    assert_eq!(view.ranges("Body"), vec![(9, 23)]);
    // Every allow button stays off until the box is ticked.
    assert!(view.allow_blocked(false));
    assert!(!view.allow_blocked(true));
    let keys: Vec<(&str, &str)> = view
        .editable
        .iter()
        .map(|f| (f.key.as_str(), f.label.as_str()))
        .collect();
    assert_eq!(keys, vec![("subject", "Subject"), ("body", "Body")]);
    assert!(!view.edited);

    let neutral = draft_card(&card(None, Vec::new())).unwrap();
    assert_eq!(neutral.voice_line, "Neutral style");
    assert!(!neutral.allow_blocked(false));
    let mut read = card(None, Vec::new());
    read.outgoing = None;
    assert!(draft_card(&read).is_none());
}

#[test]
fn guide_edits_remove_rules_and_examples_and_add_the_users_rule() {
    let removed = apply_guide_edit(
        GUIDE,
        &GuideEdit::RemoveRule {
            section: "core".into(),
            index: 1,
        },
    );
    assert!(!removed.contains("No exclamation marks."));
    assert!(removed.contains("- Short sentences."));
    let removed = apply_guide_edit(
        GUIDE,
        &GuideEdit::RemoveExample {
            section: "email.formal".into(),
            index: 0,
        },
    );
    assert!(!removed.contains("Thanks."));
    assert!(removed.contains("The invoice is attached."));
    let added = apply_guide_edit(
        GUIDE,
        &GuideEdit::AddHardRule {
            text: "Never sign as the team.".into(),
        },
    );
    assert!(added.contains("- Never sign as the team. (you)"));
    let same = apply_guide_edit(
        GUIDE,
        &GuideEdit::Markdown {
            content: "# Voice\n".into(),
        },
    );
    assert_eq!(same, "# Voice\n");
}

#[test]
fn the_voice_task_card_takes_its_steps_from_the_summary() {
    let (rest, steps) = voice_task_steps(
        "I read your sent mail and wrote a style guide.\n- Gmail, sent mail: 214 read\n- Left out 17 messages that the assistant wrote",
    );
    assert_eq!(rest, "I read your sent mail and wrote a style guide.");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1], "Left out 17 messages that the assistant wrote");
}

#[test]
fn changes_become_one_set_request() {
    let changes = VoiceChanges {
        enabled: Some(true),
        disclosure: Some(Disclosure::Footer),
        ..VoiceChanges::default()
    };
    let request = changes.request();
    assert_eq!(request.enabled, Some(true));
    assert_eq!(request.disclosure, Some(Disclosure::Footer));
    assert!(request.sources.is_none());
}
