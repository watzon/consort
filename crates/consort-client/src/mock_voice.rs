//! Mock voice: the content of the voice designs (Settings > Voice off and
//! on, the voice task and the offer "Save this style guide?" in Tutti, and
//! the card of a draft in your voice with its flags and Edit), and the
//! voice commands on the mock state.
//!
//! The example data is the designs': the user is Chris, the mail account is
//! chris@bitstormtech.com, the Slack workspace is Work, and Dana is the
//! contact in the draft.

use crate::connectors::{CardFieldView, ConnectorCardView, HubCardView, ToolKindView};
use crate::proactivity::{ProactiveKind, ProactiveMarkView};
use crate::view::{
    ApprovalChoice, ApprovalView, DayView, DecisionCardView, DecisionOption, DecisionView,
    PostBody, RequestState, Speaker, TaskCardView, TaskStatus, TaskThreadView, ThreadPost,
    TimelineItem, ViewState, WorkerChip,
};
use crate::voice::{
    DraftCardView, EditableFieldView, FlagView, GuideEdit, VOICE_TASK_NOTE, VOICE_TASK_TITLE,
    VoiceSampleState, VoiceSampleView, VoiceSourceKind, VoiceSourceRow, VoiceView,
    apply_guide_edit, guide_sections, voice_line,
};
use crate::{ClientError, Command, CommandOutput};

pub const TUTTI: &str = "chn_tutti";
pub const GMAIL: &str = "con_00000000-0000-4000-8000-00000000a001";
pub const SLACK: &str = "con_00000000-0000-4000-8000-00000000a002";
pub const VOICE_TASK: &str = "tsk_voice_learn";
pub const VOICE_TASK_CARD: &str = "msg_voice_task";
pub const VOICE_OFFER: &str = "msg_voice_offer";
pub const VOICE_DECISION: &str = "dec_voice_offer";
pub const REPLY_TASK: &str = "tsk_reply_dana";
pub const REPLY_TASK_CARD: &str = "msg_reply_dana_task";
pub const DRAFT_APPROVAL: &str = "apr_voice_draft";

/// The `--screen` names of the voice designs.
pub const SCREENS: &[&str] = &[
    "voice-off",
    "voice-on",
    "voice-offer",
    "voice-approval",
    "voice-approval-checked",
    "voice-approval-edit",
];

/// The style guide of the design, as `voice.md`. Public posts has no
/// source in this build, so it stays empty and says so.
pub const GUIDE: &str = r#"# Voice
sources: gmail chris@bitstormtech.com (sent, 90 days), slack work (public channels)
learned: 214 sent mails and 380 Slack messages

## Core
- Short sentences. Plain words. Lead with the answer.
- No exclamation marks.
- Do not use "I hope this finds you well" or "circle back".

## email.formal (Clients and new contacts)
learned from: 96 mails
- Greet with "Hi {first name},". Sign off with "Best," and your first name.
- Two to four short paragraphs. Answer in the first line.
- No emoji. No contractions in a first mail to a new contact.
- Examples:
  1. "Hi {name},\nThanks for the quote. I read it twice and I have two questions.\nBest,\nChris"
  2. "Hi {name},\nThe invoice is attached. Tell me if a number looks wrong.\nBest,\nChris"
  3. "Hi {name},\nI need the signed copy this week. Can you send it by {day}?\nBest,\nChris"

## email.close (Colleagues and friends)
learned from: 118 mails
- Greet with "Hey {first name}," or with no greeting inside a thread. Sign off with your first name.
- One to three lines. Contractions are fine.
- Examples:
  1. "Hey {name},\nGot it. Looks good to me, ship it.\nChris"
  2. "Hey {name}, thanks for the heads up. I will tell the team.\nChris"

## chat (Slack and other chat)
learned from: 342 Slack messages
- No greeting and no sign-off. One idea in each message.
- Say "ok" and "yep", not "certainly". A lowercase start is fine.
- Examples:
  1. "yep, I saw it. fixing now"
  2. "ok, can you send the link to {name}?"

## post.public (Announcements and social posts)
Not enough text yet. Add documents to learn this context.

