//! The task thread in bubbles (Paper `BDH-0`): the brief as the first
//! bubbles with the placement as a caption, the activity chip whose lines
//! open an item in full, and the task lines above approvals and decisions.
//! Also the archived tasks under the Resolved tab.

use consort_client::{ActivityLine, Isolation, RequestState, TaskStatus};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Panel},
    convo::ConvoAction,
    ui::parts::{ButtonKind, button, detail, focusable, icon, line, status_glyph, text},
};

/// The caption under the brief: where the task runs.
pub fn placement_caption(placement: Option<&consort_client::PlacementView>) -> String {
    let Some(p) = placement else {
        return "Waiting for a worker.".to_owned();
    };
    let isolation = match p.isolation {
        Isolation::InPlace => "in place in",
        Isolation::Worktree => "worktree of",
        Isolation::Clone => "clone of",
    };
    match (&p.path, &p.repo, &p.branch) {
        (path, _, Some(branch)) if !path.is_empty() => {
            format!("{}, {isolation} {path} at {branch}", p.worker_name)
        }
        (path, _, None) if !path.is_empty() => format!("{}, {isolation} {path}", p.worker_name),
        (_, Some(repo), _) => format!("{}, {isolation} {repo}", p.worker_name),
        _ => format!("{}, in a new empty folder", p.worker_name),
    }
}

impl ConsortApp {
    /// The brief of the open task as its first bubbles, with the placement
    /// under them.
    pub fn render_brief_bubbles(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let chat = t.component.chat;
        let Panel::Thread(task_id) = self.panel.clone() else {
            return div().into_any_element();
        };
        let Some(thread) = self.state.thread(&task_id).cloned() else {
            return div().into_any_element();
        };
        // The brief is one bubble (DECISIONS 731).
        let parts: Vec<String> = Some(thread.brief.trim().to_owned())
            .filter(|p| !p.is_empty())
            .into_iter()
            .collect();
        let max = self.right_panel_width(t) - t.space.page;
        let mut col = div().flex().flex_col().items_start().gap(t.space.xs);
        let count = parts.len();
        for (i, part) in parts.into_iter().enumerate() {
            let key = format!("brief-{task_id}.{i}");
            let md = self.render_markdown(
                &key,
                &part,
                crate::ui::timeline::post_menu(&part),
                c.surface_raised,
                t,
                cx,
            );
            col = col.child(
                div()
                    .max_w(max)
                    .min_w_0()
                    .py(chat.bubble_pad_y)
                    .px(chat.bubble_pad_x)
                    .rounded(chat.bubble_radius)
                    .when(i + 1 == count, |d| d.rounded_bl(chat.bubble_tail))
                    .bg(c.surface_card)
                    .type_style(t.text.body)
                    .text_color(c.text_primary)
                    .child(md),
            );
        }
        if thread.text_is_preview {
            col = col.child(text("Loading the whole brief.", detail(t), c.text_tertiary));
        }
        col.child(
            div()
                .flex()
                .flex_wrap()
                .pt(t.space.xxs)
                .type_style(detail(t))
                .text_color(c.text_tertiary)
                .child(placement_caption(thread.placement.as_ref())),
        )
        .into_any_element()
    }

