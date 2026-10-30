//! Connectors: Composio services, the connections to them, and the audit log
//! of connector tool calls.
//!
//! A **connection** is one connected account of one Composio service, such
//! as "Gmail Work". The hub keeps every connection. A channel can use a
//! connection when the connection's [`UseIn`] scope matches the channel and
//! the channel did not turn it off (a channel exception). The assistant sees
//! each connection that its channel can use by its handle, for example
//! `gmail_work`.

use std::collections::BTreeSet;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::common::{Sha256Hash, ValidationError};
use super::conversation::Channel;
use super::tag::normalize_tags;
use crate::ids::{ChannelId, ConnectionId, ConnectorCallId, SessionId, TaskId};
use crate::time::Timestamp;

/// The most characters in a connection name.
pub const CONNECTION_NAME_MAX_CHARS: usize = 80;
/// The most characters in a connection handle.
pub const CONNECTION_HANDLE_MAX_CHARS: usize = 40;
/// How long the hub waits for a new connection to become active, in
/// milliseconds (10 minutes). Then the connection fails.
pub const CONNECT_ATTEMPT_TIMEOUT_MS: i64 = 10 * 60 * 1_000;
/// How long the hub keeps the catalog before it reads it again, in
/// milliseconds (one day).
pub const CATALOG_REFRESH_MS: i64 = 24 * 60 * 60 * 1_000;

/// Where a connection or a skill can be used. Serialized with a `kind`
/// field.
///
/// The Tutti channel matches only [`UseIn::EveryChannel`], unless it
/// has tags of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UseIn {
    /// Every channel, Tutti too.
    #[default]
    EveryChannel,
    /// The channels that have every one of these tags.
    Tags {
        /// One or more tags. A channel must have all of them.
        tags: Vec<String>,
    },
    /// Only these channels.
    Channels {
        /// One or more channels.
        channel_ids: Vec<ChannelId>,
    },
}

impl UseIn {
    /// Whether a channel with these properties is in the scope.
    pub fn matches(&self, channel_id: ChannelId, is_tutti: bool, channel_tags: &[String]) -> bool {
        match self {
            Self::EveryChannel => true,
            Self::Tags { tags } => {
                if is_tutti && channel_tags.is_empty() {
                    return false;
                }
                !tags.is_empty() && tags.iter().all(|t| channel_tags.contains(t))
            }
            Self::Channels { channel_ids } => channel_ids.contains(&channel_id),
        }
    }

    /// Whether `channel` is in the scope.
    pub fn matches_channel(&self, channel: &Channel) -> bool {
        self.matches(channel.id, channel.is_tutti(), &channel.tags)
    }

    /// Trims and checks the tags, drops repeats and repeated channels, and
    /// refuses an empty list.
    pub fn normalized(self) -> Result<Self, ValidationError> {
        match self {
            Self::EveryChannel => Ok(Self::EveryChannel),
            Self::Tags { tags } => {
                let tags = normalize_tags(tags)
                    .map_err(|e| ValidationError::new("use_in.tags", e.to_string()))?;
                if tags.is_empty() {
                    return Err(ValidationError::new(
                        "use_in.tags",
                        "is empty. Use every_channel for every channel",
                    ));
                }
                Ok(Self::Tags { tags })
            }
            Self::Channels { channel_ids } => {
                let mut seen = BTreeSet::new();
                let channel_ids: Vec<ChannelId> = channel_ids
                    .into_iter()
                    .filter(|c| seen.insert(*c))
                    .collect();
                if channel_ids.is_empty() {
                    return Err(ValidationError::new("use_in.channel_ids", "is empty"));
                }
                Ok(Self::Channels { channel_ids })
            }
        }
    }

    /// The channels of `channels` that `self` adds when it replaces `old`.
    /// A management tool asks first when this list is not empty.
    pub fn added_channels<'a>(&self, old: &UseIn, channels: &'a [Channel]) -> Vec<&'a Channel> {
        channels
            .iter()
            .filter(|c| c.archived_at.is_none())
            .filter(|c| self.matches_channel(c) && !old.matches_channel(c))
            .collect()
    }
}

