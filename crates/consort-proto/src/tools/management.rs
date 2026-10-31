//! Arguments and results of the management tools: the tools with which the
//! Tutti assistant manages everything, and a channel's assistant
//! manages its own channel.
//!
//! The hub enforces the scope and the risk rule. A change that adds
//! something new and narrow, or that makes access smaller or approvals
//! stricter, runs at once. A change that deletes or archives, widens
//! access, makes approvals looser, turns on a skill with scripts, changes
//! worker or channel tags, waits for the user on a management approval
//! card. Every such tool answers with a [`ManagementResult`], whose `status` says what happened.
//!
//! A connection is named by its handle or its ID, and a skill by its name
//! or its ID.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{AccountId, ApprovalId, ChannelId, ConnectionId, SkillId, WorkerId};
use crate::model::{
    AccessDuration, Connection, ConnectorService, ModelSelection, Skill, SkillDirectoryEntry,
    ToolApprovals, UseIn,
};

/// `create_channel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateChannelArgs {
    /// The channel name.
    pub name: String,
    /// The one-line purpose.
    pub purpose: String,
    /// The name of a sidebar group. The hub uses the group with this name,
    /// or makes it.
    #[serde(default)]
    pub group: Option<String>,
    /// The channel's tags. Work from the channel runs only on workers with
    /// every tag.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// The model of the channel's assistant. Leave it out for Tutti's.
    #[serde(default)]
    pub model: Option<ModelSelection>,
    /// The default model for tasks.
    #[serde(default)]
    pub task_model: Option<ModelSelection>,
}

/// `update_channel`. Fields that you leave out do not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateChannelArgs {
    /// The channel.
    pub channel: ChannelId,
    /// The new name.
    #[serde(default)]
    pub name: Option<String>,
    /// The new purpose.
    #[serde(default)]
    pub purpose: Option<String>,
    /// The new instructions, in markdown.
    #[serde(default)]
    pub instructions: Option<String>,
    /// The name of the new sidebar group.
    #[serde(default)]
    pub group: Option<String>,
    /// The new tags. A change to the tags asks the user first.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// The new model of the channel's assistant.
    #[serde(default)]
    pub model: Option<ModelSelection>,
    /// The new default model for tasks.
    #[serde(default)]
    pub task_model: Option<ModelSelection>,
    /// The git remote URLs of the channel's repos, in order.
    #[serde(default)]
    pub repos: Option<Vec<String>>,
}

/// `archive_channel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveChannelArgs {
    /// The channel.
    pub channel: ChannelId,
}

/// `set_default_model`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetDefaultModelArgs {
    /// The account label, model and options for every channel that has no
    /// model of its own.
    pub model: ModelSelection,
}

/// `set_worker_tags`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetWorkerTagsArgs {
    /// The worker.
    pub worker: WorkerId,
    /// Every user tag of the worker after the change.
    pub tags: Vec<String>,
}

/// `install_tool`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallToolArgs {
    /// The worker.
    pub worker: WorkerId,
    /// The managed tool: `node`, `claude-code`, `claude-agent-acp`, `codex`,
    /// `opencode`, `gemini-cli` or `acp:<registry id>`.
    pub tool: String,
}

/// `add_account`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddAccountArgs {
    /// The worker.
    pub worker: WorkerId,
    /// The provider: `claude`, `codex`, `opencode`, `gemini`, or
    /// `acp:<registry id>`.
    pub provider: String,
    /// The account name, for example "Claude Work".
    pub name: String,
}

/// `start_sign_in`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StartSignInArgs {
    /// The worker.
    pub worker: WorkerId,
    /// The account.
    pub account: AccountId,
    /// The kind of a sign-in method of the account, for example
    /// `claude_plan` or `chatgpt_device`. Leave it out to let the user
    /// choose on the card.
    #[serde(default)]
    pub method: Option<String>,
}

/// `search_connector_catalog`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchConnectorCatalogArgs {
    /// Words to find, for example "email".
    pub search: String,
    /// Only services in this category slug.
    #[serde(default)]
    pub category: Option<String>,
}

