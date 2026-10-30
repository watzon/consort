//! The corner notifications. Up to three show at once, with the newest at
//! the bottom. An error stays until the user dismisses it, and the errors
//! over three wait. An information notice leaves after six seconds and never
//! waits. Each notice has a serial, so the timer of an older notice never
//! clears a newer one.
//!
//! A notice is for something that the user did in the app and that has no
//! place in the conversation: a save, a rename, an upload, a hub that did
//! not answer. The assistant that could not answer a message is an in-chat
//! error card, and a message that the hub refused stays in the lane of the
//! user (DECISIONS 732, 733 and 734).

use std::{collections::VecDeque, time::Duration};

/// How long an info notice stays. An error notice stays until the user
/// dismisses it.
pub const INFO_TIMEOUT: Duration = Duration::from_secs(6);

/// How many notices show at once. The errors over this number wait.
pub const MAX_SHOWN: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    /// A short fact. It times out.
    Info,
    /// Something went wrong. It stays until the user dismisses it.
    Error,
}

/// The button of a notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    /// Puts the text of a message that the hub refused back in its box. The
    /// notice has the ID of the failure.
    PutBack,
    /// Loads the older messages of this channel again.
    LoadOlder(String),
}

impl NoticeAction {
    /// The label of the button.
    pub fn label(&self) -> &'static str {
        match self {
            Self::PutBack => "Put back in the message box",
            Self::LoadOlder(_) => "Try again",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub serial: u64,
    pub kind: NoticeKind,
    /// What the caller wrote: what happened, and what to do next.
    pub message: String,
    /// More text under the message, such as the text of a message that the
    /// hub refused. It can be long, so the notice scrolls it.
    pub detail: Option<String>,
    /// The ID of the failed send in the client that this notice shows. The
    /// client keeps that failure until the user dismisses this notice.
    pub failure: Option<String>,
    pub action: Option<NoticeAction>,
}

impl Notice {
    /// The title: for an error, the first sentence of the message, without
    /// its last period. An information notice is one line, so its title is
    /// the whole message.
    pub fn title(&self) -> &str {
        match self.kind {
            NoticeKind::Info => &self.message,
            NoticeKind::Error => split_message(&self.message).0,
        }
    }

    /// The text under the title of an error: the rest of the message. `None`
    /// when the message is one sentence, and for an information notice.
    pub fn text(&self) -> Option<&str> {
        match self.kind {
            NoticeKind::Info => None,
            NoticeKind::Error => split_message(&self.message).1,
        }
    }
}

/// Splits an error message into its title and the text under it. The title
/// is the first sentence, without its last period. A message of one sentence
/// has a title and no text.
pub fn split_message(message: &str) -> (&str, Option<&str>) {
    let message = message.trim();
    let end = message
        .char_indices()
        .find(|&(i, c)| {
            c == '.'
                && message[i + c.len_utf8()..]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
        })
        .map(|(i, _)| i);
    match end {
        Some(i) => {
            let rest = message[i + 1..].trim();
            (&message[..i], (!rest.is_empty()).then_some(rest))
        }
        None => (message.trim_end_matches('.'), None),
    }
}

#[derive(Debug, Default)]
pub struct Notices {
    last_serial: u64,
    /// The notices on screen first (at most [`MAX_SHOWN`], the oldest at the
    /// top), then the errors that wait.
    queue: VecDeque<Notice>,
}

impl Notices {
    fn push(&mut self, kind: NoticeKind, message: String, extra: Extra) -> Notice {
        self.last_serial += 1;
        let notice = Notice {
            serial: self.last_serial,
            kind,
            message,
            detail: extra.detail,
            failure: extra.failure,
            action: extra.action,
        };
        self.queue.push_back(notice.clone());
        notice
    }

    /// The position of the oldest information notice among those on screen.
    fn oldest_info_shown(&self) -> Option<usize> {
        self.queue
            .iter()
            .take(MAX_SHOWN)
            .position(|n| n.kind == NoticeKind::Info)
    }

    /// Makes room for an error: when three notices show, an information
    /// notice leaves, so the error shows at once.
    fn make_room_for_error(&mut self) {
        if self.queue.len() >= MAX_SHOWN
            && let Some(at) = self.oldest_info_shown()
        {
            self.queue.remove(at);
        }
    }

