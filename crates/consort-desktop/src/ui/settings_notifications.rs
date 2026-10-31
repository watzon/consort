//! Settings > Notifications (Paper `8XN-0`): the five switches of what
//! notifies, Show message text, the Dock count with its preview, the
//! system permission line, an example banner, and Test notification.

use consort_client::notify::{NotificationSettings, example_caption, example_notification};
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, ParentElement,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::app::ConsortApp;
use crate::notify_platform::HAS_DOCK_BADGE;
use crate::ui::admin_parts::{column, header, page, row, rows, section_title, status_dot};
use crate::ui::mark::still_mark;
use crate::ui::parts::{ButtonKind, button, detail, text};
use crate::ui::settings::switch;

/// One switch of the page: its ID, title, hint and field.
type SwitchRow = (
    &'static str,
    &'static str,
    &'static str,
    fn(&NotificationSettings) -> bool,
    fn(&mut NotificationSettings, bool),
);

/// The switches of "Notify me when", in the order of the design. The last
/// one is for the messages that the assistant posts on its own (phase 4b).
const NOTIFY_ME_WHEN: [SwitchRow; 6] = [
    (
        "notify-waiting",
        "An approval or a decision waits for me",
        "The task stops until you answer.",
        |s| s.waiting,
        |s, v| s.waiting = v,
    ),
    (
        "notify-review",
        "A task is ready for review",
        "The work is done, and only you can resolve it.",
        |s| s.review,
        |s, v| s.review = v,
    ),
    (
        "notify-failed",
        "A task failed",
        "You can read why in its thread and retry it.",
        |s| s.failed,
        |s, v| s.failed = v,
    ),
    (
        "notify-messages",
        "A message comes in a channel that is not open",
        "Messages from the assistant. The channel on screen does not notify.",
        |s| s.messages,
        |s, v| s.messages = v,
    ),
    (
        "notify-routines",
        "A routine has a result",
        "Results also stay in the routine's channel.",
        |s| s.routines,
        |s, v| s.routines = v,
    ),
    (
        "notify-attention",
        "When the assistant needs your attention",
        "A message that the assistant posts on its own notifies only when you must act or a deadline is close. Never in quiet hours.",
        |s| s.attention,
        |s, v| s.attention = v,
    ),
];

