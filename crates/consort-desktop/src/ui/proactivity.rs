//! The assistant on its own, drawn: Settings > Proactivity (Paper `FYE-0`),
//! the level select that the channel settings dialog reuses (`GHK-0`), the
//! marks, the offer card, the morning brief and the hover actions in the
//! timeline (`G2C-0`), the quick-reply chips and the writing bubble
//! (`F6N-0`, `FDX-0`, `FJC-0`), and the Follow-ups section of the Routines
//! tab (`G8V-0`).

use consort_client::proactivity::{
    BriefView, ChannelLevelView, LogRowView, ProactiveMarkView, ResultTone, SnoozeOption,
    SuggestionView, level_helper,
};
use consort_client::proactivity::{LevelKind as ChannelKind, ProactiveRating, ProactivityLevel};
use consort_client::routines::RoutineView;
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, MouseButton, ParentElement,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled, Window, div, img,
    prelude::FluentBuilder as _, relative,
};

use crate::app::ConsortApp;
use crate::proactivity::{
    LEVEL_SELECTS, LOG_CHANNEL_SELECT, LOG_KIND_SELECT, LOG_KINDS, QUIET_END_SELECT,
    QUIET_START_SELECT, ZONE_SELECT, level_option, quiet_times, zone_options,
};
use crate::ui::admin_parts::{column, header, page, rows, section_title, select};
use crate::ui::mark::still_mark;
use crate::ui::parts::{ButtonKind, button, detail, icon, text};
use crate::ui::settings::{segmented, switch};

/// The label of a level select: the level, and "Default for this channel"
/// when the channel has none of its own.
fn level_label(level: Option<ProactivityLevel>, effective: Option<ProactivityLevel>) -> String {
    match (level, effective) {
        (Some(level), _) => level.label().to_owned(),
        (None, Some(effective)) => effective.label().to_owned(),
        (None, None) => "Default".to_owned(),
    }
}

/// The options of a level select: the four levels, and the default for the
/// channel, which names the level it gives.
pub fn level_options(
    channel_id: &str,
    level: Option<ProactivityLevel>,
    kind: ChannelKind,
) -> Vec<(String, String, bool)> {
    let default = ProactivityLevel::default_for(kind);
    let mut options: Vec<(String, String, bool)> = ProactivityLevel::ALL
        .iter()
        .map(|l| {
            (
                l.label().to_owned(),
                level_option(channel_id, Some(*l)),
                level == Some(*l),
            )
        })
        .collect();
    options.push((
        format!("Default for this channel ({})", default.label()),
        level_option(channel_id, None),
        level.is_none(),
    ));
    options
}

/// The level select of one channel, 112 px wide. The channel settings
/// dialog uses it with [`crate::proactivity::CHANNEL_SETTINGS_LEVEL_SELECT`]
/// and a wider `width`; a pick comes back as `MenuAction::Select(id,
/// "<channel-id>:<level>")`, which [`crate::proactivity::parse_level_option`]
/// reads.
#[allow(clippy::too_many_arguments)]
pub fn level_select(
    id: &'static str,
    channel_id: &str,
    level: Option<ProactivityLevel>,
    kind: ChannelKind,
    width: gpui_kit::Pixels,
    app: &ConsortApp,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> Stateful<Div> {
    let effective = Some(level.unwrap_or_else(|| ProactivityLevel::default_for(kind)));
    select(
        id,
        level_label(level, effective),
        width,
        level_options(channel_id, level, kind),
        app,
        t,
        cx,
    )
}

/// The helper text under the level select of the channel settings dialog.
/// The dialog comes with the phase 4 merge; `docs/notes/phase4b.md` says
/// how it uses this.
#[allow(dead_code)]
pub fn level_helper_text(level: Option<ProactivityLevel>, kind: ChannelKind, t: &Tokens) -> Div {
    let effective = level.unwrap_or_else(|| ProactivityLevel::default_for(kind));
    text(
        level_helper(effective, kind),
        detail(t),
        t.colors.text_tertiary,
    )
}

/// A time box of the quiet hours: the time in mono, 72 px wide, with no
/// chevron. A click opens the times every 30 minutes under it.
fn time_box(
    id: &'static str,
    value: String,
    options: Vec<(String, String, bool)>,
    app: &ConsortApp,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> Stateful<Div> {
    use crate::ui::menu::{MenuAction, MenuItem, OpenMenu};
    let c = &t.colors;
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .w(t.component.proactivity.time_box)
        .py(t.space.xs)
        .px(t.space.sm + t.space.xxs)
        .rounded(t.radius.md)
        .border(t.border_width)
        .border_color(c.border_strong)
        .bg(c.surface_raised)
        .cursor_pointer()
        .hover(|s| s.bg(c.hover_raised))
        .child(app.triggers.track(id))
        .child(
            text(value, t.text.ui, c.text_primary)
                .font_family(t.font_mono.clone())
                .whitespace_nowrap(),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(bounds) = this.triggers.get(id) {
                let items = options
                    .iter()
                    .map(|(label, option, checked)| {
                        MenuItem::new(label.clone(), MenuAction::Select(id, option.clone()))
                            .checked(*checked)
                    })
                    .collect();
                this.open_menu(OpenMenu::under(bounds, items, false), cx);
            }
        }))
}

