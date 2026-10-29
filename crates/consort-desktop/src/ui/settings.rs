//! Settings, in the window: Appearance and Workers, and a line for the
//! sections that come later.

use consort_client::{ApprovalMode, AuthState, WorkerView};
use consort_theme::{Appearance, Mode, TextSize, ThemeLibrary, Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::{
    app::{ConsortApp, Dialog, Page, SettingsSection},
    ui::sizes,
    ui::window_controls::DragArea as _,
    ui::{
        menu::{MenuAction, MenuItem, OpenMenu},
        parts::{ButtonKind, button, detail, glyph, icon, icon_button, line, text, worker_dot},
        sidebar::{row, title_bar_height},
    },
};

/// A settings section: its page, its name and its icon.
type SectionEntry = (SettingsSection, &'static str, &'static str);

const SECTIONS: [(&str, &[SectionEntry]); 3] = [
    (
        "App",
        &[
            (SettingsSection::Appearance, "Appearance", "contrast"),
            (SettingsSection::Notifications, "Notifications", "bell"),
        ],
    ),
    (
        "Machines",
        &[
            (SettingsSection::Workers, "Workers", "server"),
            (SettingsSection::Clients, "Clients", "smartphone"),
            (SettingsSection::Hub, "Hub", "cloud"),
        ],
    ),
    (
        "Assistant",
        &[
            (SettingsSection::Memory, "Memory", "book-open"),
            (SettingsSection::Routines, "Routines", "clock"),
        ],
    ),
];

/// A segmented control: one choice of a few.
pub fn segmented<T: Copy + PartialEq + 'static>(
    id: &str,
    options: &[(T, &str)],
    current: T,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
    label_style: impl Fn(T) -> consort_theme::Type,
    on_pick: impl Fn(&mut ConsortApp, T, &mut Window, &mut Context<ConsortApp>) + Clone + 'static,
) -> Div {
    let c = &t.colors;
    let mut bar = div()
        .flex()
        .gap(t.space.xxs)
        .p(t.space.xxs)
        .bg(c.surface_card)
        .rounded(t.radius.md);
    for (i, (value, label)) in options.iter().enumerate() {
        let active = *value == current;
        let value = *value;
        let pick = on_pick.clone();
        bar = bar.child(
            div()
                .id(SharedString::from(format!("{id}-{i}")))
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .py(t.space.xs)
                .px(t.space.md)
                .rounded(t.radius.sm)
                .cursor_pointer()
                .whitespace_nowrap()
                .when(active, |d| d.bg(c.surface_selected))
                .when(!active, |d| d.hover(|s| s.bg(c.hover_card)))
                .type_style(label_style(value))
                .line_height(t.text.ui.line)
                .text_color(if active {
                    c.text_primary
                } else {
                    c.text_secondary
                })
                .when(active, |d| d.font_weight(t.text.ui_strong.weight))
                .child(label.to_string())
                .on_click(cx.listener(move |this, _, window, cx| pick(this, value, window, cx))),
        );
    }
    bar
}

/// A settings row: a 160 px label column, then the control.
fn setting_row(label: &str, hint: &str, control: impl IntoElement, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .gap(t.space.xl)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .w(sizes::SETTINGS_LABEL)
                .pt(t.space.xs)
                .child(text(label.to_owned(), t.text.ui_strong, c.text_primary))
                .child(text(hint.to_owned(), detail(t), c.text_tertiary)),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// A switch: `accent.text` when on.
pub fn switch(id: &str, on: bool, t: &Tokens) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    let thumb_color = if on {
        c.surface_sidebar
    } else {
        c.text_secondary
    };
    div()
        .id(SharedString::from(id.to_owned()))
        .flex()
        .items_center()
        .w(t.space.xxl)
        .h(t.space.lg + t.space.xxs)
        .px(sizes::SWITCH_INSET)
        .rounded(t.radius.full)
        .cursor_pointer()
        .when(on, |d| d.bg(c.accent_text).justify_end())
        .when(!on, |d| {
            d.bg(c.surface_selected)
                .border(t.border_width)
                .border_color(c.border_strong)
        })
        .child(
            div()
                .size(t.space.md)
                .rounded(t.radius.full)
                .bg(thumb_color),
        )
}

