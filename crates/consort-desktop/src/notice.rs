//! The notice toast. One notice shows at a time. Each notice has a serial,
//! so the timer of an older notice never clears a newer one.

use std::time::Duration;

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
}

#[derive(Debug, Default)]
pub struct Notices {
    last_serial: u64,
    current: Option<Notice>,
}

impl Notices {
    /// Shows a notice and returns it. An info notice never replaces an error
    /// notice that is still on screen, so the error stays until it is
    /// dismissed. Then this returns `None`.
    pub fn show(&mut self, kind: NoticeKind, message: String) -> Option<Notice> {
        if kind == NoticeKind::Info
            && self
                .current
                .as_ref()
                .is_some_and(|n| n.kind == NoticeKind::Error)
        {
            return None;
        }
        self.last_serial += 1;
        let notice = Notice {
            serial: self.last_serial,
            kind,
            message,
        };
        self.current = Some(notice.clone());
        Some(notice)
    }

    /// Clears the notice with this serial. A newer notice stays.
    pub fn clear_if(&mut self, serial: u64) -> bool {
        if self.current.as_ref().is_some_and(|n| n.serial == serial) {
            self.current = None;
            true
        } else {
            false
        }
    }

    pub fn dismiss(&mut self) {
        self.current = None;
    }

    pub fn current(&self) -> Option<&Notice> {
        self.current.as_ref()
    }
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
        notices.dismiss();
        assert!(notices.current().is_none());
        assert!(notices.show(NoticeKind::Info, "Copied".into()).is_some());
    }

    #[test]
    fn a_new_error_replaces_any_notice() {
        let mut notices = Notices::default();
        notices.show(NoticeKind::Error, "One".into());
        let second = notices
            .show(NoticeKind::Error, "Two".into())
            .expect("shown");
        assert_eq!(notices.current(), Some(&second));
        notices.show(NoticeKind::Info, "ignored".into());
        notices.dismiss();
        notices.show(NoticeKind::Info, "Info".into());
        let error = notices
            .show(NoticeKind::Error, "Three".into())
            .expect("shown");
        assert_eq!(notices.current(), Some(&error));
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
