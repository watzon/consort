//! Phase 4 in the desktop app: threads on messages, reactions, search, the
//! New task dialog, the task item view, the channel settings, the simple New
//! channel dialog, groups, the task thread in bubbles and About.
//!
//! The state lives here, in [`ConvoState`] and [`ConvoInputs`]. The views
//! are in `ui/convo_*.rs`. The client side is `consort_client::convo`. The
//! decisions are in `docs/notes/phase4.md`.

use consort_client::{
    ApprovalMode, ChannelChanges, Isolation, SelectionView,
    convo::{NewTask, RepoChoice, RunsOn, SearchScope, StartWhen},
};
use gpui_kit::{
    AppContext as _, Context, Entity, Subscription, Window,
    component::input::{InputEvent, InputState, TextareaState},
};

use crate::{
    app::{ConsortApp, Dialog, Field, Panel},
    ui::menu::{MenuAnchor, MenuItem},
};

pub use crate::ui::convo_chat::render_reaction_bar;

gpui_kit::actions!(consort_convo, [ShowAbout, ScopeNext, ScopePrev]);

/// The key context of the search palette.
pub const SEARCH_CONTEXT: &str = "SearchPalette";

/// The keys of phase 4: Tab and Shift+Tab change the scope of the search.
/// GPUI matches key bindings before key listeners, and the base layer binds
/// Tab to focus traversal, so the palette needs bindings of its own.
pub fn bind_keys(cx: &mut gpui_kit::App) {
    cx.bind_keys([
        gpui_kit::KeyBinding::new("tab", ScopeNext, Some(SEARCH_CONTEXT)),
        gpui_kit::KeyBinding::new("shift-tab", ScopePrev, Some(SEARCH_CONTEXT)),
    ]);
}

/// The quick icons of the New channel dialog and the channel settings, in
/// the order of the design (`docs/notes/design-icon-picker.md`): software,
/// the web, a homelab, a house, notes and reading, design, and anything
/// else. An icon picked in All icons takes the last place.
pub const NEW_CHANNEL_ICONS: [&str; 7] = [
    "code",
    "globe",
    "server",
    "home",
    "memory",
    "appearance",
    "star",
];

/// The quick row for the icon that is picked now: the seven quick icons,
/// with a picked icon that is not one of them in the last place.
pub fn quick_icons(current: &str) -> [&'static str; 7] {
    let mut row = NEW_CHANNEL_ICONS;
    let current = crate::ui::icons::resolve(current);
    if !row.contains(&current)
        && let Some(name) = crate::ui::icon_catalog::flatten(&crate::ui::icon_catalog::search(""))
            .into_iter()
            .find(|n| *n == current)
    {
        row[6] = name;
    }
    row
}

/// What an icon from the All icons dialog is for.
#[derive(Clone, Debug, PartialEq)]
pub enum IconTarget {
    /// The open New channel dialog.
    NewChannel,
    /// The open channel settings dialog. Save changes keeps it.
    ChannelSettings,
    /// A channel, from Change icon in its menu. The pick saves at once.
    Channel(String),
}

/// The open All icons dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct IconPicker {
    pub target: IconTarget,
    /// The icon that the arrow keys moved to, as an index into the
    /// results. `None` while the search field has the focus.
    pub focus: Option<usize>,
}

/// How long the search waits after a key press before it asks the hub.
const SEARCH_DELAY: std::time::Duration = std::time::Duration::from_millis(220);

/// What a phase 4 menu item or control does.
#[derive(Clone, Debug, PartialEq)]
pub enum ConvoAction {
    /// Add or remove your reaction.
    React {
        message_id: String,
        emoji: String,
        remove: bool,
    },
    /// Open the emoji picker for a message.
    MoreEmoji(String),
    /// The New channel dialog, in a group or in none.
    NewChannel(Option<String>),
    NewGroup,
    RenameGroup(String),
    DeleteGroup(String),
    DeleteTask(String),
    RestoreTask(String),
    /// An icon for the New channel dialog or the channel settings, or for
    /// the target of the open All icons dialog.
    PickIcon(String),
    /// Open the All icons dialog.
    OpenIconPicker(IconTarget),
    /// The channel of the New task dialog.
    TaskChannel(String),
    /// "One worker" in the New task dialog.
    TaskWorker(String),
    /// The repo of the New task dialog. `None` is the channel's repos.
    TaskRepo(Option<String>),
    /// The day of a task that starts later, as the local midnight.
    TaskDay(i64),
    /// The time of a task that starts later, in minutes after midnight.
    TaskTime(u32),
    /// The default approval mode in the channel settings.
    SettingsMode(ApprovalMode),
    /// Attach a repo that the hub knows to the channel settings form.
    AttachRepo(String),
}

/// The quick emoji row at the top of a message menu.
#[derive(Clone, Debug, PartialEq)]
pub struct ReactionBar {
    pub message_id: String,
    /// The emoji that you chose on the message already.
    pub chosen: Vec<String>,
}

/// The New task dialog form (`ARL-0`).
#[derive(Clone, Debug, Default)]
pub struct NewTaskForm {
    pub channel_id: String,
    /// `None` is the channel's task model.
    pub selection: Option<SelectionView>,
    pub runs_on: RunsOn,
    pub repo_id: Option<String>,
    pub isolation: Isolation,
    pub approval_mode: ApprovalMode,
    /// Start at a time, not now.
    pub later: bool,
    pub day: i64,
    pub minutes: u32,
}

