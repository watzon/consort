//! The Three voices mark: three dabs of pigment for the voices of a broken
//! consort. It replaces the brace logo, and over the timeline it shows what
//! the channel's assistant does.
//!
//! GPUI cannot run an SVG noise filter at every frame, so `design/mark`
//! renders the frames ahead of time into `assets/mark/`. GPUI's image
//! renderer (resvg) draws their filter and their screen and multiply blends.
//! The app plays the frames at 8 frames a second on the shared
//! [`crate::ui::motion::FrameClock`].

use std::time::{Duration, Instant};

use consort_client::activity::MarkState;
use consort_theme::Tokens;
use gpui_kit::{AnyElement, IntoElement, Pixels, SharedString, Styled, img};

/// What the mark shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MarkPhase {
    /// At rest: one pose whose edges shimmer at half speed.
    Idle,
    /// The voices circle each other.
    Working,
    /// Gold comes forward and beats twice a second.
    NeedsYou,
    /// The voices close into a chord, for a moment after work ends.
    Done,
}

impl MarkPhase {
    /// The frames in one loop.
    pub fn frames(self) -> u64 {
        match self {
            MarkPhase::Done => 1,
            MarkPhase::Idle => 3,
            MarkPhase::Working => 12,
            MarkPhase::NeedsYou => 24,
        }
    }

    fn file(self) -> &'static str {
        match self {
            MarkPhase::Idle => "idle",
            MarkPhase::Working => "working",
            MarkPhase::NeedsYou => "needs",
            MarkPhase::Done => "done",
        }
    }

    /// True for a phase that loops.
    pub fn animates(self) -> bool {
        self.frames() > 1
    }

    /// Clock ticks per frame: idle shimmers at half speed (4 fps), so an
    /// app at rest redraws half as often.
    pub fn ticks_per_frame(self) -> u64 {
        match self {
            MarkPhase::Idle => 2,
            _ => 1,
        }
    }

    /// The `--screen` name of the phase.
    pub fn from_screen(name: &str) -> Option<MarkPhase> {
        match name {
            "mark-idle" => Some(MarkPhase::Idle),
            "mark-working" => Some(MarkPhase::Working),
            "mark-needs-you" => Some(MarkPhase::NeedsYou),
            "mark-done" => Some(MarkPhase::Done),
            _ => None,
        }
    }
}

impl From<MarkState> for MarkPhase {
    fn from(state: MarkState) -> MarkPhase {
        match state {
            MarkState::Idle => MarkPhase::Idle,
            MarkState::Working => MarkPhase::Working,
            MarkState::NeedsYou => MarkPhase::NeedsYou,
        }
    }
}

/// The asset path of one frame. `sharp` is the version with no paint edges,
/// for 16 to 20 px.
pub fn frame_path(phase: MarkPhase, frame: u64, dark: bool, sharp: bool) -> SharedString {
    let set = match (dark, sharp) {
        (true, false) => "dark",
        (false, false) => "light",
        (true, true) => "dark-sharp",
        (false, true) => "light-sharp",
    };
    let n = frame % phase.frames();
    format!("mark/{set}/{}-{n:02}.svg", phase.file()).into()
}

/// The still mark at a small size, such as the Tutti row in the sidebar.
/// At 20 px and under it uses the sharp frames.
pub fn still_mark(size: Pixels, t: &Tokens) -> AnyElement {
    let sharp = size <= t.component.chat.mark_small + t.space.xs;
    img(frame_path(MarkPhase::Idle, 0, t.is_dark(), sharp))
        .size(size)
        .flex_none()
        .into_any_element()
}

/// Follows the activity of the open channel and decides what the mark
/// shows: Working ends with Done for `motion.done_hold`, then the new state.
/// Each phase starts at its first frame.
#[derive(Clone, Debug)]
pub struct MarkPlayer {
    /// The state that the client reports now.
    state: MarkState,
    /// The channel that the state belongs to.
    channel: String,
    /// Done shows until this time.
    done_until: Option<Instant>,
    /// The clock tick at which the shown phase started.
    start_tick: u64,
    /// The phase that the last call showed, to restart the loop when it
    /// changes.
    shown: MarkPhase,
}

