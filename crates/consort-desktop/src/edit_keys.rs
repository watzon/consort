//! The edit shortcuts of the text inputs.
//!
//! GPUI Kit binds Select all, Copy, Cut, Paste, Undo and Redo to the keys of
//! the platform in the `Input` key context. The app also reads them in its
//! root key handler. A key press that reaches that handler was not taken by
//! an input binding, so the handler sends the action to the focused input.
//! Then these shortcuts work in every input, also when a platform sends the
//! key press in a way that skips the key map.

use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};

/// One edit action of a text input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKey {
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
}

impl EditKey {
    /// The edit action for a key press, or `None` when the key is not an edit
    /// shortcut. `command` is Cmd on macOS and Ctrl on the other platforms.
    pub fn from_keystroke(key: &str, command: bool, shift: bool) -> Option<Self> {
        if !command {
            return None;
        }
        match (key, shift) {
            ("a", false) => Some(Self::SelectAll),
            ("c", false) => Some(Self::Copy),
            ("x", false) => Some(Self::Cut),
            ("v", false) => Some(Self::Paste),
            ("z", false) => Some(Self::Undo),
            ("z", true) | ("y", false) => Some(Self::Redo),
            _ => None,
        }
    }

    pub fn action(self) -> Box<dyn gpui_kit::Action> {
        match self {
            Self::SelectAll => Box::new(SelectAll),
            Self::Copy => Box::new(Copy),
            Self::Cut => Box::new(Cut),
            Self::Paste => Box::new(Paste),
            Self::Undo => Box::new(Undo),
            Self::Redo => Box::new(Redo),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_key_with_an_edit_letter_is_an_edit_shortcut() {
        assert_eq!(
            EditKey::from_keystroke("a", true, false),
            Some(EditKey::SelectAll)
        );
        assert_eq!(
            EditKey::from_keystroke("c", true, false),
            Some(EditKey::Copy)
        );
        assert_eq!(
            EditKey::from_keystroke("x", true, false),
            Some(EditKey::Cut)
        );
        assert_eq!(
            EditKey::from_keystroke("v", true, false),
            Some(EditKey::Paste)
        );
        assert_eq!(
            EditKey::from_keystroke("z", true, false),
            Some(EditKey::Undo)
        );
        assert_eq!(
            EditKey::from_keystroke("z", true, true),
            Some(EditKey::Redo)
        );
    }

    #[test]
    fn a_letter_without_the_command_key_is_text() {
        assert_eq!(EditKey::from_keystroke("a", false, false), None);
        assert_eq!(EditKey::from_keystroke("a", false, true), None);
    }

    #[test]
    fn other_command_keys_are_not_edit_shortcuts() {
        assert_eq!(EditKey::from_keystroke("k", true, false), None);
        assert_eq!(EditKey::from_keystroke("a", true, true), None);
    }
}
