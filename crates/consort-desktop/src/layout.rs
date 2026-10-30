//! How the window shares its width: the sidebar, the chat column and the
//! right panel. These rules need no window, so tests prove them directly.
//!
//! When the window shrinks, the panel shrinks to its least width first. Then
//! the sidebar hides on its own, which does not change the user's choice.
//! Then the panel covers the window, and under `compact_width` the app is one
//! column.

/// The sizes that the rules use, in pixels.
#[derive(Clone, Copy, Debug)]
pub struct Sizes {
    pub sidebar: f32,
    pub panel_min: f32,
    pub chat_min: f32,
    pub compact: f32,
}

/// The result for one window width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// The app is one column, with the top bar and the drawer.
    pub compact: bool,
    /// The sidebar sits at the left of the chat.
    pub sidebar: bool,
    /// The width of the open panel, 0 when none is open.
    pub panel: f32,
    /// The panel covers the window, with Back at the top left.
    pub panel_over: bool,
}

/// `wanted` is the width that the open panel asks for, `None` with no panel.
pub fn frame(width: f32, sizes: Sizes, sidebar_hidden: bool, wanted: Option<f32>) -> Frame {
    let compact = width < sizes.compact;
    let Some(wanted) = wanted else {
        return Frame {
            compact,
            sidebar: !compact && !sidebar_hidden && width - sizes.sidebar >= sizes.chat_min,
            panel: 0.0,
            panel_over: false,
        };
    };
    let sidebar =
        !compact && !sidebar_hidden && width - sizes.sidebar - sizes.chat_min >= sizes.panel_min;
    let room = width - sizes.chat_min - if sidebar { sizes.sidebar } else { 0.0 };
    if compact || room < sizes.panel_min {
        return Frame {
            compact,
            sidebar: false,
            panel: width,
            panel_over: true,
        };
    }
    Frame {
        compact,
        sidebar,
        panel: wanted.max(sizes.panel_min).min(room),
        panel_over: false,
    }
}

/// Whether the corner notifications use the compact position (the full width
/// under the top bar) in place of the corner above the composer.
///
/// The stack is `stack` px wide, with its margin, and sits at the right edge
/// of the window. It would cover the chat column when the column has less
/// than `chat_min` px clear of it. The column is the chat beside the sidebar
/// and the panel. A panel that is as wide as the stack carries it, so the
/// stack covers the panel and not the chat. A panel that covers the window is
/// the column then. In the compact layout the stack is always compact.
///
/// With the sidebar shown and no panel, the corner works from
/// `sidebar + chat_min + stack` px: 1144 px with the design tokens.
pub fn notices_compact(width: f32, frame: &Frame, sizes: Sizes, stack: f32) -> bool {
    if frame.compact {
        return true;
    }
    let (column, beside) = if frame.panel_over {
        (frame.panel, 0.0)
    } else {
        let sidebar = if frame.sidebar { sizes.sidebar } else { 0.0 };
        (width - sidebar - frame.panel, frame.panel)
    };
    let covered = (stack - beside).max(0.0);
    column - covered < sizes.chat_min
}

