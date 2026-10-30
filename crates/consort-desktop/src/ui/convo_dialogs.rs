//! The phase 4 dialogs: New channel (`EYQ-0`, `FWA-0`), New task
//! (`ARL-0`), the channel settings (`CYJ-0`), Rename group, Delete group
//! (`FWA-0`), Delete task, and About (`DWF-0`).

use consort_client::{
    ApprovalMode, ChannelKind, Isolation, NewChannel,
    convo::{
        RepoChoice, RunsOn, approval_caption, day_choices, placement_caption, start_caption,
        time_choices,
    },
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, Focusable as _, InteractiveElement, IntoElement, ParentElement,
    Pixels, SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Dialog, Field},
    convo::{ConvoAction, NEW_CHANNEL_ICONS},
    ui::{
        dialogs::{field_block, input_box},
        menu::{MenuAction, MenuItem, OpenMenu},
        model_picker::{PickerTarget, TriggerStyle},
        parts::{ButtonKind, button, detail, focusable, icon, line, tag_chip, text},
    },
};

/// The parts of a dialog: title, description, width, body, actions and the
/// tile before the title.
pub type DialogParts = (String, String, Pixels, Div, Div, Option<AnyElement>);

/// A box that looks like an input and opens a select menu.
fn select_field(
    id: &'static str,
    value: Div,
    app: &ConsortApp,
    t: &Tokens,
) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    focusable(div().id(id), t)
        .relative()
        .flex()
        .items_center()
        .justify_between()
        .gap(t.space.sm)
        .py(t.component.input_pad_y)
        .px(t.space.md)
        .bg(c.surface_card)
        .border(t.border_width)
        .border_color(c.border_strong)
        .rounded(t.radius.md)
        .cursor_pointer()
        .hover(|s| s.border_color(c.text_tertiary))
        .child(app.triggers.track(id))
        .child(value.min_w_0())
        .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary))
}

/// A segmented choice whose options can hold anything.
fn choice_bar(t: &Tokens) -> Div {
    div()
        .flex()
        .gap(t.space.xxs)
        .p(t.space.xxs)
        .bg(t.colors.surface_card)
        .rounded(t.radius.md)
}

fn choice(
    id: impl Into<SharedString>,
    active: bool,
    content: impl IntoElement,
    t: &Tokens,
) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    focusable(div().id(gpui_kit::ElementId::Name(id.into())), t)
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(t.space.sm)
        .py(t.space.xs)
        .px(t.space.md)
        .rounded(t.radius.sm)
        .cursor_pointer()
        .whitespace_nowrap()
        .type_style(if active { t.text.ui_strong } else { t.text.ui })
        .text_color(if active {
            c.text_primary
        } else {
            c.text_secondary
        })
        .when(active, |d| d.bg(c.surface_selected))
        .when(!active, |d| d.hover(|s| s.bg(c.hover_card)))
        .child(content)
}

/// A section title of the channel settings.
fn section(title: &str, t: &Tokens) -> Div {
    text(title.to_owned(), t.text.title, t.colors.text_primary)
}

/// The icon choices of the New channel dialog and the channel settings: the
/// seven of the design, and More with every icon.
fn icon_choices(
    current: &str,
    triggers: &crate::ui::menu::TriggerBounds,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> Div {
    let c = &t.colors;
    let v = t.component.convo;
    let mut row = div().flex().items_center().gap(t.space.xs + t.space.xxs);
    for name in NEW_CHANNEL_ICONS {
        let active = current == name;
        row = row.child(
            focusable(
                div().id(SharedString::from(format!("icon-choice-{name}"))),
                t,
            )
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(v.icon_choice)
            .rounded(t.radius.md)
            .border(t.border_width)
            .border_color(if active {
                c.text_secondary
            } else {
                c.border_subtle
            })
            .when(active, |d| d.bg(c.surface_selected))
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_raised))
            .child(icon(name, t.icon.size, c.text_secondary))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.run_convo(ConvoAction::PickIcon(name.to_owned()), window, cx)
            })),
        );
    }
    let custom = !NEW_CHANNEL_ICONS.contains(&current);
    row.child(
        focusable(div().id("icon-choice-more"), t)
            .relative()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(t.space.xs)
            .h(v.icon_choice)
            .rounded(t.radius.md)
            .border(t.border_width)
            .border_dashed()
            .border_color(if custom {
                c.text_secondary
            } else {
                c.border_strong
            })
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_raised))
            .child(triggers.track("icon-choice-more"))
            .when(custom && !current.is_empty(), |d| {
                d.child(icon(current, t.icon.size, c.text_secondary))
            })
            .child(text("More", t.text.ui_strong, c.text_secondary))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(bounds) = this.triggers.get("icon-choice-more") {
                    let current = if this.dialog == Some(Dialog::NewChannel) {
                        this.convo.new_channel_icon.clone()
                    } else {
                        this.convo.settings.icon.clone()
                    };
                    let items = crate::app::CHANNEL_ICONS
                        .iter()
                        .map(|(name, label)| {
                            MenuItem::new(
                                *label,
                                MenuAction::Convo(ConvoAction::PickIcon((*name).to_owned())),
                            )
                            .icon(name)
                            .checked(current == *name)
                        })
                        .collect();
                    this.open_menu(OpenMenu::under(bounds, items, false), cx);
                }
            })),
    )
}

