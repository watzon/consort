//! The card of a draft or send tool: Edit, the voice line, the disclosure
//! line, the commitment flags with "I checked the marked lines", the draft
//! pairs and "Not how I write".

use consort_proto::client_api::{ApprovalEdit, ApprovalResult};
use consort_proto::ids::{ApprovalId, ChannelId, ConnectionId, SessionId, TaskId};
use consort_proto::model::{
    Approval, ApprovalDetails, ApprovalStatus, Author, CORE_CONTEXT, CommitmentFlag, ConnectorCard,
    DISCLOSURE_LINE, Disclosure, DraftPair, EDITABLE_ARGUMENTS, OutgoingKind, ProactivityLevel,
    TaskCreator, ToolKind, VOICE_EDITS_WAKE_PAIRS, VoiceGuide, VoiceUse, outgoing_kind,
    placeholders,
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
    /// "Write in my voice" is on.
    pub voice_on: bool,
}

impl CardVoice {
    /// Whether the hub must show the card, whatever the connection's rule
    /// and earlier allows say: a call with flags, and every send while the
    /// voice is on. A draft keeps the connection's rule.
    pub(crate) fn must_ask(&self) -> bool {
        !self.flags.is_empty() || (self.voice_on && self.outgoing == Some(OutgoingKind::Send))
    }

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
pub(crate) fn editable_keys(arguments: &Value) -> Vec<String> {
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

/// Whether an argument key holds a recipient, an ID, a URL or another value
/// that is not text in the user's name. The scan skips such keys at every
/// depth.
fn is_address_or_id(key: &str) -> bool {
    const KEYS: &[&str] = &[
        "to",
        "cc",
        "bcc",
        "from",
        "sender",
        "reply_to",
        "recipient",
        "recipients",
        "channel",
        "channels",
        "user",
        "users",
        "username",
        "thread_ts",
        "ts",
        "id",
        "ids",
        "url",
        "urls",
        "href",
        "link",
        "links",
        "email",
        "emails",
        "type",
        "mimetype",
        "style",
        "icon_emoji",
        "team",
        "workspace",
        "visibility",
    ];
    let k = key.to_ascii_lowercase();
    KEYS.contains(&k.as_str())
        || [
            "_id", "_ids", "_url", "_urls", "_email", "_emails", "_ts", "_type", "_at", "_uri",
        ]
        .iter()
        .any(|s| k.ends_with(s))
        || k.starts_with("recipient")
        || k.starts_with("to_")
}

/// One string of a call's arguments that the scan reads.
struct TextValue {
    /// The key, or the path of a nested value such as `blocks.0.text.text`.
    path: String,
    /// The label of its top-level key.
    label: String,
    value: String,
}

/// Every string value of the arguments, at any depth, except recipients,
/// IDs and URLs: Slack `blocks`, LinkedIn `commentary`, a nested
/// `body.content`. The editable fields come first, in their order.
fn text_values(arguments: &Value) -> Vec<TextValue> {
    fn walk(value: &Value, path: &mut Vec<String>, out: &mut Vec<TextValue>) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map {
                    if is_address_or_id(key) {
                        continue;
                    }
                    path.push(key.clone());
                    walk(inner, path, out);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (i, inner) in items.iter().enumerate() {
                    path.push(i.to_string());
                    walk(inner, path, out);
                    path.pop();
                }
            }
            Value::String(text) if !path.is_empty() && !text.trim().is_empty() => {
                out.push(TextValue {
                    path: path.join("."),
                    label: label_of(&path[0]),
                    value: text.clone(),
                });
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(arguments, &mut Vec::new(), &mut out);
    let rank = |path: &str| {
        EDITABLE_ARGUMENTS
            .iter()
            .position(|k| *k == path)
            .unwrap_or(EDITABLE_ARGUMENTS.len())
    };
    out.sort_by_key(|t| rank(&t.path));
    out
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

/// The text fields of a draft without the subject, each alone.
fn bodies(arguments: &Value, keys: &[String]) -> Vec<String> {
    keys.iter()
        .filter(|k| *k != "subject")
        .filter_map(|k| arguments.get(k)?.as_str().map(str::to_owned))
        .filter(|t| !t.trim().is_empty())
        .collect()
}

/// A user text without the lines that quote other text (`> ...`), such
/// as a pasted mail.
fn own_words(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('>'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The user's own words in the open turn of a coordinator session: the
/// user messages, the typed answers to decisions and the "Try it" request.
/// When the turn has a user message, the newest user messages of the
/// channel count too. A turn that no user text started (a proactive turn,
/// a task update) gives an empty text.
pub(crate) fn turn_user_text(
    conn: &Db,
    session_id: SessionId,
    channel_id: ChannelId,
) -> Result<String> {
    let row = coord_store::coordinator(conn, channel_id)?;
    if row.session_id != Some(session_id) || !row.turn_open {
        return Ok(String::new());
    }
    let mut parts = Vec::new();
    let mut user_message = false;
    for event in &row.turn_events {
        match event {
            WakeEvent::UserMessage { text, .. } => {
                parts.push(own_words(text));
                user_message = true;
            }
            WakeEvent::DecisionAnswered {
                text: Some(text), ..
            } => parts.push(own_words(text)),
            WakeEvent::VoiceSample { request, .. } => parts.push(own_words(request)),
            _ => {}
        }
    }
    if user_message {
        let (messages, _) =
            conversation::messages_page(conn, channel_id, None, None, 50, usize::MAX)?;
        parts.extend(
            messages
                .into_iter()
                .filter(|m| m.author == Author::User)
                .take(REQUEST_MESSAGES)
                .map(|m| own_words(&m.body)),
        );
    }
    Ok(parts.join("\n"))
}

/// What the user asked for, in the user's own words only. For a task: the
/// user text of the coordinator turn that started it (or its brief, when
/// the user wrote the task in a client) and the user's replies in its
/// thread. For a coordinator: the user text of its open turn. Never a brief
/// or a title that a model wrote, a `message_channel` text, quoted mail or
/// tool output.
fn request_text(
    conn: &Db,
    session_id: SessionId,
    channel_id: ChannelId,
    task_id: Option<TaskId>,
) -> Result<String> {
    let Some(task_id) = task_id else {
        return turn_user_text(conn, session_id, channel_id);
    };
    let task = work::task_or_404(conn, task_id)?;
    let mut parts = Vec::new();
    if task.created_by == TaskCreator::User {
        parts.push(own_words(&task.title));
        parts.push(own_words(&task.brief));
    }
    if let Some(text) = store::task_request(conn, task_id)? {
        parts.push(text);
    }
    for message in conversation::thread_replies(conn, task.card_message_id)? {
        if message.author == Author::User {
            parts.push(own_words(&message.body));
        }
    }
    Ok(parts.join("\n"))
}

/// The flags of a call's text: every string value, nested ones too.
fn flags_for(arguments: &Value, request: &str, guide: &VoiceGuide) -> Vec<CommitmentFlag> {
    let values = text_values(arguments);
    let fields: Vec<ScanField<'_>> = values
        .iter()
        .map(|t| ScanField {
            key: &t.path,
            label: &t.label,
            value: &t.value,
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
    /// arguments, names the voice that wrote the text, and scans every
    /// string of the arguments.
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
        let (voice_on, found) = self
            .store
            .read(move |conn| {
                let settings = store::settings(conn)?;
                let Some(guide) = active_guide(conn)? else {
                    return Ok((settings.enabled, None));
                };
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
                Ok((true, Some((guide, context, disclose, request))))
            })
            .await?;
        let Some((guide, context, disclose, request)) = found else {
            return Ok(CardVoice {
                outgoing: Some(outgoing),
                editable,
                voice_on,
                ..CardVoice::default()
            });
        };
        if disclose {
            add_disclosure(arguments, &editable);
        }
        let flags = flags_for(arguments, &request, &guide);
        Ok(CardVoice {
            outgoing: Some(outgoing),
            editable,
            voice: Some(VoiceUse { context }),
            flags,
            voice_on,
        })
    }

    /// `approval.edit`: new text for the fields of a draft card. The hub
    /// keeps the assistant's arguments, checks the new text again, and
    /// clears the old flags. The new flags have a new review hash, so an
    /// earlier "I checked the marked lines" no longer holds.
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
                        flags_for(&card.arguments, &request, &guide)
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

/// An allow of a card with flags needs "I checked the marked lines" for
/// the card as it is now: `checked_hash` must be the card's review hash.
pub(crate) fn check_allow(
    approval: &Approval,
    allow: bool,
    checked: bool,
    checked_hash: Option<&str>,
) -> Result<()> {
    let ApprovalDetails::Connector(card) = &approval.details else {
        return Ok(());
    };
    if !allow || card.flags.is_empty() {
        return Ok(());
    }
    if !checked {
        return Err(HubError::invalid(
            "Check the marked lines and tick \"I checked the marked lines\" before you allow this call.",
        ));
    }
    if checked_hash != Some(card.review_hash().as_str()) {
        return Err(HubError::conflict(
            "The text or the marked lines of this card changed after you ticked \"I checked the marked lines\". Check the new lines and tick the box again.",
        ));
    }
    Ok(())
}

/// Whether the Tutti coordinator may make offers about the voice: its
/// proactivity level is not Off. With Off, edited pairs wait.
fn voice_offers_allowed(conn: &Db) -> Result<bool> {
    Ok(conversation::tutti_channel(conn)?.proactivity_level() != ProactivityLevel::Off)
}

/// Queues a `voice_edits` wake when 5 edited pairs of one context wait and
/// Tutti may make offers. Returns Tutti, which the caller kicks after the
/// commit.
fn wake_for_edits(tx: &mut Tx<'_>, context: String) -> Result<Option<ChannelId>> {
    let settings = store::settings(tx.db())?;
    if !settings.enabled || !settings.learn_from_edits || !voice_offers_allowed(tx.db())? {
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
        final_bodies: bodies(&card.arguments, &card.editable),
        recipients: recipients(&card.arguments),
        unchanged: !card.edited(),
        not_my_voice: card.not_my_voice,
        at: tx.now(),
    };
    tx.save_draft_pair(&pair)?;
    match pair.context.filter(|_| !pair.unchanged) {
        Some(context) => wake_for_edits(tx, context),
        None => Ok(None),
    }
}

/// Stores a draft or send that ran with no card (the connection's rule or
/// an earlier allow let it run). The user saw no draft, so it counts as
/// sent unchanged: the assistant's text, never used for learning.
pub(crate) fn record_unasked_pair(
    tx: &mut Tx<'_>,
    connection_id: ConnectionId,
    tool: &str,
    arguments: &Value,
    context: Option<String>,
) -> Result<()> {
    let keys = editable_keys(arguments);
    let text = draft_text(arguments, &keys);
    if text.trim().is_empty() {
        return Ok(());
    }
    tx.save_draft_pair(&DraftPair {
        approval_id: ApprovalId::new(),
        connection_id,
        tool: tool.to_owned(),
        context,
        draft: text.clone(),
        final_text: text,
        final_bodies: bodies(arguments, &keys),
        recipients: recipients(arguments),
        unchanged: true,
        not_my_voice: false,
        at: tx.now(),
    })
}

/// "Not how I write": marks the card and its pair, and wakes Tutti once
/// with a `voice_feedback` event. Returns Tutti when it queued the wake.
/// The draft in the event has placeholders for addresses, phone numbers
/// and links.
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
    let draft = placeholders(&draft_text(
        card.draft_arguments.as_ref().unwrap_or(&card.arguments),
        &card.editable,
    ));
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