impl NewTaskForm {
    /// The task to make from the form and the two text fields.
    pub fn task(&self, title: String, brief: String) -> NewTask {
        NewTask {
            channel_id: self.channel_id.clone(),
            title,
            brief,
            selection: self.selection.clone(),
            runs_on: self.runs_on.clone(),
            repo_id: self.repo_id.clone(),
            isolation: Some(self.isolation),
            approval_mode: Some(self.approval_mode),
            start: if self.later {
                StartWhen::At(consort_client::convo::start_time(self.day, self.minutes))
            } else {
                StartWhen::Now
            },
        }
    }
}

/// The channel settings form (`CYJ-0`). The text fields are inputs.
#[derive(Clone, Debug, Default)]
pub struct SettingsForm {
    pub channel_id: String,
    pub icon: String,
    /// `None` is Tutti's model.
    pub selection: Option<SelectionView>,
    pub memory: bool,
    pub tags: Vec<String>,
    pub repos: Vec<RepoChoice>,
    /// The Add a repo field is open.
    pub adding_repo: bool,
    pub isolation: Isolation,
    pub approval_mode: ApprovalMode,
    pub servers: Vec<String>,
    /// The level of the channel's own. `None` is "Default for this channel".
    pub proactivity: Option<consort_client::proactivity::ProactivityLevel>,
}

/// The diff of one file of a new skill version, or why it did not load.
pub type SkillFileDiff = Result<Vec<consort_client::convo::DiffLine>, String>;

/// The phase 4 state of the window.
#[derive(Default)]
pub struct ConvoState {
    pub new_channel_icon: String,
    /// The group of the open New channel dialog.
    pub new_channel_group: Option<String>,
    pub new_task: NewTaskForm,
    pub settings: SettingsForm,
    /// The open emoji picker: the message and where it opens.
    pub emoji_picker: Option<(String, MenuAnchor)>,
    /// The All icons dialog, over the dialog that opened it.
    pub icon_picker: Option<IconPicker>,
    /// The scroll of the All icons list.
    pub icon_scroll: gpui_kit::ScrollHandle,
    pub search_scope: SearchScope,
    /// Counts the key presses in the search, so only the last one asks.
    pub search_serial: u64,
    /// The sidebar row under the pointer: `tutti` or a group ID.
    pub hover: Option<String>,
    /// The Tasks panel shows the archived tasks.
    pub archived_open: bool,
    /// The message thread whose draft the thread reply box holds.
    pub reply_for: Option<String>,
    /// The message that the timeline should show once, after a search.
    pub scroll_to: Option<String>,
    /// The group that is dragged now, for the drop line.
    pub dragging_group: Option<String>,
    /// The split lines of the open task item.
    pub item_cache: crate::ui::convo_item::ItemCache,
    /// The files of the open skill review: the path and its diff, once
    /// it loaded (or why it did not).
    pub skill_diff: Vec<(String, Option<SkillFileDiff>)>,
    /// The Permissions dialog: its draft, its rule dialog and its picker.
    pub permissions: crate::permissions::PermissionsState,
}

/// The text inputs of phase 4.
pub struct ConvoInputs {
    pub task_title: Entity<InputState>,
    pub task_brief: Entity<TextareaState>,
    pub settings_name: Entity<InputState>,
    pub settings_purpose: Entity<InputState>,
    pub settings_instructions: Entity<TextareaState>,
    pub repo_url: Entity<InputState>,
    pub thread_reply: Entity<TextareaState>,
    pub icon_search: Entity<InputState>,
    /// The fields of the rule dialog of the Permissions dialog.
    pub rule_pattern: Entity<InputState>,
    pub rule_except: Entity<InputState>,
    pub rule_description: Entity<InputState>,
    /// The search of the tool picker.
    pub exception_search: Entity<InputState>,
}

