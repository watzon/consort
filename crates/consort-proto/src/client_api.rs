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
    AccountId, ApprovalId, ChannelId, ClientId, DecisionId, GroupId, ItemId, MessageId, RepoId,
    RoutineId, SessionId, SignInId, TaskId, WorkerId,
};
use crate::model::{
    AcpAccountSpec, AcpRegistryAgent, Approval, ApprovalMode, BlobRef, Channel, ChannelFile,
    Client, Connection, Decision, DecisionAnswer, ExpectedHash, FavoriteModel, Group, Isolation,
    MemoryNote, MemoryScope, Message, ModelSelection, PREFERENCES_ENTITY_ID, PlacementRequest,
    Preferences, PushProvider, ReadMarker, Repo, Routine, RoutineAction, RoutineRun,
    RoutineSchedule, SecretEnv, SessionItem, Sha256Hash, SignIn, SignInMethod, Skill, Task, ToolId,
    Worker, memory_note_entity_id,
};
use crate::time::Timestamp;
use crate::tools::{FilesListResult, MemoryListResult, RoutineResult, SearchResult};
use crate::worker_link::ItemDetailResult;

mod connectors;
mod skills;

pub use connectors::{
    CATALOG_PAGE_MAX, CONNECTOR_CALLS_PAGE_MAX, ChannelSetConnectionEnabled,
    ChannelSetSkillEnabled, ComposioGetStatus, ComposioRemoveKey, ComposioSetKey,
    ComposioStatusResult, ConnectAttempt, ConnectionCalls, ConnectionCallsResult,
    ConnectionConnect, ConnectionDisconnect, ConnectionReconnect, ConnectionResult,
    ConnectionUpdate, ConnectorCatalogCategories, ConnectorCatalogCategoriesResult,
    ConnectorCatalogSearch, ConnectorCatalogSearchResult, ConnectorCatalogService,
    ConnectorServiceDetail,
};
pub use skills::{
    SKILL_DIRECTORY_PAGE_MAX, SkillDirectoryAddSource, SkillDirectoryRemoveSource,
    SkillDirectorySearch, SkillDirectorySearchResult, SkillDirectorySources,
    SkillDirectorySourcesResult, SkillFileContent, SkillFiles, SkillFilesResult, SkillInstall,
    SkillInstallSource, SkillPreview, SkillPreviewResult, SkillReadFile, SkillRemove, SkillResult,
    SkillUpdate, SkillVersion,
};

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
///
/// In both scopes, a long `brief` or `review_summary` of a task is cut to a
/// short preview, and the task ID is in `tasks_cut`. Resolved tasks come
/// last: the hub leaves out the oldest ones when the snapshot would not fit
/// in one frame, and then sets `tasks_has_more`. Get one full task with
/// `task.get`.
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
    /// For a channel scope: true when the channel has older timeline
    /// messages than the ones in `messages`. A page can stop before
    /// [`SNAPSHOT_MESSAGE_LIMIT`] messages when they are large, so a short
    /// page does not mean that no older messages exist.
    pub messages_has_more: bool,
    /// Tasks.
    pub tasks: Vec<Task>,
    /// The tasks in `tasks` whose `brief` or `review_summary` is cut to a
    /// preview. Send `task.get` for the full task before you show the full
    /// text. A feed entry for a task always holds the full task.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tasks_cut: Vec<TaskId>,
    /// True when the snapshot left out older resolved tasks, so that it fits
    /// in one frame. Send `task.get` for a task that is not in `tasks`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub tasks_has_more: bool,
    /// Approvals.
    pub approvals: Vec<Approval>,
    /// Decisions.
    pub decisions: Vec<Decision>,
    /// Routines.
    pub routines: Vec<Routine>,
    /// Channel files.
    pub channel_files: Vec<ChannelFile>,
    /// The sign-ins that have not ended, for the scope `all`. A client that
    /// loads while a sign-in runs shows it from here, and then follows its
    /// feed entries. Ended sign-ins are not in the list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sign_ins: Vec<SignIn>,
    /// The user's preferences, for the scope `all`. A channel snapshot and a
    /// snapshot from an older hub leave it out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferences: Option<Preferences>,
    /// Every connection, for the scope `all`. Left out when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<Connection>,
    /// Every installed skill, for the scope `all`. Left out when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<Skill>,
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
    /// [`SignIn`].
    SignIn,
    /// [`Preferences`]. There is one record, with the ID
    /// [`PREFERENCES_ENTITY_ID`].
    Preferences,
    /// [`Connection`].
    Connection,
    /// [`Skill`].
    Skill,
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
    /// A sign-in.
    SignIn(SignIn),
    /// The user's preferences.
    Preferences(Preferences),
    /// A connection.
    Connection(Connection),
    /// An installed skill.
    Skill(Skill),
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
            Self::SignIn(_) => EntityKind::SignIn,
            Self::Preferences(_) => EntityKind::Preferences,
            Self::Connection(_) => EntityKind::Connection,
            Self::Skill(_) => EntityKind::Skill,
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
            Self::SignIn(e) => e.id.to_string(),
            Self::Preferences(_) => PREFERENCES_ENTITY_ID.to_owned(),
            Self::Connection(e) => e.id.to_string(),
            Self::Skill(e) => e.id.to_string(),
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
            | Self::Client(_)
            | Self::SignIn(_)
            | Self::Preferences(_)
            | Self::Connection(_)
            | Self::Skill(_) => None,
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
    /// The channel's tags. Work from the channel runs only on a worker with
    /// every tag.
    #[serde(default)]
    pub tags: Vec<String>,
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
    /// The new list of local MCP servers that a task that the assistant
    /// starts may use. An empty list allows none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_local_mcp_servers: Option<Vec<String>>,
    /// Turn channel memory on or off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_enabled: Option<bool>,
    /// The new tags. An empty list removes every tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
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
            allowed_local_mcp_servers: None,
            memory_enabled: None,
            tags: None,
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

