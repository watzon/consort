//! Dialogs: New channel, Add worker, Rename, Instructions, tags and the
//! confirmations. 520 to 560 px wide, `xl` corners, `shadow.float`, over the
//! scrim.

use consort_client::{ChannelChanges, Isolation, NewChannel, Os};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, Entity, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window,
    component::input::InputState, div, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Dialog, Field},
    notice::{Notice, NoticeKind},
    ui::{
        menu::{MenuAction, MenuItem, OpenMenu},
        parts::{
            ButtonKind, button, detail, error_box, focusable, icon, icon_button, selectable, text,
        },
        settings::segmented,
    },
};

/// A labelled field.
fn field_block(label: &str, body: impl IntoElement, hint: Option<&str>, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .flex_col()
        .gap(t.space.xs + t.space.xxs)
        .child(text(label.to_owned(), t.text.label, c.text_secondary))
        .child(body)
        .when_some(hint, |d, hint| {
            d.child(text(hint.to_owned(), detail(t), c.text_tertiary))
        })
}

/// A one-line text input in the dialog style.
pub fn input_box(input: &Entity<InputState>, focused: bool, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .py(t.component.input_pad_y)
        .px(t.space.md)
        .bg(c.surface_card)
        .border(t.border_width)
        .border_color(if focused {
            c.accent_text
        } else {
            c.border_strong
        })
        .rounded(t.radius.md)
        .type_style(t.text.ui)
        .text_color(c.text_primary)
        .child(crate::ui::parts::input(input, t.text.ui))
}

/// A box that looks like an input and opens a select menu.
fn select_box(
    id: &'static str,
    value: String,
    app: &ConsortApp,
    t: &Tokens,
) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    focusable(div().id(id), t)
        .relative()
        .flex()
        .items_center()
        .justify_between()
        .py(t.component.input_pad_y)
        .pl(t.space.md)
        .pr(t.space.sm + t.space.xxs)
        .bg(c.surface_card)
        .border(t.border_width)
        .border_color(c.border_strong)
        .rounded(t.radius.md)
        .cursor_pointer()
        .hover(|s| s.border_color(c.text_tertiary))
        .child(app.triggers.track(id))
        .child(text(value, t.text.ui, c.text_primary))
        .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary))
}

/// The items of the Group select in the New channel dialog: every group, then
/// No group and New group. A hub with no group still gets two items.
pub fn group_pick_items(
    groups: &[consort_client::GroupView],
    current: Option<&str>,
    new_group: bool,
) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = groups
        .iter()
        .map(|g| {
            MenuItem::new(
                g.name.clone(),
                MenuAction::Select("new-channel-group", g.id.clone()),
            )
            .checked(!new_group && current == Some(g.id.as_str()))
        })
        .collect();
    let in_known_group = current.is_some_and(|c| groups.iter().any(|g| g.id == c));
    let mut none = MenuItem::new(
        "No group",
        MenuAction::Select("new-channel-group", NO_GROUP.to_owned()),
    )
    .checked(!new_group && !in_known_group);
    if !items.is_empty() {
        none = none.separated();
    }
    items.push(none);
    items.push(
        MenuItem::new(
            "New group",
            MenuAction::Select("new-channel-group", NEW_GROUP.to_owned()),
        )
        .checked(new_group),
    );
    items
}

/// The select option that puts the new channel in no group. It is not a
/// group ID, so it cannot be a group.
pub const NO_GROUP: &str = "no-group";
/// The select option that makes a group for the new channel.
pub const NEW_GROUP: &str = "new-group";

