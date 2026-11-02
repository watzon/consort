//! Connectors in the view state: the connections, the Composio key, the
//! catalog, the hub cards, and the pure functions that the settings pages,
//! the dialogs and the channel dialog share.
//!
//! The rules here follow the hub: [`use_in_matches`] is the same rule as
//! `UseIn::matches` in `consort-proto`, so the app never shows a connector
//! in a channel that the hub would not give it.

use std::sync::Arc;

use crate::view::{ChannelKind, ChannelView, ProviderLogo, SignInChoice, ViewState};

/// Where a connection or a skill can be used. It mirrors the proto's
/// `UseIn`, with channel IDs as text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum UseInView {
    /// Every channel, Tutti too.
    #[default]
    EveryChannel,
    /// The channels that have every one of these tags.
    Tags(Vec<String>),
    /// Only these channels.
    Channels(Vec<String>),
}

/// What the hub does before it runs a connector tool of one kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ApprovalRuleView {
    #[default]
    RunWithoutAsking,
    AskOncePerTask,
    AskEveryTime,
}

impl ApprovalRuleView {
    pub const ALL: [ApprovalRuleView; 3] = [
        ApprovalRuleView::RunWithoutAsking,
        ApprovalRuleView::AskOncePerTask,
        ApprovalRuleView::AskEveryTime,
    ];

    /// The words in the select.
    pub fn label(self) -> &'static str {
        match self {
            ApprovalRuleView::RunWithoutAsking => "Run without asking",
            ApprovalRuleView::AskOncePerTask => "Ask once per task",
            ApprovalRuleView::AskEveryTime => "Ask every time",
        }
    }

    /// A stable key for a select menu.
    pub fn key(self) -> &'static str {
        match self {
            ApprovalRuleView::RunWithoutAsking => "run_without_asking",
            ApprovalRuleView::AskOncePerTask => "ask_once_per_task",
            ApprovalRuleView::AskEveryTime => "ask_every_time",
        }
    }

    pub fn from_key(key: &str) -> Option<ApprovalRuleView> {
        ApprovalRuleView::ALL.into_iter().find(|r| r.key() == key)
    }

    /// The verb phrase after a tool kind, such as "runs without asking".
    fn verb(self, plural: bool) -> &'static str {
        match (self, plural) {
            (ApprovalRuleView::RunWithoutAsking, false) => "runs without asking",
            (ApprovalRuleView::RunWithoutAsking, true) => "run without asking",
            (ApprovalRuleView::AskOncePerTask, false) => "asks you once per task",
            (ApprovalRuleView::AskOncePerTask, true) => "ask you once per task",
            (ApprovalRuleView::AskEveryTime, false) => "asks you",
            (ApprovalRuleView::AskEveryTime, true) => "ask you",
        }
    }
}

/// The kind of a connector tool. Composio's hints decide it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ToolKindView {
    #[default]
    Read,
    Change,
    Delete,
}

impl ToolKindView {
    pub const ALL: [ToolKindView; 3] = [
        ToolKindView::Read,
        ToolKindView::Change,
        ToolKindView::Delete,
    ];

    /// The title of the approval row.
    pub fn label(self) -> &'static str {
        match self {
            ToolKindView::Read => "Read",
            ToolKindView::Change => "Create and change",
            ToolKindView::Delete => "Delete",
        }
    }

    /// The kind in a sentence, as a plural noun.
    fn noun(self) -> &'static str {
        match self {
            ToolKindView::Read => "read",
            ToolKindView::Change => "changes",
            ToolKindView::Delete => "deletes",
        }
    }

    /// What tools of this kind do, for the approval row.
    pub fn hint(self) -> &'static str {
        match self {
            ToolKindView::Read => "Tools that only look things up.",
            ToolKindView::Change => "Tools that create, send or change things.",
            ToolKindView::Delete => "Tools that delete things.",
        }
    }
}

/// The approval rule for each tool kind of one connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ToolApprovalsView {
    pub read: ApprovalRuleView,
    pub change: ApprovalRuleView,
    pub delete: ApprovalRuleView,
}

impl Default for ToolApprovalsView {
    /// The plan's defaults: read runs, changes and deletes ask every time.
    fn default() -> Self {
        ToolApprovalsView {
            read: ApprovalRuleView::RunWithoutAsking,
            change: ApprovalRuleView::AskEveryTime,
            delete: ApprovalRuleView::AskEveryTime,
        }
    }
}

impl ToolApprovalsView {
    pub fn rule_for(&self, kind: ToolKindView) -> ApprovalRuleView {
        match kind {
            ToolKindView::Read => self.read,
            ToolKindView::Change => self.change,
            ToolKindView::Delete => self.delete,
        }
    }

    /// The same rules with one kind changed.
    pub fn with(mut self, kind: ToolKindView, rule: ApprovalRuleView) -> Self {
        match kind {
            ToolKindView::Read => self.read = rule,
            ToolKindView::Change => self.change = rule,
            ToolKindView::Delete => self.delete = rule,
        }
        self
    }
}

/// Where a connection is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum ConnectionStatusView {
    /// The user must finish the sign-in, in the browser or in the form.
    Connecting {
        redirect_url: Option<String>,
        /// Unix milliseconds.
        expires_at: i64,
    },
    #[default]
    Active,
    NeedsSignIn {
        message: Option<String>,
    },
    Failed {
        message: String,
    },
}

impl ConnectionStatusView {
    pub fn is_active(&self) -> bool {
        matches!(self, ConnectionStatusView::Active)
    }

