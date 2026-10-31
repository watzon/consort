//! The status line above the composer when the assistant cannot answer.
//!
//! The hub keeps this state live on the channel (`coordinator_blocked`). It
//! posts no message for it. The line shows only while a message of the user
//! waits for an answer, or while a turn cannot start, and it goes away when
//! the hub clears the state. The hub waits for a grace period before it sets
//! the state, so a start of the hub shows nothing.

use consort_proto::model::{
    Author, CoordinatorBlocked, CoordinatorBlockedKind, Message, MessageKind,
};

/// The kinds of notice that older hubs posted for this state. The app does
/// not show them, because the state is live and the old rows would stay.
pub const RETIRED_NOTICE_KINDS: [&str; 2] = ["assistant_unavailable", "assistant_signed_out"];

/// What the status line shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssistantStatusView {
    /// What is wrong, in a full sentence.
    pub text: String,
    /// The account that is signed out. The line then has Sign in again.
    pub account: Option<String>,
}

/// True for an old notice row of a retired kind.
pub fn is_retired_notice(message: &Message) -> bool {
    if message.kind != MessageKind::Notice {
        return false;
    }
    #[derive(serde::Deserialize)]
    struct Kind {
        #[serde(default)]
        kind: String,
    }
    serde_json::from_str::<Kind>(&message.body)
        .is_ok_and(|parsed| RETIRED_NOTICE_KINDS.contains(&parsed.kind.as_str()))
}

/// True when the newest message of the user in the timeline has no answer of
/// the assistant after it. Notices and replies in threads do not count.
pub fn user_message_waits<'a>(messages: impl IntoIterator<Item = &'a Message>) -> bool {
    let mut newest_user = None;
    let mut newest_answer = None;
    for message in messages {
        if message.deleted_at.is_some()
            || message.thread_root_id.is_some()
            || message.kind != MessageKind::Text
        {
            continue;
        }
        let slot = match message.author {
            Author::User => &mut newest_user,
            Author::Assistant => &mut newest_answer,
            Author::System => continue,
        };
        if slot.is_none_or(|at| message.created_at > at) {
            *slot = Some(message.created_at);
        }
    }
    match (newest_user, newest_answer) {
        (Some(user), Some(answer)) => user > answer,
        (Some(_), None) => true,
        _ => false,
    }
}

/// The status line of a channel, or `None` when it shows nothing.
pub fn status_view(blocked: &CoordinatorBlocked, user_waits: bool) -> Option<AssistantStatusView> {
    // A turn that cannot start is worth a line, whoever started it.
    if !user_waits && blocked.kind != CoordinatorBlockedKind::StartFailed {
        return None;
    }
    Some(AssistantStatusView {
        text: blocked.reason.clone(),
        account: blocked
            .account
            .clone()
            .filter(|a| !a.trim().is_empty())
            .filter(|_| blocked.kind == CoordinatorBlockedKind::SignedOut),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::ids::{ChannelId, MessageId};
    use consort_proto::time::Timestamp;

    fn message(author: Author, kind: MessageKind, at: i64, body: &str) -> Message {
        Message {
            id: MessageId::new(),
            channel_id: ChannelId::new(),
            thread_root_id: None,
            author,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            proactive: None,
            suggestions: Vec::new(),
            kind,
            body: body.to_owned(),
            attachments: Vec::new(),
            reactions: Vec::new(),
            created_at: Timestamp::from_millis(at),
            edited_at: None,
            deleted_at: None,
            reply_count: 0,
            last_reply_at: None,
            source_message_id: None,
            client_message_id: None,
            in_timeline: false,
        }
    }

    fn blocked(kind: CoordinatorBlockedKind, account: Option<&str>) -> CoordinatorBlocked {
        CoordinatorBlocked {
            kind,
            reason: "No online worker has the account Claude Work.".to_owned(),
            account: account.map(str::to_owned),
        }
    }

    #[test]
    fn old_rows_of_the_two_kinds_are_retired() {
        let old = |kind: &str| {
            message(
                Author::System,
                MessageKind::Notice,
                1,
                &format!(r#"{{"kind":"{kind}","text":"x"}}"#),
            )
        };
        assert!(is_retired_notice(&old("assistant_unavailable")));
        assert!(is_retired_notice(&old("assistant_signed_out")));
        assert!(!is_retired_notice(&old("turn_failed")));
        assert!(!is_retired_notice(&message(
            Author::System,
            MessageKind::Notice,
            1,
            "a plain body"
        )));
        // A text message that holds the same JSON is not a notice.
        assert!(!is_retired_notice(&message(
            Author::User,
            MessageKind::Text,
            1,
            r#"{"kind":"assistant_unavailable","text":"x"}"#
        )));
    }

    #[test]
    fn a_message_waits_until_the_assistant_answers_after_it() {
        let user = |at| message(Author::User, MessageKind::Text, at, "hi");
        let answer = |at| message(Author::Assistant, MessageKind::Text, at, "hello");
        assert!(!user_message_waits([]));
        assert!(user_message_waits([&user(1)]));
        assert!(!user_message_waits([&user(1), &answer(2)]));
        assert!(user_message_waits([&user(1), &answer(2), &user(3)]));
        // A notice, and a reply in a thread, answer nothing.
        let notice = message(Author::System, MessageKind::Notice, 4, "{}");
        let mut reply = answer(5);
        reply.thread_root_id = Some(MessageId::new());
        assert!(user_message_waits([&user(3), &notice, &reply]));
    }

    #[test]
    fn the_line_shows_only_while_a_message_waits_or_a_turn_cannot_start() {
        let no_place = blocked(CoordinatorBlockedKind::NoPlacement, None);
        assert!(status_view(&no_place, false).is_none());
        let line = status_view(&no_place, true).unwrap();
        assert_eq!(line.text, "No online worker has the account Claude Work.");
        assert!(line.account.is_none());
        // A start that fails shows without a waiting message.
        let failed = blocked(CoordinatorBlockedKind::StartFailed, None);
        assert!(status_view(&failed, false).is_some());
    }

    #[test]
    fn only_a_signed_out_account_gets_sign_in_again() {
        let signed_out = blocked(CoordinatorBlockedKind::SignedOut, Some("Claude Work"));
        let line = status_view(&signed_out, true).unwrap();
        assert_eq!(line.account.as_deref(), Some("Claude Work"));
        let other = blocked(CoordinatorBlockedKind::NoPlacement, Some("Claude Work"));
        assert!(status_view(&other, true).unwrap().account.is_none());
        let empty = blocked(CoordinatorBlockedKind::SignedOut, Some("  "));
        assert!(status_view(&empty, true).unwrap().account.is_none());
    }
}