/// `connect_service`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectServiceArgs {
    /// The service slug from `search_connector_catalog`, for example
    /// `gmail`.
    pub service: String,
    /// The connection name, for example "Gmail Work".
    pub name: String,
    /// Where it can be used. Tutti's default is every channel. A
    /// channel's default is the channels with its tags, or the channel
    /// alone when it has no tags.
    #[serde(default)]
    pub use_in: Option<UseIn>,
    /// The approval rules. The default runs reads without asking and asks
    /// for every change and delete.
    #[serde(default)]
    pub approvals: Option<ToolApprovals>,
}

/// `update_connection`. Fields that you leave out do not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateConnectionArgs {
    /// The connection handle or ID.
    pub connection: String,
    /// The new name.
    #[serde(default)]
    pub name: Option<String>,
    /// The new scope. A wider scope asks the user first.
    #[serde(default)]
    pub use_in: Option<UseIn>,
    /// The new approval rules. A looser rule asks the user first.
    #[serde(default)]
    pub approvals: Option<ToolApprovals>,
}

/// `disconnect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DisconnectArgs {
    /// The connection handle or ID.
    pub connection: String,
}

/// `search_skills`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchSkillsArgs {
    /// Words to find.
    pub search: String,
    /// Only this directory: `smithery`, `anthropic`, or the URL of a
    /// marketplace that the user added.
    #[serde(default)]
    pub source: Option<String>,
}

/// `add_skill`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddSkillArgs {
    /// A git URL, `owner/repo` for GitHub, or an entry ID from
    /// `search_skills`.
    pub source: String,
    /// The folder of the skill in the repo.
    #[serde(default)]
    pub path: Option<String>,
    /// Where it can be used, with the same defaults as `connect_service`.
    #[serde(default)]
    pub use_in: Option<UseIn>,
}

/// `update_skill`. Fields that you leave out do not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateSkillArgs {
    /// The skill name or ID.
    pub skill: String,
    /// Turn it on or off. Turning on a skill with scripts asks the user
    /// first.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// The new scope. A wider scope asks the user first.
    #[serde(default)]
    pub use_in: Option<UseIn>,
    /// True takes the new version that waits for review.
    #[serde(default)]
    pub accept_new_version: Option<bool>,
}

/// `remove_skill`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoveSkillArgs {
    /// The skill name or ID.
    pub skill: String,
}

/// `update_this_channel`. Fields that you leave out do not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateThisChannelArgs {
    /// The new purpose.
    #[serde(default)]
    pub purpose: Option<String>,
    /// The new instructions, in markdown.
    #[serde(default)]
    pub instructions: Option<String>,
    /// The new model of this channel's assistant.
    #[serde(default)]
    pub model: Option<ModelSelection>,
    /// The new default model for tasks.
    #[serde(default)]
    pub task_model: Option<ModelSelection>,
    /// The git remote URLs of the channel's repos, in order.
    #[serde(default)]
    pub repos: Option<Vec<String>>,
}

/// `set_channel_tags`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetChannelTagsArgs {
    /// Every tag of this channel after the change.
    pub tags: Vec<String>,
}

/// `set_channel_connector`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetChannelConnectorArgs {
    /// The connection handle or ID. Its "Use in" must give it to this
    /// channel.
    pub connection: String,
    /// False turns it off in this channel. True turns it on again.
    pub enabled: bool,
}

/// `set_channel_skill`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetChannelSkillArgs {
    /// The skill name or ID. Its "Use in" must give it to this channel.
    pub skill: String,
    /// False turns it off in this channel. True turns it on again.
    pub enabled: bool,
}

/// `request_access`. Give exactly one of `connection` and `skill`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestAccessArgs {
    /// The connection handle or ID.
    #[serde(default)]
    pub connection: Option<String>,
    /// The skill name or ID. A skill is always lasting access.
    #[serde(default)]
    pub skill: Option<String>,
    /// Ask for Monid. Leave out the provider to ask for all of Monid. Name a
    /// provider, or a provider and an endpoint, to ask for less.
    #[serde(default)]
    pub monid: Option<MonidAccessArgs>,
    /// `temporary` lasts until your task ends, or 24 hours outside a task.
    /// `permanent` lasts from now on in this channel. Ask for temporary
    /// unless this channel will need it again and again.
    #[serde(default)]
    pub duration: AccessDuration,
    /// One short sentence for the user: why you need it.
    #[serde(default)]
    pub reason: Option<String>,
}

