//! When the app marks the open channel as read, and when it tells the user
//! that a channel did not load.
//!
//! The app marks a channel read only when the user can have seen its newest
//! messages: the channel is loaded, the window has the focus, the timeline
//! sits at its live edge (scrolled to the bottom) and the link is up. The
//! decision follows the channel's unread count, not the last message the app
//! sent a mark for, so a mark that failed is tried again as long as messages
//! stay unread.

use std::time::{Duration, Instant};

/// How long the app waits before it tries a failed mark again.
pub const MARK_RETRY: Duration = Duration::from_secs(5);

/// How long a channel may stay unloaded before the timeline says so. A new
/// channel loads in a moment, and a message about it would only flash.
pub const UNLOADED_GRACE: Duration = Duration::from_millis(1500);

/// What the app knows when it decides to mark the open channel as read.
#[derive(Clone, Copy, Debug, Default)]
pub struct MarkInputs {
    /// The main page shows, not Settings or onboarding.
    pub on_main_page: bool,
    /// The window has the focus.
    pub window_active: bool,
    /// The link to the hub is up (or the demo runs).
    pub online: bool,
    /// The newest messages of the channel are in the client. A channel that
    /// never loaded shows as empty, and its first live message is not the
    /// newest one the user has seen.
    pub loaded: bool,
    /// The timeline is scrolled to the bottom.
    pub at_live_edge: bool,
    /// The unread count of the channel.
    pub unread: u32,
    /// A mark is on its way to the hub.
    pub running: bool,
}

/// True when the app must ask the client to mark the open channel as read.
pub fn should_mark(i: &MarkInputs) -> bool {
    i.on_main_page
        && i.window_active
        && i.online
        && i.loaded
        && i.at_live_edge
        && i.unread > 0
        && !i.running
}

/// Tracks how long the open channel has been unloaded while the link is up.
#[derive(Debug, Default)]
pub struct Unloaded {
    since: Option<(String, Instant)>,
}

impl Unloaded {
    /// Records the state of the open channel. Returns true when the channel
    /// just became unloaded, so the caller can redraw once the grace time
    /// has passed.
    pub fn update(&mut self, channel_id: &str, unloaded: bool, now: Instant) -> bool {
        if !unloaded {
            self.since = None;
            return false;
        }
        match &self.since {
            Some((id, _)) if id == channel_id => false,
            _ => {
                self.since = Some((channel_id.to_owned(), now));
                true
            }
        }
    }

    /// True when the timeline must say that the channel did not load.
    pub fn visible(&self, now: Instant) -> bool {
        self.since
            .as_ref()
            .is_some_and(|(_, since)| now.duration_since(*since) >= UNLOADED_GRACE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> MarkInputs {
        MarkInputs {
            on_main_page: true,
            window_active: true,
            online: true,
            loaded: true,
            at_live_edge: true,
            unread: 2,
            running: false,
        }
    }

    #[test]
    fn a_ready_channel_with_unread_messages_is_marked() {
        assert!(should_mark(&ready()));
    }

    #[test]
    fn nothing_is_marked_when_nothing_is_unread() {
        assert!(!should_mark(&MarkInputs {
            unread: 0,
            ..ready()
        }));
    }

    /// Finding 7 of review cycle 2: a channel that never loaded, a window
    /// without focus and a timeline that is scrolled up must not be marked,
    /// because the user has not seen the newest messages.
    #[test]
    fn a_channel_is_not_marked_until_the_user_can_have_seen_it() {
        for blocked in [
            MarkInputs {
                loaded: false,
                ..ready()
            },
            MarkInputs {
                window_active: false,
                ..ready()
            },
            MarkInputs {
                at_live_edge: false,
                ..ready()
            },
            MarkInputs {
                online: false,
                ..ready()
            },
            MarkInputs {
                on_main_page: false,
                ..ready()
            },
        ] {
            assert!(!should_mark(&blocked), "{blocked:?}");
        }
    }

    /// Finding 6 of review cycle 2: a failed mark was never tried again,
    /// because the app remembered the mark it had sent. The decision follows
    /// the unread count now: while messages stay unread and no mark runs, the
    /// app marks again.
    #[test]
    fn a_failed_mark_is_tried_again_while_messages_stay_unread() {
        // The first try is on its way: no second one starts.
        assert!(!should_mark(&MarkInputs {
            running: true,
            ..ready()
        }));
        // It failed. The channel is still unread and nothing runs.
        assert!(should_mark(&ready()));
    }

    #[test]
    fn the_retry_waits_a_few_seconds() {
        assert!(MARK_RETRY >= Duration::from_secs(1));
    }

    #[test]
    fn an_unloaded_channel_says_so_only_after_a_short_wait() {
        let start = Instant::now();
        let mut unloaded = Unloaded::default();
        assert!(!unloaded.visible(start));
        assert!(
            unloaded.update("c1", true, start),
            "it just became unloaded"
        );
        assert!(!unloaded.visible(start), "a new channel is not a failure");
        assert!(!unloaded.update("c1", true, start + Duration::from_secs(1)));
        assert!(unloaded.visible(start + UNLOADED_GRACE));
        // It loads: the message goes away and the clock starts over.
        assert!(!unloaded.update("c1", false, start + Duration::from_secs(5)));
        assert!(!unloaded.visible(start + Duration::from_secs(6)));
    }

    #[test]
    fn another_channel_starts_its_own_wait() {
        let start = Instant::now();
        let mut unloaded = Unloaded::default();
        unloaded.update("c1", true, start);
        let later = start + UNLOADED_GRACE * 2;
        assert!(unloaded.visible(later));
        assert!(unloaded.update("c2", true, later));
        assert!(!unloaded.visible(later), "c2 just began to wait");
    }
}
