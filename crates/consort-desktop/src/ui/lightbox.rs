//! The light box: one item large over the app. It is one component for
//! pictures (Paper G4 `V6S-0`) and visuals (G5 `VEE-0`, docs/
//! VISUALS-PLAN.md).
//!
//! - The top buttons are icon buttons with tooltips: Save, Open and Close
//!   for a picture; Save image, Open in browser and Close for a visual.
//! - A click on the dark area closes the light box, as Escape and Close do.
//!   A click on the picture, the sheet of a visual, an arrow or a button
//!   does not.
//! - The arrow keys and the arrow buttons step through the other items, and
//!   "1 of 2" says where the user is. A visual opens alone, with no arrows
//!   and no count.
//! - A visual is live on a sheet of the canvas color, at the width of the
//!   sheet. A page taller than the window scrolls in itself. The sheet
//!   starts below the reach of the tooltips of the top buttons, because the
//!   live page would draw over a tooltip.
//!
//! The sizes come from `component.files`.

use std::{cell::Cell, path::PathBuf, rc::Rc, sync::Arc};

use consort_client::{AttachmentView, VisualView};
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, AppContext as _, Bounds, Context, DevicePixels, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ObjectFit, ParentElement, Pixels, Point, Size,
    StatefulInteractiveElement as _, Styled, StyledImage as _, Window, canvas, div, img,
    prelude::FluentBuilder as _, px,
};

use crate::{
    app::ConsortApp,
    ui::{
        convo_sidebar::Tip,
        parts::{detail, focusable, icon, icon_button, line, text},
        timeline::open_context_menu,
        visual_cover::TimelineArea,
    },
    visual::{Place, visual_menu},
};

/// GPUI draws a tooltip 1 px right of and below the pointer
/// (`Window::prepaint_tooltip` in gpui-pre 0.3.10).
const TOOLTIP_OFFSET: Pixels = px(1.);

/// What the light box shows.
#[derive(Clone, Debug, PartialEq)]
pub enum LightboxItem {
    /// A picture: an attachment or a file of the channel.
    Picture(AttachmentView),
    /// A visual, live on a sheet.
    Visual(VisualView),
}

impl LightboxItem {
    /// The first line of the top bar.
    pub fn name(&self) -> &str {
        match self {
            Self::Picture(picture) => &picture.name,
            Self::Visual(visual) => &visual.title,
        }
    }

    /// The details before the caption: "JPEG, 3.1 MB, 4032 by 3024".
    pub fn detail(&self) -> &str {
        match self {
            Self::Picture(picture) => &picture.detail,
            Self::Visual(_) => "",
        }
    }

    /// The top buttons, in their order.
    pub fn buttons(&self) -> &'static [LightboxButton; 3] {
        match self {
            Self::Picture(_) => &PICTURE_BUTTONS,
            Self::Visual(_) => &VISUAL_BUTTONS,
        }
    }
}

/// What a top button of the light box does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightboxAction {
    /// The system save dialog, then a copy of the file there. For a visual,
    /// the image.
    Save,
    /// The file in the app of the system. For a visual, the page with the
    /// theme in the system browser.
    Open,
    Close,
}

/// One icon button of the top bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightboxButton {
    /// The element ID and the debug selector.
    pub id: &'static str,
    pub icon: &'static str,
    /// The tooltip: the name of the button.
    pub tip: &'static str,
    pub action: LightboxAction,
}

const PICTURE_BUTTONS: [LightboxButton; 3] = [
    LightboxButton {
        id: "lightbox-save",
        icon: "download",
        tip: "Save",
        action: LightboxAction::Save,
    },
    LightboxButton {
        id: "lightbox-open",
        icon: "external-link",
        tip: "Open",
        action: LightboxAction::Open,
    },
    LightboxButton {
        id: "lightbox-close",
        icon: "x",
        tip: "Close",
        action: LightboxAction::Close,
    },
];

