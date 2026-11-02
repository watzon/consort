//! The cover rule of a visual (docs/VISUALS-PLAN.md, "Live or image"). In
//! each frame it decides whether a visual in the timeline shows its live
//! page, its image or nothing. It is pure, so the tests need no webview.
//!
//! The rule exists because the native webview of a live page draws above
//! all GPUI content in its bounds and does not clip (docs/research/
//! visualizations.md). So a page can be live only where nothing must draw
//! over it: all of it is between the top tint and the bottom fade, no
//! overlay is open, and the window is active.

use consort_theme::Tokens;
use gpui_kit::{Bounds, Pixels, point, px};

/// What a visual in the timeline draws in this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// The live page. All of the item is in the clear part of the timeline,
    /// nothing is over it, and the window is active.
    Live,
    /// The image of the page. The item is on screen, but it cannot be live.
    Image,
    /// Nothing. The item is off screen.
    Hidden,
}

/// The parts of the timeline that the cover rule reads, in window
/// coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineArea {
    /// The bounds of the list.
    pub viewport: Bounds<Pixels>,
    /// The part of the list that the top tint and the bottom fade do not
    /// cover. Its height is zero when the list is shorter than the two.
    pub clear: Bounds<Pixels>,
}

impl TimelineArea {
    /// The area of a list whose top `top_tint` is under the top tint, and
    /// whose bottom `fade` is under the bottom fade. `fade` is `None` while
    /// the fade is not drawn.
    pub fn new(viewport: Bounds<Pixels>, top_tint: Pixels, fade: Option<Pixels>) -> Self {
        let top = (viewport.top() + top_tint).min(viewport.bottom());
        let bottom = (viewport.bottom() - fade.unwrap_or(Pixels::ZERO)).max(top);
        let clear =
            Bounds::from_corners(point(viewport.left(), top), point(viewport.right(), bottom));
        Self { viewport, clear }
    }

    /// The area of the timeline: the full height of `layout.top_tint` at
    /// the top, and `layout.fade_bottom` at the bottom. The timeline draws
    /// the bottom fade only while it is not at its end.
    pub fn of_timeline(viewport: Bounds<Pixels>, t: &Tokens, at_end: bool) -> Self {
        let fade = (!at_end).then_some(t.layout.fade_bottom);
        Self::new(viewport, t.layout.top_tint_height(), fade)
    }

    /// Whether a page of this height fits in the clear part.
    pub fn fits(&self, page_height: Pixels) -> bool {
        page_height <= self.clear.size.height
    }
}

/// Where a visual whose item has the bounds `item` shows in this frame.
///
/// - `Hidden` when the item does not touch the viewport.
/// - `Image` when it touches the viewport but is not all in the clear part,
///   when an overlay `covered` the timeline, when the window is not active,
///   or when the page does not fit in the clear part.
/// - `Live` in all other cases.
pub fn visual_presence(
    item: Bounds<Pixels>,
    area: TimelineArea,
    covered: bool,
    window_active: bool,
    page_fits: bool,
) -> Presence {
    if !overlaps(item, area.viewport) {
        return Presence::Hidden;
    }
    if covered || !window_active || !page_fits || !inside(item, area.clear) {
        return Presence::Image;
    }
    Presence::Live
}

/// Whether two bounds share an area larger than zero. Bounds that only
/// touch at an edge do not overlap, and bounds with no area overlap nothing.
fn overlaps(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    !a.is_empty()
        && !b.is_empty()
        && a.left() < b.right()
        && b.left() < a.right()
        && a.top() < b.bottom()
        && b.top() < a.bottom()
}