    /// The user must reconnect.
    pub fn needs_sign_in(&self) -> bool {
        matches!(
            self,
            ConnectionStatusView::NeedsSignIn { .. } | ConnectionStatusView::Failed { .. }
        )
    }
}

/// An image that the client loaded: its bytes and a stable key for a cache.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LogoImage {
    /// A short stable key, made from the URL.
    pub key: String,
    /// The image file: SVG, PNG, JPEG, WebP or GIF.
    pub bytes: Arc<Vec<u8>>,
}

/// The logo of a service. The image is there once the client has loaded it.
/// Until then, and when it does not load, the app draws the monogram.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ServiceLogo {
    pub url: Option<String>,
    pub image: Option<LogoImage>,
    /// One or two letters of the service name.
    pub monogram: String,
}

/// The monogram of a service name: its first letter in upper case.
pub fn monogram(name: &str) -> String {
    name.trim()
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned())
}

/// One connected account of one Composio service.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionView {
    pub id: String,
    /// The name, such as "Gmail Work".
    pub name: String,
    /// The name that the assistant uses, such as `gmail_work`.
    pub handle: String,
    /// The Composio toolkit slug, such as `gmail`.
    pub service: String,
    pub service_name: String,
    pub logo: ServiceLogo,
    /// The account identity: an email address or a login.
    pub identity: Option<String>,
    pub use_in: UseInView,
    pub approvals: ToolApprovalsView,
    /// Tools with their own rule.
    pub tool_exceptions: Vec<ToolExceptionView>,
    /// The account belongs to the assistant.
    pub assistant_owned: bool,
    pub status: ConnectionStatusView,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// One tool of a connection with its own approval rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolExceptionView {
    /// The Composio slug.
    pub tool: String,
    /// The name for the user, such as "Merge a pull request".
    pub name: String,
    pub rule: ApprovalRuleView,
}

/// Whether a load from the hub has run, and how it went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LoadState {
    #[default]
    NotLoaded,
    Loading,
    Loaded,
    /// What went wrong and what to do next.
    Failed(String),
    /// The hub does not have this part yet. The text says so and what to do.
    Unsupported(String),
}

impl LoadState {
    pub fn is_loading(&self) -> bool {
        matches!(self, LoadState::Loading)
    }

    /// The text of a failure, if any.
    pub fn error(&self) -> Option<&str> {
        match self {
            LoadState::Failed(m) | LoadState::Unsupported(m) => Some(m),
            _ => None,
        }
    }
}

/// The Composio key and the plan, as the hub reports them. The key itself
/// never leaves the hub.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComposioView {
    pub load: LoadState,
    pub key_present: bool,
    /// The last characters of the key, such as "…k3Fq".
    pub key_hint: Option<String>,
    pub plan: Option<String>,
    /// Tool calls this month.
    pub used: Option<u64>,
    pub limit: Option<u64>,
    /// Why the last check of the key failed.
    pub error: Option<String>,
}

impl ComposioView {
    /// The line under the usage bar, such as "12,340 of 100,000 tool calls
    /// this month. Hobby plan."
    pub fn usage_line(&self) -> Option<String> {
        let plan = self
            .plan
            .as_deref()
            .map(|p| format!(" {} plan.", p.trim().trim_end_matches(" plan")));
        let usage = match (self.used, self.limit) {
            (Some(used), Some(limit)) => Some(format!(
                "{} of {} tool calls this month.",
                thousands(used),
                thousands(limit)
            )),
            (Some(used), None) => Some(format!("{} tool calls this month.", thousands(used))),
            _ => None,
        };
        match (usage, plan) {
            (Some(u), Some(p)) => Some(format!("{u}{p}")),
            (Some(u), None) => Some(u),
            (None, Some(p)) => Some(p.trim().to_owned()),
            (None, None) => None,
        }
    }

    /// How much of the plan is used, from 0 to 1.
    pub fn usage_share(&self) -> Option<f32> {
        match (self.used, self.limit) {
            (Some(used), Some(limit)) if limit > 0 => Some((used as f32 / limit as f32).min(1.0)),
            _ => None,
        }
    }
}

/// A number with commas between the thousands: 12340 gives "12,340".
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A category of the connector catalog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CategoryView {
    pub slug: String,
    pub name: String,
}

/// How a service signs in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AuthKind {
    #[default]
    OAuth2,
    OAuth1,
    ApiKey,
    BearerToken,
    Basic,
    NoAuth,
    Other,
}

impl AuthKind {
    /// The scheme signs in on the service's own page in the browser.
    pub fn uses_browser(self) -> bool {
        matches!(self, AuthKind::OAuth2 | AuthKind::OAuth1)
    }
}

/// One field of a credential form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CredentialFieldView {
    /// The name that Composio expects.
    pub name: String,
    pub label: String,
    pub description: Option<String>,
    /// The form masks it.
    pub secret: bool,
    pub required: bool,
}

/// One way to sign in to a service.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthSchemeView {
    pub kind: AuthKind,
    /// Composio manages the app for this scheme.
    pub managed: bool,
    pub fields: Vec<CredentialFieldView>,
}

/// A service in the connector catalog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceView {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub logo: ServiceLogo,
    pub categories: Vec<String>,
    pub tool_count: u32,
    pub auth_schemes: Vec<AuthSchemeView>,
    /// How many connections the user has of this service.
    pub connected_count: u32,
}

