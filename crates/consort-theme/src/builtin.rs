//! The built-in theme files, embedded at build time. This module builds
//! without GPUI.

/// The built-in themes: Spruce (the default) and Velvet.
pub fn builtin_theme_sources() -> [&'static str; 2] {
    [
        include_str!("../../../design/themes/spruce.json"),
        include_str!("../../../design/themes/velvet.json"),
    ]
}
