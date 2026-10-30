//! The All icons dialog (Paper `IIJ-0`, `IQF-0`, `IAN-0`) and the icon tile
//! that it shares with the quick row of the New channel dialog and the
//! channel settings.
//!
//! The dialog opens over the dialog that opened it, or alone from Change
//! icon in a channel's menu. A second scrim dims what is under it. Escape,
//! Cancel, the close button and a click on the scrim close only this
//! dialog. A click or Enter picks an icon and closes it.
//!
//! A tile under the pointer shows the active state: the accent tint, the
//! icon in `text.primary`, and the shimmer through its three hand-drawn
//! versions. The picked tile has the quiet selection style and does not
//! shimmer, so the accent stays the color of hover and focus.

use consort_theme::TypeExt as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Div, ElementId, Focusable as _, InteractiveElement as _,
    IntoElement as _, ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled, Window, div, prelude::FluentBuilder as _,
};

use consort_theme::Tokens;

use crate::{
    app::ConsortApp,
    convo::{ConvoAction, IconPicker, IconTarget},
    ui::{
        convo_sidebar::Tip,
        icon_catalog::{self, ResultGroup},
        parts::{ButtonKind, button, focusable, icon_button, icon_version, text},
    },
};

/// The tiles in one row of the list. The tokens give it: a 560 px dialog
/// with `space.xl` padding leaves 512 px, and 11 tiles of 40 px with 6 px
/// gaps take 500 px.
pub const COLUMNS: usize = 11;

/// The hint under the list.
pub const HINT: &str = "Click an icon to pick it. Press Esc to close.";

/// One icon tile: at rest no ground; on hover the accent tint, the icon in
/// `text.primary` and the shimmer; when picked the selection ground and a
/// border; with the keyboard on it the focus ring.
#[allow(clippy::too_many_arguments)]
pub fn icon_tile(
    id: impl Into<ElementId>,
    name: &str,
    size: gpui_kit::Pixels,
    icon_size: gpui_kit::Pixels,
    selected: bool,
    keyboard: bool,
    t: &Tokens,
    cx: &gpui_kit::App,
) -> Stateful<Div> {
    let c = &t.colors;
    let id: ElementId = id.into();
    let hovered = crate::ui::motion::is_hovered(&id, cx);
    let version = crate::ui::motion::shimmer_version(hovered, cx);
    let hover_id = id.clone();
    let color = if selected || hovered {
        c.text_primary
    } else {
        c.text_secondary
    };
    focusable(div().id(id), t)
        .on_hover(move |on, window, cx| {
            if let Some(clock) = cx.try_global::<crate::ui::motion::FrameClock>() {
                clock.set_hovered(hover_id.clone(), *on);
            }
            window.refresh();
        })
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(size)
        .rounded(t.radius.md)
        .border(t.border_width)
        .border_color(if selected {
            c.text_secondary
        } else {
            gpui_kit::transparent_black()
        })
        .when(selected, |d| d.bg(c.surface_selected))
        .when(hovered, |d| d.bg(c.tint_accent))
        .when(keyboard, |d| d.shadow(t.focus_ring()))
        .cursor_pointer()
        .child(icon_version(name, version, icon_size, color))
}

/// Where each icon of a result sits in the list: its row among all rows
/// (labels included) and its column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    /// The child of the list that holds the icon's row.
    pub child: usize,
    /// The row among the tile rows only, for the arrow keys.
    pub row: usize,
    pub col: usize,
}

/// The slot of every icon of `groups`, in the order of [`icon_catalog::flatten`].
pub fn slots(groups: &[ResultGroup]) -> Vec<Slot> {
    let mut out = Vec::new();
    let (mut child, mut row) = (0, 0);
    for group in groups {
        // The label of the category is one child.
        child += 1;
        for (ix, _) in group.icons.iter().enumerate() {
            out.push(Slot {
                child: child + ix / COLUMNS,
                row: row + ix / COLUMNS,
                col: ix % COLUMNS,
            });
        }
        let rows = group.icons.len().div_ceil(COLUMNS);
        child += rows;
        row += rows;
    }
    out
}

