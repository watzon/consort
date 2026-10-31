//! Settings > Providers (Paper `PSH-0`): the harness tools, the accounts and
//! the API endpoints. All of them are on the hub machine.

use consort_client::{ProviderSection, ToolState, provider_sections};
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::app::{ConsortApp, Dialog};
use crate::ui::admin_parts::{column, header, hint, page};
use crate::ui::menu::{MenuItem, OpenMenu};
use crate::ui::parts::{ButtonKind, button, icon, icon_button, provider_logo, text};
use crate::ui::workers::row_list;

impl ConsortApp {
    pub fn render_providers(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let host = self.state.session_host.clone();
        let online = host.as_ref().is_some_and(|h| h.online);
        let label_color = if online {
            c.text_primary
        } else {
            c.text_tertiary
        };
        let host_id = host.as_ref().map(|h| h.id.clone());
        let add = button("add-account", "Add account", ButtonKind::Secondary, t)
            .child(icon("plus", t.icon.dense, label_color))
            .flex_row_reverse()
            .when(!online, |b| b.text_color(label_color).cursor_default())
            .when_some(host_id.filter(|_| online), |b, id| {
                b.on_click(cx.listener(move |this, _, window, cx| {
                    this.open_dialog(Dialog::AddAccount(id.clone()), window, cx)
                }))
            });
        let line = text(
            "Consort runs these agents on the hub machine. Each account signs in here. A worker holds no sign-in.",
            t.text.ui,
            c.text_secondary,
        );
        let mut body = column(t).child(header(
            "Providers",
            div().child(line),
            Some(add.into_any_element()),
            t,
        ));
        match &host {
            None => {
                body = body.child(empty_card(
                    "The session host is not paired",
                    "The session host runs the agents on the hub machine. Start it with `consort session-host run`.",
                    t,
                ));
            }
            Some(host) => {
                if !host.online {
                    body = body.child(hint(
                        "The session host on the hub machine is offline. You can change accounts when it is online again.",
                        t,
                    ));
                }
                let sections = provider_sections(host);
                if sections.is_empty() {
                    body = body.child(empty_card(
                        "No account is signed in",
                        "Use Add account at the top of this page to sign in to a provider.",
                        t,
                    ));
                }
                for section in sections {
                    body = body.child(self.provider_section(host, &section, t, cx));
                }
            }
        }
        let endpoints = self.endpoints_section(t, cx);
        page("providers-page", t)
            .child(body.child(endpoints))
            .into_any_element()
    }

    fn provider_section(
        &mut self,
        host: &consort_client::WorkerView,
        section: &ProviderSection,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let mut list = row_list(t);
        let count = section.accounts.len();
        let tool_missing = section
            .tool
            .as_ref()
            .is_some_and(|tool| tool.state == ToolState::Missing);
        if let Some(tool) = &section.tool {
            list = list.child(self.tool_row(host, tool, count == 0, t, cx));
        }
        for (index, account) in section.accounts.iter().enumerate() {
            list = list.child(self.account_row(
                host,
                account,
                index + 1 == count,
                tool_missing,
                t,
                cx,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(provider_logo(
                        &section.logo,
                        t.component.provider_logo,
                        c.text_primary,
                    ))
                    .child(text(section.name.clone(), t.text.title, c.text_primary).min_w_0()),
            )
            .child(list)
    }
}

fn empty_card(title: &str, line: &str, t: &Tokens) -> Div {
    let c = &t.colors;
    row_list(t).child(
        div()
            .flex()
            .flex_col()
            .gap(t.space.xxs)
            .py(t.space.md)
            .px(t.space.lg)
            .child(text(title.to_owned(), t.text.ui_strong, c.text_primary))
            .child(text(line.to_owned(), t.text.ui, c.text_secondary)),
    )
}

/// The three dots button at the end of a row. It opens the same menu as a
/// right click on the row.
pub fn dots_button(
    id: impl Into<SharedString>,
    items: Vec<MenuItem>,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    let id: SharedString = id.into();
    let selector = id.to_string();
    icon_button(
        gpui_kit::ElementId::Name(id),
        "more-horizontal",
        t.component.row_dots,
        t.icon.dense,
        c.text_tertiary,
        c.hover_raised,
        t,
        cx,
    )
    .debug_selector(move || selector.clone())
    .on_click(
        cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
            this.open_menu(OpenMenu::at(event.position(), items.clone()), cx);
            cx.stop_propagation();
        }),
    )
}
