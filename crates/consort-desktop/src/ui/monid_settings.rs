//! Monid in Settings > Connectors: the row under the list, the Monid page,
//! and the Add Monid dialog (ADR 0016).
//!
//! The row sits under the connections, with the Composio block. A click on
//! it opens the page. The page has the key, Use in, the approvals, the
//! spending limits, what the assistant sees, and what each channel spent
//! this month.

use consort_client::{
    ApprovalRuleView, LoadState, ServiceLogo,
    monid::{MONID_SITE, MONID_WALLET_URL, MonidUnattendedView, money_text},
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Dialog},
    extensions::UseInTarget,
    monid::{ADD_RULE_SELECT, HUB_RULE_SELECT},
    ui::{
        add_account::AccountDialogParts,
        admin_parts::select,
        dialogs::{field_block, input_box},
        ext_parts::{
            hint, logo_tile, mono_block, page_section, rows_frame, service_logo, setting_row,
            use_in_control,
        },
        parts::{ButtonKind, button, detail, error_box, focusable, icon, selectable, text},
        settings::segmented,
    },
};

/// The logo of Monid: a monogram tile.
pub fn monid_logo() -> ServiceLogo {
    ServiceLogo {
        monogram: "$".to_owned(),
        ..ServiceLogo::default()
    }
}