impl ServiceView {
    /// The scheme that the Connect dialog uses: a browser scheme first,
    /// then a managed one, then the first.
    pub fn preferred_scheme(&self) -> Option<&AuthSchemeView> {
        self.auth_schemes
            .iter()
            .find(|s| s.kind.uses_browser() && s.managed)
            .or_else(|| self.auth_schemes.iter().find(|s| s.kind.uses_browser()))
            .or_else(|| self.auth_schemes.first())
    }
}

/// The connector catalog as the Add connector dialog shows it: one search,
/// paged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogView {
    /// The search that `services` answers.
    pub search: String,
    /// The category that `services` answers. `None` is every category.
    pub category: Option<String>,
    pub categories: Vec<CategoryView>,
    pub services: Vec<ServiceView>,
    /// There is another page.
    pub next_cursor: Option<String>,
    /// The first page.
    pub load: LoadState,
    /// The next page.
    pub more: LoadState,
}

/// A tool of a service, for the approval rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectorToolView {
    pub slug: String,
    pub description: String,
    pub kind: ToolKindView,
}

/// The tools of one service, once loaded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceToolsView {
    pub service: String,
    pub tools: Vec<ConnectorToolView>,
    pub load: LoadState,
}

impl ServiceToolsView {
    /// How many tools of a kind the service has.
    pub fn count(&self, kind: ToolKindView) -> usize {
        self.tools.iter().filter(|t| t.kind == kind).count()
    }
}

/// One call of a connector tool, from the audit log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectorCallView {
    pub id: String,
    pub tool: String,
    pub kind: ToolKindView,
    /// Such as "Ran without asking" or "Denied by you".
    pub decision: String,
    pub succeeded: bool,
    pub error: Option<String>,
    pub time_label: String,
    /// The day of the call, such as "Today" or "October 12".
    pub day_label: String,
    /// The size of the result, such as "1.2 KB".
    pub size_label: String,
}

/// A page of the audit log of one connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionCallsView {
    pub connection_id: String,
    pub calls: Vec<ConnectorCallView>,
    pub has_more: bool,
}

// Hub cards ------------------------------------------------------------------

/// A label and a value of a connector card, such as To and an address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardFieldView {
    pub label: String,
    pub value: String,
}

/// A connector tool call that waits for the user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectorCardView {
    pub connection_id: String,
    pub connection_name: String,
    pub service_name: String,
    pub logo: ServiceLogo,
    /// The tool slug, such as `GMAIL_SEND_EMAIL`.
    pub tool: String,
    pub kind: ToolKindView,
    /// The arguments in a form to read.
    pub fields: Vec<CardFieldView>,
    /// Why it asks, such as "Create and change tools ask every time".
    pub rule_note: String,
    /// The voice parts, for a tool that drafts or sends in the user's
    /// name: the voice line, the flags and Edit.
    pub draft: Option<crate::voice::DraftCardView>,
}

/// A management change that waits for the user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManagementCardView {
    pub tool: String,
    pub change: String,
    pub effect: String,
    /// The rows of a permissions change.
    pub permissions: Option<PermissionsCardView>,
}

/// The rows of a permissions card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionsCardView {
    pub groups: Vec<PermissionGroupView>,
    /// The risk that the user must read.
    pub warning: Option<String>,
}

impl PermissionsCardView {
    /// The number of changes, for the answered line.
    pub fn change_count(&self) -> usize {
        self.groups.iter().map(|g| g.rows.len()).sum()
    }
}

/// One group of a permissions card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionGroupView {
    pub heading: String,
    pub rows: Vec<PermissionRowView>,
    pub note: Option<String>,
}

/// One row of a permissions card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionRowView {
    pub subject: String,
    /// The subject is a command pattern.
    pub code: bool,
    pub text: String,
}

/// The connect card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectCardView {
    pub service: String,
    pub service_name: String,
    pub logo: ServiceLogo,
    pub name: String,
    pub use_in: UseInView,
    pub approvals: ToolApprovalsView,
    /// The connection, after the user continued.
    pub connection_id: Option<String>,
}

/// The sign-in card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignInCardView {
    pub worker_id: String,
    pub account_id: String,
    pub account_label: String,
    pub harness: String,
    pub provider: String,
    pub logo: ProviderLogo,
    pub method: Option<SignInChoice>,
    pub sign_in_id: Option<String>,
}

/// The install card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstallCardView {
    pub name: String,
    pub description: String,
    /// Such as "github.com/anthropics/skills".
    pub source: String,
    pub commit: Option<String>,
    pub license: Option<String>,
    pub scripts: Vec<String>,
    pub audits: Vec<crate::skills::AuditView>,
    pub use_in: UseInView,
    pub blocked_reason: Option<String>,
    pub skill_id: Option<String>,
}

/// The worker card. The code is not here: this app makes it, and it stays on
/// this client (`ViewState::worker_pairing`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddWorkerCardView {
    /// A worker on this computer. Otherwise the card shows each system.
    pub this_computer: bool,
    /// The worker that came online.
    pub worker_name: Option<String>,
}

/// A card that the hub made itself. It is answered with `approval.answer`,
/// like any approval.
#[derive(Clone, Debug, PartialEq)]
pub enum HubCardView {
    Connector(Box<ConnectorCardView>),
    Management(ManagementCardView),
    ConnectService(ConnectCardView),
    AddWorker(AddWorkerCardView),
    SignIn(SignInCardView),
    SkillInstall(InstallCardView),
    Monid(Box<crate::monid::MonidCardView>),
    Access(Box<crate::access::AccessCardView>),
}

// Pure functions ---------------------------------------------------------------

