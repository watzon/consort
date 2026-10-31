//! Conversation entities: groups, channels, repos, messages and read markers.

use serde::{Deserialize, Serialize};

use super::access::AccessGrant;
use super::common::{BlobRef, ModelSelection};
use super::fallback::CoordinatorFallback;
use super::knowledge::MemoryScope;
use super::monid::MonidChannelAccess;
use super::proactivity::{ProactiveMark, ProactiveTasks, ProactivityLevel, Suggestion};
use super::work::{ApprovalMode, Isolation};
use crate::ids::{
    ChannelId, ConnectionId, GroupId, MessageId, RepoId, RoutineId, SessionId, SkillId, TaskId,
    WorkerId,
};
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
    /// The one Tutti channel. It has no group and cannot be archived or
    /// deleted. The channel was called Everything before: old data and older
    /// peers say `everything`, and this type still reads it.
    #[serde(alias = "everything")]
    Tutti,
    /// Any other channel.
    Standard,
}

/// A conversation scoped to one subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// The ID.
    pub id: ChannelId,
    /// `tutti` or `standard`.
    pub kind: ChannelKind,
    /// The name.
    pub name: String,
    /// The icon name.
    pub icon: Option<String>,
    /// The color role name.
    pub color: Option<String>,
    /// The group. Always empty for Tutti.
    pub group_id: Option<GroupId>,
    /// Order inside the group.
    pub position: i64,
    /// The one-line purpose.
    pub purpose: String,
    /// The instructions, in markdown.
    pub instructions: String,
    /// The coordinator's model selection. Empty means the Tutti
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
    /// Tags that a worker must all have to run work from this channel:
    /// coordinator turns, tasks and routine runs.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Connections that this channel turned off (channel exceptions). A
    /// channel can turn off only a connection whose "Use in" gives it to the
    /// channel.
    #[serde(default)]
    pub disabled_connections: Vec<ConnectionId>,
    /// Skills that this channel turned off (channel exceptions).
    #[serde(default)]
    pub disabled_skills: Vec<SkillId>,
    /// When the channel was archived.
    pub archived_at: Option<Timestamp>,
    /// When the channel was created.
    pub created_at: Timestamp,
    /// When the channel last changed.
    pub updated_at: Timestamp,
    /// The proactivity level. Empty means the default for the channel kind:
    /// Normal in Tutti, Quiet elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proactivity: Option<ProactivityLevel>,
    /// What a task that a proactive turn starts may do. The default is
    /// draft mode.
    #[serde(default)]
    pub proactive_tasks: ProactiveTasks,
    /// Set while the assistant of this channel runs on a fallback of the
    /// path. The hub clears it when the assistant goes back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_fallback: Option<Box<CoordinatorFallback>>,
    /// Set while the assistant of this channel cannot answer a message that
    /// waits. It is live state, not a message: the hub clears it when a turn
    /// goes out, and at every start of the hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_blocked: Option<Box<CoordinatorBlocked>>,
    /// What this channel may do with Monid.
    #[serde(default, skip_serializing_if = "is_default_monid")]
    pub monid: MonidChannelAccess,
    /// Temporary access that the user gave to this channel or its tasks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access_grants: Vec<AccessGrant>,
}

fn is_default_monid(access: &MonidChannelAccess) -> bool {
    *access == MonidChannelAccess::default()
}

/// Why the assistant of a channel cannot answer now. The app shows it as a
/// status line above the composer, only while a message of the user waits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorBlocked {
    /// The kind of cause.
    pub kind: CoordinatorBlockedKind,
    /// A full sentence that says what is wrong.
    pub reason: String,
    /// The name of the account that is signed out. It is set only for
    /// `signed_out`, when the assistant waits for a sign-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// The cause of a [`CoordinatorBlocked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorBlockedKind {
    /// Placement finds no worker or account for the assistant.
    NoPlacement,
    /// The account of the assistant is signed out.
    SignedOut,
    /// A session or a turn of the assistant did not start. The hub tries
    /// again.
    StartFailed,
}

impl Channel {
    /// The proactivity level that applies: the channel's own, or the
    /// default for its kind.
    pub fn proactivity_level(&self) -> ProactivityLevel {
        self.proactivity
            .unwrap_or_else(|| ProactivityLevel::default_for(self.kind))
    }

    /// Whether this is the Tutti channel.
    pub fn is_tutti(&self) -> bool {
        self.kind == ChannelKind::Tutti
    }