/// The kind of a connector tool. Composio's tool hints decide it (see
/// [`ToolKind::from_hints`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Reads only.
    Read,
    /// Creates or changes something.
    Change,
    /// Deletes something.
    Delete,
}

impl ToolKind {
    /// The kind from Composio's `readOnlyHint`, `createHint`, `updateHint`
    /// and `destructiveHint` tags. `destructiveHint` wins. A tool with only
    /// `readOnlyHint` reads. Every other tool, also one with no hint,
    /// changes, so an unknown tool never runs as a read.
    pub fn from_hints(read_only: bool, create: bool, update: bool, destructive: bool) -> Self {
        if destructive {
            Self::Delete
        } else if read_only && !create && !update {
            Self::Read
        } else {
            Self::Change
        }
    }

    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Change => "change",
            Self::Delete => "delete",
        }
    }
}

/// What the hub does before it runs a connector tool of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRule {
    /// Run it.
    RunWithoutAsking,
    /// Ask the first time in a task, then run it for the rest of the task.
    /// In a coordinator session, it asks the first time in the session.
    AskOncePerTask,
    /// Ask every time.
    AskEveryTime,
}

impl ApprovalRule {
    /// 0 for the loosest rule, 2 for the strictest.
    pub fn strictness(self) -> u8 {
        match self {
            Self::RunWithoutAsking => 0,
            Self::AskOncePerTask => 1,
            Self::AskEveryTime => 2,
        }
    }
}

/// The approval rule for each tool kind of one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ToolApprovals {
    /// Tools that read. The default is to run without asking.
    pub read: ApprovalRule,
    /// Tools that create or change. The default is to ask every time.
    pub change: ApprovalRule,
    /// Tools that delete. The default is to ask every time.
    pub delete: ApprovalRule,
}

impl Default for ToolApprovals {
    fn default() -> Self {
        Self {
            read: ApprovalRule::RunWithoutAsking,
            change: ApprovalRule::AskEveryTime,
            delete: ApprovalRule::AskEveryTime,
        }
    }
}

impl ToolApprovals {
    /// The rule for one tool kind.
    pub fn rule_for(&self, kind: ToolKind) -> ApprovalRule {
        match kind {
            ToolKind::Read => self.read,
            ToolKind::Change => self.change,
            ToolKind::Delete => self.delete,
        }
    }

    /// Whether any rule of `self` is looser than the same rule of `old`. A
    /// management tool asks first when this is true.
    pub fn is_looser_than(&self, old: &ToolApprovals) -> bool {
        [ToolKind::Read, ToolKind::Change, ToolKind::Delete]
            .into_iter()
            .any(|k| self.rule_for(k).strictness() < old.rule_for(k).strictness())
    }
}

/// Where a connection is. Serialized with a `kind` field.
///
/// A new connection starts in `connecting`. The hub polls Composio until the
/// account is active, for at most [`CONNECT_ATTEMPT_TIMEOUT_MS`]. Each change
/// is a feed upsert of the connection, so the connect dialog and the connect
/// card follow the connection entity. There is no separate attempt entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionStatus {
    /// The user must finish the sign-in.
    Connecting {
        /// The sign-in page to open in the system browser. Empty for a
        /// scheme with no browser step, such as an API key.
        redirect_url: Option<String>,
        /// When the hub stops waiting and sets `failed`.
        expires_at: Timestamp,
    },
    /// Ready to use.
    Active,
    /// The sign-in expired or was revoked. The user must reconnect.
    NeedsSignIn {
        /// What Composio said, when it said something.
        message: Option<String>,
    },
    /// The connect flow failed or timed out.
    Failed {
        /// What went wrong and what to do next.
        message: String,
    },
}

impl ConnectionStatus {
    /// Whether tools of the connection can run.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }
}