/// Whether `channel` is in the scope. The same rule as the proto's
/// `UseIn::matches`: Tutti matches only "every channel" unless it has
/// tags of its own, and empty tags match nothing.
pub fn use_in_matches(use_in: &UseInView, channel: &ChannelView) -> bool {
    match use_in {
        UseInView::EveryChannel => true,
        UseInView::Tags(tags) => {
            if channel.kind == ChannelKind::Tutti && channel.tags.is_empty() {
                return false;
            }
            !tags.is_empty() && tags.iter().all(|t| channel.tags.contains(t))
        }
        UseInView::Channels(ids) => ids.contains(&channel.id),
    }
}

/// The channels in the scope, in the order of `channels`.
pub fn matching_channels<'a>(
    use_in: &UseInView,
    channels: &'a [ChannelView],
) -> Vec<&'a ChannelView> {
    channels
        .iter()
        .filter(|c| use_in_matches(use_in, c))
        .collect()
}

/// "a", "a and b", "a, b and c".
pub fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// "tagged work", "tagged work and gpu".
fn tagged(tags: &[String]) -> String {
    format!("tagged {}", join_and(tags))
}

/// The names of the channels with these IDs, in the order of `channels`.
fn channel_names(ids: &[String], channels: &[ChannelView]) -> Vec<String> {
    channels
        .iter()
        .filter(|c| ids.contains(&c.id))
        .map(|c| c.name.clone())
        .collect()
}

/// The scope in a few words, for a list row: "every channel", "channels
/// tagged work", "acme only", "acme and hiring" or "4 channels".
pub fn use_in_short(use_in: &UseInView, channels: &[ChannelView]) -> String {
    match use_in {
        UseInView::EveryChannel => "every channel".to_owned(),
        UseInView::Tags(tags) => format!("channels {}", tagged(tags)),
        UseInView::Channels(ids) => {
            let names = channel_names(ids, channels);
            match names.len() {
                0 => "no channel".to_owned(),
                1 => format!("{} only", names[0]),
                2 | 3 => join_and(&names),
                n => format!("{n} channels"),
            }
        }
    }
}

/// Whether the scope is this one channel and no other.
pub fn use_in_is_only(use_in: &UseInView, channel_id: &str) -> bool {
    matches!(use_in, UseInView::Channels(ids) if ids.len() == 1 && ids[0] == channel_id)
}

/// The short label of a chosen scope in the Use in control: "acme", "4
/// channels" or "Tagged", which the tag chips follow.
pub fn use_in_choice_label(use_in: &UseInView, channels: &[ChannelView]) -> String {
    match use_in {
        UseInView::EveryChannel => "Every channel".to_owned(),
        UseInView::Tags(_) => "Tagged".to_owned(),
        UseInView::Channels(ids) => match channel_names(ids, channels).as_slice() {
            [] => "No channel".to_owned(),
            [one] => one.clone(),
            many => format!("{} channels", many.len()),
        },
    }
}

/// The scope as a sentence: "Used in every channel." or "Used in channels
/// tagged work."
pub fn use_in_sentence(use_in: &UseInView, channels: &[ChannelView]) -> String {
    format!("Used in {}.", use_in_short(use_in, channels))
}

/// The hint under Use in on a settings page: "Every channel. 9 channels."
/// or "3 channels: acme, acme-infra, hiring. Their work runs only on
/// workers tagged work."
pub fn use_in_detail(use_in: &UseInView, channels: &[ChannelView]) -> String {
    let matching = matching_channels(use_in, channels);
    let names: Vec<String> = matching.iter().map(|c| c.name.clone()).collect();
    let count = |n: usize| {
        if n == 1 {
            "1 channel".to_owned()
        } else {
            format!("{n} channels")
        }
    };
    match use_in {
        UseInView::EveryChannel => format!("Every channel. {}.", count(names.len())),
        UseInView::Tags(tags) if names.is_empty() => {
            format!("No channel is {} yet.", tagged(tags))
        }
        UseInView::Tags(tags) => format!(
            "{}: {}. {} work runs only on workers {}.",
            count(names.len()),
            names.join(", "),
            if names.len() == 1 { "Its" } else { "Their" },
            tagged(tags)
        ),
        UseInView::Channels(_) if names.is_empty() => "No channel. Choose a scope.".to_owned(),
        UseInView::Channels(_) => format!("{}: {}.", count(names.len()), names.join(", ")),
    }
}

/// The note under Use in in a dialog: "3 channels match: acme, acme-infra,
/// hiring." or "No channel matches yet."
pub fn use_in_match_note(use_in: &UseInView, channels: &[ChannelView]) -> String {
    let names: Vec<String> = matching_channels(use_in, channels)
        .iter()
        .map(|c| c.name.clone())
        .collect();
    match names.len() {
        0 => "No channel matches yet.".to_owned(),
        1 => format!("1 channel matches: {}.", names[0]),
        n => format!("{n} channels match: {}.", names.join(", ")),
    }
}

