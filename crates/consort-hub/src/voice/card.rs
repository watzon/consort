//! The card of a draft or send tool: Edit, the voice line, the disclosure
//! line, the commitment flags with "I checked the marked lines", the draft
//! pairs and "Not how I write".

use consort_proto::client_api::{ApprovalEdit, ApprovalResult};
use consort_proto::ids::{ApprovalId, ChannelId, SessionId, TaskId};
use consort_proto::model::{
    Approval, ApprovalDetails, ApprovalStatus, Author, CORE_CONTEXT, CommitmentFlag, ConnectorCard,
    DISCLOSURE_LINE, Disclosure, DraftPair, EDITABLE_ARGUMENTS, OutgoingKind, ToolKind,
    VOICE_EDITS_WAKE_PAIRS, VoiceGuide, VoiceUse, outgoing_kind,
};
use consort_proto::turn_input::WakeEvent;
use rusqlite::Connection as Db;
use serde_json::{Value, json};

use super::scan::{ScanField, scan};
use super::{active_guide, edits, store};
use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub, ToolCtx};
use crate::store::{Tx, conversation, coordinator as coord_store, work};

/// The most bytes in one edited field.
const EDIT_FIELD_MAX_BYTES: usize = 64 * 1024;
/// The user messages of a coordinator's channel that count as the request.
const REQUEST_MESSAGES: usize = 10;

/// What the voice adds to a connector call.
#[derive(Debug, Clone, Default)]
pub(crate) struct CardVoice {
    pub outgoing: Option<OutgoingKind>,
    pub editable: Vec<String>,
    pub voice: Option<VoiceUse>,
    pub flags: Vec<CommitmentFlag>,
}

impl CardVoice {
    /// Puts the voice parts on a card.
    pub(crate) fn apply(self, card: &mut ConnectorCard) {
        card.outgoing = self.outgoing;
        card.editable = self.editable;
        card.voice = self.voice;
        card.flags = self.flags;
    }
}

/// The argument keys of a call that the user can edit: text values of the
/// keys in [`EDITABLE_ARGUMENTS`], in that order.
fn editable_keys(arguments: &Value) -> Vec<String> {
    EDITABLE_ARGUMENTS
        .iter()
        .filter(|k| arguments.get(**k).is_some_and(Value::is_string))
        .map(|k| (*k).to_owned())
        .collect()
}

/// The label of an argument on the card, such as "Body" for `message`.
fn label_of(key: &str) -> String {
    crate::connectors::run::display_fields(&json!({ key: "x" }))
        .into_iter()
        .next()
        .map_or_else(|| key.to_owned(), |f| f.label)
}

/// The recipients of a call: the values of its address fields.
fn recipients(arguments: &Value) -> Vec<String> {
    let keys = [
        "to",
        "recipient_email",
        "recipient",
        "recipients",
        "to_email",
        "to_emails",
        "cc",
        "channel",
        "user",
    ];
    let mut out = Vec::new();
    for key in keys {
        match arguments.get(key) {
            Some(Value::String(s)) => out.extend(
                s.split([',', ';'])
                    .map(|p| p.trim().to_lowercase())
                    .filter(|p| !p.is_empty()),
            ),
            Some(Value::Array(items)) => out.extend(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|p| p.trim().to_lowercase()),
            ),
            _ => {}
        }
    }
    out
}

