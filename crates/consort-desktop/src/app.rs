//! The root view: app state, client subscription, actions and the layout of
//! the window.

use std::sync::Arc;

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
    px,
};

use crate::ui::{
    markdown::TextCache,
    menu::{MenuAction, MenuItem, OpenMenu, TriggerBounds, render_menu},
};

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
}

/// The fields of the New channel dialog that are not text inputs.
#[derive(Clone, Debug, Default)]
pub struct NewChannelForm {
    pub group_id: Option<String>,
    pub workers: Vec<String>,
    pub selection: Option<String>,
    pub isolation: Isolation,
    pub error: Option<String>,
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
    pub name: Entity<InputState>,
    pub purpose: Entity<InputState>,
    pub notes: Entity<TextareaState>,
    pub hub_url: Entity<InputState>,
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
    pub notice: Option<String>,
    pub new_channel: NewChannelForm,
    pub add_worker_os: Os,
    pub decision_writing: Option<String>,
    pub activity_open: Vec<String>,
    pub last_thread: Option<String>,
    pub onboarding: Onboarding,
    pub triggers: TriggerBounds,
    pub focus: FocusHandle,
    styled_for: Option<(gpui_kit::SharedString, consort_theme::Mode)>,
    _subscriptions: Vec<Subscription>,
}

const OVERDRAW: f32 = 400.0;