impl ConsortApp {
    pub fn render_settings(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let mut nav = div().flex().flex_col();
        for (gi, (group, items)) in SECTIONS.iter().enumerate() {
            nav = nav.child(
                div()
                    .px(t.space.sm)
                    .pt(if gi == 0 { t.space.md } else { t.space.lg })
                    .pb(t.space.xs)
                    .child(text(*group, t.text.label, c.text_tertiary)),
            );
            for (section, label, icon_name) in items.iter() {
                let selected = self.settings_section == *section;
                let section = *section;
                nav = nav.child(
                    row(t)
                        .id(SharedString::from(format!("settings-{label}")))
                        .cursor_pointer()
                        .when(selected, |d| d.bg(c.surface_selected))
                        .when(!selected, |d| d.hover(|s| s.bg(c.hover_sidebar)))
                        .child(icon(
                            icon_name,
                            t.icon.size,
                            if selected {
                                c.text_primary
                            } else {
                                c.text_secondary
                            },
                        ))
                        .child(
                            text(
                                *label,
                                t.text.ui,
                                if selected {
                                    c.text_primary
                                } else {
                                    c.text_secondary
                                },
                            )
                            .when(selected, |d| d.font_weight(t.text.ui_strong.weight)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.settings_section = section;
                            cx.notify();
                        })),
                );
            }
        }
        let sidebar = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(t.layout.sidebar_width)
            .h_full()
            .px(t.space.sm)
            .bg(c.surface_sidebar)
            .border_r(t.border_width)
            .border_color(c.border_subtle)
            .child(div().h(title_bar_height(t)).w_full().drag_area())
            .child(
                row(t)
                    .id("settings-back")
                    .mt(t.space.xs)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover_sidebar))
                    .child(icon("chevron-left", t.icon.size, c.text_secondary))
                    .child(text("Back to Consort", t.text.ui, c.text_primary))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::Main;
                        cx.notify();
                    })),
            )
            .child(nav);
        let content = match self.settings_section {
            SettingsSection::Appearance => self.render_appearance(t, window, cx),
            SettingsSection::Workers => self.render_workers(t, window, cx),
            other => self.render_later(other, t),
        };
        div()
            .flex()
            .size_full()
            .child(sidebar)
            .child(content)
            .into_any_element()
    }

    fn page_frame(&self, t: &Tokens) -> gpui_kit::Stateful<Div> {
        div()
            .id("settings-page")
            .flex_1()
            .h_full()
            .min_w_0()
            .overflow_y_scroll()
            .pt(t.space.page - t.space.sm)
            .px(t.layout.speaker_gutter)
            .pb(t.space.page)
    }

    fn page_title(&self, title: &str, description: &str, t: &Tokens) -> Div {
        let c = &t.colors;
        div()
            .flex()
            .flex_col()
            .gap(t.space.xs + t.space.xxs)
            .child(text(title.to_owned(), t.text.heading, c.text_primary))
            .child(text(description.to_owned(), t.text.ui, c.text_secondary))
    }

    fn render_later(&self, section: SettingsSection, t: &Tokens) -> AnyElement {
        let (title, line_text) = match section {
            SettingsSection::Notifications => (
                "Notifications",
                "Notifications come in a later build. Consort will tell you when an approval or a decision waits for you.",
            ),
            SettingsSection::Clients => (
                "Clients",
                "This page comes in a later build. It will list the paired desktop and phone apps, and pair a new one with a QR code.",
            ),
            SettingsSection::Hub => (
                "Hub",
                "This page comes in a later build. It will show the hub address, its version and its storage.",
            ),
            SettingsSection::Memory => (
                "Memory",
                "This page comes in a later build. It will list the memory notes for each channel, so you can read and edit them.",
            ),
            SettingsSection::Routines => (
                "Routines",
                "This page comes in a later build. It will list the routines of each channel, with their schedules and last runs.",
            ),
            _ => ("", ""),
        };
        self.page_frame(t)
            .child(
                div()
                    .max_w(t.layout.timeline_max_width)
                    .child(self.page_title(title, line_text, t)),
            )
            .into_any_element()
    }

    fn render_appearance(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let library = ThemeLibrary::global(cx).clone();
        let mode = segmented(
            "mode",
            &[
                (Appearance::System, "Match system"),
                (Appearance::Light, "Light"),
                (Appearance::Dark, "Dark"),
            ],
            library.appearance,
            t,
            cx,
            |_| t.text.ui,
            |_, value, window, cx| consort_theme::choose(None, Some(value), None, window, cx),
        )
        .w(sizes::SETTINGS_SEGMENTED);
        let mut cards = div().flex().flex_wrap().gap(t.space.md);
        for theme in &library.themes {
            let selected = theme.file.name == library.theme.as_ref();
            let name: SharedString = theme.file.name.clone().into();
            let description = match theme.file.name.as_str() {
                "Spruce" => "Spruce green and brass. Default".to_owned(),
                "Velvet" => "Plum velvet and verdigris".to_owned(),
                _ => theme
                    .file
                    .author
                    .clone()
                    .map(|a| format!("By {a}"))
                    .unwrap_or_else(|| "From your themes folder".to_owned()),
            };
            let pick = name.clone();
            cards = cards.child(
                div()
                    .id(SharedString::from(format!("theme-{name}")))
                    .flex()
                    .flex_col()
                    .gap(t.space.md)
                    .w(sizes::THEME_CARD)
                    .p(t.space.sm + t.space.xxs)
                    .rounded(t.radius.lg)
                    .border(t.border_width)
                    .border_color(if selected {
                        c.accent_text
                    } else {
                        c.border_subtle
                    })
                    .bg(c.surface_card)
                    .cursor_pointer()
                    .when(!selected, |d| d.hover(|s| s.border_color(c.border_strong)))
                    .child(
                        div()
                            .flex()
                            .gap(t.space.xs + t.space.xxs)
                            .child(theme_preview(&theme.file, Mode::Dark, t))
                            .child(theme_preview(&theme.file, Mode::Light, t)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .px(t.space.xxs)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .child(text(name.clone(), t.text.ui_strong, c.text_primary))
                                    .child(text(description, detail(t), c.text_tertiary)),
                            )
                            .when(selected, |d| {
                                d.child(glyph("status-resolved", t.icon.size, c.accent_text))
                            }),
                    )
                    .on_click(cx.listener(move |_, _, window, cx| {
                        consort_theme::choose(Some(pick.clone()), None, None, window, cx)
                    })),
            );
        }
        let folder = library
            .user_dir()
            .map(|p| {
                let home = std::env::var("HOME").unwrap_or_default();
                let p = p.display().to_string();
                if !home.is_empty() && p.starts_with(&home) {
                    format!("~{}", &p[home.len()..])
                } else {
                    p
                }
            })
            .unwrap_or_else(|| "the themes folder".to_owned());
        let open_dir = library.user_dir().map(|p| p.to_path_buf());
        let add = div()
            .flex()
            .items_center()
            .gap(t.space.md)
            .w_full()
            .max_w(sizes::THEME_ROW)
            .py(t.space.md + t.space.xxs)
            .px(t.space.lg)
            .rounded(t.radius.lg)
            .border(t.border_width)
            .border_color(c.border_strong)
            .border_dashed()
            .child(icon("plus", t.icon.size, c.text_secondary))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(text("Add a theme", t.text.ui_strong, c.text_primary))
                    .child(text(
                        "Put a theme file in this folder. Consort checks its contrast before it loads it.",
                        detail(t),
                        c.text_tertiary,
                    ))
                    .child(
                        div()
                            .font_family(t.font_mono.clone())
                            .type_style(detail(t))
                            .text_color(c.text_secondary)
                            .child(gpui_kit::base::SelectableText::new("themes-folder", folder)),
                    ),
            )
            .child(
                button("open-themes", "Open folder", ButtonKind::Quiet, t).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if let Some(dir) = open_dir.clone() {
                            let _ = std::fs::create_dir_all(&dir);
                            cx.open_url(&format!("file://{}", dir.display()));
                        } else {
                            this.show_notice("Consort cannot find your config folder.".into(), cx);
                        }
                    },
                )),
            );
        let mut rejected = div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .w_full()
            .max_w(sizes::THEME_ROW);
        for bad in &library.rejected {
            rejected = rejected.child(
                div()
                    .flex()
                    .gap(t.space.sm)
                    .p(t.space.md)
                    .rounded(t.radius.md)
                    .bg(c.tint_failed)
                    .child(icon("circle-alert", t.icon.size, c.status_failed))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(text(
                                format!("{} did not load", bad.file_name),
                                t.text.ui_strong,
                                c.text_primary,
                            ))
                            .child(text(bad.reason.clone(), detail(t), c.text_secondary)),
                    ),
            );
        }
        let sizes = segmented(
            "text-size",
            &[
                (TextSize::Smaller, "Smaller"),
                (TextSize::Default, "Default"),
                (TextSize::Larger, "Larger"),
            ],
            library.text_size,
            t,
            cx,
            |size| {
                let base = t.text.ui;
                let scale = size.scale() / t.text_size.scale();
                consort_theme::Type {
                    size: px((f32::from(base.size) * scale).round()),
                    ..base
                }
            },
            |_, value, window, cx| consort_theme::choose(None, None, Some(value), window, cx),
        )
        .w(sizes::SETTINGS_SEGMENTED);
        let _ = window;
        self.page_frame(t)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xxl + t.space.xs)
                    .child(self.page_title(
                        "Appearance",
                        "A theme changes colors only. Type, spacing and corners stay the same, so every screen keeps its layout.",
                        t,
                    ))
                    .child(setting_row("Mode", "On this computer only", mode, t))
                    .child(setting_row(
                        "Theme",
                        "Each theme has a dark and a light mode",
                        div().flex().flex_col().gap(t.space.md).child(cards).child(add).child(rejected),
                        t,
                    ))
                    .child(setting_row("Text size", "Scales every text style", sizes, t)),
            )
            .into_any_element()
    }

    fn render_workers(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let selected = self
            .settings_worker
            .clone()
            .filter(|id| state.worker(id).is_some())
            .or_else(|| state.workers.first().map(|w| w.id.clone()));
        let mut rows = div().flex().flex_col().gap(t.space.xxs).p(t.space.sm);
        for worker in &state.workers {
            let is_selected = selected.as_deref() == Some(worker.id.as_str());
            let id = worker.id.clone();
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("settings-worker-{}", worker.id)))
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
                            .flex()
                            .items_center()
                            .child(worker_dot(worker.online, sizes::WORKER_DOT, t.icon.size, t)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(line(worker.name.clone(), t.text.ui, c.text_primary))
                            .child(line(
                                worker.list_detail.clone(),
                                detail(t),
                                if is_selected {
                                    c.text_secondary
                                } else {
                                    c.text_tertiary
                                },
                            )),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_worker = Some(id.clone());
                        cx.notify();
                    })),
            );
        }
        let list = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(sizes::WORKER_LIST)
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
                    .child(div().flex_1().type_style(t.text.title).child("Workers"))
                    .child(
                        button("add-worker", "Add worker", ButtonKind::Secondary, t)
                            .child(icon("plus", t.icon.dense, c.text_primary))
                            .flex_row_reverse()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_dialog(Dialog::AddWorker, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .id("workers-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(rows),
            );
        let detail_view = selected
            .and_then(|id| state.worker(&id).cloned())
            .map(|w| self.render_worker_detail(&w, t, window, cx))
            .unwrap_or_else(|| {
                div()
                    .flex_1()
                    .p(t.layout.speaker_gutter)
                    .child(text(
                        "No workers yet. Add one to run tasks.",
                        t.text.ui,
                        c.text_secondary,
                    ))
                    .into_any_element()
            });
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(list)
            .child(detail_view)
            .into_any_element()
    }

    fn render_worker_detail(
        &mut self,
        w: &WorkerView,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let id = w.id.clone();
        let paused = w.paused;
        let mut labels = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(t.space.xs + t.space.xxs);
        for label in &w.labels {
            labels = labels.child(
                div()
                    .py(t.space.xxs)
                    .px(t.space.sm)
                    .rounded(t.radius.sm)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .type_style(detail(t))
                    .text_color(c.text_secondary)
                    .child(label.clone()),
            );
        }
        let labels_id = w.id.clone();
        labels = labels.child(
            div()
                .id("add-label")
                .pl(t.space.sm)
                .type_style(detail(t))
                .text_color(c.text_tertiary)
                .cursor_pointer()
                .hover(|s| s.text_color(c.text_primary))
                .child("Add label")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_dialog(Dialog::EditLabels(labels_id.clone()), window, cx)
                })),
        );
        let card = |t: &Tokens| {
            div()
                .flex()
                .flex_col()
                .rounded(t.radius.lg)
                .border(t.border_width)
                .border_color(t.colors.border_subtle)
                .bg(t.colors.surface_card)
        };
        let mut accounts = card(t);
        let last = w.accounts.len().saturating_sub(1);
        for (i, account) in w.accounts.iter().enumerate() {
            let works = account.auth.works();
            let action = if account.auth == AuthState::SignedOut {
                "Check again"
            } else {
                "Edit"
            };
            accounts = accounts.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .py(t.space.md)
                    .px(t.space.lg)
                    .when(i < last, |d| {
                        d.border_b(t.border_width).border_color(c.border_subtle)
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(text(
                                account.label.clone(),
                                t.text.ui_strong,
                                c.text_primary,
                            ))
                            .child(code_line(&account.detail, &account.detail_code, t)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.xs + t.space.xxs)
                            .w(sizes::ACCOUNT_STATE)
                            .flex_none()
                            .child(worker_dot(
                                works,
                                sizes::STATE_DOT,
                                t.space.sm + t.space.xxs,
                                t,
                            ))
                            .child(text(
                                account.auth.label(),
                                detail(t),
                                if works {
                                    c.text_secondary
                                } else {
                                    c.text_tertiary
                                },
                            )),
                    )
                    .child(
                        button(
                            SharedString::from(format!("account-{}", account.id)),
                            action,
                            ButtonKind::Quiet,
                            t,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_notice(
                                "Harness accounts are set on the worker in worker.toml.".into(),
                                cx,
                            )
                        })),
                    ),
            );
        }
        let policy = &w.policy;
        let mode_trigger = self.triggers.clone();
        let current_mode = policy.max_approval_mode;
        let select = div()
            .id("approval-mode")
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .w(sizes::SETTINGS_SELECT)
            .py(t.space.xs + t.space.xxs)
            .pl(t.space.md)
            .pr(t.space.sm + t.space.xxs)
            .rounded(t.radius.md)
            .border(t.border_width)
            .border_color(c.border_strong)
            .bg(c.surface_card)
            .cursor_pointer()
            .hover(|s| s.border_color(c.text_tertiary))
            .child(mode_trigger.track("approval-mode"))
            .child(text(current_mode.label(), t.text.ui, c.text_primary))
            .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(bounds) = this.triggers.get("approval-mode") {
                    let items = ApprovalMode::ALL
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            MenuItem::new(m.label(), MenuAction::Select("approval-mode", i))
                                .checked(*m == current_mode)
                        })
                        .collect();
                    this.open_menu(OpenMenu::under(bounds, items, true), cx);
                }
            }));
        let roots: Vec<String> = policy.allowed_roots.clone();
        let roots_line = match roots.len() {
            0 => "No folders yet. Tasks cannot open files on this worker.".to_owned(),
            _ => String::new(),
        };
        let shell = policy.allow_shell;
        let name = w.name.clone();
        let mut limits = card(t);
        limits = limits
            .child(limit_row(
                "Highest approval mode",
                &format!(
                    "Tasks on {name} ask before anything riskier than {}.",
                    risk(current_mode)
                ),
                select.into_any_element(),
                true,
                t,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .py(t.space.md)
                    .px(t.space.lg)
                    .border_b(t.border_width)
                    .border_color(c.border_subtle)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(text("Allowed folders", t.text.ui_strong, c.text_primary))
                            .child(if roots.is_empty() {
                                text(roots_line, detail(t), c.text_tertiary).into_any_element()
                            } else {
                                folders_line(&roots, t).into_any_element()
                            }),
                    )
                    .child(
                        button("edit-folders", "Edit folders", ButtonKind::Quiet, t).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.show_notice(
                                    "Allowed folders are set on the worker in worker.toml.".into(),
                                    cx,
                                )
                            }),
                        ),
                    ),
            )
            .child(limit_row(
                "Shell access",
                &format!("Tasks can run commands in {}.", policy.shell_name),
                switch("shell-access", shell, t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_notice(
                            "Shell access is set on the worker in worker.toml.".into(),
                            cx,
                        )
                    }))
                    .into_any_element(),
                false,
                t,
            ));
        let more_id = w.id.clone();
        let more_trigger = self.triggers.clone();
        div()
            .id("worker-detail")
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
                    .max_w(sizes::SETTINGS_DETAIL_MAX)
                    .gap(t.space.xxl + t.space.xs)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(t.space.md)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(t.space.md)
                                    .child(
                                        text(w.name.clone(), t.text.heading, c.text_primary)
                                            .flex_1(),
                                    )
                                    .child(
                                        button(
                                            "pause-worker",
                                            if paused {
                                                "Resume worker"
                                            } else {
                                                "Pause worker"
                                            },
                                            ButtonKind::Secondary,
                                            t,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.run_menu(
                                                    MenuAction::PauseWorker(id.clone(), !paused),
                                                    window,
                                                    cx,
                                                )
                                            }),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .relative()
                                            .child(more_trigger.track("worker-more"))
                                            .child(
                                                icon_button(
                                                    "worker-more",
                                                    "ellipsis",
                                                    t.space.xxl,
                                                    t.icon.size,
                                                    c.text_secondary,
                                                    c.hover_canvas,
                                                    t,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if let Some(bounds) =
                                                        this.triggers.get("worker-more")
                                                    {
                                                        let id = more_id.clone();
                                                        let items = vec![
                                                            MenuItem::new(
                                                                "Rename",
                                                                MenuAction::RenameWorker(
                                                                    id.clone(),
                                                                ),
                                                            ),
                                                            MenuItem::new(
                                                                "Edit labels",
                                                                MenuAction::EditLabels(id.clone()),
                                                            ),
                                                            MenuItem::new(
                                                                "Remove",
                                                                MenuAction::RemoveWorker(id),
                                                            )
                                                            .danger()
                                                            .separated(),
                                                        ];
                                                        this.open_menu(
                                                            OpenMenu::under(bounds, items, false),
                                                            cx,
                                                        );
                                                    }
                                                })),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(t.space.sm)
                                    .child(worker_dot(w.online, sizes::WORKER_DOT, t.space.sm, t))
                                    .child(text(
                                        w.status_line.clone(),
                                        t.text.ui,
                                        c.text_secondary,
                                    )),
                            )
                            .child(labels),
                    )
                    .child(section_block(
                        "Harness accounts",
                        &format!(
                            "Each login stays on {}. Consort only sees whether it works.",
                            w.name
                        ),
                        Some((
                            "Add account",
                            cx.listener(|this, _, _, cx| {
                                this.show_notice(
                                    "Add harness accounts on the worker in worker.toml.".into(),
                                    cx,
                                )
                            }),
                        )),
                        accounts,
                        t,
                    ))
                    .child(section_block(
                        "Limits",
                        &format!(
                            "{} enforces these itself. The hub cannot raise them.",
                            w.name
                        ),
                        None::<(
                            &str,
                            fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App),
                        )>,
                        limits,
                        t,
                    )),
            )
            .into_any_element()
    }

    /// A pick from a select menu.
    pub fn pick_select(
        &mut self,
        id: &'static str,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match id {
            "approval-mode" => {
                let mode = ApprovalMode::ALL.get(index).copied().unwrap_or_default();
                self.show_notice(
                    format!("The approval mode is set on the worker. Set max_approval_mode to {} in worker.toml.", mode_key(mode)),
                    cx,
                );
            }
            "new-channel-group" => {
                self.new_channel.group_id = self.state.groups.get(index).map(|g| g.id.clone());
            }
            "new-channel-selection" => {
                self.new_channel.selection = self.state.selections.get(index).cloned();
            }
            "new-channel-worker" => {
                let free: Vec<String> = self
                    .state
                    .workers
                    .iter()
                    .filter(|w| !self.new_channel.workers.contains(&w.id))
                    .map(|w| w.id.clone())
                    .collect();
                if let Some(id) = free.get(index) {
                    self.new_channel.workers.push(id.clone());
                }
            }
            _ => {}
        }
        cx.notify();
    }
}

