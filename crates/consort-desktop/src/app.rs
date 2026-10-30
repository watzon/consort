//! The root view: app state, client subscription, actions and the layout of
//! the window.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use consort_client::{
    ApprovalChoice, ChannelChanges, Client, ClientError, Connection, Drafts, Isolation, Os,
    SelectionView, ViewState,
};
use consort_theme::{ActiveTokens as _, Tokens, TypeExt as _};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ListAlignment, ListState, ParentElement, Render, Styled,
    Subscription, Window,
    component::input::{InputEvent, InputState, TextareaState},
    div,
    prelude::FluentBuilder as _,
};

use crate::{
    countdown::{self, RetryCountdown},
    drafts::{self, InFlight, Kept},
    notice::{INFO_TIMEOUT, NoticeKind, Notices},
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

/// The text that the New task buttons put in a message box.
pub const START_TASK_TEXT: &str = "Start a task to ";

/// Which page fills the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Main,
    Settings,
    Onboarding,
}

/// The side panel at the right of the timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Panel {
    None,
    Tasks,
    Thread(String),
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
    Connectors,
    Skills,
    Memory,
    Routines,
}

/// The open dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog {
    NewChannel,
    AddWorker,
    RenameChannel(String),
    Instructions(String),
    RenameWorker(String),
    EditTags(String),
    /// A small dialog that names a new group. With a channel ID, the channel
    /// moves into the group once it exists.
    NewGroup(Option<String>),
    ConfirmDeleteChannel(String),
    ConfirmRemoveWorker(String),
    /// A link whose words do not match its address: the address to show.
    ConfirmOpenLink(String),
    /// Add account, for a worker. The form is in `ConsortApp::add_account`.
    AddAccount(String),
    /// Rename an account: the worker ID and the account ID.
    RenameAccount(String, String),
    /// Remove an account: the worker ID and the account ID.
    ConfirmRemoveAccount(String, String),
    /// The worker tags of a channel.
    ChannelTags(String),
    /// Change or remove the Composio key.
    ComposioKey,
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
    /// The connectors and skills of a channel.
    ChannelExtensions(String),
    /// The settings of an account: the worker ID and the account ID.
    AccountSettings(String, String),
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
}

/// The fields of the New channel dialog that are not text inputs.
#[derive(Clone, Debug, Default)]
pub struct NewChannelForm {
    pub group_id: Option<String>,
    /// The channel goes in a new group. The group's name is in the
    /// `group_name` input.
    pub new_group: bool,
    pub workers: Vec<String>,
    /// "Model for the assistant". `None` is "Same as Tutti".
    pub coordinator_selection: Option<SelectionView>,
    /// "Model for tasks". `None` is "Same as Tutti".
    pub task_selection: Option<SelectionView>,
    pub isolation: Isolation,
    /// The worker tags that the channel requires.
    pub tags: Vec<String>,
}

/// The onboarding form state.
#[derive(Clone, Debug, Default)]
pub struct Onboarding {
    pub busy: bool,
    pub error: Option<String>,
}

/// Text inputs, made once and reused.
pub struct Inputs {
    pub composer: Entity<TextareaState>,
    pub reply: Entity<TextareaState>,
    pub decision_answer: Entity<TextareaState>,
    pub edit: Entity<TextareaState>,
    pub name: Entity<InputState>,
    pub purpose: Entity<InputState>,
    pub notes: Entity<TextareaState>,
    pub hub_url: Entity<InputState>,
    pub search: Entity<InputState>,
    pub setup_code: Entity<InputState>,
    /// The name of a new group in the New channel dialog.
    pub group_name: Entity<InputState>,
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
    pub menu: Option<OpenMenu>,
    pub menu_anchor: Option<OpenMenu>,
    /// The open model picker.
    pub picker: Option<OpenPicker>,
    /// The open effort menu.
    pub effort_menu: Option<OpenEffort>,
    pub inputs: Inputs,
    pub texts: TextCache,
    pub timeline: ListState,
    pub timeline_keys: Vec<String>,
    pub timeline_channel: String,
    pub thread: ListState,
    pub thread_keys: Vec<String>,
    pub notices: Notices,
    /// The IDs of the refused messages that a notice already showed.
    announced_failures: std::collections::HashSet<String>,
    pub new_channel: NewChannelForm,
    /// The tags in the open Worker tags dialog of a channel.
    pub channel_tags: Vec<String>,
    pub add_account: AddAccountForm,
    /// The worker whose Add tag field is open.
    pub tag_entry: Option<String>,
    /// Why the hub refused the last tag: the worker ID and the message.
    pub tag_error: Option<(String, String)>,
    pub add_worker_os: Os,
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
    placeholder_stale: bool,
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
    pub focus: FocusHandle,
    styled_for: Option<(gpui_kit::SharedString, consort_theme::Mode)>,
    _subscriptions: Vec<Subscription>,
}

