//! The model picker: one component for every place that chooses a model.
//!
//! - A trigger, as a compact chip in the composer or as a full-width select
//!   field in a dialog. It shows the provider logo, the model name and the
//!   account name.
//! - A popover with a rail at its left (Favorites, then one button per
//!   signed-in account) and a main pane: a search field, a header line, and
//!   the models of the view that the rail selects. Typing a search hides the
//!   rail and shows the matching models of every account. The list scrolls
//!   inside the popover with fades at its edges (AGENTS.md rule 3), and the
//!   popover flips up or down by the room it has (rule 4).
//! - The effort control: a chip (or a select in a dialog) with a small menu
//!   of the levels that the provider reports for the model.
//!
//! The data and the rules (rail, badges, Favorites, search, effort fallback)
//! are in `consort_client::picker`, so they have tests without a window.

use consort_client::{
    ChannelChanges, ChannelKind, EffortView, ModelGroupView, SelectionSource, SelectionView,
    picker::{self, EffortRow, PickerView, RowRef},
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement,
    Pixels, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled, Window, anchored,
    deferred, div, linear_color_stop, linear_gradient, point, prelude::FluentBuilder as _,
    transparent_black,
};

use crate::{
    app::{ConsortApp, SettingsSection},
    ui::{
        menu::{MenuAnchor, floating},
        parts::{ButtonKind, button, detail, focusable, icon, line, provider_logo, text},
    },
};

/// The selection that a picker changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerTarget {
    /// The assistant (the coordinator) of a channel, by channel ID. A
    /// choice sends `channel.update` at once.
    Assistant(String),
    /// The model of a "Start a task" routine, in the routine dialog.
    RoutineTask,
    /// "Model and effort" in the channel settings dialog. A choice changes
    /// only the form; Save changes sends it.
    SettingsAssistant,
    /// "Model and effort" in the New task dialog.
    NewTask,
    /// The model of a task that has started, by task ID, in the task
    /// thread. A choice calls `Client::set_task_model`. The picker offers
    /// only the models of the worker that holds the task.
    Task(String),
    /// A new step of the fallback path, in Settings > Models. A choice adds
    /// the step. A model that the path has shows "Step N" and cannot be
    /// chosen again.
    FallbackStep,
    /// The model of the command check, in Settings > Models. A choice saves
    /// at once. "Choose for me" gives the choice back to Consort.
    CommandCheck,
}

/// The part of the picker that the arrow keys move in. The search field
/// keeps the text focus in both, so typing always searches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerFocus {
    /// Up and Down move between Favorites and the accounts. `ring` is true
    /// when the keyboard moved here, so the selected button shows the focus
    /// ring. A click on the rail does not show it.
    Rail { ring: bool },
    /// Up and Down move between the model rows.
    List,
}

/// The open model picker.
#[derive(Clone, Debug)]
pub struct OpenPicker {
    pub target: PickerTarget,
    pub anchor: MenuAnchor,
    /// What the rail selects: Favorites or an account. A search replaces it
    /// while the query has a word.
    pub view: PickerView,
    pub focus: PickerFocus,
    /// The model row that the keyboard is on, as an index into the rows
    /// that the main pane shows.
    pub highlighted: Option<usize>,
    pub scroll: ScrollHandle,
    /// The rail, which scrolls when it has more accounts than room.
    pub rail_scroll: ScrollHandle,
    /// The rail button under the pointer, as an index into the rail, for
    /// its tooltip.
    pub rail_hover: Option<usize>,
    /// The list row to scroll to once the list knows the bounds of its
    /// rows, which is after its first frame.
    pub pending_row: Option<usize>,
}

/// The open effort menu.
#[derive(Clone, Debug)]
pub struct OpenEffort {
    pub target: PickerTarget,
    pub anchor: MenuAnchor,
    /// The row that the keyboard is on.
    pub highlighted: Option<usize>,
}

/// How a trigger looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerStyle {
    /// A compact chip, for the composer.
    Chip,
    /// The chip with no account name, for a composer with little room.
    ChipShort,
    /// A full-width select field, for a dialog.
    Field,
}

/// The index among `rows` of a selection's model, if the rows show it.
fn row_of(groups: &[ModelGroupView], rows: &[RowRef], selection: &SelectionView) -> Option<usize> {
    rows.iter().position(|r| {
        groups[r.group].account_label == selection.account_label
            && groups[r.group].models[r.model].id == selection.model_id
    })
}

/// The child index in the rail of rail item `index`: Favorites is the first
/// child, then the divider, then the accounts.
fn rail_child(index: usize) -> usize {
    if index == 0 { 0 } else { index + 1 }
}

/// Moves a highlight by `delta` rows in a list of `len` rows, and wraps.
pub fn step(highlighted: Option<usize>, delta: isize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let len = len as isize;
    let next = match highlighted {
        None if delta > 0 => 0,
        None => len - 1,
        Some(i) => (i as isize + delta).rem_euclid(len),
    };
    Some(next as usize)
}

/// The items and the header of the chip menu on a fallback.
pub fn fallback_menu(
    fallback: &consort_client::ChannelFallbackView,
    channel_id: &str,
) -> (Vec<crate::ui::menu::MenuItem>, crate::ui::menu::MenuHeader) {
    use crate::{
        fallback::FallbackAction,
        ui::menu::{MenuAction, MenuHeader, MenuItem},
    };
    use consort_client::FallbackCauseView;
    let from = &fallback.from;
    let why = match fallback.cause {
        FallbackCauseView::UsageLimit => format!("{} reached its usage limit.", from.account_label),
        FallbackCauseView::Outage => format!("{} did not answer.", from.account_label),
    };
    let body = format!(
        "{why} The assistant uses {}{}.",
        fallback.to.account_label,
        fallback
            .available_label
            .as_deref()
            .and_then(|l| l.strip_prefix("Available at "))
            .map(|at| format!(" until {at}"))
            .unwrap_or_default()
    );
    let mut go_back = MenuItem::new(
        format!("Go back to {}", from.model_name),
        MenuAction::Fallback(FallbackAction::GoBack(channel_id.to_owned())),
    );
    if let Some(label) = &fallback.available_label {
        go_back = go_back.disabled(true).trailing(label.clone());
    }
    let items = vec![
        go_back,
        MenuItem::new(
            "Choose another model",
            MenuAction::Fallback(FallbackAction::ChooseModel(channel_id.to_owned())),
        ),
        MenuItem::new("Edit path", MenuAction::Fallback(FallbackAction::EditPath)),
    ];
    (
        items,
        MenuHeader {
            title: "On a fallback".into(),
            body: body.into(),
            marker: true,
        },
    )
}

/// The note under the effort levels.
pub fn effort_note(provider: &str) -> String {
    format!(
        "{provider} offers these levels for this model. More effort thinks longer and uses more of your plan."
    )
}