impl ConsortApp {
    pub fn render_notifications(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let settings = self.admin.settings;
        let test = button(
            "test-notification",
            "Test notification",
            ButtonKind::Secondary,
            t,
        )
        .on_click(cx.listener(|this, _, _, cx| this.send_test_notification(cx)));
        let computer = match std::env::consts::OS {
            "macos" => "this Mac",
            "windows" => "this PC",
            _ => "this computer",
        };
        let line = text(
            format!(
                "Consort tells {computer} when something needs you, also when the window is closed."
            ),
            t.text.ui,
            c.text_secondary,
        );

        let mut when = rows(t);
        for (i, (id, title, hint, get, set)) in NOTIFY_ME_WHEN.iter().enumerate() {
            let on = get(&settings);
            let set = *set;
            when = when.child(switch_row(
                id,
                title,
                hint,
                on,
                i + 1 == NOTIFY_ME_WHEN.len(),
                t,
                cx,
                move |this, cx| this.set_notification_setting(|s| set(s, !on), cx),
            ));
        }

        let show_text = settings.show_text;
        let badge_on = settings.dock_badge;
        let count = self.state.admin.attention.count;
        let (badge_title, badge_hint) = if HAS_DOCK_BADGE {
            (
                "Show a count on the Dock icon",
                format!(
                    "It counts the approvals and decisions that wait, and the tasks in Ready for review. Now {count}."
                ),
            )
        } else {
            (
                "Show a count in the window title",
                format!(
                    "It counts the approvals and decisions that wait, and the tasks in Ready for review. Now {count}."
                ),
            )
        };
        let shows = rows(t)
            .child(switch_row(
                "notify-show-text",
                "Show message text",
                "Off: a notification names the channel and what happened, but not what was written. Anyone who can see your screen can read the text when this is on.",
                show_text,
                false,
                t,
                cx,
                move |this, cx| this.set_notification_setting(|s| s.show_text = !show_text, cx),
            ))
            .child(
                row(true, t)
                    .child(dock_preview(count, t))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(text(badge_title, t.text.ui_strong, c.text_primary))
                            .child(text(badge_hint, detail(t), c.text_tertiary)),
                    )
                    .child(switch("notify-dock-badge", badge_on, t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.set_notification_setting(|s| s.dock_badge = !badge_on, cx)
                        },
                    ))),
            );

        let permission = &self.admin.permission;
        let system = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .child(status_dot(permission.allowed(), t))
            .child(text(permission.line(), detail(t), c.text_tertiary));

        let example = example_notification(&self.state.admin.attention, &settings);
        let caption = example_caption(&example, &settings);
        let a = &t.component.admin;
        let banner = div()
            .flex()
            .flex_none()
            .items_start()
            .gap(a.banner_gap)
            .w(a.banner_width)
            .py(a.banner_gap)
            .px(t.space.md)
            .rounded(t.radius.xl)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_strong)
            .shadow(t.shadow_float())
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(a.banner_tile)
                    .rounded(a.banner_tile_radius)
                    .bg(c.surface_card)
                    .child(still_mark(a.banner_mark, t)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(text(example.title.clone(), a.banner_title, c.text_primary))
                            .child(text("now", t.text.caption, c.text_tertiary)),
                    )
                    .child(text(example.body.clone(), a.banner_text, c.text_secondary)),
            );

        page("notifications-page", t)
            .child(
                column(t)
                    .child(header(
                        "Notifications",
                        line,
                        Some(test.into_any_element()),
                        t,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(t.space.md)
                            .child(section_title("Notify me when", t))
                            .child(when),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(t.space.md)
                            .child(section_title("What a notification shows", t))
                            .child(shows)
                            .child(system)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(t.space.lg + t.space.xs)
                                    .pt(t.space.md)
                                    .child(banner)
                                    .child(
                                        text(caption, detail(t), c.text_tertiary)
                                            .flex_1()
                                            .min_w_0(),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// A row with a title, a hint and a switch at the right.
#[allow(clippy::too_many_arguments)]
fn switch_row(
    id: &'static str,
    title: &str,
    hint: &str,
    on: bool,
    last: bool,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
    toggle: impl Fn(&mut ConsortApp, &mut Context<ConsortApp>) + 'static,
) -> Div {
    let c = &t.colors;
    row(last, t)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(text(title.to_owned(), t.text.ui_strong, c.text_primary))
                .child(text(hint.to_owned(), detail(t), c.text_tertiary)),
        )
        .child(
            switch(id, on, t)
                .debug_selector(move || id.to_owned())
                .on_click(cx.listener(move |this, _, _, cx| toggle(this, cx))),
        )
}

/// The Dock icon with its count, as the badge looks.
fn dock_preview(count: u32, t: &Tokens) -> Div {
    let c = &t.colors;
    let a = &t.component.admin;
    div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(a.dock_tile)
        .rounded(t.radius.lg)
        .bg(c.surface_card)
        .border(t.border_width)
        .border_color(c.border_strong)
        .child(still_mark(a.dock_mark, t))
        .when(count > 0, |d| {
            d.child(
                div()
                    .absolute()
                    .top(-a.dock_badge_offset)
                    .right(-a.dock_badge_offset)
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(a.dock_badge)
                    .min_w(a.dock_badge)
                    .px(t.space.xs)
                    .rounded(t.radius.full)
                    .bg(c.status_failed)
                    .child(text(
                        if count > 99 {
                            "99+".to_owned()
                        } else {
                            count.to_string()
                        },
                        a.dock_badge_text,
                        c.on_failed,
                    )),
            )
        })
}
