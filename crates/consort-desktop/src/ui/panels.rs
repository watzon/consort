//! The side panels: the Tasks panel and the task thread.

use consort_client::{
    DecisionView, PostBody, RequestState, Speaker, TaskStatus, TaskView, ThreadPost,
    plan::PlanStepState,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window, div, list,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Panel, TasksTab},
    ui::{
        menu::{MenuAction, OpenMenu},
        parts::{
            ButtonKind, button, detail, focusable, icon, icon_button, leaving_link, line,
            selectable, status_color, status_glyph, text,
        },
        timeline::{open_context_menu, task_menu},
    },
};

/// The glyph of an approval or a decision: the fermata while it waits,
/// the resolved disc once answered, and the quiet ring once it closed.
pub(crate) fn request_glyph(state: RequestState) -> TaskStatus {
    match state {
        RequestState::Pending | RequestState::Sending => TaskStatus::NeedsInput,
        RequestState::Answered => TaskStatus::Resolved,
        RequestState::Expired | RequestState::Closed => TaskStatus::UpNext,
    }
}

/// The panel frame: 400 px, `surface.sidebar`, a border at the left.
fn frame(width: gpui_kit::Pixels, t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(width)
        .h_full()
        .bg(t.colors.surface_sidebar)
        .border_l(t.border_width)
        .border_color(t.colors.border_subtle)
}

/// How a section of the thread panel looks: its label, its toggle and where
/// it sits.
struct TextSection {
    /// The key of its Markdown text.
    key: String,
    toggle_id: &'static str,
    label: &'static str,
    open: bool,
    /// The section sits below the posts, so its border is above it.
    above: bool,
    /// The client holds only the start of the text and is loading the rest.
    loading: bool,
}

/// Adds `id` to `list`, or takes it out when it is there.
pub fn toggle_id(list: &mut Vec<String>, id: &str) {
    if let Some(at) = list.iter().position(|x| x == id) {
        list.remove(at);
    } else {
        list.push(id.to_owned());
    }
}

