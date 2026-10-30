//! The app side of routines and memory: the state of the Routines and
//! Memory tabs, of the routine dialog and of the note editor, their text
//! inputs, their menu actions, and the commands that they send. The views
//! are in `ui/routines.rs`, `ui/schedule.rs` and `ui/memory.rs`.

use std::collections::{BTreeMap, BTreeSet};

use consort_client::{
    ClientError, Command, CommandOutput,
    memory::{GLOBAL, MemoryNoteView},
    routines::{IntervalUnit, RoutineForm, RoutineView},
};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, Window,
    component::input::{InputEvent, InputState, TextareaState},
};

use crate::app::{ChannelTab, ConsortApp, Dialog, Page, Panel, SettingsSection};
use crate::ui::menu::{MenuAction, MenuItem};

/// What a routine or memory menu item, or a select of their dialogs, does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgeAction {
    RunRoutine(String),
    RoutineHistory(String),
    EditRoutine(String),
    SetRoutineEnabled(String, bool),
    DeleteRoutine(String),
    /// A note: its scope and its path.
    NoteHistory(String, String),
    CopyNotePath(String),
    RenameNote(String, String),
    DeleteNote(String, String),
    RestoreNote(String, String),
    /// The channel of the routine dialog.
    PickChannel(String),
    /// The unit of the Interval preset.
    PickUnit(IntervalUnit),
    /// The channel filter of Settings > Routines. `None` is every channel.
    FilterChannel(Option<String>),
}

/// The status filter of Settings > Routines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusFilter {
    #[default]
    All,
    On,
    Off,
    Failed,
}

impl StatusFilter {
    pub const ALL: [StatusFilter; 4] = [
        StatusFilter::All,
        StatusFilter::On,
        StatusFilter::Off,
        StatusFilter::Failed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StatusFilter::All => "All",
            StatusFilter::On => "On",
            StatusFilter::Off => "Off",
            StatusFilter::Failed => "Last run failed",
        }
    }

    pub fn matches(self, routine: &RoutineView) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::On => routine.enabled,
            StatusFilter::Off => !routine.enabled,
            StatusFilter::Failed => {
                routine.last_run_status == Some(consort_client::routines::RoutineRunStatus::Error)
            }
        }
    }
}

/// The open routine dialog. The text of its fields is in
/// [`KnowledgeInputs`]; the rest is here.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutineDialog {
    /// `None` for a new routine.
    pub routine_id: Option<String>,
    pub form: RoutineForm,
    /// The time zone popover is open.
    pub zone_open: bool,
    /// The calendar of the date field is open on this month.
    pub calendar: Option<(i32, u32)>,
}

/// Where a note opens: in the Channel panel or in Settings > Memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Panel,
    Settings,
}

/// The note that the user edits.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteEditor {
    pub place: Place,
    pub scope: String,
    pub path: String,
    /// The hash that the edit started from. `None` for a new note.
    pub base_sha: Option<String>,
    pub base_version: u64,
    /// When the editor opened, for "opened at 20:38".
    pub opened_at: i64,
    /// The newer version that the hub had when a save found the note
    /// changed. Save stays off until the user chooses.
    pub conflict: Option<MemoryNoteView>,
    /// A save runs.
    pub saving: bool,
}

/// The open version history of a note.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryView {
    pub scope: String,
    pub path: String,
    /// Newest first.
    pub versions: Vec<MemoryNoteView>,
    /// The version that the preview shows, by index in `versions`.
    pub selected: usize,
    pub loading: bool,
    pub error: Option<String>,
}

/// The state of routines and memory in the app.
#[derive(Clone, Debug, Default)]
pub struct KnowledgeState {
    /// The routine that the Channel panel shows in place of the list.
    pub routine_open: Option<String>,
    pub routine_dialog: Option<RoutineDialog>,
    pub status_filter: StatusFilter,
    pub channel_filter: Option<String>,
    /// The note that the Channel panel shows in place of the tree: scope
    /// and path.
    pub panel_note: Option<(String, String)>,
    /// The note that Settings > Memory shows.
    pub settings_note: Option<(String, String)>,
    /// Settings > Memory shows Recently deleted.
    pub settings_deleted: bool,
    /// The closed folders of the memory trees, as `<scope>:<folder>`.
    pub closed: BTreeSet<String>,
    /// The notes that the user opened, with the version they saw.
    pub seen: BTreeMap<String, u64>,
    pub editor: Option<NoteEditor>,
    pub history: Option<HistoryView>,
    /// The scopes whose notes this app asked for.
    pub asked: BTreeSet<String>,
    /// The routines whose runs this app asked for.
    pub runs_asked: BTreeSet<String>,
    /// Counts the history loads, so an old answer is dropped.
    pub history_serial: u64,
    /// A routine row that shows its hover actions with no pointer on it,
    /// for the design screens.
    pub hover: Option<String>,
}

