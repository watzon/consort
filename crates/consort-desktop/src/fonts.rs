//! Atkinson Hyperlegible Next and Atkinson Hyperlegible Mono, bundled in the
//! binary. Both use the SIL Open Font License 1.1. The files, the license
//! texts and the tests are in `consort_theme::fonts`.

use std::borrow::Cow;

use consort_theme::fonts::FILES;
use gpui_kit::App;

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
