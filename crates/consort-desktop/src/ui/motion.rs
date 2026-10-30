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
    /// What moved needs the full 8 fps. Only the idle mark moved: 4 fps.
    fast: Cell<bool>,
    /// The icon button under the pointer. Only it shimmers.
    hovered: RefCell<Option<ElementId>>,
}

impl Global for FrameClock {}

impl FrameClock {
    /// Starts a draw: nothing that moves has been drawn yet.
    pub fn begin_frame(&self) {
        self.used.set(false);
        self.fast.set(false);
    }

    /// The last draw showed something that moves.
    pub fn used(&self) -> bool {
        self.used.get()
    }

    /// A draw shows something that moves at 8 fps, so the timer must run.
    pub fn mark_used(&self) {
        self.used.set(true);
        self.fast.set(true);
    }

    /// A draw shows something that moves at half speed (the idle mark), so
    /// the timer must run, at 4 fps unless something else needs more.
    pub fn mark_used_slow(&self) {
        self.used.set(true);
    }

    /// The last draw showed something that needs the full 8 fps.
    pub fn fast(&self) -> bool {
        self.fast.get()
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
/// moves and Reduce motion is off. Work in progress (`working`) moves also in
/// a window without the focus, because the user often watches it from another
/// app. The idle mark moves only in the active window, so an app at rest uses
/// no power.
pub fn clock_runs(used: bool, working: bool, window_active: bool, reduce_motion: bool) -> bool {
    used && !reduce_motion && (window_active || working)
}

/// The steps of one turn of the working spinner. At 8 frames a second, a turn
/// takes 1.5 seconds.
const SPIN_STEPS: u64 = 12;

/// The angle of the working spinner at a clock tick, in radians.
pub fn spin_angle(tick: u64) -> f32 {
    (tick % SPIN_STEPS) as f32 / SPIN_STEPS as f32 * std::f32::consts::TAU
}

/// The angle of a spinner now. A spinner that is not live, or any spinner
/// under Reduce motion, stands still. A live one tells the clock that the
/// draw moves.
pub fn spinner_angle(live: bool, cx: &App) -> f32 {
    if !live || cx.reduce_motion() {
        return 0.0;
    }
    match cx.try_global::<FrameClock>() {
        Some(clock) => {
            clock.mark_used();
            spin_angle(clock.tick())
        }
        None => 0.0,
    }
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

/// How far one dab of the writing bubble is up now, from 0 to 1. The dabs rise
/// one after another, two frames apart, and the wave takes 8 frames (one
/// second). The shimmer of a 10 px dab is too small to see, so the rise is what
/// shows the work. Under Reduce motion a dab stays down.
pub fn writing_lift(dab: u64, cx: &App) -> f32 {
    if cx.reduce_motion() {
        return 0.0;
    }
    cx.try_global::<FrameClock>()
        .map_or(0.0, |clock| wave_lift(clock.tick(), dab))
}

/// One step of the wave of [`writing_lift`], which is a pure function of the
/// clock.
fn wave_lift(tick: u64, dab: u64) -> f32 {
    const WAVE: [f32; 8] = [0.0, 0.6, 1.0, 0.6, 0.0, 0.0, 0.0, 0.0];
    let steps = WAVE.len() as u64;
    let delay = dab.wrapping_mul(2) % steps;
    WAVE[((tick % steps + steps - delay) % steps) as usize]
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
    fn the_dabs_rise_one_after_another_and_the_wave_loops() {
        assert_eq!(wave_lift(2, 0), 1.0, "the first dab is up on frame 2");
        assert_eq!(wave_lift(2, 1), 0.0, "the next dab is still down");
        assert_eq!(wave_lift(4, 1), 1.0);
        assert_eq!(wave_lift(6, 2), 1.0);
        for dab in 0..3 {
            for tick in 0..8 {
                assert_eq!(wave_lift(tick, dab), wave_lift(tick + 8, dab));
            }
        }
    }

    #[test]
    fn the_clock_runs_for_work_in_any_window_and_for_the_rest_only_in_an_active_one() {
        assert!(clock_runs(true, false, true, false));
        assert!(
            !clock_runs(false, true, true, false),
            "nothing moves on screen"
        );
        assert!(
            clock_runs(true, true, false, false),
            "work shows without the focus"
        );
        assert!(
            !clock_runs(true, false, false, false),
            "the idle mark rests"
        );
        assert!(!clock_runs(true, true, true, true), "Reduce motion is on");
    }

    #[test]
    fn the_spinner_turns_once_in_twelve_steps() {
        assert_eq!(spin_angle(0), 0.0);
        assert_eq!(spin_angle(12), 0.0);
        let quarter = spin_angle(3) - std::f32::consts::FRAC_PI_2;
        assert!(quarter.abs() < 0.001, "{quarter}");
        let angles: std::collections::BTreeSet<u32> =
            (0..12).map(|tick| spin_angle(tick).to_bits()).collect();
        assert_eq!(angles.len(), 12, "every step shows a new angle");
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
    fn only_the_idle_mark_lets_the_clock_slow_down() {
        let clock = FrameClock::default();
        clock.mark_used_slow();
        assert!(clock.used() && !clock.fast());
        clock.mark_used();
        assert!(clock.fast());
        clock.begin_frame();
        assert!(!clock.used() && !clock.fast());
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
