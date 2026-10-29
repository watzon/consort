//! The client API: requests from a client to the hub, the feed, and
//! snapshots.
//!
//! Every client request is in [`ClientRequest`]. Each body type implements
//! [`crate::frame::Request`], so a client can send it and read its typed
//! response:
//!
//! ```
//! use consort_proto::client_api::{TaskResolve, TaskResult};
//! use consort_proto::frame::Request;
//!
//! fn response_type<R: Request>() -> &'static str { std::any::type_name::<R::Response>() }
//! assert!(response_type::<TaskResolve>().ends_with("TaskResult"));
//! ```
//!
//! After `sync.subscribe`, the hub sends `feed` events ([`HubToClient`]).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::auth::{AuthHttpToken, ClientHello, ClientHelloResult, HttpToken, PairingCode};
use crate::frame::{Empty, message_family};
use crate::ids::{
    ApprovalId, ChannelId, ClientId, DecisionId, GroupId, ItemId, MessageId, RepoId, RoutineId,
    SessionId, TaskId, WorkerId,
};
use crate::model::{
    Approval, ApprovalMode, BlobRef, Channel, ChannelFile, Client, Decision, DecisionAnswer,
    ExpectedHash, Group, Isolation, MemoryNote, MemoryScope, Message, ModelSelection,
    PlacementRequest, PushProvider, ReadMarker, Repo, Routine, RoutineAction, RoutineRun,
    RoutineSchedule, SessionItem, Sha256Hash, Task, Worker, memory_note_entity_id,
};
use crate::time::Timestamp;
use crate::tools::{FilesListResult, MemoryListResult, RoutineResult, SearchResult};
use crate::worker_link::ItemDetailResult;

/// How long the hub keeps feed entries, in milliseconds (30 days).
pub const FEED_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
/// The hub merges updates to one session item within this many milliseconds
/// into one feed entry.
pub const FEED_MERGE_WINDOW_MS: i64 = 250;
/// The number of newest timeline messages in a channel snapshot.
pub const SNAPSHOT_MESSAGE_LIMIT: usize = 50;

/// Reads `null` as `Some(None)` and a missing field as `None`, for patch
/// fields that can be cleared.
mod patch {
    use serde::{Deserialize, Deserializer};

    pub(super) fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

// Sync -------------------------------------------------------------------

/// The scope of a snapshot: `"all"` or a channel ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapshotScope {
    /// Everything that the sidebar and the Tasks panel need.
    All,
    /// One channel.
    Channel(ChannelId),
}

impl fmt::Display for SnapshotScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => f.write_str("all"),
            Self::Channel(id) => id.fmt(f),
        }
    }
}

impl FromStr for SnapshotScope {
    type Err = crate::ids::IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "all" {
            Ok(Self::All)
        } else {
            s.parse().map(Self::Channel)
        }
    }
}

impl Serialize for SnapshotScope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SnapshotScope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// `sync.snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSnapshot {
    /// `all` or a channel ID.
    pub scope: SnapshotScope,
}

/// The response to `sync.snapshot`.
///
/// For `all`: every group, channel, repo, worker and read marker, every task
/// that is not archived, the pending approvals and decisions, and every
/// routine. No messages or files.
///
/// For a channel: that channel, its newest [`SNAPSHOT_MESSAGE_LIMIT`]
/// timeline messages, its tasks that are not archived, its pending approvals
/// and decisions, its routines and its channel files.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// The feed sequence number at the time of the snapshot. Subscribe with
    /// `after_seq` set to it.
    pub seq: u64,
    /// Groups.
    pub groups: Vec<Group>,
    /// Channels.
    pub channels: Vec<Channel>,
    /// Repos.
    pub repos: Vec<Repo>,
    /// Workers.
    pub workers: Vec<Worker>,
    /// Read markers.
    pub read_markers: Vec<ReadMarker>,
    /// Messages, oldest first.
    pub messages: Vec<Message>,
    /// Tasks.
    pub tasks: Vec<Task>,
    /// Approvals.
    pub approvals: Vec<Approval>,
    /// Decisions.
    pub decisions: Vec<Decision>,
    /// Routines.
    pub routines: Vec<Routine>,
    /// Channel files.
    pub channel_files: Vec<ChannelFile>,
}

/// `sync.subscribe`. The hub answers `ok`, then sends every feed entry after
/// `after_seq`, then new entries as they happen. If `after_seq` is older than
/// the oldest kept entry, the hub answers `resync_required`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSubscribe {
    /// The last feed `seq` that the client has.
    pub after_seq: u64,
    /// Only entries for these channels, plus entries with no channel. Empty
    /// means every channel.
    #[serde(default)]
    pub channel_ids: Option<Vec<ChannelId>>,
}

/// The kind of entity in a feed entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// [`Channel`].
    Channel,
    /// [`Group`].
    Group,
    /// [`Repo`].
    Repo,
    /// [`Message`].
    Message,
    /// [`ReadMarker`].
    ReadMarker,
    /// [`Task`].
    Task,
    /// [`SessionItem`].
    SessionItem,
    /// [`Approval`].
    Approval,
    /// [`Decision`].
    Decision,
    /// [`MemoryNote`].
    MemoryNote,
    /// [`ChannelFile`].
    ChannelFile,
    /// [`Routine`].
    Routine,
    /// [`Worker`].
    Worker,
    /// [`Client`].
    Client,
}

/// The full client view of an entity after a write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "entity", content = "payload", rename_all = "snake_case")]
pub enum EntityPayload {
    /// A channel.
    Channel(Channel),
    /// A group.
    Group(Group),
    /// A repo.
    Repo(Repo),
    /// A message.
    Message(Message),
    /// A read marker.
    ReadMarker(ReadMarker),
    /// A task.
    Task(Task),
    /// A session item. The client finds its task through the session.
    SessionItem(SessionItem),
    /// An approval.
    Approval(Approval),
    /// A decision.
    Decision(Decision),
    /// A memory note.
    MemoryNote(MemoryNote),
    /// A channel file.
    ChannelFile(ChannelFile),
    /// A routine.
    Routine(Routine),
    /// A worker.
    Worker(Worker),
    /// A client.
    Client(Client),
}

impl EntityPayload {
    /// The entity kind.
    pub fn kind(&self) -> EntityKind {
        match self {
            Self::Channel(_) => EntityKind::Channel,
            Self::Group(_) => EntityKind::Group,
            Self::Repo(_) => EntityKind::Repo,
            Self::Message(_) => EntityKind::Message,
            Self::ReadMarker(_) => EntityKind::ReadMarker,
            Self::Task(_) => EntityKind::Task,
            Self::SessionItem(_) => EntityKind::SessionItem,
            Self::Approval(_) => EntityKind::Approval,
            Self::Decision(_) => EntityKind::Decision,
            Self::MemoryNote(_) => EntityKind::MemoryNote,
            Self::ChannelFile(_) => EntityKind::ChannelFile,
            Self::Routine(_) => EntityKind::Routine,
            Self::Worker(_) => EntityKind::Worker,
            Self::Client(_) => EntityKind::Client,
        }
    }

