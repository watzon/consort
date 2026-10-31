//! Settings > Connectors, and its dialogs: the Composio key, Add a
//! connector, Connect and Disconnect.

use consort_client::{
    ConnectionStatusView, ConnectionView, LoadState, ToolKindView, connection_row_detail,
    connection_status_line, connections_of_service, connectors_context, example_channel,
    use_in_detail, use_in_match_note,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _, relative,
};

use crate::{
    app::{ConsortApp, Dialog},
    extensions::UseInTarget,
    ui::{
        add_account::AccountDialogParts,
        dialogs::{field_block, input_box},
        ext_parts::{
            dot, faded_scroll, hint, logo_tile, mono_block, page_section, rows_frame, rule_select,
            service_logo, setting_row, use_in_control,
        },
        menu::{OpenMenu, connection_menu},
        parts::{
            ButtonKind, button, detail, error_box, focusable, icon, icon_button, line, selectable,
            text,
        },
        window_controls::DragArea as _,
    },
};

/// The color of a connection's state.
fn status_color(status: &ConnectionStatusView, t: &Tokens) -> gpui_kit::Hsla {
    let c = &t.colors;
    match status {
        ConnectionStatusView::Active => c.status_done,
        ConnectionStatusView::Connecting { .. } => c.status_working,
        ConnectionStatusView::NeedsSignIn { .. } | ConnectionStatusView::Failed { .. } => {
            c.status_failed
        }
    }
}

