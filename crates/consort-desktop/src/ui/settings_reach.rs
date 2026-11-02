//! Settings > Hub > Reach from other devices (Paper R1, R2 and R3).
//!
//! The hub does the Tailscale work, and `consort_client::reach` holds the words
//! and the actions. This module only draws them.

use consort_client::format;
use consort_client::reach::{
    AddDeviceView, GET_TAILSCALE_URL, ReachAction, ReachCard, ReachMethod, ReachRoute, ReachView,
    add_device, open_tailscale_app,
};
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _,
};

use crate::app::{ConsortApp, Dialog};
use crate::ui::admin_parts::{column, header, hint, page, status_dot};
use crate::ui::parts::{
    ButtonKind, button, detail, error_box, icon, icon_button, selectable, text,
};
use crate::ui::settings::segmented;
use crate::ui::settings_clients::qr_code;
use consort_client::connectors::LoadState;

/// A mono value that the user can select and copy, in a box.
fn value_box(
    id: String,
    value: String,
    what: &'static str,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> Div {
    let c = &t.colors;
    let copy = value.clone();
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
                .child(selectable(
                    SharedString::from(id.clone()),
                    value,
                    t.text.mono,
                    c.text_primary,
                )),
        )
        .child(
            icon_button(
                SharedString::from(format!("copy-{id}")),
                "copy",
                t.component.admin.copy_small,
                t.icon.dense,
                c.text_secondary,
                c.hover_card,
                t,
                cx,
            )
            .flex_none()
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                this.show_notice(format!("Copied the {what}."), cx);
            })),
        )
}

/// The ground of the cards of this page.
fn card_shell(t: &Tokens) -> Div {
    div()
        .flex()
        .gap(t.space.lg)
        .p(t.space.lg)
        .rounded(t.radius.lg)
        .border(t.border_width)
        .border_color(t.colors.border_subtle)
}

impl ConsortApp {
    /// What a button of a card does.
    pub fn run_reach_action(&mut self, action: ReachAction, cx: &mut Context<Self>) {
        match action {
            ReachAction::Set { method, on } => self.set_reach(method, on, cx),
            ReachAction::OpenUrl(url) => cx.open_url(&url),
            ReachAction::OpenTailscale => {
                if self.demo {
                    return;
                }
                if !open_tailscale_app() {
                    cx.open_url(GET_TAILSCALE_URL);
                }
            }
        }
    }