    /// The feed `entity_id`. Most entities use their ID. A memory note uses
    /// `<scope>:<path>`, a channel file `<channel-id>:<path>`, a read marker
    /// `<channel-id>` or `<channel-id>:<thread-root-id>`, and a session item
    /// `<session-id>:<item-id>`.
    pub fn entity_id(&self) -> String {
        match self {
            Self::Channel(e) => e.id.to_string(),
            Self::Group(e) => e.id.to_string(),
            Self::Repo(e) => e.id.to_string(),
            Self::Message(e) => e.id.to_string(),
            Self::ReadMarker(e) => match e.thread_root_id {
                Some(root) => format!("{}:{root}", e.channel_id),
                None => e.channel_id.to_string(),
            },
            Self::Task(e) => e.id.to_string(),
            Self::SessionItem(e) => format!("{}:{}", e.session_id, e.id),
            Self::Approval(e) => e.id.to_string(),
            Self::Decision(e) => e.id.to_string(),
            Self::MemoryNote(e) => memory_note_entity_id(e.scope, &e.path),
            Self::ChannelFile(e) => e.entity_id(),
            Self::Routine(e) => e.id.to_string(),
            Self::Worker(e) => e.id.to_string(),
            Self::Client(e) => e.id.to_string(),
        }
    }

    /// The channel of the entity, when it has one. Session items carry no
    /// channel, so the hub sets the feed entry's `channel_id` from the
    /// session.
    pub fn channel_id(&self) -> Option<ChannelId> {
        match self {
            Self::Channel(e) => Some(e.id),
            Self::Message(e) => Some(e.channel_id),
            Self::ReadMarker(e) => Some(e.channel_id),
            Self::Task(e) => Some(e.channel_id),
            Self::Approval(e) => Some(e.channel_id),
            Self::Decision(e) => Some(e.channel_id),
            Self::MemoryNote(e) => match e.scope {
                MemoryScope::Channel(id) => Some(id),
                MemoryScope::Global => None,
            },
            Self::ChannelFile(e) => Some(e.channel_id),
            Self::Routine(e) => Some(e.channel_id),
            Self::Group(_)
            | Self::Repo(_)
            | Self::SessionItem(_)
            | Self::Worker(_)
            | Self::Client(_) => None,
        }
    }
}

/// What a feed entry did.
// A feed entry lives for one frame or one cache write, so a large variant
// costs little, and a box would make every match harder to read.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum FeedChange {
    /// The entity was created or changed. The payload is its full client view.
    Upsert(EntityPayload),
    /// The entity was deleted. The wire payload is `null`.
    Delete(EntityKind),
}

impl FeedChange {
    /// The entity kind.
    pub fn kind(&self) -> EntityKind {
        match self {
            Self::Upsert(payload) => payload.kind(),
            Self::Delete(kind) => *kind,
        }
    }
}

/// The `op` of a feed entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedOp {
    /// Created or changed.
    Upsert,
    /// Deleted.
    Delete,
}

/// One write on the hub, as clients see it. The body of a `feed` event.
///
/// On the wire: `{seq, at, channel_id, entity, entity_id, op, payload}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawFeedEntry", into = "RawFeedEntry")]
pub struct FeedEntry {
    /// Increases by one for each entry, across the whole hub.
    pub seq: u64,
    /// When the write happened.
    pub at: Timestamp,
    /// The channel, so a client can filter.
    pub channel_id: Option<ChannelId>,
    /// The entity's feed ID. See [`EntityPayload::entity_id`].
    pub entity_id: String,
    /// What changed.
    pub change: FeedChange,
}

impl FeedEntry {
    /// An upsert entry. The entity ID and channel come from the payload.
    pub fn upsert(seq: u64, at: Timestamp, payload: EntityPayload) -> Self {
        Self {
            seq,
            at,
            channel_id: payload.channel_id(),
            entity_id: payload.entity_id(),
            change: FeedChange::Upsert(payload),
        }
    }

    /// A delete entry.
    pub fn delete(
        seq: u64,
        at: Timestamp,
        kind: EntityKind,
        entity_id: impl Into<String>,
        channel_id: Option<ChannelId>,
    ) -> Self {
        Self {
            seq,
            at,
            channel_id,
            entity_id: entity_id.into(),
            change: FeedChange::Delete(kind),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct RawFeedEntry {
    seq: u64,
    at: Timestamp,
    #[serde(default)]
    channel_id: Option<ChannelId>,
    entity: EntityKind,
    entity_id: String,
    op: FeedOp,
    #[serde(default)]
    payload: Value,
}

/// The error when a feed payload does not match its entity kind.
#[derive(Debug, thiserror::Error)]
#[error("feed entry {seq} has an invalid {entity:?} payload: {source}")]
pub struct FeedPayloadError {
    seq: u64,
    entity: EntityKind,
    #[source]
    source: serde_json::Error,
}

impl TryFrom<RawFeedEntry> for FeedEntry {
    type Error = FeedPayloadError;

    fn try_from(raw: RawFeedEntry) -> Result<Self, Self::Error> {
        let change = match raw.op {
            FeedOp::Delete => FeedChange::Delete(raw.entity),
            FeedOp::Upsert => {
                let tagged = serde_json::json!({ "entity": raw.entity, "payload": raw.payload });
                let payload =
                    serde_json::from_value(tagged).map_err(|source| FeedPayloadError {
                        seq: raw.seq,
                        entity: raw.entity,
                        source,
                    })?;
                FeedChange::Upsert(payload)
            }
        };
        Ok(Self {
            seq: raw.seq,
            at: raw.at,
            channel_id: raw.channel_id,
            entity_id: raw.entity_id,
            change,
        })
    }
}

impl From<FeedEntry> for RawFeedEntry {
    fn from(entry: FeedEntry) -> Self {
        let (entity, op, payload) = match entry.change {
            FeedChange::Delete(kind) => (kind, FeedOp::Delete, Value::Null),
            FeedChange::Upsert(payload) => {
                let kind = payload.kind();
                // Entity payloads have only string map keys, so this cannot
                // fail. A failure would send `null`, which a client reads as
                // an invalid entry and reloads its snapshot.
                let value = serde_json::to_value(&payload)
                    .ok()
                    .and_then(|mut v| v.get_mut("payload").map(Value::take))
                    .unwrap_or(Value::Null);
                (kind, FeedOp::Upsert, value)
            }
        };
        Self {
            seq: entry.seq,
            at: entry.at,
            channel_id: entry.channel_id,
            entity,
            entity_id: entry.entity_id,
            op,
            payload,
        }
    }
}

// Channels, groups and repos ----------------------------------------------

/// `channel.create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelCreate {
    /// The name.
    pub name: String,
    /// The one-line purpose.
    #[serde(default)]
    pub purpose: String,
    /// The instructions, in markdown.
    #[serde(default)]
    pub instructions: String,
    /// The icon name.
    #[serde(default)]
    pub icon: Option<String>,
    /// The color role name.
    #[serde(default)]
    pub color: Option<String>,
    /// The group.
    #[serde(default)]
    pub group_id: Option<GroupId>,
    /// The coordinator's model selection.
    #[serde(default)]
    pub coordinator_selection: Option<ModelSelection>,
    /// The default model selection for tasks.
    #[serde(default)]
    pub task_selection: Option<ModelSelection>,
    /// The attached repos.
    #[serde(default)]
    pub repo_ids: Vec<RepoId>,
    /// The default isolation. Empty means `worktree`.
    #[serde(default)]
    pub default_isolation: Option<Isolation>,
    /// The default approval mode. Empty means `ask`.
    #[serde(default)]
    pub default_approval_mode: Option<ApprovalMode>,
    /// Whether the channel has its own memory. Empty means true.
    #[serde(default)]
    pub memory_enabled: Option<bool>,
}

/// `channel.update`. Fields that are left out do not change. For the fields
/// that can be cleared, `null` clears them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelUpdate {
    /// The channel.
    pub channel_id: ChannelId,
    /// The new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The new purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// The new instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// The new icon. `null` clears it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "patch::deserialize"
    )]
    pub icon: Option<Option<String>>,
    /// The new color. `null` clears it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "patch::deserialize"
    )]
    pub color: Option<Option<String>>,
    /// The new coordinator selection. `null` falls back to Everything's.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "patch::deserialize"
    )]
    pub coordinator_selection: Option<Option<ModelSelection>>,
    /// The new task selection. `null` clears it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "patch::deserialize"
    )]
    pub task_selection: Option<Option<ModelSelection>>,
    /// The new preferred workers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_workers: Option<Vec<WorkerId>>,
    /// The new attached repos, in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_ids: Option<Vec<RepoId>>,
    /// The new default isolation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_isolation: Option<Isolation>,
    /// The new default approval mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_approval_mode: Option<ApprovalMode>,
    /// Turn channel memory on or off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_enabled: Option<bool>,
}