    /// The memory scope that this channel owns. The Tutti channel's
    /// memory is the global memory.
    pub fn memory_scope(&self) -> MemoryScope {
        match self.kind {
            ChannelKind::Tutti => MemoryScope::Global,
            ChannelKind::Standard => MemoryScope::Channel(self.id),
        }
    }

    /// The coordinator's model selection, with the fallback to the
    /// Tutti channel's value.
    pub fn effective_coordinator_selection<'a>(
        &'a self,
        tutti: &'a Channel,
    ) -> Option<&'a ModelSelection> {
        self.coordinator_selection
            .as_ref()
            .or(tutti.coordinator_selection.as_ref())
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

/// What kind of failure ended a turn. The worker sends it when the harness
/// says so in structured data, or when the error text is a known one. A
/// failure that nothing identifies is `Unknown`. The hub puts it in the
/// `turn_failed` notice, so a client can write a title and offer the
/// actions that fit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnErrorKind {
    /// The account reached a usage limit or a rate limit.
    RateLimit,
    /// The sign-in of the account expired, or the account is signed out.
    ExpiredSignIn,
    /// The provider is down or overloaded: a 5xx error, an overloaded
    /// error, or a connection that the provider closed. The harness says so
    /// in structured data or in a known text. After three failed tries, the
    /// fallback path moves the work.
    ProviderOutage,
    /// The worker restarted during the turn.
    WorkerRestarted,
    /// Anything else. Also what a kind from a newer peer reads as.
    #[default]
    #[serde(other)]
    Unknown,
}

impl TurnErrorKind {
    /// The name in the JSON body of a `turn_failed` notice.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RateLimit => "rate_limit",
            Self::ExpiredSignIn => "expired_sign_in",
            Self::ProviderOutage => "provider_outage",
            Self::WorkerRestarted => "worker_restarted",
            Self::Unknown => "unknown",
        }
    }

    /// The kind with this name. A name that is not known reads as
    /// `Unknown`.
    pub fn from_name(name: &str) -> Self {
        match name {
            "rate_limit" => Self::RateLimit,
            "expired_sign_in" => Self::ExpiredSignIn,
            "provider_outage" => Self::ProviderOutage,
            "worker_restarted" => Self::WorkerRestarted,
            _ => Self::Unknown,
        }
    }
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
    /// Quick replies under an assistant message. At most four.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<Suggestion>,
    /// Set on a message that a proactive turn posted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proactive: Option<ProactiveMark>,
    /// Set on a user message in a task thread that the hub copied from the
    /// timeline: the ID of the timeline message. A client shows "Show in
    /// channel" for it. The copy has the same body as the source message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_message_id: Option<MessageId>,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The main channel was called Everything. New data says `tutti`, and
    /// stored data and older peers still say `everything`.
    /// A message from before `source_message_id` has no such field, and a
    /// message without a source does not write one.
    #[test]
    fn a_message_without_a_source_reads_and_writes_without_the_field() {
        let message = Message {
            id: MessageId::new(),
            channel_id: ChannelId::new(),
            thread_root_id: None,
            author: Author::User,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            kind: MessageKind::Text,
            body: "Hi".into(),
            attachments: Vec::new(),
            in_timeline: false,
            client_message_id: None,
            reply_count: 0,
            last_reply_at: None,
            reactions: Vec::new(),
            edited_at: None,
            deleted_at: None,
            created_at: Timestamp::from_millis(1),
            suggestions: Vec::new(),
            proactive: None,
            source_message_id: None,
        };
        let mut json = serde_json::to_value(&message).unwrap();
        assert!(json.get("source_message_id").is_none(), "{json}");
        let back: Message = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, message);
        let source = MessageId::new();
        json["source_message_id"] = serde_json::json!(source);
        let with_source: Message = serde_json::from_value(json).unwrap();
        assert_eq!(with_source.source_message_id, Some(source));
    }

    #[test]
    fn the_tutti_kind_reads_the_old_name_and_writes_the_new_one() {
        let new: ChannelKind = serde_json::from_str("\"tutti\"").unwrap();
        let old: ChannelKind = serde_json::from_str("\"everything\"").unwrap();
        assert_eq!((new, old), (ChannelKind::Tutti, ChannelKind::Tutti));
        assert_eq!(
            serde_json::to_string(&ChannelKind::Tutti).unwrap(),
            "\"tutti\""
        );
        let standard: ChannelKind = serde_json::from_str("\"standard\"").unwrap();
        assert_eq!(standard, ChannelKind::Standard);
    }
}
