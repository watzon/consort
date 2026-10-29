//! The retry countdown of an offline link. The client says "retry in N ms"
//! once. The app remembers when it first saw that state and counts down from
//! there.

use std::time::{Duration, Instant};

use consort_client::Connection;

/// The first time the app saw one Offline state.
#[derive(Clone, Debug)]
pub struct RetryCountdown {
    seen: Instant,
    retry_in_ms: u64,
    reason: String,
}

impl RetryCountdown {
    pub fn new(seen: Instant, retry_in_ms: u64, reason: String) -> RetryCountdown {
        RetryCountdown {
            seen,
            retry_in_ms,
            reason,
        }
    }

    /// Whole seconds left, rounded up. It is 0 once the time is over.
    pub fn seconds_left(&self, now: Instant) -> u64 {
        seconds_left(self.retry_in_ms, now.saturating_duration_since(self.seen))
    }

    /// True when `connection` is the same Offline state this countdown
    /// started from.
    pub fn is_for(&self, connection: &Connection) -> bool {
        matches!(
            connection,
            Connection::Offline { retry_in_ms, reason }
                if *retry_in_ms == self.retry_in_ms && *reason == self.reason
        )
    }
}

/// Whole seconds left of `retry_in_ms` after `elapsed`, rounded up.
pub fn seconds_left(retry_in_ms: u64, elapsed: Duration) -> u64 {
    let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    retry_in_ms.saturating_sub(elapsed_ms).div_ceil(1000)
}

/// Follows the connection: keeps the countdown while the same Offline state
/// lasts, starts a new one for a new Offline state, and drops it for any
/// other state.
pub fn follow(
    current: Option<RetryCountdown>,
    connection: &Connection,
    now: Instant,
) -> Option<RetryCountdown> {
    match connection {
        Connection::Offline {
            retry_in_ms,
            reason,
        } => match current {
            Some(countdown) if countdown.is_for(connection) => Some(countdown),
            _ => Some(RetryCountdown::new(now, *retry_in_ms, reason.clone())),
        },
        _ => None,
    }
}

/// The line under the composer for an offline link, such as "Cannot reach
/// the hub. Check that the hub runs. Retrying in 12 s."
pub fn offline_line(seconds_left: u64) -> String {
    if seconds_left == 0 {
        "Cannot reach the hub. Check that the hub runs. Retrying now.".to_owned()
    } else {
        format!("Cannot reach the hub. Check that the hub runs. Retrying in {seconds_left} s.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offline(retry_in_ms: u64, reason: &str) -> Connection {
        Connection::Offline {
            retry_in_ms,
            reason: reason.into(),
        }
    }

    #[test]
    fn seconds_round_up_and_stop_at_zero() {
        assert_eq!(seconds_left(12_000, Duration::ZERO), 12);
        assert_eq!(seconds_left(12_000, Duration::from_millis(1)), 12);
        assert_eq!(seconds_left(12_000, Duration::from_millis(1_000)), 11);
        assert_eq!(seconds_left(12_000, Duration::from_millis(11_001)), 1);
        assert_eq!(seconds_left(12_000, Duration::from_millis(12_000)), 0);
        assert_eq!(seconds_left(12_000, Duration::from_secs(60)), 0);
        assert_eq!(seconds_left(0, Duration::ZERO), 0);
    }

    #[test]
    fn the_countdown_ticks_from_the_first_sight() {
        let start = Instant::now();
        let countdown = RetryCountdown::new(start, 5_000, "refused".into());
        assert_eq!(countdown.seconds_left(start), 5);
        assert_eq!(countdown.seconds_left(start + Duration::from_secs(2)), 3);
        assert_eq!(countdown.seconds_left(start + Duration::from_secs(9)), 0);
        // A clock that reads earlier than the start does not underflow.
        assert_eq!(countdown.seconds_left(start), 5);
    }

    #[test]
    fn the_same_offline_state_keeps_its_countdown() {
        let start = Instant::now();
        let state = offline(5_000, "refused");
        let first = follow(None, &state, start).expect("offline");
        let later = start + Duration::from_secs(3);
        let kept = follow(Some(first), &state, later).expect("offline");
        // Seen at `start`, so 3 s have passed.
        assert_eq!(kept.seconds_left(later), 2);
    }

    #[test]
    fn a_new_offline_state_starts_a_new_countdown() {
        let start = Instant::now();
        let first = follow(None, &offline(5_000, "refused"), start).expect("offline");
        let later = start + Duration::from_secs(5);
        let next = follow(Some(first), &offline(10_000, "refused"), later).expect("offline");
        assert_eq!(next.seconds_left(later), 10);
    }

    #[test]
    fn another_connection_state_drops_the_countdown() {
        let start = Instant::now();
        let first = follow(None, &offline(5_000, "refused"), start);
        assert!(first.is_some());
        assert!(follow(first.clone(), &Connection::Connecting, start).is_none());
        assert!(follow(first, &Connection::Online, start).is_none());
    }

    #[test]
    fn the_line_names_the_problem_and_the_next_step() {
        assert_eq!(
            offline_line(12),
            "Cannot reach the hub. Check that the hub runs. Retrying in 12 s."
        );
        assert_eq!(
            offline_line(0),
            "Cannot reach the hub. Check that the hub runs. Retrying now."
        );
    }
}
