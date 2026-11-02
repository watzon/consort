//! The root view: app state, client subscription, actions and the layout of
//! the window.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use consort_client::{
    ApprovalChoice, ChannelChanges, Client, ClientError, Connection, Drafts, Os, ViewState,
    update::Updates,
};
use consort_theme::{ActiveTokens as _, Tokens, TypeExt as _};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ListAlignment, ListState, ParentElement, Render, Styled,
    Subscription, Window,
    component::input::{EditorState, InputEvent, InputState, TextareaState},
    div,
    prelude::FluentBuilder as _,
};

use crate::{
    countdown::{self, RetryCountdown},
    drafts::{self, InFlight, Kept},
    notice::{INFO_TIMEOUT, NoticeAction, NoticeKind, Notices},
    read_marker::{self, MARK_RETRY, MarkInputs, UNLOADED_GRACE, Unloaded},
    ui::{
        links::LinkAction,
        mark::{MarkPhase, MarkPlayer},
        markdown::TextCache,
        menu::{EditOp, MenuAction, MenuItem, OpenMenu, TextTarget, TriggerBounds, render_menu},
        model_picker::{OpenEffort, OpenPicker, PickerTarget},
        motion::FrameClock,
    },
    waiting::{Waiting, waiting_for_channel},
};

/// The key that guards the open dialog against a second submit.
const DIALOG_KEY: &str = "dialog";

/// The key that lets one mark-as-read run at a time.
const MARK_KEY: &str = "mark-read";

/// How often the offline countdown redraws.
const COUNTDOWN_TICK: Duration = Duration::from_secs(1);

/// How long a change to a draft waits before it goes to the disk. A burst of
/// typing makes one write.
const DRAFT_SAVE_DELAY: Duration = Duration::from_millis(1_500);

/// Which page fills the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Main,
    Settings,
    /// S1: Connect to your hub, and Pair this app again.
    Onboarding,
    /// The other setup screens. The setup gate picks them.
    Setup,
}

/// The side panel at the right of the timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Panel {
    None,
    Tasks,
    Thread(String),
    /// The Channel panel of the open channel: Files, Routines, Memory and
    /// Connectors.
    Channel,
    /// The thread of a message, by the root's message ID.
    MessageThread(String),
    /// A document opened from a file card: the text is in `ConsortApp::document`.
    Document,
}

/// The tabs of the Channel panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelTab {
    Files,
    Routines,
    Memory,
    /// The connectors and skills of the channel, with their switches.
    Connectors,
}

/// Where files that the user drags over the window go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropOver {
    /// The chat column: they attach to the composer.
    Chat,
    /// The Channel panel: they go into its open folder.
    Files,
    /// The task thread: they attach to the reply box.
    Reply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TasksTab {
    Active,
    UpNext,
    Scheduled,
    Resolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    Appearance,
    Notifications,
    Workers,
    Clients,
    Hub,
    /// The harness tools, the accounts and the API endpoints.
    Providers,
    /// The fallback path and the command check.
    Models,
    Connectors,
    Skills,
    Memory,
    Routines,
    Proactivity,
    Voice,
    Dictation,
}

/// The open dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog {
    /// The New channel dialog: a name and an icon. The group, if any, is in
    /// `ConsortApp::convo.new_channel_group`.
    NewChannel,
    /// The Deepgram key of Settings > Dictation.
    DeepgramKey,
    AddWorker,
    RenameWorker(String),
    EditTags(String),
    /// The allowed folders of a worker.
    WorkerFolders(String),
    /// A small dialog that names a new group. With a channel ID, the channel
    /// moves into the group once it exists.
    NewGroup(Option<String>),
    ConfirmDeleteChannel(String),
    ConfirmRemoveWorker(String),
    /// The first step of the fallback path on another harness. The step
    /// waits in `ConsortApp::fallback_ui`.
    ConfirmOtherHarnessStep,
    /// A link whose words do not match its address: the address to show.
    ConfirmOpenLink(String),
    /// Add account, for a worker. The form is in `ConsortApp::add_account`.
    AddAccount(String),
    /// Rename an account: the worker ID and the account ID.
    RenameAccount(String, String),
    /// Remove an account: the worker ID and the account ID.
    ConfirmRemoveAccount(String, String),
    /// Add an API endpoint, or edit the one with this ID. The fields are in
    /// `ConsortApp::check_inputs`.
    Endpoint(Option<String>),
    /// Remove an API endpoint.
    ConfirmRemoveEndpoint(String),
    /// Change or remove the Composio key.
    ComposioKey,
    /// Add Monid, or change or remove its key.
    MonidKey,
    /// The connector catalog.
    AddConnector,
    /// Connect a service. The form is in `ConsortApp::ext.connect`.
    Connect,
    /// Disconnect a connection.
    ConfirmDisconnect(String),
    /// The skill directories, a git URL or an upload.
    AddSkill,
    /// Add a skill source: a git repo or a `marketplace.json` file.
    AddSkillSource,
    /// The calls that the assistant made with a connection.
    ConnectionActivity(String),
    /// Remove a skill.
    ConfirmRemoveSkill(String),
    /// The files of a skill.
    SkillFiles(String),
    /// The settings of an account: the worker ID and the account ID.
    AccountSettings(String, String),
    /// Delete a channel file: the channel ID and the path.
    ConfirmDeleteFile(String, String),
    /// Rename a channel file: the channel ID and the path.
    RenameFile(String, String),
    /// Move a channel file to another folder: the channel ID and the path.
    MoveFile(String, String),
    /// Pick files from the channel files, for a composer or a reply box.
    ChooseChannelFiles(consort_client::UploadTarget),
    /// Pair a new app: a setup code, a QR code and the hub address.
    PairClient,
    /// New routine (`None`) or Edit routine. The form is in
    /// `ConsortApp::knowledge.routine_dialog`.
    Routine(Option<String>),
    ConfirmDeleteRoutine(String),
    /// Name a new memory note: the scope.
    NewNote(String),
    /// Rename a memory note: the scope and the path.
    RenameNote(String, String),
    /// Delete a memory note: the scope and the path.
    ConfirmDeleteNote(String, String),
    /// Phase 4: New task, the channel settings, a group's name and its
    /// delete, a task's delete, and About. See `crate::convo`.
    NewTask,
    /// Tasks work on the hub on this computer, and an update must restart it.
    RestartWithTasks,
    /// Remove the hub from this computer: keep the data or not.
    LocalRemove,
    /// Move to a server: the backup and the steps.
    LocalMove,
    /// Leave this hub. It keeps running.
    LocalConnectOther,
    ChannelSettings(String),
    /// What the assistant can do in a channel without asking: the
    /// command rules and the connections. See `crate::permissions`.
    Permissions(String),
    RenameGroup(String),
    ConfirmDeleteGroup(String),
    ConfirmDeleteTask(String),
    About,
    /// The changes of a new skill version, by skill ID.
    SkillDiff(String),
    /// A dialog of Settings > Voice. See `crate::voice`.
    Voice(crate::voice::VoiceDialog),
}

/// The step of the Add account dialog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccountStep {
    #[default]
    Provider,
    Name,
    SignIn,
}

/// The provider that the Add account dialog makes an account of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderPick {
    /// A built-in harness: `claude`, `codex`, `opencode` or `gemini`.
    Builtin(&'static str),
    /// An agent from the ACP registry.
    Registry { id: String, name: String },
    /// A program that the user names.
    Command,
}

/// The state of the Add account dialog that is not in text inputs.
#[derive(Clone, Debug, Default)]
pub struct AddAccountForm {
    pub worker_id: String,
    pub step: AccountStep,
    pub provider: Option<ProviderPick>,
    /// The account, once `account.create` made it.
    pub account_id: Option<String>,
    /// The chosen sign-in method, as an index into the account's methods.
    pub method: usize,
    /// The running or last sign-in of this dialog.
    pub sign_in_id: Option<String>,
    /// An API key was saved in this dialog.
    pub key_saved: bool,
    /// The dialog has started the sign-in of the account once, or tried to.
    pub auto_started: bool,
    /// The user sent a code in this sign-in, so a failure means that code.
    pub code_sent: bool,
}

/// The onboarding form state.
#[derive(Clone, Debug, Default)]
pub struct Onboarding {
    pub busy: bool,
    pub error: Option<String>,
    /// The first line of the error, for a version error (Paper `TED-0`).
    pub error_title: Option<String>,
}

/// Text inputs, made once and reused.
pub struct Inputs {
    pub composer: Entity<EditorState>,
    pub reply: Entity<EditorState>,
    pub decision_answer: Entity<TextareaState>,
    pub edit: Entity<TextareaState>,
    pub name: Entity<InputState>,
    pub hub_url: Entity<InputState>,
    pub search: Entity<InputState>,
    pub setup_code: Entity<InputState>,
    /// The search of the ACP registry in the Add account dialog.
    pub agent_search: Entity<InputState>,
    /// The program and the arguments of a custom command.
    pub program: Entity<InputState>,
    pub program_args: Entity<InputState>,
    /// The name of a new account.
    pub account_name: Entity<InputState>,
    /// The code that a sign-in page shows.
    pub sign_in_code: Entity<InputState>,
    /// An API key. The field masks it.
    pub api_key: Entity<InputState>,
    /// A new tag for a worker.
    pub tag: Entity<InputState>,
    pub folder: Entity<InputState>,
    /// The search of the model picker.
    pub model_search: Entity<InputState>,
}

/// What the Enter key does in the open dialog, as the last render set it.
pub(crate) type DialogConfirm = Rc<dyn Fn(&mut ConsortApp, &mut Window, &mut Context<ConsortApp>)>;

pub struct ConsortApp {
    pub client: Client,
    pub demo: bool,
    pub state: Arc<ViewState>,
    pub page: Page,
    pub panel: Panel,
    pub tasks_tab: TasksTab,
    pub settings_section: SettingsSection,
    pub settings_worker: Option<String>,
    pub channel_id: String,
    pub dialog: Option<Dialog>,
    pub dictation: crate::dictation::DictationUi,
    pub menu: Option<OpenMenu>,
    pub menu_anchor: Option<OpenMenu>,
    /// The open model picker.
    pub picker: Option<OpenPicker>,
    /// The open effort menu.
    pub effort_menu: Option<OpenEffort>,
    pub inputs: Inputs,
    pub texts: TextCache,
    /// The document that the Document panel shows.
    pub document: Option<crate::ui::document::OpenDocument>,
    pub timeline: ListState,
    pub timeline_keys: Vec<String>,
    pub timeline_channel: String,
    pub thread: ListState,
    pub thread_keys: Vec<String>,
    pub notices: Notices,
    /// The IDs of the refused messages that a notice already showed.
    announced_failures: std::collections::HashSet<String>,
    /// Phase 4: threads, reactions, search, New task, channel settings,
    /// groups and About. See `crate::convo`.
    pub convo: crate::convo::ConvoState,
    /// What the Models page keeps while the user edits the fallback path.
    pub fallback_ui: crate::fallback::FallbackUi,
    /// The endpoint dialog and the command check of Settings > Models.
    pub check_ui: crate::ui::command_check::CheckUi,
    pub check_inputs: crate::ui::command_check::CheckInputs,
    pub convo_inputs: crate::convo::ConvoInputs,
    pub add_account: AddAccountForm,
    /// The worker whose Add tag field is open.
    pub tag_entry: Option<String>,
    /// Why the hub refused the last tag: the worker ID and the message.
    pub tag_error: Option<(String, String)>,
    pub add_worker_os: Os,
    /// The step of the Add worker dialog that the user chose.
    pub add_worker_step: crate::ui::add_worker::AddWorkerStep,
    /// Updates of the app: the check, the download and the restart.
    pub updates: consort_client::update::Updates,
    /// The hub on this computer, if any.
    pub local: crate::ui::local_hub::LocalUi,
    /// A dialog closes with no hub for this app: go to the first screen.
    pub leaving_hub: bool,
    /// The setup screens. The setup gate decides what opens.
    pub setup: crate::ui::setup::SetupUi,
    pub decision_writing: Option<String>,
    pub activity_open: Vec<String>,
    pub last_thread: Option<String>,
    /// The message that the user edits in place.
    pub editing: Option<String>,
    /// The timeline column is narrower than 600 px.
    pub narrow: bool,
    /// The channel whose older page is loading.
    pub loading_older: Option<String>,
    /// The name of a channel the user just created. It opens when the hub
    /// sends it.
    pub open_created: Option<(String, Vec<String>)>,
    /// The workers there were when the Add worker dialog opened. A worker
    /// that is not in this list is the one the dialog paired.
    pub add_worker_before: Vec<String>,
    /// The worker that the open Add worker dialog paired.
    pub added_worker: Option<String>,
    /// The composer placeholder must follow a channel change made without a
    /// window.
    pub(crate) placeholder_stale: bool,
    /// The commands that run now, so a second submit waits.
    pub in_flight: InFlight,
    /// What went wrong in the open dialog. The dialog shows it.
    pub dialog_error: Option<String>,
    /// Counts the dialogs that opened, so a command that ends late can tell
    /// whether its dialog is still the open one.
    dialog_serial: u64,
    /// How long the open channel has been unloaded while the link is up.
    unloaded: Unloaded,
    /// The timeline sits at its live edge (scrolled to the bottom). The
    /// scroll handler keeps this; a channel change sets it again.
    timeline_at_end: bool,
    /// A retry of a failed mark-as-read waits on its timer.
    mark_retry_waiting: bool,
    /// The window has the focus.
    pub(crate) window_active: bool,
    /// The retry countdown, while the link is offline.
    pub offline: Option<RetryCountdown>,
    offline_ticker: bool,
    /// The open switcher and its selected row.
    pub switcher: Option<usize>,
    pub onboarding: Onboarding,
    pub triggers: TriggerBounds,
    /// The drafts of the channels and the task threads. The message boxes
    /// hold the draft of the open channel and the open thread; this map
    /// holds the others.
    pub drafts: Drafts,
    /// The channel whose draft the composer holds. The composer switches to
    /// the draft of `channel_id` when this differs from it.
    composer_for: String,
    /// The task whose reply draft the reply box holds.
    reply_for: Option<String>,
    /// A write of the drafts waits on its timer.
    drafts_saving: bool,
    /// The task threads whose brief is open.
    pub brief_open: Vec<String>,
    /// The task threads whose review summary the user closed.
    pub summary_closed: Vec<String>,
    /// The answered approvals that the user opened, by key. The key names
    /// the approval and the place (the timeline or a thread), so each place
    /// keeps its own state (`ui/approvals.rs`).
    pub approvals_open: std::collections::BTreeSet<String>,
    /// The in-chat error cards that the user opened, and their open Details,
    /// one flag for each card and each place (`ui/errors.rs`).
    pub errors_open: std::collections::BTreeSet<String>,
    /// The waiting approvals whose command shows in full ("Show all").
    pub approvals_all: std::collections::BTreeSet<String>,
    /// How the window shares its width. See `crate::ui::window_layout`.
    pub layout: crate::ui::window_layout::LayoutUi,
    /// The chat row under the pointer and its timeline item. The footer of
    /// a message group shows while this item is in the group.
    pub chat_hover: Option<(usize, usize)>,
    /// The Archived list in the sidebar is open.
    pub archived_open: bool,
    /// The 8 fps timer of the stop motion runs (see [`FrameClock`]).
    clock_ticker: bool,
    /// The rows of the open channel's chat: bubbles, cards and time
    /// separators, made from its timeline (see [`consort_client::chat`]).
    pub chat_rows: Vec<consort_client::chat::ChatRow>,
    /// What the Three voices mark over the timeline shows.
    pub mark: MarkPlayer,
    /// A phase that `--screen mark-*` holds, for screenshots.
    pub mark_forced: Option<MarkPhase>,
    /// What Enter does in the open dialog: the confirm button, unless Enter
    /// must not confirm (a destructive dialog, or a box with many lines).
    pub(crate) dialog_confirm: RefCell<Option<DialogConfirm>>,
    /// Connectors and skills: their dialogs and the cards that wait.
    pub ext: crate::extensions::ExtState,
    pub ext_inputs: crate::extensions::ExtInputs,
    /// A name for the connector name field, set when no window was at hand.
    pub pending_connection_name: Option<String>,
    /// Why the Composio key on the Connectors page was refused.
    pub key_error: Option<String>,
    /// The connector state changed: the render follows it with a window.
    ext_stale: bool,
    /// The tab of the Channel panel.
    pub channel_tab: ChannelTab,
    /// The open folder of the Files tab, empty for the top folder.
    pub files_folder: String,
    /// Files are dragged over the window: where they would go, and how
    /// many there are.
    pub drop_hover: Option<(DropOver, usize)>,
    /// The open light box (`ui/lightbox.rs`).
    pub lightbox: Option<crate::ui::lightbox::Lightbox>,
    /// The folder chosen in the open Move dialog.
    pub move_to: String,
    /// A screen holds the drop state with no drag, for a screenshot.
    pub hold_drop: bool,
    /// The file whose "more" button a screen opens the row menu under.
    pub files_menu_for: Option<String>,
    /// The blobs that the app asked the client to download, so it asks once.
    pub fetching: std::collections::HashSet<String>,
    /// Settings > Hub, Clients and Notifications, and the notifications.
    pub admin: crate::admin::AdminState,
    /// Routines and memory: their tabs, dialogs and the note editor.
    pub knowledge: crate::knowledge::KnowledgeState,
    pub knowledge_inputs: crate::knowledge::KnowledgeInputs,
    /// Settings > Proactivity, the ratings and the quick replies.
    pub proactivity: crate::proactivity::ProactivityUi,
    /// Monid: its page, its panel view and its dialog. See `crate::monid`.
    pub monid_ui: crate::monid::MonidUi,
    pub monid_inputs: crate::monid::MonidInputs,
    /// Settings > Voice and the draft cards. See `crate::voice`.
    pub voice: crate::voice::VoiceUi,
    /// The visuals: their pages, images and live pages. See
    /// `crate::visual`.
    pub visuals: crate::visual::VisualUi,
    pub voice_inputs: crate::voice::VoiceInputs,
    pub focus: FocusHandle,
    styled_for: Option<(gpui_kit::SharedString, consort_theme::Mode)>,
    _subscriptions: Vec<Subscription>,
}

