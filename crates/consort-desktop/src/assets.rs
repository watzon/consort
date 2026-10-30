//! SVG assets in the binary: Consort's hand-drawn icons (`design/icons`)
//! with their two shimmer versions in `icons/v1` and `icons/v2`, Consort's
//! own glyphs from `design/glyphs`, the provider logos, and the frames of
//! the Three voices mark (`design/mark`). GPUI Kit's icons are the fallback
//! for its own components.
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

/// The asset path of a service logo that the client loaded. It stores the
/// image the first time, so a draw can use the path at once. GPUI reads the
/// format from the bytes: SVG, PNG, JPEG, WebP or GIF.
pub fn remote_image(key: &str, bytes: &[u8]) -> SharedString {
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = format!("remote/logo-{safe}");
    let known = REMOTE.read().is_ok_and(|images| images.contains_key(&path));
    if !known && let Ok(mut images) = REMOTE.write() {
        images.insert(path.clone(), Arc::from(bytes));
    }
    path.into()
}

// The table of every SVG in `assets/`, made by `build.rs`.
include!(concat!(env!("OUT_DIR"), "/asset_files.rs"));

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
        // The build script found the icon versions, the glyphs and the
        // mark frames.
        for path in [
            "icons/v2/plus.svg",
            "glyphs/task-connector.svg",
            "mark/light-sharp/needs-23.svg",
        ] {
            assert!(Assets.load(path).ok().flatten().is_some(), "{path}");
        }
        assert!(FILES.iter().all(|(p, _)| !p.starts_with("fonts/")));
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
