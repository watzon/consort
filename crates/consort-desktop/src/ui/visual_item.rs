//! The visual item in the timeline and in a task thread (Paper G1 and G3),
//! and the page area that the item and the light box share.
//!
//! - The title line: the visual icon and the title in `text.secondary`. It
//!   is chrome, so it is not selectable.
//! - The page area: the full width of the column, with the corners of a
//!   card. It holds the image, "Loading the visual" or "The visual did not
//!   load", and on macOS the live page over them while the cover rule says
//!   `Live`.
//! - The footer, on hover: Open full screen, Copy image, Save image and the
//!   time. Its line keeps its height, so the timeline does not move.
//!
//! A right click opens the menu of Paper G2. The state and the actions are
//! in `crate::visual`.

use std::path::PathBuf;

use consort_client::VisualView;
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Div, ElementId, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement as _, Styled, Window, base::SelectableText, div, img, px,
};

use crate::{
    app::ConsortApp,
    ui::{
        convo_sidebar::Tip,
        motion,
        parts::{ButtonKind, button, detail, icon, icon_button, text},
        proactivity::dab_asset,
        timeline::open_context_menu,
    },
    visual::{
        Body, FAILED_TITLE, LOADING_TEXT, Place, VisualAction, body, page_element, page_height,
        render_width, scaled_height, visual_menu,
    },
};

/// The selector suffix of an item: its place and its message.
fn key(place: Place, message_id: &str) -> String {
    format!("{}-{message_id}", place.key())
}

