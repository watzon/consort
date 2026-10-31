//! The frame of the main page: the sidebar that hides and slides, Search and
//! Hide sidebar on the line of the traffic lights, the edge that resizes the
//! right panel, and the drawer and the full panel of the compact layout. The
//! rules for the widths are in [`crate::layout`].

use consort_client::store::LayoutSettings;
use consort_theme::Tokens;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, Context, CursorStyle, Empty,
    InteractiveElement, IntoElement, ParentElement, Pixels, StatefulInteractiveElement as _,
    Styled, div, ease_out_quint, px,
};

use crate::{
    app::{ConsortApp, Panel},
    layout::{self, Frame, Sizes},
    ui::{
        convo_sidebar::Tip,
        parts::{icon_button, text},
        window_controls::DragArea as _,
    },
};

/// The state of the frame that the render follows.
pub struct LayoutUi {
    pub settings: LayoutSettings,
    pub window_width: Pixels,
    /// The sidebar is open over the chat, with a scrim.
    pub drawer_open: bool,
    /// Whether the last draw showed the sidebar. `None` before the first
    /// draw, which does not slide.
    shown: Option<bool>,
    /// Counts the slides, so each one has its own animation.
    slide: u64,
    /// A slide runs: the sidebar stays in the tree while it closes.
    pub sliding: bool,
    /// The pointer drags the edge of the panel.
    pub resizing: bool,
    /// Compact Settings shows its list of sections, not a section.
    pub settings_list: bool,
}

impl LayoutUi {
    pub fn new(settings: LayoutSettings, window_width: Pixels) -> Self {
        Self {
            settings,
            window_width,
            drawer_open: false,
            shown: None,
            slide: 0,
            sliding: false,
            resizing: false,
            settings_list: false,
        }
    }
}

/// What a drag of the panel edge carries.
#[derive(Clone, Copy)]
pub struct PanelResize;

/// The drag of the panel edge shows nothing under the pointer.
struct NoDragView;