impl ConsortApp {
    /// The selection that a target has now: for a channel, the selection
    /// that its assistant uses, its own or Tutti's.
    pub fn picker_selection(&self, target: &PickerTarget) -> Option<SelectionView> {
        match target {
            PickerTarget::Assistant(id) => self
                .state
                .channel(id)
                .and_then(|c| c.coordinator_selection.as_ref())
                .map(|e| e.selection.clone()),
            PickerTarget::RoutineTask => self
                .knowledge
                .routine_dialog
                .as_ref()
                .and_then(|d| d.form.selection.clone()),
            PickerTarget::SettingsAssistant => self.convo.settings.selection.clone(),
            PickerTarget::NewTask => self.convo.new_task.selection.clone(),
            PickerTarget::Task(id) => self.state.thread(id).and_then(|th| th.selection.clone()),
            PickerTarget::FallbackStep => None,
            PickerTarget::CommandCheck => self.state.command_check.account_model().cloned(),
        }
    }

    /// The groups that the picker of a target offers. A task offers only
    /// the models of its worker, because the hub refuses any other.
    pub fn picker_groups(&self, target: &PickerTarget) -> Vec<ModelGroupView> {
        let PickerTarget::Task(id) = target else {
            return self.state.model_groups.clone();
        };
        let options = self
            .state
            .thread(id)
            .map(|th| th.model_options.as_slice())
            .unwrap_or_default();
        self.state
            .model_groups
            .iter()
            .filter_map(|group| {
                let models: Vec<_> = group
                    .models
                    .iter()
                    .filter(|m| {
                        options
                            .iter()
                            .any(|o| o.account_label == group.account_label && o.model_id == m.id)
                    })
                    .cloned()
                    .collect();
                (!models.is_empty()).then(|| ModelGroupView {
                    models,
                    ..group.clone()
                })
            })
            .collect()
    }

    /// The note that a pick of a task shows under its row, such as "This
    /// starts a new session in the same folder."
    pub(crate) fn task_note(
        &self,
        target: &PickerTarget,
        account: &str,
        model_id: &str,
    ) -> Option<String> {
        if *target == PickerTarget::FallbackStep {
            return self
                .state
                .fallback_path
                .step_number(account, model_id)
                .map(|n| format!("Step {n}"));
        }
        let PickerTarget::Task(id) = target else {
            return None;
        };
        self.state
            .thread(id)?
            .model_options
            .iter()
            .find(|o| o.account_label == account && o.model_id == model_id)?
            .note
            .clone()
    }

    /// Whether the chips of a target show but do not open. A task that the
    /// hub will not change (it is Resolved) or that has no model list yet
    /// is locked.
    pub fn picker_locked(&self, target: &PickerTarget) -> bool {
        match target {
            PickerTarget::Task(id) => self
                .state
                .thread(id)
                .is_none_or(|th| !th.can_change_model || th.model_options.is_empty()),
            _ => false,
        }
    }

    /// Whether the target has a model of its own that it can give up for
    /// the Tutti model.
    fn picker_can_inherit(&self, target: &PickerTarget) -> bool {
        match target {
            PickerTarget::Assistant(id) => self.state.channel(id).is_some_and(|c| {
                c.kind != ChannelKind::Tutti
                    && c.coordinator_selection
                        .as_ref()
                        .is_some_and(|e| e.source == SelectionSource::Channel)
            }),
            PickerTarget::RoutineTask => self
                .knowledge
                .routine_dialog
                .as_ref()
                .is_some_and(|d| d.form.selection.is_some()),
            PickerTarget::SettingsAssistant => self.convo.settings.selection.is_some(),
            PickerTarget::NewTask => self.convo.new_task.selection.is_some(),
            // A task always has a model of its own.
            PickerTarget::Task(_) => false,
            PickerTarget::FallbackStep => false,
            PickerTarget::CommandCheck => self.state.command_check.choice.account_model.is_some(),
        }
    }

    /// Sets the selection of a target. For a channel's assistant, this
    /// sends `channel.update` at once. `None` gives the channel the
    /// Tutti model again.
    pub fn apply_selection(
        &mut self,
        target: &PickerTarget,
        selection: Option<SelectionView>,
        cx: &mut Context<Self>,
    ) {
        match target {
            PickerTarget::Assistant(id) => {
                let client = self.client.clone();
                let id = id.clone();
                self.spawn_command(cx, async move {
                    client
                        .update_channel(
                            id,
                            ChannelChanges {
                                coordinator_selection: Some(selection),
                                ..ChannelChanges::default()
                            },
                        )
                        .await
                });
            }
            PickerTarget::RoutineTask => {
                if let Some(dialog) = self.knowledge.routine_dialog.as_mut() {
                    dialog.form.selection = selection;
                }
            }
            PickerTarget::SettingsAssistant => self.convo.settings.selection = selection,
            PickerTarget::NewTask => self.convo.new_task.selection = selection,
            // `choose_model_row` adds the step: it has the window that the
            // confirm dialog needs.
            PickerTarget::FallbackStep => {}
            PickerTarget::CommandCheck => self.set_check_account_model(selection, cx),
            PickerTarget::Task(id) => {
                // The hub saves the pick at once and a running turn goes
                // on. The thread shows the new pick when the hub answers.
                if let Some(selection) = selection {
                    let client = self.client.clone();
                    let id = id.clone();
                    self.spawn_command(
                        cx,
                        async move { client.set_task_model(id, selection).await },
                    );
                }
            }
        }
        cx.notify();
    }

    /// The search text of the picker.
    fn picker_query(&self, cx: &gpui_kit::App) -> String {
        self.inputs.model_search.read(cx).value().to_string()
    }

    /// The rows that the main pane of the open picker shows now, and
    /// whether a search is on (then the rail is hidden).
    pub fn picker_rows(&self, cx: &gpui_kit::App) -> (Vec<RowRef>, bool) {
        let query = self.picker_query(cx);
        let searching = !query.trim().is_empty();
        let view = self
            .picker
            .as_ref()
            .map(|p| p.view.clone())
            .unwrap_or(PickerView::Favorites);
        let groups = self.open_groups();
        let rows = picker::view_rows(&groups, &self.state.favorite_models, &view, &query);
        (rows, searching)
    }

    /// The groups of the open picker: every group, or only the models of
    /// the task's worker.
    fn open_groups(&self) -> Vec<ModelGroupView> {
        match self.picker.as_ref() {
            Some(open) => self.picker_groups(&open.target),
            None => self.state.model_groups.clone(),
        }
    }

    /// The row to put the keyboard on in some rows: the current model, else
    /// the first row.
    fn first_highlight(
        &self,
        target: &PickerTarget,
        groups: &[ModelGroupView],
        rows: &[RowRef],
    ) -> Option<usize> {
        self.picker_selection(target)
            .and_then(|s| row_of(groups, rows, &s))
            .or_else(|| (!rows.is_empty()).then_some(0))
    }

