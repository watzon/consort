//! View state: what a client draws. The reducers build it from the hub's
//! entities, and the mock builds it from fixed content. Every type is a plain
//! record or enum, so UniFFI can export it to the mobile apps.
//!
//! Labels that need the user's clock or language ("9:12 AM", "Today",
//! "2 min") are made here, so every client shows the same words.

/// The name that every client shows for the main channel
/// (`ChannelKind::Tutti` in code). See `CONTEXT.md`.
pub const TUTTI: &str = "Tutti";

/// Everything a client draws, at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub connection: Connection,
    pub user: UserView,
    pub sidebar: SidebarView,
    /// Every channel, Tutti first.
    pub channels: Vec<ChannelView>,
    pub groups: Vec<GroupView>,
    /// The channels that the user archived, by name. They have no timeline
    /// in this state. The sidebar lists them with a way to bring them back.
    pub archived_channels: Vec<ArchivedChannelView>,
    pub timelines: Vec<TimelineView>,
    pub tasks: Vec<TaskView>,
    pub threads: Vec<TaskThreadView>,
    pub tasks_panel: TasksPanelView,
    pub workers: Vec<WorkerView>,
    /// Every worker tag: the user tags of all workers first, then the
    /// detected tags, each once and sorted. Tag fields offer these.
    pub tags: Vec<String>,
    /// The model picker's list: one group per signed-in account label, in
    /// the order of the inventory, with the models of that account on every
    /// worker where it is signed in. See [`crate::picker`].
    pub model_groups: Vec<ModelGroupView>,
    /// The models that the user starred, from the hub's preferences, in the
    /// order they were starred. Every client shows the same ones. A favorite
    /// whose account or model is gone stays here, and the picker hides it
    /// (see [`crate::picker::favorite_rows`]).
    pub favorite_models: Vec<FavoriteModelView>,
    /// The sign-ins that this client started, newest last. A sign-in that
    /// another client started does not show (see DECISIONS row 269).
    pub sign_ins: Vec<SignInView>,
    /// The agents of the ACP registry, for the Add account dialog. The
    /// client loads them with [`crate::Command::LoadAcpRegistry`].
    pub acp_registry: AcpRegistryView,
    /// Set while the Add worker dialog waits for a pairing code.
    pub worker_pairing: Option<PairingCodeView>,
    /// Messages that waited in the outbox and that the hub then refused, or
    /// that could not go to the hub they were written for. The text is here
    /// so the user can copy it or put it back in the message box. A failure
    /// stays until the user dismisses it with
    /// [`crate::Command::DismissSendFailure`].
    pub send_failures: Vec<SendFailureView>,
    /// The connected accounts of Composio services, oldest first.
    pub connections: Vec<crate::connectors::ConnectionView>,
    /// The installed skills, oldest first.
    pub skills: Vec<crate::skills::SkillView>,
    /// The Composio key and plan. The client loads it with
    /// [`crate::Command::LoadComposioStatus`].
    pub composio: crate::connectors::ComposioView,
    /// The connector catalog of the Add connector dialog.
    pub connector_catalog: crate::connectors::CatalogView,
    /// The tools of the services that the client loaded.
    pub service_tools: Vec<crate::connectors::ServiceToolsView>,
    /// The skill directory search of the Add skill dialog.
    pub skill_directory: crate::skills::SkillDirectoryView,
    /// What the assistant of each channel does now, for the Three voices
    /// mark and its activity line. A channel that is not here is idle.
    pub activity: Vec<crate::activity::ChannelActivity>,
    /// The files that the user attached and that are not sent yet, of every
    /// composer, reply box and Files tab. See [`crate::uploads`].
    pub uploads: Vec<crate::uploads::UploadView>,
    /// The files of the channels whose files are loaded.
    pub channel_files: Vec<crate::files::ChannelFileView>,
    /// The channels whose files are loaded.
    pub files_loaded: Vec<String>,
    /// Settings > Hub, Settings > Clients, the pairing dialog, and what
    /// needs the user for notifications. See [`crate::admin`].
    pub admin: crate::admin::AdminView,
    /// Every routine of every channel. See [`crate::routines`].
    pub routines: Vec<crate::routines::RoutineView>,
    /// The memory notes of the loaded scopes, also the ones in Recently
    /// deleted. See [`crate::memory`].
    pub memory_notes: Vec<crate::memory::MemoryNoteView>,
    /// The memory scopes (`global` or a channel ID) whose notes are loaded.
    pub memory_loaded: Vec<String>,
    /// The scopes whose Recently deleted notes are loaded.
    pub memory_deleted_loaded: Vec<String>,
}

/// A message that the hub refused after it left the composer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SendFailureView {
    /// The `client_message_id` of the message.
    pub id: String,
    pub channel_id: String,
    /// The task whose thread the message was a reply in. `None` for a
    /// message in the timeline.
    pub task_id: Option<String>,
    pub channel_name: String,
    /// The text that the user wrote.
    pub body: String,
    /// What happened and what to do next.
    pub message: String,
}

impl ViewState {
    pub fn channel(&self, id: &str) -> Option<&ChannelView> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn tutti(&self) -> Option<&ChannelView> {
        self.channels.iter().find(|c| c.kind == ChannelKind::Tutti)
    }