/// `task.get`: one task with its full brief and review summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskGet {
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

/// `worker.set_tags`: replace the user tags on a worker. The hub checks
/// each tag (see [`crate::model::validate_tag`]) and answers `invalid_tag`
/// for a bad one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerSetTags {
    /// The worker.
    pub worker_id: WorkerId,
    /// The user tags.
    #[serde(alias = "labels")]
    pub tags: Vec<String>,
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

/// `worker.tools_check`: the worker checks for tool updates now. The result
/// comes in the next inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerToolsCheck {
    /// The worker.
    pub worker_id: WorkerId,
}

/// `worker.tool_install`: install or update one managed tool on a worker.
/// The progress comes in the inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerToolInstall {
    /// The worker.
    pub worker_id: WorkerId,
    /// The tool.
    pub tool: ToolId,
}

// Accounts and sign-in ---------------------------------------------------

/// `account.create`: make an account with its own profile folder on a
/// worker. Fails with `worker_offline` when the worker is offline.
///
/// `Debug` shows the names in `env`, never the values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountCreate {
    /// The worker.
    pub worker_id: WorkerId,
    /// The harness: `claude`, `codex`, `opencode`, `gemini` or `acp`.
    pub harness: String,
    /// The account name, for example "Claude Personal".
    pub label: String,
    /// The ACP agent, for an `acp` account.
    #[serde(default)]
    pub acp: Option<AcpAccountSpec>,
    /// Secret variables, such as an API key. The worker writes them to
    /// `env.json` in the profile folder. The hub does not store them.
    #[serde(default)]
    pub env: SecretEnv,
}

/// The response to `account.create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountCreateResult {
    /// The new account.
    pub account_id: AccountId,
}

/// `account.update`: rename an account or change its secret variables.
///
/// `Debug` shows the names in `env_set`, never the values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountUpdate {
    /// The worker.
    pub worker_id: WorkerId,
    /// The account.
    pub account_id: AccountId,
    /// The new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Secret variables to add or replace.
    #[serde(default)]
    pub env_set: SecretEnv,
    /// Secret variables to remove, by name.
    #[serde(default)]
    pub env_remove: Vec<String>,
    /// Turn the provider extras of the account on or off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extras_enabled: Option<bool>,
}

/// `account.remove`: the worker signs out where it can and deletes the
/// profile folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRemove {
    /// The worker.
    pub worker_id: WorkerId,
    /// The account.
    pub account_id: AccountId,
}

/// `account.sign_out`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSignOut {
    /// The worker.
    pub worker_id: WorkerId,
    /// The account.
    pub account_id: AccountId,
}

/// `sign_in.start`: start a provider's sign-in flow for an account. The hub
/// answers with the new sign-in. Each state change comes as a feed upsert
/// of the sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInStart {
    /// The worker.
    pub worker_id: WorkerId,
    /// The account.
    pub account_id: AccountId,
    /// The method, one of the account's `sign_in_methods`.
    pub method: SignInMethod,
}

/// The response to `sign_in.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInResult {
    /// The sign-in.
    pub sign_in: SignIn,
}

/// `sign_in.submit_code`: the code that the sign-in page showed, for a
/// sign-in in `needs_code`. The worker writes it to the harness. Nobody
/// stores it.
///
/// `Debug` does not show the code.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInSubmitCode {
    /// The sign-in.
    pub sign_in_id: SignInId,
    /// The code.
    pub code: String,
}

impl fmt::Debug for SignInSubmitCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignInSubmitCode")
            .field("sign_in_id", &self.sign_in_id)
            .field("code", &crate::model::Redacted)
            .finish()
    }
}

/// `sign_in.cancel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInCancel {
    /// The sign-in.
    pub sign_in_id: SignInId,
}

/// `acp_registry.list`: the agents in the ACP registry. The hub caches the
/// registry for one hour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpRegistryList {}

/// The response to `acp_registry.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpRegistryListResult {
    /// The agents.
    pub agents: Vec<AcpRegistryAgent>,
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

/// `preferences.set_favorite_model`: star a model in the model picker, or
/// take the star away. Adding a favorite that is there already, or removing
/// one that is not, changes nothing, so a retry is safe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreferencesSetFavoriteModel {
    /// The account label and the model ID.
    #[serde(flatten)]
    pub model: FavoriteModel,
    /// True to add the star, false to take it away.
    pub favorite: bool,
}

