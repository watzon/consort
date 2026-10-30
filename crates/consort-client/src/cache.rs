//! The local cache: a copy of the hub entities that this client has seen,
//! and the feed position. In memory for now; a SQLite cache can replace it
//! behind the same methods.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use consort_proto::{
    client_api::{EntityKind, EntityPayload, FeedChange, FeedEntry, Snapshot},
    ids::{ChannelId, TaskId},
    model::{
        AcpRegistryAgent, Approval, BlobRef, Channel, ChannelFile, Connection, Decision, Group,
        ItemPayload, MemoryNote, MemoryNoteSummary, Message, MessageKind, PREFERENCES_ENTITY_ID,
        Preferences, ReadMarker, Repo, Routine, RoutineRun, SessionItem, SignIn, Skill, Task,
        Worker,
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
    /// The uploaded blobs that go out with the message. An outbox file from
    /// an older version has none.
    #[serde(default)]
    pub attachments: Vec<BlobRef>,
}

/// A message that the hub refused after the user had left the composer, such
/// as a message that waited offline for a channel that was deleted meanwhile.
/// The view shows it so the text is not lost.
#[derive(Clone, Debug, PartialEq)]
pub struct SendFailure {
    /// The `client_message_id` of the message.
    pub id: String,
    pub channel_id: String,
    /// The card message of the task thread that the message was a reply in,
    /// or `None` for a message in the timeline.
    pub thread_root_id: Option<String>,
    pub body: String,
    /// The name of the channel, kept for a channel that is gone from the
    /// cache. Empty when the cache still holds the channel.
    pub channel_name: String,
    /// What the hub said, with a next step.
    pub message: String,
}

/// The part of a session item that the view reads. The cache keeps nothing
/// else, so a command with a large output costs a few bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// The hub epoch that `seq` belongs to (`hello`). A hub that was
    /// restored from a backup has a new epoch, and its feed numbers name
    /// other changes, so the client loads a snapshot.
    pub hub_epoch: Option<String>,
    pub groups: BTreeMap<String, Group>,
    pub channels: BTreeMap<String, Channel>,
    pub repos: BTreeMap<String, Repo>,
    pub workers: BTreeMap<String, Worker>,
    pub messages: BTreeMap<String, Message>,
    /// The files of the loaded channels, by their feed entity ID
    /// (`<channel-id>:<path>`). A deleted file is not here.
    pub channel_files: BTreeMap<String, ChannelFile>,
    /// The channels whose files are in the cache: from a channel snapshot
    /// or from `files.list`.
    pub files_loaded: BTreeSet<String>,
    /// Every routine, by ID, from the snapshots and the feed.
    pub routines: BTreeMap<String, Routine>,
    /// The runs of the routines whose runs are loaded, by routine ID and run
    /// number.
    pub routine_runs: BTreeMap<String, BTreeMap<u64, RoutineRun>>,
    /// The routines whose runs are in the cache, from `routine.runs`. Feed
    /// entries for the runs of other routines are dropped, so a partial
    /// list never looks whole.
    pub runs_loaded: BTreeSet<String>,
    /// The memory notes of the loaded scopes, by feed entity ID
    /// (`<scope>:<path>`). A note in Recently deleted has `deleted_at`.
    pub memory_notes: BTreeMap<String, CachedNote>,
    /// The memory scopes (`global` or a channel ID) whose notes are loaded.
    pub memory_loaded: BTreeSet<String>,
    /// The scopes whose Recently deleted notes are loaded.
    pub memory_deleted_loaded: BTreeSet<String>,
    /// The blobs in the local blob cache: the hash and the file.
    pub local_blobs: BTreeMap<String, String>,
    pub tasks: BTreeMap<String, Task>,
    /// The IDs of the cached tasks whose `brief` and `review_summary` are
    /// previews, because a snapshot cut them (`Snapshot::tasks_cut`). A feed
    /// entry or `task.get` holds the full task and ends the preview state.
    /// Lists and task cards may show the preview. The task thread needs the
    /// full text, so the client asks the hub for it first.
    pub task_previews: BTreeSet<String>,
    /// True when a snapshot left out resolved tasks (`Snapshot::tasks_has_more`),
    /// so a task card can name a task that the cache does not hold.
    pub tasks_have_more: bool,
    /// The IDs of tasks that the hub said it does not have. The client does
    /// not ask for them again.
    pub tasks_absent: BTreeSet<String>,
    pub approvals: BTreeMap<String, Approval>,
    pub decisions: BTreeMap<String, Decision>,
    pub read_markers: BTreeMap<String, ReadMarker>,
    /// What the view reads of the session items, by session ID and item ID.
    pub session_items: BTreeMap<String, BTreeMap<String, ItemSummary>>,
    /// The newest item of each session, by session ID, with the channel of
    /// its feed entry. The activity line of the Three voices mark reads it.
    pub session_activity: BTreeMap<String, crate::activity::SessionActivity>,
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
    /// The `seq` of the newest feed entry that changed or deleted an entity,
    /// by kind and ID. A snapshot that was read before that `seq` must not
    /// overwrite the entity. See [`Cache::merge_entities`].
    pub touched: HashMap<(EntityKind, String), u64>,
    /// The sign-ins that this client started, by ID: from the
    /// `sign_in.start` result, then from their feed entries.
    pub sign_ins: BTreeMap<String, SignIn>,
    /// The sign-ins that the user dismissed. A late feed entry does not
    /// bring them back.
    pub sign_ins_dismissed: BTreeSet<String>,
    /// The ACP registry, once loaded, and how its load went.
    pub acp_registry: Vec<AcpRegistryAgent>,
    pub acp_registry_load: crate::view::RegistryLoad,
    /// The icons of registry agents that loaded, as SVG text, by agent ID.
    pub acp_icons: BTreeMap<String, String>,
    /// The user's preferences, from the `all` snapshot and the feed. Empty
    /// until they load.
    pub preferences: Preferences,
    /// The connections, by ID, from the `all` snapshot and the feed.
    pub connections: BTreeMap<String, Connection>,
    /// The installed skills, by ID, from the `all` snapshot and the feed.
    pub skills: BTreeMap<String, Skill>,
    /// What the client asked the hub about connectors and skills. These are
    /// not feed entities, so a reload keeps them.
    pub extensions: Extensions,
    /// The hub status and the paired clients. See [`crate::admin`].
    pub admin: crate::admin::AdminCache,
    /// Phase 4: search, message threads, the open item, archived tasks.
    pub convo: crate::convo::ConvoCache,
    /// The proactivity settings and log. See [`crate::proactivity`].
    pub proactivity: crate::proactivity::ProactivityCache,
}

/// A memory note in the cache. A list gives only the summary, so the text
/// is there only after a read, a write or a feed entry.
#[derive(Clone, Debug, PartialEq)]
pub struct CachedNote {
    pub note: MemoryNote,
    /// `note.content` is the text of `note.sha256`.
    pub has_content: bool,
}

impl CachedNote {
    fn full(note: MemoryNote) -> Self {
        Self {
            note,
            has_content: true,
        }
    }

    fn summary(summary: MemoryNoteSummary) -> Self {
        Self {
            note: MemoryNote {
                scope: summary.scope,
                path: summary.path,
                content: String::new(),
                sha256: summary.sha256,
                version: summary.version,
                updated_by: summary.updated_by,
                updated_at: summary.updated_at,
                suggested_by: summary.suggested_by,
                deleted_at: summary.deleted_at,
            },
            has_content: false,
        }
    }
}

/// The parts of the connector and skill views that the client loads on
/// request: the Composio status, the catalog, the tools of services, the
/// skill directory and the logos.
#[derive(Clone, Debug, Default)]
pub struct Extensions {
    pub composio: crate::connectors::ComposioView,
    pub catalog: crate::connectors::CatalogView,
    /// Counts the catalog searches, so an old answer does not replace a
    /// newer one.
    pub catalog_serial: u64,
    pub service_tools: BTreeMap<String, crate::connectors::ServiceToolsView>,
    pub directory: crate::skills::SkillDirectoryView,
    pub directory_serial: u64,
    /// The logos that loaded, by URL.
    pub logos: BTreeMap<String, crate::connectors::LogoImage>,
    /// The logo URLs that the client tried, so a logo that does not load is
    /// not asked for again.
    pub logos_tried: BTreeSet<String>,
}