fn mode_key(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "ask",
        ApprovalMode::AutoEdits => "auto_edits",
        ApprovalMode::Auto => "auto",
        ApprovalMode::FullAccess => "full_access",
    }
}

fn risk(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "reading files",
        ApprovalMode::AutoEdits => "editing files",
        ApprovalMode::Auto => "running commands in its workspace",
        ApprovalMode::FullAccess => "anything outside its allowed folders",
    }
}

fn section_block(
    title: &str,
    description: &str,
    action: Option<(
        &str,
        impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
    )>,
    body: Div,
    t: &Tokens,
) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .flex_col()
        .gap(t.space.md + t.space.xxs)
        .child(
            div()
                .flex()
                .items_end()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .gap(t.space.xxs)
                        .child(
                            div()
                                .type_style(t.text.body)
                                .font_weight(t.text.title.weight)
                                .text_color(c.text_primary)
                                .child(title.to_owned()),
                        )
                        .child(text(description.to_owned(), t.text.ui, c.text_tertiary)),
                )
                .when_some(action, |d, (label, on_click)| {
                    d.child(
                        button(
                            SharedString::from(format!("section-{title}")),
                            label.to_owned(),
                            ButtonKind::Quiet,
                            t,
                        )
                        .on_click(on_click),
                    )
                }),
        )
        .child(body)
}