/// Where an arrow key moves the keyboard in the list. `None` from the first
/// row going up: the focus goes back to the search field. Left and right go
/// along the list; up and down go to the row above or below, to the same
/// column or the last tile of a shorter row.
pub fn step(slots: &[Slot], from: usize, key: &str) -> Option<Option<usize>> {
    let here = *slots.get(from)?;
    let last = slots.len().checked_sub(1)?;
    let to = match key {
        "left" => from.saturating_sub(1),
        "right" => (from + 1).min(last),
        "up" | "down" => {
            let row = match key {
                "up" if here.row == 0 => return Some(None),
                "up" => here.row - 1,
                _ => here.row + 1,
            };
            let in_row: Vec<usize> = (0..slots.len()).filter(|&i| slots[i].row == row).collect();
            match in_row.iter().find(|&&i| slots[i].col == here.col) {
                Some(&i) => i,
                None => match in_row.last() {
                    Some(&i) => i,
                    None => from,
                },
            }
        }
        _ => return None,
    };
    Some(Some(to))
}

impl ConsortApp {
    /// Opens the All icons dialog for `target`, with an empty search that
    /// has the focus.
    pub fn open_icon_picker(
        &mut self,
        target: IconTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        self.convo.icon_picker = Some(IconPicker {
            target,
            focus: None,
        });
        let search = self.convo_inputs.icon_search.clone();
        search.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        // Show the icon that is picked now.
        let current = self.icon_picker_current(cx);
        let results = icon_catalog::search("");
        if let Some(ix) = icon_catalog::flatten(&results)
            .iter()
            .position(|n| *n == current)
        {
            self.convo
                .icon_scroll
                .scroll_to_item(slots(&results)[ix].child);
        }
        cx.notify();
    }

