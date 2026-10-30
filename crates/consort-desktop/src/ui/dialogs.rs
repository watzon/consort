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
        model_picker::{PickerTarget, TriggerStyle},
        parts::{
            ButtonKind, button, detail, error_box, focusable, icon, icon_button, selectable, text,
        },
        settings::segmented,
    },
};

/// A labelled field.
pub(crate) fn field_block(
    label: &str,
    body: impl IntoElement,
    hint: Option<&str>,
    t: &Tokens,
) -> Div {
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
    select_box_with_logo(id, value, None, app, t)
}

/// A select box with the logo of a provider before the value.
fn select_box_with_logo(
    id: &'static str,
    value: String,
    logo: Option<&consort_client::ProviderLogo>,
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
        .child(
            div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .min_w_0()
                .children(
                    logo.map(|logo| {
                        crate::ui::parts::provider_logo(logo, t.icon.size, c.text_primary)
                    }),
                )
                .child(text(value, t.text.ui, c.text_primary)),
        )
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

/// The note under a Worker tags field: what the tags do and how many
/// workers match, or that none does and what to do.
pub fn tags_note(matching: usize) -> String {
    match matching {
        0 => "No worker has every tag here, so work in this channel waits. Add the tags to a worker in Settings > Workers, or remove a tag.".to_owned(),
        1 => "All work in this channel runs only on workers that have every tag here. 1 worker matches.".to_owned(),
        n => format!("All work in this channel runs only on workers that have every tag here. {n} workers match."),
    }
}

/// The words before the suggested tags. With no tag chosen yet there is
/// nothing to be "also" with.
pub fn tag_suggestions_label(chosen: &[String]) -> &'static str {
    if chosen.is_empty() {
        "On workers:\u{a0}"
    } else {
        "Also on workers:\u{a0}"
    }
}

/// The tags to offer after "Also on workers:": the user tags of the
/// matching workers that are not chosen yet, at most three. With no tag
/// chosen, every worker matches, so a tag that any worker has can start the
/// list.
pub fn tag_suggestions(state: &consort_client::ViewState, chosen: &[String]) -> Vec<String> {
    let mut out: Vec<String> = state
        .workers_with_tags(chosen)
        .iter()
        .flat_map(|w| w.tags.iter())
        .filter(|tag| !chosen.contains(tag))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out.truncate(3);
    out
}

/// The select option that puts the new channel in no group. It is not a
/// group ID, so it cannot be a group.
pub const NO_GROUP: &str = "no-group";
/// The select option that makes a group for the new channel.
pub const NEW_GROUP: &str = "new-group";

