//! Choose channels or tags for a connection or a skill: the dialog behind the
//! third segment of the Use in control. It opens over the dialog or the page
//! that opened it. Nothing changes until Save.

use consort_client::{ChannelKind, use_in_match_note};
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _,
};

use crate::{
    app::ConsortApp,
    extensions::{UseInPicker, UseInPickerTab, scope_tags, toggle_in},
    ui::{
        ext_parts::faded_scroll,
        parts::{ButtonKind, button, focusable, icon, icon_button, text},
        voice::check_mark,
    },
};

impl ConsortApp {
    fn close_use_in_picker(&mut self, cx: &mut Context<Self>) {
        self.ext.use_in_picker = None;
        cx.notify();
    }

    /// Saves the choice of the open dialog. An empty choice has no Save.
    fn save_use_in_picker(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.ext.use_in_picker.take() else {
            return;
        };
        let use_in = picker.use_in();
        self.set_use_in(picker.target, use_in, cx);
    }

    /// Escape closes only this dialog.
    pub fn use_in_picker_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        if self.ext.use_in_picker.is_none() {
            return false;
        }
        if key == "escape" {
            self.close_use_in_picker(cx);
        }
        // The keys do not reach the dialog or the page under it.
        true
    }

    /// One row of the list: a check box, and what it stands for.
    fn use_in_row(
        &self,
        id: String,
        checked: bool,
        content: gpui_kit::Div,
        t: &Tokens,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut UseInPicker) + 'static,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let c = &t.colors;
        let selector = format!("use-in-row-{id}");
        focusable(div().id(SharedString::from(selector.clone())), t)
            .debug_selector(move || selector)
            .flex()
            .flex_none()
            .items_center()
            .gap(t.space.sm)
            .py(t.space.xs + t.space.xxs)
            .px(t.space.sm)
            .rounded(t.radius.md)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_card))
            .child(check_mark(checked, t))
            .child(content)
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(picker) = this.ext.use_in_picker.as_mut() {
                    on_click(picker);
                    cx.notify();
                }
            }))
    }

    /// The dialog, when it is open.
    pub fn render_use_in_picker(
        &mut self,
        t: &Tokens,
        window: &gpui_kit::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let picker = self.ext.use_in_picker.clone()?;
        let c = &t.colors;
        let state = self.state.clone();
        let mut list = div().flex().flex_col().gap(t.space.xxs);
        match picker.tab {
            UseInPickerTab::Channels => {
                for channel in &state.channels {
                    let id = channel.id.clone();
                    let leading = if channel.kind == ChannelKind::Tutti {
                        crate::ui::mark::still_mark(t.icon.size, t)
                    } else {
                        icon(&channel.icon, t.icon.size, c.text_secondary).into_any_element()
                    };
                    list = list.child(
                        self.use_in_row(
                            id.clone(),
                            picker.channels.contains(&id),
                            div()
                                .flex()
                                .items_center()
                                .gap(t.space.sm)
                                .child(leading)
                                .child(text(channel.name.clone(), t.text.ui, c.text_primary)),
                            t,
                            cx,
                            move |p| toggle_in(&mut p.channels, &id),
                        ),
                    );
                }
            }
            UseInPickerTab::Tags => {
                let tags = scope_tags(&state);
                if tags.is_empty() {
                    list = list.child(text(
                        "No channel has a tag yet. Give a channel a tag with Worker tags in its menu, then choose it here.",
                        t.text.ui,
                        c.text_secondary,
                    ));
                }
                for tag in tags {
                    let value = tag.clone();
                    list = list.child(self.use_in_row(
                        tag.clone(),
                        picker.tags.contains(&tag),
                        text(tag, t.text.ui, c.text_primary),
                        t,
                        cx,
                        move |p| toggle_in(&mut p.tags, &value),
                    ));
                }
            }
        }
        let tab = |id: &'static str,
                   label: &'static str,
                   value: UseInPickerTab,
                   cx: &mut Context<Self>| {
            let active = picker.tab == value;
            focusable(div().id(id), t)
                .debug_selector(move || id.to_owned())
                .flex()
                .flex_1()
                .justify_center()
                .py(t.space.xs)
                .rounded(t.radius.sm)
                .cursor_pointer()
                .when(active, |d| d.bg(c.surface_selected))
                .when(!active, |d| d.hover(|s| s.bg(c.hover_card)))
                .child(text(
                    label,
                    if active { t.text.ui_strong } else { t.text.ui },
                    if active {
                        c.text_primary
                    } else {
                        c.text_secondary
                    },
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(picker) = this.ext.use_in_picker.as_mut() {
                        picker.tab = value;
                        this.ext.use_in_scroll = gpui_kit::ScrollHandle::new();
                        cx.notify();
                    }
                }))
        };
        let tabs = div()
            .flex()
            .flex_none()
            .gap(t.space.xxs)
            .p(t.space.xxs)
            .bg(c.surface_card)
            .rounded(t.radius.md)
            .child(tab(
                "use-in-tab-channels",
                "Channels",
                UseInPickerTab::Channels,
                cx,
            ))
            .child(tab("use-in-tab-tags", "Tags", UseInPickerTab::Tags, cx));
        let note = if picker.is_empty() {
            match picker.tab {
                UseInPickerTab::Channels => "Choose at least one channel.".to_owned(),
                UseInPickerTab::Tags => {
                    "Choose at least one tag. A channel must have every tag that you choose."
                        .to_owned()
                }
            }
        } else {
            use_in_match_note(&picker.use_in(), &state.channels)
        };
        let can_save = !picker.is_empty();
        let save = button("use-in-save", "Save", ButtonKind::Primary, t)
            .debug_selector(|| "use-in-save".into())
            .when(!can_save, |d| d.opacity(t.opacity.nav_disabled))
            .when(can_save, |d| {
                d.on_click(cx.listener(|this, _, _, cx| this.save_use_in_picker(cx)))
            });
        let card = div()
            .id("use-in-picker")
            .debug_selector(|| "use-in-picker".into())
            .occlude()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .w(t.component.convo.dialog_small)
            .max_w(window.viewport_size().width - t.space.page)
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
                    .flex_none()
                    .items_center()
                    .child(div().flex_1().child(text(
                        "Choose where to use it",
                        t.text.heading,
                        c.text_primary,
                    )))
                    .child(
                        icon_button(
                            "use-in-close",
                            "close",
                            t.space.xl + t.space.xs,
                            t.icon.size,
                            c.text_secondary,
                            c.hover_raised,
                            t,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.close_use_in_picker(cx))),
                    ),
            )
            .child(tabs)
            // A fixed height, so the card does not move when the tab changes.
            .child(
                faded_scroll(
                    "use-in-list",
                    &self.ext.use_in_scroll.clone(),
                    t.component.convo.icon_list_height,
                    list,
                    c.surface_raised,
                    c.raised_clear,
                    t,
                    cx,
                )
                .debug_selector(|| "use-in-list".into()),
            )
            .child(
                // Two lines are always there, so the card does not move
                // when the note grows.
                div().flex_none().min_h(t.text.label.line * 2.0).child(text(
                    note,
                    t.text.label,
                    c.text_tertiary,
                )),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    .gap(t.space.sm)
                    .child(
                        button("use-in-cancel", "Cancel", ButtonKind::Secondary, t)
                            .debug_selector(|| "use-in-cancel".into())
                            .on_click(cx.listener(|this, _, _, cx| this.close_use_in_picker(cx))),
                    )
                    .child(save),
            );
        Some(
            div()
                .id("use-in-scrim")
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
                    cx.listener(|this, _, _, cx| this.close_use_in_picker(cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}