/// The text inputs of routines and memory.
pub struct KnowledgeInputs {
    pub name: Entity<InputState>,
    pub prompt: Entity<TextareaState>,
    pub title: Entity<InputState>,
    pub time: Entity<InputState>,
    pub every: Entity<InputState>,
    pub date: Entity<InputState>,
    pub cron: Entity<InputState>,
    pub zone_search: Entity<InputState>,
    /// The text filter of Settings > Routines.
    pub filter: Entity<InputState>,
    /// The path of a new note, or the new path of a rename.
    pub note_path: Entity<InputState>,
    /// The note editor.
    pub note: Entity<TextareaState>,
}

impl KnowledgeInputs {
    pub fn new(window: &mut Window, cx: &mut Context<ConsortApp>) -> KnowledgeInputs {
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<ConsortApp>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        KnowledgeInputs {
            name: field("Morning light check", window, cx),
            prompt: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 8)
                    .placeholder("Write what the routine must do")
            }),
            title: field("Task title", window, cx),
            time: field("8:00", window, cx),
            every: field("30", window, cx),
            date: field("Tue, Nov 3, 2026", window, cx),
            cron: field("0 8 * * 1-5", window, cx),
            zone_search: field("Search time zones", window, cx),
            filter: field("Filter by name or prompt", window, cx),
            note_path: field("people/kevin.md", window, cx),
            note: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(16, 40)
                    .placeholder("Write the note in Markdown")
            }),
        }
    }

    /// The one-line fields, for the editor style.
    pub fn fields(&self) -> [&Entity<InputState>; 9] {
        [
            &self.name,
            &self.title,
            &self.time,
            &self.every,
            &self.date,
            &self.cron,
            &self.zone_search,
            &self.filter,
            &self.note_path,
        ]
    }

    /// The multi-line fields, for the editor style.
    pub fn areas(&self) -> [&Entity<TextareaState>; 2] {
        [&self.prompt, &self.note]
    }

    /// Redraws on each change, so the dialog checks its fields as the user
    /// types.
    pub fn subscribe(
        &self,
        window: &mut Window,
        cx: &mut Context<ConsortApp>,
    ) -> Vec<gpui_kit::Subscription> {
        let mut out = Vec::new();
        for field in self.fields() {
            out.push(
                cx.subscribe_in(field, window, |_, _, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        cx.notify();
                    }
                }),
            );
        }
        for area in self.areas() {
            out.push(
                cx.subscribe_in(area, window, |_, _, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        cx.notify();
                    }
                }),
            );
        }
        out
    }
}

/// The menu of a routine row and of the routine page: Run now, Run
/// history, Edit, Turn off (Turn on when it is off), then Delete.
pub fn routine_menu(routine: &RoutineView) -> Vec<MenuItem> {
    let id = routine.id.clone();
    let k = |a: KnowledgeAction| MenuAction::Knowledge(a);
    // The hub sets the prompt of a check-in, so it has no Edit.
    let check_in = routine.kind == consort_client::proactivity::RoutineKind::CheckIn;
    let mut items = vec![
        MenuItem::new("Run now", k(KnowledgeAction::RunRoutine(id.clone()))),
        MenuItem::new(
            "Run history",
            k(KnowledgeAction::RoutineHistory(id.clone())),
        ),
        MenuItem::new("Edit", k(KnowledgeAction::EditRoutine(id.clone()))),
        MenuItem::new(
            if routine.enabled {
                "Turn off"
            } else {
                "Turn on"
            },
            k(KnowledgeAction::SetRoutineEnabled(
                id.clone(),
                !routine.enabled,
            )),
        ),
        MenuItem::new("Delete", k(KnowledgeAction::DeleteRoutine(id)))
            .danger()
            .separated(),
    ];
    if check_in {
        items.retain(|i| {
            !matches!(
                i.action,
                MenuAction::Knowledge(KnowledgeAction::EditRoutine(_))
            )
        });
    }
    items
}