    /// The unread count of a channel, from the sidebar. 0 for a channel
    /// that is not there.
    pub fn unread(&self, channel_id: &str) -> u32 {
        if self.sidebar.tutti.id == channel_id {
            return self.sidebar.tutti.unread;
        }
        self.sidebar
            .groups
            .iter()
            .flat_map(|g| g.channels.iter())
            .chain(self.sidebar.ungrouped.iter())
            .find(|c| c.id == channel_id)
            .map(|c| c.unread)
            .unwrap_or(0)
    }

    /// What the assistant of a channel does now. Idle when nothing is known.
    pub fn activity(&self, channel_id: &str) -> crate::activity::ActivityView {
        self.activity
            .iter()
            .find(|a| a.channel_id == channel_id)
            .map(|a| a.activity.clone())
            .unwrap_or_default()
    }

    pub fn timeline(&self, channel_id: &str) -> Option<&TimelineView> {
        self.timelines.iter().find(|t| t.channel_id == channel_id)
    }

    pub fn task(&self, id: &str) -> Option<&TaskView> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn thread(&self, task_id: &str) -> Option<&TaskThreadView> {
        self.threads.iter().find(|t| t.task_id == task_id)
    }

    pub fn worker(&self, id: &str) -> Option<&WorkerView> {
        self.workers.iter().find(|w| w.id == id)
    }

    pub fn connection(&self, id: &str) -> Option<&crate::connectors::ConnectionView> {
        self.connections.iter().find(|c| c.id == id)
    }

    pub fn skill(&self, id: &str) -> Option<&crate::skills::SkillView> {
        self.skills.iter().find(|s| s.id == id)
    }

    /// The tools of a service, once loaded.
    pub fn tools_of(&self, service: &str) -> Option<&crate::connectors::ServiceToolsView> {
        self.service_tools.iter().find(|t| t.service == service)
    }

    pub fn sign_in(&self, id: &str) -> Option<&SignInView> {
        self.sign_ins.iter().find(|s| s.id == id)
    }

    /// The workers that have every tag in `tags`, user or detected. With no
    /// tags, every worker matches.
    pub fn workers_with_tags(&self, tags: &[String]) -> Vec<&WorkerView> {
        self.workers.iter().filter(|w| w.has_tags(tags)).collect()
    }

    /// The account names already used for a provider on workers other than
    /// `worker_id`, each with the workers that use it. The Add account
    /// dialog offers them, so the same account on several workers gets one
    /// name.
    pub fn account_names(&self, harness: &str, worker_id: &str) -> Vec<AccountNameView> {
        let mut names: Vec<AccountNameView> = Vec::new();
        for worker in self.workers.iter().filter(|w| w.id != worker_id) {
            for account in worker.accounts.iter().filter(|a| a.harness == harness) {
                match names.iter_mut().find(|n| n.label == account.label) {
                    Some(name) => {
                        if !name.workers.contains(&worker.name) {
                            name.workers.push(worker.name.clone());
                        }
                    }
                    None => names.push(AccountNameView {
                        label: account.label.clone(),
                        workers: vec![worker.name.clone()],
                    }),
                }
            }
        }
        names
    }

    /// The group and the model of the picker with this account label and
    /// model ID.
    pub fn model_choice(
        &self,
        account_label: &str,
        model_id: &str,
    ) -> Option<(&ModelGroupView, &ModelChoiceView)> {
        let group = self
            .model_groups
            .iter()
            .find(|g| g.account_label == account_label)?;
        let model = group.models.iter().find(|m| m.id == model_id)?;
        Some((group, model))
    }

    /// The effort levels that the model of a selection offers. `None` when
    /// the model has no effort option, or when no worker reports it now.
    pub fn effort_for(&self, selection: &SelectionView) -> Option<&EffortView> {
        self.model_choice(&selection.account_label, &selection.model_id)
            .and_then(|(_, model)| model.effort.as_ref())
    }

    /// Whether the user starred the model with this account label and
    /// model ID.
    pub fn is_favorite(&self, account_label: &str, model_id: &str) -> bool {
        self.favorite_models
            .iter()
            .any(|f| f.account_label == account_label && f.model_id == model_id)
    }

    /// The provider logo of the account with this label. Accounts with one
    /// label on several workers are one account, so the first one counts.
    pub fn account_logo(&self, label: &str) -> Option<&ProviderLogo> {
        self.workers
            .iter()
            .flat_map(|w| w.accounts.iter())
            .find(|a| a.label == label)
            .map(|a| &a.logo)
    }
}

/// The folder name that a profile folder shows in the Add account dialog:
/// the account name in lowercase, with a `-` between words, such as
/// `claude-work`. An empty or odd name gives `account`.
pub fn profile_slug(label: &str) -> String {
    let mut slug = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    // The worker caps the folder name at 48 characters, so the dialog shows
    // the same name.
    let slug: String = slug.trim_matches('-').chars().take(48).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "account".to_owned()
    } else {
        slug.to_owned()
    }
}

/// A name that an account of one provider already has on other workers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountNameView {
    pub label: String,
    /// The names of the workers that have an account with this name.
    pub workers: Vec<String>,
}

/// The value of a model option.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionOption {
    Bool(bool),
    Text(String),
}

/// A model selection: an account label, a model and its options, such as
/// "Claude Personal, Opus 5.5". The hub needs the model ID, and the user
/// reads the model name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectionView {
    pub account_label: String,
    /// The model ID, as the account reports it. Empty for an account with
    /// no model list: the harness then uses its default.
    pub model_id: String,
    /// The model name to show. The ID when the name is not known.
    pub model_name: String,
    /// Option values by option ID, such as `effort`.
    pub options: Vec<(String, SelectionOption)>,
}