impl gpui_kit::Render for NoDragView {
    fn render(&mut self, _: &mut gpui_kit::Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// A point between `from` and `to`, for a slide at `delta`.
fn between(from: Pixels, to: Pixels, delta: f32) -> Pixels {
    from + (to - from) * delta
}

/// The shortcut in a tooltip.
fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

impl ConsortApp {
    fn layout_sizes(t: &Tokens) -> Sizes {
        Sizes {
            sidebar: t.layout.sidebar_width.into(),
            panel_min: t.layout.panel_min_width.into(),
            chat_min: t.layout.chat_min_width.into(),
            compact: t.layout.compact_width.into(),
        }
    }

    /// The width that the open panel asks for. A task item starts wider.
    fn panel_wanted(&self, t: &Tokens) -> Option<f32> {
        let user = self
            .layout
            .settings
            .panel_width
            .unwrap_or(t.layout.panel_width.into());
        match &self.panel {
            Panel::None => None,
            Panel::Thread(task) if self.item_open_for(task) => {
                Some(user.max(t.component.convo.panel_wide.into()))
            }
            _ => Some(user),
        }
    }

    /// How the window shares its width now.
    pub fn layout_frame(&self, t: &Tokens) -> Frame {
        layout::frame(
            self.layout.window_width.into(),
            Self::layout_sizes(t),
            self.layout.settings.sidebar_hidden,
            self.panel_wanted(t),
        )
    }

    /// Whether the corner notifications use the compact position. See
    /// [`layout::notices_compact`] and DECISIONS 735.
    pub fn notices_compact(&self, t: &Tokens) -> bool {
        layout::notices_compact(
            self.layout.window_width.into(),
            &self.layout_frame(t),
            Self::layout_sizes(t),
            (t.layout.notice_width + t.space.lg).into(),
        )
    }

    /// The width of the open panel.
    pub fn right_panel_width(&self, t: &Tokens) -> Pixels {
        px(self.layout_frame(t).panel)
    }

    fn save_layout(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.client.save_layout(&self.layout.settings) {
            self.show_error(
                format!("The app could not save the window layout. {err}"),
                cx,
            );
        }
    }

    /// Hide sidebar or Show sidebar. When the window has no room for the
    /// sidebar, it opens as a drawer, so the button always does something.
    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        let t = Self::tokens(cx);
        if self.layout.drawer_open {
            self.layout.drawer_open = false;
        } else if self.layout_frame(&t).sidebar {
            self.layout.settings.sidebar_hidden = true;
            self.save_layout(cx);
        } else if self.layout_frame(&t).compact {
            self.layout.drawer_open = true;
        } else {
            if self.layout.settings.sidebar_hidden {
                self.layout.settings.sidebar_hidden = false;
                self.save_layout(cx);
            }
            self.layout.drawer_open = !self.layout_frame(&t).sidebar;
        }
        cx.notify();
    }

    /// Follows the frame of this draw: a change of the sidebar starts a
    /// slide, and a window with room for the sidebar closes the drawer.
    pub fn follow_frame(&mut self, frame: Frame, t: &Tokens, cx: &mut Context<Self>) {
        if frame.sidebar {
            self.layout.drawer_open = false;
        }
        if self.layout.resizing && !cx.has_active_drag() {
            self.layout.resizing = false;
            self.save_layout(cx);
        }
        let last = self.layout.shown.replace(frame.sidebar);
        if last.is_none_or(|last| last == frame.sidebar) || cx.reduce_motion() {
            return;
        }
        self.layout.slide += 1;
        self.layout.sliding = true;
        let slide = self.layout.slide;
        let duration = t.motion.normal;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            this.update(cx, |this, cx| {
                if this.layout.slide == slide {
                    this.layout.sliding = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The slot of the sidebar: its width slides, and the sidebar in it
    /// keeps its own width, so its text does not wrap on the way.
    pub fn render_sidebar_slot(
        &self,
        sidebar: Option<AnyElement>,
        frame: Frame,
        t: &Tokens,
    ) -> AnyElement {
        let width = t.layout.sidebar_width;
        let (from, to) = if frame.sidebar {
            (px(0.), width)
        } else {
            (width, px(0.))
        };
        let slot = div()
            .flex_none()
            .h_full()
            .overflow_hidden()
            .children(sidebar);
        if self.layout.sliding {
            slot.with_animation(
                ("sidebar-slide", self.layout.slide),
                Animation::new(t.motion.normal).with_easing(ease_out_quint()),
                move |slot, delta| slot.w(between(from, to, delta)),
            )
            .into_any_element()
        } else {
            slot.w(to).into_any_element()
        }
    }

    /// Search and Hide sidebar, on the center line of the traffic lights.
    /// They slide as a group, in step with the edge of the sidebar: to the
    /// right end of its title bar when it is open, next to the traffic
    /// lights when it is closed. `None` in the compact layout, which has the
    /// top bar instead, unless the drawer is open.
    pub fn render_sidebar_buttons(
        &self,
        frame: Frame,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let c = &t.colors;
        let sb = t.component.sidebar;
        let open = frame.sidebar || self.layout.drawer_open;
        if frame.compact && !open {
            return None;
        }
        let size = t.component.composer_chip;
        let gap = t.space.xxs;
        let open_left = t.layout.sidebar_width - sb.inset - size * 2.0 - gap;
        let closed_left = closed_left(t);
        let (from, to) = if open {
            (closed_left, open_left)
        } else {
            (open_left, closed_left)
        };
        let button = |id: &'static str, name: &str, tip: String, cx: &mut Context<Self>| {
            icon_button(
                id,
                name,
                size,
                t.icon.size,
                c.text_secondary,
                c.hover_sidebar,
                t,
                cx,
            )
            .tooltip(move |_, cx| cx.new(|_| Tip(tip.clone().into())).into())
        };
        let search = button(
            "sidebar-search",
            "search",
            format!("Search ({})", shortcut("K")),
            cx,
        )
        .debug_selector(|| "sidebar-search".into())
        .on_click(cx.listener(|this, _, window, cx| this.open_switcher(window, cx)));
        let toggle = button(
            "sidebar-toggle",
            "sidebar",
            format!(
                "{} sidebar ({})",
                if open { "Hide" } else { "Show" },
                shortcut("\\")
            ),
            cx,
        )
        .debug_selector(|| "sidebar-toggle".into())
        .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)));
        let group = div()
            .id("sidebar-buttons")
            .debug_selector(|| "sidebar-buttons".into())
            .absolute()
            .top(top_on_light_line(size, t))
            .flex()
            .items_center()
            .gap(gap)
            .occlude()
            .child(toggle)
            .child(search);
        Some(if self.layout.sliding {
            group
                .with_animation(
                    ("sidebar-buttons-slide", self.layout.slide),
                    Animation::new(t.motion.normal).with_easing(ease_out_quint()),
                    move |group, delta| group.left(between(from, to, delta)),
                )
                .into_any_element()
        } else {
            group.left(to).into_any_element()
        })
    }

    /// The edge of the right panel: a drag resizes it, and a double click
    /// resets the width.
    pub fn render_resize_handle(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let width = t.layout.resize_handle;
        let strong = t.colors.border_strong;
        div()
            .id("panel-resize")
            .debug_selector(|| "panel-resize".into())
            .group("panel-resize")
            .absolute()
            .top_0()
            .bottom_0()
            .left(-(width / 2.0))
            .w(width)
            .occlude()
            .cursor_col_resize()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(width / 2.0)
                    .w(t.border_width)
                    .group_hover("panel-resize", move |s| s.bg(strong)),
            )
            .on_drag(PanelResize, |_, _, window, cx| {
                cx.set_active_drag_cursor_style(CursorStyle::ResizeLeftRight, window);
                cx.new(|_| NoDragView)
            })
            .on_click(cx.listener(|this, event: &gpui_kit::ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.layout.settings.panel_width = None;
                    this.save_layout(cx);
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    /// A drag of the panel edge: the pointer sets the width, inside the
    /// limits of the frame.
    pub fn drag_panel_edge(&mut self, pointer_x: Pixels, t: &Tokens, cx: &mut Context<Self>) {
        let width: f32 = self.layout.window_width.into();
        let wanted = layout::dragged_panel_width(width, pointer_x.into(), Self::layout_sizes(t));
        let frame = layout::frame(
            width,
            Self::layout_sizes(t),
            self.layout.settings.sidebar_hidden,
            Some(wanted),
        );
        self.layout.resizing = true;
        self.layout.settings.panel_width = Some(frame.panel);
        cx.notify();
    }

    /// The sidebar over the chat, with a scrim. A click on the scrim closes
    /// it.
    pub fn render_drawer(
        &self,
        sidebar: AnyElement,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("sidebar-drawer")
            .debug_selector(|| "sidebar-drawer".into())
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(t.colors.scrim)
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.layout.drawer_open = false;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .h_full()
                    .w(t.layout.sidebar_width)
                    .occlude()
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation()
                    })
                    .child(sidebar),
            )
            .into_any_element()
    }

