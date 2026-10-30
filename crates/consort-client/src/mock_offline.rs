//! The mock state for `--mock --screen hub-offline` (Paper `ERQ-0`): the app
//! opens with its saved data while the hub cannot be reached.

use crate::{
    mock,
    offline::{self, SavedData},
    view::{Connection, MessageView, Speaker, TimelineItem, ViewState},
};

/// The screens that this file builds.
pub fn screen_state(screen: &str) -> Option<ViewState> {
    (screen == "hub-offline").then(hub_offline)
}

/// Tutti with saved data from 9:41 AM, a status that is unknown for every
/// worker, an approval that waits, and a message that waits in the outbox.
pub fn hub_offline() -> ViewState {
    let mut state = mock::view_state();
    state.connection = Connection::Offline {
        retry_in_ms: 12_000,
        reason: "The connection was refused.".into(),
    };
    let tutti = state.tutti().map(|c| c.id.clone()).unwrap_or_default();
    if let Some(timeline) = state.timelines.iter_mut().find(|t| t.channel_id == tutti) {
        timeline.items.push(TimelineItem::Message(MessageView {
            id: "pending-demo-offline".into(),
            channel_id: tutti.clone(),
            speaker: Speaker::You,
            body: "Also check the homelab backup when you can.".into(),
            pending: true,
            ..MessageView::default()
        }));
    }
    // A fixed label, so the screenshot reads the same on every day.
    offline::mark_saved(
        &mut state,
        Some(SavedData {
            from_ms: crate::format::now_ms(),
            from_label: "9:41 AM".into(),
        }),
    );
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_hub_offline_screen_is_built_here() {
        assert!(screen_state("hub-offline").is_some());
        assert!(screen_state("tutti").is_none());
    }

    #[test]
    fn the_screen_has_a_queued_message_and_unknown_workers() {
        let state = hub_offline();
        assert!(matches!(state.connection, Connection::Offline { .. }));
        assert_eq!(
            state.saved_data.as_ref().map(SavedData::pill_text),
            Some("Cannot reach the hub. You see saved data from 9:41 AM.".to_owned())
        );
        assert!(state.workers.iter().all(|w| !w.online));
        let queued = state
            .timeline(&state.tutti().unwrap().id)
            .unwrap()
            .items
            .iter()
            .any(|i| matches!(i, TimelineItem::Message(m) if m.pending));
        assert!(queued);
        assert!(state.answers_blocked().is_some());
    }
}