const VISUAL_BUTTONS: [LightboxButton; 3] = [
    LightboxButton {
        id: "lightbox-save",
        icon: "download",
        tip: "Save image",
        action: LightboxAction::Save,
    },
    LightboxButton {
        id: "lightbox-open",
        icon: "external-link",
        tip: "Open in browser",
        action: LightboxAction::Open,
    },
    LightboxButton {
        id: "lightbox-close",
        icon: "x",
        tip: "Close",
        action: LightboxAction::Close,
    },
];

/// The space above the sheet of a visual, under the top bar. A tooltip of a
/// top button starts just under the pointer, so it can reach from the
/// bottom edge of the button down by its own height. The sheet starts below
/// that, because a live page draws over anything in its bounds.
pub fn sheet_top(t: &Tokens) -> Pixels {
    let bar = t.component.files.lightbox_bar;
    let button_bottom = (bar + t.component.chat.top_action) / 2.;
    // The height of `Tip`: its padding, a label line and its border.
    let tip = t.space.xs * 2. + t.text.label.line + t.border_width * 2.;
    (button_bottom + TOOLTIP_OFFSET + tip + t.space.xs - bar).max(t.space.sm)
}

/// The details line under the name: the details of the item and the
/// caption, the empty ones left out.
pub fn details_line(detail: &str, caption: &str) -> String {
    [detail, caption]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(". ")
}

/// The open light box: its items, the one it shows, and the caption.
#[derive(Clone, Debug)]
pub struct Lightbox {
    pub items: Vec<LightboxItem>,
    pub index: usize,
    /// Who sent it and when: "You sent it today at 7:40 PM."
    pub caption: String,
    /// Where the picture drew in the last frame. A click there does not
    /// close the light box.
    held: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Lightbox {
    /// A light box on the item at `index`, or on the last item when `index`
    /// is past the end. `None` when there are no items.
    pub fn new(items: Vec<LightboxItem>, index: usize, caption: String) -> Option<Self> {
        let last = items.len().checked_sub(1)?;
        Some(Self {
            index: index.min(last),
            items,
            caption,
            held: Rc::default(),
        })
    }

    /// The item that the light box shows.
    pub fn current(&self) -> Option<&LightboxItem> {
        self.items.get(self.index)
    }

    /// Steps `delta` items. A step past either end does nothing.
    pub fn step(&mut self, delta: isize) {
        let next = self.index as isize + delta;
        if next >= 0 && (next as usize) < self.items.len() {
            self.index = next as usize;
        }
    }

    /// Whether a click at `at` is on the picture, so the light box stays
    /// open.
    pub fn holds(&self, at: Point<Pixels>) -> bool {
        self.held.get().is_some_and(|bounds| bounds.contains(&at))
    }
}

/// Where a picture of the size `natural` draws in `frame`. The picture fits
/// in the frame as `ObjectFit::Contain` draws it, so a picture of another
/// shape leaves dark bands that close the light box on a click. With no
/// size yet, the picture takes the whole frame.
pub fn picture_bounds(
    frame: Bounds<Pixels>,
    natural: Option<Size<DevicePixels>>,
) -> Bounds<Pixels> {
    match natural {
        Some(size) if size.width.0 > 0 && size.height.0 > 0 && !frame.is_empty() => {
            ObjectFit::Contain.get_bounds(frame, size)
        }
        _ => frame,
    }
}

impl ConsortApp {
    /// Opens pictures in the light box, on the picture at `index`.
    pub fn open_lightbox(
        &mut self,
        images: Vec<AttachmentView>,
        index: usize,
        caption: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = images.into_iter().map(LightboxItem::Picture).collect();
        self.open_lightbox_items(items, index, caption, window, cx);
    }

