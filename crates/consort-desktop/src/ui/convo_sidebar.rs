//! New channel or group in the sidebar (Paper `FOZ-0`): the "+" on the
//! Tutti row with its menu, the "..." and "+" on a group row, the "Add a
//! channel" row of an empty group, and the drag that reorders the groups.

use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div,
};

use crate::{
    app::ConsortApp,
    convo::{ConvoAction, GroupDrag},
    ui::{
        menu::{MenuAction, MenuItem, OpenMenu},
        parts::{focusable, icon, icon_button, text},
        sidebar::row,
    },
};

/// A small tooltip, as the designs draw them.
pub struct Tip(pub SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = ConsortApp::tokens(cx);
        div()
            .py(t.space.xs)
            .px(t.space.sm)
            .rounded(t.radius.md)
            .bg(t.colors.surface_raised)
            .border(t.border_width)
            .border_color(t.colors.border_subtle)
            .shadow(t.shadow_float())
            .font_family(t.font_sans.clone())
            .type_style(t.text.label)
            .text_color(t.colors.text_secondary)
            .child(self.0.clone())
    }
}

/// What a dragged group shows under the pointer.
pub struct GroupDragView(pub SharedString);

impl Render for GroupDragView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = ConsortApp::tokens(cx);
        div()
            .py(t.space.xs)
            .px(t.space.md)
            .rounded(t.radius.md)
            .bg(t.colors.surface_raised)
            .border(t.border_width)
            .border_color(t.colors.border_strong)
            .shadow(t.shadow_float())
            .font_family(t.font_sans.clone())
            .type_style(t.component.sidebar.label_text)
            .text_color(t.colors.text_primary)
            .child(self.0.clone())
    }
}

/// The items of the Tutti "+" menu.
pub fn new_menu_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new(
            "New channel",
            MenuAction::Convo(ConvoAction::NewChannel(None)),
        ),
        MenuItem::new("New group", MenuAction::Convo(ConvoAction::NewGroup)),
    ]
}

impl ConsortApp {
    /// A small square button of a sidebar row, with a tooltip.
    fn row_button(
        &self,
        id: &'static str,
        name: &str,
        tip: String,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let c = &t.colors;
        // As tall as the row text, so the row keeps its height.
        icon_button(
            id,
            name,
            t.component.sidebar.row_icon + t.space.xs,
            t.icon.dense,
            c.text_secondary,
            c.hover_raised,
            t,
            cx,
        )
        .relative()
        .child(self.triggers.track(id))
        .tooltip(move |_, cx| cx.new(|_| Tip(tip.clone().into())).into())
    }

    /// The "+" of the Tutti row: the menu New channel and New group.
    pub fn tutti_plus(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        self.row_button(
            "tutti-plus",
            "plus",
            "New channel or group".to_owned(),
            t,
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            if let Some(bounds) = this.triggers.get("tutti-plus") {
                this.open_menu(OpenMenu::under(bounds, new_menu_items(), false), cx);
            }
            cx.stop_propagation();
        }))
        .into_any_element()
    }

    /// The "..." and "+" of a group row, while the pointer is on it. Their
    /// slots are always there, so the name and the row do not move.
    pub fn group_buttons(
        &self,
        group_id: &str,
        group_name: &str,
        hovered: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !hovered {
            let button = t.component.sidebar.row_icon + t.space.xs;
            return div()
                .flex_none()
                .w(button * 2.0 + t.space.xxs)
                .into_any_element();
        }
        let menu_group = group_id.to_owned();
        let add_group = group_id.to_owned();
        div()
            .flex()
            .items_center()
            .gap(t.space.xxs)
            .child(
                self.row_button("group-more", "ellipsis", "Group options".to_owned(), t, cx)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(bounds) = this.triggers.get("group-more") {
                            let items = this.group_menu_items(&menu_group);
                            this.open_menu(OpenMenu::under(bounds, items, false), cx);
                        }
                        cx.stop_propagation();
                    })),
            )
            .child(
                self.row_button(
                    "group-plus",
                    "plus",
                    format!("New channel in {group_name}"),
                    t,
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.run_convo(ConvoAction::NewChannel(Some(add_group.clone())), window, cx)
                })),
            )
            .into_any_element()
    }

    /// The hint row of an empty group: "Add a channel".
    pub fn add_channel_row(
        &self,
        group_id: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let sb = t.component.sidebar;
        let group = group_id.to_owned();
        focusable(
            row(t)
                .id(SharedString::from(format!("add-channel-{group_id}")))
                .debug_selector(|| "add-channel".into()),
            t,
        )
        .cursor_pointer()
        .hover(|s| s.bg(c.hover_sidebar))
        .child(icon("plus", sb.row_icon, c.text_tertiary))
        .child(text("Add a channel", sb.row_text, c.text_tertiary))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.run_convo(ConvoAction::NewChannel(Some(group.clone())), window, cx)
        }))
        .into_any_element()
    }

    /// Makes a group label draggable, and a place to drop a dragged group
    /// before it. `before` is the group that the drop goes in front of.
    pub fn group_drag<E>(
        &self,
        label: E,
        group_id: &str,
        name: &str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> E
    where
        E: gpui_kit::StatefulInteractiveElement + Styled,
    {
        let accent = t.colors.accent_text;
        let line = t.component.convo.drag_line;
        let drop_before = group_id.to_owned();
        label
            .on_drag(
                GroupDrag {
                    id: group_id.to_owned(),
                    name: name.to_owned(),
                },
                |drag, _, _, cx| cx.new(|_| GroupDragView(drag.name.clone().into())),
            )
            .drag_over::<GroupDrag>(move |style, _, _, _| style.border_t(line).border_color(accent))
            .on_drop(cx.listener(move |this, drag: &GroupDrag, _, cx| {
                this.drop_group(&drag.id, Some(&drop_before), cx);
            }))
    }

    /// Whether the pointer is on the sidebar row with this key.
    pub fn row_hovered(&self, key: &str) -> bool {
        self.convo.hover.as_deref() == Some(key)
            || self
                .menu
                .as_ref()
                .is_some_and(|m| m.items.iter().any(|i| owns(&i.action, key)))
    }

    /// Keeps the hovered row in step with the pointer.
    pub fn set_row_hover(&mut self, key: &str, on: bool, cx: &mut Context<Self>) {
        if on {
            self.convo.hover = Some(key.to_owned());
        } else if self.convo.hover.as_deref() == Some(key) {
            self.convo.hover = None;
        }
        cx.notify();
    }
}

/// Whether an open menu belongs to the row `key`, so its buttons stay while
/// the menu is open.
fn owns(action: &MenuAction, key: &str) -> bool {
    match action {
        MenuAction::Convo(ConvoAction::NewGroup) => key == "tutti",
        MenuAction::Convo(ConvoAction::RenameGroup(id)) => id == key,
        _ => false,
    }
}