/// The preferences after a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreferencesResult {
    /// The preferences.
    pub preferences: Preferences,
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
        /// Get one full task.
        TaskGet("task.get", TaskGet => TaskResult),
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
        /// Set a worker's user tags.
        WorkerSetTags("worker.set_tags", WorkerSetTags => WorkerResult),
        /// Pause or resume a worker.
        WorkerSetPaused("worker.set_paused", WorkerSetPaused => WorkerResult),
        /// Remove a worker.
        WorkerRevoke("worker.revoke", WorkerRevoke => Empty),
        /// Ask a worker for a new inventory.
        WorkerRefreshInventory("worker.refresh_inventory", WorkerRefreshInventory => Empty),
        /// Check a worker's managed tools for updates now.
        WorkerToolsCheck("worker.tools_check", WorkerToolsCheck => Empty),
        /// Install or update one managed tool on a worker.
        WorkerToolInstall("worker.tool_install", WorkerToolInstall => Empty),
        /// Make an account on a worker.
        AccountCreate("account.create", AccountCreate => AccountCreateResult),
        /// Rename an account or change its secret variables.
        AccountUpdate("account.update", AccountUpdate => Empty),
        /// Remove an account and its profile folder.
        AccountRemove("account.remove", AccountRemove => Empty),
        /// Sign an account out.
        AccountSignOut("account.sign_out", AccountSignOut => Empty),
        /// Start a sign-in.
        SignInStart("sign_in.start", SignInStart => SignInResult),
        /// Send the code of a sign-in.
        SignInSubmitCode("sign_in.submit_code", SignInSubmitCode => Empty),
        /// Cancel a sign-in.
        SignInCancel("sign_in.cancel", SignInCancel => Empty),
        /// List the ACP registry agents.
        AcpRegistryList("acp_registry.list", AcpRegistryList => AcpRegistryListResult),
        /// Make a client pairing code.
        ClientPairingCode("client.pairing_code", ClientPairingCode => ClientPairingCodeResult),
        /// List clients.
        ClientList("client.list", ClientList => ClientListResult),
        /// Revoke a client.
        ClientRevoke("client.revoke", ClientRevoke => Empty),
        /// Set the push token.
        ClientPushRegister("client.push_register", ClientPushRegister => Empty),
        /// Star a model, or take the star away.
        PreferencesSetFavoriteModel("preferences.set_favorite_model", PreferencesSetFavoriteModel => PreferencesResult),
        /// Get a bearer token for the blob endpoints.
        AuthHttpToken("auth.http_token", AuthHttpToken => HttpToken),
        /// Store the Composio key on the hub.
        ComposioSetKey("composio.set_key", ComposioSetKey => ComposioStatusResult),
        /// Delete the Composio key.
        ComposioRemoveKey("composio.remove_key", ComposioRemoveKey => ComposioStatusResult),
        /// The Composio settings, without the key.
        ComposioGetStatus("composio.status", ComposioGetStatus => ComposioStatusResult),
        /// Search the connector catalog.
        ConnectorCatalogSearch("connector_catalog.search", ConnectorCatalogSearch => ConnectorCatalogSearchResult),
        /// The categories of the connector catalog.
        ConnectorCatalogCategories("connector_catalog.categories", ConnectorCatalogCategories => ConnectorCatalogCategoriesResult),
        /// One service with its tools.
        ConnectorCatalogService("connector_catalog.service", ConnectorCatalogService => ConnectorServiceDetail),
        /// Connect a service.
        ConnectionConnect("connection.connect", ConnectionConnect => ConnectAttempt),
        /// Change a connection.
        ConnectionUpdate("connection.update", ConnectionUpdate => ConnectionResult),
        /// Disconnect a connection.
        ConnectionDisconnect("connection.disconnect", ConnectionDisconnect => Empty),
        /// Sign a connection in again.
        ConnectionReconnect("connection.reconnect", ConnectionReconnect => ConnectAttempt),
        /// A page of a connection's audit log.
        ConnectionCalls("connection.calls", ConnectionCalls => ConnectionCallsResult),
        /// Turn a connection off or on in one channel.
        ChannelSetConnectionEnabled("channel.set_connection_enabled", ChannelSetConnectionEnabled => ChannelResult),
        /// Turn a skill off or on in one channel.
        ChannelSetSkillEnabled("channel.set_skill_enabled", ChannelSetSkillEnabled => ChannelResult),
        /// Search the skill directories.
        SkillDirectorySearch("skill_directory.search", SkillDirectorySearch => SkillDirectorySearchResult),
        /// The skill directories.
        SkillDirectorySources("skill_directory.sources", SkillDirectorySources => SkillDirectorySourcesResult),
        /// Add a marketplace file.
        SkillDirectoryAddSource("skill_directory.add_source", SkillDirectoryAddSource => SkillDirectorySourcesResult),
        /// Remove a marketplace file.
        SkillDirectoryRemoveSource("skill_directory.remove_source", SkillDirectoryRemoveSource => SkillDirectorySourcesResult),
        /// Fetch and check a skill without installing it.
        SkillPreview("skill.preview", SkillPreview => SkillPreviewResult),
        /// Install a skill.
        SkillInstall("skill.install", SkillInstall => SkillResult),
        /// Change a skill.
        SkillUpdate("skill.update", SkillUpdate => SkillResult),
        /// Remove a skill.
        SkillRemove("skill.remove", SkillRemove => Empty),
        /// The file list of a skill.
        SkillFiles("skill.files", SkillFiles => SkillFilesResult),
        /// One file of a skill.
        SkillReadFile("skill.read_file", SkillReadFile => SkillFileContent),
    }
}

/// `feed.skipped`: the hub could not send the feed entry `seq`, because the
/// entry does not fit in one frame. The feed goes on after it. The client
/// gets the entity again another way, for example with `message.list` or a
/// new snapshot. A client that does not know this event can ignore it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedSkipped {
    /// The `seq` of the entry that the hub did not send.
    pub seq: u64,
    /// The channel of the entry, as in the feed entry.
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    /// The kind of entity.
    pub entity: EntityKind,
    /// The feed ID of the entity.
    pub entity_id: String,
}