impl ConsortApp {
    /// The Monid block under the list of connections: the title, Change key,
    /// and the balance and the spend of the month. A click on the block
    /// opens the page.
    pub fn monid_footer(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let monid = &self.state.monid;
        let key_present = monid.key_present;
        let line_text = match (&monid.load, key_present) {
            (LoadState::Unsupported(m), _) | (LoadState::Failed(m), _) => m.clone(),
            (_, false) => {
                "No key yet. Add your Monid key to give the assistant paid tools.".to_owned()
            }
            (_, true) => monid.error.clone().unwrap_or_else(|| monid.footer_detail()),
        };
        let selected = self.monid_ui.page && key_present;
        let action = if key_present { "Change key" } else { "Add key" };
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
                    .child(
                        focusable(div().id("monid-open-page"), t)
                            .debug_selector(|| "monid-open-page".into())
                            .rounded(t.radius.sm)
                            .when(key_present, |d| d.cursor_pointer())
                            .type_style(t.text.label)
                            .text_color(if selected {
                                c.text_primary
                            } else {
                                c.text_secondary
                            })
                            .child("Monid")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.state.monid.key_present {
                                    this.select_monid_page(window, cx);
                                } else {
                                    this.open_monid_key_dialog(false, window, cx);
                                }
                            })),
                    )
                    .child(
                        focusable(div().id("monid-change-key"), t)
                            .debug_selector(|| "monid-change-key".into())
                            .rounded(t.radius.sm)
                            .cursor_pointer()
                            .type_style(detail(t))
                            .text_color(c.text_tertiary)
                            .hover(|s| s.text_color(c.text_primary))
                            .child(action)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_monid_key_dialog(false, window, cx)
                            })),
                    ),
            )
            .child(text(
                line_text,
                consort_theme::Type {
                    line: t.text.caption.line,
                    ..t.text.caption
                },
                if monid.error.is_some() {
                    c.status_failed
                } else {
                    c.text_tertiary
                },
            ))
    }

    /// The first row of the list in Settings > Connectors. It opens the Monid
    /// page. Only a hub with a key has it.
    pub fn monid_list_row(
        &self,
        is_selected: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = &t.colors;
        let monid = &self.state.monid;
        let scope = consort_client::monid::use_in_text(&monid.settings, &self.state.channels);
        let detail_text = match monid.balance_text() {
            Some(balance) => format!("Balance {balance}, {scope}"),
            None => scope,
        };
        focusable(div().id("monid-list-row"), t)
            .debug_selector(|| "monid-list-row".into())
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
                    .child(service_logo(&monid_logo(), t.icon.size, t)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(text("Monid", t.text.ui, c.text_primary))
                    .child(text(
                        detail_text,
                        detail(t),
                        if is_selected {
                            c.text_secondary
                        } else {
                            c.text_tertiary
                        },
                    )),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.select_monid_page(window, cx);
            }))
    }

    /// The Monid page of Settings > Connectors.
    pub fn render_monid_page(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let state = self.state.clone();
        let monid = &state.monid;
        let settings = &monid.settings;
        let header = div()
            .flex()
            .items_start()
            .gap(t.space.lg)
            .child(logo_tile(&monid_logo(), t))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.xs + t.space.xxs)
                    .child(text("Monid", t.text.heading, c.text_primary))
                    .child(text(monid.page_subtitle(), t.text.ui, c.text_secondary).min_w_0()),
            )
            .child(
                div().flex_none().child(
                    button("monid-add-money", "Add money", ButtonKind::Secondary, t)
                        .child(icon("external-link", t.icon.dense, c.text_secondary))
                        .flex_row_reverse()
                        .debug_selector(|| "monid-add-money".into())
                        .on_click(cx.listener(|this, _, _, cx| this.open_monid_wallet(cx))),
                ),
            );
        let scope = rows_frame(t)
            .child(setting_row(
                "Key",
                hint(
                    "The key stays on the hub. Workers and sessions never get it.",
                    t,
                ),
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .child(
                        div()
                            .font_family(t.font_mono.clone())
                            .type_style(t.text.mono)
                            .text_color(c.text_secondary)
                            .child(monid.key_text()),
                    )
                    .child(
                        button("monid-key-change", "Change key", ButtonKind::Secondary, t)
                            .debug_selector(|| "monid-key-change".into())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_monid_key_dialog(false, window, cx)
                            })),
                    ),
                false,
                t,
            ))
            .child(setting_row(
                "Use in",
                hint(
                    format!(
                        "Used in {}.",
                        consort_client::monid::use_in_text(settings, &state.channels)
                    ),
                    t,
                ),
                use_in_control(UseInTarget::Monid, &settings.use_in, false, t, self, cx),
                true,
                t,
            ));
        let rule_options: Vec<(String, String, bool)> = ApprovalRuleView::ALL
            .iter()
            .rev()
            .map(|r| {
                (
                    r.label().to_owned(),
                    r.key().to_owned(),
                    *r == settings.approval,
                )
            })
            .collect();
        let each_call = select(
            HUB_RULE_SELECT,
            settings.approval.label().to_owned(),
            t.component.group_select,
            rule_options,
            self,
            t,
            cx,
        );
        let unattended = |id: &'static str,
                          current: MonidUnattendedView,
                          routines: bool,
                          t: &Tokens,
                          cx: &mut Context<ConsortApp>| {
            let options: Vec<(MonidUnattendedView, &str)> = MonidUnattendedView::ALL
                .iter()
                .map(|v| (*v, v.label()))
                .collect();
            segmented(
                id,
                &options,
                current,
                t,
                cx,
                |_| t.text.ui,
                move |this, value, _, cx| this.set_monid_unattended(routines, value, cx),
            )
        };
        let approvals = rows_frame(t)
            .child(setting_row(
                "Each call",
                hint("The rule for a channel that has no rule of its own.", t),
                each_call,
                false,
                t,
            ))
            .child(setting_row(
                "Routines",
                hint(
                    "A routine has no one to ask, so its run waits in Needs input.",
                    t,
                ),
                unattended("monid-routines", settings.routines, true, t, cx),
                false,
                t,
            ))
            .child(setting_row(
                "Proactive turns",
                hint("Turns that start with no message from you.", t),
                unattended("monid-proactive", settings.proactive, false, t, cx),
                true,
                t,
            ));
        let limit_rows: [(&str, &str, _); 4] = [
            (
                "Per call",
                "The most that one call may cost.",
                self.monid_inputs.per_call.clone(),
            ),
            (
                "Per task",
                "The most that one task may spend.",
                self.monid_inputs.per_task.clone(),
            ),
            (
                "Per channel each month",
                "The most that one channel may spend in a month.",
                self.monid_inputs.per_channel.clone(),
            ),
            (
                "Per hub each month",
                "The most that all channels may spend in a month.",
                self.monid_inputs.per_hub.clone(),
            ),
        ];
        let mut limits = rows_frame(t);
        let count = limit_rows.len();
        for (i, (title, help, input)) in limit_rows.into_iter().enumerate() {
            let focused = {
                use gpui_kit::Focusable as _;
                input.read(cx).focus_handle(cx).is_focused(window)
            };
            limits = limits.child(setting_row(
                title,
                hint(help, t),
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(text("$", t.text.ui, c.text_tertiary))
                    .child(
                        div()
                            .w(t.component.connection_field)
                            .child(input_box(&input, focused, t)),
                    ),
                i + 1 == count,
                t,
            ));
        }
        let limits_block = div()
            .flex()
            .flex_col()
            .gap(t.space.sm)
            .child(limits)
            .children(
                self.monid_ui
                    .limits_error
                    .clone()
                    .map(|message| error_box("monid-limits-error", message, t)),
            );
        let sees = mono_block(
            "monid-assistant-sees",
            consort_client::MONID_PROMPT_BLOCK.to_owned(),
            t,
        );
        let mut spent = rows_frame(t);
        if monid.spend.is_empty() {
            spent = spent.child(
                div()
                    .py(t.space.md)
                    .px(t.space.lg)
                    .child(hint("No channel has spent anything this month.", t)),
            );
        }
        let rows = monid.spend.len();
        for (i, row) in monid.spend.iter().enumerate() {
            spent = spent.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.lg)
                    .py(t.space.md)
                    .px(t.space.lg)
                    .when(i + 1 < rows, |d| {
                        d.border_b(t.border_width).border_color(c.border_subtle)
                    })
                    .child(
                        selectable(
                            SharedString::from(format!("monid-spend-{}", row.channel_id)),
                            row.channel_name.clone(),
                            t.text.ui,
                            c.text_primary,
                        )
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        text(row.calls_label(), t.text.ui, c.text_tertiary)
                            .flex_none()
                            .whitespace_nowrap(),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(t.layout.speaker_gutter + t.space.xl)
                            .flex()
                            .justify_end()
                            .font_family(t.font_mono.clone())
                            .type_style(t.text.mono)
                            .text_color(c.text_primary)
                            .child(money_text(row.cost_micros)),
                    ),
            );
        }
        div()
            .id("monid-page")
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
                        "The assistant has Monid in these channels.",
                        scope,
                        t,
                    ))
                    .child(page_section(
                        "Approvals",
                        "Off, ask or run without asking. When it asks, the run waits in Needs input.",
                        approvals,
                        t,
                    ))
                    .child(page_section(
                        "Spending limits",
                        "None by default. Over a limit, Consort asks you, also in Full access. It does not end the work.",
                        limits_block,
                        t,
                    ))
                    .child(page_section(
                        "What the assistant sees",
                        "Every session starts with this note. The tools stay out of the context until the assistant asks for them.",
                        sees,
                        t,
                    ))
                    .child(page_section(
                        "Spent this month",
                        &format!("{} in all.", money_text(monid.spent_month_micros)),
                        spent,
                        t,
                    )),
            )
            .into_any_element()
    }

    /// Opens the wallet of Monid in the browser. The demo has no browser.
    pub fn open_monid_wallet(&mut self, cx: &mut Context<Self>) {
        if self.demo {
            self.show_notice(
                format!("This is a demo, so {MONID_SITE} does not open."),
                cx,
            );
        } else {
            cx.open_url(MONID_WALLET_URL);
        }
    }

    /// The Add Monid dialog, and the dialog that changes or removes the key.
    pub fn monid_key_dialog(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AccountDialogParts {
        let c = &t.colors;
        let focused = {
            use gpui_kit::Focusable as _;
            self.monid_inputs
                .key
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        };
        let monid = self.state.monid.clone();
        let busy = self.dialog_busy();
        let save: crate::app::DialogConfirm =
            std::rc::Rc::new(|this, window, cx| this.save_monid_key(window, cx));
        self.dialog_confirm.replace(Some(save.clone()));
        let adding = !monid.key_present;
        let form = self.monid_ui.add.clone();
        let mut body = div().flex().flex_col().gap(t.space.lg).child(field_block(
            "API key",
            input_box(&self.monid_inputs.key, focused, t),
            Some(if adding {
                "Create a key at app.monid.ai. The hub encrypts it, and workers and sessions never get it."
            } else {
                "The hub keeps the key. The new key replaces the old one when you save."
            }),
            t,
        ));
        if adding {
            let options: Vec<(String, String, bool)> = ApprovalRuleView::ALL
                .iter()
                .rev()
                .map(|r| {
                    (
                        r.label().to_owned(),
                        r.key().to_owned(),
                        *r == form.approval,
                    )
                })
                .collect();
            body = body
                .child(field_block(
                    "Use in",
                    use_in_control(UseInTarget::MonidDialog, &form.use_in, true, t, self, cx),
                    None,
                    t,
                ))
                .child(field_block(
                    "Approvals start as",
                    select(
                        ADD_RULE_SELECT,
                        form.approval.label().to_owned(),
                        t.component.group_select,
                        options,
                        self,
                        t,
                        cx,
                    ),
                    Some("No spending limits. You can set them later."),
                    t,
                ));
        }
        let from_catalog = form.from_catalog;
        let actions = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .when(!adding, |d| {
                d.child(
                    button("remove-monid-key", "Remove key", ButtonKind::Danger, t).on_click(
                        cx.listener(|this, _, window, cx| this.remove_monid_key(window, cx)),
                    ),
                )
            })
            .child(div().flex_1().min_w_0())
            .child(
                button(
                    "dialog-cancel",
                    if adding && from_catalog {
                        "Back"
                    } else {
                        "Cancel"
                    },
                    ButtonKind::Quiet,
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
                button(
                    "dialog-confirm",
                    if adding { "Add key" } else { "Save key" },
                    ButtonKind::Primary,
                    t,
                )
                .when(busy, |b| b.opacity(t.opacity.busy))
                .on_click(cx.listener(move |this, _, window, cx| save(this, window, cx))),
            );
        (
            if adding {
                "Add Monid".to_owned()
            } else {
                "Change the Monid key".to_owned()
            },
            "Monid gives the assistant paid tools from many providers, with one wallet. Each call asks you first, unless you say otherwise.".to_owned(),
            t.layout.dialog_width,
            body,
            actions,
            Some(logo_tile(&monid_logo(), t).into_any_element()),
        )
    }

    /// The hub's setting for routines or for proactive turns, as a section
    /// of its page: "In a routine" and "In proactive turns". `None` while
    /// the hub has no Monid key.
    pub fn monid_unattended_section(
        &mut self,
        routines: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if !self.state.monid.key_present {
            return None;
        }
        let settings = &self.state.monid.settings;
        let (title, help, current, id) = if routines {
            (
                "In a routine",
                "When it asks, the run waits in Needs input.",
                settings.routines,
                "monid-routine-setting",
            )
        } else {
            (
                "In proactive turns",
                "Turns that start with no message from you.",
                settings.proactive,
                "monid-proactive-setting",
            )
        };
        let options: Vec<(MonidUnattendedView, &str)> = MonidUnattendedView::ALL
            .iter()
            .map(|v| (*v, v.label()))
            .collect();
        let control = segmented(
            id,
            &options,
            current,
            t,
            cx,
            |_| t.text.ui,
            move |this, value, _, cx| this.set_monid_unattended(routines, value, cx),
        );
        Some(page_section(
            title,
            "",
            rows_frame(t).child(setting_row("Monid", hint(help, t), control, true, t)),
            t,
        ))
    }

    /// The Monid row of a routine: what it may do with Monid, and a way back
    /// to the hub's setting.
    pub fn routine_monid_row(
        &mut self,
        routine: &consort_client::routines::RoutineView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let hub = self.state.monid.settings.routines;
        let own = routine.monid;
        let current = own.unwrap_or(hub);
        let options: Vec<(MonidUnattendedView, &str)> = MonidUnattendedView::ALL
            .iter()
            .map(|v| (*v, v.label()))
            .collect();
        let routine_id = routine.id.clone();
        let control = segmented(
            "routine-monid",
            &options,
            current,
            t,
            cx,
            |_| t.text.ui,
            move |this, value, _, cx| this.set_routine_monid(routine_id.clone(), Some(value), cx),
        );
        let routine_id = routine.id.clone();
        let note = if own.is_some() {
            div().flex().child(
                button(
                    "routine-monid-hub",
                    "Use the hub's setting",
                    ButtonKind::Quiet,
                    t,
                )
                .debug_selector(|| "routine-monid-hub".into())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_routine_monid(routine_id.clone(), None, cx)
                })),
            )
        } else {
            div().child(text(
                format!("This is the hub's setting: {}.", hub.label()),
                detail(t),
                c.text_tertiary,
            ))
        };
        div()
            .flex()
            .flex_col()
            .gap(t.space.xs)
            .pt(t.space.xs)
            .child(text("Monid", detail(t), c.text_tertiary))
            .child(control)
            .child(note)
    }

    /// The note that Full access includes spending. It shows when the mode
    /// is Full access and the channel has Monid.
    pub fn monid_full_access_note(
        &self,
        channel_id: &str,
        mode: consort_client::ApprovalMode,
        t: &Tokens,
    ) -> Option<Div> {
        if mode != consort_client::ApprovalMode::FullAccess {
            return None;
        }
        let channel = self.state.channel(channel_id)?;
        let note = consort_client::monid::full_access_note(&self.state.monid, channel)?;
        let c = &t.colors;
        Some(
            div()
                .debug_selector(|| "monid-full-access-note".into())
                .flex()
                .flex_col()
                .gap(t.space.xs)
                .py(t.space.md)
                .px(t.space.lg)
                .rounded(t.radius.lg)
                .border(t.border_width)
                .border_color(c.border_subtle)
                .bg(c.surface_card)
                .child(text(note.title, t.text.ui_strong, c.text_primary))
                .child(
                    selectable(
                        SharedString::from(format!("monid-full-access-{channel_id}")),
                        note.body,
                        t.text.ui,
                        c.text_secondary,
                    )
                    .w_full(),
                ),
        )
    }

    /// The offer in the Add connector dialog: Monid gives paid tools with
    /// one wallet. It shows while the hub has no Monid key.
    pub fn monid_offer(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Option<Div> {
        if self.state.monid.key_present {
            return None;
        }
        let c = &t.colors;
        Some(
            div()
                .flex()
                .items_center()
                .gap(t.space.md)
                .py(t.space.sm + t.space.xxs)
                .px(t.space.md)
                .rounded(t.radius.lg)
                .border(t.border_width)
                .border_color(c.border_subtle)
                .bg(c.surface_card)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(text("Monid", t.text.ui_strong, c.text_primary))
                        .child(text(
                            "Paid tools from many providers, with one wallet. Each call asks you first.",
                            detail(t),
                            c.text_tertiary,
                        )),
                )
                .child(
                    button("offer-add-monid", "Add Monid", ButtonKind::Secondary, t)
                        .debug_selector(|| "offer-add-monid".into())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_monid_key_dialog(true, window, cx)
                        })),
                ),
        )
    }
}
