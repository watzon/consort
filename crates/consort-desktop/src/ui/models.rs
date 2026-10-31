//! Settings > Models: the fallback path (designs P2 and B2) and the lines of
//! a path report that the notices and the error card share (B4).
//!
//! The path is for every channel. The first row is the model of the channel
//! and cannot change here. The built-in row "Other accounts with the same
//! model" has a switch. The steps are the user's: they have a handle (drag,
//! or Alt with the up and down keys), a number, the model, its account and
//! effort, and a remove button. A right click on a step opens Move up, Move
//! down and Remove. The last row, "Stop and show the error", is fixed.

use consort_client::{
    FALLBACK_STEPS_MAX, FallbackPathView, FallbackStepView, PathLineView, PathOutcomeView, StepSkip,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Div, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Window, base::SelectableText, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, SettingsSection},
    fallback::{Direction, FallbackAction, StepDrag},
    ui::{
        admin_parts::{column, header, hint, page, section_title},
        menu::{MenuAction, MenuItem, OpenMenu},
        model_picker::PickerTarget,
        parts::{
            ButtonKind, button, detail, focusable, icon, icon_text_button, provider_logo, text,
        },
        settings::switch,
    },
};

/// What a dragged step shows under the pointer.
pub struct StepDragView(pub SharedString);

impl Render for StepDragView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = ConsortApp::tokens(cx);
        div()
            .flex()
            .items_center()
            .py(t.space.sm)
            .px(t.space.md)
            .rounded(t.radius.md)
            .bg(t.colors.surface_raised)
            .border(t.border_width)
            .border_color(t.colors.border_strong)
            .shadow(t.shadow_float())
            .font_family(t.font_sans.clone())
            .type_style(t.text.ui_strong)
            .text_color(t.colors.text_primary)
            .child(self.0.clone())
    }
}

/// The menu of a step row: Move up, Move down and Remove. The first step has
/// no Move up, and the last has no Move down: they show dimmed.
pub fn step_menu(index: usize, steps: usize) -> Vec<MenuItem> {
    vec![
        MenuItem::new(
            "Move up",
            MenuAction::Fallback(FallbackAction::MoveStep(index, Direction::Up)),
        )
        .icon("chevron-up")
        .disabled(index == 0),
        MenuItem::new(
            "Move down",
            MenuAction::Fallback(FallbackAction::MoveStep(index, Direction::Down)),
        )
        .icon("chevron-down")
        .disabled(index + 1 >= steps),
        MenuItem::new(
            "Remove",
            MenuAction::Fallback(FallbackAction::RemoveStep(index)),
        )
        .icon("close")
        .separated(),
    ]
}

/// The note of the built-in row: which other accounts have the model.
pub fn other_accounts_note(labels: &[String], model: &str) -> String {
    match labels {
        [] => {
            "No other account has this model now. The assistant moves to one when it has the model."
                .to_owned()
        }
        [one] => format!(
            "{one} has {model}. The assistant moves there first, with the same harness and the same model."
        ),
        [first @ .., last] => format!(
            "{} and {last} have {model}. The assistant moves there first, with the same harness and the same model.",
            first.join(", ")
        ),
    }
}

/// The dot of a path line: filled for a step that failed, a ring for a step
/// that Consort skipped, and filled green for the step that took the work.
pub fn path_dot(outcome: PathOutcomeView, t: &Tokens) -> Div {
    let c = &t.colors;
    let dot = div()
        .flex_none()
        .size(t.component.worker_dot)
        .rounded(t.radius.full);
    match outcome {
        PathOutcomeView::Failed => dot.bg(c.status_failed),
        PathOutcomeView::Chosen => dot.bg(c.status_done),
        PathOutcomeView::Skipped => dot
            .border(t.border_width + t.border_width)
            .border_color(c.text_tertiary),
    }
}