fn limit_row(title: &str, hint: &str, control: AnyElement, border: bool, t: &Tokens) -> Div {
    let c = &t.colors;
    div()
        .flex()
        .items_center()
        .gap(t.space.md)
        .py(t.space.md)
        .px(t.space.lg)
        .when(border, |d| {
            d.border_b(t.border_width).border_color(c.border_subtle)
        })
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .child(text(title.to_owned(), t.text.ui_strong, c.text_primary))
                .child(text(hint.to_owned(), detail(t), c.text_tertiary)),
        )
        .child(control)
}

/// A detail line with code spans in mono.
fn code_line(detail_text: &str, code: &[String], t: &Tokens) -> Div {
    let c = &t.colors;
    let mut out = div()
        .flex()
        .flex_wrap()
        .type_style(detail(t))
        .text_color(c.text_tertiary);
    let mut rest = detail_text;
    loop {
        let next = code
            .iter()
            .filter_map(|word| rest.find(word.as_str()).map(|at| (at, word)))
            .min_by_key(|(at, _)| *at);
        match next {
            Some((at, word)) => {
                if at > 0 {
                    out = out.child(rest[..at].to_owned());
                }
                out = out.child(div().font_family(t.font_mono.clone()).child(word.clone()));
                rest = &rest[at + word.len()..];
            }
            None => {
                if !rest.is_empty() {
                    out = out.child(rest.to_owned());
                }
                break;
            }
        }
    }
    out
}

