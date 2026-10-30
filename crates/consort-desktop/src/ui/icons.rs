//! Consort's hand-drawn icon set (`design/icons`) and the names that the app
//! and the hub used for the Lucide icons before it.
//!
//! Every icon has three hand-drawn versions: `icons/<name>.svg`,
//! `icons/v1/<name>.svg` and `icons/v2/<name>.svg`. The shimmer plays them
//! in turn at 8 frames a second (see [`crate::ui::motion`]).
//!
//! Channel icons are data: the hub keeps the Lucide name that a channel got,
//! such as `hash` or `mic`. [`resolve`] gives the nearest icon of the new
//! set for each old name, so old channels keep an icon.

use gpui_kit::SharedString;

/// The nearest icon of the new set for a Lucide name. A name of the new set
/// stays as it is. `docs/notes/redesign-desktop.md` lists the names that
/// have no close match.
pub fn resolve(name: &str) -> &str {
    match name {
        "arrow-left" => "back",
        "book-open" => "memory",
        "box" => "skill",
        // No match: the nearest is the warning sign.
        "circle-alert" => "warning",
        "cloud" => "globe",
        "contrast" => "appearance",
        "corner-down-left" => "send",
        "ellipsis" => "more-horizontal",
        "file-text" => "file",
        "folder-open" => "folder",
        "git-branch" => "branch",
        // No match: a drive is storage on a machine.
        "hard-drive" => "server",
        "hash" => "channel",
        "inbox" => "mail",
        "key-round" => "key",
        // No match: the nearest light is the sun.
        "lightbulb" => "sun",
        "mic" => "voice",
        "panel-right" => "sidebar",
        // No match: two boxes, like the restore control of a window.
        "restore" => "copy",
        "shield" => "lock",
        "signal" => "effort",
        // No match: a client is a device that the user holds.
        "smartphone" => "worker",
        "square" => "stop",
        "square-pen" => "compose",
        "terminal-square" => "terminal",
        "x" => "close",
        other => other,
    }
}

/// The asset path of an icon in one of its three versions (0, 1 or 2).
pub fn path(name: &str, version: usize) -> SharedString {
    let name = resolve(name);
    match version % 3 {
        0 => format!("icons/{name}.svg"),
        v => format!("icons/v{v}/{name}.svg"),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::AssetSource as _;

    /// Every Lucide name that the app or the mock used, and every channel
    /// icon of the Change icon menu.
    const OLD: [&str; 41] = [
        "arrow-left",
        "bell",
        "book-open",
        "box",
        "check",
        "chevron-down",
        "chevron-left",
        "chevron-right",
        "chevron-up",
        "chevrons-up-down",
        "circle-alert",
        "clock",
        "cloud",
        "contrast",
        "copy",
        "corner-down-left",
        "ellipsis",
        "external-link",
        "file-text",
        "folder-open",
        "folder",
        "git-branch",
        "hard-drive",
        "hash",
        "inbox",
        "info",
        "key-round",
        "lightbulb",
        "link",
        "mic",
        "minus",
        "panel-right",
        "plus",
        "restore",
        "search",
        "server",
        "settings",
        "shield",
        "signal",
        "smartphone",
        "square",
    ];

    #[test]
    fn every_old_name_and_its_versions_load() {
        let assets = crate::assets::Assets;
        for name in OLD.iter().chain(&[
            "square-pen",
            "star",
            "star-filled",
            "terminal",
            "terminal-square",
            "upload",
            "users",
            "x",
        ]) {
            for v in 0..3 {
                let p = path(name, v);
                assert!(assets.load(&p).ok().flatten().is_some(), "{name} -> {p}");
            }
        }
    }

    #[test]
    fn the_versions_are_different_drawings() {
        let assets = crate::assets::Assets;
        let load = |v| {
            assets
                .load(&path("plus", v))
                .ok()
                .flatten()
                .map(|b| b.to_vec())
        };
        assert_ne!(load(0), load(1));
        assert_ne!(load(1), load(2));
        assert_eq!(path("hash", 4), "icons/v1/channel.svg");
    }
}
