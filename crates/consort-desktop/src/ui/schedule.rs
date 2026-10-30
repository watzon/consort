//! The schedule field of the routine dialog: the presets Every day,
//! Weekdays, Weekly, Interval, Once and Cron, their inputs, the time zone
//! select with its search, the calendar of the date field, and the preview
//! of the next runs. Each error sits under its field.

use consort_client::routines::{
    self as r, DAY_NAMES, FormField, IntervalUnit, Preset, RoutineForm,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, Window, component::input::InputState, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    knowledge::KnowledgeAction,
    ui::{
        menu::{MenuAction, MenuAnchor, MenuItem, OpenMenu, floating},
        parts::{detail, focusable, icon, line, text},
        routines::error_under,
    },
};

/// A preset chip or a day button: `accent.text` border and text on
/// `tint.accent` when chosen.
fn choice(
    id: impl Into<SharedString>,
    label: &str,
    chosen: bool,
    round: bool,
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
        .justify_center()
        .py(t.space.xs)
        .px(t.space.sm + t.space.xxs)
        .rounded(if round { t.radius.full } else { t.radius.md })
        .border(t.border_width)
        .cursor_pointer()
        .when(chosen, |d| {
            d.border_color(c.accent_text)
                .bg(c.tint_accent)
                .text_color(c.accent_text)
        })
        .when(!chosen, |d| {
            d.border_color(c.border_strong)
                .text_color(c.text_secondary)
                .hover(|s| s.bg(c.hover_card).text_color(c.text_primary))
        })
        .type_style(t.text.ui)
        .child(label.to_owned())
}

/// A one-line input of the schedule, red while it has an error.
fn schedule_input(
    input: &Entity<InputState>,
    focused: bool,
    failed: bool,
    mono: bool,
    t: &Tokens,
) -> Div {
    let c = &t.colors;
    div()
        .py(t.space.xs + t.space.xxs)
        .px(t.space.md)
        .bg(c.surface_card)
        .border(t.border_width)
        .border_color(if failed {
            c.status_failed
        } else if focused {
            c.accent_text
        } else {
            c.border_strong
        })
        .rounded(t.radius.md)
        .when(mono, |d| d.font_family(t.font_mono.clone()))
        .child(crate::ui::parts::input(
            input,
            if mono { t.text.mono } else { t.text.ui },
        ))
}