fn folders_line(roots: &[String], t: &Tokens) -> Div {
    let c = &t.colors;
    let mut out = div()
        .flex()
        .flex_wrap()
        .type_style(detail(t))
        .text_color(c.text_tertiary);
    for (i, root) in roots.iter().enumerate() {
        if i > 0 {
            out = out.child(if i + 1 == roots.len() { " and " } else { ", " });
        }
        out = out.child(div().font_family(t.font_mono.clone()).child(root.clone()));
    }
    out.child(". Tasks cannot open anything outside them.")
}

/// A small drawing of a theme mode: a sidebar, two lines and a card.
fn theme_preview(theme: &consort_theme::ThemeFile, mode: Mode, t: &Tokens) -> Div {
    use sizes::preview as pv;
    let p = consort_theme::Palette::new(theme.mode(mode), mode);
    let bar = |w: f32, color: Hsla| div().h(pv::BAR).w(px(w)).rounded(t.radius.full).bg(color);
    div()
        .flex()
        .flex_1()
        .h(sizes::THEME_PREVIEW_HEIGHT)
        .rounded(t.radius.md)
        .overflow_hidden()
        .border(t.border_width)
        .border_color(p.border_subtle)
        .bg(p.surface_canvas)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(pv::BAR_GAP)
                .w(pv::SIDEBAR)
                .h_full()
                .pt(t.space.sm)
                .px(t.space.xs + t.space.xxs)
                .bg(p.surface_sidebar)
                .child(bar(pv::SIDEBAR_ITEM, p.border_strong))
                .child(bar(pv::SIDEBAR_ITEM, p.border_strong))
                .child(bar(pv::SIDEBAR_ITEM, p.border_strong)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .gap(t.space.xs + t.space.xxs)
                .pt(t.space.sm)
                .px(t.space.sm)
                .child(
                    div()
                        .h(pv::CHIP)
                        .w(pv::TITLE)
                        .rounded(t.radius.full)
                        .bg(p.accent_surface),
                )
                .child(bar(pv::TEXT_LINE, p.text_secondary))
                .child(
                    div()
                        .mt(t.space.xs)
                        .flex()
                        .items_end()
                        .gap(t.space.xs)
                        .h(pv::CARD)
                        .p(t.space.xs + t.space.xxs)
                        .rounded(t.radius.sm)
                        .bg(p.surface_card)
                        .border(t.border_width)
                        .border_color(p.border_strong)
                        .child(
                            div()
                                .h(pv::CHIP)
                                .w(pv::BUTTON)
                                .rounded(pv::BUTTON_RADIUS)
                                .bg(p.accent_fill),
                        )
                        .child(
                            div()
                                .h(pv::CHIP)
                                .w(pv::BUTTON)
                                .rounded(pv::BUTTON_RADIUS)
                                .bg(p.border_strong),
                        ),
                ),
        )
}

/// Keeps the import list stable on every platform.
#[allow(dead_code)]
fn _unused(_: &MouseDownEvent, _: MouseButton) {}