/// The menu of a memory note: Version history, Copy path, Rename, then
/// Delete.
pub fn note_menu(scope: &str, path: &str) -> Vec<MenuItem> {
    let (s, p) = (scope.to_owned(), path.to_owned());
    let k = |a: KnowledgeAction| MenuAction::Knowledge(a);
    vec![
        MenuItem::new(
            "Version history",
            k(KnowledgeAction::NoteHistory(s.clone(), p.clone())),
        ),
        MenuItem::new("Copy path", k(KnowledgeAction::CopyNotePath(p.clone()))),
        MenuItem::new(
            "Rename",
            k(KnowledgeAction::RenameNote(s.clone(), p.clone())),
        ),
        MenuItem::new("Delete", k(KnowledgeAction::DeleteNote(s, p)))
            .danger()
            .separated(),
    ]
}

/// The `--screen` names of phase 2.
pub const SCREENS: [&str; 17] = [
    "routines",
    "routine-runs",
    "routine-new",
    "routine-edit",
    "schedule-picker",
    "schedule-interval",
    "schedule-once",
    "schedule-once-past",
    "schedule-cron",
    "settings-routines",
    "memory",
    "memory-conflict",
    "settings-memory",
    "memory-history",
    "memory-deleted",
    "routines-empty",
    "memory-empty",
];

impl ConsortApp {
    /// The memory scope of a channel: `global` for Tutti.
    pub fn memory_scope(&self, channel_id: &str) -> String {
        let tutti = self
            .state
            .channel(channel_id)
            .is_some_and(|c| c.kind == consort_client::ChannelKind::Tutti);
        consort_client::memory::scope_of(channel_id, tutti)
    }

