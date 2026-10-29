//! Conversation entities: groups, channels, repos, messages and read markers.

use serde::{Deserialize, Serialize};

use super::common::{BlobRef, ModelSelection};
use super::knowledge::MemoryScope;
use super::work::{ApprovalMode, Isolation};
use crate::ids::{ChannelId, GroupId, MessageId, RepoId, RoutineId, SessionId, TaskId, WorkerId};
use crate::time::Timestamp;

/// A named folder of channels in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// The ID.
    pub id: GroupId,
    /// The name.
    pub name: String,
    /// Sidebar order.
    pub position: i64,
    /// Whether the group is collapsed. Shared by all clients.
    pub collapsed: bool,
}

/// The kind of a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// The one Everything channel. It has no group and cannot be archived or
    /// deleted.
    Everything,
    /// Any other channel.
    Standard,
}

/// A conversation scoped to one subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// The ID.
    pub id: ChannelId,
    /// `everything` or `standard`.
    pub kind: ChannelKind,
    /// The name.
    pub name: String,
    /// The icon name.
    pub icon: Option<String>,
    /// The color role name.
    pub color: Option<String>,
    /// The group. Always empty for Everything.
    pub group_id: Option<GroupId>,
    /// Order inside the group.
    pub position: i64,
    /// The one-line purpose.
    pub purpose: String,
    /// The instructions, in markdown.
    pub instructions: String,
    /// The coordinator's model selection. Empty means the Everything
    /// channel's value.
    pub coordinator_selection: Option<ModelSelection>,
    /// The default model selection for new tasks.
    pub task_selection: Option<ModelSelection>,
    /// Workers that placement tries first, in order.
    #[serde(default)]
    pub preferred_workers: Vec<WorkerId>,
    /// The attached repos, in order. This is the `channel_repos` join.
    #[serde(default)]
    pub repo_ids: Vec<RepoId>,
    /// The default isolation for new tasks.
    pub default_isolation: Isolation,
    /// The default approval mode for new tasks. It is also the most that the
    /// assistant can give a task that it starts.
    pub default_approval_mode: ApprovalMode,
    /// The local MCP servers, by name, that a task that the assistant starts
    /// may use. The default is none. A task that the user starts is not
    /// limited by this list.
    #[serde(default)]
    pub allowed_local_mcp_servers: Vec<String>,
    /// Whether the channel has its own memory.
    pub memory_enabled: bool,
    /// When the channel was archived.
    pub archived_at: Option<Timestamp>,
    /// When the channel was created.
    pub created_at: Timestamp,
    /// When the channel last changed.
    pub updated_at: Timestamp,
}

impl Channel {
    /// Whether this is the Everything channel.
    pub fn is_everything(&self) -> bool {
        self.kind == ChannelKind::Everything
    }

    /// The memory scope that this channel owns. The Everything channel's
    /// memory is the global memory.
    pub fn memory_scope(&self) -> MemoryScope {
        match self.kind {
            ChannelKind::Everything => MemoryScope::Global,
            ChannelKind::Standard => MemoryScope::Channel(self.id),
        }
    }

    /// The coordinator's model selection, with the fallback to the
    /// Everything channel's value.
    pub fn effective_coordinator_selection<'a>(
        &'a self,
        everything: &'a Channel,
    ) -> Option<&'a ModelSelection> {
        self.coordinator_selection
            .as_ref()
            .or(everything.coordinator_selection.as_ref())
    }
}

/// A git repository. A repo is global, so two channels can attach it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    /// The ID.
    pub id: RepoId,
    /// The normalized git remote URL.
    pub url: String,
    /// The name to show.
    pub name: String,
    /// The default branch, for example `main`.
    pub default_branch: String,
}

/// One row of the join between channels and repos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRepo {
    /// The channel.
    pub channel_id: ChannelId,
    /// The repo.
    pub repo_id: RepoId,
    /// Order in the channel.
    pub position: i64,
}

/// Who wrote a message or a reaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    /// The user.
    User,
    /// The assistant.
    Assistant,
    /// The hub.
    System,
}

/// What a message shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// Markdown text.
    Text,
    /// A task card. `author_task_id` names the task.
    TaskCard,
    /// A decision card.
    Decision,
    /// An approval card.
    Approval,
    /// A hub notice. The body is a JSON object with a `kind` field.
    Notice,
}

/// A reaction on a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
    /// Who reacted.
    pub author: Author,
    /// The emoji.
    pub emoji: String,
    /// When.
    pub created_at: Timestamp,
}

/// A message in a channel's timeline or in a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// The ID.
    pub id: MessageId,
    /// The channel.
    pub channel_id: ChannelId,
    /// The thread root. Empty for timeline messages.
    pub thread_root_id: Option<MessageId>,
    /// Who wrote it.
    pub author: Author,
    /// The coordinator or task session that posted it.
    pub author_session_id: Option<SessionId>,
    /// The task that posted it, or the task of a task card.
    pub author_task_id: Option<TaskId>,
    /// The routine that caused it.
    pub author_routine_id: Option<RoutineId>,
    /// What the message shows.
    pub kind: MessageKind,
    /// Markdown. For `notice`, a JSON object with a `kind` field.
    pub body: String,
    /// Attached blobs.
    #[serde(default)]
    pub attachments: Vec<BlobRef>,
    /// True for a thread reply that also shows in the timeline.
    #[serde(default)]
    pub in_timeline: bool,
    /// The client's idempotency key. Unique per client.
    pub client_message_id: Option<String>,
    /// The number of replies. Set on thread roots only.
    #[serde(default)]
    pub reply_count: u32,
    /// The time of the newest reply. Set on thread roots only.
    pub last_reply_at: Option<Timestamp>,
    /// Reactions, oldest first.
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    /// When the message was last edited.
    pub edited_at: Option<Timestamp>,
    /// When the message was deleted. A deleted message keeps its place but
    /// has an empty body.
    pub deleted_at: Option<Timestamp>,
    /// When the message was posted.
    pub created_at: Timestamp,
}

impl Message {
    /// Whether the message shows in the timeline: a root message, or a
    /// thread reply with `in_timeline`.
    pub fn shows_in_timeline(&self) -> bool {
        self.thread_root_id.is_none() || self.in_timeline
    }
}

/// How far the user has read in a timeline or a thread. The hub has one
/// user, so a read marker applies to every client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadMarker {
    /// The channel.
    pub channel_id: ChannelId,
    /// The thread root. Empty for the timeline.
    pub thread_root_id: Option<MessageId>,
    /// The newest message that the user has read.
    pub last_read_message_id: MessageId,
}
