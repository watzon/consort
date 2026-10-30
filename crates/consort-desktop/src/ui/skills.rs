//! Settings > Skills, and its dialogs: Add a skill, the files of a skill,
//! and Remove.

use consort_client::{
    AuditStatusView, DirectoryEntryView, LoadState, SkillChanges, SkillInstallInput, SkillState,
    SkillUpdateView, SkillView, audit_summary, example_channel, skill_row_detail, skills_context,
    source_line, use_in_detail, use_in_match_note,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Dialog},
    extensions::{AddSkillTab, UseInTarget},
    ui::{
        add_account::AccountDialogParts,
        dialogs::{field_block, input_box},
        ext_parts::{
            dot, faded_scroll, hint, mono_block, page_section, ring, rows_frame, setting_row,
            use_in_control,
        },
        menu::{OpenMenu, skill_menu, skill_source_menu},
        parts::{
            ButtonKind, button, detail, error_box, focusable, icon, icon_button, line, selectable,
            text,
        },
        settings::{segmented, switch},
        window_controls::DragArea as _,
    },
};

/// The color of an audit result.
fn audit_color(status: AuditStatusView, t: &Tokens) -> Hsla {
    let c = &t.colors;
    match status {
        AuditStatusView::Passed => c.status_done,
        AuditStatusView::Warning => c.accent_text,
        AuditStatusView::Failed => c.status_failed,
        AuditStatusView::Unknown => c.text_tertiary,
    }
}

/// The dot of a skill's state in the list.
fn state_mark(state: SkillState, t: &Tokens) -> Div {
    let c = &t.colors;
    match state {
        SkillState::On => dot(c.status_done, t),
        SkillState::NewVersion => dot(c.accent_text, t),
        SkillState::Blocked => dot(c.status_failed, t),
        SkillState::Off => ring(c.text_tertiary, t),
    }
}

/// A small "scripts" badge after a skill name.
fn scripts_badge(t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex_none()
        .px(t.space.xs + t.space.xxs)
        .rounded(t.radius.sm)
        .border(t.border_width)
        .border_color(c.border_strong)
        .type_style(t.text.caption)
        .text_color(c.text_tertiary)
        .child("scripts")
}

/// A skill name: mono, as the assistant writes it.
fn skill_name(name: &str, style: consort_theme::Type, color: Hsla, t: &Tokens) -> Div {
    div()
        .type_style(style)
        .font_family(t.font_mono.clone())
        .text_color(color)
        .min_w_0()
        .truncate()
        .child(name.to_owned())
}