impl ChannelUpdate {
    /// An update of `channel_id` that changes nothing. Set the fields to
    /// change.
    pub fn new(channel_id: ChannelId) -> Self {
        Self {
            channel_id,
            name: None,
            purpose: None,
            instructions: None,
            icon: None,
            color: None,
            coordinator_selection: None,
            task_selection: None,
            preferred_workers: None,
            repo_ids: None,
            default_isolation: None,
            default_approval_mode: None,
            memory_enabled: None,
        }
    }
}

/// The response to requests that return one channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelResult {
    /// The channel after the change.
    pub channel: Channel,
}

/// `channel.archive`. The Everything channel cannot be archived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelArchive {
    /// The channel.
    pub channel_id: ChannelId,
    /// False to unarchive. The default is true.
    #[serde(default = "default_true")]
    pub archived: bool,
}

fn default_true() -> bool {
    true
}

/// `channel.delete`. The Everything channel cannot be deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelDelete {
    /// The channel.
    pub channel_id: ChannelId,
}

/// `channel.move`: put a channel in a group at a position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelMove {
    /// The channel.
    pub channel_id: ChannelId,
    /// The new group. Empty means no group.
    pub group_id: Option<GroupId>,
    /// The new position in the group.
    pub position: i64,
}

/// `group.create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupCreate {
    /// The name.
    pub name: String,
}

/// `group.update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupUpdate {
    /// The group.
    pub group_id: GroupId,
    /// The new name.
    #[serde(default)]
    pub name: Option<String>,
    /// Collapse or expand.
    #[serde(default)]
    pub collapsed: Option<bool>,
}

/// The response to requests that return one group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupResult {
    /// The group after the change.
    pub group: Group,
}

/// `group.delete`. Its channels move out of the group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupDelete {
    /// The group.
    pub group_id: GroupId,
}

/// `group.reorder`: set the order of every group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupReorder {
    /// Every group ID, in the new order.
    pub group_ids: Vec<GroupId>,
}

/// `repo.create`: add a repo so that channels can attach it. The hub
/// normalizes the URL and returns the existing repo when it already has one
/// with that URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoCreate {
    /// The git remote URL.
    pub url: String,
    /// The name. Empty means the last part of the URL.
    #[serde(default)]
    pub name: Option<String>,
    /// The default branch. Empty means `main`.
    #[serde(default)]
    pub default_branch: Option<String>,
}

/// The response to `repo.create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoResult {
    /// The repo.
    pub repo: Repo,
}

// Messages ---------------------------------------------------------------

/// `message.send`. A `client_message_id` that the hub already stored returns
/// the stored message, so a client can retry without a duplicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSend {
    /// The channel.
    pub channel_id: ChannelId,
    /// The idempotency key. Unique per client.
    pub client_message_id: String,
    /// The text, in markdown.
    pub body: String,
    /// Uploaded blobs to attach.
    #[serde(default)]
    pub attachments: Vec<BlobRef>,
    /// The thread root, for a reply.
    #[serde(default)]
    pub thread_root_id: Option<MessageId>,
    /// Also show the reply in the timeline.
    #[serde(default)]
    pub in_timeline: bool,
}

/// The response to requests that return one message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageResult {
    /// The message.
    pub message: Message,
}

/// `message.list`: a page of a timeline or a thread, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageList {
    /// The channel.
    pub channel_id: ChannelId,
    /// The thread root. Empty means the timeline.
    #[serde(default)]
    pub thread_root_id: Option<MessageId>,
    /// Only messages older than this one.
    #[serde(default)]
    pub before: Option<MessageId>,
    /// The most messages. The default is 50 and the limit is 200.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// The response to `message.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageListResult {
    /// The messages, newest first.
    pub messages: Vec<Message>,
    /// Whether older messages exist.
    pub has_more: bool,
}

/// `message.edit`. Only the user's own messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEdit {
    /// The message.
    pub message_id: MessageId,
    /// The new text.
    pub body: String,
}

/// `message.delete`. Only the user's own messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDelete {
    /// The message.
    pub message_id: MessageId,
}

/// `message.react`: add or remove a reaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageReact {
    /// The message.
    pub message_id: MessageId,
    /// The emoji.
    pub emoji: String,
    /// True to remove the user's reaction.
    #[serde(default)]
    pub remove: bool,
}

/// `message.search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSearch {
    /// The words to find.
    pub query: String,
    /// Search only this channel.
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    /// The most results.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `read.mark`: move a read marker forward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadMark {
    /// The channel.
    pub channel_id: ChannelId,
    /// The thread root. Empty means the timeline.
    #[serde(default)]
    pub thread_root_id: Option<MessageId>,
    /// The newest message that the user has read. The hub ignores a marker
    /// that is older than the stored one.
    pub last_read_message_id: MessageId,
}

