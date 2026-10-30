//! Routines: the Routines tab of the Channel panel, a routine opened in the
//! panel with its run history, Settings > Routines, and the routine dialog.
//! The schedule field of the dialog is in `ui/schedule.rs`.
//!
//! A routine row has three lines: the name, the schedule in words with the
//! action, and the last result with the next run. On hover it shows Run now,
//! Run history and the row menu. A routine that is off says "Off. It does
//! not run until you turn it on."

use consort_client::{
    ChannelKind, Command,
    routines::{
        self as r, RoutineActionView, RoutineRunStatus, RoutineRunView, RoutineView, StatusTone,
    },
};
use consort_theme::{Tokens, Type};
use gpui_kit::{
    AnyElement, Context, Div, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    linear_color_stop, linear_gradient, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Dialog, Page},
    knowledge::{KnowledgeAction, StatusFilter, routine_menu},
    ui::{
        dialogs::{field_block, input_box},
        menu::{MenuAction, MenuItem, OpenMenu},
        parts::{
            ButtonKind, button, detail, focusable, glyph, icon, icon_button, icon_text_button,
            leaving_link, line, selectable, text,
        },
    },
};

/// The words under the Routines tab.
pub const ROUTINES_HINT: &str = "Routines run on the hub, so they run when this app is closed. Times use each routine's time zone.";

/// The small switch of a routine: `accent.text` with the thumb on the right
/// when on, `border.strong` with the thumb on the left when off.
pub fn small_switch(id: impl Into<SharedString>, on: bool, t: &Tokens) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    let k = t.component.knowledge;
    let id: SharedString = id.into();
    let selector = id.to_string();
    focusable(div().id(id), t)
        .debug_selector(move || selector)
        .flex()
        .flex_none()
        .items_center()
        .w(k.switch_w)
        .h(k.switch_h)
        .p(t.space.xxs)
        .rounded(t.radius.full)
        .cursor_pointer()
        .bg(if on { c.accent_text } else { c.border_strong })
        .when(on, |d| d.justify_end())
        .child(div().size(k.switch_thumb).rounded(t.radius.full).bg(if on {
            c.accent_on_fill
        } else {
            c.text_tertiary
        }))
}

/// The color of a result.
fn tone_color(tone: StatusTone, t: &Tokens) -> Hsla {
    let c = &t.colors;
    match tone {
        StatusTone::Ok => c.status_done,
        StatusTone::Failed => c.status_failed,
        StatusTone::Running => c.status_working,
        StatusTone::Quiet | StatusTone::Off => c.text_tertiary,
    }
}

/// The dot before a result: filled, or a ring for a routine that is off.
fn status_dot(tone: StatusTone, t: &Tokens) -> Div {
    let k = t.component.knowledge;
    let color = tone_color(tone, t);
    let dot = div().size(k.status_dot).flex_none().rounded(t.radius.full);
    if tone == StatusTone::Off {
        dot.border(t.border_width).border_color(color)
    } else {
        dot.bg(color)
    }
}

/// A small button in a routine row: 22 px high, label text.
fn row_button(
    id: impl Into<SharedString>,
    label: &str,
    icon_name: Option<&str>,
    secondary: bool,
    t: &Tokens,
) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    let id: SharedString = id.into();
    let selector = id.to_string();
    focusable(div().id(id), t)
        .debug_selector(move || selector)
        .flex()
        .flex_none()
        .items_center()
        .gap(t.space.xs + t.space.xxs)
        .py(t.space.xxs + t.border_width)
        .px(t.space.sm + t.space.xxs)
        .rounded(t.radius.md)
        .cursor_pointer()
        .when(secondary, |d| {
            d.bg(c.surface_raised)
                .border(t.border_width)
                .border_color(c.border_strong)
                .hover(|s| s.bg(c.hover_raised))
        })
        .when(!secondary, |d| d.hover(|s| s.bg(c.surface_selected)))
        .when_some(icon_name, |d, name| {
            d.child(icon(name, t.space.md, c.text_primary))
        })
        .child(text(
            label.to_owned(),
            t.text.label,
            if secondary {
                c.text_primary
            } else {
                c.text_secondary
            },
        ))
}

/// The glyph of a run in the run history.
fn run_glyph(status: RoutineRunStatus, t: &Tokens) -> AnyElement {
    let c = &t.colors;
    match status {
        RoutineRunStatus::Ok => icon("done", t.icon.size, c.status_done).into_any_element(),
        RoutineRunStatus::Error => icon("failed", t.icon.size, c.status_failed).into_any_element(),
        RoutineRunStatus::Running => {
            icon("working", t.icon.size, c.status_working).into_any_element()
        }
        RoutineRunStatus::Skipped => {
            glyph("status-up-next", t.icon.size, c.text_tertiary).into_any_element()
        }
    }
}

fn run_color(status: RoutineRunStatus, t: &Tokens) -> Hsla {
    let c = &t.colors;
    match status {
        RoutineRunStatus::Ok => c.status_done,
        RoutineRunStatus::Error => c.status_failed,
        RoutineRunStatus::Running => c.status_working,
        RoutineRunStatus::Skipped => c.text_tertiary,
    }
}

/// The text under a failed or skipped run: the hub's reason. A skip
/// because the last run was still going names the run time.
fn run_reason(
    run: &RoutineRunView,
    schedule: &consort_client::routines::RoutineSchedule,
) -> Option<String> {
    let error = run.error.clone()?;
    if run.status == RoutineRunStatus::Skipped && error == "The last run was still going." {
        return Some(format!(
            "The last run was still going at {}.",
            r::routine_clock(run.started_at, schedule)
        ));
    }
    Some(error)
}

/// Where a routine row is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowPlace {
    Panel,
    Settings,
}

