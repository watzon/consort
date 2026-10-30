//! Draft cards: the words of a card that asks to send or save text in the
//! user's name, and the card that `show_draft` posts.
//!
//! A connector card with `outgoing` set is a draft card. It has a title, a
//! To line, a connection line, a subject and a body, and the words Send (or
//! Save draft) and Not now. After the user answers, or after a new message
//! replaces it, it shows what happened and the text that went out. A card
//! from `show_draft` waits for nobody: the user edits it and copies the
//! text.

use consort_proto::model::{ApprovalStatus, ConnectorCard, OutgoingKind};
use serde::Deserialize;

/// The `kind` of the notice that `show_draft` posts.
pub const DRAFT_KIND: &str = "draft";

/// The title of a draft card: the subject, or "Email to <name>" when the
/// subject is empty. `None` for the address too gives "Draft".
pub fn draft_title(subject: Option<&str>, to: Option<&str>) -> String {
    if let Some(subject) = subject.map(str::trim).filter(|s| !s.is_empty()) {
        return subject.to_owned();
    }
    match to.map(recipient_name).filter(|n| !n.is_empty()) {
        Some(name) => format!("Email to {name}"),
        None => "Draft".to_owned(),
    }
}

/// The name of the first recipient. "Dana Smith <dana@example.com>" gives
/// "Dana Smith". A bare address gives the address.
pub fn recipient_name(to: &str) -> String {
    let first = to.split([',', ';']).next().unwrap_or_default().trim();
    if let Some((name, rest)) = first.split_once('<')
        && rest.contains('>')
    {
        let name = name.trim().trim_matches('"').trim();
        if !name.is_empty() {
            return name.to_owned();
        }
        return rest.split('>').next().unwrap_or_default().trim().to_owned();
    }
    first.to_owned()
}

/// What happened to a draft card that is closed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftOutcomeView {
    /// "Sent to Dana", "Not sent" or "Replaced by a new version". A card
    /// that saves a draft says "Draft saved in Gmail Work" instead of the
    /// first one.
    pub text: String,
    /// The text that went out, for a card that ran. `None` for a card that
    /// did not.
    pub final_text: Option<String>,
}

/// The outcome of a draft card, or `None` while it waits.
pub fn outcome(card: &ConnectorCard, status: ApprovalStatus) -> Option<DraftOutcomeView> {
    let outgoing = card.outgoing?;
    let (to, _) = card_to_and_subject(card);
    Some(match status {
        ApprovalStatus::Pending => return None,
        ApprovalStatus::Allowed => DraftOutcomeView {
            text: match outgoing {
                OutgoingKind::Send => match to.as_deref().map(recipient_name) {
                    Some(name) if !name.is_empty() => format!("Sent to {name}"),
                    _ => "Sent".to_owned(),
                },
                OutgoingKind::Draft => format!("Draft saved in {}", card.connection_name),
            },
            final_text: Some(card_body(card)),
        },
        ApprovalStatus::Denied | ApprovalStatus::Expired | ApprovalStatus::Canceled => {
            DraftOutcomeView {
                text: "Not sent".to_owned(),
                final_text: None,
            }
        }
        ApprovalStatus::Replaced => DraftOutcomeView {
            text: "Replaced by a new version".to_owned(),
            final_text: None,
        },
    })
}

