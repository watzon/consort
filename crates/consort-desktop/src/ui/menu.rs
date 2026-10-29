//! Menus and dropdowns. One layer for every right-click menu and select, so
//! they look the same and all flip: a menu opens downward when there is room
//! and upward when there is not (GPUI's `anchored` with anchor switching).

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    Anchor, AnyElement, App, Bounds, Context, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Point, SharedString, StatefulInteractiveElement as _, Styled, Window,
    anchored, canvas, deferred, div, point, prelude::FluentBuilder as _,
};

use super::parts::icon;

/// What a menu item does. The app runs it in `ConsortApp::run_menu`.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Copy(String),
    QuoteInReply(String),
    AskConsort(String),
    CopyMarkdown(String),
    ReplyInThread(String),
    /// Edit a message in place: its ID and its text.
    EditMessage(String, String),
    DeleteMessage(String),
    OpenThread(String),
    ResolveTask(String),
    ReopenTask(String),
    RetryTask(String),
    StopTask(String),
    RenameChannel(String),
    ChannelInstructions(String),
    ChangeIcon(String),
    SetIcon(String, String),
    MoveToGroup(String),
    SetGroup(String, String),
    ArchiveChannel(String),
    DeleteChannel(String),
    RenameWorker(String),
    EditLabels(String),
    PauseWorker(String, bool),
    RemoveWorker(String),
    /// Cut, Copy, Paste or Select all in one of the message boxes.
    TextEdit(TextTarget, EditOp),
    /// Pick the option with the stable ID `option` in the select `id`. The ID
    /// is the ID of the thing picked, so a state change between the opening
    /// of the menu and the click cannot move the pick to another row.
    Select(&'static str, String),
}

/// The message box that a Cut, Copy, Paste or Select all acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextTarget {
    /// The composer under the timeline.
    Composer,
    /// The reply box in a task thread.
    Reply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditOp {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// The menu of a message box: Cut, Copy, Paste, Select all.
pub fn text_edit_menu(target: TextTarget) -> Vec<MenuItem> {
    vec![
        MenuItem::new("Cut", MenuAction::TextEdit(target, EditOp::Cut)),
        MenuItem::new("Copy", MenuAction::TextEdit(target, EditOp::Copy)),
        MenuItem::new("Paste", MenuAction::TextEdit(target, EditOp::Paste)),
        MenuItem::new(
            "Select all",
            MenuAction::TextEdit(target, EditOp::SelectAll),
        )
        .separated(),
    ]
}

#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: SharedString,
    pub action: MenuAction,
    pub danger: bool,
    pub checked: bool,
    pub icon: Option<&'static str>,
    pub separator_before: bool,
}

impl MenuItem {
    pub fn new(label: impl Into<SharedString>, action: MenuAction) -> MenuItem {
        MenuItem {
            label: label.into(),
            action,
            danger: false,
            checked: false,
            icon: None,
            separator_before: false,
        }
    }

    pub fn danger(mut self) -> MenuItem {
        self.danger = true;
        self
    }

    pub fn checked(mut self, checked: bool) -> MenuItem {
        self.checked = checked;
        self
    }

    pub fn icon(mut self, name: &'static str) -> MenuItem {
        self.icon = Some(name);
        self
    }

    pub fn separated(mut self) -> MenuItem {
        self.separator_before = true;
        self
    }
}

/// Where a menu opens.
#[derive(Clone, Copy, Debug)]
pub enum MenuAnchor {
    /// At the pointer, for a right click.
    Point(Point<Pixels>),
    /// Under a trigger, for a select or a "more" button. It opens above the
    /// trigger when there is no room below.
    Trigger(Bounds<Pixels>),
}

#[derive(Clone, Debug)]
pub struct OpenMenu {
    pub anchor: MenuAnchor,
    pub items: Vec<MenuItem>,
    pub width: Option<Pixels>,
    pub highlighted: Option<usize>,
}

impl OpenMenu {
    pub fn at(position: Point<Pixels>, items: Vec<MenuItem>) -> OpenMenu {
        OpenMenu {
            anchor: MenuAnchor::Point(position),
            items,
            width: None,
            highlighted: None,
        }
    }

    pub fn under(bounds: Bounds<Pixels>, items: Vec<MenuItem>, match_width: bool) -> OpenMenu {
        OpenMenu {
            anchor: MenuAnchor::Trigger(bounds),
            items,
            width: match_width.then_some(bounds.size.width),
            highlighted: None,
        }
    }

    /// Moves the keyboard highlight by `delta` rows.
    pub fn step(&mut self, delta: isize) {
        let len = self.items.len() as isize;
        if len == 0 {
            return;
        }
        let next = match self.highlighted {
            None if delta > 0 => 0,
            None => len - 1,
            Some(i) => (i as isize + delta).rem_euclid(len),
        };
        self.highlighted = Some(next as usize);
    }
}

/// The bounds of named triggers, recorded while they paint, so a select can
/// open under its trigger.
#[derive(Clone, Default)]
pub struct TriggerBounds(Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>);