    /// Shows a notice and returns it, or returns `None` when it does not
    /// show.
    ///
    /// - An info notice shows below the others, and a copy of it that is on
    ///   screen already leaves first. With three notices on screen, it takes
    ///   the place of the oldest info notice. When all three are errors it
    ///   does not show, so no error is lost.
    /// - An error shows at once while fewer than three notices show. When an
    ///   info notice takes the third place, the info notice leaves. Otherwise
    ///   it waits behind the others, and shows when one is dismissed.
    /// - An error with the same text as one that is there already is not
    ///   added twice.
    pub fn show(&mut self, kind: NoticeKind, message: String) -> Option<Notice> {
        self.show_with(kind, message, Extra::default())
    }

    /// The same as [`Notices::show`], with a button.
    pub fn show_with_action(
        &mut self,
        kind: NoticeKind,
        message: String,
        action: NoticeAction,
    ) -> Option<Notice> {
        self.show_with(
            kind,
            message,
            Extra {
                action: Some(action),
                ..Extra::default()
            },
        )
    }

    fn show_with(&mut self, kind: NoticeKind, message: String, extra: Extra) -> Option<Notice> {
        match kind {
            NoticeKind::Info => {
                self.queue
                    .retain(|n| !(n.kind == NoticeKind::Info && n.message == message));
                if self.queue.len() >= MAX_SHOWN {
                    let at = self.oldest_info_shown()?;
                    self.queue.remove(at);
                    // An error that waited may take the place. Information
                    // never waits, so it does not show then.
                    if self.queue.len() >= MAX_SHOWN {
                        return None;
                    }
                }
            }
            NoticeKind::Error => {
                if self.queue.iter().any(|n| {
                    n.kind == NoticeKind::Error && n.failure.is_none() && n.message == message
                }) {
                    return None;
                }
                self.make_room_for_error();
            }
        }
        Some(self.push(kind, message, extra))
    }

    /// Shows a message that the hub refused, as an error. `failure` is the
    /// ID of the failed send in the client, and `body` is the text of the
    /// message. A failure that is in the queue already is not added twice.
    pub fn show_failure(
        &mut self,
        failure: String,
        message: String,
        body: String,
    ) -> Option<Notice> {
        if self
            .queue
            .iter()
            .any(|n| n.failure.as_deref() == Some(failure.as_str()))
        {
            return None;
        }
        self.make_room_for_error();
        Some(self.push(
            NoticeKind::Error,
            message,
            Extra {
                detail: Some(body),
                failure: Some(failure),
                action: Some(NoticeAction::PutBack),
            },
        ))
    }

    /// Clears the information notice with this serial, when it is still
    /// there. A newer notice stays, and so does an error: its timer never
    /// clears it.
    pub fn clear_if(&mut self, serial: u64) -> bool {
        match self
            .queue
            .iter()
            .position(|n| n.serial == serial && n.kind == NoticeKind::Info)
        {
            Some(at) => {
                self.queue.remove(at);
                true
            }
            None => false,
        }
    }

    /// Dismisses the notice with this serial and returns it. The next error
    /// that waits, if there is one, shows.
    pub fn dismiss(&mut self, serial: u64) -> Option<Notice> {
        let at = self.queue.iter().position(|n| n.serial == serial)?;
        self.queue.remove(at)
    }

    /// Dismisses the notice of a failed send and returns it.
    pub fn dismiss_failure(&mut self, failure: &str) -> Option<Notice> {
        let at = self
            .queue
            .iter()
            .position(|n| n.failure.as_deref() == Some(failure))?;
        self.queue.remove(at)
    }

    /// The notices on screen, the oldest first. The view draws the newest at
    /// the bottom.
    pub fn visible(&self) -> impl Iterator<Item = &Notice> {
        self.queue.iter().take(MAX_SHOWN)
    }

    /// The notice with this serial, on screen or waiting.
    pub fn get(&self, serial: u64) -> Option<&Notice> {
        self.queue.iter().find(|n| n.serial == serial)
    }

    /// How many errors wait behind the notices on screen.
    pub fn waiting(&self) -> usize {
        self.queue.len().saturating_sub(MAX_SHOWN)
    }
}

#[derive(Default)]
struct Extra {
    detail: Option<String>,
    failure: Option<String>,
    action: Option<NoticeAction>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(notices: &Notices) -> Vec<&str> {
        notices.visible().map(|n| n.message.as_str()).collect()
    }