/// One connected account of one Composio service. A feed entity with no
/// channel (`connection`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    /// The ID.
    pub id: ConnectionId,
    /// The name, for example "Gmail Work".
    pub name: String,
    /// The assistant's name for it, a unique slug of the name, for example
    /// `gmail_work`. See [`connection_handle`].
    pub handle: String,
    /// The Composio toolkit slug, for example `gmail`.
    pub service: String,
    /// The service name to show, for example "Gmail".
    pub service_name: String,
    /// The service logo, from the hub's logo cache.
    pub logo_url: Option<String>,
    /// The account identity, such as an email address or a login, when the
    /// service tells.
    pub identity: Option<String>,
    /// Where it can be used.
    pub use_in: UseIn,
    /// The approval rule for each tool kind.
    pub approvals: ToolApprovals,
    /// Where it is.
    pub status: ConnectionStatus,
    /// When a tool of it last ran.
    #[serde(default)]
    pub last_used_at: Option<Timestamp>,
    /// When it was made.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
}

impl Connection {
    /// Whether a session in `channel` can use this connection: its scope
    /// matches and the channel did not turn it off.
    pub fn is_available_in(&self, channel: &Channel) -> bool {
        self.use_in.matches_channel(channel) && !channel.disabled_connections.contains(&self.id)
    }
}

/// The handle of a connection name: lowercase ASCII letters and digits, with
/// one `_` between words, at most [`CONNECTION_HANDLE_MAX_CHARS`]
/// characters. "Gmail Work" gives `gmail_work`. Every other character,
/// also a letter with an accent, splits words. A name with no ASCII letter
/// or digit gives `connection`.
pub fn connection_handle(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    if out.is_empty() {
        out.push_str("connection");
    }
    if out.len() > CONNECTION_HANDLE_MAX_CHARS {
        out.truncate(CONNECTION_HANDLE_MAX_CHARS);
        while out.ends_with('_') {
            out.pop();
        }
    }
    out
}

/// A handle for `name` that is not in `taken`: [`connection_handle`], then
/// `_2`, `_3` and so on.
pub fn unique_connection_handle<'a>(
    name: &str,
    taken: impl IntoIterator<Item = &'a str>,
) -> String {
    let taken: BTreeSet<&str> = taken.into_iter().collect();
    let base = connection_handle(name);
    if !taken.contains(base.as_str()) {
        return base;
    }
    let mut n = 2u32;
    loop {
        let suffix = format!("_{n}");
        let mut stem = base.clone();
        stem.truncate(CONNECTION_HANDLE_MAX_CHARS - suffix.len());
        while stem.ends_with('_') {
            stem.pop();
        }
        let candidate = format!("{stem}{suffix}");
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
        n += 1;
    }
}

/// Checks a connection name: not empty after a trim, at most
/// [`CONNECTION_NAME_MAX_CHARS`] characters, no control character.
pub fn validate_connection_name(name: &str) -> Result<(), ValidationError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ValidationError::new("name", "is empty"));
    }
    if name.chars().count() > CONNECTION_NAME_MAX_CHARS {
        return Err(ValidationError::new(
            "name",
            format!("is longer than {CONNECTION_NAME_MAX_CHARS} characters"),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(ValidationError::new("name", "contains a control character"));
    }
    Ok(())
}

/// The kind of an auth scheme of a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthSchemeKind {
    /// OAuth 2. The user signs in in the browser.
    Oauth2,
    /// OAuth 1. The user signs in in the browser.
    Oauth1,
    /// An API key in a form.
    ApiKey,
    /// A bearer token in a form.
    BearerToken,
    /// A user name and a password in a form.
    Basic,
    /// No credentials.
    NoAuth,
    /// A scheme that this version does not know.
    #[serde(other)]
    Other,
}

impl AuthSchemeKind {
    /// Whether the user signs in in the browser.
    pub fn uses_browser(self) -> bool {
        matches!(self, Self::Oauth2 | Self::Oauth1)
    }
}

/// One field of a credential form for a scheme with no browser step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialField {
    /// The name that goes to Composio, for example `api_key`.
    pub name: String,
    /// The label to show.
    pub label: String,
    /// Help text, when Composio gives it.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether the value is a secret. The app hides it as the user types.
    #[serde(default)]
    pub secret: bool,
    /// Whether the user must fill it.
    #[serde(default)]
    pub required: bool,
}

