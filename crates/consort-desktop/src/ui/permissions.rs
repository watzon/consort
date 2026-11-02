//! The Permissions dialog (ADR 0020), its rule dialog and its tool picker.
//!
//! The dialog has the shell of the channel settings: the icon, the title,
//! two columns with a line between them, and Cancel and Save changes. The
//! left column has the command rules in groups. The right column has one
//! card for each connection that the channel has. Every edit is a draft in
//! `ConvoState::permissions` until Save changes.

use consort_client::{ApprovalRuleView, ChannelKind, ConnectionView, ToolKindView};
use consort_proto::model::{CommandRule, CommandRuleResult};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Pixels, SharedString, Stateful, StatefulInteractiveElement as _, Styled, Window,
    div, prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    permissions::{
        EXCEPTION_RULES, EXCEPTION_SELECT, PermissionsAction, RULE_SELECT, exception_candidates,
        exception_option, result_words, rule_groups, rule_note, rule_option, tutti_rules,
    },
    ui::{
        convo_dialogs::{DialogParts, choice, choice_bar, section},
        dialogs::{field_block, input_box},
        ext_parts::faded_scroll_up_to,
        menu::{MenuAction, MenuItem, OpenMenu},
        parts::{
            ButtonKind, button, detail, error_box, focusable, icon, icon_button, line, selectable,
            text,
        },
    },
};

/// The menu of a command rule row: Edit and Remove for a rule of the
/// channel, Override here for a rule from Tutti.
pub fn rule_menu(pattern: &str, from_tutti: bool) -> Vec<MenuItem> {
    let pattern = pattern.to_owned();
    if from_tutti {
        return vec![MenuItem::new(
            "Override here",
            MenuAction::Permissions(PermissionsAction::OverrideRule(pattern)),
        )];
    }
    vec![
        MenuItem::new(
            "Edit",
            MenuAction::Permissions(PermissionsAction::EditRule(pattern.clone())),
        ),
        MenuItem::new(
            "Remove",
            MenuAction::Permissions(PermissionsAction::RemoveRule(pattern)),
        )
        .danger()
        .separated(),
    ]
}

/// The menu of an exception row: Remove.
pub fn exception_menu(connection: &str, tool: &str) -> Vec<MenuItem> {
    vec![MenuItem::new(
        "Remove",
        MenuAction::Permissions(PermissionsAction::RemoveException(
            connection.to_owned(),
            tool.to_owned(),
        )),
    )]
}

/// The height that the columns of the Permissions dialog can have: the
/// window, less the margins of the dialog and its header and footer. The
/// columns are as high as their content up to this.
pub fn columns_height(viewport: Pixels, t: &Tokens) -> Pixels {
    let reserved = t.space.xxl * 2.0 + t.space.xl * 2.0 + t.space.page * 3.0;
    (viewport - reserved)
        .max(t.component.convo.icon_list_height)
        .min(t.component.convo.icon_list_height * 2.0)
}

/// A box that looks like a mono input.
fn mono_input_box(
    input: &gpui_kit::Entity<gpui_kit::component::input::InputState>,
    focused: bool,
    t: &Tokens,
) -> Div {
    input_box(input, focused, t).font_family(t.font_mono.clone())
}

/// The scrim and the card of a dialog that opens over the Permissions
/// dialog.
fn overlay(
    id: &'static str,
    card: Stateful<Div>,
    on_close: impl Fn(&mut ConsortApp, &mut Window, &mut Context<ConsortApp>) + 'static,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> AnyElement {
    div()
        .id(id)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(t.colors.scrim)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| on_close(this, window, cx)),
        )
        .child(card)
        .into_any_element()
}

