//! The version surfaces (Paper `TB3-0` and `TED-0`): the card above the
//! composer when the hub and this app do not fit together, the hold on an
//! update in the sidebar, and the newer version in Settings > Hub.
//!
//! The text comes from `consort_client::versions`. The app never talks to the
//! network, and it only shows a command for the user to run.

use consort_client::Connection;
use consort_client::admin::HubStatusView;
use consort_client::update::{UpdateState, UpdateView, Version};
use consort_client::versions::{
    HubUpdateView, IncompatibleView, TooOld, WorkerUpdateTone, WorkerUpdateView, hub_update_view,
    incompatible_view, upgrade_guide_url,
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, ElementId, Hsla, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement as _,
    Styled, base::SelectableText, div, prelude::FluentBuilder as _,
};

use crate::app::{ConsortApp, SettingsSection};
use crate::ui::menu::{MenuAction, MenuItem};
use crate::ui::parts::{ButtonKind, button, focusable, icon, selectable, text};
use crate::ui::timeline::open_context_menu;

/// The card for this connection, or `None` for any other state.
pub fn card_for(connection: &Connection) -> Option<IncompatibleView> {
    match connection {
        Connection::Incompatible {
            too_old,
            hub,
            app_version,
            ..
        } => Some(incompatible_view(*too_old, hub.as_ref(), app_version)),
        _ => None,
    }
}

/// The card in Settings > Hub for this hub and this updater.
pub fn hub_card_for(hub: &HubStatusView, updates: &UpdateView) -> Option<HubUpdateView> {
    hub_update_view(
        &hub.hub_version,
        hub.install,
        &updates.current,
        updates.latest(),
    )
}

/// The color of the line about a worker update.
pub fn worker_update_color(update: &WorkerUpdateView, t: &Tokens) -> Hsla {
    match update.tone {
        WorkerUpdateTone::Quiet => t.colors.text_tertiary,
        WorkerUpdateTone::Working => t.colors.status_working,
        WorkerUpdateTone::Failed => t.colors.status_failed,
    }
}

/// An error box with a title, for the Connect screen. The user can select the
/// detail and copy it.
pub fn titled_error_box(
    id: impl Into<ElementId>,
    title: String,
    message: String,
    t: &Tokens,
) -> Div {
    let c = &t.colors;
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
                .gap(t.space.xxs)
                .flex_1()
                .min_w_0()
                .child(text(title, t.text.ui_strong, c.text_primary))
                .child(selectable(id, message, t.text.ui, c.text_secondary)),
        )
}