// Tasks ------------------------------------------------------------------

/// `task.create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCreate {
    /// The channel.
    pub channel_id: ChannelId,
    /// The title.
    pub title: String,
    /// The brief, in markdown.
    pub brief: String,
    /// The model selection. Empty means the channel's `task_selection`.
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
    /// Where the task may run.
    #[serde(default)]
    pub placement: PlacementRequest,
    /// The approval mode. Empty means the channel's default.
    #[serde(default)]
    pub approval_mode: Option<ApprovalMode>,
    /// Start later, at this time.
    #[serde(default)]
    pub start_at: Option<Timestamp>,
}

/// `task.update`. Fields that are left out do not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskUpdate {
    /// The task.
    pub task_id: TaskId,
    /// The new title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The new brief. Only for a task that has not started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    /// The new model selection. `null` means the channel's default.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "patch::deserialize"
    )]
    pub model_selection: Option<Option<ModelSelection>>,
    /// The new placement request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<PlacementRequest>,
    /// The new approval mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<ApprovalMode>,
    /// The new start time. `null` starts a Scheduled task now.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "patch::deserialize"
    )]
    pub start_at: Option<Option<Timestamp>>,
}

/// The response to requests that return one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResult {
    /// The task after the change.
    pub task: Task,
}

/// `task.resolve`: Ready for review to Resolved. Only from `review`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResolve {
    /// The task.
    pub task_id: TaskId,
}

/// `task.reopen`: **Not done**, Resolved to Ready for review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskReopen {
    /// The task.
    pub task_id: TaskId,
}

/// `task.retry`: Failed to Up next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRetry {
    /// The task.
    pub task_id: TaskId,
}

/// `task.interrupt`: **Stop** the running turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskInterrupt {
    /// The task.
    pub task_id: TaskId,
}

/// `task.archive`: hide or show a task in any status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskArchive {
    /// The task.
    pub task_id: TaskId,
    /// False to unarchive. The default is true.
    #[serde(default = "default_true")]
    pub archived: bool,
}

/// `task.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDelete {
    /// The task.
    pub task_id: TaskId,
}

/// `task.items`: a page of a task's session items, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItems {
    /// The task.
    pub task_id: TaskId,
    /// Only this session. Empty means every session of the task.
    #[serde(default)]
    pub session_id: Option<SessionId>,
    /// Only items after this one.
    #[serde(default)]
    pub after_item_id: Option<ItemId>,
    /// The most items. The default is 200.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// The response to `task.items`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskItemsResult {
    /// The items, oldest first.
    pub items: Vec<SessionItem>,
    /// Whether more items exist.
    pub has_more: bool,
}

/// `task.item_detail`: the full output or diff of an item. The hub asks the
/// worker with `item.detail`, so this fails with `worker_offline` when the
/// worker is offline. Download the blob with the HTTP token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItemDetail {
    /// The session of the item.
    pub session_id: SessionId,
    /// The item.
    pub item_id: ItemId,
}

// Waiting on the user ----------------------------------------------------

/// `approval.answer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalAnswer {
    /// The approval.
    pub approval_id: ApprovalId,
    /// The chosen option's ID.
    pub option_id: String,
}

/// The response to `approval.answer`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalResult {
    /// The approval after the answer.
    pub approval: Approval,
}

/// `decision.answer`: `{decision_id, option_index, text}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionAnswerRequest {
    /// The decision.
    pub decision_id: DecisionId,
    /// The answer.
    #[serde(flatten)]
    pub answer: DecisionAnswer,
}

/// The response to `decision.answer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionResult {
    /// The decision after the answer.
    pub decision: Decision,
}

// Memory -----------------------------------------------------------------

/// `memory.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryList {
    /// Only this scope. Empty means every scope.
    #[serde(default)]
    pub scope: Option<MemoryScope>,
}

/// `memory.read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRead {
    /// The scope.
    pub scope: MemoryScope,
    /// The path.
    pub path: String,
}

/// The response to requests that return one memory note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryNoteResult {
    /// The note.
    pub note: MemoryNote,
}

/// `memory.write`. Fails with `stale_write` when the stored hash differs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryWrite {
    /// The scope.
    pub scope: MemoryScope,
    /// The path.
    pub path: String,
    /// The full new content.
    pub content: String,
    /// The hash that the client read, or `not_exists`.
    pub expected_sha256: ExpectedHash,
}

/// `memory.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryDelete {
    /// The scope.
    pub scope: MemoryScope,
    /// The path.
    pub path: String,
    /// The hash that the client read.
    pub expected_sha256: Sha256Hash,
}

/// `memory.history`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryHistory {
    /// The scope.
    pub scope: MemoryScope,
    /// The path.
    pub path: String,
}

/// The response to `memory.history`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryHistoryResult {
    /// Every version, newest first.
    pub versions: Vec<MemoryNote>,
}

// Channel files ----------------------------------------------------------

/// `files.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesList {
    /// The channel.
    pub channel_id: ChannelId,
    /// Only paths that start with this text.
    #[serde(default)]
    pub prefix: Option<String>,
}

/// `files.put`: add or replace a channel file after a blob upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesPut {
    /// The channel.
    pub channel_id: ChannelId,
    /// The path.
    pub path: String,
    /// The uploaded blob.
    pub blob_sha256: Sha256Hash,
    /// The size in bytes.
    pub size: u64,
    /// The media type.
    pub mime: String,
}

/// The response to requests that return one channel file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelFileResult {
    /// The file after the change.
    pub file: ChannelFile,
}

/// `files.move`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesMove {
    /// The channel.
    pub channel_id: ChannelId,
    /// The current path.
    pub from_path: String,
    /// The new path.
    pub to_path: String,
}

/// `files.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesDelete {
    /// The channel.
    pub channel_id: ChannelId,
    /// The path.
    pub path: String,
}

// Routines ---------------------------------------------------------------

/// `routine.create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineCreate {
    /// The channel.
    pub channel_id: ChannelId,
    /// The name.
    pub name: String,
    /// When it runs.
    pub schedule: RoutineSchedule,
    /// The prompt.
    pub prompt: String,
    /// What it does.
    pub action: RoutineAction,
    /// Whether it runs. The default is true.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// `routine.update`. Fields that are left out do not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineUpdate {
    /// The routine.
    pub routine_id: RoutineId,
    /// The new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The new schedule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<RoutineSchedule>,
    /// The new prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The new action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<RoutineAction>,
    /// Turn it on or off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// `routine.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineDelete {
    /// The routine.
    pub routine_id: RoutineId,
}

/// `routine.run_now`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineRunNow {
    /// The routine.
    pub routine_id: RoutineId,
}

/// `routine.runs`: the newest runs of a routine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineRuns {
    /// The routine.
    pub routine_id: RoutineId,
    /// The most runs. The default is 20.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// The response to `routine.runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineRunsResult {
    /// The runs, newest first.
    pub runs: Vec<RoutineRun>,
}