impl TriggerBounds {
    pub fn get(&self, id: &str) -> Option<Bounds<Pixels>> {
        self.0.borrow().get(id).copied()
    }

    /// An invisible child that records the bounds of its parent.
    pub fn track(&self, id: &'static str) -> impl IntoElement {
        let store = self.0.clone();
        canvas(
            move |bounds, _, _| {
                store.borrow_mut().insert(id, bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// Renders the open menu above everything, with a layer behind it that
/// closes it on a click outside.
pub fn render_menu<V: 'static>(
    menu: &OpenMenu,
    t: &Tokens,
    window: &Window,
    cx: &mut Context<V>,
    on_pick: impl Fn(&mut V, MenuAction, &mut Window, &mut Context<V>) + Clone + 'static,
    on_close: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let c = &t.colors;
    let gap = t.space.xs;
    let (position, pad) = match menu.anchor {
        MenuAnchor::Point(p) => (p, Pixels::ZERO),
        // From the middle of the trigger, padded by half its height and the
        // gap, the menu clears the trigger whichever way it opens.
        MenuAnchor::Trigger(b) => (
            point(b.origin.x, b.origin.y + b.size.height / 2.0),
            b.size.height / 2.0 + gap,
        ),
    };
    let mut list = div()
        .id("menu")
        .occlude()
        .flex()
        .flex_col()
        .p(t.space.xs)
        .min_w(t.layout.speaker_gutter * 3.0)
        .when_some(menu.width, |d, w| d.w(w))
        .bg(c.surface_raised)
        .border(t.border_width)
        .border_color(c.border_subtle)
        .rounded(t.radius.lg)
        .shadow(t.shadow_float())
        .font_family(t.font_sans.clone())
        .type_style(t.text.ui);
    for (ix, item) in menu.items.iter().enumerate() {
        if item.separator_before && ix > 0 {
            list = list.child(
                div()
                    .my(t.space.xs)
                    .mx(t.space.sm)
                    .h(t.border_width)
                    .bg(c.border_subtle),
            );
        }
        let action = item.action.clone();
        let pick = on_pick.clone();
        let color = if item.danger {
            c.status_failed
        } else {
            c.text_primary
        };
        let highlighted = menu.highlighted == Some(ix);
        let hover = if item.danger {
            c.tint_failed
        } else {
            c.surface_selected
        };
        list = list.child(
            div()
                .id(("menu-item", ix))
                .flex()
                .items_center()
                .gap(t.space.sm)
                .h(t.space.xxl - t.space.xs)
                .px(t.space.sm)
                .rounded(t.radius.md)
                .text_color(color)
                .cursor_pointer()
                .when(highlighted, |d| d.bg(hover))
                .hover(move |s| s.bg(hover))
                .when_some(item.icon, |d, name| {
                    d.child(icon(name, t.icon.size, c.text_secondary))
                })
                .child(div().flex_1().whitespace_nowrap().child(item.label.clone()))
                .when(item.checked, |d| {
                    d.child(icon("check", t.icon.dense, c.accent_text))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    pick(this, action.clone(), window, cx);
                })),
        );
    }
    let close = on_close.clone();
    let viewport = window.viewport_size();
    let catcher = div()
        .id("menu-catcher")
        .absolute()
        .top_0()
        .left_0()
        .w(viewport.width)
        .h(viewport.height)
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| close(this, window, cx)),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _, window, cx| on_close(this, window, cx)),
        );
    deferred(
        anchored()
            .position(point(Pixels::ZERO, Pixels::ZERO))
            .child(
                div()
                    .w(viewport.width)
                    .h(viewport.height)
                    .child(catcher)
                    .child(
                        anchored()
                            .position(position)
                            .anchor(Anchor::TopLeft)
                            .child(div().py(pad).child(list)),
                    ),
            ),
    )
    .with_priority(2)
    .into_any_element()
}

/// Keeps `App` in the import list for doc links on every platform.
#[allow(dead_code)]
fn _app(_: &App) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_box_has_the_same_menu_in_the_same_order() {
        for target in [TextTarget::Composer, TextTarget::Reply] {
            let items = text_edit_menu(target);
            let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
            assert_eq!(labels, ["Cut", "Copy", "Paste", "Select all"]);
            assert!(
                items
                    .iter()
                    .all(|i| matches!(i.action, MenuAction::TextEdit(t, _) if t == target))
            );
            assert!(items[3].separator_before);
        }
    }

    #[test]
    fn the_keyboard_steps_through_the_rows_and_wraps() {
        let mut menu = OpenMenu::at(
            point(Pixels::ZERO, Pixels::ZERO),
            text_edit_menu(TextTarget::Composer),
        );
        menu.step(1);
        assert_eq!(menu.highlighted, Some(0));
        menu.step(-1);
        assert_eq!(menu.highlighted, Some(3));
        menu.step(1);
        assert_eq!(menu.highlighted, Some(0));
    }
}
