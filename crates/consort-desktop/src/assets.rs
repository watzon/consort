//! SVG assets in the binary: Lucide icons with a 1.5 px stroke at 16 px, and
//! Consort's own glyphs from `design/glyphs`. GPUI Kit's icons are the
//! fallback for its own components.

use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString};

const FILES: [(&str, &[u8]); 44] = [
    (
        "icons/arrow-left.svg",
        include_bytes!("../assets/icons/arrow-left.svg"),
    ),
    ("icons/bell.svg", include_bytes!("../assets/icons/bell.svg")),
    (
        "icons/book-open.svg",
        include_bytes!("../assets/icons/book-open.svg"),
    ),
    (
        "icons/check.svg",
        include_bytes!("../assets/icons/check.svg"),
    ),
    (
        "icons/chevron-down.svg",
        include_bytes!("../assets/icons/chevron-down.svg"),
    ),
    (
        "icons/chevron-left.svg",
        include_bytes!("../assets/icons/chevron-left.svg"),
    ),
    (
        "icons/chevron-right.svg",
        include_bytes!("../assets/icons/chevron-right.svg"),
    ),
    (
        "icons/chevron-up.svg",
        include_bytes!("../assets/icons/chevron-up.svg"),
    ),
    (
        "icons/chevrons-up-down.svg",
        include_bytes!("../assets/icons/chevrons-up-down.svg"),
    ),
    (
        "icons/circle-alert.svg",
        include_bytes!("../assets/icons/circle-alert.svg"),
    ),
    (
        "icons/clock.svg",
        include_bytes!("../assets/icons/clock.svg"),
    ),
    (
        "icons/cloud.svg",
        include_bytes!("../assets/icons/cloud.svg"),
    ),
    (
        "icons/contrast.svg",
        include_bytes!("../assets/icons/contrast.svg"),
    ),
    ("icons/copy.svg", include_bytes!("../assets/icons/copy.svg")),
    (
        "icons/corner-down-left.svg",
        include_bytes!("../assets/icons/corner-down-left.svg"),
    ),
    (
        "icons/ellipsis.svg",
        include_bytes!("../assets/icons/ellipsis.svg"),
    ),
    (
        "icons/folder-open.svg",
        include_bytes!("../assets/icons/folder-open.svg"),
    ),
    (
        "icons/git-branch.svg",
        include_bytes!("../assets/icons/git-branch.svg"),
    ),
    (
        "icons/hard-drive.svg",
        include_bytes!("../assets/icons/hard-drive.svg"),
    ),
    ("icons/hash.svg", include_bytes!("../assets/icons/hash.svg")),
    (
        "icons/key-round.svg",
        include_bytes!("../assets/icons/key-round.svg"),
    ),
    (
        "icons/lightbulb.svg",
        include_bytes!("../assets/icons/lightbulb.svg"),
    ),
    ("icons/link.svg", include_bytes!("../assets/icons/link.svg")),
    ("icons/mic.svg", include_bytes!("../assets/icons/mic.svg")),
    (
        "icons/panel-right.svg",
        include_bytes!("../assets/icons/panel-right.svg"),
    ),
    ("icons/plus.svg", include_bytes!("../assets/icons/plus.svg")),
    (
        "icons/search.svg",
        include_bytes!("../assets/icons/search.svg"),
    ),
    (
        "icons/server.svg",
        include_bytes!("../assets/icons/server.svg"),
    ),
    (
        "icons/settings.svg",
        include_bytes!("../assets/icons/settings.svg"),
    ),
    (
        "icons/shield.svg",
        include_bytes!("../assets/icons/shield.svg"),
    ),
    (
        "icons/smartphone.svg",
        include_bytes!("../assets/icons/smartphone.svg"),
    ),
    (
        "icons/square-pen.svg",
        include_bytes!("../assets/icons/square-pen.svg"),
    ),
    (
        "icons/terminal.svg",
        include_bytes!("../assets/icons/terminal.svg"),
    ),
    ("icons/x.svg", include_bytes!("../assets/icons/x.svg")),
    (
        "glyphs/brace.svg",
        include_bytes!("../assets/glyphs/brace.svg"),
    ),
    (
        "glyphs/mark.svg",
        include_bytes!("../assets/glyphs/mark.svg"),
    ),
    (
        "glyphs/status-failed.svg",
        include_bytes!("../assets/glyphs/status-failed.svg"),
    ),
    (
        "glyphs/status-needs-input.svg",
        include_bytes!("../assets/glyphs/status-needs-input.svg"),
    ),
    (
        "glyphs/status-resolved.svg",
        include_bytes!("../assets/glyphs/status-resolved.svg"),
    ),
    (
        "glyphs/status-review.svg",
        include_bytes!("../assets/glyphs/status-review.svg"),
    ),
    (
        "glyphs/status-scheduled.svg",
        include_bytes!("../assets/glyphs/status-scheduled.svg"),
    ),
    (
        "glyphs/status-up-next.svg",
        include_bytes!("../assets/glyphs/status-up-next.svg"),
    ),
    (
        "glyphs/status-working-arc.svg",
        include_bytes!("../assets/glyphs/status-working-arc.svg"),
    ),
    (
        "glyphs/status-working-track.svg",
        include_bytes!("../assets/glyphs/status-working-track.svg"),
    ),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = FILES.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(*bytes)));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut items = gpui_kit::assets::Assets.list(path)?;
        items.extend(
            FILES
                .iter()
                .filter(|(p, _)| p.starts_with(path))
                .map(|(p, _)| SharedString::from(*p)),
        );
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_and_glyph_loads() {
        for (path, bytes) in FILES {
            assert!(bytes.starts_with(b"<svg"), "{path} is an SVG");
            assert!(Assets.load(path).ok().flatten().is_some(), "{path}");
        }
        assert!(
            Assets
                .load("glyphs/status-working-arc.svg")
                .ok()
                .flatten()
                .is_some()
        );
    }
}
