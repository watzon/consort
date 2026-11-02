//! Atkinson Hyperlegible Next and Atkinson Hyperlegible Mono, in the binary.
//! Both use the SIL Open Font License 1.1. The files and the license texts
//! are in `design/fonts/`. This module builds without GPUI, so the desktop
//! app, the visual page contract and the renderer share one copy.

/// Font files in the binary, as (file name, bytes).
pub const FILES: [(&str, &[u8]); 8] = [
    (
        "AtkinsonHyperlegibleNext-Regular.ttf",
        include_bytes!("../../../design/fonts/AtkinsonHyperlegibleNext-Regular.ttf"),
    ),
    (
        "AtkinsonHyperlegibleNext-Italic.ttf",
        include_bytes!("../../../design/fonts/AtkinsonHyperlegibleNext-Italic.ttf"),
    ),
    (
        "AtkinsonHyperlegibleNext-Medium.ttf",
        include_bytes!("../../../design/fonts/AtkinsonHyperlegibleNext-Medium.ttf"),
    ),
    (
        "AtkinsonHyperlegibleNext-SemiBold.ttf",
        include_bytes!("../../../design/fonts/AtkinsonHyperlegibleNext-SemiBold.ttf"),
    ),
    (
        "AtkinsonHyperlegibleNext-Bold.ttf",
        include_bytes!("../../../design/fonts/AtkinsonHyperlegibleNext-Bold.ttf"),
    ),
    (
        "AtkinsonHyperlegibleMono-Regular.ttf",
        include_bytes!("../../../design/fonts/AtkinsonHyperlegibleMono-Regular.ttf"),
    ),
    (
        "AtkinsonHyperlegibleMono-Medium.ttf",
        include_bytes!("../../../design/fonts/AtkinsonHyperlegibleMono-Medium.ttf"),
    ),
    (
        "AtkinsonHyperlegibleMono-SemiBold.ttf",
        include_bytes!("../../../design/fonts/AtkinsonHyperlegibleMono-SemiBold.ttf"),
    ),
];

/// The license text of Atkinson Hyperlegible Next.
pub const LICENSE: &str = include_str!("../../../design/fonts/OFL.txt");

/// The license text of Atkinson Hyperlegible Mono.
pub const LICENSE_MONO: &str = include_str!("../../../design/fonts/OFL-Mono.txt");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_font_file_is_a_truetype_font() {
        for (name, bytes) in FILES {
            assert!(bytes.len() > 40_000, "{name} is too small");
            assert_eq!(&bytes[..4], &[0, 1, 0, 0], "{name} is not a TrueType file");
        }
    }

    #[test]
    fn the_ofl_license_ships_with_the_fonts() {
        assert!(LICENSE.contains("SIL Open Font License, Version 1.1"));
        assert!(LICENSE_MONO.contains("Atkinson Hyperlegible Mono"));
    }

    #[test]
    fn the_file_names_are_the_ones_that_the_apps_expect() {
        let names: Vec<&str> = FILES.iter().map(|(name, _)| *name).collect();
        assert_eq!(names[0], "AtkinsonHyperlegibleNext-Regular.ttf");
        assert_eq!(names[4], "AtkinsonHyperlegibleNext-Bold.ttf");
        assert_eq!(names[7], "AtkinsonHyperlegibleMono-SemiBold.ttf");
    }
}
