//! The notice toast. One notice shows at a time. Errors queue: a second error
//! waits behind the first and shows when the user dismisses it. Each notice
//! has a serial, so the timer of an older notice never clears a newer one.

use std::{collections::VecDeque, time::Duration};

/// How long an info notice stays. An error notice stays until the user
/// dismisses it.
pub const INFO_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    /// A short fact. It times out.
    Info,
    /// Something went wrong. It stays until the user dismisses it.
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub serial: u64,
    pub kind: NoticeKind,
    pub message: String,
    /// More text under the message, such as the text of a message that the
    /// hub refused. It can be long, so the toast scrolls it.
    pub detail: Option<String>,
    /// The ID of the failed send in the client that this notice shows. The
    /// client keeps that failure until the user dismisses this notice.
    pub failure: Option<String>,
}

#[derive(Debug, Default)]
pub struct Notices {
    last_serial: u64,
    /// The notice on screen first, then the errors that wait.
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
        };
        self.queue.push_back(notice.clone());
        notice
    }

    /// Shows a notice and returns it, or returns `None` when it does not
    /// show.
    ///
    /// - An info notice never shows while an error is on screen or waits,
    ///   so no error is lost. A new info notice replaces an older one.
    /// - An error shows at once when nothing else shows. An info notice on
    ///   screen leaves. Otherwise it waits behind the errors that are there,
    ///   and shows when the user dismisses them.
    /// - An error with the same text as one that is there already is not
    ///   added twice.
    pub fn show(&mut self, kind: NoticeKind, message: String) -> Option<Notice> {
        match kind {
            NoticeKind::Info => {
                if self.queue.iter().any(|n| n.kind == NoticeKind::Error) {
                    return None;
                }
                self.queue.clear();
            }
            NoticeKind::Error => {
                if self.queue.iter().any(|n| {
                    n.kind == NoticeKind::Error && n.failure.is_none() && n.message == message
                }) {
                    return None;
                }
                self.queue.retain(|n| n.kind == NoticeKind::Error);
            }
        }
        Some(self.push(kind, message, Extra::default()))
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
        self.queue.retain(|n| n.kind == NoticeKind::Error);
        Some(self.push(
            NoticeKind::Error,
            message,
            Extra {
                detail: Some(body),
                failure: Some(failure),
            },
        ))
    }

    /// Clears the notice with this serial, when it is the one on screen. A
    /// newer notice stays, and so does an error that waits.
    pub fn clear_if(&mut self, serial: u64) -> bool {
        if self.queue.front().is_some_and(|n| n.serial == serial) {
            self.queue.pop_front();
            true
        } else {
            false
        }
    }

    /// Dismisses the notice on screen and returns it. The next error, if
    /// one waits, shows.
    pub fn dismiss(&mut self) -> Option<Notice> {
        self.queue.pop_front()
    }

    /// The notice on screen.
    pub fn current(&self) -> Option<&Notice> {
        self.queue.front()
    }

    /// How many errors wait behind the notice on screen.
    pub fn waiting(&self) -> usize {
        self.queue.len().saturating_sub(1)
    }
}

#[derive(Default)]
struct Extra {
    detail: Option<String>,
    failure: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timer_of_an_older_notice_does_not_clear_a_newer_one() {
        let mut notices = Notices::default();
        let first = notices
            .show(NoticeKind::Info, "First".into())
            .expect("shown");
        let second = notices
            .show(NoticeKind::Info, "Second".into())
            .expect("shown");
        assert_ne!(first.serial, second.serial);
        // The timer of the first notice fires while the second shows.
        assert!(!notices.clear_if(first.serial));
        assert_eq!(
            notices.current().map(|n| n.message.as_str()),
            Some("Second")
        );
        assert!(notices.clear_if(second.serial));
        assert!(notices.current().is_none());
    }

    #[test]
    fn an_error_stays_until_it_is_dismissed() {
        let mut notices = Notices::default();
        let error = notices
            .show(NoticeKind::Error, "The hub is down".into())
            .expect("shown");
        assert!(notices.show(NoticeKind::Info, "Copied".into()).is_none());
        assert_eq!(notices.current(), Some(&error));
        // The timer of an info notice never clears an error.
        assert!(!notices.clear_if(error.serial + 1));
        assert_eq!(notices.dismiss(), Some(error));
        assert!(notices.current().is_none());
        assert!(notices.show(NoticeKind::Info, "Copied".into()).is_some());
    }

    /// Finding 8 of review cycle 2: a second error replaced the first before
    /// the user could read it.
    #[test]
    fn a_second_error_waits_behind_the_first() {
        let mut notices = Notices::default();
        notices.show(NoticeKind::Error, "One".into());
        notices
            .show(NoticeKind::Error, "Two".into())
            .expect("queued");
        assert_eq!(notices.current().map(|n| n.message.as_str()), Some("One"));
        assert_eq!(notices.waiting(), 1);
        // Info stays out while errors wait.
        assert!(notices.show(NoticeKind::Info, "ignored".into()).is_none());
        assert_eq!(notices.dismiss().map(|n| n.message), Some("One".into()));
        assert_eq!(notices.current().map(|n| n.message.as_str()), Some("Two"));
        assert_eq!(notices.waiting(), 0);
        notices.dismiss();
        assert!(notices.current().is_none());
    }

    #[test]
    fn an_error_replaces_an_info_notice_and_the_same_error_is_not_added_twice() {
        let mut notices = Notices::default();
        notices.show(NoticeKind::Info, "Copied".into());
        let error = notices
            .show(NoticeKind::Error, "Failed".into())
            .expect("shown");
        assert_eq!(notices.current(), Some(&error));
        assert_eq!(notices.waiting(), 0, "the info notice left");
        assert!(notices.show(NoticeKind::Error, "Failed".into()).is_none());
        assert_eq!(notices.waiting(), 0);
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
        assert_eq!(notices.waiting(), 1);
        let first = notices.dismiss().expect("first");
        assert_eq!(
            first.failure.as_deref(),
            Some("c-1"),
            "the caller can dismiss it in the client"
        );
        assert_eq!(
            notices.current().and_then(|n| n.detail.as_deref()),
            Some("Other text")
        );
    }

    #[test]
    fn serials_only_go_up() {
        let mut notices = Notices::default();
        let a = notices.show(NoticeKind::Info, "a".into()).expect("shown");
        notices.dismiss();
        let b = notices.show(NoticeKind::Info, "b".into()).expect("shown");
        assert!(b.serial > a.serial);
        // A late timer of the dismissed notice finds nothing to clear.
        assert!(!notices.clear_if(a.serial));
    }
}
