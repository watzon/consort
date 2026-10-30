//! Memory: the Memory tab of the Channel panel, a note opened in the panel
//! with its editor and the stale-write message, Settings > Memory with
//! global memory and each channel's memory, the version history, Recently
//! deleted, the note dialogs, and the memory saved line in the timeline.
//!
//! Notes show as a tree by path. Folders open and close. A note name is in
//! mono. A note that the assistant just changed shows an accent dot and
//! "Just now" until the user opens it.

use consort_client::{
    ChannelKind, Command,
    memory::{self as m, GLOBAL, MemoryNoteView, MemoryRow},
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Dialog},
    knowledge::{KnowledgeAction, Place, note_menu},
    ui::{
        dialogs::{field_block, input_box},
        menu::{MenuAction, OpenMenu},
        parts::{
            ButtonKind, button, detail, focusable, icon, icon_button, icon_text_button, line,
            selectable, text,
        },
        routines::panel_scroll,
    },
};

/// The hint under the Memory tab of a channel.
pub fn memory_hint(channel_name: &str, tutti: bool) -> String {
    if tutti {
        "Every channel reads global memory. Only Tutti writes it, and you can edit it here."
            .to_owned()
    } else {
        format!(
            "The assistant reads these notes in every {channel_name} session. Tutti can read them too."
        )
    }
}

impl ConsortApp {
    /// The rows of the tree of one scope, with the open and closed folders.
    fn tree_rows(&self, scope: &str) -> Vec<MemoryRow> {
        let prefix = format!("{scope}:");
        let closed: std::collections::BTreeSet<String> = self
            .knowledge
            .closed
            .iter()
            .filter_map(|k| k.strip_prefix(&prefix).map(str::to_owned))
            .collect();
        m::tree(
            &self.state.memory_notes,
            scope,
            &closed,
            &self.knowledge.seen,
            consort_client::format::now_ms(),
        )
    }

