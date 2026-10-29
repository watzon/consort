//! The root view: app state, client subscription, actions and the layout of
//! the window.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use consort_client::{
    ApprovalChoice, ChannelChanges, Client, ClientError, Connection, Isolation, Os, ViewState,
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
    drafts::{self, InFlight},
    notice::{INFO_TIMEOUT, NoticeKind, Notices},
    read_marker::{self, ReadMarker},
    ui::{
        links::LinkAction,
        markdown::TextCache,
        menu::{EditOp, MenuAction, MenuItem, OpenMenu, TextTarget, TriggerBounds, render_menu},
    },
};

/// The key that guards the open dialog against a second submit.
const DIALOG_KEY: &str = "dialog";

/// How often the offline countdown redraws.
const COUNTDOWN_TICK: Duration = Duration::from_secs(1);

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
    EditLabels(String),
    ConfirmDeleteChannel(String),
    ConfirmRemoveWorker(String),
    /// A link whose words do not match its address: the address to show.
    ConfirmOpenLink(String),
}

/// The fields of the New channel dialog that are not text inputs.
#[derive(Clone, Debug, Default)]
pub struct NewChannelForm {
    pub group_id: Option<String>,
    pub workers: Vec<String>,
    pub selection: Option<String>,
    pub isolation: Isolation,
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
}

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
    /// The newest message that the app marked as read, by channel.
    read_marker: ReadMarker,
    /// The window has the focus.
    window_active: bool,
    /// The retry countdown, while the link is offline.
    pub offline: Option<RetryCountdown>,
    offline_ticker: bool,
    /// The open switcher and its selected row.
    pub switcher: Option<usize>,
    pub onboarding: Onboarding,
    pub triggers: TriggerBounds,
    pub focus: FocusHandle,
    styled_for: Option<(gpui_kit::SharedString, consort_theme::Mode)>,
    _subscriptions: Vec<Subscription>,
}