/// The lines of a path report, one for each step that Consort tried or
/// skipped. The words are error details, so the user can select them.
/// `key` makes the selection ids unique. With `place` on, a lane at the left
/// names the place ("First choice", "Step 2"), as in the error card.
pub fn path_lines(lines: &[PathLineView], key: &str, place: bool, t: &Tokens) -> Div {
    let c = &t.colors;
    let mut list = div().flex().flex_col().gap(t.space.sm);
    for (n, line) in lines.iter().enumerate() {
        let mut words = format!("{}, {}.", line.model, line.account);
        if let Some(effort) = &line.effort {
            words = format!("{}, {}, {effort}.", line.model, line.account);
        }
        words.push(' ');
        words.push_str(&line.detail);
        list = list.child(
            div()
                .flex()
                .items_start()
                .gap(t.space.sm + t.space.xxs)
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(t.text.ui.line)
                        .child(path_dot(line.outcome, t)),
                )
                .when(place, |d| {
                    d.child(
                        text(line.place.clone(), t.text.ui, c.text_tertiary)
                            .flex_none()
                            .w(t.component.fallback.place),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .type_style(t.text.ui)
                        .text_color(c.text_secondary)
                        .child(SelectableText::new(
                            SharedString::from(format!("{key}.line.{n}")),
                            words,
                        )),
                ),
        );
    }
    list
}

impl ConsortApp {
    pub fn render_models(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let path = self.state.fallback_path.clone();
        let intro = div()
            .flex()
            .flex_wrap()
            .items_baseline()
            .type_style(t.text.ui)
            .text_color(c.text_secondary)
            .child("Where the assistant goes when its model is not available. Sign-ins and API keys are in\u{a0}")
            .child(
                div()
                    .id("models-to-providers")
                    .cursor_pointer()
                    .text_color(c.text_primary)
                    .underline()
                    .child("Providers")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_settings(SettingsSection::Providers, cx)
                    })),
            )
            .child(".");
        page("models-page", t)
            .child(
                column(t)
                    .child(header("Models", intro, None, t))
                    .child(self.fallback_path_section(&path, t, cx))
                    .child(how_the_path_works(t))
                    .child(self.command_check_section(t, window, cx)),
            )
            .into_any_element()
    }

    fn fallback_path_section(
        &mut self,
        path: &FallbackPathView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let dragging = self.fallback_ui.dragging.filter(|_| cx.has_active_drag());
        let mut card = div()
            .id("fallback-path")
            .debug_selector(|| "fallback-path".into())
            .flex()
            .flex_col()
            .border(t.border_width)
            .border_color(c.border_subtle)
            .rounded(t.radius.lg)
            .overflow_hidden()
            .child(first_choice_row(path.first.as_ref(), t))
            .child(
                div()
                    .px(t.space.md)
                    .py(t.space.xs + t.space.xxs)
                    .mx(t.space.xs)
                    .rounded(t.radius.sm)
                    .bg(c.surface_card)
                    .child(text(
                        "If that is not available",
                        t.text.label,
                        c.text_secondary,
                    )),
            )
            .child(self.other_accounts_row(path, t, cx));
        let count = path.steps.len();
        for (index, step) in path.steps.iter().enumerate() {
            card = card.child(self.step_row(index, count, step, dragging, t, cx));
        }
        card = card.child(self.add_step_row(path, t, cx));
        card = card.child(end_row(t));
        div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section_title("Fallback path", t))
            .child(hint(
                "The assistant tries each step in order. This path is for every channel. Consort never moves to another harness unless you add a step for it.",
                t,
            ))
            .child(card)
    }

    fn other_accounts_row(
        &mut self,
        path: &FallbackPathView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let on = path.other_accounts;
        let model = path
            .first
            .as_ref()
            .map(|f| f.selection.model_name.clone())
            .unwrap_or_else(|| "its model".to_owned());
        lane_row(t)
            .child(lead(t).child(icon("sort", t.icon.size, c.text_secondary)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(text(
                        "Other accounts with the same model",
                        t.text.ui_strong,
                        c.text_primary,
                    ))
                    .child(hint(
                        other_accounts_note(&path.other_account_labels, &model),
                        t,
                    )),
            )
            .child(
                switch("fallback-other-accounts", on, t)
                    .debug_selector(|| "fallback-other-accounts".into())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_fallback(FallbackAction::SetOtherAccounts(!on), window, cx)
                    })),
            )
    }

    fn step_row(
        &mut self,
        index: usize,
        count: usize,
        step: &FallbackStepView,
        dragging: Option<usize>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let group = SharedString::from(format!("fallback-step-{index}"));
        let words = format!(
            "{}, {}",
            step.selection.model_name, step.selection.account_label
        );
        let key = format!(
            "{}\u{1f}{}",
            step.selection.account_label, step.selection.model_id
        );
        let id = SharedString::from(format!("fallback-step-{key}"));
        let lifted = dragging == Some(index);
        let accent = c.accent_text;
        let handle = div()
            .id(SharedString::from(format!("fallback-handle-{index}")))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(t.component.fallback.handle)
            .cursor_grab()
            .child(
                icon("more-vertical", t.icon.dense, c.text_tertiary)
                    .opacity(0.)
                    .group_hover(group.clone(), |s| s.opacity(1.)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.fallback_ui.dragging = Some(index);
                    cx.notify();
                }),
            )
            .on_drag(
                StepDrag {
                    from: index,
                    label: words.clone(),
                },
                |drag, _, _, cx| cx.new(|_| StepDragView(drag.label.clone().into())),
            );
        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(step_title(step, t))
            .when(step.skipped.is_some(), |d| {
                d.child(skipped_note(step.skipped, t))
            })
            .when(step.other_harness, |d| {
                d.child(harness_note(&step.company, t))
            });
        let remove = focusable(
            div().id(SharedString::from(format!("fallback-remove-{index}"))),
            t,
        )
        .debug_selector(move || format!("fallback-remove-{index}"))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(t.space.xl)
        .rounded(t.radius.md)
        .cursor_pointer()
        .hover(|s| s.bg(c.hover_card))
        .child(
            icon("close", t.icon.dense, c.text_secondary)
                .opacity(0.)
                .group_hover(group.clone(), |s| s.opacity(1.)),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            this.run_fallback(FallbackAction::RemoveStep(index), window, cx)
        }));
        let menu = step_menu(index, count);
        lane_row(t)
            .id(id)
            .debug_selector(move || format!("fallback-step-{index}"))
            .group(group)
            .border(t.border_width)
            .border_color(gpui_kit::transparent_black())
            .when(lifted, |d| d.border_dashed().border_color(c.border_strong))
            .drag_over::<StepDrag>(move |style, drag, _, _| {
                // The accent box shows where the step lands.
                let _ = drag;
                style.border_dashed().border_color(accent)
            })
            .on_drop(cx.listener(move |this, drag: &StepDrag, window, cx| {
                this.fallback_ui.dragging = None;
                let path = this.state.fallback_path.clone();
                let steps = crate::fallback::dropped(
                    crate::fallback::step_selections(&path),
                    drag.from,
                    index,
                );
                this.save_fallback_path(path.other_accounts, steps, cx);
                let _ = window;
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_menu(OpenMenu::at(event.position, menu.clone()), cx);
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if !event.keystroke.modifiers.alt {
                    return;
                }
                let direction = match event.keystroke.key.as_str() {
                    "up" => Direction::Up,
                    "down" => Direction::Down,
                    _ => return,
                };
                this.run_fallback(FallbackAction::MoveStep(index, direction), window, cx);
                cx.stop_propagation();
            }))
            .child(handle)
            .child(
                text(format!("{}", index + 1), t.text.ui, c.text_tertiary)
                    .flex_none()
                    .w(t.component.fallback.number),
            )
            .child(
                div()
                    .when(lifted, |d| d.opacity(0.))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_start()
                    .gap(t.space.md)
                    .child(provider_logo(&step.logo, t.icon.size, c.text_primary))
                    .child(body)
                    .child(effort_words(step, t))
                    .child(remove),
            )
            .into_any_element()
    }

    fn add_step_row(&mut self, path: &FallbackPathView, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let empty = path.steps.is_empty();
        let full = path.steps.len() >= FALLBACK_STEPS_MAX;
        let add = if empty {
            button("fallback-add", "Add a step", ButtonKind::Primary, t)
        } else {
            icon_text_button("fallback-add", "plus", "Add a step", t)
        }
        .relative()
        .debug_selector(|| "fallback-add".into())
        .child(self.triggers.track("fallback-add"))
        .on_click(cx.listener(|this, _, window, cx| {
            this.open_picker_under("fallback-add", PickerTarget::FallbackStep, window, cx)
        }));
        let mut row = div()
            .flex()
            .flex_col()
            .items_start()
            .gap(t.space.md)
            .py(t.space.md)
            .pl(t.space.lg + t.component.fallback.handle + t.component.fallback.number)
            .pr(t.space.lg);
        if empty {
            row = row.child(text(
                "No steps yet. Add a model for the assistant to use when the two choices above are not available.",
                t.text.ui,
                c.text_secondary,
            ));
        }
        if full {
            row.child(hint(
                format!("The path has {FALLBACK_STEPS_MAX} steps, which is the most. Remove a step to add another."),
                t,
            ))
        } else {
            row.child(add)
        }
    }
}