/// Why a message that waited for a channel did not go out when the channel
/// was deleted.
pub const CHANNEL_DELETED: &str = "The channel was deleted.";

/// A feed entry older than this many `seq` steps is forgotten by `touched`.
/// A snapshot read that long ago is not in flight any more.
const TOUCHED_WINDOW: u64 = 20_000;

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
        let dismissed = std::mem::take(&mut self.sign_ins_dismissed);
        let extensions = std::mem::take(&mut self.extensions);
        // No snapshot holds clients or the hub status, so they stay.
        let admin = std::mem::take(&mut self.admin);
        let local_blobs = std::mem::take(&mut self.local_blobs);
        *self = Cache::default();
        self.extensions = extensions;
        self.admin = admin;
        // The local blob cache is on disk, so it stays.
        self.local_blobs = local_blobs;
        // A sign-in that the user closed stays closed after a reload.
        self.sign_ins_dismissed = dismissed;
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

    /// Moves every message that waits for the hub to the failed sends, with
    /// `reason` as the text that says what happened. The user keeps the text
    /// and can send it again. Used when the messages cannot go to the hub
    /// they were written for, such as after pairing with another hub.
    pub fn fail_pending(&mut self, reason: &str) {
        for p in std::mem::take(&mut self.pending) {
            let channel_name = self
                .channels
                .get(&p.channel_id)
                .map(|c| c.name.clone())
                .unwrap_or_default();
            self.send_failures.push(SendFailure {
                id: p.client_message_id,
                channel_id: p.channel_id,
                thread_root_id: p.thread_root_id,
                body: p.body,
                channel_name,
                message: reason.to_owned(),
            });
        }
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
        // A channel snapshot holds all the files of the channel, so it
        // replaces the files that the cache held for it.
        let prefix = format!("{id}:");
        let fresh: BTreeSet<String> = snapshot
            .channel_files
            .iter()
            .map(ChannelFile::entity_id)
            .collect();
        let at = snapshot.seq;
        let stale: Vec<String> = self
            .channel_files
            .keys()
            .filter(|k| {
                k.starts_with(&prefix)
                    && !fresh.contains(*k)
                    && !self.changed_after(EntityKind::ChannelFile, k, at)
            })
            .cloned()
            .collect();
        for key in stale {
            self.channel_files.remove(&key);
        }
        self.merge_entities(snapshot);
        self.files_loaded.insert(id.clone());
        self.channels_loaded.insert(id);
    }

    /// Replaces the files of one channel with the answer to `files.list`.
    /// A feed entry that changed a file after `asked_at` wins.
    pub fn load_files(&mut self, channel_id: &str, files: Vec<ChannelFile>, asked_at: u64) {
        let prefix = format!("{channel_id}:");
        let fresh: BTreeSet<String> = files.iter().map(ChannelFile::entity_id).collect();
        let stale: Vec<String> = self
            .channel_files
            .keys()
            .filter(|k| {
                k.starts_with(&prefix)
                    && !fresh.contains(*k)
                    && !self.changed_after(EntityKind::ChannelFile, k, asked_at)
            })
            .cloned()
            .collect();
        for key in stale {
            self.channel_files.remove(&key);
        }
        for file in files {
            self.merge_file(file, asked_at);
        }
        self.files_loaded.insert(channel_id.to_owned());
    }

    /// Adds or replaces one channel file, as from the answer to `files.put`
    /// or `files.move`, unless a feed entry changed it after `asked_at`.
    pub fn merge_file(&mut self, file: ChannelFile, asked_at: u64) {
        let key = file.entity_id();
        if self.changed_after(EntityKind::ChannelFile, &key, asked_at) {
            return;
        }
        if file.deleted_at.is_some() {
            self.channel_files.remove(&key);
        } else {
            self.channel_files.insert(key, file);
        }
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
    ///
    /// A message that waited in the outbox for the channel is not dropped.
    /// It moves to the failed sends with the reason, so the user keeps the
    /// text.
    fn remove_channel(&mut self, id: &str) -> bool {
        self.proactivity.drop_channel(id);
        let channel = self.channels.remove(id);
        let channel_name = channel.as_ref().map(|c| c.name.clone()).unwrap_or_default();
        let mut removed = channel.is_some();
        let mut sessions: BTreeSet<String> = BTreeSet::new();
        let mut cards: BTreeSet<String> = BTreeSet::new();
        let in_channel = |c: &ChannelId| c.to_string() == id;
        let prefix = format!("{id}:");
        self.channel_files.retain(|k, _| !k.starts_with(&prefix));
        self.files_loaded.remove(id);
        let gone: Vec<String> = self
            .routines
            .iter()
            .filter(|(_, r)| in_channel(&r.channel_id))
            .map(|(k, _)| k.clone())
            .collect();
        for routine in gone {
            removed = true;
            self.routines.remove(&routine);
            self.routine_runs.remove(&routine);
            self.runs_loaded.remove(&routine);
        }
        let before = self.memory_notes.len();
        self.memory_notes.retain(|k, _| !k.starts_with(&prefix));
        removed |= self.memory_notes.len() != before;
        self.memory_loaded.remove(id);
        self.memory_deleted_loaded.remove(id);
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
        let tasks = &self.tasks;
        self.task_previews.retain(|id| tasks.contains_key(id));
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
        let before = self.session_activity.len();
        self.session_activity.retain(|session, a| {
            !sessions.contains(session) && a.channel_id.as_deref() != Some(id)
        });
        removed |= self.session_activity.len() != before;
        let (waiting, other): (Vec<PendingSend>, Vec<PendingSend>) =
            std::mem::take(&mut self.pending)
                .into_iter()
                .partition(|p| p.channel_id == id);
        self.pending = other;
        for p in waiting {
            removed = true;
            self.send_failures.push(SendFailure {
                id: p.client_message_id,
                channel_id: p.channel_id,
                thread_root_id: p.thread_root_id,
                body: p.body,
                channel_name: channel_name.clone(),
                message: CHANNEL_DELETED.to_owned(),
            });
        }
        self.has_older.remove(id);
        self.channels_loaded.remove(id);
        for card in cards {
            self.loaded_threads.remove(&card);
        }
        removed
    }

    /// Adds the entities of a channel snapshot, and keeps the feed position.
    /// Feed entries after the `all` snapshot replay over them in order, so a
    /// change that arrives meanwhile is not lost.
    ///
    /// The snapshot may be older than the cache: the hub read it at its
    /// `seq`, and feed entries above that `seq` may have arrived before the
    /// answer did. Those entries are newer than the snapshot, so an entity
    /// that one of them changed or deleted keeps the cache's version. The
    /// rest of the snapshot is merged.
    pub fn merge_entities(&mut self, mut snapshot: Snapshot) {
        let seq = self.seq;
        if snapshot.seq < seq {
            self.drop_newer_than(&mut snapshot);
        }
        self.merge_snapshot(snapshot);
        self.seq = seq;
    }

    /// True when a feed entry above `seq` changed or deleted the entity.
    fn changed_after(&self, kind: EntityKind, id: &str, seq: u64) -> bool {
        self.touched
            .get(&(kind, id.to_owned()))
            .is_some_and(|at| *at > seq)
    }

    /// Takes out of `snapshot` the entities that the feed changed after the
    /// snapshot was read.
    fn drop_newer_than(&self, snapshot: &mut Snapshot) {
        let at = snapshot.seq;
        snapshot
            .groups
            .retain(|g| !self.changed_after(EntityKind::Group, &g.id.to_string(), at));
        snapshot
            .channels
            .retain(|c| !self.changed_after(EntityKind::Channel, &c.id.to_string(), at));
        snapshot
            .repos
            .retain(|r| !self.changed_after(EntityKind::Repo, &r.id.to_string(), at));
        snapshot
            .workers
            .retain(|w| !self.changed_after(EntityKind::Worker, &w.id.to_string(), at));
        snapshot
            .read_markers
            .retain(|m| !self.changed_after(EntityKind::ReadMarker, &marker_key(m), at));
        snapshot
            .messages
            .retain(|m| !self.changed_after(EntityKind::Message, &m.id.to_string(), at));
        snapshot
            .tasks
            .retain(|t| !self.changed_after(EntityKind::Task, &t.id.to_string(), at));
        snapshot
            .approvals
            .retain(|a| !self.changed_after(EntityKind::Approval, &a.id.to_string(), at));
        snapshot
            .decisions
            .retain(|d| !self.changed_after(EntityKind::Decision, &d.id.to_string(), at));
        snapshot
            .sign_ins
            .retain(|s| !self.changed_after(EntityKind::SignIn, &s.id.to_string(), at));
        snapshot
            .connections
            .retain(|c| !self.changed_after(EntityKind::Connection, &c.id.to_string(), at));
        snapshot
            .skills
            .retain(|s| !self.changed_after(EntityKind::Skill, &s.id.to_string(), at));
        snapshot
            .channel_files
            .retain(|f| !self.changed_after(EntityKind::ChannelFile, &f.entity_id(), at));
        snapshot
            .routines
            .retain(|r| !self.changed_after(EntityKind::Routine, &r.id.to_string(), at));
        if self.changed_after(EntityKind::Preferences, PREFERENCES_ENTITY_ID, at) {
            snapshot.preferences = None;
        }
        if self.changed_after(
            EntityKind::ProactivitySettings,
            consort_proto::model::PROACTIVITY_ENTITY_ID,
            at,
        ) {
            snapshot.proactivity = None;
        }
    }

    /// Remembers that the feed entry at `seq` changed the entity.
    fn touch(&mut self, kind: EntityKind, id: String, seq: u64) {
        if seq == 0 {
            return;
        }
        self.touched.insert((kind, id), seq);
        if self.touched.len() > 2 * TOUCHED_WINDOW as usize {
            let floor = self.seq.saturating_sub(TOUCHED_WINDOW);
            self.touched.retain(|_, at| *at > floor);
        }
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
        // A task that the snapshot cut is a preview. One that it did not cut
        // is whole, even when the cache held a preview of it before.
        self.tasks_have_more |= snapshot.tasks_has_more;
        let cut: BTreeSet<TaskId> = snapshot.tasks_cut.into_iter().collect();
        for t in snapshot.tasks {
            let id = t.id.to_string();
            if cut.contains(&t.id) {
                self.task_previews.insert(id.clone());
            } else {
                self.task_previews.remove(&id);
            }
            self.tasks_absent.remove(&id);
            self.tasks.insert(id, t);
        }
        for a in snapshot.approvals {
            self.approvals.insert(a.id.to_string(), a);
        }
        for d in snapshot.decisions {
            self.decisions.insert(d.id.to_string(), d);
        }
        // A sign-in that runs now, also one that another client or an earlier
        // run of this client started. Its feed entries then update it.
        for s in snapshot.sign_ins {
            let id = s.id.to_string();
            if !s.state.is_final() && !self.sign_ins_dismissed.contains(&id) {
                self.sign_ins.insert(id, s);
            }
        }
        // Only the `all` snapshot has the preferences, the connections and
        // the skills.
        if let Some(preferences) = snapshot.preferences {
            self.preferences = preferences;
        }
        if let Some(settings) = snapshot.proactivity {
            self.proactivity.settings = Some(settings);
        }
        for c in snapshot.connections {
            self.connections.insert(c.id.to_string(), c);
        }
        for s in snapshot.skills {
            self.skills.insert(s.id.to_string(), s);
        }
        for f in snapshot.channel_files {
            if f.deleted_at.is_none() {
                self.channel_files.insert(f.entity_id(), f);
            }
        }
        for r in snapshot.routines {
            self.routines.insert(r.id.to_string(), r);
        }
    }

    /// Replaces the runs of a routine with the answer to `routine.runs`. A
    /// feed entry that changed a run after `asked_at` wins.
    pub fn load_routine_runs(&mut self, routine_id: &str, runs: Vec<RoutineRun>, asked_at: u64) {
        let kept: BTreeMap<u64, RoutineRun> = self
            .routine_runs
            .remove(routine_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|(id, _)| self.changed_after(EntityKind::RoutineRun, &id.to_string(), asked_at))
            .collect();
        let mut fresh: BTreeMap<u64, RoutineRun> = runs.into_iter().map(|r| (r.id, r)).collect();
        fresh.extend(kept);
        self.routine_runs.insert(routine_id.to_owned(), fresh);
        self.runs_loaded.insert(routine_id.to_owned());
    }

    /// Replaces the notes of a scope with the answer to `memory.list`: the
    /// notes that are not deleted, or (with `deleted`) the ones in Recently
    /// deleted. A feed entry that changed a note after `asked_at` wins. A
    /// note keeps its text when its hash did not change.
    pub fn load_memory(
        &mut self,
        scope: &str,
        notes: Vec<MemoryNoteSummary>,
        deleted: bool,
        asked_at: u64,
    ) {
        let prefix = format!("{scope}:");
        let fresh: BTreeSet<String> = notes
            .iter()
            .map(|n| consort_proto::model::memory_note_entity_id(n.scope, &n.path))
            .collect();
        let stale: Vec<String> = self
            .memory_notes
            .iter()
            .filter(|(k, n)| {
                k.starts_with(&prefix)
                    && n.note.deleted_at.is_some() == deleted
                    && !fresh.contains(*k)
                    && !self.changed_after(EntityKind::MemoryNote, k, asked_at)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for key in stale {
            self.memory_notes.remove(&key);
        }
        for summary in notes {
            let key = consort_proto::model::memory_note_entity_id(summary.scope, &summary.path);
            if self.changed_after(EntityKind::MemoryNote, &key, asked_at) {
                continue;
            }
            let mut cached = CachedNote::summary(summary);
            if let Some(old) = self.memory_notes.get(&key)
                && old.has_content
                && old.note.sha256 == cached.note.sha256
            {
                cached.note.content = old.note.content.clone();
                cached.has_content = true;
            }
            self.memory_notes.insert(key, cached);
        }
        if deleted {
            self.memory_deleted_loaded.insert(scope.to_owned());
        } else {
            self.memory_loaded.insert(scope.to_owned());
        }
    }

    /// Adds or replaces one full note, as from `memory.read`, `memory.write`
    /// or `memory.restore`, unless a feed entry changed it after `asked_at`.
    pub fn merge_memory_note(&mut self, note: MemoryNote, asked_at: u64) {
        let key = note.entity_id();
        if self.changed_after(EntityKind::MemoryNote, &key, asked_at) {
            return;
        }
        self.memory_notes.insert(key, CachedNote::full(note));
    }

    /// Adds or replaces one connection, as from the answer to
    /// `connection.connect`. A feed entry that is newer wins: the answer
    /// does not overwrite a status that the feed changed after it.
    pub fn merge_connection(&mut self, connection: Connection) {
        let id = connection.id.to_string();
        let newer = self
            .connections
            .get(&id)
            .is_some_and(|c| c.updated_at > connection.updated_at);
        if !newer {
            self.connections.insert(id, connection);
        }
    }

    /// The same for a skill.
    pub fn merge_skill(&mut self, skill: Skill) {
        let id = skill.id.to_string();
        let newer = self
            .skills
            .get(&id)
            .is_some_and(|s| s.updated_at > skill.updated_at);
        if !newer {
            self.skills.insert(id, skill);
        }
    }

    /// The logo URLs that the view uses and that the client has not tried
    /// yet: those of the connections, of the catalog and of the cards.
    pub fn wanted_logos(&self) -> Vec<String> {
        let mut urls: BTreeSet<String> = BTreeSet::new();
        urls.extend(self.connections.values().filter_map(|c| c.logo_url.clone()));
        urls.extend(
            self.extensions
                .catalog
                .services
                .iter()
                .filter_map(|s| s.logo.url.clone()),
        );
        for a in self.approvals.values() {
            if let consort_proto::model::ApprovalDetails::ConnectService(card) = &a.details
                && let Some(url) = &card.logo_url
            {
                urls.insert(url.clone());
            }
        }
        urls.into_iter()
            .filter(|u| !self.extensions.logos_tried.contains(u))
            .collect()
    }

    fn insert_message(&mut self, message: Message) {
        if let Some(cmid) = &message.client_message_id {
            self.pending.retain(|p| &p.client_message_id != cmid);
        }
        self.messages.insert(message.id.to_string(), message);
    }

    /// Stores a task with its full text.
    fn insert_full_task(&mut self, task: Task) {
        let id = task.id.to_string();
        self.task_previews.remove(&id);
        self.tasks_absent.remove(&id);
        self.tasks.insert(id, task);
    }

    /// Adds the answer to `task.get`. `asked_at` is the feed position
    /// (`seq`) of the cache when the client sent the request, so the hub read
    /// the task at that position or later. A feed entry above `asked_at` that
    /// changed or deleted the task is newer than the answer may be, and it
    /// holds the full task already. Then the answer is dropped, as a snapshot
    /// is in [`Cache::merge_entities`]. Returns true when the cache took the
    /// answer.
    pub fn merge_task(&mut self, task: Task, asked_at: u64) -> bool {
        if self.changed_after(EntityKind::Task, &task.id.to_string(), asked_at) {
            return false;
        }
        self.insert_full_task(task);
        true
    }

    /// Remembers that the hub has no task with this ID, so the client does
    /// not ask for it again. A task that the cache holds stays.
    pub fn mark_task_absent(&mut self, task_id: &str) {
        if !self.tasks.contains_key(task_id) {
            self.tasks_absent.insert(task_id.to_owned());
        }
    }

    /// True when the cache must ask the hub for the task before a view may
    /// show its full brief and review summary: the cache holds only a
    /// preview, or the task is not in the cache and a snapshot left tasks
    /// out, so the hub may know it.
    pub fn needs_task_fetch(&self, task_id: &str) -> bool {
        self.task_previews.contains(task_id)
            || (self.tasks_have_more
                && !self.tasks.contains_key(task_id)
                && !self.tasks_absent.contains(task_id))
    }

    /// The tasks that the client should fetch with `task.get` without a
    /// user action:
    /// - a task that a task card names and the cache does not hold, when a
    ///   snapshot left tasks out (`tasks_have_more`). Without that, a task
    ///   that is not in the cache does not exist, or its feed entry is on
    ///   its way,
    /// - a task whose thread is open and that is a preview again, for
    ///   example after a snapshot loaded again.
    pub fn tasks_to_fetch(&self) -> Vec<TaskId> {
        let mut wanted: BTreeMap<String, TaskId> = BTreeMap::new();
        for id in &self.task_previews {
            if let Some(task) = self.tasks.get(id)
                && self
                    .loaded_threads
                    .contains(&task.card_message_id.to_string())
            {
                wanted.insert(id.clone(), task.id);
            }
        }
        if self.tasks_have_more {
            for message in self.messages.values() {
                if message.kind != MessageKind::TaskCard || message.deleted_at.is_some() {
                    continue;
                }
                if let Some(task_id) = message.author_task_id {
                    let id = task_id.to_string();
                    if !self.tasks.contains_key(&id) && !self.tasks_absent.contains(&id) {
                        wanted.insert(id, task_id);
                    }
                }
            }
        }
        wanted.into_values().collect()
    }
    /// Applies one feed entry. Entries at or below the current `seq` are
    /// ignored, so a replay does no harm. Returns true when the entry changed
    /// the cache.
    pub fn apply(&mut self, entry: FeedEntry) -> bool {
        if entry.seq != 0 && entry.seq <= self.seq {
            return false;
        }
        self.seq = self.seq.max(entry.seq);
        // A session item is not in any snapshot, and it changes often.
        let kind = entry.change.kind();
        if kind != EntityKind::SessionItem {
            self.touch(kind, entry.entity_id.clone(), entry.seq);
        }
        let entry_channel = entry.channel_id.map(|c| c.to_string());
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
                // A feed entry always holds the full task.
                EntityPayload::Task(t) => self.insert_full_task(t),
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
                    let activity = crate::activity::SessionActivity::of(&item, entry_channel);
                    let newer = self
                        .session_activity
                        .get(&activity.session_id)
                        .is_none_or(|old| {
                            old.updated_at <= activity.updated_at && *old != activity
                        });
                    if newer {
                        self.session_activity
                            .insert(activity.session_id.clone(), activity);
                    }
                    if let Some(summary) = ItemSummary::of(&item) {
                        self.session_items
                            .entry(item.session_id.to_string())
                            .or_default()
                            .insert(item.id.to_string(), summary);
                    } else {
                        return newer;
                    }
                }
                // Only the sign-ins that this client started show. Another
                // client's sign-in is not this user's flow to follow here.
                EntityPayload::SignIn(sign_in) => {
                    let id = sign_in.id.to_string();
                    // A sign-in that the hub started for a sign-in card shows
                    // too: the user follows it from the card.
                    let from_card = self.approvals.values().any(|a| {
                        matches!(&a.details, consort_proto::model::ApprovalDetails::SignIn(card)
                            if card.sign_in_id == Some(sign_in.id))
                    });
                    if (!self.sign_ins.contains_key(&id) && !from_card)
                        || self.sign_ins_dismissed.contains(&id)
                    {
                        return false;
                    }
                    self.sign_ins.insert(id, sign_in);
                }
                EntityPayload::Preferences(preferences) => {
                    if self.preferences == preferences {
                        return false;
                    }
                    self.preferences = preferences;
                }
                EntityPayload::Connection(c) => {
                    self.connections.insert(c.id.to_string(), c);
                }
                EntityPayload::ProactivitySettings(settings) => {
                    return self.proactivity.apply_settings(settings);
                }
                EntityPayload::ProactiveWake(wake) => return self.proactivity.apply_wake(wake),
                EntityPayload::Skill(skill) => {
                    self.skills.insert(skill.id.to_string(), skill);
                }
                EntityPayload::ChannelFile(file) => {
                    // Only the files of a channel whose files are loaded:
                    // a partial list of another channel would look whole.
                    let channel = file.channel_id.to_string();
                    if !self.files_loaded.contains(&channel) {
                        return false;
                    }
                    let key = file.entity_id();
                    if file.deleted_at.is_some() {
                        return self.channel_files.remove(&key).is_some();
                    }
                    if self.channel_files.get(&key) == Some(&file) {
                        return false;
                    }
                    self.channel_files.insert(key, file);
                }
                EntityPayload::Client(client) => {
                    let id = client.id.to_string();
                    if self.admin.clients.get(&id) == Some(&client) {
                        return false;
                    }
                    self.admin.clients.insert(id, client);
                }
                // Memory notes and routines have no view yet.
                EntityPayload::Routine(routine) => {
                    let id = routine.id.to_string();
                    if self.routines.get(&id) == Some(&routine) {
                        return false;
                    }
                    self.routines.insert(id, routine);
                }
                EntityPayload::RoutineRun(run) => {
                    // Only the runs of a routine whose runs are loaded.
                    let routine = run.routine_id.to_string();
                    if !self.runs_loaded.contains(&routine) {
                        return false;
                    }
                    let runs = self.routine_runs.entry(routine).or_default();
                    if runs.get(&run.id) == Some(&run) {
                        return false;
                    }
                    runs.insert(run.id, run);
                }
                EntityPayload::MemoryNote(note) => {
                    // Only the notes of a scope whose notes are loaded.
                    let scope = note.scope.to_string();
                    let list = if note.deleted_at.is_some() {
                        &self.memory_deleted_loaded
                    } else {
                        &self.memory_loaded
                    };
                    if !list.contains(&scope) && !self.memory_notes.contains_key(&note.entity_id())
                    {
                        return false;
                    }
                    self.memory_notes
                        .insert(note.entity_id(), CachedNote::full(note));
                }
            },
            FeedChange::Delete(kind) => {
                let id = &entry.entity_id;
                let removed = match kind {
                    EntityKind::Group => self.groups.remove(id).is_some(),
                    EntityKind::Channel => self.remove_channel(id),
                    EntityKind::Worker => self.workers.remove(id).is_some(),
                    EntityKind::Repo => self.repos.remove(id).is_some(),
                    EntityKind::Message => self.messages.remove(id).is_some(),
                    EntityKind::Task => {
                        self.task_previews.remove(id);
                        self.tasks.remove(id).is_some()
                    }
                    EntityKind::Approval => self.approvals.remove(id).is_some(),
                    EntityKind::Decision => self.decisions.remove(id).is_some(),
                    EntityKind::ReadMarker => self.read_markers.remove(id).is_some(),
                    EntityKind::SignIn => self.sign_ins.remove(id).is_some(),
                    EntityKind::Connection => self.connections.remove(id).is_some(),
                    EntityKind::Skill => self.skills.remove(id).is_some(),
                    EntityKind::ChannelFile => self.channel_files.remove(id).is_some(),
                    EntityKind::Client => self.admin.clients.remove(id).is_some(),
                    EntityKind::Routine => {
                        self.routine_runs.remove(id);
                        self.runs_loaded.remove(id);
                        self.routines.remove(id).is_some()
                    }
                    EntityKind::RoutineRun => id.parse::<u64>().is_ok_and(|run| {
                        self.routine_runs
                            .values_mut()
                            .any(|runs| runs.remove(&run).is_some())
                    }),
                    EntityKind::MemoryNote => self.memory_notes.remove(id).is_some(),
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

#[cfg(test)]
mod tests {
    use consort_proto::{
        ids::{ChannelId, GroupId, MessageId},
        model::{ApprovalMode, Author, MessageKind, PlacementRequest, TaskCreator},
        status::TaskStatus,
        time::Timestamp,
    };

    use super::*;

    fn task(id: TaskId, channel: ChannelId, card: MessageId, brief: &str) -> Task {
        Task {
            id,
            channel_id: channel,
            title: "A task".into(),
            brief: brief.into(),
            status: TaskStatus::Review,
            status_reason: None,
            created_by: TaskCreator::Assistant,
            card_message_id: card,
            model_selection: None,
            placement: PlacementRequest::default(),
            approval_mode: ApprovalMode::Ask,
            start_at: None,
            current_session_id: None,
            worker_id: None,
            review_summary: None,
            resolved_at: None,
            session_ended: false,
            archived_at: None,
            created_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(1),
        }
    }

    fn card(id: MessageId, channel: ChannelId, task: TaskId) -> Message {
        Message {
            kind: MessageKind::TaskCard,
            author_task_id: Some(task),
            ..text(id, channel, "")
        }
    }

    /// A cache that holds one task as a preview, as after a snapshot.
    fn cache_with_preview() -> (Cache, TaskId, ChannelId, MessageId) {
        let (id, channel, card) = (TaskId::new(), ChannelId::new(), MessageId::new());
        let mut cache = Cache::default();
        let mut snapshot = snapshot_at(10);
        snapshot.tasks = vec![task(id, channel, card, "The start of a long brief")];
        snapshot.tasks_cut = vec![id];
        cache.load_snapshot(snapshot);
        (cache, id, channel, card)
    }

    fn group(id: GroupId, name: &str) -> Group {
        Group {
            id,
            name: name.into(),
            position: 0,
            collapsed: false,
        }
    }

    fn text(id: MessageId, channel: ChannelId, body: &str) -> Message {
        Message {
            id,
            channel_id: channel,
            thread_root_id: None,
            author: Author::Assistant,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            kind: MessageKind::Text,
            body: body.into(),
            attachments: vec![],
            in_timeline: true,
            client_message_id: None,
            reply_count: 0,
            last_reply_at: None,
            reactions: vec![],
            edited_at: None,
            deleted_at: None,
            created_at: Timestamp::from_millis(1),
            proactive: None,
            suggestions: Vec::new(),
        }
    }

    fn channel_file(channel: ChannelId, path: &str, byte: u8) -> ChannelFile {
        ChannelFile {
            channel_id: channel,
            path: path.into(),
            blob_sha256: consort_proto::model::Sha256Hash::from_bytes([byte; 32]),
            size: 10,
            mime: "application/pdf".into(),
            task_id: None,
            title: None,
            updated_by: "user".into(),
            updated_at: Timestamp::from_millis(1),
            deleted_at: None,
        }
    }

    /// The cache dropped the `channel_file` feed entries. It keeps them now
    /// for the channels whose files it holds.
    #[test]
    fn the_cache_keeps_and_reduces_the_files_of_a_loaded_channel() {
        let mut cache = Cache::default();
        let (lights, other) = (ChannelId::new(), ChannelId::new());
        let mut snapshot = snapshot_at(10);
        snapshot.channel_files = vec![
            channel_file(lights, "photos/front.jpg", 1),
            channel_file(lights, "quote.pdf", 2),
        ];
        cache.load_channel(lights, snapshot);
        assert!(cache.files_loaded.contains(&lights.to_string()));
        assert_eq!(cache.channel_files.len(), 2);
        // A new file and a change arrive in the feed.
        assert!(cache.apply(upsert(
            11,
            EntityPayload::ChannelFile(channel_file(lights, "wiring.pdf", 3))
        )));
        let mut changed = channel_file(lights, "quote.pdf", 4);
        changed.size = 99;
        assert!(cache.apply(upsert(12, EntityPayload::ChannelFile(changed))));
        assert_eq!(cache.channel_files[&format!("{lights}:quote.pdf")].size, 99);
        // The same entry again changes nothing.
        let same = cache.channel_files[&format!("{lights}:quote.pdf")].clone();
        assert!(!cache.apply(upsert(0, EntityPayload::ChannelFile(same))));
        // A file of a channel whose files are not loaded does not make a
        // partial list.
        assert!(!cache.apply(upsert(
            13,
            EntityPayload::ChannelFile(channel_file(other, "x.txt", 5))
        )));
        assert_eq!(cache.channel_files.len(), 3);
        // A file with `deleted_at` and a delete entry both remove it.
        let mut gone = channel_file(lights, "wiring.pdf", 3);
        gone.deleted_at = Some(Timestamp::from_millis(2));
        assert!(cache.apply(upsert(14, EntityPayload::ChannelFile(gone))));
        let delete = FeedEntry {
            seq: 15,
            at: Timestamp::from_millis(0),
            entity_id: format!("{lights}:photos/front.jpg"),
            channel_id: Some(lights),
            change: FeedChange::Delete(EntityKind::ChannelFile),
        };
        assert!(cache.apply(delete));
        let left: Vec<_> = cache
            .channel_files
            .values()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(left, ["quote.pdf"]);
        // An older `files.list` answer does not bring back the deleted file
        // or overwrite the newer size.
        cache.load_files(
            &lights.to_string(),
            vec![
                channel_file(lights, "photos/front.jpg", 1),
                channel_file(lights, "quote.pdf", 2),
            ],
            9,
        );
        let left: Vec<_> = cache.channel_files.values().map(|f| f.size).collect();
        assert_eq!(left, [99]);
        // A newer answer replaces the list.
        cache.load_files(
            &lights.to_string(),
            vec![channel_file(lights, "new.pdf", 6)],
            20,
        );
        let left: Vec<_> = cache
            .channel_files
            .values()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(left, ["new.pdf"]);
        // The files go with their channel.
        let delete = FeedEntry {
            seq: 21,
            at: Timestamp::from_millis(0),
            entity_id: lights.to_string(),
            channel_id: Some(lights),
            change: FeedChange::Delete(EntityKind::Channel),
        };
        cache.apply(delete);
        assert!(cache.channel_files.is_empty());
        assert!(!cache.files_loaded.contains(&lights.to_string()));
    }

    #[test]
    fn the_view_names_the_task_of_an_artifact_attachment() {
        let mut cache = Cache::default();
        let (channel, task_id, card_id) = (ChannelId::new(), TaskId::new(), MessageId::new());
        let mut snapshot = snapshot_at(10);
        let mut artifact = channel_file(channel, "artifacts/x/comparison.xlsx", 7);
        artifact.task_id = Some(task_id);
        snapshot.channel_files = vec![artifact.clone()];
        snapshot.tasks = vec![task(task_id, channel, card_id, "brief")];
        snapshot.channels = vec![crate::test_entities::channel(channel, "house-lights")];
        let mut message = text(MessageId::new(), channel, "Ready.");
        message.attachments = vec![BlobRef {
            sha256: artifact.blob_sha256.clone(),
            name: "comparison.xlsx".into(),
            mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
            size: 10,
        }];
        snapshot.messages = vec![message];
        cache.load_channel(channel, snapshot);
        let state = crate::reduce::view_state(
            &cache,
            crate::view::Connection::Online,
            crate::view::UserView::default(),
            0,
        );
        let attachment = state
            .timelines
            .iter()
            .flat_map(|t| t.items.iter())
            .find_map(|i| match i {
                crate::view::TimelineItem::Message(m) => m.attachments.first().cloned(),
                _ => None,
            })
            .expect("the attachment");
        assert_eq!(attachment.detail, "XLSX, 10 B");
        let task = attachment.artifact.expect("an artifact");
        assert_eq!(task.task_id, task_id.to_string());
        assert_eq!(state.channel_files.len(), 1);
        assert_eq!(state.files_loaded, [channel.to_string()]);
    }

    fn routine(channel: ChannelId) -> consort_proto::model::Routine {
        consort_proto::model::Routine {
            id: consort_proto::ids::RoutineId::new(),
            channel_id: channel,
            name: "Sunset check".into(),
            schedule: consort_proto::model::RoutineSchedule::Interval { seconds: 3_600 },
            prompt: "Check.".into(),
            action: consort_proto::model::RoutineAction::CoordinatorTurn,
            enabled: true,
            next_run_at: None,
            last_run_at: None,
            last_run_status: None,
            created_at: None,
            kind: Default::default(),
            origin: Default::default(),
        }
    }

    fn run(routine: &consort_proto::model::Routine, id: u64) -> RoutineRun {
        RoutineRun {
            id,
            routine_id: routine.id,
            channel_id: routine.channel_id,
            started_at: Timestamp::from_millis(1),
            ended_at: None,
            status: consort_proto::model::RoutineRunStatus::Running,
            manual: false,
            message_id: None,
            task_id: None,
            error: None,
        }
    }

    fn delete(seq: u64, kind: EntityKind, id: String, channel: Option<ChannelId>) -> FeedEntry {
        FeedEntry {
            seq,
            at: Timestamp::from_millis(0),
            entity_id: id,
            channel_id: channel,
            change: FeedChange::Delete(kind),
        }
    }

    #[test]
    fn the_cache_keeps_routines_and_the_runs_of_loaded_routines() {
        let mut cache = Cache::default();
        let lights = ChannelId::new();
        let sunset = routine(lights);
        let mut snapshot = snapshot_at(10);
        snapshot.routines = vec![sunset.clone()];
        cache.load_snapshot(snapshot);
        assert_eq!(cache.routines.len(), 1);

        // A change in the feed replaces the routine. The same entry again
        // changes nothing.
        let mut off = sunset.clone();
        off.enabled = false;
        assert!(cache.apply(upsert(11, EntityPayload::Routine(off.clone()))));
        assert!(!cache.apply(upsert(0, EntityPayload::Routine(off))));
        assert!(!cache.routines[&sunset.id.to_string()].enabled);

        // A run of a routine whose runs are not loaded does not make a
        // partial list.
        assert!(!cache.apply(upsert(12, EntityPayload::RoutineRun(run(&sunset, 1)))));
        cache.load_routine_runs(&sunset.id.to_string(), vec![run(&sunset, 1)], 12);
        let mut ended = run(&sunset, 1);
        ended.status = consort_proto::model::RoutineRunStatus::Ok;
        assert!(cache.apply(upsert(13, EntityPayload::RoutineRun(ended))));
        assert!(cache.apply(upsert(14, EntityPayload::RoutineRun(run(&sunset, 2)))));
        let runs = &cache.routine_runs[&sunset.id.to_string()];
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[&1].status, consort_proto::model::RoutineRunStatus::Ok);
        // An older answer does not undo the feed.
        cache.load_routine_runs(&sunset.id.to_string(), vec![run(&sunset, 1)], 12);
        let runs = &cache.routine_runs[&sunset.id.to_string()];
        assert_eq!(runs[&1].status, consort_proto::model::RoutineRunStatus::Ok);

        // A deleted routine takes its runs with it.
        assert!(cache.apply(delete(
            15,
            EntityKind::Routine,
            sunset.id.to_string(),
            Some(lights)
        )));
        assert!(cache.routines.is_empty() && cache.routine_runs.is_empty());

        // The routines of a deleted channel go with it.
        let other = routine(lights);
        assert!(cache.apply(upsert(16, EntityPayload::Routine(other))));
        cache.apply(delete(
            17,
            EntityKind::Channel,
            lights.to_string(),
            Some(lights),
        ));
        assert!(cache.routines.is_empty());
    }

    fn summary(
        scope: consort_proto::model::MemoryScope,
        path: &str,
        byte: u8,
    ) -> MemoryNoteSummary {
        MemoryNoteSummary {
            scope,
            path: path.into(),
            sha256: consort_proto::model::Sha256Hash::from_bytes([byte; 32]),
            version: 1,
            updated_by: consort_proto::model::MemoryAuthor::User,
            updated_at: Timestamp::from_millis(1),
            suggested_by: None,
            deleted_at: None,
        }
    }

    fn full(summary: &MemoryNoteSummary, content: &str) -> MemoryNote {
        MemoryNote {
            scope: summary.scope,
            path: summary.path.clone(),
            content: content.into(),
            sha256: summary.sha256.clone(),
            version: summary.version,
            updated_by: summary.updated_by,
            updated_at: summary.updated_at,
            suggested_by: None,
            deleted_at: summary.deleted_at,
        }
    }

    #[test]
    fn the_cache_keeps_the_memory_of_loaded_scopes() {
        use consort_proto::model::MemoryScope;
        let mut cache = Cache::default();
        let lights = ChannelId::new();
        let scope = MemoryScope::Channel(lights);
        let roof = summary(scope, "decisions/roofline.md", 1);
        // A note of a scope that is not loaded is dropped.
        assert!(!cache.apply(upsert(5, EntityPayload::MemoryNote(full(&roof, "Lumen.")))));
        cache.load_memory(&lights.to_string(), vec![roof.clone()], false, 5);
        let key = format!("{lights}:decisions/roofline.md");
        assert!(!cache.memory_notes[&key].has_content);
        // A read gives the text. A list with the same hash keeps it.
        cache.merge_memory_note(full(&roof, "Lumen."), 6);
        cache.load_memory(&lights.to_string(), vec![roof.clone()], false, 6);
        assert!(cache.memory_notes[&key].has_content);
        // The feed brings a new version, then the delete to Recently deleted.
        let mut newer = full(&summary(scope, "decisions/roofline.md", 2), "Lumen Co.");
        newer.version = 2;
        assert!(cache.apply(upsert(7, EntityPayload::MemoryNote(newer.clone()))));
        assert_eq!(cache.memory_notes[&key].note.version, 2);
        newer.deleted_at = Some(Timestamp::from_millis(9));
        assert!(cache.apply(upsert(8, EntityPayload::MemoryNote(newer))));
        assert!(cache.memory_notes[&key].note.deleted_at.is_some());
        // An older list does not bring the deleted note back as live.
        cache.load_memory(&lights.to_string(), vec![roof.clone()], false, 6);
        assert!(cache.memory_notes[&key].note.deleted_at.is_some());
        // A purge removes it, and the channel takes its memory with it.
        assert!(cache.apply(delete(9, EntityKind::MemoryNote, key.clone(), Some(lights))));
        assert!(cache.memory_notes.is_empty());
        cache.load_memory(&lights.to_string(), vec![roof], false, 9);
        cache.apply(delete(
            10,
            EntityKind::Channel,
            lights.to_string(),
            Some(lights),
        ));
        assert!(cache.memory_notes.is_empty());
        assert!(!cache.memory_loaded.contains(&lights.to_string()));
    }

    fn upsert(seq: u64, payload: EntityPayload) -> FeedEntry {
        FeedEntry::upsert(seq, Timestamp::from_millis(0), payload)
    }

    fn snapshot_at(seq: u64) -> Snapshot {
        Snapshot {
            seq,
            ..Snapshot::default()
        }
    }

    /// Finding 5 of review cycle 2: a snapshot read at seq N was merged after
    /// feed entries above N, and it restored old data.
    #[test]
    fn an_older_snapshot_does_not_overwrite_what_the_feed_changed_after_it() {
        let mut cache = Cache::default();
        let id = GroupId::new();
        cache.apply(upsert(10, EntityPayload::Group(group(id, "Old name"))));
        // The hub reads a snapshot at seq 10. A rename follows at seq 12.
        let mut snapshot = snapshot_at(10);
        snapshot.groups.push(group(id, "Old name"));
        cache.apply(upsert(12, EntityPayload::Group(group(id, "New name"))));
        cache.merge_entities(snapshot);
        assert_eq!(cache.groups[&id.to_string()].name, "New name");
        assert_eq!(cache.seq, 12, "the feed position stays");
    }

    /// The favorites come from the `all` snapshot and then from the feed,
    /// and the view lists them. An older snapshot does not undo a newer
    /// star, and a channel snapshot leaves them as they are.
    #[test]
    fn the_cache_applies_preferences_from_the_snapshot_and_the_feed() {
        use consort_proto::model::{FavoriteModel, Preferences};
        let prefs = |models: &[&str]| Preferences {
            favorite_models: models
                .iter()
                .map(|m| FavoriteModel {
                    account_label: "Claude Work".into(),
                    model: (*m).into(),
                })
                .collect(),
        };
        let mut cache = Cache::default();
        let mut snapshot = snapshot_at(5);
        snapshot.preferences = Some(prefs(&["opus"]));
        cache.load_snapshot(snapshot);
        assert_eq!(cache.preferences, prefs(&["opus"]));

        assert!(cache.apply(upsert(
            6,
            EntityPayload::Preferences(prefs(&["opus", "haiku"]))
        )));
        assert!(
            !cache.apply(upsert(
                7,
                EntityPayload::Preferences(prefs(&["opus", "haiku"]))
            )),
            "the same preferences change nothing"
        );
        // A snapshot read at seq 5 arrives late, and a channel snapshot has
        // no preferences.
        let mut old = snapshot_at(5);
        old.preferences = Some(prefs(&["opus"]));
        cache.merge_entities(old);
        cache.merge_entities(snapshot_at(9));
        assert_eq!(cache.preferences, prefs(&["opus", "haiku"]));

        let view = crate::reduce::view_state(
            &cache,
            crate::view::Connection::Online,
            crate::view::UserView::default(),
            0,
        );
        let ids: Vec<&str> = view
            .favorite_models
            .iter()
            .map(|f| f.model_id.as_str())
            .collect();
        assert_eq!(ids, ["opus", "haiku"]);
        assert!(view.is_favorite("Claude Work", "haiku"));
        assert!(!view.is_favorite("Claude Personal", "haiku"));
    }

    #[test]
    fn the_cache_keeps_connections_and_skills_from_the_snapshot_and_the_feed() {
        use consort_proto::model::{ConnectionStatus, UseIn};
        let mut cache = Cache::default();
        let work = crate::test_entities::connection("Gmail Work", UseIn::EveryChannel, 1);
        let pdf = crate::test_entities::skill("pdf", 1);
        let mut snapshot = snapshot_at(10);
        snapshot.connections = vec![work.clone()];
        snapshot.skills = vec![pdf.clone()];
        cache.load_snapshot(snapshot);
        assert_eq!(cache.connections.len(), 1);
        assert_eq!(cache.skills.len(), 1);
        // The feed changes the status, and a stale answer does not undo it.
        let mut signed_out = work.clone();
        signed_out.status = ConnectionStatus::NeedsSignIn { message: None };
        signed_out.updated_at = Timestamp::from_millis(5);
        assert!(cache.apply(upsert(11, EntityPayload::Connection(signed_out))));
        cache.merge_connection(work.clone());
        assert!(matches!(
            cache.connections[&work.id.to_string()].status,
            ConnectionStatus::NeedsSignIn { .. }
        ));
        // An older snapshot does not bring back what the feed deleted.
        let delete = FeedEntry {
            seq: 12,
            at: Timestamp::from_millis(0),
            entity_id: pdf.id.to_string(),
            channel_id: None,
            change: FeedChange::Delete(EntityKind::Skill),
        };
        assert!(cache.apply(delete));
        let mut old = snapshot_at(9);
        old.skills = vec![pdf.clone()];
        cache.merge_entities(old);
        assert!(cache.skills.is_empty());
        // A reload keeps what the client loaded on request.
        cache.extensions.composio.key_present = true;
        cache.load_snapshot(snapshot_at(20));
        assert!(cache.extensions.composio.key_present);
        assert!(cache.connections.is_empty());
    }

    #[test]
    fn an_older_snapshot_does_not_bring_back_a_deleted_message() {
        let mut cache = Cache::default();
        let channel = ChannelId::new();
        let keep = MessageId::new();
        let gone = MessageId::new();
        let mut snapshot = snapshot_at(20);
        snapshot.messages = vec![text(keep, channel, "keep"), text(gone, channel, "gone")];
        // The delete of one message arrives before the snapshot answer.
        cache.apply(FeedEntry {
            seq: 22,
            at: Timestamp::from_millis(0),
            channel_id: Some(channel),
            entity_id: gone.to_string(),
            change: FeedChange::Delete(EntityKind::Message),
        });
        cache.merge_entities(snapshot);
        assert!(cache.messages.contains_key(&keep.to_string()));
        assert!(
            !cache.messages.contains_key(&gone.to_string()),
            "a deleted message stays deleted"
        );
    }

    #[test]
    fn a_snapshot_still_fills_what_the_feed_did_not_touch() {
        let mut cache = Cache::default();
        let (touched, other) = (GroupId::new(), GroupId::new());
        cache.apply(upsert(30, EntityPayload::Group(group(touched, "Live"))));
        let mut snapshot = snapshot_at(25);
        snapshot.groups = vec![group(touched, "Stale"), group(other, "From snapshot")];
        cache.merge_entities(snapshot);
        assert_eq!(cache.groups[&touched.to_string()].name, "Live");
        assert_eq!(cache.groups[&other.to_string()].name, "From snapshot");
    }

    #[test]
    fn a_snapshot_that_is_newer_than_the_change_wins() {
        let mut cache = Cache::default();
        let id = GroupId::new();
        cache.apply(upsert(10, EntityPayload::Group(group(id, "Before"))));
        let mut snapshot = snapshot_at(15);
        snapshot.groups = vec![group(id, "After")];
        cache.merge_entities(snapshot);
        assert_eq!(cache.groups[&id.to_string()].name, "After");
    }

    #[test]
    fn a_pending_message_moves_to_the_failed_sends_with_its_text() {
        let mut cache = Cache::default();
        cache.pending.push(PendingSend {
            client_message_id: "c-1".into(),
            channel_id: "chan".into(),
            thread_root_id: Some("card".into()),
            body: "Unsent words".into(),
            created_at: 1,
            attachments: Vec::new(),
        });
        cache.fail_pending("The message did not go out.");
        assert!(cache.pending.is_empty());
        assert_eq!(cache.send_failures.len(), 1);
        let failure = &cache.send_failures[0];
        assert_eq!(failure.id, "c-1");
        assert_eq!(failure.body, "Unsent words");
        assert_eq!(failure.thread_root_id.as_deref(), Some("card"));
        assert_eq!(failure.message, "The message did not go out.");
    }

    #[test]
    fn a_snapshot_marks_the_tasks_it_cut_as_previews() {
        let (cache, id, channel, card) = cache_with_preview();
        assert!(cache.task_previews.contains(&id.to_string()));
        assert!(cache.needs_task_fetch(&id.to_string()));
        // A later snapshot with the whole task ends the preview state.
        let mut cache = cache;
        let mut snapshot = snapshot_at(11);
        snapshot.tasks = vec![task(id, channel, card, "The whole brief")];
        cache.merge_entities(snapshot);
        assert!(cache.task_previews.is_empty());
        assert!(!cache.needs_task_fetch(&id.to_string()));
    }

    #[test]
    fn a_feed_entry_holds_the_full_task_and_ends_the_preview_state() {
        let (mut cache, id, channel, card) = cache_with_preview();
        cache.apply(upsert(
            12,
            EntityPayload::Task(task(id, channel, card, "The whole brief")),
        ));
        assert!(cache.task_previews.is_empty());
        assert_eq!(cache.tasks[&id.to_string()].brief, "The whole brief");
    }

    #[test]
    fn a_deleted_task_and_a_deleted_channel_leave_no_preview_flag() {
        let (mut cache, id, _, _) = cache_with_preview();
        cache.apply(FeedEntry {
            seq: 12,
            at: Timestamp::from_millis(0),
            channel_id: None,
            entity_id: id.to_string(),
            change: FeedChange::Delete(EntityKind::Task),
        });
        assert!(cache.task_previews.is_empty());
        let (mut cache, _, channel, _) = cache_with_preview();
        cache.apply(FeedEntry {
            seq: 12,
            at: Timestamp::from_millis(0),
            channel_id: Some(channel),
            entity_id: channel.to_string(),
            change: FeedChange::Delete(EntityKind::Channel),
        });
        assert!(cache.tasks.is_empty());
        assert!(cache.task_previews.is_empty());
    }

    #[test]
    fn the_answer_to_task_get_replaces_the_preview() {
        let (mut cache, id, channel, card) = cache_with_preview();
        let at = cache.seq;
        assert!(cache.merge_task(task(id, channel, card, "The whole brief"), at));
        assert_eq!(cache.tasks[&id.to_string()].brief, "The whole brief");
        assert!(cache.task_previews.is_empty());
    }

    /// The hub read the task, and a feed entry above the request position
    /// changed it. The feed entry is newer than the answer may be.
    #[test]
    fn an_older_answer_does_not_overwrite_what_the_feed_changed_after_the_request() {
        let (mut cache, id, channel, card) = cache_with_preview();
        let asked_at = cache.seq;
        let mut newer = task(id, channel, card, "The brief after the change");
        newer.title = "Renamed".into();
        cache.apply(upsert(asked_at + 2, EntityPayload::Task(newer)));
        let stale = task(id, channel, card, "The brief before the change");
        assert!(!cache.merge_task(stale, asked_at));
        assert_eq!(cache.tasks[&id.to_string()].title, "Renamed");
        assert_eq!(
            cache.tasks[&id.to_string()].brief,
            "The brief after the change"
        );
        assert!(cache.task_previews.is_empty());
    }

    #[test]
    fn an_answer_does_not_bring_back_a_task_that_the_feed_deleted() {
        let (mut cache, id, channel, card) = cache_with_preview();
        let asked_at = cache.seq;
        cache.apply(FeedEntry {
            seq: asked_at + 1,
            at: Timestamp::from_millis(0),
            channel_id: Some(channel),
            entity_id: id.to_string(),
            change: FeedChange::Delete(EntityKind::Task),
        });
        assert!(!cache.merge_task(task(id, channel, card, "Whole"), asked_at));
        assert!(cache.tasks.is_empty());
    }

    #[test]
    fn an_answer_is_newer_than_a_feed_entry_from_before_the_request() {
        let (mut cache, id, channel, card) = cache_with_preview();
        cache.apply(upsert(
            20,
            EntityPayload::Task(task(id, channel, card, "Old")),
        ));
        // The client asks after the feed entry, so the answer is as new.
        assert!(cache.merge_task(task(id, channel, card, "Newer"), cache.seq));
        assert_eq!(cache.tasks[&id.to_string()].brief, "Newer");
    }

    #[test]
    fn a_task_outside_the_snapshot_is_fetched_only_when_the_snapshot_left_tasks_out() {
        let (channel, outside, card_id) = (ChannelId::new(), TaskId::new(), MessageId::new());
        let mut cache = Cache::default();
        let mut snapshot = snapshot_at(5);
        snapshot.messages = vec![card(card_id, channel, outside)];
        cache.load_snapshot(snapshot);
        assert!(cache.tasks_to_fetch().is_empty());
        assert!(!cache.needs_task_fetch(&outside.to_string()));

        let mut snapshot = snapshot_at(5);
        snapshot.tasks_has_more = true;
        cache.merge_entities(snapshot);
        assert_eq!(cache.tasks_to_fetch(), vec![outside]);
        assert!(cache.needs_task_fetch(&outside.to_string()));

        // The hub has no such task: nobody asks again.
        cache.mark_task_absent(&outside.to_string());
        assert!(cache.tasks_to_fetch().is_empty());
        assert!(!cache.needs_task_fetch(&outside.to_string()));
    }

    #[test]
    fn a_task_that_arrives_ends_the_wish_to_fetch_it() {
        let (channel, outside, card_id) = (ChannelId::new(), TaskId::new(), MessageId::new());
        let mut cache = Cache::default();
        let mut snapshot = snapshot_at(5);
        snapshot.messages = vec![card(card_id, channel, outside)];
        snapshot.tasks_has_more = true;
        cache.load_snapshot(snapshot);
        assert_eq!(cache.tasks_to_fetch(), vec![outside]);
        let at = cache.seq;
        cache.merge_task(task(outside, channel, card_id, "Old work"), at);
        assert!(cache.tasks_to_fetch().is_empty());
    }

    #[test]
    fn a_preview_of_a_task_with_an_open_thread_is_fetched_again_after_a_reload() {
        let (mut cache, id, channel, card_id) = cache_with_preview();
        // No thread is open: the preview may stay.
        assert!(cache.tasks_to_fetch().is_empty());
        cache.loaded_threads.insert(card_id.to_string());
        assert_eq!(cache.tasks_to_fetch(), vec![id]);
        // A reload of the snapshot makes a preview again. The open thread
        // stays marked, so the task is still wanted.
        let mut snapshot = snapshot_at(30);
        snapshot.tasks = vec![task(id, channel, card_id, "Preview")];
        snapshot.tasks_cut = vec![id];
        cache.load_snapshot(snapshot);
        assert_eq!(cache.tasks_to_fetch(), vec![id]);
    }

    #[test]
    fn the_record_of_touched_entities_stays_small() {
        let mut cache = Cache::default();
        let total = 3 * TOUCHED_WINDOW;
        for seq in 1..=total {
            cache.apply(upsert(
                seq,
                EntityPayload::Group(group(GroupId::new(), "g")),
            ));
        }
        assert!(
            cache.touched.len() <= 2 * TOUCHED_WINDOW as usize + 1,
            "{} entries",
            cache.touched.len()
        );
    }
}
