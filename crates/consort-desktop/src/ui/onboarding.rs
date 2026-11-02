//! S1 (Paper `S65-0`): connect to a hub with its address and a setup code. A
//! client that the hub refused pairs again here. A pasted pairing link holds the
//! code too.

use consort_client::Connection;
use consort_client::setup::Dabs;
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, Context, Focusable as _, InteractiveElement as _, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Field, Page},
    ui::{
        parts::{ButtonKind, detail, error_box, text},
        setup::{SetupStep, failure_card, link_button, setup_button, setup_frame, setup_heading},
    },
};

impl ConsortApp {
    /// Shows why a pairing failed, with the words of the design.
    pub fn show_pair_error(&mut self, err: &consort_client::ClientError, cx: &mut Context<Self>) {
        let (title, detail) = consort_client::setup::pair_failure(err);
        self.onboarding.error_title = title;
        self.onboarding.error = Some(detail);
        cx.notify();
    }

    pub fn connect(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.onboarding.error_title = None;
        let url = self.field(Field::HubUrl, cx);
        let code = self.field(Field::SetupCode, cx);
        if url.is_empty() {
            self.onboarding.error =
                Some("Enter the address of your hub, such as https://hub.example.com.".into());
            cx.notify();
            return;
        }
        // A pasted pairing link holds the code too.
        if code.is_empty() && !consort_client::reach::is_invite_text(&url) {
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
                    Ok(()) => {
                        this.page = crate::app::Page::Main;
                        this.follow_setup(cx);
                    }
                    Err(err) => this.show_pair_error(&err, cx),
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
        // The hub refused this app, or its saved key is damaged. The user
        // pairs again with a new code. The reason says what happened.
        let refused = matches!(self.state.connection, Connection::Refused { .. });
        let (title, intro, action, action_busy) = if refused {
            (
                "Pair this app again",
                "The hub does not accept this app. Enter the address of your hub and a new setup code to pair this app again.",
                "Pair again",
                "Pairing",
            )
        } else {
            (
                "Connect to your hub",
                "The hub keeps your channels, tasks and memory. This app connects to it once with a setup code, and then on every start.",
                "Connect",
                "Connecting",
            )
        };
        let label = |value: &'static str| text(value, t.text.label, c.text_secondary);
        let field = |input: &gpui_kit::Entity<gpui_kit::component::input::InputState>, focused| {
            div()
                .flex()
                .items_center()
                .h(t.component.setup_field)
                .px(t.space.md)
                .bg(c.surface_card)
                .border(t.border_width)
                .border_color(if focused {
                    c.accent_text
                } else {
                    c.border_strong
                })
                .rounded(t.radius.lg)
                .font_family(t.font_mono.clone())
                .child(crate::ui::parts::input(input, t.text.ui).flex_1().min_w_0())
        };
        let block = |d: gpui_kit::Div| d.flex().flex_col().gap(t.space.xs + t.space.xxs);
        let form = div()
            .flex()
            .flex_col()
            .child(setup_heading(title, intro, t))
            .child(
                block(div().pt(t.space.xxl))
                    .child(label("Hub address"))
                    .child(field(&self.inputs.hub_url, url_focused)),
            )
            .child(
                block(div().pt(t.space.lg))
                    .child(label("Setup code"))
                    .child(field(&self.inputs.setup_code, code_focused))
                    .child(text(
                        "The hub prints this code when it starts for the first time. Another paired app can also make one.",
                        detail(t),
                        c.text_tertiary,
                    )),
            )
            .when_some(self.onboarding.error.clone(), |d, err| {
                // The error is selectable, so the user can copy it.
                let id = SharedString::from(format!("onboarding-error-{err}"));
                let error = match self.onboarding.error_title.clone() {
                    Some(title) => failure_card("onboarding-error-title", title, err, None, t),
                    None => error_box(id, err, t),
                };
                d.child(div().pt(t.space.lg).child(error))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(t.space.sm)
                    .pt(t.component.setup_gap)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(t.space.sm)
                            .child(
                                setup_button(
                                    "connect",
                                    if busy { action_busy } else { action },
                                    ButtonKind::Primary,
                                    t,
                                )
                                .when(busy, |b| b.opacity(t.opacity.busy))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if !this.onboarding.busy {
                                        this.connect(window, cx)
                                    }
                                })),
                            )
                            // Back is not drawn in Paper. The plan asks for it.
                            .when(!refused, |d| {
                                d.child(
                                    setup_button(
                                        "connect-back",
                                        "Back",
                                        ButtonKind::Secondary,
                                        t,
                                    )
                                    .debug_selector(|| "connect-back".into())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.onboarding.error = None;
                                        this.onboarding.error_title = None;
                                        this.setup.step = SetupStep::Start;
                                        this.page = Page::Setup;
                                        cx.notify();
                                    })),
                                )
                            }),
                    )
                    .when(!refused, |d| {
                        d.child(
                            link_button("connect-no-hub", "I do not have a hub yet", t)
                                .debug_selector(|| "connect-no-hub".into())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.setup.step = SetupStep::Server;
                                    this.page = Page::Setup;
                                    cx.notify();
                                })),
                        )
                    }),
            );
        setup_frame("onboarding", "onboarding", Dabs::Hub, form, t)
    }
}
