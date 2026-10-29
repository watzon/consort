//! Client requests for connectors: the Composio key, the catalog, the
//! connections, their audit log, and the channel exceptions.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{ApprovalId, ChannelId, ConnectionId, ConnectorCallId, SkillId};
use crate::model::{
    AuthSchemeKind, ComposioStatus, Connection, ConnectorCall, ConnectorCategory, ConnectorService,
    ConnectorToolSummary, Redacted, SecretEnv, ToolApprovals, UseIn,
};
use crate::time::Timestamp;

/// The most results of one catalog page.
pub const CATALOG_PAGE_MAX: u32 = 100;
/// The most records of one `connection.calls` page.
pub const CONNECTOR_CALLS_PAGE_MAX: u32 = 200;

// Composio settings -------------------------------------------------------

/// `composio.set_key`: store the Composio project key on the hub. The hub
/// checks the key with Composio, encrypts it at rest, and never sends it
/// back.
///
/// `Debug` does not show the key.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposioSetKey {
    /// The key.
    pub api_key: String,
}

impl fmt::Debug for ComposioSetKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComposioSetKey")
            .field("api_key", &Redacted)
            .finish()
    }
}

/// `composio.remove_key`: delete the key. Connections stay, but their tools
/// fail with `composio_not_configured` until a key is back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposioRemoveKey {}

/// `composio.status`: the Composio settings, with the plan and the usage
/// when Composio tells. Never the key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposioGetStatus {}

/// The response to the `composio.*` requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposioStatusResult {
    /// The settings.
    pub status: ComposioStatus,
}

// Catalog ------------------------------------------------------------------

/// `connector_catalog.search`: a page of services from the hub's cached
/// catalog. With no search and no category, the most used services come
/// first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCatalogSearch {
    /// Words to find in the name, the slug and the description.
    #[serde(default)]
    pub search: Option<String>,
    /// Only services in this category slug.
    #[serde(default)]
    pub category: Option<String>,
    /// The `next_cursor` of the page before.
    #[serde(default)]
    pub cursor: Option<String>,
    /// The most services. The default is 50 and the limit is
    /// [`CATALOG_PAGE_MAX`].
    #[serde(default)]
    pub limit: Option<u32>,
}

/// The response to `connector_catalog.search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCatalogSearchResult {
    /// The services.
    pub services: Vec<ConnectorService>,
    /// The cursor of the next page. Empty on the last page.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// `connector_catalog.categories`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCatalogCategories {}

/// The response to `connector_catalog.categories`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCatalogCategoriesResult {
    /// The categories, in the order to show.
    pub categories: Vec<ConnectorCategory>,
}

/// `connector_catalog.service`: one service with its tools, for the connect
/// dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCatalogService {
    /// The toolkit slug.
    pub service: String,
}

/// The response to `connector_catalog.service`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorServiceDetail {
    /// The service.
    pub service: ConnectorService,
    /// Its tools, with their kinds.
    pub tools: Vec<ConnectorToolSummary>,
}

// Connections ---------------------------------------------------------------

/// `connection.connect`: connect a service. The hub makes the connection in
/// `connecting`, asks Composio for a sign-in link, and answers with the
/// attempt. The app opens `redirect_url` in the system browser. The hub
/// polls until the account is active, for at most 10 minutes, and each
/// change is a feed upsert of the connection.
///
/// For a scheme with no browser step, such as an API key, `credentials`
/// holds the form values. They go to Composio once. Consort does not store
/// them. `Debug` shows only their names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionConnect {
    /// The toolkit slug, for example `gmail`.
    pub service: String,
    /// The connection name, for example "Gmail Work".
    pub name: String,
    /// Where it can be used. The default is every channel.
    #[serde(default)]
    pub use_in: UseIn,
    /// The approval rules. The default runs reads without asking and asks
    /// for every change and delete.
    #[serde(default)]
    pub approvals: ToolApprovals,
    /// The scheme to use. Empty means the service's preferred scheme.
    #[serde(default)]
    pub auth_scheme: Option<AuthSchemeKind>,
    /// The form values for a scheme with no browser step.
    #[serde(default)]
    pub credentials: SecretEnv,
    /// The connect card that this answers, when the user continued from a
    /// card. The hub marks the card allowed and links the connection.
    #[serde(default)]
    pub approval_id: Option<ApprovalId>,
}

/// The response to `connection.connect` and `connection.reconnect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectAttempt {
    /// The connection, in `connecting`, or `active` when no browser step
    /// was needed.
    pub connection: Connection,
    /// The page to open in the system browser. Empty when there is no
    /// browser step.
    #[serde(default)]
    pub redirect_url: Option<String>,
    /// When the hub stops waiting.
    #[serde(default)]
    pub expires_at: Option<Timestamp>,
}

/// `connection.update`. Fields that are left out do not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionUpdate {
    /// The connection.
    pub connection_id: ConnectionId,
    /// The new name. The handle changes with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The new scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_in: Option<UseIn>,
    /// The new approval rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approvals: Option<ToolApprovals>,
}

/// The response to requests that return one connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionResult {
    /// The connection after the change.
    pub connection: Connection,
}

/// `connection.disconnect`: delete the account at Composio and the
/// connection on the hub. The audit log stays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionDisconnect {
    /// The connection.
    pub connection_id: ConnectionId,
}

/// `connection.reconnect`: sign in again, for a connection that needs a new
/// sign-in or that failed. The connection keeps its ID, name, scope and
/// rules. `Debug` shows only the names in `credentials`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionReconnect {
    /// The connection.
    pub connection_id: ConnectionId,
    /// The form values for a scheme with no browser step.
    #[serde(default)]
    pub credentials: SecretEnv,
}

/// `connection.calls`: a page of the audit log of one connection, newest
/// first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionCalls {
    /// The connection.
    pub connection_id: ConnectionId,
    /// Only calls older than this one.
    #[serde(default)]
    pub before: Option<ConnectorCallId>,
    /// The most calls. The default is 50 and the limit is
    /// [`CONNECTOR_CALLS_PAGE_MAX`].
    #[serde(default)]
    pub limit: Option<u32>,
}

/// The response to `connection.calls`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionCallsResult {
    /// The calls, newest first.
    pub calls: Vec<ConnectorCall>,
    /// Whether older calls exist.
    pub has_more: bool,
}

// Channel exceptions ------------------------------------------------------

/// `channel.set_connection_enabled`: turn one connection off or on again in
/// one channel. Turning on only removes the exception: a connection whose
/// "Use in" does not give it to the channel stays out, and the hub answers
/// `conflict`. One connection per request, so two clients do not undo each
/// other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSetConnectionEnabled {
    /// The channel.
    pub channel_id: ChannelId,
    /// The connection.
    pub connection_id: ConnectionId,
    /// False adds the exception. True removes it.
    pub enabled: bool,
}

/// `channel.set_skill_enabled`: turn one skill off or on again in one
/// channel, with the same rules as `channel.set_connection_enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSetSkillEnabled {
    /// The channel.
    pub channel_id: ChannelId,
    /// The skill.
    pub skill_id: SkillId,
    /// False adds the exception. True removes it.
    pub enabled: bool,
}
