//! The local cache: a copy of the hub entities that this client has seen,
//! and the feed position. In memory for now; a SQLite cache can replace it
//! behind the same methods.

use std::collections::{BTreeMap, BTreeSet};

use consort_proto::{
    client_api::{EntityKind, EntityPayload, FeedChange, FeedEntry, Snapshot},
    ids::ChannelId,
    model::{
        Approval, Channel, Decision, Group, ItemPayload, Message, ReadMarker, Repo, SessionItem,
        Task, Worker,
    },
};
use serde::{Deserialize, Serialize};

/// A message that the user sent and the hub has not stored yet. The client
/// keeps these in its outbox file, so a quit does not lose them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingSend {
    pub client_message_id: String,
    pub channel_id: String,
    pub thread_root_id: Option<String>,
    pub body: String,
    pub created_at: i64,
}

/// A message that the hub refused after the user had left the composer, such
/// as a message that waited offline for a channel that was deleted meanwhile.
/// The view shows it so the text is not lost.
#[derive(Clone, Debug, PartialEq)]
pub struct SendFailure {
    /// The `client_message_id` of the message.
    pub id: String,
    pub channel_id: String,
    pub body: String,
    /// What the hub said, with a next step.
    pub message: String,
}

/// The part of a session item that the view reads. The cache keeps nothing
/// else, so a command with a large output costs a few bytes.
#[derive(Clone, Debug, PartialEq)]
pub enum ItemSummary {
    /// The command line.
    Command(String),
    /// The paths of the changed files.
    Files(Vec<String>),
}