impl ConvoInputs {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<ConsortApp>,
    ) -> (ConvoInputs, Vec<Subscription>) {
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<ConsortApp>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let inputs = ConvoInputs {
            task_title: field("What should the task do?", window, cx),
            task_brief: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(4, 10)
                    .placeholder("Say what to do, and what done means")
            }),
            settings_name: field("Name", window, cx),
            settings_purpose: field("What is this channel for?", window, cx),
            settings_instructions: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(4, 10)
                    .placeholder("Standing guidance for the assistant in this channel")
            }),
            repo_url: field(
                "https://github.com/owner/repo or git@host:owner/repo",
                window,
                cx,
            ),
            thread_reply: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(1, 6)
                    .submit_on_enter(true)
                    .placeholder("Reply in thread")
            }),
            icon_search: field("Search icons", window, cx),
            rule_pattern: field("git push", window, cx),
            rule_except: field("--force, main", window, cx),
            rule_description: field("Push to a branch", window, cx),
            exception_search: field("Search tools", window, cx),
        };
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe_in(
            &inputs.thread_reply,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.send_thread_reply(window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &inputs.icon_search,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                // A new word starts at the top of the results.
                InputEvent::Change => {
                    if let Some(picker) = this.convo.icon_picker.as_mut() {
                        picker.focus = None;
                    }
                    cx.notify();
                }
                // Enter picks the first result.
                InputEvent::PressEnter { .. } => {
                    let query = this.convo_inputs.icon_search.read(cx).value().to_string();
                    let results = crate::ui::icon_catalog::search(&query);
                    if let Some(name) = crate::ui::icon_catalog::flatten(&results).first() {
                        this.run_convo(ConvoAction::PickIcon((*name).to_owned()), window, cx);
                    }
                }
                _ => {}
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &inputs.repo_url,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_repo_url(window, cx);
                }
            },
        ));
        // Enter in a field of the rule dialog adds the rule.
        for field in [
            &inputs.rule_pattern,
            &inputs.rule_except,
            &inputs.rule_description,
        ] {
            subscriptions.push(cx.subscribe_in(
                field,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.save_rule_dialog(window, cx);
                    }
                },
            ));
        }
        // Typing in the search of the tool picker filters its list.
        subscriptions.push(cx.subscribe_in(
            &inputs.exception_search,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.convo.permissions.picker_scroll = gpui_kit::ScrollHandle::new();
                    cx.notify();
                }
            },
        ));
        for field in [
            &inputs.task_title,
            &inputs.settings_name,
            &inputs.settings_purpose,
        ] {
            subscriptions.push(cx.subscribe_in(
                field,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        let confirm = this.dialog_confirm.borrow().clone();
                        if let Some(confirm) = confirm {
                            confirm(this, window, cx);
                        }
                    }
                },
            ));
        }
        (inputs, subscriptions)
    }

    /// Gives every input the editor colors of the theme.
    pub fn set_style(
        &self,
        style: &gpui_kit::base::input::InputEditorStyle,
        cx: &mut Context<ConsortApp>,
    ) {
        for field in [
            &self.task_title,
            &self.settings_name,
            &self.settings_purpose,
            &self.repo_url,
            &self.icon_search,
            &self.rule_pattern,
            &self.rule_except,
            &self.rule_description,
            &self.exception_search,
        ] {
            let style = style.clone();
            field.update(cx, |s, cx| {
                s.set_editor_style(style);
                cx.notify();
            });
        }
        for area in [
            &self.task_brief,
            &self.settings_instructions,
            &self.thread_reply,
        ] {
            let style = style.clone();
            area.update(cx, |s, cx| {
                s.set_editor_style(style);
                cx.notify();
            });
        }
    }
}

pub(crate) fn set_input(
    input: &Entity<InputState>,
    value: &str,
    window: &mut Window,
    cx: &mut Context<ConsortApp>,
) {
    input.update(cx, |input, cx| input.set_value(value, window, cx));
}

fn set_area(
    input: &Entity<TextareaState>,
    value: &str,
    window: &mut Window,
    cx: &mut Context<ConsortApp>,
) {
    input.update(cx, |input, cx| input.set_value(value, window, cx));
}

impl ConsortApp {
    /// Runs a phase 4 action.
    pub fn run_convo(&mut self, action: ConvoAction, window: &mut Window, cx: &mut Context<Self>) {
        let client = self.client.clone();
        match action {
            ConvoAction::React {
                message_id,
                emoji,
                remove,
            } => {
                self.convo.emoji_picker = None;
                self.spawn_command(cx, async move {
                    client
                        .run(consort_client::Command::React {
                            message_id,
                            emoji,
                            remove,
                        })
                        .await
                });
            }
            ConvoAction::MoreEmoji(message_id) => {
                if let Some(anchor) = self.menu_anchor.clone() {
                    self.convo.emoji_picker = Some((message_id, anchor.anchor));
                }
            }
            ConvoAction::NewChannel(group) => {
                self.convo.new_channel_group = group;
                self.open_dialog(Dialog::NewChannel, window, cx);
            }
            ConvoAction::NewGroup => self.open_dialog(Dialog::NewGroup(None), window, cx),
            ConvoAction::RenameGroup(id) => self.open_dialog(Dialog::RenameGroup(id), window, cx),
            ConvoAction::DeleteGroup(id) => {
                self.open_dialog(Dialog::ConfirmDeleteGroup(id), window, cx)
            }
            ConvoAction::DeleteTask(id) => {
                self.open_dialog(Dialog::ConfirmDeleteTask(id), window, cx)
            }
            ConvoAction::RestoreTask(task_id) => self.spawn_command(cx, async move {
                client
                    .run(consort_client::Command::RestoreTask { task_id })
                    .await
            }),
            ConvoAction::PickIcon(icon) => {
                let target = match self.convo.icon_picker.take() {
                    Some(picker) => {
                        self.after_icon_picker(window, cx);
                        picker.target
                    }
                    None if self.dialog == Some(Dialog::NewChannel) => IconTarget::NewChannel,
                    None => IconTarget::ChannelSettings,
                };
                match target {
                    IconTarget::NewChannel => self.convo.new_channel_icon = icon,
                    IconTarget::ChannelSettings => self.convo.settings.icon = icon,
                    IconTarget::Channel(id) => {
                        self.run_menu(crate::ui::menu::MenuAction::SetIcon(id, icon), window, cx)
                    }
                }
            }
            ConvoAction::OpenIconPicker(target) => self.open_icon_picker(target, window, cx),
            ConvoAction::TaskChannel(id) => self.set_task_channel(id),
            ConvoAction::TaskWorker(id) => self.convo.new_task.runs_on = RunsOn::Worker(id),
            ConvoAction::TaskRepo(repo) => self.convo.new_task.repo_id = repo,
            ConvoAction::TaskDay(day) => self.convo.new_task.day = day,
            ConvoAction::TaskTime(minutes) => self.convo.new_task.minutes = minutes,
            ConvoAction::SettingsMode(mode) => self.convo.settings.approval_mode = mode,
            ConvoAction::AttachRepo(id) => {
                let choice = RepoChoice::Existing(id);
                if !self.convo.settings.repos.contains(&choice) {
                    self.convo.settings.repos.push(choice);
                }
                self.convo.settings.adding_repo = false;
            }
        }
        cx.notify();
    }

