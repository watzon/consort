//! The side panels: the Tasks panel and the task thread.

use consort_client::{
    DecisionView, PostBody, RequestState, Speaker, TaskStatus, TaskView, ThreadPost,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window, div, list,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Panel, TasksTab},
    ui::window_controls::DragArea as _,
    ui::{
        menu::{MenuAction, OpenMenu, TextTarget, text_edit_menu},
        parts::{
            ButtonKind, button, detail, focusable, icon, icon_button, line, selectable,
            status_color, status_glyph, text,
        },
        timeline::{open_context_menu, task_menu, time_text},
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
fn frame(t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(t.layout.panel_width)
        .h_full()
        .bg(t.colors.surface_sidebar)
        .border_l(t.border_width)
        .border_color(t.colors.border_subtle)
}

/// The 52 px panel header.
fn header(t: &Tokens) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(t.space.page + t.space.xs)
        .border_b(t.border_width)
        .border_color(t.colors.border_subtle)
        .drag_area()
}

/// What an approval card says after the user answered. The wide choice
/// repeats the harness's own words, so the card keeps the scope that the user
/// saw on the button.
pub fn answered_text(approval: &consort_client::ApprovalView, in_task: bool) -> String {
    use consort_client::ApprovalChoice;
    match approval.answer {
        Some(ApprovalChoice::AllowOnce) => "You allowed it once.".to_owned(),
        Some(ApprovalChoice::AllowForTask) => match approval.allow_always_label.as_deref() {
            Some(label) if !label.trim().is_empty() => format!("You chose: {}", label.trim()),
            _ if in_task => "You allowed it for this task.".to_owned(),
            _ => "You allowed it from now on.".to_owned(),
        },
        Some(ApprovalChoice::Deny) => "You denied it.".to_owned(),
        None => "Answered.".to_owned(),
    }
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
        if !any {
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
        frame(t)
            .child(
                header(t)
                    .gap(t.space.xs)
                    .pl(t.space.lg)
                    .pr(t.space.sm)
                    .child(
                        div()
                            .type_style(t.text.title)
                            .line_height(t.text.body.line)
                            .child("Tasks"),
                    )
                    // How many tasks need you. The header of the column
                    // said it before the redesign; the Tasks button has only
                    // a dot now.
                    .child(div().flex_1().pl(t.space.xs).when(need_you > 0, |d| {
                        d.child(text(
                            format!("{need_you} need you"),
                            t.text.ui,
                            c.accent_text,
                        ))
                    }))
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
                            cx.listener(|this, _, window, cx| this.start_task_draft(window, cx)),
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
            .child(
                div()
                    .flex()
                    .px(t.space.lg)
                    .pt(t.space.md)
                    .pb(t.space.xs)
                    .child(segmented),
            )
            .child(
                div()
                    .id("tasks-scroll")
                    .flex_1()
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
                open_context_menu(this, event, task_menu(&menu_id, status, true), window, cx)
            }),
        )
        .into_any_element()
    }

    pub fn render_thread_panel(
        &mut self,
        task_id: &str,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let task = state.task(task_id).cloned();
        let thread = state.thread(task_id).cloned();
        let title = task.as_ref().map(|t| t.title.clone()).unwrap_or_default();
        let status = task.as_ref().map(|t| t.status).unwrap_or_default();
        let status_detail = thread
            .as_ref()
            .map(|t| t.status_detail.clone())
            .unwrap_or_default();
        let mode_tag = thread.as_ref().map(|t| t.approval_mode.tag());
        let menu_task = task_id.to_owned();
        let more_trigger = self.triggers.clone();

        let head = header(t)
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
                            .child(text(status.label(), detail(t), status_color(status, t)))
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
                                let items = task_menu(&menu_task, status, false);
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
            .and_then(|th| th.reason.clone())
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

        // The brief that started the task, in full. It sits under the header
        // as a section that opens. The text is selectable Markdown, and it
        // scrolls when it is long.
        let loading = thread.as_ref().is_some_and(|th| th.text_is_preview);
        let brief = thread
            .as_ref()
            .filter(|th| !th.brief.trim().is_empty())
            .map(|th| th.brief.clone())
            .map(|brief| {
                let open = self.brief_open.iter().any(|id| id == task_id);
                self.render_text_section(
                    TextSection {
                        key: format!("thread-brief-{task_id}"),
                        toggle_id: "thread-brief-toggle",
                        label: "Brief",
                        open,
                        above: false,
                        loading,
                    },
                    &brief,
                    t,
                    cx,
                    {
                        let task = task_id.to_owned();
                        move |this: &mut ConsortApp| toggle_id(&mut this.brief_open, &task)
                    },
                )
            });

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

        let placement = thread
            .as_ref()
            .and_then(|th| th.placement.clone())
            .map(|p| {
                let row = |label: &str, value: AnyElement| {
                    div()
                        .flex()
                        .gap(t.space.md)
                        .child(
                            text(label.to_owned(), detail(t), c.text_tertiary)
                                .w(t.space.page + t.space.xxl)
                                .flex_none(),
                        )
                        .child(value)
                };
                // A known path in mono, else the repo, else a task with no
                // repo, which runs in a new empty folder.
                let workspace = div()
                    .flex()
                    .flex_wrap()
                    .min_w_0()
                    .type_style(detail(t))
                    .text_color(c.text_secondary);
                // "Worktree of x", "Clone of x", but "In x" for in place.
                let lead = match p.isolation {
                    consort_client::Isolation::InPlace => "In ".to_owned(),
                    other => format!("{} of ", other.label()),
                };
                // The path, the branch and the repo are text that the user may
                // copy, so they are selectable.
                let mono = |what: &str, value: String| {
                    selectable(
                        SharedString::from(format!("workspace-{what}-{task_id}")),
                        value,
                        detail(t),
                        c.text_secondary,
                    )
                    .font_family(t.font_mono.clone())
                };
                let workspace = if !p.path.is_empty() {
                    workspace
                        .child(lead)
                        .child(mono("path", p.path.clone()))
                        .when_some(p.branch.clone(), |d, b| {
                            d.child(" at ").child(mono("branch", b))
                        })
                } else if let Some(repo) = p.repo.clone() {
                    workspace.child(lead).child(mono("repo", repo))
                } else {
                    workspace.child("A new empty folder")
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs + t.space.xxs)
                    .py(t.space.md)
                    .px(t.space.lg)
                    .border_b(t.border_width)
                    .border_color(c.border_subtle)
                    .child(row(
                        "Worker",
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.xs + t.space.xxs)
                            .child(
                                div()
                                    .size(t.space.xs + t.space.xxs)
                                    .rounded(t.radius.full)
                                    .when(p.worker_online, |d| d.bg(c.status_done))
                                    .when(!p.worker_online, |d| {
                                        d.border(t.border_width).border_color(c.text_tertiary)
                                    }),
                            )
                            .child(text(
                                format!("{}, {}", p.worker_name, p.worker_os),
                                detail(t),
                                c.text_secondary,
                            ))
                            .into_any_element(),
                    ))
                    .child(row(
                        "Harness",
                        text(p.harness.clone(), detail(t), c.text_secondary).into_any_element(),
                    ))
                    .child(row("Workspace", workspace.into_any_element()))
            });

        let count = self.thread_keys.len();
        let posts = list(
            self.thread.clone(),
            cx.processor(move |this, ix: usize, window, cx| {
                let t = ConsortApp::tokens(cx);
                let post = this
                    .thread_post(ix)
                    .map(|post| this.render_post(&post, &t, window, cx))
                    .unwrap_or_else(|| div().into_any_element());
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

        // The task keeps the model that it started with. The thread shows it
        // as text, with no way to change it.
        let model_label = self
            .state
            .thread(task_id)
            .and_then(|th| th.model_label.clone());
        let reply = div()
            .flex_none()
            .px(t.space.lg)
            .pb(t.space.xl)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.sm + t.space.xxs)
                    .pl(t.space.md + t.space.xxs)
                    .pr(t.space.sm + t.space.xxs)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .rounded(t.radius.lg)
                    // The reply box has the same menu as the composer.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            let items = text_edit_menu(TextTarget::Reply);
                            this.open_menu(OpenMenu::at(event.position, items), cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .type_style(t.text.ui)
                            .child(crate::ui::parts::textarea(&self.inputs.reply, t.text.ui)),
                    )
                    .child(
                        icon_button(
                            "reply-attach",
                            "plus",
                            t.space.xl + t.space.xs,
                            t.icon.size,
                            c.text_secondary,
                            c.hover_card,
                            t,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_notice("Attachments come in a later build.".into(), cx)
                        })),
                    )
                    .child(
                        focusable(div().id("reply-send"), t)
                            .size(t.space.xl + t.space.xs)
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(t.radius.md)
                            .bg(c.surface_raised)
                            .border(t.border_width)
                            .border_color(c.border_strong)
                            .cursor_pointer()
                            .hover(|s| s.bg(c.hover_raised))
                            .child(icon("corner-down-left", t.icon.dense, c.text_primary))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.send_reply(window, cx)),
                            ),
                    ),
            )
            .when_some(model_label, |d, label| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.xs + t.space.xxs)
                        .pt(t.space.sm)
                        .px(t.space.md)
                        .child(icon("signal", t.icon.dense, c.text_tertiary))
                        .child(line(
                            format!("Runs with {label}"),
                            crate::ui::parts::detail(t),
                            c.text_tertiary,
                        )),
                )
            });

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

        frame(t)
            .child(head)
            .children(brief)
            .children(reason)
            .children(placement)
            .child(
                div()
                    .relative()
                    .flex_1()
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
                d.child(line(first_line, detail(t), c.text_tertiary).flex_1())
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
        self.state.thread(task_id)?.posts.get(ix).cloned()
    }

    fn render_post(
        &mut self,
        post: &ThreadPost,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let you = post.speaker == Speaker::You;
        // A system notice has no speaker: it is not a message from anyone.
        let notice_only = post
            .bodies
            .iter()
            .all(|b| matches!(b, PostBody::Notice { .. }));
        let mut col = div().flex().flex_col().gap(t.space.sm).child(
            div()
                .flex()
                .items_baseline()
                .gap(t.space.sm)
                .when(!notice_only, |d| {
                    d.child(text(
                        post.speaker.label(),
                        t.text.label,
                        if you { c.accent_text } else { c.text_secondary },
                    ))
                })
                .child(text(
                    time_text(&post.time_label, post.pending).to_owned(),
                    t.text.caption,
                    c.text_tertiary,
                )),
        );
        for (i, body) in post.bodies.iter().enumerate() {
            let key = format!("{}.{i}", post.id);
            let element = match body {
                PostBody::Text(markdown) => {
                    let md = self.render_markdown(
                        &key,
                        markdown,
                        crate::ui::timeline::post_menu(markdown),
                        c.surface_card,
                        t,
                        cx,
                    );
                    if you {
                        div()
                            .flex()
                            .child(
                                div()
                                    .bg(c.accent_surface)
                                    .rounded(t.radius.lg)
                                    .py(t.space.xs + t.space.xxs)
                                    .px(t.space.md)
                                    .type_style(t.text.ui)
                                    .child(md),
                            )
                            .into_any_element()
                    } else {
                        md.type_style(t.text.ui)
                            .text_color(c.text_primary)
                            .into_any_element()
                    }
                }
                PostBody::Activity { summary, items } => {
                    let open = self.activity_open.contains(&key);
                    let toggle_key = key.clone();
                    div()
                        .flex()
                        .flex_col()
                        .border(t.border_width)
                        .border_color(c.border_subtle)
                        .rounded(t.radius.md)
                        .child(
                            focusable(div().id(SharedString::from(format!("{key}-activity"))), t)
                                .flex()
                                .items_center()
                                .gap(t.space.sm)
                                .py(t.space.xs + t.space.xxs)
                                .px(t.space.sm + t.space.xxs)
                                .cursor_pointer()
                                .rounded(t.radius.md)
                                .hover(|s| s.bg(c.hover_sidebar))
                                .child(icon("terminal", t.icon.dense, c.text_tertiary))
                                .child(text(summary.clone(), detail(t), c.text_secondary).flex_1())
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
                                    if let Some(pos) =
                                        this.activity_open.iter().position(|k| *k == toggle_key)
                                    {
                                        this.activity_open.remove(pos);
                                    } else {
                                        this.activity_open.push(toggle_key.clone());
                                    }
                                    cx.notify();
                                })),
                        )
                        .when(open, |d| {
                            let mut list = div()
                                .flex()
                                .flex_col()
                                .gap(t.space.xxs)
                                .px(t.space.sm + t.space.xxs)
                                .pb(t.space.sm)
                                .pl(t.icon.dense + t.space.sm + t.space.sm + t.space.xxs);
                            for (n, item) in items.iter().enumerate() {
                                // Command lines are selectable, so the user
                                // can copy them.
                                list = list.child(
                                    selectable(
                                        SharedString::from(format!("{key}-item-{n}")),
                                        item.clone(),
                                        detail(t),
                                        c.text_tertiary,
                                    )
                                    .font_family(t.font_mono.clone()),
                                );
                            }
                            d.child(list)
                        })
                        .into_any_element()
                }
                PostBody::Approval(approval) => {
                    self.render_approval_card(approval, &key, false, t, cx)
                }
                PostBody::Decision(decision) => self.render_decision(decision, &key, true, t, cx),
                // A system notice: quiet, selectable, no speaker of its own.
                PostBody::Notice { text: notice, .. } => selectable(
                    SharedString::from(format!("{key}-notice")),
                    notice.clone(),
                    t.text.ui,
                    c.text_tertiary,
                )
                .into_any_element(),
            };
            col = col.child(element);
        }
        col.into_any_element()
    }

    /// An approval card: what asks, the detail, and the buttons or the answer.
    /// Task threads and the timeline both use it.
    /// An approval card. `chat` gives it the corners of a card in the
    /// chat (see [`crate::ui::parts::card_corners`]).
    pub fn render_approval_card(
        &mut self,
        approval: &consort_client::ApprovalView,
        key: &str,
        chat: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(hub_card) = approval.card.clone() {
            return self.render_hub_card(approval, &hub_card, key, chat, t, cx);
        }
        let c = &t.colors;
        let answered = approval.state == RequestState::Answered;
        let card = crate::ui::parts::card_corners(div(), chat, t)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(t.space.sm + t.space.xxs)
            .pt(t.space.md)
            .pb(t.space.md + t.space.xxs)
            .px(t.space.md + t.space.xxs)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm + t.space.xxs)
                    .child(status_glyph(
                        SharedString::from(format!("{key}-glyph")),
                        request_glyph(approval.state),
                        t.icon.size,
                        t,
                        cx,
                    ))
                    .child(
                        selectable(
                            SharedString::from(format!("{key}-summary")),
                            approval.summary.clone(),
                            t.text.ui_strong,
                            c.text_primary,
                        )
                        .flex_1()
                        .min_w_0(),
                    ),
            )
            .child(self.render_approval_detail(approval, false, t, cx));
        let task = approval.task_id.is_some();
        let card = if answered {
            card.child(selectable(
                SharedString::from(format!("{key}-answer")),
                answered_text(approval, task),
                t.text.ui,
                c.text_secondary,
            ))
        } else if approval.state == RequestState::Expired {
            card.child(text(
                "This request closed before anyone answered it.",
                t.text.ui,
                c.text_secondary,
            ))
        } else if approval.state == RequestState::Closed {
            card.child(text("This request is closed.", t.text.ui, c.text_secondary))
        } else {
            card.child(self.render_approval_buttons(approval, t, cx))
        };
        card.into_any_element()
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
                        d.child(
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
                        )
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
                        .child(
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
                                    // The answer box keeps the text until the
                                    // hub takes the answer.
                                    this.answer_decision(id.clone(), None, Some(text), window, cx);
                                },
                            )),
                        )
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