fn card_shell(id: &'static str, width: Pixels, window: &Window, t: &Tokens) -> Stateful<Div> {
    let c = &t.colors;
    div()
        .id(id)
        .debug_selector(move || id.into())
        .occlude()
        .flex()
        .flex_col()
        .gap(t.space.md)
        .w(width)
        .max_w(window.viewport_size().width - t.space.page)
        .p(t.space.xl)
        .bg(c.surface_raised)
        .border(t.border_width)
        .border_color(c.border_strong)
        .rounded(t.radius.xl)
        .shadow(t.shadow_float())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

impl ConsortApp {
    /// A select that opens a menu under itself. The menu flips when there is
    /// no room below.
    fn permissions_select(
        &self,
        key: String,
        label: &str,
        items: Vec<MenuItem>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let c = &t.colors;
        let trigger = key.clone();
        focusable(div().id(SharedString::from(key.clone())), t)
            .debug_selector(move || key.clone())
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .w(t.component.group_select)
            .py(t.space.xs + t.space.xxs)
            .pl(t.space.md - t.space.xxs)
            .pr(t.space.sm + t.space.xxs)
            .rounded(t.radius.md)
            .border(t.border_width)
            .border_color(c.border_strong)
            .bg(c.surface_raised)
            .cursor_pointer()
            .hover(|s| s.border_color(c.text_tertiary))
            .child(self.triggers.track(trigger.clone()))
            .child(text(label.to_owned(), t.text.ui, c.text_primary).min_w_0())
            .child(icon("chevrons-up-down", t.icon.dense, c.text_secondary))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(bounds) = this.triggers.get(&trigger) {
                    this.open_menu(OpenMenu::under(bounds, items.clone(), true), cx);
                }
            }))
    }

    /// One command rule: the pattern, what it does, and its note.
    fn rule_row(
        &self,
        rule: &CommandRule,
        from_tutti: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let group = if from_tutti { "tutti" } else { "own" };
        let key = format!("permissions-rule-{group}-{}", rule.pattern);
        let menu_key = format!("{key}-more");
        let items = rule_menu(&rule.pattern, from_tutti);
        let right_click = items.clone();
        let description = if from_tutti {
            format!(
                "{}. {}",
                rule.description.trim_end_matches('.'),
                result_words(rule.result)
            )
        } else {
            rule.description.clone()
        };
        let note = rule_note(rule);
        div()
            .debug_selector({
                let key = key.clone();
                move || key.clone()
            })
            .flex()
            .items_start()
            .gap(t.space.sm)
            .py(t.space.sm)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_menu(OpenMenu::at(event.position, right_click.clone()), cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_family(t.font_mono.clone()).child(selectable(
                        SharedString::from(format!("{key}-pattern")),
                        rule.pattern.clone(),
                        t.text.mono,
                        c.text_primary,
                    )))
                    .child(text(description, t.text.ui, c.text_secondary))
                    .children(note.map(|n| text(n, detail(t), c.text_tertiary))),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .child(self.triggers.track(menu_key.clone()))
                    .child(
                        icon_button(
                            SharedString::from(menu_key.clone()),
                            "ellipsis",
                            t.space.xl,
                            t.icon.dense,
                            c.text_secondary,
                            c.hover_card,
                            t,
                            cx,
                        )
                        .debug_selector(move || menu_key.clone())
                        .on_click(cx.listener({
                            let menu_key = format!("{key}-more");
                            move |this, _, _, cx| {
                                if let Some(bounds) = this.triggers.get(&menu_key) {
                                    this.open_menu(
                                        OpenMenu::under(bounds, items.clone(), false),
                                        cx,
                                    );
                                }
                            }
                        })),
                    ),
            )
    }

    /// A group of rules with its heading.
    fn rule_group(
        &self,
        heading: &str,
        rules: &[CommandRule],
        from_tutti: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if rules.is_empty() {
            return None;
        }
        let mut group = div().flex().flex_col().child(text(
            heading.to_owned(),
            t.text.label,
            t.colors.text_secondary,
        ));
        for rule in rules {
            group = group.child(self.rule_row(rule, from_tutti, t, cx));
        }
        Some(group)
    }

    /// The card of one connection.
    fn connection_card(
        &self,
        connection: &ConnectionView,
        channel_id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let c = &t.colors;
        let draft = self
            .convo
            .permissions
            .form
            .connection(&connection.id)?
            .clone();
        let others = self.other_channels_of(connection, channel_id);
        let others: Vec<&str> = others.iter().map(String::as_str).collect();
        let mut card = div()
            .debug_selector({
                let id = format!("permissions-connection-{}", connection.id);
                move || id.clone()
            })
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .p(t.space.lg)
            .rounded(t.radius.lg)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .bg(c.surface_card);
        // The header: the name, and where else the account works.
        let (side, side_color) = if connection.assistant_owned {
            (Some("The assistant's account".to_owned()), c.accent_text)
        } else if others.is_empty() {
            (None, c.text_tertiary)
        } else {
            (Some(crate::permissions::also_in(&others)), c.text_tertiary)
        };
        card = card.child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .gap(t.space.md)
                .child(
                    line(connection.name.clone(), t.text.ui_strong, c.text_primary)
                        .flex_1()
                        .min_w_0(),
                )
                .children(side.map(|s| {
                    text(s, detail(t), side_color)
                        .flex_1()
                        .min_w_0()
                        .text_right()
                })),
        );
        for (kind, label) in [
            (ToolKindView::Read, "Read"),
            (ToolKindView::Change, "Change"),
            (ToolKindView::Delete, "Delete"),
        ] {
            let current = draft.approvals.rule_for(kind);
            let items = ApprovalRuleView::ALL
                .into_iter()
                .map(|r| {
                    MenuItem::new(
                        r.label(),
                        MenuAction::Select(RULE_SELECT, rule_option(&connection.id, kind, r)),
                    )
                    .checked(r == current)
                })
                .collect();
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(t.space.md)
                    .child(text(label, t.text.ui, c.text_secondary).flex_1().min_w_0())
                    .child(self.permissions_select(
                        format!("permissions-rule-{}-{label}", connection.id),
                        current.label(),
                        items,
                        t,
                        cx,
                    )),
            );
        }
        if !draft.exceptions.is_empty() {
            let mut list = div()
                .flex()
                .flex_col()
                .gap(t.space.sm)
                .pt(t.space.sm)
                .border_t(t.border_width)
                .border_color(c.border_subtle);
            for exception in &draft.exceptions {
                let items = EXCEPTION_RULES
                    .into_iter()
                    .map(|r| {
                        MenuItem::new(
                            r.label(),
                            MenuAction::Select(
                                EXCEPTION_SELECT,
                                exception_option(&connection.id, &exception.tool, r),
                            ),
                        )
                        .checked(r == exception.rule)
                    })
                    .collect();
                let menu = exception_menu(&connection.id, &exception.tool);
                let (connection_id, tool) = (connection.id.clone(), exception.tool.clone());
                let row_key = format!("permissions-exception-{}-{}", connection.id, exception.tool);
                list = list.child(
                    div()
                        .debug_selector({
                            let row_key = row_key.clone();
                            move || row_key.clone()
                        })
                        .flex()
                        .items_center()
                        .gap(t.space.sm)
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                this.open_menu(OpenMenu::at(event.position, menu.clone()), cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            line(exception.name.clone(), t.text.ui, c.text_primary)
                                .flex_1()
                                .min_w_0(),
                        )
                        .child(self.permissions_select(
                            format!("{row_key}-rule"),
                            exception.rule.label(),
                            items,
                            t,
                            cx,
                        ))
                        .child(
                            icon_button(
                                SharedString::from(format!("{row_key}-remove")),
                                "x",
                                t.space.xl,
                                t.icon.dense,
                                c.text_secondary,
                                c.hover_card,
                                t,
                                cx,
                            )
                            .debug_selector({
                                let id = format!("{row_key}-remove");
                                move || id.clone()
                            })
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.run_permissions_action(
                                        PermissionsAction::RemoveException(
                                            connection_id.clone(),
                                            tool.clone(),
                                        ),
                                        window,
                                        cx,
                                    )
                                },
                            )),
                        ),
                );
            }
            card = card.child(list);
        }
        let add_id = connection.id.clone();
        card = card.child(
            div().flex().child(
                focusable(
                    div().id(SharedString::from(format!(
                        "permissions-add-exception-{}",
                        connection.id
                    ))),
                    t,
                )
                .debug_selector({
                    let id = format!("permissions-add-exception-{}", connection.id);
                    move || id.clone()
                })
                .flex()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .py(t.space.xxs)
                .cursor_pointer()
                .child(icon("plus", t.icon.dense, c.text_secondary))
                .child(text("Add an exception", t.text.ui, c.text_secondary))
                .hover(|s| s.text_color(c.text_primary))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_permissions_action(
                        PermissionsAction::AddException(add_id.clone()),
                        window,
                        cx,
                    )
                })),
            ),
        );
        if connection.assistant_owned {
            card = card.child(text(
                "It sends without asking when its rule says so, also in routines. It does not use your voice.",
                detail(t),
                c.text_tertiary,
            ));
        }
        Some(card)
    }

    /// The Permissions dialog of a channel.
    pub(crate) fn permissions_parts(
        &mut self,
        id: &str,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<DialogParts> {
        let c = &t.colors;
        let channel = self.state.channel(id)?.clone();
        let tutti = channel.kind == ChannelKind::Tutti;
        let name = if tutti {
            crate::ui::TUTTI.to_owned()
        } else {
            channel.name.clone()
        };
        let height = columns_height(window.viewport_size().height, t);
        let rules = self.convo.permissions.form.rules.clone();
        let groups = rule_groups(&rules, tutti_rules(&self.state), tutti);

        // Commands.
        let mut list = div().flex().flex_col().gap(t.space.sm);
        if groups.is_empty() {
            list = list.child(
                div()
                    .debug_selector(|| "permissions-empty".into())
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(t.space.md)
                    .p(t.space.lg)
                    .rounded(t.radius.lg)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .bg(c.surface_card)
                    .child(text(
                        "No rules yet. Tell the assistant what it can do, such as: You can open PRs. Ask me before a merge.",
                        t.text.ui,
                        c.text_secondary,
                    ))
                    .child(
                        button("permissions-add-rule-empty", "Add a rule", ButtonKind::Secondary, t)
                            .debug_selector(|| "permissions-add-rule-empty".into())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run_permissions_action(PermissionsAction::AddRule, window, cx)
                            })),
                    ),
            );
        }
        for group in [
            self.rule_group("Runs without asking", &groups.allow, false, t, cx),
            self.rule_group(
                "Always asks, also in Full access",
                &groups.ask,
                false,
                t,
                cx,
            ),
            self.rule_group("From Tutti, in every channel", &groups.tutti, true, t, cx),
        ]
        .into_iter()
        .flatten()
        {
            list = list.child(group);
        }
        let commands = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(t.space.md)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(t.space.md)
                    .child(section("Commands", t).flex_1().min_w_0())
                    .when(!groups.is_empty(), |d| {
                        d.child(
                            button(
                                "permissions-add-rule",
                                "Add a rule",
                                ButtonKind::Secondary,
                                t,
                            )
                            .debug_selector(|| "permissions-add-rule".into())
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.run_permissions_action(
                                        PermissionsAction::AddRule,
                                        window,
                                        cx,
                                    )
                                },
                            )),
                        )
                    }),
            )
            .child(faded_scroll_up_to(
                "permissions-commands-list",
                &self.convo.permissions.commands_scroll.clone(),
                height,
                list,
                c.surface_raised,
                c.raised_clear,
                t,
                cx,
            ));

        // Connections.
        let mut cards = div().flex().flex_col().gap(t.space.md);
        let connections: Vec<ConnectionView> =
            crate::permissions::available_connections(&self.state, id)
                .into_iter()
                .cloned()
                .collect();
        if connections.is_empty() {
            cards = cards.child(text(
                "No connection is on in this channel. Turn one on in Connectors and skills.",
                t.text.ui,
                c.text_secondary,
            ));
        }
        for connection in &connections {
            cards = cards.children(self.connection_card(connection, id, t, cx));
        }
        let connections_column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(t.space.md)
            .child(section("Connections", t))
            .child(faded_scroll_up_to(
                "permissions-connections-list",
                &self.convo.permissions.connections_scroll.clone(),
                height,
                cards,
                c.surface_raised,
                c.raised_clear,
                t,
                cx,
            ));

        let body = div()
            .flex()
            .items_start()
            .gap(t.component.convo.settings_column_gap)
            .child(commands)
            .child(div().self_stretch().w(t.border_width).bg(c.border_subtle))
            .child(connections_column);

        let save: crate::app::DialogConfirm =
            std::rc::Rc::new(|this, window, cx| this.save_permissions(window, cx));
        self.dialog_confirm.replace(Some(save.clone()));
        let busy = self.dialog_busy();
        let footer = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                button("dialog-cancel", "Cancel", ButtonKind::Secondary, t)
                    .debug_selector(|| "dialog-cancel".into())
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            )
            .child(
                button("dialog-confirm", "Save changes", ButtonKind::Primary, t)
                    .debug_selector(|| "dialog-confirm".into())
                    .when(busy, |b| b.opacity(t.opacity.busy))
                    .on_click(cx.listener(move |this, _, window, cx| save(this, window, cx))),
            );
        let lead = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(t.space.xxl + t.space.sm)
            .rounded(t.radius.lg)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_strong)
            .child(if tutti {
                crate::ui::mark::still_mark(t.icon.size, t)
            } else {
                icon(&channel.icon, t.icon.size, c.text_secondary).into_any_element()
            })
            .into_any_element();
        Some((
            format!("{name} permissions"),
            "What the assistant can do without asking you. You can also tell the assistant, and it asks you first.".to_owned(),
            t.component.convo.dialog_settings,
            body,
            footer,
            Some(lead),
        ))
    }

    /// The Add a rule dialog, or `None` while it is closed.
    pub fn render_rule_dialog(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use gpui_kit::Focusable as _;
        let dialog = self.convo.permissions.rule.clone()?;
        let c = &t.colors;
        let focused =
            |input: &gpui_kit::Entity<gpui_kit::component::input::InputState>,
             cx: &gpui_kit::App| { input.read(cx).focus_handle(cx).is_focused(window) };
        let (pattern_focus, except_focus, description_focus) = (
            focused(&self.convo_inputs.rule_pattern, cx),
            focused(&self.convo_inputs.rule_except, cx),
            focused(&self.convo_inputs.rule_description, cx),
        );
        let mut when = choice_bar(t);
        for (result, label, id) in [
            (
                CommandRuleResult::Allow,
                "Runs without asking",
                "rule-result-allow",
            ),
            (CommandRuleResult::Ask, "Always asks", "rule-result-ask"),
        ] {
            when = when.child(
                choice(id, dialog.result == result, label, t)
                    .debug_selector(move || id.into())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(open) = this.convo.permissions.rule.as_mut() {
                            open.result = result;
                            cx.notify();
                        }
                    })),
            );
        }
        let card = card_shell("permissions-rule-dialog", t.layout.dialog_wide, window, t)
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(text(
                        dialog.title(),
                        t.text.title,
                        c.text_primary,
                    )))
                    .child(
                        icon_button(
                            "rule-dialog-close",
                            "x",
                            t.space.xl + t.space.xs,
                            t.icon.size,
                            c.text_secondary,
                            c.hover_raised,
                            t,
                            cx,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.close_rule_dialog(window, cx)),
                        ),
                    ),
            )
            .child(field_block(
                "Command",
                mono_input_box(&self.convo_inputs.rule_pattern, pattern_focus, t),
                Some("The program and the words after it."),
                t,
            ))
            .child(field_block("When it matches", when, None, t))
            .child(field_block(
                "Except",
                mono_input_box(&self.convo_inputs.rule_except, except_focus, t),
                Some("Words that stop the rule from matching. Separate them with commas."),
                t,
            ))
            .child(field_block(
                "What it does",
                input_box(&self.convo_inputs.rule_description, description_focus, t),
                None,
                t,
            ))
            .children(dialog.error.clone().map(|error| {
                error_box(
                    SharedString::from(format!("rule-dialog-error-{error}")),
                    error,
                    t,
                )
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(t.space.sm)
                    .child(
                        button("rule-dialog-cancel", "Cancel", ButtonKind::Secondary, t).on_click(
                            cx.listener(|this, _, window, cx| this.close_rule_dialog(window, cx)),
                        ),
                    )
                    .child(
                        button(
                            "rule-dialog-confirm",
                            dialog.confirm_label(),
                            ButtonKind::Primary,
                            t,
                        )
                        .debug_selector(|| "rule-dialog-confirm".into())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.save_rule_dialog(window, cx)),
                        ),
                    ),
            );
        Some(overlay(
            "permissions-rule-scrim",
            card,
            |this, window, cx| this.close_rule_dialog(window, cx),
            t,
            cx,
        ))
    }

    fn close_rule_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.convo.permissions.rule = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn close_exception_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.convo.permissions.picker = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// The tool picker of a connection, or `None` while it is closed.
    pub fn render_exception_picker(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use gpui_kit::Focusable as _;
        let picker = self.convo.permissions.picker.clone()?;
        let c = &t.colors;
        let connection = self.state.connection(&picker.connection_id)?.clone();
        let draft = self
            .convo
            .permissions
            .form
            .connection(&picker.connection_id)?
            .clone();
        let query = self
            .convo_inputs
            .exception_search
            .read(cx)
            .value()
            .to_string();
        let search_focused = self
            .convo_inputs
            .exception_search
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let search = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .py(t.component.input_pad_y)
            .px(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(if search_focused {
                c.accent_text
            } else {
                c.border_strong
            })
            .rounded(t.radius.md)
            .child(icon("search", t.icon.dense, c.text_tertiary))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .type_style(t.text.ui)
                    .text_color(c.text_primary)
                    .child(crate::ui::parts::input(
                        &self.convo_inputs.exception_search,
                        t.text.ui,
                    )),
            );
        let mut rows = div().flex().flex_col().gap(t.space.xxs);
        match self.state.tools_of(&connection.service) {
            Some(tools) if tools.load == consort_client::LoadState::Loaded => {
                let found = exception_candidates(tools, &draft, &query, &connection.service);
                if found.is_empty() {
                    rows = rows.child(text(
                        if query.trim().is_empty() {
                            "Every tool has an exception already."
                        } else {
                            "No tool matches. Try another word."
                        },
                        t.text.ui,
                        c.text_secondary,
                    ));
                }
                for tool in found {
                    let slug = tool.slug.clone();
                    let row_id = format!("permissions-tool-{}", tool.slug);
                    rows = rows.child(
                        focusable(div().id(SharedString::from(row_id.clone())), t)
                            .debug_selector({
                                let row_id = row_id.clone();
                                move || row_id.clone()
                            })
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(t.space.sm)
                            .py(t.space.xs + t.space.xxs)
                            .px(t.space.sm)
                            .rounded(t.radius.md)
                            .cursor_pointer()
                            .hover(|s| s.bg(c.hover_card))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(line(
                                        consort_proto::model::tool_display_name(
                                            &connection.service,
                                            &tool.slug,
                                        ),
                                        t.text.ui,
                                        c.text_primary,
                                    ))
                                    .when(!tool.description.is_empty(), |d| {
                                        d.child(line(
                                            tool.description.clone(),
                                            detail(t),
                                            c.text_tertiary,
                                        ))
                                    }),
                            )
                            .child(
                                text(tool.kind.label(), detail(t), c.text_tertiary)
                                    .flex_none()
                                    .whitespace_nowrap(),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.pick_exception_tool(&slug, window, cx)
                            })),
                    );
                }
            }
            Some(tools) if tools.load.error().is_some() => {
                rows = rows.child(text(
                    "Consort could not load the tools. Close this list and try again.",
                    t.text.ui,
                    c.status_failed,
                ));
            }
            _ => {
                rows = rows.child(text("Loading the tools.", t.text.ui, c.text_secondary));
            }
        }
        let card = card_shell(
            "permissions-picker",
            t.component.convo.dialog_small,
            window,
            t,
        )
        .child(
            div()
                .flex()
                .items_center()
                .child(div().flex_1().min_w_0().child(text(
                    format!("Add an exception to {}", connection.name),
                    t.text.title,
                    c.text_primary,
                )))
                .child(
                    icon_button(
                        "permissions-picker-close",
                        "x",
                        t.space.xl + t.space.xs,
                        t.icon.size,
                        c.text_secondary,
                        c.hover_raised,
                        t,
                        cx,
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.close_exception_picker(window, cx)),
                    ),
                ),
        )
        .child(text(
            "Choose a tool. It gets its own rule, which wins over the rule of its kind.",
            detail(t),
            c.text_tertiary,
        ))
        .child(search)
        .child(crate::ui::ext_parts::faded_scroll(
            "permissions-picker-list",
            &self.convo.permissions.picker_scroll.clone(),
            t.component.convo.icon_list_height,
            rows,
            c.surface_raised,
            c.raised_clear,
            t,
            cx,
        ))
        .child(
            div().flex().justify_end().child(
                button(
                    "permissions-picker-cancel",
                    "Cancel",
                    ButtonKind::Secondary,
                    t,
                )
                .on_click(
                    cx.listener(|this, _, window, cx| this.close_exception_picker(window, cx)),
                ),
            ),
        );
        Some(overlay(
            "permissions-picker-scrim",
            card,
            |this, window, cx| this.close_exception_picker(window, cx),
            t,
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(items: &[MenuItem]) -> Vec<String> {
        items.iter().map(|i| i.label.to_string()).collect()
    }

    #[test]
    fn the_rule_menus_keep_the_names_and_the_order() {
        assert_eq!(labels(&rule_menu("git push", false)), ["Edit", "Remove"]);
        assert_eq!(labels(&rule_menu("rm -rf", true)), ["Override here"]);
        assert_eq!(labels(&exception_menu("con_1", "GITHUB_X")), ["Remove"]);
        assert!(rule_menu("git push", false)[1].danger);
    }
}
