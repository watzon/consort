//! The parts of Settings > Workers that manage a worker's tags, accounts and
//! tools. The Limits section and the page frame stay in `settings.rs`.

use consort_client::{
    AccountAction, AccountStatus, HarnessAccountView, ToolState, ToolView, WorkerView,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, Focusable as _, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window,
    component::text::TextView, div, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Dialog},
    ui::{
        menu::{OpenMenu, account_menu},
        parts::{
            ButtonKind, button, detail, focusable, icon, provider_logo, status_glyph, tag_chip,
            text,
        },
    },
};

/// How many completions the Add tag field offers at most.
const TAG_COMPLETIONS: usize = 6;

/// A section of the worker page: a title, a line under it and an action at
/// the right, over a body.
pub fn worker_section(
    title: &str,
    description: AnyElement,
    action: Option<AnyElement>,
    body: impl IntoElement,
    t: &Tokens,
) -> Div {
    let c = &t.colors;
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
                        .child(
                            div()
                                .type_style(t.text.body)
                                .font_weight(t.text.title.weight)
                                .text_color(c.text_primary)
                                .child(title.to_owned()),
                        )
                        .child(description),
                )
                .children(action),
        )
        .child(body)
}

/// A list with a border and rows inside it, as in the designs: no fill of
/// its own.
pub fn row_list(t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(t.radius.lg)
        .border(t.border_width)
        .border_color(t.colors.border_subtle)
}

/// One row of a row list.
fn list_row(last: bool, t: &Tokens) -> Div {
    div()
        .flex()
        .items_center()
        .gap(t.space.md)
        .py(t.space.sm + t.space.xxs)
        .px(t.space.lg)
        .when(!last, |d| {
            d.border_b(t.border_width)
                .border_color(t.colors.border_subtle)
        })
}

/// A small bordered button inside a row: Sign in again, Update to X.
fn row_button(id: impl Into<SharedString>, label: String, t: &Tokens) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    focusable(div().id(gpui_kit::ElementId::Name(id.into())), t)
        .flex_none()
        .py(t.space.xxs)
        .px(t.space.sm + t.space.xxs)
        .rounded(t.radius.md)
        .bg(c.surface_raised)
        .border(t.border_width)
        .border_color(c.border_strong)
        .cursor_pointer()
        .hover(|s| s.bg(c.hover_raised))
        .type_style(t.text.label)
        .line_height(t.text.ui.line)
        .text_color(c.text_primary)
        .whitespace_nowrap()
        .child(label)
}

/// A text action inside a row: Sign out, Install now.
fn row_link(
    id: impl Into<SharedString>,
    label: String,
    style: consort_theme::Type,
    t: &Tokens,
) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    focusable(div().id(gpui_kit::ElementId::Name(id.into())), t)
        .flex_none()
        .rounded(t.radius.sm)
        .cursor_pointer()
        .type_style(style)
        .text_color(c.text_secondary)
        .hover(|s| s.text_color(c.text_primary))
        .whitespace_nowrap()
        .child(label)
}

/// The dot of an account status: solid for a working or failed account, an
/// open ring for one that is not signed in.
fn status_dot(status: AccountStatus, t: &Tokens) -> Div {
    let c = &t.colors;
    let dot = div()
        .size(t.component.worker_dot)
        .flex_none()
        .rounded(t.radius.full);
    match status {
        AccountStatus::SignedIn | AccountStatus::Ready => dot.bg(c.status_done),
        AccountStatus::Expired | AccountStatus::Error => dot.bg(c.status_failed),
        AccountStatus::Installing => dot.bg(c.status_working),
        AccountStatus::NotSignedIn | AccountStatus::Unknown => {
            dot.border(t.stroke).border_color(c.text_tertiary)
        }
    }
}

