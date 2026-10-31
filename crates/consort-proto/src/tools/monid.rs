//! Arguments and results of `search_tools`, `get_monid_tool` and
//! `run_monid_tool` (ADR 0016).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::connectors::any_object;
use crate::ids::{ApprovalId, MonidCallId};

/// The most rows of one `search_tools` result.
pub const SEARCH_TOOLS_MAX: usize = 20;
/// The most bytes of the output in a `run_monid_tool` result (256 KiB). The
/// app shows the whole output.
pub const MONID_RESULT_MAX_BYTES: usize = 256 * 1024;

/// `search_tools`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchToolsArgs {
    /// What the tool must do, in a few words: "instagram profile".
    pub query: String,
}

/// One row of `search_tools`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchToolRow {
    /// `monid`, or the handle of a connector, for example `gmail_work`.
    pub source: String,
    /// The tool: the connector tool slug, or the Monid endpoint.
    pub tool: String,
    /// The Monid provider. Empty for a connector.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    /// One line about the tool.
    pub description: String,
    /// The rate, for a paid tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    /// Whether this session can use it now.
    pub access: SearchToolAccess,
}

/// Whether a session can use a tool in a `search_tools` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchToolAccess {
    /// The session can use it now.
    Available,
    /// The channel does not have it. `request_access` asks the user.
    Request,
}

/// `search_tools` result. Connectors come first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchToolsResult {
    /// The rows, at most [`SEARCH_TOOLS_MAX`].
    pub tools: Vec<SearchToolRow>,
    /// Why Monid gave no rows, when it could not answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `get_monid_tool`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GetMonidToolArgs {
    /// The provider from `search_tools`.
    pub provider: String,
    /// The endpoint from `search_tools`, for example `/text-to-speech`.
    pub endpoint: String,
}

/// `get_monid_tool` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetMonidToolResult {
    /// The provider.
    pub provider: String,
    /// The endpoint.
    pub endpoint: String,
    /// What the tool does.
    pub description: String,
    /// The rate.
    pub price: String,
    /// The schema: `body`, `queryParams` and `pathParams`.
    pub input: Value,
}

/// `run_monid_tool`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunMonidToolArgs {
    /// The provider.
    pub provider: String,
    /// The endpoint.
    pub endpoint: String,
    /// The body, as `get_monid_tool` describes it.
    #[serde(default)]
    #[schemars(schema_with = "any_object")]
    pub body: Value,
    /// The query parameters, as `get_monid_tool` describes them.
    #[serde(default)]
    #[schemars(schema_with = "any_object")]
    pub query_params: Value,
    /// The path parameters, as `get_monid_tool` describes them.
    #[serde(default)]
    #[schemars(schema_with = "any_object")]
    pub path_params: Value,
}

/// `run_monid_tool` result. Serialized with a `status` field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunMonidToolResult {
    /// The run ended.
    Ran {
        /// The call.
        call_id: MonidCallId,
        /// Whether the tool gave a result.
        successful: bool,
        /// What Monid charged: "$0.0035".
        cost: String,
        /// The output.
        #[serde(default)]
        data: Value,
        /// What went wrong.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// True when the hub cut `data` to [`MONID_RESULT_MAX_BYTES`].
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
        /// The files that the hub saved, as channel file paths.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
    },
    /// The user did not answer the card in time. The call runs when the user
    /// allows it, and the result arrives in a later turn.
    WaitingForUser {
        /// The card.
        approval_id: ApprovalId,
        /// What to tell the user.
        message: String,
    },
    /// The run takes long. The result arrives in a later turn.
    StillRunning {
        /// The call.
        call_id: MonidCallId,
        /// What to tell the user.
        message: String,
    },
    /// The call did not run and cost nothing.
    Denied {
        /// The call.
        call_id: MonidCallId,
        /// Why, and where the user can change it.
        message: String,
    },
}