impl ConsortApp {
    /// The toast at the bottom. Its text is selectable. An error has a mark
    /// and stays until the user dismisses it. An info notice times out. The
    /// text scrolls when it is long, so the close button never leaves the
    /// window. A message that the hub refused has a button that puts its text
    /// back in the message box. More errors wait behind this one.
    pub fn render_notice(&self, notice: Notice, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let is_error = notice.kind == NoticeKind::Error;
        let waiting = self.notices.waiting();
        let failure = notice.failure.clone();
        let serial = notice.serial;
        let text_block = div()
            .id(SharedString::from(format!("notice-scroll-{serial}")))
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .flex_1()
            .min_w_0()
            .max_h(t.layout.error_text_max_height)
            .overflow_y_scroll()
            .child(selectable(
                SharedString::from(format!("notice-text-{serial}")),
                notice.message,
                t.text.ui,
                c.text_primary,
            ))
            .when_some(notice.detail, |d, body| {
                d.child(
                    div()
                        .p(t.space.sm)
                        .rounded(t.radius.md)
                        .bg(c.surface_card)
                        .child(selectable(
                            SharedString::from(format!("notice-detail-{serial}")),
                            body,
                            t.text.ui,
                            c.text_secondary,
                        )),
                )
            });
        div()
            .absolute()
            .bottom(t.space.xl)
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .px(t.space.xl)
            .child(
                div()
                    .id("notice")
                    .flex()
                    .flex_col()
                    .gap(t.space.sm)
                    .w_full()
                    .max_w(t.layout.timeline_max_width)
                    .py(t.space.sm)
                    .pl(t.space.md)
                    .pr(t.space.xs)
                    .bg(c.surface_raised)
                    .border(t.border_width)
                    .border_color(if is_error {
                        c.status_failed
                    } else {
                        c.border_strong
                    })
                    .rounded(t.radius.lg)
                    .shadow(t.shadow_float())
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(t.space.sm)
                            .when(is_error, |d| {
                                d.child(div().flex_none().pt(t.space.xxs).child(icon(
                                    "circle-alert",
                                    t.icon.size,
                                    c.status_failed,
                                )))
                            })
                            .child(text_block)
                            .child(
                                icon_button(
                                    "notice-close",
                                    "x",
                                    t.space.xl,
                                    t.icon.dense,
                                    c.text_secondary,
                                    c.hover_raised,
                                    t,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.dismiss_notice(cx))),
                            ),
                    )
                    .when(failure.is_some() || waiting > 0, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(t.space.md)
                                .pr(t.space.sm)
                                .when_some(failure, |d, id| {
                                    d.child(
                                        button(
                                            "notice-restore",
                                            "Put text back in the box",
                                            ButtonKind::Secondary,
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.restore_failed_text(&id, window, cx)
                                            }),
                                        ),
                                    )
                                })
                                .child(div().flex_1())
                                .when(waiting > 0, |d| {
                                    d.child(text(
                                        if waiting == 1 {
                                            "1 more error waits".to_owned()
                                        } else {
                                            format!("{waiting} more errors wait")
                                        },
                                        detail(t),
                                        c.text_tertiary,
                                    ))
                                }),
                        )
                    }),
            )
            .into_any_element()
    }

    pub fn render_dialog(
        &mut self,
        dialog: &Dialog,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let (title, description, width, body, actions) = match dialog {
            Dialog::NewChannel => self.new_channel_dialog(t, window, cx),
            Dialog::AddWorker => self.add_worker_dialog(t, cx),
            Dialog::RenameChannel(id) => {
                let id = id.clone();
                let focused = self
                    .inputs
                    .name
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
                (
                    "Rename channel".to_owned(),
                    "The name shows in the sidebar and in Everything.".to_owned(),
                    t.layout.dialog_width,
                    field_block("Name", input_box(&self.inputs.name, focused, t), None, t),
                    self.dialog_actions("Rename", t, cx, move |this, window, cx| {
                        let name = this.field(Field::Name, cx);
                        if name.is_empty() {
                            this.dialog_error = Some("Enter a name for the channel.".into());
                            cx.notify();
                            return;
                        }
                        let client = this.client.clone();
                        let id = id.clone();
                        this.submit_dialog(
                            async move {
                                client
                                    .update_channel(
                                        id,
                                        ChannelChanges {
                                            name: Some(name),
                                            ..ChannelChanges::default()
                                        },
                                    )
                                    .await
                            },
                            window,
                            cx,
                        );
                    }),
                )
            }
            Dialog::Instructions(id) => {
                let id = id.clone();
                let name = self
                    .state
                    .channel(&id)
                    .map(|ch| ch.name.clone())
                    .unwrap_or_default();
                (
                    format!("Instructions for {name}"),
                    "Standing guidance that Consort reads before every turn in this channel. Write it in Markdown.".to_owned(),
                    t.layout.dialog_wide,
                    div()
                        .min_h(t.component.notes_min_height)
                        .py(t.space.sm)
                        .px(t.space.md)
                        .bg(c.surface_card)
                        .border(t.border_width)
                        .border_color(c.border_strong)
                        .rounded(t.radius.md)
                        .type_style(t.text.ui)
                        .child(
                            crate::ui::parts::textarea(&self.inputs.notes, t.text.ui),
                        ),
                    self.dialog_actions("Save instructions", t, cx, move |this, window, cx| {
                        let notes = this.inputs.notes.read(cx).value().to_string();
                        let client = this.client.clone();
                        let id = id.clone();
                        this.submit_dialog(
                            async move {
                                client
                                    .update_channel(id, ChannelChanges { instructions: Some(notes), ..ChannelChanges::default() })
                                    .await
                            },
                            window,
                            cx,
                        );
                    }),
                )
            }
            Dialog::RenameWorker(id) => {
                let id = id.clone();
                let focused = self
                    .inputs
                    .name
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
                (
                    "Rename worker".to_owned(),
                    "The name shows in the sidebar, on task cards and in Settings.".to_owned(),
                    t.layout.dialog_width,
                    field_block("Name", input_box(&self.inputs.name, focused, t), None, t),
                    self.dialog_actions("Rename", t, cx, move |this, window, cx| {
                        let name = this.field(Field::Name, cx);
                        if name.is_empty() {
                            this.dialog_error = Some("Enter a name for the worker.".into());
                            cx.notify();
                            return;
                        }
                        let client = this.client.clone();
                        let id = id.clone();
                        this.submit_dialog(
                            async move { client.rename_worker(id, name).await },
                            window,
                            cx,
                        );
                    }),
                )
            }
            Dialog::EditTags(id) => {
                let id = id.clone();
                let focused = self
                    .inputs
                    .name
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
                (
                    "Edit tags".to_owned(),
                    "Channels and tasks can ask for a worker with a tag, such as gpu.".to_owned(),
                    t.layout.dialog_width,
                    field_block(
                        "Tags",
                        input_box(&self.inputs.name, focused, t),
                        Some(
                            "Separate tags with commas. Use lowercase letters, digits and hyphens.",
                        ),
                        t,
                    ),
                    self.dialog_actions("Save tags", t, cx, move |this, window, cx| {
                        let tags: Vec<String> = this
                            .field(Field::Name, cx)
                            .split(',')
                            .map(|l| l.trim().to_owned())
                            .filter(|l| !l.is_empty())
                            .collect();
                        let client = this.client.clone();
                        let id = id.clone();
                        this.submit_dialog(
                            async move { client.set_worker_tags(id, tags).await },
                            window,
                            cx,
                        );
                    }),
                )
            }
            Dialog::NewGroup(channel) => {
                let channel = channel.clone();
                let focused = self
                    .inputs
                    .name
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
                (
                    "New group".to_owned(),
                    "A group is a named folder of channels in the sidebar.".to_owned(),
                    t.layout.dialog_width,
                    field_block("Name", input_box(&self.inputs.name, focused, t), None, t),
                    self.dialog_actions("Create group", t, cx, move |this, window, cx| {
                        let name = this.field(Field::Name, cx);
                        if name.is_empty() {
                            this.dialog_error = Some("Enter a name for the group.".into());
                            cx.notify();
                            return;
                        }
                        let client = this.client.clone();
                        let channel = channel.clone();
                        this.submit_dialog(
                            async move { client.create_group(name, channel).await },
                            window,
                            cx,
                        );
                    }),
                )
            }
            Dialog::ConfirmDeleteChannel(id) => {
                let id = id.clone();
                let name = self
                    .state
                    .channel(&id)
                    .map(|ch| ch.name.clone())
                    .unwrap_or_default();
                (
                    format!("Delete {name}?"),
                    "Consort deletes its messages, tasks, memory and files. You cannot undo this. To keep them, archive the channel instead. An archived channel waits in the Archived list in the sidebar, and you can bring it back.".to_owned(),
                    t.layout.dialog_width,
                    div(),
                    self.dialog_actions_kind("Delete channel", ButtonKind::Danger, t, cx, move |this, window, cx| {
                        let client = this.client.clone();
                        let id = id.clone();
                        this.submit_dialog(
                            async move { client.delete_channel(id).await },
                            window,
                            cx,
                        );
                    }),
                )
            }
            Dialog::ConfirmRemoveWorker(id) => {
                let id = id.clone();
                let name = self
                    .state
                    .worker(&id)
                    .map(|w| w.name.clone())
                    .unwrap_or_default();
                (
                    format!("Remove {name}?"),
                    "The hub stops trusting this worker. Its tasks fail, and it must pair again to come back.".to_owned(),
                    t.layout.dialog_width,
                    div(),
                    self.dialog_actions_kind("Remove worker", ButtonKind::Danger, t, cx, move |this, window, cx| {
                        let client = this.client.clone();
                        let id = id.clone();
                        this.submit_dialog(
                            async move {
                                client.run(consort_client::Command::RemoveWorker { worker_id: id }).await
                            },
                            window,
                            cx,
                        );
                    }),
                )
            }
            Dialog::ConfirmOpenLink(url) => {
                let url = url.clone();
                let open = url.clone();
                (
                    "Open this link?".to_owned(),
                    "The words of this link do not match where it goes. It goes to this address."
                        .to_owned(),
                    t.layout.dialog_width,
                    div()
                        .p(t.space.md)
                        .bg(c.surface_card)
                        .border(t.border_width)
                        .border_color(c.border_subtle)
                        .rounded(t.radius.md)
                        .font_family(t.font_mono.clone())
                        .child(selectable(
                            SharedString::from(format!("link-url-{url}")),
                            url,
                            t.text.mono,
                            c.text_primary,
                        )),
                    self.dialog_actions("Open link", t, cx, move |this, window, cx| {
                        cx.open_url(&open);
                        this.close_dialog(window, cx);
                    }),
                )
            }
        };
        let max_height = window.viewport_size().height - t.space.xxl * 2.0;
        let card = div()
            .id("dialog")
            .occlude()
            .max_h(max_height)
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(t.space.lg + t.space.xxs)
            .w(width)
            .p(t.space.xl)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.xl)
            .shadow(t.shadow_float())
            .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation()
            })
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
                            .gap(t.space.xs)
                            .child(text(title, t.text.heading, c.text_primary))
                            .child(text(description, t.text.ui, c.text_secondary)),
                    )
                    .child(
                        icon_button(
                            "dialog-close",
                            "x",
                            t.space.xl + t.space.xs,
                            t.icon.size,
                            c.text_secondary,
                            c.hover_raised,
                            t,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                    ),
            )
            .child(body)
            .children(
                self.dialog_error.clone().map(|err| {
                    error_box(SharedString::from(format!("dialog-error-{err}")), err, t)
                }),
            )
            .child(actions);
        div()
            .id("scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(c.scrim)
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_dialog(window, cx)),
            )
            .child(card)
            .into_any_element()
    }

    fn dialog_actions(
        &self,
        label: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
        on_confirm: impl Fn(&mut ConsortApp, &mut Window, &mut Context<ConsortApp>) + 'static,
    ) -> Div {
        self.dialog_actions_kind(label, ButtonKind::Primary, t, cx, on_confirm)
    }

    fn dialog_actions_kind(
        &self,
        label: &str,
        kind: ButtonKind,
        t: &Tokens,
        cx: &mut Context<Self>,
        on_confirm: impl Fn(&mut ConsortApp, &mut Window, &mut Context<ConsortApp>) + 'static,
    ) -> Div {
        let c = &t.colors;
        let busy = self.dialog_busy();
        let danger_hover = t.opacity.danger_hover;
        // Enter in a field of the dialog does what the confirm button does.
        // A button that destroys something takes a click, not a key press
        // from a field.
        let on_confirm: crate::app::DialogConfirm = std::rc::Rc::new(on_confirm);
        if kind != ButtonKind::Danger {
            self.dialog_confirm.replace(Some(on_confirm.clone()));
        }
        let confirm = button("dialog-confirm", label.to_owned(), kind, t)
            .when(kind == ButtonKind::Danger, |b| {
                b.bg(c.status_failed)
                    .text_color(c.on_failed)
                    .hover(|s| s.bg(c.status_failed).opacity(danger_hover))
            })
            .when(busy, |b| b.opacity(t.opacity.busy))
            .px(t.space.md + t.space.xxs)
            .on_click(cx.listener(move |this, _, window, cx| on_confirm(this, window, cx)));
        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                button("dialog-cancel", "Cancel", ButtonKind::Quiet, t)
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            )
            .child(confirm)
    }

    fn new_channel_dialog(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (String, String, gpui_kit::Pixels, Div, Div) {
        let c = &t.colors;
        let state = self.state.clone();
        let form = self.new_channel.clone();
        let name_focused = self
            .inputs
            .name
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let purpose_focused = self
            .inputs
            .purpose
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let group_name = if form.new_group {
            "New group".to_owned()
        } else {
            form.group_id
                .as_ref()
                .and_then(|g| state.groups.iter().find(|x| &x.id == g))
                .map(|g| g.name.clone())
                .unwrap_or_else(|| "No group".to_owned())
        };
        let groups = state.groups.clone();
        let current_group = form.group_id.clone();
        let new_group = form.new_group;
        let group_select = select_box("new-channel-group", group_name, self, t).on_click(
            cx.listener(move |this, _, _, cx| {
                if let Some(bounds) = this.triggers.get("new-channel-group") {
                    let items = group_pick_items(&groups, current_group.as_deref(), new_group);
                    this.open_menu(OpenMenu::under(bounds, items, true), cx);
                }
            }),
        );
        let group_name_focused = self
            .inputs
            .group_name
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let mut chips = div().flex().flex_wrap().gap(t.space.xs + t.space.xxs);
        for (i, worker_id) in form.workers.iter().enumerate() {
            let name = state
                .worker(worker_id)
                .map(|w| w.name.clone())
                .unwrap_or_default();
            let remove = worker_id.clone();
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("chip-{worker_id}")))
                    .flex()
                    .items_center()
                    .gap(t.space.xs + t.space.xxs)
                    .py(t.space.xs)
                    .px(t.space.sm + t.space.xxs)
                    .rounded(t.radius.full)
                    .bg(c.surface_selected)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .cursor_pointer()
                    .child(text((i + 1).to_string(), detail(t), c.text_secondary))
                    .child(text(name, t.text.ui, c.text_primary))
                    .child(icon("x", t.space.md, c.text_secondary))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.new_channel.workers.retain(|w| *w != remove);
                        cx.notify();
                    })),
            );
        }
        let free: Vec<(String, String)> = state
            .workers
            .iter()
            .filter(|w| !form.workers.contains(&w.id))
            .map(|w| (w.id.clone(), w.name.clone()))
            .collect();
        if !free.is_empty() {
            chips = chips.child(
                div()
                    .id("new-channel-worker")
                    .relative()
                    .flex()
                    .items_center()
                    .py(t.space.xs)
                    .px(t.space.sm + t.space.xxs)
                    .rounded(t.radius.full)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .border_dashed()
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover_raised))
                    .child(self.triggers.track("new-channel-worker"))
                    .child(text("Add worker", t.text.ui, c.text_secondary))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(bounds) = this.triggers.get("new-channel-worker") {
                            let items = free
                                .iter()
                                .map(|(id, name)| {
                                    MenuItem::new(
                                        name.clone(),
                                        MenuAction::Select("new-channel-worker", id.clone()),
                                    )
                                })
                                .collect();
                            this.open_menu(OpenMenu::under(bounds, items, false), cx);
                        }
                    })),
            );
        }
        let selections = state.selections.clone();
        let current_selection = form.selection.clone();
        let selection_select = select_box(
            "new-channel-selection",
            form.selection
                .clone()
                .unwrap_or_else(|| "Choose a harness".to_owned()),
            self,
            t,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(bounds) = this.triggers.get("new-channel-selection") {
                let items = selections
                    .iter()
                    .map(|s| {
                        MenuItem::new(
                            s.clone(),
                            MenuAction::Select("new-channel-selection", s.clone()),
                        )
                        .checked(current_selection.as_deref() == Some(s.as_str()))
                    })
                    .collect();
                this.open_menu(OpenMenu::under(bounds, items, true), cx);
            }
        }));
        let isolation = segmented(
            "isolation",
            &[
                (Isolation::InPlace, "In place"),
                (Isolation::Worktree, "Worktree"),
                (Isolation::Clone, "Clone"),
            ],
            form.isolation,
            t,
            cx,
            |_| detail(t),
            |this, value, _, cx| {
                this.new_channel.isolation = value;
                cx.notify();
            },
        )
        .border(t.border_width)
        .border_color(c.border_strong);
        let isolation_hint = match form.isolation {
            Isolation::InPlace => {
                "Tasks work in your checkout. They see and change your working tree."
            }
            Isolation::Worktree => {
                "A worktree gets your local commits and uncommitted changes, and it leaves your own working tree alone."
            }
            Isolation::Clone => "Each task gets a fresh clone. It sees only what is pushed.",
        };
        let repos = div()
            .flex()
            .flex_col()
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.md)
            .child(
                div()
                    .id("add-repo")
                    .flex()
                    .items_center()
                    .gap(t.space.sm + t.space.xxs)
                    .py(t.space.sm)
                    .px(t.space.md)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover_card))
                    .child(icon("plus", t.icon.size, c.text_tertiary))
                    .child(text("Add a repo", t.text.ui, c.text_secondary))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_notice(
                            "Repos come in a later build. Ask Consort to attach one.".into(),
                            cx,
                        )
                    })),
            );
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.lg + t.space.xxs)
            .child(
                div()
                    .flex()
                    .gap(t.space.md)
                    .child(
                        field_block("Name", input_box(&self.inputs.name, name_focused, t), None, t).flex_1(),
                    )
                    .child(field_block("Group", group_select, None, t).w(t.component.group_select).flex_none()),
            )
            .child(field_block(
                "Purpose",
                input_box(&self.inputs.purpose, purpose_focused, t),
                Some("One line. Consort reads it before every turn in this channel."),
                t,
            ))
            .when(form.new_group, |d| {
                d.child(field_block(
                    "Group name",
                    input_box(&self.inputs.group_name, group_name_focused, t),
                    Some("The channel goes in this new group."),
                    t,
                ))
            })
            .child(field_block("Repos", repos, None, t))
            .child(field_block(
                "Run tasks on",
                chips,
                Some("Consort tries these in order, then any online worker with the repo and the harness."),
                t,
            ))
            .child(
                div()
                    .flex()
                    .gap(t.space.md)
                    .child(field_block("Default harness for tasks", selection_select, None, t).flex_1())
                    .child(field_block("Workspace", isolation, None, t).w(t.component.workspace_control).flex_none()),
            )
            .child(text(isolation_hint, detail(t), c.text_tertiary))