impl ConsortApp {
    pub fn new(client: Client, demo: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = client.state();
        let drafts = client.load_drafts();
        let client_layout = client.load_layout();
        let overdraw = Self::tokens(cx).layout.list_overdraw;
        let channel_id = state.tutti().map(|c| c.id.clone()).unwrap_or_default();
        let textarea = |placeholder: &str,
                        rows: (usize, usize),
                        window: &mut Window,
                        cx: &mut Context<Self>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(rows.0, rows.1)
                    .submit_on_enter(true)
                    .placeholder(placeholder)
            })
        };
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let inputs = Inputs {
            composer: cx.new(|cx| crate::ui::parts::message_box("Message Consort", window, cx)),
            reply: cx.new(|cx| crate::ui::parts::message_box("Reply to the task", window, cx)),
            decision_answer: textarea("Write your answer", (2, 6), window, cx),
            edit: textarea("Message", (1, 10), window, cx),
            name: field("Name", window, cx),
            hub_url: field("https://hub.example.com", window, cx),
            search: field("Go to a channel or a task", window, cx),
            setup_code: field("ABCD-EFGH", window, cx),
            agent_search: field("Search agents", window, cx),
            program: field("Program, such as my-agent", window, cx),
            program_args: field("Arguments, such as --acp", window, cx),
            account_name: field("Personal or Work", window, cx),
            sign_in_code: field("Paste code", window, cx),
            api_key: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Paste the key")
                    .masked(true)
            }),
            tag: field("New tag", window, cx),
            folder: field("/home/me/code", window, cx),
            model_search: field("Search every account", window, cx),
        };
        let ext_inputs = crate::extensions::ExtInputs::new(window, cx);
        let (check_inputs, check_subscriptions) =
            crate::ui::command_check::CheckInputs::new(window, cx);
        let (convo_inputs, convo_subscriptions) = crate::convo::ConvoInputs::new(window, cx);
        let (admin, admin_subscriptions) = crate::admin::AdminState::new(&client, demo, window, cx);
        let mut subscriptions = admin_subscriptions;
        let knowledge_inputs = crate::knowledge::KnowledgeInputs::new(window, cx);
        let voice_inputs = crate::voice::VoiceInputs::new(window, cx);
        let monid_inputs = crate::monid::MonidInputs::new(window, cx);
        subscriptions.extend(knowledge_inputs.subscribe(window, cx));
        subscriptions.extend(convo_subscriptions);
        subscriptions.extend(check_subscriptions);
        subscriptions.push(cx.subscribe_in(
            &inputs.composer,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.send_message(window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &inputs.reply,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.send_reply(window, cx);
                }
            },
        ));
        // A change to a draft goes to the disk soon, so a quit loses little.
        for area in [&inputs.composer, &inputs.reply] {
            subscriptions.push(cx.subscribe_in(
                area,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        this.save_drafts_soon(cx);
                    }
                },
            ));
        }
        // Enter in a field of the open dialog, or of the onboarding form,
        // submits it.
        for field in [
            &inputs.name,
            &inputs.hub_url,
            &inputs.setup_code,
            &inputs.program,
            &inputs.program_args,
            &inputs.account_name,
            &inputs.sign_in_code,
            &inputs.api_key,
        ] {
            subscriptions.push(cx.subscribe_in(
                field,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit_on_enter(window, cx);
                    }
                },
            ));
        }
        // Enter in the tag field adds the tag. A change hides an old error
        // and filters the completions.
        subscriptions.push(cx.subscribe_in(
            &inputs.tag,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.add_worker_tag(None, window, cx),
                InputEvent::Change => {
                    this.tag_error = None;
                    cx.notify();
                }
                _ => {}
            },
        ));
        for field in [&inputs.agent_search, &inputs.account_name] {
            subscriptions.push(cx.subscribe_in(
                field,
                window,
                |_, _, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        cx.notify();
                    }
                },
            ));
        }
        // A new search moves the keyboard to the first model that matches.
        subscriptions.push(cx.subscribe_in(
            &inputs.model_search,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.picker_search_changed(cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &inputs.edit,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.save_edit(window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &inputs.search,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    if this.switcher.is_some() {
                        this.switcher = Some(0);
                        this.search_changed(cx);
                    }
                    cx.notify();
                }
            },
        ));
        subscriptions.push(cx.observe_window_appearance(window, |_, window, cx| {
            consort_theme::apply(window.appearance(), cx);
        }));
        // Unread messages in the open channel clear when the window has the
        // focus, not while it sits behind other windows.
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            this.window_active = window.is_window_active();
            if this.window_active {
                this.mark_read(cx);
            }
            // Stop motion stops in a window that is not active, and goes on
            // when it is. A draw restarts the timer.
            cx.notify();
        }));
        // A quit writes the drafts, so the text in the message boxes comes
        // back at the next start.
        subscriptions.push(cx.on_app_quit(|this, cx| {
            this.save_drafts_now(cx);
            async {}
        }));
        cx.set_global(FrameClock::default());
        let mut sub = client.subscribe();
        cx.spawn(async move |this, cx| {
            while let Some(state) = sub.next().await {
                if this
                    .update(cx, |this, cx| this.set_state(state, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        // A refused client shows onboarding, with the reason, even when the
        // refusal came before the window opened. The gate decides the rest
        // when the window opens.
        let page = if demo {
            Page::Main
        } else if matches!(state.connection, Connection::Refused { .. }) {
            Page::Onboarding
        } else {
            Page::Setup
        };
        let mut this = ConsortApp {
            client,
            demo,
            // A demo and a test have no engine. A test sends the updates.
            dictation: crate::dictation::DictationUi::new(demo || cfg!(test)),
            state: state.clone(),
            page,
            panel: Panel::None,
            tasks_tab: TasksTab::Active,
            settings_section: SettingsSection::Appearance,
            settings_worker: None,
            channel_id,
            dialog: None,
            menu: None,
            menu_anchor: None,
            picker: None,
            effort_menu: None,
            inputs,
            texts: TextCache::default(),
            document: None,
            timeline: ListState::new(0, ListAlignment::Bottom, overdraw),
            timeline_keys: Vec::new(),
            timeline_channel: String::new(),
            thread: ListState::new(0, ListAlignment::Bottom, overdraw),
            thread_keys: Vec::new(),
            notices: Notices::default(),
            announced_failures: std::collections::HashSet::new(),
            convo: crate::convo::ConvoState::default(),
            fallback_ui: crate::fallback::FallbackUi::default(),
            check_ui: crate::ui::command_check::CheckUi::default(),
            check_inputs,
            convo_inputs,
            add_account: AddAccountForm::default(),
            tag_entry: None,
            tag_error: None,
            add_worker_os: Os::Linux,
            add_worker_step: crate::ui::add_worker::AddWorkerStep::default(),
            updates: crate::ui::update_notice::start_updates(demo),
            local: crate::ui::local_hub::LocalUi::start(demo),
            setup: crate::ui::setup::SetupUi::default(),
            leaving_hub: false,
            decision_writing: None,
            activity_open: Vec::new(),
            last_thread: None,
            editing: None,
            layout: crate::ui::window_layout::LayoutUi::new(
                client_layout,
                window.viewport_size().width,
            ),
            chat_hover: None,
            narrow: false,
            loading_older: None,
            open_created: None,
            add_worker_before: Vec::new(),
            added_worker: None,
            placeholder_stale: false,
            in_flight: InFlight::default(),
            dialog_error: None,
            dialog_serial: 0,
            unloaded: Unloaded::default(),
            timeline_at_end: true,
            mark_retry_waiting: false,
            window_active: window.is_window_active(),
            offline: None,
            offline_ticker: false,
            switcher: None,
            onboarding: Onboarding::default(),
            triggers: TriggerBounds::default(),
            drafts,
            composer_for: String::new(),
            reply_for: None,
            drafts_saving: false,
            brief_open: Vec::new(),
            summary_closed: Vec::new(),
            approvals_open: std::collections::BTreeSet::new(),
            errors_open: std::collections::BTreeSet::new(),
            approvals_all: std::collections::BTreeSet::new(),
            archived_open: false,
            clock_ticker: false,
            chat_rows: Vec::new(),
            mark: MarkPlayer::default(),
            mark_forced: None,
            dialog_confirm: RefCell::new(None),
            ext: crate::extensions::ExtState::default(),
            ext_inputs,
            pending_connection_name: None,
            key_error: None,
            ext_stale: false,
            channel_tab: ChannelTab::Files,
            files_folder: String::new(),
            drop_hover: None,
            lightbox: None,
            move_to: String::new(),
            hold_drop: false,
            files_menu_for: None,
            fetching: std::collections::HashSet::new(),
            admin,
            knowledge: crate::knowledge::KnowledgeState::default(),
            knowledge_inputs,
            proactivity: crate::proactivity::ProactivityUi::default(),
            monid_ui: crate::monid::MonidUi::default(),
            monid_inputs,
            voice: crate::voice::VoiceUi::default(),
            visuals: crate::visual::VisualUi::default(),
            voice_inputs,
            focus: cx.focus_handle(),
            styled_for: None,
            _subscriptions: subscriptions,
        };
        let ext_subscriptions = this.subscribe_ext_inputs(window, cx);
        this._subscriptions.extend(ext_subscriptions);
        let monid_subscriptions = this.subscribe_monid_inputs(window, cx);
        this._subscriptions.extend(monid_subscriptions);
        // A click on a notification opens its task thread or its channel.
        let weak = cx.weak_entity();
        let handle = window.window_handle();
        cx.on_system_notification_response(move |response, cx| {
            let weak = weak.clone();
            let _ = handle.update(cx, |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.open_notification(&response.tag, window, cx)
                });
            });
        });
        this.check_permission(cx);
        this.sync_lists();
        this.update_placeholder(window, cx);
        this.sync_draft_inputs(window, cx);
        this.follow_connection(cx);
        this.follow_setup(cx);
        this.follow_updates(cx);
        this.follow_local(window, cx);
        this.mark_read(cx);
        // Reaching the top of the timeline loads older messages. Reaching the
        // bottom marks the channel read.
        let weak = cx.weak_entity();
        this.timeline.set_scroll_handler(move |event, _, cx| {
            if event.is_scrolled {
                let weak = weak.clone();
                let at_top = event.visible_range.start == 0;
                cx.defer(move |cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if at_top {
                            this.load_older(cx);
                        }
                        this.timeline_scrolled(cx);
                    });
                });
            }
        });
        this
    }

    /// Gives every input the editor colors of the active theme, once per
    /// theme and mode.
    fn style_inputs(&mut self, t: &Tokens, cx: &mut Context<Self>) {
        let key = (t.theme_name.clone(), t.mode);
        if self.styled_for.as_ref() == Some(&key) {
            return;
        }
        self.styled_for = Some(key);
        let style = crate::ui::parts::editor_style(t);
        for message_box in [&self.inputs.composer, &self.inputs.reply] {
            let style = style.clone();
            message_box.update(cx, |s, cx| {
                s.set_editor_style(style);
                cx.notify();
            });
        }
        for area in [&self.inputs.decision_answer, &self.inputs.edit]
            .into_iter()
            .chain(self.knowledge_inputs.areas())
            .chain(self.voice_inputs.areas())
        {
            let style = style.clone();
            area.update(cx, |s, cx| {
                s.set_editor_style(style);
                cx.notify();
            });
        }
        for field in [
            &self.inputs.name,
            &self.inputs.hub_url,
            &self.inputs.setup_code,
            &self.inputs.search,
            &self.inputs.agent_search,
            &self.inputs.program,
            &self.inputs.program_args,
            &self.inputs.account_name,
            &self.inputs.sign_in_code,
            &self.inputs.api_key,
            &self.inputs.tag,
            &self.inputs.model_search,
        ]
        .into_iter()
        .chain(self.ext_inputs.all())
        .chain(self.check_inputs.all())
        .chain(self.knowledge_inputs.fields())
        .chain(self.voice_inputs.fields())
        .chain(self.monid_inputs.fields())
        {
            let style = style.clone();
            field.update(cx, |s, cx| {
                s.set_editor_style(style);
                cx.notify();
            });
        }
        self.convo_inputs.set_style(&style, cx);
    }

    pub fn tokens(cx: &gpui_kit::App) -> Tokens {
        cx.tokens().clone()
    }

    /// Takes a new view state from the client. The client publishes changes
    /// only, but a state that equals the old one would still make every row
    /// measure again, so it ends here.
    pub(crate) fn set_state(&mut self, state: Arc<ViewState>, cx: &mut Context<Self>) {
        if Arc::ptr_eq(&state, &self.state) || *state == *self.state {
            return;
        }
        self.apply_state(state, cx);
    }

    pub(crate) fn apply_state(&mut self, state: Arc<ViewState>, cx: &mut Context<Self>) {
        self.state = state;
        // An update that cannot talk to this hub waits.
        self.updates.set_hub(self.state.hub_versions.clone());
        if self.state.channel(&self.channel_id).is_none()
            && let Some(tutti) = self.state.tutti()
        {
            self.channel_id = tutti.id.clone();
        }
        self.follow_connection(cx);
        self.follow_setup(cx);
        if let Panel::Thread(task_id) = &self.panel
            && self.state.thread(task_id).is_none()
            && self.state.task(task_id).is_none()
        {
            self.panel = Panel::None;
        }
        self.follow_convo(cx);
        if let Some((name, before)) = self.open_created.clone()
            && let Some(created) = self
                .state
                .channels
                .iter()
                .find(|c| c.name == name && !before.contains(&c.id))
                .map(|c| c.id.clone())
        {
            self.open_created = None;
            self.channel_id = created;
            self.page = Page::Main;
            self.placeholder_stale = true;
        }
        if self.dialog == Some(Dialog::AddWorker)
            && self.added_worker.is_none()
            && let Some(added) = self
                .state
                .workers
                .iter()
                .find(|w| !self.add_worker_before.contains(&w.id))
                .map(|w| w.id.clone())
        {
            // The new worker shows behind the dialog, selected.
            self.settings_worker = Some(added.clone());
            self.added_worker = Some(added);
        }
        self.sync_lists();
        self.report_send_failures(cx);
        self.track_unloaded(cx);
        self.mark_read(cx);
        self.ext_stale = true;
        self.follow_extensions(None, cx);
        self.follow_admin(cx);
        cx.notify();
    }

    /// Shows the messages that waited in the outbox and that the hub then
    /// refused, when no lane shows them. A message with a channel or a task
    /// on screen stays in the lane of the user, dimmed, with its reason and
    /// "Put back in the message box" (`SendFailureView::in_lane`), so it
    /// gets no notice. The others (the channel or the task is gone) are an
    /// error notice with the text and a button that puts the text back in
    /// the message box. The client keeps the failure until the user
    /// dismisses it, so a failure that the user has not seen is never lost.
    fn report_send_failures(&mut self, cx: &mut Context<Self>) {
        let fresh: Vec<_> = self
            .state
            .send_failures
            .iter()
            .filter(|f| !f.in_lane)
            .filter(|f| self.announced_failures.insert(f.id.clone()))
            .cloned()
            .collect();
        for failure in fresh {
            let head = if failure.channel_name.is_empty() {
                failure.message.clone()
            } else {
                format!(
                    "A message to {} did not go out. {}",
                    failure.channel_name, failure.message
                )
            };
            if self
                .notices
                .show_failure(failure.id, head, failure.body)
                .is_some()
            {
                cx.notify();
            }
        }
    }

    /// Dismisses one notice. The next error that waits, if there is one,
    /// shows. A failed send leaves the client with its notice.
    pub fn dismiss_notice(&mut self, serial: u64, cx: &mut Context<Self>) {
        if let Some(id) = self.notices.dismiss(serial).and_then(|n| n.failure) {
            self.dismiss_send_failure(id, cx);
        }
        cx.notify();
    }

    /// Tells the client that the user has seen a refused message, so the
    /// client drops it. The lane of the user shows it until then.
    pub fn dismiss_send_failure(&mut self, id: String, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |_, _| {
            let _ = client
                .run(consort_client::Command::DismissSendFailure { id })
                .await;
        })
        .detach();
    }

    /// The button of a notice. Its notice goes first, so a second click
    /// does nothing.
    pub fn run_notice_action(&mut self, serial: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(notice) = self.notices.get(serial).cloned() else {
            return;
        };
        match notice.action {
            Some(NoticeAction::PutBack) => {
                if let Some(id) = notice.failure {
                    self.restore_failed_text(&id, window, cx);
                }
            }
            Some(NoticeAction::LoadOlder(channel_id)) => {
                self.dismiss_notice(serial, cx);
                if self.channel_id == channel_id {
                    self.load_older(cx);
                }
            }
            None => {}
        }
    }

    /// Puts the text of a failed send back in the message box it came from,
    /// after what the user has typed since, and dismisses the notice. The
    /// message box is the reply box of the task thread for a reply, else the
    /// composer of the channel.
    pub fn restore_failed_text(
        &mut self,
        failure_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(failure) = self
            .state
            .send_failures
            .iter()
            .find(|f| f.id == failure_id)
            .cloned()
        else {
            return;
        };
        let thread = failure
            .task_id
            .clone()
            .filter(|task| self.state.task(task).is_some());
        let input = if let Some(task) = thread {
            if self.state.channel(&failure.channel_id).is_some() {
                self.open_channel(failure.channel_id.clone(), window, cx);
            }
            self.open_thread(task, window, cx);
            self.inputs.reply.clone()
        } else {
            if self.state.channel(&failure.channel_id).is_some() {
                self.open_channel(failure.channel_id.clone(), window, cx);
            }
            self.inputs.composer.clone()
        };
        input.update(cx, |input, cx| {
            let draft = input.value().to_string();
            input.set_value(drafts::append_text(&draft, &failure.body), window, cx);
            input.focus(window, cx);
        });
        // The notice (when the failure has one) and the client's copy go.
        self.notices.dismiss_failure(failure_id);
        self.dismiss_send_failure(failure.id.clone(), cx);
        cx.notify();
    }

    /// Acts on the state of the link: a refused client goes to onboarding
    /// with the reason, and an offline link starts its retry countdown.
    fn follow_connection(&mut self, cx: &mut Context<Self>) {
        if let Connection::Refused { reason } = &self.state.connection {
            self.page = Page::Onboarding;
            self.onboarding.error = Some(reason.clone());
            self.onboarding.error_title = None;
        }
        self.offline =
            countdown::follow(self.offline.take(), &self.state.connection, Instant::now());
        if self.offline.is_some() && !self.offline_ticker {
            self.offline_ticker = true;
            // The countdown redraws once a second while the link is offline,
            // and not at all otherwise.
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(COUNTDOWN_TICK).await;
                    let keep = this.update(cx, |this, cx| {
                        if this.offline.is_none() {
                            this.offline_ticker = false;
                            return false;
                        }
                        cx.notify();
                        true
                    });
                    if !matches!(keep, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// Marks the open channel as read when it has unread messages and the
    /// user can have seen them: the channel is loaded, the window has the
    /// focus, the timeline sits at its live edge and the link is up. The
    /// unread count drives this, so a mark that failed is tried again, after
    /// a short wait, for as long as the messages stay unread.
    pub fn mark_read(&mut self, cx: &mut Context<Self>) {
        let timeline = self.state.timeline(&self.channel_id);
        let inputs = MarkInputs {
            on_main_page: self.page == Page::Main,
            window_active: self.window_active,
            online: matches!(self.state.connection, Connection::Online | Connection::Demo),
            loaded: timeline.is_some_and(|t| t.loaded),
            at_live_edge: self.timeline_at_end,
            unread: self.state.unread(&self.channel_id),
            running: self.in_flight.is_running(MARK_KEY),
        };
        if !read_marker::should_mark(&inputs) || !self.in_flight.begin(MARK_KEY) {
            return;
        }
        let client = self.client.clone();
        let channel_id = self.channel_id.clone();
        cx.spawn(async move |this, cx| {
            let marked = client
                .run(consort_client::Command::MarkRead { channel_id })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.in_flight.finish(MARK_KEY);
                if marked.is_err() {
                    this.retry_mark_read_later(cx);
                } else {
                    // A message may have come in while the mark ran.
                    this.mark_read(cx);
                }
            });
        })
        .detach();
    }

    /// Tries to mark the open channel read again after a short wait. One
    /// timer runs at a time.
    fn retry_mark_read_later(&mut self, cx: &mut Context<Self>) {
        if self.mark_retry_waiting {
            return;
        }
        self.mark_retry_waiting = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MARK_RETRY).await;
            let _ = this.update(cx, |this, cx| {
                this.mark_retry_waiting = false;
                this.mark_read(cx);
            });
        })
        .detach();
    }

    /// The timeline scrolled. Remembers whether it sits at the live edge,
    /// and marks the channel read when the user reaches the bottom.
    fn timeline_scrolled(&mut self, cx: &mut Context<Self>) {
        // A list that cannot scroll, or whose size is not known yet, has
        // nothing below the view.
        self.timeline_at_end = self.timeline.is_scrolled_to_end().unwrap_or(true);
        if self.timeline_at_end {
            self.mark_read(cx);
        }
    }

    /// Notes whether the open channel is loaded. A channel that is not loaded
    /// for a moment is normal. After a short wait the timeline tells the
    /// user that it could not load, and a timer redraws it then.
    fn track_unloaded(&mut self, cx: &mut Context<Self>) {
        let unloaded = self.state.connection == Connection::Online
            && self
                .state
                .timeline(&self.channel_id)
                .is_some_and(|t| !t.loaded);
        if self
            .unloaded
            .update(&self.channel_id, unloaded, Instant::now())
        {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(UNLOADED_GRACE).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    /// True when the timeline must say that the open channel did not load.
    pub fn channel_unloaded(&self) -> bool {
        self.unloaded.visible(Instant::now())
    }

    /// Keeps the virtual lists in step with the state: appended rows are
    /// spliced in, so the list keeps following new messages; other changes
    /// reset it.
    pub fn sync_lists(&mut self) {
        let (rows, mut keys) = match self.state.timeline(&self.channel_id) {
            Some(t) => {
                let rows = consort_client::chat::rows(&t.items);
                let keys: Vec<String> = rows.iter().map(|r| r.key(&t.items)).collect();
                (rows, keys)
            }
            None => (Vec::new(), Vec::new()),
        };
        // The writing bubble hangs under the last row, so that row is
        // measured again when the bubble comes or goes.
        if consort_client::proactivity::is_writing(&self.state, &self.channel_id)
            && let Some(last) = keys.last_mut()
        {
            last.push_str("+writing");
        }
        self.chat_rows = rows;
        let channel_changed = self.timeline_channel != self.channel_id;
        sync_list(
            &self.timeline,
            &mut self.timeline_keys,
            keys,
            channel_changed,
        );
        if channel_changed {
            // A list that was reset starts at the live edge.
            self.timeline_at_end = true;
        }
        self.timeline_channel = self.channel_id.clone();
        let thread_keys: Vec<String> = match &self.panel {
            // The brief is the first row of the thread, as bubbles.
            Panel::Thread(task_id) => self
                .state
                .thread(task_id)
                .map(|t| {
                    let brief = (!t.brief.trim().is_empty())
                        .then(|| format!("brief-{task_id}-{}", t.text_is_preview));
                    // The plan card is under the newest post.
                    let plan = t
                        .plan
                        .as_ref()
                        .map(|p| format!("plan-{task_id}-{}", p.key()));
                    // The writing bubble is under the plan, while the turn of
                    // the task runs.
                    let writing = consort_client::proactivity::is_task_writing(t)
                        .then(|| format!("writing-{task_id}"));
                    brief
                        .into_iter()
                        .chain(t.posts.iter().map(|p| p.id.clone()))
                        .chain(plan)
                        .chain(writing)
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        sync_list(&self.thread, &mut self.thread_keys, thread_keys, false);
    }

    /// The composer's placeholder: "Message <channel>", and "Message Tutti"
    /// in the main channel.
    pub fn composer_placeholder(&self) -> String {
        if self.no_account_in_tutti() {
            return "Sign in to a provider to message the assistant".to_owned();
        }
        let name = self
            .state
            .channel(&self.channel_id)
            .map(|c| match c.kind {
                consort_client::ChannelKind::Tutti => crate::ui::TUTTI.to_owned(),
                consort_client::ChannelKind::Standard => c.name.clone(),
            })
            .unwrap_or_else(|| crate::ui::TUTTI.to_owned());
        format!("Message {name}")
    }

    pub fn update_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = self.composer_placeholder();
        self.inputs.composer.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
        });
    }

    /// Gives the mock its hub on this computer, for `--screen`.
    fn show_local_screen(&mut self, screen: &str, cx: &mut Context<Self>) {
        if let Some(view) = consort_client::mock_local::screen_view(screen) {
            self.local = crate::ui::local_hub::LocalUi::with(
                consort_client::local_hub::LocalHub::fixed(view),
            );
            self.local.seen_swap = self.local.hub.view().swap;
            cx.notify();
        }
    }

    /// Opens one of the designed screens by name, for `--screen`.
    pub fn open_screen(&mut self, screen: &str, window: &mut Window, cx: &mut Context<Self>) {
        let sayso = self
            .state
            .channels
            .iter()
            .find(|c| c.name == "sayso")
            .map(|c| c.id.clone());
        let first_task = self
            .state
            .tasks_panel
            .active
            .first()
            .and_then(|s| s.task_ids.first().cloned());
        match screen {
            // A task in review: its card in the timeline and its thread.
            "review" => {
                let channel = self
                    .state
                    .channels
                    .iter()
                    .find(|c| c.name == "miss-lucy-tg")
                    .map(|c| c.id.clone());
                let task = self
                    .state
                    .tasks
                    .iter()
                    .find(|t| t.status == consort_client::TaskStatus::Review)
                    .map(|t| t.id.clone());
                if let (Some(channel), Some(task)) = (channel, task) {
                    self.open_channel(channel, window, cx);
                    // The brief opens under the header. The summary is open
                    // above the Resolve bar already.
                    self.brief_open.push(task.clone());
                    self.open_thread(task, window, cx);
                }
            }
            // A task with no repo: the thread says it runs in a new folder.
            // A failed task: the reason shows in the thread header, in full.
            "failed" => {
                let task = self
                    .state
                    .tasks
                    .iter()
                    .find(|t| t.status == consort_client::TaskStatus::Failed)
                    .map(|t| (t.channel_id.clone(), t.id.clone()));
                if let Some((channel, task)) = task {
                    self.open_channel(channel, window, cx);
                    self.open_thread(task, window, cx);
                }
            }
            "dictation-settings" => self.open_settings(SettingsSection::Dictation, cx),
            screen if screen.starts_with("dictation-") => {
                crate::dictation::open_dictation_screen(self, screen, window, cx);
            }
            // A long error and a second one that waits behind it.
            "error" => {
                let body = "Please look at the failing build and tell me why it fails. ".repeat(24);
                let _ = self.notices.show_failure(
                    "demo".into(),
                    "A message to sayso did not go out. That channel does not exist. Put your text back in the message box, then send it again.".into(),
                    body,
                );
                self.show_error("The hub did not answer in time.".into(), cx);
            }
            "new-folder" => {
                let channel = self
                    .state
                    .channels
                    .iter()
                    .find(|c| c.name == "house-lights")
                    .map(|c| c.id.clone());
                let task = self
                    .state
                    .tasks
                    .iter()
                    .find(|t| t.channel_name == "house-lights")
                    .map(|t| t.id.clone());
                if let (Some(channel), Some(task)) = (channel, task) {
                    self.open_channel(channel, window, cx);
                    self.open_thread(task, window, cx);
                }
            }
            "channel-requests" => {
                if let Some(id) = self
                    .state
                    .channels
                    .iter()
                    .find(|c| c.name == "house-lights")
                    .map(|c| c.id.clone())
                {
                    self.open_channel(id, window, cx);
                }
            }
            "sayso" => {
                if let Some(id) = sayso {
                    self.open_channel(id, window, cx);
                }
            }
            "tasks" => {
                if let Some(id) = sayso {
                    self.open_channel(id, window, cx);
                }
                self.panel = Panel::Tasks;
            }
            "composer-grown" | "composer-full" => {
                let words = if screen == "composer-full" { 160 } else { 60 };
                let text = "The composer grows upward as you type. ".repeat(words / 7);
                self.inputs.composer.update(cx, |input, cx| {
                    input.set_value(text, window, cx);
                    input.focus(window, cx);
                });
            }
            "thread" => {
                if let Some(id) = sayso {
                    self.open_channel(id, window, cx);
                }
                if let Some(task) = first_task {
                    self.open_thread(task, window, cx);
                }
            }
            // The Three voices mark in each state, over Tutti.
            "mark-idle" | "mark-working" | "mark-needs-you" | "mark-done" => {
                self.mark_forced = MarkPhase::from_screen(screen);
            }
            "settings" => self.open_settings(SettingsSection::Appearance, cx),
            "workers" => self.open_settings(SettingsSection::Workers, cx),
            // Connectors and skills, as the designs show them.
            "connectors" => self.open_settings(SettingsSection::Connectors, cx),
            "add-connector" | "connect" => {
                self.open_settings(SettingsSection::Connectors, cx);
                let _ = self
                    .client
                    .run_blocking(consort_client::Command::SearchConnectorCatalog {
                        search: String::new(),
                        category: None,
                    });
                if screen == "add-connector" {
                    self.open_dialog(Dialog::AddConnector, window, cx);
                } else if let Some(gmail) = self
                    .client
                    .state()
                    .connector_catalog
                    .services
                    .iter()
                    .find(|s| s.slug == "gmail")
                    .cloned()
                {
                    self.state = self.client.state();
                    self.open_connect(gmail, None, true, window, cx);
                    self.ext.connect.use_in = consort_client::UseInView::Tags(vec!["work".into()]);
                    self.ext_inputs
                        .connect_name
                        .update(cx, |input, cx| input.set_value("Gmail Work", window, cx));
                }
            }
            "skills" => self.open_settings(SettingsSection::Skills, cx),
            "add-skill" | "add-skill-unsupported" => {
                self.open_settings(SettingsSection::Skills, cx);
                self.open_dialog(Dialog::AddSkill, window, cx);
                self.ext_inputs
                    .skill_search
                    .update(cx, |input, cx| input.set_value("rust", window, cx));
                self.search_skills(cx);
            }
            "connectors-in-channel" | "channel-connectors" => {
                self.open_channel(consort_client::mock_extensions::ACME.into(), window, cx);
                if screen == "channel-connectors" {
                    self.open_channel_connectors(
                        consort_client::mock_extensions::ACME.into(),
                        window,
                        cx,
                    );
                }
            }
            "hub-cards" => {
                self.open_channel(consort_client::mock_extensions::HIRING.into(), window, cx);
            }
            // Monid (ADR 0016) and access requests (ADR 0017), in
            // house-lights, where the mock keeps them.
            "monid-cards" | "monid-access" | "monid-task" => {
                self.open_channel("chn_house_lights".into(), window, cx);
            }
            "monid-calls" => {
                self.open_channel("chn_house_lights".into(), window, cx);
                for id in [
                    consort_client::mock_monid::CALL_ELEVEN,
                    consort_client::mock_monid::CALL_BLOCKED,
                    consort_client::mock_monid::CALL_STOPPED_BY_MONID,
                ] {
                    let _ = self
                        .client
                        .run_blocking(consort_client::Command::LoadMonidCall {
                            call_id: id.into(),
                        });
                    self.monid_ui
                        .open_calls
                        .insert(crate::monid::call_key("chat", id));
                }
                self.state = self.client.state();
            }
            "monid-panel" | "monid-view" => {
                self.open_channel_connectors("chn_house_lights".into(), window, cx);
                if screen == "monid-view" {
                    let _ = self
                        .client
                        .run_blocking(consort_client::Command::LoadMonidCalls {
                            channel_id: "chn_house_lights".into(),
                        });
                    self.state = self.client.state();
                    self.open_monid_panel(window, cx);
                }
            }
            "monid-settings" => self.open_monid_page(window, cx),
            "monid-add" | "monid-no-key" => {
                self.open_settings(SettingsSection::Connectors, cx);
                if screen == "monid-add" {
                    self.open_monid_key_dialog(false, window, cx);
                }
            }
            "monid-offer" => {
                self.open_settings(SettingsSection::Connectors, cx);
                self.open_dialog(Dialog::AddConnector, window, cx);
            }
            "monid-routines" => self.open_settings(SettingsSection::Routines, cx),
            "monid-routine" => {
                self.open_settings(SettingsSection::Routines, cx);
                self.open_routine_in_settings("rtn_sunset_check", cx);
            }
            "monid-full-access" => {
                self.open_channel("chn_house_lights".into(), window, cx);
                self.open_new_task(window, cx);
                self.convo.new_task.approval_mode = consort_client::ApprovalMode::FullAccess;
            }
            "account-settings" => {
                self.open_settings(SettingsSection::Workers, cx);
                self.settings_worker = Some("wrk_bigboi".into());
                self.open_dialog(
                    Dialog::AccountSettings("wrk_bigboi".into(), "acc_bigboi_claude".into()),
                    window,
                    cx,
                );
            }
            // The Add account dialog on bigboi: the provider step, the name
            // step with a name used on other workers, and the Claude sign-in
            // that waits for its code.
            "add-account" | "add-account-name" | "sign-in" => {
                self.open_settings(SettingsSection::Workers, cx);
                // The registry must be there before the first frame.
                let _ = self
                    .client
                    .run_blocking(consort_client::Command::LoadAcpRegistry);
                self.state = self.client.state();
                let worker = self.settings_worker.clone().unwrap_or_default();
                match screen {
                    "add-account" => {
                        self.open_dialog(Dialog::AddAccount(worker), window, cx);
                    }
                    "add-account-name" => {
                        self.open_dialog(Dialog::AddAccount(worker), window, cx);
                        self.add_account.step = AccountStep::Name;
                        self.inputs.account_name.update(cx, |input, cx| {
                            input.set_value("Claude Work", window, cx);
                            input.focus(window, cx);
                        });
                    }
                    _ => {
                        let account = self
                            .state
                            .worker(&worker)
                            .and_then(|w| w.accounts.iter().find(|a| a.label == "Claude Work"))
                            .map(|a| a.id.clone())
                            .unwrap_or_default();
                        self.open_dialog(Dialog::AddAccount(worker.clone()), window, cx);
                        self.add_account.step = AccountStep::SignIn;
                        self.add_account.account_id = Some(account.clone());
                        self.add_account.auto_started = true;
                        let started =
                            self.client
                                .run_blocking(consort_client::Command::StartSignIn {
                                    worker_id: worker,
                                    account_id: account,
                                    method: consort_client::SignInChoice::ClaudePlan,
                                });
                        if let Ok(consort_client::CommandOutput::SignInStarted(id)) = started {
                            self.add_account.sign_in_id = Some(id);
                        }
                        self.state = self.client.state();
                        self.inputs
                            .sign_in_code
                            .update(cx, |input, cx| input.focus(window, cx));
                    }
                }
            }
            "update-ready" => {
                // The notice above the account row: the update is
                // downloaded and checked.
                self.updates = Updates::fixed(crate::ui::update_notice::ready_view());
            }
            "add-worker" | "add-worker-install" | "add-worker-loopback" | "worker-added" => {
                self.open_settings(SettingsSection::Workers, cx);
                // The designs show Windows, chosen in step 1.
                self.add_worker_os = Os::Windows;
                if screen != "add-worker" {
                    self.add_worker_step = crate::ui::add_worker::AddWorkerStep::Install;
                }
                // The window draws its first frame from this state, so the
                // code must be there before it opens.
                let _ = self
                    .client
                    .run_blocking(consort_client::Command::WorkerPairingCode);
                self.state = self.client.state();
                self.dialog = Some(Dialog::AddWorker);
                self.add_worker_before = self.state.workers.iter().map(|w| w.id.clone()).collect();
                if screen == "worker-added" {
                    // As if bigboi had just paired and connected.
                    self.add_worker_before.retain(|id| {
                        self.state
                            .workers
                            .iter()
                            .any(|w| &w.id == id && w.name != "bigboi")
                    });
                    self.apply_state(self.state.clone(), cx);
                }
            }
            // Phase 1: files and attachments, in house-lights.
            "attachments" | "drop" | "bubbles" | "lightbox" | "files" | "files-menu"
            | "files-drop" | "thread-attach" => {
                use consort_client::mock_files::{LIGHTS, ROOF_TASK};
                self.open_channel(LIGHTS.into(), window, cx);
                let text = match screen {
                    "attachments" => {
                        "Here are the roofline photos and the quote from Brightline. Can you compare it with the other two?"
                    }
                    "drop" => "Here are the roofline photos and the quote from Brightline.",
                    _ => "",
                };
                self.inputs
                    .composer
                    .update(cx, |input, cx| input.set_value(text, window, cx));
                match screen {
                    "drop" => {
                        self.drop_hover = Some((DropOver::Chat, 3));
                        self.hold_drop = true;
                    }
                    "lightbox" => {
                        let images = self
                            .state
                            .timeline(LIGHTS)
                            .and_then(|tl| {
                                tl.items.iter().find_map(|i| match i {
                                    consort_client::TimelineItem::Message(m)
                                        if m.attachments.iter().any(|a| a.image) =>
                                    {
                                        Some(m.attachments.clone())
                                    }
                                    _ => None,
                                })
                            })
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|a| a.image)
                            .collect();
                        self.open_lightbox(
                            images,
                            0,
                            "You sent it today at 7:40 PM.".into(),
                            window,
                            cx,
                        );
                    }
                    "files" | "files-menu" | "files-drop" => {
                        self.panel = Panel::Channel;
                        if screen == "files-drop" {
                            self.drop_hover = Some((DropOver::Files, 2));
                            self.hold_drop = true;
                        }
                        if screen == "files-menu" {
                            self.files_menu_for = Some("wiring-plan.pdf".into());
                            cx.on_next_frame(window, |this, window, cx| {
                                cx.on_next_frame(window, |this, _, cx| {
                                    let file = this
                                        .state
                                        .channel_files
                                        .iter()
                                        .find(|f| f.path == "wiring-plan.pdf")
                                        .cloned();
                                    if let (Some(file), Some(bounds)) =
                                        (file, this.triggers.get("files-menu-anchor"))
                                    {
                                        let items = crate::ui::menu::file_menu(&file);
                                        this.open_menu(OpenMenu::under(bounds, items, false), cx);
                                    }
                                });
                                let _ = this;
                            });
                        }
                    }
                    "thread-attach" => {
                        self.open_thread(ROOF_TASK.into(), window, cx);
                        self.inputs.reply.update(cx, |input, cx| {
                            input.set_value(
                                "Add the garage side too. The run there is 22 ft.",
                                window,
                                cx,
                            )
                        });
                        cx.on_next_frame(window, |this, window, cx| {
                            cx.on_next_frame(window, |this, _, cx| {
                                let target = consort_client::UploadTarget::Reply {
                                    task_id: ROOF_TASK.into(),
                                };
                                this.open_add_menu("reply-attach", target, cx);
                            });
                            let _ = this;
                        });
                    }
                    _ => {}
                }
            }
            "files-empty" => {
                self.open_channel(consort_client::mock_files::EMPTY_CHANNEL.into(), window, cx);
                self.panel = Panel::Channel;
            }
            "add-skill-upload" => {
                self.open_settings(SettingsSection::Skills, cx);
                self.open_dialog(Dialog::AddSkill, window, cx);
                self.ext.add_skill.tab = crate::extensions::AddSkillTab::Upload;
                if let Some(path) = consort_client::mock_files::skill_zip_path() {
                    self.ext.add_skill.upload_path = Some(path.clone());
                    self.preview_skill(
                        consort_client::SkillInstallInput::Upload { file_path: path },
                        "upload".into(),
                        cx,
                    );
                }
            }
            "onboarding" => self.page = Page::Onboarding,
            screen if screen.starts_with("setup-") => self.open_setup_screen(screen, window, cx),
            "first-t8" | "first-signed-out" => {
                self.setup.gate = consort_client::setup::Gate::NoAccount;
                self.placeholder_stale = true;
            }
            "first-permission" => {
                self.admin.permission = crate::notify_platform::Permission::NotAsked;
            }
            "first-failed" => {}
            "local-install" | "local-windows" | "local-bubblewrap" | "local-failed"
            | "local-found" => {
                self.show_local_screen(screen, cx);
                self.page = Page::Setup;
                self.setup.gate = if screen == "local-found" {
                    consort_client::setup::Gate::LocalFound
                } else {
                    consort_client::setup::Gate::Where
                };
            }
            "local-stopped" => self.show_local_screen(screen, cx),
            "hub-local" => {
                self.show_local_screen(screen, cx);
                self.open_settings(SettingsSection::Hub, cx);
            }
            "hub-local-remove" | "hub-local-move" | "hub-local-connect" => {
                self.show_local_screen(screen, cx);
                self.open_settings(SettingsSection::Hub, cx);
                let dialog = match screen {
                    "hub-local-remove" => Dialog::LocalRemove,
                    "hub-local-move" => Dialog::LocalMove,
                    _ => Dialog::LocalConnectOther,
                };
                self.open_dialog(dialog, window, cx);
            }
            "restart-tasks" => {
                self.show_local_screen(screen, cx);
                self.open_dialog(Dialog::RestartWithTasks, window, cx);
            }
            "pair-too-old" => {
                self.page = Page::Onboarding;
                let (title, detail) = consort_client::versions::pair_error(Some(
                    &consort_client::versions::HubVersions {
                        hub_version: "0.6.2".into(),
                        protocol_versions: Vec::new(),
                        install: None,
                    },
                ));
                self.onboarding.error_title = Some(title);
                self.onboarding.error = Some(detail);
            }
            "update-held" | "app-too-old" => {
                self.updates = Updates::fixed(crate::ui::versions::screen_view(
                    self.updates.view(),
                    screen,
                ));
                self.updates.set_hub(self.state.hub_versions.clone());
            }
            "hub-update" => {
                self.updates = Updates::fixed(crate::ui::versions::screen_view(
                    self.updates.view(),
                    screen,
                ));
                self.open_settings(SettingsSection::Hub, cx);
            }
            "workers-updating" => self.open_settings(SettingsSection::Workers, cx),
            "edit" => {
                let mine = self.state.timeline(&self.channel_id).and_then(|t| {
                    t.items.iter().rev().find_map(|i| match i {
                        consort_client::TimelineItem::Message(m)
                            if m.speaker == consort_client::Speaker::You =>
                        {
                            Some((m.id.clone(), m.body.clone()))
                        }
                        _ => None,
                    })
                });
                if let Some((id, body)) = mine {
                    self.run_menu(MenuAction::EditMessage(id, body), window, cx);
                }
            }
            // The model picker and the effort menu of the composer. They
            // open under their chips, which have bounds after the first
            // frame.
            // `model-picker` opens as a user sees it (on Favorites in the
            // mock), `model-picker-favorites` the same, `model-picker-rail`
            // on the account of the current model, and `model-picker-search`
            // with a search for "5.5".
            "model-picker"
            | "model-picker-no-account"
            | "model-picker-favorites"
            | "model-picker-rail"
            | "model-picker-search"
            | "effort-menu" => {
                let screen = screen.to_owned();
                cx.on_next_frame(window, move |this, window, cx| {
                    let target = PickerTarget::Assistant(this.channel_id.clone());
                    if screen == "effort-menu" {
                        this.open_effort("composer-effort", target, window, cx);
                        return;
                    }
                    this.open_picker_under("composer-model", target.clone(), window, cx);
                    match screen.as_str() {
                        "model-picker-rail" => {
                            if let Some(account) =
                                this.picker_selection(&target).map(|s| s.account_label)
                            {
                                this.show_picker_view(
                                    consort_client::picker::PickerView::Account(account),
                                    cx,
                                );
                            }
                        }
                        "model-picker-favorites" => {
                            this.show_picker_view(consort_client::picker::PickerView::Favorites, cx)
                        }
                        "model-picker-search" => {
                            this.inputs
                                .model_search
                                .update(cx, |input, cx| input.set_value("5.5", window, cx));
                            this.picker_search_changed(cx);
                        }
                        _ => {}
                    }
                });
            }
            "switcher" => {
                self.open_switcher(window, cx);
                self.inputs
                    .search
                    .update(cx, |input, cx| input.set_value("s", window, cx));
            }
            // A right click near the bottom of the window: the menu opens
            // upward, because there is no room below.
            "menu" => {
                let t = Self::tokens(cx);
                let items = crate::ui::timeline::selection_menu("Two tasks need you".into());
                let at = gpui_kit::point(
                    t.layout.sidebar_width + t.layout.timeline_max_width / 2.0,
                    window.viewport_size().height - t.space.xl,
                );
                self.open_menu(OpenMenu::at(at, items), cx);
            }
            // The menu of a channel row in the sidebar.
            "channel-menu" => {
                if let Some(id) = sayso {
                    let items = crate::ui::menu::channel_menu(&id);
                    let t = Self::tokens(cx);
                    let at = gpui_kit::point(
                        t.layout.sidebar_width / 2.0,
                        t.space.page * 3.0 + t.space.xl,
                    );
                    self.open_menu(OpenMenu::at(at, items), cx);
                }
            }
            screen if crate::knowledge::SCREENS.contains(&screen) => {
                self.open_knowledge_screen(screen, window, cx)
            }
            screen if crate::voice::SCREENS.contains(&screen) => {
                self.open_voice_screen(screen, window, cx);
            }
            screen if crate::proactivity::SCREENS.contains(&screen) => {
                self.open_proactive_screen(screen, window, cx);
            }
            other => {
                if !self.open_convo_screen(other, window, cx) {
                    self.open_admin_screen(other, window, cx);
                }
            }
        }
        cx.notify();
    }

    // Navigation

    pub fn open_channel(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.channel_id != id {
            self.files_folder.clear();
        }
        self.channel_id = id;
        self.page = Page::Main;
        self.layout.drawer_open = false;
        // The composer holds the draft of this channel from now on.
        self.sync_draft_inputs(window, cx);
        self.sync_lists();
        self.update_placeholder(window, cx);
        self.track_unloaded(cx);
        self.mark_read(cx);
        if !self.demo {
            self.follow_first_run(cx);
        }
        cx.notify();
    }

    /// Opens the Channel panel, or closes it when it is open. It takes the
    /// place of the Tasks panel or a task thread.
    pub fn toggle_channel_panel(&mut self, cx: &mut Context<Self>) {
        self.panel = if self.panel == Panel::Channel {
            Panel::None
        } else {
            Panel::Channel
        };
        if self.panel == Panel::Channel {
            self.load_channel_files(cx);
        }
        self.sync_lists();
        cx.notify();
    }

    pub fn toggle_tasks(&mut self, cx: &mut Context<Self>) {
        self.panel = if self.panel == Panel::Tasks {
            Panel::None
        } else {
            Panel::Tasks
        };
        cx.notify();
    }

    pub fn open_thread(&mut self, task_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = Panel::Thread(task_id.clone());
        self.decision_writing = None;
        // The reply box holds the draft of this thread from now on.
        self.sync_draft_inputs(window, cx);
        self.sync_lists();
        cx.notify();
        // Replies come from the hub the first time a thread opens.
        let client = self.client.clone();
        self.spawn_command(cx, async move { client.load_thread(task_id).await });
    }

    /// Gives each message box the draft of the channel or the thread that is
    /// open. When the open channel or thread is not the one the box held, the
    /// text in the box goes to the drafts first, and the box takes the draft
    /// of the new one. A draft written in channel A never goes to channel B.
    pub fn sync_draft_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer_for != self.channel_id {
            let text = self.inputs.composer.read(cx).value().to_string();
            let next = drafts::switch_channel(
                &mut self.drafts,
                &self.composer_for,
                &text,
                &self.channel_id,
            );
            self.inputs
                .composer
                .update(cx, |input, cx| input.set_value(next, window, cx));
            self.composer_for = self.channel_id.clone();
        }
        let wanted = match &self.panel {
            Panel::Thread(task_id) => Some(task_id.clone()),
            _ => None,
        };
        if self.reply_for != wanted {
            let text = self.inputs.reply.read(cx).value().to_string();
            let next = drafts::switch_thread(
                &mut self.drafts,
                self.reply_for.as_deref(),
                &text,
                wanted.as_deref(),
            );
            self.inputs
                .reply
                .update(cx, |input, cx| input.set_value(next, window, cx));
            self.reply_for = wanted;
        }
    }

    /// Puts the text of the two message boxes in the drafts, and drops the
    /// drafts of channels and tasks that are gone.
    fn collect_drafts(&mut self, cx: &mut Context<Self>) {
        let composer = self.inputs.composer.read(cx).value().to_string();
        self.drafts.set_channel(&self.composer_for, &composer);
        if let Some(task_id) = self.reply_for.clone() {
            let reply = self.inputs.reply.read(cx).value().to_string();
            self.drafts.set_thread(&task_id, &reply);
        }
        // Until the first snapshot, the app knows no channel, so it cannot
        // tell a gone channel from one that has not loaded.
        if !self.state.channels.is_empty() {
            let channels = self
                .state
                .channels
                .iter()
                .map(|c| c.id.as_str())
                .chain(self.state.archived_channels.iter().map(|c| c.id.as_str()));
            let tasks = self.state.tasks.iter().map(|t| t.id.as_str());
            self.drafts.keep_only(channels, tasks);
        }
    }

    /// Writes the drafts to the client's folder now. A failure is only
    /// printed: a draft is a convenience, and the text is still in its box.
    pub fn save_drafts_now(&mut self, cx: &mut Context<Self>) {
        if self.demo {
            return;
        }
        self.collect_drafts(cx);
        if let Err(err) = self.client.save_drafts(&self.drafts) {
            eprintln!("consort: cannot save the drafts: {err}");
        }
    }

    /// Writes the drafts after a short wait. One write covers a burst of
    /// typing.
    fn save_drafts_soon(&mut self, cx: &mut Context<Self>) {
        if self.drafts_saving || self.demo {
            return;
        }
        self.drafts_saving = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DRAFT_SAVE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                this.drafts_saving = false;
                this.save_drafts_now(cx);
            });
        })
        .detach();
    }

    /// Forgets the saved draft of a channel or a thread when it holds the
    /// text that went out. The text is in the hub now.
    fn forget_sent_draft(&mut self, channel: Option<&str>, thread: Option<&str>, sent: &str) {
        if let Some(channel) = channel
            && self.drafts.channel(channel).trim() == sent.trim()
        {
            self.drafts.set_channel(channel, "");
        }
        if let Some(thread) = thread
            && self.drafts.thread(thread).trim() == sent.trim()
        {
            self.drafts.set_thread(thread, "");
        }
    }

    /// Puts "Start a task to " in the composer after the draft. A draft is
    /// never replaced.
    /// The New task buttons open the New task dialog.
    pub fn open_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.waiting().is_some() {
            return;
        }
        self.open_dialog(Dialog::NewTask, window, cx);
    }

    /// Connects to the hub now, without waiting for the retry timer.
    pub fn try_again(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(cx, async move { client.reconnect().await });
    }

    /// Why the app has no channel to show, if it has none yet.
    pub fn waiting(&self) -> Option<Waiting> {
        waiting_for_channel(&self.state, &self.channel_id)
    }

    /// Enter in a field submits the open dialog or the onboarding form. A
    /// dialog that destroys something, or that asks for many lines, does not
    /// take Enter: its button does.
    pub(crate) fn submit_on_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            let confirm = self.dialog_confirm.borrow().clone();
            if let Some(confirm) = confirm {
                confirm(self, window, cx);
            }
        } else if self.page == Page::Onboarding && !self.onboarding.busy {
            self.connect(window, cx);
        } else if self.page == Page::Setup {
            // The sign-in screen has the fields of the dialog.
            let confirm = self.dialog_confirm.borrow().clone();
            if let Some(confirm) = confirm {
                confirm(self, window, cx);
            }
        }
    }

    /// True when something that moves can be on screen: a live status icon
    /// (a task works or needs input, the link connects, a dialog waits for a
    /// worker or a sign-in), an icon button under the pointer, or a mark
    /// that animates. What really drew is counted by [`FrameClock`] after
    /// each draw.
    fn motion_may_show(&self, cx: &gpui_kit::App) -> bool {
        self.state.tasks.iter().any(|t| {
            matches!(
                t.status,
                consort_client::TaskStatus::Working | consort_client::TaskStatus::NeedsInput
            )
        }) || matches!(
            self.state.connection,
            Connection::Connecting | Connection::Offline { .. }
        ) || self.dialog == Some(Dialog::AddWorker)
            || matches!(self.dialog, Some(Dialog::AddAccount(_)))
            || self.page == Page::Setup
            || self
                .state
                .session_host
                .iter()
                .flat_map(|w| w.tools.iter())
                .any(|t| t.state == consort_client::ToolState::Installing)
            || self
                .state
                .sidebar
                .groups
                .iter()
                .flat_map(|g| g.channels.iter())
                .chain(self.state.sidebar.ungrouped.iter())
                .chain(std::iter::once(&self.state.sidebar.tutti))
                .any(|c| {
                    matches!(
                        c.status,
                        Some(
                            consort_client::TaskStatus::Working
                                | consort_client::TaskStatus::NeedsInput
                        )
                    )
                })
            || cx.global::<FrameClock>().hovered().is_some()
            || self.mark_phase(cx).animates()
    }

    /// True while a task or an assistant works or waits for the user.
    fn work_shows(&self, cx: &gpui_kit::App) -> bool {
        self.state.tasks.iter().any(|t| {
            matches!(
                t.status,
                consort_client::TaskStatus::Working | consort_client::TaskStatus::NeedsInput
            )
        }) || matches!(
            self.mark_phase(cx),
            MarkPhase::Working | MarkPhase::NeedsYou
        )
    }

    /// What the mark shows now.
    pub fn mark_phase(&self, cx: &gpui_kit::App) -> MarkPhase {
        self.mark_forced
            .unwrap_or_else(|| self.mark.phase(cx.background_executor().now()))
    }

    /// Gives the mark the activity of the open channel. When work ends, the
    /// mark holds Done for `motion.done_hold`, and a timer draws again at
    /// its end.
    fn follow_mark(&mut self, cx: &mut Context<Self>) {
        let state = self.state.activity(&self.channel_id).state;
        let hold = Self::tokens(cx).motion.done_hold;
        let now = cx.background_executor().now();
        if let Some(until) = self.mark.follow(&self.channel_id, state, now, hold) {
            cx.spawn(async move |this, cx| {
                let wait = until.saturating_duration_since(cx.background_executor().now());
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    /// Starts the timer of the stop motion: the Three voices mark and the
    /// icon shimmer, at 8 frames a second (`motion.frame`). It runs only
    /// while a draw showed something that moves and the window is active.
    /// It stops itself at the first tick that finds nothing that moves, no
    /// active window, or Reduce motion on. See [`FrameClock`].
    fn follow_clock(&mut self, cx: &mut Context<Self>) {
        if self.clock_ticker
            || !crate::ui::motion::clock_runs(
                self.motion_may_show(cx),
                self.work_shows(cx),
                self.window_active,
                cx.reduce_motion(),
            )
        {
            return;
        }
        self.clock_ticker = true;
        let frame = Self::tokens(cx).motion.frame;
        cx.spawn(async move |this, cx| {
            let mut period = frame;
            loop {
                cx.background_executor().timer(period).await;
                let next = this.update(cx, |this, cx| {
                    // The last draw says if something moved, and how fast.
                    let clock = cx.global::<FrameClock>();
                    let runs = crate::ui::motion::clock_runs(
                        clock.used(),
                        clock.fast() || this.work_shows(cx),
                        this.window_active,
                        cx.reduce_motion(),
                    );
                    if !runs {
                        this.clock_ticker = false;
                        return None;
                    }
                    // Only the idle mark moved: half speed.
                    let next = if clock.fast() { frame } else { frame * 2 };
                    clock.advance();
                    if !clock.fast() {
                        // The idle mark changes frame every two ticks.
                        clock.advance();
                    }
                    cx.notify();
                    Some(next)
                });
                match next {
                    Ok(Some(next)) => period = next,
                    _ => break,
                }
            }
        })
        .detach();
    }

    /// Loads the next page of older messages when the reader reaches the
    /// top of a timeline that has more on the hub.
    pub fn load_older(&mut self, cx: &mut Context<Self>) {
        let channel_id = self.channel_id.clone();
        let more = self
            .state
            .timeline(&channel_id)
            .is_some_and(|t| t.has_older);
        if !more || self.loading_older.as_deref() == Some(channel_id.as_str()) {
            return;
        }
        self.loading_older = Some(channel_id.clone());
        let client = self.client.clone();
        let channel = channel_id.clone();
        cx.spawn(async move |this, cx| {
            let result = client.load_older(channel_id).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_older = None;
                if let Err(err) = result {
                    // Older messages did not load: the notice says so, and
                    // its button tries again.
                    let message = format!("Could not load older messages. {err}");
                    this.notices.show_with_action(
                        NoticeKind::Error,
                        message,
                        NoticeAction::LoadOlder(channel),
                    );
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn close_panel(&mut self, cx: &mut Context<Self>) {
        self.panel = Panel::None;
        self.sync_lists();
        cx.notify();
    }

    pub fn open_settings(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        self.page = Page::Settings;
        self.layout.drawer_open = false;
        self.layout.settings_list = false;
        self.settings_section = section;
        // A choice in the Settings list shows the list, not a routine that
        // was open in it.
        self.knowledge.settings_routine = None;
        if section == SettingsSection::Workers {
            self.load_registry_for_logos(cx);
        }
        if section == SettingsSection::Connectors {
            self.enter_connectors(cx);
        }
        if section == SettingsSection::Proactivity {
            self.load_proactivity_log(cx);
        }
        if section == SettingsSection::Voice {
            self.load_voice(cx);
        }
        if section == SettingsSection::Skills {
            self.enter_skills(cx);
        }
        self.enter_admin_section(section, cx);
        if self.settings_worker.is_none() {
            self.settings_worker = self
                .state
                .workers
                .iter()
                .find(|w| w.name == "bigboi")
                .or(self.state.workers.first())
                .map(|w| w.id.clone());
        }
        cx.notify();
    }

    /// Loads the ACP registry once, when an account of a registry agent
    /// needs its logo. The client does nothing when it has the registry.
    pub fn load_registry_for_logos(&mut self, cx: &mut Context<Self>) {
        let needed = self.state.accounts().map(|(_, a)| a).any(|a| {
            matches!(
                a.logo,
                consort_client::ProviderLogo::Registry { svg: None, .. }
            )
        });
        if needed && self.state.acp_registry.load == consort_client::RegistryLoad::NotLoaded {
            let client = self.client.clone();
            cx.spawn(async move |_, _| {
                let _ = client.load_acp_registry().await;
            })
            .detach();
        }
    }

    pub fn open_dialog(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.picker = None;
        self.effort_menu = None;
        self.dialog_error = None;
        self.dialog_serial += 1;
        match &dialog {
            Dialog::NewChannel => {
                self.convo.new_channel_icon = crate::convo::NEW_CHANNEL_ICONS[0].to_owned();
                self.set_field(Field::Name, "", window, cx);
                focus_input(&self.inputs.name, window, cx);
            }
            Dialog::NewGroup(_) => {
                self.set_field(Field::Name, "", window, cx);
                focus_input(&self.inputs.name, window, cx);
            }
            Dialog::NewTask
            | Dialog::ChannelSettings(_)
            | Dialog::Permissions(_)
            | Dialog::RenameGroup(_)
            | Dialog::ConfirmDeleteGroup(_)
            | Dialog::ConfirmDeleteTask(_)
            | Dialog::SkillDiff(_)
            | Dialog::About => self.prepare_convo_dialog(&dialog, window, cx),
            Dialog::Voice(voice) => self.prepare_voice_dialog(voice, window, cx),
            Dialog::RenameWorker(id) => {
                let name = self
                    .state
                    .worker(id)
                    .map(|w| w.name.clone())
                    .unwrap_or_default();
                self.set_field(Field::Name, &name, window, cx);
                focus_input(&self.inputs.name, window, cx);
            }
            Dialog::EditTags(id) => {
                let tags = self
                    .state
                    .worker(id)
                    .map(|w| w.tags.join(", "))
                    .unwrap_or_default();
                self.set_field(Field::Name, &tags, window, cx);
                focus_input(&self.inputs.name, window, cx);
            }
            Dialog::DeepgramKey => self.clear_deepgram_key_field(window, cx),
            Dialog::AddWorker => {
                self.add_worker_os = crate::ui::add_worker::default_os();
                self.add_worker_step = crate::ui::add_worker::AddWorkerStep::default();
                self.add_worker_before = self.state.workers.iter().map(|w| w.id.clone()).collect();
                self.added_worker = None;
                let client = self.client.clone();
                let serial = self.dialog_serial;
                // Without a code the dialog is empty, so its error shows
                // in the dialog.
                cx.spawn(async move |this, cx| {
                    if let Err(err) = client.worker_pairing_code().await {
                        let _ = this.update(cx, |this, cx| {
                            if this.dialog_serial == serial && this.dialog.is_some() {
                                this.dialog_error = Some(err.to_string());
                            } else {
                                this.show_error(err.to_string(), cx);
                            }
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
            Dialog::AddAccount(worker_id) => {
                self.add_account = AddAccountForm {
                    worker_id: worker_id.clone(),
                    provider: Some(ProviderPick::Builtin("claude")),
                    ..AddAccountForm::default()
                };
                for input in [
                    &self.inputs.agent_search,
                    &self.inputs.program,
                    &self.inputs.program_args,
                    &self.inputs.account_name,
                    &self.inputs.sign_in_code,
                    &self.inputs.api_key,
                ] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
                let client = self.client.clone();
                self.spawn_command(cx, async move { client.load_acp_registry().await });
            }
            Dialog::WorkerFolders(_) => {
                let folder = self.inputs.folder.clone();
                folder.update(cx, |input, cx| input.set_value("", window, cx));
                focus_input(&folder, window, cx);
            }
            Dialog::Endpoint(id) => {
                let id = id.clone();
                self.prepare_endpoint_dialog(id.as_deref(), window, cx);
            }
            Dialog::RenameAccount(worker_id, account_id) => {
                let name = self
                    .state
                    .worker(worker_id)
                    .and_then(|w| w.account(account_id))
                    .map(|a| a.label.clone())
                    .unwrap_or_default();
                self.set_field(Field::Name, &name, window, cx);
                focus_input(&self.inputs.name, window, cx);
            }
            Dialog::RenameFile(_, path) => {
                let name = consort_client::files::split_path(path).1.to_owned();
                self.set_field(Field::Name, &name, window, cx);
                focus_input(&self.inputs.name, window, cx);
            }
            Dialog::MoveFile(_, path) => {
                self.move_to = consort_client::files::split_path(path).0.to_owned();
            }
            Dialog::ConfirmDeleteFile(..) | Dialog::ChooseChannelFiles(_) => {}
            Dialog::RestartWithTasks
            | Dialog::LocalRemove
            | Dialog::LocalMove
            | Dialog::LocalConnectOther => {}
            Dialog::PairClient => self.start_client_pairing(cx),
            Dialog::Routine(_)
            | Dialog::ConfirmDeleteRoutine(_)
            | Dialog::NewNote(_)
            | Dialog::RenameNote(..)
            | Dialog::ConfirmDeleteNote(..) => {}
            Dialog::ConfirmDeleteChannel(_)
            | Dialog::ConfirmRemoveWorker(_)
            | Dialog::ConfirmOtherHarnessStep
            | Dialog::ConfirmRemoveAccount(..)
            | Dialog::ConfirmRemoveEndpoint(_)
            | Dialog::ConfirmOpenLink(_)
            | Dialog::Connect
            | Dialog::ConfirmDisconnect(_)
            | Dialog::ConfirmRemoveSkill(_)
            | Dialog::SkillFiles(_) => {}
            Dialog::ConnectionActivity(id) => {
                let id = id.clone();
                self.load_activity(id, false, cx);
            }
            Dialog::AddSkillSource => {
                let input = self.ext_inputs.source_url.clone();
                input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
            }
            Dialog::MonidKey => {
                let input = self.monid_inputs.key.clone();
                input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
                self.load_monid_status(cx);
            }
            Dialog::ComposioKey => {
                let input = self.ext_inputs.composio_key.clone();
                input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
            }
            Dialog::AddConnector => {
                let input = self.ext_inputs.catalog_search.clone();
                input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
                let client = self.client.clone();
                self.spawn_command(cx, async move {
                    client
                        .run(consort_client::Command::LoadConnectorCategories)
                        .await?;
                    client
                        .run(consort_client::Command::SearchConnectorCatalog {
                            search: String::new(),
                            category: None,
                        })
                        .await
                });
            }
            Dialog::AddSkill => {
                self.ext.add_skill = crate::extensions::AddSkillForm {
                    use_in: crate::extensions::default_use_in(&self.state, &self.channel_id),
                    ..Default::default()
                };
                for input in [
                    &self.ext_inputs.skill_search,
                    &self.ext_inputs.git_url,
                    &self.ext_inputs.git_path,
                ] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
                let search = self.ext_inputs.skill_search.clone();
                search.update(cx, |input, cx| input.focus(window, cx));
                let client = self.client.clone();
                self.spawn_command(cx, async move {
                    client
                        .run(consort_client::Command::LoadSkillDirectorySources)
                        .await?;
                    client
                        .run(consort_client::Command::SearchSkillDirectory {
                            search: String::new(),
                            source: None,
                        })
                        .await
                });
            }
            Dialog::AccountSettings(worker_id, account_id) => {
                let name = self
                    .state
                    .worker(worker_id)
                    .and_then(|w| w.account(account_id))
                    .map(|a| a.label.clone())
                    .unwrap_or_default();
                self.set_field(Field::Name, &name, window, cx);
            }
        }
        self.dialog = Some(dialog);
        cx.notify();
    }

    pub fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.dialog, Some(Dialog::AddAccount(_))) {
            self.end_add_account(cx);
        }
        if self.dialog == Some(Dialog::PairClient) {
            self.end_client_pairing(cx);
        }
        if self.dialog == Some(Dialog::RestartWithTasks)
            && matches!(
                self.local.hub.view().swap,
                consort_client::local_hub::SwapState::NeedsChoice { .. }
            )
        {
            let hub = self.local.hub.clone();
            self.local.restart_after_swap = false;
            crate::ui::update_notice::allow_restart();
            self.spawn_command(cx, async move { hub.cancel_swap().await });
        }
        if self.leaving_hub {
            self.leaving_hub = false;
            self.page = Page::Setup;
            self.setup.gate = consort_client::setup::Gate::Neutral;
        }
        if self.dialog == Some(Dialog::AddWorker) {
            let client = self.client.clone();
            self.spawn_command(cx, async move {
                client
                    .run(consort_client::Command::CancelWorkerPairing)
                    .await
            });
            self.added_worker = None;
        }
        self.dialog = None;
        self.dialog_error = None;
        self.picker = None;
        self.effort_menu = None;
        self.convo.icon_picker = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Runs the command of the open dialog. The dialog stays open while the
    /// command runs, and a second submit waits. The dialog closes when the
    /// command succeeds. When it fails, the dialog stays with its text and
    /// shows the error, so the user can fix it and try again.
    pub fn submit_dialog<F>(&mut self, command: F, window: &mut Window, cx: &mut Context<Self>)
    where
        F: std::future::Future<Output = Result<(), ClientError>> + 'static,
    {
        if !self.in_flight.begin(DIALOG_KEY) {
            return;
        }
        self.dialog_error = None;
        let serial = self.dialog_serial;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = command.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.in_flight.finish(DIALOG_KEY);
                // The user may have closed the dialog while the command ran.
                let still_open = this.dialog_serial == serial && this.dialog.is_some();
                match result {
                    Ok(()) if still_open => this.close_dialog(window, cx),
                    Ok(()) => {}
                    Err(err) => {
                        // A failed command makes no channel to open.
                        this.open_created = None;
                        this.leaving_hub = false;
                        if still_open {
                            this.dialog_error = Some(err.to_string());
                        } else {
                            this.show_error(err.to_string(), cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// True while the open dialog waits for its command.
    pub fn dialog_busy(&self) -> bool {
        self.in_flight.is_running(DIALOG_KEY)
    }

    pub fn set_field(
        &self,
        field: Field,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = match field {
            Field::Name => &self.inputs.name,
            Field::HubUrl => &self.inputs.hub_url,
            Field::SetupCode => &self.inputs.setup_code,
        };
        let value = value.to_owned();
        input.update(cx, |input, cx| input.set_value(value, window, cx));
    }

    pub fn field(&self, field: Field, cx: &gpui_kit::App) -> String {
        let input = match field {
            Field::Name => &self.inputs.name,
            Field::HubUrl => &self.inputs.hub_url,
            Field::SetupCode => &self.inputs.setup_code,
        };
        input.read(cx).value().trim().to_owned()
    }

    // Commands

    /// Runs a client command in the background, and shows its error.
    pub fn spawn_command<F>(&self, cx: &mut Context<Self>, command: F)
    where
        F: std::future::Future<Output = Result<(), ClientError>> + 'static,
    {
        cx.spawn(async move |this, cx| {
            if let Err(err) = command.await {
                let _ = this.update(cx, |this, cx| this.show_error(err.to_string(), cx));
            }
        })
        .detach();
    }

    /// Shows a short fact. It goes away by itself.
    pub fn show_notice(&mut self, message: String, cx: &mut Context<Self>) {
        self.push_notice(NoticeKind::Info, message, cx);
    }

    /// Shows an error. It stays until the user dismisses it.
    pub fn show_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.push_notice(NoticeKind::Error, message, cx);
    }

    fn push_notice(&mut self, kind: NoticeKind, message: String, cx: &mut Context<Self>) {
        let Some(notice) = self.notices.show(kind, message) else {
            return;
        };
        cx.notify();
        if kind == NoticeKind::Info {
            // The timer clears only its own notice. A newer one stays.
            let serial = notice.serial;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(INFO_TIMEOUT).await;
                let _ = this.update(cx, |this, cx| {
                    if this.notices.clear_if(serial) {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    /// Runs a command once at a time for `key`. A second call with the same
    /// key does nothing until the first ends. A failure shows the error with
    /// `kept`, a sentence that says what the user still has. On success,
    /// `on_ok` runs.
    pub(crate) fn run_guarded<F>(
        &mut self,
        key: String,
        command: F,
        kept: Kept,
        on_ok: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        F: std::future::Future<Output = Result<(), ClientError>> + 'static,
    {
        if !self.in_flight.begin(&key) {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let result = command.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.in_flight.finish(&key);
                match result {
                    Ok(()) => on_ok(this, window, cx),
                    Err(err) => this.show_error(kept.failure(&err), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Empties a message box after its text went out, if the box still holds
    /// what was sent. Text that the user typed meanwhile stays.
    ///
    /// Clearing a box sends no change event, so the drafts on disk would keep
    /// the sent text: a save that ran while the user paused before Enter
    /// wrote it there. So the saved drafts forget the sent text here too, and
    /// the next start shows an empty box.
    pub(crate) fn clear_draft_if_sent<M: gpui_kit::component::input::InputModeKind>(
        &mut self,
        input: &Entity<gpui_kit::base::input::InputBaseState<M>>,
        sent: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = input.read(cx).value().to_string();
        if drafts::can_clear_after_send(sent, &now) {
            input.update(cx, |input, cx| input.set_value("", window, cx));
            let channel = self.composer_for.clone();
            let thread = self.reply_for.clone();
            self.forget_sent_draft(Some(&channel), thread.as_deref(), sent);
            self.save_drafts_soon(cx);
        }
    }

    pub fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // No channel has loaded: there is nowhere to send to.
        if self.waiting().is_some() {
            return;
        }
        if self.hold_send_for_dictation(crate::ui::menu::TextTarget::Composer, cx) {
            return;
        }
        let body = self.inputs.composer.read(cx).value().trim().to_owned();
        let target = consort_client::UploadTarget::Composer {
            channel_id: self.channel_id.clone(),
        };
        let attachments = self.upload_ids(&target);
        if body.is_empty() && attachments.is_empty() {
            return;
        }
        let client = self.client.clone();
        let channel_id = self.channel_id.clone();
        let sent_to = channel_id.clone();
        let input = self.inputs.composer.clone();
        let sent = body.clone();
        // The text stays in the box until the client takes the message. The
        // client waits for the uploads first.
        self.run_guarded(
            "composer".into(),
            async move {
                client
                    .send_message_with(channel_id, body, attachments)
                    .await
            },
            Kept::Draft("Your message is still in the box."),
            move |this, window, cx| {
                if this.composer_for == sent_to {
                    this.clear_draft_if_sent(&input, &sent, window, cx);
                } else {
                    // The user went to another channel meanwhile. The draft
                    // of the channel they left is in the drafts.
                    this.forget_sent_draft(Some(&sent_to), None, &sent);
                    this.save_drafts_soon(cx);
                }
            },
            window,
            cx,
        );
    }

    pub fn send_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Panel::Thread(task_id) = self.panel.clone() else {
            return;
        };
        if self.hold_send_for_dictation(crate::ui::menu::TextTarget::Reply, cx) {
            return;
        }
        let body = self.inputs.reply.read(cx).value().trim().to_owned();
        let target = consort_client::UploadTarget::Reply {
            task_id: task_id.clone(),
        };
        let attachments = self.upload_ids(&target);
        if body.is_empty() && attachments.is_empty() {
            return;
        }
        let client = self.client.clone();
        let input = self.inputs.reply.clone();
        let sent = body.clone();
        let sent_to = task_id.clone();
        self.run_guarded(
            "reply".into(),
            async move { client.reply_to_task_with(task_id, body, attachments).await },
            Kept::Draft("Your reply is still in the box."),
            move |this, window, cx| {
                if this.reply_for.as_deref() == Some(sent_to.as_str()) {
                    this.clear_draft_if_sent(&input, &sent, window, cx);
                } else {
                    this.forget_sent_draft(None, Some(&sent_to), &sent);
                    this.save_drafts_soon(cx);
                }
            },
            window,
            cx,
        );
    }

    pub fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(message_id) = self.editing.clone() else {
            return;
        };
        let body = self.inputs.edit.read(cx).value().trim().to_owned();
        let unchanged = self.state.timelines.iter().flat_map(|t| t.items.iter()).any(|i| {
            matches!(i, consort_client::TimelineItem::Message(m) if m.id == message_id && m.body.trim() == body)
        });
        if body.is_empty() || unchanged {
            self.cancel_edit(window, cx);
            return;
        }
        let client = self.client.clone();
        let edited = message_id.clone();
        // The edit box stays open until the hub takes the new text.
        self.run_guarded(
            format!("edit:{message_id}"),
            async move { client.edit_message(edited, body).await },
            Kept::Draft("Your edit is still in the box."),
            move |this, window, cx| {
                if this.editing.as_deref() == Some(message_id.as_str()) {
                    this.cancel_edit(window, cx);
                }
            },
            window,
            cx,
        );
    }

    pub fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.timeline.remeasure();
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub fn answer_approval(
        &mut self,
        id: String,
        choice: ApprovalChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.blocked_by_hub(cx) {
            return;
        }
        let client = self.client.clone();
        let approval_id = id.clone();
        self.run_guarded(
            format!("approval:{id}"),
            async move { client.answer_approval(approval_id, choice).await },
            Kept::Request("The request still waits for your answer."),
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// Answers a decision with an option, or with text from the answer box.
    /// The answer box closes and empties only after the hub takes the answer.
    pub fn answer_decision(
        &mut self,
        id: String,
        option: Option<u32>,
        text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.blocked_by_hub(cx) {
            return;
        }
        let client = self.client.clone();
        let decision_id = id.clone();
        let sent = text.clone();
        let input = self.inputs.decision_answer.clone();
        self.run_guarded(
            format!("decision:{id}"),
            async move { client.answer_decision(decision_id, option, text).await },
            Kept::Request("The decision still waits for your answer."),
            move |this, window, cx| {
                if this.decision_writing.as_deref() == Some(id.as_str()) {
                    this.decision_writing = None;
                }
                if let Some(sent) = sent {
                    this.clear_draft_if_sent(&input, &sent, window, cx);
                }
            },
            window,
            cx,
        );
    }

    /// Adds a quote after the draft in a message box. The draft stays.
    fn quote_into(
        input: &Entity<EditorState>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let quote = drafts::quote_block(text);
        input.update(cx, |input, cx| {
            let draft = input.value().to_string();
            input.set_value(drafts::append_quote(&draft, &quote), window, cx);
            input.focus(window, cx);
        });
    }

    pub fn quote_in_reply(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = if matches!(self.panel, Panel::Thread(_)) {
            self.inputs.reply.clone()
        } else {
            self.inputs.composer.clone()
        };
        Self::quote_into(&target, text, window, cx);
    }

    pub fn ask_consort(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tutti) = self.state.tutti().map(|c| c.id.clone()) {
            self.open_channel(tutti, window, cx);
        }
        // The composer may hold a draft. The quote goes after it.
        let composer = self.inputs.composer.clone();
        Self::quote_into(&composer, text, window, cx);
    }

    /// Acts on a link that the reader clicked.
    pub fn follow_link(&mut self, action: LinkAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            LinkAction::Open(url) => cx.open_url(&url),
            LinkAction::Confirm(url) => self.open_dialog(Dialog::ConfirmOpenLink(url), window, cx),
            LinkAction::Block(blocked) => self.show_notice(blocked.notice(), cx),
        }
    }

    pub fn open_menu(&mut self, menu: OpenMenu, cx: &mut Context<Self>) {
        self.menu_anchor = Some(menu.clone());
        self.menu = Some(menu);
        cx.notify();
    }

    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
    }

    /// Runs a menu item.
    pub fn run_menu(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let client = self.client.clone();
        match action {
            MenuAction::Copy(text) | MenuAction::CopyMarkdown(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            MenuAction::QuoteInReply(text) => self.quote_in_reply(&text, window, cx),
            MenuAction::AskConsort(text) => self.ask_consort(&text, window, cx),
            MenuAction::OpenThread(task_id) => self.open_thread(task_id, window, cx),
            MenuAction::ReplyInThread(message_id) => {
                self.open_message_thread(message_id, window, cx)
            }
            MenuAction::Convo(action) => self.run_convo(action, window, cx),
            MenuAction::Fallback(action) => self.run_fallback(action, window, cx),
            MenuAction::EditMessage(message_id, text) => {
                self.editing = Some(message_id);
                let end = text.len();
                self.inputs.edit.update(cx, |input, cx| {
                    input.set_value(text, window, cx);
                    input.set_selected_range(end..end, cx);
                    input.focus(window, cx);
                });
                // The row grows, so measure it again.
                self.timeline.remeasure();
            }
            MenuAction::DeleteMessage(message_id) => self.spawn_command(cx, async move {
                client
                    .run(consort_client::Command::DeleteMessage { message_id })
                    .await
            }),
            MenuAction::ResolveTask(id) => {
                self.spawn_command(cx, async move { client.resolve_task(id).await })
            }
            MenuAction::ReopenTask(id) => {
                self.spawn_command(cx, async move { client.reopen_task(id).await })
            }
            MenuAction::RetryTask(id) => {
                self.spawn_command(cx, async move { client.retry_task(id).await })
            }
            MenuAction::StopTask(id) => {
                self.spawn_command(cx, async move { client.stop_task(id).await })
            }
            MenuAction::ArchiveTask(id) => {
                self.spawn_command(cx, async move { client.archive_task(id).await })
            }
            // Rename, Instructions, Model and Worker tags all open the
            // channel settings, which replaced their dialogs.
            MenuAction::RenameChannel(id) | MenuAction::ChannelInstructions(id) => {
                self.open_dialog(Dialog::ChannelSettings(id), window, cx)
            }
            MenuAction::ChangeIcon(id) => {
                self.open_icon_picker(crate::convo::IconTarget::Channel(id), window, cx)
            }
            MenuAction::SetIcon(id, icon) => self.spawn_command(cx, async move {
                client
                    .update_channel(
                        id,
                        ChannelChanges {
                            icon: Some(icon),
                            ..ChannelChanges::default()
                        },
                    )
                    .await
            }),
            MenuAction::MoveToGroup(id) => {
                if let Some(anchor) = self.last_menu_anchor() {
                    let current = self.state.channel(&id).and_then(|c| c.group_id.clone());
                    let items = group_menu(&self.state, &id, current.as_deref());
                    self.open_menu(OpenMenu { items, ..anchor }, cx);
                }
            }
            MenuAction::SetGroup(id, group) => {
                self.spawn_command(
                    cx,
                    async move { client.move_channel(id, Some(group)).await },
                )
            }
            MenuAction::NoGroup(id) => {
                self.spawn_command(cx, async move { client.move_channel(id, None).await })
            }
            MenuAction::NewGroup(channel) => {
                self.open_dialog(Dialog::NewGroup(channel), window, cx)
            }
            MenuAction::UnarchiveChannel(id) => {
                self.spawn_command(cx, async move { client.unarchive_channel(id).await })
            }
            MenuAction::ArchiveChannel(id) => {
                self.spawn_command(cx, async move { client.archive_channel(id).await })
            }
            MenuAction::DeleteChannel(id) => {
                self.open_dialog(Dialog::ConfirmDeleteChannel(id), window, cx)
            }
            MenuAction::RenameWorker(id) => self.open_dialog(Dialog::RenameWorker(id), window, cx),
            MenuAction::EditTags(id) => self.open_dialog(Dialog::EditTags(id), window, cx),
            MenuAction::PauseWorker(worker_id, paused) => self.spawn_command(cx, async move {
                client
                    .run(consort_client::Command::SetWorkerPaused { worker_id, paused })
                    .await
            }),
            MenuAction::RemoveWorker(id) => {
                self.open_dialog(Dialog::ConfirmRemoveWorker(id), window, cx)
            }
            MenuAction::ChannelTags(id) | MenuAction::ChannelModel(id) => {
                self.open_dialog(Dialog::ChannelSettings(id), window, cx)
            }
            MenuAction::CheckTools(worker_id) => {
                self.spawn_command(cx, async move { client.check_tools(worker_id).await });
                self.show_notice("Checking for updates.".into(), cx);
            }
            MenuAction::InstallTool(worker_id, tool) => {
                self.spawn_command(
                    cx,
                    async move { client.install_tool(worker_id, tool).await },
                )
            }
            MenuAction::EditEndpoint(id) => {
                self.open_dialog(Dialog::Endpoint(Some(id)), window, cx)
            }
            MenuAction::TestEndpoint(id) => self.test_saved_endpoint(id, cx),
            MenuAction::RemoveEndpoint(id) => {
                self.open_dialog(Dialog::ConfirmRemoveEndpoint(id), window, cx)
            }
            MenuAction::RenameAccount(worker_id, account_id) => {
                self.open_dialog(Dialog::RenameAccount(worker_id, account_id), window, cx)
            }
            MenuAction::SignOutAccount(worker_id, account_id) => self
                .spawn_command(cx, async move {
                    client.sign_out_account(worker_id, account_id).await
                }),
            MenuAction::RemoveAccount(worker_id, account_id) => self.open_dialog(
                Dialog::ConfirmRemoveAccount(worker_id, account_id),
                window,
                cx,
            ),
            MenuAction::ChannelExtensions(id) => self.open_channel_connectors(id, window, cx),
            MenuAction::ReconnectConnection(id) => self.reconnect(id, window, cx),
            MenuAction::Disconnect(id) => {
                self.open_dialog(Dialog::ConfirmDisconnect(id), window, cx)
            }
            MenuAction::SetRule(target, kind, rule) => match target {
                Some(id) => {
                    let approvals = self
                        .state
                        .connection(&id)
                        .map(|c| c.approvals.with(kind, rule));
                    if let Some(approvals) = approvals {
                        self.update_connection(
                            id,
                            consort_client::ConnectionChanges {
                                approvals: Some(approvals),
                                ..Default::default()
                            },
                            cx,
                        );
                    }
                }
                None => self.ext.connect.approvals = self.ext.connect.approvals.with(kind, rule),
            },
            MenuAction::SetSkillEnabled(id, enabled) => self.update_skill(
                id,
                consort_client::SkillChanges {
                    enabled: Some(enabled),
                    ..Default::default()
                },
                cx,
            ),
            MenuAction::SkillFiles(id) => self.open_skill_files(id, window, cx),
            MenuAction::ConnectionActivity(id) => {
                self.open_dialog(Dialog::ConnectionActivity(id), window, cx)
            }
            MenuAction::RemoveSkillSource(id) => self.remove_skill_source(id, cx),
            MenuAction::RemoveSkill(id) => {
                self.open_dialog(Dialog::ConfirmRemoveSkill(id), window, cx)
            }
            MenuAction::AccountSettings(worker_id, account_id) => {
                self.open_dialog(Dialog::AccountSettings(worker_id, account_id), window, cx)
            }
            MenuAction::RemoveComposioKey => {
                self.spawn_command(cx, async move {
                    client.run(consort_client::Command::RemoveComposioKey).await
                });
                self.close_dialog(window, cx);
            }
            MenuAction::TextEdit(target, op) => self.text_edit(target, op, window, cx),
            MenuAction::Select(id, option) => self.pick_select(id, &option, window, cx),
            // Files and attachments.
            MenuAction::OpenAttachment(att) => self.open_blob(att.sha256, att.size, att.name, cx),
            MenuAction::SaveAttachment(att) => self.save_blob(att.sha256, att.size, att.name, cx),
            MenuAction::SaveToChannelFiles(channel_id, att) => {
                self.save_to_channel_files(channel_id, att, cx)
            }
            MenuAction::AttachFromComputer(target) => self.choose_files(target, cx),
            MenuAction::AttachFromChannel(target) => {
                self.open_dialog(Dialog::ChooseChannelFiles(target), window, cx)
            }
            MenuAction::RetryUpload(id) => self.spawn_command(cx, async move {
                client
                    .run(consort_client::Command::RetryUpload { id })
                    .await
            }),
            MenuAction::RemoveUpload(id) => self.spawn_command(cx, async move {
                client
                    .run(consort_client::Command::RemoveUpload { id })
                    .await
            }),
            MenuAction::OpenFile(file) => self.open_blob(file.sha256, file.size, file.name, cx),
            MenuAction::DownloadFile(file) => self.save_blob(file.sha256, file.size, file.name, cx),
            MenuAction::RenameFile(channel_id, path) => {
                self.open_dialog(Dialog::RenameFile(channel_id, path), window, cx)
            }
            MenuAction::MoveFile(channel_id, path) => {
                self.open_dialog(Dialog::MoveFile(channel_id, path), window, cx)
            }
            MenuAction::DeleteFile(channel_id, path) => {
                self.open_dialog(Dialog::ConfirmDeleteFile(channel_id, path), window, cx)
            }
            MenuAction::Permissions(action) => self.run_permissions_action(action, window, cx),
            MenuAction::Knowledge(action) => self.run_knowledge_menu(action, window, cx),
            MenuAction::Monid(action) => self.run_monid_action(action, window, cx),
            MenuAction::Visual(action) => self.run_visual_action(action, window, cx),
        }
        cx.notify();
    }

    fn last_menu_anchor(&self) -> Option<OpenMenu> {
        // The submenu opens where the menu was. `run_menu` cleared the menu,
        // so the anchor is kept in `menu_anchor`.
        self.menu_anchor.clone()
    }

    /// Cut, Copy, Paste or Select all in one of the message boxes.
    fn text_edit(
        &mut self,
        target: TextTarget,
        op: EditOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::input::{Copy, Cut, Paste, SelectAll};
        let input = match target {
            TextTarget::Composer => &self.inputs.composer,
            TextTarget::Reply => &self.inputs.reply,
        };
        let handle = input.read(cx).focus_handle(cx);
        handle.focus(window, cx);
        let action: Box<dyn gpui_kit::Action> = match op {
            EditOp::Cut => Box::new(Cut),
            EditOp::Copy => Box::new(Copy),
            EditOp::Paste => Box::new(Paste),
            EditOp::SelectAll => Box::new(SelectAll),
        };
        window.dispatch_action(action, cx);
    }

    /// Whether the Add tag field of the worker detail in Settings shows.
    fn tag_field_open(&self) -> bool {
        self.page == Page::Settings
            && self.settings_section == SettingsSection::Workers
            && self.tag_entry.is_some()
            && self.tag_entry == self.settings_worker
    }

    /// Whether something is open over the timeline: a menu, a picker, a
    /// popover, a dialog, the light box, the switcher, a notice, the
    /// drawer, a panel over the whole window, the drop state of files, or a
    /// page other than the main one. While it is, a visual shows its image
    /// and not its live page, because the native webview of the page would
    /// draw over the overlay (`ui/visual_cover.rs`). GPUI tooltips are not
    /// in this list: the app does not know when one shows.
    pub fn covers_timeline(&self, t: &Tokens) -> bool {
        self.covers() || self.layout_frame(t).panel_over
    }

    /// Whether something is open over a task thread: the same overlays as
    /// for the timeline, but a panel over the whole window is the thread
    /// itself.
    pub fn covers_thread(&self) -> bool {
        self.covers()
    }

    /// Whether something is open over the light box: a menu, a picker, the
    /// switcher or a notice. Dialogs and panels are under it.
    pub fn covers_lightbox(&self) -> bool {
        self.menu.is_some()
            || self.picker.is_some()
            || self.effort_menu.is_some()
            || self.convo.emoji_picker.is_some()
            || self.switcher.is_some()
            || self.notices.visible().next().is_some()
            || self.dictation.notice.is_some()
    }

    /// The overlays over the timeline and a task thread.
    fn covers(&self) -> bool {
        let menus = self.menu.is_some()
            || self.picker.is_some()
            || self.effort_menu.is_some()
            || self.convo.emoji_picker.is_some()
            || self.ext.use_in_picker.is_some();
        let dialogs = self.dialog.is_some()
            || self.convo.icon_picker.is_some()
            || self.convo.permissions.rule.is_some()
            || self.convo.permissions.picker.is_some()
            || self
                .knowledge
                .routine_dialog
                .as_ref()
                .is_some_and(|d| d.zone_open);
        let layers = self.switcher.is_some()
            || self.lightbox.is_some()
            || self.notices.visible().next().is_some()
            || self.dictation.notice.is_some()
            || self.layout.drawer_open
            || self.drop_hover.is_some()
            || self.hold_drop;
        self.page != Page::Main || menus || dialogs || layers
    }

    /// Closes the Add tag field, with its text and its error.
    pub fn close_tag_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tag_entry = None;
        self.tag_error = None;
        self.inputs
            .tag
            .update(cx, |input, cx| input.set_value("", window, cx));
        // The focus goes back to the app, so keys do not reach a hidden input.
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.dictation_key_down(event, window, cx) {
            cx.stop_propagation();
            return;
        }
        let key = event.keystroke.key.as_str();
        if self.effort_key(key, window, cx) {
            cx.stop_propagation();
            return;
        }
        // The All icons dialog is on top of everything, so its keys come
        // first: Escape closes only it.
        if self.icon_picker_key(key, window, cx) {
            cx.stop_propagation();
            return;
        }
        if self.use_in_picker_key(key, cx) {
            cx.stop_propagation();
            return;
        }
        if self.permissions_key(key, window, cx) {
            cx.stop_propagation();
            return;
        }
        if let Some(menu) = self.menu.as_mut() {
            match key {
                "escape" => self.menu = None,
                "down" => menu.step(1),
                "up" => menu.step(-1),
                "enter" => {
                    if let Some(item) = menu
                        .highlighted
                        .and_then(|i| menu.items.get(i))
                        .filter(|item| !item.disabled)
                    {
                        let action = item.action.clone();
                        self.run_menu(action, window, cx);
                    }
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.lightbox.is_some() && self.dialog.is_none() {
            match key {
                "escape" => self.lightbox = None,
                "left" => self.step_lightbox(-1),
                "right" => self.step_lightbox(1),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "escape" {
            if self.dialog.is_some() {
                self.close_dialog(window, cx);
                cx.stop_propagation();
            } else if self.layout.drawer_open {
                self.layout.drawer_open = false;
                cx.notify();
                cx.stop_propagation();
            } else if self.tag_field_open() {
                // Escape closes the Add tag field of the worker, and only that.
                self.close_tag_entry(window, cx);
                cx.stop_propagation();
            } else if self.page == Page::Settings && self.close_settings_routine(cx) {
                // A routine open in Settings goes back to the list first.
                // A second Escape leaves Settings.
                cx.stop_propagation();
            } else if self.page == Page::Settings {
                self.page = Page::Main;
                self.mark_read(cx);
                cx.notify();
                cx.stop_propagation();
            } else if self.panel != Panel::None {
                self.close_panel(cx);
                cx.stop_propagation();
            }
        }
        let modifiers = event.keystroke.modifiers;
        let command = if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control
        };
        // Select all, Copy, Cut, Paste, Undo and Redo go to the focused input
        // when its own key map did not take the key press.
        if let Some(edit) = crate::edit_keys::EditKey::from_keystroke(key, command, modifiers.shift)
        {
            window.dispatch_action(edit.action(), cx);
            cx.stop_propagation();
            return;
        }
        if key == "\\" && command && self.page == Page::Main {
            self.toggle_sidebar(cx);
            cx.stop_propagation();
        }
        if key == "k" && command {
            if self.switcher.is_some() {
                self.close_switcher(window, cx);
            } else {
                self.open_switcher(window, cx);
            }
            cx.stop_propagation();
        }
    }
}

/// The text fields that dialogs share.
#[derive(Clone, Copy)]
pub enum Field {
    Name,
    HubUrl,
    SetupCode,
}

fn focus_input(input: &Entity<InputState>, window: &mut Window, cx: &mut Context<ConsortApp>) {
    input.update(cx, |input, cx| input.focus(window, cx));
}

/// The menu of "Move to group": every group, then No group and New group.
/// A hub with no group still gets a menu with two items.
pub fn group_menu(state: &ViewState, channel_id: &str, current: Option<&str>) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = state
        .groups
        .iter()
        .map(|g| {
            MenuItem::new(
                g.name.clone(),
                MenuAction::SetGroup(channel_id.to_owned(), g.id.clone()),
            )
            .checked(current == Some(g.id.as_str()))
        })
        .collect();
    let in_known_group = current.is_some_and(|c| state.groups.iter().any(|g| g.id == c));
    let mut no_group = MenuItem::new("No group", MenuAction::NoGroup(channel_id.to_owned()))
        .checked(!in_known_group);
    if !items.is_empty() {
        no_group = no_group.separated();
    }
    items.push(no_group);
    items.push(MenuItem::new(
        "New group",
        MenuAction::NewGroup(Some(channel_id.to_owned())),
    ));
    items
}

/// Keeps a virtual list in step with its rows. Only the rows between the
/// common start and the common end are spliced, so new messages at the end
/// keep the list following them, and an older page at the start keeps the
/// reader where they are.
fn sync_list(list: &ListState, current: &mut Vec<String>, keys: Vec<String>, force_reset: bool) {
    if force_reset {
        list.reset(keys.len());
        *current = keys;
        return;
    }
    let (start, old_end, new_end) = changed_range(current, &keys);
    if start == old_end && start == new_end {
        // Same rows: their content may have changed size.
        list.remeasure();
    } else {
        list.splice(start..old_end, new_end - start);
    }
    *current = keys;
}

/// The rows that differ between `old` and `new`: from the end of their
/// common start to the start of their common end, in each list.
fn changed_range(old: &[String], new: &[String]) -> (usize, usize, usize) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    (prefix, old.len() - suffix, new.len() - suffix)
}

impl Focusable for ConsortApp {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ConsortApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Self::tokens(cx);
        self.style_inputs(&t, cx);
        cx.global::<FrameClock>().begin_frame();
        self.begin_visual_frame(&t, window, cx);
        self.follow_mark(cx);
        self.apply_window_badge(window);
        self.dialog_confirm.replace(None);
        // A change of channel that came with a state (a deleted channel, a
        // channel that the hub just made) gives the composer its draft too.
        self.sync_draft_inputs(window, cx);
        // No channel yet: the composer takes no text.
        let waiting = self.waiting().is_some();
        self.inputs
            .composer
            .update(cx, |input, cx| input.set_disabled(waiting, cx));
        {
            use gpui_kit::Focusable as _;
            self.ext.key_field_focused = self
                .ext_inputs
                .composio_key
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
        }
        if self.placeholder_stale {
            self.placeholder_stale = false;
            self.update_placeholder(window, cx);
        }
        if self.ext_stale {
            self.ext_stale = false;
            self.follow_extensions(Some(window), cx);
        }
        // A drag of files that ended or left the window shows no drop state.
        if self.drop_hover.is_some() && !cx.has_active_drag() && !self.hold_drop {
            self.drop_hover = None;
        }
        let body = match self.page {
            Page::Onboarding => self.render_onboarding(&t, window, cx),
            Page::Setup => self.render_setup(&t, window, cx),
            Page::Settings => self.render_settings(&t, window, cx),
            Page::Main => self.render_main(&t, window, cx),
        };
        let has_notices =
            self.notices.visible().next().is_some() || self.dictation.notice.is_some();
        let menu = self.menu.clone();
        let picker = self.render_picker(&t, window, cx);
        let picker_tip = self.render_picker_tooltip(&t);
        let effort = self.render_effort_menu(&t, window, cx);
        self.follow_clock(cx);
        div()
            .id("consort")
            .key_context("Consort")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(|this, event: &gpui_kit::KeyUpEvent, _, cx| {
                this.dictation_key_up(event, cx);
            }))
            .on_action(
                cx.listener(|this, _: &crate::convo::ShowAbout, window, cx| {
                    this.open_dialog(Dialog::About, window, cx)
                }),
            )
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.colors.surface_canvas)
            .text_color(t.colors.text_primary)
            .font_family(t.font_sans.clone())
            .type_style(t.text.ui)
            // Windows and Linux: a title bar with the window controls. On
            // macOS the traffic lights sit in the sidebar instead.
            .children(crate::ui::window_controls::title_bar(&t, window))
            .child(
                // Dialogs and the switcher cover the app but not the title
                // bar, so the window can always move and close.
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .w_full()
                    .child(body)
                    .when_some(self.dialog.clone(), |layer, dialog| {
                        layer.child(self.render_dialog(&dialog, &t, window, cx))
                    })
                    // All icons opens over the dialog that opened it.
                    .children(self.render_icon_picker(&t, window, cx))
                    .children(self.render_use_in_picker(&t, window, cx))
                    // The rule dialog and the tool picker open over the
                    // Permissions dialog.
                    .children(self.render_rule_dialog(&t, window, cx))
                    .children(self.render_exception_picker(&t, window, cx))
                    .when(self.lightbox.is_some(), |layer| {
                        layer.child(self.render_lightbox(&t, window, cx))
                    })
                    .when(has_notices, |layer| {
                        layer.child(self.render_notice_stack(&t, cx))
                    })
                    .when(self.switcher.is_some(), |layer| {
                        layer.child(self.render_switcher(&t, cx))
                    }),
            )
            .children(picker)
            .children(picker_tip)
            .children(effort)
            .children(self.render_emoji_picker(&t, window, cx))
            .when_some(menu, |root, menu| {
                root.child(render_menu(
                    &menu,
                    &t,
                    window,
                    cx,
                    |this: &mut Self, action, window, cx| this.run_menu(action, window, cx),
                    |this: &mut Self, _, cx| this.close_menu(cx),
                ))
            })
            // Last: it hides each live page that no item showed in this
            // frame.
            .child(self.visuals.frame.sweep())
    }
}

#[cfg(test)]
mod tests {
    use super::changed_range;

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|k| (*k).to_owned()).collect()
    }

    #[test]
    fn appended_rows_splice_at_the_end() {
        assert_eq!(
            changed_range(&keys(&["a", "b"]), &keys(&["a", "b", "c"])),
            (2, 2, 3)
        );
    }

    #[test]
    fn an_older_page_splices_after_the_day_label() {
        let old = keys(&["day", "m3", "m4"]);
        let new = keys(&["day", "m1", "m2", "m3", "m4"]);
        assert_eq!(changed_range(&old, &new), (1, 1, 3));
    }

    #[test]
    fn a_removed_row_and_no_change() {
        assert_eq!(
            changed_range(&keys(&["a", "b", "c"]), &keys(&["a", "c"])),
            (1, 2, 1)
        );
        assert_eq!(changed_range(&keys(&["a"]), &keys(&["a"])), (1, 1, 1));
    }
}
