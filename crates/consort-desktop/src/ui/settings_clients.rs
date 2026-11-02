//! Settings > Clients (Paper `86L-0`) and the Pair a new app dialog
//! (`8L0-0`).

use consort_client::LoadState;
use consort_client::admin::{ClientRowView, countdown};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Bounds, Context, Div, Hsla, InteractiveElement as _, IntoElement, ParentElement,
    Pixels, SharedString, StatefulInteractiveElement as _, Styled, Window, canvas, div, fill,
    point, prelude::FluentBuilder as _, relative, size,
};

use crate::app::{ConsortApp, Dialog};
use crate::ui::admin_parts::{column, copy_button, header, page, rows};
use crate::ui::parts::{ButtonKind, button, detail, error_box, icon, status_glyph, text};

/// The modules of a QR code: its width and the dark modules, row by row.
pub fn qr_modules(data: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::with_error_correction_level(data, qrcode::EcLevel::M).ok()?;
    let width = code.width();
    let dark = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Some((width, dark))
}

/// Draws a QR code: light ground, dark modules, a quiet zone of two
/// modules.
fn qr_element(data: &str, side: Pixels, light: Hsla, dark: Hsla, radius: Pixels) -> AnyElement {
    let Some((width, modules)) = qr_modules(data) else {
        return div().size(side).into_any_element();
    };
    let quiet = 2;
    let cells = width + 2 * quiet;
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let cell = bounds.size.width / cells as f32;
            for y in 0..width {
                for x in 0..width {
                    if modules[y * width + x] {
                        let origin = point(
                            bounds.origin.x + cell * (x + quiet) as f32,
                            bounds.origin.y + cell * (y + quiet) as f32,
                        );
                        window.paint_quad(fill(Bounds::new(origin, size(cell, cell)), dark));
                    }
                }
            }
        },
    )
    .size(side)
    .bg(light)
    .rounded(radius)
    .into_any_element()
}

/// A QR code in the size of these pages. It needs dark modules on a light
/// ground in both modes.
pub fn qr_code(data: &str, t: &Tokens) -> AnyElement {
    let c = &t.colors;
    let (ground, ink) = if c.text_primary.l > c.surface_canvas.l {
        (c.text_primary, c.surface_canvas)
    } else {
        (c.surface_canvas, c.text_primary)
    };
    qr_element(data, t.component.admin.qr, ground, ink, t.radius.md)
}

impl ConsortApp {
    pub fn render_clients(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let view = self.state.admin.clients.clone();
        let pair = button("pair-new-app", "Pair a new app", ButtonKind::Secondary, t)
            .child(icon("plus", t.icon.dense, c.text_primary))
            .flex_row_reverse()
            .on_click(
                cx.listener(|this, _, window, cx| this.open_dialog(Dialog::PairClient, window, cx)),
            );
        let line = text(
            "Apps that can talk to the assistant through this hub. Revoke an app that you lost or do not use.",
            t.text.ui,
            c.text_secondary,
        );
        let mut col = column(t).child(header("Clients", line, Some(pair.into_any_element()), t));
        match &view.load {
            LoadState::Failed(message) if view.rows.is_empty() => {
                col = col.child(error_box("clients-error", message.clone(), t));
            }
            _ if view.rows.is_empty() => {
                col = col.child(text(
                    "Getting the paired apps.",
                    t.text.ui,
                    c.text_secondary,
                ));
            }
            _ => {
                let count = view.rows.len();
                let mut list = rows(t);
                for (i, client) in view.rows.iter().enumerate() {
                    list = list.child(self.client_row(client, i + 1 == count, t, cx));
                }
                col = col
                    .child(list)
                    .children(
                        view.error
                            .clone()
                            .map(|err| error_box("revoke-error", err, t)),
                    )
                    .child(
                        text(
                            "A client is an app that you use. Machines that run tasks are workers, and you pair them in Settings > Workers.",
                            detail(t),
                            c.text_tertiary,
                        )
                        .mt(-t.space.lg),
                    );
            }
        }
        page("clients-page", t).child(col).into_any_element()
    }