;
        let actions = self.dialog_actions("Create channel", t, cx, |this, window, cx| {
            let name = this.field(Field::Name, cx);
            if name.is_empty() {
                this.dialog_error = Some("Enter a name for the channel.".into());
                cx.notify();
                return;
            }
            let form = this.new_channel.clone();
            let new_group = if form.new_group {
                let group = this.field(Field::GroupName, cx);
                if group.is_empty() {
                    this.dialog_error = Some("Enter a name for the group.".into());
                    cx.notify();
                    return;
                }
                Some(group)
            } else {
                None
            };
            let channel = NewChannel {
                name,
                purpose: this.field(Field::Purpose, cx),
                group_id: if form.new_group { None } else { form.group_id },
                new_group,
                repos: Vec::new(),
                preferred_workers: form.workers,
                task_selection: form.selection,
                isolation: form.isolation,
            };
            // The new channel opens as soon as the hub sends it.
            let before = this.state.channels.iter().map(|c| c.id.clone()).collect();
            this.open_created = Some((channel.name.clone(), before));
            let client = this.client.clone();
            this.submit_dialog(
                async move { client.create_channel(channel).await },
                window,
                cx,
            );
        });
        (
            "New channel".to_owned(),
            "A channel keeps one subject together: its memory, tasks, routines, files and repos."
                .to_owned(),
            t.layout.dialog_wide,
            body,
            actions,
        )
    }

    fn add_worker_dialog(
        &mut self,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> (String, String, gpui_kit::Pixels, Div, Div) {
        let c = &t.colors;
        let pairing = self.state.worker_pairing.clone();
        let code = pairing
            .as_ref()
            .map(|p| p.code.clone())
            .unwrap_or_else(|| "Making a code".to_owned());
        let hub_url = pairing
            .as_ref()
            .map(|p| p.hub_url.clone())
            .unwrap_or_default();
        // There is no installer yet, so the step builds the consort command
        // from a checkout. The command is the same on every system; what
        // the build needs is not.
        let install = "cargo install --path crates/consort-cli";
        let needs = match self.add_worker_os {
            Os::MacOs => "the Xcode command line tools",
            Os::Windows => "the Visual Studio C++ build tools",
            Os::Linux => "a C compiler",
        };
        let install_note = format!(
            "There is no installer yet. In a checkout of the Consort repo, build the consort command. It needs Rust from rustup.rs and {needs}."
        );
        let pair = if hub_url.is_empty() {
            format!("consort worker pair --code {code}")
        } else {
            format!("consort worker pair {hub_url} --code {code}")
        };
        let step = |n: &str,
                    title: &str,
                    trailing: Option<AnyElement>,
                    body: Option<AnyElement>,
                    t: &Tokens| {
            div()
                .flex()
                .gap(t.space.md)
                .child(
                    div()
                        .size(t.space.lg + t.space.sm - t.space.xxs)
                        .flex_none()
                        .rounded(t.radius.full)
                        .border(t.border_width)
                        .border_color(c.border_strong)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(text(n.to_owned(), detail(t), c.text_secondary)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(t.space.sm + t.space.xxs)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .child(
                                    text(title.to_owned(), t.text.ui_strong, c.text_primary)
                                        .flex_1()
                                        .min_w_0(),
                                )
                                .children(trailing),
                        )
                        .children(body),
                )
        };
        let command_box =
            |id: &'static str, command: String, t: &Tokens, cx: &mut Context<Self>| {
                let copy = command.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.sm)
                    .pl(t.space.md)
                    .pr(t.space.xs)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .rounded(t.radius.md)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(t.font_mono.clone())
                            .type_style(t.text.mono)
                            .text_color(c.text_primary)
                            .child(gpui_kit::base::SelectableText::new(id, command)),
                    )
                    .child(
                        icon_button(
                            id,
                            "copy",
                            t.space.xl + t.space.xs,
                            t.icon.size,
                            c.text_secondary,
                            c.hover_card,
                            t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                            this.show_notice("Copied the command.".into(), cx);
                        })),
                    )
            };
        let os_tabs = segmented(
            "worker-os",
            &[
                (Os::MacOs, "macOS"),
                (Os::Windows, "Windows"),
                (Os::Linux, "Linux"),
            ],
            self.add_worker_os,
            t,
            cx,
            |_| detail(t),
            |this, value, _, cx| {
                this.add_worker_os = value;
                cx.notify();
            },
        )
        .flex_none();
        let expires = pairing
            .as_ref()
            .map(|p| {
                let left = (p.expires_at - consort_client::format::now_ms()).max(0) / 1000;
                format!("Expires in {}:{:02}", left / 60, left % 60)
            })
            .unwrap_or_default();
        let code_card = div()
            .flex()
            .items_center()
            .py(t.space.md + t.space.xxs)
            .px(t.space.lg)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .rounded(t.radius.md)
            .child(
                div()
                    .flex_1()
                    .font_family(t.font_mono.clone())
                    .text_size(t.text.display.size)
                    .line_height(t.text.display.line)
                    .text_color(c.text_primary)
                    .child(gpui_kit::base::SelectableText::new(
                        "pairing-code",
                        code.clone(),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .child(text("Single use", detail(t), c.text_secondary))
                    .child(text(expires, detail(t), c.text_secondary)),
            );
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.lg + t.space.xs)
            .child(step(
                "1",
                "Install Consort on the machine",
                Some(os_tabs.into_any_element()),
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(t.space.sm)
                        .child(text(install_note, t.text.ui, c.text_secondary))
                        .child(command_box("install-command", install.to_owned(), t, cx))
                        .into_any_element(),
                ),
                t,
            ))
            .child(step(
                "2",
                "Pair it with this code",
                None,
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(t.space.sm)
                        .child(code_card)
                        .child(command_box("pair-command", pair, t, cx))
                        .into_any_element(),
                ),
                t,
            ))
            .child(step(
                "3",
                "Sign in to your harnesses on that machine",
                None,
                Some(
                    text(
                        "Consort finds Claude Code, Codex, Pi, OpenCode and ACP agents by itself. Their logins stay on the machine.",
                        t.text.ui,
                        c.text_secondary,
                    )
                    .into_any_element(),
                ),
                t,
            ));
        // The worker that this dialog paired: one that was not in the list
        // when the dialog opened.
        let added = self
            .added_worker
            .as_ref()
            .and_then(|id| self.state.workers.iter().find(|w| &w.id == id));
        let (status, note, done): (AnyElement, String, bool) = match added {
            Some(w) if w.online => (
                crate::ui::parts::worker_dot(true, t.component.state_dot, t.icon.size, t)
                    .into_any_element(),
                format!("{} connected. It shows in the list, selected.", w.name),
                true,
            ),
            Some(w) => (
                crate::ui::parts::status_glyph(
                    "waiting-worker",
                    consort_client::TaskStatus::Working,
                    t.icon.size,
                    t,
                    cx,
                ),
                format!("{} paired. Waiting for it to connect", w.name),
                false,
            ),
            None => (
                crate::ui::parts::status_glyph(
                    "waiting-worker",
                    consort_client::TaskStatus::Working,
                    t.icon.size,
                    t,
                    cx,
                ),
                "Waiting for the worker to connect".to_owned(),
                false,
            ),
        };
        let close = if done {
            button("dialog-done", "Done", ButtonKind::Primary, t)
        } else {
            button("dialog-cancel", "Cancel", ButtonKind::Secondary, t)
        };
        let actions = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(status)
            .child(text(note, t.text.ui, c.text_secondary).flex_1().min_w_0())
            .child(
                close.on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            );
        (
            "Add a worker".to_owned(),
            "A worker runs tasks on its own machine. It connects out to the hub, so the machine needs no open ports.".to_owned(),
            t.layout.dialog_worker,
            body,
            actions,
        )
    }
}
