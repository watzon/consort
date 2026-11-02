//! Why a page was refused. Each message is for the model that wrote the
//! page: it says what happened and what to do.

use consort_proto::model::{
    VISUAL_HTML_MAX_CHARS, VISUAL_IMAGE_MAX_BYTES, VISUAL_PAGE_MAX_BYTES, VisualStyleError,
};

const MIB: u64 = 1024 * 1024;

/// Why a page, an image or a style was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PageError {
    /// The page has no content.
    #[error("The page is empty. Write the HTML of the visual, then try again.")]
    Empty,
    /// The page has too many characters.
    #[error(
        "The page has {chars} characters. The limit is {VISUAL_HTML_MAX_CHARS}. Make the page shorter, then try again."
    )]
    TooLong {
        /// The number of characters in the page.
        chars: usize,
    },
    /// A `<style>` block or a `style=` attribute uses `vh`, `svh`, `lvh` or
    /// `dvh`.
    #[error(
        "The page uses viewport height units ({}). A visual grows to fit its content. Set heights in px, or let the content set them.",
        .found.join(", ")
    )]
    ViewportUnits {
        /// Up to five samples, as they are written in the page.
        found: Vec<String>,
    },
    /// A local image file is too large.
    #[error(
        "The image {path} is larger than {} MiB. Use a smaller image, then try again.",
        VISUAL_IMAGE_MAX_BYTES / MIB
    )]
    ImageTooLarge {
        /// The path of the file.
        path: String,
    },
    /// A local file is not an image of a type that a page may use.
    #[error(
        "The file {path} is not a PNG, JPEG, GIF, WebP or SVG image. Use only image files of those types, then try again."
    )]
    NotAnImage {
        /// The path of the file.
        path: String,
    },
    /// The page with its images inlined is too large.
    #[error(
        "The page is {bytes} bytes with its images. The limit is {} MiB. Use fewer or smaller images, then try again.",
        VISUAL_PAGE_MAX_BYTES / MIB
    )]
    PageTooLarge {
        /// The size of the page with its images.
        bytes: u64,
    },
    /// The style (the theme variables) is not allowed.
    #[error("{0}")]
    Style(#[from] VisualStyleError),
}