/// The approval rules in one sentence, kinds with the same rule together:
/// "Read runs without asking. Changes and deletes ask you."
pub fn approvals_sentence(approvals: &ToolApprovalsView) -> String {
    let mut groups: Vec<(ApprovalRuleView, Vec<ToolKindView>)> = Vec::new();
    for kind in ToolKindView::ALL {
        let rule = approvals.rule_for(kind);
        match groups.iter_mut().find(|(r, _)| *r == rule) {
            Some((_, kinds)) => kinds.push(kind),
            None => groups.push((rule, vec![kind])),
        }
    }
    let clauses: Vec<String> = groups
        .iter()
        .map(|(rule, kinds)| {
            let nouns: Vec<String> = kinds.iter().map(|k| k.noun().to_owned()).collect();
            // "read" alone is a mass noun: "read runs".
            let plural = kinds.len() > 1 || kinds[0] != ToolKindView::Read;
            format!("{} {}", join_and(&nouns), rule.verb(plural))
        })
        .collect();
    clauses
        .iter()
        .map(|clause| {
            let mut letters = clause.chars();
            match letters.next() {
                Some(first) => format!("{}{}.", first.to_uppercase(), letters.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Why a connector card asks, such as "Create and change tools ask every
/// time".
pub fn rule_note(kind: ToolKindView, rule: ApprovalRuleView) -> String {
    let rule = match rule {
        ApprovalRuleView::RunWithoutAsking => "run without asking",
        ApprovalRuleView::AskOncePerTask => "ask once per task",
        ApprovalRuleView::AskEveryTime => "ask every time",
    };
    format!("{} tools {rule}", kind.label())
}

/// The connections of one service, in the order of the state.
pub fn connections_of_service<'a>(state: &'a ViewState, service: &str) -> Vec<&'a ConnectionView> {
    state
        .connections
        .iter()
        .filter(|c| c.service == service)
        .collect()
}

/// The first line of the Connect dialog: what to name the account.
pub fn connect_intro(service_name: &str, existing: &[&ConnectionView]) -> String {
    let names: Vec<String> = existing.iter().map(|c| c.name.clone()).collect();
    match names.len() {
        0 => format!(
            "Give this {service_name} account a name. The assistant sees it under that name."
        ),
        _ => format!(
            "You already have {}. Give this account a name the assistant can tell apart.",
            join_and(&names)
        ),
    }
}

/// The name that the Connect dialog starts with: the service name when no
/// connection has it, else nothing, so the user names the new account.
pub fn suggested_connection_name(service_name: &str, existing: &[&ConnectionView]) -> String {
    if existing
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(service_name))
    {
        String::new()
    } else {
        service_name.to_owned()
    }
}

/// The handle that a name gets, next to the handles of the other
/// connections. The hub uses the same rule.
pub fn handle_for(name: &str, state: &ViewState, except: Option<&str>) -> String {
    consort_proto::model::unique_connection_handle(
        name,
        state
            .connections
            .iter()
            .filter(|c| Some(c.id.as_str()) != except)
            .map(|c| c.handle.as_str()),
    )
}

/// The services of a catalog page that match a search and a category,
/// ignoring case. The search looks at the name, the slug and the
/// description. The mock filters with it; the hub filters on its side.
pub fn filter_services<'a>(
    services: &'a [ServiceView],
    search: &str,
    category: Option<&str>,
) -> Vec<&'a ServiceView> {
    let search = search.trim().to_lowercase();
    services
        .iter()
        .filter(|s| category.is_none_or(|c| s.categories.iter().any(|x| x == c)))
        .filter(|s| {
            search.is_empty()
                || s.name.to_lowercase().contains(&search)
                || s.slug.to_lowercase().contains(&search)
                || s.description.to_lowercase().contains(&search)
        })
        .collect()
}

/// The line under a connection in the Settings list: "chris@acme.dev,
/// channels tagged work", or the problem.
pub fn connection_row_detail(connection: &ConnectionView, channels: &[ChannelView]) -> String {
    match &connection.status {
        ConnectionStatusView::NeedsSignIn { .. } => "Needs a new sign-in".to_owned(),
        ConnectionStatusView::Failed { .. } => "Could not connect".to_owned(),
        ConnectionStatusView::Connecting { .. } => "Waiting for the sign-in".to_owned(),
        ConnectionStatusView::Active => {
            let scope = use_in_short(&connection.use_in, channels);
            match connection.identity.as_deref().filter(|i| !i.is_empty()) {
                Some(identity) => format!("{identity}, {scope}"),
                None => {
                    let mut chars = scope.chars();
                    chars
                        .next()
                        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                        .unwrap_or_default()
                }
            }
        }
    }
}

/// The status line under the name on the connection page.
pub fn connection_status_line(connection: &ConnectionView, tool_count: Option<usize>) -> String {
    let tools = tool_count
        .filter(|n| *n > 0)
        .map(|n| format!(" {n} tools."))
        .unwrap_or_default();
    match &connection.status {
        ConnectionStatusView::Active => match connection.identity.as_deref() {
            Some(identity) if !identity.is_empty() => {
                format!("Connected as {identity} through Composio.{tools}")
            }
            _ => format!("Connected through Composio.{tools}"),
        },
        ConnectionStatusView::Connecting { .. } => {
            "Waiting for you to finish the sign-in.".to_owned()
        }
        ConnectionStatusView::NeedsSignIn { message } => match message {
            Some(m) if !m.trim().is_empty() => format!(
                "Needs a new sign-in. {} Reconnect it, then the assistant can use it again.",
                crate::reduce::sentence(m)
            ),
            _ => {
                "Needs a new sign-in. Reconnect it, then the assistant can use it again.".to_owned()
            }
        },
        ConnectionStatusView::Failed { message } => {
            format!(
                "{} Reconnect to try again.",
                crate::reduce::sentence(message)
            )
        }
    }
}

/// One connector or skill in the dialog of a channel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelItemView {
    pub id: String,
    pub name: String,
    /// The line under the name.
    pub detail: String,
    /// The logo of a connector. `None` for a skill.
    pub logo: Option<ServiceLogo>,
    /// The item's Use in gives it to the channel.
    pub given: bool,
    /// Given and not turned off in the channel.
    pub enabled: bool,
}