impl ConsortApp {
    /// The Routines tab: the summary, New routine, and the rows.
    pub fn render_routines_tab(
        &mut self,
        channel_name: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        // Follow-ups that the assistant set have their own section.
        let (follow_ups, routines): (Vec<RoutineView>, Vec<RoutineView>) = self
            .routines_of(&self.channel_id.clone())
            .into_iter()
            .partition(|r| r.kind == consort_client::proactivity::RoutineKind::FollowUp);
        let mut routines = routines;
        // The check-in comes first, as in the design.
        routines.sort_by_key(|r| r.kind != consort_client::proactivity::RoutineKind::CheckIn);
        let on = routines.iter().filter(|r| r.enabled).count();
        let channel_id = self.channel_id.clone();
        let new_button = icon_text_button("routine-new", "plus", "New routine", t)
            .debug_selector(|| "routine-new".into())
            .px(t.space.sm + t.space.xxs)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_routine_dialog(None, &channel_id, window, cx)
            }));
        if routines.is_empty() && follow_ups.is_empty() {
            let channel_id = self.channel_id.clone();
            return div().flex_1().min_h_0().p(t.space.lg).child(
                div()
                    .debug_selector(|| "routines-empty".into())
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
                            .child(icon("routine", t.icon.size, c.text_secondary)),
                    )
                    .child(text(
                        format!("No routines in {channel_name}"),
                        t.text.ui_strong,
                        c.text_primary,
                    ))
                    .child(div().text_center().child(text(
                        "A routine runs a prompt on a schedule. For example: check the frost forecast every day at 6:00.",
                        t.text.ui,
                        c.text_secondary,
                    )))
                    .child(
                        button("routine-new-empty", "New routine", ButtonKind::Secondary, t)
                            .mt(t.space.sm)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_routine_dialog(None, &channel_id, window, cx)
                            })),
                    ),
            );
        }
        let summary = match (routines.len(), on) {
            (1, 1) => "1 routine, on".to_owned(),
            (1, _) => "1 routine, off".to_owned(),
            (n, on) => format!("{n} routines, {on} on"),
        };
        let mut list = div()
            .flex()
            .flex_col()
            .gap(t.space.xxs)
            .px(t.space.sm)
            .pt(t.space.xs)
            .pb(t.space.sm);
        for routine in &routines {
            list = list.child(self.routine_row(routine, RowPlace::Panel, t, cx));
        }
        if !follow_ups.is_empty() {
            list = list.child(self.render_follow_ups(&follow_ups, t, cx));
        }
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
                            .debug_selector(|| "routines-summary".into())
                            .flex_1()
                            .child(text(summary, t.text.ui, c.text_secondary)),
                    )
                    .child(new_button),
            )
            .child(panel_scroll("routines-scroll", list, t))
    }

    /// One routine row: the icon, three lines, the switch, and on hover Run
    /// now, Run history and the menu.
    pub fn routine_row(
        &mut self,
        routine: &RoutineView,
        place: RowPlace,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let k = t.component.knowledge;
        let now = consort_client::format::now_ms();
        let id = routine.id.clone();
        let mut status = r::status_line(routine, now);
        let summary = r::summary_line(routine);
        let (words, action) = summary
            .rsplit_once(", ")
            .map(|(w, a)| (w.to_owned(), format!(", {a}")))
            .unwrap_or((summary.clone(), String::new()));
        // A check-in: "Every 6 hours, uses check-in.md", and its last result
        // from the proactivity log, such as "Stayed silent at 7:25.".
        let check_in = routine.kind == consort_client::proactivity::RoutineKind::CheckIn;
        let (words, action) = if check_in {
            (format!("{words}, uses "), String::new())
        } else {
            (words, action)
        };
        if check_in
            && routine.enabled
            && let Some(result) = consort_client::proactivity::check_in_line(
                &self.state.proactivity,
                &routine.channel_id,
            )
        {
            status.result = result;
        }
        let note_scope = self.memory_scope(&routine.channel_id);
        let menu_open = self.menu.as_ref().is_some_and(|m| {
            m.items.first().is_some_and(|i| {
                i.action == MenuAction::Knowledge(KnowledgeAction::RunRoutine(id.clone()))
            })
        });
        let hovered = self.knowledge.hover.as_deref() == Some(id.as_str()) || menu_open;
        let enabled = routine.enabled;
        let row_id = SharedString::from(format!("routine-row-{}", routine.id));
        let selector = row_id.to_string();
        let hover_id = id.clone();
        let menu_routine = routine.clone();
        let more_routine = routine.clone();
        let (run_id, history_id, switch_id) = (id.clone(), id.clone(), id.clone());
        let lines = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(t.space.xxs)
            .child(line(
                routine.name.clone(),
                t.text.ui_strong,
                if enabled {
                    c.text_primary
                } else {
                    c.text_secondary
                },
            ))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .child(line(
                        words,
                        detail(t),
                        if enabled {
                            c.text_secondary
                        } else {
                            c.text_tertiary
                        },
                    ))
                    .child(line(action, detail(t), c.text_tertiary))
                    .when(check_in, |d| {
                        d.child(
                            text(
                                consort_client::proactivity::CHECK_IN_NOTE_PATH,
                                Type {
                                    size: detail(t).size,
                                    ..t.text.mono
                                },
                                c.text_secondary,
                            )
                            .font_family(t.font_mono.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.xs + t.space.xxs)
                    .pt(t.space.xxs)
                    .min_w_0()
                    .child(status_dot(status.tone, t))
                    .child(line(
                        status.result.clone(),
                        detail(t),
                        if status.tone == StatusTone::Failed {
                            c.status_failed
                        } else {
                            c.text_tertiary
                        },
                    ))
                    .when(!status.next.is_empty(), |d| {
                        d.child(line(status.next.clone(), detail(t), c.text_tertiary))
                    }),
            )
            .when(hovered, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.xs + t.space.xxs)
                        .pt(t.space.sm)
                        .child(
                            row_button(
                                format!("routine-run-{}", routine.id),
                                "Run now",
                                Some("play"),
                                true,
                                t,
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.run_knowledge_menu(
                                        KnowledgeAction::RunRoutine(run_id.clone()),
                                        window,
                                        cx,
                                    )
                                },
                            )),
                        )
                        .child(
                            row_button(
                                format!("routine-history-{}", routine.id),
                                "Run history",
                                None,
                                false,
                                t,
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    match place {
                                        RowPlace::Panel => {
                                            this.open_routine(&history_id, window, cx)
                                        }
                                        RowPlace::Settings => {
                                            this.open_routine_in_settings(&history_id, cx)
                                        }
                                    }
                                },
                            )),
                        )
                        .when(check_in, |d| {
                            let scope = note_scope.clone();
                            d.child(
                                row_button(
                                    format!("routine-note-{}", routine.id),
                                    "Open note",
                                    None,
                                    false,
                                    t,
                                )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        let path = consort_client::proactivity::CHECK_IN_NOTE_PATH;
                                        match place {
                                            RowPlace::Panel => {
                                                this.open_channel_tab(
                                                    crate::app::ChannelTab::Memory,
                                                    cx,
                                                );
                                                this.open_note_in_panel(&scope, path, window, cx);
                                            }
                                            // The note opens in Settings > Memory.
                                            RowPlace::Settings => {
                                                this.open_settings(
                                                    crate::app::SettingsSection::Memory,
                                                    cx,
                                                );
                                                this.open_note_in_settings(&scope, path, cx);
                                            }
                                        }
                                    },
                                )),
                            )
                        })
                        .child(div().flex_1())
                        .child(
                            icon_button(
                                SharedString::from(format!("routine-more-{}", routine.id)),
                                "more-horizontal",
                                t.space.xl,
                                t.icon.size,
                                c.text_primary,
                                c.hover_selected,
                                t,
                                cx,
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(
                                move |this, event: &gpui_kit::ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.open_menu(
                                        OpenMenu::at(event.position(), routine_menu(&more_routine)),
                                        cx,
                                    );
                                },
                            )),
                        ),
                )
            });
        div()
            .id(row_id)
            .debug_selector(move || selector)
            .flex()
            .items_start()
            .gap(t.space.md)
            .py(k.row_pad_y)
            .px(t.space.sm)
            .rounded(t.radius.md)
            .cursor_pointer()
            .when(hovered, |d| {
                d.bg(if place == RowPlace::Panel {
                    c.surface_card
                } else {
                    c.hover_canvas
                })
            })
            .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                if *on {
                    this.knowledge.hover = Some(hover_id.clone());
                } else if this.knowledge.hover.as_deref() == Some(hover_id.as_str()) {
                    this.knowledge.hover = None;
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(t.text.ui.line)
                    .w(t.icon.size)
                    .when(!enabled, |d| d.opacity(t.opacity.answered_dim))
                    .child(icon(
                        if check_in { "eye" } else { "routine" },
                        t.icon.size,
                        c.text_secondary,
                    )),
            )
            .child(lines)
            .child(
                small_switch(format!("routine-switch-{}", routine.id), enabled, t)
                    .mt(t.border_width)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.run_knowledge_menu(
                            KnowledgeAction::SetRoutineEnabled(switch_id.clone(), !enabled),
                            window,
                            cx,
                        )
                    })),
            )
            .on_click(cx.listener(move |this, _, window, cx| match place {
                RowPlace::Panel => this.open_routine(&id, window, cx),
                // A click in Settings stays in Settings.
                RowPlace::Settings => this.open_routine_in_settings(&id, cx),
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_menu(
                        OpenMenu::at(event.position, routine_menu(&menu_routine)),
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .into_any_element()
    }

    /// A routine opened in the Channel panel: its summary, Run now, Edit,
    /// and the run history of the last 30 days.
    pub fn render_routine_page(
        &mut self,
        routine: &RoutineView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_routine_page_in(routine, RowPlace::Panel, t, cx)
    }

    /// A routine opened in Settings > Routines, in place of the list. It has
    /// Back at the top left and "Show in channel", which leaves Settings on
    /// purpose.
    pub fn render_settings_routine(
        &mut self,
        routine: &RoutineView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_routine_page_in(routine, RowPlace::Settings, t, cx)
    }

    /// The page of one routine, in the Channel panel or in Settings.
    fn render_routine_page_in(
        &mut self,
        routine: &RoutineView,
        place: RowPlace,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let k = t.component.knowledge;
        let now = consort_client::format::now_ms();
        self.load_runs(&routine.id, cx);
        let channel_name = self
            .state
            .channel(&routine.channel_id)
            .map(|ch| ch.name.clone())
            .unwrap_or_default();
        let (switch_id, run_id, edit_id, show_id) = (
            routine.id.clone(),
            routine.id.clone(),
            routine.id.clone(),
            routine.id.clone(),
        );
        let enabled = routine.enabled;
        let more_routine = routine.clone();
        let menu_routine = routine.clone();
        let in_settings = place == RowPlace::Settings;
        let title = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(line(routine.name.clone(), t.text.ui_strong, c.text_primary))
            .child(line(
                format!("Routine in {channel_name}"),
                detail(t),
                c.text_tertiary,
            ));
        let switch = small_switch("routine-page-switch", enabled, t)
            .mr(t.space.xs)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.run_knowledge_menu(
                    KnowledgeAction::SetRoutineEnabled(switch_id.clone(), !enabled),
                    window,
                    cx,
                )
            }));
        let more = icon_button(
            "routine-page-more",
            "more-horizontal",
            t.space.xxl,
            t.icon.size,
            c.text_secondary,
            c.hover_sidebar,
            t,
            cx,
        )
        .on_click(
            cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                this.open_menu(
                    OpenMenu::at(event.position(), routine_menu(&more_routine)),
                    cx,
                )
            }),
        );
        let header = if in_settings {
            // Back is at the top left of the content. "Show in channel"
            // leaves Settings and says so.
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(t.space.lg)
                .child(
                    div().flex().child(
                        icon_text_button("routine-back", "chevron-left", "Back", t)
                            .debug_selector(|| "routine-back".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_settings_routine(cx);
                            })),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.md)
                        .child(title)
                        .child(
                            leaving_link("routine-show-in-channel", "Show in channel", t)
                                .debug_selector(|| "routine-show-in-channel".into())
                                .mr(t.space.sm)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_routine(&show_id, window, cx)
                                })),
                        )
                        .child(switch)
                        .child(more),
                )
        } else {
            crate::ui::parts::panel_header(t)
                .pl(t.space.sm)
                .child(
                    icon_button(
                        "routine-back",
                        "chevron-left",
                        t.space.xxl,
                        t.icon.size,
                        c.text_secondary,
                        c.hover_sidebar,
                        t,
                        cx,
                    )
                    .debug_selector(|| "routine-back".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.knowledge.routine_open = None;
                        cx.notify();
                    })),
                )
                .child(title)
                .child(switch)
                .child(more)
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
                )
        };
        let row = |label: &str, value: AnyElement| {
            div()
                .flex()
                .gap(t.space.md)
                .child(
                    text(label.to_owned(), detail(t), c.text_tertiary)
                        .w(k.summary_label)
                        .flex_none(),
                )
                .child(div().flex_1().min_w_0().child(value))
        };
        let action = match &routine.action {
            RoutineActionView::Ask => format!("Asks the assistant in {channel_name}"),
            RoutineActionView::Task { title, .. } => format!("Starts the task {title}"),
        };
        let next = r::next_runs_label(routine, now);
        let summary = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(t.space.sm - t.space.xxs)
            .pt(t.space.md)
            .pb(t.space.md + t.space.xxs)
            .when(!in_settings, |d| d.px(t.space.lg))
            .border_b(t.border_width)
            .border_color(c.border_subtle)
            .child(row(
                "Schedule",
                text(
                    r::schedule_detail(&routine.schedule),
                    detail(t),
                    c.text_primary,
                )
                .into_any_element(),
            ))
            .child(row(
                "Action",
                text(action, detail(t), c.text_primary).into_any_element(),
            ))
            .child(row(
                "Prompt",
                selectable(
                    SharedString::from(format!("routine-prompt-{}", routine.id)),
                    routine.prompt.clone(),
                    detail(t),
                    c.text_primary,
                )
                .into_any_element(),
            ))
            .child(row(
                "Next runs",
                text(
                    if next.is_empty() {
                        r::OFF_TEXT.to_owned()
                    } else {
                        next
                    },
                    detail(t),
                    c.text_primary,
                )
                .into_any_element(),
            ))
            .child(
                div()
                    .flex()
                    .gap(t.space.sm)
                    .pt(t.space.sm)
                    .child(
                        icon_text_button("routine-page-run", "play", "Run now", t)
                            .debug_selector(|| "routine-page-run".into())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run_knowledge_menu(
                                    KnowledgeAction::RunRoutine(run_id.clone()),
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(
                        button("routine-page-edit", "Edit", ButtonKind::Secondary, t)
                            .debug_selector(|| "routine-page-edit".into())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_routine_dialog(Some(edit_id.clone()), "", window, cx)
                            })),
                    ),
            );
        let runs = r::recent_runs(routine, now);
        let mut list = div()
            .flex()
            .flex_col()
            .gap(t.space.xxs)
            .when(!in_settings, |d| d.px(t.space.sm))
            .pb(t.space.sm);
        if routine.runs_loaded && runs.is_empty() {
            list = list.child(div().px(t.space.sm).py(t.space.sm).child(text(
                "This routine has not run in the last 30 days. Select Run now to run it.",
                t.text.ui,
                c.text_secondary,
            )));
        }
        if !routine.runs_loaded {
            list = list.child(div().px(t.space.sm).py(t.space.sm).child(text(
                "Loading the runs",
                t.text.ui,
                c.text_tertiary,
            )));
        }
        for run in runs {
            list =
                list.child(self.run_row(run, &routine.schedule, &routine.channel_id, place, t, cx));
        }
        let runs_label = div()
            .debug_selector(|| "routine-runs-label".into())
            .flex()
            .flex_none()
            .items_baseline()
            .gap(t.space.xs + t.space.xxs)
            .pt(t.space.md + t.space.xxs)
            .pb(t.space.xs + t.space.xxs)
            .when(!in_settings, |d| d.px(t.space.lg))
            .child(text("Run history", t.text.label, c.text_secondary))
            .child(text("last 30 days", detail(t), c.text_tertiary));
        // A right click on the page opens the menu of the routine, as on
        // its row.
        let on_right_click = cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            this.open_menu(
                OpenMenu::at(event.position, routine_menu(&menu_routine)),
                cx,
            );
            cx.stop_propagation();
        });
        if in_settings {
            return div()
                .id("settings-routine-page")
                .debug_selector(|| "settings-routine".into())
                .flex_1()
                .h_full()
                .min_w_0()
                .overflow_y_scroll()
                .pt(t.space.page - t.space.sm)
                .px(t.space.page)
                .pb(t.space.page)
                .on_mouse_down(MouseButton::Right, on_right_click)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .max_w(t.component.settings_detail_max)
                        .child(header)
                        .child(div().pt(t.space.lg).child(summary))
                        .child(runs_label)
                        .child(list),
                )
                .into_any_element();
        }
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
            .on_mouse_down(MouseButton::Right, on_right_click)
            .child(header)
            .child(summary)
            .child(runs_label)
            .child(panel_scroll("routine-runs-scroll", list, t))
            .into_any_element()
    }

    /// One run of the run history.
    fn run_row(
        &mut self,
        run: &RoutineRunView,
        schedule: &consort_client::routines::RoutineSchedule,
        channel_id: &str,
        place: RowPlace,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let k = t.component.knowledge;
        let now = consort_client::format::now_ms();
        let failed = run.status == RoutineRunStatus::Error;
        let reason = run_reason(run, schedule).filter(|_| {
            matches!(
                run.status,
                RoutineRunStatus::Error | RoutineRunStatus::Skipped
            )
        });
        // In the panel a run links to its message or task in place. In
        // Settings the same link leaves Settings, so it says so.
        let in_settings = place == RowPlace::Settings;
        let link_id = SharedString::from(format!("run-link-{}", run.id));
        let link_selector = link_id.to_string();
        let link: Option<AnyElement> = match (&run.task_id, &run.message_id) {
            (Some(task_id), _) => {
                let (task_id, channel_id) = (task_id.clone(), channel_id.to_owned());
                Some(if in_settings {
                    leaving_link(link_id, "Show task", t)
                        .debug_selector(move || link_selector)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.show_task(&task_id, &channel_id, window, cx)
                        }))
                        .into_any_element()
                } else {
                    div()
                        .id(link_id)
                        .cursor_pointer()
                        .child(text("Task", t.text.ui, c.text_secondary).underline())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_thread(task_id.clone(), window, cx)
                        }))
                        .into_any_element()
                })
            }
            (None, Some(message_id)) => {
                let (channel_id, message_id) = (channel_id.to_owned(), message_id.clone());
                Some(if in_settings {
                    leaving_link(link_id, "Show in channel", t)
                        .debug_selector(move || link_selector)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.reveal_message(&channel_id, &message_id, window, cx)
                        }))
                        .into_any_element()
                } else {
                    div()
                        .id(link_id)
                        .cursor_pointer()
                        .child(text("Message", t.text.ui, c.text_secondary).underline())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.reveal_message(&channel_id, &message_id, window, cx)
                        }))
                        .into_any_element()
                })
            }
            _ => None,
        };
        let selector = format!("run-{}", run.id);
        div()
            .id(SharedString::from(format!("run-{}", run.id)))
            .debug_selector(move || selector)
            .flex()
            .items_start()
            .gap(t.space.md)
            .py(t.space.sm - t.space.xxs)
            .px(t.space.sm)
            .rounded(t.radius.md)
            .when(failed, |d| d.bg(c.surface_card))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(t.text.ui.line)
                    .w(t.icon.size)
                    .child(run_glyph(run.status, t)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(t.space.xs + t.space.xxs)
                            .child(text(
                                r::run_time_label(run.started_at, now, schedule),
                                t.text.ui,
                                c.text_primary,
                            ))
                            .when(run.manual, |d| {
                                d.child(text("Run now", detail(t), c.text_tertiary))
                            }),
                    )
                    .child(text(
                        r::run_status_label(run.status),
                        detail(t),
                        run_color(run.status, t),
                    ))
                    .when_some(reason, |d, reason| {
                        d.child(div().pt(t.space.sm - t.space.xxs).child(selectable(
                            SharedString::from(format!("run-error-{}", run.id)),
                            reason,
                            detail(t),
                            c.text_secondary,
                        )))
                    }),
            )
            .child(
                text(r::run_duration_label(run), t.text.ui, c.text_tertiary)
                    .w(k.run_duration)
                    .flex_none()
                    .text_right(),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    // The longer links of Settings take the room they need.
                    .when(!in_settings, |d| d.w(k.run_link))
                    .children(link),
            )
            .into_any_element()
    }

    /// Leaves Settings for a task: its channel opens, then its thread.
    pub fn show_task(
        &mut self,
        task_id: &str,
        fallback_channel: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel = self
            .state
            .task(task_id)
            .map(|task| task.channel_id.clone())
            .unwrap_or_else(|| fallback_channel.to_owned());
        if self.state.channel(&channel).is_some() {
            self.open_channel(channel, window, cx);
        }
        // The page changes even when the channel is not known.
        self.page = Page::Main;
        self.open_thread(task_id.to_owned(), window, cx);
    }

    /// Opens a channel and shows one of its messages in the timeline.
    pub fn reveal_message(
        &mut self,
        channel_id: &str,
        message_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // From Settings this leaves Settings, even for the open channel.
        if self.channel_id != channel_id || self.page != Page::Main {
            self.open_channel(channel_id.to_owned(), window, cx);
        }
        let prefix = format!("{message_id}#");
        if let Some(ix) = self
            .timeline_keys
            .iter()
            .position(|key| key == message_id || key.starts_with(&prefix))
        {
            self.timeline.scroll_to_reveal_item(ix);
        }
        cx.notify();
    }

    /// Settings > Routines: every routine of every channel, grouped by
    /// channel, with a text filter, a status filter and a channel filter.
    pub fn render_routines_settings(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        // A routine opens in place of the list, with Back.
        if let Some(id) = self.knowledge.settings_routine.clone() {
            match self.state.routines.iter().find(|r| r.id == id).cloned() {
                Some(routine) => return self.render_settings_routine(&routine, t, cx),
                None => self.knowledge.settings_routine = None,
            }
        }
        let query = self
            .knowledge_inputs
            .filter
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let all = self.state.routines.clone();
        let count = |f: StatusFilter| all.iter().filter(|r| f.matches(r)).count();
        let filter = self.knowledge.status_filter;
        let channel_filter = self.knowledge.channel_filter.clone();
        let shown: Vec<&RoutineView> = all
            .iter()
            .filter(|r| filter.matches(r))
            .filter(|r| channel_filter.as_ref().is_none_or(|ch| &r.channel_id == ch))
            .filter(|r| {
                query.is_empty()
                    || r.name.to_lowercase().contains(&query)
                    || r.prompt.to_lowercase().contains(&query)
            })
            .collect();
        let filter_focused = {
            use gpui_kit::Focusable as _;
            self.knowledge_inputs
                .filter
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        };
        let mut statuses = div()
            .flex()
            .gap(t.space.xxs)
            .p(t.space.xxs)
            .bg(c.surface_card)
            .rounded(t.radius.md);
        for status in StatusFilter::ALL {
            let active = status == filter;
            let n = count(status);
            let selector = format!("routine-filter-{}", status.label());
            statuses = statuses.child(
                focusable(
                    div().id(SharedString::from(format!(
                        "routine-filter-{}",
                        status.label()
                    ))),
                    t,
                )
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .py(t.space.xs)
                .px(t.space.sm + t.space.xxs)
                .rounded(t.radius.sm)
                .cursor_pointer()
                .when(active, |d| d.bg(c.surface_selected))
                .when(!active, |d| d.hover(|s| s.bg(c.hover_card)))
                .child(text(
                    status.label(),
                    if active { t.text.ui_strong } else { t.text.ui },
                    if active {
                        c.text_primary
                    } else {
                        c.text_secondary
                    },
                ))
                .child(text(
                    n.to_string(),
                    t.text.ui,
                    if status == StatusFilter::Failed && n > 0 {
                        c.status_failed
                    } else {
                        c.text_tertiary
                    },
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.knowledge.status_filter = status;
                    cx.notify();
                })),
            );
        }
        let channel_label = channel_filter
            .as_ref()
            .and_then(|id| self.state.channel(id))
            .map(|ch| ch.name.clone())
            .unwrap_or_else(|| "Every channel".to_owned());
        let channel_items: Vec<MenuItem> = std::iter::once(
            MenuItem::new(
                "Every channel",
                MenuAction::Knowledge(KnowledgeAction::FilterChannel(None)),
            )
            .checked(channel_filter.is_none()),
        )
        .chain(self.state.channels.iter().map(|ch| {
            MenuItem::new(
                ch.name.clone(),
                MenuAction::Knowledge(KnowledgeAction::FilterChannel(Some(ch.id.clone()))),
            )
            .checked(channel_filter.as_deref() == Some(ch.id.as_str()))
        }))
        .collect();
        let channel_select = focusable(div().id("routine-channel-filter"), t)
            .relative()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .py(t.space.xs + t.space.xxs)
            .px(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.md)
            .cursor_pointer()
            .child(self.triggers.track("routine-channel-filter"))
            .child(text(channel_label, t.text.ui, c.text_primary))
            .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(bounds) = this.triggers.get("routine-channel-filter") {
                    this.open_menu(OpenMenu::under(bounds, channel_items.clone(), false), cx);
                }
            }));
        let channel_id = self.channel_id.clone();
        let header = div()
            .flex()
            .items_start()
            .justify_between()
            .gap(t.space.xl)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs + t.space.xxs)
                    .child(text("Routines", t.text.heading, c.text_primary))
                    .child(text(
                        "Every routine in every channel. The hub runs them, so they run when this app is closed.",
                        t.text.ui,
                        c.text_secondary,
                    )),
            )
            .child(
                icon_text_button("settings-routine-new", "plus", "New routine", t).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.open_routine_dialog(None, &channel_id, window, cx)
                    }),
                ),
            );
        let filters = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .child(
                div()
                    .debug_selector(|| "routine-text-filter".into())
                    .w(t.component.settings_segmented - t.space.page - t.space.xxl)
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.xs + t.space.xxs)
                    .px(t.space.md)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(if filter_focused {
                        c.accent_text
                    } else {
                        c.border_strong
                    })
                    .rounded(t.radius.md)
                    .child(icon("search", t.icon.dense, c.text_tertiary))
                    .child(div().flex_1().child(crate::ui::parts::input(
                        &self.knowledge_inputs.filter,
                        t.text.ui,
                    ))),
            )
            .child(statuses)
            .child(div().flex_1())
            .child(channel_select);
        let mut groups = div().flex().flex_col().gap(t.space.xl);
        // The open channel first, then the others in the order of the
        // sidebar.
        let mut ordered: Vec<&consort_client::ChannelView> = self.state.channels.iter().collect();
        ordered.sort_by_key(|ch| ch.id != self.channel_id);
        let channels: Vec<(String, String, String, bool)> = ordered
            .into_iter()
            .map(|ch| {
                (
                    ch.id.clone(),
                    ch.name.clone(),
                    ch.icon.to_string(),
                    ch.kind == ChannelKind::Tutti,
                )
            })
            .collect();
        // Only the channels that have routines that match.
        let mut any = false;
        for (id, name, icon_name, tutti) in channels {
            let rows: Vec<&RoutineView> = shown
                .iter()
                .copied()
                .filter(|r| r.channel_id == id)
                .collect();
            if rows.is_empty() {
                continue;
            }
            any = true;
            let mut card = div()
                .flex()
                .flex_col()
                .gap(t.space.xxs)
                .p(t.space.xs)
                .border(t.border_width)
                .border_color(c.border_subtle)
                .rounded(t.radius.lg);
            for routine in &rows {
                card = card.child(self.routine_row(routine, RowPlace::Settings, t, cx));
            }
            let selector = format!("routine-group-{name}");
            groups = groups.child(
                div()
                    .debug_selector(move || selector)
                    .flex()
                    .flex_col()
                    .gap(t.space.sm + t.space.xxs)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .px(t.space.xs)
                            .child(if tutti {
                                crate::ui::mark::still_mark(t.icon.size, t)
                            } else {
                                icon(&icon_name, t.icon.size, c.text_secondary).into_any_element()
                            })
                            .child(text(name, t.text.ui_strong, c.text_primary))
                            .child(text(rows.len().to_string(), t.text.ui, c.text_tertiary)),
                    )
                    .child(card),
            );
        }
        if !any {
            groups = groups.child(text(
                if all.is_empty() {
                    "No routines yet. Select New routine to make one, such as a morning brief on weekdays."
                } else {
                    "No routine matches these filters. Change the filters to see more."
                },
                t.text.ui,
                c.text_secondary,
            ));
        }
        div()
            .id("settings-page")
            .debug_selector(|| "settings-routines".into())
            .flex_1()
            .h_full()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(&self.knowledge.settings_routines_scroll)
            .pt(t.space.page - t.space.sm)
            .px(t.space.page)
            .pb(t.space.page)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xl)
                    .max_w(t.component.settings_detail_max)
                    .child(header)
                    .child(filters)
                    .child(groups),
            )
            .into_any_element()
    }

    /// The confirmation before a routine is deleted.
    pub fn confirm_delete_routine_dialog(
        &mut self,
        routine_id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> (String, String, gpui_kit::Pixels, Div, Div) {
        let name = self
            .state
            .routines
            .iter()
            .find(|r| r.id == routine_id)
            .map(|r| r.name.clone())
            .unwrap_or_default();
        let id = routine_id.to_owned();
        (
            format!("Delete {name}?"),
            "The routine stops, and its run history goes with it. Messages and tasks that its runs made stay. You cannot undo this.".to_owned(),
            t.layout.dialog_width,
            div(),
            self.dialog_actions_kind(
                "Delete routine",
                ButtonKind::Danger,
                t,
                cx,
                move |this, window, cx| {
                    let client = this.client.clone();
                    let routine_id = id.clone();
                    if this.knowledge.routine_open.as_deref() == Some(routine_id.as_str()) {
                        this.knowledge.routine_open = None;
                    }
                    this.submit_dialog(
                        async move { client.run(Command::DeleteRoutine { routine_id }).await },
                        window,
                        cx,
                    );
                },
            ),
        )
    }

    /// The routine dialog: New routine or Edit routine.
    pub fn routine_dialog(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (String, String, gpui_kit::Pixels, Div, Div) {
        let c = &t.colors;
        let k = t.component.knowledge;
        let now = consort_client::format::now_ms();
        let Some(form) = self.routine_form(cx) else {
            return (
                String::new(),
                String::new(),
                t.layout.dialog_wide,
                div(),
                div(),
            );
        };
        let editing = self
            .knowledge
            .routine_dialog
            .as_ref()
            .and_then(|d| d.routine_id.clone());
        let errors = form.errors(now);
        let error_of = |field: r::FormField| {
            errors
                .iter()
                .find(|e| e.field == field)
                .map(|e| e.message.clone())
        };
        // A field shows its error once it has text, so an empty new form
        // does not open in red. Save stays off either way.
        let fresh = editing.is_none();
        let shown_error = |field: r::FormField, has_text: bool| {
            if fresh && !has_text {
                None
            } else {
                error_of(field)
            }
        };
        let ki = KnowledgeFields::of(self);
        let focus_of = |input: &gpui_kit::Entity<gpui_kit::component::input::InputState>,
                        cx: &Context<Self>| {
            use gpui_kit::Focusable as _;
            input.read(cx).focus_handle(cx).is_focused(window)
        };
        let (name_focused, title_focused) = (focus_of(&ki.name, cx), focus_of(&ki.title, cx));
        let channel = self.state.channel(&form.channel_id).cloned();
        let channel_name = channel
            .as_ref()
            .map(|ch| ch.name.clone())
            .unwrap_or_default();
        let channel_items: Vec<MenuItem> = self
            .state
            .channels
            .iter()
            .map(|ch| {
                MenuItem::new(
                    ch.name.clone(),
                    MenuAction::Knowledge(KnowledgeAction::PickChannel(ch.id.clone())),
                )
                .checked(ch.id == form.channel_id)
            })
            .collect();
        let channel_select = focusable(div().id("routine-channel"), t)
            .debug_selector(|| "routine-channel".into())
            .relative()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .w(k.channel_field)
            .flex_none()
            .py(t.component.input_pad_y)
            .px(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.md)
            .cursor_pointer()
            .hover(|s| s.border_color(c.text_tertiary))
            .child(self.triggers.track("routine-channel"))
            .child(match &channel {
                Some(ch) if ch.kind == ChannelKind::Tutti => {
                    crate::ui::mark::still_mark(t.icon.size, t)
                }
                Some(ch) => icon(&ch.icon, t.icon.size, c.text_secondary).into_any_element(),
                None => div().into_any_element(),
            })
            .child(line(channel_name.clone(), t.text.ui, c.text_primary).flex_1())
            .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(bounds) = this.triggers.get("routine-channel") {
                    this.open_menu(OpenMenu::under(bounds, channel_items.clone(), true), cx);
                }
            }));
        let name_error = shown_error(r::FormField::Name, !form.name.trim().is_empty());
        let names = div()
            .flex()
            .gap(t.space.md)
            .child(div().flex_1().min_w_0().child(error_under(
                field_block("Name", input_box(&ki.name, name_focused, t), None, t),
                name_error,
                t,
            )))
            .child(field_block("Channel", channel_select, None, t));
        let starts_task = form.starts_task;
        let action_segment = crate::ui::settings::segmented(
            "routine-action",
            &[(false, "Ask the assistant"), (true, "Start a task")],
            starts_task,
            t,
            cx,
            |_| t.text.ui,
            |this, value, _, cx| this.edit_routine_form(cx, |form| form.starts_task = value),
        );
        let prompt_error = shown_error(r::FormField::Prompt, !form.prompt.trim().is_empty());
        let title_error = shown_error(r::FormField::Title, !form.title.trim().is_empty());
        let prompt_box = div()
            .debug_selector(|| "routine-prompt".into())
            .min_h(t.text.ui.line * 3.0 + t.space.md * 2.0)
            .py(t.space.sm + t.space.xxs)
            .px(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.md)
            .child(crate::ui::parts::textarea(&ki.prompt, t.text.ui));
        let what = div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .child(text("What it does", t.text.label, c.text_secondary))
            .child(action_segment)
            .when(starts_task, |d| {
                d.child(error_under(
                    input_box(&ki.title, title_focused, t)
                        .debug_selector(|| "routine-title".into()),
                    title_error,
                    t,
                ))
            })
            .child(error_under(prompt_box, prompt_error, t))
            .child(text(
                if starts_task {
                    "Each run starts a new task with this title and brief. The task reports in its own thread.".to_owned()
                } else {
                    format!(
                        "The assistant gets this prompt in {channel_name} as a message from the routine, and answers in the timeline."
                    )
                },
                detail(t),
                c.text_tertiary,
            ));
        let schedule = self.render_schedule_field(&form, fresh, t, window, cx);
        let model = self.routine_model_field(&form, &channel_name, t, cx);
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.lg + t.space.xxs)
            .child(names)
            .child(what)
            .child(schedule)
            .child(model);
        let can_save = errors.is_empty() && !self.dialog_busy();
        let save_label = if editing.is_some() {
            "Save changes"
        } else {
            "Create routine"
        };
        let save = button("dialog-confirm", save_label, ButtonKind::Primary, t)
            .debug_selector(|| "dialog-confirm".into())
            .px(t.space.md + t.space.xxs)
            .when(!can_save, |b| {
                b.opacity(t.opacity.nav_disabled).cursor_default()
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if can_save {
                    this.save_routine(window, cx)
                }
            }));
        let left = match &editing {
            Some(id) => {
                let id = id.clone();
                button("routine-delete", "Delete routine", ButtonKind::Danger, t)
                    .debug_selector(|| "routine-delete".into())
                    .px(t.space.xs)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_dialog(Dialog::ConfirmDeleteRoutine(id.clone()), window, cx)
                    }))
                    .into_any_element()
            }
            None => text(
                "It starts on. You can turn it off at any time.",
                detail(t),
                c.text_tertiary,
            )
            .into_any_element(),
        };
        let actions = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(div().flex().flex_1().min_w_0().child(left))
            .child(
                button("dialog-cancel", "Cancel", ButtonKind::Secondary, t)
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            )
            .child(save);
        if can_save {
            self.dialog_confirm.replace(Some(std::rc::Rc::new(
                |this: &mut ConsortApp, window: &mut Window, cx: &mut Context<ConsortApp>| {
                    this.save_routine(window, cx)
                },
            )));
        }
        let (title, description) = if editing.is_some() {
            (
                "Edit routine",
                "Changes apply from the next run. Runs that already started keep their old settings.",
            )
        } else {
            (
                "New routine",
                "A routine runs a prompt on a schedule in one channel. The hub runs it, so it runs when this app is closed.",
            )
        };
        (
            title.to_owned(),
            description.to_owned(),
            t.layout.dialog_wide,
            body,
            actions,
        )
    }

    /// The model and effort of the routine dialog. "Ask the assistant" uses
    /// the channel's model and shows it, with "Same as <channel>". "Start a
    /// task" has its own model and effort.
    fn routine_model_field(
        &mut self,
        form: &consort_client::routines::RoutineForm,
        channel_name: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let controls: AnyElement = if form.starts_task {
            let target = crate::ui::model_picker::PickerTarget::RoutineTask;
            div()
                .flex()
                .items_center()
                .gap(t.space.xs)
                .child(self.render_model_trigger(
                    "routine-model",
                    target.clone(),
                    crate::ui::model_picker::TriggerStyle::Chip,
                    t,
                    cx,
                ))
                .children(self.render_effort_trigger(
                    "routine-effort",
                    target,
                    crate::ui::model_picker::TriggerStyle::Chip,
                    t,
                    cx,
                ))
                .into_any_element()
        } else {
            // The channel's model, shown as it is. It changes in the
            // channel's settings, not here.
            let selection = self
                .state
                .channel(&form.channel_id)
                .and_then(|ch| ch.coordinator_selection.as_ref())
                .map(|e| e.selection.clone());
            let logo = selection
                .as_ref()
                .and_then(|s| self.state.account_logo(&s.account_label).cloned());
            let effort = selection
                .as_ref()
                .and_then(|s| s.effort().map(str::to_owned))
                .map(|e| {
                    let mut chars = e.chars();
                    chars
                        .next()
                        .map(|f| f.to_uppercase().collect::<String>() + chars.as_str())
                        .unwrap_or_default()
                });
            div()
                .debug_selector(|| "routine-model-same".into())
                .flex()
                .items_center()
                .gap(t.space.md)
                .px(t.space.sm)
                .h(t.component.composer_chip)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.xs + t.space.xxs)
                        .when_some(logo, |d, logo| {
                            d.child(crate::ui::parts::provider_logo(
                                &logo,
                                t.icon.dense,
                                c.text_primary,
                            ))
                        })
                        .child(text(
                            selection
                                .as_ref()
                                .map(|s| s.model_name.clone())
                                .unwrap_or_else(|| "The channel's model".to_owned()),
                            t.text.label,
                            c.text_secondary,
                        ))
                        .when_some(selection.as_ref(), |d, s| {
                            d.child(text(s.account_label.clone(), detail(t), c.text_tertiary))
                        }),
                )
                .when_some(effort, |d, effort| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.xs + t.space.xxs)
                            .child(icon("signal", t.icon.dense, c.text_tertiary))
                            .child(text(effort, detail(t), c.text_secondary)),
                    )
                })
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .child(text("Model and effort", t.text.label, c.text_secondary))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(t.space.md)
                    .py(t.space.xxs + t.border_width)
                    .px(t.space.xxs)
                    .rounded(t.radius.md)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .child(controls)
                    .child(
                        text(
                            if form.starts_task {
                                "For this task only".to_owned()
                            } else {
                                format!("Same as {channel_name}")
                            },
                            detail(t),
                            c.text_tertiary,
                        )
                        .pr(t.space.md),
                    ),
            )
    }
}

