//! View state: what a client draws. The reducers build it from the hub's
//! entities, and the mock builds it from fixed content. Every type is a plain
//! record or enum, so UniFFI can export it to the mobile apps.
//!
//! Labels that need the user's clock or language ("9:12 AM", "Today",
//! "2 min") are made here, so every client shows the same words.

/// Everything a client draws, at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub connection: Connection,
    pub user: UserView,
    pub sidebar: SidebarView,
    /// Every channel, Everything first.
    pub channels: Vec<ChannelView>,
    pub groups: Vec<GroupView>,
    pub timelines: Vec<TimelineView>,
    pub tasks: Vec<TaskView>,
    pub threads: Vec<TaskThreadView>,
    pub tasks_panel: TasksPanelView,
    pub workers: Vec<WorkerView>,
    /// The harness accounts that tasks can use, as "label, model" choices.
    pub selections: Vec<String>,
    /// Set while the Add worker dialog waits for a pairing code.
    pub worker_pairing: Option<PairingCodeView>,
}

impl ViewState {
    pub fn channel(&self, id: &str) -> Option<&ChannelView> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn everything(&self) -> Option<&ChannelView> {
        self.channels
            .iter()
            .find(|c| c.kind == ChannelKind::Everything)
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
    Everything,
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
    pub everything: SidebarChannel,
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
    /// The harness account of the coordinator, for the composer hint.
    pub coordinator_account: Option<String>,
    pub repos: Vec<RepoView>,
    pub preferred_workers: Vec<String>,
    pub task_selection: Option<String>,
    pub default_isolation: Isolation,
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
}

// Items live in one Vec per channel, so a large variant costs little, and a
// box would make every match harder to read.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum TimelineItem {
    Day(DayView),
    Message(MessageView),
    TaskCard(TaskCardView),
}

impl TimelineItem {
    /// A stable key for the item, for element IDs and caches.
    pub fn key(&self) -> &str {
        match self {
            TimelineItem::Day(day) => &day.key,
            TimelineItem::Message(m) => &m.id,
            TimelineItem::TaskCard(c) => &c.message_id,
        }
    }
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
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApprovalView {
    pub id: String,
    pub task_id: String,
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThreadPost {
    pub id: String,
    pub speaker: Speaker,
    pub time_label: String,
    pub bodies: Vec<PostBody>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskThreadView {
    pub task_id: String,
    pub title: String,
    pub status: TaskStatus,
    /// Such as "1 decision, 1 approval".
    pub status_detail: String,
    pub placement: Option<PlacementView>,
    pub posts: Vec<ThreadPost>,
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HarnessAccountView {
    pub id: String,
    pub label: String,
    /// Such as "Codex 0.165.0. ChatGPT Pro plan."
    pub detail: String,
    /// Code to show in mono inside the detail, such as an environment
    /// variable or a command.
    pub detail_code: Vec<String>,
    pub auth: AuthState,
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
    pub labels: Vec<String>,
    pub accounts: Vec<HarnessAccountView>,
    pub policy: PolicyView,
}

/// A pairing code for a new worker.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PairingCodeView {
    pub code: String,
    pub hub_url: String,
    pub expires_at: i64,
}