/// Every connector and skill as one channel sees them: first the ones its
/// scope gives, then the others, each in the order of the state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelExtensionsView {
    pub channel_id: String,
    pub channel_name: String,
    pub connectors: Vec<ChannelItemView>,
    pub skills: Vec<ChannelItemView>,
}

impl ChannelExtensionsView {
    /// The handles of the connectors that sessions in the channel get.
    pub fn enabled_connector_ids(&self) -> Vec<&str> {
        self.connectors
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.id.as_str())
            .collect()
    }

    pub fn enabled_skill_ids(&self) -> Vec<&str> {
        self.skills
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.id.as_str())
            .collect()
    }
}

/// The line under an item in the channel dialog.
fn channel_item_detail(
    identity: Option<&str>,
    use_in: &UseInView,
    given: bool,
    enabled: bool,
    channel: &ChannelView,
    channels: &[ChannelView],
) -> String {
    let name = &channel.name;
    if !given {
        return format!(
            "Not in {name}. It is used in {}.",
            use_in_short(use_in, channels)
        );
    }
    if !enabled {
        return match use_in {
            UseInView::EveryChannel => format!("Off in {name}. Every other channel still has it."),
            _ => format!("Off in {name}. Other channels in its scope still have it."),
        };
    }
    let sentence = use_in_sentence(use_in, channels);
    match identity.filter(|i| !i.is_empty()) {
        Some(identity) => format!("{identity}. {sentence}"),
        None => sentence,
    }
}

/// The effective set of a channel with its exceptions. `None` when the
/// channel is not in the state.
pub fn channel_extensions(state: &ViewState, channel_id: &str) -> Option<ChannelExtensionsView> {
    let channel = state.channel(channel_id)?;
    let channels = &state.channels;
    let mut connectors: Vec<ChannelItemView> = state
        .connections
        .iter()
        .map(|c| {
            let given = use_in_matches(&c.use_in, channel);
            let enabled = given && !channel.disabled_connections.contains(&c.id);
            let mut detail = channel_item_detail(
                c.identity.as_deref(),
                &c.use_in,
                given,
                enabled,
                channel,
                channels,
            );
            if given && c.status.needs_sign_in() {
                detail = format!("{detail} Needs a new sign-in.");
            }
            ChannelItemView {
                id: c.id.clone(),
                name: c.name.clone(),
                detail,
                logo: Some(c.logo.clone()),
                given,
                enabled,
            }
        })
        .collect();
    let mut skills: Vec<ChannelItemView> = state
        .skills
        .iter()
        .map(|s| {
            let given = s.enabled && use_in_matches(&s.use_in, channel);
            let enabled = given && !channel.disabled_skills.contains(&s.id);
            let detail = if !s.enabled {
                "Off everywhere. Turn it on in Settings > Skills.".to_owned()
            } else {
                channel_item_detail(None, &s.use_in, given, enabled, channel, channels)
            };
            ChannelItemView {
                id: s.id.clone(),
                name: s.name.clone(),
                detail,
                logo: None,
                given,
                enabled,
            }
        })
        .collect();
    // Given items first. The sort is stable, so the state order stays.
    connectors.sort_by_key(|c| !c.given);
    skills.sort_by_key(|s| !s.given);
    Some(ChannelExtensionsView {
        channel_id: channel.id.clone(),
        channel_name: channel.name.clone(),
        connectors,
        skills,
    })
}

/// The "Connectors in this channel" block that a session in the channel
/// starts with, as the hub renders it.
pub fn connectors_context(state: &ViewState, channel_id: &str) -> String {
    let Some(channel) = state.channel(channel_id) else {
        return String::new();
    };
    let entries: Vec<consort_proto::model::ConnectorContextEntry> = state
        .connections
        .iter()
        .filter(|c| use_in_matches(&c.use_in, channel))
        .filter(|c| !channel.disabled_connections.contains(&c.id))
        .filter(|c| !matches!(c.status, ConnectionStatusView::Connecting { .. }))
        .map(|c| consort_proto::model::ConnectorContextEntry {
            handle: c.handle.clone(),
            service_name: c.service_name.clone(),
            identity: c.identity.clone(),
            needs_sign_in: c.status.needs_sign_in(),
            assistant_owned: c.assistant_owned,
        })
        .collect();
    consort_proto::model::render_connectors_block(&entries)
}