## Hard rules
- Never promise a date or a price. (you)
- Never agree to a meeting. Offer to check the calendar. (you)
- Do not use emoji in mail.
"#;

/// The body of the draft in the approval card.
pub const DRAFT_BODY: &str = "Hi Dana,\nThanks for the update. Friday at 3 PM works for me. Send the contract and I will sign it.\nBest,\nChris";

fn sources() -> Vec<VoiceSourceRow> {
    vec![
        VoiceSourceRow {
            connection_id: GMAIL.into(),
            kind: Some(VoiceSourceKind::Mail),
            monogram: "G".into(),
            title: "Gmail, chris@bitstormtech.com".into(),
            detail: "Sent mail, last 90 days".into(),
            ticked: true,
            include_private: false,
        },
        VoiceSourceRow {
            connection_id: SLACK.into(),
            kind: Some(VoiceSourceKind::Chat),
            monogram: "S".into(),
            title: "Slack, Work".into(),
            detail: "Messages that you wrote.".into(),
            ticked: true,
            include_private: false,
        },
    ]
}

/// Settings > Voice with the voice off (H3V-0).
pub fn off_view() -> VoiceView {
    VoiceView {
        available: true,
        enabled: false,
        sources: sources(),
        learn_from_edits: true,
        sources_line: "Gmail, chris@bitstormtech.com (sent mail, last 90 days) and Slack, Work (public channels). Your edits to drafts also teach it.".into(),
        ..VoiceView::default()
    }
}

/// Settings > Voice with the voice on and the guide (H7T-0).
pub fn on_view() -> VoiceView {
    VoiceView {
        enabled: true,
        learned_line: Some("Learned from 214 sent mails and 380 Slack messages on Oct 30".into()),
        guide: guide_sections(&consort_proto::model::VoiceGuide::parse(GUIDE)),
        guide_caption: "The note voice.md, saved on Oct 30. A rule with (you) is one that you wrote. A learned change never replaces it.".into(),
        note: Some((GUIDE.into(), "sha_mock_voice".into())),
        sample: Some(VoiceSampleView {
            request: "Reply to Dana, say yes".into(),
            state: VoiceSampleState::Ready,
            draft: Some(
                "Hi Dana,\nYes. Send me the details and I will take it from there.\nBest,\nChris"
                    .into(),
            ),
            voice_line: voice_line(Some("email.formal")),
        }),
        ..off_view()
    }
}

fn worker() -> Option<WorkerChip> {
    Some(WorkerChip {
        name: "MacBook Pro".into(),
        online: true,
        account: "Claude Work".into(),
    })
}

/// The voice task card and the offer in Tutti (HL7-0).
pub fn offer_items() -> Vec<TimelineItem> {
    let mut mark = ProactiveMarkView {
        kind: Some(ProactiveKind::Voice),
        label: "Offer".into(),
        icon: "compose".into(),
        offer: true,
        show_caption: true,
        ..ProactiveMarkView::default()
    };
    mark.reason = None;
    vec![
        TimelineItem::Day(DayView {
            key: "day-voice-task".into(),
            label: "Today".into(),
        }),
        TimelineItem::TaskCard(TaskCardView {
            message_id: VOICE_TASK_CARD.into(),
            task_id: VOICE_TASK.into(),
            time_label: "9:02 AM".into(),
            title: VOICE_TASK_TITLE.into(),
            status: TaskStatus::Review,
            worker: worker(),
            summary: "Ready for review".into(),
            latest: Some(
                "I read your sent mail and Slack messages and wrote a style guide. Tutti shows it to you next."
                    .into(),
            ),
            steps: vec![
                "Gmail, sent mail: 214 read, 9 threads skipped as private".into(),
                "Slack, public channels: 380 read".into(),
                "Left out 17 messages that the assistant wrote".into(),
            ],
            note: Some(VOICE_TASK_NOTE.into()),
            approval_mode: crate::view::ApprovalMode::Ask,
            ..TaskCardView::default()
        }),
        TimelineItem::Day(DayView {
            key: "day-voice-offer".into(),
            label: "Today".into(),
        }),
        TimelineItem::Decision(DecisionCardView {
            message_id: VOICE_OFFER.into(),
            speaker: Speaker::Consort,
            time_label: "9:06 AM".into(),
            created_at: 0,
            decision: DecisionView {
                id: VOICE_DECISION.into(),
                task_id: None,
                question: "Save this style guide?".into(),
                context: Some("I read 214 sent mails and 380 Slack messages. You write short sentences and lead with the answer. You use no exclamation marks, and in chat you use no greeting and no sign-off.\n\nThe guide has three contexts: Formal email, Close email and Chat. I save nothing until you say Do it.".into()),
                options: ["Do it", "Not now", "Never about this"]
                    .into_iter()
                    .map(|label| DecisionOption {
                        label: label.into(),
                        consequence: String::new(),
                    })
                    .collect(),
                recommended: None,
                allow_free_text: false,
                state: RequestState::Pending,
                chosen: None,
                answer_text: None,
            },
            proactive: Some(mark),
        }),
    ]
}

