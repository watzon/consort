//! Client requests for Monid: the key, the settings, the channel access and
//! the record of calls (ADR 0016).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{AccessGrantId, ChannelId, MonidCallId};
use crate::model::{MonidCall, MonidChannelAccess, MonidSettings, MonidStatus, Redacted};

/// The most records of one `monid.calls` page.
pub const MONID_CALLS_PAGE_MAX: u32 = 100;

/// `monid.set_key`: store the Monid key on the hub. The hub checks the key
/// with Monid, encrypts it at rest, and never sends it back.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidSetKey {
    /// The key from app.monid.ai.
    pub api_key: String,
    /// The settings to start with, from the Add Monid dialog. `None` keeps
    /// the settings that the hub has.
    #[serde(default)]
    pub settings: Option<MonidSettings>,
}

impl fmt::Debug for MonidSetKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MonidSetKey")
            .field("api_key", &Redacted)
            .field("settings", &self.settings)
            .finish()
    }
}

/// `monid.remove_key`: delete the key. The record of calls stays.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidRemoveKey {}

/// `monid.status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidGetStatus {}

/// The Monid state of the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidStatusResult {
    /// The state.
    pub status: MonidStatus,
}

/// `monid.update_settings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidUpdateSettings {
    /// The whole new settings.
    pub settings: MonidSettings,
}

/// `monid.search`: find tools to add to a channel's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidCatalogSearch {
    /// What to find.
    pub query: String,
}

/// One tool in a `monid.search` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidCatalogTool {
    /// The provider.
    pub provider: String,
    /// The provider's name to show.
    pub provider_name: String,
    /// The endpoint.
    pub endpoint: String,
    /// The tool's name to show.
    pub name: String,
    /// The rate.
    pub rate: String,
}

/// `monid.search` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidCatalogSearchResult {
    /// The tools.
    pub tools: Vec<MonidCatalogTool>,
}

/// `monid.call`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidCallGet {
    /// The call.
    pub id: MonidCallId,
}

/// One call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonidCallResult {
    /// The call.
    pub call: MonidCall,
}

/// `monid.calls`: the channel's calls, the newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidCallList {
    /// The channel.
    pub channel_id: ChannelId,
    /// How many, at most [`MONID_CALLS_PAGE_MAX`].
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `monid.calls` result. The calls have no input or output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonidCallListResult {
    /// The calls.
    pub calls: Vec<MonidCall>,
}

/// `monid.stop`: ask Monid to stop a run. The call ends when Monid says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonidCallStop {
    /// The call.
    pub id: MonidCallId,
}

/// `channel.set_monid`: change what a channel may do with Monid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSetMonid {
    /// The channel.
    pub channel_id: ChannelId,
    /// The whole new access.
    pub access: MonidChannelAccess,
}

/// `channel.end_access`: end temporary access now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelEndAccess {
    /// The channel.
    pub channel_id: ChannelId,
    /// The grant.
    pub grant_id: AccessGrantId,
}