/// The panel width while the pointer drags its edge at `pointer_x`.
pub fn dragged_panel_width(width: f32, pointer_x: f32, sizes: Sizes) -> f32 {
    (width - pointer_x).max(sizes.panel_min)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: Sizes = Sizes {
        sidebar: 288.0,
        panel_min: 320.0,
        chat_min: 480.0,
        compact: 760.0,
    };

    #[test]
    fn a_wide_window_gives_the_panel_the_width_it_asks_for() {
        let f = frame(1440.0, SIZES, false, Some(400.0));
        assert!(f.sidebar && !f.compact && !f.panel_over);
        assert_eq!(f.panel, 400.0);
    }

    #[test]
    fn the_panel_shrinks_first_and_the_chat_keeps_its_least_width() {
        let f = frame(1200.0, SIZES, false, Some(600.0));
        assert!(f.sidebar);
        assert_eq!(f.panel, 1200.0 - 288.0 - 480.0);
        assert_eq!(frame(1088.0, SIZES, false, Some(600.0)).panel, 320.0);
    }

    #[test]
    fn the_sidebar_hides_on_its_own_after_the_panel_reaches_its_least_width() {
        let f = frame(1087.0, SIZES, false, Some(400.0));
        assert!(!f.sidebar && !f.panel_over);
        assert_eq!(f.panel, 400.0);
    }

    #[test]
    fn a_window_with_no_room_for_chat_and_panel_covers_the_window_with_the_panel() {
        let f = frame(799.0, SIZES, false, Some(400.0));
        assert!(f.panel_over && !f.compact && !f.sidebar);
        assert_eq!(f.panel, 799.0);
        assert!(!frame(800.0, SIZES, false, Some(400.0)).panel_over);
    }

    #[test]
    fn under_the_compact_width_the_app_is_one_column() {
        let f = frame(759.0, SIZES, false, None);
        assert!(f.compact && !f.sidebar);
        let f = frame(759.0, SIZES, false, Some(400.0));
        assert!(f.compact && f.panel_over);
        assert!(!frame(760.0, SIZES, true, None).compact);
    }

    #[test]
    fn the_user_can_hide_the_sidebar_at_any_width() {
        assert!(!frame(1440.0, SIZES, true, None).sidebar);
        assert!(frame(1440.0, SIZES, false, None).sidebar);
    }

    #[test]
    fn a_drag_never_goes_under_the_least_width() {
        assert_eq!(dragged_panel_width(1440.0, 1000.0, SIZES), 440.0);
        assert_eq!(dragged_panel_width(1440.0, 1400.0, SIZES), 320.0);
    }

    /// The notification stack: 360 px and a 16 px margin.
    const STACK: f32 = 376.0;

    fn compact(width: f32, hidden: bool, panel: Option<f32>) -> bool {
        let f = frame(width, SIZES, hidden, panel);
        notices_compact(width, &f, SIZES, STACK)
    }

    #[test]
    fn the_stack_sits_in_the_corner_when_it_leaves_the_chat_its_least_width() {
        // Sidebar shown, no panel: 288 + 480 + 376 = 1144 px.
        assert!(!compact(1440.0, false, None));
        assert!(!compact(1144.0, false, None));
        assert!(compact(1143.0, false, None));
        // The user's window of 760 to 1100 px: the compact position.
        for width in [760.0, 900.0, 1000.0, 1100.0] {
            assert!(compact(width, false, None), "{width}");
        }
    }

    #[test]
    fn a_hidden_sidebar_gives_the_chat_back_its_room() {
        // The sidebar is hidden by the user: 480 + 376 = 856 px.
        assert!(!compact(856.0, true, None));
        assert!(compact(855.0, true, None));
        // The sidebar hides on its own in a narrow window: the same rule.
        assert!(compact(800.0, false, None));
    }

    #[test]
    fn a_panel_as_wide_as_the_stack_carries_it() {
        // The stack covers the panel, not the chat, so the chat keeps its
        // width and the corner stays.
        assert!(!compact(1440.0, false, Some(400.0)));
        assert!(!compact(1200.0, false, Some(400.0)));
        // A panel at its least width (320 px) lets the stack cover 56 px of
        // the chat. The chat then needs 536 px.
        assert!(!compact(288.0 + 536.0 + 320.0, false, Some(320.0)));
        assert!(compact(288.0 + 535.0 + 320.0, false, Some(320.0)));
    }

    #[test]
    fn a_panel_over_the_window_is_the_column_and_the_compact_layout_is_compact() {
        // 799 px: the panel covers the window. The stack leaves 423 px of it.
        assert!(compact(799.0, false, Some(400.0)));
        assert!(compact(759.0, false, None));
        assert!(compact(420.0, false, Some(400.0)));
    }
}
