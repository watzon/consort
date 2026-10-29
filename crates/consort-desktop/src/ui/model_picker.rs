//! The model picker: one component for every place that chooses a model.
//!
//! - A trigger, as a compact chip in the composer or as a full-width select
//!   field in a dialog. It shows the provider logo, the model name and the
//!   account name.
//! - A popover with a search field, one group per signed-in account, a list
//!   that scrolls inside the popover with fades at its edges (AGENTS.md rule
//!   3), and a footer that says what the choice changes. It flips up or down
//!   by the room it has (rule 4).
//! - The effort control: a chip (or a select in a dialog) with a small menu
//!   of the levels that the provider reports for the model.
//!
//! The data and the rules (groups, search, effort fallback) are in
//! `consort_client::picker`, so they have tests without a window.

use consort_client::{
    ChannelChanges, ChannelKind, EffortView, ModelGroupView, SelectionSource, SelectionView,
    picker::{self, EffortRow, PickerPurpose},
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, Pixels, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled,
    Window, div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Field, SettingsSection},
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
    /// "Model for the assistant" in the New channel dialog.
    NewChannelAssistant,
    /// "Model for tasks" in the New channel dialog.
    NewChannelTasks,
}

/// The open model picker.
#[derive(Clone, Debug)]
pub struct OpenPicker {
    pub target: PickerTarget,
    pub anchor: MenuAnchor,
    /// The model row that the keyboard is on, as an index into the models
    /// that the search shows.
    pub highlighted: Option<usize>,
    pub scroll: ScrollHandle,
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
    /// A full-width select field, for a dialog.
    Field,
}

/// One row of the picker list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerRow {
    /// The header of a group, by group index.
    Group(usize),
    /// A model: the group index, the model index in that group, and the
    /// index among the model rows.
    Model(usize, usize, usize),
}

/// The rows of the picker list for some groups, headers and models in
/// order. The list scrolls by these rows, so a row's index is its child
/// index in the list.
pub fn picker_rows(groups: &[ModelGroupView]) -> Vec<PickerRow> {
    let mut rows = Vec::new();
    let mut n = 0;
    for (gi, group) in groups.iter().enumerate() {
        rows.push(PickerRow::Group(gi));
        for mi in 0..group.models.len() {
            rows.push(PickerRow::Model(gi, mi, n));
            n += 1;
        }
    }
    rows
}

/// The number of model rows in some groups.
fn model_count(groups: &[ModelGroupView]) -> usize {
    groups.iter().map(|g| g.models.len()).sum()
}

/// The group and model index of the `n`th model row.
fn nth_model(groups: &[ModelGroupView], n: usize) -> Option<(usize, usize)> {
    picker_rows(groups).into_iter().find_map(|row| match row {
        PickerRow::Model(gi, mi, i) if i == n => Some((gi, mi)),
        _ => None,
    })
}

/// The child index in the list of the `n`th model row.
fn row_of_model(groups: &[ModelGroupView], n: usize) -> Option<usize> {
    picker_rows(groups)
        .iter()
        .position(|row| matches!(row, PickerRow::Model(_, _, i) if *i == n))
}

