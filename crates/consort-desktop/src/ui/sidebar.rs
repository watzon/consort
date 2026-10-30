//! The sidebar: search, Tutti, the groups of channels, New channel, the
//! workers and the account. The macOS traffic lights sit in its title bar
//! strip (`component.sidebar.title_bar`). The sizes come from
//! `component.sidebar` in `design/structure.json`.

use consort_client::{SidebarChannel, WorkerView};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Dialog, Page, SettingsSection},
    ui::window_controls::DragArea as _,
    ui::{
        menu::{OpenMenu, archived_channel_menu, channel_menu, worker_menu},
        parts::{ButtonKind, button, focusable, icon, line, status_glyph, text, worker_dot},
    },
};

/// The height of the title bar strip, where the traffic lights sit.
pub fn title_bar_height(t: &Tokens) -> gpui_kit::Pixels {
    t.component.sidebar.title_bar
}

/// A sidebar row: 36 px with the row text, `lg` corners. The settings rail
/// uses it too.
pub fn row(t: &Tokens) -> Div {
    let sb = t.component.sidebar;
    div()
        .flex()
        .items_center()
        .gap(sb.row_gap)
        .py(sb.row_pad_y)
        .px(sb.row_pad_x)
        .rounded(t.radius.lg)
}

/// A group label in the sidebar or the settings rail.
pub fn group_label(t: &Tokens) -> Div {
    let sb = t.component.sidebar;
    div()
        .flex()
        .items_center()
        .gap(t.space.xs)
        .py(sb.label_pad_y)
        .px(sb.row_pad_x)
}