impl ConsortApp {
    /// A visual in the timeline or in a task thread: the title line, the
    /// page area and the footer.
    pub fn render_visual_item(
        &mut self,
        view: &VisualView,
        place: Place,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let id = view.message_id.clone();
        let key = key(place, &id);
        let estimate = match place {
            Place::Thread => self.right_panel_width(t) - t.space.lg * 2.,
            _ => t.layout.timeline_max_width,
        };
        let page = self.render_visual_page(view, place, estimate, None, t, window, cx);
        let hovered = self
            .visuals
            .hovered
            .as_ref()
            .is_some_and(|(p, h)| *p == place && *h == id);
        let title = div()
            .flex()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .pl(t.space.xs)
            .child(icon("visual", t.icon.dense, c.text_tertiary).flex_none())
            .child(
                text(view.title.clone(), t.text.label, c.text_secondary)
                    .flex_1()
                    .min_w_0(),
            );
        let footer = self.render_visual_footer(view, place, hovered, t, cx);
        let menu = visual_menu(view, false);
        let hover_id = id.clone();
        div()
            .id(SharedString::from(format!("visual-{key}")))
            .debug_selector({
                let key = key.clone();
                move || format!("visual-{key}")
            })
            .flex()
            .flex_col()
            .w_full()
            .gap(t.space.xs + t.space.xxs)
            .pb(t.space.xs)
            .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                let here = (place, hover_id.clone());
                if *on {
                    this.visuals.hovered = Some(here);
                } else if this.visuals.hovered.as_ref() == Some(&here) {
                    this.visuals.hovered = None;
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    open_context_menu(this, event, menu.clone(), window, cx)
                }),
            )
            .child(title)
            .child(page)
            .child(footer)
            .into_any_element()
    }

    /// The footer of an item: on hover, Open full screen, Copy image, Save
    /// image and the time. Its line is there with or without the hover.
    fn render_visual_footer(
        &mut self,
        view: &VisualView,
        place: Place,
        hovered: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let chat = t.component.chat;
        let key = key(place, &view.message_id);
        let row = div()
            .debug_selector({
                let key = key.clone();
                move || format!("visual-footer-{key}")
            })
            .flex()
            .flex_none()
            .items_center()
            .h(chat.footer_line)
            .gap(t.space.xxs)
            .pl(t.space.xxs);
        if !hovered {
            return row;
        }
        let boxed = Box::new(view.clone());
        let button = |kind: &str, name: &str, tip: &'static str, action: VisualAction| {
            let id = SharedString::from(format!("visual-{kind}-{key}"));
            let element: ElementId = id.clone().into();
            let color = if motion::is_hovered(&element, cx) {
                c.text_primary
            } else {
                c.text_tertiary
            };
            icon_button(
                element,
                name,
                chat.footer_button,
                t.icon.size,
                color,
                c.surface_selected,
                t,
                cx,
            )
            .flex_none()
            .debug_selector(move || id.to_string())
            .tooltip(move |_, cx| cx.new(|_| Tip(tip.into())).into())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.run_visual_action(action.clone(), window, cx)
            }))
        };
        let open = button(
            "open",
            "maximize",
            "Open full screen",
            VisualAction::OpenFullScreen(boxed.clone()),
        );
        let copy = button(
            "copy",
            "copy",
            "Copy image",
            VisualAction::CopyImage(boxed.clone()),
        );
        let save = button(
            "save",
            "download",
            "Save image",
            VisualAction::SaveImage(boxed),
        );
        row.child(open).child(copy).child(save).child(
            div().flex_none().pl(t.space.sm).child(
                text(view.time_label.clone(), detail(t), c.text_tertiary).whitespace_nowrap(),
            ),
        )
    }

    /// The page area of a visual at its width, `estimate` until it has
    /// drawn once. `max_height` caps it in the light box: the image then
    /// scrolls in the area, and the live page scrolls in itself.
    #[allow(clippy::too_many_arguments)]
    pub fn render_visual_page(
        &mut self,
        view: &VisualView,
        place: Place,
        estimate: Pixels,
        max_height: Option<Pixels>,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = view.message_id.clone();
        let key = key(place, &id);
        self.want_visual(view, place, window, cx);
        let width = render_width(self.visuals.width(place, &id).unwrap_or(estimate));
        let shown = self.visuals.shown(&id, width);
        let full = px(page_height(width, shown.live, shown.image.as_ref(), &view.notice) as f32);
        let height = max_height.map_or(full, |max| full.min(max));
        let body = body(
            shown.image.as_ref(),
            shown.image_failure.as_ref(),
            shown.page_failure.as_ref(),
        );
        let page = self.visual_page(&id, place);
        let probe = self.visuals.frame.probe(
            place,
            id.clone(),
            self.covers_place(t, place),
            self.window_active,
            page.clone(),
        );
        let body = self.render_visual_body(body, &id, &key, width, height, t, cx);
        div()
            .debug_selector({
                let key = key.clone();
                move || format!("visual-page-{key}")
            })
            .relative()
            .flex_none()
            .w_full()
            .h(height)
            .rounded(t.radius.lg)
            .overflow_hidden()
            .child(body)
            // The probe comes before the page, so the page shows or hides
            // before it takes its place.
            .child(probe)
            .children(page_element(page))
            .into_any_element()
    }

    /// What the page area draws under the live page.
    #[allow(clippy::too_many_arguments)]
    fn render_visual_body(
        &mut self,
        body: Body,
        message_id: &str,
        key: &str,
        width: u32,
        height: Pixels,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        match body {
            Body::Image(file) => {
                let image_height = px(scaled_height(&file, width) as f32);
                let picture = img(PathBuf::from(&file.path))
                    .debug_selector({
                        let key = key.to_owned();
                        move || format!("visual-image-{key}")
                    })
                    .w_full()
                    .h(image_height);
                if image_height > height {
                    div()
                        .id(SharedString::from(format!("visual-scroll-{key}")))
                        .size_full()
                        .overflow_y_scroll()
                        .child(picture)
                        .into_any_element()
                } else {
                    div().size_full().child(picture).into_any_element()
                }
            }
            Body::Loading => div()
                .debug_selector({
                    let key = key.to_owned();
                    move || format!("visual-loading-{key}")
                })
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(t.space.sm + t.space.xxs)
                .rounded(t.radius.lg)
                .bg(c.surface_card.opacity(t.opacity.loading_fill))
                .child(loading_dabs(t, cx))
                .child(text(LOADING_TEXT, detail(t), c.text_tertiary))
                .into_any_element(),
            Body::Failed(message) => {
                let id = message_id.to_owned();
                let head = div()
                    .flex()
                    .items_start()
                    .gap(t.space.sm + t.space.xxs)
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(t.space.xl)
                            .child(icon("failed", t.icon.size, c.status_failed)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(t.space.xxs)
                            .child(
                                div()
                                    .type_style(t.text.body)
                                    .font_weight(t.text.ui_strong.weight)
                                    .text_color(c.text_primary)
                                    .child(FAILED_TITLE),
                            )
                            // The error is a detail that the user may copy.
                            .child(
                                div()
                                    .type_style(t.text.body)
                                    .text_color(c.text_secondary)
                                    .child(SelectableText::new(
                                        SharedString::from(format!("visual-error-{key}")),
                                        message,
                                    )),
                            ),
                    );
                let try_id = format!("visual-try-again-{key}");
                div()
                    .debug_selector({
                        let key = key.to_owned();
                        move || format!("visual-failed-{key}")
                    })
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(t.space.md)
                    .py(t.space.lg)
                    .px(t.space.lg + t.space.xs)
                    .rounded(t.radius.lg)
                    .bg(c.surface_card)
                    .border(t.border_width)
                    .border_color(c.status_failed.opacity(t.opacity.error_border))
                    .child(head)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .pl(t.icon.size + t.space.sm + t.space.xxs)
                            .child(
                                button(
                                    SharedString::from(try_id.clone()),
                                    "Try again",
                                    ButtonKind::Secondary,
                                    t,
                                )
                                .debug_selector(move || try_id.clone())
                                .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.visual_try_again(&id, cx)
                                    }),
                                ),
                            ),
                    )
                    .into_any_element()
            }
        }
    }
}

/// The three dabs of the mark, which shimmer on the mark clock while the
/// visual loads (Paper G3), as in the writing bubble.
fn loading_dabs(t: &Tokens, cx: &mut Context<ConsortApp>) -> Div {
    let p = t.component.proactivity;
    let version = motion::shimmer_version(true, cx);
    let dab = |name: &str, index: u64, offset: Pixels| {
        let lift = p.writing_lift * motion::writing_lift(index, cx);
        div()
            .relative()
            .top(-lift)
            .flex_none()
            .mt(offset)
            .child(img(SharedString::from(dab_asset(name, version))).size(p.dab))
    };
    div()
        .flex()
        .items_center()
        .gap(p.dab_gap)
        .child(dab("gold", 0, p.dab_drop))
        .child(dab("verdigris", 1, -p.dab_rise))
        .child(dab("lapis", 2, t.border_width))
}