/// The index among the model rows of a selection, if the list shows it.
fn model_index_of(groups: &[ModelGroupView], selection: &SelectionView) -> Option<usize> {
    picker_rows(groups).into_iter().find_map(|row| match row {
        PickerRow::Model(gi, mi, n)
            if groups[gi].account_label == selection.account_label
                && groups[gi].models[mi].id == selection.model_id =>
        {
            Some(n)
        }
        _ => None,
    })
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

/// The note under the effort levels.
pub fn effort_note(provider: &str) -> String {
    format!(
        "{provider} offers these levels for this model. More effort thinks longer and uses more of your plan."
    )
}

impl ConsortApp {
    /// The selection that a target has now: for a channel, the selection
    /// that its assistant uses, its own or Everything's.
    pub fn picker_selection(&self, target: &PickerTarget) -> Option<SelectionView> {
        match target {
            PickerTarget::Assistant(id) => self
                .state
                .channel(id)
                .and_then(|c| c.coordinator_selection.as_ref())
                .map(|e| e.selection.clone()),
            PickerTarget::NewChannelAssistant => self.new_channel.coordinator_selection.clone(),
            PickerTarget::NewChannelTasks => self.new_channel.task_selection.clone(),
        }
    }

    /// What a choice changes, for the footer.
    fn picker_purpose(&self, target: &PickerTarget, cx: &gpui_kit::App) -> PickerPurpose {
        match target {
            PickerTarget::Assistant(id) => match self.state.channel(id) {
                Some(c) if c.kind == ChannelKind::Everything => PickerPurpose::Everything,
                Some(c) => PickerPurpose::Channel(c.name.clone()),
                None => PickerPurpose::Channel(String::new()),
            },
            PickerTarget::NewChannelAssistant => {
                PickerPurpose::Channel(self.field(Field::Name, cx))
            }
            PickerTarget::NewChannelTasks => PickerPurpose::Tasks(self.field(Field::Name, cx)),
        }
    }

    /// Whether the target has a model of its own that it can give up for
    /// the Everything model.
    fn picker_can_inherit(&self, target: &PickerTarget) -> bool {
        match target {
            PickerTarget::Assistant(id) => self.state.channel(id).is_some_and(|c| {
                c.kind != ChannelKind::Everything
                    && c.coordinator_selection
                        .as_ref()
                        .is_some_and(|e| e.source == SelectionSource::Channel)
            }),
            PickerTarget::NewChannelAssistant => self.new_channel.coordinator_selection.is_some(),
            PickerTarget::NewChannelTasks => self.new_channel.task_selection.is_some(),
        }
    }

    /// Sets the selection of a target. For a channel's assistant, this
    /// sends `channel.update` at once. `None` gives the channel the
    /// Everything model again.
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
            PickerTarget::NewChannelAssistant => self.new_channel.coordinator_selection = selection,
            PickerTarget::NewChannelTasks => self.new_channel.task_selection = selection,
        }
        cx.notify();
    }

    /// The groups that the search shows now.
    fn picker_groups(&self, cx: &gpui_kit::App) -> Vec<ModelGroupView> {
        let query = self.inputs.model_search.read(cx).value().to_string();
        picker::filter(&self.state.model_groups, &query)
    }

    /// Opens the picker for a target, with an empty search that has the
    /// focus, and the keyboard on the current model.
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
        let groups = self.state.model_groups.clone();
        let highlighted = current
            .as_ref()
            .and_then(|s| model_index_of(&groups, s))
            .or_else(|| (model_count(&groups) > 0).then_some(0));
        self.picker = Some(OpenPicker {
            target,
            anchor,
            highlighted,
            scroll: ScrollHandle::new(),
            pending_row: highlighted.and_then(|n| row_of_model(&groups, n)),
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
        if self.picker.take().is_some() {
            self.focus_after_popover(window, cx);
        }
        cx.notify();
    }

    /// After a popover closes, the focus goes back to the composer when no
    /// dialog is open, else to the app.
    fn focus_after_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_none() {
            self.inputs
                .composer
                .update(cx, |input, cx| input.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
    }

    /// Chooses the `n`th model row that the search shows. The effort stays
    /// when the new model offers that level (see `picker::choose_model`).
    pub fn choose_model_row(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.picker.clone() else {
            return;
        };
        let groups = self.picker_groups(cx);
        let Some((gi, mi)) = nth_model(&groups, n) else {
            return;
        };
        let previous = self.picker_selection(&open.target);
        let chosen = picker::choose_model(previous.as_ref(), &groups[gi], &groups[gi].models[mi]);
        self.close_picker(window, cx);
        // A pick of the model that the channel uses already, from
        // Everything, makes it the channel's own.
        self.apply_selection(&open.target, Some(chosen), cx);
    }

    /// Keys in the open picker: arrows move, Enter chooses, Escape closes
    /// only the picker.
    fn picker_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.picker.as_ref() else {
            return;
        };
        let groups = self.picker_groups(cx);
        let len = model_count(&groups);
        match event.keystroke.key.as_str() {
            "escape" => self.close_picker(window, cx),
            "down" | "up" => {
                let delta = if event.keystroke.key == "down" { 1 } else { -1 };
                let next = step(open.highlighted.filter(|h| *h < len), delta, len);
                if let Some(row) = next.and_then(|n| row_of_model(&groups, n)) {
                    open.scroll.scroll_to_item(row);
                }
                if let Some(open) = self.picker.as_mut() {
                    open.highlighted = next;
                }
            }
            "enter" => {
                if let Some(n) = open.highlighted.filter(|h| *h < len) {
                    self.choose_model_row(n, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The search changed: the keyboard goes to the first match.
    pub fn picker_search_changed(&mut self, cx: &mut Context<Self>) {
        let has_rows = model_count(&self.picker_groups(cx)) > 0;
        if let Some(open) = self.picker.as_mut() {
            open.highlighted = has_rows.then_some(0);
            open.pending_row = None;
            open.scroll.scroll_to_item(0);
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
        if self.effort_menu.take().is_some() {
            self.focus_after_popover(window, cx);
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
        let selection = self.picker_selection(&target);
        let open = self
            .picker
            .as_ref()
            .is_some_and(|p| p.target == target && self.dialog_allows(&p.target));
        let logo = selection
            .as_ref()
            .and_then(|s| self.state.account_logo(&s.account_label).cloned());
        // In the dialog, a field with no model of its own uses Everything's.
        let (name, account) = match &selection {
            Some(s) if s.model_id.is_empty() => (s.account_label.clone(), String::new()),
            Some(s) => (s.model_name.clone(), s.account_label.clone()),
            None if style == TriggerStyle::Field => {
                let everything = match &target {
                    PickerTarget::NewChannelTasks => self
                        .state
                        .everything()
                        .and_then(|c| c.task_selection.as_ref())
                        .map(|s| s.model_name.clone()),
                    _ => self
                        .state
                        .everything()
                        .and_then(|c| c.coordinator_selection.as_ref())
                        .map(|e| e.selection.model_name.clone()),
                };
                (
                    "Same as Everything".to_owned(),
                    everything.unwrap_or_default(),
                )
            }
            None => ("Choose a model".to_owned(), String::new()),
        };
        let target_click = target.clone();
        let body = div()
            .flex()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .min_w_0()
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
            TriggerStyle::Chip => focusable(div().id(id), t)
                .relative()
                .flex()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .h(t.component.composer_chip)
                .px(t.space.sm)
                .min_w_0()
                .rounded(t.radius.md)
                .cursor_pointer()
                .when(open, |d| d.bg(c.surface_selected))
                .when(!open, |d| d.hover(|s| s.bg(c.hover_card)))
                .child(self.triggers.track(id))
                .child(body)
                .child(icon("chevron-down", t.space.md, c.text_secondary)),
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
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_picker_under(id, target_click.clone(), window, cx)
            }))
            .into_any_element()
    }

    /// Whether a popover for this target may show now. A dialog picker shows
    /// only while its dialog is open.
    fn dialog_allows(&self, target: &PickerTarget) -> bool {
        match target {
            PickerTarget::Assistant(_) => true,
            PickerTarget::NewChannelAssistant | PickerTarget::NewChannelTasks => {
                self.dialog == Some(crate::app::Dialog::NewChannel)
            }
        }
    }

    fn trigger_type(style: TriggerStyle, strong: bool, t: &Tokens) -> consort_theme::Type {
        match (style, strong) {
            (TriggerStyle::Chip, true) => t.text.label,
            (TriggerStyle::Chip, false) => detail(t),
            (TriggerStyle::Field, true) => t.text.ui_strong,
            (TriggerStyle::Field, false) => t.text.ui,
        }
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
        let open = self
            .effort_menu
            .as_ref()
            .is_some_and(|e| e.target == target);
        let target_click = target.clone();
        let trigger = match style {
            TriggerStyle::Chip => focusable(div().id(id), t)
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .h(t.component.composer_chip)
                .px(t.space.sm)
                .rounded(t.radius.md)
                .cursor_pointer()
                .when(open, |d| d.bg(c.surface_selected))
                .when(!open, |d| d.hover(|s| s.bg(c.hover_card)))
                .child(self.triggers.track(id))
                .child(icon("signal", t.icon.dense, c.text_secondary))
                .child(text(label, detail(t), c.text_secondary))
                .child(icon("chevron-down", t.space.md, c.text_secondary)),
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
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_effort(id, target_click.clone(), window, cx)
                }))
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
        let groups = self.picker_groups(cx);
        let current = self.picker_selection(&open.target);
        let query = self.inputs.model_search.read(cx).value().trim().to_owned();
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
            .child(
                div()
                    .flex_1()
                    .text_color(c.text_primary)
                    .child(crate::ui::parts::input(
                        &self.inputs.model_search,
                        t.text.ui,
                    )),
            )
            .child(
                div()
                    .px(t.space.xs)
                    .rounded(t.radius.sm)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .type_style(t.text.caption)
                    .text_color(c.text_tertiary)
                    .child("Esc"),
            );
        let body: AnyElement = if self.state.model_groups.is_empty() {
            self.render_no_account(t, cx)
        } else if groups.is_empty() {
            div()
                .flex()
                .flex_col()
                .gap(t.space.xxs)
                .p(t.space.md)
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
            self.render_picker_list(&groups, &open, current.as_ref(), t, cx)
        };
        let purpose = self.picker_purpose(&open.target, cx);
        let can_inherit = self.picker_can_inherit(&open.target);
        let target = open.target.clone();
        let footer = div()
            .flex()
            .flex_none()
            .items_start()
            .gap(t.space.sm)
            .px(t.space.md)
            .py(t.space.sm + t.space.xxs)
            .bg(c.surface_card)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(t.text.caption.line)
                    .child(icon("info", t.icon.dense, c.text_tertiary)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.xs)
                    .child(text(
                        purpose.footer(),
                        consort_theme::Type {
                            line: t.text.caption.line,
                            ..detail(t)
                        },
                        c.text_tertiary,
                    ))
                    .when(can_inherit, |d| {
                        d.child(
                            div().flex().child(
                                button(
                                    "use-everything-model",
                                    "Use the Everything model",
                                    ButtonKind::Secondary,
                                    t,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.close_picker(window, cx);
                                        this.apply_selection(&target, None, cx);
                                    },
                                )),
                            ),
                        )
                    }),
            );
        let panel = div()
            .id("model-picker")
            .occlude()
            .flex()
            .flex_col()
            .w(t.component.model_picker)
            .overflow_hidden()
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.lg)
            .shadow(t.shadow_float())
            .font_family(t.font_sans.clone())
            .capture_key_down(cx.listener(Self::picker_key))
            .child(search)
            .child(body)
            .child(footer)
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

    /// The groups and models, in a list that scrolls inside the picker.
    fn render_picker_list(
        &self,
        groups: &[ModelGroupView],
        open: &OpenPicker,
        current: Option<&SelectionView>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let indent = t.space.sm + t.icon.dense + t.space.sm;
        let mut list = div()
            .id("model-list")
            .flex()
            .flex_col()
            .gap(t.border_width)
            .max_h(t.component.model_list_max)
            .p(t.space.xs + t.space.xxs)
            .overflow_y_scroll()
            .track_scroll(&open.scroll);
        for row in picker_rows(groups) {
            match row {
                PickerRow::Group(gi) => {
                    // The space between groups is a margin, so every header
                    // row has the height of the pinned header.
                    list =
                        list.child(group_header(&groups[gi], t).when(gi > 0, |d| d.mt(t.space.sm)));
                }
                PickerRow::Model(gi, mi, n) => {
                    let group = &groups[gi];
                    let model = &group.models[mi];
                    let selected = current.is_some_and(|s| {
                        s.account_label == group.account_label && s.model_id == model.id
                    });
                    let highlighted = open.highlighted == Some(n);
                    let range = model.effort.as_ref().map(EffortView::range_label);
                    list = list.child(
                        div()
                            .id(SharedString::from(format!(
                                "model-row-{}-{}",
                                group.account_label, model.id
                            )))
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(t.space.sm)
                            .py(t.space.xs + t.space.xxs)
                            .pl(indent)
                            .pr(t.space.sm)
                            .rounded(t.radius.md)
                            .cursor_pointer()
                            .when(selected && !highlighted, |d| d.bg(c.surface_selected))
                            .when(selected && highlighted, |d| d.bg(c.hover_selected))
                            .when(!selected && highlighted, |d| d.bg(c.hover_raised))
                            .when(!selected, |d| d.hover(|s| s.bg(c.hover_raised)))
                            .child(
                                line(
                                    model.name.clone(),
                                    if selected {
                                        t.text.ui_strong
                                    } else {
                                        t.text.ui
                                    },
                                    c.text_primary,
                                )
                                .flex_1(),
                            )
                            .when_some(range, |d, range| {
                                // `text.tertiary` never sits on `surface.selected`.
                                d.child(text(
                                    range,
                                    t.text.caption,
                                    if selected {
                                        c.text_secondary
                                    } else {
                                        c.text_tertiary
                                    },
                                ))
                            })
                            .child(div().flex_none().w(t.icon.dense).when(selected, |d| {
                                d.child(icon("check", t.icon.dense, c.accent_text))
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose_model_row(n, window, cx)
                            })),
                    );
                }
            }
        }
        // The list never stops at a hard edge (AGENTS.md rule 3): it fades
        // into the popover where more rows are above or below.
        let offset = open.scroll.offset();
        let max = open.scroll.max_offset();
        let above = offset.y < -Pixels::from(0.5);
        let below = -offset.y + Pixels::from(0.5) < max.y;
        let fade = t.space.xl + t.space.md;
        // The header of the group in view sticks under the search field.
        // The bounds are from the last frame, which is how the list lies now.
        let list_top = open.scroll.bounds().top();
        let rows = picker_rows(groups);
        let header_rows: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| matches!(row, PickerRow::Group(_)).then_some(i))
            .collect();
        let tops: Option<Vec<f32>> = header_rows
            .iter()
            .map(|row| {
                open.scroll
                    .bounds_for_item(*row)
                    .map(|b| f32::from(b.top() - list_top))
            })
            .collect();
        let height = header_rows
            .first()
            .and_then(|row| open.scroll.bounds_for_item(*row))
            .map(|b| f32::from(b.size.height));
        let pinned = tops
            .zip(height)
            .filter(|(tops, _)| tops.len() == groups.len())
            .and_then(|(tops, height)| pinned_header(&tops, height, f32::from(-offset.y)));
        let pinned_height = height.map(Pixels::from).unwrap_or_default();
        div()
            .relative()
            .flex()
            .flex_col()
            .min_h_0()
            // A header that the next one pushes up leaves at the top edge
            // of the list, not over the search field.
            .overflow_hidden()
            .child(list)
            .when(above, |d| {
                // The top fade starts under a pinned header, so the header
                // stays solid and the rows fade below it.
                let start = if pinned.is_some() {
                    pinned_height
                } else {
                    Pixels::ZERO
                };
                d.child(
                    div()
                        .absolute()
                        .top(start)
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
            .when_some(pinned, |d, (gi, y)| {
                d.child(
                    div()
                        .absolute()
                        .top(Pixels::from(y))
                        .left_0()
                        .right_0()
                        .px(t.space.xs + t.space.xxs)
                        .bg(c.surface_raised)
                        // Rows under the header take no hover or click.
                        .occlude()
                        .child(group_header(&groups[gi], t)),
                )
            })
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
                "No account is signed in. Add one in Settings > Workers.",
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

/// The header of a group: the provider logo, the account name and the
/// workers where the account is signed in.
fn group_header(group: &ModelGroupView, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(t.space.sm)
        .px(t.space.sm)
        .pt(t.space.xs)
        .pb(t.space.xs)
        .child(provider_logo(&group.logo, t.icon.dense, c.text_secondary))
        .child(line(group.account_label.clone(), t.text.label, c.text_secondary).flex_none())
        .child(line(
            group.workers.join(", "),
            t.text.caption,
            c.text_tertiary,
        ))
}

/// The group header that sticks to the top of the list, and where it sits.
///
/// `tops` are the tops of the group headers in the list content, in order,
/// and `height` is the height of a header. `scrolled` is how far the list
/// has scrolled down. The pinned header is the last one whose top has
/// scrolled past the top of the list. The next header pushes it up as it
/// arrives, so the returned offset is 0 or less. `None` when no header has
/// scrolled past the top, so the headers sit in the list as they are.
pub fn pinned_header(tops: &[f32], height: f32, scrolled: f32) -> Option<(usize, f32)> {
    let index = tops.iter().rposition(|top| *top < scrolled)?;
    let y = match tops.get(index + 1) {
        Some(next) => (next - scrolled - height).min(0.0),
        None => 0.0,
    };
    Some((index, y))
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
            .flex_1(),
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

    fn groups() -> Vec<ModelGroupView> {
        consort_client::mock::model_groups()
    }

    #[test]
    fn rows_put_each_group_header_before_its_models() {
        let groups = groups();
        let rows = picker_rows(&groups);
        assert_eq!(rows[0], PickerRow::Group(0));
        assert_eq!(rows[1], PickerRow::Model(0, 0, 0));
        let models = rows
            .iter()
            .filter(|r| matches!(r, PickerRow::Model(..)))
            .count();
        assert_eq!(models, model_count(&groups));
        assert_eq!(rows.len(), models + groups.len());
        // The second group's first model is the row after its header.
        let second = groups[0].models.len();
        assert_eq!(row_of_model(&groups, second), Some(second + 2));
        assert_eq!(nth_model(&groups, second), Some((1, 0)));
    }

    #[test]
    fn the_header_of_the_group_in_view_is_pinned_and_pushed_up_by_the_next() {
        // Three groups: headers at 6, 150 and 300, each 24 high.
        let tops = [6.0, 150.0, 300.0];
        // At the top, the headers sit in the list.
        assert_eq!(pinned_header(&tops, 24.0, 0.0), None);
        assert_eq!(pinned_header(&tops, 24.0, 6.0), None);
        // The first header has scrolled past the top: it is pinned.
        assert_eq!(pinned_header(&tops, 24.0, 7.0), Some((0, 0.0)));
        assert_eq!(pinned_header(&tops, 24.0, 100.0), Some((0, 0.0)));
        // The second header arrives and pushes the first one up.
        assert_eq!(pinned_header(&tops, 24.0, 136.0), Some((0, -10.0)));
        assert_eq!(pinned_header(&tops, 24.0, 149.0), Some((0, -23.0)));
        // Then it replaces it.
        assert_eq!(pinned_header(&tops, 24.0, 151.0), Some((1, 0.0)));
        // The last header stays pinned to the end.
        assert_eq!(pinned_header(&tops, 24.0, 900.0), Some((2, 0.0)));
        assert_eq!(pinned_header(&[], 24.0, 50.0), None);
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
