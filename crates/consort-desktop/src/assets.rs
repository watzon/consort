//! SVG assets in the binary: Lucide icons with a 1.5 px stroke at 16 px,
//! Consort's own glyphs from `design/glyphs`, and the provider logos. GPUI
//! Kit's icons are the fallback for its own components.
//!
//! The icons of ACP registry agents are not in the binary. The client loads
//! them, and the app puts each one here with [`remote_icon`] before it draws
//! it, so GPUI loads it like any other asset.

use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, LazyLock, RwLock},
};

use gpui_kit::{AssetSource, SharedString};

/// The icons that the client loaded, by asset path.
static REMOTE: LazyLock<RwLock<HashMap<String, Arc<[u8]>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The asset path of a registry agent's icon. It stores the SVG text the
/// first time, so a draw can use the path at once.
pub fn remote_icon(agent_id: &str, svg: &str) -> SharedString {
    let safe: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = format!("remote/acp-{safe}.svg");
    let known = REMOTE.read().is_ok_and(|icons| icons.contains_key(&path));
    if !known && let Ok(mut icons) = REMOTE.write() {
        icons.insert(path.clone(), Arc::from(svg.as_bytes()));
    }
    path.into()
}

const FILES: [(&str, &[u8]); 55] = [
    (
        "icons/minus.svg",
        include_bytes!("../assets/icons/minus.svg"),
    ),
    (
        "icons/square.svg",
        include_bytes!("../assets/icons/square.svg"),
    ),
    (
        "icons/restore.svg",
        include_bytes!("../assets/icons/restore.svg"),
    ),
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
    ("icons/bot.svg", include_bytes!("../assets/icons/bot.svg")),
    (
        "icons/external-link.svg",
        include_bytes!("../assets/icons/external-link.svg"),
    ),
    (
        "icons/folder.svg",
        include_bytes!("../assets/icons/folder.svg"),
    ),
    (
        "icons/terminal-square.svg",
        include_bytes!("../assets/icons/terminal-square.svg"),
    ),
    (
        "providers/claude-code.svg",
        include_bytes!("../assets/providers/claude-code.svg"),
    ),
    (
        "providers/codex.svg",
        include_bytes!("../assets/providers/codex.svg"),
    ),
    (
        "providers/gemini-cli.svg",
        include_bytes!("../assets/providers/gemini-cli.svg"),
    ),
    (
        "providers/opencode.svg",
        include_bytes!("../assets/providers/opencode.svg"),
    ),
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
        if let Some(bytes) = REMOTE
            .read()
            .ok()
            .and_then(|icons| icons.get(path).cloned())
        {
            return Ok(Some(Cow::Owned(bytes.to_vec())));
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

    #[test]
    fn a_registry_icon_loads_from_its_path() {
        let path = remote_icon("amp/acp", "<svg></svg>");
        assert_eq!(path.as_ref(), "remote/acp-amp_acp.svg");
        assert_eq!(
            Assets.load(&path).ok().flatten().as_deref(),
            Some(b"<svg></svg>".as_slice())
        );
    }
}