    /// The card of one way, or of one state in the list of all states.
    fn reach_card(
        &self,
        key: &str,
        card: &ReachCard,
        busy: Option<ReachMethod>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let title = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .when(card.answering, |d| d.child(status_dot(Some(true), t)))
            .child(
                text(card.title.clone(), t.text.ui_strong, c.text_primary)
                    .flex_1()
                    .min_w_0(),
            );
        let check = card.check.clone().map(|line| {
            div()
                .flex()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .child(
                    div()
                        .flex_none()
                        .child(icon("check", t.icon.dense, c.status_done)),
                )
                .child(text(line, detail(t), c.text_secondary).flex_1().min_w_0())
        });
        let info = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(t.space.sm)
            .child(title)
            .when(!card.body.is_empty(), |d| {
                d.child(text(card.body.clone(), t.text.ui, c.text_secondary))
            })
            .children(check)
            .children(
                card.address
                    .clone()
                    .map(|address| value_box(format!("{key}-address"), address, "address", t, cx)),
            )
            .children(
                card.command
                    .clone()
                    .map(|command| value_box(format!("{key}-command"), command, "command", t, cx)),
            )
            .children(card.note.clone().map(|note| hint(note, t)))
            .children(card.detail.clone().map(|detail_text| {
                div().font_family(t.font_mono.clone()).child(selectable(
                    SharedString::from(format!("{key}-detail")),
                    detail_text,
                    detail(t),
                    c.text_tertiary,
                ))
            }));
        let mut actions = div()
            .flex()
            .flex_none()
            .flex_col()
            .items_end()
            .gap(t.space.sm);
        for (i, action) in card.buttons.iter().enumerate() {
            let kind = match (action.primary, i) {
                (true, _) => ButtonKind::Primary,
                (false, 0) => ButtonKind::Secondary,
                (false, _) => ButtonKind::Quiet,
            };
            let waiting = matches!(
                action.action,
                ReachAction::Set { method, .. } if busy == Some(method)
            );
            let selector = format!("{key}-{}", action.label.to_lowercase().replace(' ', "-"));
            let run = action.action.clone();
            actions = actions.child(
                button(SharedString::from(selector.clone()), action.label, kind, t)
                    .debug_selector(move || selector.clone())
                    .when(waiting, |b| b.opacity(t.opacity.busy))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let waits = matches!(
                            run,
                            ReachAction::Set { method, .. }
                                if this.state.admin.reach.busy == Some(method)
                        );
                        if !waits {
                            this.run_reach_action(run.clone(), cx);
                        }
                    })),
            );
        }
        card_shell(t)
            .debug_selector({
                let key = key.to_owned();
                move || key.clone()
            })
            .child(info)
            .when(!card.buttons.is_empty(), |d| d.child(actions))
    }

    /// The route that Add a device shows.
    fn reach_route(&self, view: &ReachView) -> Option<ReachRoute> {
        view.routes
            .iter()
            .find(|route| Some(route.method) == self.admin.reach_route)
            .or(view.routes.first())
            .cloned()
    }

    /// The Add a device card: a code, an address and a QR code.
    fn add_device_card(&self, view: &ReachView, t: &Tokens, cx: &mut Context<Self>) -> Option<Div> {
        let c = &t.colors;
        let a = &t.component.admin;
        let route = self.reach_route(view)?;
        let pairing = self.state.admin.pairing.clone();
        let now = format::now_ms();
        let expired = pairing.as_ref().is_some_and(|p| p.expires_at <= now);
        let device: Option<AddDeviceView> =
            pairing.as_ref().and_then(|p| add_device(&route, &p.code));
        let choices: Vec<(ReachMethod, &str)> =
            view.routes.iter().map(|r| (r.method, r.label)).collect();
        let code = match &device {
            Some(device) if !expired => div()
                .flex()
                .items_center()
                .gap(t.space.md)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(t.font_mono.clone())
                        .child(selectable(
                            "reach-pairing-code",
                            device.code.clone(),
                            a.code_text,
                            c.text_primary,
                        )),
                )
                .into_any_element(),
            Some(_) => text(
                "The code expired. Choose New code.",
                detail(t),
                c.status_failed,
            )
            .into_any_element(),
            None => text("Making a code", t.text.ui, c.text_secondary).into_any_element(),
        };
        let info = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(t.space.md)
            .child(text("Add a device", t.text.ui_strong, c.text_primary))
            .when(choices.len() > 1, |d| {
                d.child(div().flex().child(segmented(
                    "reach-route",
                    &choices,
                    route.method,
                    t,
                    cx,
                    |_| t.text.ui,
                    |this, method, _, cx| {
                        this.admin.reach_route = Some(method);
                        cx.notify();
                    },
                )))
            })
            .children(
                device
                    .as_ref()
                    .map(|device| text(device.body, t.text.ui, c.text_secondary)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .child(text("Setup code", detail(t), c.text_secondary))
                    .child(code),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.xs)
                    .child(text("Hub address", detail(t), c.text_secondary))
                    .child(value_box(
                        "reach-pairing-address".to_owned(),
                        route.hub_url.clone(),
                        "hub address",
                        t,
                        cx,
                    )),
            )
            .when(route.tls_sha256.is_some(), |d| {
                d.children(device.as_ref().filter(|_| !expired).map(|device| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(t.space.xs)
                        .child(text("Pairing link", detail(t), c.text_secondary))
                        .child(value_box(
                            "reach-pairing-link".to_owned(),
                            device.link.clone(),
                            "pairing link",
                            t,
                            cx,
                        ))
                }))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(t.space.sm)
                    .child(
                        button("reach-new-code", "New code", ButtonKind::Secondary, t)
                            .debug_selector(|| "reach-new-code".to_owned())
                            .on_click(cx.listener(|this, _, _, cx| this.new_client_code(cx))),
                    )
                    .child(
                        button("reach-add-worker", "Add a worker", ButtonKind::Secondary, t)
                            .debug_selector(|| "reach-add-worker".to_owned())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_dialog(Dialog::AddWorker, window, cx);
                            })),
                    ),
            );
        let qr = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .self_start()
            .p(a.qr_pad)
            .rounded(t.radius.lg)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .child(match device.as_ref().filter(|_| !expired) {
                Some(device) => qr_code(&device.qr_text, t),
                None => div()
                    .size(a.qr)
                    .rounded(t.radius.md)
                    .bg(c.surface_selected)
                    .into_any_element(),
            });
        Some(
            card_shell(t)
                .debug_selector(|| "reach-add-device".to_owned())
                .child(info)
                .child(qr),
        )
    }

    /// The page. In `reach-states` it is the list of all states.
    pub fn render_reach(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let view = self.state.admin.reach.clone();
        let back = button("reach-back", "Back to Hub", ButtonKind::Quiet, t)
            .debug_selector(|| "reach-back".to_owned())
            .on_click(cx.listener(|this, _, _, cx| this.close_reach(cx)))
            .into_any_element();
        let line = text(view.line.clone(), t.text.ui, c.text_secondary);
        let mut body = column(t).child(header("Reach from other devices", line, Some(back), t));
        if self.admin.reach_gallery {
            for (i, (label, card)) in consort_client::mock_reach::gallery().iter().enumerate() {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(t.space.sm)
                        .child(hint(label.to_string(), t))
                        .child(self.reach_card(&format!("reach-state-{i}"), card, None, t, cx)),
                );
            }
            return page("reach-page", t).child(body).into_any_element();
        }
        if let Some(error) = &view.error {
            body = body.child(error_box("reach-error", error.clone(), t));
        }
        let (Some(tailscale), Some(network)) = (&view.tailscale, &view.this_network) else {
            let state = match &view.load {
                LoadState::Failed(message) | LoadState::Unsupported(message) => div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(t.space.md)
                    .child(error_box("reach-load-error", message.clone(), t).w_full())
                    .child(
                        button("reach-retry", "Try again", ButtonKind::Secondary, t)
                            .on_click(cx.listener(|this, _, _, cx| this.load_reach(cx))),
                    ),
                _ => text("Asking the hub.", t.text.ui, c.text_secondary),
            };
            return page("reach-page", t)
                .child(body.child(state))
                .into_any_element();
        };
        let busy = view.busy;
        let cards = [("reach-tailscale", tailscale), ("reach-network", network)];
        // The ways that are on come first, and then Add a device.
        for (key, card) in cards.iter().filter(|(_, card)| card.answering) {
            body = body.child(self.reach_card(key, card, busy, t, cx));
        }
        body = body.children(self.add_device_card(&view, t, cx));
        for (key, card) in cards.iter().filter(|(_, card)| !card.answering) {
            body = body.child(self.reach_card(key, card, busy, t, cx));
        }
        page("reach-page", t).child(body).into_any_element()
    }

    /// The notice above a code that only works on the computer of the hub.
    pub fn loopback_notice(
        &self,
        message: &'static str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        div()
            .flex()
            .items_center()
            .gap(t.space.md)
            .p(t.space.md)
            .rounded(t.radius.md)
            .bg(c.surface_card)
            .child(
                text(message, t.text.ui, c.text_secondary)
                    .flex_1()
                    .min_w_0(),
            )
            .child(
                div().flex_none().child(
                    button(
                        "open-reach",
                        "Choose how devices reach the hub",
                        ButtonKind::Secondary,
                        t,
                    )
                    .debug_selector(|| "open-reach".to_owned())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_dialog(window, cx);
                        this.open_settings(crate::app::SettingsSection::Hub, cx);
                        this.open_reach(cx);
                    })),
                ),
            )
    }

    /// The error of a client whose hub is on the tailnet and that cannot
    /// connect (Paper R3).
    pub fn tailnet_card(&self, t: &Tokens) -> Option<AnyElement> {
        let notice = self.state.tailnet_notice.as_ref()?;
        Some(
            crate::ui::versions::titled_error_box(
                "tailnet-notice",
                notice.title.to_owned(),
                notice.body.to_owned(),
                t,
            )
            .debug_selector(|| "tailnet-notice".to_owned())
            .into_any_element(),
        )
    }
}
