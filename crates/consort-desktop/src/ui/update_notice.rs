//! The update notice in the sidebar, above the account row (Paper `EL0-0`):
//! "Update ready. Restart to install." with Restart. It shows only when the
//! update is downloaded and its checksum is right.
//!
//! The update logic is in `consort_client::update`. This file starts it,
//! redraws the app when its state changes, and draws the notice. The About
//! dialog, which another build adds, reads the same state from
//! `ConsortApp::updates`.

use std::sync::atomic::{AtomicBool, Ordering};

use consort_client::update::{UpdateConfig, UpdateState, UpdateView, Updates, Version};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div,
};

use crate::{
    app::ConsortApp,
    ui::parts::{focusable, icon, text},
};

/// A restart is under way. A second click on Restart does nothing.
static RESTARTING: AtomicBool = AtomicBool::new(false);

/// The version of this app.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The updates of this app. The mock mode and a build that runs from a
/// build folder never check: they cannot replace themselves.
pub fn start_updates(demo: bool) -> Updates {
    if !demo
        && let Some(config) = UpdateConfig::desktop(VERSION)
        && config.installable
        && let Ok(updates) = Updates::start(config)
    {
        return updates;
    }
    Updates::fixed(UpdateView {
        state: UpdateState::Idle,
        auto: true,
        current: Version::parse(VERSION).unwrap_or(Version {
            major: 0,
            minor: 0,
            patch: 0,
            pre: None,
        }),
        installable: false,
    })
}

/// The state of an update that is ready, for the screen `update-ready`.
pub fn ready_view() -> UpdateView {
    let mut view = start_updates(true).view();
    view.state = UpdateState::Ready {
        version: Version {
            major: 0,
            minor: 7,
            patch: 1,
            pre: None,
        },
        archive: std::path::PathBuf::new(),
    };
    view
}

impl ConsortApp {
    /// Redraws the app when the update state changes.
    pub fn follow_updates(&mut self, cx: &mut Context<Self>) {
        let mut rx = self.updates.subscribe();
        cx.spawn(async move |this, cx| {
            while rx.changed().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Replaces the app with the staged update, and starts the new copy.
    pub fn restart_to_update(&mut self, cx: &mut Context<Self>) {
        if RESTARTING.swap(true, Ordering::SeqCst) {
            return;
        }
        let updates = self.updates.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updates.install_and_relaunch() })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => cx.quit(),
                Err(err) => {
                    RESTARTING.store(false, Ordering::SeqCst);
                    this.show_error(err.to_string(), cx);
                }
            });
        })
        .detach();
    }

    /// The notice, or `None` when no update is ready.
    pub fn update_notice(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let c = &t.colors;
        let n = t.component.update_notice;
        let view = self.updates.view();
        let message = view.notice()?;
        Some(
            div()
                .id("update-notice")
                .debug_selector(|| "update-notice".into())
                .flex()
                .items_center()
                .gap(n.gap)
                .py(n.pad_y)
                .pl(n.pad_left)
                .pr(n.pad_right)
                .mb(n.margin_bottom)
                .bg(c.surface_card)
                .border(t.border_width)
                .border_color(c.border_subtle)
                .rounded(t.radius.lg)
                .child(icon("download", t.icon.size, c.status_working))
                .child(
                    text(message, t.text.ui, c.text_secondary)
                        .flex_1()
                        .min_w_0(),
                )
                .child(
                    focusable(
                        div()
                            .id(SharedString::from("update-restart"))
                            .debug_selector(|| "update-restart".into()),
                        t,
                    )
                    .flex_none()
                    .py(n.action_pad_y)
                    .px(n.action_pad_x)
                    .rounded(t.radius.md)
                    .type_style(t.text.ui_strong)
                    .text_color(c.text_primary)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover_sidebar))
                    .child("Restart")
                    .on_click(cx.listener(|this, _, _, cx| this.restart_to_update(cx))),
                )
                .into_any_element(),
        )
    }
}