    /// Opens the picker for a target, with an empty search that has the
    /// focus. It opens on Favorites when a favorite has a row, else on the
    /// account of the current model (see `picker::initial_view`), with the
    /// keyboard on the current model.
    pub fn open_picker(
        &mut self,
        target: PickerTarget,
        anchor: MenuAnchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        self.effort_menu = None;
        self.inputs.model_search.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        let current = self.picker_selection(&target);
        let groups = self.picker_groups(&target);
        let favorites = &self.state.favorite_models;
        let view = picker::initial_view(&groups, favorites, current.as_ref());
        let rows = picker::view_rows(&groups, favorites, &view, "");
        let highlighted = self.first_highlight(&target, &groups, &rows);
        self.picker = Some(OpenPicker {
            target,
            anchor,
            view,
            focus: PickerFocus::List,
            highlighted,
            scroll: ScrollHandle::new(),
            rail_scroll: ScrollHandle::new(),
            rail_hover: None,
            pending_row: highlighted,
        });
        cx.notify();
    }

    /// Opens the picker under a trigger whose bounds were recorded.
    pub fn open_picker_under(
        &mut self,
        trigger: &'static str,
        target: PickerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(bounds) = self.triggers.get(trigger) {
            self.open_picker(target, MenuAnchor::Trigger(bounds), window, cx);
        }
    }

