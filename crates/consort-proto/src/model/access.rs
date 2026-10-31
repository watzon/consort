//! Access requests: a session asks for a connection or for Monid that its
//! channel does not have (ADR 0017).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{AccessGrantId, ConnectionId, TaskId};
use crate::time::Timestamp;

/// The option of an access card that gives temporary access.
pub const ACCESS_CARD_TEMPORARY: &str = "allow_temporary";
/// The option of an access card that gives lasting access.
pub const ACCESS_CARD_ALWAYS: &str = "allow_always";
/// How long temporary access lasts for a coordinator: 24 hours. In a task it
/// lasts as long as the task.
pub const TEMPORARY_ACCESS_MS: i64 = 24 * 60 * 60 * 1_000;

/// How long access lasts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessDuration {
    /// Until the task ends, or for 24 hours outside a task.
    #[default]
    Temporary,
    /// From now on, in this channel.
    Permanent,
}

/// What a session asks for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccessTarget {
    /// One connection.
    Connection {
        /// The connection.
        connection_id: ConnectionId,
        /// Its name, for example "Gmail Work".
        name: String,
    },
    /// All of Monid, apart from the tools that the channel blocks.
    Monid,
    /// A Monid provider or one Monid tool.
    MonidTool {
        /// The provider.
        provider: String,
        /// The endpoint. `None` means every tool of the provider.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
        /// The name to show.
        name: String,
    },
}

impl AccessTarget {
    /// The name to show.
    pub fn label(&self) -> &str {
        match self {
            Self::Connection { name, .. } | Self::MonidTool { name, .. } => name,
            Self::Monid => "Monid",
        }
    }

    /// A stable text for this target, to compare requests.
    pub fn key(&self) -> String {
        match self {
            Self::Connection { connection_id, .. } => format!("connection:{connection_id}"),
            Self::Monid => "monid".to_owned(),
            Self::MonidTool {
                provider, endpoint, ..
            } => format!("monid:{provider}:{}", endpoint.as_deref().unwrap_or("*")),
        }
    }

    /// Whether this target is a part of Monid.
    pub fn is_monid(&self) -> bool {
        matches!(self, Self::Monid | Self::MonidTool { .. })
    }
}

/// Temporary access that the user gave. It is on the channel, so a client
/// sees it and the user can end it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccessGrant {
    /// The ID.
    pub id: AccessGrantId,
    /// What it gives.
    pub target: AccessTarget,
    /// Set when it is for one task. Only sessions of that task have it, and
    /// it ends with the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    /// When it ends, for access outside a task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// When the user gave it.
    pub granted_at: Timestamp,
}

impl AccessGrant {
    /// Whether a session of this task, or a coordinator with `None`, has it
    /// at this time. Channel-wide access reaches tasks too.
    pub fn applies(&self, task_id: Option<TaskId>, now: Timestamp) -> bool {
        self.expires_at.is_none_or(|end| end > now)
            && (self.task_id.is_none() || self.task_id == task_id)
    }
}

/// The card of an access request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRequestCard {
    /// What the session asks for.
    pub target: AccessTarget,
    /// The length that the session asked for. Its option is first.
    pub requested: AccessDuration,
    /// "Give this task access to Gmail Work".
    pub title: String,
    /// Why the session asks, in its words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What each answer does.
    pub effect: String,
    /// The label of the temporary option: "Allow for this task" or "Allow
    /// for 24 hours".
    pub temporary_label: String,
}
