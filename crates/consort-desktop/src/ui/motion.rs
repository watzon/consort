//! Stop motion: the one clock that moves the Three voices mark and the icon
//! shimmer, at 8 frames a second (`motion.frame`).
//!
//! Nothing moves by itself unless it says so in a draw. A draw that shows a
//! live status icon (Working or Needs input), a hovered icon button or an
//! animating mark calls [`FrameClock::mark_used`]. The app's timer runs only
//! while the last draw used the clock, the window is active, and Reduce
//! motion is off. An idle screen costs nothing.

use std::cell::{Cell, RefCell};

use gpui_kit::{App, ElementId, Global};

/// The frame counter that every stop-motion element reads. The app sets it
/// as a global.
#[derive(Default)]
pub struct FrameClock {
    tick: Cell<u64>,
    /// A draw since the last `begin_frame` showed something that moves.
    used: Cell<bool>,
    /// The icon button under the pointer. Only it shimmers.
    hovered: RefCell<Option<ElementId>>,
}

impl Global for FrameClock {}

impl FrameClock {
    /// Starts a draw: nothing that moves has been drawn yet.
    pub fn begin_frame(&self) {
        self.used.set(false);
    }

    /// The last draw showed something that moves.
    pub fn used(&self) -> bool {
        self.used.get()
    }

    /// A draw shows something that moves, so the timer must run.
    pub fn mark_used(&self) {
        self.used.set(true);
    }

    /// Moves the clock on by one frame.
    pub fn advance(&self) {
        self.tick.set(self.tick.get().wrapping_add(1));
    }

    pub fn tick(&self) -> u64 {
        self.tick.get()
    }

    /// The icon button under the pointer, if any.
    pub fn hovered(&self) -> Option<ElementId> {
        self.hovered.borrow().clone()
    }

    /// Notes that the pointer entered or left an icon button.
    pub fn set_hovered(&self, id: ElementId, on: bool) {
        let mut hovered = self.hovered.borrow_mut();
        if on {
            *hovered = Some(id);
        } else if hovered.as_ref() == Some(&id) {
            *hovered = None;
        }
    }
}

/// True when the frame timer must run: the last draw showed something that
/// moves, the window has the focus, and Reduce motion is off.
pub fn clock_runs(used: bool, window_active: bool, reduce_motion: bool) -> bool {
    used && window_active && !reduce_motion
}

/// The version (0, 1 or 2) that a shimmering icon shows now. An icon that
/// is not live, or any icon under Reduce motion, shows version 0. A live
/// icon tells the clock that the draw moves.
pub fn shimmer_version(live: bool, cx: &App) -> usize {
    if !live || cx.reduce_motion() {
        return 0;
    }
    match cx.try_global::<FrameClock>() {
        Some(clock) => {
            clock.mark_used();
            (clock.tick() % 3) as usize
        }
        None => 0,
    }
}

/// True when the icon button with this ID is under the pointer.
pub fn is_hovered(id: &ElementId, cx: &App) -> bool {
    cx.try_global::<FrameClock>()
        .is_some_and(|clock| clock.hovered().as_ref() == Some(id))
}

/// What the operating system asks for: on macOS, the Reduce motion setting
/// of Accessibility > Display, read with `defaults` (no `unsafe`). `None`
/// when it cannot be read, and on other systems.
pub fn system_reduce_motion() -> Option<bool> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("defaults")
        .args(["read", "com.apple.universalaccess", "reduceMotion"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_defaults_bool(&String::from_utf8_lossy(&out.stdout))
}

/// A boolean as `defaults read` prints it: `1` or `0`, or `true` or
/// `false`.
pub fn parse_defaults_bool(text: &str) -> Option<bool> {
    match text.trim() {
        "1" | "true" | "YES" => Some(true),
        "0" | "false" | "NO" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_runs_only_for_motion_on_screen_in_an_active_window() {
        assert!(clock_runs(true, true, false));
        assert!(!clock_runs(false, true, false), "nothing moves on screen");
        assert!(!clock_runs(true, false, false), "the window is not active");
        assert!(!clock_runs(true, true, true), "Reduce motion is on");
    }

    #[test]
    fn the_system_setting_reads_as_defaults_prints_it() {
        assert_eq!(parse_defaults_bool("1\n"), Some(true));
        assert_eq!(parse_defaults_bool("0"), Some(false));
        assert_eq!(parse_defaults_bool("true"), Some(true));
        assert_eq!(parse_defaults_bool(""), None);
        assert_eq!(parse_defaults_bool("2"), None);
    }

    #[test]
    fn a_draw_notes_whether_it_used_the_clock() {
        let clock = FrameClock::default();
        assert!(!clock.used());
        clock.mark_used();
        assert!(clock.used());
        clock.begin_frame();
        assert!(!clock.used());
    }

    #[test]
    fn only_the_button_under_the_pointer_is_hovered() {
        let clock = FrameClock::default();
        let a = ElementId::from("a");
        let b = ElementId::from("b");
        clock.set_hovered(a.clone(), true);
        assert_eq!(clock.hovered(), Some(a.clone()));
        // Leaving another button does not clear it.
        clock.set_hovered(b.clone(), false);
        assert_eq!(clock.hovered(), Some(a.clone()));
        clock.set_hovered(b.clone(), true);
        assert_eq!(clock.hovered(), Some(b.clone()));
        clock.set_hovered(b, false);
        assert_eq!(clock.hovered(), None);
    }
}