/// The To value and the subject of a card, from its fields.
pub(crate) fn card_to_and_subject(card: &ConnectorCard) -> (Option<String>, Option<String>) {
    let field = |label: &str| {
        card.fields
            .iter()
            .find(|f| f.label.eq_ignore_ascii_case(label))
            .map(|f| f.value.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    (field("To"), field("Subject"))
}

/// The text of the body of a card, as it is now (after any edit).
pub(crate) fn card_body(card: &ConnectorCard) -> String {
    card.arguments
        .get("body")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .or_else(|| {
            card.fields
                .iter()
                .find(|f| f.label.eq_ignore_ascii_case("Body"))
                .map(|f| f.value.clone())
        })
        .unwrap_or_default()
}

/// A card that `show_draft` posted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShownDraftView {
    /// The message that holds the card. `Client::edit_draft` takes it.
    pub message_id: String,
    pub title: String,
    /// Who the text is for. `None` when the assistant gave none.
    pub to: Option<String>,
    /// The text, with the edits of the user.
    pub body: String,
    /// The user changed the text.
    pub edited: bool,
    /// The text that the assistant wrote, for a card that was edited.
    pub original_body: Option<String>,
}

/// Reads the body of a `draft` notice. `None` for any other notice.
pub fn parse_shown_draft(message_id: &str, body: &str) -> Option<ShownDraftView> {
    #[derive(Deserialize)]
    struct Parsed {
        #[serde(default)]
        kind: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        to: Option<String>,
        #[serde(default)]
        body: String,
        #[serde(default)]
        original_body: Option<String>,
    }
    let parsed: Parsed = serde_json::from_str(body).ok()?;
    if parsed.kind != "draft" || parsed.body.is_empty() {
        return None;
    }
    let edited = parsed
        .original_body
        .as_deref()
        .is_some_and(|original| original != parsed.body);
    Some(ShownDraftView {
        message_id: message_id.to_owned(),
        title: parsed.title,
        to: parsed.to,
        body: parsed.body,
        edited,
        original_body: parsed.original_body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::ids::ConnectionId;
    use consort_proto::model::{DisplayField, ToolKind};
    use serde_json::json;

    fn card(outgoing: Option<OutgoingKind>) -> ConnectorCard {
        ConnectorCard {
            connection_id: ConnectionId::new(),
            connection_name: "Gmail Work".into(),
            service_name: "Gmail".into(),
            tool: "GMAIL_SEND_EMAIL".into(),
            tool_kind: ToolKind::Change,
            fields: vec![
                DisplayField {
                    label: "To".into(),
                    value: "Dana Smith <dana@example.com>".into(),
                },
                DisplayField {
                    label: "Subject".into(),
                    value: "Friday".into(),
                },
            ],
            arguments: json!({"recipient_email": "dana@example.com", "subject": "Friday", "body": "See you."}),
            outgoing,
            editable: vec!["subject".into(), "body".into()],
            voice: None,
            flags: Vec::new(),
            draft_arguments: None,
            not_my_voice: false,
            ask_rule: None,
            replaced_by: None,
        }
    }

    #[test]
    fn the_title_is_the_subject_or_email_to_the_name() {
        assert_eq!(
            draft_title(Some("Friday plan"), Some("dana@example.com")),
            "Friday plan"
        );
        assert_eq!(
            draft_title(Some("  "), Some("Dana Smith <dana@example.com>")),
            "Email to Dana Smith"
        );
        assert_eq!(
            draft_title(None, Some("dana@example.com")),
            "Email to dana@example.com"
        );
        assert_eq!(
            draft_title(None, Some("<dana@example.com>")),
            "Email to dana@example.com"
        );
        assert_eq!(
            draft_title(Some(""), Some("\"Dana S.\" <d@x.com>, Eli <e@x.com>")),
            "Email to Dana S."
        );
        assert_eq!(draft_title(None, None), "Draft");
        assert_eq!(draft_title(Some(" "), Some(" ")), "Draft");
    }

    #[test]
    fn a_closed_card_says_what_happened_in_the_three_phrases() {
        let send = card(Some(OutgoingKind::Send));
        let sent = outcome(&send, ApprovalStatus::Allowed).unwrap();
        assert_eq!(sent.text, "Sent to Dana Smith");
        assert_eq!(sent.final_text.as_deref(), Some("See you."));
        let denied = outcome(&send, ApprovalStatus::Denied).unwrap();
        assert_eq!(denied.text, "Not sent");
        assert_eq!(denied.final_text, None);
        assert_eq!(
            outcome(&send, ApprovalStatus::Expired).unwrap().text,
            "Not sent"
        );
        let replaced = outcome(&send, ApprovalStatus::Replaced).unwrap();
        assert_eq!(replaced.text, "Replaced by a new version");
        assert_eq!(replaced.final_text, None);
        assert_eq!(outcome(&send, ApprovalStatus::Pending), None);
    }

    #[test]
    fn a_card_that_saves_a_draft_says_where_and_other_cards_have_no_outcome() {
        let draft = card(Some(OutgoingKind::Draft));
        assert_eq!(
            outcome(&draft, ApprovalStatus::Allowed).unwrap().text,
            "Draft saved in Gmail Work"
        );
        assert_eq!(outcome(&card(None), ApprovalStatus::Allowed), None);
    }

    #[test]
    fn the_final_text_is_the_edited_body() {
        let mut edited = card(Some(OutgoingKind::Send));
        edited.draft_arguments = Some(edited.arguments.clone());
        edited.arguments["body"] = json!("See you at six.");
        let sent = outcome(&edited, ApprovalStatus::Allowed).unwrap();
        assert_eq!(sent.final_text.as_deref(), Some("See you at six."));
    }

    #[test]
    fn a_shown_draft_reads_from_its_notice_and_knows_an_edit() {
        let body = json!({"kind": "draft", "text": "x", "title": "Claim", "to": "Dr. Meyer", "body": "Dear Dr. Meyer"}).to_string();
        let view = parse_shown_draft("msg_1", &body).unwrap();
        assert_eq!(view.title, "Claim");
        assert_eq!(view.to.as_deref(), Some("Dr. Meyer"));
        assert!(!view.edited);
        let body = json!({"kind": "draft", "title": "Claim", "body": "Dear Sir", "original_body": "Dear Dr. Meyer"}).to_string();
        let view = parse_shown_draft("msg_1", &body).unwrap();
        assert!(view.edited);
        assert_eq!(view.original_body.as_deref(), Some("Dear Dr. Meyer"));
        assert!(parse_shown_draft("m", r#"{"kind":"turn_failed","text":"x"}"#).is_none());
    }
}