impl ConsortApp {
    /// The schedule field. `fresh` is true for a new routine: an empty
    /// field does not show its error yet.
    pub fn render_schedule_field(
        &mut self,
        form: &RoutineForm,
        fresh: bool,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let k = t.component.knowledge;
        let now = consort_client::format::now_ms();
        let s = &form.schedule;
        let errors = s.errors(now);
        let error_of = |field: FormField| {
            errors
                .iter()
                .find(|e| e.field == field)
                .map(|e| e.message.clone())
        };
        let ki = crate::ui::routines::KnowledgeFields::of(self);
        let focus_of = |input: &Entity<InputState>, cx: &Context<Self>| {
            use gpui_kit::Focusable as _;
            input.read(cx).focus_handle(cx).is_focused(window)
        };
        let (time_focused, every_focused, date_focused, cron_focused) = (
            focus_of(&ki.time, cx),
            focus_of(&ki.every, cx),
            focus_of(&ki.date, cx),
            focus_of(&ki.cron, cx),
        );
        let mut presets = div().flex().flex_wrap().gap(t.space.xs + t.space.xxs);
        for preset in Preset::ALL {
            presets = presets.child(
                choice(
                    format!("preset-{}", preset.label()),
                    preset.label(),
                    preset == s.preset,
                    true,
                    t,
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.pick_preset(preset, window, cx)),
                ),
            );
        }
        let time_error = error_of(FormField::Time).filter(|_| !(fresh && s.time.trim().is_empty()));
        // A Once time in the past marks the date and the time.
        let once_past = s.preset == Preset::Once
            && s.once_at().is_some_and(|at| at <= now)
            && r::parse_date(&s.date).is_some();
        let time = div().w(k.time_field).flex_none().child(schedule_input(
            &ki.time,
            time_focused,
            time_error.is_some() || once_past,
            false,
            t,
        ));
        let zone_error = error_of(FormField::TimeZone);
        let zone = self.zone_select(s, zone_error.is_some(), t, window, cx);
        let label = |words: &str| text(words.to_owned(), t.text.ui, c.text_secondary).flex_none();
        let at_row = |time: Div, zone: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(t.space.sm + t.space.xxs)
                .child(label("At"))
                .child(time)
                .child(label("Time zone").ml(t.space.sm))
                .child(div().flex_1().min_w_0().child(zone))
        };
        let mut inputs = div().flex().flex_col().gap(t.space.sm);
        match s.preset {
            Preset::EveryDay | Preset::Weekdays => {
                inputs = inputs.child(at_row(time, zone));
            }
            Preset::Weekly => {
                let mut days = div().flex().gap(t.space.xs + t.space.xxs);
                for (i, name) in DAY_NAMES.iter().enumerate() {
                    days = days.child(
                        choice(format!("day-{name}"), name, s.days[i], false, t)
                            .w(k.day_button)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit_routine_form(cx, |form| {
                                    form.schedule.days[i] = !form.schedule.days[i]
                                })
                            })),
                    );
                }
                inputs = inputs
                    .child(error_under(days, error_of(FormField::Days), t))
                    .child(at_row(time, zone));
            }
            Preset::Interval => {
                let every_error =
                    error_of(FormField::Every).filter(|_| !(fresh && s.every.trim().is_empty()));
                let unit_items: Vec<MenuItem> = IntervalUnit::ALL
                    .iter()
                    .map(|u| {
                        MenuItem::new(
                            u.label(),
                            MenuAction::Knowledge(KnowledgeAction::PickUnit(*u)),
                        )
                        .checked(*u == s.unit)
                    })
                    .collect();
                let unit = focusable(div().id("schedule-unit"), t)
                    .debug_selector(|| "schedule-unit".into())
                    .relative()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w(k.unit_field)
                    .flex_none()
                    .py(t.space.xs + t.space.xxs)
                    .px(t.space.md)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .rounded(t.radius.md)
                    .cursor_pointer()
                    .child(self.triggers.track("schedule-unit"))
                    .child(text(s.unit.label(), t.text.ui, c.text_primary))
                    .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(bounds) = this.triggers.get("schedule-unit") {
                            this.open_menu(OpenMenu::under(bounds, unit_items.clone(), true), cx);
                        }
                    }));
                inputs = inputs.child(error_under(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.sm + t.space.xxs)
                        .child(label("Every"))
                        .child(div().w(k.every_field).flex_none().child(schedule_input(
                            &ki.every,
                            every_focused,
                            every_error.is_some(),
                            false,
                            t,
                        )))
                        .child(unit)
                        .child(
                            text(
                                "1 minute or more. No time zone.",
                                detail(t),
                                c.text_tertiary,
                            )
                            .flex_1(),
                        ),
                    every_error,
                    t,
                ));
            }
            Preset::Once => {
                let date_error = error_of(FormField::Date);
                let date_failed = date_error.is_some();
                let calendar_open = self
                    .knowledge
                    .routine_dialog
                    .as_ref()
                    .and_then(|d| d.calendar);
                let date = div()
                    .relative()
                    .flex()
                    .items_center()
                    .flex_1()
                    .min_w_0()
                    .child(self.triggers.track("schedule-date"))
                    .child(div().flex_1().min_w_0().child(schedule_input(
                        &ki.date,
                        date_focused,
                        date_failed,
                        false,
                        t,
                    )))
                    .child(
                        div()
                            .id("schedule-calendar")
                            .debug_selector(|| "schedule-calendar".into())
                            .absolute()
                            .right(t.space.sm)
                            .top(t.space.sm)
                            .cursor_pointer()
                            .child(icon("calendar", t.icon.dense, c.text_secondary))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let start = this
                                    .routine_form(cx)
                                    .map(|f| {
                                        r::calendar_start(
                                            &f.schedule.date,
                                            consort_client::format::now_ms(),
                                        )
                                    })
                                    .unwrap_or_default();
                                if let Some(dialog) = this.knowledge.routine_dialog.as_mut() {
                                    dialog.calendar = if dialog.calendar.is_some() {
                                        None
                                    } else {
                                        Some(start)
                                    };
                                    dialog.zone_open = false;
                                }
                                cx.notify();
                            })),
                    );
                inputs = inputs
                    .child(error_under(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm + t.space.xxs)
                            .child(label("On"))
                            .child(date)
                            .child(label("at"))
                            .child(time),
                        date_error.or(time_error.clone()),
                        t,
                    ))
                    .child(div().pl(t.space.xl + t.space.xs).child(zone));
                if let Some((year, month)) = calendar_open {
                    inputs = inputs.child(self.calendar(year, month, s, t, cx));
                }
            }
            Preset::Cron => {
                let cron_error =
                    error_of(FormField::Cron).filter(|_| !(fresh && s.cron.trim().is_empty()));
                inputs = inputs.child(error_under(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.sm + t.space.xxs)
                        .child(label("Cron"))
                        .child(div().flex_1().min_w_0().child(schedule_input(
                            &ki.cron,
                            cron_focused,
                            cron_error.is_some(),
                            true,
                            t,
                        )))
                        .child(label("Time zone").ml(t.space.xs))
                        .child(
                            div()
                                .w(k.zone_popover - t.space.page * 2.0)
                                .flex_none()
                                .child(zone),
                        ),
                    cron_error,
                    t,
                ));
            }
        }
        if matches!(
            s.preset,
            Preset::EveryDay | Preset::Weekdays | Preset::Weekly
        ) {
            inputs =
                inputs.child(div().children(time_error.map(|e| error_under(div(), Some(e), t))));
        }
        if zone_error.is_some() && s.preset != Preset::Interval {
            inputs = inputs.child(error_under(div(), zone_error, t));
        }
        let preview = s.preview(now);
        let preview_box = div()
            .debug_selector(|| "schedule-preview".into())
            .flex()
            .flex_col()
            .gap(t.space.xxs)
            .py(t.space.sm + t.space.xxs)
            .px(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .rounded(t.radius.md)
            .child(text(
                preview.headline,
                t.text.ui,
                if preview.valid {
                    c.text_primary
                } else {
                    c.text_secondary
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap(t.space.xs)
                    .child(text(preview.detail, detail(t), c.text_tertiary))
                    .when_some(preview.cron, |d, cron| {
                        d.child(
                            div()
                                .debug_selector(|| "schedule-cron".into())
                                .font_family(t.font_mono.clone())
                                .type_style(detail(t))
                                .text_color(c.text_secondary)
                                .child(cron),
                        )
                    }),
            );
        div()
            .debug_selector(|| "schedule-field".into())
            .flex()
            .flex_col()
            .gap(t.space.sm + t.space.xxs)
            .child(text("Schedule", t.text.label, c.text_secondary))
            .child(presets)
            .child(inputs)
            .child(preview_box)
    }

    /// The time zone select. It opens a popover with a search and the list
    /// of IANA zones. This computer's zone comes first.
    fn zone_select(
        &mut self,
        s: &consort_client::routines::ScheduleForm,
        failed: bool,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let local = r::local_time_zone();
        let open = self
            .knowledge
            .routine_dialog
            .as_ref()
            .is_some_and(|d| d.zone_open);
        let value = s.time_zone.clone();
        let select = focusable(div().id("schedule-zone"), t)
            .debug_selector(|| "schedule-zone".into())
            .relative()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .py(t.space.xs + t.space.xxs)
            .px(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(if failed {
                c.status_failed
            } else if open {
                c.accent_text
            } else {
                c.border_strong
            })
            .rounded(t.radius.md)
            .cursor_pointer()
            .child(self.triggers.track("schedule-zone"))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.xs)
                    .child(line(value.clone(), t.text.ui, c.text_primary).flex_none())
                    .when(value == local, |d| {
                        d.child(line("(your time zone)", t.text.ui, c.text_tertiary))
                    }),
            )
            .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary))
            .on_click(cx.listener(|this, _, window, cx| {
                let open = if let Some(dialog) = this.knowledge.routine_dialog.as_mut() {
                    dialog.zone_open = !dialog.zone_open;
                    dialog.calendar = None;
                    dialog.zone_open
                } else {
                    false
                };
                if open {
                    this.knowledge_inputs.zone_search.update(cx, |i, cx| {
                        i.set_value("", window, cx);
                        i.focus(window, cx);
                    });
                }
                cx.notify();
            }));
        let mut out = div().child(select);
        if open && let Some(bounds) = self.triggers.get("schedule-zone") {
            out = out.child(self.zone_popover(&value, &local, bounds, t, window, cx));
        }
        out.into_any_element()
    }

    fn zone_popover(
        &mut self,
        current: &str,
        local: &str,
        bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let k = t.component.knowledge;
        let query = self
            .knowledge_inputs
            .zone_search
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let mut zones: Vec<&'static str> = r::time_zones()
            .into_iter()
            .filter(|z| query.is_empty() || z.to_lowercase().contains(&query))
            .collect();
        if let Some(ix) = zones.iter().position(|z| *z == local) {
            let first = zones.remove(ix);
            zones.insert(0, first);
        }
        let mut list = div()
            .id("zone-list")
            .flex()
            .flex_col()
            .max_h(k.zone_list)
            .overflow_y_scroll();
        if zones.is_empty() {
            list = list.child(div().p(t.space.sm).child(text(
                "No time zone matches. Try a city, such as Berlin.",
                t.text.ui,
                c.text_secondary,
            )));
        }
        for zone in zones {
            let chosen = zone == current;
            let selector = format!("zone-{zone}");
            list = list.child(
                div()
                    .id(SharedString::from(format!("zone-{zone}")))
                    .debug_selector(move || selector)
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.xs + t.space.xxs)
                    .px(t.space.sm)
                    .rounded(t.radius.md)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.surface_selected))
                    .child(line(zone.to_owned(), t.text.ui, c.text_primary).flex_1())
                    .when(zone == local, |d| {
                        d.child(text("Your time zone", detail(t), c.text_tertiary))
                    })
                    .when(chosen, |d| {
                        d.child(icon("check", t.icon.dense, c.accent_text))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(dialog) = this.knowledge.routine_dialog.as_mut() {
                            dialog.form.schedule.time_zone = zone.to_owned();
                            dialog.zone_open = false;
                        }
                        cx.notify();
                    })),
            );
        }
        let content = div()
            .id("zone-popover")
            .debug_selector(|| "zone-popover".into())
            .occlude()
            .flex()
            .flex_col()
            .gap(t.space.xs)
            .w(bounds.size.width.max(k.zone_popover))
            .p(t.space.xs)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .rounded(t.radius.lg)
            .shadow(t.shadow_float())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.xs + t.space.xxs)
                    .px(t.space.sm)
                    .border_b(t.border_width)
                    .border_color(c.border_subtle)
                    .child(icon("search", t.icon.dense, c.text_tertiary))
                    .child(div().flex_1().child(crate::ui::parts::input(
                        &self.knowledge_inputs.zone_search,
                        t.text.ui,
                    ))),
            )
            .child(list)
            .into_any_element();
        floating(
            MenuAnchor::Trigger(bounds),
            content,
            t,
            window,
            cx,
            |this: &mut ConsortApp, _, cx| {
                if let Some(dialog) = this.knowledge.routine_dialog.as_mut() {
                    dialog.zone_open = false;
                }
                cx.notify();
            },
        )
    }

    /// The calendar of the date field: one month, Monday first, with the
    /// days before today dimmed.
    fn calendar(
        &mut self,
        year: i32,
        month: u32,
        s: &consort_client::routines::ScheduleForm,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let k = t.component.knowledge;
        let now = consort_client::format::now_ms();
        let Some(cal) = r::calendar_month(year, month, &s.date, now) else {
            return div().into_any_element();
        };
        let nav = |id: &'static str, name: &'static str, to: (i32, u32), cx: &mut Context<Self>| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .size(t.space.xl)
                .flex()
                .items_center()
                .justify_center()
                .rounded(t.radius.md)
                .cursor_pointer()
                .hover(|s| s.bg(c.surface_selected))
                .child(icon(name, t.icon.dense, c.text_secondary))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(dialog) = this.knowledge.routine_dialog.as_mut() {
                        dialog.calendar = Some(to);
                    }
                    cx.notify();
                }))
        };
        let mut grid = div().flex().flex_col().gap(t.space.xxs);
        let mut header = div().flex().gap(t.space.xxs);
        for name in DAY_NAMES {
            header = header.child(div().w(k.calendar_cell).flex().justify_center().child(text(
                &name[..2],
                detail(t),
                c.text_tertiary,
            )));
        }
        grid = grid.child(header);
        for days in cal.weeks {
            let mut week = div().flex().gap(t.space.xxs);
            for day in days {
                let selector = format!("calendar-{}", day.key);
                let value = day.text.clone();
                let (past, chosen, in_month) = (day.past, day.chosen, day.in_month);
                week = week.child(
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector)
                        .size(k.calendar_cell)
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(t.radius.md)
                        .type_style(t.text.ui)
                        .when(chosen, |d| d.bg(c.accent_fill).text_color(c.accent_on_fill))
                        .when(!chosen, |d| {
                            d.text_color(if !in_month || past {
                                c.text_tertiary
                            } else {
                                c.text_primary
                            })
                        })
                        .when(!past && !chosen, |d| {
                            d.cursor_pointer().hover(|s| s.bg(c.surface_selected))
                        })
                        .child(day.day.to_string())
                        .when(!past, |d| {
                            d.on_click(cx.listener(move |this, _, window, cx| {
                                let value = value.clone();
                                this.knowledge_inputs
                                    .date
                                    .update(cx, |i, cx| i.set_value(value, window, cx));
                                if let Some(dialog) = this.knowledge.routine_dialog.as_mut() {
                                    dialog.calendar = None;
                                }
                                cx.notify();
                            }))
                        }),
                );
            }
            grid = grid.child(week);
        }
        div()
            .debug_selector(|| "schedule-calendar-popover".into())
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .p(t.space.sm)
            .ml(t.space.xl + t.space.xs)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .rounded(t.radius.lg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.xs)
                    .child(nav("calendar-prev", "chevron-left", cal.prev, cx))
                    .child(div().flex_1().flex().justify_center().child(text(
                        cal.title,
                        t.text.ui_strong,
                        c.text_primary,
                    )))
                    .child(nav("calendar-next", "chevron-right", cal.next, cx)),
            )
            .child(grid)
            .into_any_element()
    }
}