    /// A panel over the whole window, with Back at the top left.
    pub fn render_panel_over(
        &self,
        panel: AnyElement,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        div()
            .id("panel-over")
            .debug_selector(|| "panel-over".into())
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .flex()
            .flex_col()
            .bg(c.surface_sidebar)
            .child(
                div()
                    .relative()
                    .flex_none()
                    .h(t.component.sidebar.title_bar)
                    .child(div().absolute().top_0().left_0().size_full().drag_zone())
                    .child(
                        div()
                            .id("panel-back")
                            .debug_selector(|| "panel-back".into())
                            .absolute()
                            .top(top_on_light_line(t.component.composer_chip, t))
                            .left(closed_left(t))
                            .occlude()
                            .flex()
                            .items_center()
                            .gap(t.space.xs)
                            .h(t.component.composer_chip)
                            .pl(t.space.xs)
                            .pr(t.space.sm)
                            .rounded(t.radius.md)
                            .cursor_pointer()
                            .hover(|s| s.bg(c.hover_sidebar))
                            .child(crate::ui::parts::icon(
                                "chevron-left",
                                t.icon.size,
                                c.text_secondary,
                            ))
                            .child(text("Back", t.text.ui, c.text_primary))
                            .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx))),
                    ),
            )
            .child(div().flex_1().min_w_0().min_h_0().w_full().child(panel))
            .into_any_element()
    }
}

/// Next to the traffic lights on macOS, at the left edge elsewhere.
pub fn closed_left(t: &Tokens) -> Pixels {
    if cfg!(target_os = "macos") {
        t.component.sidebar.lights_end
    } else {
        t.space.sm
    }
}

/// The top of a control of this size whose center is on the line of the
/// traffic lights.
pub fn top_on_light_line(size: Pixels, t: &Tokens) -> Pixels {
    (t.component.sidebar.lights_center - size / 2.0).max(px(0.))
}