    fn error(notices: &mut Notices, text: &str) -> Option<Notice> {
        notices.show(NoticeKind::Error, text.into())
    }

    fn info(notices: &mut Notices, text: &str) -> Option<Notice> {
        notices.show(NoticeKind::Info, text.into())
    }

    #[test]
    fn the_timer_of_an_older_notice_does_not_clear_a_newer_one() {
        let mut notices = Notices::default();
        let first = info(&mut notices, "First").expect("shown");
        let second = info(&mut notices, "Second").expect("shown");
        assert_ne!(first.serial, second.serial);
        // Both show, the newest at the bottom.
        assert_eq!(messages(&notices), ["First", "Second"]);
        // The timer of the first notice clears the first and not the second.
        assert!(notices.clear_if(first.serial));
        assert_eq!(messages(&notices), ["Second"]);
        assert!(!notices.clear_if(first.serial), "it is gone already");
        assert!(notices.clear_if(second.serial));
        assert!(notices.visible().last().is_none());
    }

    #[test]
    fn the_same_information_shows_once_and_moves_to_the_bottom() {
        let mut notices = Notices::default();
        let copied = info(&mut notices, "Copied the path.").expect("shown");
        info(&mut notices, "Routine saved.").expect("shown");
        let again = info(&mut notices, "Copied the path.").expect("shown");
        assert_eq!(messages(&notices), ["Routine saved.", "Copied the path."]);
        // The timer of the first copy finds nothing to clear.
        assert!(!notices.clear_if(copied.serial));
        assert!(notices.clear_if(again.serial));
    }

    #[test]
    fn an_error_stays_until_it_is_dismissed_and_its_timer_clears_nothing() {
        let mut notices = Notices::default();
        let failed = error(&mut notices, "The hub is down").expect("shown");
        // Information shows next to the error and does not hide it.
        let copied = info(&mut notices, "Copied").expect("shown");
        assert_eq!(messages(&notices), ["The hub is down", "Copied"]);
        assert!(!notices.clear_if(failed.serial), "an error has no timer");
        assert_eq!(notices.dismiss(failed.serial), Some(failed));
        assert_eq!(messages(&notices), ["Copied"]);
        assert!(notices.clear_if(copied.serial));
        assert!(notices.visible().last().is_none());
    }

    #[test]
    fn three_show_at_most_the_newest_at_the_bottom_and_the_others_wait() {
        let mut notices = Notices::default();
        let one = error(&mut notices, "One").expect("shown");
        error(&mut notices, "Two");
        error(&mut notices, "Three");
        error(&mut notices, "Four").expect("queued");
        error(&mut notices, "Five").expect("queued");
        assert_eq!(messages(&notices), ["One", "Two", "Three"]);
        assert_eq!(notices.waiting(), 2);
        assert_eq!(
            notices.visible().last().map(|n| n.message.as_str()),
            Some("Three")
        );
        // A dismissed error makes room: the next one that waited shows, at
        // the bottom.
        notices.dismiss(one.serial);
        assert_eq!(messages(&notices), ["Two", "Three", "Four"]);
        assert_eq!(notices.waiting(), 1);
        // A card in the middle can go too.
        let two = notices.visible().next().expect("two").serial;
        notices.dismiss(two);
        assert_eq!(messages(&notices), ["Three", "Four", "Five"]);
        assert_eq!(notices.waiting(), 0);
    }

    #[test]
    fn information_never_waits_and_never_pushes_an_error_out() {
        let mut notices = Notices::default();
        error(&mut notices, "One");
        error(&mut notices, "Two");
        error(&mut notices, "Three");
        // Three errors fill the stack: information does not show.
        assert!(info(&mut notices, "Copied").is_none());
        assert_eq!(messages(&notices), ["One", "Two", "Three"]);
        assert_eq!(notices.waiting(), 0);
    }