impl SelectionView {
    /// The words to show, such as "Claude Personal, Opus 5.5".
    pub fn label(&self) -> String {
        if self.model_id.is_empty() {
            self.account_label.clone()
        } else {
            format!("{}, {}", self.account_label, self.model_name)
        }
    }

    /// A stable key for a select menu: the account label and the model ID.
    /// Options are not part of it, so a pick keeps the options it had.
    pub fn key(&self) -> String {
        format!("{}\u{1f}{}", self.account_label, self.model_id)
    }

    /// The effort value of the selection, if it has one.
    pub fn effort(&self) -> Option<&str> {
        self.options.iter().find_map(|(id, value)| match value {
            SelectionOption::Text(v) if id == crate::picker::EFFORT => Some(v.as_str()),
            _ => None,
        })
    }
}

/// One account in the model picker. Accounts with one label on several
/// workers are one group.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelGroupView {
    pub account_label: String,
    /// The provider name, such as "Claude Code".
    pub provider: String,
    pub logo: ProviderLogo,
    /// The names of the workers where this account is signed in.
    pub workers: Vec<String>,
    /// The union of the models of the account on those workers, in the
    /// order that the workers report them.
    pub models: Vec<ModelChoiceView>,
}

/// A model that the user starred: an account label and a model ID.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FavoriteModelView {
    pub account_label: String,
    pub model_id: String,
}

/// One model in the picker.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelChoiceView {
    /// The model ID, as the account reports it.
    pub id: String,
    /// The name to show.
    pub name: String,
    /// The effort levels. `None` when the model has no effort option.
    pub effort: Option<EffortView>,
}

/// The effort levels that a provider reports for one model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffortView {
    /// The levels, lowest first, as the provider orders them.
    pub levels: Vec<EffortLevel>,
    /// The level that the provider uses when the selection has none. `None`
    /// when the provider does not say.
    pub default: Option<String>,
}

/// One effort level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffortLevel {
    /// The value to send.
    pub value: String,
    /// The text to show, such as "High".
    pub label: String,
}

/// Where the coordinator selection of a channel comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionSource {
    /// The channel has its own selection. Tutti's own selection also
    /// counts as this.
    #[default]
    Channel,
    /// The channel has none, so it uses Tutti's.
    Tutti,
}

/// The coordinator selection that a channel uses, and where it comes from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffectiveSelection {
    pub selection: SelectionView,
    pub source: SelectionSource,
}

/// The state of the link to the hub.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Connection {
    /// No hub is paired yet. The app shows onboarding.
    #[default]
    NotPaired,
    Connecting,
    Online,
    /// The link dropped. The client tries again after `retry_in_ms`.
    Offline {
        retry_in_ms: u64,
        reason: String,
    },
    /// The hub refused the client. The user must pair again.
    Refused {
        reason: String,
    },
    /// Mock data, with no hub.
    Demo,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserView {
    pub name: String,
    pub initials: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ChannelKind {
    Tutti,
    #[default]
    Standard,
}

/// The status of a task, in the order the user moves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TaskStatus {
    Scheduled,
    #[default]
    UpNext,
    Working,
    NeedsInput,
    Review,
    Failed,
    Resolved,
}

impl TaskStatus {
    /// The status as the UI names it.
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Scheduled => "Scheduled",
            TaskStatus::UpNext => "Up next",
            TaskStatus::Working => "Working",
            TaskStatus::NeedsInput => "Needs input",
            TaskStatus::Review => "Ready for review",
            TaskStatus::Failed => "Failed",
            TaskStatus::Resolved => "Resolved",
        }
    }

    /// Needs input and Ready for review are the user's turn.
    pub fn is_your_turn(self) -> bool {
        matches!(self, TaskStatus::NeedsInput | TaskStatus::Review)
    }

    /// Active tasks show in the Active tab of the Tasks panel.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            TaskStatus::Working | TaskStatus::NeedsInput | TaskStatus::Review | TaskStatus::Failed
        )
    }

    /// The rank of a status for the sidebar glyph: lower is more urgent.
    /// Statuses that the sidebar does not show have no rank.
    pub fn sidebar_rank(self) -> Option<u8> {
        match self {
            TaskStatus::NeedsInput => Some(0),
            TaskStatus::Review => Some(1),
            TaskStatus::Failed => Some(2),
            TaskStatus::Working => Some(3),
            _ => None,
        }
    }
}