/// The draft card of HTB-0: "Send email" for Gmail, in your voice, with two
/// flags.
pub fn draft_approval() -> ApprovalView {
    let body = DRAFT_BODY;
    let flag = |text: &str, kind: &str| {
        let start = body.find(text).unwrap_or(0);
        FlagView {
            field: "Body".into(),
            text: text.into(),
            kind_label: kind.into(),
            note: "Not in your request. Check it.".into(),
            start,
            end: start + text.len(),
        }
    };
    ApprovalView {
        id: DRAFT_APPROVAL.into(),
        task_id: Some(REPLY_TASK.into()),
        summary: "Send email in Gmail, chris@bitstormtech.com".into(),
        choices: vec![ApprovalChoice::AllowOnce, ApprovalChoice::Deny],
        state: RequestState::Pending,
        card: Some(Box::new(HubCardView::Connector(ConnectorCardView {
            connection_id: GMAIL.into(),
            connection_name: "Gmail Work".into(),
            service_name: "Gmail".into(),
            logo: crate::mock_extensions::logo("gmail", "Gmail"),
            tool: "GMAIL_SEND_EMAIL".into(),
            kind: ToolKindView::Change,
            fields: vec![
                CardFieldView {
                    label: "To".into(),
                    value: "Dana Reyes, dana@example.com".into(),
                },
                CardFieldView {
                    label: "Subject".into(),
                    value: "Venue for the team offsite".into(),
                },
                CardFieldView {
                    label: "Body".into(),
                    value: body.into(),
                },
            ],
            rule_note: "Create and change tools ask every time".into(),
            draft: Some(DraftCardView {
                draft_only: false,
                voice_line: voice_line(Some("email.formal")),
                in_voice: true,
                flags: vec![
                    flag("Friday at 3 PM", "Date"),
                    flag("works for me", "Agreement"),
                ],
                editable: vec![
                    EditableFieldView {
                        key: "subject".into(),
                        label: "Subject".into(),
                        value: "Venue for the team offsite".into(),
                    },
                    EditableFieldView {
                        key: "body".into(),
                        label: "Body".into(),
                        value: body.into(),
                    },
                ],
                edited: false,
                not_my_voice: false,
            }),
        }))),
        button_labels: vec![
            (ApprovalChoice::AllowOnce, "Allow".into()),
            (ApprovalChoice::Deny, "Don't".into()),
        ],
        ..ApprovalView::default()
    }
}

/// The task "Reply to Dana about the venue" with the draft card.
fn reply_thread() -> TaskThreadView {
    TaskThreadView {
        task_id: REPLY_TASK.into(),
        title: "Reply to Dana about the venue".into(),
        status: TaskStatus::NeedsInput,
        status_detail: "1 approval".into(),
        brief: "Reply to Dana about the venue for the team offsite.".into(),
        model_label: Some("Opus 5.5, Claude Work, High effort".into()),
        posts: vec![
            ThreadPost {
                id: "pst_reply_dana_1".into(),
                speaker: Speaker::Task,
                time_label: "9:20 AM".into(),
                bodies: vec![PostBody::Text(
                    "I wrote the reply to Dana. Check the marked lines, then approve it.".into(),
                )],
                pending: false,
            },
            ThreadPost {
                id: "pst_reply_dana_2".into(),
                speaker: Speaker::Task,
                time_label: "9:20 AM".into(),
                bodies: vec![PostBody::Approval(draft_approval())],
                pending: false,
            },
        ],
        ..TaskThreadView::default()
    }
}