impl ConsortApp {
    pub fn render_sidebar(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let sb = t.component.sidebar;
        let state = self.state.clone();
        let sidebar = &state.sidebar;
        let selected = self.channel_id.clone();

        let search = focusable(row(t).id("search"), t)
            .debug_selector(|| "sidebar-search".into())
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_sidebar))
            .child(icon("search", sb.row_icon, c.text_secondary))
            .child(text("Search", sb.row_text, c.text_secondary).flex_1())
            .child(text(
                if cfg!(target_os = "macos") {
                    "⌘K"
                } else {
                    "Ctrl+K"
                },
                t.text.ui,
                c.text_tertiary,
            ))
            .on_click(cx.listener(|this, _, window, cx| this.open_switcher(window, cx)));

        // Before the first snapshot, Tutti is an empty row. It would
        // open a channel with no ID, so it waits with the other channels.
        let waiting = self.waiting();
        let tutti = (!sidebar.tutti.id.is_empty())
            .then(|| self.channel_row(&sidebar.tutti, true, &selected, t, cx));

        let mut groups = div().flex().flex_col();
        for (gi, group) in sidebar.groups.iter().enumerate() {
            let collapsed = group.collapsed;
            let group_id = group.id.clone();
            let mut block = div()
                .flex()
                .flex_col()
                .gap(t.space.xxs)
                .pt(sb.group_top)
                .child(
                    focusable(
                        group_label(t)
                            .id(SharedString::from(format!("group-{}", group.id)))
                            .when(gi == 0, |d| {
                                d.debug_selector(|| "sidebar-group-label".into())
                            }),
                        t,
                    )
                    .cursor_pointer()
                    .child(text(group.name.clone(), sb.label_text, c.text_tertiary))
                    .child(icon(
                        if collapsed {
                            "chevron-right"
                        } else {
                            "chevron-down"
                        },
                        t.icon.dense,
                        c.text_tertiary,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let client = this.client.clone();
                        let group_id = group_id.clone();
                        this.spawn_command(cx, async move {
                            client
                                .run(consort_client::Command::SetGroupCollapsed {
                                    group_id,
                                    collapsed: !collapsed,
                                })
                                .await
                        });
                    })),
                );
            if !collapsed {
                for channel in &group.channels {
                    block = block.child(self.channel_row(channel, false, &selected, t, cx));
                }
            }
            if gi + 1 == sidebar.groups.len() && waiting.is_none() {
                block = block.child(self.new_channel_row(t, cx));
            }
            groups = groups.child(block);
        }
        if let Some(waiting) = &waiting {
            // No channel has loaded: say why, in place of an empty list.
            groups = groups.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .pt(sb.group_top)
                    .px(sb.row_pad_x)
                    .child(text(waiting.title, sb.row_text, c.text_secondary)),
            );
        } else if !sidebar.ungrouped.is_empty() || sidebar.groups.is_empty() {
            let mut block = div().flex().flex_col().gap(t.space.xxs).pt(sb.group_top);
            for channel in &sidebar.ungrouped {
                block = block.child(self.channel_row(channel, false, &selected, t, cx));
            }
            if sidebar.groups.is_empty() {
                block = block.child(self.new_channel_row(t, cx));
            }
            groups = groups.child(block);
        }
        if !state.archived_channels.is_empty() {
            groups = groups.child(self.archived_block(t, cx));
        }

        let mut workers = div()
            .flex()
            .flex_col()
            .gap(t.space.xxs)
            .py(t.space.sm)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                group_label(t)
                    .child(text("Workers", sb.label_text, c.text_tertiary).flex_1())
                    .child(text(
                        format!(
                            "{} of {} online",
                            sidebar.workers_online, sidebar.workers_total
                        ),
                        t.text.caption,
                        c.text_tertiary,
                    )),
            );
        for worker in &state.workers {
            workers = workers.child(self.worker_row(worker, t, cx));
        }

        let account =
            div()
                .flex()
                .items_center()
                .gap(sb.row_gap)
                .pt(sb.account_pad_top)
                .pb(sb.account_pad_bottom)
                .px(sb.row_pad_x)
                .border_t(t.border_width)
                .border_color(c.border_subtle)
                .child(
                    div()
                        .id("sidebar-avatar")
                        .debug_selector(|| "sidebar-avatar".into())
                        .size(sb.avatar)
                        .flex_none()
                        .rounded(t.radius.full)
                        .bg(c.surface_selected)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .type_style(t.text.caption)
                                .font_weight(t.text.title.weight)
                                .text_color(c.text_primary)
                                .child(state.user.initials.clone()),
                        ),
                )
                .child(text(state.user.name.clone(), sb.row_text, c.text_primary).flex_1())
                .child(
                    crate::ui::parts::icon_button(
                        "open-settings",
                        "settings",
                        t.component.composer_chip,
                        sb.row_icon,
                        c.text_tertiary,
                        c.hover_sidebar,
                        t,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_settings(SettingsSection::Appearance, cx)
                    })),
                );

        div()
            .id("sidebar")
            .debug_selector(|| "sidebar".into())
            .flex()
            .flex_col()
            .flex_none()
            .w(t.layout.sidebar_width)
            .h_full()
            .px(sb.inset)
            .pb(t.space.sm)
            .bg(c.surface_sidebar)
            .border_r(t.border_width)
            .border_color(c.border_subtle)
            // macOS draws the traffic lights in this strip. Elsewhere the
            // title bar above the window has the controls.
            .when(cfg!(target_os = "macos"), |d| {
                d.child(
                    div()
                        .h(title_bar_height(t))
                        .flex_none()
                        .w_full()
                        .drag_area(),
                )
            })
            .when(!cfg!(target_os = "macos"), |d| d.pt(sb.inset))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xxs)
                    .pt(t.space.xs)
                    .child(search)
                    .children(tutti),
            )
            .child(
                div()
                    .id("channels")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(groups),
            )
            .child(workers)
            .children(self.update_notice(t, cx))
            .child(account)
            .into_any_element()
    }

    /// The Archived list: a header that opens it, and one row for each
    /// archived channel with a way to bring it back. An archived channel has
    /// no timeline in the app, so a row does not open it.
    fn archived_block(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let sb = t.component.sidebar;
        let state = self.state.clone();
        let open = self.archived_open;
        let mut block = div()
            .flex()
            .flex_col()
            .gap(t.space.xxs)
            .pt(sb.group_top)
            .child(
                focusable(group_label(t).id("archived-header"), t)
                    .cursor_pointer()
                    .child(text("Archived", sb.label_text, c.text_tertiary))
                    .child(text(
                        state.archived_channels.len().to_string(),
                        t.text.caption,
                        c.text_tertiary,
                    ))
                    .child(icon(
                        if open {
                            "chevron-down"
                        } else {
                            "chevron-right"
                        },
                        t.icon.dense,
                        c.text_tertiary,
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.archived_open = !this.archived_open;
                        cx.notify();
                    })),
            );
        if !open {
            return block.into_any_element();
        }
        for channel in &state.archived_channels {
            let id = channel.id.clone();
            let menu_id = channel.id.clone();
            block = block.child(
                row(t)
                    .id(SharedString::from(format!("archived-{}", channel.id)))
                    .child(icon(&channel.icon, sb.row_icon, c.text_tertiary))
                    .child(line(channel.name.clone(), sb.row_text, c.text_tertiary).flex_1())
                    .child(
                        button(
                            SharedString::from(format!("unarchive-{}", channel.id)),
                            "Unarchive",
                            ButtonKind::Quiet,
                            t,
                        )
                        .py(t.space.xxs)
                        .px(t.space.sm)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.run_menu(
                                    crate::ui::menu::MenuAction::UnarchiveChannel(id.clone()),
                                    window,
                                    cx,
                                )
                            },
                        )),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &gpui_kit::MouseDownEvent, _, cx| {
                            let items = archived_channel_menu(&menu_id);
                            this.open_menu(OpenMenu::at(event.position, items), cx);
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        block.into_any_element()
    }

    fn new_channel_row(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let sb = t.component.sidebar;
        focusable(row(t).id("new-channel"), t)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_sidebar))
            .child(icon("plus", sb.row_icon, c.text_tertiary))
            .child(text("New channel", sb.row_text, c.text_tertiary))
            .on_click(
                cx.listener(|this, _, window, cx| this.open_dialog(Dialog::NewChannel, window, cx)),
            )
            .into_any_element()
    }

    fn channel_row(
        &mut self,
        channel: &SidebarChannel,
        tutti: bool,
        selected: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let sb = t.component.sidebar;
        let is_selected = self.page == Page::Main && channel.id == selected;
        let name_color = if is_selected || channel.attention {
            c.text_primary
        } else {
            c.text_secondary
        };
        let id = channel.id.clone();
        let menu_id = channel.id.clone();
        let leading = if tutti {
            // The Three voices mark, idle and sharp.
            crate::ui::mark::still_mark(sb.row_icon, t)
        } else {
            icon(&channel.icon, sb.row_icon, c.text_secondary).into_any_element()
        };
        let status = match channel.status {
            Some(status) => status_glyph(
                SharedString::from(format!("sidebar-status-{}", channel.id)),
                status,
                sb.row_icon,
                t,
                cx,
            ),
            None => div().size(sb.row_icon).flex_none().into_any_element(),
        };
        let selector = format!("sidebar-channel-{}", channel.id);
        focusable(
            row(t)
                .id(SharedString::from(format!("channel-{}", channel.id)))
                .debug_selector(move || selector),
            t,
        )
        .cursor_pointer()
        .when(is_selected, |d| d.bg(c.surface_selected))
        .when(!is_selected, |d| d.hover(|s| s.bg(c.hover_sidebar)))
        .child(leading)
        .child(
            line(channel.name.clone(), sb.row_text, name_color)
                .flex_1()
                .when(is_selected, |d| d.font_weight(t.text.ui_strong.weight)),
        )
        .when(channel.unread > 0 && channel.status.is_none(), |d| {
            d.child(
                div()
                    .size(t.space.xs + t.space.xxs)
                    .rounded(t.radius.full)
                    .bg(c.accent_text)
                    .mr(t.space.xs + t.space.xxs),
            )
        })
        .when(!tutti || channel.status.is_some(), |d| d.child(status))
        .on_click(cx.listener(move |this, _, window, cx| this.open_channel(id.clone(), window, cx)))
        .when(!tutti, |d| {
            d.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui_kit::MouseDownEvent, _, cx| {
                    let items = channel_menu(&menu_id);
                    this.open_menu(OpenMenu::at(event.position, items), cx);
                    cx.stop_propagation();
                }),
            )
        })
        .into_any_element()
    }

    fn worker_row(
        &mut self,
        worker: &WorkerView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let sb = t.component.sidebar;
        let id = worker.id.clone();
        let menu_id = worker.id.clone();
        let paused = worker.paused;
        focusable(
            div()
                .id(SharedString::from(format!("worker-{}", worker.id)))
                .debug_selector({
                    let selector = format!("sidebar-worker-{}", worker.id);
                    move || selector
                }),
            t,
        )
        .flex()
        .items_center()
        .gap(sb.row_gap)
        .py(sb.worker_pad_y)
        .px(sb.row_pad_x)
        .rounded(t.radius.lg)
        .cursor_pointer()
        .hover(|s| s.bg(c.hover_sidebar))
        .child(worker_dot(
            worker.online,
            t.component.worker_dot,
            sb.row_icon,
            t,
        ))
        .child(
            line(
                worker.name.clone(),
                sb.worker_text,
                if worker.online {
                    c.text_secondary
                } else {
                    c.text_tertiary
                },
            )
            .flex_1(),
        )
        .child(
            text(
                if worker.paused {
                    "Paused".to_owned()
                } else {
                    worker.sidebar_detail.clone()
                },
                t.text.caption,
                c.text_tertiary,
            )
            .w(t.layout.speaker_gutter)
            .flex_none()
            .text_right()
            .truncate(),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.settings_worker = Some(id.clone());
            this.open_settings(SettingsSection::Workers, cx);
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &gpui_kit::MouseDownEvent, _, cx| {
                let items = worker_menu(&menu_id, paused);
                this.open_menu(OpenMenu::at(event.position, items), cx);
                cx.stop_propagation();
            }),
        )
        .into_any_element()
    }
}
