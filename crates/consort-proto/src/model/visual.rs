//! Visuals: HTML pages that the assistant shows in the timeline as items of
//! their own (CONTEXT.md). The plan is `docs/VISUALS-PLAN.md`.
//!
//! A visual is a notice with `kind: "visual"`. Its body is
//! [`VisualNotice`], and its one attachment is the prepared page
//! (`text/html`). The page has no bootstrap: each client and the renderer
//! add it when they load the page, with the [`VisualStyle`] of that moment.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::BlobRef;

/// The version of the bootstrap that every page gets (the theme style, the
/// fonts, the base sheet, the scripts and the ink kit). An image of a page
/// depends on it, so it is a part of the key of the image cache. A test in
/// `consort-visual` fails when the bootstrap changes and this stays the same.
pub const VISUAL_BOOTSTRAP_VERSION: u32 = 4;

/// The notice kind of a visual.
pub const VISUAL_NOTICE: &str = "visual";
/// The media type of the page of a visual.
pub const VISUAL_PAGE_MIME: &str = "text/html";
/// The most characters of the HTML that the assistant writes.
pub const VISUAL_HTML_MAX_CHARS: usize = 512_000;
/// The most characters of the title of a visual.
pub const VISUAL_TITLE_MAX_CHARS: usize = 200;
/// The smallest height that the assistant may give, in CSS pixels. Also the
/// smallest height of a visual in the timeline.
pub const VISUAL_HEIGHT_MIN: u32 = 80;
/// The largest height that the assistant may give, in CSS pixels.
pub const VISUAL_HEIGHT_MAX: u32 = 2_000;
/// The height of a visual when the assistant gives none and nothing was
/// measured.
pub const VISUAL_HEIGHT_DEFAULT: u32 = 360;
/// The tallest that a page can be, measured or live, in CSS pixels.
pub const VISUAL_FIT_MAX: u32 = 20_000;
/// The most bytes of one local image that the session host inlines (10 MiB).
pub const VISUAL_IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// The most bytes of the prepared page, with its images inlined (25 MiB).
pub const VISUAL_PAGE_MAX_BYTES: u64 = 25 * 1024 * 1024;
/// The widths of the height table, in CSS pixels.
pub const VISUAL_WIDTHS: [u32; 10] = [320, 375, 430, 480, 560, 640, 720, 860, 1000, 1144];
/// The narrowest width of `preview_visual`.
pub const VISUAL_PREVIEW_WIDTH_MIN: u32 = 320;
/// The widest width of `preview_visual`.
pub const VISUAL_PREVIEW_WIDTH_MAX: u32 = 1144;
/// The width of `preview_visual` when the assistant gives none: the width
/// of the timeline column.
pub const VISUAL_PREVIEW_WIDTH_DEFAULT: u32 = 720;
/// The narrowest width of `visual.render`.
pub const VISUAL_RENDER_WIDTH_MIN: u32 = 160;
/// The widest width of `visual.render`.
pub const VISUAL_RENDER_WIDTH_MAX: u32 = 2_400;
/// The largest scale of `visual.render`.
pub const VISUAL_SCALE_MAX: u8 = 3;
/// The most variables in a [`VisualStyle`].
pub const VISUAL_STYLE_MAX_VARIABLES: usize = 64;
/// The most characters of one variable value in a [`VisualStyle`].
pub const VISUAL_STYLE_VALUE_MAX_CHARS: usize = 200;
/// The most console messages that `preview_visual` returns.
pub const VISUAL_CONSOLE_MAX_MESSAGES: usize = 20;
/// The most characters of one console message.
pub const VISUAL_CONSOLE_MAX_CHARS: usize = 500;

/// Dark or light. The page gets it as `prefers-color-scheme` and as the
/// theme variables of that mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisualMode {
    /// Dark mode.
    #[default]
    Dark,
    /// Light mode.
    Light,
}

impl VisualMode {
    /// The value of `prefers-color-scheme`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// The height of the page at one width, in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VisualHeight {
    /// The width.
    pub width: u32,
    /// The height of the content at that width.
    pub height: u32,
}

/// The body of a visual notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisualNotice {
    /// Always [`VISUAL_NOTICE`].
    pub kind: String,
    /// The line that a client with no visuals shows: the title.
    pub text: String,
    /// The title.
    pub title: String,
    /// The height that the assistant gave, clamped to
    /// [`VISUAL_HEIGHT_MIN`]..=[`VISUAL_HEIGHT_MAX`]. The last fallback.
    pub height: u32,
    /// The height table, by width. Empty when the session host had no
    /// browser at publish.
    #[serde(default)]
    pub heights: Vec<VisualHeight>,
}