impl ConsortApp {
    /// The dialog that asks once, before the first step on another harness:
    /// what the other provider receives. The step waits in
    /// `fallback_ui.pending`.
    pub(crate) fn other_harness_step_dialog(
        &self,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> (String, String, gpui_kit::Pixels, Div, Div) {
        let c = &t.colors;
        let path = &self.state.fallback_path;
        let pending = self.fallback_ui.pending.clone().unwrap_or_default();
        let provider_of = |label: &str| {
            self.state
                .model_groups
                .iter()
                .find(|g| g.account_label == label)
                .map(|g| g.provider.clone())
                .unwrap_or_else(|| "another harness".to_owned())
        };
        let provider = provider_of(&pending.account_label);
        let company = consort_client::fallback::company_of_provider(&provider);
        let first = path.first.as_ref().map(|f| f.selection.clone());
        let first_provider = first
            .as_ref()
            .map(|f| provider_of(&f.account_label))
            .unwrap_or_else(|| "the first choice".to_owned());
        let description = format!(
            "{} on {} runs on {provider}. Your first choice runs on {first_provider}. When the assistant moves to this step, {company} receives:",
            pending.model_name, pending.account_label
        );
        let mut list = div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .p(t.space.md)
            .rounded(t.radius.md)
            .bg(c.surface_card);
        for item in [
            "The messages of the channel that the assistant needs to go on",
            "The files, command output and tool results that it reads at this step",
            "Your next messages, until the assistant goes back",
        ] {
            list = list.child(
                div()
                    .flex()
                    .items_start()
                    .gap(t.space.sm + t.space.xxs)
                    .child(
                        div().flex().items_center().h(t.text.ui.line).child(
                            div()
                                .size(t.component.voice.rule_dot)
                                .rounded(t.radius.full)
                                .bg(c.text_tertiary),
                        ),
                    )
                    .child(text(item, t.text.ui, c.text_primary).flex_1().min_w_0()),
            );
        }
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(list)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(t.space.sm)
                    .child(icon("info", t.icon.dense, c.accent_text))
                    .child(
                        text(
                            format!(
                                "{} is another account. Add this step only if that account may see work from every channel that uses this path.",
                                pending.account_label
                            ),
                            t.text.ui,
                            c.text_secondary,
                        )
                        .flex_1()
                        .min_w_0(),
                    ),
            )
            .child(text(
                format!(
                    "The approval mode does not change. If {provider} cannot keep it, Consort skips this step."
                ),
                t.text.ui,
                c.text_secondary,
            ));
        (
            "Add a step on another harness?".to_owned(),
            description,
            t.layout.dialog_width,
            body,
            self.dialog_actions("Add step", t, cx, |this, window, cx| {
                this.run_fallback(FallbackAction::ConfirmStep, window, cx)
            }),
        )
    }
}