impl ConsortApp {
    /// Settings > Skills: the list and the page of the open skill.
    pub fn render_skills(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let selected = self
            .ext
            .skill
            .clone()
            .filter(|id| state.skill(id).is_some())
            .or_else(|| state.skills.first().map(|s| s.id.clone()));
        let mut rows = div().flex().flex_col().gap(t.space.xxs).p(t.space.sm);
        for skill in &state.skills {
            let is_selected = selected.as_deref() == Some(skill.id.as_str());
            let id = skill.id.clone();
            let menu_id = skill.id.clone();
            let enabled = skill.enabled;
            let skill_state = skill.state();
            rows = rows.child(
                focusable(
                    div().id(SharedString::from(format!("skill-{}", skill.id))),
                    t,
                )
                .flex()
                .items_start()
                .gap(t.space.sm + t.space.xxs)
                .py(t.space.sm + t.space.xxs)
                .px(t.space.sm)
                .rounded(t.radius.md)
                .cursor_pointer()
                .when(is_selected, |d| d.bg(c.surface_selected))
                .when(!is_selected, |d| d.hover(|s| s.bg(c.hover_canvas)))
                .child(
                    div()
                        .h(t.text.ui.line)
                        .w(t.icon.size)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(state_mark(skill_state, t)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(t.space.sm)
                                .child(skill_name(&skill.name, t.text.ui, c.text_primary, t))
                                .when(skill.has_scripts(), |d| d.child(scripts_badge(t))),
                        )
                        .child(text(
                            skill_row_detail(skill, &state.channels),
                            detail(t),
                            match skill_state {
                                SkillState::NewVersion => c.accent_text,
                                SkillState::Blocked => c.status_failed,
                                _ if is_selected => c.text_secondary,
                                _ => c.text_tertiary,
                            },
                        )),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ext.skill = Some(id.clone());
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        let items = skill_menu(&menu_id, enabled);
                        this.open_menu(OpenMenu::at(event.position, items), cx);
                        cx.stop_propagation();
                    }),
                ),
            );
        }
        let list = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(t.component.worker_list)
            .h_full()
            .border_r(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(t.space.page + t.space.xs)
                    .pl(t.space.lg)
                    .pr(t.space.sm)
                    .border_b(t.border_width)
                    .border_color(c.border_subtle)
                    .drag_area()
                    .child(div().flex_1().type_style(t.text.title).child("Skills"))
                    .child(
                        button("add-skill", "Add skill", ButtonKind::Secondary, t)
                            .child(icon("plus", t.icon.dense, c.text_primary))
                            .flex_row_reverse()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_dialog(Dialog::AddSkill, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .id("skills-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(rows)
                    .child(self.skill_sources_section(t, cx)),
            );
        let page = match selected.and_then(|id| state.skill(&id).cloned()) {
            Some(skill) => self.skill_page(&skill, t, cx),
            None => div()
                .flex_1()
                .p(t.layout.speaker_gutter)
                .flex()
                .flex_col()
                .items_start()
                .gap(t.space.md)
                .child(text(
                    "No skills yet. A skill teaches the assistant one kind of work, in every channel you choose.",
                    t.text.ui,
                    c.text_secondary,
                ))
                .child(
                    button("add-first-skill", "Add skill", ButtonKind::Primary, t).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.open_dialog(Dialog::AddSkill, window, cx)
                        }),
                    ),
                )
                .into_any_element(),
        };
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(list)
            .child(page)
            .into_any_element()
    }

    /// The sources that the hub searches, under the skill rows. The user can
    /// add a source, and remove one that they added.
    fn skill_sources_section(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let sources = self.state.skill_directory.sources.clone();
        let mut rows = div().flex().flex_col().gap(t.space.xxs);
        for source in &sources {
            let menu = skill_source_menu(source);
            let icon_name = match source.kind {
                consort_client::DirectoryKind::Smithery => "globe",
                _ => "git-branch",
            };
            let key = SharedString::from(format!("skill-source-{}", source.id));
            let selector = key.to_string();
            let mut row = div()
                .id(key)
                .debug_selector(move || selector.clone())
                .flex()
                .items_center()
                .gap(t.space.sm + t.space.xxs)
                .py(t.space.sm)
                .px(t.space.sm)
                .rounded(t.radius.md)
                .child(
                    div()
                        .h(t.text.ui.line)
                        .w(t.icon.size)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(icon_name, t.icon.dense, c.text_tertiary)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(line(source.name.clone(), t.text.ui, c.text_primary))
                        .child(line(
                            consort_client::repo_name(&source.url),
                            detail(t),
                            c.text_tertiary,
                        )),
                );
            if let Some(items) = menu {
                let more_items = items.clone();
                row = row
                    .hover(|s| s.bg(c.hover_canvas))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.open_menu(OpenMenu::at(event.position, items.clone()), cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "skill-source-more-{}",
                                source.id
                            )))
                            .flex_none()
                            .cursor_pointer()
                            .p(t.space.xs)
                            .rounded(t.radius.sm)
                            .hover(|s| s.bg(c.hover_raised))
                            .child(icon("ellipsis", t.icon.dense, c.text_secondary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    this.open_menu(
                                        OpenMenu::at(event.position, more_items.clone()),
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            ),
                    );
            }
            rows = rows.child(row);
        }
        // skills.sh is a link, not a source: its API is closed to the hub.
        rows = rows.child(
            div()
                .flex()
                .flex_col()
                .gap(t.space.xxs)
                .py(t.space.sm)
                .px(t.space.sm)
                .child(
                    focusable(div().id("skills-sh-link"), t)
                        .debug_selector(|| "skills-sh-link".into())
                        .flex()
                        .items_center()
                        .gap(t.space.sm + t.space.xxs)
                        .cursor_pointer()
                        .child(icon("external-link", t.icon.dense, c.accent_text))
                        .child(text("skills.sh", t.text.ui, c.accent_text))
                        .on_click(cx.listener(|_, _, _, cx| cx.open_url("https://skills.sh"))),
                )
                .child(hint(
                    "It has no public search. Paste owner/repo from it into the Git URL field of Add skill.",
                    t,
                )),
        );
        div()
            .flex()
            .flex_col()
            .gap(t.space.xs)
            .p(t.space.sm)
            .pt(t.space.md)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .px(t.space.sm)
                    .child(
                        div()
                            .flex_1()
                            .child(text("Sources", t.text.ui_strong, c.text_primary)),
                    )
                    .child(
                        button("add-skill-source", "Add source", ButtonKind::Secondary, t)
                            .debug_selector(|| "add-skill-source".into())
                            .child(icon("plus", t.icon.dense, c.text_primary))
                            .flex_row_reverse()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_dialog(Dialog::AddSkillSource, window, cx)
                            })),
                    ),
            )
            .child(div().px(t.space.sm).child(hint(
                "Add skill searches these. Add a git repo or a marketplace.json file.",
                t,
            )))
            .child(rows)
    }

    /// Add a skill source: one URL.
    pub fn add_skill_source_dialog(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let focused = self.is_skill_focused(&self.ext_inputs.source_url, window, cx);
        let body = field_block(
            "URL",
            input_box(&self.ext_inputs.source_url, focused, t),
            Some(
                "A git repo, or the URL of a marketplace.json file. The hub reads it once to check it. Skills that you installed from it stay when you remove it.",
            ),
            t,
        );
        let actions = self.dialog_actions("Add source", t, cx, |this, window, cx| {
            this.add_skill_source(window, cx)
        });
        (
            "Add a skill source".to_owned(),
            "Add skill searches this source with the others.".to_owned(),
            t.layout.dialog_width,
            body,
            actions,
            None,
        )
    }

    /// The page of one skill.
    fn skill_page(&mut self, skill: &SkillView, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let files_id = skill.id.clone();
        let more_id = skill.id.clone();
        let more_enabled = skill.enabled;
        let header = div()
            .flex()
            .items_start()
            .gap(t.space.lg)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.xs + t.space.xxs)
                    .child(skill_name(&skill.name, t.text.heading, c.text_primary, t))
                    .child(selectable(
                        SharedString::from(format!("skill-description-{}", skill.id)),
                        skill.description.clone(),
                        t.text.ui,
                        c.text_secondary,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .child(icon("git-branch", t.icon.dense, c.text_tertiary))
                            .child(selectable(
                                SharedString::from(format!("skill-source-{}", skill.id)),
                                source_line(skill),
                                detail(t),
                                c.text_tertiary,
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(t.space.sm)
                    .child(
                        button("skill-view-files", "View files", ButtonKind::Secondary, t)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_skill_files(files_id.clone(), window, cx)
                            })),
                    )
                    .child(
                        div()
                            .relative()
                            .child(self.triggers.track("skill-more"))
                            .child(
                                icon_button(
                                    "skill-more",
                                    "ellipsis",
                                    t.space.xxl,
                                    t.icon.size,
                                    c.text_secondary,
                                    c.hover_canvas,
                                    t,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if let Some(bounds) = this.triggers.get("skill-more") {
                                            let items = skill_menu(&more_id, more_enabled);
                                            this.open_menu(
                                                OpenMenu::under(bounds, items, false),
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            ),
                    ),
            );
        let toggle_id = skill.id.clone();
        let enabled = skill.enabled;
        let blocked = skill.blocked_reason.clone();
        let on_hint = match (&blocked, skill.has_scripts() && !enabled) {
            (Some(reason), _) => reason.clone(),
            (None, true) => "It has scripts, so it stays off until you turn it on. Turning it on allows its scripts.".to_owned(),
            (None, false) => "Turn it off to keep it installed but out of every session.".to_owned(),
        };
        let scope = rows_frame(t)
            .child(setting_row(
                "On",
                hint(on_hint, t),
                switch("skill-enabled", enabled, t).when(blocked.is_none(), |s| {
                    s.on_click(cx.listener(move |this, _, _, cx| {
                        this.update_skill(
                            toggle_id.clone(),
                            SkillChanges {
                                enabled: Some(!enabled),
                                ..SkillChanges::default()
                            },
                            cx,
                        )
                    }))
                }),
                false,
                t,
            ))
            .child(setting_row(
                "Use in",
                hint(use_in_detail(&skill.use_in, &state.channels), t),
                use_in_control(
                    UseInTarget::Skill(skill.id.clone()),
                    &skill.use_in,
                    false,
                    t,
                    self,
                    cx,
                ),
                true,
                t,
            ));
        // Safety: scripts, audits and updates.
        let scripts_hint: AnyElement = if skill.scripts.is_empty() {
            hint("No scripts. The skill is text that the assistant reads.", t).into_any_element()
        } else {
            let mut line_box = div().flex().flex_wrap();
            for (i, script) in skill.scripts.iter().enumerate() {
                if i > 0 {
                    line_box = line_box.child(hint(
                        if i + 1 == skill.scripts.len() {
                            " and "
                        } else {
                            ", "
                        },
                        t,
                    ));
                }
                line_box = line_box.child(
                    div()
                        .type_style(detail(t))
                        .font_family(t.font_mono.clone())
                        .text_color(c.text_secondary)
                        .child(script.clone()),
                );
            }
            line_box
                .child(hint(". They run inside the session's own approvals.", t))
                .into_any_element()
        };
        let scripts_state = if skill.scripts.is_empty() {
            String::new()
        } else if skill.enabled {
            if skill.installed_label.is_empty() {
                "You allowed them".to_owned()
            } else {
                format!("You allowed them on {}", skill.installed_label)
            }
        } else {
            "Waiting for your OK".to_owned()
        };
        let (audit_status, audit_text) = audit_summary(&skill.audits);
        let audit_sources: Vec<String> = skill.audits.iter().map(|a| a.source.clone()).collect();
        let audit_hint = if audit_sources.is_empty() {
            "The source gave no audit results.".to_owned()
        } else {
            format!(
                "{}, from {}.",
                consort_client::join_and(&audit_sources),
                skill.source.short()
            )
        };
        let update_id = skill.id.clone();
        let update_control: AnyElement = match &skill.update {
            SkillUpdateView::UpToDate => {
                text("Up to date", t.text.ui, c.text_secondary).into_any_element()
            }
            SkillUpdateView::CheckFailed { .. } => {
                text("Check failed", t.text.ui, c.status_failed).into_any_element()
            }
            SkillUpdateView::Available { commit, .. } => {
                let review_id = update_id.clone();
                div()
                    .flex()
                    .gap(t.space.sm)
                    .child(
                        button("skill-review", "Review changes", ButtonKind::Quiet, t).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.open_dialog(
                                    crate::app::Dialog::SkillDiff(review_id.clone()),
                                    window,
                                    cx,
                                )
                            }),
                        ),
                    )
                    .child(
                        button(
                            "skill-update",
                            match commit {
                                Some(commit) => format!("Update to {commit}"),
                                None => "Update".to_owned(),
                            },
                            ButtonKind::Secondary,
                            t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_skill(
                                update_id.clone(),
                                SkillChanges {
                                    accept_new_version: true,
                                    ..SkillChanges::default()
                                },
                                cx,
                            )
                        })),
                    )
                    .into_any_element()
            }
        };
        let update_hint = match &skill.update {
            SkillUpdateView::Available {
                changes,
                scripts_changed,
                ..
            } => {
                let files: Vec<String> = changes.iter().map(|(p, _)| p.clone()).collect();
                let mut s = format!(
                    "A new version waits for your review. It changes {}.",
                    consort_client::join_and(&files)
                );
                if *scripts_changed {
                    s.push_str(" It changes a script.");
                }
                s
            }
            SkillUpdateView::CheckFailed { message } => message.clone(),
            SkillUpdateView::UpToDate if skill.source.can_update() => {
                "Consort checks the source every day. A new version waits for your review."
                    .to_owned()
            }
            SkillUpdateView::UpToDate => "An uploaded skill gets no updates.".to_owned(),
        };
        let safety = rows_frame(t)
            .child(setting_row(
                "Scripts",
                scripts_hint,
                text(scripts_state, t.text.ui, c.text_secondary),
                false,
                t,
            ))
            .child(setting_row(
                "Security audits",
                hint(audit_hint, t),
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(dot(audit_color(audit_status, t), t))
                    .child(text(audit_text, t.text.ui, c.text_secondary)),
                false,
                t,
            ))
            .child(setting_row(
                "Updates",
                hint(update_hint, t),
                update_control,
                true,
                t,
            ));
        let sees = match example_channel(&skill.use_in, &state.channels) {
            Some(channel) => {
                let block = skills_context(&state, &channel.id);
                page_section(
                    "What the assistant sees",
                    "Sessions start with each name and description. The assistant reads the rest when it uses a skill. Each provider gets them in its own format.",
                    mono_block(
                        SharedString::from(format!("skills-context-{}", skill.id)),
                        if block.is_empty() {
                            format!("No skill is on in {}.", channel.name)
                        } else {
                            block
                        },
                        t,
                    ),
                    t,
                )
            }
            None => page_section(
                "What the assistant sees",
                "No channel is in this skill's scope yet, so no session gets it.",
                div(),
                t,
            ),
        };
        div()
            .id("skill-page")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .pt(t.space.page - t.space.sm)
            .px(t.space.page)
            .pb(t.space.page)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .max_w(t.component.settings_detail_max)
                    .gap(t.space.xxl + t.space.xs)
                    .child(header)
                    .child(page_section(
                        "Where the assistant can use it",
                        "Sessions in these channels get this skill, whichever provider runs them.",
                        scope,
                        t,
                    ))
                    .child(page_section(
                        "Safety",
                        "Consort pins the exact version and checks it before any session gets it.",
                        safety,
                        t,
                    ))
                    .child(sees),
            )
            .into_any_element()
    }

    /// One result of a directory search.
    fn directory_row(
        &mut self,
        entry: &DirectoryEntryView,
        last: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let (status, audit_text) = audit_summary(&entry.audits);
        let blocked = status == AuditStatusView::Failed;
        let checking = self.ext.add_skill.checking.as_deref() == Some(entry.entry_id.as_str());
        let source = SkillInstallInput::Directory {
            source: entry.source_id.clone(),
            entry_id: entry.entry_id.clone(),
        };
        let key = entry.entry_id.clone();
        let action: AnyElement = if entry.installed_skill_id.is_some() {
            text("Added", t.text.ui, c.status_done).into_any_element()
        } else if blocked {
            text("Blocked", t.text.ui, c.text_tertiary).into_any_element()
        } else {
            button(
                SharedString::from(format!("add-entry-{}", entry.entry_id)),
                if checking { "Checking" } else { "Add" },
                ButtonKind::Secondary,
                t,
            )
            .when(checking, |b| b.opacity(t.opacity.busy))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.preview_skill(source.clone(), key.clone(), cx)
            }))
            .into_any_element()
        };
        div()
            .flex()
            .items_center()
            .gap(t.space.lg)
            .py(t.space.md)
            .px(t.space.xl)
            .when(!last, |d| {
                d.border_b(t.border_width).border_color(c.border_subtle)
            })
            .hover(|s| s.bg(c.hover_raised))
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
                            .child(skill_name(&entry.name, t.text.ui, c.text_primary, t))
                            .children(
                                entry
                                    .author
                                    .as_ref()
                                    .map(|a| text(a.clone(), t.text.caption, c.text_tertiary)),
                            ),
                    )
                    .child(line(entry.description.clone(), detail(t), c.text_secondary)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(t.space.sm)
                    .w(t.component.tool_state)
                    .child(if entry.audits.is_empty() {
                        ring(c.text_tertiary, t)
                    } else {
                        dot(audit_color(status, t), t)
                    })
                    .child(text(
                        if status == AuditStatusView::Passed {
                            "Audits passed".to_owned()
                        } else {
                            audit_text
                        },
                        t.text.ui,
                        if entry.audits.is_empty() {
                            c.text_tertiary
                        } else {
                            audit_color(status, t)
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    .w(t.component.account_action - t.space.xl)
                    .child(action),
            )
    }

    /// Add a skill: search the directories, a git URL, or an upload.
    pub fn add_skill_dialog(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let c = &t.colors;
        if let Some((_, preview)) = self.ext.add_skill.preview.clone() {
            return self.skill_preview_parts(&preview, t, cx);
        }
        let tab = self.ext.add_skill.tab;
        let tabs = segmented(
            "add-skill-tab",
            &[
                (AddSkillTab::Directories, "Directories"),
                (AddSkillTab::Git, "Git URL"),
                (AddSkillTab::Upload, "Upload"),
            ],
            tab,
            t,
            cx,
            |_| t.text.ui,
            |this: &mut ConsortApp, tab, window, cx| {
                this.ext.add_skill.tab = tab;
                this.dialog_error = None;
                if tab == AddSkillTab::Git {
                    let input = this.ext_inputs.git_url.clone();
                    input.update(cx, |input, cx| input.focus(window, cx));
                }
                cx.notify();
            },
        );
        let directory = self.state.skill_directory.clone();
        let body = match tab {
            AddSkillTab::Directories => {
                let focused = self.is_skill_focused(&self.ext_inputs.skill_search, window, cx);
                let search = div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap(t.space.sm)
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
                    .child(icon("search", t.icon.dense, c.text_tertiary))
                    .child(crate::ui::parts::input(
                        &self.ext_inputs.skill_search,
                        t.text.ui,
                    ));
                let names: Vec<String> = directory.sources.iter().map(|s| s.name.clone()).collect();
                let note = if names.is_empty() {
                    "skills.sh has no public search yet. Paste an owner/repo from it under Git URL."
                        .to_owned()
                } else {
                    format!(
                        "Searching {}. skills.sh has no public search yet. Paste an owner/repo from it under Git URL.",
                        consort_client::join_and(&names)
                    )
                };
                let unavailable = (!directory.unavailable.is_empty()).then(|| {
                    format!(
                        "{} did not answer. Their skills are missing from this list.",
                        consort_client::join_and(&directory.unavailable)
                    )
                });
                let results: AnyElement = match &directory.load {
                    LoadState::Unsupported(message) | LoadState::Failed(message) => div()
                        .p(t.space.xl)
                        .child(error_box("directory-error", message.clone(), t))
                        .into_any_element(),
                    LoadState::Loading if directory.entries.is_empty() => div()
                        .p(t.space.xl)
                        .child(text("Searching", t.text.ui, c.text_tertiary))
                        .into_any_element(),
                    _ if directory.entries.is_empty() => div()
                        .p(t.space.xl)
                        .child(text(
                            if directory.search.trim().is_empty() {
                                "The directories have no skills to show.".to_owned()
                            } else {
                                format!(
                                    "No skill matches \"{}\". Try other words, or add it from a git URL.",
                                    directory.search.trim()
                                )
                            },
                            t.text.ui,
                            c.text_secondary,
                        ))
                        .into_any_element(),
                    _ => {
                        let mut list = div().flex().flex_col();
                        let n = directory.entries.len();
                        for (i, entry) in directory.entries.iter().enumerate() {
                            list = list.child(self.directory_row(entry, i + 1 == n, t, cx));
                        }
                        list.into_any_element()
                    }
                };
                let handle = self.ext.results_scroll.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.md)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .child(search)
                            .child(tabs),
                    )
                    .child(hint(note, t))
                    .children(unavailable.map(|u| text(u, detail(t), c.status_failed)))
                    .child(
                        faded_scroll(
                            "skill-results",
                            &handle,
                            t.component.skill_results,
                            results,
                            c.surface_raised,
                            c.raised_clear,
                            t,
                            cx,
                        )
                        .mx(-t.space.xl)
                        .mb(-t.space.xl)
                        .border_t(t.border_width)
                        .border_color(c.border_subtle),
                    )
            }
            AddSkillTab::Git => {
                let url_focused = self.is_skill_focused(&self.ext_inputs.git_url, window, cx);
                let path_focused = self.is_skill_focused(&self.ext_inputs.git_path, window, cx);
                let checking = self.ext.add_skill.checking.is_some();
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.md)
                    .child(div().flex().justify_end().child(tabs))
                    .child(crate::ui::dialogs::field_block(
                        "Git URL",
                        input_box(&self.ext_inputs.git_url, url_focused, t),
                        Some("A repo URL, such as https://github.com/anthropics/skills, or an owner/repo from skills.sh."),
                        t,
                    ))
                    .child(crate::ui::dialogs::field_block(
                        "Folder",
                        input_box(&self.ext_inputs.git_path, path_focused, t),
                        Some("Leave it empty when SKILL.md is at the top of the repo."),
                        t,
                    ))
                    .child(
                        div().flex().justify_end().child(
                            button("check-git-skill", if checking { "Checking" } else { "Check skill" }, ButtonKind::Primary, t)
                                .when(checking, |b| b.opacity(t.opacity.busy))
                                .on_click(cx.listener(|this, _, _, cx| this.check_git_skill(cx))),
                        ),
                    )
            }
            AddSkillTab::Upload => {
                let chosen = self.ext.add_skill.upload_path.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.md)
                    .child(div().flex().justify_end().child(tabs))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.md)
                            .p(t.space.lg)
                            .rounded(t.radius.lg)
                            .border(t.border_width)
                            .border_color(c.border_strong)
                            .bg(c.surface_card)
                            .child(icon("upload", t.icon.size, c.text_secondary))
                            .child(
                                text(
                                    chosen.clone().unwrap_or_else(|| {
                                        "A zip file with a SKILL.md at its top, at most 25 MB."
                                            .to_owned()
                                    }),
                                    t.text.ui,
                                    if chosen.is_some() {
                                        c.text_primary
                                    } else {
                                        c.text_secondary
                                    },
                                )
                                .flex_1()
                                .min_w_0(),
                            )
                            .child(
                                button("choose-zip", "Choose file", ButtonKind::Secondary, t)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.choose_skill_zip(cx)),
                                    ),
                            ),
                    )
                    .when_some(chosen, |d, path| {
                        d.child(div().flex().justify_end().child(
                            button("check-upload", "Check skill", ButtonKind::Primary, t).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.preview_skill(
                                        SkillInstallInput::Upload {
                                            file_path: path.clone(),
                                        },
                                        "upload".into(),
                                        cx,
                                    )
                                }),
                            ),
                        ))
                    })
            }
        };
        (
            "Add a skill".to_owned(),
            "A skill teaches the assistant how to do one kind of work. Consort installs it from its source and pins the version.".to_owned(),
            t.layout.dialog_catalog,
            body,
            div().hidden(),
            None,
        )
    }

    fn is_skill_focused(
        &self,
        input: &gpui_kit::Entity<gpui_kit::component::input::InputState>,
        window: &Window,
        cx: &gpui_kit::App,
    ) -> bool {
        use gpui_kit::Focusable as _;
        input.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Opens the system file dialog for a skill zip.
    fn choose_skill_zip(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.ext.add_skill.upload_path = Some(path.display().to_string());
                cx.notify();
            });
        })
        .detach();
    }

    /// The checked skill, before Add skill installs it.
    fn skill_preview_parts(
        &mut self,
        preview: &consort_client::SkillPreviewView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let c = &t.colors;
        let state = self.state.clone();
        let (audit_status, audit_text) = audit_summary(&preview.audits);
        let use_in = self.ext.add_skill.use_in.clone();
        let mut facts = rows_frame(t)
            .child(setting_row(
                "Version",
                hint(
                    format!(
                        "{}{} files.",
                        preview
                            .commit
                            .as_ref()
                            .map(|c| format!("Commit {c}. "))
                            .unwrap_or_default(),
                        preview.file_count
                    ),
                    t,
                ),
                text(
                    preview
                        .license
                        .clone()
                        .unwrap_or_else(|| "No license".into()),
                    t.text.ui,
                    c.text_secondary,
                ),
                false,
                t,
            ))
            .child(setting_row(
                "Scripts",
                hint(
                    if preview.scripts.is_empty() {
                        "No scripts. The skill is text that the assistant reads.".to_owned()
                    } else {
                        format!(
                            "{}. The skill stays off until you turn it on.",
                            consort_client::join_and(&preview.scripts)
                        )
                    },
                    t,
                ),
                div(),
                false,
                t,
            ))
            .child(setting_row(
                "Security audits",
                hint(
                    if preview.has_hooks {
                        "It had Claude hooks. Consort takes them out.".to_owned()
                    } else if preview.audits.is_empty() {
                        "No audits. Only a skill directory can give audit results.".to_owned()
                    } else {
                        "From the directory that lists it.".to_owned()
                    },
                    t,
                ),
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(dot(audit_color(audit_status, t), t))
                    .child(text(audit_text, t.text.ui, c.text_secondary)),
                false,
                t,
            ));
        facts = facts.child(setting_row(
            "Use in",
            hint(use_in_match_note(&use_in, &state.channels), t),
            use_in_control(UseInTarget::AddSkillDialog, &use_in, false, t, self, cx),
            true,
            t,
        ));
        let blocked = preview.blocked_reason.clone();
        let installed = preview.installed_skill_id.is_some();
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(selectable(
                "skill-preview-description",
                preview.description.clone(),
                t.text.ui,
                c.text_secondary,
            ))
            .child(facts)
            .children(blocked.clone().map(|r| error_box("skill-blocked", r, t)));
        let busy = self.dialog_busy();
        let actions = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                button("dialog-cancel", "Back", ButtonKind::Secondary, t).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.ext.add_skill.preview = None;
                        this.dialog_error = None;
                        cx.notify();
                    },
                )),
            )
            .when(blocked.is_none() && !installed, |d| {
                d.child(
                    button("dialog-confirm", "Add skill", ButtonKind::Primary, t)
                        .when(busy, |b| b.opacity(t.opacity.busy))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.install_previewed_skill(window, cx)
                        })),
                )
            });
        (
            format!("Add {}", preview.name),
            if installed {
                "You have this skill already.".to_owned()
            } else {
                "Check what it brings. Consort pins this version.".to_owned()
            },
            t.layout.dialog_wide,
            body,
            actions,
            None,
        )
    }

    /// The files of a skill: the list, and the open file.
    pub fn skill_files_dialog(
        &mut self,
        skill_id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let c = &t.colors;
        let name = self
            .state
            .skill(skill_id)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let files = self.ext.files.files.clone();
        let open = self.ext.files.open.clone();
        let mut list = div().flex().flex_col().gap(t.space.xxs);
        for file in &files {
            let path = file.path.clone();
            let is_open = open.as_ref().is_some_and(|o| o.path == file.path);
            list = list.child(
                focusable(
                    div().id(SharedString::from(format!("skill-file-{}", file.path))),
                    t,
                )
                .flex()
                .items_center()
                .gap(t.space.sm)
                .py(t.space.xs)
                .px(t.space.sm)
                .rounded(t.radius.md)
                .cursor_pointer()
                .when(is_open, |d| d.bg(c.surface_selected))
                .when(!is_open, |d| d.hover(|s| s.bg(c.hover_raised)))
                .child(icon(
                    if file.executable {
                        "terminal"
                    } else {
                        "file-text"
                    },
                    t.icon.dense,
                    c.text_secondary,
                ))
                .child(line(file.path.clone(), detail(t), c.text_primary))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.read_skill_file(path.clone(), cx)),
                ),
            );
        }
        let content: AnyElement = match &open {
            Some(file) if file.binary => text(
                "This file is not text, so it does not show here.",
                t.text.ui,
                c.text_tertiary,
            )
            .into_any_element(),
            Some(file) => {
                let mut body = file.content.clone().unwrap_or_default();
                if file.truncated {
                    body.push_str("\n\n[Consort cut this file here, because it is long.]");
                }
                div()
                    .font_family(t.font_mono.clone())
                    .child(selectable(
                        SharedString::from(format!("skill-file-content-{}", file.path)),
                        body,
                        t.text.mono,
                        c.text_primary,
                    ))
                    .into_any_element()
            }
            None => text("Loading the files", t.text.ui, c.text_tertiary).into_any_element(),
        };
        let body = div()
            .flex()
            .h(t.component.files_view)
            .gap(t.space.md)
            .child(
                div()
                    .id("skill-file-list")
                    .flex_none()
                    .w(t.component.files_list)
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                div()
                    .id("skill-file-view")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .p(t.space.md)
                    .rounded(t.radius.md)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .child(content),
            );
        let actions = div().flex().justify_end().child(
            button("dialog-confirm", "Done", ButtonKind::Primary, t)
                .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
        );
        (
            format!("Files of {name}"),
            "The version that sessions get now. Text files show here as they are.".to_owned(),
            t.layout.dialog_catalog,
            body,
            actions,
            None,
        )
    }

    /// Ask before a skill is removed.
    pub fn confirm_remove_skill_dialog(
        &mut self,
        id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let name = self
            .state
            .skill(id)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let id = id.to_owned();
        (
            format!("Remove {name}?"),
            "New sessions do not get it. Sessions that run now keep it until they end. To use it again, add it again.".to_owned(),
            t.layout.dialog_width,
            div(),
            self.dialog_actions_kind(
                "Remove skill",
                ButtonKind::Danger,
                t,
                cx,
                move |this, window, cx| {
                    let client = this.client.clone();
                    let id = id.clone();
                    this.submit_dialog(
                        async move {
                            client
                                .run(consort_client::Command::RemoveSkill { skill_id: id })
                                .await
                        },
                        window,
                        cx,
                    );
                },
            ),
            None,
        )
    }
}
