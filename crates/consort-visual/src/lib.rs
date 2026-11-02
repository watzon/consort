//! The page contract of a Consort visual (`docs/VISUALS-PLAN.md`). This
//! crate is pure: it reads no file and opens no connection. It has these
//! parts:
//!
//! - [`check_html`] checks the HTML that the assistant wrote.
//! - [`local_images`], [`sniff_image`] and [`inline_images`] find the local
//!   images of a page, check them by their magic bytes, and put them in the
//!   page as `data:` URIs. [`page_name`] makes the file name of a page.
//! - [`VARIABLES`], [`theme_style`] and [`default_style`] name the theme
//!   variables of a page and give their values.
//! - [`document`] builds the document that a client or the renderer loads:
//!   the bootstrap (the theme variables, the fonts, the base sheet, the size
//!   reporter, the link catcher and the ink kit) and the page.
//!   [`parse_message`] reads what the bootstrap sends back.
//!   [`SETTLE_EXPRESSION`] and [`MEASURE_EXPRESSION`] are for the renderer.
//!
//! It depends only on `consort-proto` and `consort-theme` (without GPUI).

mod check;
mod error;
mod images;
mod page;
mod scan;
mod style;

pub use check::check_html;
pub use error::PageError;
pub use images::{LocalImage, inline_images, local_images, page_name, sniff_image};
pub use page::{
    Host, MEASURE_EXPRESSION, PageMessage, SETTLE_EXPRESSION, document, fonts_css, parse_message,
};
pub use style::{VARIABLES, default_style, theme_style};