    /// The New task form starts from a channel's defaults.
    fn set_task_channel(&mut self, id: String) {
        let channel = self.state.channel(&id).cloned();
        let form = &mut self.convo.new_task;
        form.channel_id = id;
        form.selection = None;
        form.runs_on = RunsOn::AnyTagged;
        form.repo_id = None;
        if let Some(channel) = channel {
            // The channel's task model, with its effort, as a start.
            form.selection = channel.task_selection.clone();
            form.isolation = channel.default_isolation;
            form.approval_mode = channel.default_approval_mode;
            form.repo_id = channel.repos.first().map(|r| r.id.clone());
        }
    }

    /// Fills the form of a phase 4 dialog as it opens.
    pub fn prepare_convo_dialog(
        &mut self,
        dialog: &Dialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match dialog {
            Dialog::NewTask => {
                let channel = self.channel_id.clone();
                self.set_task_channel(channel);
                let days = consort_client::convo::day_choices(consort_client::format::now_ms(), 2);
                let form = &mut self.convo.new_task;
                form.later = false;
                form.day = days
                    .get(1)
                    .or(days.first())
                    .map(|d| d.1)
                    .unwrap_or_default();
                form.minutes = 7 * 60;
                set_input(&self.convo_inputs.task_title, "", window, cx);
                set_area(&self.convo_inputs.task_brief, "", window, cx);
                let title = self.convo_inputs.task_title.clone();
                title.update(cx, |input, cx| input.focus(window, cx));
            }
            Dialog::ChannelSettings(id) => {
                let Some(channel) = self.state.channel(id).cloned() else {
                    return;
                };
                let own = channel
                    .coordinator_selection
                    .as_ref()
                    .filter(|e| e.source == consort_client::SelectionSource::Channel)
                    .map(|e| e.selection.clone());
                self.convo.settings = SettingsForm {
                    channel_id: channel.id.clone(),
                    icon: channel.icon.clone(),
                    selection: own,
                    memory: channel.memory_enabled,
                    tags: channel.tags.clone(),
                    repos: channel
                        .repos
                        .iter()
                        .map(|r| RepoChoice::Existing(r.id.clone()))
                        .collect(),
                    adding_repo: false,
                    isolation: channel.default_isolation,
                    approval_mode: channel.default_approval_mode,
                    servers: channel.allowed_local_mcp_servers.clone(),
                    proactivity: self.channel_own_level(&channel.id),
                };
                set_input(&self.convo_inputs.settings_name, &channel.name, window, cx);
                set_input(
                    &self.convo_inputs.settings_purpose,
                    &channel.purpose,
                    window,
                    cx,
                );
                set_area(
                    &self.convo_inputs.settings_instructions,
                    &channel.instructions,
                    window,
                    cx,
                );
                set_input(&self.convo_inputs.repo_url, "", window, cx);
            }
            Dialog::Permissions(id) => self.prepare_permissions_dialog(id),
            Dialog::SkillDiff(id) => self.load_skill_diff(id.clone(), cx),
            Dialog::About => {
                let client = self.client.clone();
                self.spawn_command(cx, async move {
                    client.run(consort_client::Command::LoadHubStatus).await
                });
                self.updates.check_now();
            }
            Dialog::RenameGroup(id) => {
                let name = self
                    .state
                    .groups
                    .iter()
                    .find(|g| &g.id == id)
                    .map(|g| g.name.clone())
                    .unwrap_or_default();
                self.set_field(Field::Name, &name, window, cx);
                let input = self.inputs.name.clone();
                input.update(cx, |input, cx| input.focus(window, cx));
            }
            _ => {}
        }
    }