impl ConsortApp {
    /// A command that the user runs on the server: selectable, with Copy and
    /// the Copy code menu.
    pub(crate) fn command_block(
        &self,
        key: &'static str,
        command: String,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let c = &t.colors;
        let copy = command.clone();
        let menu = command.clone();
        div()
            .id(SharedString::from(format!("{key}-command")))
            .debug_selector(move || format!("{key}-command"))
            .flex()
            .items_center()
            .gap(t.space.sm)
            .py(t.space.sm)
            .pl(t.space.md)
            .pr(t.space.sm)
            .bg(c.surface_sidebar)
            .border(t.border_width)
            .border_color(c.border_subtle)
            .rounded(t.radius.lg)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(t.font_mono.clone())
                    .type_style(t.text.mono)
                    .text_color(c.text_primary)
                    .child(SelectableText::new(
                        SharedString::from(format!("{key}-command-text")),
                        command,
                    )),
            )
            .child(
                button(
                    SharedString::from(format!("{key}-copy")),
                    "Copy",
                    ButtonKind::Quiet,
                    t,
                )
                .flex_none()
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                    this.show_notice("Copied the command.".into(), cx);
                })),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    open_context_menu(
                        this,
                        event,
                        vec![MenuItem::new("Copy code", MenuAction::Copy(menu.clone()))],
                        window,
                        cx,
                    )
                }),
            )
    }

    /// The card above the composer while the hub and this app have no common
    /// protocol version.
    pub fn incompatible_card(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = &t.colors;
        let view = card_for(&self.state.connection)?;
        let update_ready = matches!(self.updates.view().state, UpdateState::Ready { .. });
        let actions = div().flex().flex_wrap().gap(t.space.sm);
        let actions = match view.too_old {
            TooOld::Hub => actions
                .child(
                    button(
                        "incompatible-try-again",
                        "Try again",
                        ButtonKind::Primary,
                        t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.try_again(cx))),
                )
                .child(
                    button(
                        "incompatible-guide",
                        "Open the upgrade guide",
                        ButtonKind::Secondary,
                        t,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.open_url(&upgrade_guide_url()))),
                ),
            TooOld::ThisSide if update_ready => actions.child(
                button(
                    "incompatible-update",
                    "Update and restart",
                    ButtonKind::Primary,
                    t,
                )
                .debug_selector(|| "incompatible-update".into())
                .on_click(cx.listener(|this, _, _, cx| this.restart_to_update(cx))),
            ),
            TooOld::ThisSide => actions
                .child(
                    button(
                        "incompatible-try-again",
                        "Try again",
                        ButtonKind::Primary,
                        t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.try_again(cx))),
                )
                .child(
                    button(
                        "incompatible-check",
                        "Check for updates",
                        ButtonKind::Secondary,
                        t,
                    )
                    .debug_selector(|| "incompatible-check".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.updates.check_now();
                        cx.notify();
                    })),
                ),
        };
        let command = view
            .command
            .clone()
            .map(|command| self.command_block("incompatible", command, t, cx));
        Some(
            div()
                .id("incompatible-card")
                .debug_selector(|| "incompatible-card".into())
                .flex()
                .flex_col()
                .gap(t.space.md)
                .w_full()
                .max_w(t.layout.timeline_max_width)
                .mb(t.space.sm)
                .p(t.space.lg)
                .bg(c.surface_card)
                .border(t.border_width)
                .border_color(c.border_strong)
                .rounded(t.radius.xl)
                .child(text(view.title.clone(), t.text.ui_strong, c.text_primary))
                .child(selectable(
                    "incompatible-body",
                    view.body.clone(),
                    t.text.ui,
                    c.text_secondary,
                ))
                .children(command)
                .child(actions)
                .into_any_element(),
        )
    }

    /// The hold on an update in the sidebar, in the place of the update
    /// notice. It sends the user to the command in Settings > Hub.
    pub fn update_hold_notice(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = &t.colors;
        let n = t.component.update_notice;
        let hold = self.updates.view().hold()?;
        Some(
            div()
                .id("update-hold")
                .debug_selector(|| "update-hold".into())
                .flex()
                .flex_col()
                .gap(t.space.sm)
                .py(n.pad_y)
                .pl(n.pad_left)
                .pr(n.pad_right)
                .mb(n.margin_bottom)
                .bg(c.surface_card)
                .border(t.border_width)
                .border_color(c.border_subtle)
                .rounded(t.radius.lg)
                .child(text(hold.title(), t.text.ui_strong, c.text_primary))
                .child(text(hold.body(), t.text.caption, c.text_secondary))
                .child(
                    focusable(
                        div()
                            .id(SharedString::from("update-hold-hub"))
                            .debug_selector(|| "update-hold-hub".into()),
                        t,
                    )
                    .flex_none()
                    .self_start()
                    .py(n.action_pad_y)
                    .px(n.action_pad_x)
                    .rounded(t.radius.md)
                    .type_style(t.text.ui_strong)
                    .text_color(c.text_primary)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover_sidebar))
                    .child("Update the hub")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_settings(SettingsSection::Hub, cx);
                    })),
                )
                .into_any_element(),
        )
    }

    /// The newer version in Settings > Hub, with the command for this install.
    pub fn hub_update_card(
        &self,
        hub: &HubStatusView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let c = &t.colors;
        let view = hub_card_for(hub, &self.updates.view())?;
        let command = view
            .command
            .clone()
            .map(|command| self.command_block("hub-update", command, t, cx));
        Some(
            div()
                .debug_selector(|| "hub-update-card".into())
                .flex()
                .flex_col()
                .gap(t.space.md)
                .p(t.space.lg)
                .bg(c.surface_card)
                .border(t.border_width)
                .border_color(c.border_subtle)
                .rounded(t.radius.lg)
                .child(text(view.title.clone(), t.text.ui_strong, c.text_primary))
                .child(text(view.body.clone(), t.text.ui, c.text_secondary))
                .children(command)
                .when(view.guide, |d| {
                    d.child(
                        div().flex().child(
                            button(
                                "hub-update-guide",
                                "Open the upgrade guide",
                                ButtonKind::Secondary,
                                t,
                            )
                            .on_click(cx.listener(|_, _, _, cx| cx.open_url(&upgrade_guide_url()))),
                        ),
                    )
                }),
        )
    }
}

/// The update states for the screens that show one.
pub fn screen_view(base: UpdateView, screen: &str) -> UpdateView {
    let version = |text: &str| Version::parse(text);
    let ready = |version: Option<Version>| {
        version.map(|version| UpdateState::Ready {
            version,
            archive: std::path::PathBuf::new(),
        })
    };
    let mut view = base;
    match screen {
        "update-held" => {
            if let Some(state) = ready(version("0.8.0")) {
                view.state = state;
            }
            view.release_protocols = Some(vec![2]);
        }
        "app-too-old" => {
            if let Some(state) = ready(version("0.9.0")) {
                view.state = state;
            }
        }
        "hub-update" => {
            if let Some(latest) = version("0.8.0") {
                view.state = UpdateState::Available { version: latest };
            }
        }
        _ => {}
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_incompatible_state_has_a_card() {
        assert!(card_for(&Connection::Online).is_none());
        assert!(
            card_for(&Connection::Offline {
                retry_in_ms: 1,
                reason: String::new()
            })
            .is_none()
        );
        let card = card_for(&Connection::Incompatible {
            too_old: TooOld::Hub,
            hub: None,
            app_version: "0.8.0".into(),
            retry_in_ms: 60_000,
        })
        .unwrap();
        assert_eq!(card.title, "The hub needs an update");
    }
}