    /// The activity of a task folded into one chip. Open, it lists the
    /// commands and the file changes. A click on one opens it in full.
    pub fn render_activity_chip(
        &mut self,
        key: &str,
        summary: &str,
        items: &[ActivityLine],
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let open = self.activity_open.iter().any(|k| k == key);
        let toggle_key = key.to_owned();
        let task_id = match &self.panel {
            Panel::Thread(id) => id.clone(),
            _ => String::new(),
        };
        let chip = focusable(div().id(SharedString::from(format!("{key}-activity"))), t)
            .flex()
            .items_center()
            .gap(t.space.sm)
            .py(t.space.xs + t.space.xxs)
            .px(t.space.sm + t.space.xxs)
            .rounded(t.radius.md)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_sidebar))
            .child(icon("terminal", t.icon.dense, c.text_tertiary))
            .child(text(summary.to_owned(), detail(t), c.text_secondary))
            .child(icon(
                if open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                t.icon.dense,
                c.text_tertiary,
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(pos) = this.activity_open.iter().position(|k| *k == toggle_key) {
                    this.activity_open.remove(pos);
                } else {
                    this.activity_open.push(toggle_key.clone());
                }
                cx.notify();
            }));
        let mut list = div().flex().flex_col().gap(t.space.xxs).w_full();
        if open {
            for (n, item) in items.iter().enumerate() {
                let opens = !item.item_id.is_empty();
                let (task, session, item_id) = (
                    task_id.clone(),
                    item.session_id.clone(),
                    item.item_id.clone(),
                );
                list = list.child(
                    focusable(div().id(SharedString::from(format!("{key}-item-{n}"))), t)
                        .flex()
                        .items_center()
                        .gap(t.space.sm)
                        .py(t.space.xxs)
                        .px(t.space.sm)
                        .rounded(t.radius.sm)
                        .when(opens, |d| {
                            d.cursor_pointer().hover(|s| s.bg(c.hover_sidebar))
                        })
                        .child(
                            line(item.text.clone(), detail(t), c.text_secondary)
                                .font_family(t.font_mono.clone())
                                .flex_1(),
                        )
                        .when(opens, |d| {
                            d.child(icon("chevron-right", t.icon.dense, c.text_tertiary))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_task_item(
                                        task.clone(),
                                        session.clone(),
                                        item_id.clone(),
                                        cx,
                                    )
                                }))
                        }),
                );
            }
        }
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(t.space.xs)
            .child(chip)
            .when(open, |d| d.child(list))
            .into_any_element()
    }

    /// The line above an approval or a decision in a task thread: its kind,
    /// and "Needs you" while it waits.
    pub fn request_line(
        &mut self,
        key: &str,
        kind: &str,
        state: RequestState,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let waiting = matches!(state, RequestState::Pending | RequestState::Sending);
        div()
            .flex()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .child(status_glyph(
                SharedString::from(format!("{key}-line")),
                crate::ui::panels::request_glyph(state),
                t.icon.dense,
                t,
                cx,
            ))
            .child(text(kind.to_owned(), t.text.ui_strong, c.text_primary))
            .when(waiting, |d| {
                d.child(text("Needs you", t.text.ui, c.accent_text))
            })
            .into_any_element()
    }

    /// The archived tasks, at the end of the Resolved tab: a button that
    /// loads them, then each with Restore.
    pub fn render_archived_tasks(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let archived = self.state.convo.archived.clone();
        let mut section = div().flex().flex_col().px(t.space.sm).pt(t.space.lg).child(
            div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .py(t.space.xs)
                .px(t.space.sm)
                .child(text("Archived", t.text.label, c.text_tertiary).flex_1())
                .child(
                    button(
                        "archived-toggle",
                        if self.convo.archived_open {
                            "Hide"
                        } else {
                            "Show archived tasks"
                        },
                        ButtonKind::Quiet,
                        t,
                    )
                    .py(t.space.xxs)
                    .debug_selector(|| "archived-toggle".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.convo.archived_open = !this.convo.archived_open;
                        if this.convo.archived_open {
                            let client = this.client.clone();
                            this.spawn_command(cx, async move {
                                client
                                    .run(consort_client::Command::LoadArchivedTasks { more: false })
                                    .await
                            });
                        }
                        cx.notify();
                    })),
                ),
        );
        if !self.convo.archived_open {
            return section.into_any_element();
        }
        if archived.tasks.is_empty() {
            let line_text = match &archived.load {
                consort_client::LoadState::Loading => "Loading the archived tasks.".to_owned(),
                consort_client::LoadState::Failed(message) => message.clone(),
                _ => "No archived tasks. Archive a failed or resolved task to hide it from the lists.".to_owned(),
            };
            section = section.child(text(line_text, t.text.ui, c.text_secondary).px(t.space.sm));
        }
        for task in &archived.tasks {
            let id = task.id.clone();
            section = section.child(
                div()
                    .id(SharedString::from(format!("archived-task-{}", task.id)))
                    .flex()
                    .items_start()
                    .gap(t.space.sm + t.space.xxs)
                    .p(t.space.sm)
                    .rounded(t.radius.md)
                    .hover(|s| s.bg(c.hover_sidebar))
                    .child(status_glyph(
                        SharedString::from(format!("archived-glyph-{}", task.id)),
                        if task.status == TaskStatus::Resolved {
                            TaskStatus::Resolved
                        } else {
                            task.status
                        },
                        t.icon.size,
                        t,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(line(task.title.clone(), t.text.ui_strong, c.text_primary))
                            .child(line(
                                format!("{}. {}", task.channel_name, task.archived_label),
                                detail(t),
                                c.text_tertiary,
                            )),
                    )
                    .child(
                        button(
                            SharedString::from(format!("restore-{}", task.id)),
                            "Restore",
                            ButtonKind::Secondary,
                            t,
                        )
                        .debug_selector(|| "archived-restore".into())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.run_convo(ConvoAction::RestoreTask(id.clone()), window, cx)
                            },
                        )),
                    ),
            );
        }
        if archived.has_more {
            section = section.child(
                button("archived-more", "Show more", ButtonKind::Quiet, t).on_click(cx.listener(
                    |this, _, _, cx| {
                        let client = this.client.clone();
                        this.spawn_command(cx, async move {
                            client
                                .run(consort_client::Command::LoadArchivedTasks { more: true })
                                .await
                        });
                    },
                )),
            );
        }
        section.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::placement_caption;
    use consort_client::{Isolation, PlacementView};

    #[test]
    fn the_caption_names_the_worker_and_the_workspace() {
        let p = PlacementView {
            worker_name: "bigboi".into(),
            isolation: Isolation::Worktree,
            path: r"C:\Code\sayso".into(),
            branch: Some("fix/37".into()),
            ..PlacementView::default()
        };
        assert_eq!(
            placement_caption(Some(&p)),
            r"bigboi, worktree of C:\Code\sayso at fix/37"
        );
        let empty = PlacementView {
            worker_name: "hub".into(),
            ..PlacementView::default()
        };
        assert_eq!(
            placement_caption(Some(&empty)),
            "hub, in a new empty folder"
        );
        assert_eq!(placement_caption(None), "Waiting for a worker.");
    }
}