impl ConsortApp {
    /// The Worker tags field of a channel: the chosen tags as removable
    /// chips, Add tag (a menu of every worker tag), the tags of the matching
    /// workers to add with one click, and how many workers match.
    /// `settings` says which list it edits: the open Worker tags dialog, or
    /// the New channel form.
    pub fn tags_field(
        &self,
        chosen: &[String],
        settings: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(t.space.xs + t.space.xxs);
        for tag in chosen {
            let remove = tag.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("channel-tag-{tag}")))
                    .flex()
                    .items_center()
                    .gap(t.space.xs)
                    .py(t.space.xs)
                    .pl(t.space.sm + t.space.xxs)
                    .pr(t.space.xs + t.space.xxs)
                    .rounded(t.radius.full)
                    .bg(c.tint_accent)
                    .border(t.border_width)
                    .border_color(c.accent_text)
                    .cursor_pointer()
                    .child(text(tag.clone(), t.text.ui_strong, c.accent_text))
                    .child(icon("x", t.space.md, c.accent_text))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let tags = if settings {
                            &mut this.channel_tags
                        } else {
                            &mut this.new_channel.tags
                        };
                        tags.retain(|t| *t != remove);
                        cx.notify();
                    })),
            );
        }
        let free: Vec<String> = self
            .state
            .tags
            .iter()
            .filter(|tag| !chosen.contains(tag))
            .cloned()
            .collect();
        let user_tags: Vec<String> = self
            .state
            .workers
            .iter()
            .flat_map(|w| w.tags.iter().cloned())
            .collect();
        if !free.is_empty() {
            row = row.child(
                div()
                    .id("channel-tag-add")
                    .relative()
                    .flex()
                    .items_center()
                    .gap(t.space.xs + t.space.xxs)
                    .py(t.space.xs)
                    .px(t.space.sm + t.space.xxs)
                    .rounded(t.radius.full)
                    .border(t.border_width)
                    .border_dashed()
                    .border_color(c.border_strong)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover_raised))
                    .child(self.triggers.track("channel-tag-add"))
                    .child(icon("plus", t.space.md - t.space.xxs, c.text_secondary))
                    .child(text("Add tag", t.text.ui, c.text_secondary))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(bounds) = this.triggers.get("channel-tag-add") {
                            // The user tags first, then a line, then the
                            // tags that the workers found themselves.
                            let mut items: Vec<MenuItem> = Vec::new();
                            for tag in &free {
                                let detected = !user_tags.contains(tag);
                                let first_detected = detected
                                    && !items.is_empty()
                                    && items
                                        .iter()
                                        .all(|i| user_tags.iter().any(|u| *u == i.label.as_ref()));
                                let item = MenuItem::new(
                                    tag.clone(),
                                    MenuAction::Select("channel-tag", tag.clone()),
                                );
                                items.push(if first_detected {
                                    item.separated()
                                } else {
                                    item
                                });
                            }
                            this.open_menu(OpenMenu::under(bounds, items, false), cx);
                        }
                    })),
            );
        }
        let suggestions = tag_suggestions(&self.state, chosen);
        if !suggestions.is_empty() {
            let mut also = div()
                .flex()
                .items_center()
                .flex_wrap()
                .pl(t.space.xs)
                .child(text(
                    tag_suggestions_label(chosen),
                    detail(t),
                    c.text_tertiary,
                ));
            let last = suggestions.len() - 1;
            for (i, tag) in suggestions.into_iter().enumerate() {
                let add = tag.clone();
                also = also.child(
                    div()
                        .id(SharedString::from(format!("suggest-tag-{tag}")))
                        .cursor_pointer()
                        .type_style(detail(t))
                        .text_color(c.text_tertiary)
                        .hover(|s| s.text_color(c.text_primary))
                        .child(if i < last {
                            format!("{tag},\u{a0}")
                        } else {
                            tag
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let tags = if settings {
                                &mut this.channel_tags
                            } else {
                                &mut this.new_channel.tags
                            };
                            if !tags.contains(&add) {
                                tags.push(add.clone());
                            }
                            cx.notify();
                        })),
                );
            }
            row = row.child(also);
        }
        let matching = self.state.workers_with_tags(chosen).len();
        let note = if chosen.is_empty() {
            "Add a tag to keep the work of this channel on some workers, such as work or gpu-nvidia.".to_owned()
        } else {
            tags_note(matching)
        };
        div()
            .flex()
            .flex_col()
            .gap(t.space.xs + t.space.xxs)
            .child(text("Worker tags", t.text.label, c.text_secondary))
            .child(row)
            .child(text(
                note,
                detail(t),
                if !chosen.is_empty() && matching == 0 {
                    c.status_failed
                } else {
                    c.text_tertiary
                },
            ))
    }

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
                                    cx,
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
        let mut lead: Option<AnyElement> = None;
        let mut ext_parts = match dialog {
            Dialog::ComposioKey => Some(self.composio_key_dialog(t, window, cx)),
            Dialog::AddConnector => Some(self.add_connector_dialog(t, window, cx)),
            Dialog::Connect => Some(self.connect_dialog(t, window, cx)),
            Dialog::ConfirmDisconnect(id) => Some(self.confirm_disconnect_dialog(id, t, cx)),
            Dialog::AddSkill => Some(self.add_skill_dialog(t, window, cx)),
            Dialog::AddSkillSource => Some(self.add_skill_source_dialog(t, window, cx)),
            Dialog::ConnectionActivity(id) => Some(self.connection_activity_dialog(id, t, cx)),
            Dialog::ConfirmRemoveSkill(id) => Some(self.confirm_remove_skill_dialog(id, t, cx)),
            Dialog::SkillFiles(id) => Some(self.skill_files_dialog(id, t, cx)),
            Dialog::ChannelExtensions(id) => Some(self.channel_extensions_dialog(id, t, cx)),
            Dialog::AccountSettings(w, a) => {
                Some(self.account_settings_dialog(w, a, t, window, cx))
            }
            _ => None,
        };
        let (title, description, width, body, actions) = match dialog {
            Dialog::ComposioKey
            | Dialog::AddConnector
            | Dialog::Connect
            | Dialog::ConfirmDisconnect(_)
            | Dialog::AddSkill
            | Dialog::AddSkillSource
            | Dialog::ConnectionActivity(_)
            | Dialog::ConfirmRemoveSkill(_)
            | Dialog::SkillFiles(_)
            | Dialog::ChannelExtensions(_)
            | Dialog::AccountSettings(..) => match ext_parts.take() {
                Some((title, description, width, body, actions, logo)) => {
                    lead = logo;
                    (title, description, width, body, actions)
                }
                None => (
                    String::new(),
                    String::new(),
                    t.layout.dialog_width,
                    div(),
                    div(),
                ),
            },
            Dialog::ConfirmDeleteFile(..)
            | Dialog::RenameFile(..)
            | Dialog::MoveFile(..)
            | Dialog::ChooseChannelFiles(_) => match self.file_dialog(dialog, t, window, cx) {
                Some(parts) => parts,
                None => (
                    String::new(),
                    String::new(),
                    t.layout.dialog_width,
                    div(),
                    div(),
                ),
            },
            Dialog::AddAccount(_) => {
                let (title, description, width, body, actions, logo) =
                    self.add_account_dialog(t, window, cx);
                lead = logo;
                (title, description, width, body, actions)
            }
            Dialog::RenameAccount(worker_id, account_id) => {
                let (worker_id, account_id) = (worker_id.clone(), account_id.clone());
                let focused = self
                    .inputs
                    .name
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
                (
                    "Rename account".to_owned(),
                    "Use the same name on every worker where this login is the same account. The hub treats accounts with one name as one.".to_owned(),
                    t.layout.dialog_width,
                    field_block("Name", input_box(&self.inputs.name, focused, t), None, t),
                    self.dialog_actions("Rename", t, cx, move |this, window, cx| {
                        let name = this.field(Field::Name, cx);
                        if name.is_empty() {
                            this.dialog_error = Some("Enter a name for the account.".into());
                            cx.notify();
                            return;
                        }
                        let client = this.client.clone();
                        let (w, a) = (worker_id.clone(), account_id.clone());
                        this.submit_dialog(
                            async move { client.rename_account(w, a, name).await },
                            window,
                            cx,
                        );
                    }),
                )
            }
            Dialog::ConfirmRemoveAccount(worker_id, account_id) => {
                let (worker_id, account_id) = (worker_id.clone(), account_id.clone());
                let worker = self.state.worker(&worker_id);
                let worker_name = worker.map(|w| w.name.clone()).unwrap_or_default();
                let label = worker
                    .and_then(|w| w.account(&account_id))
                    .map(|a| a.label.clone())
                    .unwrap_or_default();
                (
                    format!("Remove {label}?"),
                    format!(
                        "{worker_name} signs this account out where it can and deletes its profile folder, with the login and any saved key. Tasks that use it stop. You cannot undo this."
                    ),
                    t.layout.dialog_width,
                    div(),
                    self.dialog_actions_kind(
                        "Remove account",
                        ButtonKind::Danger,
                        t,
                        cx,
                        move |this, window, cx| {
                            let client = this.client.clone();
                            let (w, a) = (worker_id.clone(), account_id.clone());
                            this.submit_dialog(
                                async move { client.remove_account(w, a).await },
                                window,
                                cx,
                            );
                        },
                    ),
                )
            }
            Dialog::ChannelTags(id) => {
                let id = id.clone();
                let name = self
                    .state
                    .channel(&id)
                    .map(|ch| ch.name.clone())
                    .unwrap_or_default();
                let tags = self.channel_tags.clone();
                (
                    format!("Worker tags for {name}"),
                    "Choose the worker tags that this channel requires.".to_owned(),
                    t.layout.dialog_wide,
                    self.tags_field(&tags, true, t, cx),
                    self.dialog_actions("Save tags", t, cx, move |this, window, cx| {
                        let client = this.client.clone();
                        let id = id.clone();
                        let tags = this.channel_tags.clone();
                        this.submit_dialog(
                            async move {
                                client
                                    .update_channel(
                                        id,
                                        ChannelChanges {
                                            tags: Some(tags),
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
                    "The name shows in the sidebar and in Tutti.".to_owned(),
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
                let purpose_focused = self
                    .inputs
                    .purpose
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window);
                (
                    format!("Instructions for {name}"),
                    "Standing guidance that Consort reads before every turn in this channel. Write it in Markdown.".to_owned(),
                    t.layout.dialog_wide,
                    div()
                        .flex()
                        .flex_col()
                        .gap(t.space.lg)
                        .child(field_block(
                            "Purpose",
                            input_box(&self.inputs.purpose, purpose_focused, t),
                            Some("One line about what this channel is for"),
                            t,
                        ))
                        .child(field_block("Instructions", div()
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
                        ), None, t)),
                    self.dialog_actions("Save instructions", t, cx, move |this, window, cx| {
                        let notes = this.inputs.notes.read(cx).value().to_string();
                        let purpose = this.field(Field::Purpose, cx);
                        let client = this.client.clone();
                        let id = id.clone();
                        this.submit_dialog(
                            async move {
                                client
                                    .update_channel(id, ChannelChanges { purpose: Some(purpose), instructions: Some(notes), ..ChannelChanges::default() })
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
                    .children(lead)
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
                            cx,
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

    pub(crate) fn dialog_actions(
        &self,
        label: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
        on_confirm: impl Fn(&mut ConsortApp, &mut Window, &mut Context<ConsortApp>) + 'static,
    ) -> Div {
        self.dialog_actions_kind(label, ButtonKind::Primary, t, cx, on_confirm)
    }

    pub(crate) fn dialog_actions_kind(
        &self,
        label: &str,
        kind: ButtonKind,
        t: &Tokens,
        cx: &mut Context<Self>,
        on_confirm: impl Fn(&mut ConsortApp, &mut Window, &mut Context<ConsortApp>) + 'static,
    ) -> Div {
        let c = &t.colors;
        let busy = self.dialog_busy();
        // Enter in a field of the dialog does what the confirm button does.
        // A button that destroys something takes a click, not a key press
        // from a field.
        let on_confirm: crate::app::DialogConfirm = std::rc::Rc::new(on_confirm);
        if kind != ButtonKind::Danger {
            self.dialog_confirm.replace(Some(on_confirm.clone()));
        }
        // A destroying confirm button has the failed fill. The kind sets its
        // hover once: GPUI refuses a second hover style.
        let fill = if kind == ButtonKind::Danger {
            ButtonKind::DangerFill
        } else {
            kind
        };
        let confirm = button("dialog-confirm", label.to_owned(), fill, t)
            .debug_selector(|| "dialog-confirm".into())
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
        let assistant_model = self.render_model_trigger(
            "new-channel-assistant",
            PickerTarget::NewChannelAssistant,
            TriggerStyle::Field,
            t,
            cx,
        );
        let assistant_effort = self.render_effort_trigger(
            "new-channel-assistant-effort",
            PickerTarget::NewChannelAssistant,
            TriggerStyle::Field,
            t,
            cx,
        );
        let task_model = self.render_model_trigger(
            "new-channel-tasks",
            PickerTarget::NewChannelTasks,
            TriggerStyle::Field,
            t,
            cx,
        );
        let task_effort = self.render_effort_trigger(
            "new-channel-tasks-effort",
            PickerTarget::NewChannelTasks,
            TriggerStyle::Field,
            t,
            cx,
        );
        // A model field, with the effort select next to it when the model
        // has effort levels.
        let model_row = |label: &str, model: AnyElement, effort: Option<AnyElement>| {
            field_block(
                label,
                div()
                    .flex()
                    .gap(t.space.sm)
                    .child(div().flex_1().min_w_0().child(model))
                    .when_some(effort, |d, effort| {
                        d.child(div().flex_none().w(t.component.effort_select).child(effort))
                    }),
                None,
                t,
            )
        };
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
            .child(self.tags_field(&form.tags, false, t, cx))
            .child(model_row("Model for the assistant", assistant_model, assistant_effort))
            .child(model_row("Model for tasks", task_model, task_effort))
            .child(
                div()
                    .flex()
                    .gap(t.space.md)
                    .child(field_block("Run tasks on", chips, None, t).flex_1().min_w_0())
                    .child(field_block("Workspace", isolation, None, t).w(t.component.workspace_control).flex_none()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .child(text(
                        "Consort tries the workers in order, then any online worker with the repo and the account.",
                        detail(t),
                        c.text_tertiary,
                    ))
                    .child(text(isolation_hint, detail(t), c.text_tertiary)),
            )
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
                coordinator_selection: form.coordinator_selection,
                task_selection: form.task_selection,
                isolation: form.isolation,
                tags: form.tags,
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
                            cx,
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
                "Add accounts to it",
                None,
                Some(
                    text(
                        "When the worker shows in the list, add its accounts in Settings > Workers. The worker downloads each provider itself, and each login stays in its own profile folder on the machine.",
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

#[cfg(test)]
mod tests {
    use super::*;
    use consort_client::{ViewState, WorkerView};

    fn worker(name: &str, tags: &[&str]) -> WorkerView {
        WorkerView {
            id: name.to_owned(),
            name: name.to_owned(),
            online: true,
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            ..WorkerView::default()
        }
    }

    fn state() -> ViewState {
        ViewState {
            workers: vec![
                worker("bigboi", &["personal", "windows-test"]),
                worker("MacBook Pro", &["work"]),
            ],
            ..ViewState::default()
        }
    }

    #[test]
    fn the_tags_of_the_workers_are_offered_before_any_tag_is_chosen() {
        let suggestions = tag_suggestions(&state(), &[]);
        assert_eq!(suggestions, ["personal", "windows-test", "work"]);
        assert_eq!(tag_suggestions_label(&[]), "On workers:\u{a0}");
    }

    #[test]
    fn after_a_choice_only_the_matching_workers_offer_more_tags() {
        let chosen = vec!["personal".to_owned()];
        assert_eq!(tag_suggestions(&state(), &chosen), ["windows-test"]);
        assert_eq!(tag_suggestions_label(&chosen), "Also on workers:\u{a0}");
        let none_left = vec!["personal".to_owned(), "windows-test".to_owned()];
        assert!(tag_suggestions(&state(), &none_left).is_empty());
    }
}