    /// The name of a memory scope: "Global memory" or the channel name.
    pub fn scope_name(&self, scope: &str) -> String {
        if scope == GLOBAL {
            return "Global memory".to_owned();
        }
        self.state
            .channel(scope)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| scope.to_owned())
    }

    /// The routines of one channel.
    pub fn routines_of(&self, channel_id: &str) -> Vec<RoutineView> {
        self.state
            .routines
            .iter()
            .filter(|r| r.channel_id == channel_id)
            .cloned()
            .collect()
    }

    /// The notes of one scope that are not deleted.
    pub fn notes_of(&self, scope: &str) -> Vec<MemoryNoteView> {
        self.state
            .memory_notes
            .iter()
            .filter(|n| n.scope == scope && n.deleted_at.is_none())
            .cloned()
            .collect()
    }

    /// One note, also a deleted one.
    pub fn note(&self, scope: &str, path: &str) -> Option<MemoryNoteView> {
        self.state
            .memory_notes
            .iter()
            .find(|n| n.scope == scope && n.path == path)
            .cloned()
    }

    /// Loads the notes of a scope once, and its Recently deleted list.
    pub fn load_memory(&mut self, scope: &str, deleted: bool, cx: &mut Context<Self>) {
        let key = format!("{scope}:{deleted}");
        let loaded = if deleted {
            self.state.memory_deleted_loaded.iter().any(|s| s == scope)
        } else {
            self.state.memory_loaded.iter().any(|s| s == scope)
        };
        if scope.is_empty() || loaded || !self.knowledge.asked.insert(key.clone()) {
            return;
        }
        let client = self.client.clone();
        let scope = scope.to_owned();
        cx.spawn(async move |this, cx| {
            let result = client
                .run(Command::LoadMemory {
                    scope: scope.clone(),
                    deleted,
                })
                .await;
            if let Err(err) = result {
                let _ = this.update(cx, |this, cx| {
                    // A later visit asks again.
                    this.knowledge.asked.remove(&key);
                    this.show_error(err.to_string(), cx);
                });
            }
        })
        .detach();
    }

    /// Loads the runs of a routine once.
    pub fn load_runs(&mut self, routine_id: &str, cx: &mut Context<Self>) {
        let loaded = self
            .state
            .routines
            .iter()
            .any(|r| r.id == routine_id && r.runs_loaded);
        if loaded || !self.knowledge.runs_asked.insert(routine_id.to_owned()) {
            return;
        }
        let client = self.client.clone();
        let routine_id = routine_id.to_owned();
        self.spawn_command(cx, async move {
            client.run(Command::LoadRoutineRuns { routine_id }).await
        });
    }

    /// Opens the Channel panel on a tab of the open channel.
    pub fn open_channel_tab(&mut self, tab: ChannelTab, cx: &mut Context<Self>) {
        self.panel = Panel::Channel;
        self.channel_tab = tab;
        self.knowledge.routine_open = None;
        self.knowledge.panel_note = None;
        cx.notify();
    }

    /// Opens a routine in the Channel panel of its channel.
    pub fn open_routine(&mut self, routine_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = self
            .state
            .routines
            .iter()
            .find(|r| r.id == routine_id)
            .map(|r| r.channel_id.clone())
        else {
            return;
        };
        if self.channel_id != channel || self.page != Page::Main {
            self.open_channel(channel, window, cx);
        }
        self.panel = Panel::Channel;
        self.channel_tab = ChannelTab::Routines;
        self.knowledge.routine_open = Some(routine_id.to_owned());
        self.load_runs(routine_id, cx);
        cx.notify();
    }

    /// Opens a note in the Channel panel: the channel of its scope, or the
    /// open channel for global memory when it is Tutti.
    pub fn open_note_in_panel(
        &mut self,
        scope: &str,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel = if scope == GLOBAL {
            self.state.tutti().map(|c| c.id.clone())
        } else {
            Some(scope.to_owned())
        };
        let Some(channel) = channel else { return };
        if self.channel_id != channel || self.page != Page::Main {
            self.open_channel(channel, window, cx);
        }
        self.panel = Panel::Channel;
        self.channel_tab = ChannelTab::Memory;
        self.knowledge.panel_note = Some((scope.to_owned(), path.to_owned()));
        self.knowledge.editor = None;
        self.mark_seen(scope, path);
        self.load_memory(scope, false, cx);
        self.read_note(scope, path, cx);
        cx.notify();
    }

    /// Opens a note in Settings > Memory.
    pub fn open_note_in_settings(&mut self, scope: &str, path: &str, cx: &mut Context<Self>) {
        self.knowledge.settings_note = Some((scope.to_owned(), path.to_owned()));
        self.knowledge.settings_deleted = false;
        self.knowledge.history = None;
        if self
            .knowledge
            .editor
            .as_ref()
            .is_some_and(|e| e.place == Place::Settings)
        {
            self.knowledge.editor = None;
        }
        self.mark_seen(scope, path);
        self.read_note(scope, path, cx);
        cx.notify();
    }

    /// Remembers that the user saw a note's current version, so it is not
    /// "Just now" any more.
    pub fn mark_seen(&mut self, scope: &str, path: &str) {
        if let Some(note) = self.note(scope, path) {
            self.knowledge.seen.insert(note.key(), note.version);
        }
    }

    /// Reads a note's text when the cache holds only its summary.
    pub fn read_note(&mut self, scope: &str, path: &str, cx: &mut Context<Self>) {
        if self
            .note(scope, path)
            .is_some_and(|n| n.content.is_some() || n.deleted_at.is_some())
        {
            return;
        }
        let client = self.client.clone();
        let (scope, path) = (scope.to_owned(), path.to_owned());
        cx.spawn(async move |_, _| {
            let _ = client.run(Command::ReadMemoryNote { scope, path }).await;
        })
        .detach();
    }

    // The routine dialog.

    /// Opens the routine dialog: a new routine in `channel_id`, or an
    /// existing one.
    pub fn open_routine_dialog(
        &mut self,
        routine_id: Option<String>,
        channel_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = consort_client::format::now_ms();
        let form = match routine_id
            .as_ref()
            .and_then(|id| self.state.routines.iter().find(|r| &r.id == id))
        {
            Some(routine) => RoutineForm::of(routine, now),
            None => {
                let channel = if channel_id.is_empty() {
                    self.channel_id.clone()
                } else {
                    channel_id.to_owned()
                };
                RoutineForm::new(&channel, now)
            }
        };
        self.fill_routine_inputs(&form, window, cx);
        self.knowledge.routine_dialog = Some(RoutineDialog {
            routine_id: routine_id.clone(),
            form,
            zone_open: false,
            calendar: None,
        });
        self.open_dialog(Dialog::Routine(routine_id), window, cx);
    }

    fn fill_routine_inputs(&self, form: &RoutineForm, window: &mut Window, cx: &mut Context<Self>) {
        let k = &self.knowledge_inputs;
        let set = |input: &Entity<InputState>,
                   value: &str,
                   window: &mut Window,
                   cx: &mut Context<Self>| {
            let value = value.to_owned();
            input.update(cx, |i, cx| i.set_value(value, window, cx));
        };
        set(&k.name, &form.name, window, cx);
        set(&k.title, &form.title, window, cx);
        set(&k.time, &form.schedule.time, window, cx);
        set(&k.every, &form.schedule.every, window, cx);
        set(&k.date, &form.schedule.date, window, cx);
        set(&k.cron, &form.schedule.cron, window, cx);
        let prompt = form.prompt.clone();
        k.prompt.update(cx, |i, cx| i.set_value(prompt, window, cx));
    }

    /// The routine form with the text of its fields.
    pub fn routine_form(&self, cx: &gpui_kit::App) -> Option<RoutineForm> {
        let dialog = self.knowledge.routine_dialog.as_ref()?;
        let k = &self.knowledge_inputs;
        let value = |input: &Entity<InputState>| input.read(cx).value().to_string();
        let mut form = dialog.form.clone();
        form.name = value(&k.name);
        form.title = value(&k.title);
        form.prompt = k.prompt.read(cx).value().to_string();
        form.schedule.time = value(&k.time);
        form.schedule.every = value(&k.every);
        form.schedule.date = value(&k.date);
        form.schedule.cron = value(&k.cron);
        Some(form)
    }

    /// Changes the non-text part of the routine form.
    pub fn edit_routine_form(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut RoutineForm),
    ) {
        if let Some(dialog) = self.knowledge.routine_dialog.as_mut() {
            change(&mut dialog.form);
        }
        cx.notify();
    }

    /// Picks a preset. Switching to Cron puts the cron of the old preset in
    /// the cron field, so an expert can change it.
    pub fn pick_preset(
        &mut self,
        preset: consort_client::routines::Preset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut form) = self.routine_form(cx) else {
            return;
        };
        form.schedule.set_preset(preset);
        let cron = form.schedule.cron.clone();
        self.knowledge_inputs
            .cron
            .update(cx, |i, cx| i.set_value(cron, window, cx));
        if let Some(dialog) = self.knowledge.routine_dialog.as_mut() {
            dialog.form.schedule.preset = preset;
            dialog.zone_open = false;
            dialog.calendar = None;
        }
        cx.notify();
    }

    /// Saves the routine of the open dialog.
    pub fn save_routine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.routine_form(cx) else {
            return;
        };
        let now = consort_client::format::now_ms();
        if !form.errors(now).is_empty() {
            return;
        }
        let routine_id = self
            .knowledge
            .routine_dialog
            .as_ref()
            .and_then(|d| d.routine_id.clone());
        let client = self.client.clone();
        self.submit_dialog(
            async move { client.run(Command::SaveRoutine { routine_id, form }).await },
            window,
            cx,
        );
    }

    // The note editor.

    /// Opens the editor on a note, with its text.
    pub fn edit_note(
        &mut self,
        place: Place,
        scope: &str,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let note = self.note(scope, path);
        let content = note
            .as_ref()
            .and_then(|n| n.content.clone())
            .unwrap_or_default();
        self.knowledge.editor = Some(NoteEditor {
            place,
            scope: scope.to_owned(),
            path: path.to_owned(),
            base_sha: note.as_ref().map(|n| n.sha256.clone()),
            base_version: note.as_ref().map_or(0, |n| n.version),
            opened_at: consort_client::format::now_ms(),
            conflict: None,
            saving: false,
        });
        self.knowledge.history = None;
        self.knowledge_inputs.note.update(cx, |i, cx| {
            i.set_value(content, window, cx);
            i.focus(window, cx);
        });
        cx.notify();
    }

    /// Saves the editor's text. With `over` set, the save replaces the
    /// version that the conflict names.
    pub fn save_note(&mut self, over: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.knowledge.editor.clone() else {
            return;
        };
        if editor.saving || (editor.conflict.is_some() && !over) {
            return;
        }
        let expected = if over {
            editor.conflict.as_ref().map(|c| c.sha256.clone())
        } else {
            editor.base_sha.clone()
        };
        let content = self.knowledge_inputs.note.read(cx).value().to_string();
        if let Some(e) = self.knowledge.editor.as_mut() {
            e.saving = true;
        }
        let client = self.client.clone();
        let (scope, path) = (editor.scope.clone(), editor.path.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = client
                .run_for(Command::WriteMemoryNote {
                    scope: scope.clone(),
                    path: path.clone(),
                    content,
                    expected_sha256: expected,
                })
                .await;
            let stale = matches!(&result, Err(ClientError::Hub { code, .. }) if code == "stale_write");
            let latest = if stale {
                client
                    .run_for(Command::ReadMemoryNote {
                        scope: scope.clone(),
                        path: path.clone(),
                    })
                    .await
                    .ok()
            } else {
                None
            };
            let _ = this.update_in(cx, |this, _, cx| {
                if let Some(e) = this.knowledge.editor.as_mut() {
                    e.saving = false;
                }
                match result {
                    Ok(CommandOutput::MemoryNote(saved)) => {
                        this.knowledge.seen.insert(saved.key(), saved.version);
                        this.knowledge.editor = None;
                        match editor.place {
                            Place::Panel => {
                                this.knowledge.panel_note = Some((saved.scope, saved.path))
                            }
                            Place::Settings => {
                                this.knowledge.settings_note = Some((saved.scope, saved.path))
                            }
                        }
                    }
                    Ok(_) => this.knowledge.editor = None,
                    Err(_) if stale => {
                        let conflict = match latest {
                            Some(CommandOutput::MemoryNote(note)) => Some(note),
                            // The note is gone: a save makes it again.
                            _ => None,
                        };
                        if let Some(e) = this.knowledge.editor.as_mut() {
                            match conflict {
                                Some(note) => e.conflict = Some(note),
                                None => {
                                    e.base_sha = None;
                                    e.conflict = None;
                                    this.show_error(
                                        "The note was deleted since you opened it. Save again to make it with your text.".into(),
                                        cx,
                                    );
                                }
                            }
                        }
                    }
                    Err(err) => this.show_error(err.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Takes the newer version into the editor, in place of the user's text.
    pub fn load_new_version(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.knowledge.editor.as_mut() else {
            return;
        };
        let Some(latest) = editor.conflict.take() else {
            return;
        };
        editor.base_sha = Some(latest.sha256.clone());
        editor.base_version = latest.version;
        editor.opened_at = consort_client::format::now_ms();
        let content = latest.content.clone().unwrap_or_default();
        self.knowledge_inputs
            .note
            .update(cx, |i, cx| i.set_value(content, window, cx));
        cx.notify();
    }

    /// Closes the editor without saving.
    pub fn cancel_note_edit(&mut self, cx: &mut Context<Self>) {
        let new = self
            .knowledge
            .editor
            .as_ref()
            .is_some_and(|e| e.base_sha.is_none());
        if let Some(editor) = self.knowledge.editor.take()
            && new
        {
            // A new note that was never saved does not stay open.
            match editor.place {
                Place::Panel => self.knowledge.panel_note = None,
                Place::Settings => self.knowledge.settings_note = None,
            }
        }
        cx.notify();
    }

    /// Opens the version history of a note in Settings > Memory.
    pub fn open_history(&mut self, scope: &str, path: &str, cx: &mut Context<Self>) {
        self.page = Page::Settings;
        self.settings_section = SettingsSection::Memory;
        self.knowledge.settings_note = Some((scope.to_owned(), path.to_owned()));
        self.knowledge.settings_deleted = false;
        self.knowledge.editor = None;
        self.knowledge.history_serial += 1;
        let serial = self.knowledge.history_serial;
        self.knowledge.history = Some(HistoryView {
            scope: scope.to_owned(),
            path: path.to_owned(),
            loading: true,
            ..HistoryView::default()
        });
        let client = self.client.clone();
        let (s, p) = (scope.to_owned(), path.to_owned());
        cx.spawn(async move |this, cx| {
            let result = client
                .run_for(Command::LoadMemoryHistory { scope: s, path: p })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.knowledge.history_serial != serial {
                    return;
                }
                if let Some(history) = this.knowledge.history.as_mut() {
                    history.loading = false;
                    match result {
                        Ok(CommandOutput::MemoryHistory(versions)) => {
                            // The newest older version shows first.
                            history.selected = usize::from(versions.len() > 1);
                            history.versions = versions;
                        }
                        Ok(_) => {}
                        Err(err) => history.error = Some(err.to_string()),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Restores the version that the history shows: its text becomes the
    /// newest version.
    pub fn restore_version(&mut self, cx: &mut Context<Self>) {
        let Some(history) = self.knowledge.history.clone() else {
            return;
        };
        let Some(version) = history.versions.get(history.selected).cloned() else {
            return;
        };
        let current = self.note(&history.scope, &history.path);
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client
                .run_for(Command::WriteMemoryNote {
                    scope: history.scope.clone(),
                    path: history.path.clone(),
                    content: version.content.clone().unwrap_or_default(),
                    expected_sha256: current.map(|n| n.sha256),
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        this.knowledge.history = None;
                        this.show_notice(
                            format!(
                                "Restored version {} of {} as the newest version.",
                                version.version, history.path
                            ),
                            cx,
                        );
                    }
                    Err(ClientError::Hub { code, .. }) if code == "stale_write" => this
                        .show_error(
                            "The note changed while the history was open. Open the history again, then restore."
                                .into(),
                            cx,
                        ),
                    Err(err) => this.show_error(err.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // Menus.

    pub fn run_knowledge_menu(
        &mut self,
        action: KnowledgeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        match action {
            KnowledgeAction::RunRoutine(routine_id) => {
                let name = self
                    .state
                    .routines
                    .iter()
                    .find(|r| r.id == routine_id)
                    .map(|r| r.name.clone())
                    .unwrap_or_default();
                cx.spawn(async move |this, cx| {
                    let result = client.run(Command::RunRoutineNow { routine_id }).await;
                    let _ = this.update(cx, |this, cx| match result {
                        Ok(()) => this.show_notice(format!("{name} runs now."), cx),
                        Err(err) => this.show_error(err.to_string(), cx),
                    });
                })
                .detach();
            }
            KnowledgeAction::RoutineHistory(routine_id) => {
                self.open_routine(&routine_id, window, cx)
            }
            KnowledgeAction::EditRoutine(routine_id) => {
                self.open_routine_dialog(Some(routine_id), "", window, cx)
            }
            KnowledgeAction::SetRoutineEnabled(routine_id, enabled) => {
                self.spawn_command(cx, async move {
                    client
                        .run(Command::SetRoutineEnabled {
                            routine_id,
                            enabled,
                        })
                        .await
                })
            }
            KnowledgeAction::DeleteRoutine(routine_id) => {
                self.open_dialog(Dialog::ConfirmDeleteRoutine(routine_id), window, cx)
            }
            KnowledgeAction::NoteHistory(scope, path) => self.open_history(&scope, &path, cx),
            KnowledgeAction::CopyNotePath(path) => {
                cx.write_to_clipboard(ClipboardItem::new_string(path));
                self.show_notice("Copied the path.".into(), cx);
            }
            KnowledgeAction::RenameNote(scope, path) => {
                let value = path.clone();
                self.knowledge_inputs
                    .note_path
                    .update(cx, |i, cx| i.set_value(value, window, cx));
                self.open_dialog(Dialog::RenameNote(scope, path), window, cx);
            }
            KnowledgeAction::DeleteNote(scope, path) => {
                self.open_dialog(Dialog::ConfirmDeleteNote(scope, path), window, cx)
            }
            KnowledgeAction::RestoreNote(scope, path) => {
                cx.spawn(async move |this, cx| {
                    let result = client
                        .run(Command::RestoreMemoryNote {
                            scope: scope.clone(),
                            path: path.clone(),
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| match result {
                        Ok(()) => {
                            this.show_notice(format!("Restored {path}."), cx);
                            if this.page == Page::Settings {
                                this.open_note_in_settings(&scope, &path, cx);
                            }
                        }
                        Err(err) => this.show_error(err.to_string(), cx),
                    });
                })
                .detach();
            }
            KnowledgeAction::PickChannel(channel_id) => {
                self.edit_routine_form(cx, |form| form.channel_id = channel_id)
            }
            KnowledgeAction::PickUnit(unit) => {
                self.edit_routine_form(cx, |form| form.schedule.unit = unit)
            }
            KnowledgeAction::FilterChannel(channel) => {
                self.knowledge.channel_filter = channel;
            }
        }
        cx.notify();
    }

    /// Opens a screen of the phase 2 designs.
    pub fn open_knowledge_screen(
        &mut self,
        screen: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use consort_client::mock_routines::{LIGHTS, PRICE_WATCH, SUNSET};
        match screen {
            "routines" | "routine-runs" | "routine-new" | "routine-edit" | "schedule-picker"
            | "schedule-interval" | "schedule-once" | "schedule-once-past" | "schedule-cron" => {
                self.open_channel(LIGHTS.into(), window, cx);
                self.open_channel_tab(ChannelTab::Routines, cx);
                match screen {
                    "routines" => self.knowledge.hover = Some(PRICE_WATCH.into()),
                    "routine-runs" => self.open_routine(SUNSET, window, cx),
                    "routine-new" => {
                        self.open_routine_dialog(None, LIGHTS, window, cx);
                        let k = &self.knowledge_inputs;
                        k.name
                            .update(cx, |i, cx| i.set_value("Morning light check", window, cx));
                        k.prompt.update(cx, |i, cx| {
                            i.set_value(
                                "Check the weather for tonight. If rain is likely, turn off the roofline lights at 22:00 instead of midnight, and tell me in one line.",
                                window,
                                cx,
                            )
                        });
                        k.time.update(cx, |i, cx| i.set_value("8:00", window, cx));
                        self.edit_routine_form(cx, |form| {
                            form.schedule.preset = consort_client::routines::Preset::Weekdays;
                        });
                    }
                    "routine-edit" => {
                        self.open_routine_dialog(Some(PRICE_WATCH.into()), LIGHTS, window, cx);
                        self.edit_routine_form(cx, |form| {
                            form.schedule.preset = consort_client::routines::Preset::Cron;
                        });
                        self.knowledge_inputs
                            .cron
                            .update(cx, |i, cx| i.set_value("0 8 * * MON 1", window, cx));
                    }
                    _ => {
                        use consort_client::routines::Preset;
                        self.open_routine_dialog(None, LIGHTS, window, cx);
                        let k = crate::ui::routines::KnowledgeFields::of(self);
                        k.name.update(cx, |i, cx| {
                            i.set_value("Remind me to order the clips", window, cx)
                        });
                        k.prompt.update(cx, |i, cx| {
                            i.set_value("Remind me to order the gutter clips.", window, cx)
                        });
                        let now = consort_client::format::now_ms();
                        let day = 86_400_000;
                        let date = consort_client::routines::date_label_of;
                        match screen {
                            "schedule-interval" => {
                                self.edit_routine_form(cx, |f| {
                                    f.schedule.preset = Preset::Interval
                                });
                                k.every.update(cx, |i, cx| i.set_value("30", window, cx));
                            }
                            "schedule-once" | "schedule-once-past" => {
                                self.edit_routine_form(cx, |f| f.schedule.preset = Preset::Once);
                                let at = if screen == "schedule-once" {
                                    now + 4 * day
                                } else {
                                    now - day
                                };
                                let text = date(at);
                                k.date.update(cx, |i, cx| i.set_value(text, window, cx));
                                k.time.update(cx, |i, cx| i.set_value("9:00", window, cx));
                            }
                            "schedule-cron" => {
                                self.edit_routine_form(cx, |f| f.schedule.preset = Preset::Cron);
                                k.cron
                                    .update(cx, |i, cx| i.set_value("0 */2 * * 1-5", window, cx));
                            }
                            _ => self.edit_routine_form(cx, |form| {
                                form.schedule.preset = Preset::Weekly;
                                form.schedule.days =
                                    [true, false, false, true, false, false, false];
                            }),
                        }
                    }
                }
            }
            "routines-empty" | "memory-empty" => {
                if let Some(id) = self
                    .state
                    .channels
                    .iter()
                    .find(|c| c.name == "mac-ext4")
                    .map(|c| c.id.clone())
                {
                    self.open_channel(id, window, cx);
                }
                self.open_channel_tab(
                    if screen == "memory-empty" {
                        ChannelTab::Memory
                    } else {
                        ChannelTab::Routines
                    },
                    cx,
                );
            }
            "settings-routines" => {
                self.open_settings(SettingsSection::Routines, cx);
                self.knowledge.hover = Some(PRICE_WATCH.into());
            }
            "memory" | "memory-conflict" => {
                self.open_channel(LIGHTS.into(), window, cx);
                self.open_channel_tab(ChannelTab::Memory, cx);
                if screen == "memory-conflict" {
                    let path = "decisions/roofline.md";
                    self.open_note_in_panel(LIGHTS, path, window, cx);
                    self.edit_note(Place::Panel, LIGHTS, path, window, cx);
                    if let Some(editor) = self.knowledge.editor.as_mut() {
                        editor.opened_at -= 3 * 60_000;
                        editor.base_version = 4;
                        let mut newer = self
                            .state
                            .memory_notes
                            .iter()
                            .find(|n| n.scope == LIGHTS && n.path == path)
                            .cloned()
                            .unwrap_or_default();
                        newer.version = 5;
                        newer.updated_at = consort_client::format::now_ms();
                        newer.by_user = false;
                        editor.conflict = Some(newer);
                    }
                }
            }
            "settings-memory" | "memory-history" | "memory-deleted" => {
                self.open_settings(SettingsSection::Memory, cx);
                match screen {
                    "settings-memory" => {
                        self.open_note_in_settings(GLOBAL, "people/kevin.md", cx);
                        cx.on_next_frame(window, |this, window, cx| {
                            cx.on_next_frame(window, |this, _, cx| {
                                if let Some(bounds) = this.triggers.get("note-more") {
                                    let items = note_menu(GLOBAL, "people/kevin.md");
                                    this.open_menu(
                                        crate::ui::menu::OpenMenu::under(bounds, items, false),
                                        cx,
                                    );
                                }
                            });
                            let _ = this;
                        });
                    }
                    "memory-history" => self.open_history(GLOBAL, "people/kevin.md", cx),
                    _ => {
                        self.knowledge.settings_deleted = true;
                        self.knowledge.settings_note = None;
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }
}
