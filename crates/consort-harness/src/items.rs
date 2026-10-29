//! Session item bookkeeping that every adapter shares.

use std::collections::HashMap;

use consort_proto::ids::{ItemId, SessionId, TurnId};
use consort_proto::model::{ItemPayload, ItemStatus, SessionItem};
use consort_proto::time::Timestamp;
use consort_proto::worker_link::SessionEvent;

#[derive(Debug, Clone)]
struct Tracked {
    id: ItemId,
    turn_id: TurnId,
    created_at: Timestamp,
    payload: ItemPayload,
    status: ItemStatus,
}

/// Maps a harness's own item IDs to Consort item IDs, keeps the latest
/// payload of each item, and builds `item` events.
#[derive(Debug, Clone)]
pub struct ItemTracker {
    session_id: SessionId,
    turn_id: TurnId,
    items: HashMap<String, Tracked>,
    order: Vec<String>,
}

impl ItemTracker {
    /// A tracker for one session.
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            turn_id: TurnId::new(""),
            items: HashMap::new(),
            order: Vec::new(),
        }
    }

    /// The turn that new items belong to.
    pub fn set_turn(&mut self, turn_id: TurnId) {
        self.turn_id = turn_id;
    }

    /// The current turn.
    pub fn turn(&self) -> &TurnId {
        &self.turn_id
    }

    /// Whether the tracker knows a harness item.
    pub fn contains(&self, native: &str) -> bool {
        self.items.contains_key(native)
    }

    /// The Consort ID of a harness item.
    pub fn item_id(&self, native: &str) -> Option<ItemId> {
        self.items.get(native).map(|t| t.id)
    }

    /// The latest payload of a harness item.
    pub fn payload(&self, native: &str) -> Option<&ItemPayload> {
        self.items.get(native).map(|t| &t.payload)
    }

    /// The status of a harness item.
    pub fn status(&self, native: &str) -> Option<ItemStatus> {
        self.items.get(native).map(|t| t.status)
    }

    /// Sets the payload and status of a harness item, and returns the `item`
    /// event. A new item gets a new ID in the current turn.
    pub fn upsert(
        &mut self,
        native: &str,
        payload: ItemPayload,
        status: ItemStatus,
    ) -> SessionEvent {
        let now = Timestamp::now();
        let entry = self.items.entry(native.to_owned()).or_insert_with(|| {
            self.order.push(native.to_owned());
            Tracked {
                id: ItemId::new(),
                turn_id: self.turn_id.clone(),
                created_at: now,
                payload: payload.clone(),
                status,
            }
        });
        entry.payload = payload;
        entry.status = status;
        Self::event(self.session_id, entry, now)
    }

    /// Changes a known item and returns the new `item` event, or `None` for
    /// an unknown item.
    pub fn update(
        &mut self,
        native: &str,
        change: impl FnOnce(&mut ItemPayload, &mut ItemStatus),
    ) -> Option<SessionEvent> {
        let entry = self.items.get_mut(native)?;
        change(&mut entry.payload, &mut entry.status);
        Some(Self::event(self.session_id, entry, Timestamp::now()))
    }

    /// Marks every item that is still in progress as completed (or failed)
    /// and returns their events. Adapters call this when a turn ends.
    pub fn finish_open(&mut self, status: ItemStatus) -> Vec<SessionEvent> {
        let now = Timestamp::now();
        let mut events = Vec::new();
        for native in &self.order {
            if let Some(entry) = self.items.get_mut(native)
                && entry.status == ItemStatus::InProgress
            {
                entry.status = status;
                events.push(Self::event(self.session_id, entry, now));
            }
        }
        events
    }

    fn event(session_id: SessionId, entry: &Tracked, now: Timestamp) -> SessionEvent {
        SessionEvent::Item(SessionItem {
            id: entry.id,
            session_id,
            turn_id: entry.turn_id.clone(),
            payload: entry.payload.clone(),
            status: entry.status,
            created_at: entry.created_at,
            updated_at: now,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::model::AssistantMessagePayload;

    fn text(t: &str) -> ItemPayload {
        ItemPayload::AssistantMessage(AssistantMessagePayload { text: t.into() })
    }

    #[test]
    fn upsert_keeps_one_id_per_native_item() {
        let mut tracker = ItemTracker::new(SessionId::new());
        tracker.set_turn(TurnId::new("t1"));
        let SessionEvent::Item(a) = tracker.upsert("m1", text("Hel"), ItemStatus::InProgress)
        else {
            panic!("expected an item");
        };
        let SessionEvent::Item(b) = tracker.upsert("m1", text("Hello"), ItemStatus::InProgress)
        else {
            panic!("expected an item");
        };
        assert_eq!(a.id, b.id);
        assert_eq!(b.turn_id, TurnId::new("t1"));
        let finished = tracker.finish_open(ItemStatus::Completed);
        assert_eq!(finished.len(), 1);
        assert!(tracker.finish_open(ItemStatus::Completed).is_empty());
        assert!(tracker.update("nope", |_, _| {}).is_none());
    }
}
