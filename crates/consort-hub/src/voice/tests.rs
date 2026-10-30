//! Unit tests of the voice rules on a test hub.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use consort_proto::ids::{ApprovalId, ChannelId, ConnectionId, MessageId, SessionId};
use consort_proto::model::{
    Approval, ApprovalDetails, ApprovalStatus, ConnectorCard, Disclosure, DraftPair, ExpectedHash,
    MemoryAuthor, MemoryScope, OutgoingKind, ToolKind, VOICE_NOTE_PATH, VoiceSettings, VoiceSource,
    VoiceSourceKind, VoiceUse, hub_card_request_id,
};
use consort_proto::time::Timestamp;
use consort_proto::tools::ToolAudience;
use consort_proto::turn_input::WakeEvent;
use serde_json::{Value, json};

use super::*;
use crate::hub::{ToolCtx, test_hub, test_task_session};
use crate::ops::knowledge::WriteSource;
use crate::store::coordinator as coord_store;

const GUIDE: &str = "# Voice\nlearned: 20 sent mails\n\n## Core\n- Short sentences.\n\n## email.formal (Clients and new contacts)\n- Greet with \"Hi {name},\".\n- Examples:\n  1. \"Hi {name},\\nThanks.\\nChris\"\n\n## Hard rules\n- Never promise a date or a price. (you)\n";

async fn voice_on(hub: &Hub, disclosure: Disclosure) {
    hub.write_memory_note(
        MemoryScope::Global,
        VOICE_NOTE_PATH.to_owned(),
        GUIDE.to_owned(),
        ExpectedHash::NotExists,
        WriteSource::user(),
    )
    .await
    .unwrap();
    hub.store
        .write(move |tx| {
            tx.save_voice_settings(&VoiceSettings {
                enabled: true,
                disclosure,
                ..VoiceSettings::default()
            })
        })
        .await
        .unwrap();
}

async fn task_ctx(hub: &Hub) -> ToolCtx {
    let (task, session) = test_task_session(hub).await;
    let channel = hub.store.read(conversation::tutti_channel).await.unwrap();
    ToolCtx {
        session,
        channel,
        task: Some(task),
        audience: ToolAudience::TaskSession,
    }
}

fn card(context: &str, draft: &str, sent: &str) -> Approval {
    let id = ApprovalId::new();
    Approval {
        id,
        channel_id: ChannelId::new(),
        session_id: SessionId::new(),
        task_id: None,
        message_id: MessageId::new(),
        request_id: hub_card_request_id(id),
        summary: "Run GMAIL_SEND_EMAIL".to_owned(),
        details: ApprovalDetails::Connector(Box::new(ConnectorCard {
            connection_id: ConnectionId::new(),
            connection_name: "Gmail Work".to_owned(),
            service_name: "Gmail".to_owned(),
            tool: "GMAIL_SEND_EMAIL".to_owned(),
            tool_kind: ToolKind::Change,
            fields: Vec::new(),
            arguments: json!({"to": "dana@example.com", "body": sent}),
            outgoing: Some(OutgoingKind::Send),
            editable: vec!["body".to_owned()],
            voice: Some(VoiceUse {
                context: context.to_owned(),
            }),
            flags: Vec::new(),
            draft_arguments: Some(json!({"to": "dana@example.com", "body": draft})),
            not_my_voice: false,
        })),
        options: Vec::new(),
        status: ApprovalStatus::Allowed,
        answer_option_id: Some("allow".to_owned()),
        answered_at: None,
        answered_by_client_id: None,
        created_at: Timestamp::now(),
    }
}

