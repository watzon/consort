//! Atkinson Hyperlegible Next and Atkinson Hyperlegible Mono, bundled in the
//! binary. Both use the SIL Open Font License 1.1 (`assets/fonts/OFL.txt` and
//! `assets/fonts/OFL-Mono.txt`).

use std::borrow::Cow;

use gpui_kit::App;

/// Font files in the binary, as (file name, bytes).
pub const FILES: [(&str, &[u8]); 8] = [
    (
        "AtkinsonHyperlegibleNext-Regular.ttf",
        include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Regular.ttf"),
    ),
    (
        "AtkinsonHyperlegibleNext-Italic.ttf",
        include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Italic.ttf"),
    ),
    (
        "AtkinsonHyperlegibleNext-Medium.ttf",
        include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Medium.ttf"),
    ),
    (
        "AtkinsonHyperlegibleNext-SemiBold.ttf",
        include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-SemiBold.ttf"),
    ),
    (
        "AtkinsonHyperlegibleNext-Bold.ttf",
        include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Bold.ttf"),
    ),
    (
        "AtkinsonHyperlegibleMono-Regular.ttf",
        include_bytes!("../assets/fonts/AtkinsonHyperlegibleMono-Regular.ttf"),
    ),
    (
        "AtkinsonHyperlegibleMono-Medium.ttf",
        include_bytes!("../assets/fonts/AtkinsonHyperlegibleMono-Medium.ttf"),
    ),
    (
        "AtkinsonHyperlegibleMono-SemiBold.ttf",
        include_bytes!("../assets/fonts/AtkinsonHyperlegibleMono-SemiBold.ttf"),
    ),
];

/// Registers the fonts with GPUI's text system. Call before any text renders.
pub fn register(cx: &mut App) {
    let fonts = FILES
        .iter()
        .map(|(_, bytes)| Cow::Borrowed(*bytes))
        .collect();
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        eprintln!("consort: could not register the bundled fonts: {error}");
    }
}

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
        let license = include_str!("../assets/fonts/OFL.txt");
        assert!(license.contains("SIL Open Font License, Version 1.1"));
        let mono = include_str!("../assets/fonts/OFL-Mono.txt");
        assert!(mono.contains("Atkinson Hyperlegible Mono"));
    }
}
