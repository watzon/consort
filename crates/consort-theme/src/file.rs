//! Theme files: the serde types that mirror `design/theme.schema.json`, and
//! the checks that a theme must pass before it loads.

use std::path::PathBuf;

use serde::Deserialize;

use crate::color::Rgb;

/// One theme file: color roles for a dark mode and a light mode.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThemeFile {
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    pub name: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub default: bool,
    pub modes: Modes,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Modes {
    pub dark: ModeColors,
    pub light: ModeColors,
}

/// Light or dark: the two modes that every theme sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }

    pub fn is_dark(self) -> bool {
        self == Mode::Dark
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModeColors {
    pub surface: Surface,
    pub border: Border,
    pub text: Text,
    pub accent: Accent,
    pub status: Status,
    pub syntax: Syntax,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    pub sidebar: Rgb,
    pub canvas: Rgb,
    pub card: Rgb,
    pub raised: Rgb,
    pub selected: Rgb,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Border {
    pub subtle: Rgb,
    pub strong: Rgb,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub primary: Rgb,
    pub secondary: Rgb,
    pub tertiary: Rgb,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Accent {
    pub fill: Rgb,
    pub text: Rgb,
    pub on_fill: Rgb,
    pub surface: Rgb,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub working: Rgb,
    pub done: Rgb,
    pub failed: Rgb,
    pub scheduled: Rgb,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Syntax {
    pub keyword: Rgb,
    pub string: Rgb,
    pub number: Rgb,
    pub function: Rgb,
    #[serde(rename = "type")]
    pub type_: Rgb,
    pub tag: Rgb,
    pub attribute: Rgb,
}

/// Every role path that a theme sets, in schema order.
pub const ROLES: [&str; 25] = [
    "surface.sidebar",
    "surface.canvas",
    "surface.card",
    "surface.raised",
    "surface.selected",
    "border.subtle",
    "border.strong",
    "text.primary",
    "text.secondary",
    "text.tertiary",
    "accent.fill",
    "accent.text",
    "accent.on_fill",
    "accent.surface",
    "status.working",
    "status.done",
    "status.failed",
    "status.scheduled",
    "syntax.keyword",
    "syntax.string",
    "syntax.number",
    "syntax.function",
    "syntax.type",
    "syntax.tag",
    "syntax.attribute",
];

impl ModeColors {
    /// The color of a role path such as `surface.canvas` or `syntax.type`.
    pub fn role(&self, path: &str) -> Option<Rgb> {
        Some(match path {
            "surface.sidebar" => self.surface.sidebar,
            "surface.canvas" => self.surface.canvas,
            "surface.card" => self.surface.card,
            "surface.raised" => self.surface.raised,
            "surface.selected" => self.surface.selected,
            "border.subtle" => self.border.subtle,
            "border.strong" => self.border.strong,
            "text.primary" => self.text.primary,
            "text.secondary" => self.text.secondary,
            "text.tertiary" => self.text.tertiary,
            "accent.fill" => self.accent.fill,
            "accent.text" => self.accent.text,
            "accent.on_fill" => self.accent.on_fill,
            "accent.surface" => self.accent.surface,
            "status.working" => self.status.working,
            "status.done" => self.status.done,
            "status.failed" => self.status.failed,
            "status.scheduled" => self.status.scheduled,
            "syntax.keyword" => self.syntax.keyword,
            "syntax.string" => self.syntax.string,
            "syntax.number" => self.syntax.number,
            "syntax.function" => self.syntax.function,
            "syntax.type" => self.syntax.type_,
            "syntax.tag" => self.syntax.tag,
            "syntax.attribute" => self.syntax.attribute,
            _ => return None,
        })
    }
}

impl ThemeFile {
    pub fn mode(&self, mode: Mode) -> &ModeColors {
        match mode {
            Mode::Dark => &self.modes.dark,
            Mode::Light => &self.modes.light,
        }
    }
}

/// Why a theme did not load.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    /// The file is not JSON, or it does not match `theme.schema.json`.
    #[error("The theme file does not match the theme schema: {0}")]
    Schema(String),
    /// The name is empty.
    #[error("The theme has no name. Add a \"name\" to the file")]
    NoName,
    /// A color pair is below its minimum contrast ratio.
    #[error(
        "In {mode} mode, {foreground} on {background} has a contrast of {ratio:.2}. It must be at least {minimum:.1}"
    )]
    Contrast {
        mode: Mode,
        foreground: &'static str,
        background: &'static str,
        ratio: f64,
        minimum: f64,
    },
    /// The file cannot be read.
    #[error("Cannot read {path}: {message}")]
    Io { path: PathBuf, message: String },
}

/// Parses a theme file and checks it: the schema, then every contrast rule.
pub fn load_theme(json: &str) -> Result<ThemeFile, ThemeError> {
    let theme = parse_theme(json)?;
    check_contrast(&theme)?;
    Ok(theme)
}

/// Parses a theme file against the schema, without the contrast check.
pub fn parse_theme(json: &str) -> Result<ThemeFile, ThemeError> {
    let theme: ThemeFile =
        serde_json::from_str(json).map_err(|err| ThemeError::Schema(err.to_string()))?;
    if theme.name.trim().is_empty() {
        return Err(ThemeError::NoName);
    }
    Ok(theme)
}

/// One measured pair from the contrast rules.
#[derive(Clone, Debug, PartialEq)]
pub struct ContrastResult {
    pub mode: Mode,
    pub foreground: &'static str,
    pub background: &'static str,
    pub ratio: f64,
    pub minimum: f64,
}

impl ContrastResult {
    pub fn passes(&self) -> bool {
        self.ratio >= self.minimum
    }
}

/// The contrast rules from `docs/design-system.md`: foregrounds, backgrounds
/// and the minimum ratio.
pub const CONTRAST_RULES: &[(&[&str], &[&str], f64)] = &[
    (
        &["text.primary", "text.secondary"],
        &[
            "surface.sidebar",
            "surface.canvas",
            "surface.card",
            "surface.raised",
            "surface.selected",
            "accent.surface",
        ],
        4.5,
    ),
    (
        &["text.tertiary"],
        &["surface.sidebar", "surface.canvas", "surface.card"],
        4.5,
    ),
    (
        &[
            "accent.text",
            "status.working",
            "status.done",
            "status.failed",
            "status.scheduled",
        ],
        &["surface.sidebar", "surface.canvas", "surface.card"],
        4.5,
    ),
    (&["accent.on_fill"], &["accent.fill"], 4.5),
    (
        &[
            "syntax.keyword",
            "syntax.string",
            "syntax.number",
            "syntax.function",
            "syntax.type",
            "syntax.tag",
            "syntax.attribute",
        ],
        &["surface.card"],
        4.5,
    ),
    (
        &["accent.text"],
        &["surface.canvas", "surface.selected"],
        3.0,
    ),
];

/// Measures every rule in both modes.
pub fn contrast_report(theme: &ThemeFile) -> Vec<ContrastResult> {
    let mut results = Vec::new();
    for mode in [Mode::Dark, Mode::Light] {
        let colors = theme.mode(mode);
        for (foregrounds, backgrounds, minimum) in CONTRAST_RULES {
            for foreground in *foregrounds {
                for background in *backgrounds {
                    let (Some(fg), Some(bg)) = (colors.role(foreground), colors.role(background))
                    else {
                        continue;
                    };
                    results.push(ContrastResult {
                        mode,
                        foreground,
                        background,
                        ratio: fg.contrast(bg),
                        minimum: *minimum,
                    });
                }
            }
        }
    }
    results
}

/// Returns the first pair that fails, as an error that names the roles and
/// the measured ratio.
pub fn check_contrast(theme: &ThemeFile) -> Result<(), ThemeError> {
    match contrast_report(theme).into_iter().find(|r| !r.passes()) {
        None => Ok(()),
        Some(r) => Err(ThemeError::Contrast {
            mode: r.mode,
            foreground: r.foreground,
            background: r.background,
            ratio: r.ratio,
            minimum: r.minimum,
        }),
    }
}