/// The view state of a voice screen, or `None` for another screen.
pub fn screen_state(screen: &str) -> Option<ViewState> {
    if !SCREENS.contains(&screen) {
        return None;
    }
    let mut state = crate::mock::view_state();
    match screen {
        "voice-off" => state.voice = off_view(),
        "voice-on" => state.voice = on_view(),
        "voice-offer" => {
            state.voice = on_view();
            if let Some(tutti) = state.timelines.iter_mut().find(|t| t.channel_id == TUTTI) {
                tutti.items = offer_items();
            }
            state.voice.proposal = Some(GUIDE.into());
        }
        _ => {
            state.voice = on_view();
            if let Some(tutti) = state.timelines.iter_mut().find(|t| t.channel_id == TUTTI) {
                tutti.items.push(TimelineItem::TaskCard(TaskCardView {
                    message_id: REPLY_TASK_CARD.into(),
                    task_id: REPLY_TASK.into(),
                    time_label: "9:20 AM".into(),
                    title: "Reply to Dana about the venue".into(),
                    status: TaskStatus::NeedsInput,
                    worker: worker(),
                    summary: "Needs input".into(),
                    approval: Some(draft_approval()),
                    ..TaskCardView::default()
                }));
            }
            state.tasks.push(crate::view::TaskView {
                id: REPLY_TASK.into(),
                channel_id: TUTTI.into(),
                channel_name: "Tutti".into(),
                title: "Reply to Dana about the venue".into(),
                status: TaskStatus::NeedsInput,
                worker_name: Some("MacBook Pro".into()),
                account_label: Some("Claude Work".into()),
                age_label: "2 min".into(),
                card_message_id: REPLY_TASK_CARD.into(),
                ..crate::view::TaskView::default()
            });
            state.threads.push(reply_thread());
        }
    }
    crate::derive::refresh(&mut state);
    Some(state)
}

/// The draft view of a card, for the mock commands.
fn draft_of(approval: &mut ApprovalView) -> Option<(&mut Vec<CardFieldView>, &mut DraftCardView)> {
    match approval.card.as_deref_mut() {
        Some(HubCardView::Connector(card)) => {
            let draft = card.draft.as_mut()?;
            Some((&mut card.fields, draft))
        }
        _ => None,
    }
}

