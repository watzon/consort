//! `design/structure.json`: type, spacing, corners, shadows, motion, icons
//! and layout. The same for every theme.

use std::sync::LazyLock;

use serde::Deserialize;

pub(crate) const STRUCTURE_JSON: &str = include_str!("../../../design/structure.json");

#[derive(Clone, Debug, Deserialize)]
pub struct Structure {
    pub font: Fonts,
    #[serde(rename = "type")]
    pub type_scale: TypeScale,
    pub space: Space,
    pub radius: Radius,
    pub border_width: f32,
    pub shadow: Shadows,
    pub motion: Motion,
    pub icon: Icons,
    pub layout: Layout,
    pub derive: Derive,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Fonts {
    pub sans: String,
    pub mono: String,
    pub license: String,
}

/// One text style: size and line height in px, weight, and tracking in em.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct TypeStyle {
    pub size: f32,
    pub line: f32,
    pub weight: u16,
    pub tracking: f32,
    #[serde(default)]
    pub max_chars: Option<u16>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct TypeScale {
    pub display: TypeStyle,
    pub heading: TypeStyle,
    pub title: TypeStyle,
    pub body: TypeStyle,
    pub ui: TypeStyle,
    pub ui_strong: TypeStyle,
    pub label: TypeStyle,
    pub caption: TypeStyle,
    pub mono: TypeStyle,
    pub mono_inline_scale: f32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Space {
    pub xxs: f32,
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
    pub xxl: f32,
    pub page: f32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Radius {
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
    pub full: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Shadows {
    pub float: ShadowModes,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ShadowModes {
    pub dark: Shadow,
    pub light: Shadow,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Motion {
    pub fast_ms: u64,
    pub normal_ms: u64,
    pub slow_ms: u64,
    pub ease_enter: [f32; 4],
    pub ease_exit: [f32; 4],
}

#[derive(Clone, Debug, Deserialize)]
pub struct Icons {
    pub set: String,
    pub size: f32,
    pub dense_size: f32,
    pub toolbar_size: f32,
    pub stroke: f32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Layout {
    pub sidebar_width: f32,
    pub panel_width: f32,
    pub timeline_max_width: f32,
    pub speaker_gutter: f32,
    pub min_window: [f32; 2],
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Derive {
    pub hover_delta_l: f64,
    pub active_delta_l: f64,
    pub selection_alpha: f32,
    pub status_tint_alpha: f32,
    pub scrim_alpha: ScrimAlpha,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ScrimAlpha {
    pub dark: f32,
    pub light: f32,
}

static STRUCTURE: LazyLock<Structure> = LazyLock::new(|| {
    // The file is embedded at build time, and the `structure_parses` test
    // proves that it parses, so this cannot fail at run time.
    #[allow(clippy::expect_used)]
    serde_json::from_str(STRUCTURE_JSON).expect("design/structure.json parses")
});

/// The parsed `design/structure.json`.
pub fn structure() -> &'static Structure {
    &STRUCTURE
}

#[cfg(test)]
mod tests {
    #[test]
    fn structure_parses() {
        let s: super::Structure = serde_json::from_str(super::STRUCTURE_JSON).expect("parses");
        assert_eq!(s.layout.sidebar_width, 248.0);
        assert_eq!(s.type_scale.body.size, 15.0);
        assert_eq!(s.space.page, 48.0);
    }
}
