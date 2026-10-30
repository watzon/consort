//! The mic button of the composer.
//!
//! A click focuses the message box and asks the system to start dictation.
//! The click logic is here, apart from the platform call, so a headless test
//! can prove what happens when the system does not start it.
//!
//! **The platform call is not wired.** On macOS the call is the action
//! `startDictation:` sent with `-[NSApplication sendAction:to:from:]`. Checked
//! on this machine (macOS SDK 27, `xcrun --show-sdk-path`):
//!
//! - `startDictation:` is in no public AppKit header. The menu item "Start
//!   Dictation" uses it, and `NSApplication` answers to it at run time.
//! - `sendAction:to:from:` is declared `BOOL`, in `NSApplication.h`. It
//!   returns `NO` when no object in the chain takes the action. A probe
//!   program that sent `startDictation:` with no window got `YES` and no
//!   exception.
//! - Every way to send a selector in `objc2` and `objc2-app-kit` 0.3.2 is
//!   `unsafe` (`sendAction_to_from`, `tryToPerform_with`, `msg_send!`).
//!   AGENTS.md forbids `unsafe_code` in every crate, so Consort cannot make
//!   the call. [`start_system_dictation`] is the one place to change, if a
//!   safe binding exists one day. Until then it answers `false`, and the
//!   button shows the hint (DECISIONS 738).

/// What a click on the mic button did, for the caller to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MicOutcome {
    /// The system started dictation. There is nothing to say.
    Started,
    /// The system did not start it. The text tells the user how to dictate.
    Hint(String),
}

/// The words for a system that has no dictation to start.
pub const NO_DICTATION: &str = "This system has no built-in dictation that Consort can start.";

/// Decides what the click shows. `started` is the answer of the platform
/// call, and `hint` is `convo::dictation_hint()`.
pub fn outcome(started: bool, hint: Option<&str>) -> MicOutcome {
    if started {
        MicOutcome::Started
    } else {
        MicOutcome::Hint(hint.unwrap_or(NO_DICTATION).to_owned())
    }
}

/// Asks the system to start dictation. `true` when it did.
///
/// See the module text: Consort has no safe call for it, so this answers
/// `false` on every system, and the click shows the hint.
pub fn start_system_dictation() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_started_dictation_shows_nothing() {
        assert_eq!(outcome(true, Some("hint")), MicOutcome::Started);
        assert_eq!(outcome(true, None), MicOutcome::Started);
    }

    #[test]
    fn a_dictation_that_did_not_start_shows_the_hint_of_the_system() {
        assert_eq!(
            outcome(false, Some("To dictate, press Windows key+H.")),
            MicOutcome::Hint("To dictate, press Windows key+H.".into())
        );
    }

    #[test]
    fn a_system_with_no_hint_says_that_it_has_no_dictation() {
        assert_eq!(outcome(false, None), MicOutcome::Hint(NO_DICTATION.into()));
    }

    #[test]
    fn the_platform_call_is_not_wired_so_it_never_claims_a_start() {
        assert!(!start_system_dictation());
    }
}
