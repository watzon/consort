//! The sidebar: search, Everything, the groups of channels, New channel, the
//! workers and the account. The macOS traffic lights sit in its top 44 px.

use consort_client::{SidebarChannel, WorkerView};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, Window, WindowControlArea, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Dialog, Page, SettingsSection},
    ui::sizes,
    ui::{
        menu::{MenuAction, MenuItem, OpenMenu},
        parts::{icon, line, status_glyph, text, worker_dot},
    },
};

/// The height of the title bar strip, where the traffic lights sit.
pub fn title_bar_height(t: &Tokens) -> gpui_kit::Pixels {
    t.space.xxl + t.space.md
}

/// A sidebar row: 28 px, `md` corners, 8 px inset.
pub fn row(t: &Tokens) -> Div {
    div()
        .flex()
        .items_center()
        .gap(t.space.sm)
        .py(t.space.xs)
        .px(t.space.sm)
        .rounded(t.radius.md)
}

impl ConsortApp {
    pub fn render_sidebar(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let sidebar = &state.sidebar;
        let selected = self.channel_id.clone();

        let search = row(t)
            .id("search")
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_sidebar))
            .child(icon("search", t.icon.size, c.text_secondary))
            .child(text("Search", t.text.ui, c.text_secondary).flex_1())
            .child(text("⌘K", t.text.caption, c.text_tertiary))
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_notice("Search comes in a later build.".into(), cx)
            }));

        let everything = self.channel_row(&sidebar.everything, true, &selected, t, cx);

        let mut groups = div().flex().flex_col();
        for (gi, group) in sidebar.groups.iter().enumerate() {
            let collapsed = group.collapsed;
            let group_id = group.id.clone();
            let mut block = div()
                .flex()
                .flex_col()
                .gap(t.space.xxs)
                .pt(if gi == 0 { t.space.lg } else { t.space.md })
                .child(
                    div()
                        .id(SharedString::from(format!("group-{}", group.id)))
                        .flex()
                        .items_center()
                        .gap(t.space.xs)
                        .py(t.space.xs)
                        .px(t.space.sm)
                        .cursor_pointer()
                        .child(text(group.name.clone(), t.text.label, c.text_tertiary))
                        .child(icon(
                            if collapsed {
                                "chevron-right"
                            } else {
                                "chevron-down"
                            },
                            t.space.md,
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
            if gi + 1 == sidebar.groups.len() {
                block = block.child(self.new_channel_row(t, cx));
            }
            groups = groups.child(block);
        }
        if !sidebar.ungrouped.is_empty() || sidebar.groups.is_empty() {
            let mut block = div().flex().flex_col().gap(t.space.xxs).pt(t.space.md);
            for channel in &sidebar.ungrouped {
                block = block.child(self.channel_row(channel, false, &selected, t, cx));
            }
            if sidebar.groups.is_empty() {
                block = block.child(self.new_channel_row(t, cx));
            }
            groups = groups.child(block);
        }

        let mut workers = div()
            .flex()
            .flex_col()
            .gap(t.space.xxs)
            .py(t.space.sm)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .py(t.space.xs)
                    .px(t.space.sm)
                    .child(text("Workers", t.text.label, c.text_tertiary).flex_1())
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
                .gap(t.space.sm)
                .pt(t.space.sm)
                .pb(t.space.xs)
                .px(t.space.sm)
                .border_t(t.border_width)
                .border_color(c.border_subtle)
                .child(
                    div()
                        .size(t.space.xl)
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
                .child(text(state.user.name.clone(), t.text.ui, c.text_primary).flex_1())
                .child(
                    crate::ui::parts::icon_button(
                        "open-settings",
                        "settings",
                        t.space.xl,
                        t.icon.size,
                        c.text_tertiary,
                        c.hover_sidebar,
                        t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_settings(SettingsSection::Appearance, cx)
                    })),
                );

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(t.layout.sidebar_width)
            .h_full()
            .px(t.space.sm)
            .pb(t.space.sm)
            .bg(c.surface_sidebar)
            .border_r(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .h(title_bar_height(t))
                    .flex_none()
                    .w_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xxs)
                    .pt(t.space.xs)
                    .child(search)
                    .child(everything),
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
            .child(account)
            .into_any_element()
    }

    fn new_channel_row(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        row(t)
            .id("new-channel")
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_sidebar))
            .child(icon("plus", t.icon.size, c.text_tertiary))
            .child(text("New channel", t.text.ui, c.text_tertiary))
            .on_click(
                cx.listener(|this, _, window, cx| this.open_dialog(Dialog::NewChannel, window, cx)),
            )
            .into_any_element()
    }

    fn channel_row(
        &mut self,
        channel: &SidebarChannel,
        everything: bool,
        selected: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let is_selected = self.page == Page::Main && channel.id == selected;
        let name_color = if is_selected || channel.attention {
            c.text_primary
        } else {
            c.text_secondary
        };
        let id = channel.id.clone();
        let menu_id = channel.id.clone();
        let leading = if everything {
            crate::ui::parts::glyph(
                "brace",
                t.icon.size,
                if is_selected {
                    c.text_primary
                } else {
                    c.text_secondary
                },
            )
            .into_any_element()
        } else {
            icon(&channel.icon, t.icon.size, c.text_secondary).into_any_element()
        };
        let status = match channel.status {
            Some(status) => status_glyph(
                SharedString::from(format!("sidebar-status-{}", channel.id)),
                status,
                t.icon.size,
                t,
                cx,
            ),
            None => div().size(t.icon.size).flex_none().into_any_element(),
        };
        row(t)
            .id(SharedString::from(format!("channel-{}", channel.id)))
            .cursor_pointer()
            .when(is_selected, |d| d.bg(c.surface_selected))
            .when(!is_selected, |d| d.hover(|s| s.bg(c.hover_sidebar)))
            .child(leading)
            .child(
                line(channel.name.clone(), t.text.ui, name_color)
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
            .when(!everything, |d| d.child(status))
            .on_click(
                cx.listener(move |this, _, window, cx| this.open_channel(id.clone(), window, cx)),
            )
            .when(!everything, |d| {
                d.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &gpui_kit::MouseDownEvent, _, cx| {
                        let id = menu_id.clone();
                        let items = vec![
                            MenuItem::new("Rename", MenuAction::RenameChannel(id.clone())),
                            MenuItem::new(
                                "Instructions",
                                MenuAction::ChannelInstructions(id.clone()),
                            ),
                            MenuItem::new("Change icon", MenuAction::ChangeIcon(id.clone())),
                            MenuItem::new("Move to group", MenuAction::MoveToGroup(id.clone())),
                            MenuItem::new("Archive", MenuAction::ArchiveChannel(id.clone()))
                                .separated(),
                            MenuItem::new("Delete", MenuAction::DeleteChannel(id)).danger(),
                        ];
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
        let id = worker.id.clone();
        let menu_id = worker.id.clone();
        let paused = worker.paused;
        div()
            .id(SharedString::from(format!("worker-{}", worker.id)))
            .flex()
            .items_center()
            .gap(t.space.sm)
            .py(sizes::WORKER_ROW_PAD_Y)
            .px(t.space.sm)
            .rounded(t.radius.md)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_sidebar))
            .child(worker_dot(worker.online, sizes::WORKER_DOT, t.icon.size, t))
            .child(
                line(
                    worker.name.clone(),
                    t.text.ui,
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
                    let id = menu_id.clone();
                    let items = vec![
                        MenuItem::new("Rename", MenuAction::RenameWorker(id.clone())),
                        MenuItem::new("Edit labels", MenuAction::EditLabels(id.clone())),
                        MenuItem::new(
                            if paused { "Resume" } else { "Pause" },
                            MenuAction::PauseWorker(id.clone(), !paused),
                        ),
                        MenuItem::new("Remove", MenuAction::RemoveWorker(id))
                            .danger()
                            .separated(),
                    ];
                    this.open_menu(OpenMenu::at(event.position, items), cx);
                    cx.stop_propagation();
                }),
            )
            .into_any_element()
    }
}