/// A row of the card: the lane at the left, then the content.
fn lane_row(t: &Tokens) -> Div {
    div()
        .flex()
        .items_start()
        .gap(t.space.md)
        .py(t.space.md)
        .px(t.space.lg)
}

/// The lane of a built-in row: as wide as the handle and the number of a
/// step, so the logos line up.
fn lead(t: &Tokens) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_end()
        .w(t.component.fallback.handle + t.component.fallback.number)
        .h(t.text.ui_strong.line)
}

fn step_title(step: &FallbackStepView, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .items_baseline()
        .gap(t.space.sm)
        .min_w_0()
        .child(
            text(
                step.selection.model_name.clone(),
                t.text.ui_strong,
                c.text_primary,
            )
            .flex_none(),
        )
        .child(
            text(
                step.selection.account_label.clone(),
                detail(t),
                c.text_tertiary,
            )
            .truncate(),
        )
}

fn effort_words(step: &FallbackStepView, t: &Tokens) -> Div {
    text(
        step.effort_label.clone().unwrap_or_default(),
        detail(t),
        t.colors.text_tertiary,
    )
    .flex_none()
}

fn skipped_note(skipped: Option<StepSkip>, t: &Tokens) -> Div {
    let c = &t.colors;
    let words = skipped.map(StepSkip::label).unwrap_or_default();
    div()
        .flex()
        .items_center()
        .gap(t.space.xs + t.space.xxs)
        .child(path_dot(PathOutcomeView::Skipped, t))
        .child(text(words, detail(t), c.text_secondary))
}