impl ConsortApp {
    fn is_focused_input(
        &self,
        input: &gpui_kit::Entity<gpui_kit::component::input::InputState>,
        window: &Window,
        cx: &gpui_kit::App,
    ) -> bool {
        use gpui_kit::Focusable as _;
        input.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Settings > Connectors: the list with the Composio status under it,
    /// and the page of the open connection.
    pub fn render_connectors(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let selected = self
            .ext
            .connection
            .clone()
            .filter(|id| state.connection(id).is_some())
            .or_else(|| state.connections.first().map(|c| c.id.clone()));
        let monid_page = self.monid_ui.page && state.monid.key_present;
        let mut rows = div().flex().flex_col().gap(t.space.xxs).p(t.space.sm);
        if state.monid.key_present {
            rows = rows.child(self.monid_list_row(monid_page, t, cx));
        }
        for connection in &state.connections {
            let is_selected = !monid_page && selected.as_deref() == Some(connection.id.as_str());
            let id = connection.id.clone();
            let menu_id = connection.id.clone();
            let handle = connection.handle.clone();
            let problem = connection.status.needs_sign_in();
            rows = rows.child(
                focusable(
                    div().id(SharedString::from(format!("connection-{}", connection.id))),
                    t,
                )
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
                        .child(service_logo(&connection.logo, t.icon.size, t)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(line(connection.name.clone(), t.text.ui, c.text_primary))
                        .child(text(
                            connection_row_detail(connection, &state.channels),
                            detail(t),
                            if problem {
                                c.status_failed
                            } else if is_selected {
                                c.text_secondary
                            } else {
                                c.text_tertiary
                            },
                        )),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_connection(Some(id.clone()), Some(window), cx);
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        let items = connection_menu(&menu_id, &handle);
                        this.open_menu(OpenMenu::at(event.position, items), cx);
                        cx.stop_propagation();
                    }),
                ),
            );
        }
        let key_present = state.composio.key_present;
        let list = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(t.component.worker_list)
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .type_style(t.text.title)
                            .child("Connectors"),
                    )
                    .when(key_present, |d| {
                        d.child(
                            button("add-connector", "Add connector", ButtonKind::Secondary, t)
                                .child(icon("plus", t.icon.dense, c.text_primary))
                                .flex_row_reverse()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_dialog(Dialog::AddConnector, window, cx)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .id("connections-list")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .child(rows),
            )
            .child(self.composio_footer(t, cx))
            .child(self.monid_footer(t, cx));
        let page = if monid_page {
            self.render_monid_page(t, window, cx)
        } else if !key_present {
            self.composio_setup(t, window, cx)
        } else {
            match selected.and_then(|id| state.connection(&id).cloned()) {
                Some(connection) => self.connection_page(&connection, t, window, cx),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .p(t.layout.speaker_gutter)
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(t.space.md)
                    .child(text(
                        "No connectors yet. Add one, and the assistant can use that service in the channels you choose.",
                        t.text.ui,
                        c.text_secondary,
                    ))
                    .child(
                        button("add-first-connector", "Add connector", ButtonKind::Primary, t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_dialog(Dialog::AddConnector, window, cx)
                            })),
                    )
                    .into_any_element(),
            }
        };
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(list)
            .child(page)
            .into_any_element()
    }

    /// The Composio block under the list: the plan and its usage, and
    /// Change key.
    fn composio_footer(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let composio = &self.state.composio;
        let share = composio.usage_share();
        let line_text = match (&composio.load, composio.key_present) {
            (LoadState::Unsupported(m), _) | (LoadState::Failed(m), _) => Some(m.clone()),
            (_, false) => {
                Some("No key yet. Add your Composio project key to connect services.".to_owned())
            }
            (_, true) => composio
                .error
                .clone()
                .or_else(|| composio.usage_line())
                .or_else(|| {
                    composio
                        .key_hint
                        .as_ref()
                        .map(|h| format!("Key ending in {h}."))
                }),
        };
        let action = if composio.key_present {
            "Change key"
        } else {
            "Add key"
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(t.space.xs + t.space.xxs)
            .py(t.space.md)
            .px(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(text("Composio", t.text.label, c.text_secondary))
                    .child(
                        focusable(div().id("composio-change-key"), t)
                            .rounded(t.radius.sm)
                            .cursor_pointer()
                            .type_style(detail(t))
                            .text_color(c.text_tertiary)
                            .hover(|s| s.text_color(c.text_primary))
                            .child(action)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_dialog(Dialog::ComposioKey, window, cx)
                            })),
                    ),
            )
            .when_some(share, |d, share| {
                d.child(
                    div()
                        .h(t.component.usage_bar)
                        .w_full()
                        .rounded(t.radius.full)
                        .bg(c.border_strong)
                        .child(
                            div()
                                .h_full()
                                .w(relative(share))
                                .rounded(t.radius.full)
                                .bg(c.status_done),
                        ),
                )
            })
            .children(line_text.map(|l| {
                text(
                    l,
                    consort_theme::Type {
                        line: t.text.caption.line,
                        ..t.text.caption
                    },
                    if composio.error.is_some() {
                        c.status_failed
                    } else {
                        c.text_tertiary
                    },
                )
            }))
    }

    /// The page when the hub has no Composio key yet.
    fn composio_setup(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let unsupported = self.state.composio.load.error().map(str::to_owned);
        let focused = self.is_focused_input(&self.ext_inputs.composio_key, window, cx);
        let busy = self.in_flight.is_running("composio-key");
        let body = match unsupported {
            Some(message) => div().child(error_box("composio-unsupported", message, t)),
            None => div()
                .flex()
                .flex_col()
                .gap(t.space.md)
                .child(
                    div()
                        .flex()
                        .gap(t.space.sm)
                        .child(input_box(&self.ext_inputs.composio_key, focused, t).flex_1().min_w_0())
                        .child(
                            button("save-composio-key", "Save key", ButtonKind::Primary, t)
                                .when(busy, |b| b.opacity(t.opacity.busy))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_composio_key(window, cx)
                                })),
                        ),
                )
                .child(hint(
                    "Make a project key in the Composio dashboard, under Settings. The key stays on the hub. It never comes back to this app.",
                    t,
                ))
                .children(
                    self.key_error
                        .clone()
                        .map(|err| error_box("composio-key-error", err, t)),
                ),
        };
        div()
            .id("connectors-setup")
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
                    .gap(t.space.xl)
                    .max_w(t.component.settings_detail_max)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(t.space.xs + t.space.xxs)
                            .child(text("Add your Composio key", t.text.heading, c.text_primary))
                            .child(text(
                                "Consort connects services such as Gmail and GitHub through Composio. You bring a Composio project key, and you connect each account here in the app.",
                                t.text.ui,
                                c.text_secondary,
                            )),
                    )
                    .child(body),
            )
            .into_any_element()
    }

    /// The page of one connection.
    fn connection_page(
        &mut self,
        connection: &ConnectionView,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let tools = state.tools_of(&connection.service);
        let total = tools
            .filter(|t| t.load == LoadState::Loaded)
            .map(|t| t.tools.len());
        let reconnect_id = connection.id.clone();
        let more_id = connection.id.clone();
        let more_handle = connection.handle.clone();
        let focused = self.is_focused_input(&self.ext_inputs.connection_name, window, cx);
        let header = div()
            .flex()
            .items_start()
            .gap(t.space.lg)
            .child(logo_tile(&connection.logo, t))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.xs + t.space.xxs)
                    .child(text(
                        connection.name.clone(),
                        t.text.heading,
                        c.text_primary,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .child(dot(status_color(&connection.status, t), t))
                            .child(
                                text(
                                    connection_status_line(connection, total),
                                    t.text.ui,
                                    if connection.status.needs_sign_in() {
                                        c.status_failed
                                    } else {
                                        c.text_secondary
                                    },
                                )
                                .min_w_0(),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(t.space.sm)
                    .child(
                        button(
                            "reconnect-connection",
                            "Reconnect",
                            if connection.status.needs_sign_in() {
                                ButtonKind::Primary
                            } else {
                                ButtonKind::Secondary
                            },
                            t,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.reconnect(reconnect_id.clone(), window, cx)
                            },
                        )),
                    )
                    .child(
                        div()
                            .relative()
                            .child(self.triggers.track("connection-more"))
                            .child(
                                icon_button(
                                    "connection-more",
                                    "ellipsis",
                                    t.space.xxl,
                                    t.icon.size,
                                    c.text_secondary,
                                    c.hover_canvas,
                                    t,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if let Some(bounds) = this.triggers.get("connection-more") {
                                            let items = connection_menu(&more_id, &more_handle);
                                            this.open_menu(
                                                OpenMenu::under(bounds, items, false),
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            ),
                    ),
            );
        let handle_line = div()
            .flex()
            .flex_wrap()
            .child(hint("The assistant calls it ", t))
            .child(
                div()
                    .type_style(detail(t))
                    .font_family(t.font_mono.clone())
                    .text_color(c.text_secondary)
                    .child(connection.handle.clone()),
            )
            .child(hint(".", t));
        let scope = rows_frame(t)
            .child(setting_row(
                "Name",
                handle_line,
                div().w(t.component.connection_field).child(input_box(
                    &self.ext_inputs.connection_name,
                    focused,
                    t,
                )),
                false,
                t,
            ))
            .child(setting_row(
                "Use in",
                hint(use_in_detail(&connection.use_in, &state.channels), t),
                use_in_control(
                    UseInTarget::Connection(connection.id.clone()),
                    &connection.use_in,
                    false,
                    t,
                    self,
                    cx,
                ),
                true,
                t,
            ));
        let mut approvals = rows_frame(t);
        for (i, kind) in ToolKindView::ALL.iter().enumerate() {
            let count = self.tool_count(&connection.service, *kind);
            let hint_text = match count {
                Some(1) => format!("{} 1 tool.", kind.hint()),
                Some(n) => format!("{} {n} tools.", kind.hint()),
                None => kind.hint().to_owned(),
            };
            approvals = approvals.child(setting_row(
                kind.label(),
                hint(hint_text, t),
                rule_select(
                    Some(connection.id.clone()),
                    *kind,
                    connection.approvals.rule_for(*kind),
                    t,
                    self,
                    cx,
                ),
                i == ToolKindView::ALL.len() - 1,
                t,
            ));
        }
        let example = example_channel(&connection.use_in, &state.channels);
        let sees = match example {
            Some(channel) => {
                let block = connectors_context(&state, &channel.id);
                page_section(
                    "What the assistant sees",
                    &format!(
                        "Every session in {} starts with this list. Tools stay out of the context until the assistant asks for them.",
                        channel.name
                    ),
                    mono_block(
                        SharedString::from(format!("connectors-context-{}", connection.id)),
                        if block.is_empty() {
                            "No connector is on in this channel.".to_owned()
                        } else {
                            block
                        },
                        t,
                    ),
                    t,
                )
            }
            None => page_section(
                "What the assistant sees",
                "No channel is in this connector's scope yet, so no session sees it.",
                div(),
                t,
            ),
        };
        div()
            .id("connection-page")
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
                    .max_w(t.component.settings_detail_max)
                    .gap(t.space.xxl + t.space.xs)
                    .child(header)
                    .child(page_section(
                        "Where the assistant can use it",
                        "The assistant sees this connector in these channels, under the name below.",
                        scope,
                        t,
                    ))
                    .child(page_section(
                        "Approvals",
                        "Composio marks each tool as reading, changing or deleting. Consort asks you by that mark.",
                        approvals,
                        t,
                    ))
                    .child(sees),
            )
            .into_any_element()
    }

    /// The dialog that changes or removes the Composio key.
    pub fn composio_key_dialog(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let c = &t.colors;
        let focused = self.is_focused_input(&self.ext_inputs.composio_key, window, cx);
        let composio = self.state.composio.clone();
        let busy = self.dialog_busy();
        let save: crate::app::DialogConfirm =
            std::rc::Rc::new(|this, window, cx| this.save_composio_key(window, cx));
        self.dialog_confirm.replace(Some(save.clone()));
        let body = field_block(
            "Project key",
            input_box(&self.ext_inputs.composio_key, focused, t),
            Some(match &composio.key_hint {
                Some(hint) => "The hub keeps the key and uses it for every connector. The key ending in HINT stops working here when you save a new one.".replace("HINT", hint),
                None => "The hub keeps the key and uses it for every connector.".to_owned(),
            })
            .as_deref(),
            t,
        );
        let actions = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .when(composio.key_present, |d| {
                d.child(
                    button("remove-composio-key", "Remove key", ButtonKind::Danger, t).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.run_menu(
                                crate::ui::menu::MenuAction::RemoveComposioKey,
                                window,
                                cx,
                            )
                        }),
                    ),
                )
            })
            .child(div().flex_1().min_w_0())
            .child(
                button("dialog-cancel", "Cancel", ButtonKind::Quiet, t)
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            )
            .child(
                button("dialog-confirm", "Save key", ButtonKind::Primary, t)
                    .when(busy, |b| b.opacity(t.opacity.busy))
                    .on_click(cx.listener(move |this, _, window, cx| save(this, window, cx))),
            );
        (
            if composio.key_present {
                "Change the Composio key".to_owned()
            } else {
                "Add the Composio key".to_owned()
            },
            "Consort connects services through your Composio project. Removing the key stops every connector until you add a key again.".to_owned(),
            t.layout.dialog_width,
            body,
            actions,
            None,
        )
    }

    /// Add a connector: the catalog, with its search and categories. The
    /// grid scrolls inside the dialog.
    pub fn add_connector_dialog(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let c = &t.colors;
        let catalog = self.state.connector_catalog.clone();
        let focused = self.is_focused_input(&self.ext_inputs.catalog_search, window, cx);
        let search = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .py(t.component.input_pad_y)
            .px(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(if focused {
                c.accent_text
            } else {
                c.border_strong
            })
            .rounded(t.radius.md)
            .child(icon("search", t.icon.dense, c.text_tertiary))
            .child(crate::ui::parts::input(
                &self.ext_inputs.catalog_search,
                t.text.ui,
            ));
        let chip = |id: String, label: String, active: bool| {
            focusable(div().id(SharedString::from(id)), t)
                .py(t.space.xs)
                .px(t.space.sm + t.space.xxs)
                .rounded(t.radius.full)
                .border(t.border_width)
                .cursor_pointer()
                .type_style(t.text.label)
                .whitespace_nowrap()
                .when(active, |d| {
                    d.bg(c.surface_selected)
                        .border_color(c.surface_selected)
                        .text_color(c.text_primary)
                })
                .when(!active, |d| {
                    d.border_color(c.border_strong)
                        .text_color(c.text_secondary)
                        .hover(|s| s.bg(c.hover_raised))
                })
                .child(label)
        };
        let mut chips = div().flex().flex_wrap().gap(t.space.sm).child(
            chip(
                "category-popular".into(),
                "Popular".into(),
                catalog.category.is_none(),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                let search = this.ext_inputs.catalog_search.read(cx).value().to_string();
                this.search_catalog_in(search, None, cx);
            })),
        );
        for category in &catalog.categories {
            let slug = category.slug.clone();
            chips = chips.child(
                chip(
                    format!("category-{}", category.slug),
                    category.name.clone(),
                    catalog.category.as_deref() == Some(category.slug.as_str()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let search = this.ext_inputs.catalog_search.read(cx).value().to_string();
                    this.search_catalog_in(search, Some(slug.clone()), cx);
                })),
            );
        }
        let grid_content: AnyElement = match &catalog.load {
            LoadState::Failed(message) | LoadState::Unsupported(message) => div()
                .p(t.space.xl)
                .child(error_box("catalog-error", message.clone(), t))
                .into_any_element(),
            LoadState::Loading if catalog.services.is_empty() => div()
                .p(t.space.xl)
                .child(text("Loading the catalog", t.text.ui, c.text_tertiary))
                .into_any_element(),
            _ if catalog.services.is_empty() => div()
                .p(t.space.xl)
                .child(text(
                    if catalog.search.trim().is_empty() {
                        "No services in this category.".to_owned()
                    } else {
                        format!(
                            "No service matches \"{}\". Try another name.",
                            catalog.search.trim()
                        )
                    },
                    t.text.ui,
                    c.text_secondary,
                ))
                .into_any_element(),
            _ => {
                let mut grid = div()
                    .grid()
                    .grid_cols(3)
                    .gap(t.space.sm + t.space.xxs)
                    .px(t.space.xl)
                    .py(t.space.lg);
                for service in &catalog.services {
                    let picked = service.clone();
                    let connected = service.connected_count;
                    grid = grid.child(
                        focusable(
                            div().id(SharedString::from(format!("service-{}", service.slug))),
                            t,
                        )
                        .flex()
                        .flex_col()
                        .gap(t.space.sm)
                        .p(t.space.md)
                        .min_w_0()
                        .rounded(t.radius.lg)
                        .border(t.border_width)
                        .border_color(c.border_strong)
                        .bg(c.surface_card)
                        .cursor_pointer()
                        .hover(|s| s.bg(c.hover_card))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(t.space.sm)
                                .child(service_logo(&service.logo, t.component.provider_logo, t))
                                .child(
                                    line(service.name.clone(), t.text.ui_strong, c.text_primary)
                                        .flex_1()
                                        .min_w_0(),
                                )
                                .when(connected > 0, |d| {
                                    d.child(text(
                                        format!("{connected} connected"),
                                        t.text.caption,
                                        c.status_done,
                                    ))
                                }),
                        )
                        .child(text(
                            if service.tool_count > 0 {
                                format!("{} {} tools.", service.description, service.tool_count)
                            } else {
                                service.description.clone()
                            },
                            detail(t),
                            c.text_secondary,
                        ))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_connect(picked.clone(), None, true, window, cx)
                            },
                        )),
                    );
                }
                let more: Option<AnyElement> = catalog.next_cursor.as_ref().map(|_| {
                    let loading = catalog.more.is_loading();
                    div()
                        .flex()
                        .justify_center()
                        .pb(t.space.lg)
                        .child(
                            button(
                                "catalog-more",
                                if loading { "Loading more" } else { "Show more" },
                                ButtonKind::Secondary,
                                t,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                let client = this.client.clone();
                                this.spawn_command(cx, async move {
                                    client
                                        .run(consort_client::Command::LoadMoreConnectorCatalog)
                                        .await
                                });
                            })),
                        )
                        .into_any_element()
                });
                div().child(grid).children(more).into_any_element()
            }
        };
        let handle = self.ext.catalog_scroll.clone();
        let grid = faded_scroll(
            "catalog-grid",
            &handle,
            t.component.catalog_list,
            grid_content,
            c.surface_raised,
            c.raised_clear,
            t,
            cx,
        )
        // The grid reaches the edges of the dialog, under a line.
        .mx(-t.space.xl)
        .mb(-t.space.xl)
        .border_t(t.border_width)
        .border_color(c.border_subtle);
        let offer = self.monid_offer(t, cx);
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(search)
            .child(chips)
            .children(offer)
            .child(grid);
        (
            "Add a connector".to_owned(),
            "Connect a service once, and the assistant can use it in the channels you choose. Composio keeps the sign-in.".to_owned(),
            t.layout.dialog_catalog,
            body,
            div().hidden(),
            None,
        )
    }

    /// Connect one service: the name, Use in, the approvals, and the
    /// browser or the credential form.
    pub fn connect_dialog(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let c = &t.colors;
        let state = self.state.clone();
        let form = &self.ext.connect;
        let service = form.service.clone();
        let browser = form.uses_browser();
        let reconnect = form.reconnect.is_some();
        let attempt = form
            .attempt
            .as_ref()
            .and_then(|id| state.connection(id).cloned());
        let existing: Vec<&ConnectionView> = connections_of_service(&state, &service.slug)
            .into_iter()
            .filter(|c| Some(&c.id) != form.reconnect.as_ref())
            .collect();
        let intro = if reconnect {
            format!(
                "Sign in to {} again. The name and the settings stay.",
                service.name
            )
        } else {
            consort_client::connect_intro(&service.name, &existing)
        };
        let name_text = self.ext_inputs.connect_name.read(cx).value().to_string();
        let handle = consort_client::handle_for(&name_text, &state, form.reconnect.as_deref());
        let focused = self.is_focused_input(&self.ext_inputs.connect_name, window, cx);
        let mut body = div().flex().flex_col().gap(t.space.lg);
        if !reconnect {
            let handle_hint = div()
                .flex()
                .flex_wrap()
                .child(hint("The assistant sees it as ", t))
                .child(
                    div()
                        .type_style(detail(t))
                        .font_family(t.font_mono.clone())
                        .text_color(c.text_secondary)
                        .child(handle),
                )
                .child(hint(", next to the account's email address.", t));
            body = body
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(t.space.xs + t.space.xxs)
                        .child(text("Name", t.text.label, c.text_secondary))
                        .child(input_box(&self.ext_inputs.connect_name, focused, t))
                        .child(handle_hint),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(t.space.xs + t.space.xxs)
                        .child(text("Use in", t.text.label, c.text_secondary))
                        .child(use_in_control(
                            UseInTarget::ConnectDialog,
                            &form.use_in,
                            true,
                            t,
                            self,
                            cx,
                        ))
                        .child(hint(use_in_match_note(&form.use_in, &state.channels), t)),
                );
            let mut rules = div().flex().flex_col().gap(t.space.sm);
            for kind in ToolKindView::ALL {
                rules = rules.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.md)
                        .child(
                            text(kind.label(), t.text.ui, c.text_secondary)
                                .flex_1()
                                .min_w_0(),
                        )
                        .child(rule_select(
                            None,
                            kind,
                            form.approvals.rule_for(kind),
                            t,
                            self,
                            cx,
                        )),
                );
            }
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.md)
                    .p(t.space.lg - t.space.xxs)
                    .rounded(t.radius.lg)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .child(text("Approvals start as", t.text.ui_strong, c.text_primary))
                    .child(rules),
            );
        }
        let fields: Vec<_> = form.fields.clone();
        if !browser && !fields.is_empty() {
            let mut form_box = div().flex().flex_col().gap(t.space.md);
            for (field, input) in &fields {
                let focused = self.is_focused_input(input, window, cx);
                form_box = form_box.child(field_block(
                    &field.label,
                    input_box(input, focused, t),
                    field.description.as_deref(),
                    t,
                ));
            }
            body = body.child(form_box).child(hint(
                "The values go to the hub and on to Composio. Consort does not keep them, and the assistant never sees them.",
                t,
            ));
        }
        if let Some(connection) = &attempt
            && matches!(connection.status, ConnectionStatusView::Connecting { .. })
        {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .p(t.space.md)
                    .rounded(t.radius.md)
                    .bg(c.tint_working)
                    .child(crate::ui::parts::status_glyph(
                        "connect-waiting",
                        consort_client::TaskStatus::Working,
                        t.icon.size,
                        t,
                        cx,
                    ))
                    .child(text(
                        if browser {
                            format!(
                                "Waiting for you to finish the sign-in in your browser. {} turns active here when it is done.",
                                connection.name
                            )
                        } else {
                            format!("Checking the {} account.", service.name)
                        },
                        t.text.ui,
                        c.text_primary,
                    ).min_w_0().flex_1().min_w_0()),
            );
        }
        let busy = self.in_flight.is_running("connect");
        let waiting = attempt.is_some();
        let confirm_label = match (browser, waiting) {
            (true, true) => "Open the page again",
            (true, false) => "Continue in browser",
            (false, _) => "Connect",
        };
        let submit: crate::app::DialogConfirm =
            std::rc::Rc::new(|this, window, cx| this.submit_connect(window, cx));
        self.dialog_confirm.replace(Some(submit.clone()));
        let from_catalog = self.ext.connect.from_catalog;
        let actions = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                text(
                    if browser {
                        format!("{}'s sign-in opens in your browser.", service.name)
                    } else {
                        format!("{} signs in with a key, not a browser.", service.name)
                    },
                    detail(t),
                    c.text_tertiary,
                )
                .flex_1()
                .min_w_0(),
            )
            .child(
                button(
                    "dialog-cancel",
                    if from_catalog { "Back" } else { "Cancel" },
                    ButtonKind::Secondary,
                    t,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if from_catalog {
                        this.open_dialog(Dialog::AddConnector, window, cx);
                    } else {
                        this.close_dialog(window, cx);
                    }
                })),
            )
            .child(
                button("dialog-confirm", confirm_label, ButtonKind::Primary, t)
                    .when(browser, |b| {
                        b.child(icon("external-link", t.icon.dense, c.accent_on_fill))
                    })
                    .when(busy, |b| b.opacity(t.opacity.busy))
                    .on_click(cx.listener(move |this, _, window, cx| submit(this, window, cx))),
            );
        (
            if reconnect {
                format!("Reconnect {}", service.name)
            } else {
                format!("Connect {}", service.name)
            },
            intro,
            t.layout.dialog_wide,
            body,
            actions,
            Some(logo_tile(&service.logo, t).into_any_element()),
        )
    }

    /// The calls that the assistant made with a connection, newest first:
    /// the tool, the decision, the outcome, the time and the size of the
    /// result. Older calls load with Show more.
    pub fn connection_activity_dialog(
        &mut self,
        id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let c = &t.colors;
        let name = self
            .state
            .connection(id)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let activity = self.ext.activity.clone();
        // One row is a line of columns. The tool takes the free width.
        let columns = |tool: AnyElement,
                       decision: AnyElement,
                       outcome: AnyElement,
                       time: AnyElement,
                       size: AnyElement| {
            div()
                .flex()
                .items_start()
                .gap(t.space.md)
                .child(div().flex_1().min_w_0().child(tool))
                .child(div().w(relative(0.24)).flex_none().child(decision))
                .child(div().w(relative(0.12)).flex_none().child(outcome))
                .child(div().w(relative(0.2)).flex_none().child(time))
                .child(
                    div()
                        .w(relative(0.1))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .child(size),
                )
        };
        let head =
            |label: &str| text(label.to_owned(), detail(t), c.text_tertiary).into_any_element();
        let header = columns(
            head("Tool"),
            head("Decision"),
            head("Outcome"),
            head("Time"),
            head("Size"),
        )
        .pb(t.space.xs)
        .border_b(t.border_width)
        .border_color(c.border_subtle);
        let mut list = div().flex().flex_col();
        for call in &activity.calls {
            let (outcome, outcome_color) = if call.succeeded {
                ("Succeeded", c.status_done)
            } else {
                ("Failed", c.status_failed)
            };
            let tool = div()
                .flex()
                .flex_col()
                .gap(t.space.xxs)
                .child(selectable(
                    SharedString::from(format!("call-tool-{}", call.id)),
                    call.tool.clone(),
                    t.text.ui,
                    c.text_primary,
                ))
                .when_some(call.error.clone(), |d, error| {
                    d.child(selectable(
                        SharedString::from(format!("call-error-{}", call.id)),
                        error,
                        detail(t),
                        c.status_failed,
                    ))
                })
                .into_any_element();
            list = list.child(
                columns(
                    tool,
                    text(call.decision.clone(), t.text.ui, c.text_secondary).into_any_element(),
                    text(outcome, t.text.ui, outcome_color).into_any_element(),
                    text(
                        format!("{}, {}", call.day_label, call.time_label),
                        t.text.ui,
                        c.text_secondary,
                    )
                    .into_any_element(),
                    text(call.size_label.clone(), t.text.ui, c.text_secondary).into_any_element(),
                )
                .id(SharedString::from(format!("call-{}", call.id)))
                .debug_selector({
                    let key = format!("call-{}", call.id);
                    move || key.clone()
                })
                .py(t.space.sm)
                .border_b(t.border_width)
                .border_color(c.border_subtle),
            );
        }
        let connection_id = id.to_owned();
        let more_id = connection_id.clone();
        let body: AnyElement = if let Some(error) = &activity.error
            && activity.calls.is_empty()
        {
            div()
                .flex()
                .flex_col()
                .items_start()
                .gap(t.space.md)
                .child(error_box("activity-error", error.clone(), t))
                .child(
                    button("activity-retry", "Try again", ButtonKind::Secondary, t)
                        .debug_selector(|| "activity-retry".into())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.load_activity(connection_id.clone(), false, cx)
                        })),
                )
                .into_any_element()
        } else if activity.loading {
            text("Loading the activity", t.text.ui, c.text_tertiary).into_any_element()
        } else if activity.calls.is_empty() {
            text(
                "No calls yet. They show here when the assistant uses this connection.",
                t.text.ui,
                c.text_secondary,
            )
            .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(t.space.md)
                .child(header)
                .child(
                    div()
                        .id("connection-activity")
                        .h(t.component.files_view)
                        .overflow_y_scroll()
                        .child(list),
                )
                .when_some(activity.error.clone(), |d, error| {
                    d.child(error_box("activity-more-error", error, t))
                })
                .when(activity.has_more, |d| {
                    d.child(
                        div().flex().child(
                            button("activity-more", "Show more", ButtonKind::Secondary, t)
                                .debug_selector(|| "activity-more".into())
                                .when(activity.loading_more, |b| b.opacity(t.opacity.busy))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.load_activity(more_id.clone(), true, cx)
                                })),
                        ),
                    )
                })
                .into_any_element()
        };
        let actions = div().flex().justify_end().child(
            button("dialog-confirm", "Done", ButtonKind::Primary, t)
                .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
        );
        (
            format!("Activity of {name}"),
            "The tool calls that the assistant made with this connection, newest first. The hub keeps the size of each result, never the result.".to_owned(),
            t.layout.dialog_catalog,
            div().child(body),
            actions,
            None,
        )
    }

    /// Ask before a connection is disconnected.
    pub fn confirm_disconnect_dialog(
        &mut self,
        id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let name = self
            .state
            .connection(id)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let id = id.to_owned();
        (
            format!("Disconnect {name}?"),
            "Composio signs the account out, and no channel can use it. Tasks that wait on it fail. To use the account again, connect it again.".to_owned(),
            t.layout.dialog_width,
            div(),
            self.dialog_actions_kind("Disconnect", ButtonKind::Danger, t, cx, move |this, window, cx| {
                let client = this.client.clone();
                let id = id.clone();
                this.submit_dialog(
                    async move {
                        client
                            .run(consort_client::Command::DisconnectConnection {
                                connection_id: id,
                            })
                            .await
                    },
                    window,
                    cx,
                );
            }),
            None,
        )
    }
}