    pub fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.picker.take() {
            self.focus_after_popover(&open.target, window, cx);
        }
        cx.notify();
    }

    /// After a popover closes, the focus goes back to the message box of its
    /// target when no dialog is open (the reply box for a task), else to the
    /// app.
    fn focus_after_popover(
        &mut self,
        target: &PickerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog.is_none() {
            if matches!(
                target,
                PickerTarget::FallbackStep | PickerTarget::CommandCheck
            ) {
                self.focus.focus(window, cx);
                return;
            }
            let input = match target {
                PickerTarget::Task(_) => &self.inputs.reply,
                _ => &self.inputs.composer,
            };
            input.update(cx, |input, cx| input.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
    }

    /// Shows Favorites or an account in the main pane, with the keyboard on
    /// the current model of that view, or on its first row.
    pub fn show_picker_view(&mut self, view: PickerView, cx: &mut Context<Self>) {
        let Some(target) = self.picker.as_ref().map(|p| p.target.clone()) else {
            return;
        };
        let groups = self.picker_groups(&target);
        let rows = picker::view_rows(&groups, &self.state.favorite_models, &view, "");
        let highlighted = self.first_highlight(&target, &groups, &rows);
        if let Some(open) = self.picker.as_mut() {
            open.view = view;
            open.highlighted = highlighted;
            open.scroll.scroll_to_item(0);
            open.pending_row = highlighted;
        }
        cx.notify();
    }

    /// Moves the rail selection by `delta` buttons, and shows that view.
    fn step_rail(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rail = picker::rail(&self.open_groups());
        let Some(open) = self.picker.as_ref() else {
            return;
        };
        let at = rail.iter().position(|r| r.view == open.view);
        if let Some(next) = step(at, delta, rail.len()) {
            self.show_picker_view(rail[next].view.clone(), cx);
        }
    }

    /// Chooses the `n`th row that the main pane shows. The effort stays
    /// when the new model offers that level (see `picker::choose_model`).
    pub fn choose_model_row(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.picker.clone() else {
            return;
        };
        let (rows, _) = self.picker_rows(cx);
        let Some(row) = rows.get(n).copied() else {
            return;
        };
        let groups = &self.picker_groups(&open.target);
        let previous = self.picker_selection(&open.target);
        let chosen = picker::choose_model(
            previous.as_ref(),
            &groups[row.group],
            &groups[row.group].models[row.model],
        );
        if open.target == PickerTarget::FallbackStep {
            // A model that the path has stays in the list and shows its step.
            if self
                .state
                .fallback_path
                .step_number(&chosen.account_label, &chosen.model_id)
                .is_some()
            {
                return;
            }
            self.close_picker(window, cx);
            self.add_fallback_step(chosen, window, cx);
            return;
        }
        self.close_picker(window, cx);
        // A pick of the model that the channel uses already, from
        // Tutti, makes it the channel's own.
        self.apply_selection(&open.target, Some(chosen), cx);
    }

    /// Stars a model, or takes its star away. The picker stays open and the
    /// selection does not change. The hub keeps the favorites, and its feed
    /// entry updates every client.
    pub fn toggle_favorite(
        &mut self,
        account_label: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        let on = !self.state.is_favorite(&account_label, &model_id);
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.set_favorite_model(account_label, model_id, on).await
        });
        cx.notify();
    }

    /// Keys in the open picker. Up and Down move in the rail or in the
    /// list, Right and typing go from the rail to the list, Left goes from
    /// the list to the rail when the search is empty, Enter chooses, and
    /// Escape clears the search first, then closes only the picker.
    fn picker_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.picker.clone() else {
            return;
        };
        let (rows, searching) = self.picker_rows(cx);
        let len = rows.len();
        let has_rail = !searching && !self.open_groups().is_empty();
        let on_rail = has_rail && matches!(open.focus, PickerFocus::Rail { .. });
        let key = event.keystroke.key.as_str();
        match key {
            "escape" if searching => {
                self.inputs
                    .model_search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.picker_search_changed(cx);
            }
            "escape" => self.close_picker(window, cx),
            "down" | "up" => {
                let delta = if key == "down" { 1 } else { -1 };
                if on_rail {
                    self.set_picker_focus(PickerFocus::Rail { ring: true });
                    self.step_rail(delta, cx);
                } else {
                    let next = step(open.highlighted.filter(|h| *h < len), delta, len);
                    if let Some(row) = next {
                        open.scroll.scroll_to_item(row);
                    }
                    if let Some(open) = self.picker.as_mut() {
                        open.highlighted = next;
                    }
                }
            }
            "right" | "enter" if on_rail => self.set_picker_focus(PickerFocus::List),
            "left" if has_rail && open.focus == PickerFocus::List => {
                self.set_picker_focus(PickerFocus::Rail { ring: true })
            }
            "enter" => {
                if let Some(n) = open.highlighted.filter(|h| *h < len) {
                    self.choose_model_row(n, window, cx);
                }
            }
            _ => {
                // A typed character goes on to the search field, and the
                // keyboard leaves the rail.
                if on_rail && event.keystroke.key_char.is_some() {
                    self.set_picker_focus(PickerFocus::List);
                }
                return;
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn set_picker_focus(&mut self, focus: PickerFocus) {
        if let Some(open) = self.picker.as_mut() {
            open.focus = focus;
        }
    }

    /// The search changed. With a query, the keyboard goes to the first
    /// match. With none, the view of the rail comes back, with the keyboard
    /// on the current model.
    pub fn picker_search_changed(&mut self, cx: &mut Context<Self>) {
        let (rows, searching) = self.picker_rows(cx);
        let Some(target) = self.picker.as_ref().map(|p| p.target.clone()) else {
            return;
        };
        let highlighted = if searching {
            (!rows.is_empty()).then_some(0)
        } else {
            self.first_highlight(&target, &self.picker_groups(&target), &rows)
        };
        if let Some(open) = self.picker.as_mut() {
            open.highlighted = highlighted;
            open.rail_hover = None;
            open.scroll.scroll_to_item(0);
            open.pending_row = if searching { None } else { highlighted };
            if searching {
                open.focus = PickerFocus::List;
            }
        }
        cx.notify();
    }

    /// The effort levels of the target's model, and the provider name.
    fn target_effort(&self, target: &PickerTarget) -> Option<(SelectionView, EffortView, String)> {
        let selection = self.picker_selection(target)?;
        let (group, model) = self
            .state
            .model_choice(&selection.account_label, &selection.model_id)?;
        let effort = model.effort.clone()?;
        Some((selection, effort, group.provider.clone()))
    }

    pub fn open_effort(
        &mut self,
        trigger: &'static str,
        target: PickerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.triggers.get(trigger) else {
            return;
        };
        let Some((selection, effort, _)) = self.target_effort(&target) else {
            return;
        };
        self.menu = None;
        self.picker = None;
        let highlighted = effort
            .rows(selection.effort())
            .iter()
            .position(|r| r.selected);
        self.effort_menu = Some(OpenEffort {
            target,
            anchor: MenuAnchor::Trigger(bounds),
            highlighted,
        });
        // The menu has no input. The app takes the keys.
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub fn close_effort(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.effort_menu.take() {
            self.focus_after_popover(&open.target, window, cx);
        }
        cx.notify();
    }

    /// Chooses an effort for the target. `None` takes the effort out, so the
    /// provider uses its default.
    pub fn choose_effort(
        &mut self,
        value: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.effort_menu.clone() else {
            return;
        };
        self.close_effort(window, cx);
        if let Some(selection) = self.picker_selection(&open.target) {
            let chosen = picker::with_effort(&selection, value.as_deref());
            if chosen != selection {
                self.apply_selection(&open.target, Some(chosen), cx);
            }
        }
    }

    /// Keys in the open effort menu. Returns true when the menu took the key.
    pub fn effort_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(open) = self.effort_menu.as_ref() else {
            return false;
        };
        let rows = self
            .target_effort(&open.target)
            .map(|(selection, effort, _)| effort.rows(selection.effort()))
            .unwrap_or_default();
        match key {
            "escape" => self.close_effort(window, cx),
            "down" | "up" => {
                let next = step(
                    open.highlighted,
                    if key == "down" { 1 } else { -1 },
                    rows.len(),
                );
                if let Some(open) = self.effort_menu.as_mut() {
                    open.highlighted = next;
                }
            }
            "enter" => {
                if let Some(row) = open.highlighted.and_then(|i| rows.get(i)) {
                    let value = row.value.clone();
                    self.choose_effort(value, window, cx);
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// The model trigger of a target. `id` records its bounds, so the picker
    /// opens under it.
    pub fn render_model_trigger(
        &self,
        id: &'static str,
        target: PickerTarget,
        style: TriggerStyle,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        // An assistant on a fallback shows the model that it runs on now.
        let fallback = self.channel_fallback(&target);
        let selection = fallback
            .as_ref()
            .map(|f| f.to.clone())
            .or_else(|| self.picker_selection(&target));
        let locked = self.picker_locked(&target);
        let open = !locked
            && self
                .picker
                .as_ref()
                .is_some_and(|p| p.target == target && self.dialog_allows(&p.target));
        let logo = selection
            .as_ref()
            .and_then(|s| self.state.account_logo(&s.account_label).cloned());
        // In the dialog, a field with no model of its own uses Tutti's.
        let (name, account) = match &selection {
            Some(s) if s.model_id.is_empty() => (s.account_label.clone(), String::new()),
            Some(s) => (s.model_name.clone(), s.account_label.clone()),
            None if style == TriggerStyle::Field => {
                let tutti = match &target {
                    PickerTarget::NewTask => self
                        .state
                        .channel(&self.convo.new_task.channel_id)
                        .and_then(|c| c.task_selection.as_ref())
                        .map(|s| s.model_name.clone()),
                    _ => self
                        .state
                        .tutti()
                        .and_then(|c| c.coordinator_selection.as_ref())
                        .map(|e| e.selection.model_name.clone()),
                };
                let same = match &target {
                    PickerTarget::NewTask => "Same as the channel".to_owned(),
                    _ => format!("Same as {}", crate::ui::TUTTI),
                };
                (same, tutti.unwrap_or_default())
            }
            None => ("Choose a model".to_owned(), String::new()),
        };
        let account = if style == TriggerStyle::ChipShort {
            String::new()
        } else {
            account
        };
        let target_click = target.clone();
        let body = div()
            .flex()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .min_w_0()
            .when(fallback.is_some(), |d| {
                // The marker: the assistant is on a fallback.
                d.child(
                    div()
                        .id("fallback-marker")
                        .debug_selector(|| "fallback-marker".into())
                        .flex_none()
                        .size(t.component.state_dot)
                        .rounded(t.radius.full)
                        .bg(c.accent_text)
                        .tooltip(|_, cx| {
                            gpui_kit::AppContext::new(cx, |_| {
                                crate::ui::convo_sidebar::Tip("On a fallback".into())
                            })
                            .into()
                        }),
                )
            })
            .when_some(logo, |d, logo| {
                d.child(provider_logo(&logo, t.icon.dense, c.text_primary))
            })
            .child(line(name, Self::trigger_type(style, true, t), c.text_primary).flex_none())
            .when(!account.is_empty(), |d| {
                d.child(line(
                    account,
                    Self::trigger_type(style, false, t),
                    if open {
                        c.text_secondary
                    } else {
                        c.text_tertiary
                    },
                ))
            });
        let trigger = match style {
            TriggerStyle::Chip | TriggerStyle::ChipShort => focusable(div().id(id), t)
                .debug_selector(move || id.into())
                .relative()
                .flex()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .h(t.component.composer_chip)
                .px(t.space.sm)
                .min_w_0()
                .rounded(t.radius.md)
                .when(!locked, |d| d.cursor_pointer())
                .when(open, |d| d.bg(c.surface_selected))
                .when(!open && !locked, |d| d.hover(|s| s.bg(c.hover_card)))
                .child(self.triggers.track(id))
                .child(body)
                // A locked chip shows the choice and does not open.
                .when(!locked, |d| {
                    d.child(icon("chevron-down", t.space.md, c.text_secondary))
                }),
            TriggerStyle::Field => focusable(div().id(id), t)
                .relative()
                .flex()
                .items_center()
                .justify_between()
                .gap(t.space.sm)
                .py(t.component.input_pad_y)
                .pl(t.space.md)
                .pr(t.space.sm + t.space.xxs)
                .bg(c.surface_card)
                .border(t.border_width)
                .border_color(if open { c.accent_text } else { c.border_strong })
                .rounded(t.radius.md)
                .cursor_pointer()
                .hover(|s| s.border_color(c.text_tertiary))
                .child(self.triggers.track(id))
                .child(body)
                .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary)),
        };
        trigger
            .when(!locked, |d| {
                d.on_click(cx.listener(move |this, _, window, cx| {
                    // On a fallback, the chip opens its own menu first.
                    if let PickerTarget::Assistant(channel_id) = &target_click
                        && this.channel_fallback(&target_click).is_some()
                    {
                        this.open_fallback_menu(id, channel_id.clone(), cx);
                        return;
                    }
                    this.open_picker_under(id, target_click.clone(), window, cx)
                }))
            })
            .into_any_element()
    }

    /// The fallback that the assistant of this target runs on, if any.
    pub fn channel_fallback(
        &self,
        target: &PickerTarget,
    ) -> Option<consort_client::ChannelFallbackView> {
        let PickerTarget::Assistant(id) = target else {
            return None;
        };
        self.state.channel(id).and_then(|c| c.fallback.clone())
    }

    /// The menu of the chip while the assistant runs on a fallback: what
    /// happened, Go back, Choose another model and Edit path. It opens under
    /// the chip, or above it near the bottom of the window.
    pub fn open_fallback_menu(
        &mut self,
        trigger: &'static str,
        channel_id: String,
        cx: &mut Context<Self>,
    ) {
        let (Some(bounds), Some(fallback)) = (
            self.triggers.get(trigger),
            self.channel_fallback(&PickerTarget::Assistant(channel_id.clone())),
        ) else {
            return;
        };
        let (menu, header) = fallback_menu(&fallback, &channel_id);
        let mut open = crate::ui::menu::OpenMenu::under(bounds, menu, false);
        open.header = Some(header);
        self.effort_menu = None;
        self.picker = None;
        self.open_menu(open, cx);
    }

    /// Whether a popover for this target may show now. A dialog picker shows
    /// only while its dialog is open.
    fn dialog_allows(&self, target: &PickerTarget) -> bool {
        match target {
            PickerTarget::Assistant(_) | PickerTarget::Task(_) => true,
            PickerTarget::SettingsAssistant => {
                matches!(self.dialog, Some(crate::app::Dialog::ChannelSettings(_)))
            }
            PickerTarget::RoutineTask => {
                matches!(self.dialog, Some(crate::app::Dialog::Routine(_)))
            }
            PickerTarget::NewTask => self.dialog == Some(crate::app::Dialog::NewTask),
            PickerTarget::FallbackStep | PickerTarget::CommandCheck => {
                self.page == crate::app::Page::Settings
                    && self.settings_section == SettingsSection::Models
            }
        }
    }

    fn trigger_type(style: TriggerStyle, strong: bool, t: &Tokens) -> consort_theme::Type {
        match (style, strong) {
            (TriggerStyle::Chip | TriggerStyle::ChipShort, true) => t.text.label,
            (TriggerStyle::Chip | TriggerStyle::ChipShort, false) => detail(t),
            (TriggerStyle::Field, true) => t.text.ui_strong,
            (TriggerStyle::Field, false) => t.text.ui,
        }
    }

    /// What the model chip of a target says in the composer: the model name,
    /// the account name, and whether the chip has a provider logo. The
    /// composer measures these to decide what fits.
    pub fn chip_texts(&self, target: &PickerTarget) -> (String, String, bool) {
        let shown = self
            .channel_fallback(target)
            .map(|f| f.to)
            .or_else(|| self.picker_selection(target));
        match shown {
            Some(s) => {
                let logo = self.state.account_logo(&s.account_label).is_some();
                if s.model_id.is_empty() {
                    (s.account_label, String::new(), logo)
                } else {
                    (s.model_name, s.account_label, logo)
                }
            }
            None => ("Choose a model".to_owned(), String::new(), false),
        }
    }

    /// The words on the effort chip of a target, `None` when the model has
    /// no effort option.
    pub fn effort_chip_label(&self, target: &PickerTarget) -> Option<String> {
        let (selection, effort, _) = self.target_effort(target)?;
        Some(effort.chip_label(selection.effort()))
    }

    /// The effort trigger of a target. `None` when the model has no effort
    /// option.
    pub fn render_effort_trigger(
        &self,
        id: &'static str,
        target: PickerTarget,
        style: TriggerStyle,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let c = &t.colors;
        let (selection, effort, _) = self.target_effort(&target)?;
        let label = effort.chip_label(selection.effort());
        let locked = self.picker_locked(&target);
        let open = !locked
            && self
                .effort_menu
                .as_ref()
                .is_some_and(|e| e.target == target);
        let target_click = target.clone();
        let trigger = match style {
            TriggerStyle::Chip | TriggerStyle::ChipShort => focusable(div().id(id), t)
                .debug_selector(move || id.into())
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .h(t.component.composer_chip)
                .px(t.space.sm)
                .rounded(t.radius.md)
                .when(!locked, |d| d.cursor_pointer())
                .when(open, |d| d.bg(c.surface_selected))
                .when(!open && !locked, |d| d.hover(|s| s.bg(c.hover_card)))
                .child(self.triggers.track(id))
                .child(icon("signal", t.icon.dense, c.text_secondary))
                .child(text(label, detail(t), c.text_secondary))
                .when(!locked, |d| {
                    d.child(icon("chevron-down", t.space.md, c.text_secondary))
                }),
            TriggerStyle::Field => focusable(div().id(id), t)
                .relative()
                .flex()
                .items_center()
                .justify_between()
                .gap(t.space.sm)
                .py(t.component.input_pad_y)
                .pl(t.space.md)
                .pr(t.space.sm + t.space.xxs)
                .bg(c.surface_card)
                .border(t.border_width)
                .border_color(if open { c.accent_text } else { c.border_strong })
                .rounded(t.radius.md)
                .cursor_pointer()
                .hover(|s| s.border_color(c.text_tertiary))
                .child(self.triggers.track(id))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.sm)
                        .min_w_0()
                        .child(icon("signal", t.icon.dense, c.text_secondary))
                        .child(line(label, t.text.ui, c.text_primary)),
                )
                .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary)),
        };
        Some(
            trigger
                .when(!locked, |d| {
                    d.on_click(cx.listener(move |this, _, window, cx| {
                        this.open_effort(id, target_click.clone(), window, cx)
                    }))
                })
                .into_any_element(),
        )
    }

    /// The open picker, above everything. `None` when no picker is open.
    pub fn render_picker(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.picker.clone()?;
        if !self.dialog_allows(&open.target) {
            self.picker = None;
            return None;
        }
        // The scroll to the current model waits for a frame in which the
        // list knows the bounds of its rows.
        if let Some(row) = open.pending_row {
            if open.scroll.bounds_for_item(row).is_some() {
                open.scroll.scroll_to_item(row);
                if let Some(picker) = self.picker.as_mut() {
                    picker.pending_row = None;
                }
            } else {
                cx.on_next_frame(window, |_, _, cx| cx.notify());
            }
        }
        let c = &t.colors;
        let groups = self.picker_groups(&open.target);
        let (rows, searching) = self.picker_rows(cx);
        let current = self.picker_selection(&open.target);
        let query = self.picker_query(cx).trim().to_owned();
        let search = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(t.space.sm)
            .px(t.space.md)
            .py(t.space.sm + t.space.xxs)
            .border_b(t.border_width)
            .border_color(c.border_subtle)
            .child(icon("search", t.icon.dense, c.text_tertiary))
            .child(div().flex_1().min_w_0().text_color(c.text_primary).child(
                crate::ui::parts::input(&self.inputs.model_search, t.text.ui),
            ));
        let main = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(search);
        let main = if groups.is_empty() {
            main.child(self.render_no_account(t, cx))
        } else {
            let (title, detail_text) = if searching {
                ("Search results".to_owned(), "From every account".to_owned())
            } else {
                match &open.view {
                    PickerView::Favorites => {
                        ("Favorites".to_owned(), "From every account".to_owned())
                    }
                    PickerView::Account(label) => (
                        label.clone(),
                        groups
                            .iter()
                            .find(|g| &g.account_label == label)
                            .map(|g| g.provider.clone())
                            .unwrap_or_default(),
                    ),
                }
            };
            // The header line sits outside the list, so it stays under the
            // search field while the list scrolls.
            let header = self.render_picker_header(title, detail_text, &open.target, t, cx);
            let body: AnyElement = if searching && rows.is_empty() {
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xxs)
                    .px(t.space.md + t.space.xxs)
                    .py(t.space.sm)
                    .child(text(
                        format!("No model matches \u{201C}{query}\u{201D}."),
                        t.text.ui_strong,
                        c.text_primary,
                    ))
                    .child(text(
                        "Try another word, or the name of an account.",
                        t.text.ui,
                        c.text_secondary,
                    ))
                    .into_any_element()
            } else {
                let wide = searching || open.view == PickerView::Favorites;
                let hint = !searching && open.view == PickerView::Favorites;
                self.render_picker_list(&groups, &rows, &open, current.as_ref(), wide, hint, t, cx)
            };
            main.child(header).child(body)
        };
        let panel = div()
            .id("model-picker")
            .occlude()
            .flex()
            .w(t.component.model_picker)
            .when(!groups.is_empty(), |d| d.h(t.component.model_picker_height))
            .overflow_hidden()
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.lg)
            .shadow(t.shadow_float())
            .font_family(t.font_sans.clone())
            .capture_key_down(cx.listener(Self::picker_key))
            .when(!searching && !groups.is_empty(), |d| {
                d.child(self.render_rail(&groups, &open, t, cx))
            })
            .child(main)
            .into_any_element();
        Some(floating(
            open.anchor,
            panel,
            t,
            window,
            cx,
            |this: &mut Self, window, cx| this.close_picker(window, cx),
        ))
    }

    /// The header line of the main pane: a title in semibold, a detail in
    /// `text.tertiary`, and, for a target with a model of its own, the text
    /// button "Use the Tutti model" at its right end.
    fn render_picker_header(
        &self,
        title: String,
        detail_text: String,
        target: &PickerTarget,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let semibold = consort_theme::Type {
            weight: t.text.title.weight,
            ..t.text.ui
        };
        let target = target.clone();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.sm + t.space.xxs)
            .pb(t.space.xs)
            .px(t.space.md + t.space.xxs)
            .child(line(title, semibold, c.text_primary).flex_none())
            .child(
                line(detail_text, t.text.caption, c.text_tertiary)
                    .flex_1()
                    .min_w_0(),
            )
            .when(self.picker_can_inherit(&target), |d| {
                d.child(
                    focusable(div().id("use-tutti-model"), t)
                        .flex_none()
                        .px(t.space.xs)
                        .rounded(t.radius.sm)
                        .cursor_pointer()
                        .type_style(detail(t))
                        .text_color(c.text_secondary)
                        .hover(|s| s.text_color(c.text_primary).bg(c.hover_raised))
                        .child(if target == PickerTarget::CommandCheck {
                            "Choose for me".to_owned()
                        } else {
                            format!("Use the {} model", crate::ui::TUTTI)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_picker(window, cx);
                            this.apply_selection(&target, None, cx);
                        })),
                )
            })
            .into_any_element()
    }

    /// The rail: Favorites, a divider, then one button per account. The
    /// selected button has `surface.selected` and an accent bar on the
    /// rail's right edge. Accounts of one provider carry badges.
    fn render_rail(
        &self,
        groups: &[ModelGroupView],
        open: &OpenPicker,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let size = t.component.picker_rail_button;
        let ring = t.focus_ring();
        let mut rail = div()
            .id("picker-rail")
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(t.space.xs)
            .w(t.component.picker_rail)
            .py(t.space.xs + t.space.xxs)
            .bg(c.surface_card)
            .border_r(t.border_width)
            .border_color(c.border_subtle)
            .overflow_y_scroll()
            .track_scroll(&open.rail_scroll);
        for (index, item) in picker::rail(groups).into_iter().enumerate() {
            let selected = item.view == open.view;
            let content: AnyElement = match &item.view {
                PickerView::Favorites => {
                    icon("star-filled", t.component.provider_logo, c.accent_text).into_any_element()
                }
                PickerView::Account(label) => {
                    let logo = groups
                        .iter()
                        .find(|g| &g.account_label == label)
                        .map(|g| g.logo.clone())
                        .unwrap_or_default();
                    provider_logo(&logo, t.component.provider_logo, c.text_primary)
                }
            };
            let view = item.view.clone();
            let button = div()
                .id(("picker-rail-item", index))
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(size)
                .rounded(t.radius.md)
                .cursor_pointer()
                .when(selected, |d| d.bg(c.surface_selected))
                .when(!selected, |d| d.hover(|s| s.bg(c.hover_card)))
                .when(
                    selected && open.focus == (PickerFocus::Rail { ring: true }),
                    |d| d.shadow(ring.clone()),
                )
                .child(content)
                .when_some(item.badge, |d, badge| {
                    d.child(rail_badge(badge, selected, t))
                })
                .when(selected, |d| {
                    // The bar sits on the rail's right edge, centred on the
                    // button.
                    d.child(
                        div()
                            .absolute()
                            .right(-(t.component.picker_rail - size) / 2.0)
                            .top((size - t.component.picker_rail_marker_height) / 2.0)
                            .w(t.component.picker_rail_marker)
                            .h(t.component.picker_rail_marker_height)
                            .rounded(t.radius.sm)
                            .bg(c.accent_text),
                    )
                })
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if let Some(open) = this.picker.as_mut() {
                        if *hovered {
                            open.rail_hover = Some(index);
                        } else if open.rail_hover == Some(index) {
                            open.rail_hover = None;
                        }
                    }
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_picker_focus(PickerFocus::Rail { ring: false });
                    this.show_picker_view(view.clone(), cx);
                }));
            rail = rail.child(button);
            if index == 0 {
                rail = rail.child(
                    div()
                        .flex_none()
                        .w(size - t.space.sm)
                        .h(t.border_width)
                        .bg(c.border_strong),
                );
            }
        }
        rail.into_any_element()
    }

    /// The tooltip of the rail button under the pointer: the account name
    /// and its workers, or Favorites. It shows to the right of the button,
    /// above the main pane. `None` when no button is under the pointer.
    pub fn render_picker_tooltip(&self, t: &Tokens) -> Option<AnyElement> {
        let open = self.picker.as_ref()?;
        let index = open.rail_hover?;
        let bounds = open.rail_scroll.bounds_for_item(rail_child(index))?;
        let groups = self.open_groups();
        let item = picker::rail(&groups).into_iter().nth(index)?;
        let (title, detail_text) = match &item.view {
            PickerView::Favorites => ("Favorites".to_owned(), "From every account".to_owned()),
            PickerView::Account(label) => (
                label.clone(),
                self.state
                    .model_groups
                    .iter()
                    .find(|g| &g.account_label == label)
                    .map(|g| g.provider.clone())
                    .unwrap_or_default(),
            ),
        };
        let c = &t.colors;
        let tip = div()
            .flex()
            .flex_col()
            .px(t.space.sm)
            .py(t.space.xs)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.md)
            .shadow(t.shadow_float())
            .font_family(t.font_sans.clone())
            .whitespace_nowrap()
            .child(text(title, t.text.ui_strong, c.text_primary))
            .when(!detail_text.is_empty(), |d| {
                d.child(text(detail_text, t.text.caption, c.text_secondary))
            });
        Some(
            deferred(
                anchored()
                    .position(point(bounds.right() + t.space.sm, bounds.top()))
                    .snap_to_window()
                    .child(tip),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }

    /// The model rows, in a list that scrolls inside the picker. `wide`
    /// rows (Favorites and search) show the provider logo and the account
    /// name. `hint` ends the list with the note about stars.
    #[allow(clippy::too_many_arguments)]
    fn render_picker_list(
        &self,
        groups: &[ModelGroupView],
        rows: &[RowRef],
        open: &OpenPicker,
        current: Option<&SelectionView>,
        wide: bool,
        hint: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let mut list = div()
            .id("model-list")
            .flex()
            .flex_col()
            .gap(t.border_width)
            .size_full()
            .px(t.space.xs + t.space.xxs)
            .pt(t.space.xxs)
            .pb(t.space.xs + t.space.xxs)
            .overflow_y_scroll()
            .track_scroll(&open.scroll);
        for (n, row) in rows.iter().copied().enumerate() {
            let group = &groups[row.group];
            let model = &group.models[row.model];
            let selected = current
                .is_some_and(|s| s.account_label == group.account_label && s.model_id == model.id);
            let highlighted = open.highlighted == Some(n);
            let favorite = self.state.is_favorite(&group.account_label, &model.id);
            let note = self.task_note(&open.target, &group.account_label, &model.id);
            list = list.child(self.render_model_row(
                n,
                group,
                model,
                note,
                RowLook {
                    wide,
                    selected,
                    highlighted,
                    favorite,
                },
                t,
                cx,
            ));
        }
        if hint {
            list = list.child(
                div()
                    .flex_none()
                    .px(t.space.sm)
                    .pt(t.space.md + t.space.xxs)
                    .pb(t.space.xs)
                    .child(text(
                        "Star a model in any account to add it here.",
                        consort_theme::Type {
                            line: t.text.caption.line,
                            ..detail(t)
                        },
                        c.text_tertiary,
                    )),
            );
        }
        // The list never stops at a hard edge (AGENTS.md rule 3): it fades
        // into the popover where more rows are above or below.
        let offset = open.scroll.offset();
        let max = open.scroll.max_offset();
        let above = offset.y < -Pixels::from(0.5);
        let below = -offset.y + Pixels::from(0.5) < max.y;
        let fade = t.space.xl + t.space.md;
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(list)
            .when(above, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(fade)
                        .bg(linear_gradient(
                            180.,
                            linear_color_stop(c.surface_raised, 0.),
                            linear_color_stop(c.raised_clear, 1.),
                        )),
                )
            })
            .when(below, |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(fade)
                        .bg(linear_gradient(
                            0.,
                            linear_color_stop(c.surface_raised, 0.),
                            linear_color_stop(c.raised_clear, 1.),
                        )),
                )
            })
            .into_any_element()
    }

    /// One model row: the name, then (in an account) the effort range or
    /// (in Favorites and search) the provider logo before the name and the
    /// account name after it, then the star and the check.
    #[allow(clippy::too_many_arguments)]
    fn render_model_row(
        &self,
        n: usize,
        group: &ModelGroupView,
        model: &consort_client::ModelChoiceView,
        note: Option<String>,
        look: RowLook,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let RowLook {
            wide,
            selected,
            highlighted,
            favorite,
        } = look;
        let name = line(
            model.name.clone(),
            if selected {
                t.text.ui_strong
            } else {
                t.text.ui
            },
            c.text_primary,
        );
        // `text.tertiary` never sits on `surface.selected`.
        let muted = if selected {
            c.text_secondary
        } else {
            c.text_tertiary
        };
        let label = group.account_label.clone();
        let id = model.id.clone();
        // The star is filled for a favorite. Otherwise it shows as an
        // outline only while the pointer or the keyboard is on the row.
        let star_color = if favorite {
            c.accent_text
        } else if highlighted {
            c.text_secondary
        } else {
            transparent_black()
        };
        let star = div()
            .id(("picker-star", n))
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(t.icon.dense)
            .cursor_pointer()
            .when(highlighted, |d| d.child(self.triggers.track("picker-star")))
            .child(
                icon(
                    if favorite { "star-filled" } else { "star" },
                    t.icon.dense,
                    star_color,
                )
                .when(!favorite, |s| {
                    s.group_hover("picker-row", |s| s.text_color(c.text_secondary))
                }),
            )
            // A click on the star changes the favorite only. It does not
            // choose the model or close the picker.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_favorite(label.clone(), id.clone(), cx);
            }));
        let range = (!wide)
            .then(|| model.effort.as_ref().map(EffortView::range_label))
            .flatten();
        let row_id = format!("model-row-{}-{}", group.account_label, model.id);
        div()
            .id(SharedString::from(row_id.clone()))
            .debug_selector(move || row_id.clone())
            .group("picker-row")
            .flex()
            .flex_none()
            .items_center()
            .gap(if wide {
                t.space.sm + t.space.xxs
            } else {
                t.space.sm
            })
            .py(t.space.xs + t.space.xxs)
            .px(t.space.sm)
            .rounded(t.radius.md)
            .cursor_pointer()
            .when(selected && !highlighted, |d| d.bg(c.surface_selected))
            .when(selected && highlighted, |d| d.bg(c.hover_selected))
            .when(!selected && highlighted, |d| d.bg(c.hover_raised))
            .when(!selected, |d| d.hover(|s| s.bg(c.hover_raised)))
            .map(|d| {
                // The note of a task pick sits under the name.
                let with_note = |first: AnyElement| {
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(first)
                        .when_some(note.clone(), |d, note| {
                            d.child(
                                text(note, t.text.caption, muted)
                                    .debug_selector(move || format!("pick-note-{n}")),
                            )
                        })
                        .into_any_element()
                };
                if wide {
                    d.child(provider_logo(&group.logo, t.icon.dense, c.text_primary))
                        .child(with_note(
                            div()
                                .flex()
                                .min_w_0()
                                .items_center()
                                .gap(t.space.sm)
                                .child(name.flex_none())
                                .child(line(group.account_label.clone(), t.text.caption, muted))
                                .into_any_element(),
                        ))
                } else {
                    d.child(with_note(name.into_any_element()))
                        .when_some(range, |d, range| {
                            d.child(text(range, t.text.caption, muted))
                        })
                }
            })
            .child(star)
            .child(div().flex_none().w(t.icon.dense).when(selected, |d| {
                d.child(icon("check", t.icon.dense, c.accent_text))
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.choose_model_row(n, window, cx)))
            .into_any_element()
    }

    /// The picker when no account is signed in on any worker.
    fn render_no_account(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(t.space.sm)
            .p(t.space.md)
            .child(text(
                "No account is signed in. Add one in Settings > Providers.",
                t.text.ui,
                c.text_secondary,
            ))
            .child(
                button(
                    "picker-open-workers",
                    "Open Settings",
                    ButtonKind::Secondary,
                    t,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_picker(window, cx);
                    if this.dialog.is_some() {
                        this.close_dialog(window, cx);
                    }
                    this.open_settings(SettingsSection::Workers, cx);
                })),
            )
            .into_any_element()
    }

    /// The open effort menu, above everything. `None` when it is closed.
    pub fn render_effort_menu(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.effort_menu.clone()?;
        if !self.dialog_allows(&open.target) {
            self.effort_menu = None;
            return None;
        }
        let Some((selection, effort, provider)) = self.target_effort(&open.target) else {
            self.effort_menu = None;
            return None;
        };
        let c = &t.colors;
        let mut menu = div()
            .id("effort-menu")
            .occlude()
            .flex()
            .flex_col()
            .gap(t.border_width)
            .w(t.component.effort_menu)
            .p(t.space.xs + t.space.xxs)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.lg)
            .shadow(t.shadow_float())
            .font_family(t.font_sans.clone())
            .child(
                div()
                    .px(t.space.sm)
                    .pt(t.space.xs + t.space.xxs)
                    .pb(t.space.xs)
                    .child(text(
                        format!("Effort for {}", selection.model_name),
                        detail(t),
                        c.text_tertiary,
                    )),
            );
        for (i, row) in effort.rows(selection.effort()).into_iter().enumerate() {
            menu = menu.child(effort_row(i, row, open.highlighted == Some(i), t, cx));
        }
        menu = menu.child(
            div()
                .mt(t.space.xs)
                .px(t.space.sm)
                .pt(t.space.sm)
                .pb(t.space.xs)
                .border_t(t.border_width)
                .border_color(c.border_subtle)
                .child(text(
                    effort_note(&provider),
                    t.text.caption,
                    c.text_tertiary,
                )),
        );
        Some(floating(
            open.anchor,
            menu.into_any_element(),
            t,
            window,
            cx,
            |this: &mut Self, window, cx| this.close_effort(window, cx),
        ))
    }
}