    /// The rows of a memory tree. `selected` is the note that is open.
    fn tree_list(
        &mut self,
        scope: &str,
        place: Place,
        selected: Option<&str>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let k = t.component.knowledge;
        let mut list = div().flex().flex_col().gap(t.border_width);
        for row in self.tree_rows(scope) {
            match row {
                MemoryRow::Folder {
                    path,
                    name,
                    depth,
                    count,
                    open,
                } => {
                    let key = format!("{scope}:{path}");
                    let selector = format!("memory-folder-{scope}-{path}");
                    list = list.child(
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector)
                            .flex()
                            .items_center()
                            .gap(t.space.xs + t.space.xxs)
                            .py(t.space.sm - t.space.xxs)
                            .pl(t.space.sm + k.tree_indent * depth as f32)
                            .pr(t.space.sm)
                            .rounded(t.radius.md)
                            .cursor_pointer()
                            .hover(|s| s.bg(c.hover_sidebar))
                            .child(icon(
                                if open {
                                    "chevron-down"
                                } else {
                                    "chevron-right"
                                },
                                t.icon.dense,
                                c.text_tertiary,
                            ))
                            .child(icon("folder", t.icon.size, c.text_secondary))
                            .child(line(name, t.text.ui, c.text_primary).flex_1())
                            .child(text(count.to_string(), detail(t), c.text_tertiary))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.knowledge.closed.remove(&key) {
                                    this.knowledge.closed.insert(key.clone());
                                }
                                cx.notify();
                            })),
                    );
                }
                MemoryRow::Note {
                    path,
                    name,
                    depth,
                    date,
                    just_now,
                } => {
                    let chosen = selected == Some(path.as_str());
                    let selector = format!("memory-note-{scope}-{path}");
                    let (open_scope, open_path) = (scope.to_owned(), path.clone());
                    let (menu_scope, menu_path) = (scope.to_owned(), path.clone());
                    // A note in a folder lines up with the folder's name. A
                    // note at the top has the file icon.
                    let top = depth == 0 && !path.contains('/');
                    let indent = if top {
                        0.0
                    } else {
                        depth.saturating_sub(1) as f32
                    };
                    list = list.child(
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector)
                            .flex()
                            .items_center()
                            .gap(t.space.xs + t.space.xxs)
                            .py(t.space.sm - t.space.xxs)
                            .pl(t.space.sm + k.tree_indent * indent)
                            .pr(t.space.sm)
                            .rounded(t.radius.md)
                            .cursor_pointer()
                            .when(chosen, |d| d.bg(c.surface_selected))
                            .when(!chosen, |d| d.hover(|s| s.bg(c.hover_sidebar)))
                            .child(div().size(t.icon.dense).flex_none())
                            .child(if top {
                                icon("file", t.icon.size, c.text_secondary).into_any_element()
                            } else {
                                div().size(t.icon.size).flex_none().into_any_element()
                            })
                            .child(
                                line(name, t.text.mono, c.text_primary)
                                    .font_family(t.font_mono.clone())
                                    .flex_1(),
                            )
                            .when(just_now, |d| {
                                d.child(
                                    div()
                                        .size(t.component.knowledge.status_dot)
                                        .flex_none()
                                        .rounded(t.radius.full)
                                        .bg(c.accent_text),
                                )
                            })
                            .child(text(
                                date,
                                detail(t),
                                if just_now {
                                    c.accent_text
                                } else {
                                    c.text_tertiary
                                },
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| match place {
                                Place::Panel => {
                                    this.open_note_in_panel(&open_scope, &open_path, window, cx)
                                }
                                Place::Settings => {
                                    this.open_note_in_settings(&open_scope, &open_path, cx)
                                }
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    this.open_menu(
                                        OpenMenu::at(
                                            event.position,
                                            note_menu(&menu_scope, &menu_path),
                                        ),
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            ),
                    );
                }
            }
        }
        list
    }

    /// The Memory tab of the Channel panel.
    pub fn render_memory_tab(
        &mut self,
        channel_name: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let scope = self.memory_scope(&self.channel_id.clone());
        self.load_memory(&scope, false, cx);
        let notes = self.notes_of(&scope);
        let loaded = self.state.memory_loaded.contains(&scope);
        let new_scope = scope.clone();
        let new_button = icon_text_button("memory-new", "plus", "New note", t)
            .debug_selector(|| "memory-new".into())
            .px(t.space.sm + t.space.xxs)
            .on_click(
                cx.listener(move |this, _, window, cx| this.open_new_note(&new_scope, window, cx)),
            );
        if loaded && notes.is_empty() {
            let new_scope = scope.clone();
            return div().flex_1().min_h_0().p(t.space.lg).child(
                div()
                    .debug_selector(|| "memory-empty".into())
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.page + t.space.xl)
                    .px(t.space.xl)
                    .child(
                        div()
                            .size(t.component.files.drop_icon_tile + t.space.sm)
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(t.radius.full)
                            .bg(c.surface_card)
                            .child(icon("memory", t.icon.size, c.text_secondary)),
                    )
                    .child(text(
                        format!("No memory notes for {channel_name}"),
                        t.text.ui_strong,
                        c.text_primary,
                    ))
                    .child(div().text_center().child(text(
                        "The assistant saves a note here when it learns something worth keeping. You can also write one.",
                        t.text.ui,
                        c.text_secondary,
                    )))
                    .child(
                        button("memory-new-empty", "New note", ButtonKind::Secondary, t)
                            .mt(t.space.sm)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_new_note(&new_scope, window, cx)
                            })),
                    ),
            );
        }
        let summary = match notes.len() {
            1 => "1 memory note".to_owned(),
            n => format!("{n} memory notes"),
        };
        let list = self
            .tree_list(&scope, Place::Panel, None, t, cx)
            .px(t.space.sm)
            .pb(t.space.sm);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(t.space.sm)
                    .pt(t.space.sm + t.space.xxs)
                    .pr(t.space.md)
                    .pb(t.space.xs + t.space.xxs)
                    .pl(t.space.lg)
                    .child(
                        div()
                            .debug_selector(|| "memory-summary".into())
                            .flex_1()
                            .child(text(summary, t.text.ui, c.text_secondary)),
                    )
                    .child(new_button),
            )
            .child(panel_scroll("memory-scroll", list, t))
    }

    /// Opens the dialog that names a new note.
    pub fn open_new_note(&mut self, scope: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.knowledge_inputs
            .note_path
            .update(cx, |i, cx| i.set_value("", window, cx));
        self.open_dialog(Dialog::NewNote(scope.to_owned()), window, cx);
        self.knowledge_inputs
            .note_path
            .update(cx, |i, cx| i.focus(window, cx));
    }

    /// A note opened in the Channel panel: its page, or its editor.
    pub fn render_note_page(
        &mut self,
        scope: &str,
        path: &str,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let now = consort_client::format::now_ms();
        let editing = self
            .knowledge
            .editor
            .as_ref()
            .filter(|e| e.place == Place::Panel && e.scope == scope && e.path == path)
            .cloned();
        let note = self.note(scope, path);
        let subtitle = match (&editing, &note) {
            (Some(e), _) if e.base_sha.is_none() => "A new note. Save it to keep it.".to_owned(),
            (Some(e), _) => format!(
                "Editing. Version {}, opened at {}",
                e.base_version,
                consort_client::format::clock_24(e.opened_at)
            ),
            (None, Some(n)) => m::saved_line(n, now),
            (None, None) => "This note is not here any more.".to_owned(),
        };
        let (edit_scope, edit_path) = (scope.to_owned(), path.to_owned());
        let (menu_scope, menu_path) = (scope.to_owned(), path.to_owned());
        let header = crate::ui::parts::panel_header(t)
            .pl(t.space.sm)
            .child(
                icon_button(
                    "note-back",
                    "chevron-left",
                    t.space.xxl,
                    t.icon.size,
                    c.text_secondary,
                    c.hover_sidebar,
                    t,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.knowledge.panel_note = None;
                    if this
                        .knowledge
                        .editor
                        .as_ref()
                        .is_some_and(|e| e.place == Place::Panel)
                    {
                        this.knowledge.editor = None;
                    }
                    cx.notify();
                })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        line(path.to_owned(), t.text.mono, c.text_primary)
                            .font_family(t.font_mono.clone()),
                    )
                    .child(line(subtitle, detail(t), c.text_tertiary)),
            )
            .when(editing.is_none() && note.is_some(), |d| {
                d.child(
                    button("note-edit", "Edit", ButtonKind::Secondary, t)
                        .debug_selector(|| "note-edit".into())
                        .px(t.space.sm + t.space.xxs)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_note(Place::Panel, &edit_scope, &edit_path, window, cx)
                        })),
                )
                .child(
                    icon_button(
                        "note-more-panel",
                        "more-horizontal",
                        t.space.xxl,
                        t.icon.size,
                        c.text_secondary,
                        c.hover_sidebar,
                        t,
                        cx,
                    )
                    .on_click(cx.listener(
                        move |this, event: &gpui_kit::ClickEvent, _, cx| {
                            this.open_menu(
                                OpenMenu::at(event.position(), note_menu(&menu_scope, &menu_path)),
                                cx,
                            )
                        },
                    )),
                )
            })
            .child(
                icon_button(
                    "channel-panel-close",
                    "x",
                    t.space.xxl,
                    t.icon.size,
                    c.text_secondary,
                    c.hover_sidebar,
                    t,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx))),
            );
        let body: AnyElement = match (&editing, &note) {
            (Some(_), _) => self
                .note_editor(Place::Panel, t, window, cx)
                .into_any_element(),
            (None, Some(n)) => {
                let content = n.content.clone().unwrap_or_default();
                let menu = note_menu(scope, path);
                div()
                    .id("note-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py(t.space.lg)
                    .px(t.space.lg)
                    .child(
                        self.render_markdown(
                            &format!("note-{}", n.key()),
                            &content,
                            menu,
                            c.surface_card,
                            t,
                            cx,
                        )
                        .debug_selector(|| "note-text".into()),
                    )
                    .into_any_element()
            }
            (None, None) => div()
                .p(t.space.lg)
                .child(text(
                    "This note was deleted or renamed. Go back to the list to find it.",
                    t.text.ui,
                    c.text_secondary,
                ))
                .into_any_element(),
        };
        div()
            .id("channel-panel")
            .debug_selector(|| "channel-panel".into())
            .flex()
            .flex_col()
            .flex_none()
            .w(self.right_panel_width(t))
            .h_full()
            .bg(c.surface_sidebar)
            .border_l(t.border_width)
            .border_color(c.border_subtle)
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// The note editor: the stale-write message when the note changed, the
    /// text in mono, and Cancel and Save.
    pub fn note_editor(
        &mut self,
        place: Place,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let Some(editor) = self.knowledge.editor.clone() else {
            return div();
        };
        let focused = {
            use gpui_kit::Focusable as _;
            self.knowledge_inputs
                .note
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        };
        let conflict = editor.conflict.clone().map(|newer| {
            let who = if newer.by_user {
                "You saved".to_owned()
            } else if newer.scope == GLOBAL {
                "Tutti saved".to_owned()
            } else {
                "The assistant saved".to_owned()
            };
            div()
                .debug_selector(|| "note-conflict".into())
                .flex()
                .items_start()
                .gap(t.space.sm + t.space.xxs)
                .p(t.space.md)
                .rounded(t.radius.lg)
                .border(t.border_width)
                .border_color(c.status_failed.opacity(t.opacity.failed_border))
                .bg(c.tint_failed)
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(t.text.ui.line)
                        .child(icon("failed", t.icon.size, c.status_failed)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(t.space.xs)
                        .child(text(
                            "This note changed since you opened it.",
                            t.text.ui_strong,
                            c.text_primary,
                        ))
                        .child(text(
                            format!(
                                "{who} version {} at {}. Your text is not saved yet. Load the new version to see it, or save yours to replace it. Version {} stays in the history.",
                                newer.version,
                                consort_client::format::clock_24(newer.updated_at),
                                newer.version
                            ),
                            detail(t),
                            c.text_secondary,
                        ))
                        .child(
                            div()
                                .flex()
                                .gap(t.space.sm)
                                .pt(t.space.xs + t.space.xxs)
                                .child(
                                    button(
                                        "note-load-new",
                                        "Load the new version",
                                        ButtonKind::Secondary,
                                        t,
                                    )
                                    .debug_selector(|| "note-load-new".into())
                                    .px(t.space.sm + t.space.xxs)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.load_new_version(window, cx)
                                    })),
                                )
                                .child(
                                    button(
                                        "note-save-mine",
                                        "Save my version",
                                        ButtonKind::Secondary,
                                        t,
                                    )
                                    .debug_selector(|| "note-save-mine".into())
                                    .px(t.space.sm + t.space.xxs)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_note(true, window, cx)
                                    })),
                                ),
                        ),
                )
        });
        let can_save = editor.conflict.is_none() && !editor.saving;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(t.space.md)
            .when(place == Place::Panel, |d| d.p(t.space.lg))
            .children(conflict)
            .child(
                div()
                    .id("note-editor")
                    .debug_selector(|| "note-editor".into())
                    .flex_1()
                    .min_h(t.component.knowledge.editor_min_height)
                    .overflow_y_scroll()
                    .py(t.space.md)
                    .px(t.space.md)
                    .bg(c.surface_canvas)
                    .border(t.border_width)
                    .border_color(if focused {
                        c.accent_text
                    } else {
                        c.border_strong
                    })
                    .rounded(t.radius.lg)
                    .font_family(t.font_mono.clone())
                    .child(crate::ui::parts::textarea(
                        &self.knowledge_inputs.note,
                        t.text.mono,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(t.space.sm)
                    .child(text("Markdown", detail(t), c.text_tertiary).flex_1())
                    .child(
                        button("note-cancel", "Cancel", ButtonKind::Secondary, t)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_note_edit(cx))),
                    )
                    .child(
                        button("note-save", "Save", ButtonKind::Primary, t)
                            .debug_selector(|| "note-save".into())
                            .px(t.space.md + t.space.xxs)
                            .when(!can_save, |b| {
                                b.opacity(t.opacity.nav_disabled).cursor_default()
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if can_save {
                                    this.save_note(false, window, cx)
                                }
                            })),
                    ),
            )
    }

    /// Settings > Memory: global memory and each channel's memory, grouped,
    /// with Recently deleted at the end. The open note, its editor or its
    /// version history fills the rest.
    pub fn render_memory_settings(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let scopes: Vec<(String, String, Option<String>, bool)> =
            std::iter::once((GLOBAL.to_owned(), "Global memory".to_owned(), None, true))
                .chain(
                    self.state
                        .channels
                        .iter()
                        .filter(|ch| ch.kind != ChannelKind::Tutti)
                        .map(|ch| {
                            (
                                ch.id.clone(),
                                ch.name.clone(),
                                Some(ch.icon.to_string()),
                                false,
                            )
                        }),
                )
                .collect();
        for (scope, ..) in &scopes {
            self.load_memory(scope, false, cx);
            self.load_memory(scope, true, cx);
        }
        // The first note of global memory opens when nothing is chosen.
        if self.knowledge.settings_note.is_none()
            && !self.knowledge.settings_deleted
            && let Some(first) = self.tree_rows(GLOBAL).into_iter().find_map(|r| match r {
                MemoryRow::Note { path, .. } => Some(path),
                MemoryRow::Folder { .. } => None,
            })
        {
            self.knowledge.settings_note = Some((GLOBAL.to_owned(), first));
        }
        let selected = self.knowledge.settings_note.clone();
        let default_scope = selected
            .as_ref()
            .map(|(s, _)| s.clone())
            .unwrap_or_else(|| GLOBAL.to_owned());
        let mut tree = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .px(t.space.sm)
            .pb(t.space.lg);
        for (scope, name, icon_name, global) in &scopes {
            let notes = self.notes_of(scope);
            if notes.is_empty() && !global {
                continue;
            }
            let chosen_path = selected
                .as_ref()
                .filter(|(s, _)| s == scope)
                .map(|(_, p)| p.clone());
            let selector = format!("memory-group-{name}");
            tree = tree.child(
                div()
                    .debug_selector(move || selector)
                    .flex()
                    .flex_col()
                    .gap(t.space.xxs)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .px(t.space.sm)
                            .pt(t.space.xs)
                            .pb(t.space.xxs)
                            .child(match icon_name {
                                None => crate::ui::mark::still_mark(t.icon.dense, t),
                                Some(name) => {
                                    icon(name, t.icon.dense, c.text_tertiary).into_any_element()
                                }
                            })
                            .child(text(name.clone(), t.text.label, c.text_tertiary))
                            .child(text(notes.len().to_string(), detail(t), c.text_tertiary)),
                    )
                    .child(self.tree_list(scope, Place::Settings, chosen_path.as_deref(), t, cx)),
            );
        }
        let deleted_count = self
            .state
            .memory_notes
            .iter()
            .filter(|n| n.deleted_at.is_some())
            .count();
        let deleted_on = self.knowledge.settings_deleted;
        tree = tree.child(
            div()
                .id("memory-deleted-row")
                .debug_selector(|| "memory-deleted-row".into())
                .flex()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .mt(t.space.sm)
                .py(t.space.sm - t.space.xxs)
                .px(t.space.sm)
                .rounded(t.radius.md)
                .cursor_pointer()
                .when(deleted_on, |d| d.bg(c.surface_selected))
                .when(!deleted_on, |d| d.hover(|s| s.bg(c.hover_sidebar)))
                .child(icon("archive", t.icon.size, c.text_secondary))
                .child(line("Recently deleted", t.text.ui, c.text_primary).flex_1())
                .child(text(deleted_count.to_string(), detail(t), c.text_tertiary))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.knowledge.settings_deleted = true;
                    this.knowledge.settings_note = None;
                    this.knowledge.history = None;
                    if this
                        .knowledge
                        .editor
                        .as_ref()
                        .is_some_and(|e| e.place == Place::Settings)
                    {
                        this.knowledge.editor = None;
                    }
                    cx.notify();
                })),
        );
        let description = if default_scope == GLOBAL {
            "Global memory. Every channel reads it. Only Tutti writes it, and you can edit it here."
                .to_owned()
        } else {
            let name = self.scope_name(&default_scope);
            format!(
                "The memory of {name}. The assistant reads it in every {name} session, and Tutti can read it too."
            )
        };
        let new_scope = default_scope.clone();
        let column = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(t.layout.sidebar_width)
            .h_full()
            .border_r(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(t.space.sm)
                    .h(t.component.sidebar.title_bar)
                    .pl(t.space.lg)
                    .pr(t.space.sm)
                    .border_b(t.border_width)
                    .border_color(c.border_subtle)
                    .child(text("Memory", t.text.title, c.text_primary).flex_1())
                    .child(
                        icon_text_button("settings-memory-new", "plus", "New note", t)
                            .px(t.space.sm + t.space.xxs)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_new_note(&new_scope, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .pt(t.space.md)
                    .pb(t.space.sm)
                    .px(t.space.lg)
                    .child(text(description, detail(t), c.text_tertiary)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("memory-tree")
                            .debug_selector(|| "memory-tree".into())
                            .size_full()
                            .overflow_y_scroll()
                            .child(tree),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h(t.layout.fade_bottom)
                            .bg(gpui_kit::linear_gradient(
                                0.,
                                gpui_kit::linear_color_stop(c.surface_canvas, 0.),
                                gpui_kit::linear_color_stop(c.canvas_clear, 1.),
                            )),
                    ),
            );
        let content: AnyElement = if self.knowledge.settings_deleted {
            self.render_recently_deleted(t, cx)
        } else if let Some((scope, path)) = selected {
            if self
                .knowledge
                .history
                .as_ref()
                .is_some_and(|h| h.path == path && h.scope == scope)
            {
                self.render_history(t, cx)
            } else {
                self.render_settings_note(&scope, &path, t, window, cx)
            }
        } else {
            div()
                .p(t.space.page)
                .child(text(
                    "No memory notes yet. The assistant writes them as it learns about you. Select New note to write one.",
                    t.text.ui,
                    c.text_secondary,
                ))
                .into_any_element()
        };
        div()
            .debug_selector(|| "settings-memory".into())
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(column)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(content),
            )
            .into_any_element()
    }

    /// A note on the right of Settings > Memory: the path, who saved it,
    /// Edit, the menu, and the rendered text, or the editor.
    fn render_settings_note(
        &mut self,
        scope: &str,
        path: &str,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let now = consort_client::format::now_ms();
        let note = self.note(scope, path);
        let editing = self
            .knowledge
            .editor
            .as_ref()
            .filter(|e| e.place == Place::Settings && e.scope == scope && e.path == path)
            .cloned();
        let subtitle = match (&editing, &note) {
            (Some(e), _) if e.base_sha.is_none() => "A new note. Save it to keep it.".to_owned(),
            (Some(e), _) => format!(
                "Editing. Version {}, opened at {}",
                e.base_version,
                consort_client::format::clock_24(e.opened_at)
            ),
            (None, Some(n)) => m::saved_line(n, now),
            (None, None) => "This note is not here any more.".to_owned(),
        };
        let (edit_scope, edit_path) = (scope.to_owned(), path.to_owned());
        let (menu_scope, menu_path) = (scope.to_owned(), path.to_owned());
        let menu_open = self.menu.as_ref().is_some_and(|menu| {
            menu.items.first().is_some_and(|i| {
                i.action
                    == MenuAction::Knowledge(KnowledgeAction::NoteHistory(
                        scope.to_owned(),
                        path.to_owned(),
                    ))
            })
        });
        let header = div()
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
                    .child(
                        selectable(
                            SharedString::from(format!("note-path-{scope}-{path}")),
                            path.to_owned(),
                            t.text.title,
                            c.text_primary,
                        )
                        .font_family(t.font_mono.clone()),
                    )
                    .child(text(subtitle, detail(t), c.text_tertiary)),
            )
            .when(editing.is_none() && note.is_some(), |d| {
                d.child(
                    icon_text_button("settings-note-edit", "edit", "Edit", t)
                        .debug_selector(|| "settings-note-edit".into())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_note(Place::Settings, &edit_scope, &edit_path, window, cx)
                        })),
                )
                .child(
                    focusable(div().id("note-more"), t)
                        .debug_selector(|| "note-more".into())
                        .relative()
                        .size(t.space.xxl)
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(t.radius.md)
                        .border(t.border_width)
                        .border_color(c.border_strong)
                        .cursor_pointer()
                        .when(menu_open, |d| d.bg(c.surface_selected))
                        .when(!menu_open, |d| d.hover(|s| s.bg(c.hover_canvas)))
                        .child(self.triggers.track("note-more"))
                        .child(icon("more-horizontal", t.icon.size, c.text_primary))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(bounds) = this.triggers.get("note-more") {
                                this.open_menu(
                                    OpenMenu::under(
                                        bounds,
                                        note_menu(&menu_scope, &menu_path),
                                        false,
                                    ),
                                    cx,
                                );
                            }
                        })),
                )
            });
        let body: AnyElement = match (&editing, &note) {
            (Some(_), _) => self
                .note_editor(Place::Settings, t, window, cx)
                .into_any_element(),
            (None, Some(n)) => {
                let content = n.content.clone().unwrap_or_default();
                self.render_markdown(
                    &format!("settings-note-{}", n.key()),
                    &content,
                    note_menu(scope, path),
                    c.surface_card,
                    t,
                    cx,
                )
                .debug_selector(|| "settings-note-text".into())
                .into_any_element()
            }
            (None, None) => div().into_any_element(),
        };
        div()
            .id("settings-note")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(t.space.xl)
            .overflow_y_scroll()
            .pt(t.space.page - t.space.sm)
            .px(t.space.page)
            .pb(t.space.page)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xl)
                    .max_w(t.layout.timeline_max_width)
                    .child(header)
                    .child(body),
            )
            .into_any_element()
    }

    /// The version history of the open note: the list of versions, and a
    /// preview of one with its changed lines marked.
    fn render_history(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let k = t.component.knowledge;
        let Some(history) = self.knowledge.history.clone() else {
            return div().into_any_element();
        };
        let mut list = div()
            .id("version-list")
            .debug_selector(|| "version-list".into())
            .flex()
            .flex_col()
            .gap(t.space.xxs)
            .w(k.version_list)
            .flex_none()
            .overflow_y_scroll();
        let count = history.versions.len();
        for (i, version) in history.versions.iter().enumerate() {
            let chosen = i == history.selected;
            let selector = format!("version-{}", version.version);
            list = list.child(
                div()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector)
                    .flex()
                    .flex_col()
                    .py(t.space.sm)
                    .px(t.space.sm + t.space.xxs)
                    .rounded(t.radius.md)
                    .cursor_pointer()
                    .when(chosen, |d| d.bg(c.surface_selected))
                    .when(!chosen, |d| d.hover(|s| s.bg(c.hover_canvas)))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(t.space.xs + t.space.xxs)
                            .child(text(
                                format!("Version {}", version.version),
                                t.text.ui,
                                c.text_primary,
                            ))
                            .when(i == 0, |d| {
                                d.child(text("Current", detail(t), c.status_done))
                            }),
                    )
                    .child(text(
                        m::version_line(version, i + 1 == count),
                        detail(t),
                        c.text_tertiary,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(history) = this.knowledge.history.as_mut() {
                            history.selected = i;
                        }
                        cx.notify();
                    })),
            );
        }
        let current = history.versions.first().cloned();
        let shown = history.versions.get(history.selected).cloned();
        let preview: AnyElement = match (&shown, &current) {
            (Some(shown), Some(current)) => {
                let old = shown.content.clone().unwrap_or_default();
                let new = current.content.clone().unwrap_or_default();
                let changed = m::changed_lines(&old, &new);
                let is_current = shown.version == current.version;
                let mut lines = div()
                    .id("version-preview")
                    .debug_selector(|| "version-preview".into())
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py(t.space.md)
                    .font_family(t.font_mono.clone());
                for (i, text_line) in old.lines().enumerate() {
                    let marked = !is_current && changed.get(i).copied().unwrap_or(false);
                    lines = lines.child(
                        div()
                            .flex()
                            .when(marked, |d| d.bg(c.tint_working))
                            .child(
                                div()
                                    .w(k.change_bar)
                                    .flex_none()
                                    .when(marked, |d| d.bg(c.status_working)),
                            )
                            .child(
                                div()
                                    .debug_selector(move || {
                                        if marked {
                                            format!("changed-line-{i}")
                                        } else {
                                            format!("line-{i}")
                                        }
                                    })
                                    .flex_1()
                                    .min_w_0()
                                    .px(t.space.md)
                                    .type_style(t.text.mono)
                                    .text_color(c.text_primary)
                                    .child(if text_line.is_empty() {
                                        " ".to_owned()
                                    } else {
                                        text_line.to_owned()
                                    }),
                            ),
                    );
                }
                let next = current.version + 1;
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .rounded(t.radius.lg)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .bg(c.surface_card)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.md)
                            .py(t.space.sm + t.space.xxs)
                            .px(t.space.md)
                            .border_b(t.border_width)
                            .border_color(c.border_subtle)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(text(
                                        format!("Version {}", shown.version),
                                        t.text.ui_strong,
                                        c.text_primary,
                                    ))
                                    .child(text(
                                        if is_current {
                                            "This is the version that the assistant reads now."
                                                .to_owned()
                                        } else {
                                            format!(
                                                "A bar at the left marks each line that differs from version {}.",
                                                current.version
                                            )
                                        },
                                        detail(t),
                                        c.text_tertiary,
                                    )),
                            )
                            .when(!is_current, |d| {
                                d.child(
                                    button(
                                        "restore-version",
                                        "Restore this version",
                                        ButtonKind::Primary,
                                        t,
                                    )
                                    .debug_selector(|| "restore-version".into())
                                    .on_click(cx.listener(|this, _, _, cx| this.restore_version(cx))),
                                )
                            }),
                    )
                    .child(lines)
                    .when(!is_current, |d| {
                        d.child(
                            div()
                                .py(t.space.sm + t.space.xxs)
                                .px(t.space.md)
                                .border_t(t.border_width)
                                .border_color(c.border_subtle)
                                .child(text(
                                    format!(
                                        "Restore saves this text as version {next}. Version {} stays in the history.",
                                        current.version
                                    ),
                                    detail(t),
                                    c.text_tertiary,
                                )),
                        )
                    })
                    .into_any_element()
            }
            _ => div()
                .child(text(
                    if history.loading {
                        "Loading the versions".to_owned()
                    } else {
                        history
                            .error
                            .clone()
                            .unwrap_or_else(|| "This note has no versions.".to_owned())
                    },
                    t.text.ui,
                    c.text_secondary,
                ))
                .into_any_element(),
        };
        div()
            .debug_selector(|| "memory-history".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(t.space.xl)
            .pt(t.space.page - t.space.sm)
            .px(t.space.page)
            .pb(t.space.xl)
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
                            .child(
                                line(history.path.clone(), t.text.title, c.text_primary)
                                    .font_family(t.font_mono.clone()),
                            )
                            .child(text(
                                "Version history. Every save is kept. Restore saves an old version as the newest one.",
                                detail(t),
                                c.text_tertiary,
                            )),
                    )
                    .child(
                        button("close-history", "Close history", ButtonKind::Secondary, t)
                            .debug_selector(|| "close-history".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.knowledge.history = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap(t.space.xl)
                    .child(list)
                    .child(preview),
            )
            .into_any_element()
    }

    /// Recently deleted: the notes deleted in the last 30 days, each with
    /// Restore.
    fn render_recently_deleted(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let now = consort_client::format::now_ms();
        let mut deleted: Vec<MemoryNoteView> = self
            .state
            .memory_notes
            .iter()
            .filter(|n| n.deleted_at.is_some())
            .cloned()
            .collect();
        deleted.sort_by_key(|n| std::cmp::Reverse(n.deleted_at));
        let mut list = div().flex().flex_col().gap(t.space.xxs);
        if deleted.is_empty() {
            list = list.child(text(
                "No note was deleted in the last 30 days.",
                t.text.ui,
                c.text_secondary,
            ));
        }
        for note in deleted {
            let days_left = m::purge_label(note.deleted_at.unwrap_or(now), now);
            let (scope, path) = (note.scope.clone(), note.path.clone());
            let selector = format!("deleted-{}", note.key());
            list = list.child(
                div()
                    .debug_selector(move || selector)
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .py(t.space.sm)
                    .px(t.space.md)
                    .rounded(t.radius.md)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .child(icon("file", t.icon.size, c.text_secondary))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                line(note.path.clone(), t.text.mono, c.text_primary)
                                    .font_family(t.font_mono.clone()),
                            )
                            .child(text(
                                format!(
                                    "{}. Deleted {}. It goes for good {days_left}.",
                                    self.scope_name(&note.scope),
                                    match consort_client::format::file_date_label(
                                        note.deleted_at.unwrap_or(now),
                                        now
                                    )
                                    .as_str()
                                    {
                                        "Today" => "today".to_owned(),
                                        other => format!("on {other}"),
                                    }
                                ),
                                detail(t),
                                c.text_tertiary,
                            )),
                    )
                    .child(
                        button(
                            SharedString::from(format!("restore-{}", note.key())),
                            "Restore",
                            ButtonKind::Secondary,
                            t,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.run_knowledge_menu(
                                    KnowledgeAction::RestoreNote(scope.clone(), path.clone()),
                                    window,
                                    cx,
                                )
                            },
                        )),
                    ),
            );
        }
        div()
            .id("recently-deleted")
            .debug_selector(|| "recently-deleted".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(t.space.xl)
            .overflow_y_scroll()
            .pt(t.space.page - t.space.sm)
            .px(t.space.page)
            .pb(t.space.page)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .max_w(t.layout.timeline_max_width)
                    .child(text("Recently deleted", t.text.heading, c.text_primary))
                    .child(text(
                        "A deleted note stays here for 30 days, then it goes for good. The assistant does not read it while it is here. Restore brings it back as it was.",
                        t.text.ui,
                        c.text_secondary,
                    )),
            )
            .child(div().max_w(t.layout.timeline_max_width).child(list))
            .into_any_element()
    }

    /// The memory saved line in the timeline: a quiet line in the assistant
    /// lane with the `memory` icon and the note path as a link that opens
    /// the note.
    pub fn render_memory_saved_line(
        &self,
        notice: &consort_client::NoticeView,
        memory: &consort_client::MemorySavedView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let verb = if memory.created {
            "Saved the new memory note"
        } else {
            "Updated the memory note"
        };
        let (scope, path) = (memory.scope.clone(), memory.path.clone());
        let selector = format!("memory-saved-{}", notice.message_id);
        div()
            .id(SharedString::from(format!("notice-{}", notice.message_id)))
            .debug_selector(move || selector)
            .flex()
            .flex_wrap()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .child(icon(
                "memory",
                t.component.knowledge.line_icon,
                c.text_tertiary,
            ))
            .child(text(verb, detail(t), c.text_tertiary))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "memory-link-{}",
                                notice.message_id
                            )))
                            .debug_selector(|| "memory-saved-link".into())
                            .cursor_pointer()
                            .font_family(t.font_mono.clone())
                            .type_style(detail(t))
                            .text_color(c.text_secondary)
                            .underline()
                            .hover(|s| s.text_color(c.text_primary))
                            .child(path.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_note_in_panel(&scope, &path, window, cx)
                            })),
                    )
                    .when(!memory.suggested_by.is_empty(), |d| {
                        d.child(text(
                            format!(", from a suggestion by {}", memory.suggested_by),
                            detail(t),
                            c.text_tertiary,
                        ))
                    }),
            )
            .into_any_element()
    }

    /// The note dialogs: New note, Rename and the delete confirmation.
    pub fn memory_dialog(
        &mut self,
        dialog: &Dialog,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(String, String, gpui_kit::Pixels, Div, Div)> {
        let focused = {
            use gpui_kit::Focusable as _;
            self.knowledge_inputs
                .note_path
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        };
        match dialog {
            Dialog::NewNote(scope) => {
                let scope = scope.clone();
                let name = self.scope_name(&scope);
                Some((
                    "New note".to_owned(),
                    if scope == GLOBAL {
                        "The note goes in global memory. Use folders in the name to group notes, such as people/kevin.md.".to_owned()
                    } else {
                        format!(
                            "The note goes in the memory of {name}. Use folders in the name to group notes, such as vendors/lumen-co.md."
                        )
                    },
                    t.layout.dialog_width,
                    field_block(
                        "Name",
                        input_box(&self.knowledge_inputs.note_path, focused, t),
                        None,
                        t,
                    ),
                    self.dialog_actions("Write the note", t, cx, move |this, window, cx| {
                        let text = this.knowledge_inputs.note_path.read(cx).value().to_string();
                        match m::note_path(&text) {
                            Err(message) => {
                                this.dialog_error = Some(message);
                                cx.notify();
                            }
                            Ok(path)
                                if this
                                    .note(&scope, &path)
                                    .is_some_and(|n| n.deleted_at.is_none()) =>
                            {
                                this.dialog_error = Some(format!(
                                    "A note at {path} exists already. Choose another name."
                                ));
                                cx.notify();
                            }
                            Ok(path) => {
                                this.close_dialog(window, cx);
                                let place = if this.page == crate::app::Page::Settings {
                                    Place::Settings
                                } else {
                                    Place::Panel
                                };
                                match place {
                                    Place::Panel => {
                                        this.knowledge.panel_note =
                                            Some((scope.clone(), path.clone()))
                                    }
                                    Place::Settings => {
                                        this.knowledge.settings_note =
                                            Some((scope.clone(), path.clone()));
                                        this.knowledge.settings_deleted = false;
                                        this.knowledge.history = None;
                                    }
                                }
                                this.edit_note(place, &scope, &path, window, cx);
                            }
                        }
                    }),
                ))
            }
            Dialog::RenameNote(scope, path) => {
                let (scope, path) = (scope.clone(), path.clone());
                Some((
                    "Rename note".to_owned(),
                    "The note and its versions move to the new name. The assistant finds it there."
                        .to_owned(),
                    t.layout.dialog_width,
                    field_block(
                        "Name",
                        input_box(&self.knowledge_inputs.note_path, focused, t),
                        None,
                        t,
                    ),
                    self.dialog_actions("Rename", t, cx, move |this, window, cx| {
                        let text = this.knowledge_inputs.note_path.read(cx).value().to_string();
                        let to_path = match m::note_path(&text) {
                            Ok(p) => p,
                            Err(message) => {
                                this.dialog_error = Some(message);
                                cx.notify();
                                return;
                            }
                        };
                        let Some(note) = this.note(&scope, &path) else {
                            return;
                        };
                        if this.knowledge.panel_note == Some((scope.clone(), path.clone())) {
                            this.knowledge.panel_note = Some((scope.clone(), to_path.clone()));
                        }
                        if this.knowledge.settings_note == Some((scope.clone(), path.clone())) {
                            this.knowledge.settings_note = Some((scope.clone(), to_path.clone()));
                        }
                        let client = this.client.clone();
                        let (scope, from_path) = (scope.clone(), path.clone());
                        this.submit_dialog(
                            async move {
                                client
                                    .run(Command::MoveMemoryNote {
                                        scope,
                                        from_path,
                                        to_path,
                                        sha256: note.sha256,
                                    })
                                    .await
                            },
                            window,
                            cx,
                        );
                    }),
                ))
            }
            Dialog::ConfirmDeleteNote(scope, path) => {
                let (scope, path) = (scope.clone(), path.clone());
                Some((
                    format!("Delete {path}?"),
                    "The note goes to Recently deleted in Settings > Memory for 30 days. You can restore it there. The assistant does not read it while it is deleted.".to_owned(),
                    t.layout.dialog_width,
                    div(),
                    self.dialog_actions_kind(
                        "Delete note",
                        ButtonKind::Danger,
                        t,
                        cx,
                        move |this, window, cx| {
                            let Some(note) = this.note(&scope, &path) else {
                                return;
                            };
                            if this.knowledge.panel_note == Some((scope.clone(), path.clone())) {
                                this.knowledge.panel_note = None;
                            }
                            if this.knowledge.settings_note == Some((scope.clone(), path.clone())) {
                                this.knowledge.settings_note = None;
                            }
                            let client = this.client.clone();
                            let (scope, path) = (scope.clone(), path.clone());
                            this.submit_dialog(
                                async move {
                                    client
                                        .run(Command::DeleteMemoryNote {
                                            scope,
                                            path,
                                            sha256: note.sha256,
                                        })
                                        .await
                                },
                                window,
                                cx,
                            );
                        },
                    ),
                ))
            }
            _ => None,
        }
    }
}