/// The Monid part of `request_access`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MonidAccessArgs {
    /// The provider from `search_tools`.
    #[serde(default)]
    pub provider: Option<String>,
    /// The endpoint from `search_tools`.
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// `request_access` result for a connection or Monid. Serialized with a
/// `status` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RequestAccessResult {
    /// The user gave access. It works from now on in this session.
    Granted {
        /// What the session can do now.
        message: String,
    },
    /// The user has not answered. The answer arrives in a later turn.
    WaitingForUser {
        /// The card.
        approval_id: ApprovalId,
        /// What to tell the user.
        message: String,
    },
    /// The user said no.
    Denied {
        /// What to do next.
        message: String,
    },
    /// The hub posted no card.
    Refused {
        /// Why.
        message: String,
    },
}

/// What a management tool did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    /// The change is made. The hub posted a short activity line.
    Applied,
    /// A card waits for the user. The answer arrives as a `card_answered`
    /// event in a later turn.
    WaitingForUser,
    /// The hub refused the change. `summary` says why.
    Refused,
}

/// The result of every management tool that changes something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagementResult {
    /// What happened.
    pub status: ChangeStatus,
    /// One or two sentences for the model: what changed, what waits, or
    /// why the hub refused.
    pub summary: String,
    /// The card, when `status` is `waiting_for_user`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<ApprovalId>,
    /// The channel that the tool made or changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<ChannelId>,
    /// The connection that the tool made or changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<ConnectionId>,
    /// The skill that the tool made or changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<SkillId>,
    /// The account that the tool made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<AccountId>,
}

impl ManagementResult {
    fn new(status: ChangeStatus, summary: impl Into<String>) -> Self {
        Self {
            status,
            summary: summary.into(),
            approval_id: None,
            channel_id: None,
            connection_id: None,
            skill_id: None,
            account_id: None,
        }
    }

    /// The change is made.
    pub fn applied(summary: impl Into<String>) -> Self {
        Self::new(ChangeStatus::Applied, summary)
    }

    /// A card waits for the user.
    pub fn waiting_for_user(approval_id: ApprovalId, summary: impl Into<String>) -> Self {
        Self {
            approval_id: Some(approval_id),
            ..Self::new(ChangeStatus::WaitingForUser, summary)
        }
    }

    /// The hub refused the change.
    pub fn refused(reason: impl Into<String>) -> Self {
        Self::new(ChangeStatus::Refused, reason)
    }
}

/// `search_connector_catalog` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchConnectorCatalogResult {
    /// The services, best first.
    pub services: Vec<ConnectorService>,
}

/// `list_connections` result. The Tutti assistant sees every
/// connection. A channel's assistant sees the connections that its channel
/// can use, and the ones that the channel turned off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListConnectionsResult {
    /// The connections.
    pub connections: Vec<Connection>,
    /// For a channel's assistant: the connections in `connections` that
    /// this channel turned off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub off_in_this_channel: Vec<ConnectionId>,
    /// The connections that this channel does not have. Only their names:
    /// `request_access` asks the user for one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub can_request: Vec<RequestableConnection>,
}

/// A connection that a channel can ask for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestableConnection {
    /// The handle.
    pub handle: String,
    /// The name.
    pub name: String,
    /// The service name.
    pub service_name: String,
}

/// `search_skills` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchSkillsResult {
    /// The entries, best first.
    pub entries: Vec<SkillDirectoryEntry>,
}

/// `list_installed_skills` result, with the same scope rule as
/// `list_connections`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListInstalledSkillsResult {
    /// The skills.
    pub skills: Vec<Skill>,
    /// For a channel's assistant: the skills in `skills` that this channel
    /// turned off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub off_in_this_channel: Vec<SkillId>,
}