    /// Loads both versions of each changed file of a skill's new version.
    fn load_skill_diff(&mut self, skill_id: String, cx: &mut Context<Self>) {
        use consort_client::{SkillUpdateView, skills::FileChangeKind};
        let Some(SkillUpdateView::Available { changes, .. }) =
            self.state.skill(&skill_id).map(|s| s.update.clone())
        else {
            self.convo.skill_diff.clear();
            return;
        };
        self.convo.skill_diff = changes.iter().map(|(p, _)| (p.clone(), None)).collect();
        for (path, kind) in changes {
            let client = self.client.clone();
            let skill = skill_id.clone();
            cx.spawn(async move |this, cx| {
                let read = |pending: bool| {
                    let client = client.clone();
                    let (skill, path) = (skill.clone(), path.clone());
                    async move {
                        match client
                            .run_for(consort_client::Command::ReadSkillFile {
                                skill_id: skill,
                                path,
                                pending,
                            })
                            .await
                        {
                            Ok(consort_client::CommandOutput::SkillFile(f)) if f.binary => {
                                Err("This file is not text, so it has no line diff.".to_owned())
                            }
                            Ok(consort_client::CommandOutput::SkillFile(f)) => Ok(f.content),
                            Ok(_) => Ok(None),
                            Err(err) => Err(err.to_string()),
                        }
                    }
                };
                let old = if kind == FileChangeKind::Added {
                    Ok(None)
                } else {
                    read(false).await
                };
                let new = if kind == FileChangeKind::Removed {
                    Ok(None)
                } else {
                    read(true).await
                };
                let result = match (old, new) {
                    (Ok(old), Ok(new)) => Ok(consort_client::convo::text_diff(
                        &path,
                        old.as_deref(),
                        new.as_deref(),
                    )),
                    (Err(e), _) | (_, Err(e)) => Err(e),
                };
                let _ = this.update(cx, |this, cx| {
                    if let Some(slot) = this.convo.skill_diff.iter_mut().find(|(p, _)| *p == path) {
                        slot.1 = Some(result);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// The proactivity level that a channel has of its own. `None` when it
    /// uses the default for its kind.
    pub fn channel_own_level(
        &self,
        channel_id: &str,
    ) -> Option<consort_client::proactivity::ProactivityLevel> {
        self.state
            .proactivity
            .channels
            .iter()
            .find(|c| c.channel_id == channel_id)
            .and_then(|c| c.level)
    }

    /// The values that the channel settings dialog sends with Save changes:
    /// only the fields that changed.
    pub fn settings_changes(&self, cx: &gpui_kit::App) -> Option<(String, ChannelChanges)> {
        let form = &self.convo.settings;
        let channel = self.state.channel(&form.channel_id)?;
        let name = self
            .convo_inputs
            .settings_name
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let purpose = self
            .convo_inputs
            .settings_purpose
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let instructions = self
            .convo_inputs
            .settings_instructions
            .read(cx)
            .value()
            .to_string();
        let own = channel
            .coordinator_selection
            .as_ref()
            .filter(|e| e.source == consort_client::SelectionSource::Channel)
            .map(|e| e.selection.clone());
        let repos_now: Vec<RepoChoice> = channel
            .repos
            .iter()
            .map(|r| RepoChoice::Existing(r.id.clone()))
            .collect();
        let changes = ChannelChanges {
            name: (name != channel.name).then_some(name),
            purpose: (purpose != channel.purpose).then_some(purpose),
            instructions: (instructions != channel.instructions).then_some(instructions),
            icon: (form.icon != channel.icon).then(|| form.icon.clone()),
            tags: (form.tags != channel.tags).then(|| form.tags.clone()),
            coordinator_selection: (form.selection != own).then(|| form.selection.clone()),
            default_isolation: (form.isolation != channel.default_isolation)
                .then_some(form.isolation),
            default_approval_mode: (form.approval_mode != channel.default_approval_mode)
                .then_some(form.approval_mode),
            allowed_local_mcp_servers: (form.servers != channel.allowed_local_mcp_servers)
                .then(|| form.servers.clone()),
            memory_enabled: (form.memory != channel.memory_enabled).then_some(form.memory),
            proactivity: (form.proactivity != self.channel_own_level(&form.channel_id))
                .then_some(form.proactivity),
            repos: (form.repos != repos_now).then(|| form.repos.clone()),
            ..ChannelChanges::default()
        };
        Some((form.channel_id.clone(), changes))
    }

    /// Adds the URL in the Add a repo field to the form.
    pub fn add_repo_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self
            .convo_inputs
            .repo_url
            .read(cx)
            .value()
            .trim()
            .to_owned();
        if url.is_empty() {
            return;
        }
        let choice = RepoChoice::New(url);
        if !self.convo.settings.repos.contains(&choice) {
            self.convo.settings.repos.push(choice);
        }
        self.convo.settings.adding_repo = false;
        set_input(&self.convo_inputs.repo_url, "", window, cx);
        cx.notify();
    }

    /// Opens the thread of a message in the right panel and loads its
    /// replies. A message that the client does not hold is found first.
    pub fn open_message_thread(
        &mut self,
        root_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.convo.emoji_picker = None;
        self.panel = Panel::MessageThread(root_id.clone());
        self.convo.reply_for = Some(root_id.clone());
        let reply = self.convo_inputs.thread_reply.clone();
        reply.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.timeline.remeasure();
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client
                .run(consort_client::Command::LoadMessageThread {
                    message_id: root_id,
                })
                .await
        });
        cx.notify();
    }

    /// Sends the text and the files of the thread reply box.
    pub fn send_thread_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Panel::MessageThread(root_id) = self.panel.clone() else {
            return;
        };
        let Some(channel_id) = self
            .state
            .convo
            .thread(&root_id)
            .map(|t| t.channel_id.clone())
        else {
            return;
        };
        let body = self
            .convo_inputs
            .thread_reply
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let target = consort_client::UploadTarget::Thread {
            root_id: root_id.clone(),
        };
        let attachments = self.upload_ids(&target);
        if body.is_empty() && attachments.is_empty() {
            return;
        }
        let client = self.client.clone();
        let input = self.convo_inputs.thread_reply.clone();
        let sent = body.clone();
        self.run_guarded(
            format!("thread-reply:{root_id}"),
            async move {
                client
                    .run(consort_client::Command::ReplyInThread {
                        channel_id,
                        root_id,
                        body,
                        client_message_id: consort_client::new_client_message_id(),
                        attachments,
                    })
                    .await
            },
            crate::drafts::Kept::Draft("Your reply is still in the box."),
            move |this, window, cx| this.clear_draft_if_sent(&input, &sent, window, cx),
            window,
            cx,
        );
    }

    /// The search text changed: the hub search starts after a short pause.
    pub fn search_changed(&mut self, cx: &mut Context<Self>) {
        self.convo.search_serial += 1;
        let serial = self.convo.search_serial;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.convo.search_serial == serial {
                    this.run_search(cx);
                }
            });
        })
        .detach();
    }

    /// Tab and Shift+Tab in the search palette: the next or the last scope.
    pub fn step_scope(&mut self, back: bool, cx: &mut Context<Self>) {
        self.convo.search_scope = self.convo.search_scope.next(back);
        self.switcher = Some(0);
        self.run_search(cx);
        cx.notify();
    }

    /// Asks the hub now, for the text and the scope of the palette.
    pub fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self.inputs.search.read(cx).value().trim().to_owned();
        let scope = self.convo.search_scope;
        let client = self.client.clone();
        cx.spawn(async move |_, _| {
            // A failed search shows in the palette, not as a notice.
            let _ = client
                .run(consort_client::Command::Search { query, scope })
                .await;
        })
        .detach();
    }

    /// Opens a task item in full, in the task thread panel.
    pub fn open_task_item(
        &mut self,
        task_id: String,
        session_id: String,
        item_id: String,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        cx.spawn(async move |_, _| {
            let _ = client
                .run(consort_client::Command::OpenTaskItem {
                    task_id,
                    session_id,
                    item_id,
                })
                .await;
        })
        .detach();
        cx.notify();
    }

    /// Closes the open task item: the panel goes back to the thread.
    pub fn close_task_item(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.run(consort_client::Command::CloseTaskItem).await
        });
    }

    /// Whether the open item belongs to this task.
    pub fn item_open_for(&self, task_id: &str) -> bool {
        self.state
            .convo
            .item
            .as_ref()
            .is_some_and(|i| i.task_id == task_id)
    }

    /// Keeps the phase 4 parts in step with a new view state.
    pub fn follow_convo(&mut self, cx: &mut Context<Self>) {
        if let Panel::MessageThread(root) = &self.panel
            && self.state.convo.thread(root).is_none()
            && !self
                .state
                .timelines
                .iter()
                .any(|t| t.items.iter().any(|i| i.key() == root.as_str()))
        {
            self.panel = Panel::None;
        }
        // A message that a search result named: scroll to it once it is
        // in the timeline of the open channel.
        if let Some(target) = self.convo.scroll_to.clone() {
            let row = self.chat_rows.iter().position(|r| {
                self.state
                    .timeline(&self.channel_id)
                    .map(|tl| r.key(&tl.items).starts_with(target.as_str()))
                    .unwrap_or(false)
            });
            if let Some(row) = row {
                self.convo.scroll_to = None;
                self.timeline.scroll_to_reveal_item(row);
            }
        }
        // The item view closes with its task.
        if let Some(item) = &self.state.convo.item
            && self.panel != Panel::Thread(item.task_id.clone())
        {
            self.close_task_item(cx);
        }
    }

    /// The reaction bar of a message menu.
    pub fn reaction_bar(&self, message_id: &str) -> ReactionBar {
        let chosen = self
            .find_message(message_id)
            .map(|m| {
                m.reactions
                    .iter()
                    .filter(|r| r.mine)
                    .map(|r| r.emoji.clone())
                    .collect()
            })
            .unwrap_or_default();
        ReactionBar {
            message_id: message_id.to_owned(),
            chosen,
        }
    }

    /// A message of a timeline or of a message thread.
    pub fn find_message(&self, id: &str) -> Option<consort_client::MessageView> {
        self.state
            .timelines
            .iter()
            .flat_map(|t| t.items.iter())
            .find_map(|i| match i {
                consort_client::TimelineItem::Message(m) if m.id == id => Some(m.clone()),
                _ => None,
            })
            .or_else(|| {
                self.state.convo.threads.iter().find_map(|t| {
                    std::iter::once(&t.root)
                        .chain(t.replies.iter())
                        .find(|m| m.id == id)
                        .cloned()
                })
            })
    }

    /// The menu of a group row: New channel in it, Rename, Delete group.
    pub fn group_menu_items(&self, group_id: &str) -> Vec<MenuItem> {
        use crate::ui::menu::MenuAction;
        let id = group_id.to_owned();
        vec![
            MenuItem::new(
                "New channel",
                MenuAction::Convo(ConvoAction::NewChannel(Some(id.clone()))),
            ),
            MenuItem::new(
                "Rename",
                MenuAction::Convo(ConvoAction::RenameGroup(id.clone())),
            ),
            MenuItem::new(
                "Delete group",
                MenuAction::Convo(ConvoAction::DeleteGroup(id)),
            )
            .danger()
            .separated(),
        ]
    }

    /// Moves the dragged group to the place of `before` (or to the end).
    pub fn drop_group(&mut self, dragged: &str, before: Option<&str>, cx: &mut Context<Self>) {
        self.convo.dragging_group = None;
        let ids = reorder(
            self.state.groups.iter().map(|g| g.id.clone()).collect(),
            dragged,
            before,
        );
        if ids
            == self
                .state
                .groups
                .iter()
                .map(|g| g.id.clone())
                .collect::<Vec<_>>()
        {
            cx.notify();
            return;
        }
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client
                .run(consort_client::Command::ReorderGroups { group_ids: ids })
                .await
        });
    }
}