/// A channel to show the context of a connection with: the first channel
/// that the connection is in, Tutti last.
pub fn example_channel<'a>(
    use_in: &UseInView,
    channels: &'a [ChannelView],
) -> Option<&'a ChannelView> {
    let matching = matching_channels(use_in, channels);
    matching
        .iter()
        .find(|c| c.kind != ChannelKind::Tutti)
        .or_else(|| matching.first())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::ChannelView;

    fn channel(id: &str, name: &str, tags: &[&str]) -> ChannelView {
        ChannelView {
            id: id.into(),
            name: name.into(),
            kind: if id == "tutti" {
                ChannelKind::Tutti
            } else {
                ChannelKind::Standard
            },
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            ..ChannelView::default()
        }
    }

    fn channels() -> Vec<ChannelView> {
        vec![
            channel("tutti", "Tutti", &[]),
            channel("c1", "acme", &["work"]),
            channel("c2", "acme-infra", &["work", "gpu"]),
            channel("c3", "sayso", &["personal"]),
            channel("c4", "hiring", &["work"]),
        ]
    }

    fn tags(t: &[&str]) -> UseInView {
        UseInView::Tags(t.iter().map(|t| (*t).to_owned()).collect())
    }

    fn connection(id: &str, name: &str, use_in: UseInView) -> ConnectionView {
        ConnectionView {
            id: id.into(),
            name: name.into(),
            handle: consort_proto::model::connection_handle(name),
            service: "gmail".into(),
            service_name: "Gmail".into(),
            identity: Some(format!("{id}@example.com")),
            use_in,
            ..ConnectionView::default()
        }
    }

    #[test]
    fn the_scope_matches_the_hub_rule() {
        let all = channels();
        let names = |u: &UseInView| -> Vec<String> {
            matching_channels(u, &all)
                .iter()
                .map(|c| c.name.clone())
                .collect()
        };
        assert_eq!(names(&UseInView::EveryChannel).len(), 5);
        assert_eq!(names(&tags(&["work"])), ["acme", "acme-infra", "hiring"]);
        assert_eq!(names(&tags(&["work", "gpu"])), ["acme-infra"]);
        assert!(names(&tags(&[])).is_empty(), "empty tags match nothing");
        assert_eq!(
            names(&UseInView::Channels(vec!["c3".into(), "c1".into()])),
            ["acme", "sayso"]
        );
        // Tutti with no tags is not "tagged" anything.
        let mut tagged_tutti = all.clone();
        tagged_tutti[0].tags = vec!["work".into()];
        assert!(use_in_matches(&tags(&["work"]), &tagged_tutti[0]));
        assert!(!use_in_matches(&tags(&["work"]), &all[0]));
    }

    #[test]
    fn the_scope_agrees_with_the_proto_rule() {
        use consort_proto::{ids::ChannelId, model::UseIn};
        let ids: Vec<ChannelId> = (0..3).map(|_| ChannelId::new()).collect();
        let cases: Vec<(UseIn, UseInView)> = vec![
            (UseIn::EveryChannel, UseInView::EveryChannel),
            (
                UseIn::Tags {
                    tags: vec!["work".into()],
                },
                tags(&["work"]),
            ),
            (
                UseIn::Channels {
                    channel_ids: vec![ids[1]],
                },
                UseInView::Channels(vec![ids[1].to_string()]),
            ),
        ];
        for (proto, view) in cases {
            for (i, (tutti, channel_tags)) in [
                (true, vec![]),
                (true, vec!["work".to_owned()]),
                (false, vec!["work".to_owned()]),
                (false, vec![]),
            ]
            .into_iter()
            .enumerate()
            {
                let id = ids[i % 3];
                let channel = ChannelView {
                    id: id.to_string(),
                    kind: if tutti {
                        ChannelKind::Tutti
                    } else {
                        ChannelKind::Standard
                    },
                    tags: channel_tags.clone(),
                    ..ChannelView::default()
                };
                assert_eq!(
                    use_in_matches(&view, &channel),
                    proto.matches(id, tutti, &channel_tags),
                    "{view:?} on {channel_tags:?}, tutti {tutti}"
                );
            }
        }
    }

    #[test]
    fn a_scope_of_only_tutti_is_told_apart_from_a_longer_list() {
        let only = UseInView::Channels(vec!["t".into()]);
        assert!(use_in_is_only(&only, "t"));
        assert!(!use_in_is_only(&only, "x"));
        assert!(!use_in_is_only(
            &UseInView::Channels(vec!["t".into(), "x".into()]),
            "t"
        ));
        assert!(!use_in_is_only(&UseInView::EveryChannel, "t"));
    }

    #[test]
    fn use_in_labels_read_as_the_design() {
        let all = channels();
        assert_eq!(
            use_in_short(&UseInView::EveryChannel, &all),
            "every channel"
        );
        assert_eq!(use_in_short(&tags(&["work"]), &all), "channels tagged work");
        assert_eq!(
            use_in_short(&tags(&["work", "gpu"]), &all),
            "channels tagged work and gpu"
        );
        assert_eq!(
            use_in_short(&UseInView::Channels(vec!["c1".into()]), &all),
            "acme only"
        );
        assert_eq!(
            use_in_short(&UseInView::Channels(vec!["c1".into(), "c4".into()]), &all),
            "acme and hiring"
        );
        assert_eq!(
            use_in_detail(&tags(&["work"]), &all),
            "3 channels: acme, acme-infra, hiring. Their work runs only on workers tagged work."
        );
        assert_eq!(
            use_in_detail(&UseInView::EveryChannel, &all),
            "Every channel. 5 channels."
        );
        assert_eq!(
            use_in_detail(&tags(&["design"]), &all),
            "No channel is tagged design yet."
        );
        assert_eq!(
            use_in_match_note(&tags(&["work"]), &all),
            "3 channels match: acme, acme-infra, hiring."
        );
        assert_eq!(
            use_in_sentence(&UseInView::EveryChannel, &all),
            "Used in every channel."
        );
    }

    #[test]
    fn the_approvals_read_as_one_sentence() {
        assert_eq!(
            approvals_sentence(&ToolApprovalsView::default()),
            "Read runs without asking. Changes and deletes ask you."
        );
        let loose = ToolApprovalsView::default()
            .with(ToolKindView::Change, ApprovalRuleView::RunWithoutAsking);
        assert_eq!(
            approvals_sentence(&loose),
            "Read and changes run without asking. Deletes ask you."
        );
        let once = ToolApprovalsView::default()
            .with(ToolKindView::Delete, ApprovalRuleView::AskOncePerTask);
        assert_eq!(
            approvals_sentence(&once),
            "Read runs without asking. Changes ask you. Deletes ask you once per task."
        );
        assert_eq!(
            rule_note(ToolKindView::Change, ApprovalRuleView::AskEveryTime),
            "Create and change tools ask every time"
        );
    }

    #[test]
    fn the_effective_set_has_the_exceptions_and_the_items_not_given() {
        let mut state = ViewState {
            channels: channels(),
            ..ViewState::default()
        };
        state.connections = vec![
            connection("con_work", "Gmail Work", tags(&["work"])),
            connection("con_me", "Gmail Personal", UseInView::EveryChannel),
            connection("con_notion", "Notion", tags(&["personal"])),
        ];
        state.channels[1].disabled_connections = vec!["con_me".into()];
        let set = channel_extensions(&state, "c1").unwrap();
        let rows: Vec<(&str, bool, bool)> = set
            .connectors
            .iter()
            .map(|c| (c.name.as_str(), c.given, c.enabled))
            .collect();
        assert_eq!(
            rows,
            [
                ("Gmail Work", true, true),
                ("Gmail Personal", true, false),
                ("Notion", false, false)
            ]
        );
        assert_eq!(set.enabled_connector_ids(), ["con_work"]);
        assert_eq!(
            set.connectors[0].detail,
            "con_work@example.com. Used in channels tagged work."
        );
        assert_eq!(
            set.connectors[1].detail,
            "Off in acme. Every other channel still has it."
        );
        assert_eq!(
            set.connectors[2].detail,
            "Not in acme. It is used in channels tagged personal."
        );
        // The context block names only the enabled ones.
        let block = connectors_context(&state, "c1");
        assert!(block.contains("gmail_work: Gmail, con_work@example.com"));
        assert!(!block.contains("gmail_personal"));
        // Tutti sees only "every channel" items.
        let tutti = channel_extensions(&state, "tutti").unwrap();
        assert_eq!(tutti.enabled_connector_ids(), ["con_me"]);
    }

    #[test]
    fn a_skill_that_is_off_is_given_nowhere() {
        let mut state = ViewState {
            channels: channels(),
            ..ViewState::default()
        };
        state.skills = vec![
            crate::skills::SkillView {
                id: "skl_on".into(),
                name: "pdf".into(),
                enabled: true,
                ..Default::default()
            },
            crate::skills::SkillView {
                id: "skl_off".into(),
                name: "browser".into(),
                enabled: false,
                ..Default::default()
            },
        ];
        state.channels[1].disabled_skills = vec![];
        let set = channel_extensions(&state, "c1").unwrap();
        assert_eq!(set.enabled_skill_ids(), ["skl_on"]);
        assert!(!set.skills[1].given);
        assert_eq!(
            set.skills[1].detail,
            "Off everywhere. Turn it on in Settings > Skills."
        );
    }

    #[test]
    fn services_filter_by_search_and_category() {
        let service = |slug: &str, name: &str, cat: &str| ServiceView {
            slug: slug.into(),
            name: name.into(),
            description: format!("{name} tools"),
            categories: vec![cat.into()],
            ..ServiceView::default()
        };
        let all = vec![
            service("gmail", "Gmail", "email"),
            service("github", "GitHub", "developer"),
            service("jira", "Jira", "developer"),
        ];
        let names = |found: Vec<&ServiceView>| -> Vec<String> {
            found.iter().map(|s| s.name.clone()).collect()
        };
        assert_eq!(names(filter_services(&all, "", None)).len(), 3);
        assert_eq!(names(filter_services(&all, "G", None)), ["Gmail", "GitHub"]);
        assert_eq!(
            names(filter_services(&all, "", Some("developer"))),
            ["GitHub", "Jira"]
        );
        assert_eq!(
            names(filter_services(&all, "hub", Some("developer"))),
            ["GitHub"]
        );
    }

    #[test]
    fn the_connect_dialog_names_the_accounts_that_exist() {
        let mut state = ViewState {
            connections: vec![connection("a", "Gmail Personal", UseInView::EveryChannel)],
            ..ViewState::default()
        };
        let existing = connections_of_service(&state, "gmail");
        assert_eq!(
            connect_intro("Gmail", &existing),
            "You already have Gmail Personal. Give this account a name the assistant can tell apart."
        );
        assert_eq!(suggested_connection_name("Gmail", &existing), "Gmail");
        state
            .connections
            .push(connection("b", "gmail", UseInView::EveryChannel));
        let existing = connections_of_service(&state, "gmail");
        assert_eq!(suggested_connection_name("Gmail", &existing), "");
        assert_eq!(
            handle_for("Gmail Personal", &state, None),
            "gmail_personal_2"
        );
        assert_eq!(
            handle_for("Gmail Personal", &state, Some("a")),
            "gmail_personal"
        );
    }

    #[test]
    fn the_usage_line() {
        let composio = ComposioView {
            used: Some(12_340),
            limit: Some(100_000),
            plan: Some("Hobby".into()),
            ..ComposioView::default()
        };
        assert_eq!(
            composio.usage_line().as_deref(),
            Some("12,340 of 100,000 tool calls this month. Hobby plan.")
        );
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }

    #[test]
    fn a_row_says_the_identity_and_the_scope_or_the_problem() {
        let all = channels();
        let mut c = connection("x", "Gmail Work", tags(&["work"]));
        assert_eq!(
            connection_row_detail(&c, &all),
            "x@example.com, channels tagged work"
        );
        c.status = ConnectionStatusView::NeedsSignIn { message: None };
        assert_eq!(connection_row_detail(&c, &all), "Needs a new sign-in");
        assert_eq!(monogram(" linear"), "L");
    }
}