/// Whether all of `item` is in `area`. An item with no area is never in it.
fn inside(item: Bounds<Pixels>, area: Bounds<Pixels>) -> bool {
    item.size.width > px(0.)
        && item.size.height > px(0.)
        && item.left() >= area.left()
        && item.right() <= area.right()
        && item.top() >= area.top()
        && item.bottom() <= area.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::size;

    /// A timeline of 800 by 900 px at the window top, with the tint and the
    /// fade of the tokens: the clear part is from 108 to 868 px.
    fn area(at_end: bool) -> TimelineArea {
        let theme = consort_theme::builtin_themes()
            .into_iter()
            .next()
            .expect("a built-in theme");
        let t = Tokens::new(
            &theme,
            consort_theme::Mode::Dark,
            consort_theme::TextSize::Default,
        );
        TimelineArea::of_timeline(bounds(0., 900.), &t, at_end)
    }

    /// An item of the timeline column from `top`, `height` px high.
    fn bounds(top: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(top)), size(px(800.), px(height)))
    }

    fn rule(item: Bounds<Pixels>) -> Presence {
        visual_presence(item, area(false), false, true, true)
    }

    #[test]
    fn the_clear_part_is_below_the_top_tint_and_above_the_bottom_fade() {
        let scrolled = area(false);
        assert_eq!(scrolled.viewport, bounds(0., 900.));
        assert_eq!(scrolled.clear.top(), px(108.));
        assert_eq!(scrolled.clear.bottom(), px(900. - 32.));
        // At the end of the timeline there is no bottom fade.
        assert_eq!(area(true).clear.bottom(), px(900.));
        assert_eq!(area(true).clear.top(), px(108.));
    }

    #[test]
    fn a_list_shorter_than_the_tint_and_the_fade_has_no_clear_part() {
        let short = TimelineArea::new(bounds(50., 100.), px(108.), Some(px(32.)));
        assert_eq!(short.clear.size.height, px(0.));
        assert_eq!(short.clear.top(), px(150.));
        assert_eq!(
            visual_presence(bounds(60., 40.), short, false, true, true),
            Presence::Image
        );
    }

    #[test]
    fn an_item_off_screen_is_not_drawn() {
        assert_eq!(rule(bounds(-300., 200.)), Presence::Hidden, "above");
        assert_eq!(rule(bounds(950., 200.)), Presence::Hidden, "below");
        // Touching an edge is not on screen.
        assert_eq!(
            rule(bounds(-200., 200.)),
            Presence::Hidden,
            "at the top edge"
        );
        assert_eq!(
            rule(bounds(900., 200.)),
            Presence::Hidden,
            "at the bottom edge"
        );
        // An item with no height is not on screen either.
        assert_eq!(rule(bounds(400., 0.)), Presence::Hidden, "no height");
    }

    #[test]
    fn an_item_all_in_the_clear_part_is_live() {
        assert_eq!(rule(bounds(200., 300.)), Presence::Live);
        // The edges of the clear part count as inside.
        assert_eq!(rule(bounds(108., 760.)), Presence::Live);
    }

    #[test]
    fn an_item_under_the_top_tint_shows_its_image() {
        assert_eq!(rule(bounds(100., 300.)), Presence::Image);
        assert_eq!(
            rule(bounds(-100., 300.)),
            Presence::Image,
            "partly off screen"
        );
    }

    #[test]
    fn an_item_under_the_bottom_fade_shows_its_image() {
        assert_eq!(rule(bounds(600., 280.)), Presence::Image);
        assert_eq!(
            rule(bounds(700., 400.)),
            Presence::Image,
            "partly off screen"
        );
        // At the end there is no fade, so the same item is live.
        assert_eq!(
            visual_presence(bounds(600., 280.), area(true), false, true, true),
            Presence::Live
        );
    }

    #[test]
    fn an_overlay_an_inactive_window_or_a_tall_page_show_the_image() {
        let item = bounds(200., 300.);
        assert_eq!(
            visual_presence(item, area(false), true, true, true),
            Presence::Image,
            "covered"
        );
        assert_eq!(
            visual_presence(item, area(false), false, false, true),
            Presence::Image,
            "window not active"
        );
        assert_eq!(
            visual_presence(item, area(false), false, true, false),
            Presence::Image,
            "page taller than the clear part"
        );
        // Off screen stays off screen, covered or not.
        assert_eq!(
            visual_presence(bounds(1000., 300.), area(false), true, false, false),
            Presence::Hidden
        );
    }

    #[test]
    fn a_page_fits_when_it_is_not_taller_than_the_clear_part() {
        let area = area(false);
        assert!(area.fits(px(760.)));
        assert!(!area.fits(px(761.)));
    }

    #[test]
    fn an_item_wider_than_the_clear_part_shows_its_image() {
        let wide = Bounds::new(point(px(-10.), px(200.)), size(px(820.), px(300.)));
        assert_eq!(rule(wide), Presence::Image);
    }
}
