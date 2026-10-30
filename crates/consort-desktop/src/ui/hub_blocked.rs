//! Controls that need the hub, while the hub is offline (Paper `ERQ-0`).
//!
//! The user cannot answer an approval or a decision without the hub. Their
//! buttons are dimmed, and a tooltip says why. A click does nothing but show
//! the same reason. A message is different: it waits in the outbox.

use consort_theme::{ActiveTokens as _, TypeExt as _};
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Div, IntoElement, ParentElement as _, Render,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::app::ConsortApp;

/// The tooltip of a dimmed control: one line that says why.
pub struct ReasonTip {
    reason: SharedString,
}

impl Render for ReasonTip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.tokens().clone();
        let c = &t.colors;
        div()
            .px(t.space.sm)
            .py(t.space.xs)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.md)
            .shadow(t.shadow_float())
            .font_family(t.font_sans.clone())
            .type_style(t.text.caption)
            .text_color(c.text_primary)
            .whitespace_nowrap()
            .child(self.reason.clone())
    }
}

fn tip(reason: &'static str, cx: &mut App) -> AnyView {
    cx.new(|_| ReasonTip {
        reason: reason.into(),
    })
    .into()
}

/// A button that needs the hub. With a `reason` (the hub is not there), it
/// is dimmed, and its tooltip says why. Without one, it is as it was.
pub fn needs_hub(
    button: Stateful<Div>,
    reason: Option<&'static str>,
    t: &consort_theme::Tokens,
) -> Stateful<Div> {
    match reason {
        None => button,
        Some(reason) => button
            .opacity(t.opacity.offline_dim)
            .tooltip(move |_, cx| tip(reason, cx)),
    }
}

impl ConsortApp {
    /// Why the user cannot answer an approval or a decision now, or `None`.
    pub fn answers_blocked(&self) -> Option<&'static str> {
        self.state.answers_blocked()
    }

    /// Stops an answer when the hub is not there, and tells the user why.
    /// Returns true when the answer must not go on.
    pub fn blocked_by_hub(&mut self, cx: &mut Context<Self>) -> bool {
        match self.answers_blocked() {
            Some(reason) => {
                self.show_notice(reason.to_owned(), cx);
                true
            }
            None => false,
        }
    }
}
