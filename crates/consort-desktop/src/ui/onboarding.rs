//! The first-launch screen: connect to a hub with its URL and a setup code.

use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, Context, Focusable as _, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement as _, Styled, Window, WindowControlArea, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Field},
    ui::{
        dialogs::input_box,
        parts::{ButtonKind, button, detail, glyph, icon, text},
    },
};

impl ConsortApp {
    pub fn connect(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let url = self.field(Field::HubUrl, cx);
        let code = self.field(Field::SetupCode, cx);
        if url.is_empty() {
            self.onboarding.error =
                Some("Enter the address of your hub, such as https://hub.example.com.".into());
            cx.notify();
            return;
        }
        if code.is_empty() {
            self.onboarding.error = Some(
                "Enter the setup code. The hub prints it when it starts for the first time.".into(),
            );
            cx.notify();
            return;
        }
        self.onboarding.busy = true;
        self.onboarding.error = None;
        cx.notify();
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client.pair(url, code).await;
            let _ = this.update(cx, |this, cx| {
                this.onboarding.busy = false;
                match result {
                    Ok(()) => this.page = crate::app::Page::Main,
                    Err(err) => this.onboarding.error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn render_onboarding(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let url_focused = self
            .inputs
            .hub_url
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let code_focused = self
            .inputs
            .setup_code
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let busy = self.onboarding.busy;
        let mark_color = if t.is_dark() {
            c.accent_fill
        } else {
            c.accent_text
        };
        let field = |label: &str, input, hint: &str| {
            div()
                .flex()
                .flex_col()
                .gap(t.space.xs + t.space.xxs)
                .child(text(label.to_owned(), t.text.label, c.text_secondary))
                .child(input)
                .child(text(hint.to_owned(), detail(t), c.text_tertiary))
        };
        let form = div()
            .flex()
            .flex_col()
            .gap(t.space.xl)
            .w(t.layout.panel_width + t.space.page)
            .child(
                div()
                    .flex()
                    .items_center()
                    // The wordmark: the mark is 1.2 times the font size high,
                    // with a gap of 0.35 times the font size.
                    .gap(t.text.heading.size * 0.35)
                    .child(glyph("mark", t.text.heading.size * 1.2, mark_color))
                    .child(
                        div()
                            .text_size(t.text.heading.size)
                            .font_weight(t.text.heading.weight)
                            .text_color(c.text_primary)
                            .child("consort"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.sm)
                    .child(text("Connect to your hub", t.text.display, c.text_primary))
                    .child(text(
                        "The hub keeps your channels, tasks and memory. This app connects to it once with a setup code, and then on every start.",
                        t.text.body,
                        c.text_secondary,
                    )),
            )
            .child(field(
                "Hub URL",
                input_box(&self.inputs.hub_url, url_focused, t),
                "The address of the server that runs consort hub.",
            ))
            .child(field(
                "Setup code",
                input_box(&self.inputs.setup_code, code_focused, t),
                "The hub prints this code when it starts for the first time. A paired app can also make one.",
            ))
            .when_some(self.onboarding.error.clone(), |d, err| {
                d.child(
                    div()
                        .flex()
                        .gap(t.space.sm)
                        .p(t.space.md)
                        .rounded(t.radius.md)
                        .bg(c.tint_failed)
                        .child(icon("circle-alert", t.icon.size, c.status_failed))
                        .child(text(err, t.text.ui, c.text_primary).flex_1()),
                )
            })
            .child(
                div().flex().child(
                    button("connect", if busy { "Connecting" } else { "Connect" }, ButtonKind::Primary, t)
                        .px(t.space.xl)
                        .when(busy, |b| b.opacity(0.7))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if !this.onboarding.busy {
                                this.connect(window, cx)
                            }
                        })),
                ),
            );
        div()
            .id("onboarding")
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface_canvas)
            .child(
                div()
                    .h(t.space.xxl + t.space.md)
                    .w_full()
                    .flex_none()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .pb(t.space.page)
                    .type_style(t.text.ui)
                    .child(form),
            )
            .into_any_element()
    }
}