impl Default for MarkPlayer {
    fn default() -> Self {
        MarkPlayer {
            state: MarkState::Idle,
            channel: String::new(),
            done_until: None,
            start_tick: 0,
            shown: MarkPhase::Idle,
        }
    }
}

impl MarkPlayer {
    /// Takes the state of the open channel. Returns the time at which Done
    /// ends when this call started it, so the caller can wake up then.
    pub fn follow(
        &mut self,
        channel: &str,
        state: MarkState,
        now: Instant,
        hold: Duration,
    ) -> Option<Instant> {
        if channel != self.channel {
            // Another channel: no Done for work that ended elsewhere.
            self.channel = channel.to_owned();
            self.state = state;
            self.done_until = None;
            return None;
        }
        let ended = self.state == MarkState::Working && state != MarkState::Working;
        self.state = state;
        if state == MarkState::Working {
            self.done_until = None;
        } else if ended {
            let until = now + hold;
            self.done_until = Some(until);
            return Some(until);
        }
        None
    }

    /// The phase to show at `now`.
    pub fn phase(&self, now: Instant) -> MarkPhase {
        match self.done_until {
            Some(until) if now < until => MarkPhase::Done,
            _ => self.state.into(),
        }
    }

    /// The frame of `phase` at clock `tick`. Under Reduce motion the mark
    /// changes pose only when its phase changes, so it stays on frame 0.
    pub fn frame(&mut self, phase: MarkPhase, tick: u64, reduce_motion: bool) -> u64 {
        if phase != self.shown {
            self.shown = phase;
            self.start_tick = tick;
        }
        if reduce_motion || !phase.animates() {
            0
        } else {
            (tick.wrapping_sub(self.start_tick) / phase.ticks_per_frame()) % phase.frames()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_paths_name_the_generated_files() {
        assert_eq!(
            frame_path(MarkPhase::Working, 13, true, false),
            "mark/dark/working-01.svg"
        );
        assert_eq!(
            frame_path(MarkPhase::NeedsYou, 23, false, true),
            "mark/light-sharp/needs-23.svg"
        );
        assert_eq!(
            frame_path(MarkPhase::Idle, 7, true, true),
            "mark/dark-sharp/idle-01.svg"
        );
        assert_eq!(
            frame_path(MarkPhase::Done, 0, false, false),
            "mark/light/done-00.svg"
        );
    }

    #[test]
    fn every_frame_of_every_phase_is_in_the_binary() {
        use gpui_kit::AssetSource as _;
        let assets = crate::assets::Assets;
        for phase in [
            MarkPhase::Idle,
            MarkPhase::Working,
            MarkPhase::NeedsYou,
            MarkPhase::Done,
        ] {
            for frame in 0..phase.frames() {
                for (dark, sharp) in [(true, false), (false, false), (true, true), (false, true)] {
                    let path = frame_path(phase, frame, dark, sharp);
                    assert!(assets.load(&path).ok().flatten().is_some(), "{path}");
                }
            }
        }
    }

    /// Renders a frame the way GPUI draws an image: resvg at 176 px.
    fn render(svg: &str) -> Vec<u8> {
        let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default())
            .expect("the frame parses");
        let mut pixmap = resvg::tiny_skia::Pixmap::new(176, 176).expect("a pixmap");
        let scale = 176.0 / tree.size().width();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        pixmap.data().to_vec()
    }

    fn frame(path: &str) -> String {
        use gpui_kit::AssetSource as _;
        let bytes = crate::assets::Assets
            .load(path)
            .ok()
            .flatten()
            .expect("the frame is in the binary");
        String::from_utf8(bytes.to_vec()).expect("SVG text")
    }

    /// The paint filter runs: the same frame without its filter has other
    /// edges.
    #[test]
    fn gpuis_renderer_draws_the_paint_filter() {
        let svg = frame("mark/dark/working-00.svg");
        let plain = svg.replace(" filter=\"url(#f)\"", "");
        assert_ne!(svg, plain, "the frame has a filter");
        let (a, b) = (render(&svg), render(&plain));
        let changed = a
            .chunks(4)
            .zip(b.chunks(4))
            .filter(|(x, y)| x[3].abs_diff(y[3]) > 32)
            .count();
        assert!(changed > 200, "only {changed} edge pixels moved");
    }

    /// The blends run: screen makes the overlaps lighter on dark, and
    /// multiply makes them darker on light, than plain painting does.
    #[test]
    fn gpuis_renderer_draws_the_screen_and_multiply_blends() {
        let total = |pixels: &[u8]| -> u64 {
            pixels
                .chunks(4)
                .map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]))
                .sum()
        };
        for (path, blend, lighter) in [
            ("mark/dark-sharp/working-00.svg", "screen", true),
            ("mark/light-sharp/working-00.svg", "multiply", false),
        ] {
            let svg = frame(path);
            let normal = svg.replace(&format!("mix-blend-mode:{blend}"), "mix-blend-mode:normal");
            assert_ne!(svg, normal, "{path} uses {blend}");
            let (blended, plain) = (total(&render(&svg)), total(&render(&normal)));
            if lighter {
                assert!(blended > plain, "{path}: {blended} <= {plain}");
            } else {
                assert!(blended < plain, "{path}: {blended} >= {plain}");
            }
        }
    }

    #[test]
    fn working_that_ends_shows_done_then_the_new_state() {
        let hold = Duration::from_millis(1500);
        let t0 = Instant::now();
        let mut p = MarkPlayer::default();
        assert_eq!(p.follow("c", MarkState::Working, t0, hold), None);
        assert_eq!(p.phase(t0), MarkPhase::Working);
        let until = p.follow("c", MarkState::Idle, t0, hold);
        assert_eq!(until, Some(t0 + hold));
        assert_eq!(p.phase(t0 + Duration::from_millis(1499)), MarkPhase::Done);
        assert_eq!(p.phase(t0 + hold), MarkPhase::Idle);
        // Needs you after work: Done first, then Needs you.
        p.follow("c", MarkState::Working, t0, hold);
        p.follow("c", MarkState::NeedsYou, t0, hold);
        assert_eq!(p.phase(t0), MarkPhase::Done);
        assert_eq!(p.phase(t0 + hold), MarkPhase::NeedsYou);
    }

    #[test]
    fn work_that_starts_again_cuts_done_short() {
        let hold = Duration::from_millis(1500);
        let t0 = Instant::now();
        let mut p = MarkPlayer::default();
        p.follow("c", MarkState::Working, t0, hold);
        p.follow("c", MarkState::Idle, t0, hold);
        p.follow("c", MarkState::Working, t0, hold);
        assert_eq!(p.phase(t0), MarkPhase::Working);
    }

    #[test]
    fn a_channel_change_shows_the_new_state_without_done() {
        let hold = Duration::from_millis(1500);
        let t0 = Instant::now();
        let mut p = MarkPlayer::default();
        p.follow("a", MarkState::Working, t0, hold);
        assert_eq!(p.follow("b", MarkState::Idle, t0, hold), None);
        assert_eq!(p.phase(t0), MarkPhase::Idle);
    }

    #[test]
    fn the_loop_starts_at_frame_zero_and_reduce_motion_holds_it() {
        let mut p = MarkPlayer::default();
        assert_eq!(p.frame(MarkPhase::Working, 100, false), 0);
        assert_eq!(p.frame(MarkPhase::Working, 101, false), 1);
        assert_eq!(p.frame(MarkPhase::Working, 112, false), 0);
        assert_eq!(p.frame(MarkPhase::Working, 105, true), 0);
        // A new phase starts at its first frame.
        assert_eq!(p.frame(MarkPhase::NeedsYou, 107, false), 0);
        assert_eq!(p.frame(MarkPhase::NeedsYou, 130, false), 23);
        assert_eq!(p.frame(MarkPhase::Idle, 131, false), 0);
        // Idle shimmers at half speed over its three edge versions.
        assert_eq!(p.frame(MarkPhase::Idle, 132, false), 0);
        assert_eq!(p.frame(MarkPhase::Idle, 133, false), 1);
        assert_eq!(p.frame(MarkPhase::Idle, 135, false), 2);
        assert_eq!(p.frame(MarkPhase::Idle, 137, false), 0);
        assert_eq!(p.frame(MarkPhase::Idle, 138, true), 0, "Reduce motion");
    }
}