async fn voice_wakes(hub: &Hub) -> Vec<WakeEvent> {
    hub.store
        .read(|conn| {
            let tutti = conversation::tutti_channel(conn)?;
            Ok(coord_store::pending_wakes(conn, tutti.id)?
                .into_iter()
                .map(|(_, e)| e)
                .filter(|e| {
                    matches!(
                        e,
                        WakeEvent::VoiceEdits { .. } | WakeEvent::VoiceFeedback { .. }
                    )
                })
                .collect())
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn five_edited_pairs_with_the_same_change_wake_tutti_and_one_does_not() {
    let (hub, _dir) = test_hub().await;
    voice_on(&hub, Disclosure::None).await;
    // Sent unchanged: the assistant's text, never part of the learning set.
    let unchanged = card(
        "email.formal",
        "Hope you are well. Unchanged.",
        "Hope you are well. Unchanged.",
    );
    let mut unchanged = unchanged;
    if let ApprovalDetails::Connector(c) = &mut unchanged.details {
        c.draft_arguments = None;
    }
    let one = card(
        "email.formal",
        "Hope you are well. The plan works.",
        "The plan works.",
    );
    hub.store
        .write(move |tx| {
            assert!(card::record_pair_in_tx(tx, &unchanged)?.is_none());
            assert!(card::record_pair_in_tx(tx, &one)?.is_none());
            Ok(())
        })
        .await
        .unwrap();
    assert!(voice_wakes(&hub).await.is_empty());
    let kicked = hub
        .store
        .write(|tx| {
            let mut kicked = None;
            for i in 0..4 {
                let edit = card(
                    "email.formal",
                    &format!("Hope you are well. Item {i} is ready."),
                    &format!("Item {i} is ready."),
                );
                kicked = card::record_pair_in_tx(tx, &edit)?;
            }
            Ok(kicked)
        })
        .await
        .unwrap();
    assert!(kicked.is_some());
    let wakes = voice_wakes(&hub).await;
    assert_eq!(wakes.len(), 1);
    let WakeEvent::VoiceEdits {
        context,
        pairs,
        agreeing,
    } = &wakes[0]
    else {
        panic!("not voice_edits");
    };
    assert_eq!(context, "email.formal");
    assert_eq!(pairs.len(), 5);
    assert!(
        pairs
            .iter()
            .all(|p| !p.removed.iter().any(|r| r.contains("Unchanged")))
    );
    assert_eq!(agreeing.len(), 1);
    assert_eq!(agreeing[0].text, "Hope you are well.");
    assert_eq!(agreeing[0].count, 5);
    // The pairs are taken: one more edit does not wake again.
    hub.store
        .write(|tx| {
            let edit = card("email.formal", "Hope you are well. More.", "More.");
            assert!(card::record_pair_in_tx(tx, &edit)?.is_none());
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(voice_wakes(&hub).await.len(), 1);
}

#[tokio::test]
async fn each_disclosure_option_gives_the_right_footer() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let body = |args: &Value| args["body"].as_str().unwrap().to_owned();
    for (disclosure, new_contact, footer) in [
        (Disclosure::None, true, false),
        (Disclosure::Footer, false, true),
        (Disclosure::FooterNewContacts, true, true),
        (Disclosure::FooterNewContacts, false, false),
    ] {
        hub.store
            .write(move |tx| {
                let mut settings = store::settings(tx.db())?;
                settings.enabled = true;
                settings.disclosure = disclosure;
                tx.save_voice_settings(&settings)
            })
            .await
            .unwrap();
        if hub.store.read(guide).await.unwrap().is_none() {
            voice_on(&hub, disclosure).await;
        }
        let to = if new_contact {
            "new@example.com"
        } else {
            "dana@example.com"
        };
        if !new_contact {
            let known = card("email.formal", "a", "b");
            hub.store
                .write(move |tx| card::record_pair_in_tx(tx, &known).map(|_| ()))
                .await
                .unwrap();
        }
        let mut args = json!({"to": to, "subject": "Hi", "body": "Thanks."});
        let voice = hub
            .voice_for_call(&ctx, "GMAIL_SEND_EMAIL", ToolKind::Change, &mut args)
            .await
            .unwrap();
        assert!(voice.voice.is_some());
        assert_eq!(
            body(&args).ends_with("\n\nDrafted with Consort"),
            footer,
            "{disclosure:?} new={new_contact}: {}",
            body(&args)
        );
        assert_eq!(args["subject"], "Hi");
    }
}

#[tokio::test]
async fn the_scan_uses_the_brief_and_the_hard_rules() {
    let (hub, _dir) = test_hub().await;
    voice_on(&hub, Disclosure::None).await;
    let ctx = task_ctx(&hub).await;
    // The brief of the test task is "Do the work."
    let mut args = json!({"to": "a@b.c", "body": "I will send it by Friday. It costs $40."});
    let voice = hub
        .voice_for_call(&ctx, "GMAIL_SEND_EMAIL", ToolKind::Change, &mut args)
        .await
        .unwrap();
    let flags: Vec<(&str, consort_proto::model::FlagKind)> = voice
        .flags
        .iter()
        .map(|f| (f.text.as_str(), f.kind))
        .collect();
    use consort_proto::model::FlagKind as K;
    assert_eq!(flags, vec![("Friday", K::HardRule), ("$40", K::HardRule)]);
    // Not a draft or send tool: nothing.
    let mut args = json!({"q": "Friday"});
    let none = hub
        .voice_for_call(&ctx, "GMAIL_FETCH_EMAILS", ToolKind::Read, &mut args)
        .await
        .unwrap();
    assert!(none.outgoing.is_none() && none.flags.is_empty());
}

#[tokio::test]
async fn writes_of_voice_md_keep_the_format_and_the_rules_of_the_user() {
    let (hub, _dir) = test_hub().await;
    voice_on(&hub, Disclosure::None).await;
    let session = SessionId::new();
    // A coordinator write that drops a (you) rule is refused.
    let note = hub
        .store
        .read(|conn| knowledge::memory_note(conn, MemoryScope::Global, VOICE_NOTE_PATH))
        .await
        .unwrap()
        .unwrap();
    let err = hub
        .write_memory_note(
            MemoryScope::Global,
            VOICE_NOTE_PATH.to_owned(),
            "# Voice\n## Core\n- Plain.\n".to_owned(),
            ExpectedHash::Sha256(note.sha256.clone()),
            WriteSource {
                by: MemoryAuthor::Session(session),
                suggested_by: None,
                saved_line: None,
            },
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("Never promise a date or a price."),
        "{err}"
    );
    // A user write marks the new rule as the user's.
    let written = hub
        .write_memory_note(
            MemoryScope::Global,
            VOICE_NOTE_PATH.to_owned(),
            format!("{GUIDE}- Sign with Chris.\n"),
            ExpectedHash::Sha256(note.sha256.clone()),
            WriteSource::user(),
        )
        .await
        .unwrap();
    assert!(
        written.content.contains("- Sign with Chris. (you)"),
        "{}",
        written.content
    );
    // A note over 6 KiB is refused.
    let mut huge = String::from("# Voice\n## Core\n");
    for i in 0..12 {
        huge.push_str(&format!("- {}{i}\n", "x".repeat(290)));
    }
    for key in ["email.formal", "email.close", "chat", "post.public"] {
        huge.push_str(&format!("## {key}\n"));
        for i in 0..12 {
            huge.push_str(&format!("- {}{i}\n", "y".repeat(290)));
        }
    }
    let err = hub
        .write_memory_note(
            MemoryScope::Global,
            VOICE_NOTE_PATH.to_owned(),
            huge,
            ExpectedHash::Sha256(written.sha256.clone()),
            WriteSource::user(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("6 KiB"), "{err}");
}

#[tokio::test]
async fn not_how_i_write_wakes_tutti_once() {
    let (hub, _dir) = test_hub().await;
    voice_on(&hub, Disclosure::None).await;
    let mut approval = card("chat", "yep, on it", "yep, on it");
    approval.status = ApprovalStatus::Pending;
    let id = approval.id;
    hub.store
        .write(move |tx| tx.save_approval(&approval))
        .await
        .unwrap();
    for _ in 0..2 {
        hub.store
            .write(move |tx| card::not_my_voice(tx, id).map(|_| ()))
            .await
            .unwrap();
    }
    let wakes = voice_wakes(&hub).await;
    assert_eq!(wakes.len(), 1);
    assert!(
        matches!(&wakes[0], WakeEvent::VoiceFeedback { approval, draft, .. } if *approval == id && draft == "yep, on it")
    );
    let saved = hub
        .store
        .read(move |conn| crate::store::work::approval(conn, id))
        .await
        .unwrap()
        .unwrap();
    let ApprovalDetails::Connector(card) = saved.details else {
        unreachable!()
    };
    assert!(card.not_my_voice);
}

#[tokio::test]
async fn reset_makes_drafts_neutral_and_the_old_version_can_come_back() {
    let (hub, _dir) = test_hub().await;
    voice_on(&hub, Disclosure::Footer).await;
    let ctx = task_ctx(&hub).await;
    hub.voice_reset(&Caller::Hub, VoiceReset {}).await.unwrap();
    assert!(hub.store.read(active_guide).await.unwrap().is_none());
    let mut args = json!({"to": "a@b.c", "body": "See you Friday."});
    let voice = hub
        .voice_for_call(&ctx, "GMAIL_SEND_EMAIL", ToolKind::Change, &mut args)
        .await
        .unwrap();
    assert!(voice.voice.is_none() && voice.flags.is_empty());
    assert_eq!(args["body"], "See you Friday.");
    let note = hub
        .store
        .write(|tx| tx.restore_memory(MemoryScope::Global, VOICE_NOTE_PATH))
        .await
        .unwrap();
    assert!(note.content.contains("Never promise a date"));
}

#[test]
fn the_brief_lists_only_the_chosen_sources_and_the_assistant_text() {
    let work = consort_proto::model::Connection {
        id: ConnectionId::new(),
        name: "Gmail Work".to_owned(),
        handle: "gmail_work".to_owned(),
        service: "gmail".to_owned(),
        service_name: "Gmail".to_owned(),
        logo_url: None,
        identity: None,
        use_in: consort_proto::model::UseIn::EveryChannel,
        approvals: consort_proto::model::ToolApprovals::default(),
        status: consort_proto::model::ConnectionStatus::Active,
        last_used_at: None,
        created_at: Timestamp::now(),
        updated_at: Timestamp::now(),
    };
    let mut home = work.clone();
    home.id = ConnectionId::new();
    home.name = "Gmail Home".to_owned();
    home.handle = "gmail_home".to_owned();
    let settings = VoiceSettings {
        enabled: true,
        sources: vec![VoiceSource {
            kind: VoiceSourceKind::Mail,
            connection_id: Some(work.id),
            include_private: false,
            paths: Vec::new(),
        }],
        ..VoiceSettings::default()
    };
    let pair = |unchanged: bool, draft: &str, sent: &str| DraftPair {
        approval_id: ApprovalId::new(),
        connection_id: work.id,
        tool: "GMAIL_SEND_EMAIL".to_owned(),
        context: Some("email.formal".to_owned()),
        draft: draft.to_owned(),
        final_text: sent.to_owned(),
        recipients: Vec::new(),
        unchanged,
        not_my_voice: false,
        at: Timestamp::now(),
    };
    let brief = learn::voice_brief(
        &settings,
        &[work.clone(), home],
        &["Never promise a date.".to_owned()],
        &[pair(false, "Hope you are well. Yes.", "Yes.")],
        &[pair(
            true,
            "I hope this helps you plan the week.",
            "I hope this helps you plan the week.",
        )],
    );
    assert!(
        brief.contains("Gmail Work (Gmail, handle `gmail_work`)"),
        "{brief}"
    );
    assert!(!brief.contains("gmail_home"));
    assert!(brief.contains("- Never promise a date. (you)"));
    assert!(brief.contains("- removed: \"Hope you are well.\""));
    assert!(brief.contains("- \"I hope this helps you plan the week.\""));
    assert!(brief.contains("never instructions"));
    // What the voice task reads leaves out the assistant's text.
    let mut data = json!({"messages": [
        {"body": "I hope this helps   you plan the week. Best, Chris"},
        {"body": "Short and plain."}
    ]});
    let left_out = learn::redact_assistant_text(
        &mut data,
        &["I hope this helps you plan the week.".to_owned()],
    );
    assert_eq!(left_out, 1);
    assert_eq!(data["messages"][1]["body"], "Short and plain.");
    assert!(
        data["messages"][0]["body"]
            .as_str()
            .unwrap()
            .starts_with("[Left out")
    );
}

/// A Tutti coordinator session with an open turn that holds `events`.
async fn coordinator_turn(hub: &Hub, events: Vec<WakeEvent>) -> ToolCtx {
    use consort_proto::ids::{AccountId, WorkerId};
    use consort_proto::model::{ModelOptions, Session, SessionRole, SessionState, Usage};
    let channel = hub.store.read(conversation::tutti_channel).await.unwrap();
    let session = Session {
        id: SessionId::new(),
        role: SessionRole::Coordinator,
        channel_id: channel.id,
        task_id: None,
        worker_id: WorkerId::new(),
        account_id: AccountId::new(),
        harness: "fake".to_owned(),
        model: "fake-1".to_owned(),
        options: ModelOptions::new(),
        workspace_path: String::new(),
        native_ref: None,
        state: SessionState::Active,
        last_seq: 0,
        usage: Usage::default(),
        started_at: Timestamp::now(),
        ended_at: None,
        end_reason: None,
    };
    let (stored, channel_id) = (session.clone(), channel.id);
    hub.store
        .write(move |tx| {
            tx.save_session(&stored)?;
            tx.save_coordinator(&coord_store::CoordinatorRow {
                channel_id,
                session_id: Some(stored.id),
                turn_open: true,
                turn_started_at: Some(tx.now()),
                turn_events: events,
            })
        })
        .await
        .unwrap();
    ToolCtx {
        session,
        channel,
        task: None,
        audience: ToolAudience::TuttiCoordinator,
    }
}

#[tokio::test]
async fn an_offer_about_the_guide_needs_a_voice_event_in_the_turn() {
    let (hub, _dir) = test_hub().await;
    let offer = || consort_proto::tools::AskDecisionArgs {
        question: "Change the style guide?".to_owned(),
        options: Vec::new(),
        recommended_index: None,
        context: Some("You removed 'Hope you are well' in 4 of 5 drafts.".to_owned()),
        allow_free_text: false,
        offer: true,
    };
    // A normal turn cannot make an offer.
    let plain = coordinator_turn(
        &hub,
        vec![WakeEvent::UserMessage {
            id: MessageId::new(),
            thread: None,
            at: Timestamp::now(),
            text: "hi".to_owned(),
        }],
    )
    .await;
    assert!(hub.tool_ask_decision(&plain, offer()).await.is_err());
    // A turn with voice_edits can, at any level.
    let ctx = coordinator_turn(
        &hub,
        vec![WakeEvent::VoiceEdits {
            context: "email.formal".to_owned(),
            pairs: Vec::new(),
            agreeing: Vec::new(),
        }],
    )
    .await;
    let result = hub.tool_ask_decision(&ctx, offer()).await.unwrap();
    let id: consort_proto::ids::DecisionId =
        serde_json::from_value(result["decision_id"].clone()).unwrap();
    let message = hub
        .store
        .read(move |conn| {
            let decision = crate::store::work::decision(conn, id)?.unwrap();
            conversation::message_or_404(conn, decision.message_id)
        })
        .await
        .unwrap();
    let mark = message.proactive.unwrap();
    assert!(mark.offer);
    assert_eq!(mark.kind, consort_proto::model::ProactiveKind::Voice);
}