impl ConsortApp {
    /// Runs a mock command now and takes the new state, for a screen.
    fn screen_command(&mut self, command: consort_client::Command) {
        let _ = self.client.run_blocking(command);
        self.state = self.client.state();
        self.sync_lists();
    }

    /// The channel of the mock with this name.
    fn screen_channel(&self, name: &str) -> Option<String> {
        self.state
            .channels
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.id.clone())
    }

    /// Opens a phase 4 screen by name, for `--screen`. Returns false for a
    /// name that is not one.
    pub fn open_convo_screen(
        &mut self,
        screen: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use consort_client::Command;
        let tutti = self.state.tutti().map(|c| c.id.clone()).unwrap_or_default();
        let sayso = self.screen_channel("sayso").unwrap_or_default();
        let sayso_task = self
            .state
            .tasks
            .iter()
            .find(|t| t.channel_id == sayso && t.status.is_your_turn())
            .map(|t| t.id.clone())
            .unwrap_or_default();
        match screen {
            // Paper 95W-0: the thread of a Tutti message.
            "message-thread" => {
                for emoji in ["🙏", "👍"] {
                    self.screen_command(Command::React {
                        message_id: "msg_tutti_reply".into(),
                        emoji: emoji.into(),
                        remove: true,
                    });
                }
                self.open_channel(tutti, window, cx);
                self.open_message_thread("msg_tutti_reply".into(), window, cx);
            }
            // Paper 9I9-0: reactions and the message menu.
            "reactions" => {
                self.open_channel(tutti, window, cx);
                cx.on_next_frame(window, |this, window, cx| {
                    cx.on_next_frame(window, |this, _, cx| {
                        let t = ConsortApp::tokens(cx);
                        if let Some(message) = this.find_message("msg_tutti_reply") {
                            let at = gpui_kit::point(
                                t.layout.sidebar_width + t.layout.timeline_max_width * 0.73,
                                t.layout.default_window.1 * 0.62,
                            );
                            let mut menu = crate::ui::menu::OpenMenu::at(
                                at,
                                crate::ui::timeline::message_menu(&message),
                            );
                            menu.reactions = Some(this.reaction_bar(&message.id));
                            this.open_menu(menu, cx);
                        }
                    });
                    let _ = this;
                });
            }
            // Paper A0N-0 and AFG-0: the search palette.
            "search" | "search-loading" | "search-empty" => {
                consort_client::mock_convo::hold_search(screen == "search-loading");
                self.open_switcher(window, cx);
                let query = if screen == "search-empty" {
                    "zfs snapshot"
                } else {
                    "windows engine"
                };
                self.inputs
                    .search
                    .update(cx, |input, cx| input.set_value(query, window, cx));
                if screen != "search-loading" {
                    self.screen_command(Command::Search {
                        query: query.into(),
                        scope: SearchScope::All,
                    });
                }
                self.run_search(cx);
            }
            // Paper ARL-0: New task, scheduled for tomorrow at 7:00 AM.
            "new-task" => {
                self.open_channel(sayso.clone(), window, cx);
                self.open_dialog(Dialog::NewTask, window, cx);
                set_input(
                    &self.convo_inputs.task_title,
                    "Fix the blank tray icon on Windows",
                    window,
                    cx,
                );
                set_area(
                    &self.convo_inputs.task_brief,
                    "The tray icon is a blank square on Windows 11 when display scaling is 150%. Find the cause and fix it. Test at 100%, 150% and 200%, then open a PR against main.",
                    window,
                    cx,
                );
                self.convo.new_task.later = true;
                self.convo.new_task.approval_mode = ApprovalMode::AutoEdits;
                self.convo.new_task.isolation = Isolation::Worktree;
            }
            // Paper CA5-0 and CPJ-0: an item in full.
            "item-output" | "item-diff" | "item-loading" | "item-offline" => {
                consort_client::mock_convo::hold_item(screen == "item-loading");
                let (task, item) = match screen {
                    "item-output" => (sayso_task.clone(), consort_client::mock_convo::ITEM_BUILD),
                    "item-diff" => (sayso_task.clone(), consort_client::mock_convo::ITEM_DIFF),
                    "item-loading" => (sayso_task.clone(), consort_client::mock_convo::ITEM_TEST),
                    _ => (
                        self.state
                            .tasks
                            .iter()
                            .find(|t| t.channel_name == "homelab")
                            .map(|t| t.id.clone())
                            .unwrap_or_default(),
                        consort_client::mock_convo::ITEM_OFFLINE,
                    ),
                };
                self.open_channel(tutti, window, cx);
                self.open_thread(task.clone(), window, cx);
                self.screen_command(Command::OpenTaskItem {
                    task_id: task,
                    session_id: consort_client::mock_convo::SESSION.into(),
                    item_id: item.into(),
                });
                if screen == "item-output" || screen == "item-diff" {
                    // The mock worker answers after a short delay.
                    std::thread::sleep(std::time::Duration::from_millis(450));
                    self.state = self.client.state();
                }
            }
            // Paper CYJ-0: the channel settings of sayso.
            "channel-settings" => {
                self.open_channel(sayso.clone(), window, cx);
                self.open_dialog(Dialog::ChannelSettings(sayso), window, cx);
            }
            // Paper BDH-0: the task thread in bubbles.
            "task-thread" => {
                self.open_channel(tutti, window, cx);
                self.open_thread(sayso_task.clone(), window, cx);
                self.activity_open.push("pst_sayso_1.0".into());
            }
            // Paper DWF-0: About, over Settings.
            "about" => {
                self.open_settings(crate::app::SettingsSection::Clients, cx);
                self.updates =
                    consort_client::update::Updates::fixed(crate::ui::update_notice::ready_view());
                self.open_dialog(Dialog::About, window, cx);
            }
            // Paper EYQ-0 and FWA-0: New channel, simple.
            "new-channel" | "new-channel-group" => {
                let group = if screen == "new-channel-group" {
                    self.screen_command(Command::CreateGroup {
                        name: "Learning".into(),
                        channel_id: None,
                    });
                    self.state
                        .groups
                        .iter()
                        .find(|g| g.name == "Learning")
                        .map(|g| g.id.clone())
                } else {
                    None
                };
                self.run_convo(ConvoAction::NewChannel(group), window, cx);
                self.convo.new_channel_icon = "memory".into();
                let name = if screen == "new-channel" {
                    "reading"
                } else {
                    "spanish"
                };
                self.set_field(Field::Name, name, window, cx);
            }
            // Paper IIJ-0 and IQF-0: All icons over New channel, and with
            // the search "arrow".
            "icon-picker" | "icon-picker-search" => {
                self.run_convo(ConvoAction::NewChannel(None), window, cx);
                self.convo.new_channel_icon = "memory".into();
                self.set_field(Field::Name, "reading", window, cx);
                self.open_icon_picker(IconTarget::NewChannel, window, cx);
                if screen == "icon-picker-search" {
                    let search = self.convo_inputs.icon_search.clone();
                    search.update(cx, |input, cx| input.set_value("arrow", window, cx));
                }
            }
            // Paper IAN-0: New channel after a pick in All icons.
            "icon-picked" => {
                self.run_convo(ConvoAction::NewChannel(None), window, cx);
                self.convo.new_channel_icon = "connector".into();
                self.set_field(Field::Name, "reading", window, cx);
            }
            "new-group" => {
                self.open_dialog(Dialog::NewGroup(None), window, cx);
                self.set_field(Field::Name, "Learning", window, cx);
            }
            "delete-group" => {
                if let Some(home) = self.state.groups.iter().find(|g| g.name == "Home") {
                    let id = home.id.clone();
                    self.open_dialog(Dialog::ConfirmDeleteGroup(id), window, cx);
                }
            }
            // Paper FOZ-0: the Tutti "+" with its menu, and an empty group.
            "sidebar-new" => {
                self.convo.hover = Some("tutti".into());
                cx.on_next_frame(window, |this, window, cx| {
                    cx.on_next_frame(window, |this, _, cx| {
                        if let Some(bounds) = this.triggers.get("tutti-plus") {
                            this.open_menu(
                                crate::ui::menu::OpenMenu::under(
                                    bounds,
                                    crate::ui::convo_sidebar::new_menu_items(),
                                    false,
                                ),
                                cx,
                            );
                        }
                    });
                    let _ = this;
                });
            }
            "sidebar-group" => {
                self.screen_command(Command::CreateGroup {
                    name: "Learning".into(),
                    channel_id: None,
                });
                self.convo.hover = self
                    .state
                    .groups
                    .iter()
                    .find(|g| g.name == "Learning")
                    .map(|g| g.id.clone());
            }
            "archived-tasks" => {
                self.panel = Panel::Tasks;
                self.tasks_tab = crate::app::TasksTab::Resolved;
                self.convo.archived_open = true;
                self.screen_command(Command::LoadArchivedTasks { more: false });
            }
            _ => return false,
        }
        cx.notify();
        true
    }
}

/// The group IDs after `dragged` moves before `before`, or to the end.
pub fn reorder(mut ids: Vec<String>, dragged: &str, before: Option<&str>) -> Vec<String> {
    let Some(from) = ids.iter().position(|id| id == dragged) else {
        return ids;
    };
    let moved = ids.remove(from);
    let at = before
        .and_then(|b| ids.iter().position(|id| id == b))
        .unwrap_or(ids.len());
    ids.insert(at, moved);
    ids
}

/// What a dragged group carries.
#[derive(Clone, Debug)]
pub struct GroupDrag {
    pub id: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::reorder;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_dragged_group_goes_before_the_target_or_to_the_end() {
        let start = ids(&["a", "b", "c"]);
        assert_eq!(
            reorder(start.clone(), "c", Some("a")),
            ids(&["c", "a", "b"])
        );
        assert_eq!(reorder(start.clone(), "a", None), ids(&["b", "c", "a"]));
        assert_eq!(
            reorder(start.clone(), "a", Some("c")),
            ids(&["b", "a", "c"])
        );
        assert_eq!(reorder(start.clone(), "x", Some("a")), start);
    }
}