/// One way to connect a service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthScheme {
    /// The kind.
    pub kind: AuthSchemeKind,
    /// True when Composio manages the OAuth app, so the user needs no app
    /// of their own.
    #[serde(default)]
    pub managed: bool,
    /// The form fields, for a scheme with no browser step.
    #[serde(default)]
    pub fields: Vec<CredentialField>,
}

/// One service in the connector catalog (a Composio toolkit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorService {
    /// The toolkit slug, for example `gmail`.
    pub slug: String,
    /// The name, for example "Gmail".
    pub name: String,
    /// What it is.
    #[serde(default)]
    pub description: String,
    /// The logo, from the hub's logo cache.
    pub logo_url: Option<String>,
    /// The category slugs.
    #[serde(default)]
    pub categories: Vec<String>,
    /// The number of tools.
    #[serde(default)]
    pub tool_count: u32,
    /// The ways to connect it, the preferred one first.
    #[serde(default)]
    pub auth_schemes: Vec<AuthScheme>,
    /// The number of connections to it on this hub.
    #[serde(default)]
    pub connected_count: u32,
}

/// One category of the connector catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCategory {
    /// The slug, for example `email`.
    pub slug: String,
    /// The name, for example "Email".
    pub name: String,
    /// The number of services in it, when known.
    #[serde(default)]
    pub service_count: Option<u32>,
}

/// One tool of a service or a connection, without its input schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorToolSummary {
    /// The Composio tool slug, for example `GMAIL_SEND_EMAIL`.
    pub slug: String,
    /// A short description.
    #[serde(default)]
    pub description: String,
    /// The kind, which picks the approval rule.
    pub kind: ToolKind,
}

/// One labelled value of a connector tool call, in a form for a person to
/// read. An email shows To, Subject and Body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayField {
    /// The label, for example "Subject".
    pub label: String,
    /// The value as text. Long values are cut, with a note at the end.
    pub value: String,
}

/// Who or what let a connector tool call run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallDecision {
    /// The rule was "run without asking".
    RanWithoutAsking,
    /// The user allowed this call.
    AllowedByUser,
    /// The user allowed an earlier call of the same kind in this task.
    AllowedEarlierInTask,
    /// The user denied it. It did not run.
    DeniedByUser,
    /// Nobody answered in time. It did not run.
    Expired,
}

/// How a connector tool call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    /// It ran and Composio reported success.
    Succeeded,
    /// It ran and failed, or Composio refused it.
    Failed,
    /// It did not run.
    NotRun,
}

/// One record in the audit log of connector tool calls. The hub never
/// stores the arguments or the result, only a hash and a size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCall {
    /// The ID.
    pub id: ConnectorCallId,
    /// The connection.
    pub connection_id: ConnectionId,
    /// The channel of the session.
    pub channel_id: Option<ChannelId>,
    /// The session that called the tool.
    pub session_id: Option<SessionId>,
    /// The task, for a task session.
    pub task_id: Option<TaskId>,
    /// The tool slug.
    pub tool: String,
    /// The tool kind.
    pub tool_kind: ToolKind,
    /// The SHA-256 of the canonical JSON of the arguments.
    pub arguments_sha256: Sha256Hash,
    /// Who or what let it run.
    pub decision: CallDecision,
    /// How it ended.
    pub outcome: CallOutcome,
    /// The error, when it failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Composio's log ID, to find the call in the Composio dashboard.
    #[serde(default)]
    pub composio_log_id: Option<String>,
    /// The size of the result in bytes, before any cut.
    #[serde(default)]
    pub result_bytes: u64,
    /// How long it ran, in milliseconds.
    #[serde(default)]
    pub duration_ms: u64,
    /// When the call came.
    pub at: Timestamp,
}

/// The Composio account of the hub, as far as Composio tells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposioUsage {
    /// The tool calls in the current period.
    pub used: u64,
    /// The limit of the plan, when it has one.
    #[serde(default)]
    pub limit: Option<u64>,
    /// When the period ends, when known.
    #[serde(default)]
    pub resets_at: Option<Timestamp>,
}