impl ConsortApp {
    pub fn new(client: Client, demo: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = client.state();
        let overdraw = Self::tokens(cx).layout.list_overdraw;
        let channel_id = state.everything().map(|c| c.id.clone()).unwrap_or_default();
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
        };
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
        }));
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
            read_marker: ReadMarker::default(),
            window_active: window.is_window_active(),
            offline: None,
            offline_ticker: false,
            switcher: None,
            onboarding: Onboarding::default(),
            triggers: TriggerBounds::default(),
            focus: cx.focus_handle(),
            styled_for: None,
            _subscriptions: subscriptions,
        };
        this.sync_lists();
        this.update_placeholder(window, cx);
        this.follow_connection(cx);
        this.mark_read(cx);
        // Reaching the top of the timeline loads older messages.
        let weak = cx.weak_entity();
        this.timeline.set_scroll_handler(move |event, _, cx| {
            if event.is_scrolled && event.visible_range.start == 0 {
                let weak = weak.clone();
                cx.defer(move |cx| {
                    let _ = weak.update(cx, |this, cx| this.load_older(cx));
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
        ] {
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
    fn set_state(&mut self, state: Arc<ViewState>, cx: &mut Context<Self>) {
        if Arc::ptr_eq(&state, &self.state) || *state == *self.state {
            return;
        }
        self.apply_state(state, cx);
    }

    fn apply_state(&mut self, state: Arc<ViewState>, cx: &mut Context<Self>) {
        let was_paired = self.state.connection != Connection::NotPaired;
        self.state = state;
        if self.state.channel(&self.channel_id).is_none()
            && let Some(everything) = self.state.everything()
        {
            self.channel_id = everything.id.clone();
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
        self.mark_read(cx);
        cx.notify();
    }

    /// Shows the messages that waited in the outbox and that the hub then
    /// refused. The notice holds the text, so the user can copy it. The
    /// client forgets the failure once the notice shows.
    fn report_send_failures(&mut self, cx: &mut Context<Self>) {
        let fresh: Vec<_> = self
            .state
            .send_failures
            .iter()
            .filter(|f| self.announced_failures.insert(f.id.clone()))
            .cloned()
            .collect();
        if fresh.is_empty() {
            return;
        }
        let text = fresh
            .iter()
            .map(|f| format!("{}\n{}", f.message, f.body))
            .collect::<Vec<_>>()
            .join("\n\n");
        self.show_error(text, cx);
        let client = self.client.clone();
        cx.spawn(async move |_, _| {
            for failure in fresh {
                let _ = client
                    .run(consort_client::Command::DismissSendFailure { id: failure.id })
                    .await;
            }
        })
        .detach();
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

    /// Marks the open channel as read when its newest message is one the app
    /// has not marked yet, the window has the focus and the link is up. A new
    /// message in the open channel clears its unread count; a state change
    /// that brings no new message sends nothing.
    pub fn mark_read(&mut self, cx: &mut Context<Self>) {
        if self.page != Page::Main
            || !self.window_active
            || !matches!(self.state.connection, Connection::Online | Connection::Demo)
        {
            return;
        }
        let newest = self
            .state
            .timeline(&self.channel_id)
            .and_then(read_marker::newest_message)
            .map(str::to_owned);
        if !self
            .read_marker
            .should_mark(&self.channel_id, newest.as_deref())
        {
            return;
        }
        let client = self.client.clone();
        let channel_id = self.channel_id.clone();
        cx.spawn(async move |this, cx| {
            let marked = client
                .run(consort_client::Command::MarkRead {
                    channel_id: channel_id.clone(),
                })
                .await;
            if marked.is_err() {
                // Try again at the next state change.
                let _ = this.update(cx, |this, _| this.read_marker.forget(&channel_id));
            }
        })
        .detach();
    }

    /// Keeps the virtual lists in step with the state: appended rows are
    /// spliced in, so the list keeps following new messages; other changes
    /// reset it.
    pub fn sync_lists(&mut self) {
        let keys: Vec<String> = self
            .state
            .timeline(&self.channel_id)
            .map(|t| t.items.iter().map(|i| i.key().to_owned()).collect())
            .unwrap_or_default();
        sync_list(
            &self.timeline,
            &mut self.timeline_keys,
            keys,
            self.timeline_channel != self.channel_id,
        );
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

    pub fn update_placeholder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .state
            .channel(&self.channel_id)
            .map(|c| match c.kind {
                consort_client::ChannelKind::Everything => "Consort".to_owned(),
                consort_client::ChannelKind::Standard => c.name.clone(),
            })
            .unwrap_or_else(|| "Consort".to_owned());
        let placeholder = format!("Message {name}");
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
                    self.open_thread(task, cx);
                }
            }
            // A task with no repo: the thread says it runs in a new folder.
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
                    self.open_thread(task, cx);
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
                    self.open_thread(task, cx);
                }
            }
            "settings" => self.open_settings(SettingsSection::Appearance, cx),
            "workers" => self.open_settings(SettingsSection::Workers, cx),
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
                    let items = vec![
                        MenuItem::new("Rename", MenuAction::RenameChannel(id.clone())),
                        MenuItem::new("Instructions", MenuAction::ChannelInstructions(id.clone())),
                        MenuItem::new("Change icon", MenuAction::ChangeIcon(id.clone())),
                        MenuItem::new("Move to group", MenuAction::MoveToGroup(id.clone())),
                        MenuItem::new("Archive", MenuAction::ArchiveChannel(id.clone()))
                            .separated(),
                        MenuItem::new("Delete", MenuAction::DeleteChannel(id)).danger(),
                    ];
                    let t = Self::tokens(cx);
                    let at = gpui_kit::point(t.layout.sidebar_width / 2.0, t.space.page * 3.5);
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
        self.sync_lists();
        self.update_placeholder(window, cx);
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

    pub fn open_thread(&mut self, task_id: String, cx: &mut Context<Self>) {
        self.panel = Panel::Thread(task_id.clone());
        self.decision_writing = None;
        self.sync_lists();
        cx.notify();
        // Replies come from the hub the first time a thread opens.
        let client = self.client.clone();
        self.spawn_command(cx, async move { client.load_thread(task_id).await });
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

    pub fn open_dialog(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.dialog_error = None;
        self.dialog_serial += 1;
        match &dialog {
            Dialog::NewChannel => {
                self.new_channel = NewChannelForm {
                    group_id: self.state.groups.first().map(|g| g.id.clone()),
                    workers: Vec::new(),
                    selection: self.state.selections.first().cloned(),
                    isolation: Isolation::Worktree,
                };
                self.set_field(Field::Name, "", window, cx);
                self.set_field(Field::Purpose, "", window, cx);
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
            Dialog::EditLabels(id) => {
                let labels = self
                    .state
                    .worker(id)
                    .map(|w| w.labels.join(", "))
                    .unwrap_or_default();
                self.set_field(Field::Name, &labels, window, cx);
                focus_input(&self.inputs.name, window, cx);
            }
            Dialog::Instructions(id) => {
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
            Dialog::ConfirmDeleteChannel(_)
            | Dialog::ConfirmRemoveWorker(_)
            | Dialog::ConfirmOpenLink(_) => {}
        }
        self.dialog = Some(dialog);
        cx.notify();
    }

    pub fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        kept: &'static str,
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
                    Err(err) => {
                        this.show_error(drafts::failure_message(&err.to_string(), kept), cx)
                    }
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
        let body = self.inputs.composer.read(cx).value().trim().to_owned();
        if body.is_empty() {
            return;
        }
        let client = self.client.clone();
        let channel_id = self.channel_id.clone();
        let input = self.inputs.composer.clone();
        let sent = body.clone();
        // The text stays in the box until the client takes the message.
        self.run_guarded(
            "composer".into(),
            async move { client.send_message(channel_id, body).await },
            "Your message is still in the box.",
            move |_, window, cx| Self::clear_draft_if_sent(&input, &sent, window, cx),
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
        self.run_guarded(
            "reply".into(),
            async move { client.reply_to_task(task_id, body).await },
            "Your reply is still in the box.",
            move |_, window, cx| Self::clear_draft_if_sent(&input, &sent, window, cx),
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
            "Your edit is still in the box.",
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
            "The request still waits for your answer.",
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
            "The decision still waits for your answer.",
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
        if let Some(everything) = self.state.everything().map(|c| c.id.clone()) {
            self.open_channel(everything, window, cx);
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
            MenuAction::OpenThread(task_id) => self.open_thread(task_id, cx),
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
                    let items = self
                        .state
                        .groups
                        .iter()
                        .map(|g| {
                            MenuItem::new(
                                g.name.clone(),
                                MenuAction::SetGroup(id.clone(), g.id.clone()),
                            )
                            .checked(current.as_deref() == Some(g.id.as_str()))
                        })
                        .collect();
                    self.open_menu(OpenMenu { items, ..anchor }, cx);
                }
            }
            MenuAction::SetGroup(id, group) => self.spawn_command(cx, async move {
                client
                    .update_channel(
                        id,
                        ChannelChanges {
                            group_id: Some(group),
                            ..ChannelChanges::default()
                        },
                    )
                    .await
            }),
            MenuAction::ArchiveChannel(id) => {
                self.spawn_command(cx, async move { client.archive_channel(id).await })
            }
            MenuAction::DeleteChannel(id) => {
                self.open_dialog(Dialog::ConfirmDeleteChannel(id), window, cx)
            }
            MenuAction::RenameWorker(id) => self.open_dialog(Dialog::RenameWorker(id), window, cx),
            MenuAction::EditLabels(id) => self.open_dialog(Dialog::EditLabels(id), window, cx),
            MenuAction::PauseWorker(worker_id, paused) => self.spawn_command(cx, async move {
                client
                    .run(consort_client::Command::SetWorkerPaused { worker_id, paused })
                    .await
            }),
            MenuAction::RemoveWorker(id) => {
                self.open_dialog(Dialog::ConfirmRemoveWorker(id), window, cx)
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

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
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
        if self.placeholder_stale {
            self.placeholder_stale = false;
            self.update_placeholder(window, cx);
        }
        let body = match self.page {
            Page::Onboarding => self.render_onboarding(&t, window, cx),
            Page::Settings => self.render_settings(&t, window, cx),
            Page::Main => self.render_main(&t, window, cx),
        };
        let menu = self.menu.clone();
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