/// A row of a bordered group: a title, a hint under it, and a control at
/// the right.
fn setting_row(title: &str, hint: impl Into<SharedString>, last: bool, t: &Tokens) -> Div {
    let c = &t.colors;
    crate::ui::admin_parts::row(last, t).child(
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(text(title.to_owned(), t.text.ui_strong, c.text_primary))
            .child(text(hint, detail(t), c.text_tertiary)),
    )
}

/// A small label above a field.
fn field(label: &str, control: impl IntoElement, t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(t.space.xs)
        .child(text(
            label.to_owned(),
            t.text.label,
            t.colors.text_secondary,
        ))
        .child(control)
}

fn tone_color(tone: ResultTone, t: &Tokens) -> gpui_kit::Hsla {
    let c = &t.colors;
    match tone {
        ResultTone::Notified => c.accent_text,
        ResultTone::Failed => c.status_failed,
        ResultTone::Posted => c.text_primary,
        ResultTone::Quiet => c.text_tertiary,
    }
}

impl ConsortApp {
    /// Settings > Proactivity: when the assistant may start things, each
    /// channel, today, and the log.
    pub fn render_proactivity(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let p = t.component.proactivity;
        let view = self.state.proactivity.clone();
        let line = text(
            "What the assistant may start on its own, and when it stays quiet. Silence is the default.",
            t.text.ui,
            c.text_secondary,
        );
        if !view.available {
            return page("proactivity-page", t)
                .child(
                    column(t)
                        .child(header("Proactivity", line, None, t))
                        .child(text(
                            "This hub is older than the app and has no proactivity yet. Update the hub, then open this page again.",
                            t.text.ui,
                            c.text_secondary,
                        )),
                )
                .into_any_element();
        }

        // When the assistant may start things.
        let paused = view.paused;
        let pause = setting_row(
            "Pause all",
            "The assistant starts nothing on its own in any channel. It still answers you, and its tasks keep running.",
            false,
            t,
        )
        .child(
            switch("proactivity-pause", paused, t)
                .debug_selector(|| "proactivity-pause".into())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_proactivity_paused(!paused, cx)
                })),
        );
        let time_options = |current: &str| -> Vec<(String, String, bool)> {
            quiet_times(current)
                .into_iter()
                .map(|time| (time.clone(), time.clone(), time == current))
                .collect()
        };
        let quiet = setting_row(
            "Quiet hours",
            "In these hours the assistant starts nothing and sends no notification. The same hours apply to every channel.",
            false,
            t,
        )
        .child(
            div()
                .flex()
                .flex_none()
                .items_end()
                .gap(t.space.sm)
                .child(field(
                    "From",
                    time_box(
                        QUIET_START_SELECT,
                        view.quiet_start.clone(),
                        time_options(&view.quiet_start),
                        self,
                        t,
                        cx,
                    ),
                    t,
                ))
                .child(field(
                    "To",
                    time_box(
                        QUIET_END_SELECT,
                        view.quiet_end.clone(),
                        time_options(&view.quiet_end),
                        self,
                        t,
                        cx,
                    ),
                    t,
                ))
                .child(field(
                    "Time zone",
                    select(
                        ZONE_SELECT,
                        view.time_zone.clone(),
                        p.zone_select,
                        zone_options(&view.time_zone)
                            .into_iter()
                            .map(|z| (z.clone(), z.clone(), z == view.time_zone))
                            .collect(),
                        self,
                        t,
                        cx,
                    ),
                    t,
                )),
        );
        let options: Vec<(SnoozeOption, &str)> =
            SnoozeOption::ALL.iter().map(|o| (*o, o.label())).collect();
        let snooze = setting_row("Snooze", view.snooze_line.clone(), false, t).child(
            div().flex_none().child(segmented(
                "proactivity-snooze",
                &options,
                view.snooze,
                t,
                cx,
                |_| t.text.ui,
                |this, option, _, cx| this.snooze_proactivity(option, cx),
            )),
        );
        let follow_ups = setting_row(
            "Follow-ups that the assistant set",
            view.follow_ups_line.clone(),
            true,
            t,
        )
        .when(!view.follow_ups.is_empty(), |d| {
            d.child(
                button(
                    "proactivity-cancel-follow-ups",
                    "Cancel all follow-ups",
                    ButtonKind::Secondary,
                    t,
                )
                .on_click(cx.listener(|this, _, _, cx| this.cancel_all_follow_ups(None, cx))),
            )
        });
        let when = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section_title("When the assistant may start things", t))
            .child(
                rows(t)
                    .child(pause)
                    .child(quiet)
                    .child(snooze)
                    .child(follow_ups),
            );

        // Each channel.
        let mut channels = rows(t);
        let count = view.channels.len();
        for (i, channel) in view.channels.iter().enumerate() {
            channels = channels.child(self.render_level_row(channel, i, i + 1 == count, t, cx));
        }
        let each = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xxs)
                    .child(section_title("Each channel", t))
                    .child(text(
                        "The assistant can set a quieter level when you ask it to. Only you can set a louder level.",
                        detail(t),
                        c.text_tertiary,
                    )),
            )
            .child(channels);

        // Today.
        let share = if view.notify_cap == 0 {
            0.0
        } else {
            (view.notified_today as f32 / view.notify_cap as f32).min(1.0)
        };
        let number = |n: u32| {
            div()
                .type_style(t.text.heading)
                .font_weight(t.text.title.weight)
                .text_color(c.text_primary)
                .child(n.to_string())
        };
        let counts = div()
            .flex()
            .border_b(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(t.space.xxs)
                    .py(t.space.md)
                    .px(t.space.lg)
                    .border_r(t.border_width)
                    .border_color(c.border_subtle)
                    .child(text("Proactive turns", detail(t), c.text_tertiary))
                    .child(number(view.turns_today))
                    .child(text(view.turns_line.clone(), detail(t), c.text_tertiary)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(t.space.xxs)
                    .py(t.space.md)
                    .px(t.space.lg)
                    .child(text("Notifications", detail(t), c.text_tertiary))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(t.space.xs + t.space.xxs)
                            .child(number(view.notified_today))
                            .child(text(
                                format!("of {} a day", view.notify_cap),
                                t.text.ui,
                                c.text_secondary,
                            )),
                    )
                    .child(
                        div()
                            .my(t.space.xs)
                            .h(p.count_bar)
                            .w_full()
                            .rounded(t.radius.full)
                            .bg(c.surface_selected)
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(share))
                                    .rounded(t.radius.full)
                                    .bg(c.status_working),
                            ),
                    )
                    .child(text(view.notify_line.clone(), detail(t), c.text_tertiary)),
            );
        let tutti = self.state.tutti().map(|ch| ch.id.clone());
        let watch = setting_row(
            "What to watch",
            "Every check-in in Tutti reads the check-in note: what to watch and what to skip. You can also say it in chat, for example \"skip receipts\".",
            false,
            t,
        )
        .child(
            div()
                .id("proactivity-edit-note")
                .flex()
                .flex_none()
                .items_center()
                .gap(t.space.xs)
                .cursor_pointer()
                .child(icon("edit", t.icon.dense, c.text_primary))
                .child(text("Edit check-in.md", t.text.ui, c.text_primary).underline())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_check_in_note(tutti.clone(), window, cx)
                })),
        );
        let mut today_rows = rows(t).child(counts).child(watch);
        if let Some((routine_id, brief_line)) = view.brief.clone() {
            today_rows = today_rows.child(
                setting_row("Morning brief", brief_line, true, t).child(
                    div()
                        .id("proactivity-open-brief")
                        .flex_none()
                        .cursor_pointer()
                        .child(text("Open routine", t.text.ui, c.text_primary).underline())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let channel = this
                                .state
                                .routines
                                .iter()
                                .find(|r| r.id == routine_id)
                                .map(|r| r.channel_id.clone());
                            if let Some(channel) = channel {
                                this.open_channel(channel, window, cx);
                                this.open_routine(&routine_id, window, cx);
                            }
                        })),
                ),
            );
        }
        let today = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section_title("Today", t))
            .child(today_rows);

        page("proactivity-page", t)
            .child(
                column(t)
                    .child(header("Proactivity", line, None, t))
                    .when_some(view.error.clone(), |d, error| {
                        d.child(crate::ui::parts::error_box("proactivity-error", error, t))
                    })
                    .child(when)
                    .child(each)
                    .child(today)
                    .child(self.render_proactivity_log(t, cx)),
            )
            .into_any_element()
    }

    /// One channel of "Each channel": its icon, name, meaning, and select.
    fn render_level_row(
        &mut self,
        channel: &ChannelLevelView,
        index: usize,
        last: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let p = t.component.proactivity;
        let glyph = if channel.tutti {
            still_mark(t.icon.size, t)
        } else {
            icon(&channel.icon, t.icon.size, c.text_secondary).into_any_element()
        };
        let kind = if channel.tutti {
            ChannelKind::Tutti
        } else {
            ChannelKind::Standard
        };
        let id = LEVEL_SELECTS[index % LEVEL_SELECTS.len()];
        crate::ui::admin_parts::row(last, t)
            .child(div().flex_none().w(t.icon.size).child(glyph))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(text(channel.name.clone(), t.text.ui_strong, c.text_primary))
                    .child(text(channel.meaning.clone(), detail(t), c.text_tertiary)),
            )
            .child(level_select(
                id,
                &channel.channel_id,
                channel.level,
                kind,
                p.level_select,
                self,
                t,
                cx,
            ))
    }

    /// Activity: the log with its two filters, grouped by day.
    fn render_proactivity_log(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let p = t.component.proactivity;
        let ui = self.proactivity.clone();
        let groups = if self.state.proactivity.log.is_empty() {
            Vec::new()
        } else if ui.log_channel.is_none() && ui.log_kind.is_none() {
            self.state.proactivity.log.clone()
        } else {
            // Filter the rows that the view has.
            self.state
                .proactivity
                .log
                .iter()
                .map(|g| consort_client::proactivity::LogGroupView {
                    label: g.label.clone(),
                    rows: g
                        .rows
                        .iter()
                        .filter(|r| {
                            ui.log_channel
                                .as_deref()
                                .is_none_or(|ch| ch == r.channel_id)
                                && ui.log_kind.is_none_or(|k| r.kind == Some(k))
                        })
                        .cloned()
                        .collect(),
                })
                .filter(|g| !g.rows.is_empty())
                .collect()
        };
        let mut channel_options = vec![(
            "All channels".to_owned(),
            String::new(),
            ui.log_channel.is_none(),
        )];
        for channel in &self.state.proactivity.channels {
            channel_options.push((
                channel.name.clone(),
                channel.channel_id.clone(),
                ui.log_channel.as_deref() == Some(channel.channel_id.as_str()),
            ));
        }
        let channel_label = ui
            .log_channel
            .as_ref()
            .and_then(|id| {
                self.state
                    .proactivity
                    .channels
                    .iter()
                    .find(|ch| &ch.channel_id == id)
            })
            .map_or("All channels".to_owned(), |ch| ch.name.clone());
        let mut kind_options = vec![("All kinds".to_owned(), String::new(), ui.log_kind.is_none())];
        for kind in LOG_KINDS {
            kind_options.push((
                kind.label().to_owned(),
                kind.as_str().to_owned(),
                ui.log_kind == Some(kind),
            ));
        }
        let kind_label = ui
            .log_kind
            .map_or("All kinds".to_owned(), |k| k.label().to_owned());
        let filters = div()
            .flex()
            .flex_none()
            .gap(t.space.sm)
            .child(select(
                LOG_CHANNEL_SELECT,
                channel_label,
                p.filter_select,
                channel_options,
                self,
                t,
                cx,
            ))
            .child(select(
                LOG_KIND_SELECT,
                kind_label,
                p.filter_select,
                kind_options,
                self,
                t,
                cx,
            ));
        let mut log = rows(t);
        if groups.is_empty() {
            log = log.child(div().py(t.space.lg).px(t.space.lg).child(text(
                "Nothing yet. Every time the assistant wakes on its own, it shows here, also when it says nothing.",
                t.text.ui,
                c.text_secondary,
            )));
        }
        for group in &groups {
            log = log.child(
                div()
                    .py(t.space.sm)
                    .px(t.space.lg)
                    .border_b(t.border_width)
                    .border_color(c.border_subtle)
                    .child(text(group.label.clone(), detail(t), c.text_tertiary)),
            );
            for row in &group.rows {
                log = log.child(self.render_log_row(row, t, cx));
            }
        }
        div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(t.space.lg)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(t.space.xxs)
                            .child(section_title("Activity", t))
                            .child(text(
                                "Every time the assistant woke on its own in the last 30 days, also when it said nothing.",
                                detail(t),
                                c.text_tertiary,
                            )),
                    )
                    .child(filters),
            )
            .child(log)
    }

    fn render_log_row(&mut self, row: &LogRowView, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let p = t.component.proactivity;
        let message = row.message.clone();
        let channel = row.channel_id.clone();
        div()
            .flex()
            .items_start()
            .gap(t.space.md)
            .py(p.log_pad_y)
            .px(t.space.lg)
            .border_b(t.border_width)
            .border_color(c.border_subtle)
            .child(
                text(row.time_label.clone(), t.text.label, c.text_tertiary)
                    .line_height(t.text.ui.line)
                    .w(p.log_time)
                    .flex_none(),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(p.log_place)
                    .child(text(
                        row.channel_name.clone(),
                        t.text.ui_strong,
                        c.text_primary,
                    ))
                    .child(text(row.kind_label.clone(), detail(t), c.text_tertiary)),
            )
            .child(
                text(
                    row.result.clone(),
                    if row.tone == ResultTone::Quiet {
                        t.text.ui
                    } else {
                        t.text.ui_strong
                    },
                    tone_color(row.tone, t),
                )
                .w(p.log_result)
                .flex_none(),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .flex_1()
                    .min_w_0()
                    .gap_x(t.space.xs)
                    .child(text(row.detail.clone(), t.text.ui, c.text_secondary).max_w_full())
                    .when_some(message, |d, (message_id, root)| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("log-open-{}", row.id)))
                                .cursor_pointer()
                                .child(text("Open message", t.text.ui, c.text_primary).underline())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_logged_message(
                                        channel.clone(),
                                        message_id.clone(),
                                        root.clone(),
                                        window,
                                        cx,
                                    )
                                })),
                        )
                    }),
            )
    }

    /// "Open message" opens the channel at the message, or the thread when
    /// the message is a reply in a task thread.
    pub fn open_logged_message(
        &mut self,
        channel_id: String,
        _message_id: String,
        root: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_channel(channel_id, window, cx);
        if let Some(root) = root {
            let task = self
                .state
                .tasks
                .iter()
                .find(|task| task.card_message_id == root)
                .map(|task| task.id.clone());
            if let Some(task) = task {
                self.open_thread(task, window, cx);
            }
        }
    }

    /// "Edit check-in.md": Tutti's note in global memory.
    pub fn open_check_in_note(
        &mut self,
        tutti: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tutti) = tutti {
            self.open_channel(tutti, window, cx);
            self.open_channel_tab(crate::app::ChannelTab::Memory, cx);
            self.open_note_in_panel(
                consort_client::memory::GLOBAL,
                consort_client::proactivity::CHECK_IN_NOTE_PATH,
                window,
                cx,
            );
        }
    }

    // The timeline ---------------------------------------------------------

    /// The caption above the first bubble of a proactive message: the icon
    /// and the kind, and for a follow-up its reason under it.
    pub fn render_mark_caption(&self, mark: &ProactiveMarkView, t: &Tokens) -> Div {
        let c = &t.colors;
        let p = t.component.proactivity;
        div()
            .flex()
            .flex_col()
            .gap(t.border_width)
            .pb(t.space.xxs)
            .pl(p.mark_indent)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(p.mark_gap)
                    .child(icon(&mark.icon, p.mark_icon, c.text_tertiary))
                    .child(text(mark.label.clone(), t.text.label, c.text_tertiary)),
            )
            .when_some(mark.reason.clone(), |d, reason| {
                d.child(
                    div()
                        .pl(p.reason_indent)
                        .child(text(reason, detail(t), c.text_tertiary)),
                )
            })
    }

    /// Useful and Too noisy under a proactive message: on hover, and after
    /// the user rated it.
    pub fn render_feedback(
        &self,
        message_id: &str,
        rating: Option<ProactiveRating>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let p = t.component.proactivity;
        let action = |label: &'static str, value: ProactiveRating| {
            let chosen = rating == Some(value);
            let id = message_id.to_owned();
            div()
                .id(SharedString::from(format!(
                    "feedback-{message_id}-{}",
                    value.as_str()
                )))
                .debug_selector(move || format!("feedback-{}", value.as_str()))
                .flex()
                .items_center()
                .py(p.feedback_pad_y)
                .px(p.feedback_pad_x)
                .rounded(t.radius.md)
                .cursor_pointer()
                .when(chosen, |d| d.bg(c.surface_selected))
                .hover(|s| s.bg(c.surface_selected))
                .child(text(
                    label,
                    t.text.label,
                    if chosen {
                        c.text_primary
                    } else {
                        c.text_secondary
                    },
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.rate_message(id.clone(), value, rating, cx)
                }))
        };
        div()
            .flex()
            .items_center()
            .gap(t.space.xs)
            .pl(t.space.xxs)
            .child(action("Useful", ProactiveRating::Useful))
            .child(action("Too noisy", ProactiveRating::TooNoisy))
    }

    /// The quick-reply chips under the newest assistant message. "I will
    /// type it" has a dashed border and only focuses the composer.
    pub fn render_chips(
        &self,
        suggestions: &[SuggestionView],
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let p = t.component.proactivity;
        let mut row = div().flex().flex_wrap().gap(p.chip_gap).pt(p.chip_gap);
        for (i, suggestion) in suggestions.iter().enumerate() {
            let label = suggestion.label.clone();
            let type_it = suggestion.type_it;
            let selector = format!("chip-{i}");
            row = row.child(
                div()
                    .id(SharedString::from(format!("chip-{i}-{}", suggestion.label)))
                    .debug_selector(move || selector.clone())
                    .py(p.chip_pad_y)
                    .px(p.chip_pad_x)
                    .rounded(t.radius.full)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .cursor_pointer()
                    .when(type_it, |d| {
                        d.border_dashed().hover(|s| s.bg(c.hover_canvas))
                    })
                    .when(!type_it, |d| {
                        d.bg(c.surface_raised).hover(|s| s.bg(c.hover_raised))
                    })
                    .child(text(
                        suggestion.label.clone(),
                        t.text.ui_strong,
                        if type_it {
                            c.text_secondary
                        } else {
                            c.text_primary
                        },
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.use_suggestion(label.clone(), type_it, window, cx)
                    })),
            );
        }
        row
    }

    /// The writing bubble: three dabs of the mark at 10 px in the assistant
    /// lane. They shimmer on the shared mark clock and stand still with
    /// Reduce motion.
    pub fn render_writing_bubble(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let p = t.component.proactivity;
        let chat = t.component.chat;
        let version = crate::ui::motion::shimmer_version(true, cx);
        let dab = |name: &str, offset: gpui_kit::Pixels| {
            div()
                .flex_none()
                .mt(offset)
                .child(img(SharedString::from(dab_asset(name, version))).size(p.dab))
        };
        div()
            .id("writing-bubble")
            .debug_selector(|| "writing-bubble".into())
            .flex()
            .items_center()
            .gap(p.dab_gap)
            .py(p.writing_pad_y)
            .px(p.writing_pad_x)
            .rounded(chat.bubble_radius)
            .rounded_bl(chat.bubble_tail)
            .bg(c.surface_card)
            .child(dab("gold", p.dab_drop))
            .child(dab("verdigris", -p.dab_rise))
            .child(dab("lapis", t.border_width))
            .into_any_element()
    }

    /// The offer card: the question, what it will and will not do, and Do
    /// it, Not now and Never about this.
    pub fn render_offer_card(
        &mut self,
        decision: &consort_client::DecisionView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let p = t.component.proactivity;
        let chat = t.component.chat;
        let pending = matches!(
            decision.state,
            consort_client::RequestState::Pending | consort_client::RequestState::Sending
        );
        let answer = |i: u32, label: &str, kind: ButtonKind| {
            let id = decision.id.clone();
            let chosen = decision.chosen == Some(i);
            button(
                SharedString::from(format!("offer-{}-{i}", decision.id)),
                label.to_owned(),
                kind,
                t,
            )
            .when(!pending && !chosen, |d| d.opacity(t.opacity.answered_dim))
            .when(pending, |d| {
                d.on_click(cx.listener(move |this, _, window, cx| {
                    this.answer_decision(id.clone(), Some(i), None, window, cx)
                }))
            })
        };
        let label = |i: usize, fallback: &str| {
            decision
                .options
                .get(i)
                .map_or(fallback.to_owned(), |o| o.label.clone())
        };
        div()
            .id(SharedString::from(format!("offer-{}", decision.id)))
            .debug_selector(|| "offer-card".into())
            .flex()
            .flex_col()
            .gap(t.space.md)
            .w_full()
            .max_w(t.layout.bubble_max_width)
            .pt(p.offer_pad_top)
            .px(p.offer_pad)
            .pb(p.offer_pad)
            .rounded(chat.bubble_radius)
            .rounded_bl(chat.bubble_tail)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xxs)
                    .child(
                        div()
                            .type_style(t.text.body)
                            .font_weight(t.text.title.weight)
                            .text_color(c.text_primary)
                            .child(crate::ui::parts::selectable(
                                SharedString::from(format!("offer-q-{}", decision.id)),
                                decision.question.clone(),
                                t.text.body,
                                c.text_primary,
                            )),
                    )
                    .when_some(decision.context.clone(), |d, context| {
                        d.child(crate::ui::parts::selectable(
                            SharedString::from(format!("offer-c-{}", decision.id)),
                            context,
                            t.text.ui,
                            c.text_secondary,
                        ))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(answer(0, &label(0, "Do it"), ButtonKind::Primary))
                    .child(answer(1, &label(1, "Not now"), ButtonKind::Secondary))
                    .child(answer(2, &label(2, "Never about this"), ButtonKind::Quiet)),
            )
            .into_any_element()
    }

    /// The morning brief in its bubble: the first line, then the three
    /// groups with their numbered items. The name of an item opens its task
    /// or its message.
    pub fn render_brief(
        &mut self,
        key: &str,
        brief: &BriefView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let p = t.component.proactivity;
        let mut body = div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .child(crate::ui::parts::selectable(
                SharedString::from(format!("{key}-brief-intro")),
                brief.intro.clone(),
                t.text.body,
                c.text_primary,
            ));
        for (g, group) in brief.groups.iter().enumerate() {
            let mut list = div().flex().flex_col().gap(p.brief_group_gap).child(text(
                group.label.clone(),
                t.text.label,
                if group.your_turn {
                    c.accent_text
                } else {
                    c.text_tertiary
                },
            ));
            for (i, line) in group.lines.iter().enumerate() {
                let target = line.target.clone();
                list = list.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(t.space.sm)
                        .child(
                            text(format!("{}.", line.number), t.text.body, c.text_tertiary)
                                .w(t.component.chat.brief_number)
                                .flex_none(),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .flex_1()
                                .min_w_0()
                                .gap_x(t.space.xs)
                                .when(!line.name.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .id(SharedString::from(format!("{key}-brief-{g}-{i}")))
                                            .cursor_pointer()
                                            .type_style(t.text.body)
                                            .font_weight(t.text.ui_strong.weight)
                                            .text_color(c.text_primary)
                                            .underline()
                                            .child(line.name.clone())
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.open_brief_target(&target, window, cx)
                                            })),
                                    )
                                })
                                .child(
                                    text(line.rest.clone(), t.text.body, c.text_secondary)
                                        .max_w_full(),
                                ),
                        ),
                );
            }
            body = body.child(list);
        }
        body
    }

    /// Opens the target of a brief item: a task thread, the message's
    /// channel, or a web address after the link check.
    pub fn open_brief_target(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(task) = target.strip_prefix("task:") {
            if let Some(channel) = self.state.task(task).map(|t| t.channel_id.clone()) {
                self.open_channel(channel, window, cx);
            }
            self.open_thread(task.to_owned(), window, cx);
        } else if target.strip_prefix("message:").is_some() {
            // The message is in the open channel or in Tutti.
            cx.notify();
        } else if !target.is_empty() {
            cx.open_url(target);
        }
    }

    // The Routines tab -----------------------------------------------------

    /// The Follow-ups section of the Routines tab: the follow-ups that the
    /// assistant set, each with Cancel on hover, and Cancel all.
    pub fn render_follow_ups(
        &mut self,
        follow_ups: &[RoutineView],
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let k = t.component.knowledge;
        let now = consort_client::format::now_ms();
        let channel = self.channel_id.clone();
        let mut list = div().flex().flex_col().gap(t.space.xxs);
        for routine in follow_ups {
            let id = routine.id.clone();
            let hover_id = id.clone();
            let hovered = self.proactivity.hovered_follow_up.as_deref() == Some(id.as_str());
            let at = routine.next_run_at.unwrap_or_default();
            let when = consort_client::proactivity::follow_up_when(at, routine.created_at, now);
            let selector = format!("follow-up-{id}");
            list = list.child(
                div()
                    .id(SharedString::from(format!("follow-up-{id}")))
                    .debug_selector(move || selector.clone())
                    .flex()
                    .items_start()
                    .gap(t.space.md)
                    .py(k.row_pad_y)
                    .px(t.space.sm)
                    .rounded(t.radius.md)
                    .when(hovered, |d| d.bg(c.surface_card))
                    .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                        if *on {
                            this.proactivity.hovered_follow_up = Some(hover_id.clone());
                        } else if this.proactivity.hovered_follow_up.as_deref()
                            == Some(hover_id.as_str())
                        {
                            this.proactivity.hovered_follow_up = None;
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(t.text.ui.line)
                            .child(icon("clock", t.icon.size, c.text_secondary)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(t.space.xxs)
                            .child(text(
                                routine.prompt.clone(),
                                t.text.ui_strong,
                                c.text_primary,
                            ))
                            .child(text(when, detail(t), c.text_tertiary)),
                    )
                    .when(hovered, |d| {
                        d.child(
                            button(
                                SharedString::from(format!("follow-up-cancel-{id}")),
                                "Cancel",
                                ButtonKind::Secondary,
                                t,
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.cancel_follow_up(id.clone(), cx),
                            )),
                        )
                    }),
            );
        }
        let count = follow_ups.len();
        div()
            .flex()
            .flex_col()
            .gap(t.space.xs)
            .pt(t.space.lg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .px(t.space.sm)
                    .child(text("Follow-ups", t.text.ui_strong, c.text_primary))
                    .child(
                        text(
                            if count == 1 {
                                "The assistant set 1 for itself".to_owned()
                            } else {
                                format!("The assistant set {count} for itself")
                            },
                            detail(t),
                            c.text_tertiary,
                        )
                        .flex_1(),
                    )
                    .child(
                        button("follow-ups-cancel-all", "Cancel all", ButtonKind::Quiet, t)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cancel_all_follow_ups(Some(channel.clone()), cx)
                            })),
                    ),
            )
            .child(list)
    }
}

/// The asset of one dab of the writing bubble, in a shimmer version.
pub fn dab_asset(name: &str, version: usize) -> String {
    format!("glyphs/dab-{name}-{version}.svg")
}