impl VisualNotice {
    /// The body of a new visual. `height` is the height that the assistant
    /// gave, if any. `heights` is sorted by width.
    pub fn new(title: &str, height: Option<u32>, mut heights: Vec<VisualHeight>) -> Self {
        heights.sort_by_key(|h| h.width);
        heights.dedup_by_key(|h| h.width);
        for h in &mut heights {
            h.height = h.height.clamp(1, VISUAL_FIT_MAX);
        }
        Self {
            kind: VISUAL_NOTICE.to_owned(),
            text: title.to_owned(),
            title: title.to_owned(),
            height: height
                .unwrap_or(VISUAL_HEIGHT_DEFAULT)
                .clamp(VISUAL_HEIGHT_MIN, VISUAL_HEIGHT_MAX),
            heights,
        }
    }

    /// Reads a notice body. `None` when the body is not a visual.
    pub fn parse(body: &str) -> Option<Self> {
        let notice: Self = serde_json::from_str(body).ok()?;
        (notice.kind == VISUAL_NOTICE).then_some(notice)
    }

    /// The notice body as JSON text.
    pub fn to_body(&self) -> String {
        // A struct of strings, numbers and a list always serializes.
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The height from the table at this width, by linear interpolation
    /// between the two nearest widths. Past the ends it uses the nearest
    /// width. `None` when the table is empty.
    pub fn table_height(&self, width: u32) -> Option<u32> {
        let first = self.heights.first()?;
        let last = self.heights.last()?;
        if width <= first.width {
            return Some(first.height);
        }
        if width >= last.width {
            return Some(last.height);
        }
        let pair = self
            .heights
            .windows(2)
            .find(|w| w[0].width <= width && width <= w[1].width)?;
        let (a, b) = (pair[0], pair[1]);
        let span = f64::from(b.width - a.width);
        let t = f64::from(width - a.width) / span;
        let height = f64::from(a.height) + (f64::from(b.height) - f64::from(a.height)) * t;
        Some(clamp_height(height))
    }

    /// The height to lay out at this width before the page or its image
    /// reports one: the table, then the height that the assistant gave.
    pub fn height_at(&self, width: u32) -> u32 {
        self.table_height(width).unwrap_or(self.height)
    }
}

/// Clamps a measured or live height to
/// [`VISUAL_HEIGHT_MIN`]..=[`VISUAL_FIT_MAX`], rounded up. A height that is
/// not a number gives the smallest height.
pub fn clamp_height(height: f64) -> u32 {
    if !height.is_finite() {
        return VISUAL_HEIGHT_MIN;
    }
    let ceil = height
        .ceil()
        .clamp(f64::from(VISUAL_HEIGHT_MIN), f64::from(VISUAL_FIT_MAX));
    // In range of u32 after the clamp.
    ceil as u32
}

/// One CSS custom property of a [`VisualStyle`], such as `--background`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CssVariable {
    /// The name, with the two leading dashes.
    pub name: String,
    /// The value, such as `#131E1A`.
    pub value: String,
}

/// The theme of one client for a page: the mode and the CSS variables.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VisualStyle {
    /// Dark or light.
    pub mode: VisualMode,
    /// The variables, in the order of the page contract.
    #[serde(default)]
    pub variables: Vec<CssVariable>,
}

/// Why a [`VisualStyle`] was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VisualStyleError {
    /// Too many variables.
    #[error("The style has more than {VISUAL_STYLE_MAX_VARIABLES} variables.")]
    TooMany,
    /// A name that is not `--` followed by lowercase letters, digits and
    /// dashes.
    #[error("The style variable {0:?} has a name that is not allowed.")]
    Name(String),
    /// A value that is too long or has a character that can end a CSS rule
    /// or a style element.
    #[error("The value of the style variable {0} is not allowed.")]
    Value(String),
}

impl VisualStyle {
    /// Checks the names and values, so that the style cannot end its rule
    /// or its `<style>` element.
    pub fn check(&self) -> Result<(), VisualStyleError> {
        if self.variables.len() > VISUAL_STYLE_MAX_VARIABLES {
            return Err(VisualStyleError::TooMany);
        }
        for variable in &self.variables {
            let name = variable.name.as_str();
            let body = name.strip_prefix("--").unwrap_or("");
            let name_ok = !body.is_empty()
                && body.len() <= 64
                && body
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !name_ok {
                return Err(VisualStyleError::Name(name.to_owned()));
            }
            let value = variable.value.as_str();
            let value_ok = value.chars().count() <= VISUAL_STYLE_VALUE_MAX_CHARS
                && !value
                    .chars()
                    .any(|c| matches!(c, ';' | '{' | '}' | '<' | '>' | '\\') || c.is_control());
            if !value_ok {
                return Err(VisualStyleError::Value(name.to_owned()));
            }
        }
        Ok(())
    }
}

