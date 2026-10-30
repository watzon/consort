//! What the app shows while the hub cannot be reached.
//!
//! The app opens with the data that it saved (see [`crate::persist`]), also
//! when the hub is down. Then the view state says:
//! - [`ViewState::saved_data`]: the screen shows saved data, and from when.
//!   The status pill under the mark uses it.
//! - Workers have an unknown status. The client cannot know if a worker is
//!   online without the hub, so no worker reads as online.
//! - Approvals and decisions cannot be answered
//!   ([`ViewState::answers_blocked`]).
//! - A message that the user sends waits in the outbox, and its bubble says
//!   so (`MessageView::pending`). It goes out when the hub is back.

use crate::{
    format,
    view::{Connection, ViewState},
};

/// The time of the data on screen, while the hub cannot be reached.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedData {
    /// When the data was last known to be current, in Unix milliseconds.
    pub from_ms: i64,
    /// That time as the pill says it: "9:41 AM" for today, "yesterday at
    /// 9:41 AM", or "Tuesday at 9:41 AM".
    pub from_label: String,
}

impl SavedData {
    pub fn new(from_ms: i64, now_ms: i64) -> SavedData {
        SavedData {
            from_ms,
            from_label: when_label(from_ms, now_ms),
        }
    }

    /// The text of the status pill: "Cannot reach the hub. You see saved
    /// data from 9:41 AM."
    pub fn pill_text(&self) -> String {
        format!(
            "Cannot reach the hub. You see saved data from {}.",
            self.from_label
        )
    }
}

/// "9:41 AM" for today, else the day before it: "yesterday at 9:41 AM",
/// "Tuesday at 9:41 AM" or "October 12 at 9:41 AM".
pub fn when_label(ms: i64, now_ms: i64) -> String {
    let time = format::time_label(ms);
    match format::day_label(ms, now_ms).as_str() {
        "Today" | "" => time,
        "Yesterday" => format!("yesterday at {time}"),
        day => format!("{day} at {time}"),
    }
}

/// The text for a control that needs the hub, while the hub is not there.
pub const WAITING_FOR_HUB: &str = "Waiting for the hub. You can answer when it is back.";

/// What the sidebar says for a worker while its status is unknown.
pub const STATUS_UNKNOWN: &str = "Status unknown";

/// Whether the screen shows saved data now. Needs a link that is down, and a
/// cache that holds data with a known time.
pub fn saved_data(
    connection: &Connection,
    has_data: bool,
    fresh_at: i64,
    now_ms: i64,
) -> Option<SavedData> {
    (matches!(connection, Connection::Offline { .. }) && has_data && fresh_at > 0)
        .then(|| SavedData::new(fresh_at, now_ms))
}

/// Changes the view state for saved data: no worker is online, and nothing
/// works, because the client cannot know without the hub.
pub fn mark_saved(state: &mut ViewState, saved: Option<SavedData>) {
    let Some(saved) = saved else {
        return;
    };
    for worker in &mut state.workers {
        worker.online = false;
        worker.status_line = STATUS_UNKNOWN.to_owned();
    }
    state.sidebar.workers_online = 0;
    // The mark and its pill show what the assistant did at the time of the
    // saved data. Nothing runs that the client can see now.
    state.activity.clear();
    state.saved_data = Some(saved);
}

impl ViewState {
    /// Why the user cannot answer an approval or a decision now, or `None`
    /// when the hub is there. The answer needs the hub, so the buttons wait
    /// for it. A message is different: it waits in the outbox.
    pub fn answers_blocked(&self) -> Option<&'static str> {
        match self.connection {
            Connection::Online | Connection::Demo => None,
            _ => Some(WAITING_FOR_HUB),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::WorkerView;

    const DAY: i64 = 86_400_000;

    fn offline() -> Connection {
        Connection::Offline {
            retry_in_ms: 5_000,
            reason: "refused".into(),
        }
    }

    #[test]
    fn the_time_is_plain_for_today_and_names_the_day_for_an_older_day() {
        let now = 1_800_000_000_000;
        assert_eq!(when_label(now, now), format::time_label(now));
        assert_eq!(
            when_label(now - DAY, now),
            format!("yesterday at {}", format::time_label(now - DAY))
        );
        let older = when_label(now - 3 * DAY, now);
        assert!(older.contains(" at "), "{older}");
        assert!(!older.starts_with("yesterday"), "{older}");
    }

    #[test]
    fn the_pill_says_what_the_user_sees_and_from_when() {
        let now = 1_800_000_000_000;
        let saved = SavedData::new(now, now);
        assert_eq!(
            saved.pill_text(),
            format!(
                "Cannot reach the hub. You see saved data from {}.",
                format::time_label(now)
            )
        );
    }

    #[test]
    fn saved_data_shows_only_for_a_down_link_with_data() {
        let now = 1_800_000_000_000;
        assert!(saved_data(&offline(), true, now, now).is_some());
        // No data to show: the app says that it waits, not that it shows
        // saved data.
        assert!(saved_data(&offline(), false, now, now).is_none());
        // No known time.
        assert!(saved_data(&offline(), true, 0, now).is_none());
        // The link is up, or the app connects now.
        assert!(saved_data(&Connection::Online, true, now, now).is_none());
        assert!(saved_data(&Connection::Connecting, true, now, now).is_none());
        assert!(saved_data(&Connection::NotPaired, true, now, now).is_none());
    }

    #[test]
    fn saved_data_makes_every_worker_status_unknown() {
        let now = 1_800_000_000_000;
        let mut state = ViewState {
            connection: offline(),
            workers: vec![WorkerView {
                id: "wrk_1".into(),
                name: "bigboi".into(),
                online: true,
                status_line: "Online".into(),
                sidebar_detail: "Windows".into(),
                ..WorkerView::default()
            }],
            ..ViewState::default()
        };
        state.sidebar.workers_online = 1;
        mark_saved(&mut state, None);
        assert!(state.workers[0].online, "no saved data, no change");
        mark_saved(&mut state, Some(SavedData::new(now, now)));
        assert!(!state.workers[0].online);
        assert_eq!(state.workers[0].status_line, STATUS_UNKNOWN);
        assert_eq!(
            state.workers[0].sidebar_detail, "Windows",
            "the system stays: it does not change"
        );
        assert_eq!(state.sidebar.workers_online, 0);
        assert!(state.saved_data.is_some());
    }

    #[test]
    fn answers_wait_for_the_hub_in_every_state_but_online() {
        let state = |connection| ViewState {
            connection,
            ..ViewState::default()
        };
        assert_eq!(state(Connection::Online).answers_blocked(), None);
        assert_eq!(state(Connection::Demo).answers_blocked(), None);
        for blocked in [
            offline(),
            Connection::Connecting,
            Connection::NotPaired,
            Connection::Refused {
                reason: "no".into(),
            },
        ] {
            assert_eq!(state(blocked).answers_blocked(), Some(WAITING_FOR_HUB));
        }
    }
}