impl ConsortApp {
    pub fn render_tasks_panel(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let panel = &state.tasks_panel;
        let need_you = panel.need_you;
        let tabs = [
            (TasksTab::Active, "Active", Some(panel.active_count)),
            (TasksTab::UpNext, "Up next", Some(panel.up_next_count)),
            (
                TasksTab::Scheduled,
                "Scheduled",
                Some(panel.scheduled_count),
            ),
            (TasksTab::Resolved, "Resolved", None),
        ];
        let mut segmented = div()
            .flex()
            .flex_1()
            .min_w_0()
            .gap(t.space.xxs)
            .p(t.space.xxs)
            .bg(c.surface_card)
            .rounded(t.radius.md);
        for (tab, label, count) in tabs {
            let active = self.tasks_tab == tab;
            segmented = segmented.child(
                focusable(div().id(SharedString::from(format!("tab-{label}"))), t)
                    .flex()
                    .flex_1()
                    .whitespace_nowrap()
                    .justify_center()
                    .gap(t.space.xs + t.space.xxs)
                    .py(t.space.xs)
                    .rounded(t.radius.sm)
                    .cursor_pointer()
                    .when(active, |d| d.bg(c.surface_raised))
                    .when(!active, |d| d.hover(|s| s.bg(c.hover_card)))
                    .child(text(
                        label,
                        if active { t.text.ui_strong } else { t.text.ui },
                        if active {
                            c.text_primary
                        } else {
                            c.text_secondary
                        },
                    ))
                    .when_some(count.filter(|n| *n > 0), |d, n| {
                        d.child(text(
                            n.to_string(),
                            t.text.ui,
                            if active {
                                c.text_secondary
                            } else {
                                c.text_tertiary
                            },
                        ))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tasks_tab = tab;
                        cx.notify();
                    })),
            );
        }
        let sections: Vec<(String, Vec<String>)> = match self.tasks_tab {
            TasksTab::Active => panel
                .active
                .iter()
                .map(|s| (s.kind.label().to_owned(), s.task_ids.clone()))
                .collect(),
            TasksTab::UpNext => vec![("Up next".into(), panel.up_next.clone())],
            TasksTab::Scheduled => vec![("Scheduled".into(), panel.scheduled.clone())],
            TasksTab::Resolved => vec![("Resolved".into(), panel.resolved.clone())],
        };
        let mut body = div().flex().flex_col();
        let mut any = false;
        for (i, (label, ids)) in sections.iter().enumerate() {
            if ids.is_empty() {
                continue;
            }
            any = true;
            let mut section = div()
                .flex()
                .flex_col()
                .px(t.space.sm)
                .pt(if i == 0 { t.space.md } else { t.space.lg })
                .child(
                    div()
                        .flex()
                        .gap(t.space.xs + t.space.xxs)
                        .py(t.space.xs)
                        .px(t.space.sm)
                        .child(text(label.clone(), t.text.label, c.text_tertiary))
                        .child(text(ids.len().to_string(), detail(t), c.text_tertiary)),
                );
            for id in ids {
                if let Some(task) = state.task(id) {
                    section = section.child(self.task_row(task, t, cx));
                }
            }
            body = body.child(section);
        }
        if self.tasks_tab == TasksTab::Resolved {
            body = body.child(self.render_archived_tasks(t, cx));
        }
        if !any && self.tasks_tab != TasksTab::Resolved {
            let (title, line_text) = match self.tasks_tab {
                TasksTab::Active => (
                    "No active tasks",
                    "Ask Consort to start one, or start one with New task.",
                ),
                TasksTab::UpNext => (
                    "Nothing is up next",
                    "Tasks wait here for a worker to come online.",
                ),
                TasksTab::Scheduled => (
                    "Nothing is scheduled",
                    "Ask Consort to start a task at a later time.",
                ),
                TasksTab::Resolved => ("No resolved tasks", "Tasks that you resolve move here."),
            };
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .px(t.space.lg)
                    .pt(t.space.xl)
                    .child(text(title, t.text.ui_strong, c.text_primary))
                    .child(text(line_text, t.text.ui, c.text_secondary)),
            );
        }
        frame(self.right_panel_width(t), t)
            .child(
                crate::ui::parts::panel_header(t)
                    .child(
                        div()
                            .type_style(t.text.title)
                            .line_height(t.text.body.line)
                            .child("Tasks"),
                    )
                    // How many tasks need you. The header of the column
                    // said it before the redesign; the Tasks button has only
                    // a dot now.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pl(t.space.xs)
                            .when(need_you > 0, |d| {
                                d.child(text(
                                    format!("{need_you} need you"),
                                    t.text.ui,
                                    c.accent_text,
                                ))
                            }),
                    )
                    .child(
                        icon_button(
                            "tasks-new",
                            "plus",
                            t.space.xxl,
                            t.icon.size,
                            c.text_secondary,
                            c.hover_sidebar,
                            t,
                            cx,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_new_task(window, cx)),
                        ),
                    )
                    .child(
                        icon_button(
                            "tasks-close",
                            "x",
                            t.space.xxl,
                            t.icon.size,
                            c.text_secondary,
                            c.hover_sidebar,
                            t,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx))),
                    ),
            )
            .child(crate::ui::parts::panel_tabs(segmented, t))
            .child(
                div()
                    .id("tasks-scroll")
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pb(t.space.lg)
                    .child(body),
            )
            .into_any_element()
    }

    fn task_row(&mut self, task: &TaskView, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let selected = self.last_thread.as_deref() == Some(task.id.as_str());
        let id = task.id.clone();
        let menu_id = task.id.clone();
        let status = task.status;
        let ended = task.session_ended;
        // `text.tertiary` never sits on `surface.selected`.
        let quiet = if selected {
            c.text_secondary
        } else {
            c.text_tertiary
        };
        focusable(
            div().id(SharedString::from(format!("task-row-{}", task.id))),
            t,
        )
        .flex()
        .items_start()
        .gap(t.space.sm + t.space.xxs)
        .p(t.space.sm)
        .rounded(t.radius.md)
        .cursor_pointer()
        .when(selected, |d| d.bg(c.surface_selected))
        .when(!selected, |d| d.hover(|s| s.bg(c.hover_sidebar)))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(t.text.ui.line)
                .w(t.icon.size)
                .child(status_glyph(
                    SharedString::from(format!("row-glyph-{}", task.id)),
                    task.status,
                    t.icon.size,
                    t,
                    cx,
                )),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(line(task.title.clone(), t.text.ui_strong, c.text_primary))
                .child(line(task.meta.clone(), detail(t), quiet)),
        )
        .child(
            div()
                .flex_none()
                .w(t.space.page)
                .text_right()
                .type_style(detail(t))
                .line_height(t.text.ui.line)
                .text_color(quiet)
                .child(task.age_label.clone()),
        )
        .on_click(cx.listener(move |this, _, window, cx| this.open_thread(id.clone(), window, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                open_context_menu(
                    this,
                    event,
                    task_menu(&menu_id, status, ended, true),
                    window,
                    cx,
                )
            }),
        )
        .into_any_element()
    }

    pub fn render_thread_panel(
        &mut self,
        task_id: &str,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(item) = self
            .state
            .convo
            .item
            .clone()
            .filter(|i| i.task_id == task_id)
        {
            return self.render_item_view(item, t, window, cx);
        }
        let c = &t.colors;
        let state = self.state.clone();
        let task = state.task(task_id).cloned();
        let thread = state.thread(task_id).cloned();
        let session_ended = task.as_ref().is_some_and(|t| t.session_ended);
        let title = task.as_ref().map(|t| t.title.clone()).unwrap_or_default();
        let status = task.as_ref().map(|t| t.status).unwrap_or_default();
        let status_detail = thread
            .as_ref()
            .map(|t| t.status_detail.clone())
            .unwrap_or_default();
        let header_status = consort_client::plan::header_status(
            status,
            thread.as_ref().and_then(|t| t.plan.as_ref()),
        );
        let mode_tag = thread.as_ref().map(|t| t.approval_mode.tag());
        let menu_task = task_id.to_owned();
        let more_trigger = self.triggers.clone();

        let head = crate::ui::parts::panel_header(t)
            .gap(t.space.sm)
            .pl(t.space.sm)
            .pr(t.space.sm)
            .child(
                icon_button(
                    "thread-back",
                    "chevron-left",
                    t.space.xxl,
                    t.icon.size,
                    c.text_secondary,
                    c.hover_sidebar,
                    t,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Panel::Thread(id) = &this.panel {
                        this.last_thread = Some(id.clone());
                    }
                    this.panel = Panel::Tasks;
                    this.sync_lists();
                    cx.notify();
                })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(line(title, t.text.ui, c.text_primary).font_weight(t.text.title.weight))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.xs + t.space.xxs)
                            .child(status_glyph("thread-status", status, t.space.md, t, cx))
                            .child(line(
                                header_status.clone(),
                                detail(t),
                                // Only the two statuses that wait for the
                                // user use the accent. Working is quiet.
                                if status == TaskStatus::Working {
                                    c.text_tertiary
                                } else {
                                    status_color(status, t)
                                },
                            ))
                            .when(!status_detail.is_empty(), |d| {
                                d.child(line(status_detail, detail(t), c.text_tertiary))
                            })
                            // How much the task may do without asking. The
                            // worker can cap it.
                            .when_some(mode_tag, |d, tag| {
                                d.child(
                                    text(tag, detail(t), c.text_tertiary)
                                        .flex_none()
                                        .whitespace_nowrap(),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .relative()
                    .child(more_trigger.track("thread-more"))
                    .child(
                        icon_button(
                            "thread-more",
                            "ellipsis",
                            t.space.xxl,
                            t.icon.size,
                            c.text_secondary,
                            c.hover_sidebar,
                            t,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(bounds) = this.triggers.get("thread-more") {
                                // The thread is open, so the menu has no Open thread.
                                let items = task_menu(&menu_task, status, session_ended, false);
                                this.open_menu(OpenMenu::under(bounds, items, false), cx);
                            }
                        })),
                    ),
            )
            .child(
                icon_button(
                    "thread-close",
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

        // Why the task failed or waits, in full. The text is selectable so the
        // user can copy it, and it scrolls when it is long.
        let reason = thread
            .as_ref()
            .and_then(|th| th.reason.as_deref().map(reason_text))
            .map(|reason| {
                let body = div()
                    .id("thread-reason")
                    .max_h(t.layout.error_text_max_height)
                    .overflow_y_scroll()
                    .child(selectable(
                        SharedString::from(format!("thread-reason-{task_id}")),
                        reason,
                        t.text.ui,
                        c.text_primary,
                    ));
                div()
                    .flex_none()
                    .py(t.space.md)
                    .px(t.space.lg)
                    .border_b(t.border_width)
                    .border_color(c.border_subtle)
                    .child(if status == TaskStatus::Failed {
                        div()
                            .flex()
                            .gap(t.space.sm)
                            .p(t.space.md)
                            .rounded(t.radius.md)
                            .bg(c.tint_failed)
                            .child(icon("circle-alert", t.icon.size, c.status_failed))
                            .child(body.flex_1().min_w_0())
                    } else {
                        div().child(body)
                    })
            });

        let loading = thread.as_ref().is_some_and(|th| th.text_is_preview);
        // What the task said when it went to Ready for review, in full. It
        // sits above the Resolve bar, open by default, so the user reads it
        // before they resolve the task.
        let summary = thread
            .as_ref()
            .filter(|_| matches!(status, TaskStatus::Review | TaskStatus::Resolved))
            .and_then(|th| th.review_summary.clone())
            .map(|summary| {
                let open = !self.summary_closed.iter().any(|id| id == task_id);
                self.render_text_section(
                    TextSection {
                        key: format!("thread-summary-{task_id}"),
                        toggle_id: "thread-summary-toggle",
                        label: "Summary from the task",
                        open,
                        above: true,
                        loading,
                    },
                    &summary,
                    t,
                    cx,
                    {
                        let task = task_id.to_owned();
                        move |this: &mut ConsortApp| toggle_id(&mut this.summary_closed, &task)
                    },
                )
            });

        let count = self.thread_keys.len();
        let posts = list(
            self.thread.clone(),
            cx.processor(move |this, ix: usize, window, cx| {
                let t = ConsortApp::tokens(cx);
                let post = if this.thread_has_brief() && ix == 0 {
                    this.render_brief_bubbles(&t, cx)
                } else if this.thread_is_plan(ix) {
                    this.render_plan_card(&t, cx)
                } else if this.thread_is_writing(ix) {
                    div()
                        .flex()
                        .child(this.render_writing_bubble(&t, cx))
                        .into_any_element()
                } else {
                    this.thread_post(ix)
                        .map(|post| this.render_post(&post, &t, window, cx))
                        .unwrap_or_else(|| div().into_any_element())
                };
                div()
                    .px(t.space.lg)
                    .when(ix == 0, |d| d.pt(t.space.lg))
                    .when(ix + 1 < count, |d| d.pb(t.space.lg + t.space.xs))
                    .when(ix + 1 == count, |d| d.pb(t.space.lg))
                    .child(post)
                    .into_any_element()
            }),
        )
        .size_full();

        let column = self.right_panel_width(t);
        let reply = self.render_composer_box(
            crate::ui::composer::ComposerTarget::Task(task_id.to_owned()),
            column,
            None,
            t,
            window,
            cx,
        );

        let fade = |top: bool| {
            let (from, to, angle) = (
                c.surface_sidebar,
                c.sidebar_clear,
                if top { 180. } else { 0. },
            );
            div()
                .absolute()
                .left_0()
                .right_0()
                .h(if top { t.space.xl } else { t.space.xxl })
                .when(top, |d| d.top_0())
                .when(!top, |d| d.bottom(-t.space.sm))
                .bg(gpui_kit::linear_gradient(
                    angle,
                    gpui_kit::linear_color_stop(from, 0.),
                    gpui_kit::linear_color_stop(to, 1.),
                ))
        };

        // Ready for review: the answer is the user's, so it sits by the reply
        // box. Resolved: Not done puts the task back in review.
        let review = match status {
            TaskStatus::Review | TaskStatus::Resolved => {
                let resolved = status == TaskStatus::Resolved;
                let id = task_id.to_owned();
                Some(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(t.space.sm)
                        .mx(t.space.lg)
                        .mb(t.space.sm)
                        .py(t.space.sm)
                        .px(t.space.md)
                        .rounded(t.radius.lg)
                        .border(t.border_width)
                        .border_color(if resolved {
                            c.border_subtle
                        } else {
                            c.accent_text
                        })
                        .bg(c.surface_card)
                        .child(status_glyph("review-bar", status, t.icon.size, t, cx))
                        .child(
                            text(
                                if resolved {
                                    "You resolved this task."
                                } else {
                                    "Ready for review. Resolve it when the work is done."
                                },
                                t.text.ui,
                                c.text_secondary,
                            )
                            .flex_1()
                            .min_w_0(),
                        )
                        .child(
                            button(
                                "review-action",
                                if resolved { "Not done" } else { "Resolve" },
                                if resolved {
                                    ButtonKind::Secondary
                                } else {
                                    ButtonKind::Primary
                                },
                                t,
                            )
                            .debug_selector(|| "review-action".into())
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    let action = if resolved {
                                        MenuAction::ReopenTask(id.clone())
                                    } else {
                                        MenuAction::ResolveTask(id.clone())
                                    };
                                    this.run_menu(action, window, cx)
                                },
                            )),
                        ),
                )
            }
            _ => None,
        };

        frame(self.right_panel_width(t), t)
            .id("thread-panel")
            .debug_selector(|| "thread-panel".into())
            .on_drag_move::<gpui_kit::ExternalPaths>(cx.listener(
                |this, event: &gpui_kit::DragMoveEvent<gpui_kit::ExternalPaths>, _, cx| {
                    let inside = event.bounds.contains(&event.event.position);
                    let count = event.drag(cx).paths().len();
                    this.drag_files_over(crate::app::DropOver::Reply, inside, count, cx);
                },
            ))
            .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, _, cx| {
                this.drop_files(crate::app::DropOver::Reply, paths, cx);
            }))
            .child(head)
            .children(reason)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(posts)
                    .child(fade(true))
                    .child(fade(false)),
            )
            .children(summary)
            .children(review)
            .child(reply)
            .into_any_element()
    }

    /// A section of the thread panel with a header that opens and closes it,
    /// and Markdown text under it. The text is selectable and scrolls when it
    /// is long.
    fn render_text_section(
        &mut self,
        section: TextSection,
        markdown: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
        toggle: impl Fn(&mut ConsortApp) + 'static,
    ) -> AnyElement {
        let c = &t.colors;
        let first_line = consort_client::format::card_summary(markdown)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        let header = focusable(div().id(section.toggle_id), t)
            .flex()
            .items_center()
            .gap(t.space.sm)
            .py(t.space.sm)
            .px(t.space.lg)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_sidebar))
            .child(icon(
                if section.open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                t.icon.dense,
                c.text_tertiary,
            ))
            .child(text(section.label, t.text.label, c.text_secondary).flex_none())
            // A closed section shows the first line, so the reader knows
            // what is in it.
            .when(!section.open, |d| {
                d.child(
                    line(first_line, detail(t), c.text_tertiary)
                        .flex_1()
                        .min_w_0(),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle(this);
                cx.notify();
            }));
        let body = section.open.then(|| {
            let md = self
                .render_markdown(
                    &section.key,
                    markdown,
                    crate::ui::timeline::post_menu(markdown),
                    c.surface_raised,
                    t,
                    cx,
                )
                .type_style(t.text.ui)
                .text_color(c.text_primary);
            div()
                .id(SharedString::from(format!("{}-body", section.key)))
                .max_h(t.layout.error_text_max_height)
                .overflow_y_scroll()
                .px(t.space.lg)
                .pb(t.space.md)
                .child(md)
                .when(section.loading, |d| {
                    d.child(div().pt(t.space.sm).child(text(
                        "Loading the full text",
                        detail(t),
                        c.text_tertiary,
                    )))
                })
        });
        div()
            .flex()
            .flex_col()
            .flex_none()
            .when(section.above, |d| d.border_t(t.border_width))
            .when(!section.above, |d| d.border_b(t.border_width))
            .border_color(c.border_subtle)
            .child(header)
            .children(body)
            .into_any_element()
    }

    fn thread_post(&self, ix: usize) -> Option<ThreadPost> {
        let Panel::Thread(task_id) = &self.panel else {
            return None;
        };
        let offset = usize::from(self.thread_has_brief());
        self.state
            .thread(task_id)?
            .posts
            .get(ix.checked_sub(offset)?)
            .cloned()
    }

    /// The row after the posts is the plan card, when the harness made a
    /// plan. It sits under the newest message.
    fn thread_is_plan(&self, ix: usize) -> bool {
        self.thread_keys
            .get(ix)
            .is_some_and(|k| k.starts_with("plan-"))
    }

    /// The last row of the thread is the writing bubble while the turn of
    /// the task runs. It sits under the plan card.
    fn thread_is_writing(&self, ix: usize) -> bool {
        ix + 1 == self.thread_keys.len()
            && self
                .thread_keys
                .last()
                .is_some_and(|k| k.starts_with("writing-"))
    }

    /// The plan of the task as a checklist: done steps in `text.tertiary`
    /// with the check, the current step in `text.primary` with the working
    /// glyph, and the next steps with the open ring. The plan is not
    /// repeated as an activity line.
    fn render_plan_card(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let Panel::Thread(task_id) = &self.panel else {
            return div().into_any_element();
        };
        let Some(plan) = self.state.thread(task_id).and_then(|th| th.plan.clone()) else {
            return div().into_any_element();
        };
        let mut card = div()
            .id("thread-plan")
            .debug_selector(|| "thread-plan".into())
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .w_full()
            .min_w_0()
            .py(t.space.md)
            .px(t.space.md + t.space.xxs)
            .rounded(t.radius.lg)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .bg(c.surface_card)
            .child(text(plan.heading(), detail(t), c.text_tertiary));
        for (i, step) in plan.steps.iter().enumerate() {
            let (glyph, color): (AnyElement, _) = match step.state {
                PlanStepState::Done => (
                    icon("done", t.icon.dense, c.status_done).into_any_element(),
                    c.text_tertiary,
                ),
                PlanStepState::Current => (
                    status_glyph(
                        SharedString::from(format!("plan-step-{i}")),
                        TaskStatus::Working,
                        t.icon.dense,
                        t,
                        cx,
                    ),
                    c.text_primary,
                ),
                PlanStepState::Next => (
                    status_glyph(
                        SharedString::from(format!("plan-step-{i}")),
                        TaskStatus::UpNext,
                        t.icon.dense,
                        t,
                        cx,
                    ),
                    c.text_secondary,
                ),
            };
            card = card.child(
                div()
                    .flex()
                    .items_start()
                    .gap(t.space.sm + t.space.xxs)
                    .debug_selector({
                        let id = format!("plan-step-row-{i}");
                        move || id.clone()
                    })
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(t.text.ui.line)
                            .child(glyph),
                    )
                    .child(div().flex_1().min_w_0().child(selectable(
                        SharedString::from(format!("plan-step-text-{task_id}-{i}")),
                        step.text.clone(),
                        t.text.ui,
                        color,
                    ))),
            );
        }
        card.into_any_element()
    }

    /// The first row of the thread is the brief.
    fn thread_has_brief(&self) -> bool {
        self.thread_keys
            .first()
            .is_some_and(|k| k.starts_with("brief-"))
    }

    fn render_post(
        &mut self,
        post: &ThreadPost,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let chat = t.component.chat;
        let you = post.speaker == Speaker::You;
        let task_id = match &self.panel {
            Panel::Thread(id) => id.clone(),
            _ => String::new(),
        };
        let max = self.right_panel_width(t) - t.space.page;
        // The thread is bubbles, as the timeline (`BDH-0`): the task's words
        // at the left, yours at the right.
        let mut col = div()
            .flex()
            .flex_col()
            .gap(t.space.xs + t.space.xxs)
            .w_full()
            .when(you, |d| d.items_end())
            .when(!you, |d| d.items_start());
        let count = post.bodies.len();
        for (i, body) in post.bodies.iter().enumerate() {
            let key = format!("{}.{i}", post.id);
            let element = match body {
                PostBody::Text(markdown) => {
                    let md = self.render_markdown(
                        &key,
                        markdown,
                        crate::ui::timeline::post_menu(markdown),
                        c.surface_raised,
                        t,
                        cx,
                    );
                    let last = i + 1 == count;
                    div()
                        .max_w(max)
                        .min_w_0()
                        .py(chat.bubble_pad_y)
                        .px(chat.bubble_pad_x)
                        .rounded(chat.bubble_radius)
                        .when(last && you, |d| d.rounded_br(chat.bubble_tail))
                        .when(last && !you, |d| d.rounded_bl(chat.bubble_tail))
                        .bg(if you {
                            c.accent_surface
                        } else {
                            c.surface_card
                        })
                        .type_style(t.text.body)
                        .text_color(c.text_primary)
                        .child(md)
                        .into_any_element()
                }
                PostBody::Activity { summary, items } => {
                    self.render_activity_chip(&key, summary, items, t, cx)
                }
                // The card says "Needs you" itself, and the line after the
                // answer says the answer, so there is no line above it.
                PostBody::Approval(approval) => div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .child(self.render_approval_card(approval, &key, false, t, cx))
                    .into_any_element(),
                PostBody::Decision(decision) => div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(t.space.xs + t.space.xxs)
                    .child(self.request_line(&key, "Decision", decision.state, t, cx))
                    .child(self.render_decision(decision, &key, true, t, cx))
                    .into_any_element(),
                // A system notice: quiet, selectable, no speaker of its own.
                PostBody::Notice {
                    draft: Some(draft), ..
                } => div()
                    .flex()
                    .w_full()
                    .child(self.render_shown_draft(draft, &format!("{key}-shown"), false, t, cx))
                    .into_any_element(),
                // A failed turn is a card, as it is in the timeline.
                PostBody::Notice {
                    error: Some(card), ..
                } => div()
                    .flex()
                    .w_full()
                    .child(self.render_error_card(
                        card,
                        &format!("err-{key}"),
                        crate::ui::errors::ErrorPlace::Task(task_id.clone()),
                        t,
                        cx,
                    ))
                    .into_any_element(),
                // A paid call, as it is in the timeline.
                PostBody::Notice {
                    monid: Some(row), ..
                } => div()
                    .flex()
                    .w_full()
                    .child(self.render_monid_call(row, "thread", t, cx))
                    .into_any_element(),
                // A move of the task: the notice says that the task stays.
                PostBody::Notice {
                    fallback: Some(fallback),
                    ..
                } => {
                    let channel_id = self
                        .state
                        .task(&task_id)
                        .map(|t| t.channel_id.clone())
                        .unwrap_or_default();
                    self.render_fallback_notice(
                        fallback,
                        &format!("{key}-fallback"),
                        &post.time_label,
                        &channel_id,
                        true,
                        t,
                        cx,
                    )
                }
                PostBody::Notice { text: notice, .. } => div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(selectable(
                        SharedString::from(format!("{key}-notice")),
                        notice.clone(),
                        t.text.ui,
                        c.text_tertiary,
                    ))
                    .into_any_element(),
                PostBody::Attachments(list) => self.render_post_attachments(&key, list, t, cx),
            };
            col = col.child(element);
        }
        // The words of the user that the assistant passed on to the task:
        // a link back to the message in the channel.
        if you
            && let Some(source) = post.source_message_id.clone()
            && let Panel::Thread(task_id) = &self.panel
            && let Some(channel_id) = self.state.task(task_id).map(|t| t.channel_id.clone())
        {
            let link_id = format!("show-in-channel-{}", post.id);
            col = col.child(
                leaving_link(SharedString::from(link_id.clone()), "Show in channel", t)
                    .debug_selector(move || link_id.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_message(
                            channel_id.clone(),
                            source.clone(),
                            None,
                            None,
                            window,
                            cx,
                        )
                    })),
            );
        }
        if post.pending {
            col = col.child(text(
                crate::ui::timeline::pending_text(matches!(
                    self.state.connection,
                    consort_client::Connection::Offline { .. }
                ))
                .to_owned(),
                t.text.caption,
                c.text_tertiary,
            ));
        }
        // A reply that the hub refused stays in the lane of the user, dimmed,
        // with the reason under it and "Put back in the message box".
        match &post.refused {
            Some(refused) => {
                let note = self.render_refused_note(refused, &post.id, t, cx);
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .w_full()
                    .child(col.opacity(t.opacity.answered_dim))
                    .child(note)
                    .into_any_element()
            }
            None => col.into_any_element(),
        }
    }

    /// A decision card. In a task thread, Answer later closes the panel. In
    /// the timeline the decision simply stays until you answer it.
    pub fn render_decision(
        &mut self,
        decision: &DecisionView,
        key: &str,
        in_thread: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let answered = decision.state == RequestState::Answered;
        let blocked = self.answers_blocked();
        let mut options = div().flex().flex_col().gap(t.space.xs + t.space.xxs);
        for (i, option) in decision.options.iter().enumerate() {
            let i = i as u32;
            let recommended = decision.recommended == Some(i);
            let chosen = decision.chosen == Some(i);
            let dim = answered && !chosen;
            let ring = if recommended || chosen {
                c.accent_text
            } else {
                c.border_strong
            };
            let id = decision.id.clone();
            // The answer is irreversible. Only the Choose button sends it. The
            // row itself is not a control: GPUI fires a click on mouse-up
            // with no drag threshold, so a click on the row would answer
            // when the user only selected or double-clicked its text.
            options = options.child(
                div()
                    .id(SharedString::from(format!("{key}-option-{i}")))
                    .flex()
                    .items_start()
                    .gap(t.space.sm + t.space.xxs)
                    .py(t.space.sm + t.space.xxs)
                    .px(t.space.md)
                    .rounded(t.radius.md)
                    .border(t.border_width)
                    .border_color(if recommended || chosen {
                        c.accent_text
                    } else {
                        c.border_strong
                    })
                    .when(recommended || chosen, |d| d.bg(c.surface_raised))
                    .when(dim, |d| d.opacity(t.opacity.answered_dim))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(t.text.ui.line)
                            .w(t.icon.size)
                            .child(
                                div()
                                    .size(t.space.md + t.space.xxs)
                                    .rounded(t.radius.full)
                                    .border(t.stroke)
                                    .border_color(ring)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(chosen, |d| {
                                        d.child(
                                            div()
                                                .size(t.space.xs + t.space.xxs)
                                                .rounded(t.radius.full)
                                                .bg(c.accent_text),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(t.space.xxs)
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(t.space.sm)
                                    .child(text(
                                        option.label.clone(),
                                        t.text.ui_strong,
                                        c.text_primary,
                                    ))
                                    .when(recommended, |d| {
                                        d.child(text("Recommended", detail(t), c.accent_text))
                                    }),
                            )
                            .child(selectable(
                                SharedString::from(format!("{key}-consequence-{i}")),
                                option.consequence.clone(),
                                detail(t),
                                c.text_secondary,
                            )),
                    )
                    .when(!answered, |d| {
                        d.child(crate::ui::hub_blocked::needs_hub(
                            button(
                                SharedString::from(format!("{key}-choose-{i}")),
                                "Choose",
                                if recommended {
                                    ButtonKind::Primary
                                } else {
                                    ButtonKind::Secondary
                                },
                                t,
                            )
                            .flex_none()
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.answer_decision(id.clone(), Some(i), None, window, cx)
                                },
                            )),
                            blocked,
                            t,
                        ))
                    }),
            );
        }
        let writing = self.decision_writing.as_deref() == Some(decision.id.as_str());
        let closed = matches!(decision.state, RequestState::Closed | RequestState::Expired);
        let footer: AnyElement = if closed {
            text("This decision is closed.", t.text.ui, c.text_secondary).into_any_element()
        } else if answered {
            let answer = decision
                .chosen
                .and_then(|i| decision.options.get(i as usize))
                .map(|o| format!("You chose {}.", o.label))
                .or_else(|| {
                    decision
                        .answer_text
                        .as_ref()
                        .map(|t| format!("You answered: {t}"))
                })
                .unwrap_or_else(|| "You answered.".to_owned());
            text(answer, t.text.ui, c.text_secondary).into_any_element()
        } else if writing {
            let id = decision.id.clone();
            div()
                .flex()
                .flex_col()
                .gap(t.space.sm)
                .child(
                    div()
                        .bg(c.surface_raised)
                        .border(t.border_width)
                        .border_color(c.accent_text)
                        .rounded(t.radius.md)
                        .py(t.space.xs + t.space.xxs)
                        .px(t.space.md)
                        .type_style(t.text.ui)
                        .child(crate::ui::parts::textarea(
                            &self.inputs.decision_answer,
                            t.text.ui,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .gap(t.space.sm)
                        .child(crate::ui::hub_blocked::needs_hub(
                            button(
                                SharedString::from(format!("{key}-send")),
                                "Send answer",
                                ButtonKind::Primary,
                                t,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    let text = this
                                        .inputs
                                        .decision_answer
                                        .read(cx)
                                        .value()
                                        .trim()
                                        .to_owned();
                                    if text.is_empty() {
                                        return;
                                    }
                                    // The answer box keeps the text until
                                    // the hub takes the answer.
                                    this.answer_decision(id.clone(), None, Some(text), window, cx);
                                },
                            )),
                            blocked,
                            t,
                        ))
                        .child(
                            button(
                                SharedString::from(format!("{key}-cancel")),
                                "Cancel",
                                ButtonKind::Quiet,
                                t,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.decision_writing = None;
                                cx.notify();
                            })),
                        ),
                )
                .into_any_element()
        } else {
            let id = decision.id.clone();
            let link = |id_suffix: &str, label: &'static str| {
                focusable(
                    div().id(SharedString::from(format!("{key}-{id_suffix}"))),
                    t,
                )
                .type_style(t.text.ui_strong)
                .text_color(c.text_secondary)
                .cursor_pointer()
                .hover(|s| s.text_color(c.text_primary))
                .child(label)
            };
            div()
                .flex()
                .items_center()
                .gap(t.space.lg)
                .pl(t.space.xxs)
                .when(decision.allow_free_text, |d| {
                    d.child(link("own", "Write my own answer").on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.decision_writing = Some(id.clone());
                            this.inputs
                                .decision_answer
                                .update(cx, |input, cx| input.focus(window, cx));
                            cx.notify();
                        },
                    )))
                })
                .when(in_thread, |d| {
                    d.child(
                        link("later", "Answer later")
                            .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx))),
                    )
                })
                .into_any_element()
        };
        crate::ui::parts::card_corners(div(), !in_thread, t)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(t.space.md)
            .p(t.space.md + t.space.xxs)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .flex()
                    .gap(t.space.sm + t.space.xxs)
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(t.text.body.line)
                            .w(t.icon.size)
                            .child(status_glyph(
                                SharedString::from(format!("{key}-glyph")),
                                request_glyph(decision.state),
                                t.icon.size,
                                t,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(t.space.xs)
                            .child(
                                selectable(
                                    SharedString::from(format!("{key}-question")),
                                    decision.question.clone(),
                                    t.text.body,
                                    c.text_primary,
                                )
                                .font_weight(t.text.title.weight),
                            )
                            .when_some(decision.context.clone(), |d, ctx| {
                                d.child(selectable(
                                    SharedString::from(format!("{key}-context")),
                                    ctx,
                                    t.text.ui,
                                    c.text_secondary,
                                ))
                            }),
                    ),
            )
            .child(options)
            .child(footer)
            .into_any_element()
    }
}

/// The reason on the failed or waiting task, without Markdown marks. The
/// banner shows plain text and agents write the reason in Markdown.
pub(crate) fn reason_text(reason: &str) -> String {
    crate::ui::timeline::plain_text(reason)
}