/// An image of a visual at one width, scale and style.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisualImage {
    /// The PNG.
    pub image: BlobRef,
    /// The height of the page in CSS pixels. The PNG is this height times
    /// the scale.
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table() -> VisualNotice {
        VisualNotice::new(
            "Tokens by week",
            Some(300),
            vec![
                VisualHeight {
                    width: 720,
                    height: 400,
                },
                VisualHeight {
                    width: 320,
                    height: 800,
                },
            ],
        )
    }

    #[test]
    fn a_notice_round_trips_and_sorts_its_table() {
        let notice = table();
        assert_eq!(notice.heights[0].width, 320);
        let body = notice.to_body();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["kind"], "visual");
        assert_eq!(value["text"], "Tokens by week");
        assert_eq!(value["heights"][0], json!({"width": 320, "height": 800}));
        assert_eq!(VisualNotice::parse(&body), Some(notice));
    }

    #[test]
    fn other_notices_are_not_visuals() {
        assert_eq!(VisualNotice::parse(r#"{"kind":"draft","text":"x"}"#), None);
        assert_eq!(VisualNotice::parse("not json"), None);
        let old = VisualNotice::parse(r#"{"kind":"visual","text":"A","title":"A","height":200}"#)
            .unwrap();
        assert!(old.heights.is_empty());
    }

    #[test]
    fn the_height_given_is_clamped() {
        assert_eq!(
            VisualNotice::new("A", Some(10), vec![]).height,
            VISUAL_HEIGHT_MIN
        );
        assert_eq!(
            VisualNotice::new("A", Some(9_000), vec![]).height,
            VISUAL_HEIGHT_MAX
        );
        assert_eq!(
            VisualNotice::new("A", None, vec![]).height,
            VISUAL_HEIGHT_DEFAULT
        );
    }

    #[test]
    fn the_table_interpolates_and_holds_its_ends() {
        let notice = table();
        assert_eq!(notice.table_height(320), Some(800));
        assert_eq!(notice.table_height(200), Some(800));
        assert_eq!(notice.table_height(2_000), Some(400));
        assert_eq!(notice.table_height(520), Some(600));
        assert_eq!(notice.height_at(520), 600);
        let empty = VisualNotice::new("A", Some(300), vec![]);
        assert_eq!(empty.table_height(520), None);
        assert_eq!(empty.height_at(520), 300);
    }

    #[test]
    fn live_heights_are_clamped_and_rounded_up() {
        assert_eq!(clamp_height(12.0), VISUAL_HEIGHT_MIN);
        assert_eq!(clamp_height(200.2), 201);
        assert_eq!(clamp_height(1e9), VISUAL_FIT_MAX);
        assert_eq!(clamp_height(f64::NAN), VISUAL_HEIGHT_MIN);
    }

    #[test]
    fn a_style_cannot_end_its_rule() {
        let style = |name: &str, value: &str| VisualStyle {
            mode: VisualMode::Light,
            variables: vec![CssVariable {
                name: name.to_owned(),
                value: value.to_owned(),
            }],
        };
        assert!(style("--chart-1", "#2F74C0").check().is_ok());
        assert!(
            style(
                "--font-sans",
                "\"Atkinson Hyperlegible Next\", system-ui, sans-serif"
            )
            .check()
            .is_ok()
        );
        assert_eq!(
            style("background", "#fff").check(),
            Err(VisualStyleError::Name("background".to_owned()))
        );
        assert!(style("--Bg", "#fff").check().is_err());
        assert!(style("--bg", "red;}body{display:none").check().is_err());
        assert!(style("--bg", "</style><script>").check().is_err());
        assert!(style("--bg", &"a".repeat(201)).check().is_err());
        let many = VisualStyle {
            mode: VisualMode::Dark,
            variables: (0..65)
                .map(|i| CssVariable {
                    name: format!("--v{i}"),
                    value: "1".to_owned(),
                })
                .collect(),
        };
        assert_eq!(many.check(), Err(VisualStyleError::TooMany));
    }

    #[test]
    fn modes_are_snake_case() {
        assert_eq!(
            serde_json::to_value(VisualMode::Light).unwrap(),
            json!("light")
        );
        assert_eq!(VisualMode::default(), VisualMode::Dark);
        assert_eq!(VisualMode::Dark.as_str(), "dark");
    }
}
