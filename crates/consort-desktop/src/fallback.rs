//! The fallback path in the app: what the Models page keeps while the user
//! edits the path, and what each edit does. The words and the rows are in
//! `ui/models.rs`. The path lives on the hub, so each edit sends the whole
//! new list of steps (`Client::set_fallback_path`) and the feed shows the
//! result to every client.

use consort_client::{FallbackPathView, ModelGroupView, SelectionView};
use gpui_kit::{Context, Window};

use crate::app::{ConsortApp, Dialog, SettingsSection};

/// Something that the user does with the path. A menu item, a key or a
/// button gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FallbackAction {
    /// Move the step at this place (from 0) up or down one place.
    MoveStep(usize, Direction),
    /// Remove the step at this place.
    RemoveStep(usize),
    /// Turn the built-in row "Other accounts with the same model" on or off.
    SetOtherAccounts(bool),
    /// Add the step that waits in the confirm dialog.
    ConfirmStep,
    /// Take the assistant of a channel back to its first choice.
    GoBack(String),
    /// Open the model picker of the assistant of a channel.
    ChooseModel(String),
    /// Open Settings > Models.
    EditPath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// What a step that the user drags carries.
#[derive(Clone, Debug)]
pub struct StepDrag {
    /// The place of the step (from 0) when the drag began.
    pub from: usize,
    /// The words under the pointer.
    pub label: String,
}

/// What the Models page keeps.
#[derive(Debug, Default)]
pub struct FallbackUi {
    /// The place of the step that the pointer holds. The page draws an empty
    /// outline there while the drag runs.
    pub dragging: Option<usize>,
    /// The step that waits for the user to confirm that it goes to another
    /// harness. See [`Dialog::ConfirmOtherHarnessStep`].
    pub pending: Option<SelectionView>,
}

/// What adding a model to the path does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddOutcome {
    /// The path has this model already, as this step (from 1). The picker
    /// shows "Step N" and cannot choose it again.
    Repeat(usize),
    /// The model is on another harness than the first choice, and no step is
    /// yet. The dialog asks once.
    Confirm,
    /// Add the step now.
    Add,
}

/// Whether and how a model joins the path. `groups` are the accounts of the
/// model picker: the provider of the account tells whether the model is on
/// another harness than the first choice.
pub fn add_outcome(
    path: &FallbackPathView,
    groups: &[ModelGroupView],
    selection: &SelectionView,
) -> AddOutcome {
    if let Some(n) = path.step_number(&selection.account_label, &selection.model_id) {
        return AddOutcome::Repeat(n);
    }
    let provider = groups
        .iter()
        .find(|g| g.account_label == selection.account_label)
        .map(|g| g.provider.as_str());
    let other = match (&path.first, provider) {
        (Some(first), Some(provider)) => first.provider != provider,
        _ => false,
    };
    if other && !path.has_other_harness_step() {
        AddOutcome::Confirm
    } else {
        AddOutcome::Add
    }
}

/// The selections of the steps, in order.
pub fn step_selections(path: &FallbackPathView) -> Vec<SelectionView> {
    path.steps.iter().map(|s| s.selection.clone()).collect()
}

/// The steps after one moves a place. A step at an end stays.
pub fn moved(
    mut steps: Vec<SelectionView>,
    index: usize,
    direction: Direction,
) -> Vec<SelectionView> {
    let to = match direction {
        Direction::Up if index > 0 => index - 1,
        Direction::Down if index + 1 < steps.len() => index + 1,
        _ => return steps,
    };
    steps.swap(index, to);
    steps
}

/// The steps after the dragged step lands at `to` (a place from 0 in the new
/// list). Dropping on its own place changes nothing.
pub fn dropped(mut steps: Vec<SelectionView>, from: usize, to: usize) -> Vec<SelectionView> {
    if from >= steps.len() || from == to {
        return steps;
    }
    let step = steps.remove(from);
    steps.insert(to.min(steps.len()), step);
    steps
}

/// The steps without the one at this place.
pub fn without(mut steps: Vec<SelectionView>, index: usize) -> Vec<SelectionView> {
    if index < steps.len() {
        steps.remove(index);
    }
    steps
}

impl ConsortApp {
    /// Sends the path to the hub. The feed entry of the change updates the
    /// page here and in every other client.
    pub fn save_fallback_path(
        &mut self,
        other_accounts: bool,
        steps: Vec<SelectionView>,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.set_fallback_path(other_accounts, steps).await
        });
        cx.notify();
    }

    /// Runs one action of the path.
    pub fn run_fallback(
        &mut self,
        action: FallbackAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.state.fallback_path.clone();
        let steps = step_selections(&path);
        match action {
            FallbackAction::MoveStep(index, direction) => {
                self.save_fallback_path(path.other_accounts, moved(steps, index, direction), cx)
            }
            FallbackAction::RemoveStep(index) => {
                self.save_fallback_path(path.other_accounts, without(steps, index), cx)
            }
            FallbackAction::SetOtherAccounts(on) => self.save_fallback_path(on, steps, cx),
            FallbackAction::ConfirmStep => {
                if let Some(selection) = self.fallback_ui.pending.take() {
                    let mut steps = steps;
                    steps.push(selection);
                    self.save_fallback_path(path.other_accounts, steps, cx);
                }
                self.close_dialog(window, cx);
            }
            FallbackAction::GoBack(channel_id) => {
                let client = self.client.clone();
                self.spawn_command(cx, async move { client.go_back(channel_id).await });
            }
            FallbackAction::ChooseModel(channel_id) => {
                let trigger = "composer-model";
                self.open_picker_under(
                    trigger,
                    crate::ui::model_picker::PickerTarget::Assistant(channel_id),
                    window,
                    cx,
                );
            }
            FallbackAction::EditPath => self.open_settings(SettingsSection::Models, cx),
        }
    }

    /// The user picked a model for a new step. A model that the path has
    /// stays out. The first model on another harness opens the confirm
    /// dialog. Any other model joins the path at once.
    pub fn add_fallback_step(
        &mut self,
        selection: SelectionView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match add_outcome(
            &self.state.fallback_path,
            &self.state.model_groups,
            &selection,
        ) {
            AddOutcome::Repeat(_) => {}
            AddOutcome::Confirm => {
                self.fallback_ui.pending = Some(selection);
                self.open_dialog(Dialog::ConfirmOtherHarnessStep, window, cx);
            }
            AddOutcome::Add => {
                let path = self.state.fallback_path.clone();
                let mut steps = step_selections(&path);
                steps.push(selection);
                self.save_fallback_path(path.other_accounts, steps, cx);
            }
        }
    }
}