/// How a model row looks.
#[derive(Clone, Copy)]
struct RowLook {
    /// A row of Favorites or of a search: logo and account name.
    wide: bool,
    selected: bool,
    highlighted: bool,
    favorite: bool,
}

/// The badge on an account button of the rail, such as "P1": at its bottom
/// right corner, on `surface.raised` with a `border.strong` ring.
fn rail_badge(badge: String, selected: bool, t: &Tokens) -> impl IntoElement {
    let c = &t.colors;
    let side = t.component.picker_rail_badge;
    div()
        .absolute()
        .bottom(t.border_width)
        .right(t.border_width)
        .flex()
        .items_center()
        .justify_center()
        .h(side)
        .min_w(side)
        .px(t.space.xxs + t.border_width)
        .bg(c.surface_raised)
        .border(t.border_width)
        .border_color(c.border_strong)
        .rounded(t.radius.full)
        .text_size(t.component.picker_rail_badge_text)
        .line_height(side - t.border_width * 2.0)
        .font_weight(t.text.title.weight)
        .text_color(if selected {
            c.text_primary
        } else {
            c.text_secondary
        })
        .child(badge)
}

/// One row of the effort menu.
fn effort_row(
    i: usize,
    row: EffortRow,
    highlighted: bool,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> AnyElement {
    let c = &t.colors;
    let value = row.value.clone();
    let mut d = div()
        .flex()
        .items_center()
        .gap(t.space.sm)
        .py(t.space.xs + t.space.xxs)
        .px(t.space.sm)
        .rounded(t.radius.md)
        .cursor_pointer()
        .when(row.selected, |d| d.bg(c.surface_selected))
        .when(!row.selected && highlighted, |d| d.bg(c.hover_raised))
        .when(!row.selected, |d| d.hover(|s| s.bg(c.hover_raised)))
        .child(
            line(
                row.label,
                if row.selected {
                    t.text.ui_strong
                } else {
                    t.text.ui
                },
                c.text_primary,
            )
            .flex_1()
            .min_w_0(),
        );
    if row.is_default {
        d = d.child(text(
            "Default",
            t.text.caption,
            if row.selected {
                c.text_secondary
            } else {
                c.text_tertiary
            },
        ));
    }
    d.child(div().flex_none().w(t.icon.dense).when(row.selected, |d| {
        d.child(icon("check", t.icon.dense, c.accent_text))
    }))
    .id(("effort-row", i))
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .on_click(cx.listener(move |this, _, window, cx| this.choose_effort(value.clone(), window, cx)))
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rail_divider_comes_after_favorites() {
        assert_eq!(rail_child(0), 0);
        assert_eq!(rail_child(1), 2);
        assert_eq!(rail_child(4), 5);
    }

    #[test]
    fn the_keyboard_wraps_around_the_list() {
        assert_eq!(step(None, 1, 3), Some(0));
        assert_eq!(step(None, -1, 3), Some(2));
        assert_eq!(step(Some(2), 1, 3), Some(0));
        assert_eq!(step(Some(0), -1, 3), Some(2));
        assert_eq!(step(Some(0), 1, 0), None);
    }

    #[test]
    fn the_effort_note_names_the_provider() {
        assert_eq!(
            effort_note("Claude Code"),
            "Claude Code offers these levels for this model. More effort thinks longer and uses more of your plan."
        );
    }
}