    fn client_row(
        &self,
        client: &ClientRowView,
        last: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let a = &t.component.admin;
        let confirming = self.admin.revoke_confirm.as_deref() == Some(client.id.as_str());
        let id = client.id.clone();
        let name = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .child(text(client.name.clone(), t.text.ui_strong, c.text_primary))
            .when(client.this_app, |d| {
                d.child(
                    div()
                        .px(t.space.xs + t.space.xxs)
                        .rounded(t.radius.sm)
                        .bg(c.surface_selected)
                        .child(text("This app", detail(t), c.text_secondary)),
                )
            });
        let status = div()
            .flex()
            .items_center()
            .flex_none()
            .w(a.status_col)
            .gap(t.space.xs + t.space.xxs)
            .child(crate::ui::admin_parts::status_dot(
                client.online.then_some(true),
                t,
            ))
            .child(text(client.status.clone(), detail(t), c.text_secondary));
        let action = div().flex().flex_none().justify_end().w(a.action_col).when(
            !client.this_app && !confirming,
            |d| {
                d.child(
                    button(
                        SharedString::from(format!("revoke-{}", client.id)),
                        "Revoke",
                        ButtonKind::Quiet,
                        t,
                    )
                    .px(t.space.xs)
                    .debug_selector({
                        let id = client.id.clone();
                        move || format!("revoke-{id}")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.admin.revoke_confirm = Some(id.clone());
                        cx.notify();
                    })),
                )
            },
        );
        let main = div()
            .flex()
            .items_center()
            .gap(a.row_gap)
            .py(t.space.md)
            .px(t.space.lg)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(a.client_tile)
                    .rounded(t.radius.md)
                    .bg(c.surface_card)
                    .child(icon("worker", a.client_icon, c.text_secondary)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(name)
                    .child(text(client.detail.clone(), detail(t), c.text_tertiary)),
            )
            .child(status)
            .child(action);
        let confirm_id = client.id.clone();
        div()
            .flex()
            .flex_col()
            .when(!last, |d| {
                d.border_b(t.border_width).border_color(c.border_subtle)
            })
            .when(confirming, |d| d.bg(c.tint_failed))
            .child(main)
            .when(confirming, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.md)
                        .pb(t.space.md + t.space.xxs)
                        .pl(a.confirm_indent)
                        .pr(t.space.lg)
                        .child(
                            text(
                                format!(
                                    "Revoke {}? It loses access to the hub at once. To use it again, pair it with a new code.",
                                    client.name
                                ),
                                t.text.ui,
                                c.text_primary,
                            )
                            .flex_1()
                            .min_w_0(),
                        )
                        .child(
                            button("revoke-cancel", "Cancel", ButtonKind::Secondary, t)
                                .bg(gpui_kit::transparent_black())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.admin.revoke_confirm = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            button("revoke-confirm", "Revoke", ButtonKind::DangerFill, t)
                                .debug_selector(|| "revoke-confirm".to_owned())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.revoke_client(confirm_id.clone(), cx)
                                })),
                        ),
                )
            })
    }

    /// The Pair a new app dialog: (title, description, width, body, actions,
    /// lead), as the other dialogs of this kind.
    pub fn pair_client_dialog(
        &mut self,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> (String, String, Pixels, Div, Div, Option<AnyElement>) {
        let c = &t.colors;
        let a = &t.component.admin;
        let pairing = self.state.admin.pairing.clone();
        let now = consort_client::format::now_ms();
        let (left, share) = pairing
            .as_ref()
            .map(|p| countdown(p.expires_at, now))
            .unwrap_or_else(|| ("10:00".to_owned(), 1.0));
        let expired = pairing.as_ref().is_some_and(|p| p.expires_at <= now);
        let code = pairing
            .as_ref()
            .map(|p| p.code.clone())
            .unwrap_or_else(|| "Making a code".to_owned());
        let code_panel = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(t.space.sm + t.space.xxs)
            .py(t.space.lg)
            .px(a.code_pad_x)
            .rounded(t.radius.lg)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .child(
                div()
                    .type_style(detail(t))
                    .font_weight(t.text.label.weight)
                    .text_color(c.text_secondary)
                    .child("Setup code"),
            )
            .child(
                div()
                    .font_family(t.font_mono.clone())
                    .type_style(a.code_text)
                    .text_color(if expired {
                        c.text_tertiary
                    } else {
                        c.text_primary
                    })
                    .child(gpui_kit::base::SelectableText::new(
                        "client-pairing-code",
                        code.clone(),
                    )),
            )
            .child(div().flex_1().min_w_0())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs + t.space.xxs)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(if expired {
                                text(
                                    "The code expired. Choose New code.",
                                    detail(t),
                                    c.status_failed,
                                )
                            } else {
                                div()
                                    .flex()
                                    .type_style(detail(t))
                                    .text_color(c.text_secondary)
                                    .child("Expires in\u{a0}")
                                    .child(div().font_family(t.font_mono.clone()).child(left))
                            })
                            .child(text("Works once", detail(t), c.text_tertiary)),
                    )
                    .child(
                        div()
                            .h(a.countdown_bar)
                            .w_full()
                            .rounded(t.radius.full)
                            .bg(c.border_strong)
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(share))
                                    .rounded(t.radius.full)
                                    .bg(c.text_secondary),
                            ),
                    ),
            );
        let qr_text = pairing
            .as_ref()
            .map(|p| p.qr_text.clone())
            .unwrap_or_default();
        let qr = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .p(a.qr_pad)
            .rounded(t.radius.lg)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .child(if qr_text.is_empty() || expired {
                div()
                    .size(a.qr)
                    .rounded(t.radius.md)
                    .bg(c.surface_selected)
                    .into_any_element()
            } else {
                qr_code(&qr_text, t)
            });
        let address = pairing
            .as_ref()
            .map(|p| p.hub_url.clone())
            .unwrap_or_default();
        let address_block = div()
            .flex()
            .flex_col()
            .gap(t.space.xs + t.space.xxs)
            .child(
                div()
                    .type_style(detail(t))
                    .font_weight(t.text.label.weight)
                    .text_color(c.text_secondary)
                    .child("Hub address"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .py(t.space.xs + t.space.xxs)
                    .pl(t.space.md)
                    .pr(t.space.xs + t.space.xxs)
                    .rounded(t.radius.md)
                    .bg(c.surface_canvas)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(t.font_mono.clone())
                            .type_style(t.text.mono)
                            .text_color(c.text_primary)
                            .child(gpui_kit::base::SelectableText::new(
                                "pairing-hub-address",
                                address.clone(),
                            )),
                    )
                    .child(copy_button(
                        "copy-pairing-address",
                        address,
                        "hub address",
                        a.copy_small,
                        t,
                        cx,
                    )),
            )
            .child(text(
                "The QR code holds the address and the code. If you type the code, the new app asks for this address too.",
                detail(t),
                c.text_tertiary,
            ));
        let loopback = pairing
            .as_ref()
            .is_some_and(|p| p.loopback_only)
            .then(|| self.loopback_notice(consort_client::reach::LOOPBACK_DEVICE_NOTICE, t, cx));
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.xl - t.space.xs)
            .children(loopback)
            .child(div().flex().gap(t.space.lg).child(code_panel).child(qr))
            .child(address_block);
        let paired = pairing.as_ref().and_then(|p| p.paired.clone());
        let (status, note): (AnyElement, String) = match &paired {
            Some(name) => (
                icon("check", t.icon.size, c.status_done).into_any_element(),
                format!("{name} is paired. It shows in the list."),
            ),
            None if expired => (
                icon("clock", t.icon.size, c.text_tertiary).into_any_element(),
                "The code expired".to_owned(),
            ),
            None => (
                status_glyph(
                    "waiting-app",
                    consort_client::TaskStatus::Working,
                    t.icon.size,
                    t,
                    cx,
                ),
                "Waiting for the new app".to_owned(),
            ),
        };
        let mut actions = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.md + t.space.xxs)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(status)
            .child(text(note, t.text.ui, c.text_secondary).flex_1().min_w_0());
        if paired.is_some() {
            actions = actions.child(
                button("pairing-done", "Done", ButtonKind::Primary, t)
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            );
        } else {
            actions = actions
                .child(
                    button("pairing-new-code", "New code", ButtonKind::Secondary, t)
                        .debug_selector(|| "pairing-new-code".to_owned())
                        .bg(gpui_kit::transparent_black())
                        .on_click(cx.listener(|this, _, _, cx| this.new_client_code(cx))),
                )
                .child(
                    button("pairing-cancel", "Cancel", ButtonKind::Secondary, t)
                        .bg(gpui_kit::transparent_black())
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                );
        }
        (
            "Pair a new app".to_owned(),
            "Open Consort on the new device and choose Pair with a hub. Type this code there, or scan it with a phone.".to_owned(),
            t.layout.dialog_wide,
            body,
            actions,
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_code_holds_the_pairing_text() {
        let (width, modules) =
            qr_modules(r#"{"code":"KQ7M-4XPD","hub_url":"wss://hub.example.com:7443"}"#).unwrap();
        assert_eq!(modules.len(), width * width);
        assert!(width >= 21, "a QR code is at least 21 modules wide");
        // The three finder patterns: the top-left corner is dark.
        assert!(modules[0]);
        assert!(modules.iter().any(|m| !m));
    }
}