/// The Composio settings of the hub. Never holds the key.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ComposioStatus {
    /// Whether the hub has a key.
    pub key_present: bool,
    /// The last 4 characters of the key, so the user can tell keys apart.
    #[serde(default)]
    pub key_hint: Option<String>,
    /// The plan name, when Composio tells.
    #[serde(default)]
    pub plan: Option<String>,
    /// The usage, when Composio tells.
    #[serde(default)]
    pub usage: Option<ComposioUsage>,
    /// What went wrong at the last check, for example a key that Composio
    /// refused.
    #[serde(default)]
    pub error: Option<String>,
    /// When the hub last asked Composio.
    #[serde(default)]
    pub checked_at: Option<Timestamp>,
}

/// One line of the "Connectors in this channel" context block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorContextEntry {
    /// The handle, for example `gmail_work`.
    pub handle: String,
    /// The service name, for example "Gmail".
    pub service_name: String,
    /// The account identity, for example an email address.
    #[serde(default)]
    pub identity: Option<String>,
    /// True when the connection needs a new sign-in.
    #[serde(default)]
    pub needs_sign_in: bool,
}

impl From<&Connection> for ConnectorContextEntry {
    fn from(c: &Connection) -> Self {
        Self {
            handle: c.handle.clone(),
            service_name: c.service_name.clone(),
            identity: c.identity.clone(),
            needs_sign_in: matches!(c.status, ConnectionStatus::NeedsSignIn { .. }),
        }
    }
}

impl fmt::Display for ConnectorContextEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}",
            one_line(&self.handle),
            one_line(&self.service_name)
        )?;
        if let Some(identity) = self.identity.as_deref().filter(|i| !i.trim().is_empty()) {
            write!(f, ", {}", one_line(identity))?;
        }
        if self.needs_sign_in {
            f.write_str(" (needs a new sign-in: ask the user to reconnect it)")?;
        }
        Ok(())
    }
}

