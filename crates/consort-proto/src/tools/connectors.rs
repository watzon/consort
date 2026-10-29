//! Arguments and results of the connector and skill tools that every
//! session can have: `list_connector_tools`, `get_connector_tool`,
//! `run_connector_tool`, `list_skills` and `read_skill`.
//!
//! The hub lists these tools only for a session whose channel can use at
//! least one connection (the connector tools) or that needs the skill tools
//! (a harness with no native skills). A connector is named by its handle,
//! for example `gmail_work`, as the "Connectors in this channel" block
//! shows it.

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{ApprovalId, ConnectorCallId};
use crate::model::{ApprovalRule, SkillContextEntry, ToolKind};

/// The most tools in one `list_connector_tools` result. A service with more
/// tools needs a `search`.
pub const CONNECTOR_TOOLS_LIST_MAX: usize = 200;
/// The most bytes of the `data` of a `run_connector_tool` result (256 KiB).
/// The hub cuts a larger result and sets `truncated`.
pub const CONNECTOR_RESULT_MAX_BYTES: usize = 256 * 1024;
/// How long `run_connector_tool` waits for the user to answer an approval
/// card before it returns `waiting_for_user`, in milliseconds (5 minutes).
pub const CONNECTOR_APPROVAL_WAIT_MS: u64 = 5 * 60 * 1_000;

/// The schema of a free JSON object argument.
pub(crate) fn any_object(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "description": "The arguments, as the tool's input schema describes them.",
    })
}

/// `list_connector_tools`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListConnectorToolsArgs {
    /// The connector handle, for example `gmail_work`.
    pub connector: String,
    /// Words to find in the tool slugs and descriptions. Use it for a
    /// service with many tools.
    #[serde(default)]
    pub search: Option<String>,
}

/// `get_connector_tool`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetConnectorToolArgs {
    /// The connector handle.
    pub connector: String,
    /// The tool slug from `list_connector_tools`, for example
    /// `GMAIL_SEND_EMAIL`.
    pub tool: String,
}

/// `run_connector_tool`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunConnectorToolArgs {
    /// The connector handle.
    pub connector: String,
    /// The tool slug.
    pub tool: String,
    /// The arguments, as `get_connector_tool` describes them.
    #[serde(default)]
    #[schemars(schema_with = "any_object")]
    pub arguments: Value,
}

/// `read_skill`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadSkillArgs {
    /// The skill name.
    pub skill: String,
    /// A file in the skill folder. Leave it out for `SKILL.md`.
    #[serde(default)]
    pub path: Option<String>,
}

/// One tool of a connector, for the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorToolInfo {
    /// The tool slug.
    pub slug: String,
    /// A short description.
    pub description: String,
    /// The tool kind.
    pub kind: ToolKind,
    /// What the hub does before it runs the tool.
    pub approval: ApprovalRule,
}

/// `list_connector_tools` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListConnectorToolsResult {
    /// The connector handle.
    pub connector: String,
    /// The service name.
    pub service_name: String,
    /// The tools, at most [`CONNECTOR_TOOLS_LIST_MAX`].
    pub tools: Vec<ConnectorToolInfo>,
    /// True when more tools match. Use `search` to find them.
    #[serde(default)]
    pub has_more: bool,
}

/// `get_connector_tool` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetConnectorToolResult {
    /// The connector handle.
    pub connector: String,
    /// The tool.
    #[serde(flatten)]
    pub tool: ConnectorToolInfo,
    /// The JSON Schema of the arguments.
    pub input_schema: Value,
}

/// `run_connector_tool` result. Serialized with a `status` field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunConnectorToolResult {
    /// The tool ran.
    Ran {
        /// The audit record.
        call_id: ConnectorCallId,
        /// Whether the service reported success.
        successful: bool,
        /// The data from the service.
        #[serde(default)]
        data: Value,
        /// The error from the service, when it failed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// True when the hub cut `data` to [`CONNECTOR_RESULT_MAX_BYTES`].
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
    },
    /// The user did not answer the approval card in time. The call runs
    /// when the user allows it, and the result arrives as a
    /// `card_answered` event in a later turn.
    WaitingForUser {
        /// The approval card.
        approval_id: ApprovalId,
        /// What to tell the user.
        message: String,
    },
    /// The user denied the call. It did not run.
    Denied {
        /// The audit record.
        call_id: ConnectorCallId,
        /// What happened.
        message: String,
    },
}

/// `list_skills` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSkillsResult {
    /// The skills of this session.
    pub skills: Vec<SkillContextEntry>,
}

/// `read_skill` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadSkillResult {
    /// The skill name.
    pub skill: String,
    /// The file.
    pub path: String,
    /// The text, at most [`crate::model::SKILL_READ_MAX_BYTES`].
    pub content: String,
    /// True when `content` is cut.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The other files of the skill, for a read of `SKILL.md`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}