/// Clones of the text inputs of the routine dialog, so a view can hold
/// them while it calls methods of the app.
pub struct KnowledgeFields {
    pub name: gpui_kit::Entity<gpui_kit::component::input::InputState>,
    pub title: gpui_kit::Entity<gpui_kit::component::input::InputState>,
    pub prompt: gpui_kit::Entity<gpui_kit::component::input::TextareaState>,
    pub time: gpui_kit::Entity<gpui_kit::component::input::InputState>,
    pub every: gpui_kit::Entity<gpui_kit::component::input::InputState>,
    pub date: gpui_kit::Entity<gpui_kit::component::input::InputState>,
    pub cron: gpui_kit::Entity<gpui_kit::component::input::InputState>,
}

impl KnowledgeFields {
    pub fn of(app: &ConsortApp) -> KnowledgeFields {
        let k = &app.knowledge_inputs;
        KnowledgeFields {
            name: k.name.clone(),
            title: k.title.clone(),
            prompt: k.prompt.clone(),
            time: k.time.clone(),
            every: k.every.clone(),
            date: k.date.clone(),
            cron: k.cron.clone(),
        }
    }
}

/// A field with its error under it, in `status.failed`, with the failed
/// glyph.
pub fn error_under(field: impl IntoElement, error: Option<String>, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .flex_col()
        .gap(t.space.xs + t.space.xxs)
        .child(field)
        .when_some(error, |d, error| {
            d.child(
                div()
                    .debug_selector(|| "field-error".into())
                    .flex()
                    .items_start()
                    .gap(t.space.xs + t.space.xxs)
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(t.text.label.line)
                            .child(icon("failed", t.icon.dense, c.status_failed)),
                    )
                    .child(text(error, detail(t), c.status_failed).flex_1().min_w_0()),
            )
        })
}

/// A list that scrolls inside the Channel panel, with soft edges.
pub fn panel_scroll(id: &'static str, list: Div, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .child(div().id(id).size_full().overflow_y_scroll().child(list))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(t.space.sm)
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(c.surface_sidebar, 0.),
                    linear_color_stop(c.sidebar_clear, 1.),
                )),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(t.layout.fade_bottom)
                .bg(linear_gradient(
                    0.,
                    linear_color_stop(c.surface_sidebar, 0.),
                    linear_color_stop(c.sidebar_clear, 1.),
                )),
        )
}