    #[test]
    fn an_error_takes_the_place_of_the_oldest_information_when_the_stack_is_full() {
        let mut notices = Notices::default();
        info(&mut notices, "Saved A.");
        info(&mut notices, "Saved B.");
        info(&mut notices, "Saved C.");
        assert_eq!(messages(&notices), ["Saved A.", "Saved B.", "Saved C."]);
        // Information replaces the oldest information.
        info(&mut notices, "Saved D.").expect("shown");
        assert_eq!(messages(&notices), ["Saved B.", "Saved C.", "Saved D."]);
        // An error shows at once: the oldest information leaves.
        error(&mut notices, "The hub is down").expect("shown");
        assert_eq!(
            messages(&notices),
            ["Saved C.", "Saved D.", "The hub is down"]
        );
        assert_eq!(notices.waiting(), 0);
    }

    #[test]
    fn the_same_error_is_not_added_twice() {
        let mut notices = Notices::default();
        assert!(error(&mut notices, "Failed").is_some());
        assert!(error(&mut notices, "Failed").is_none());
        assert_eq!(messages(&notices), ["Failed"]);
    }

    #[test]
    fn a_title_is_the_first_sentence_and_the_text_is_the_rest() {
        let mut notices = Notices::default();
        let shown = error(
            &mut notices,
            "Could not upload budget-2026.xlsx. The file is larger than 100 MB. Choose a smaller file.",
        )
        .expect("shown");
        assert_eq!(shown.title(), "Could not upload budget-2026.xlsx");
        assert_eq!(
            shown.text(),
            Some("The file is larger than 100 MB. Choose a smaller file.")
        );
        // One sentence is a title with no text, and its period goes.
        let single = error(&mut notices, "The hub did not answer in time.").expect("shown");
        assert_eq!(single.title(), "The hub did not answer in time");
        assert_eq!(single.text(), None);
        // A dot inside a word or a number does not end the title.
        assert_eq!(
            split_message("Saved v1.2 and budget.xlsx. Done."),
            ("Saved v1.2 and budget.xlsx", Some("Done."))
        );
        // Information is one line.
        let copied = info(&mut notices, "Copied the path.").expect("shown");
        assert_eq!(copied.title(), "Copied the path.");
        assert_eq!(copied.text(), None);
    }

    #[test]
    fn a_notice_can_carry_a_button() {
        let mut notices = Notices::default();
        let shown = notices
            .show_with_action(
                NoticeKind::Error,
                "Could not load older messages. The hub did not answer.".into(),
                NoticeAction::LoadOlder("chn_1".into()),
            )
            .expect("shown");
        assert_eq!(
            shown.action.as_ref().map(NoticeAction::label),
            Some("Try again")
        );
        assert_eq!(
            notices.get(shown.serial).and_then(|n| n.action.clone()),
            Some(NoticeAction::LoadOlder("chn_1".into()))
        );
    }

    #[test]
    fn a_failed_send_keeps_its_text_and_its_id_and_shows_once() {
        let mut notices = Notices::default();
        let shown = notices
            .show_failure(
                "c-1".into(),
                "The hub said no.".into(),
                "My long text".into(),
            )
            .expect("shown");
        assert_eq!(shown.kind, NoticeKind::Error);
        assert_eq!(shown.failure.as_deref(), Some("c-1"));
        assert_eq!(shown.detail.as_deref(), Some("My long text"));
        assert_eq!(shown.action, Some(NoticeAction::PutBack));
        assert!(
            notices
                .show_failure("c-1".into(), "again".into(), "text".into())
                .is_none(),
            "the same failure is not added twice"
        );
        // A second failure with the same message is another message.
        assert!(
            notices
                .show_failure("c-2".into(), "The hub said no.".into(), "Other text".into())
                .is_some()
        );
        assert_eq!(notices.visible().count(), 2);
        let first = notices.dismiss_failure("c-1").expect("first");
        assert_eq!(
            first.failure.as_deref(),
            Some("c-1"),
            "the caller can dismiss it in the client"
        );
        assert_eq!(
            notices.visible().last().and_then(|n| n.detail.as_deref()),
            Some("Other text")
        );
    }

    #[test]
    fn serials_only_go_up() {
        let mut notices = Notices::default();
        let a = info(&mut notices, "a").expect("shown");
        notices.dismiss(a.serial);
        let b = info(&mut notices, "b").expect("shown");
        assert!(b.serial > a.serial);
        // A late timer of the dismissed notice finds nothing to clear.
        assert!(!notices.clear_if(a.serial));
    }
}