/// Runs a voice command on the mock state.
pub fn apply(state: &mut ViewState, command: Command) -> Result<CommandOutput, ClientError> {
    match command {
        Command::LoadVoice => {}
        Command::SetVoice(changes) => {
            if let Some(sources) = changes.sources {
                for row in &mut state.voice.sources {
                    let chosen = sources.iter().find(|s| {
                        s.connection_id.map(|c| c.to_string()) == Some(row.connection_id.clone())
                    });
                    row.ticked = chosen.is_some();
                    row.include_private = chosen.is_some_and(|s| s.include_private);
                }
                state.voice.documents = sources
                    .iter()
                    .filter(|s| s.kind == VoiceSourceKind::Documents)
                    .flat_map(|s| s.paths.clone())
                    .collect();
            }
            if let Some(edits) = changes.learn_from_edits {
                state.voice.learn_from_edits = edits;
            }
            if let Some(disclosure) = changes.disclosure {
                state.voice.disclosure = disclosure;
            }
            match changes.enabled {
                Some(true) if !state.voice.enabled => {
                    let sources = state.voice.sources.clone();
                    let documents = state.voice.documents.clone();
                    state.voice = VoiceView {
                        sources,
                        documents,
                        ..on_view()
                    };
                }
                Some(false) => state.voice.enabled = false,
                _ => {}
            }
        }
        Command::LearnVoice => {
            state.voice.learning = Some(crate::voice::VoiceLearningView {
                task_id: VOICE_TASK.into(),
                title: VOICE_TASK_TITLE.into(),
                line: "Learning from your sources".into(),
                status: TaskStatus::Working,
            });
        }
        Command::ResetVoice => state.voice = off_view(),
        Command::TryVoice { request } => {
            state.voice.sample = Some(VoiceSampleView {
                request,
                state: VoiceSampleState::Ready,
                draft: Some(
                    "Hi Dana,\nYes. Send me the details and I will take it from there.\nBest,\nChris"
                        .into(),
                ),
                voice_line: voice_line(Some("email.formal")),
            });
        }
        Command::EditVoiceGuide(edit) => {
            let Some((content, sha)) = state.voice.note.clone() else {
                return Err(ClientError::Other("There is no style guide yet.".into()));
            };
            let content = match &edit {
                GuideEdit::Markdown { content } => content.clone(),
                other => apply_guide_edit(&content, other),
            };
            state.voice.guide = guide_sections(&consort_proto::model::VoiceGuide::parse(&content));
            state.voice.note = Some((content, sha));
        }
        Command::VoiceFeedback { approval_id } => {
            crate::mock_backend::for_each_approval(state, &approval_id, |approval| {
                if let Some((_, draft)) = draft_of(approval) {
                    draft.not_my_voice = true;
                }
            });
        }
        Command::EditApproval {
            approval_id,
            fields,
        } => {
            crate::mock_backend::for_each_approval(state, &approval_id, |approval| {
                if let Some((card_fields, draft)) = draft_of(approval) {
                    for (key, value) in &fields {
                        if let Some(field) = draft.editable.iter_mut().find(|f| &f.key == key) {
                            field.value = value.clone();
                            if let Some(shown) =
                                card_fields.iter_mut().find(|f| f.label == field.label)
                            {
                                shown.value = value.clone();
                            }
                        }
                    }
                    // The mock checks the new text again and finds nothing.
                    draft.flags.clear();
                    draft.edited = true;
                }
            });
        }
        _ => return Err(ClientError::Other("not a voice command".into())),
    }
    Ok(CommandOutput::Done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_voice_screen_has_its_state() {
        for screen in SCREENS {
            let state = screen_state(screen).expect("a voice screen");
            assert!(state.voice.available, "{screen}");
        }
        assert!(screen_state("tutti").is_none());
        let on = screen_state("voice-on").unwrap();
        assert_eq!(on.voice.guide.len(), 6);
        assert!(on.voice.guide[5].rules[0].by_user);
        let approval = screen_state("voice-approval").unwrap();
        assert!(approval.threads.iter().any(|t| t.task_id == REPLY_TASK));
    }

    #[test]
    fn the_mock_commands_change_the_voice() {
        let mut state = screen_state("voice-off").unwrap();
        apply(
            &mut state,
            Command::SetVoice(crate::voice::VoiceChanges {
                enabled: Some(true),
                sources: Some(Vec::new()),
                ..Default::default()
            }),
        )
        .unwrap();
        assert!(state.voice.enabled && !state.voice.guide.is_empty());
        apply(
            &mut state,
            Command::EditVoiceGuide(GuideEdit::AddHardRule {
                text: "Never sign as the team.".into(),
            }),
        )
        .unwrap();
        assert_eq!(state.voice.guide[5].rules.len(), 4);
        apply(&mut state, Command::ResetVoice).unwrap();
        assert!(!state.voice.enabled);

        let mut state = screen_state("voice-approval").unwrap();
        apply(
            &mut state,
            Command::EditApproval {
                approval_id: DRAFT_APPROVAL.into(),
                fields: vec![("body".into(), "I will check.".into())],
            },
        )
        .unwrap();
        let draft = state
            .threads
            .iter()
            .flat_map(|t| t.posts.iter())
            .flat_map(|p| p.bodies.iter())
            .find_map(|b| match b {
                PostBody::Approval(a) => match a.card.as_deref() {
                    Some(HubCardView::Connector(c)) => c.draft.clone(),
                    _ => None,
                },
                _ => None,
            })
            .unwrap();
        assert!(draft.flags.is_empty() && draft.edited);
        assert_eq!(draft.editable[1].value, "I will check.");
    }
}
