//! Monid in the Channel panel (ADR 0016 and ADR 0017): the Monid row of the
//! Connectors tab, the Monid view that the row opens, and the Temporary
//! access section.
//!
//! The view has the rule for each call, the list of allowed tools or the
//! list of tools that the channel never uses, a search that adds a tool to a
//! list, and the recent calls. A change applies to the next call.

use consort_client::{
    ApprovalRuleView, LoadState,
    access::temporary_access_rows,
    monid::{
        MonidAccessView, MonidAllowedView, MonidSearchHit, MonidToolView, channel_row,
        channel_subtitle, each_call_choices, each_call_label,
    },
};
use consort_theme::Tokens;
use gpui_kit::{
    Context, Div, InteractiveElement, MouseButton, MouseDownEvent, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    monid::{CHANNEL_RULE_SELECT, FOLLOW_HUB, MonidAction},
    ui::{
        admin_parts::select,
        dialogs::input_box,
        ext_parts::{hint, service_logo},
        menu::{MenuAction, MenuItem, OpenMenu},
        parts::{ButtonKind, button, detail, focusable, icon, line, selectable, text},
        settings::{segmented, switch},
        timeline::open_context_menu,
    },
};

impl ConsortApp {
    /// The Monid row of the Connectors tab, or `None` when the channel does
    /// not show Monid. A click on the row opens the view. The switch turns
    /// Monid off in this channel only.
    pub fn monid_channel_row(
        &mut self,
        channel_id: &str,
        last: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::Stateful<Div>> {
        let c = &t.colors;
        let row = channel_row(&self.state, channel_id)?;
        let enabled = row.enabled;
        let channel = channel_id.to_owned();
        let logo = crate::ui::monid_settings::monid_logo();
        Some(
            focusable(div().id("channel-monid-row"), t)
                .debug_selector(|| "channel-monid-row".into())
                .flex()
                .items_center()
                .gap(t.space.md)
                .py(t.space.sm + t.space.xxs)
                .px(t.space.md + t.space.xxs)
                .cursor_pointer()
                .when(!last, |d| {
                    d.border_b(t.border_width).border_color(c.border_subtle)
                })
                .child(
                    div()
                        .when(!enabled, |d| d.opacity(t.opacity.answered_dim))
                        .child(service_logo(&logo, t.component.provider_logo, t)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(line(
                            "Monid",
                            t.text.ui_strong,
                            if enabled {
                                c.text_primary
                            } else {
                                c.text_secondary
                            },
                        ))
                        .child(text(row.detail, detail(t), c.text_tertiary)),
                )
                .child(icon("chevron-right", t.icon.dense, c.text_tertiary))
                .child(
                    div().flex_none().child(
                        switch("channel-monid-switch", enabled, t)
                            .debug_selector(|| "channel-monid-switch".into())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_monid_access(&channel, cx, |access| {
                                    access.with_disabled(enabled)
                                });
                                cx.stop_propagation();
                            })),
                    ),
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_monid_panel(window, cx))),
        )
    }

    /// The Temporary access section, or `None` when no grant is live.
    pub fn temporary_access_section(
        &mut self,
        channel_id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::Stateful<Div>> {
        let c = &t.colors;
        let rows = temporary_access_rows(&self.state, channel_id, consort_client::format::now_ms());
        if rows.is_empty() {
            return None;
        }
        let mut list = div()
            .id("temporary-access")
            .debug_selector(|| "temporary-access".into())
            .flex()
            .flex_col()
            .gap(t.space.xs)
            .py(t.space.md)
            .px(t.space.md + t.space.xxs)
            .rounded(t.radius.lg)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .bg(c.surface_card)
            .child(text("Temporary access", t.text.ui_strong, c.text_primary));
        for row in rows {
            let channel = channel_id.to_owned();
            let grant = row.grant_id.clone();
            let id = format!("end-access-{}", row.grant_id);
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .pt(t.space.sm)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(selectable(
                                SharedString::from(format!("access-name-{}", row.grant_id)),
                                row.name.clone(),
                                t.text.ui,
                                c.text_primary,
                            ))
                            .child(text(row.detail.clone(), detail(t), c.text_tertiary)),
                    )
                    .child(
                        button(
                            SharedString::from(id.clone()),
                            "End",
                            ButtonKind::Secondary,
                            t,
                        )
                        .debug_selector(move || id.clone())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.run_monid_action(
                                    MonidAction::EndAccess(channel.clone(), grant.clone()),
                                    window,
                                    cx,
                                )
                            },
                        )),
                    ),
            );
        }
        Some(list)
    }

    /// The Monid view of the Connectors tab.
    pub fn render_monid_view(
        &mut self,
        channel_id: &str,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let state = self.state.clone();
        let access = state
            .channel(channel_id)
            .map(|ch| ch.monid.clone())
            .unwrap_or_default();
        let settings = &state.monid.settings;
        let back = focusable(div().id("monid-back"), t)
            .debug_selector(|| "monid-back".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(t.space.xs)
            .self_start()
            .rounded(t.radius.md)
            .px(t.space.xs)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_canvas))
            .child(icon("chevron-left", t.icon.dense, c.text_secondary))
            .child(text("Connectors", t.text.ui, c.text_secondary))
            .on_click(cx.listener(|this, _, _, cx| this.close_monid_panel(cx)));
        let title = div()
            .flex()
            .flex_wrap()
            .items_baseline()
            .gap(t.space.sm)
            .child(text("Monid", t.text.body, c.text_primary).font_weight(t.text.title.weight))
            .child(text(
                channel_subtitle(&state.monid, channel_id),
                detail(t),
                c.text_tertiary,
            ));
        // Each call.
        let current = access.approval;
        let mut options: Vec<(String, String, bool)> = Vec::new();
        for (rule, label) in each_call_choices(settings) {
            let option = rule.map_or(FOLLOW_HUB, ApprovalRuleView::key).to_owned();
            options.push((label, option, rule == current));
        }
        let each_call = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(t.space.md)
            .child(text("Each call", t.text.ui, c.text_primary).flex_none())
            .child(
                select(
                    CHANNEL_RULE_SELECT,
                    each_call_label(settings, current),
                    t.component.group_select,
                    options,
                    self,
                    t,
                    cx,
                )
                .min_w_0(),
            );
        // Tools: all, or only these.
        let only = access.only_allowed;
        let channel = channel_id.to_owned();
        let mode = segmented(
            "monid-tools-mode",
            &[(false, "All tools"), (true, "Only these tools")],
            only,
            t,
            cx,
            |_| t.text.ui,
            move |this, value, _, cx| {
                this.update_monid_access(&channel, cx, move |access| {
                    access.with_only_allowed(value)
                })
            },
        );
        let focused = {
            use gpui_kit::Focusable as _;
            self.monid_inputs
                .search
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        };
        let search = input_box(&self.monid_inputs.search, focused, t);
        let query = self.monid_inputs.search.read(cx).value().trim().to_owned();
        let results = if query.is_empty() {
            None
        } else {
            Some(self.monid_search_results(channel_id, &access, t, cx))
        };
        let mut body = div()
            .id("monid-view")
            .debug_selector(|| "monid-view".into())
            .flex()
            .flex_col()
            .gap(t.space.lg)
            .px(t.space.lg)
            .pt(t.space.sm)
            .pb(t.space.lg)
            .child(back)
            .child(title)
            .child(each_call)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.sm)
                    .child(text("Tools", t.text.ui, c.text_secondary))
                    .child(mode)
                    .child(search)
                    .children(results),
            );
        if only {
            body = body.child(self.monid_tool_list(channel_id, None, &access, t, cx));
        }
        body = body.child(self.monid_tool_list(channel_id, Some("Never use here"), &access, t, cx));
        body = body.child(self.monid_recent_calls(channel_id, t));
        let list = body.overflow_y_scroll();
        div().flex().flex_col().flex_1().min_w_0().min_h_0().child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(list.size_full())
                .child(fade_top(t))
                .child(fade_bottom(t)),
        )
    }

    /// The results of the search: one row for each tool, with Add and a
    /// menu.
    fn monid_search_results(
        &mut self,
        channel_id: &str,
        access: &MonidAccessView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let search = self.state.monid.search.clone();
        let mut list = div().flex().flex_col().gap(t.space.xxs);
        match &search.load {
            LoadState::Loading => {
                list = list.child(hint("Searching the Monid catalog.", t));
            }
            LoadState::Failed(message) | LoadState::Unsupported(message) => {
                list = list.child(text(message.clone(), detail(t), c.status_failed));
            }
            _ if search.hits.is_empty() => {
                list = list.child(hint("No tool matches. Try other words.", t));
            }
            _ => {}
        }
        for (i, hit) in search.hits.iter().enumerate() {
            list = list.child(self.monid_hit_row(channel_id, access, i, hit, t, cx));
        }
        list
    }

    fn monid_hit_row(
        &mut self,
        channel_id: &str,
        access: &MonidAccessView,
        index: usize,
        hit: &MonidSearchHit,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let tool = hit.tool();
        let provider = hit.provider_tool();
        let added = access.allowed.iter().any(|a| a.tool == tool);
        let channel = channel_id.to_owned();
        let add_channel = channel.clone();
        let add_tool = tool.clone();
        let menu_items = vec![
            MenuItem::new(
                format!("Add all {} tools", hit.provider_name),
                MenuAction::Monid(MonidAction::AddAllowed(channel.clone(), provider)),
            ),
            MenuItem::new(
                "Never use this tool here",
                MenuAction::Monid(MonidAction::AddBlocked(channel, tool)),
            ),
        ];
        let id = format!("monid-hit-{index}");
        let more_trigger = format!("{id}-more");
        div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .py(t.space.xs)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(line(hit.name.clone(), t.text.ui, c.text_primary))
                    .child(line(
                        format!("{}. {}", hit.provider_name, hit.rate),
                        detail(t),
                        c.text_tertiary,
                    )),
            )
            .child(
                button(
                    SharedString::from(format!("{id}-add")),
                    if added { "Added" } else { "Add" },
                    ButtonKind::Quiet,
                    t,
                )
                .when(added, |b| b.opacity(t.opacity.answered_dim))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !added {
                        this.run_monid_action(
                            MonidAction::AddAllowed(add_channel.clone(), add_tool.clone()),
                            window,
                            cx,
                        );
                    }
                })),
            )
            .child(
                crate::ui::parts::icon_button(
                    SharedString::from(format!("{id}-more")),
                    "ellipsis",
                    t.space.xl,
                    t.icon.dense,
                    c.text_secondary,
                    c.hover_canvas,
                    t,
                    cx,
                )
                .relative()
                .child(self.triggers.track(format!("{id}-more")))
                .on_click(cx.listener(
                    move |this, event: &gpui_kit::ClickEvent, _, cx| {
                        let menu = match this.triggers.get(&more_trigger) {
                            Some(bounds) => OpenMenu::under(bounds, menu_items.clone(), false),
                            None => OpenMenu::at(event.position(), menu_items.clone()),
                        };
                        this.open_menu(menu, cx);
                    },
                )),
            )
    }

    /// The list of allowed tools (`title` is `None`), or of the tools that
    /// the channel never uses.
    fn monid_tool_list(
        &mut self,
        channel_id: &str,
        title: Option<&'static str>,
        access: &MonidAccessView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let blocked = title.is_some();
        let mut list = div().flex().flex_col().gap(t.space.xs);
        if let Some(title) = title {
            if access.blocked.is_empty() {
                return div();
            }
            list = list.child(text(title, t.text.ui, c.text_secondary));
        } else if access.allowed.is_empty() {
            return div().child(hint(
                "No tool is on the list. The assistant cannot use Monid here until you add one.",
                t,
            ));
        }
        let rows: Vec<(MonidToolView, Option<&'static str>, bool)> = if blocked {
            access
                .blocked
                .iter()
                .map(|tool| (tool.clone(), None, false))
                .collect()
        } else {
            access
                .allowed
                .iter()
                .map(|a: &MonidAllowedView| {
                    (
                        a.tool.clone(),
                        a.run_without_asking.then_some("Runs without asking"),
                        a.tool.is_provider(),
                    )
                })
                .collect()
        };
        for (i, (tool, note, is_provider)) in rows.into_iter().enumerate() {
            let channel = channel_id.to_owned();
            let action = if blocked {
                MonidAction::RemoveBlocked(channel.clone(), i)
            } else {
                MonidAction::RemoveAllowed(channel.clone(), i)
            };
            let menu = vec![MenuItem::new("Remove", MenuAction::Monid(action.clone())).danger()];
            let id = format!("monid-{}-{i}", if blocked { "never" } else { "allowed" });
            list = list.child(
                div()
                    .id(SharedString::from(id.clone()))
                    .debug_selector({
                        let id = id.clone();
                        move || id.clone()
                    })
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.xxs)
                    .child(
                        line(tool.title(), t.text.ui, c.text_primary)
                            .flex_1()
                            .min_w_0(),
                    )
                    .children(is_provider.then(|| {
                        text("Provider", detail(t), c.text_tertiary)
                            .flex_none()
                            .whitespace_nowrap()
                    }))
                    .children(note.map(|n| {
                        text(n, detail(t), c.text_tertiary)
                            .flex_none()
                            .whitespace_nowrap()
                    }))
                    .child(
                        button(
                            SharedString::from(format!("{id}-remove")),
                            "Remove",
                            ButtonKind::Quiet,
                            t,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.run_monid_action(action.clone(), window, cx)
                            },
                        )),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            open_context_menu(this, event, menu.clone(), window, cx)
                        }),
                    ),
            );
        }
        list
    }

    /// The recent calls of the channel: the tool, the time and the cost.
    fn monid_recent_calls(&mut self, channel_id: &str, t: &Tokens) -> Div {
        let c = &t.colors;
        let calls = self.state.monid.calls.get(channel_id).cloned();
        let Some(calls) = calls.filter(|c| !c.calls.is_empty()) else {
            return div();
        };
        let mut list = div().flex().flex_col().gap(t.space.xs).child(text(
            "Recent calls",
            t.text.ui,
            c.text_secondary,
        ));
        for call in &calls.calls {
            list = list.child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(t.space.sm)
                    .py(t.space.xxs)
                    .child(
                        line(call.name.clone(), t.text.ui, c.text_primary)
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        text(call.time_label.clone(), detail(t), c.text_tertiary)
                            .flex_none()
                            .whitespace_nowrap(),
                    )
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .font_family(t.font_mono.clone())
                            .type_style(t.text.mono)
                            .text_color(if call.state.is_error() {
                                c.status_failed
                            } else {
                                c.text_secondary
                            })
                            .child(call.summary.clone()),
                    ),
            );
        }
        list
    }
}

use consort_theme::TypeExt as _;
use gpui_kit::{linear_color_stop, linear_gradient};

fn fade_top(t: &Tokens) -> Div {
    let c = &t.colors;
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
        ))
}

fn fade_bottom(t: &Tokens) -> Div {
    let c = &t.colors;
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
        ))
}