/// The note under a step on another harness.
fn harness_note(company: &str, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .items_center()
        .gap(t.space.xs + t.space.xxs)
        .child(icon("info", t.icon.dense, c.accent_text))
        .child(
            text(
                format!("Another harness. The work and its context go to {company}."),
                detail(t),
                c.text_secondary,
            )
            .min_w_0(),
        )
}

/// The locked first row: the model of the channel.
fn first_choice_row(first: Option<&FallbackStepView>, t: &Tokens) -> Div {
    let c = &t.colors;
    let row = lane_row(t).items_center();
    match first {
        Some(step) => row
            .child(lead(t))
            .child(provider_logo(&step.logo, t.icon.size, c.text_primary))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(step_title(step, t))
                    .child(hint(
                        "First choice. This is the model of the channel. Change it in the composer.",
                        t,
                    )),
            )
            .child(effort_words(step, t))
            .child(icon("lock", t.icon.dense, c.text_tertiary)),
        None => row
            .child(lead(t))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(text("First choice", t.text.ui_strong, c.text_primary))
                    .child(hint(
                        "This is the model of the channel. Choose one in the composer.",
                        t,
                    )),
            )
            .child(icon("lock", t.icon.dense, c.text_tertiary)),
    }
}

/// The locked last row.
fn end_row(t: &Tokens) -> Div {
    let c = &t.colors;
    lane_row(t)
        .items_center()
        .border_t(t.border_width)
        .border_color(c.border_subtle)
        .child(lead(t).child(icon("failed", t.icon.size, c.status_failed)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(text("Stop and show the error", t.text.ui_strong, c.text_primary))
                .child(hint(
                    "When no step is available, the assistant stops. The error card lists what it tried.",
                    t,
                )),
        )
        .child(icon("lock", t.icon.dense, c.text_tertiary))
}

/// The table under the path: when it moves, when it does not, and where the
/// work runs.
fn how_the_path_works(t: &Tokens) -> Div {
    const ROWS: [(&str, &str); 5] = [
        (
            "It moves when",
            "An account reaches its usage limit, or a model provider is down or overloaded after Consort tried again.",
        ),
        (
            "It does not move when",
            "The sign-in of the first choice has expired. The error card says Sign in again.",
        ),
        (
            "It skips a step when",
            "The account is at its limit or signed out, or it cannot keep the approval mode. A step never lowers the approval mode.",
        ),
        (
            "It goes back when",
            "The limit resets. The assistant returns to the first choice on its own. A task stays where it moved, because it holds a running session.",
        ),
        (
            "Where work runs",
            "The channel tags still choose the worker of a task. An offline worker stops the work of that task. It does not change the model or the path.",
        ),
    ];
    let c = &t.colors;
    let mut card = div()
        .flex()
        .flex_col()
        .gap(t.space.lg)
        .p(t.space.lg)
        .border(t.border_width)
        .border_color(c.border_subtle)
        .rounded(t.radius.lg);
    for (label, words) in ROWS {
        card = card.child(
            div()
                .flex()
                .items_start()
                .gap(t.space.lg)
                .child(
                    text(label, t.text.ui_strong, c.text_primary)
                        .flex_none()
                        .w(t.component.settings_label),
                )
                .child(text(words, t.text.ui, c.text_secondary).flex_1().min_w_0()),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap(t.space.md)
        .child(section_title("How the path works", t))
        .child(card)
}
