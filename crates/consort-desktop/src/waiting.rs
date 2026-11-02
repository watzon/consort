//! What the main column says while no channel has loaded. A cold start with
//! the hub down has no channels, so the timeline cannot say that a channel
//! "has no messages yet", and the composer must not take a message for a
//! channel that does not exist.

use consort_client::{Connection, ViewState};

/// What to show while the app waits for the hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiting {
    /// The line in the timeline and in the sidebar.
    pub title: &'static str,
    /// What went wrong, for an offline link. The user can select it.
    pub reason: Option<String>,
    /// What the user may do next.
    pub next: &'static str,
    /// The notice has a Try again button.
    pub can_retry: bool,
}

/// Why the app has no channel to show, or `None` when `channel_id` is a
/// channel that the state holds.
pub fn waiting_for_channel(state: &ViewState, channel_id: &str) -> Option<Waiting> {
    if !channel_id.is_empty() && state.channel(channel_id).is_some() {
        return None;
    }
    Some(match &state.connection {
        Connection::Offline { reason, .. } if state.tailnet_notice.is_some() => {
            let notice = state.tailnet_notice.as_ref();
            Waiting {
                title: notice.map_or("Cannot reach the hub", |n| n.title),
                reason: Some(reason.trim().to_owned()).filter(|r| !r.is_empty()),
                next: notice.map_or("Consort tries again by itself.", |n| n.body),
                can_retry: true,
            }
        }
        Connection::Offline { reason, .. } => Waiting {
            title: "Cannot reach the hub",
            reason: Some(reason.trim().to_owned()).filter(|r| !r.is_empty()),
            next: "Check that the hub runs. Consort tries again by itself.",
            can_retry: true,
        },
        Connection::Incompatible {
            too_old,
            hub,
            app_version,
            ..
        } => Waiting {
            title: match too_old {
                consort_client::versions::TooOld::Hub => "The hub needs an update",
                consort_client::versions::TooOld::ThisSide => "This app needs an update",
            },
            reason: Some(
                consort_client::versions::incompatible_view(*too_old, hub.as_ref(), app_version)
                    .body,
            ),
            next: "Consort tries again by itself.",
            can_retry: true,
        },
        // The link is up and the first snapshot has not come yet, or the app
        // is still connecting.
        _ => Waiting {
            title: "Connecting to the hub",
            reason: None,
            next: "Your channels show here when it connects.",
            can_retry: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_client::ChannelView;

    fn state(connection: Connection, channels: &[&str]) -> ViewState {
        ViewState {
            connection,
            channels: channels
                .iter()
                .map(|id| ChannelView {
                    id: (*id).to_owned(),
                    ..ChannelView::default()
                })
                .collect(),
            ..ViewState::default()
        }
    }

    /// Review cycle 3, finding 6: a cold start with the hub down showed
    /// " has no messages yet" and an empty sidebar.
    #[test]
    fn no_channel_while_connecting_says_connecting() {
        let waiting = waiting_for_channel(&state(Connection::Connecting, &[]), "").expect("waits");
        assert_eq!(waiting.title, "Connecting to the hub");
        assert!(!waiting.can_retry);
        assert!(waiting.reason.is_none());
    }

    #[test]
    fn no_channel_while_offline_says_why_and_offers_try_again() {
        let offline = Connection::Offline {
            retry_in_ms: 8_000,
            reason: " The connection failed: connection refused ".into(),
        };
        let waiting = waiting_for_channel(&state(offline, &[]), "").expect("waits");
        assert_eq!(waiting.title, "Cannot reach the hub");
        assert_eq!(
            waiting.reason.as_deref(),
            Some("The connection failed: connection refused")
        );
        assert!(waiting.can_retry);
        let silent = Connection::Offline {
            retry_in_ms: 1,
            reason: "  ".into(),
        };
        assert!(
            waiting_for_channel(&state(silent, &[]), "")
                .expect("waits")
                .reason
                .is_none()
        );
    }

    #[test]
    fn a_hub_on_the_tailnet_says_to_open_tailscale() {
        let mut with_notice = state(
            Connection::Offline {
                retry_in_ms: 1,
                reason: "x".into(),
            },
            &[],
        );
        with_notice.tailnet_notice = consort_client::reach::tailnet_notice(
            "https://m.tail1.ts.net",
            &with_notice.connection,
        );
        let waiting = waiting_for_channel(&with_notice, "").expect("waits");
        assert_eq!(waiting.title, "This device cannot reach your tailnet");
        assert!(waiting.next.contains("Open Tailscale"));
    }

    #[test]
    fn a_loaded_channel_does_not_wait() {
        let loaded = state(Connection::Online, &["chn_1"]);
        assert!(waiting_for_channel(&loaded, "chn_1").is_none());
        // An empty ID is never a channel, and an unknown one is not loaded.
        assert!(waiting_for_channel(&loaded, "").is_some());
        assert!(waiting_for_channel(&loaded, "chn_2").is_some());
        // Offline with the cache loaded still shows the channel.
        let offline = state(
            Connection::Offline {
                retry_in_ms: 1,
                reason: "x".into(),
            },
            &["chn_1"],
        );
        assert!(waiting_for_channel(&offline, "chn_1").is_none());
    }
}