/// The icon of a channel, as a Lucide icon name.
pub type IconName = String;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarView {
    pub tutti: SidebarChannel,
    pub groups: Vec<SidebarGroup>,
    /// Channels with no group, after the groups.
    pub ungrouped: Vec<SidebarChannel>,
    pub workers_online: u32,
    pub workers_total: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarGroup {
    pub id: String,
    pub name: String,
    pub collapsed: bool,
    pub channels: Vec<SidebarChannel>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarChannel {
    pub id: String,
    pub name: String,
    pub icon: IconName,
    /// The most urgent task status in the channel, from Needs input, Ready for
    /// review, Failed and Working.
    pub status: Option<TaskStatus>,
    /// True when something waits for the user, or there are unread messages.
    /// The name is then in `text.primary`.
    pub attention: bool,
    pub unread: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupView {
    pub id: String,
    pub name: String,
}

/// A channel that the user archived.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArchivedChannelView {
    pub id: String,
    pub name: String,
    pub icon: IconName,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Isolation {
    InPlace,
    #[default]
    Worktree,
    Clone,
}

impl Isolation {
    pub fn label(self) -> &'static str {
        match self {
            Isolation::InPlace => "In place",
            Isolation::Worktree => "Worktree",
            Isolation::Clone => "Clone",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelView {
    pub id: String,
    pub kind: ChannelKind,
    pub name: String,
    pub purpose: String,
    pub instructions: String,
    pub icon: IconName,
    pub group_id: Option<String>,
    /// The worker the coordinator runs on, for the composer hint.
    pub coordinator_worker: Option<String>,
    /// The model of the coordinator, with the fallback to Tutti's.
    /// `None` when neither channel has one: placement then chooses.
    pub coordinator_selection: Option<EffectiveSelection>,
    pub repos: Vec<RepoView>,
    pub preferred_workers: Vec<String>,
    pub task_selection: Option<SelectionView>,
    pub default_isolation: Isolation,
    /// The worker tags that the channel requires: all work in it runs only
    /// on workers that have every one of them.
    pub tags: Vec<String>,
    /// The connections that the channel turned off, though their Use in
    /// gives them to it.
    pub disabled_connections: Vec<String>,
    /// The skills that the channel turned off.
    pub disabled_skills: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepoView {
    pub id: String,
    pub name: String,
    pub checkouts: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimelineView {
    pub channel_id: String,
    pub items: Vec<TimelineItem>,
    /// Older messages exist on the hub.
    pub has_older: bool,
    /// The newest messages of the channel are in the cache. A channel that
    /// is not loaded shows no messages, which does not mean it has none.
    /// Nothing may be marked read while this is false.
    pub loaded: bool,
}

// Items live in one Vec per channel, so a large variant costs little, and a
// box would make every match harder to read.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum TimelineItem {
    Day(DayView),
    Message(MessageView),
    TaskCard(TaskCardView),
    /// An approval in the timeline itself, from the channel coordinator.
    Approval(ApprovalCardView),
    /// A decision in the timeline itself, from the channel coordinator.
    Decision(DecisionCardView),
    /// A system notice, such as "The assistant is not available". It is a
    /// quiet line, not a message from anyone.
    Notice(NoticeView),
}

impl TimelineItem {
    /// A stable key for the item, for element IDs and caches.
    pub fn key(&self) -> &str {
        match self {
            TimelineItem::Day(day) => &day.key,
            TimelineItem::Message(m) => &m.id,
            TimelineItem::TaskCard(c) => &c.message_id,
            TimelineItem::Approval(r) => &r.message_id,
            TimelineItem::Decision(r) => &r.message_id,
            TimelineItem::Notice(n) => &n.message_id,
        }
    }
}

/// A system notice in a timeline. The hub sends the body as JSON with a
/// kind and a text (DECISIONS row 66). A body that is not that JSON is
/// shown as it is, with an empty kind.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoticeView {
    pub message_id: String,
    pub time_label: String,
    /// When the hub wrote it, in Unix milliseconds. The timeline uses it
    /// for its time separators.
    pub created_at: i64,
    /// The kind, such as `turn_failed`. Empty when the body did not parse.
    pub kind: String,
    pub text: String,
    /// For a `memory_saved` notice: the note that the assistant saved.
    pub memory: Option<MemorySavedView>,
}

/// The memory saved line: the assistant saved a memory note. The timeline
/// shows it as a quiet line with the note path as a link.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemorySavedView {
    /// `global` or a channel ID.
    pub scope: String,
    pub path: String,
    pub version: u64,
    /// True when the write made the note.
    pub created: bool,
    /// The title of the task or the name of the channel whose memory
    /// suggestion the note keeps. Empty when it came from no suggestion.
    pub suggested_by: String,
}

/// An approval that the coordinator posted in the timeline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApprovalCardView {
    pub message_id: String,
    pub speaker: Speaker,
    pub time_label: String,
    /// When the hub wrote it, in Unix milliseconds.
    pub created_at: i64,
    pub approval: ApprovalView,
}

/// A decision that the coordinator posted in the timeline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecisionCardView {
    pub message_id: String,
    pub speaker: Speaker,
    pub time_label: String,
    /// When the hub wrote it, in Unix milliseconds.
    pub created_at: i64,
    pub decision: DecisionView,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DayView {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Speaker {
    You,
    #[default]
    Consort,
    Task,
    System,
}

impl Speaker {
    pub fn label(self) -> &'static str {
        match self {
            Speaker::You => "You",
            Speaker::Consort => "Consort",
            Speaker::Task => "Task",
            Speaker::System => "Consort",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageView {
    pub id: String,
    pub channel_id: String,
    pub speaker: Speaker,
    pub time_label: String,
    pub created_at: i64,
    /// A title above the body, for a brief from a routine.
    pub title: Option<String>,
    /// Markdown.
    pub body: String,
    /// Task references under the body, for a brief from a routine.
    pub items: Vec<BriefItem>,
    pub edited: bool,
    pub reply_count: u32,
    /// The message is still on its way to the hub.
    pub pending: bool,
    /// The files attached to the message.
    pub attachments: Vec<crate::files::AttachmentView>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BriefItem {
    pub task_id: Option<String>,
    pub status: TaskStatus,
    pub text: String,
    pub channel_name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkerChip {
    pub name: String,
    pub online: bool,
    pub account: String,
}

/// The task card in a timeline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskCardView {
    pub message_id: String,
    pub task_id: String,
    pub time_label: String,
    /// When the hub wrote the card, in Unix milliseconds.
    pub created_at: i64,
    pub title: String,
    pub status: TaskStatus,
    /// The channel name, shown when the card is outside its channel.
    pub channel_name: Option<String>,
    pub worker: Option<WorkerChip>,
    /// A pending approval, shown inline with its buttons.
    pub approval: Option<ApprovalView>,
    /// The line for a collapsed card, such as "Resolved, 6 replies".
    pub summary: String,
    pub reply_count: u32,
    /// The newest update, for a working or failed card.
    pub latest: Option<String>,
    /// How much the task may do without asking.
    pub approval_mode: ApprovalMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ApprovalChoice {
    #[default]
    AllowOnce,
    AllowForTask,
    Deny,
}

impl ApprovalChoice {
    pub fn label(self) -> &'static str {
        match self {
            ApprovalChoice::AllowOnce => "Allow once",
            ApprovalChoice::AllowForTask => "Allow for this task",
            ApprovalChoice::Deny => "Deny",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RequestState {
    #[default]
    Pending,
    /// Sent to the hub, no answer yet.
    Sending,
    Answered,
    Expired,
    /// Answered or canceled before this client loaded it. Only the message
    /// is left, so the answer is not known.
    Closed,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApprovalView {
    pub id: String,
    /// The task that asks. `None` for the channel coordinator.
    pub task_id: Option<String>,
    /// One line, such as "Codex wants to run a command".
    pub summary: String,
    /// The folder of a command, shown in mono after "in".
    pub folder: Option<String>,
    /// The command or the change, shown in a selectable block.
    pub detail: Option<String>,
    pub choices: Vec<ApprovalChoice>,
    pub state: RequestState,
    /// The answer, once given.
    pub answer: Option<ApprovalChoice>,
    /// The harness's own words for its "allow always" option, such as "Yes,
    /// and don't ask again for python3 commands". The scope of that option
    /// is the harness's, and it can be wider than this task, so the button
    /// shows these words and not our own.
    pub allow_always_label: Option<String>,
    /// The facts of a card that the hub made itself. `None` for an approval
    /// from a harness.
    pub card: Option<Box<crate::connectors::HubCardView>>,
    /// The hub's words for the buttons of a hub card, such as "Continue in
    /// browser" and "Not now".
    pub button_labels: Vec<(ApprovalChoice, String)>,
}

impl ApprovalView {
    /// The words on a choice's button. "Allow once" and "Deny" are ours.
    /// The wide choice shows what the harness says it does. When the
    /// harness gave no words, it is "Allow for this task", or "Always allow"
    /// when the coordinator asks, because the first reads wrong there.
    pub fn choice_label(&self, choice: ApprovalChoice) -> String {
        if let Some((_, label)) = self.button_labels.iter().find(|(c, _)| *c == choice) {
            return label.clone();
        }
        if choice == ApprovalChoice::AllowForTask {
            if let Some(label) = self
                .allow_always_label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
            {
                return label.to_owned();
            }
            if self.task_id.is_none() {
                return "Always allow".to_owned();
            }
        }
        choice.label().to_owned()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecisionOption {
    pub label: String,
    pub consequence: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecisionView {
    pub id: String,
    pub task_id: Option<String>,
    pub question: String,
    pub context: Option<String>,
    pub options: Vec<DecisionOption>,
    pub recommended: Option<u32>,
    pub allow_free_text: bool,
    pub state: RequestState,
    pub chosen: Option<u32>,
    pub answer_text: Option<String>,
}

/// A row of the Tasks panel, and the data of a task.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskView {
    pub id: String,
    pub channel_id: String,
    pub channel_name: String,
    pub title: String,
    pub status: TaskStatus,
    /// The line under the title, such as "Needs input. sayso on bigboi".
    /// [`crate::derive::refresh`] makes it from the fields below.
    pub meta: String,
    pub worker_name: Option<String>,
    pub account_label: Option<String>,
    /// Why the task is in its status, such as "Code signing failed" or
    /// "Starts Friday at 9:00 AM".
    pub status_reason: Option<String>,
    /// The age, such as "2 min" or "3 h".
    pub age_label: String,
    pub updated_at: i64,
    pub card_message_id: String,
    /// The approval mode that the task asked for. The worker may cap it.
    pub approval_mode: ApprovalMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TaskSectionKind {
    #[default]
    YourTurn,
    Working,
    Failed,
}

impl TaskSectionKind {
    pub fn label(self) -> &'static str {
        match self {
            TaskSectionKind::YourTurn => "Your turn",
            TaskSectionKind::Working => "Working",
            TaskSectionKind::Failed => "Failed",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskSection {
    pub kind: TaskSectionKind,
    pub task_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TasksPanelView {
    /// Tasks that need the user: Needs input and Ready for review.
    pub need_you: u32,
    pub active_count: u32,
    pub up_next_count: u32,
    pub scheduled_count: u32,
    pub resolved_count: u32,
    /// The Active tab: Your turn, Working, Failed. Empty sections are left out.
    pub active: Vec<TaskSection>,
    pub up_next: Vec<String>,
    pub scheduled: Vec<String>,
    pub resolved: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlacementView {
    pub worker_name: String,
    pub worker_os: String,
    pub worker_online: bool,
    /// Such as "Codex Personal, high effort".
    pub harness: String,
    pub isolation: Isolation,
    pub path: String,
    pub branch: Option<String>,
    /// The repo the workspace comes from. `None` when the task has no repo:
    /// it then runs in a new empty folder.
    pub repo: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PostBody {
    /// Markdown.
    Text(String),
    /// Session items folded into one line, such as "Ran 14 commands and
    /// changed 6 files".
    Activity {
        summary: String,
        items: Vec<String>,
    },
    Approval(ApprovalView),
    Decision(DecisionView),
    /// A system notice. The kind is empty when the body did not parse.
    Notice {
        kind: String,
        text: String,
    },
    /// The files attached to the post.
    Attachments(Vec<crate::files::AttachmentView>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThreadPost {
    pub id: String,
    pub speaker: Speaker,
    pub time_label: String,
    pub bodies: Vec<PostBody>,
    /// The post is still on its way to the hub.
    pub pending: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskThreadView {
    pub task_id: String,
    pub title: String,
    pub status: TaskStatus,
    /// Such as "1 decision, 1 approval".
    pub status_detail: String,
    /// Why a Failed or Up next task is in that status, in full, such as the
    /// error text from the worker. `None` for other statuses.
    pub reason: Option<String>,
    pub placement: Option<PlacementView>,
    pub posts: Vec<ThreadPost>,
    /// The brief that started the task, in full, as Markdown.
    pub brief: String,
    /// What the task said when it went to Ready for review, in full, as
    /// Markdown.
    pub review_summary: Option<String>,
    /// The client holds only the start of the brief and the summary, and it
    /// is loading the rest. The thread loads the full task when it opens.
    pub text_is_preview: bool,
    /// The approval mode that the task asked for. The worker may cap it.
    pub approval_mode: ApprovalMode,
    /// The model of the task, such as "Opus 5.5, Claude Personal, High
    /// effort". The thread composer shows it. `None` when the task has no
    /// selection and its channel and Tutti have none.
    pub model_label: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Os {
    MacOs,
    Windows,
    #[default]
    Linux,
}

impl Os {
    pub fn label(self) -> &'static str {
        match self {
            Os::MacOs => "macOS",
            Os::Windows => "Windows",
            Os::Linux => "Linux",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AuthState {
    SignedIn,
    /// An API key is set in the environment.
    KeyFound,
    SignedOut,
    Error,
    #[default]
    Unknown,
}

impl AuthState {
    pub fn label(self) -> &'static str {
        match self {
            AuthState::SignedIn => "Signed in",
            AuthState::KeyFound => "Key found",
            AuthState::SignedOut => "Signed out",
            AuthState::Error => "Sign-in error",
            AuthState::Unknown => "Not checked",
        }
    }

    pub fn works(self) -> bool {
        matches!(self, AuthState::SignedIn | AuthState::KeyFound)
    }
}

/// The logo of a provider. The app draws Claude Code and Gemini CLI in
/// their own colors, and the others in the text color.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum ProviderLogo {
    ClaudeCode,
    Codex,
    OpenCode,
    GeminiCli,
    /// An agent from the ACP registry: its registry ID, and its icon (an SVG
    /// that draws in `currentColor`) once the client has loaded it.
    Registry {
        id: String,
        svg: Option<String>,
    },
    /// A provider with no logo of its own: a custom command, or a harness
    /// that this client does not know. The app draws a neutral glyph.
    #[default]
    Generic,
}

/// The name and the logo of a harness, from its name in the inventory:
/// `claude`, `codex`, `opencode`, `gemini`, `pi`, `acp` or `acp:<id>`.
pub fn provider_of(harness: &str) -> (String, ProviderLogo) {
    match harness {
        "claude" | "claude-code" => ("Claude Code".into(), ProviderLogo::ClaudeCode),
        "codex" => ("Codex".into(), ProviderLogo::Codex),
        "opencode" => ("OpenCode".into(), ProviderLogo::OpenCode),
        "gemini" | "gemini-cli" => ("Gemini CLI".into(), ProviderLogo::GeminiCli),
        "pi" => ("Pi".into(), ProviderLogo::Generic),
        other => match other.strip_prefix("acp:").filter(|id| !id.is_empty()) {
            Some(id) => (
                id.to_owned(),
                ProviderLogo::Registry {
                    id: id.to_owned(),
                    svg: None,
                },
            ),
            None => ("ACP agent".into(), ProviderLogo::Generic),
        },
    }
}

/// Where the sign-in of an account is, as the account row shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AccountStatus {
    SignedIn,
    /// An API key is saved, so there is nothing to sign in to.
    Ready,
    /// The login expired. The user must sign in again.
    Expired,
    /// The worker could not check the login.
    Error,
    NotSignedIn,
    /// The worker installs the provider first.
    Installing,
    #[default]
    Unknown,
}

impl AccountStatus {
    pub fn label(self) -> &'static str {
        match self {
            AccountStatus::SignedIn => "Signed in",
            AccountStatus::Ready => "Ready",
            AccountStatus::Expired => "Expired",
            AccountStatus::Error => "Sign-in error",
            AccountStatus::NotSignedIn => "Not signed in",
            AccountStatus::Installing => "Installing",
            AccountStatus::Unknown => "Not checked",
        }
    }

    /// Tasks can use the account.
    pub fn works(self) -> bool {
        matches!(self, AccountStatus::SignedIn | AccountStatus::Ready)
    }

    /// The status is a problem: the dot and the words are in the failed
    /// color.
    pub fn is_problem(self) -> bool {
        matches!(self, AccountStatus::Expired | AccountStatus::Error)
    }

    /// The one action of an account row in this status.
    pub fn action(self) -> AccountAction {
        match self {
            AccountStatus::SignedIn => AccountAction::SignOut,
            AccountStatus::Expired | AccountStatus::Error => AccountAction::SignInAgain,
            AccountStatus::NotSignedIn | AccountStatus::Unknown => AccountAction::SignIn,
            AccountStatus::Ready | AccountStatus::Installing => AccountAction::None,
        }
    }
}

/// The action button of an account row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AccountAction {
    SignOut,
    SignIn,
    SignInAgain,
    #[default]
    None,
}

impl AccountAction {
    pub fn label(self) -> &'static str {
        match self {
            AccountAction::SignOut => "Sign out",
            AccountAction::SignIn => "Sign in",
            AccountAction::SignInAgain => "Sign in again",
            AccountAction::None => "",
        }
    }
}

/// A way to sign in to an account. It mirrors the proto's `SignInMethod`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SignInChoice {
    ClaudePlan,
    ClaudeConsole,
    ChatgptDevice,
    ChatgptBrowser,
    /// An API key in this variable.
    ApiKey {
        variable: String,
    },
    /// An ACP agent's own method.
    AcpAgent {
        method_id: String,
        name: String,
    },
    /// A command that the user runs on the worker.
    Terminal {
        command: String,
    },
}

impl SignInChoice {
    /// The name on the method tab.
    pub fn label(&self) -> String {
        match self {
            SignInChoice::ClaudePlan => "Claude plan".into(),
            SignInChoice::ClaudeConsole => "Console".into(),
            SignInChoice::ChatgptDevice => "ChatGPT".into(),
            SignInChoice::ChatgptBrowser => "ChatGPT in a browser".into(),
            SignInChoice::ApiKey { .. } => "API key".into(),
            SignInChoice::AcpAgent { name, .. } => name.clone(),
            SignInChoice::Terminal { .. } => "Terminal".into(),
        }
    }

    /// The method has a flow on the worker. An API key has none: the app
    /// saves the key, and a terminal method is a command for the user.
    pub fn has_flow(&self) -> bool {
        !matches!(
            self,
            SignInChoice::ApiKey { .. } | SignInChoice::Terminal { .. }
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HarnessAccountView {
    pub id: String,
    pub label: String,
    /// The harness, as the inventory names it, such as `claude`.
    pub harness: String,
    /// The provider name, such as "Claude Code".
    pub provider: String,
    pub logo: ProviderLogo,
    /// The plan or the problem, such as "Claude Max plan." or "API key saved
    /// on bigboi."
    pub detail: String,
    /// Code to show in mono inside the detail, such as an environment
    /// variable or a command.
    pub detail_code: Vec<String>,
    pub auth: AuthState,
    pub status: AccountStatus,
    /// The one action of the row. `None` when the worker offers no way to
    /// do it from the app.
    pub action: AccountAction,
    /// The ways the app may offer to sign in, in the worker's order.
    pub sign_in_methods: Vec<SignInChoice>,
    /// The names of the secret variables saved for the account.
    pub secret_names: Vec<String>,
    /// Who is signed in, when the harness tells.
    pub email: Option<String>,
    /// The worker starts the harness with the account's provider extras.
    pub extras_enabled: bool,
    /// What the account brings from the provider's side, when the worker
    /// can tell.
    pub extras: Option<crate::connectors::ProviderExtrasView>,
}

/// Where a managed tool is on a worker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ToolState {
    #[default]
    Missing,
    Installing,
    Ready,
    UpdateAvailable,
    Failed,
}

/// A managed tool on a worker, as the Installed tools list shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolView {
    /// The tool ID, such as `claude-code`. The install command takes it.
    pub id: String,
    pub name: String,
    pub logo: ProviderLogo,
    pub version: Option<String>,
    /// The newest version that the last check found.
    pub latest: Option<String>,
    pub state: ToolState,
    /// The line under the name: why an install failed, or that the tool is
    /// not installed yet.
    pub detail: Option<String>,
}

impl ToolView {
    /// The words or the button at the end of the row.
    pub fn action_label(&self) -> String {
        match self.state {
            ToolState::Ready => "Up to date".into(),
            ToolState::UpdateAvailable => match &self.latest {
                Some(latest) => format!("Update to {latest}"),
                None => "Update now".into(),
            },
            ToolState::Installing => "Installing".into(),
            ToolState::Missing => "Install now".into(),
            ToolState::Failed => "Try again".into(),
        }
    }

    /// The end of the row is a button.
    pub fn can_install(&self) -> bool {
        matches!(
            self.state,
            ToolState::UpdateAvailable | ToolState::Missing | ToolState::Failed
        )
    }
}

/// Where one sign-in flow is. It mirrors the proto's `SignInState`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SignInStep {
    #[default]
    Starting,
    OpenUrl {
        url: String,
    },
    DeviceCode {
        url: String,
        user_code: String,
    },
    NeedsCode {
        url: String,
    },
    Verifying,
    Succeeded {
        detail: Option<String>,
    },
    Failed {
        message: String,
    },
    Cancelled,
    Expired,
}

impl SignInStep {
    /// The flow has ended.
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            SignInStep::Succeeded { .. }
                | SignInStep::Failed { .. }
                | SignInStep::Cancelled
                | SignInStep::Expired
        )
    }
}

/// One sign-in that this client started.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignInView {
    pub id: String,
    pub worker_id: String,
    pub account_id: String,
    pub method: Option<SignInChoice>,
    pub step: SignInStep,
}

/// Whether the client has the ACP registry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RegistryLoad {
    #[default]
    NotLoaded,
    Loading,
    Loaded,
    /// What went wrong and what to do next.
    Failed(String),
}

/// An agent of the ACP registry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AcpAgentView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub logo: ProviderLogo,
    /// A worker can install it.
    pub supported: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AcpRegistryView {
    pub load: RegistryLoad,
    pub agents: Vec<AcpAgentView>,
}

impl AcpRegistryView {
    /// The agents whose name, ID or description holds `query`, ignoring
    /// case. Agents that a worker can install come first.
    pub fn search(&self, query: &str) -> Vec<&AcpAgentView> {
        let query = query.trim().to_lowercase();
        let mut found: Vec<&AcpAgentView> = self
            .agents
            .iter()
            .filter(|a| {
                query.is_empty()
                    || a.name.to_lowercase().contains(&query)
                    || a.id.to_lowercase().contains(&query)
                    || a.description.to_lowercase().contains(&query)
            })
            .collect();
        found.sort_by_key(|a| !a.supported);
        found
    }

    /// The logo of a registry agent, with its icon when it is loaded.
    pub fn logo(&self, id: &str) -> ProviderLogo {
        self.agents
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.logo.clone())
            .unwrap_or(ProviderLogo::Registry {
                id: id.to_owned(),
                svg: None,
            })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ApprovalMode {
    Ask,
    #[default]
    AutoEdits,
    Auto,
    FullAccess,
}

impl ApprovalMode {
    pub const ALL: [ApprovalMode; 4] = [
        ApprovalMode::Ask,
        ApprovalMode::AutoEdits,
        ApprovalMode::Auto,
        ApprovalMode::FullAccess,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ApprovalMode::Ask => "Ask",
            ApprovalMode::AutoEdits => "Auto edits",
            ApprovalMode::Auto => "Auto",
            ApprovalMode::FullAccess => "Full access",
        }
    }

    /// The mode as a short tag on a task, such as "Ask mode".
    pub fn tag(self) -> &'static str {
        match self {
            ApprovalMode::Ask => "Ask mode",
            ApprovalMode::AutoEdits => "Auto edits mode",
            ApprovalMode::Auto => "Auto mode",
            ApprovalMode::FullAccess => "Full access mode",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PolicyView {
    pub max_approval_mode: ApprovalMode,
    pub allowed_roots: Vec<String>,
    pub allow_shell: bool,
    /// The shell that commands run in, such as "PowerShell".
    pub shell_name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkerView {
    pub id: String,
    pub name: String,
    pub os: Os,
    pub online: bool,
    pub paused: bool,
    /// The detail in the sidebar, such as "Linux, VPS" or "Offline".
    pub sidebar_detail: String,
    /// The line in the Settings list, such as "Windows, 5 accounts".
    pub list_detail: String,
    /// The status line in the Settings detail.
    pub status_line: String,
    /// The user tags, set in a client.
    pub tags: Vec<String>,
    /// The tags that the worker found itself, such as `macos` or `ram-32gb`.
    pub detected_tags: Vec<String>,
    pub accounts: Vec<HarnessAccountView>,
    /// The managed tools, providers first.
    pub tools: Vec<ToolView>,
    /// Where the worker keeps its tools, such as `~/.consort/tools`.
    pub tools_folder: String,
    pub policy: PolicyView,
}

impl WorkerView {
    /// The worker has every tag in `tags`, as a user tag or a detected tag.
    pub fn has_tags(&self, tags: &[String]) -> bool {
        tags.iter()
            .all(|t| self.tags.contains(t) || self.detected_tags.contains(t))
    }

    pub fn account(&self, id: &str) -> Option<&HarnessAccountView> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// The tags of `all_tags` that this worker can still get: the ones that it
    /// has neither as a user tag nor as a detected tag. The order of
    /// `all_tags` stays.
    pub fn suggested_tags<'a>(&self, all_tags: &'a [String]) -> Vec<&'a String> {
        all_tags
            .iter()
            .filter(|t| !self.tags.contains(t) && !self.detected_tags.contains(t))
            .collect()
    }
}

/// A pairing code for a new worker.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PairingCodeView {
    pub code: String,
    pub hub_url: String,
    pub expires_at: i64,
}

#[cfg(test)]
mod profile_slug_tests {
    use super::profile_slug;

    /// The worker makes the folder with the same rule
    /// (`consort-worker/src/profile.rs`), so the dialog shows the real name.
    #[test]
    fn the_slug_matches_the_worker_rule() {
        assert_eq!(profile_slug("Claude Work"), "claude-work");
        assert_eq!(profile_slug("  --Codex (Personal)!  "), "codex-personal");
        assert_eq!(profile_slug("***"), "account");
        let long = "a".repeat(47) + " b";
        assert_eq!(profile_slug(&long), "a".repeat(47));
        assert_eq!(profile_slug(&"x".repeat(60)).len(), 48);
    }
}