impl ConsortApp {
    pub fn new(client: Client, demo: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = client.state();
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
            name: field("Name", window, cx),
            purpose: field("What is this channel for?", window, cx),
            notes: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(8, 16)
                    .placeholder("Write standing guidance for Consort in this channel")
            }),
            hub_url: field("https://hub.example.com", window, cx),
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
        subscriptions.push(cx.observe_window_appearance(window, |_, window, cx| {
            consort_theme::apply(window.appearance(), cx);
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

        let page = if demo || state.connection != Connection::NotPaired {
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
            timeline: ListState::new(0, ListAlignment::Bottom, px(OVERDRAW)),
            timeline_keys: Vec::new(),
            timeline_channel: String::new(),
            thread: ListState::new(0, ListAlignment::Bottom, px(OVERDRAW)),
            thread_keys: Vec::new(),
            notice: None,
            new_channel: NewChannelForm::default(),
            add_worker_os: Os::Linux,
            decision_writing: None,
            activity_open: Vec::new(),
            last_thread: None,
            onboarding: Onboarding::default(),
            triggers: TriggerBounds::default(),
            focus: cx.focus_handle(),
            styled_for: None,
            _subscriptions: subscriptions,
        };
        this.sync_lists();
        this.update_placeholder(window, cx);
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

    fn set_state(&mut self, state: Arc<ViewState>, cx: &mut Context<Self>) {
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
        if let Connection::Refused { reason } = &self.state.connection {
            self.page = Page::Onboarding;
            self.onboarding.error = Some(reason.clone());
        }
        if let Panel::Thread(task_id) = &self.panel
            && self.state.thread(task_id).is_none()
            && self.state.task(task_id).is_none()
        {
            self.panel = Panel::None;
        }
        self.sync_lists();
        cx.notify();
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
            "add-worker" => {
                self.open_settings(SettingsSection::Workers, cx);
                // The window draws its first frame from this state, so the
                // code must be there before it opens.
                let _ = self
                    .client
                    .run_blocking(consort_client::Command::WorkerPairingCode);
                self.state = self.client.state();
                self.dialog = Some(Dialog::AddWorker);
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
            // A right click near the bottom of the window: the menu opens
            // upward, because there is no room below.
            "menu" => {
                let items = crate::ui::timeline::selection_menu("Two tasks need you".into());
                self.open_menu(
                    OpenMenu::at(
                        gpui_kit::point(gpui_kit::px(620.0), gpui_kit::px(840.0)),
                        items,
                    ),
                    cx,
                );
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
                    self.open_menu(
                        OpenMenu::at(
                            gpui_kit::point(gpui_kit::px(120.0), gpui_kit::px(170.0)),
                            items,
                        ),
                        cx,
                    );
                }
            }
            _ => {}
        }
        cx.notify();
    }

    // Navigation

    pub fn open_channel(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.channel_id = id.clone();
        self.page = Page::Main;
        self.sync_lists();
        self.update_placeholder(window, cx);
        let client = self.client.clone();
        cx.spawn(async move |_, _| {
            let _ = client
                .run(consort_client::Command::MarkRead { channel_id: id })
                .await;
        })
        .detach();
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
        self.panel = Panel::Thread(task_id);
        self.decision_writing = None;
        self.sync_lists();
        cx.notify();
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
        match &dialog {
            Dialog::NewChannel => {
                self.new_channel = NewChannelForm {
                    group_id: self.state.groups.first().map(|g| g.id.clone()),
                    workers: Vec::new(),
                    selection: self.state.selections.first().cloned(),
                    isolation: Isolation::Worktree,
                    error: None,
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
                let client = self.client.clone();
                self.spawn_command(cx, async move { client.worker_pairing_code().await });
            }
            Dialog::ConfirmDeleteChannel(_) | Dialog::ConfirmRemoveWorker(_) => {}
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
        }
        self.dialog = None;
        self.focus.focus(window, cx);
        cx.notify();
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
                let _ = this.update(cx, |this, cx| this.show_notice(err.to_string(), cx));
            }
        })
        .detach();
    }

    pub fn show_notice(&mut self, message: String, cx: &mut Context<Self>) {
        self.notice = Some(message);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(6))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.notice = None;
                cx.notify();
            });
        })
        .detach();
    }

    pub fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.inputs.composer.read(cx).value().trim().to_owned();
        if body.is_empty() {
            return;
        }
        self.inputs
            .composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        let client = self.client.clone();
        let channel_id = self.channel_id.clone();
        self.spawn_command(
            cx,
            async move { client.send_message(channel_id, body).await },
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
        self.inputs
            .reply
            .update(cx, |input, cx| input.set_value("", window, cx));
        let client = self.client.clone();
        self.spawn_command(cx, async move { client.reply_to_task(task_id, body).await });
    }

    pub fn answer_approval(&mut self, id: String, choice: ApprovalChoice, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(cx, async move { client.answer_approval(id, choice).await });
    }

    pub fn answer_decision(
        &mut self,
        id: String,
        option: Option<u32>,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.decision_writing = None;
        let client = self.client.clone();
        self.spawn_command(
            cx,
            async move { client.answer_decision(id, option, text).await },
        );
    }

    pub fn quote_in_reply(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let quoted: String = text.lines().map(|l| format!("> {l}\n")).collect::<String>() + "\n";
        let target = if matches!(self.panel, Panel::Thread(_)) {
            &self.inputs.reply
        } else {
            &self.inputs.composer
        };
        target.update(cx, |input, cx| {
            let current = input.value().to_string();
            input.set_value(format!("{current}{quoted}"), window, cx);
            input.focus(window, cx);
        });
    }

    pub fn ask_consort(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(everything) = self.state.everything().map(|c| c.id.clone()) {
            self.open_channel(everything, window, cx);
        }
        let quoted: String = text.lines().map(|l| format!("> {l}\n")).collect();
        self.inputs.composer.update(cx, |input, cx| {
            input.set_value(format!("{quoted}\n"), window, cx);
            input.focus(window, cx);
        });
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
            MenuAction::EditMessage(text) => {
                self.inputs.composer.update(cx, |input, cx| {
                    input.set_value(text, window, cx);
                    input.focus(window, cx);
                });
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
            MenuAction::ComposerCut => {
                self.composer_action(window, cx, Box::new(gpui_kit::component::input::Cut))
            }
            MenuAction::ComposerCopy => {
                self.composer_action(window, cx, Box::new(gpui_kit::component::input::Copy))
            }
            MenuAction::ComposerPaste => {
                self.composer_action(window, cx, Box::new(gpui_kit::component::input::Paste))
            }
            MenuAction::ComposerSelectAll => {
                self.composer_action(window, cx, Box::new(gpui_kit::component::input::SelectAll))
            }
            MenuAction::Select(id, index) => self.pick_select(id, index, window, cx),
        }
        cx.notify();
    }

    fn last_menu_anchor(&self) -> Option<OpenMenu> {
        // The submenu opens where the menu was. `run_menu` cleared the menu,
        // so the anchor is kept in `menu_anchor`.
        self.menu_anchor.clone()
    }

    fn composer_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: Box<dyn gpui_kit::Action>,
    ) {
        let handle = self.inputs.composer.read(cx).focus_handle(cx);
        handle.focus(window, cx);
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
            } else if self.panel != Panel::None {
                self.close_panel(cx);
                cx.stop_propagation();
            } else if self.page == Page::Settings {
                self.page = Page::Main;
                cx.notify();
                cx.stop_propagation();
            }
        }
        if key == "k" && event.keystroke.modifiers.platform {
            self.show_notice("Search comes in a later build.".into(), cx);
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

fn sync_list(list: &ListState, current: &mut Vec<String>, keys: Vec<String>, force_reset: bool) {
    if !force_reset && keys.len() >= current.len() && keys[..current.len()] == current[..] {
        let added = keys.len() - current.len();
        if added > 0 {
            let start = current.len();
            list.splice(start..start, added);
        } else {
            list.remeasure();
        }
    } else {
        list.reset(keys.len());
    }
    *current = keys;
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
            .bg(t.colors.surface_canvas)
            .text_color(t.colors.text_primary)
            .font_family(t.font_sans.clone())
            .type_style(t.text.ui)
            .child(body)
            .when_some(self.dialog.clone(), |root, dialog| {
                root.child(self.render_dialog(&dialog, &t, window, cx))
            })
            .when_some(self.notice.clone(), |root, notice| {
                root.child(self.render_notice(notice, &t, cx))
            })
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
