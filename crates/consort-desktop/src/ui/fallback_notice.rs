//! The notices of the fallback path (designs B3 and B4): the assistant or a
//! task moved, the assistant went back, and a move skipped an account that
//! is signed out.
//!
//! A notice is one quiet centered line with an icon. A move has a Details
//! toggle (the lines of the path report, a filled dot for a step that
//! failed and a ring for a step that Consort skipped), Edit path, and
//! Go back now once the limit has reset and the assistant is still on the
//! fallback. In a task thread the move says that the task stays, and
//! Details is open at first. The open state is local to the app, like the
//! details of an error card.

use consort_client::{FallbackNoticeKind, FallbackNoticeView};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, base::SelectableText, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, SettingsSection},
    fallback::FallbackAction,
    ui::{
        models::path_lines,
        parts::{ButtonKind, button, focusable, icon, text},
        timeline::is_open_click,
    },
};

impl ConsortApp {
    /// A fallback notice. `key` is unique for the place (the channel
    /// timeline or the task thread). `channel_id` is the channel whose
    /// assistant Go back now takes back. `in_thread` is true in a task
    /// thread, where Details starts open.
    #[allow(clippy::too_many_arguments)]
    pub fn render_fallback_notice(
        &mut self,
        notice: &FallbackNoticeView,
        key: &str,
        time_label: &str,
        channel_id: &str,
        in_thread: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let (icon_name, tint) = match notice.kind {
            FallbackNoticeKind::Moved => ("sort", c.accent_text),
            FallbackNoticeKind::Returned => ("reply", c.text_secondary),
            FallbackNoticeKind::SignedOut => ("warning", c.accent_text),
        };
        let line = div()
            .flex()
            .items_start()
            .justify_center()
            .gap(t.space.sm)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(t.text.ui.line)
                    .child(icon(icon_name, t.icon.dense, tint)),
            )
            .child(
                div()
                    .min_w_0()
                    .type_style(t.text.ui)
                    .text_color(c.text_secondary)
                    .child(SelectableText::new(
                        SharedString::from(format!("{key}.text")),
                        notice.text.clone(),
                    )),
            );

        // Details is open at first in a task thread. A click toggles it.
        let details_key = format!("{key}-details");
        let toggled = self.errors_open.contains(&details_key);
        let details_open = in_thread != toggled;
        let has_details = notice.kind == FallbackNoticeKind::Moved && !notice.lines.is_empty();
        let mut actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap(t.space.md);
        if has_details {
            let toggle = details_key.clone();
            actions = actions.child(
                focusable(
                    div().id(SharedString::from(format!("fallback-details-{key}"))),
                    t,
                )
                .debug_selector({
                    let id = format!("fallback-details-{key}");
                    move || id.clone()
                })
                .flex()
                .items_center()
                .gap(t.space.xxs)
                .rounded(t.radius.md)
                .px(t.space.xs)
                .cursor_pointer()
                .hover(|s| s.bg(c.hover_canvas))
                .child(text("Details", t.text.ui, c.text_secondary))
                .child(icon(
                    if details_open {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                    t.icon.dense,
                    c.text_secondary,
                ))
                .on_click(cx.listener(move |this, event, _, cx| {
                    if !is_open_click(event) {
                        return;
                    }
                    if !this.errors_open.remove(&toggle) {
                        this.errors_open.insert(toggle.clone());
                    }
                    this.timeline.remeasure();
                    cx.notify();
                })),
            );
        }
        if notice.go_back {
            let channel = channel_id.to_owned();
            actions = actions.child(
                button(
                    SharedString::from(format!("fallback-go-back-{key}")),
                    "Go back now",
                    ButtonKind::Secondary,
                    t,
                )
                .debug_selector({
                    let id = format!("fallback-go-back-{key}");
                    move || id.clone()
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_fallback(FallbackAction::GoBack(channel.clone()), window, cx)
                })),
            );
        }
        match notice.kind {
            FallbackNoticeKind::Moved => {
                actions = actions.child(link(
                    format!("fallback-edit-{key}"),
                    "Edit path",
                    t,
                    cx.listener(|this, _, _, cx| this.open_settings(SettingsSection::Models, cx)),
                ));
            }
            FallbackNoticeKind::SignedOut => {
                actions = actions.child(link(
                    format!("fallback-sign-in-{key}"),
                    "Open Workers",
                    t,
                    cx.listener(|this, _, _, cx| this.open_settings(SettingsSection::Workers, cx)),
                ));
            }
            FallbackNoticeKind::Returned => {}
        }
        actions = actions.child(text(time_label.to_owned(), t.text.caption, c.text_tertiary));

        div()
            .id(SharedString::from(format!("fallback-notice-{key}")))
            .debug_selector({
                let id = format!("fallback-notice-{key}");
                move || id.clone()
            })
            .flex()
            .flex_col()
            .items_center()
            .gap(t.space.xs)
            .w_full()
            .child(line)
            .child(actions)
            .when(has_details && details_open, |d| {
                d.child(
                    div()
                        .w_full()
                        .max_w(t.layout.bubble_max_width)
                        .p(t.space.md)
                        .rounded(t.radius.md)
                        .bg(c.surface_card)
                        .child(path_lines(&notice.lines, key, false, t)),
                )
            })
            .into_any_element()
    }
}

/// An underlined action in a notice, such as Edit path.
fn link(
    id: String,
    label: &'static str,
    t: &Tokens,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
) -> Div {
    let c = &t.colors;
    div().child(
        focusable(div().id(SharedString::from(id.clone())), t)
            .debug_selector(move || id.clone())
            .cursor_pointer()
            .type_style(t.text.ui)
            .text_color(c.text_primary)
            .underline()
            .child(label)
            .on_click(on_click),
    )
}