    /// Closes the All icons dialog with no change.
    pub fn close_icon_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.convo.icon_picker.take().is_some() {
            self.after_icon_picker(window, cx);
        }
        cx.notify();
    }

    /// After the All icons dialog closed: the search is empty again, and the
    /// keys go back to the app.
    pub fn after_icon_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.convo_inputs
            .icon_search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.focus.focus(window, cx);
    }

    /// The icon that is picked now for the target of the open dialog.
    fn icon_picker_current(&self, _cx: &Context<Self>) -> String {
        let name = match self.convo.icon_picker.as_ref().map(|p| &p.target) {
            Some(IconTarget::NewChannel) => self.convo.new_channel_icon.clone(),
            Some(IconTarget::ChannelSettings) => self.convo.settings.icon.clone(),
            Some(IconTarget::Channel(id)) => self
                .state
                .channel(id)
                .map(|c| c.icon.clone())
                .unwrap_or_default(),
            None => String::new(),
        };
        crate::ui::icons::resolve(&name).to_owned()
    }

    /// The keys of the open All icons dialog. True when the key was for it.
    pub fn icon_picker_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(picker) = self.convo.icon_picker.clone() else {
            return false;
        };
        if key == "escape" {
            self.close_icon_picker(window, cx);
            return true;
        }
        let query = self.convo_inputs.icon_search.read(cx).value().to_string();
        let results = icon_catalog::search(&query);
        let icons = icon_catalog::flatten(&results);
        let slots = slots(&results);
        match (picker.focus, key) {
            // From the search field, only Down goes into the list. Left
            // and right move in the text.
            (None, "down") if !icons.is_empty() => {
                self.move_icon_focus(Some(0), &slots, window, cx)
            }
            (None, _) => return false,
            (Some(ix), "enter") => {
                if let Some(name) = icons.get(ix) {
                    self.run_convo(ConvoAction::PickIcon((*name).to_owned()), window, cx);
                }
            }
            (Some(ix), "left" | "right" | "up" | "down") => match step(&slots, ix, key) {
                Some(to) => self.move_icon_focus(to, &slots, window, cx),
                None => return false,
            },
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Moves the keyboard to a tile, or back to the search field.
    fn move_icon_focus(
        &mut self,
        to: Option<usize>,
        slots: &[Slot],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(picker) = self.convo.icon_picker.as_mut() {
            picker.focus = to;
        }
        match to {
            Some(ix) => {
                // The list keys come to the app, not to the search field.
                self.focus.focus(window, cx);
                if let Some(slot) = slots.get(ix) {
                    self.convo.icon_scroll.scroll_to_item(slot.child);
                }
            }
            None => {
                let search = self.convo_inputs.icon_search.clone();
                search.update(cx, |input, cx| input.focus(window, cx));
            }
        }
    }

    /// The All icons dialog, or `None` when it is closed.
    pub fn render_icon_picker(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let picker = self.convo.icon_picker.clone()?;
        let c = &t.colors;
        let v = t.component.convo;
        let query = self.convo_inputs.icon_search.read(cx).value().to_string();
        let searching = !query.trim().is_empty();
        let results = icon_catalog::search(&query);
        let count = icon_catalog::flatten(&results).len();
        let current = self.icon_picker_current(cx);
        let search_focused = self
            .convo_inputs
            .icon_search
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);

        // The search field, with the count and a clear button while there
        // is a word.
        let search = div()
            .flex()
            .items_center()
            .gap(t.space.sm)
            .py(t.component.input_pad_y)
            .px(t.space.md)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(if search_focused {
                c.accent_text
            } else {
                c.border_strong
            })
            .rounded(t.radius.md)
            .child(crate::ui::parts::icon(
                "search",
                t.icon.dense,
                c.text_tertiary,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .type_style(t.text.ui)
                    .text_color(c.text_primary)
                    .child(crate::ui::parts::input(
                        &self.convo_inputs.icon_search,
                        t.text.ui,
                    )),
            )
            .when(searching, |d| {
                d.child(text(
                    icon_catalog::count_text(count),
                    t.text.label,
                    c.text_tertiary,
                ))
                .child(
                    icon_button(
                        "icon-search-clear",
                        "close",
                        t.space.lg + t.space.xs,
                        t.icon.dense,
                        c.text_secondary,
                        c.hover_card,
                        t,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        let search = this.convo_inputs.icon_search.clone();
                        search.update(cx, |input, cx| {
                            input.set_value("", window, cx);
                            input.focus(window, cx);
                        });
                        if let Some(picker) = this.convo.icon_picker.as_mut() {
                            picker.focus = None;
                        }
                        cx.notify();
                    })),
                )
            });

        // The list: a label row for each category, then rows of tiles. Each
        // row is its own child of the scroll, so the arrow keys can bring a
        // row into view. The rows carry the padding, so the focus ring of a
        // tile at the edge is not cut.
        let mut rows: Vec<AnyElement> = Vec::new();
        if results.is_empty() {
            rows.push(
                div()
                    .h(v.icon_list_height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(text(
                        format!("No icon matches \"{}\". Try another word.", query.trim()),
                        t.text.ui,
                        c.text_secondary,
                    ))
                    .into_any_element(),
            );
        }
        let mut ix = 0;
        for (n, group) in results.iter().enumerate() {
            rows.push(
                text(group.title, t.text.label, c.text_tertiary)
                    .px(t.space.xs)
                    .pt(if n == 0 { t.space.xs } else { t.space.md })
                    .pb(t.space.sm)
                    .into_any_element(),
            );
            for chunk in group.icons.chunks(COLUMNS) {
                let mut row = div()
                    .flex()
                    .gap(v.icon_tile_gap)
                    .px(t.space.xs)
                    .pb(v.icon_tile_gap);
                for name in chunk {
                    let name: &'static str = name;
                    let keyboard = picker.focus == Some(ix);
                    let label = icon_catalog::label(name);
                    row = row.child(
                        icon_tile(
                            SharedString::from(format!("icon-tile-{name}")),
                            name,
                            v.icon_tile,
                            v.icon_tile_icon,
                            current == name,
                            keyboard,
                            t,
                            cx,
                        )
                        .debug_selector(move || format!("icon-tile-{name}"))
                        .tooltip(move |_, cx| cx.new(|_| Tip(label.clone().into())).into())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.run_convo(ConvoAction::PickIcon(name.to_owned()), window, cx)
                            },
                        )),
                    );
                    ix += 1;
                }
                rows.push(row.into_any_element());
            }
        }
        let list = crate::ui::ext_parts::faded_scroll_rows(
            "icon-list",
            &self.convo.icon_scroll.clone(),
            v.icon_list_height,
            rows,
            c.surface_raised,
            c.raised_clear,
            t,
            cx,
        )
        .debug_selector(|| "icon-list".into());

        let card = div()
            .id("icon-picker")
            .debug_selector(|| "icon-picker".into())
            .occlude()
            .flex()
            .flex_col()
            .gap(t.space.lg)
            .w(v.icon_picker_width)
            .p(t.space.xl)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.xl)
            .shadow(t.shadow_float())
            .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation()
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .child(text("All icons", t.text.heading, c.text_primary)),
                    )
                    .child(
                        icon_button(
                            "icon-picker-close",
                            "close",
                            t.space.xl + t.space.xs,
                            t.icon.size,
                            c.text_secondary,
                            c.hover_raised,
                            t,
                            cx,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.close_icon_picker(window, cx)),
                        ),
                    ),
            )
            .child(search)
            .child(list)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.md)
                    .child(
                        div()
                            .flex_1()
                            .child(text(HINT, t.text.label, c.text_tertiary)),
                    )
                    .child(
                        button("icon-picker-cancel", "Cancel", ButtonKind::Secondary, t).on_click(
                            cx.listener(|this, _, window, cx| this.close_icon_picker(window, cx)),
                        ),
                    ),
            );
        Some(
            div()
                .id("icon-picker-scrim")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(c.scrim)
                .on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_icon_picker(window, cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_follow_the_rows_of_each_category() {
        let groups = icon_catalog::search("");
        let slots = slots(&groups);
        assert_eq!(slots.len(), 85);
        // Work and code has 16 icons: a label, then rows of 11 and 5.
        assert_eq!(
            slots[0],
            Slot {
                child: 1,
                row: 0,
                col: 0
            }
        );
        assert_eq!(
            slots[11],
            Slot {
                child: 2,
                row: 1,
                col: 0
            }
        );
        assert_eq!(
            slots[15],
            Slot {
                child: 2,
                row: 1,
                col: 4
            }
        );
        // Machines starts after its own label.
        assert_eq!(
            slots[16],
            Slot {
                child: 4,
                row: 2,
                col: 0
            }
        );
    }

    #[test]
    fn the_arrows_move_by_tile_and_by_row() {
        let groups = icon_catalog::search("");
        let s = slots(&groups);
        assert_eq!(step(&s, 0, "right"), Some(Some(1)));
        assert_eq!(step(&s, 0, "left"), Some(Some(0)));
        assert_eq!(step(&s, 3, "down"), Some(Some(14)));
        // Row 1 has 5 tiles: from column 8 the last one of the row.
        assert_eq!(step(&s, 8, "down"), Some(Some(15)));
        // Down from row 1 (5 tiles) goes into Machines.
        assert_eq!(step(&s, 12, "down"), Some(Some(17)));
        assert_eq!(step(&s, 14, "up"), Some(Some(3)));
        // Up from the first row goes back to the search field.
        assert_eq!(step(&s, 4, "up"), Some(None));
        assert_eq!(step(&s, 84, "right"), Some(Some(84)));
        assert_eq!(step(&s, 0, "tab"), None);
    }
}