impl ConsortApp {
    /// The parts of a phase 4 dialog. `None` when its subject is gone.
    pub fn convo_dialog(
        &mut self,
        dialog: &Dialog,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<DialogParts> {
        match dialog {
            Dialog::NewChannel => Some(self.new_channel_parts(t, window, cx)),
            Dialog::NewTask => Some(self.new_task_parts(t, window, cx)),
            Dialog::ChannelSettings(id) => self.channel_settings_parts(id, t, window, cx),
            Dialog::RenameGroup(id) => Some(self.rename_group_parts(id, t, window, cx)),
            Dialog::ConfirmDeleteGroup(id) => self.delete_group_parts(id, t, cx),
            Dialog::ConfirmDeleteTask(id) => self.delete_task_parts(id, t, cx),
            Dialog::About => Some(self.about_parts(t, cx)),
            Dialog::SkillDiff(id) => self.skill_diff_parts(id, t, cx),
            _ => None,
        }
    }

    fn new_channel_parts(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DialogParts {
        let c = &t.colors;
        let focused = self
            .inputs
            .name
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let group = self.convo.new_channel_group.clone();
        let group_name = group
            .as_ref()
            .and_then(|g| self.state.groups.iter().find(|x| &x.id == g))
            .map(|g| g.name.clone());
        let icon_now = self.convo.new_channel_icon.clone();
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.lg)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs + t.space.xxs)
                    .child(text("Icon", t.text.label, c.text_secondary))
                    .child(icon_choices(&icon_now, &self.triggers.clone(), t, cx)),
            )
            .child(field_block(
                "Name",
                input_box(&self.inputs.name, focused, t),
                Some("Short and in lower case works best. You can change it later."),
                t,
            ));
        let actions = self.dialog_actions("Create channel", t, cx, move |this, window, cx| {
            let name = this.field(Field::Name, cx);
            if name.is_empty() {
                this.dialog_error = Some("Enter a name for the channel.".into());
                cx.notify();
                return;
            }
            let client = this.client.clone();
            let before: Vec<String> = this.state.channels.iter().map(|c| c.id.clone()).collect();
            this.open_created = Some((name.clone(), before));
            let channel = NewChannel {
                name,
                group_id: this.convo.new_channel_group.clone(),
                icon: Some(this.convo.new_channel_icon.clone()),
                ..NewChannel::default()
            };
            this.submit_dialog(
                async move { client.create_channel(channel).await },
                window,
                cx,
            );
        });
        (
            "New channel".to_owned(),
            match group_name {
                Some(name) => format!("In {name}"),
                None => "Give it a name and an icon. Then the assistant helps you set it up in the channel.".to_owned(),
            },
            t.component.convo.dialog_small,
            body,
            actions,
            None,
        )
    }

    fn rename_group_parts(
        &mut self,
        id: &str,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DialogParts {
        let focused = self
            .inputs
            .name
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let id = id.to_owned();
        let actions = self.dialog_actions("Rename group", t, cx, move |this, window, cx| {
            let name = this.field(Field::Name, cx);
            if name.is_empty() {
                this.dialog_error = Some("Enter a name for the group.".into());
                cx.notify();
                return;
            }
            let client = this.client.clone();
            let group_id = id.clone();
            this.submit_dialog(
                async move {
                    client
                        .run(consort_client::Command::RenameGroup { group_id, name })
                        .await
                },
                window,
                cx,
            );
        });
        (
            "Rename group".to_owned(),
            "A group is a folder of channels in the sidebar.".to_owned(),
            t.component.convo.dialog_group,
            field_block("Name", input_box(&self.inputs.name, focused, t), None, t),
            actions,
            None,
        )
    }

    fn delete_group_parts(
        &mut self,
        id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<DialogParts> {
        let c = &t.colors;
        let group = self.state.groups.iter().find(|g| g.id == id)?.clone();
        let channels: Vec<String> = self
            .state
            .channels
            .iter()
            .filter(|ch| ch.group_id.as_deref() == Some(id))
            .map(|ch| ch.name.clone())
            .collect();
        let names: Vec<&str> = channels.iter().map(String::as_str).collect();
        let what = match channels.len() {
            0 => "The group has no channels.".to_owned(),
            1 => format!(
                "Its channel, {}, moves out of the group, to the channels with no group.",
                names[0]
            ),
            n => format!(
                "Its {n} channels, {}, move out of the group, to the channels with no group.",
                consort_client::convo::join_names(&names)
            ),
        };
        let group_id = group.id.clone();
        // A destructive button: Enter does not press it.
        let actions = self.dialog_actions_kind(
            "Delete group",
            ButtonKind::Danger,
            t,
            cx,
            move |this, window, cx| {
                let client = this.client.clone();
                let group_id = group_id.clone();
                this.submit_dialog(
                    async move {
                        client
                            .run(consort_client::Command::DeleteGroup { group_id })
                            .await
                    },
                    window,
                    cx,
                );
            },
        );
        Some((
            format!("Delete the group {}?", group.name),
            what,
            t.component.convo.dialog_group,
            div().child(text(
                "No channel, task, message or file is deleted.",
                t.text.ui,
                c.text_secondary,
            )),
            actions,
            None,
        ))
    }

    fn delete_task_parts(
        &mut self,
        id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<DialogParts> {
        let task = self.state.task(id)?.clone();
        let task_id = task.id.clone();
        let actions = self.dialog_actions_kind(
            "Delete task",
            ButtonKind::Danger,
            t,
            cx,
            move |this, window, cx| {
                let client = this.client.clone();
                let task_id = task_id.clone();
                if matches!(&this.panel, crate::app::Panel::Thread(open) if *open == task_id) {
                    this.panel = crate::app::Panel::None;
                }
                this.submit_dialog(
                    async move {
                        client
                            .run(consort_client::Command::DeleteTask { task_id })
                            .await
                    },
                    window,
                    cx,
                );
            },
        );
        Some((
            format!("Delete the task {}?", task.title),
            "Consort deletes the task, its thread and its workspace. You cannot undo this. To keep them, archive the task instead.".to_owned(),
            t.layout.dialog_width,
            div(),
            actions,
            None,
        ))
    }

    /// The footer of the New task dialog: what happens at the start, Cancel
    /// and the confirm button. Enter in the title field confirms.
    fn task_footer(&self, caption: String, label: &str, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let on_confirm: crate::app::DialogConfirm =
            std::rc::Rc::new(|this, window, cx| this.create_task(window, cx));
        self.dialog_confirm.replace(Some(on_confirm.clone()));
        let busy = self.dialog_busy();
        div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                text(caption, detail(t), c.text_secondary)
                    .flex_1()
                    .min_w_0(),
            )
            .child(
                button("dialog-cancel", "Cancel", ButtonKind::Secondary, t)
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            )
            .child(
                button("dialog-confirm", label.to_owned(), ButtonKind::Primary, t)
                    .debug_selector(|| "dialog-confirm".into())
                    .when(busy, |b| b.opacity(t.opacity.busy))
                    .on_click(cx.listener(move |this, _, window, cx| on_confirm(this, window, cx))),
            )
    }

    /// Sends the New task form.
    pub fn create_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self
            .convo_inputs
            .task_title
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let brief = self.convo_inputs.task_brief.read(cx).value().to_string();
        if title.is_empty() {
            self.dialog_error = Some("Give the task a title.".into());
            cx.notify();
            return;
        }
        if brief.trim().is_empty() {
            self.dialog_error = Some("Write the brief. Say what to do and what done means.".into());
            cx.notify();
            return;
        }
        let task = self.convo.new_task.task(title, brief);
        let client = self.client.clone();
        self.submit_dialog(
            async move { client.run(consort_client::Command::CreateTask(task)).await },
            window,
            cx,
        );
    }

    fn new_task_parts(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DialogParts {
        let c = &t.colors;
        let form = self.convo.new_task.clone();
        let channel = self
            .state
            .channel(&form.channel_id)
            .cloned()
            .unwrap_or_default();
        let channel_name = if channel.kind == ChannelKind::Tutti {
            crate::ui::TUTTI.to_owned()
        } else {
            channel.name.clone()
        };
        let title_focused = self
            .convo_inputs
            .task_title
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let brief_focused = self
            .convo_inputs
            .task_brief
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let brief = div()
            .min_h(t.component.convo.brief_box)
            .py(t.component.input_pad_y)
            .px(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(if brief_focused {
                c.accent_text
            } else {
                c.border_strong
            })
            .rounded(t.radius.md)
            .text_color(c.text_primary)
            .child(crate::ui::parts::textarea(
                &self.convo_inputs.task_brief,
                t.text.ui,
            ));

        // Channel and model.
        let channel_value = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .child(icon(&channel.icon, t.icon.dense, c.text_secondary))
            .child(line(channel_name.clone(), t.text.ui, c.text_primary));
        let channel_select = select_field("task-channel", channel_value, self, t).on_click(
            cx.listener(|this, _, _, cx| {
                if let Some(bounds) = this.triggers.get("task-channel") {
                    let current = this.convo.new_task.channel_id.clone();
                    let items = this
                        .state
                        .channels
                        .iter()
                        .map(|ch| {
                            let name = if ch.kind == ChannelKind::Tutti {
                                crate::ui::TUTTI.to_owned()
                            } else {
                                ch.name.clone()
                            };
                            MenuItem::new(
                                name,
                                MenuAction::Convo(ConvoAction::TaskChannel(ch.id.clone())),
                            )
                            .checked(ch.id == current)
                        })
                        .collect();
                    this.open_menu(OpenMenu::under(bounds, items, true), cx);
                }
            }),
        );
        let model = self.render_model_trigger(
            "task-model",
            PickerTarget::NewTask,
            TriggerStyle::Field,
            t,
            cx,
        );
        let effort = self.render_effort_trigger(
            "task-effort",
            PickerTarget::NewTask,
            TriggerStyle::Field,
            t,
            cx,
        );
        let row1 = div()
            .flex()
            .gap(t.space.md)
            .child(field_block("Channel", channel_select, None, t).w(t.space.page * 3.5))
            .child(
                field_block(
                    "Model and effort",
                    div()
                        .flex()
                        .gap(t.space.sm)
                        .child(div().flex_1().min_w_0().child(model))
                        .children(effort.map(|e| div().w(t.space.page * 2.5).child(e))),
                    None,
                    t,
                )
                .flex_1()
                .min_w_0(),
            );

        // Where it runs.
        let any = form.runs_on == RunsOn::AnyTagged;
        let tagged = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .child(if channel.tags.is_empty() {
                "Any worker"
            } else {
                "Any worker tagged"
            })
            .children(channel.tags.iter().map(|tag| tag_chip(tag, true, t)));
        let worker_name = match &form.runs_on {
            RunsOn::Worker(id) => self.state.worker(id).map(|w| w.name.clone()),
            RunsOn::AnyTagged => None,
        };
        let where_bar = choice_bar(t)
            .child(
                choice("runs-any", any, tagged, t).on_click(cx.listener(|this, _, _, cx| {
                    this.convo.new_task.runs_on = RunsOn::AnyTagged;
                    cx.notify();
                })),
            )
            .child(
                choice(
                    "runs-one",
                    !any,
                    match &worker_name {
                        Some(name) => format!("On {name}"),
                        None => "One worker".to_owned(),
                    },
                    t,
                )
                .relative()
                .child(self.triggers.track("runs-one"))
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(bounds) = this.triggers.get("runs-one") {
                        let current = match &this.convo.new_task.runs_on {
                            RunsOn::Worker(id) => Some(id.clone()),
                            RunsOn::AnyTagged => None,
                        };
                        let items = this
                            .state
                            .workers
                            .iter()
                            .map(|w| {
                                let label = if w.online {
                                    w.name.clone()
                                } else {
                                    format!("{}, offline", w.name)
                                };
                                MenuItem::new(
                                    label,
                                    MenuAction::Convo(ConvoAction::TaskWorker(w.id.clone())),
                                )
                                .checked(current.as_deref() == Some(w.id.as_str()))
                            })
                            .collect();
                        this.open_menu(OpenMenu::under(bounds, items, true), cx);
                    }
                })),
            );
        let caption = placement_caption(&self.state, &form.channel_id, &form.runs_on);

        // Repo and isolation.
        let repo_name = form
            .repo_id
            .as_ref()
            .and_then(|id| self.state.convo.repos.iter().find(|r| &r.id == id))
            .map(|r| r.name.clone());
        let repo_value = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .child(text("Repo", t.text.ui, c.text_tertiary))
            .child(
                line(
                    repo_name.unwrap_or_else(|| "No repo, a new folder".to_owned()),
                    t.text.mono,
                    c.text_primary,
                )
                .font_family(t.font_mono.clone()),
            );
        let repo_select = select_field("task-repo", repo_value, self, t)
            .flex_1()
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(bounds) = this.triggers.get("task-repo") {
                    let current = this.convo.new_task.repo_id.clone();
                    let channel_repos: Vec<String> = this
                        .state
                        .channel(&this.convo.new_task.channel_id)
                        .map(|c| c.repos.iter().map(|r| r.id.clone()).collect())
                        .unwrap_or_default();
                    let mut items: Vec<MenuItem> = this
                        .state
                        .convo
                        .repos
                        .iter()
                        .filter(|r| channel_repos.is_empty() || channel_repos.contains(&r.id))
                        .map(|r| {
                            MenuItem::new(
                                r.name.clone(),
                                MenuAction::Convo(ConvoAction::TaskRepo(Some(r.id.clone()))),
                            )
                            .checked(current.as_deref() == Some(r.id.as_str()))
                        })
                        .collect();
                    items.push(
                        MenuItem::new(
                            "No repo, a new folder",
                            MenuAction::Convo(ConvoAction::TaskRepo(None)),
                        )
                        .checked(current.is_none())
                        .separated(),
                    );
                    this.open_menu(OpenMenu::under(bounds, items, true), cx);
                }
            }));
        let mut isolation = choice_bar(t);
        for (value, label) in [
            (Isolation::InPlace, "In place"),
            (Isolation::Worktree, "Worktree"),
            (Isolation::Clone, "Clone"),
        ] {
            isolation = isolation.child(
                choice(
                    format!("task-iso-{label}"),
                    form.isolation == value,
                    label,
                    t,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.convo.new_task.isolation = value;
                    cx.notify();
                })),
            );
        }
        let row_repo = div()
            .flex()
            .gap(t.space.sm)
            .child(repo_select)
            .child(isolation);

        // Approval mode.
        let mut modes = choice_bar(t);
        for mode in ApprovalMode::ALL {
            modes = modes.child(
                choice(
                    format!("task-mode-{}", mode.label()),
                    form.approval_mode == mode,
                    mode.label(),
                    t,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.convo.new_task.approval_mode = mode;
                    cx.notify();
                })),
            );
        }

        // Start.
        let days = day_choices(consort_client::format::now_ms(), 14);
        let times = time_choices();
        let day_label = days
            .iter()
            .find(|d| d.1 == form.day)
            .map(|d| d.0.clone())
            .unwrap_or_default();
        let time_label = times
            .iter()
            .find(|x| x.1 == form.minutes)
            .map(|x| x.0.clone())
            .unwrap_or_default();
        let start_bar = choice_bar(t)
            .flex_none()
            .child(
                choice("start-now", !form.later, "Now", t).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.convo.new_task.later = false;
                        cx.notify();
                    },
                )),
            )
            .child(
                choice("start-later", form.later, "At a time", t).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.convo.new_task.later = true;
                        cx.notify();
                    },
                )),
            );
        let mut start_row = div().flex().gap(t.space.sm).child(start_bar);
        if form.later {
            let day_items: Vec<(String, i64)> = days.clone();
            start_row = start_row
                .child(
                    select_field(
                        "start-day",
                        line(day_label, t.text.ui, c.text_primary),
                        self,
                        t,
                    )
                    .flex_1()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(bounds) = this.triggers.get("start-day") {
                            let current = this.convo.new_task.day;
                            let items = day_items
                                .iter()
                                .map(|(label, day)| {
                                    MenuItem::new(
                                        label.clone(),
                                        MenuAction::Convo(ConvoAction::TaskDay(*day)),
                                    )
                                    .checked(*day == current)
                                })
                                .collect();
                            this.open_menu(OpenMenu::under(bounds, items, true), cx);
                        }
                    })),
                )
                .child(
                    select_field(
                        "start-time",
                        line(time_label, t.text.ui, c.text_primary),
                        self,
                        t,
                    )
                    .w(t.space.page * 2.25)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(bounds) = this.triggers.get("start-time") {
                            let current = this.convo.new_task.minutes;
                            let items = times
                                .iter()
                                .map(|(label, minutes)| {
                                    MenuItem::new(
                                        label.clone(),
                                        MenuAction::Convo(ConvoAction::TaskTime(*minutes)),
                                    )
                                    .checked(*minutes == current)
                                })
                                .collect();
                            this.open_menu(OpenMenu::under(bounds, items, true), cx);
                        }
                    })),
                );
        }

        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.lg)
            .child(field_block(
                "Title",
                input_box(&self.convo_inputs.task_title, title_focused, t),
                None,
                t,
            ))
            .child(field_block(
                "Brief",
                brief,
                Some("The task reads the brief first. Say what done means."),
                t,
            ))
            .child(row1)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.sm)
                    .child(field_block("Where it runs", where_bar, Some(&caption), t))
                    .child(row_repo),
            )
            .child(field_block(
                "Approval mode",
                modes,
                Some(approval_caption(form.approval_mode)),
                t,
            ))
            .child(field_block("Start", start_row, None, t));
        let start = if form.later {
            consort_client::convo::StartWhen::At(consort_client::convo::start_time(
                form.day,
                form.minutes,
            ))
        } else {
            consort_client::convo::StartWhen::Now
        };
        let footer = self.task_footer(
            start_caption(start, &channel_name),
            if form.later {
                "Schedule task"
            } else {
                "Start task"
            },
            t,
            cx,
        );
        (
            "New task".to_owned(),
            "A task runs in its own session and reports in its own thread.".to_owned(),
            t.layout.dialog_wide,
            body,
            footer,
            None,
        )
    }

    fn channel_settings_parts(
        &mut self,
        id: &str,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<DialogParts> {
        let c = &t.colors;
        let v = t.component.convo;
        let channel = self.state.channel(id)?.clone();
        let form = self.convo.settings.clone();
        let tutti = channel.kind == ChannelKind::Tutti;
        let name = if tutti {
            crate::ui::TUTTI.to_owned()
        } else {
            channel.name.clone()
        };
        let focused = |input: &gpui_kit::Entity<gpui_kit::component::input::InputState>,
                       cx: &mut Context<Self>| {
            input.read(cx).focus_handle(cx).is_focused(window)
        };
        let name_focused = focused(&self.convo_inputs.settings_name, cx);
        let purpose_focused = focused(&self.convo_inputs.settings_purpose, cx);
        let notes_focused = self
            .convo_inputs
            .settings_instructions
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);

        // General.
        let icon_button = focusable(div().id("settings-icon"), t)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(v.icon_choice)
            .rounded(t.radius.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_strong)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_card))
            .child(self.triggers.track("settings-icon"))
            .child(icon(&form.icon, t.icon.size, c.text_secondary))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(bounds) = this.triggers.get("settings-icon") {
                    let current = this.convo.settings.icon.clone();
                    let items = crate::app::CHANNEL_ICONS
                        .iter()
                        .map(|(n, label)| (*n, *label))
                        .chain(NEW_CHANNEL_ICONS.iter().map(|n| (*n, *n)))
                        .map(|(n, label)| {
                            let label = label.replace('-', " ");
                            let mut chars = label.chars();
                            let label = chars
                                .next()
                                .map(|f| f.to_uppercase().collect::<String>() + chars.as_str())
                                .unwrap_or_default();
                            MenuItem::new(
                                label,
                                MenuAction::Convo(ConvoAction::PickIcon(n.to_owned())),
                            )
                            .icon(n)
                            .checked(current == n)
                        })
                        .collect();
                    this.open_menu(OpenMenu::under(bounds, items, false), cx);
                }
            }));
        let instructions = div()
            .min_h(v.instructions_box)
            .py(t.component.input_pad_y)
            .px(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(if notes_focused {
                c.accent_text
            } else {
                c.border_strong
            })
            .rounded(t.radius.md)
            .text_color(c.text_primary)
            .child(crate::ui::parts::textarea(
                &self.convo_inputs.settings_instructions,
                t.text.ui,
            ));
        let general = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section("General", t))
            .child(
                div()
                    .flex()
                    .gap(t.space.sm)
                    .child(field_block("Icon", icon_button, None, t).flex_none())
                    .child(
                        field_block(
                            "Name",
                            input_box(&self.convo_inputs.settings_name, name_focused, t),
                            None,
                            t,
                        )
                        .flex_1(),
                    ),
            )
            .child(field_block(
                "Purpose",
                input_box(&self.convo_inputs.settings_purpose, purpose_focused, t),
                None,
                t,
            ))
            .child(field_block(
                "Instructions",
                instructions,
                Some(&format!(
                    "The assistant reads these before every turn in {name}."
                )),
                t,
            ));

        // Assistant.
        let model = self.render_model_trigger(
            "settings-model",
            PickerTarget::SettingsAssistant,
            TriggerStyle::Field,
            t,
            cx,
        );
        let effort = self.render_effort_trigger(
            "settings-effort",
            PickerTarget::SettingsAssistant,
            TriggerStyle::Field,
            t,
            cx,
        );
        let memory_on = form.memory;
        let assistant = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section("Assistant", t))
            .child(field_block(
                "Model and effort",
                div()
                    .flex()
                    .gap(t.space.sm)
                    .child(div().flex_1().min_w_0().child(model))
                    .children(effort.map(|e| div().w(t.space.page * 2.5).child(e))),
                None,
                t,
            ))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(t.space.md)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(text("Channel memory", t.text.ui_strong, c.text_primary))
                            .child(text(
                                format!(
                                    "The assistant keeps notes about {name} and reads them in every turn."
                                ),
                                detail(t),
                                c.text_tertiary,
                            )),
                    )
                    .child(
                        crate::ui::settings::switch("settings-memory", memory_on, t).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.convo.settings.memory = !this.convo.settings.memory;
                                cx.notify();
                            },
                        )),
                    ),
            );

        // Work: tags, repos, isolation, approval mode, local MCP servers.
        let tags = self.tags_field(&form.tags, t, cx);
        let mut repos = div()
            .flex()
            .flex_col()
            .rounded(t.radius.md)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .bg(c.surface_card);
        for (i, repo) in form.repos.iter().enumerate() {
            let (repo_name, repo_line) = match repo {
                RepoChoice::Existing(rid) => self
                    .state
                    .convo
                    .repos
                    .iter()
                    .chain(channel.repos.iter())
                    .find(|r| &r.id == rid)
                    .map(|r| (r.name.clone(), r.checkouts.clone()))
                    .unwrap_or_else(|| (rid.clone(), String::new())),
                RepoChoice::New(url) => (
                    url.clone(),
                    "Consort adds this repo when you save.".to_owned(),
                ),
            };
            let remove = repo.clone();
            repos = repos.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.sm)
                    .px(t.space.md)
                    .when(i > 0, |d| {
                        d.border_t(t.border_width).border_color(c.border_subtle)
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                line(repo_name, t.text.mono, c.text_primary)
                                    .font_family(t.font_mono.clone()),
                            )
                            .child(line(repo_line, detail(t), c.text_tertiary)),
                    )
                    .child(
                        crate::ui::parts::icon_button(
                            SharedString::from(format!("repo-remove-{i}")),
                            "x",
                            t.space.xl,
                            t.icon.dense,
                            c.text_secondary,
                            c.hover_card,
                            t,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.convo.settings.repos.retain(|r| *r != remove);
                            cx.notify();
                        })),
                    ),
            );
        }
        let has_repos = !form.repos.is_empty();
        if form.adding_repo {
            let url_focused = focused(&self.convo_inputs.repo_url, cx);
            repos = repos.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .p(t.space.sm)
                    .when(has_repos, |d| {
                        d.border_t(t.border_width).border_color(c.border_subtle)
                    })
                    .child(div().flex_1().min_w_0().child(input_box(
                        &self.convo_inputs.repo_url,
                        url_focused,
                        t,
                    )))
                    .child(
                        button("repo-add", "Add", ButtonKind::Secondary, t).on_click(
                            cx.listener(|this, _, window, cx| this.add_repo_url(window, cx)),
                        ),
                    )
                    .child(
                        div()
                            .relative()
                            .child(self.triggers.track("repo-known"))
                            .child(
                                button("repo-known", "Known repos", ButtonKind::Quiet, t).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(bounds) = this.triggers.get("repo-known") {
                                            let items: Vec<MenuItem> = this
                                                .state
                                                .convo
                                                .repos
                                                .iter()
                                                .map(|r| {
                                                    MenuItem::new(
                                                        r.name.clone(),
                                                        MenuAction::Convo(ConvoAction::AttachRepo(
                                                            r.id.clone(),
                                                        )),
                                                    )
                                                })
                                                .collect();
                                            if !items.is_empty() {
                                                this.open_menu(
                                                    OpenMenu::under(bounds, items, false),
                                                    cx,
                                                );
                                            }
                                        }
                                    }),
                                ),
                            ),
                    ),
            );
        } else {
            repos = repos.child(
                focusable(div().id("repo-add-row"), t)
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.sm)
                    .px(t.space.md)
                    .when(has_repos, |d| {
                        d.border_t(t.border_width).border_color(c.border_subtle)
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover_card))
                    .child(icon("plus", t.icon.dense, c.text_secondary))
                    .child(text("Add a repo", t.text.ui, c.text_secondary))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.convo.settings.adding_repo = true;
                        let input = this.convo_inputs.repo_url.clone();
                        input.update(cx, |input, cx| input.focus(window, cx));
                        cx.notify();
                    })),
            );
        }
        let mut isolation = choice_bar(t);
        for (value, label) in [
            (Isolation::InPlace, "In place"),
            (Isolation::Worktree, "Worktree"),
            (Isolation::Clone, "Clone"),
        ] {
            isolation = isolation.child(
                choice(
                    format!("settings-iso-{label}"),
                    form.isolation == value,
                    label,
                    t,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.convo.settings.isolation = value;
                    cx.notify();
                })),
            );
        }
        let mode_select = select_field(
            "settings-mode",
            line(form.approval_mode.label(), t.text.ui, c.text_primary),
            self,
            t,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            if let Some(bounds) = this.triggers.get("settings-mode") {
                let current = this.convo.settings.approval_mode;
                let items = ApprovalMode::ALL
                    .iter()
                    .map(|mode| {
                        MenuItem::new(
                            mode.label(),
                            MenuAction::Convo(ConvoAction::SettingsMode(*mode)),
                        )
                        .checked(*mode == current)
                    })
                    .collect();
                this.open_menu(OpenMenu::under(bounds, items, true), cx);
            }
        }));
        // Every local MCP server of every worker, with the workers that
        // have it.
        let mut servers: Vec<(String, Vec<String>)> = Vec::new();
        for worker in &self.state.workers {
            for server in &worker.local_mcp_servers {
                match servers.iter_mut().find(|(s, _)| s == server) {
                    Some((_, workers)) => workers.push(worker.name.clone()),
                    None => servers.push((server.clone(), vec![worker.name.clone()])),
                }
            }
        }
        for allowed in &form.servers {
            if !servers.iter().any(|(s, _)| s == allowed) {
                servers.push((allowed.clone(), Vec::new()));
            }
        }
        let mut server_list = div().flex().flex_col().gap(t.space.xs);
        if servers.is_empty() {
            server_list = server_list.child(text(
                "No worker reports a local MCP server.",
                detail(t),
                c.text_tertiary,
            ));
        }
        for (server, workers) in servers {
            let on = form.servers.contains(&server);
            let toggle = server.clone();
            let names: Vec<&str> = workers.iter().map(String::as_str).collect();
            server_list = server_list.child(
                focusable(div().id(SharedString::from(format!("server-{server}"))), t)
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.xxs)
                    .cursor_pointer()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(t.space.lg)
                            .rounded(t.radius.sm)
                            .border(t.border_width)
                            .when(on, |d| d.bg(c.accent_fill).border_color(c.accent_fill))
                            .when(!on, |d| d.border_color(c.border_strong))
                            .when(on, |d| d.child(icon("check", t.space.md, c.accent_on_fill))),
                    )
                    .child(
                        line(server.clone(), t.text.mono, c.text_primary)
                            .font_family(t.font_mono.clone())
                            .flex_1(),
                    )
                    .child(text(
                        if names.is_empty() {
                            "No worker has it now".to_owned()
                        } else {
                            format!("On {}", consort_client::convo::join_names(&names))
                        },
                        detail(t),
                        c.text_tertiary,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let list = &mut this.convo.settings.servers;
                        if let Some(at) = list.iter().position(|s| *s == toggle) {
                            list.remove(at);
                        } else {
                            list.push(toggle.clone());
                        }
                        cx.notify();
                    })),
            );
        }
        let (ext_line, ext_off) = consort_client::convo::extensions_summary(&self.state, id);
        let ext_id = id.to_owned();
        let extensions = div()
            .flex()
            .items_center()
            .gap(t.space.md)
            .p(t.space.md)
            .rounded(t.radius.md)
            .bg(c.surface_card)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(text(ext_line, t.text.ui, c.text_secondary))
                    .children(ext_off.map(|l| text(l, t.text.ui, c.text_secondary))),
            )
            .child(
                button(
                    "settings-extensions",
                    "Edit connectors and skills",
                    ButtonKind::Secondary,
                    t,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_dialog(Dialog::ChannelExtensions(ext_id.clone()), window, cx)
                })),
            );
        let work = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section("Work", t))
            .child(tags)
            .child(field_block("Repos", repos, None, t))
            .child(
                div()
                    .flex()
                    .gap(t.space.sm)
                    .child(field_block("Default isolation", isolation, None, t).flex_1())
                    .child(field_block("Default approval mode", mode_select, None, t).flex_1()),
            )
            .child(field_block(
                "Allowed local MCP servers",
                server_list,
                None,
                t,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.sm)
                    .pt(t.space.sm)
                    .child(section("Connectors and skills", t))
                    .child(extensions),
            );

        let body = div()
            .flex()
            .gap(v.settings_column_gap)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.xl)
                    .child(general)
                    .child(assistant),
            )
            .child(div().w(t.border_width).bg(c.border_subtle))
            .child(div().flex().flex_col().flex_1().min_w_0().child(work));

        // Footer: Archive and Delete at the left, Cancel and Save changes.
        let archive_id = id.to_owned();
        let delete_id = id.to_owned();
        let save: crate::app::DialogConfirm =
            std::rc::Rc::new(|this, window, cx| this.save_channel_settings(window, cx));
        self.dialog_confirm.replace(Some(save.clone()));
        let busy = self.dialog_busy();
        let footer =
            div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .pt(t.space.lg)
                .border_t(t.border_width)
                .border_color(c.border_subtle)
                .when(!tutti, |d| {
                    d.child(
                        button("settings-archive", "Archive channel", ButtonKind::Quiet, t)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_dialog(window, cx);
                                this.run_menu(
                                    MenuAction::ArchiveChannel(archive_id.clone()),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("settings-delete", "Delete channel", ButtonKind::Danger, t)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_dialog(
                                    Dialog::ConfirmDeleteChannel(delete_id.clone()),
                                    window,
                                    cx,
                                )
                            })),
                    )
                })
                .child(div().flex_1())
                .child(
                    button("dialog-cancel", "Cancel", ButtonKind::Secondary, t)
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                )
                .child(
                    button("dialog-confirm", "Save changes", ButtonKind::Primary, t)
                        .debug_selector(|| "dialog-confirm".into())
                        .when(busy, |b| b.opacity(t.opacity.busy))
                        .on_click(cx.listener(move |this, _, window, cx| save(this, window, cx))),
                );
        let lead = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(t.space.xxl + t.space.sm)
            .rounded(t.radius.lg)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_strong)
            .child(icon(&form.icon, t.icon.size, c.text_secondary))
            .into_any_element();
        Some((
            format!("{name} settings"),
            "How the assistant works in this channel. Files, routines and memory notes are in the channel panel.".to_owned(),
            v.dialog_settings,
            body,
            footer,
            Some(lead),
        ))
    }

    /// Sends the changed fields of the channel settings.
    pub fn save_channel_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .convo_inputs
            .settings_name
            .read(cx)
            .value()
            .trim()
            .to_owned();
        if name.is_empty() {
            self.dialog_error = Some("Enter a name for the channel.".into());
            cx.notify();
            return;
        }
        let Some((channel_id, changes)) = self.settings_changes(cx) else {
            self.close_dialog(window, cx);
            return;
        };
        if changes == consort_client::ChannelChanges::default() {
            self.close_dialog(window, cx);
            return;
        }
        let client = self.client.clone();
        self.submit_dialog(
            async move { client.update_channel(channel_id, changes).await },
            window,
            cx,
        );
    }

    /// The review of a new skill version: a diff of each changed file, and
    /// Update.
    fn skill_diff_parts(
        &mut self,
        id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<DialogParts> {
        let c = &t.colors;
        let skill = self.state.skill(id)?.clone();
        let commit = match &skill.update {
            consort_client::SkillUpdateView::Available { commit, .. } => commit.clone(),
            _ => None,
        };
        let mut body = div().flex().flex_col().gap(t.space.md);
        for (path, diff) in self.convo.skill_diff.clone() {
            let content: AnyElement = match diff {
                None => {
                    text("Loading both versions.", detail(t), c.text_tertiary).into_any_element()
                }
                Some(Err(message)) => text(message, detail(t), c.status_failed).into_any_element(),
                Some(Ok(lines)) => {
                    let mut rows = div().flex().flex_col().py(t.space.xs);
                    for line in &lines {
                        rows = rows.child(crate::ui::convo_item::diff_row(
                            line,
                            t.component.convo.line_number_col,
                            t,
                        ));
                    }
                    rows.into_any_element()
                }
            };
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(t.radius.md)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .bg(c.surface_canvas)
                    .overflow_hidden()
                    .child(
                        div()
                            .px(t.space.md)
                            .py(t.space.sm)
                            .border_b(t.border_width)
                            .border_color(c.border_subtle)
                            .child(
                                line(path.clone(), t.text.mono, c.text_primary)
                                    .font_family(t.font_mono.clone()),
                            ),
                    )
                    .child(content),
            );
        }
        let skill_id = skill.id.clone();
        let label = match &commit {
            Some(commit) => format!("Update to {commit}"),
            None => "Update".to_owned(),
        };
        let actions = self.dialog_actions(&label, t, cx, move |this, window, cx| {
            this.update_skill(
                skill_id.clone(),
                consort_client::SkillChanges {
                    accept_new_version: true,
                    ..Default::default()
                },
                cx,
            );
            this.close_dialog(window, cx);
        });
        Some((
            format!("Changes in the new version of {}", skill.name),
            "Read what changes before you update. Lines with + are new, and lines with - go away."
                .to_owned(),
            t.component.convo.dialog_settings,
            body,
            actions,
            None,
        ))
    }

    /// The footer row of the settings rail that opens About.
    pub fn about_row(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let sb = t.component.sidebar;
        let open = self.dialog == Some(Dialog::About);
        div()
            .py(t.space.sm)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                focusable(
                    crate::ui::sidebar::row(t)
                        .id("settings-about")
                        .debug_selector(|| "settings-about".into()),
                    t,
                )
                .cursor_pointer()
                .when(open, |d| d.bg(c.surface_selected))
                .hover(|s| s.bg(c.hover_sidebar))
                .child(icon("info", sb.row_icon, c.text_secondary))
                .child(text("About Consort", sb.row_text, c.text_secondary))
                .on_click(
                    cx.listener(|this, _, window, cx| this.open_dialog(Dialog::About, window, cx)),
                ),
            )
            .into_any_element()
    }

    fn about_parts(&mut self, t: &Tokens, cx: &mut Context<Self>) -> DialogParts {
        let c = &t.colors;
        let v = t.component.convo;
        let view = self.updates.view();
        let hub = &self.state.admin.hub;
        let host = hub
            .address
            .split("://")
            .nth(1)
            .unwrap_or(&hub.address)
            .trim_end_matches('/')
            .to_owned();
        let build = option_env!("CONSORT_BUILD").unwrap_or("dev");
        let mono = |s: String| div().font_family(t.font_mono.clone()).child(s);
        let connected = if host.is_empty() {
            div().child(match self.state.connection {
                consort_client::Connection::Demo => "Mock content, with no hub.".to_owned(),
                _ => "Not connected to a hub now.".to_owned(),
            })
        } else {
            div()
                .flex()
                .gap(t.space.xs)
                .child("Connected to")
                .child(mono(host))
                .when(!hub.hub_version.is_empty(), |d| {
                    d.child(format!(
                        ", hub {}, protocol {}",
                        hub.hub_version, hub.protocol
                    ))
                })
        };
        let ready = matches!(
            view.state,
            consort_client::update::UpdateState::Ready { .. }
        );
        let update_title = match &view.state {
            consort_client::update::UpdateState::Ready { version, .. } => {
                format!("Version {version} is ready")
            }
            _ => view.summary(),
        };
        let checked = match &view.state {
            consort_client::update::UpdateState::UpToDate { checked_at } => format!(
                "Checked {} at {}.",
                consort_client::format::day_label(*checked_at, consort_client::format::now_ms())
                    .to_lowercase(),
                consort_client::format::time_label(*checked_at)
            ),
            _ if view.auto => "Consort downloads new versions in the background.".to_owned(),
            _ => "Consort tells you about new versions and waits.".to_owned(),
        };
        let auto = view.auto;
        let updates = self.updates.clone();
        let update_box = div()
            .flex()
            .flex_col()
            .rounded(t.radius.lg)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .bg(c.surface_card)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .p(t.space.md)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(t.space.sm)
                                    .when(ready, |d| {
                                        d.child(
                                            div()
                                                .size(t.space.xs + t.space.xxs)
                                                .rounded(t.radius.full)
                                                .bg(c.status_working),
                                        )
                                    })
                                    .child(text(update_title, t.text.ui_strong, c.text_primary)),
                            )
                            .when(ready, |d| {
                                d.child(text(
                                    "It installs when Consort restarts. Tasks keep running on the workers.",
                                    detail(t),
                                    c.text_secondary,
                                ))
                            }),
                    )
                    .child(if ready {
                        button("about-restart", "Restart to install", ButtonKind::Secondary, t)
                            .debug_selector(|| "about-restart".into())
                            .on_click(cx.listener(|this, _, _, cx| this.restart_to_update(cx)))
                            .into_any_element()
                    } else {
                        let updates = updates.clone();
                        button("about-check", "Check now", ButtonKind::Secondary, t)
                            .on_click(move |_, _, _| updates.check_now())
                            .into_any_element()
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .p(t.space.md)
                    .border_t(t.border_width)
                    .border_color(c.border_subtle)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(text("Get updates on their own", t.text.ui, c.text_primary))
                            .child(text(checked, detail(t), c.text_secondary)),
                    )
                    .child(
                        crate::ui::settings::switch("about-auto", auto, t).on_click(cx.listener(
                            move |_, _, _, cx| {
                                updates.set_auto(!auto);
                                cx.notify();
                            },
                        )),
                    ),
            );
        let link = |id: &'static str, label: &'static str, url: String| {
            focusable(div().id(id), t)
                .px(t.space.sm)
                .py(t.space.xs)
                .rounded(t.radius.md)
                .cursor_pointer()
                .type_style(t.text.ui)
                .text_color(c.text_secondary)
                .hover(|s| s.bg(c.hover_raised).text_color(c.text_primary))
                .child(label)
                .on_click(move |_, _, cx| cx.open_url(&url))
        };
        let repo = consort_client::update::RELEASES_REPO;
        let body = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(t.space.lg)
            .child(crate::ui::mark::still_mark(v.about_mark, t))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(t.space.xs)
                    .child(
                        div()
                            .type_style(v.wordmark)
                            .text_color(c.text_primary)
                            .child("consort"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(t.space.xs)
                            .type_style(t.text.ui)
                            .text_color(c.text_secondary)
                            .child("Version")
                            .child(mono(format!("{},", env!("CARGO_PKG_VERSION"))))
                            .child(format!("build {build}")),
                    )
                    .child(
                        div()
                            .type_style(detail(t))
                            .text_color(c.text_secondary)
                            .child(connected),
                    ),
            )
            .child(div().w_full().child(update_box))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(t.space.xs)
                    .child(
                        div()
                            .flex()
                            .gap(t.space.xs)
                            .child(link(
                                "about-notes",
                                "Release notes",
                                format!("https://github.com/{repo}/releases"),
                            ))
                            .child(link(
                                "about-docs",
                                "Documentation",
                                format!("https://github.com/{repo}/tree/main/docs"),
                            ))
                            .child(link(
                                "about-source",
                                "Source code",
                                format!("https://github.com/{repo}"),
                            ))
                            .child(link(
                                "about-licenses",
                                "Licenses",
                                format!("https://github.com/{repo}/blob/main/LICENSE"),
                            )),
                    )
                    .child(text(
                        "Links open in your browser.",
                        detail(t),
                        c.text_tertiary,
                    )),
            );
        (
            String::new(),
            String::new(),
            v.dialog_about,
            body,
            div(),
            None,
        )
    }
}