message_family! {
    /// Events from the hub to a client after authentication.
    pub enum HubToClient {
        /// One feed entry.
        Feed("feed", FeedEntry),
        /// A feed entry that the hub could not send.
        FeedSkipped("feed.skipped", FeedSkipped),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Frame, FrameId, MessageFamily};
    use crate::ids::{ConnectionId, ConnectorCallId, SkillId};
    use crate::model::{
        ApprovalMode, AuthSchemeKind, Author, ChannelKind, ConnectionStatus, Isolation,
        MemoryAuthor, MessageKind, SkillSource, SkillUpdateState, TaskCreator, ToolApprovals,
        UseIn,
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
        let acc: AccountId = "acc_00000000-0000-0000-0000-000000000009".parse().unwrap();
        let sgn: SignInId = "sgn_00000000-0000-0000-0000-00000000000a".parse().unwrap();
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
                tags: vec!["work".into()],
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
            TaskGet { task_id: tsk }.into(),
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
            WorkerSetTags {
                worker_id: wrk,
                tags: vec!["gpu".into()],
            }
            .into(),
            WorkerSetPaused {
                worker_id: wrk,
                paused: true,
            }
            .into(),
            WorkerRevoke { worker_id: wrk }.into(),
            WorkerRefreshInventory { worker_id: wrk }.into(),
            WorkerToolsCheck { worker_id: wrk }.into(),
            WorkerToolInstall {
                worker_id: wrk,
                tool: ToolId::claude_code(),
            }
            .into(),
            AccountCreate {
                worker_id: wrk,
                harness: "acp".into(),
                label: "Goose".into(),
                acp: Some(AcpAccountSpec::Registry { id: "goose".into() }),
                env: [("GOOSE_API_KEY", "secret")].into_iter().collect(),
            }
            .into(),
            AccountUpdate {
                worker_id: wrk,
                account_id: acc,
                label: Some("Claude Work".into()),
                env_set: [("ANTHROPIC_API_KEY", "sk")].into_iter().collect(),
                env_remove: vec!["OLD".into()],
                extras_enabled: Some(false),
            }
            .into(),
            AccountRemove {
                worker_id: wrk,
                account_id: acc,
            }
            .into(),
            AccountSignOut {
                worker_id: wrk,
                account_id: acc,
            }
            .into(),
            SignInStart {
                worker_id: wrk,
                account_id: acc,
                method: SignInMethod::ClaudePlan,
            }
            .into(),
            SignInSubmitCode {
                sign_in_id: sgn,
                code: "abc#def".into(),
            }
            .into(),
            SignInCancel { sign_in_id: sgn }.into(),
            AcpRegistryList {}.into(),
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
            PreferencesSetFavoriteModel {
                model: FavoriteModel {
                    account_label: "Claude Personal".into(),
                    model: "claude-opus-5-5".into(),
                },
                favorite: true,
            }
            .into(),
            AuthHttpToken {}.into(),
            ComposioSetKey {
                api_key: "ak_secret".into(),
            }
            .into(),
            ComposioRemoveKey {}.into(),
            ComposioGetStatus {}.into(),
            ConnectorCatalogSearch {
                search: Some("mail".into()),
                category: Some("email".into()),
                cursor: None,
                limit: Some(20),
            }
            .into(),
            ConnectorCatalogCategories {}.into(),
            ConnectorCatalogService {
                service: "gmail".into(),
            }
            .into(),
            ConnectionConnect {
                service: "gmail".into(),
                name: "Gmail Work".into(),
                use_in: UseIn::Tags {
                    tags: vec!["work".into()],
                },
                approvals: ToolApprovals::default(),
                auth_scheme: Some(AuthSchemeKind::Oauth2),
                credentials: SecretEnv::new(),
                approval_id: Some(ApprovalId::new()),
            }
            .into(),
            ConnectionUpdate {
                connection_id: con(),
                name: Some("Gmail Office".into()),
                use_in: Some(UseIn::EveryChannel),
                approvals: None,
            }
            .into(),
            ConnectionDisconnect {
                connection_id: con(),
            }
            .into(),
            ConnectionReconnect {
                connection_id: con(),
                credentials: [("api_key", "k")].into_iter().collect(),
            }
            .into(),
            ConnectionCalls {
                connection_id: con(),
                before: Some(ConnectorCallId::new()),
                limit: Some(20),
            }
            .into(),
            ChannelSetConnectionEnabled {
                channel_id: chn(),
                connection_id: con(),
                enabled: false,
            }
            .into(),
            ChannelSetSkillEnabled {
                channel_id: chn(),
                skill_id: skl(),
                enabled: true,
            }
            .into(),
            SkillDirectorySearch {
                search: Some("pdf".into()),
                source: Some("smithery".into()),
                cursor: Some("c2".into()),
                limit: None,
            }
            .into(),
            SkillDirectorySources {}.into(),
            SkillDirectoryAddSource {
                url: "https://example.com/.claude-plugin/marketplace.json".into(),
            }
            .into(),
            SkillDirectoryRemoveSource {
                id: "https://example.com/.claude-plugin/marketplace.json".into(),
            }
            .into(),
            SkillPreview {
                source: SkillInstallSource::Git {
                    url: "anthropics/skills".into(),
                    path: Some("pdf".into()),
                    git_ref: Some("main".into()),
                },
            }
            .into(),
            SkillInstall {
                source: SkillInstallSource::Upload {
                    blob_sha256: "f".repeat(64).parse().unwrap(),
                    size: 2_048,
                    file_name: "my-skill.zip".into(),
                },
                use_in: UseIn::Channels {
                    channel_ids: vec![chn()],
                },
                enabled: Some(true),
                approval_id: None,
            }
            .into(),
            SkillUpdate {
                skill_id: skl(),
                enabled: Some(true),
                use_in: None,
                accept_new_version: true,
            }
            .into(),
            SkillRemove { skill_id: skl() }.into(),
            SkillFiles {
                skill_id: skl(),
                version: SkillVersion::Pending,
            }
            .into(),
            SkillReadFile {
                skill_id: skl(),
                path: "SKILL.md".into(),
                version: SkillVersion::Installed,
            }
            .into(),
        ]
    }

    fn con() -> ConnectionId {
        "con_00000000-0000-0000-0000-00000000000b".parse().unwrap()
    }

    fn skl() -> SkillId {
        "skl_00000000-0000-0000-0000-00000000000c".parse().unwrap()
    }

    fn connection() -> Connection {
        Connection {
            id: con(),
            name: "Gmail Work".into(),
            handle: "gmail_work".into(),
            service: "gmail".into(),
            service_name: "Gmail".into(),
            logo_url: None,
            identity: Some("chris@work.example".into()),
            use_in: UseIn::EveryChannel,
            approvals: ToolApprovals::default(),
            status: ConnectionStatus::Connecting {
                redirect_url: Some("https://connect.composio.dev/link/abc".into()),
                expires_at: Timestamp::from_millis(600_000),
            },
            last_used_at: None,
            created_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(1),
        }
    }

    fn skill() -> Skill {
        Skill {
            id: skl(),
            name: "pdf-forms".into(),
            description: "Fill PDF forms.".into(),
            source: SkillSource::Upload {
                file_name: "pdf-forms.zip".into(),
            },
            commit: None,
            content_hash: "d".repeat(64).parse().unwrap(),
            license: None,
            scripts: vec![],
            audits: vec![],
            enabled: true,
            use_in: UseIn::EveryChannel,
            update: SkillUpdateState::UpToDate,
            blocked_reason: None,
            hooks_removed: true,
            file_count: 1,
            size_bytes: 300,
            installed_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(1),
        }
    }

    #[test]
    fn connection_and_skill_feed_entries_and_snapshot() {
        for payload in [
            EntityPayload::Connection(connection()),
            EntityPayload::Skill(skill()),
        ] {
            assert_eq!(payload.channel_id(), None);
            let kind = payload.kind();
            let id = payload.entity_id();
            let entry = FeedEntry::upsert(11, Timestamp::from_millis(10), payload);
            let value = serde_json::to_value(&entry).unwrap();
            assert_eq!(value["entity"], serde_json::to_value(kind).unwrap());
            assert_eq!(value["entity_id"], id);
            assert_eq!(serde_json::from_value::<FeedEntry>(value).unwrap(), entry);
        }
        assert_eq!(
            serde_json::to_value(EntityKind::Connection).unwrap(),
            "connection"
        );
        assert_eq!(serde_json::to_value(EntityKind::Skill).unwrap(), "skill");
        let snapshot = Snapshot {
            seq: 4,
            connections: vec![connection()],
            skills: vec![skill()],
            ..Snapshot::default()
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["connections"][0]["handle"], "gmail_work");
        assert_eq!(serde_json::from_value::<Snapshot>(value).unwrap(), snapshot);
        let old: Snapshot = serde_json::from_value(json!({ "seq": 5 })).unwrap();
        assert!(old.connections.is_empty() && old.skills.is_empty());
        let out = serde_json::to_value(&old).unwrap();
        assert!(out.get("connections").is_none() && out.get("skills").is_none());
    }

    #[test]
    fn connector_and_skill_results_round_trip() {
        fn round<T>(value: &T)
        where
            T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
        {
            let back: T = serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap();
            assert_eq!(&back, value);
        }
        round(&ComposioStatusResult {
            status: crate::model::ComposioStatus {
                key_present: true,
                key_hint: Some("x9k2".into()),
                ..Default::default()
            },
        });
        let service = crate::model::ConnectorService {
            slug: "gmail".into(),
            name: "Gmail".into(),
            description: String::new(),
            logo_url: None,
            categories: vec![],
            tool_count: 2,
            auth_schemes: vec![],
            connected_count: 0,
        };
        round(&ConnectorCatalogSearchResult {
            services: vec![service.clone()],
            next_cursor: Some("n".into()),
        });
        round(&ConnectorCatalogCategoriesResult {
            categories: vec![crate::model::ConnectorCategory {
                slug: "email".into(),
                name: "Email".into(),
                service_count: None,
            }],
        });
        round(&ConnectorServiceDetail {
            service,
            tools: vec![crate::model::ConnectorToolSummary {
                slug: "GMAIL_FETCH_EMAILS".into(),
                description: "Fetch emails.".into(),
                kind: crate::model::ToolKind::Read,
            }],
        });
        round(&ConnectAttempt {
            connection: connection(),
            redirect_url: Some("https://connect.composio.dev/link/abc".into()),
            expires_at: Some(Timestamp::from_millis(600_000)),
        });
        round(&ConnectionResult {
            connection: connection(),
        });
        round(&ConnectionCallsResult {
            calls: vec![],
            has_more: false,
        });
        round(&SkillDirectorySearchResult {
            entries: vec![],
            next_cursor: None,
            unavailable: vec!["Smithery".into()],
        });
        round(&SkillDirectorySourcesResult { sources: vec![] });
        round(&SkillPreviewResult {
            name: "pdf-forms".into(),
            description: "Fill PDF forms.".into(),
            commit: Some("abc".into()),
            content_hash: "d".repeat(64).parse().unwrap(),
            license: Some("MIT".into()),
            scripts: vec!["scripts/fill.py".into()],
            audits: vec![],
            files: vec![],
            has_hooks: true,
            blocked_reason: None,
            installed_skill_id: None,
        });
        round(&SkillResult { skill: skill() });
        round(&SkillFilesResult {
            content_hash: "d".repeat(64).parse().unwrap(),
            files: vec![],
        });
        round(&SkillFileContent {
            path: "SKILL.md".into(),
            sha256: "a".repeat(64).parse().unwrap(),
            size: 12,
            content: Some("---\nname: pdf\n".into()),
            binary: false,
            truncated: false,
        });
    }

    #[test]
    fn new_requests_read_minimal_bodies() {
        let connect: ConnectionConnect =
            serde_json::from_value(json!({"service": "gmail", "name": "Gmail Work"})).unwrap();
        assert_eq!(connect.use_in, UseIn::EveryChannel);
        assert_eq!(connect.approvals, ToolApprovals::default());
        assert!(connect.credentials.is_empty());
        assert_eq!(connect.approval_id, None);
        let files: SkillFiles =
            serde_json::from_value(json!({"skill_id": skl().to_string()})).unwrap();
        assert_eq!(files.version, SkillVersion::Installed);
        let update = SkillUpdate {
            skill_id: skl(),
            enabled: None,
            use_in: None,
            accept_new_version: false,
        };
        assert_eq!(
            serde_json::to_value(&update).unwrap(),
            json!({"skill_id": skl().to_string()})
        );
        let install: SkillInstall = serde_json::from_value(json!({
            "source": {"kind": "directory", "entry_id": "@anthropic/pdf"}
        }))
        .unwrap();
        assert_eq!(install.use_in, UseIn::EveryChannel);
        assert!(matches!(
            install.source,
            SkillInstallSource::Directory { source: None, .. }
        ));
        let git = serde_json::to_value(SkillInstallSource::Git {
            url: "anthropics/skills".into(),
            path: None,
            git_ref: Some("v1".into()),
        })
        .unwrap();
        assert_eq!(git["ref"], "v1");
        assert_eq!(git["kind"], "git");
        // `account.update` from an older client leaves the extras alone.
        let old: AccountUpdate = serde_json::from_value(json!({
            "worker_id": "wrk_00000000-0000-0000-0000-000000000006",
            "account_id": "acc_00000000-0000-0000-0000-000000000009"
        }))
        .unwrap();
        assert_eq!(old.extras_enabled, None);
        assert!(
            serde_json::to_value(&old)
                .unwrap()
                .get("extras_enabled")
                .is_none()
        );
    }

    /// The Composio key and connector credentials never show in `Debug`,
    /// also through the request enum.
    #[test]
    fn debug_hides_connector_secrets() {
        let key = ComposioSetKey {
            api_key: "ak_composio_secret".into(),
        };
        let connect = ConnectionConnect {
            service: "linear".into(),
            name: "Linear".into(),
            use_in: UseIn::EveryChannel,
            approvals: ToolApprovals::default(),
            auth_scheme: Some(AuthSchemeKind::ApiKey),
            credentials: [("api_key", "lin_api_secret")].into_iter().collect(),
            approval_id: None,
        };
        let reconnect = ConnectionReconnect {
            connection_id: con(),
            credentials: [("api_key", "lin_api_other")].into_iter().collect(),
        };
        let texts = [
            format!("{key:?}"),
            format!("{:?}", ClientRequest::from(key.clone())),
            format!("{connect:?}"),
            format!("{:#?}", ClientRequest::from(connect.clone())),
            format!("{reconnect:?}"),
            format!("{:?}", ClientRequest::from(reconnect)),
        ];
        for text in &texts {
            for secret in ["ak_composio_secret", "lin_api_secret", "lin_api_other"] {
                assert!(!text.contains(secret), "{text}");
            }
        }
        assert!(texts[0].contains("<redacted>"), "{}", texts[0]);
        assert!(texts[2].contains("api_key"), "{}", texts[2]);
        // The wire still carries the values.
        assert_eq!(
            serde_json::to_value(&key).unwrap()["api_key"],
            "ak_composio_secret"
        );
        assert_eq!(
            serde_json::to_value(&connect).unwrap()["credentials"]["api_key"],
            "lin_api_secret"
        );
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
            "task.get",
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
            "worker.set_tags",
            "worker.revoke",
            "worker.refresh_inventory",
            "worker.tools_check",
            "worker.tool_install",
            "account.create",
            "account.update",
            "account.remove",
            "account.sign_out",
            "sign_in.start",
            "sign_in.submit_code",
            "sign_in.cancel",
            "acp_registry.list",
            "client.pairing_code",
            "client.list",
            "client.revoke",
            "client.push_register",
            "auth.http_token",
            "preferences.set_favorite_model",
            "composio.set_key",
            "composio.remove_key",
            "composio.status",
            "connector_catalog.search",
            "connector_catalog.categories",
            "connector_catalog.service",
            "connection.connect",
            "connection.update",
            "connection.disconnect",
            "connection.reconnect",
            "connection.calls",
            "channel.set_connection_enabled",
            "channel.set_skill_enabled",
            "skill_directory.search",
            "skill_directory.sources",
            "skill_directory.add_source",
            "skill_directory.remove_source",
            "skill.preview",
            "skill.install",
            "skill.update",
            "skill.remove",
            "skill.files",
            "skill.read_file",
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
            allowed_local_mcp_servers: vec!["home-assistant".into()],
            memory_enabled: true,
            tags: vec!["work".into()],
            disabled_connections: vec![],
            disabled_skills: vec![],
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

    /// A channel from a hub before `allowed_local_mcp_servers` allows none,
    /// and an update that leaves the list out does not send it.
    #[test]
    fn an_old_channel_payload_allows_no_local_mcp_server() {
        let mut value = serde_json::to_value(Channel {
            id: chn(),
            kind: ChannelKind::Standard,
            name: "Home".into(),
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
            allowed_local_mcp_servers: vec!["home-assistant".into()],
            memory_enabled: true,
            tags: vec!["work".into()],
            disabled_connections: vec![],
            disabled_skills: vec![],
            archived_at: None,
            created_at: Timestamp::from_millis(0),
            updated_at: Timestamp::from_millis(0),
        })
        .unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("allowed_local_mcp_servers");
        value.as_object_mut().unwrap().remove("tags");
        value
            .as_object_mut()
            .unwrap()
            .remove("disabled_connections");
        value.as_object_mut().unwrap().remove("disabled_skills");
        let old: Channel = serde_json::from_value(value).unwrap();
        assert!(old.allowed_local_mcp_servers.is_empty());
        assert!(old.tags.is_empty());
        assert!(old.disabled_connections.is_empty());
        assert!(old.disabled_skills.is_empty());
        let update = serde_json::to_value(ChannelUpdate::new(chn())).unwrap();
        assert!(update.get("allowed_local_mcp_servers").is_none());
        assert!(update.get("tags").is_none());
    }

    #[test]
    fn worker_set_tags_reads_old_labels() {
        let old: WorkerSetTags = serde_json::from_value(json!({
            "worker_id": "wrk_00000000-0000-0000-0000-000000000006",
            "labels": ["gpu"]
        }))
        .unwrap();
        assert_eq!(old.tags, ["gpu"]);
        assert!(serde_json::to_value(&old).unwrap().get("labels").is_none());
    }

    /// AGENTS.md and the plan: secret values and sign-in codes never show
    /// in `Debug` output, also through the request enum.
    #[test]
    fn debug_hides_secrets() {
        let wrk: WorkerId = "wrk_00000000-0000-0000-0000-000000000006".parse().unwrap();
        let acc = AccountId::new();
        let create = AccountCreate {
            worker_id: wrk,
            harness: "claude".into(),
            label: "Claude API".into(),
            acp: None,
            env: [("ANTHROPIC_API_KEY", "sk-ant-very-secret")]
                .into_iter()
                .collect(),
        };
        let update = AccountUpdate {
            worker_id: wrk,
            account_id: acc,
            label: None,
            env_set: [("OPENAI_API_KEY", "sk-openai-secret")]
                .into_iter()
                .collect(),
            env_remove: vec![],
            extras_enabled: None,
        };
        let code = SignInSubmitCode {
            sign_in_id: SignInId::new(),
            code: "pasted-code-123".into(),
        };
        let texts = [
            format!("{create:?}"),
            format!("{:?}", ClientRequest::from(create.clone())),
            format!("{update:?}"),
            format!("{:?}", ClientRequest::from(update)),
            format!("{code:?}"),
            format!("{:?}", ClientRequest::from(code)),
            format!("{create:#?}"),
        ];
        for text in &texts {
            for secret in ["sk-ant-very-secret", "sk-openai-secret", "pasted-code-123"] {
                assert!(!text.contains(secret), "{text}");
            }
        }
        assert!(texts[0].contains("ANTHROPIC_API_KEY"), "{}", texts[0]);
        assert!(texts[0].contains("Claude API"), "{}", texts[0]);
        assert!(texts[2].contains("OPENAI_API_KEY"), "{}", texts[2]);
        // The wire still carries the values.
        let value = serde_json::to_value(&create).unwrap();
        assert_eq!(value["env"]["ANTHROPIC_API_KEY"], "sk-ant-very-secret");
    }

    #[test]
    fn account_requests_read_minimal_bodies() {
        let create: AccountCreate = serde_json::from_value(json!({
            "worker_id": "wrk_00000000-0000-0000-0000-000000000006",
            "harness": "claude",
            "label": "Claude Personal"
        }))
        .unwrap();
        assert!(create.env.is_empty());
        assert_eq!(create.acp, None);
        let update: AccountUpdate = serde_json::from_value(json!({
            "worker_id": "wrk_00000000-0000-0000-0000-000000000006",
            "account_id": "acc_00000000-0000-0000-0000-000000000009"
        }))
        .unwrap();
        assert_eq!(update.label, None);
        assert!(update.env_set.is_empty() && update.env_remove.is_empty());
        let result = AccountCreateResult {
            account_id: "acc_00000000-0000-0000-0000-000000000009".parse().unwrap(),
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({"account_id": "acc_00000000-0000-0000-0000-000000000009"})
        );
        let list = AcpRegistryListResult {
            agents: vec![AcpRegistryAgent {
                id: "goose".into(),
                name: "Goose".into(),
                description: String::new(),
                version: "1.0.0".into(),
                icon_url: None,
                supported: false,
            }],
        };
        let back: AcpRegistryListResult =
            serde_json::from_value(serde_json::to_value(&list).unwrap()).unwrap();
        assert_eq!(back, list);
    }

    #[test]
    fn a_snapshot_with_sign_ins_round_trips_and_an_old_one_has_none() {
        use crate::model::SignInState;
        let sign_in = SignIn {
            id: "sgn_00000000-0000-0000-0000-00000000000a".parse().unwrap(),
            worker_id: "wrk_00000000-0000-0000-0000-000000000006".parse().unwrap(),
            account_id: "acc_00000000-0000-0000-0000-000000000009".parse().unwrap(),
            method: SignInMethod::ClaudePlan,
            state: SignInState::NeedsCode {
                url: "https://claude.com/cai/oauth/authorize".into(),
            },
            created_at: Timestamp::from_millis(10),
            updated_at: Timestamp::from_millis(11),
            expires_at: Timestamp::from_millis(20),
        };
        let snapshot = Snapshot {
            seq: 3,
            sign_ins: vec![sign_in],
            ..Snapshot::default()
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["sign_ins"][0]["state"]["kind"], "needs_code");
        assert_eq!(serde_json::from_value::<Snapshot>(value).unwrap(), snapshot);
        // A snapshot from a hub that does not know sign-ins has none, and one
        // with none leaves the field out.
        let old: Snapshot = serde_json::from_value(serde_json::json!({ "seq": 5 })).unwrap();
        assert!(old.sign_ins.is_empty());
        let json = serde_json::to_value(&old).unwrap();
        assert!(json.get("sign_ins").is_none());
    }

    #[test]
    fn sign_in_feed_entry_round_trips() {
        use crate::model::{SIGN_IN_IDLE_EXPIRY_MS, SignInState};
        let sign_in = SignIn {
            id: "sgn_00000000-0000-0000-0000-00000000000a".parse().unwrap(),
            worker_id: "wrk_00000000-0000-0000-0000-000000000006".parse().unwrap(),
            account_id: "acc_00000000-0000-0000-0000-000000000009".parse().unwrap(),
            method: SignInMethod::ChatgptDevice,
            state: SignInState::DeviceCode {
                url: "https://auth.openai.com/device".into(),
                user_code: "ABCD-EFGH".into(),
            },
            created_at: Timestamp::from_millis(10),
            updated_at: Timestamp::from_millis(10),
            expires_at: Timestamp::from_millis(10 + SIGN_IN_IDLE_EXPIRY_MS),
        };
        let payload = EntityPayload::SignIn(sign_in.clone());
        assert_eq!(payload.kind(), EntityKind::SignIn);
        assert_eq!(payload.channel_id(), None);
        assert_eq!(
            payload.entity_id(),
            "sgn_00000000-0000-0000-0000-00000000000a"
        );
        let entry = FeedEntry::upsert(7, Timestamp::from_millis(10), payload);
        let value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["entity"], "sign_in");
        assert_eq!(value["payload"]["state"]["kind"], "device_code");
        assert_eq!(serde_json::from_value::<FeedEntry>(value).unwrap(), entry);
        let result = SignInResult { sign_in };
        let back: SignInResult =
            serde_json::from_value(serde_json::to_value(&result).unwrap()).unwrap();
        assert_eq!(back, result);
    }

    #[test]
    fn preferences_round_trip_in_a_request_a_snapshot_and_the_feed() {
        let favorite = FavoriteModel {
            account_label: "Claude Work".into(),
            model: "claude-sonnet-5-5".into(),
        };
        // The request has the fields of the favorite at its top level.
        let request = PreferencesSetFavoriteModel {
            model: favorite.clone(),
            favorite: false,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            value,
            json!({ "account_label": "Claude Work", "model": "claude-sonnet-5-5", "favorite": false })
        );
        assert_eq!(
            serde_json::from_value::<PreferencesSetFavoriteModel>(value).unwrap(),
            request
        );
        let preferences = Preferences {
            favorite_models: vec![favorite],
        };
        let result = PreferencesResult {
            preferences: preferences.clone(),
        };
        let back: PreferencesResult =
            serde_json::from_value(serde_json::to_value(&result).unwrap()).unwrap();
        assert_eq!(back, result);
        // In the feed: one record, with no channel.
        let payload = EntityPayload::Preferences(preferences.clone());
        assert_eq!(payload.kind(), EntityKind::Preferences);
        assert_eq!(payload.entity_id(), "preferences");
        assert_eq!(payload.channel_id(), None);
        let entry = FeedEntry::upsert(9, Timestamp::from_millis(10), payload);
        let value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["entity"], "preferences");
        assert_eq!(
            value["payload"]["favorite_models"][0]["model"],
            "claude-sonnet-5-5"
        );
        assert_eq!(serde_json::from_value::<FeedEntry>(value).unwrap(), entry);
        // In a snapshot of `all`. An older snapshot has none.
        let snapshot = Snapshot {
            seq: 4,
            preferences: Some(preferences),
            ..Snapshot::default()
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(serde_json::from_value::<Snapshot>(value).unwrap(), snapshot);
        let old: Snapshot = serde_json::from_value(json!({ "seq": 5 })).unwrap();
        assert_eq!(old.preferences, None);
        assert!(
            serde_json::to_value(&old)
                .unwrap()
                .get("preferences")
                .is_none()
        );
    }
}