    /// Opens items in the light box, on the item at `index`. It closes an
    /// open menu, and it takes the keyboard from a text box under it, so
    /// Escape and the arrow keys reach it.
    pub fn open_lightbox_items(
        &mut self,
        items: Vec<LightboxItem>,
        index: usize,
        caption: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(lightbox) = Lightbox::new(items, index, caption) else {
            return;
        };
        self.menu = None;
        self.lightbox = Some(lightbox);
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub fn step_lightbox(&mut self, delta: isize) {
        if let Some(lightbox) = self.lightbox.as_mut() {
            lightbox.step(delta);
        }
    }

    pub fn close_lightbox(&mut self, cx: &mut Context<Self>) {
        if self.lightbox.take().is_some() {
            cx.notify();
        }
    }

    /// What a top button does to the item that the light box shows.
    pub fn run_lightbox_action(&mut self, action: LightboxAction, cx: &mut Context<Self>) {
        let Some(item) = self.lightbox.as_ref().and_then(|l| l.current()).cloned() else {
            return;
        };
        match (action, item) {
            (LightboxAction::Close, _) => self.close_lightbox(cx),
            (LightboxAction::Save, LightboxItem::Picture(p)) => {
                self.save_blob(p.sha256, p.size, p.name, cx)
            }
            (LightboxAction::Open, LightboxItem::Picture(p)) => {
                self.open_blob(p.sha256, p.size, p.name, cx)
            }
            (LightboxAction::Save, LightboxItem::Visual(v)) => self.save_visual_image(&v, cx),
            (LightboxAction::Open, LightboxItem::Visual(v)) => self.open_visual_in_browser(&v, cx),
        }
    }

    /// The light box: the item large, its name and details, the top buttons,
    /// Previous and Next, and "1 of 2".
    pub fn render_lightbox(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let f = t.component.files;
        let Some(lightbox) = self.lightbox.clone() else {
            return div().into_any_element();
        };
        let Some(item) = lightbox.current().cloned() else {
            return div().into_any_element();
        };
        let count = lightbox.items.len();
        let first = lightbox.index == 0;
        let last = lightbox.index + 1 >= count;
        let details = details_line(item.detail(), &lightbox.caption);
        // A visual opens alone: no arrows and no count.
        let alone = count == 1 && matches!(item, LightboxItem::Visual(_));
        let mut top = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(t.space.xs)
            .h(f.lightbox_bar)
            // On macOS the window buttons sit at the top left.
            .pl(if cfg!(target_os = "macos") {
                t.space.page * 2.0
            } else {
                t.space.xl
            })
            .pr(t.space.lg)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(line(
                        item.name().to_owned(),
                        t.text.ui_strong,
                        c.text_primary,
                    ))
                    .child(
                        div()
                            .debug_selector(|| "lightbox-details".into())
                            .child(line(details, detail(t), c.text_tertiary)),
                    ),
            );
        for button in item.buttons() {
            let (id, action, tip) = (button.id, button.action, button.tip);
            top = top.child(
                icon_button(
                    id,
                    button.icon,
                    t.component.chat.top_action,
                    t.component.chat.top_action_icon,
                    c.text_primary,
                    c.hover_raised,
                    t,
                    cx,
                )
                .debug_selector(move || id.to_owned())
                // A click on a button does not reach the dark area.
                .occlude()
                .tooltip(move |_, cx| cx.new(|_| Tip(tip.into())).into())
                .on_click(cx.listener(move |this, _, _, cx| this.run_lightbox_action(action, cx))),
            );
        }
        let nav = |id: &'static str, icon_name: &'static str, disabled: bool| {
            focusable(div().id(id), t)
                .debug_selector(move || id.to_owned())
                .occlude()
                .size(f.lightbox_nav)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(t.radius.full)
                .bg(c.surface_raised)
                .when(disabled, |d| d.opacity(t.opacity.nav_disabled))
                .when(!disabled, |d| {
                    d.cursor_pointer().hover(|s| s.bg(c.hover_raised))
                })
                .child(icon(icon_name, t.icon.size, c.text_primary))
        };
        let stage = match &item {
            LightboxItem::Picture(image) => {
                let (picture, natural) = self.render_picture(image, t, window, cx);
                let held = lightbox.held.clone();
                // Where the picture draws, for the click on the dark area.
                // The canvas has the bounds of the frame.
                let measure = canvas(
                    move |frame, _, _| held.set(Some(picture_bounds(frame, natural))),
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full();
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .items_center()
                    .justify_center()
                    .gap(t.space.xl)
                    .py(t.space.sm)
                    .px(t.space.xl)
                    .child(
                        nav("lightbox-previous", "chevron-left", first).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.step_lightbox(-1);
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        div()
                            .id("lightbox-image")
                            .debug_selector(|| "lightbox-image".into())
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .max_w(f.lightbox_image_w)
                            .max_h(f.lightbox_image_h)
                            .rounded(t.radius.lg)
                            .overflow_hidden()
                            .child(picture)
                            .child(measure),
                    )
                    .child(
                        nav("lightbox-next", "chevron-right", last).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.step_lightbox(1);
                                cx.notify();
                            },
                        )),
                    )
                    .into_any_element()
            }
            LightboxItem::Visual(view) => self.render_visual_stage(view, &lightbox, t, window, cx),
        };
        div()
            .id("lightbox")
            .debug_selector(|| "lightbox".into())
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .flex()
            .flex_col()
            .bg(c.lightbox)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if this
                        .lightbox
                        .as_ref()
                        .is_some_and(|l| !l.holds(event.position))
                    {
                        this.close_lightbox(cx);
                    }
                }),
            )
            .child(top)
            .child(stage)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .h(f.lightbox_bar)
                    .when(!alone, |d| {
                        d.child(div().debug_selector(|| "lightbox-count".into()).child(text(
                            format!("{} of {count}", lightbox.index + 1),
                            detail(t),
                            c.text_secondary,
                        )))
                    }),
            )
            .into_any_element()
    }

    /// The stage of a visual (Paper G5): the sheet in the canvas color with
    /// the page area, at the width of the sheet. The page area is no taller
    /// than the stage, so a live page never draws over the bars.
    fn render_visual_stage(
        &mut self,
        view: &VisualView,
        lightbox: &Lightbox,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let f = t.component.files;
        let top = sheet_top(t);
        let bottom = t.space.sm;
        let side = t.space.xl;
        // The stage of the last frame, else the window less the two bars.
        let stage = self
            .visuals
            .frame
            .area(Place::Lightbox)
            .map(|area| area.viewport.size)
            .unwrap_or_else(|| {
                let window = window.viewport_size();
                gpui_kit::size(window.width, window.height - f.lightbox_bar * 2.)
            });
        let sheet_w = f.lightbox_sheet_w.min(stage.width - side * 2.);
        let inset = t.space.lg * 2. + t.border_width * 2.;
        let max_h = stage.height - top - bottom - inset;
        let area = self.render_visual_page(
            view,
            Place::Lightbox,
            sheet_w - inset,
            Some(max_h),
            t,
            window,
            cx,
        );
        let held = lightbox.held.clone();
        // The sheet holds a click, for the click on the dark area.
        let measure = canvas(move |sheet, _, _| held.set(Some(sheet)), |_, _, _, _| {})
            .absolute()
            .top_0()
            .left_0()
            .size_full();
        let menu = visual_menu(view, true);
        div()
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .items_center()
            .justify_center()
            .pt(top)
            .pb(bottom)
            .px(side)
            // Before the sheet, so the probe of the page reads this frame.
            .child(self.visuals.frame.area_probe(Place::Lightbox, |stage| {
                TimelineArea::new(stage, Pixels::ZERO, None)
            }))
            .child(
                div()
                    .id("lightbox-sheet")
                    .debug_selector(|| "lightbox-sheet".into())
                    .relative()
                    .flex_none()
                    .w(sheet_w)
                    .p(t.space.lg)
                    .rounded(t.radius.xl)
                    .bg(c.surface_canvas)
                    .border(t.border_width)
                    .border_color(c.border_subtle)
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            open_context_menu(this, event, menu.clone(), window, cx)
                        }),
                    )
                    .child(area)
                    .child(measure),
            )
            .into_any_element()
    }

    /// A picture of the light box, and its size once it has loaded.
    fn render_picture(
        &mut self,
        image: &AttachmentView,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, Option<Size<DevicePixels>>) {
        let Some(path) = image.local_path.clone() else {
            self.want_blob(&image.sha256, image.size, cx);
            let loading = div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(text(
                    "Loading the picture",
                    t.text.ui,
                    t.colors.text_secondary,
                ))
                .into_any_element();
            return (loading, None);
        };
        let path = PathBuf::from(path);
        // The same resource that `img` loads, so the file loads once.
        let resource = gpui_kit::Resource::Path(Arc::from(path.as_path()));
        let natural = match window.use_asset::<gpui_kit::ImgResourceLoader>(&resource, cx) {
            Some(Ok(data)) => Some(data.size(0)),
            _ => None,
        };
        let picture = img(path)
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element();
        (picture, natural)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, px, size};

    fn picture(name: &str) -> LightboxItem {
        LightboxItem::Picture(AttachmentView {
            name: name.into(),
            detail: "PNG, 2 KB".into(),
            ..AttachmentView::default()
        })
    }

    #[test]
    fn a_picture_has_save_open_and_close_with_their_names() {
        let tips: Vec<_> = picture("a.png")
            .buttons()
            .iter()
            .map(|b| (b.tip, b.icon, b.action))
            .collect();
        assert_eq!(
            tips,
            [
                ("Save", "download", LightboxAction::Save),
                ("Open", "external-link", LightboxAction::Open),
                ("Close", "x", LightboxAction::Close),
            ]
        );
    }

    #[test]
    fn the_details_line_leaves_out_the_empty_parts() {
        assert_eq!(
            details_line("PNG, 2 KB", "You sent it today at 7:40 PM."),
            "PNG, 2 KB. You sent it today at 7:40 PM."
        );
        assert_eq!(details_line("PNG, 2 KB", ""), "PNG, 2 KB");
        // A visual has no details of its own.
        assert_eq!(
            details_line("", "Visual from mac-ext4 at 9:44 AM."),
            "Visual from mac-ext4 at 9:44 AM."
        );
    }

    #[test]
    fn a_light_box_needs_an_item_and_stays_in_range() {
        assert!(Lightbox::new(Vec::new(), 0, String::new()).is_none());
        let mut lb =
            Lightbox::new(vec![picture("a"), picture("b")], 5, String::new()).expect("two items");
        assert_eq!(lb.index, 1, "past the end opens the last");
        lb.step(1);
        assert_eq!(lb.index, 1, "no step past the last");
        lb.step(-1);
        lb.step(-1);
        assert_eq!(lb.index, 0, "no step before the first");
        assert_eq!(lb.current().map(LightboxItem::name), Some("a"));
    }

    #[test]
    fn a_click_holds_only_on_the_picture() {
        let lb = Lightbox::new(vec![picture("a")], 0, String::new()).expect("one item");
        // Nothing drew yet: every click closes.
        assert!(!lb.holds(point(px(10.), px(10.))));
        lb.held.set(Some(Bounds::new(
            point(px(100.), px(100.)),
            size(px(200.), px(100.)),
        )));
        assert!(lb.holds(point(px(150.), px(150.))));
        assert!(!lb.holds(point(px(50.), px(150.))));
        assert!(!lb.holds(point(px(150.), px(250.))));
    }

    #[test]
    fn a_tall_picture_leaves_dark_bands_at_its_sides() {
        let frame = Bounds::new(point(px(0.), px(0.)), size(px(920.), px(690.)));
        let tall = size(DevicePixels(300), DevicePixels(600));
        let bounds = picture_bounds(frame, Some(tall));
        assert_eq!(bounds.size, size(px(345.), px(690.)));
        assert_eq!(bounds.origin, point(px(287.5), px(0.)));
        // The same shape as the frame fills it.
        let same = size(DevicePixels(4032), DevicePixels(3024));
        assert_eq!(picture_bounds(frame, Some(same)), frame);
        // With no size yet, or a size of zero, the picture is the frame.
        assert_eq!(picture_bounds(frame, None), frame);
        let empty = size(DevicePixels(0), DevicePixels(10));
        assert_eq!(picture_bounds(frame, Some(empty)), frame);
    }
}