/// The text of a draft: the subject as one line, then the other text
/// fields.
fn draft_text(arguments: &Value, keys: &[String]) -> String {
    keys.iter()
        .filter_map(|k| {
            let value = arguments.get(k)?.as_str()?;
            Some(if k == "subject" {
                format!("Subject: {value}")
            } else {
                value.to_owned()
            })
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What the user asked for: a task's brief and the user's messages in its
/// thread, or the user messages of a coordinator's turn and its channel.
fn request_text(
    conn: &Db,
    session_id: SessionId,
    channel_id: ChannelId,
    task_id: Option<TaskId>,
) -> Result<String> {
    let mut parts = Vec::new();
    if let Some(task_id) = task_id {
        let task = work::task_or_404(conn, task_id)?;
        parts.push(task.title.clone());
        parts.push(task.brief.clone());
        for message in conversation::thread_replies(conn, task.card_message_id)? {
            if message.author == Author::User {
                parts.push(message.body);
            }
        }
        return Ok(parts.join("\n"));
    }
    let row = coord_store::coordinator(conn, channel_id)?;
    if row.session_id == Some(session_id) {
        for event in &row.turn_events {
            match event {
                WakeEvent::UserMessage { text, .. }
                | WakeEvent::TuttiMessage { text, .. }
                | WakeEvent::VoiceSample { request: text, .. } => parts.push(text.clone()),
                WakeEvent::DecisionAnswered {
                    text: Some(text), ..
                } => parts.push(text.clone()),
                _ => {}
            }
        }
    }
    let (messages, _) = conversation::messages_page(conn, channel_id, None, None, 50, usize::MAX)?;
    parts.extend(
        messages
            .into_iter()
            .filter(|m| m.author == Author::User)
            .take(REQUEST_MESSAGES)
            .map(|m| m.body),
    );
    Ok(parts.join("\n"))
}

/// The flags of a call's text.
fn flags_for(
    arguments: &Value,
    keys: &[String],
    request: &str,
    guide: &VoiceGuide,
) -> Vec<CommitmentFlag> {
    let labels: Vec<String> = keys.iter().map(|k| label_of(k)).collect();
    let fields: Vec<ScanField<'_>> = keys
        .iter()
        .zip(&labels)
        .filter_map(|(k, label)| {
            Some(ScanField {
                key: k,
                label,
                value: arguments.get(k)?.as_str()?,
            })
        })
        .collect();
    scan(&fields, request, &guide.hard_rules)
}

/// Adds the disclosure line at the end of the main text field.
fn add_disclosure(arguments: &mut Value, keys: &[String]) {
    let Some(key) = keys.iter().find(|k| *k != "subject") else {
        return;
    };
    if let Some(Value::String(text)) = arguments.get_mut(key.as_str())
        && !text.contains(DISCLOSURE_LINE)
    {
        let trimmed = text.trim_end().to_owned();
        *text = format!("{trimmed}\n\n{DISCLOSURE_LINE}");
    }
}

impl Hub {
    /// The voice parts of a connector call, before the hub asks or runs
    /// it. With the voice on, it adds the disclosure line to the
    /// arguments, names the voice that wrote the text, and scans the text.
    pub(crate) async fn voice_for_call(
        &self,
        ctx: &ToolCtx,
        slug: &str,
        kind: ToolKind,
        arguments: &mut Value,
    ) -> Result<CardVoice> {
        let Some(outgoing) = outgoing_kind(slug, kind) else {
            return Ok(CardVoice::default());
        };
        let editable = editable_keys(arguments);
        let (session_id, channel_id) = (ctx.session.id, ctx.channel.id);
        let task_id = ctx.task.as_ref().map(|t| t.id);
        let to = recipients(arguments);
        let found = self
            .store
            .read(move |conn| {
                let Some(guide) = active_guide(conn)? else {
                    return Ok(None);
                };
                let settings = store::settings(conn)?;
                let context = store::last_voice_use(conn, session_id)?
                    .unwrap_or_else(|| CORE_CONTEXT.to_owned());
                let disclose = match settings.disclosure {
                    Disclosure::None => false,
                    Disclosure::Footer => true,
                    Disclosure::FooterNewContacts => {
                        let mut new = false;
                        for r in &to {
                            if !store::wrote_to_before(conn, r)? {
                                new = true;
                            }
                        }
                        new
                    }
                };
                let request = request_text(conn, session_id, channel_id, task_id)?;
                Ok(Some((guide, context, disclose, request)))
            })
            .await?;
        let Some((guide, context, disclose, request)) = found else {
            return Ok(CardVoice {
                outgoing: Some(outgoing),
                editable,
                ..CardVoice::default()
            });
        };
        if disclose {
            add_disclosure(arguments, &editable);
        }
        let flags = flags_for(arguments, &editable, &request, &guide);
        Ok(CardVoice {
            outgoing: Some(outgoing),
            editable,
            voice: Some(VoiceUse { context }),
            flags,
        })
    }

    /// `approval.edit`: new text for the fields of a draft card. The hub
    /// keeps the assistant's arguments, checks the new text again, and
    /// clears the old flags.
    pub(crate) async fn approval_edit(
        &self,
        _caller: &Caller,
        request: ApprovalEdit,
    ) -> Result<ApprovalResult> {
        if request.fields.is_empty() {
            return Err(HubError::invalid("Change at least one field."));
        }
        if request
            .fields
            .values()
            .any(|v| v.len() > EDIT_FIELD_MAX_BYTES)
        {
            return Err(HubError::invalid(format!(
                "A field is larger than {} KiB. Shorten it.",
                EDIT_FIELD_MAX_BYTES / 1024
            )));
        }
        let approval = self
            .store
            .write(move |tx| {
                let mut approval = work::approval(tx.db(), request.approval_id)?
                    .ok_or_else(|| HubError::not_found("The approval does not exist."))?;
                if approval.status != ApprovalStatus::Pending {
                    return Err(HubError::conflict(
                        "This card is already answered or closed, so it cannot change.",
                    ));
                }
                let (session_id, channel_id, task_id) =
                    (approval.session_id, approval.channel_id, approval.task_id);
                let ApprovalDetails::Connector(card) = &mut approval.details else {
                    return Err(HubError::invalid("Only a connector card has Edit."));
                };
                if card.outgoing.is_none() {
                    return Err(HubError::invalid(
                        "Only the card of a draft or send tool has Edit.",
                    ));
                }
                for key in request.fields.keys() {
                    if !card.editable.contains(key) {
                        return Err(HubError::invalid(format!(
                            "The field {key} cannot be edited. Edit one of: {}.",
                            card.editable.join(", ")
                        )));
                    }
                }
                if card.draft_arguments.is_none() {
                    card.draft_arguments = Some(card.arguments.clone());
                }
                for (key, value) in &request.fields {
                    card.arguments[key.as_str()] = Value::String(value.clone());
                }
                card.fields = crate::connectors::run::display_fields(&card.arguments);
                card.flags = match active_guide(tx.db())? {
                    Some(guide) => {
                        let request = request_text(tx.db(), session_id, channel_id, task_id)?;
                        flags_for(&card.arguments, &card.editable, &request, &guide)
                    }
                    None => Vec::new(),
                };
                tx.save_approval(&approval)?;
                Ok(approval)
            })
            .await?;
        Ok(ApprovalResult { approval })
    }
}

/// An allow of a card with flags needs "I checked the marked lines".
pub(crate) fn check_allow(approval: &Approval, allow: bool, checked: bool) -> Result<()> {
    if let ApprovalDetails::Connector(card) = &approval.details
        && allow
        && !card.flags.is_empty()
        && !checked
    {
        return Err(HubError::invalid(
            "Check the marked lines and tick \"I checked the marked lines\" before you allow this call.",
        ));
    }
    Ok(())
}

/// Stores the draft pair of an allowed draft card. When 5 edited pairs of
/// one context wait, it queues a `voice_edits` wake for Tutti and returns
/// Tutti, which the caller kicks after the commit.
pub(crate) fn record_pair_in_tx(tx: &mut Tx<'_>, approval: &Approval) -> Result<Option<ChannelId>> {
    let ApprovalDetails::Connector(card) = &approval.details else {
        return Ok(None);
    };
    if card.outgoing.is_none() || approval.status != ApprovalStatus::Allowed {
        return Ok(None);
    }
    let draft_arguments = card.draft_arguments.as_ref().unwrap_or(&card.arguments);
    let pair = DraftPair {
        approval_id: approval.id,
        connection_id: card.connection_id,
        tool: card.tool.clone(),
        context: card.voice.as_ref().map(|v| v.context.clone()),
        draft: draft_text(draft_arguments, &card.editable),
        final_text: draft_text(&card.arguments, &card.editable),
        recipients: recipients(&card.arguments),
        unchanged: !card.edited(),
        not_my_voice: card.not_my_voice,
        at: tx.now(),
    };
    tx.save_draft_pair(&pair)?;
    let Some(context) = pair.context.filter(|_| !pair.unchanged) else {
        return Ok(None);
    };
    let settings = store::settings(tx.db())?;
    if !settings.enabled || !settings.learn_from_edits {
        return Ok(None);
    }
    let waiting = store::unused_edited_pairs(tx.db(), &context)?;
    if waiting.len() < VOICE_EDITS_WAKE_PAIRS {
        return Ok(None);
    }
    let pairs = edits::edit_pairs(&waiting);
    let agreeing = edits::agreeing(&pairs);
    let tutti = conversation::tutti_channel(tx.db())?;
    tx.queue_wake(
        tutti.id,
        &WakeEvent::VoiceEdits {
            context,
            pairs,
            agreeing,
        },
    )?;
    let ids: Vec<ApprovalId> = waiting.iter().map(|p| p.approval_id).collect();
    tx.mark_pairs_used(&ids)?;
    Ok(Some(tutti.id))
}

/// "Not how I write": marks the card and its pair, and wakes Tutti once
/// with a `voice_feedback` event. Returns Tutti when it queued the wake.
pub(crate) fn not_my_voice(tx: &mut Tx<'_>, approval_id: ApprovalId) -> Result<Option<ChannelId>> {
    let mut approval = work::approval(tx.db(), approval_id)?
        .ok_or_else(|| HubError::not_found("The approval does not exist."))?;
    let ApprovalDetails::Connector(card) = &mut approval.details else {
        return Err(HubError::invalid(
            "\"Not how I write\" is only on the card of a draft.",
        ));
    };
    if card.outgoing.is_none() {
        return Err(HubError::invalid(
            "\"Not how I write\" is only on the card of a draft.",
        ));
    }
    if card.not_my_voice {
        return Ok(None);
    }
    card.not_my_voice = true;
    let context = card.voice.as_ref().map(|v| v.context.clone());
    let draft = draft_text(
        card.draft_arguments.as_ref().unwrap_or(&card.arguments),
        &card.editable,
    );
    tx.save_approval(&approval)?;
    if let Some(mut pair) = store::draft_pair(tx.db(), approval_id)? {
        pair.not_my_voice = true;
        tx.save_draft_pair(&pair)?;
    }
    let tutti = conversation::tutti_channel(tx.db())?;
    tx.queue_wake(
        tutti.id,
        &WakeEvent::VoiceFeedback {
            approval: approval_id,
            context,
            draft,
        },
    )?;
    Ok(Some(tutti.id))
}