impl ItemSummary {
    fn of(item: &SessionItem) -> Option<ItemSummary> {
        match &item.payload {
            ItemPayload::Command(c) => Some(ItemSummary::Command(c.command.clone())),
            ItemPayload::FileChange(f) => Some(ItemSummary::Files(
                f.changes.iter().map(|c| c.path.clone()).collect(),
            )),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Cache {
    /// The newest feed `seq` that this cache holds.
    pub seq: u64,
    /// True once the whole first load is done: the `all` snapshot, the
    /// snapshot of each channel and the subscription. Then a reconnect can
    /// catch up from `seq` instead of loading everything again. A load that
    /// stops half way leaves it false.
    pub loaded: bool,
    pub groups: BTreeMap<String, Group>,
    pub channels: BTreeMap<String, Channel>,
    pub repos: BTreeMap<String, Repo>,
    pub workers: BTreeMap<String, Worker>,
    pub messages: BTreeMap<String, Message>,
    pub tasks: BTreeMap<String, Task>,
    pub approvals: BTreeMap<String, Approval>,
    pub decisions: BTreeMap<String, Decision>,
    pub read_markers: BTreeMap<String, ReadMarker>,
    /// What the view reads of the session items, by session ID and item ID.
    pub session_items: BTreeMap<String, BTreeMap<String, ItemSummary>>,
    /// The messages that wait for the hub, in the order the user sent them.
    pub pending: Vec<PendingSend>,
    /// The messages that the hub refused after they left the composer.
    pub send_failures: Vec<SendFailure>,
    /// Channels whose timeline has older messages on the hub than the cache
    /// holds.
    pub has_older: BTreeMap<String, bool>,
    /// The channels whose snapshot (and so whose newest messages) is in the
    /// cache.
    pub channels_loaded: BTreeSet<String>,
    /// The card message IDs of the task threads whose replies are loaded.
    pub loaded_threads: BTreeSet<String>,
}

fn marker_key(marker: &ReadMarker) -> String {
    match marker.thread_root_id {
        Some(root) => format!("{}:{root}", marker.channel_id),
        None => marker.channel_id.to_string(),
    }
}

impl Cache {
    /// Replaces everything with a snapshot of scope `all`. The cache is not
    /// `loaded` until the caller has also loaded the channels and
    /// subscribed. See [`Cache::finish_load`].
    pub fn load_snapshot(&mut self, snapshot: Snapshot) {
        let pending = std::mem::take(&mut self.pending);
        let failures = std::mem::take(&mut self.send_failures);
        let threads = std::mem::take(&mut self.loaded_threads);
        *self = Cache::default();
        self.pending = pending;
        self.send_failures = failures;
        // The threads stay marked, so the link loads their replies again.
        self.loaded_threads = threads;
        self.merge_snapshot(snapshot);
    }

    /// Marks the first load as done.
    pub fn finish_load(&mut self) {
        self.loaded = true;
    }

    /// Moves the feed position past an entry that the hub or the client could
    /// not use, so a reconnect does not ask for it again.
    pub fn skip_to(&mut self, seq: u64) {
        self.seq = self.seq.max(seq);
    }

    /// Adds the snapshot of one channel. The first one sets whether older
    /// messages exist; a later one (a refresh) keeps what pages say.
    pub fn load_channel(&mut self, channel_id: ChannelId, snapshot: Snapshot) {
        let id = channel_id.to_string();
        if !self.channels_loaded.contains(&id) {
            self.has_older
                .insert(id.clone(), snapshot.messages_has_more);
        }
        self.merge_entities(snapshot);
        self.channels_loaded.insert(id);
    }

    /// The open channels that have no snapshot in the cache yet. A channel
    /// that is archived waits until it is unarchived.
    pub fn channels_to_load(&self) -> Vec<ChannelId> {
        self.channels
            .values()
            .filter(|c| {
                c.archived_at.is_none() && !self.channels_loaded.contains(&c.id.to_string())
            })
            .map(|c| c.id)
            .collect()
    }

    /// Removes a channel and everything that belongs to it.
    fn remove_channel(&mut self, id: &str) -> bool {
        let mut removed = self.channels.remove(id).is_some();
        let mut sessions: BTreeSet<String> = BTreeSet::new();
        let mut cards: BTreeSet<String> = BTreeSet::new();
        let in_channel = |c: &ChannelId| c.to_string() == id;
        self.messages.retain(|_, m| {
            let keep = !in_channel(&m.channel_id);
            if !keep {
                removed = true;
                if let Some(s) = m.author_session_id {
                    sessions.insert(s.to_string());
                }
            }
            keep
        });
        self.tasks.retain(|_, t| {
            let keep = !in_channel(&t.channel_id);
            if !keep {
                removed = true;
                cards.insert(t.card_message_id.to_string());
                if let Some(s) = t.current_session_id {
                    sessions.insert(s.to_string());
                }
            }
            keep
        });
        self.approvals.retain(|_, a| {
            let keep = !in_channel(&a.channel_id);
            if !keep {
                removed = true;
                sessions.insert(a.session_id.to_string());
            }
            keep
        });
        self.decisions.retain(|_, d| {
            let keep = !in_channel(&d.channel_id);
            removed |= !keep;
            keep
        });
        self.read_markers.retain(|_, m| {
            let keep = !in_channel(&m.channel_id);
            removed |= !keep;
            keep
        });
        for session in &sessions {
            removed |= self.session_items.remove(session).is_some();
        }
        self.pending.retain(|p| p.channel_id != id);
        self.has_older.remove(id);
        self.channels_loaded.remove(id);
        for card in cards {
            self.loaded_threads.remove(&card);
        }
        removed
    }

    /// Adds the entities of a channel snapshot or a page of messages, and
    /// keeps the feed position. Feed entries after the `all` snapshot replay
    /// over them in order, so a change that arrives meanwhile is not lost.
    pub fn merge_entities(&mut self, snapshot: Snapshot) {
        let seq = self.seq;
        self.merge_snapshot(snapshot);
        self.seq = seq;
    }

    /// Adds a page of messages, as from `message.list`.
    pub fn merge_messages(&mut self, messages: Vec<consort_proto::model::Message>) {
        for m in messages {
            self.insert_message(m);
        }
    }

    /// The oldest timeline message of a channel in the cache.
    pub fn oldest_timeline_message(
        &self,
        channel_id: &str,
    ) -> Option<consort_proto::ids::MessageId> {
        self.messages
            .values()
            .filter(|m| m.channel_id.to_string() == channel_id && m.shows_in_timeline())
            .min_by_key(|m| (m.created_at, m.id.to_string()))
            .map(|m| m.id)
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
        for r in snapshot.repos {
            self.repos.insert(r.id.to_string(), r);
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
                EntityPayload::Repo(r) => {
                    self.repos.insert(r.id.to_string(), r);
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
                    if let Some(summary) = ItemSummary::of(&item) {
                        self.session_items
                            .entry(item.session_id.to_string())
                            .or_default()
                            .insert(item.id.to_string(), summary);
                    } else {
                        return false;
                    }
                }
                // Memory notes, files, routines and clients have no view
                // yet.
                _ => return false,
            },
            FeedChange::Delete(kind) => {
                let id = &entry.entity_id;
                let removed = match kind {
                    EntityKind::Group => self.groups.remove(id).is_some(),
                    EntityKind::Channel => self.remove_channel(id),
                    EntityKind::Worker => self.workers.remove(id).is_some(),
                    EntityKind::Repo => self.repos.remove(id).is_some(),
                    EntityKind::Message => self.messages.remove(id).is_some(),
                    EntityKind::Task => self.tasks.remove(id).is_some(),
                    EntityKind::Approval => self.approvals.remove(id).is_some(),
                    EntityKind::Decision => self.decisions.remove(id).is_some(),
                    EntityKind::ReadMarker => self.read_markers.remove(id).is_some(),
                    EntityKind::SessionItem => id.split_once(':').is_some_and(|(session, item)| {
                        let Some(items) = self.session_items.get_mut(session) else {
                            return false;
                        };
                        let removed = items.remove(item).is_some();
                        if items.is_empty() {
                            self.session_items.remove(session);
                        }
                        removed
                    }),
                    _ => false,
                };
                return removed;
            }
        }
        true
    }
}