impl ConsortApp {
    /// The Tags row (user tags, removable, and Add tag) and the Detected row
    /// under the worker header.
    pub fn worker_tags(
        &mut self,
        w: &WorkerView,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let label = |name: &str| {
            text(name.to_owned(), detail(t), c.text_tertiary)
                .w(t.component.tag_label)
                .flex_none()
        };
        let required: Vec<String> = self
            .state
            .channels
            .iter()
            .flat_map(|ch| ch.tags.iter().cloned())
            .collect();
        let mut user = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .child(label("Tags"));
        for tag in &w.tags {
            let strong = required.contains(tag);
            let worker_id = w.id.clone();
            let remove = tag.clone();
            let current = w.tags.clone();
            user = user.child(
                tag_chip(tag, strong, t).pr(t.space.xs).child(
                    div()
                        .id(SharedString::from(format!("remove-tag-{}-{tag}", w.id)))
                        .flex_none()
                        .rounded(t.radius.sm)
                        .cursor_pointer()
                        .hover(|s| s.bg(c.hover_canvas))
                        .child(icon(
                            "x",
                            t.space.md,
                            if strong {
                                c.accent_text
                            } else {
                                c.text_tertiary
                            },
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let tags: Vec<String> =
                                current.iter().filter(|t| **t != remove).cloned().collect();
                            this.save_worker_tags(worker_id.clone(), tags, false, window, cx);
                        })),
                ),
            );
        }
        let entering = self.tag_entry.as_deref() == Some(w.id.as_str());
        if entering {
            let focused = self.inputs.tag.read(cx).focus_handle(cx).is_focused(window);
            user = user.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.xs)
                    .w(t.component.settings_select)
                    .py(t.space.xxs)
                    .px(t.space.sm)
                    .rounded(t.radius.sm)
                    .border(t.border_width)
                    .border_color(if focused {
                        c.accent_text
                    } else {
                        c.border_strong
                    })
                    .bg(c.surface_card)
                    .child(crate::ui::parts::input(&self.inputs.tag, detail(t)).flex_1())
                    .child(
                        div()
                            .id("tag-entry-close")
                            .cursor_pointer()
                            .child(icon("x", t.space.md, c.text_tertiary))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_tag_entry(window, cx)),
                            ),
                    ),
            );
        } else if w.online {
            let id = w.id.clone();
            user = user.child(
                focusable(div().id("add-tag"), t)
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(t.space.xs)
                    .py(t.space.xxs)
                    .px(t.space.sm)
                    .rounded(t.radius.sm)
                    .border(t.border_width)
                    .border_dashed()
                    .border_color(c.border_strong)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover_canvas))
                    .child(icon("plus", t.space.md - t.space.xxs, c.text_tertiary))
                    .child(text("Add tag", detail(t), c.text_tertiary))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.tag_entry = Some(id.clone());
                        this.tag_error = None;
                        this.inputs.tag.update(cx, |input, cx| {
                            input.set_value("", window, cx);
                            input.focus(window, cx);
                        });
                        cx.notify();
                    })),
            );
        }
        let mut rows = div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .pt(t.space.xs + t.space.xxs)
            .child(user);
        if entering {
            // Completions: every worker tag that this worker does not have
            // yet (as a user tag or a detected tag), that starts with what the
            // user typed. With none, the Choose row stays hidden.
            let typed = self.inputs.tag.read(cx).value().trim().to_lowercase();
            let completions: Vec<String> = w
                .suggested_tags(&self.state.tags)
                .into_iter()
                .filter(|tag| tag.starts_with(&typed))
                .take(TAG_COMPLETIONS)
                .cloned()
                .collect();
            if !completions.is_empty() {
                let mut line = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(t.space.xs + t.space.xxs)
                    .child(div().w(t.component.tag_label).flex_none())
                    .child(text("Choose", detail(t), c.text_tertiary));
                for tag in completions {
                    let pick = tag.clone();
                    line = line.child(
                        div()
                            .id(SharedString::from(format!("tag-completion-{tag}")))
                            .cursor_pointer()
                            .hover(|s| s.opacity(t.opacity.danger_hover))
                            .child(tag_chip(&tag, false, t))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_worker_tag(Some(pick.clone()), window, cx)
                            })),
                    );
                }
                rows = rows.child(line);
            }
        }
        if let Some((worker, message)) = &self.tag_error
            && worker == &w.id
        {
            rows = rows.child(
                div()
                    .flex()
                    .gap(t.space.xs + t.space.xxs)
                    .child(div().w(t.component.tag_label).flex_none())
                    .child(
                        crate::ui::parts::selectable(
                            SharedString::from(format!("tag-error-{}", w.id)),
                            message.clone(),
                            detail(t),
                            c.status_failed,
                        )
                        .flex_1(),
                    ),
            );
        }
        if !w.detected_tags.is_empty() {
            let mut detected = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .child(label("Detected"));
            for tag in &w.detected_tags {
                detected = detected.child(
                    div()
                        .flex_none()
                        .py(t.space.xxs)
                        .px(t.space.sm)
                        .rounded(t.radius.sm)
                        .bg(c.surface_card)
                        .font_family(t.font_mono.clone())
                        .type_style(t.text.caption)
                        .text_color(c.text_secondary)
                        .child(tag.clone()),
                );
            }
            rows = rows.child(detected);
        }
        rows
    }

    /// Adds a tag to the worker whose Add tag field is open: `tag`, or the
    /// text in the field.
    pub fn add_worker_tag(
        &mut self,
        tag: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(worker_id) = self.tag_entry.clone() else {
            return;
        };
        let tag = tag.unwrap_or_else(|| self.inputs.tag.read(cx).value().trim().to_owned());
        if tag.is_empty() {
            return;
        }
        let Some(mut tags) = self.state.worker(&worker_id).map(|w| w.tags.clone()) else {
            return;
        };
        if !tags.contains(&tag) {
            tags.push(tag);
        }
        self.save_worker_tags(worker_id, tags, true, window, cx);
    }

    /// Sends the user tags of a worker. The hub checks them, and its
    /// `invalid_tag` answer shows under the Tags row. The typed tag stays in
    /// the field until the hub takes it, so the user can fix it.
    pub fn save_worker_tags(
        &mut self,
        worker_id: String,
        tags: Vec<String>,
        clear_entry: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tag_error = None;
        let client = self.client.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = client.set_worker_tags(worker_id.clone(), tags).await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) if clear_entry => this
                        .inputs
                        .tag
                        .update(cx, |input, cx| input.set_value("", window, cx)),
                    Ok(()) => {}
                    Err(consort_client::ClientError::Hub { code, message })
                        if code == "invalid_tag" =>
                    {
                        this.tag_error = Some((worker_id, message));
                    }
                    Err(err) => this.show_error(err.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The Accounts section: one row per account, with its logo, name, plan
    /// or problem, status and one action. A right click opens the row menu.
    pub fn accounts_section(&mut self, w: &WorkerView, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let online = w.online;
        let mut list = row_list(t);
        let last = w.accounts.len().saturating_sub(1);
        for (i, account) in w.accounts.iter().enumerate() {
            list = list.child(self.account_row(w, account, i == last, t, cx));
        }
        if w.accounts.is_empty() {
            list = list.child(div().py(t.space.md).px(t.space.lg).child(text(
                format!("{} has no accounts yet. Add one to run tasks here.", w.name),
                t.text.ui,
                c.text_secondary,
            )));
        }
        let worker_id = w.id.clone();
        let label_color = if online {
            c.text_primary
        } else {
            c.text_tertiary
        };
        let add = button("add-account", "Add account", ButtonKind::Secondary, t)
            .child(icon("plus", t.icon.dense, label_color))
            .flex_row_reverse()
            .when(!online, |b| b.text_color(label_color).cursor_default())
            .when(online, |b| {
                b.on_click(cx.listener(move |this, _, window, cx| {
                    this.open_dialog(Dialog::AddAccount(worker_id.clone()), window, cx)
                }))
            });
        let description = if online {
            format!(
                "Each account has its own sign-in and profile folder on {}.",
                w.name
            )
        } else {
            format!(
                "{} is offline. You can change its accounts when it is online again.",
                w.name
            )
        };
        worker_section(
            "Accounts",
            text(description, t.text.ui, c.text_tertiary).into_any_element(),
            Some(add.into_any_element()),
            list,
            t,
        )
    }

    fn account_row(
        &mut self,
        w: &WorkerView,
        account: &HarnessAccountView,
        last: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let c = &t.colors;
        let online = w.online;
        let quiet = matches!(
            account.status,
            AccountStatus::NotSignedIn | AccountStatus::Unknown
        );
        // The detail is Markdown with the code words in backticks, so the
        // reader can select and copy them.
        let detail_source =
            crate::ui::settings::detail_markdown(&account.detail, &account.detail_code);
        let detail_state = self.texts.get(
            &format!("account.{}.{}", w.id, account.id),
            &detail_source,
            cx,
        );
        // Text from a worker is not trusted: a link in it goes through the
        // same policy as a link in a message.
        let weak = cx.weak_entity();
        let account_detail = div()
            .w_full()
            .min_w_0()
            .type_style(detail(t))
            .text_color(c.text_tertiary)
            .child(TextView::new(&detail_state).max_lines(3).on_link_click(
                move |url, event, window, cx| {
                    if !crate::ui::timeline::is_open_click(event) {
                        return;
                    }
                    let action = crate::ui::links::click_action(&detail_source, url);
                    let _ = weak.update(cx, |this, cx| this.follow_link(action, window, cx));
                },
            ));
        let status = account.status;
        let status_color = if status.is_problem() {
            c.status_failed
        } else if status.works() {
            c.text_secondary
        } else {
            c.text_tertiary
        };
        let (worker_id, account_id) = (w.id.clone(), account.id.clone());
        let action_id = format!("account-action-{}", account.id);
        let action: AnyElement = match account.action {
            AccountAction::None => div().into_any_element(),
            _ if !online => {
                text(account.action.label(), t.text.ui, c.text_tertiary).into_any_element()
            }
            AccountAction::SignOut => row_link(action_id, "Sign out".into(), t.text.ui, t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let client = this.client.clone();
                    let (w, a) = (worker_id.clone(), account_id.clone());
                    this.spawn_command(cx, async move { client.sign_out_account(w, a).await });
                }))
                .into_any_element(),
            AccountAction::SignIn => row_link(action_id, "Sign in".into(), t.text.ui, t)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_sign_in(worker_id.clone(), account_id.clone(), window, cx)
                }))
                .into_any_element(),
            AccountAction::SignInAgain => row_button(action_id, "Sign in again".into(), t)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_sign_in(worker_id.clone(), account_id.clone(), window, cx)
                }))
                .into_any_element(),
        };
        let (menu_worker, menu_account) = (w.id.clone(), account.id.clone());
        let (settings_worker, settings_account) = (w.id.clone(), account.id.clone());
        let can_sign_out = matches!(status, AccountStatus::SignedIn | AccountStatus::Ready);
        list_row(last, t)
            .id(SharedString::from(format!("account-row-{}", account.id)))
            .child(
                div()
                    .flex_none()
                    .when(quiet, |d| d.opacity(t.opacity.answered_dim))
                    .child(provider_logo(
                        &account.logo,
                        t.component.provider_logo,
                        c.text_primary,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        // The name opens the account's settings: its name
                        // and its provider extras.
                        focusable(
                            div().id(SharedString::from(format!(
                                "account-settings-{}",
                                account.id
                            ))),
                            t,
                        )
                        .rounded(t.radius.sm)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .child(text(
                            account.label.clone(),
                            t.text.ui_strong,
                            if quiet {
                                c.text_secondary
                            } else {
                                c.text_primary
                            },
                        ))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_dialog(
                                    crate::app::Dialog::AccountSettings(
                                        settings_worker.clone(),
                                        settings_account.clone(),
                                    ),
                                    window,
                                    cx,
                                )
                            },
                        )),
                    )
                    .child(account_detail),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(t.space.xs + t.space.xxs)
                    .w(t.component.account_state)
                    .child(status_dot(status, t))
                    .child(text(status.label(), detail(t), status_color)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    .w(t.component.account_action)
                    .child(action),
            )
            .when(online, |row| {
                row.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        let items = account_menu(&menu_worker, &menu_account, can_sign_out);
                        this.open_menu(OpenMenu::at(event.position, items), cx);
                        cx.stop_propagation();
                    }),
                )
            })
    }

    /// The Installed tools section: each tool's logo, version and state,
    /// with Check now.
    pub fn tools_section(&mut self, w: &WorkerView, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let online = w.online;
        let mut list = row_list(t);
        let last = w.tools.len().saturating_sub(1);
        for (i, tool) in w.tools.iter().enumerate() {
            list = list.child(self.tool_row(w, tool, i == last, t, cx));
        }
        if w.tools.is_empty() {
            list = list.child(div().py(t.space.md).px(t.space.lg).child(text(
                format!(
                    "No tools yet. {} installs a provider when an account needs it.",
                    w.name
                ),
                t.text.ui,
                c.text_secondary,
            )));
        }
        let worker_id = w.id.clone();
        let check = div().py(t.space.xs).child(
            row_link("check-tools", "Check now".into(), t.text.ui_strong, t)
                .when(!online, |b| b.opacity(t.opacity.answered_dim))
                .when(online, |b| {
                    b.on_click(cx.listener(move |this, _, _, cx| {
                        let client = this.client.clone();
                        let id = worker_id.clone();
                        this.spawn_command(cx, async move { client.check_tools(id).await });
                        this.show_notice("Checking for updates.".into(), cx);
                    }))
                }),
        );
        let description = div()
            .flex()
            .flex_wrap()
            .type_style(t.text.ui)
            .text_color(c.text_tertiary)
            .child(format!("{} keeps its own copies in\u{a0}", w.name))
            .child(
                crate::ui::parts::selectable(
                    SharedString::from(format!("tools-folder-{}", w.id)),
                    w.tools_folder.clone(),
                    t.text.ui,
                    c.text_tertiary,
                )
                .font_family(t.font_mono.clone()),
            )
            .child("\u{a0}and checks for updates every day.");
        worker_section(
            "Installed tools",
            description.into_any_element(),
            Some(check.into_any_element()),
            list,
            t,
        )
    }

    fn tool_row(
        &mut self,
        w: &WorkerView,
        tool: &ToolView,
        last: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let missing = tool.state == ToolState::Missing;
        let label = tool.action_label();
        let action_id = format!("tool-{}-{}", w.id, tool.id);
        let (worker_id, tool_id) = (w.id.clone(), tool.id.clone());
        let install = move |this: &mut ConsortApp, cx: &mut Context<ConsortApp>| {
            let client = this.client.clone();
            let (w, tool) = (worker_id.clone(), tool_id.clone());
            this.spawn_command(cx, async move { client.install_tool(w, tool).await });
        };
        let state: AnyElement = match tool.state {
            _ if tool.can_install() && !w.online => {
                text(label, detail(t), c.text_tertiary).into_any_element()
            }
            ToolState::UpdateAvailable => row_button(action_id, label, t)
                .on_click(cx.listener(move |this, _, _, cx| install(this, cx)))
                .into_any_element(),
            ToolState::Missing | ToolState::Failed => row_link(action_id, label, detail(t), t)
                .on_click(cx.listener(move |this, _, _, cx| install(this, cx)))
                .into_any_element(),
            ToolState::Installing => div()
                .flex()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .child(status_glyph(
                    SharedString::from(format!("installing-{}", tool.id)),
                    consort_client::TaskStatus::Working,
                    t.icon.dense,
                    t,
                    cx,
                ))
                .child(text(label, detail(t), c.text_tertiary))
                .into_any_element(),
            ToolState::Ready => text(label, detail(t), c.text_tertiary).into_any_element(),
        };
        let name = text(
            tool.name.clone(),
            t.text.ui_strong,
            if missing {
                c.text_secondary
            } else {
                c.text_primary
            },
        );
        list_row(last, t)
            .child(
                div()
                    .flex_none()
                    .when(missing, |d| d.opacity(t.opacity.answered_dim))
                    .child(provider_logo(
                        &tool.logo,
                        t.component.provider_logo,
                        c.text_primary,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(name)
                    .when_some(tool.detail.clone(), |d, line| {
                        d.child(text(
                            line,
                            detail(t),
                            if tool.state == ToolState::Failed {
                                c.status_failed
                            } else {
                                c.text_tertiary
                            },
                        ))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .w(t.component.tool_version)
                    .font_family(t.font_mono.clone())
                    .type_style(t.text.label)
                    .font_weight(t.text.caption.weight)
                    .line_height(t.text.ui.line)
                    .text_color(c.text_secondary)
                    .child(tool.version.clone().unwrap_or_default()),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    .w(t.component.tool_state)
                    .child(state),
            )
    }
}