// Workers and clients ----------------------------------------------------

/// `worker.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerList {}

/// The response to `worker.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerListResult {
    /// The workers that are not revoked.
    pub workers: Vec<Worker>,
}

/// `worker.pairing_code`: **Add worker**.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerPairingCode {}

/// The response to `worker.pairing_code`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingCodeResult {
    /// The single-use code.
    pub code: PairingCode,
    /// When the code stops working, 10 minutes after it was made.
    pub expires_at: Timestamp,
}

/// `worker.rename`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRename {
    /// The worker.
    pub worker_id: WorkerId,
    /// The new name.
    pub name: String,
}

/// `worker.set_labels`: replace the user's labels on a worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSetLabels {
    /// The worker.
    pub worker_id: WorkerId,
    /// The labels.
    pub labels: Vec<String>,
}

/// `worker.set_paused`: **Pause** or resume a worker. Placement skips a
/// paused worker. Running sessions continue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSetPaused {
    /// The worker.
    pub worker_id: WorkerId,
    /// True to pause.
    pub paused: bool,
}

/// The response to requests that return one worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerResult {
    /// The worker after the change.
    pub worker: Worker,
}

/// `worker.revoke`: **Remove** a worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRevoke {
    /// The worker.
    pub worker_id: WorkerId,
}

/// `worker.refresh_inventory`: the hub sends `inventory.refresh`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRefreshInventory {
    /// The worker.
    pub worker_id: WorkerId,
}

/// `client.pairing_code`: the data for the pairing QR code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientPairingCode {}

/// The response to `client.pairing_code`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientPairingCodeResult {
    /// The hub's public URL.
    pub hub_url: String,
    /// The single-use code.
    pub code: PairingCode,
    /// When the code stops working.
    pub expires_at: Timestamp,
}

/// `client.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientList {}

/// The response to `client.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientListResult {
    /// The clients that are not revoked.
    pub clients: Vec<Client>,
}

/// `client.revoke`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRevoke {
    /// The client.
    pub client_id: ClientId,
}

/// `client.push_register`: set this client's push token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientPushRegister {
    /// The push service.
    pub provider: PushProvider,
    /// The device token.
    pub token: String,
}

message_family! {
    /// Every request from a client to the hub after authentication.
    pub enum ClientRequest {
        /// The first message after authentication.
        Hello("hello", ClientHello => ClientHelloResult),
        /// Load a snapshot.
        SyncSnapshot("sync.snapshot", SyncSnapshot => Snapshot),
        /// Follow the feed.
        SyncSubscribe("sync.subscribe", SyncSubscribe => Empty),
        /// Create a channel.
        ChannelCreate("channel.create", ChannelCreate => ChannelResult),
        /// Change a channel.
        ChannelUpdate("channel.update", ChannelUpdate => ChannelResult),
        /// Archive or unarchive a channel.
        ChannelArchive("channel.archive", ChannelArchive => ChannelResult),
        /// Delete a channel.
        ChannelDelete("channel.delete", ChannelDelete => Empty),
        /// Move a channel.
        ChannelMove("channel.move", ChannelMove => ChannelResult),
        /// Create a group.
        GroupCreate("group.create", GroupCreate => GroupResult),
        /// Change a group.
        GroupUpdate("group.update", GroupUpdate => GroupResult),
        /// Delete a group.
        GroupDelete("group.delete", GroupDelete => Empty),
        /// Order the groups.
        GroupReorder("group.reorder", GroupReorder => Empty),
        /// Add a repo.
        RepoCreate("repo.create", RepoCreate => RepoResult),
        /// Send a message.
        MessageSend("message.send", MessageSend => MessageResult),
        /// Load a page of messages.
        MessageList("message.list", MessageList => MessageListResult),
        /// Edit a message.
        MessageEdit("message.edit", MessageEdit => MessageResult),
        /// Delete a message.
        MessageDelete("message.delete", MessageDelete => Empty),
        /// React to a message.
        MessageReact("message.react", MessageReact => Empty),
        /// Search.
        MessageSearch("message.search", MessageSearch => SearchResult),
        /// Move a read marker.
        ReadMark("read.mark", ReadMark => Empty),
        /// Create a task.
        TaskCreate("task.create", TaskCreate => TaskResult),
        /// Change a task.
        TaskUpdate("task.update", TaskUpdate => TaskResult),
        /// Resolve a task.
        TaskResolve("task.resolve", TaskResolve => TaskResult),
        /// Not done.
        TaskReopen("task.reopen", TaskReopen => TaskResult),
        /// Retry a failed task.
        TaskRetry("task.retry", TaskRetry => TaskResult),
        /// Stop a task's turn.
        TaskInterrupt("task.interrupt", TaskInterrupt => Empty),
        /// Archive a task.
        TaskArchive("task.archive", TaskArchive => TaskResult),
        /// Delete a task.
        TaskDelete("task.delete", TaskDelete => Empty),
        /// Load a task's session items.
        TaskItems("task.items", TaskItems => TaskItemsResult),
        /// Get an item's full data.
        TaskItemDetail("task.item_detail", TaskItemDetail => ItemDetailResult),
        /// Answer an approval.
        ApprovalAnswer("approval.answer", ApprovalAnswer => ApprovalResult),
        /// Answer a decision.
        DecisionAnswer("decision.answer", DecisionAnswerRequest => DecisionResult),
        /// List memory notes.
        MemoryList("memory.list", MemoryList => MemoryListResult),
        /// Read a memory note.
        MemoryRead("memory.read", MemoryRead => MemoryNoteResult),
        /// Write a memory note.
        MemoryWrite("memory.write", MemoryWrite => MemoryNoteResult),
        /// Delete a memory note.
        MemoryDelete("memory.delete", MemoryDelete => Empty),
        /// A memory note's versions.
        MemoryHistory("memory.history", MemoryHistory => MemoryHistoryResult),
        /// List channel files.
        FilesList("files.list", FilesList => FilesListResult),
        /// Add a channel file.
        FilesPut("files.put", FilesPut => ChannelFileResult),
        /// Move a channel file.
        FilesMove("files.move", FilesMove => ChannelFileResult),
        /// Delete a channel file.
        FilesDelete("files.delete", FilesDelete => Empty),
        /// Create a routine.
        RoutineCreate("routine.create", RoutineCreate => RoutineResult),
        /// Change a routine.
        RoutineUpdate("routine.update", RoutineUpdate => RoutineResult),
        /// Delete a routine.
        RoutineDelete("routine.delete", RoutineDelete => Empty),
        /// Run a routine now.
        RoutineRunNow("routine.run_now", RoutineRunNow => Empty),
        /// A routine's runs.
        RoutineRuns("routine.runs", RoutineRuns => RoutineRunsResult),
        /// List workers.
        WorkerList("worker.list", WorkerList => WorkerListResult),
        /// Make a worker pairing code.
        WorkerPairingCode("worker.pairing_code", WorkerPairingCode => PairingCodeResult),
        /// Rename a worker.
        WorkerRename("worker.rename", WorkerRename => WorkerResult),
        /// Set a worker's labels.
        WorkerSetLabels("worker.set_labels", WorkerSetLabels => WorkerResult),
        /// Pause or resume a worker.
        WorkerSetPaused("worker.set_paused", WorkerSetPaused => WorkerResult),
        /// Remove a worker.
        WorkerRevoke("worker.revoke", WorkerRevoke => Empty),
        /// Ask a worker for a new inventory.
        WorkerRefreshInventory("worker.refresh_inventory", WorkerRefreshInventory => Empty),
        /// Make a client pairing code.
        ClientPairingCode("client.pairing_code", ClientPairingCode => ClientPairingCodeResult),
        /// List clients.
        ClientList("client.list", ClientList => ClientListResult),
        /// Revoke a client.
        ClientRevoke("client.revoke", ClientRevoke => Empty),
        /// Set the push token.
        ClientPushRegister("client.push_register", ClientPushRegister => Empty),
        /// Get a bearer token for the blob endpoints.
        AuthHttpToken("auth.http_token", AuthHttpToken => HttpToken),
    }
}