impl ConsortApp {
    pub fn new(client: Client, demo: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = client.state();
        let drafts = client.load_drafts();
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
            composer: textarea("Message Consort", (1, 10), window, cx),
            reply: textarea("Reply to the task", (1, 6), window, cx),
            decision_answer: textarea("Write your answer", (2, 6), window, cx),
            edit: textarea("Message", (1, 10), window, cx),
            name: field("Name", window, cx),
            purpose: field("What is this channel for?", window, cx),
            notes: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(8, 16)
                    .placeholder("Write standing guidance for Consort in this channel")
            }),
            hub_url: field("https://hub.example.com", window, cx),
            search: field("Go to a channel or a task", window, cx),
            setup_code: field("ABCD-EFGH", window, cx),
            group_name: field("Name of the new group", window, cx),
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
            model_search: field("Search every account", window, cx),
        };
        let ext_inputs = crate::extensions::ExtInputs::new(window, cx);
        let mut subscriptions = Vec::new();
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
            &inputs.purpose,
            &inputs.group_name,
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
        // refusal came before the window opened.
        let page = if demo
            || (state.connection != Connection::NotPaired
                && !matches!(state.connection, Connection::Refused { .. }))
        {
            Page::Main
        } else {
            Page::Onboarding
        };
        let mut this = ConsortApp {
            client,
            demo,
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
            timeline: ListState::new(0, ListAlignment::Bottom, overdraw),
            timeline_keys: Vec::new(),
            timeline_channel: String::new(),
            thread: ListState::new(0, ListAlignment::Bottom, overdraw),
            thread_keys: Vec::new(),
            notices: Notices::default(),
            announced_failures: std::collections::HashSet::new(),
            new_channel: NewChannelForm::default(),
            channel_tags: Vec::new(),
            add_account: AddAccountForm::default(),
            tag_entry: None,
            tag_error: None,
            add_worker_os: Os::Linux,
            decision_writing: None,
            activity_open: Vec::new(),
            last_thread: None,
            editing: None,
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
            focus: cx.focus_handle(),
            styled_for: None,
            _subscriptions: subscriptions,
        };
        let ext_subscriptions = this.subscribe_ext_inputs(window, cx);
        this._subscriptions.extend(ext_subscriptions);
        this.sync_lists();
        this.update_placeholder(window, cx);
        this.sync_draft_inputs(window, cx);
        this.follow_connection(cx);
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
        for area in [
            &self.inputs.composer,
            &self.inputs.reply,
            &self.inputs.decision_answer,
            &self.inputs.notes,
            &self.inputs.edit,
        ] {
            let style = style.clone();
            area.update(cx, |s, cx| {
                s.set_editor_style(style);
                cx.notify();
            });
        }
        for field in [
            &self.inputs.name,
            &self.inputs.purpose,
            &self.inputs.hub_url,
            &self.inputs.setup_code,
            &self.inputs.search,
            &self.inputs.group_name,
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
        {
            let style = style.clone();
            field.update(cx, |s, cx| {
                s.set_editor_style(style);
                cx.notify();
            });
        }
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

    fn apply_state(&mut self, state: Arc<ViewState>, cx: &mut Context<Self>) {
        let was_paired = self.state.connection != Connection::NotPaired;
        self.state = state;
        if self.state.channel(&self.channel_id).is_none()
            && let Some(tutti) = self.state.tutti()
        {
            self.channel_id = tutti.id.clone();
        }
        if !self.demo && !was_paired && self.state.connection != Connection::NotPaired {
            self.page = Page::Main;
        }
        self.follow_connection(cx);
        if let Panel::Thread(task_id) = &self.panel
            && self.state.thread(task_id).is_none()
            && self.state.task(task_id).is_none()
        {
            self.panel = Panel::None;
        }
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
        cx.notify();
    }

    /// Shows the messages that waited in the outbox and that the hub then
    /// refused. Each one is an error notice with the text, and the notice has
    /// a button that puts the text back in the message box. The client keeps
    /// the failure until the user dismisses the notice, so a failure that
    /// the user has not seen is never lost. Errors queue behind each other.
    fn report_send_failures(&mut self, cx: &mut Context<Self>) {
        let fresh: Vec<_> = self
            .state
            .send_failures
            .iter()
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

    /// Dismisses the notice on screen. The next error, if one waits, shows.
    /// A failed send leaves the client with its notice.
    pub fn dismiss_notice(&mut self, cx: &mut Context<Self>) {
        if let Some(notice) = self.notices.dismiss()
            && let Some(id) = notice.failure
        {
            let client = self.client.clone();
            cx.spawn(async move |_, _| {
                let _ = client
                    .run(consort_client::Command::DismissSendFailure { id })
                    .await;
            })
            .detach();
        }
        cx.notify();
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
        self.dismiss_notice(cx);
    }

    /// Acts on the state of the link: a refused client goes to onboarding
    /// with the reason, and an offline link starts its retry countdown.
    fn follow_connection(&mut self, cx: &mut Context<Self>) {
        if let Connection::Refused { reason } = &self.state.connection {
            self.page = Page::Onboarding;
            self.onboarding.error = Some(reason.clone());
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
        let (rows, keys) = match self.state.timeline(&self.channel_id) {
            Some(t) => {
                let rows = consort_client::chat::rows(&t.items);
                let keys = rows.iter().map(|r| r.key(&t.items)).collect();
                (rows, keys)
            }
            None => (Vec::new(), Vec::new()),
        };
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
            Panel::Thread(task_id) => self
                .state
                .thread(task_id)
                .map(|t| t.posts.iter().map(|p| p.id.clone()).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        sync_list(&self.thread, &mut self.thread_keys, thread_keys, false);
    }

    /// The composer's placeholder: "Message <channel>", and "Message Tutti"
    /// in the main channel.
    pub fn composer_placeholder(&self) -> String {
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
                    self.open_dialog(
                        Dialog::ChannelExtensions(consort_client::mock_extensions::ACME.into()),
                        window,
                        cx,
                    );
                }
            }
            "hub-cards" => {
                self.open_channel(consort_client::mock_extensions::HIRING.into(), window, cx);
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
            // The Instructions dialog of sayso, with its purpose.
            "instructions" => {
                if let Some(id) = sayso.clone() {
                    self.open_channel(id.clone(), window, cx);
                    self.open_dialog(Dialog::Instructions(id), window, cx);
                }
            }
            "channel-tags" => {
                if let Some(id) = sayso.clone() {
                    self.open_channel(id.clone(), window, cx);
                    self.open_dialog(Dialog::ChannelTags(id), window, cx);
                }
            }
            "add-worker" | "worker-added" => {
                self.open_settings(SettingsSection::Workers, cx);
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
            "new-channel" => {
                self.open_dialog(Dialog::NewChannel, window, cx);
                self.set_field(Field::Name, "consort", window, cx);
                self.set_field(
                    Field::Purpose,
                    "Build Consort: the hub, the workers and the desktop app",
                    window,
                    cx,
                );
                self.new_channel.workers = self
                    .state
                    .workers
                    .iter()
                    .filter(|w| w.name == "MacBook Pro" || w.name == "homelab")
                    .rev()
                    .map(|w| w.id.clone())
                    .collect();
                self.new_channel.tags = vec!["personal".to_owned()];
            }
            "onboarding" => self.page = Page::Onboarding,
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
            _ => {}
        }
        cx.notify();
    }

    // Navigation

    pub fn open_channel(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.channel_id = id;
        self.page = Page::Main;
        // The composer holds the draft of this channel from now on.
        self.sync_draft_inputs(window, cx);
        self.sync_lists();
        self.update_placeholder(window, cx);
        self.track_unloaded(cx);
        self.mark_read(cx);
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

    /// Forgets the draft of a channel or a thread whose text went out while
    /// another one was open. The text is in the hub now.
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
    pub fn start_task_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.waiting().is_some() {
            return;
        }
        self.inputs.composer.update(cx, |input, cx| {
            let draft = input.value().to_string();
            input.set_value(drafts::start_task_text(&draft), window, cx);
            input.focus(window, cx);
        });
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
    fn submit_on_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            let confirm = self.dialog_confirm.borrow().clone();
            if let Some(confirm) = confirm {
                confirm(self, window, cx);
            }
        } else if self.page == Page::Onboarding && !self.onboarding.busy {
            self.connect(window, cx);
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
            || self
                .state
                .workers
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
                self.window_active,
                cx.reduce_motion(),
            )
        {
            return;
        }
        self.clock_ticker = true;
        let period = Self::tokens(cx).motion.frame;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(period).await;
                let keep = this.update(cx, |this, cx| {
                    // The last draw says if something moved.
                    let runs = crate::ui::motion::clock_runs(
                        cx.global::<FrameClock>().used(),
                        this.window_active,
                        cx.reduce_motion(),
                    );
                    if !runs {
                        this.clock_ticker = false;
                        return false;
                    }
                    cx.global::<FrameClock>().advance();
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
        cx.spawn(async move |this, cx| {
            let result = client.load_older(channel_id).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_older = None;
                if let Err(err) = result {
                    this.show_error(err.to_string(), cx);
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
        self.settings_section = section;
        if section == SettingsSection::Workers {
            self.load_registry_for_logos(cx);
        }
        if section == SettingsSection::Connectors {
            self.enter_connectors(cx);
        }
        if section == SettingsSection::Skills {
            self.enter_skills(cx);
        }
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
        let needed = self
            .state
            .workers
            .iter()
            .flat_map(|w| w.accounts.iter())
            .any(|a| {
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
                self.new_channel = NewChannelForm {
                    group_id: self.state.groups.first().map(|g| g.id.clone()),
                    new_group: false,
                    workers: Vec::new(),
                    coordinator_selection: None,
                    task_selection: None,
                    isolation: Isolation::Worktree,
                    tags: Vec::new(),
                };
                self.set_field(Field::Name, "", window, cx);
                self.set_field(Field::Purpose, "", window, cx);
                self.set_field(Field::GroupName, "", window, cx);
                focus_input(&self.inputs.name, window, cx);
            }
            Dialog::NewGroup(_) => {
                self.set_field(Field::Name, "", window, cx);
                focus_input(&self.inputs.name, window, cx);
            }
            Dialog::RenameChannel(id) => {
                let name = self
                    .state
                    .channel(id)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                self.set_field(Field::Name, &name, window, cx);
                focus_input(&self.inputs.name, window, cx);
            }
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
            Dialog::Instructions(id) => {
                // The purpose lives here since the channel header went away.
                let purpose = self
                    .state
                    .channel(id)
                    .map(|c| c.purpose.clone())
                    .unwrap_or_default();
                self.set_field(Field::Purpose, &purpose, window, cx);
                let notes = self
                    .state
                    .channel(id)
                    .map(|c| c.instructions.clone())
                    .unwrap_or_default();
                self.inputs
                    .notes
                    .update(cx, |input, cx| input.set_value(notes, window, cx));
                self.inputs
                    .notes
                    .update(cx, |input, cx| input.focus(window, cx));
            }
            Dialog::AddWorker => {
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
            Dialog::ChannelTags(id) => {
                self.channel_tags = self
                    .state
                    .channel(id)
                    .map(|c| c.tags.clone())
                    .unwrap_or_default();
            }
            Dialog::ConfirmDeleteChannel(_)
            | Dialog::ConfirmRemoveWorker(_)
            | Dialog::ConfirmRemoveAccount(..)
            | Dialog::ConfirmOpenLink(_)
            | Dialog::Connect
            | Dialog::ConfirmDisconnect(_)
            | Dialog::ConfirmRemoveSkill(_)
            | Dialog::SkillFiles(_)
            | Dialog::ChannelExtensions(_) => {}
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
                self.ext.extras_list_open = false;
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
            Field::Purpose => &self.inputs.purpose,
            Field::GroupName => &self.inputs.group_name,
            Field::HubUrl => &self.inputs.hub_url,
            Field::SetupCode => &self.inputs.setup_code,
        };
        let value = value.to_owned();
        input.update(cx, |input, cx| input.set_value(value, window, cx));
    }

    pub fn field(&self, field: Field, cx: &gpui_kit::App) -> String {
        let input = match field {
            Field::Name => &self.inputs.name,
            Field::Purpose => &self.inputs.purpose,
            Field::GroupName => &self.inputs.group_name,
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
    fn run_guarded<F>(
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
    fn clear_draft_if_sent(
        input: &Entity<TextareaState>,
        sent: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = input.read(cx).value().to_string();
        if drafts::can_clear_after_send(sent, &now) {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    pub fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // No channel has loaded: there is nowhere to send to.
        if self.waiting().is_some() {
            return;
        }
        let body = self.inputs.composer.read(cx).value().trim().to_owned();
        if body.is_empty() {
            return;
        }
        let client = self.client.clone();
        let channel_id = self.channel_id.clone();
        let sent_to = channel_id.clone();
        let input = self.inputs.composer.clone();
        let sent = body.clone();
        // The text stays in the box until the client takes the message.
        self.run_guarded(
            "composer".into(),
            async move { client.send_message(channel_id, body).await },
            Kept::Draft("Your message is still in the box."),
            move |this, window, cx| {
                if this.composer_for == sent_to {
                    Self::clear_draft_if_sent(&input, &sent, window, cx);
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
        let body = self.inputs.reply.read(cx).value().trim().to_owned();
        if body.is_empty() {
            return;
        }
        let client = self.client.clone();
        let input = self.inputs.reply.clone();
        let sent = body.clone();
        let sent_to = task_id.clone();
        self.run_guarded(
            "reply".into(),
            async move { client.reply_to_task(task_id, body).await },
            Kept::Draft("Your reply is still in the box."),
            move |this, window, cx| {
                if this.reply_for.as_deref() == Some(sent_to.as_str()) {
                    Self::clear_draft_if_sent(&input, &sent, window, cx);
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
                    Self::clear_draft_if_sent(&input, &sent, window, cx);
                }
            },
            window,
            cx,
        );
    }

    /// Adds a quote after the draft in a message box. The draft stays.
    fn quote_into(
        input: &Entity<TextareaState>,
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
            MenuAction::ReplyInThread(text) => {
                self.quote_in_reply(&text, window, cx);
                self.show_notice(
                    "Threads on messages come in a later build. The message is quoted in the composer.".into(),
                    cx,
                );
            }
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
            MenuAction::RenameChannel(id) => {
                self.open_dialog(Dialog::RenameChannel(id), window, cx)
            }
            MenuAction::ChannelInstructions(id) => {
                self.open_dialog(Dialog::Instructions(id), window, cx)
            }
            MenuAction::ChangeIcon(id) => {
                if let Some(anchor) = self.last_menu_anchor() {
                    let current = self
                        .state
                        .channel(&id)
                        .map(|c| c.icon.clone())
                        .unwrap_or_default();
                    let items = CHANNEL_ICONS
                        .iter()
                        .map(|(icon, label)| {
                            MenuItem::new(
                                *label,
                                MenuAction::SetIcon(id.clone(), (*icon).to_owned()),
                            )
                            .icon(icon)
                            .checked(current == *icon)
                        })
                        .collect();
                    self.open_menu(OpenMenu { items, ..anchor }, cx);
                }
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
            MenuAction::ChannelTags(id) => self.open_dialog(Dialog::ChannelTags(id), window, cx),
            MenuAction::ChannelModel(id) => {
                // The picker opens where the channel menu was.
                if let Some(anchor) = self.last_menu_anchor() {
                    self.open_picker(PickerTarget::Assistant(id), anchor.anchor, window, cx);
                }
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
            MenuAction::ChannelExtensions(id) => {
                self.open_dialog(Dialog::ChannelExtensions(id), window, cx)
            }
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
            MenuAction::ToggleScopeTag(target, tag) => {
                let use_in = crate::extensions::toggle_tag(&self.use_in_of(&target), &tag);
                self.set_use_in(target, use_in, cx);
            }
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
        let key = event.keystroke.key.as_str();
        if self.effort_key(key, window, cx) {
            cx.stop_propagation();
            return;
        }
        if let Some(menu) = self.menu.as_mut() {
            match key {
                "escape" => self.menu = None,
                "down" => menu.step(1),
                "up" => menu.step(-1),
                "enter" => {
                    if let Some(item) = menu.highlighted.and_then(|i| menu.items.get(i)) {
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
        if key == "escape" {
            if self.dialog.is_some() {
                self.close_dialog(window, cx);
                cx.stop_propagation();
            } else if self.tag_field_open() {
                // Escape closes the Add tag field of the worker, and only that.
                self.close_tag_entry(window, cx);
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
    Purpose,
    GroupName,
    HubUrl,
    SetupCode,
}

fn focus_input(input: &Entity<InputState>, window: &mut Window, cx: &mut Context<ConsortApp>) {
    input.update(cx, |input, cx| input.focus(window, cx));
}

/// The icons that a channel can have, with their names in the menu.
pub const CHANNEL_ICONS: [(&str, &str); 9] = [
    ("hash", "Hash"),
    ("mic", "Microphone"),
    ("shield", "Shield"),
    ("hard-drive", "Drive"),
    ("server", "Server"),
    ("lightbulb", "Light bulb"),
    ("book-open", "Book"),
    ("cloud", "Cloud"),
    ("terminal", "Terminal"),
];

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
        self.follow_mark(cx);
        self.dialog_confirm.replace(None);
        // A change of channel that came with a state (a deleted channel, a
        // channel that the hub just made) gives the composer its draft too.
        self.sync_draft_inputs(window, cx);
        // No channel yet: the composer takes no text.
        let waiting = self.waiting().is_some();
        self.inputs
            .composer
            .update(cx, |input, cx| input.set_disabled(waiting, cx));
        if self.placeholder_stale {
            self.placeholder_stale = false;
            self.update_placeholder(window, cx);
        }
        if self.ext_stale {
            self.ext_stale = false;
            self.follow_extensions(Some(window), cx);
        }
        let body = match self.page {
            Page::Onboarding => self.render_onboarding(&t, window, cx),
            Page::Settings => self.render_settings(&t, window, cx),
            Page::Main => self.render_main(&t, window, cx),
        };
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
                    .min_h_0()
                    .w_full()
                    .child(body)
                    .when_some(self.dialog.clone(), |layer, dialog| {
                        layer.child(self.render_dialog(&dialog, &t, window, cx))
                    })
                    .when_some(self.notices.current().cloned(), |layer, notice| {
                        layer.child(self.render_notice(notice, &t, cx))
                    })
                    .when(self.switcher.is_some(), |layer| {
                        layer.child(self.render_switcher(&t, cx))
                    }),
            )
            .children(picker)
            .children(picker_tip)
            .children(effort)
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
