//! The local cache: a copy of the hub entities that this client has seen,
//! and the feed position. In memory for now; a SQLite cache can replace it
//! behind the same methods.

use std::collections::BTreeMap;

use consort_proto::{
    client_api::{EntityKind, EntityPayload, FeedChange, FeedEntry, Snapshot},
    model::{Approval, Channel, Decision, Group, Message, ReadMarker, SessionItem, Task, Worker},
};

/// A message that the user sent and the hub has not stored yet.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingSend {
    pub client_message_id: String,
    pub channel_id: String,
    pub thread_root_id: Option<String>,
    pub body: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Cache {
    /// The newest feed `seq` that this cache holds.
    pub seq: u64,
    pub groups: BTreeMap<String, Group>,
    pub channels: BTreeMap<String, Channel>,
    pub workers: BTreeMap<String, Worker>,
    pub messages: BTreeMap<String, Message>,
    pub tasks: BTreeMap<String, Task>,
    pub approvals: BTreeMap<String, Approval>,
    pub decisions: BTreeMap<String, Decision>,
    pub read_markers: BTreeMap<String, ReadMarker>,
    pub session_items: BTreeMap<String, SessionItem>,
    pub pending: Vec<PendingSend>,
}

fn marker_key(marker: &ReadMarker) -> String {
    match marker.thread_root_id {
        Some(root) => format!("{}:{root}", marker.channel_id),
        None => marker.channel_id.to_string(),
    }
}

impl Cache {
    /// Replaces everything with a snapshot of scope `all`.
    pub fn load_snapshot(&mut self, snapshot: Snapshot) {
        let pending = std::mem::take(&mut self.pending);
        *self = Cache::default();
        self.pending = pending;
        self.merge_snapshot(snapshot);
    }

    /// Adds a snapshot of one channel, or the messages of a page.
    pub fn merge_snapshot(&mut self, snapshot: Snapshot) {
        self.seq = self.seq.max(snapshot.seq);
        for g in snapshot.groups {
            self.groups.insert(g.id.to_string(), g);
        }
        for c in snapshot.channels {
            self.channels.insert(c.id.to_string(), c);
        }
        for w in snapshot.workers {
            self.workers.insert(w.id.to_string(), w);
        }
        for m in snapshot.read_markers {
            self.read_markers.insert(marker_key(&m), m);
        }
        for m in snapshot.messages {
            self.insert_message(m);
        }
        for t in snapshot.tasks {
            self.tasks.insert(t.id.to_string(), t);
        }
        for a in snapshot.approvals {
            self.approvals.insert(a.id.to_string(), a);
        }
        for d in snapshot.decisions {
            self.decisions.insert(d.id.to_string(), d);
        }
    }

    fn insert_message(&mut self, message: Message) {
        if let Some(cmid) = &message.client_message_id {
            self.pending.retain(|p| &p.client_message_id != cmid);
        }
        self.messages.insert(message.id.to_string(), message);
    }

    /// Applies one feed entry. Entries at or below the current `seq` are
    /// ignored, so a replay does no harm. Returns true when the entry changed
    /// the cache.
    pub fn apply(&mut self, entry: FeedEntry) -> bool {
        if entry.seq != 0 && entry.seq <= self.seq {
            return false;
        }
        self.seq = self.seq.max(entry.seq);
        match entry.change {
            FeedChange::Upsert(payload) => match payload {
                EntityPayload::Group(g) => {
                    self.groups.insert(g.id.to_string(), g);
                }
                EntityPayload::Channel(c) => {
                    self.channels.insert(c.id.to_string(), c);
                }
                EntityPayload::Worker(w) => {
                    self.workers.insert(w.id.to_string(), w);
                }
                EntityPayload::Message(m) => self.insert_message(m),
                EntityPayload::Task(t) => {
                    self.tasks.insert(t.id.to_string(), t);
                }
                EntityPayload::Approval(a) => {
                    self.approvals.insert(a.id.to_string(), a);
                }
                EntityPayload::Decision(d) => {
                    self.decisions.insert(d.id.to_string(), d);
                }
                EntityPayload::ReadMarker(m) => {
                    self.read_markers.insert(marker_key(&m), m);
                }
                EntityPayload::SessionItem(item) => {
                    self.session_items
                        .insert(format!("{}:{}", item.session_id, item.id), item);
                }
                // Memory notes, files, routines, repos and clients have no
                // view yet.
                _ => return false,
            },
            FeedChange::Delete(kind) => {
                let id = &entry.entity_id;
                let removed = match kind {
                    EntityKind::Group => self.groups.remove(id).is_some(),
                    EntityKind::Channel => self.channels.remove(id).is_some(),
                    EntityKind::Worker => self.workers.remove(id).is_some(),
                    EntityKind::Message => self.messages.remove(id).is_some(),
                    EntityKind::Task => self.tasks.remove(id).is_some(),
                    EntityKind::Approval => self.approvals.remove(id).is_some(),
                    EntityKind::Decision => self.decisions.remove(id).is_some(),
                    EntityKind::ReadMarker => self.read_markers.remove(id).is_some(),
                    EntityKind::SessionItem => self.session_items.remove(id).is_some(),
                    _ => false,
                };
                return removed;
            }
        }
        true
    }
}