message_family! {
    /// Events from the hub to a client after authentication.
    pub enum HubToClient {
        /// One feed entry.
        Feed("feed", FeedEntry),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Frame, FrameId, MessageFamily};
    use crate::model::{
        ApprovalMode, Author, ChannelKind, Isolation, MemoryAuthor, MessageKind, TaskCreator,
    };
    use crate::status::TaskStatus;
    use serde_json::json;

    fn chn() -> ChannelId {
        "chn_00000000-0000-0000-0000-000000000001".parse().unwrap()
    }

    fn message() -> Message {
        Message {
            id: "msg_00000000-0000-0000-0000-000000000002".parse().unwrap(),
            channel_id: chn(),
            thread_root_id: None,
            author: Author::User,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            kind: MessageKind::Text,
            body: "Explain what makes a channel special.".into(),
            attachments: vec![],
            in_timeline: false,
            client_message_id: Some("c-1".into()),
            reply_count: 0,
            last_reply_at: None,
            reactions: vec![],
            edited_at: None,
            deleted_at: None,
            created_at: Timestamp::from_millis(1_000),
        }
    }

    fn task() -> Task {
        Task {
            id: "tsk_00000000-0000-0000-0000-000000000003".parse().unwrap(),
            channel_id: chn(),
            title: "Go through open Sayso issues".into(),
            brief: "Triage them.".into(),
            status: TaskStatus::Review,
            status_reason: None,
            created_by: TaskCreator::Assistant,
            card_message_id: "msg_00000000-0000-0000-0000-000000000004".parse().unwrap(),
            model_selection: None,
            placement: PlacementRequest::default(),
            approval_mode: ApprovalMode::AutoEdits,
            start_at: None,
            current_session_id: None,
            worker_id: None,
            review_summary: Some("PR #62 is up.".into()),
            resolved_at: None,
            archived_at: None,
            created_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(2),
        }
    }

    #[test]
    fn message_send_frame_shape() {
        let send = MessageSend {
            channel_id: chn(),
            client_message_id: "c-1".into(),
            body: "Hi".into(),
            attachments: vec![],
            thread_root_id: None,
            in_timeline: false,
        };
        let frame = Frame::request(FrameId::new("01JC5"), &send).unwrap();
        let value: Value = serde_json::from_str(&frame.to_text().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({
                "v": 1, "id": "01JC5", "type": "message.send",
                "body": {
                    "channel_id": "chn_00000000-0000-0000-0000-000000000001",
                    "client_message_id": "c-1",
                    "body": "Hi",
                    "attachments": [],
                    "thread_root_id": null,
                    "in_timeline": false
                }
            })
        );
        // A minimal body from another client also reads.
        let minimal: MessageSend = serde_json::from_value(json!({
            "channel_id": "chn_00000000-0000-0000-0000-000000000001",
            "client_message_id": "c-2",
            "body": "Hello"
        }))
        .unwrap();
        assert!(minimal.attachments.is_empty());
    }

    #[test]
    fn feed_entry_shape_and_round_trip() {
        let entry = FeedEntry::upsert(
            42,
            Timestamp::from_millis(5),
            EntityPayload::Message(message()),
        );
        assert_eq!(entry.channel_id, Some(chn()));
        assert_eq!(entry.entity_id, "msg_00000000-0000-0000-0000-000000000002");
        let value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["seq"], 42);
        assert_eq!(value["at"], 5);
        assert_eq!(value["entity"], "message");
        assert_eq!(value["op"], "upsert");
        assert_eq!(
            value["payload"]["body"],
            "Explain what makes a channel special."
        );
        let mut keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "at",
                "channel_id",
                "entity",
                "entity_id",
                "op",
                "payload",
                "seq"
            ]
        );
        let back: FeedEntry = serde_json::from_value(value).unwrap();
        assert_eq!(back, entry);

        let delete = FeedEntry::delete(
            43,
            Timestamp::from_millis(6),
            EntityKind::Task,
            "tsk_00000000-0000-0000-0000-000000000003",
            Some(chn()),
        );
        let value = serde_json::to_value(&delete).unwrap();
        assert_eq!(value["op"], "delete");
        assert_eq!(value["payload"], Value::Null);
        assert_eq!(serde_json::from_value::<FeedEntry>(value).unwrap(), delete);
    }

    #[test]
    fn feed_event_frame() {
        let entry = FeedEntry::upsert(1, Timestamp::from_millis(5), EntityPayload::Task(task()));
        let frame = Frame::family_event(&HubToClient::Feed(entry.clone())).unwrap();
        let text = frame.to_text().unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["type"], "feed");
        assert!(value.get("id").is_none());
        let Frame::Event(event) = Frame::parse(&text).unwrap() else {
            panic!("expected an event");
        };
        assert_eq!(
            event.decode::<HubToClient>().unwrap(),
            HubToClient::Feed(entry)
        );
    }

    #[test]
    fn feed_rejects_mismatched_payload() {
        let bad = json!({
            "seq": 1, "at": 1, "channel_id": null, "entity": "task",
            "entity_id": "x", "op": "upsert", "payload": {"name": "not a task"}
        });
        let err = serde_json::from_value::<FeedEntry>(bad).unwrap_err();
        assert!(err.to_string().contains("Task"), "{err}");
    }

    #[test]
    fn entity_ids_and_channels() {
        let note = MemoryNote {
            scope: MemoryScope::Global,
            path: "people/kevin.md".into(),
            content: "Kevin.".into(),
            sha256: "e".repeat(64).parse().unwrap(),
            version: 1,
            updated_by: MemoryAuthor::User,
            updated_at: Timestamp::from_millis(1),
        };
        let payload = EntityPayload::MemoryNote(note);
        assert_eq!(payload.entity_id(), "global:people/kevin.md");
        assert_eq!(payload.channel_id(), None);
        let marker = EntityPayload::ReadMarker(ReadMarker {
            channel_id: chn(),
            thread_root_id: None,
            last_read_message_id: message().id,
        });
        assert_eq!(marker.entity_id(), chn().to_string());
        assert_eq!(marker.kind(), EntityKind::ReadMarker);
    }

    #[test]
    fn patch_fields_distinguish_missing_and_null() {
        let update: ChannelUpdate = serde_json::from_value(json!({
            "channel_id": "chn_00000000-0000-0000-0000-000000000001",
            "icon": null,
            "name": "Sayso"
        }))
        .unwrap();
        assert_eq!(update.icon, Some(None));
        assert_eq!(update.color, None);
        assert_eq!(update.name.as_deref(), Some("Sayso"));
        let value = serde_json::to_value(&update).unwrap();
        assert_eq!(
            value,
            json!({"channel_id": "chn_00000000-0000-0000-0000-000000000001", "name": "Sayso", "icon": null})
        );

        let start_now: TaskUpdate = serde_json::from_value(json!({
            "task_id": "tsk_00000000-0000-0000-0000-000000000003",
            "start_at": null
        }))
        .unwrap();
        assert_eq!(start_now.start_at, Some(None));
        assert_eq!(start_now.model_selection, None);
    }

    #[test]
    fn decision_answer_is_flat() {
        let req = DecisionAnswerRequest {
            decision_id: "dec_00000000-0000-0000-0000-000000000001".parse().unwrap(),
            answer: DecisionAnswer {
                option_index: Some(1),
                text: None,
            },
        };
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            json!({"decision_id": "dec_00000000-0000-0000-0000-000000000001", "option_index": 1, "text": null})
        );
        let back: DecisionAnswerRequest = serde_json::from_value(
            json!({"decision_id": "dec_00000000-0000-0000-0000-000000000001", "text": "Later"}),
        )
        .unwrap();
        assert_eq!(back.answer.text.as_deref(), Some("Later"));
    }

    #[test]
    fn snapshot_scope_and_defaults() {
        assert_eq!(
            serde_json::to_value(SnapshotScope::All).unwrap(),
            json!("all")
        );
        let scoped: SyncSnapshot =
            serde_json::from_value(json!({"scope": "chn_00000000-0000-0000-0000-000000000001"}))
                .unwrap();
        assert_eq!(scoped.scope, SnapshotScope::Channel(chn()));
        assert!(serde_json::from_value::<SyncSnapshot>(json!({"scope": "some"})).is_err());
        let snapshot: Snapshot = serde_json::from_value(json!({"seq": 9})).unwrap();
        assert_eq!(snapshot.seq, 9);
        assert!(snapshot.channels.is_empty());
    }

    fn sample_requests() -> Vec<ClientRequest> {
        let tsk = task().id;
        let rtn: RoutineId = "rtn_00000000-0000-0000-0000-000000000005".parse().unwrap();
        let wrk: WorkerId = "wrk_00000000-0000-0000-0000-000000000006".parse().unwrap();
        let grp: GroupId = "grp_00000000-0000-0000-0000-000000000007".parse().unwrap();
        let hash: Sha256Hash = "a".repeat(64).parse().unwrap();
        vec![
            ClientHello {
                client_id: "cli_00000000-0000-0000-0000-000000000008".parse().unwrap(),
                version: "0.1.0".into(),
                protocol_versions: vec![1],
            }
            .into(),
            SyncSnapshot {
                scope: SnapshotScope::All,
            }
            .into(),
            SyncSubscribe {
                after_seq: 10,
                channel_ids: Some(vec![chn()]),
            }
            .into(),
            ChannelCreate {
                name: "Sayso".into(),
                purpose: "The Sayso app".into(),
                instructions: String::new(),
                icon: None,
                color: None,
                group_id: Some(grp),
                coordinator_selection: None,
                task_selection: None,
                repo_ids: vec![],
                default_isolation: Some(Isolation::Worktree),
                default_approval_mode: None,
                memory_enabled: None,
            }
            .into(),
            ChannelUpdate {
                name: Some("New".into()),
                ..ChannelUpdate::new(chn())
            }
            .into(),
            ChannelArchive {
                channel_id: chn(),
                archived: true,
            }
            .into(),
            ChannelDelete { channel_id: chn() }.into(),
            ChannelMove {
                channel_id: chn(),
                group_id: None,
                position: 3,
            }
            .into(),
            GroupCreate {
                name: "Work".into(),
            }
            .into(),
            GroupUpdate {
                group_id: grp,
                name: None,
                collapsed: Some(true),
            }
            .into(),
            GroupDelete { group_id: grp }.into(),
            GroupReorder {
                group_ids: vec![grp],
            }
            .into(),
            RepoCreate {
                url: "git@github.com:watzon/sayso.git".into(),
                name: None,
                default_branch: None,
            }
            .into(),
            MessageSend {
                channel_id: chn(),
                client_message_id: "c-1".into(),
                body: "Hi".into(),
                attachments: vec![],
                thread_root_id: None,
                in_timeline: false,
            }
            .into(),
            MessageList {
                channel_id: chn(),
                thread_root_id: None,
                before: None,
                limit: Some(50),
            }
            .into(),
            MessageEdit {
                message_id: message().id,
                body: "Hi!".into(),
            }
            .into(),
            MessageDelete {
                message_id: message().id,
            }
            .into(),
            MessageReact {
                message_id: message().id,
                emoji: "👍".into(),
                remove: false,
            }
            .into(),
            MessageSearch {
                query: "sayso".into(),
                channel_id: None,
                limit: None,
            }
            .into(),
            ReadMark {
                channel_id: chn(),
                thread_root_id: None,
                last_read_message_id: message().id,
            }
            .into(),
            TaskCreate {
                channel_id: chn(),
                title: "Fix".into(),
                brief: "Fix it.".into(),
                model_selection: None,
                placement: PlacementRequest::default(),
                approval_mode: None,
                start_at: Some(Timestamp::from_millis(9)),
            }
            .into(),
            TaskUpdate {
                task_id: tsk,
                title: Some("Fix it".into()),
                brief: None,
                model_selection: Some(None),
                placement: None,
                approval_mode: None,
                start_at: None,
            }
            .into(),
            TaskResolve { task_id: tsk }.into(),
            TaskReopen { task_id: tsk }.into(),
            TaskRetry { task_id: tsk }.into(),
            TaskInterrupt { task_id: tsk }.into(),
            TaskArchive {
                task_id: tsk,
                archived: false,
            }
            .into(),
            TaskDelete { task_id: tsk }.into(),
            TaskItems {
                task_id: tsk,
                session_id: None,
                after_item_id: None,
                limit: None,
            }
            .into(),
            TaskItemDetail {
                session_id: SessionId::new(),
                item_id: ItemId::new(),
            }
            .into(),
            ApprovalAnswer {
                approval_id: ApprovalId::new(),
                option_id: "allow".into(),
            }
            .into(),
            DecisionAnswerRequest {
                decision_id: DecisionId::new(),
                answer: DecisionAnswer {
                    option_index: Some(0),
                    text: None,
                },
            }
            .into(),
            MemoryList {
                scope: Some(MemoryScope::Global),
            }
            .into(),
            MemoryRead {
                scope: MemoryScope::Channel(chn()),
                path: "a.md".into(),
            }
            .into(),
            MemoryWrite {
                scope: MemoryScope::Global,
                path: "a.md".into(),
                content: "A".into(),
                expected_sha256: ExpectedHash::NotExists,
            }
            .into(),
            MemoryDelete {
                scope: MemoryScope::Global,
                path: "a.md".into(),
                expected_sha256: hash.clone(),
            }
            .into(),
            MemoryHistory {
                scope: MemoryScope::Global,
                path: "a.md".into(),
            }
            .into(),
            FilesList {
                channel_id: chn(),
                prefix: None,
            }
            .into(),
            FilesPut {
                channel_id: chn(),
                path: "notes.txt".into(),
                blob_sha256: hash,
                size: 3,
                mime: "text/plain".into(),
            }
            .into(),
            FilesMove {
                channel_id: chn(),
                from_path: "a".into(),
                to_path: "b".into(),
            }
            .into(),
            FilesDelete {
                channel_id: chn(),
                path: "b".into(),
            }
            .into(),
            RoutineCreate {
                channel_id: chn(),
                name: "Morning brief".into(),
                schedule: RoutineSchedule::Interval { seconds: 3600 },
                prompt: "Summarize.".into(),
                action: RoutineAction::CoordinatorTurn,
                enabled: true,
            }
            .into(),
            RoutineUpdate {
                routine_id: rtn,
                name: None,
                schedule: None,
                prompt: None,
                action: None,
                enabled: Some(false),
            }
            .into(),
            RoutineDelete { routine_id: rtn }.into(),
            RoutineRunNow { routine_id: rtn }.into(),
            RoutineRuns {
                routine_id: rtn,
                limit: Some(5),
            }
            .into(),
            WorkerList {}.into(),
            WorkerPairingCode {}.into(),
            WorkerRename {
                worker_id: wrk,
                name: "bigboi".into(),
            }
            .into(),
            WorkerSetLabels {
                worker_id: wrk,
                labels: vec!["gpu".into()],
            }
            .into(),
            WorkerSetPaused {
                worker_id: wrk,
                paused: true,
            }
            .into(),
            WorkerRevoke { worker_id: wrk }.into(),
            WorkerRefreshInventory { worker_id: wrk }.into(),
            ClientPairingCode {}.into(),
            ClientList {}.into(),
            ClientRevoke {
                client_id: ClientId::new(),
            }
            .into(),
            ClientPushRegister {
                provider: PushProvider::Apns,
                token: "t".into(),
            }
            .into(),
            AuthHttpToken {}.into(),
        ]
    }

    #[test]
    fn every_client_request_round_trips() {
        let requests = sample_requests();
        assert_eq!(requests.len(), ClientRequest::TYPES.len());
        let mut seen: Vec<&str> = Vec::new();
        for request in requests {
            let frame = Frame::family_request(FrameId::generate(), &request).unwrap();
            let Frame::Request(req) = Frame::parse(&frame.to_text().unwrap()).unwrap() else {
                panic!("expected a request");
            };
            assert_eq!(req.kind, request.type_name());
            assert_eq!(req.decode::<ClientRequest>().unwrap(), request);
            seen.push(request.type_name());
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), ClientRequest::TYPES.len());
    }

    #[test]
    fn protocol_request_types_are_covered() {
        let expected = [
            "sync.snapshot",
            "sync.subscribe",
            "channel.create",
            "channel.update",
            "channel.archive",
            "channel.delete",
            "channel.move",
            "group.create",
            "group.update",
            "group.delete",
            "group.reorder",
            "message.send",
            "message.edit",
            "message.delete",
            "message.react",
            "message.search",
            "read.mark",
            "task.create",
            "task.update",
            "task.resolve",
            "task.reopen",
            "task.retry",
            "task.interrupt",
            "task.archive",
            "task.delete",
            "task.items",
            "task.item_detail",
            "approval.answer",
            "decision.answer",
            "memory.list",
            "memory.read",
            "memory.write",
            "memory.delete",
            "memory.history",
            "files.list",
            "files.put",
            "files.move",
            "files.delete",
            "routine.create",
            "routine.update",
            "routine.delete",
            "routine.run_now",
            "routine.runs",
            "worker.list",
            "worker.pairing_code",
            "worker.rename",
            "worker.set_labels",
            "worker.revoke",
            "worker.refresh_inventory",
            "client.pairing_code",
            "client.list",
            "client.revoke",
            "client.push_register",
            "auth.http_token",
        ];
        for ty in expected {
            assert!(ClientRequest::TYPES.contains(&ty), "{ty} is missing");
        }
    }

    #[test]
    fn typed_responses() {
        let result = TaskResult { task: task() };
        let frame = Frame::ok(FrameId::new("r"), &result).unwrap();
        let Frame::Response(resp) = Frame::parse(&frame.to_text().unwrap()).unwrap() else {
            panic!("expected a response");
        };
        assert_eq!(resp.into_response::<TaskResolve>().unwrap(), result);

        let err = Frame::error(
            FrameId::new("r"),
            crate::error::ErrorBody::conflict("The task is Up next."),
        );
        let Frame::Response(resp) = Frame::parse(&err.to_text().unwrap()).unwrap() else {
            panic!("expected a response");
        };
        let failure = resp.into_response::<TaskResolve>().unwrap_err();
        assert!(matches!(
            failure,
            crate::frame::ResponseError::Remote(ref body) if body.code == crate::error::ErrorCode::Conflict
        ));
    }

    #[test]
    fn channel_payload_round_trips() {
        let channel = Channel {
            id: chn(),
            kind: ChannelKind::Everything,
            name: "Everything".into(),
            icon: None,
            color: None,
            group_id: None,
            position: 0,
            purpose: String::new(),
            instructions: String::new(),
            coordinator_selection: None,
            task_selection: None,
            preferred_workers: vec![],
            repo_ids: vec![],
            default_isolation: Isolation::Worktree,
            default_approval_mode: ApprovalMode::Ask,
            memory_enabled: true,
            archived_at: None,
            created_at: Timestamp::from_millis(0),
            updated_at: Timestamp::from_millis(0),
        };
        let entry = FeedEntry::upsert(
            3,
            Timestamp::from_millis(0),
            EntityPayload::Channel(channel),
        );
        let back: FeedEntry =
            serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(back, entry);
    }
}