/// Text on one line: line breaks become spaces, so a name cannot add a line
/// to a context block.
pub(crate) fn one_line(text: &str) -> String {
    text.split(['\n', '\r'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The "Connectors in this channel" block for a session's prompt. Empty
/// when the channel can use no connection.
///
/// ```text
/// Connectors in this channel:
/// - gmail_work: Gmail, chris@work.example
/// - gmail_personal: Gmail, chris@example.com
/// To use one, call list_connector_tools with its handle, then run_connector_tool.
/// ```
pub fn render_connectors_block(entries: &[ConnectorContextEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::from("Connectors in this channel:\n");
    for entry in entries {
        out.push_str("- ");
        out.push_str(&entry.to_string());
        out.push('\n');
    }
    out.push_str("To use one, call list_connector_tools with its handle, then run_connector_tool.");
    out
}

/// A JSON value as canonical bytes: object keys sorted, no spaces. The hub
/// hashes these bytes for [`ConnectorCall::arguments_sha256`].
pub fn canonical_json(value: &Value) -> String {
    fn write(value: &Value, out: &mut String) {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push('{');
                for (i, key) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String(key.clone()).to_string());
                    out.push(':');
                    write(&map[key], out);
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ApprovalMode, ChannelKind, Isolation};
    use serde_json::json;

    fn channel(kind: ChannelKind, tags: &[&str]) -> Channel {
        Channel {
            id: ChannelId::new(),
            kind,
            name: "Work".into(),
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
            allowed_local_mcp_servers: vec![],
            memory_enabled: true,
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            disabled_connections: vec![],
            disabled_skills: vec![],
            archived_at: None,
            created_at: Timestamp::from_millis(0),
            updated_at: Timestamp::from_millis(0),
        }
    }

    pub(crate) fn connection() -> Connection {
        Connection {
            id: "con_00000000-0000-0000-0000-000000000001".parse().unwrap(),
            name: "Gmail Work".into(),
            handle: "gmail_work".into(),
            service: "gmail".into(),
            service_name: "Gmail".into(),
            logo_url: Some("https://hub.example/v1/logos/gmail.svg".into()),
            identity: Some("chris@work.example".into()),
            use_in: UseIn::Tags {
                tags: vec!["work".into()],
            },
            approvals: ToolApprovals::default(),
            status: ConnectionStatus::Active,
            last_used_at: None,
            created_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(2),
        }
    }

    #[test]
    fn use_in_shapes_and_matching() {
        assert_eq!(
            serde_json::to_value(UseIn::EveryChannel).unwrap(),
            json!({"kind": "every_channel"})
        );
        let tags = UseIn::Tags {
            tags: vec!["work".into(), "mail".into()],
        };
        assert_eq!(
            serde_json::to_value(&tags).unwrap(),
            json!({"kind": "tags", "tags": ["work", "mail"]})
        );
        let work = channel(ChannelKind::Standard, &["work", "mail", "x"]);
        let other = channel(ChannelKind::Standard, &["work"]);
        let tutti = channel(ChannelKind::Tutti, &[]);
        let tagged_tutti = channel(ChannelKind::Tutti, &["work", "mail"]);
        assert!(tags.matches_channel(&work));
        assert!(!tags.matches_channel(&other));
        assert!(!tags.matches_channel(&tutti));
        assert!(tags.matches_channel(&tagged_tutti));
        assert!(UseIn::EveryChannel.matches_channel(&tutti));
        let only = UseIn::Channels {
            channel_ids: vec![other.id],
        };
        assert!(only.matches_channel(&other));
        assert!(!only.matches_channel(&work));
        assert_eq!(
            serde_json::from_value::<UseIn>(serde_json::to_value(&only).unwrap()).unwrap(),
            only
        );
        // Empty tags never match, so they cannot widen by accident.
        assert!(!UseIn::Tags { tags: vec![] }.matches_channel(&work));
    }

    #[test]
    fn use_in_normalizes_and_finds_added_channels() {
        let use_in = UseIn::Tags {
            tags: vec![" work ".into(), "work".into()],
        }
        .normalized()
        .unwrap();
        assert_eq!(
            use_in,
            UseIn::Tags {
                tags: vec!["work".into()]
            }
        );
        assert!(UseIn::Tags { tags: vec![] }.normalized().is_err());
        assert!(
            UseIn::Tags {
                tags: vec!["Bad Tag".into()]
            }
            .normalized()
            .is_err()
        );
        assert!(
            UseIn::Channels {
                channel_ids: vec![]
            }
            .normalized()
            .is_err()
        );

        let work = channel(ChannelKind::Standard, &["work"]);
        let home = channel(ChannelKind::Standard, &["home"]);
        let tutti = channel(ChannelKind::Tutti, &[]);
        let channels = vec![work.clone(), home.clone(), tutti];
        let added = UseIn::EveryChannel.added_channels(&use_in, &channels);
        assert_eq!(added.len(), 2);
        assert!(
            use_in
                .added_channels(&UseIn::EveryChannel, &channels)
                .is_empty()
        );
    }

    #[test]
    fn tool_kinds_from_hints() {
        assert_eq!(
            ToolKind::from_hints(true, false, false, false),
            ToolKind::Read
        );
        assert_eq!(
            ToolKind::from_hints(false, true, false, false),
            ToolKind::Change
        );
        assert_eq!(
            ToolKind::from_hints(false, false, true, false),
            ToolKind::Change
        );
        assert_eq!(
            ToolKind::from_hints(true, false, false, true),
            ToolKind::Delete
        );
        assert_eq!(
            ToolKind::from_hints(false, false, false, false),
            ToolKind::Change
        );
        assert_eq!(
            ToolKind::from_hints(true, true, false, false),
            ToolKind::Change
        );
    }

    #[test]
    fn approvals_default_and_loosening() {
        let defaults = ToolApprovals::default();
        assert_eq!(
            serde_json::to_value(defaults).unwrap(),
            json!({"read": "run_without_asking", "change": "ask_every_time", "delete": "ask_every_time"})
        );
        assert_eq!(
            defaults.rule_for(ToolKind::Delete),
            ApprovalRule::AskEveryTime
        );
        let looser = ToolApprovals {
            change: ApprovalRule::AskOncePerTask,
            ..defaults
        };
        assert!(looser.is_looser_than(&defaults));
        assert!(!defaults.is_looser_than(&looser));
        let stricter = ToolApprovals {
            read: ApprovalRule::AskOncePerTask,
            ..defaults
        };
        assert!(!stricter.is_looser_than(&defaults));
    }

    #[test]
    fn connection_round_trips_with_each_status() {
        let mut c = connection();
        for status in [
            ConnectionStatus::Connecting {
                redirect_url: Some("https://connect.composio.dev/link/abc".into()),
                expires_at: Timestamp::from_millis(CONNECT_ATTEMPT_TIMEOUT_MS),
            },
            ConnectionStatus::Connecting {
                redirect_url: None,
                expires_at: Timestamp::from_millis(5),
            },
            ConnectionStatus::Active,
            ConnectionStatus::NeedsSignIn { message: None },
            ConnectionStatus::Failed {
                message: "The sign-in did not finish in 10 minutes. Try again.".into(),
            },
        ] {
            c.status = status;
            let value = serde_json::to_value(&c).unwrap();
            assert!(value["status"]["kind"].is_string());
            assert_eq!(serde_json::from_value::<Connection>(value).unwrap(), c);
        }
        assert_eq!(
            serde_json::to_value(ConnectionStatus::Active).unwrap(),
            json!({"kind": "active"})
        );
    }

    #[test]
    fn availability_follows_scope_and_exceptions() {
        let c = connection();
        let mut work = channel(ChannelKind::Standard, &["work"]);
        assert!(c.is_available_in(&work));
        work.disabled_connections.push(c.id);
        assert!(!c.is_available_in(&work));
        assert!(!c.is_available_in(&channel(ChannelKind::Standard, &[])));
    }

    #[test]
    fn handles() {
        assert_eq!(connection_handle("Gmail Work"), "gmail_work");
        assert_eq!(
            connection_handle("  GitHub -- (Personal) "),
            "github_personal"
        );
        assert_eq!(connection_handle("Café Mail"), "caf_mail");
        assert_eq!(connection_handle("🙂"), "connection");
        let long = connection_handle(&"word ".repeat(30));
        assert!(long.len() <= CONNECTION_HANDLE_MAX_CHARS);
        assert!(!long.ends_with('_'));
        assert_eq!(
            unique_connection_handle("Gmail Work", ["gmail"]),
            "gmail_work"
        );
        assert_eq!(
            unique_connection_handle("Gmail Work", ["gmail_work", "gmail_work_2"]),
            "gmail_work_3"
        );
        let taken = connection_handle(&"x".repeat(60));
        let next = unique_connection_handle(&"x".repeat(60), [taken.as_str()]);
        assert!(next.len() <= CONNECTION_HANDLE_MAX_CHARS);
        assert!(next.ends_with("_2"));
    }

    #[test]
    fn names_are_checked() {
        assert!(validate_connection_name("Gmail Work").is_ok());
        assert!(validate_connection_name("   ").is_err());
        assert!(validate_connection_name(&"a".repeat(81)).is_err());
        assert!(validate_connection_name("a\nb").is_err());
    }

    #[test]
    fn catalog_types_round_trip() {
        let service = ConnectorService {
            slug: "gmail".into(),
            name: "Gmail".into(),
            description: "Email from Google.".into(),
            logo_url: None,
            categories: vec!["email".into()],
            tool_count: 61,
            auth_schemes: vec![
                AuthScheme {
                    kind: AuthSchemeKind::Oauth2,
                    managed: true,
                    fields: vec![],
                },
                AuthScheme {
                    kind: AuthSchemeKind::ApiKey,
                    managed: false,
                    fields: vec![CredentialField {
                        name: "api_key".into(),
                        label: "API key".into(),
                        description: None,
                        secret: true,
                        required: true,
                    }],
                },
            ],
            connected_count: 2,
        };
        let value = serde_json::to_value(&service).unwrap();
        assert_eq!(value["auth_schemes"][0]["kind"], "oauth2");
        assert_eq!(
            serde_json::from_value::<ConnectorService>(value).unwrap(),
            service
        );
        // A scheme that this version does not know reads as `other`.
        let scheme: AuthScheme =
            serde_json::from_value(json!({"kind": "saml", "managed": false})).unwrap();
        assert_eq!(scheme.kind, AuthSchemeKind::Other);
        assert!(AuthSchemeKind::Oauth2.uses_browser());
        assert!(!AuthSchemeKind::ApiKey.uses_browser());
        let category = ConnectorCategory {
            slug: "email".into(),
            name: "Email".into(),
            service_count: Some(12),
        };
        assert_eq!(
            serde_json::from_value::<ConnectorCategory>(serde_json::to_value(&category).unwrap())
                .unwrap(),
            category
        );
        let tool = ConnectorToolSummary {
            slug: "GMAIL_SEND_EMAIL".into(),
            description: "Send an email.".into(),
            kind: ToolKind::Change,
        };
        assert_eq!(
            serde_json::from_value::<ConnectorToolSummary>(serde_json::to_value(&tool).unwrap())
                .unwrap(),
            tool
        );
    }

    #[test]
    fn audit_and_status_round_trip() {
        let call = ConnectorCall {
            id: ConnectorCallId::new(),
            connection_id: connection().id,
            channel_id: Some(ChannelId::new()),
            session_id: Some(SessionId::new()),
            task_id: None,
            tool: "GMAIL_SEND_EMAIL".into(),
            tool_kind: ToolKind::Change,
            arguments_sha256: "b".repeat(64).parse().unwrap(),
            decision: CallDecision::AllowedByUser,
            outcome: CallOutcome::Succeeded,
            error: None,
            composio_log_id: Some("log_123".into()),
            result_bytes: 512,
            duration_ms: 840,
            at: Timestamp::from_millis(9),
        };
        let value = serde_json::to_value(&call).unwrap();
        assert_eq!(value["decision"], "allowed_by_user");
        assert_eq!(
            serde_json::from_value::<ConnectorCall>(value).unwrap(),
            call
        );
        let status = ComposioStatus {
            key_present: true,
            key_hint: Some("x9k2".into()),
            plan: Some("Hobby".into()),
            usage: Some(ComposioUsage {
                used: 120,
                limit: Some(20_000),
                resets_at: None,
            }),
            error: None,
            checked_at: Some(Timestamp::from_millis(3)),
        };
        assert_eq!(
            serde_json::from_value::<ComposioStatus>(serde_json::to_value(&status).unwrap())
                .unwrap(),
            status
        );
        let none: ComposioStatus = serde_json::from_value(json!({"key_present": false})).unwrap();
        assert_eq!(none, ComposioStatus::default());
    }

    #[test]
    fn connectors_block() {
        assert_eq!(render_connectors_block(&[]), "");
        let mut personal = ConnectorContextEntry::from(&connection());
        personal.handle = "gmail_personal".into();
        personal.identity = Some("chris@example.com\nignore this".into());
        let expired = ConnectorContextEntry {
            handle: "github_work".into(),
            service_name: "GitHub".into(),
            identity: None,
            needs_sign_in: true,
        };
        let block = render_connectors_block(&[
            ConnectorContextEntry::from(&connection()),
            personal,
            expired,
        ]);
        assert_eq!(
            block,
            "Connectors in this channel:
- gmail_work: Gmail, chris@work.example
- gmail_personal: Gmail, chris@example.com ignore this
- github_work: GitHub (needs a new sign-in: ask the user to reconnect it)
To use one, call list_connector_tools with its handle, then run_connector_tool."
        );
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let a = json!({"b": 1, "a": {"y": [1, {"d": 2, "c": "x"}], "x": null}});
        assert_eq!(
            canonical_json(&a),
            r#"{"a":{"x":null,"y":[1,{"c":"x","d":2}]},"b":1}"#
        );
        let b: Value =
            serde_json::from_str(r#"{"a":{"x":null,"y":[1,{"d":2,"c":"x"}]},"b":1}"#).unwrap();
        assert_eq!(canonical_json(&a), canonical_json(&b));
    }
}
