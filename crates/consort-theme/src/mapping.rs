//! Derives a GPUI Kit theme from Consort color roles, with
//! `design/gpui-kit-mapping.json` and the derived-color rules.

use std::{collections::BTreeMap, sync::LazyLock};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    color::{Rgb, Rgba},
    file::{Mode, ModeColors, ThemeFile},
    structure::{Structure, structure},
};

pub(crate) const MAPPING_JSON: &str = include_str!("../../../design/gpui-kit-mapping.json");

#[derive(Debug, Deserialize)]
struct MappingFile {
    colors: BTreeMap<String, String>,
    highlight: Highlight,
}

#[derive(Debug, Deserialize)]
struct Highlight {
    syntax: BTreeMap<String, String>,
    #[serde(flatten)]
    rest: BTreeMap<String, String>,
}

static MAPPING: LazyLock<MappingFile> = LazyLock::new(|| {
    // Embedded at build time. The `mapping_parses_and_every_expression_resolves`
    // test proves that it parses, so this cannot fail at run time.
    #[allow(clippy::expect_used)]
    serde_json::from_str(MAPPING_JSON).expect("design/gpui-kit-mapping.json parses")
});

/// Keys in the mapping file that GPUI Kit 0.8.1 spells differently.
const KEY_RENAMES: &[(&str, &str)] = &[
    ("link.foreground", "link"),
    ("link.hover.foreground", "link.hover"),
    ("link.active.foreground", "link.active"),
    ("drag_border", "drag.border"),
    (
        "description_list_label.foreground",
        "description_list.label.foreground",
    ),
    ("progress_bar.background", "progress.bar.background"),
    ("slider.bar.background", "slider.background"),
];

/// GPUI Kit 0.8.1 keys that the mapping file does not list. Without a value,
/// the kit would fall back to its own palette, so Consort derives them with
/// the same expressions as the rest.
const EXTRA_KEYS: &[(&str, &str)] = &[
    ("button.background", "surface.raised"),
    ("button.foreground", "text.primary"),
    ("button.hover.background", "hover(surface.raised)"),
    ("button.active.background", "active(surface.raised)"),
    ("button.primary.background", "accent.fill"),
    ("button.primary.foreground", "accent.on_fill"),
    ("button.primary.hover.background", "hover(accent.fill)"),
    ("button.primary.active.background", "active(accent.fill)"),
    ("button.secondary.background", "surface.raised"),
    ("button.secondary.foreground", "text.primary"),
    ("button.secondary.hover.background", "hover(surface.raised)"),
    (
        "button.secondary.active.background",
        "active(surface.raised)",
    ),
    ("button.danger.background", "status.failed"),
    ("button.danger.foreground", "on(status.failed)"),
    ("button.danger.hover.background", "hover(status.failed)"),
    ("button.danger.active.background", "active(status.failed)"),
    ("button.info.background", "status.working"),
    ("button.info.foreground", "on(status.working)"),
    ("button.info.hover.background", "hover(status.working)"),
    ("button.info.active.background", "active(status.working)"),
    ("button.success.background", "status.done"),
    ("button.success.foreground", "on(status.done)"),
    ("button.success.hover.background", "hover(status.done)"),
    ("button.success.active.background", "active(status.done)"),
    ("button.warning.background", "accent.fill"),
    ("button.warning.foreground", "accent.on_fill"),
    ("button.warning.hover.background", "hover(accent.fill)"),
    ("button.warning.active.background", "active(accent.fill)"),
    ("danger.hover.background", "hover(status.failed)"),
    ("danger.active.background", "active(status.failed)"),
    ("info.hover.background", "hover(status.working)"),
    ("info.active.background", "active(status.working)"),
    ("success.hover.background", "hover(status.done)"),
    ("success.active.background", "active(status.done)"),
    ("warning.hover.background", "hover(accent.fill)"),
    ("warning.active.background", "active(accent.fill)"),
    ("description_list.label.background", "surface.card"),
    ("group_box.title.foreground", "text.secondary"),
    ("switch.thumb.background", "on(accent.text)"),
    ("table.foot.background", "surface.sidebar"),
    ("table.foot.foreground", "text.secondary"),
];

/// An expression in the mapping file could not be resolved.
#[derive(Debug, thiserror::Error)]
#[error("The mapping expression \"{0}\" is not valid")]
pub struct MappingError(pub String);

/// Resolves one mapping expression for a mode.
pub fn eval(expr: &str, colors: &ModeColors, mode: Mode) -> Result<Rgba, MappingError> {
    let s = structure();
    let expr = expr.trim();
    let bad = || MappingError(expr.to_owned());
    if expr == "transparent" {
        return Ok(Rgba::TRANSPARENT);
    }
    if expr == "scrim" {
        let alpha = if mode.is_dark() {
            s.derive.scrim_alpha.dark
        } else {
            s.derive.scrim_alpha.light
        };
        return Ok(Rgb::BLACK.with_alpha(alpha));
    }
    if let Some(rgba) = Rgba::parse(expr) {
        return Ok(rgba);
    }
    if let Some((func, args)) = expr.split_once('(') {
        let args = args.strip_suffix(')').ok_or_else(bad)?;
        let mut parts = args.splitn(2, ',').map(str::trim);
        let inner = parts.next().ok_or_else(bad)?;
        let base = eval(inner, colors, mode)?;
        let sign = if mode.is_dark() { 1.0 } else { -1.0 };
        return match func.trim() {
            "hover" => Ok(Rgba {
                rgb: base.rgb.shift_lightness(sign * s.derive.hover_delta_l),
                a: base.a,
            }),
            "active" => Ok(Rgba {
                rgb: base.rgb.shift_lightness(sign * s.derive.active_delta_l),
                a: base.a,
            }),
            "alpha" => {
                let alpha: f32 = parts.next().and_then(|a| a.parse().ok()).ok_or_else(bad)?;
                Ok(base.rgb.with_alpha(alpha))
            }
            "tint" => Ok(base.rgb.with_alpha(s.derive.status_tint_alpha)),
            "on" => Ok(on(base.rgb, colors).opaque()),
            _ => Err(bad()),
        };
    }
    colors.role(expr).map(Rgb::opaque).ok_or_else(bad)
}

/// Whichever of `surface.sidebar` and `text.primary` has the higher contrast
/// on `background`.
pub fn on(background: Rgb, colors: &ModeColors) -> Rgb {
    let dark = colors.surface.sidebar;
    let light = colors.text.primary;
    if background.contrast(dark) >= background.contrast(light) {
        dark
    } else {
        light
    }
}

/// The GPUI Kit theme name for one mode of a Consort theme.
pub fn kit_theme_name(theme: &str, mode: Mode) -> String {
    format!("{theme} {mode}")
}

/// Every GPUI Kit color key with its value, as `#RRGGBBAA`.
pub fn kit_colors(colors: &ModeColors, mode: Mode) -> Result<Map<String, Value>, MappingError> {
    let mut out = Map::new();
    for (key, expr) in &MAPPING.colors {
        let key = KEY_RENAMES
            .iter()
            .find(|(from, _)| from == key)
            .map_or(key.as_str(), |(_, to)| to);
        out.insert(key.to_owned(), json!(eval(expr, colors, mode)?.to_string()));
    }
    for (key, expr) in EXTRA_KEYS {
        out.insert(
            (*key).to_owned(),
            json!(eval(expr, colors, mode)?.to_string()),
        );
    }
    Ok(out)
}

/// The GPUI Kit highlight section: editor colors, status colors and syntax.
pub fn kit_highlight(colors: &ModeColors, mode: Mode) -> Result<Value, MappingError> {
    let mut out = Map::new();
    for (key, expr) in &MAPPING.highlight.rest {
        out.insert(key.clone(), json!(eval(expr, colors, mode)?.to_string()));
    }
    let mut syntax = Map::new();
    for (key, expr) in &MAPPING.highlight.syntax {
        let mut style = Map::new();
        style.insert("color".into(), json!(eval(expr, colors, mode)?.to_string()));
        match key.as_str() {
            "emphasis" | "comment" | "comment.doc" => {
                style.insert("font_style".into(), json!("italic"));
            }
            "emphasis.strong" | "title" => {
                style.insert("font_weight".into(), json!(600));
            }
            _ => {}
        }
        syntax.insert(key.clone(), Value::Object(style));
    }
    out.insert("syntax".into(), Value::Object(syntax));
    Ok(Value::Object(out))
}

/// One mode of a Consort theme as a GPUI Kit `ThemeConfig` JSON object.
pub fn kit_theme_json(theme: &ThemeFile, mode: Mode) -> Result<Value, MappingError> {
    let colors = theme.mode(mode);
    let s: &Structure = structure();
    Ok(json!({
        "name": kit_theme_name(&theme.name, mode),
        "mode": mode.name(),
        "is_default": theme.default,
        "font.family": s.font.sans,
        "mono_font.family": s.font.mono,
        "font.size": s.type_scale.ui.size,
        "mono_font.size": s.type_scale.mono.size,
        "radius": s.radius.md as usize,
        "radius.lg": s.radius.lg as usize,
        "shadow": true,
        "colors": Value::Object(kit_colors(colors, mode)?),
        "highlight": kit_highlight(colors, mode)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_themes;

    #[test]
    fn mapping_parses_and_every_expression_resolves() {
        let mapping: MappingFile = serde_json::from_str(MAPPING_JSON).expect("parses");
        let theme = &builtin_themes()[0];
        for mode in [Mode::Dark, Mode::Light] {
            for expr in mapping.colors.values() {
                eval(expr, theme.mode(mode), mode).expect(expr);
            }
            for expr in mapping.highlight.syntax.values() {
                eval(expr, theme.mode(mode), mode).expect(expr);
            }
        }
    }

    #[cfg(feature = "gpui")]
    #[test]
    fn mapping_covers_every_gpui_kit_key() {
        use gpui_kit::component::ThemeConfigColors;
        let kit = serde_json::to_value(ThemeConfigColors::default()).expect("serializes");
        let kit_keys: Vec<String> = kit
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        assert!(kit_keys.len() > 100, "the kit has {} keys", kit_keys.len());
        for theme in builtin_themes() {
            for mode in [Mode::Dark, Mode::Light] {
                let ours = kit_colors(theme.mode(mode), mode).expect("resolves");
                for key in &kit_keys {
                    assert!(ours.contains_key(key), "no value for GPUI Kit key {key}");
                }
                for key in ours.keys() {
                    assert!(kit_keys.contains(key), "{key} is not a GPUI Kit key");
                }
            }
        }
    }

    #[cfg(feature = "gpui")]
    #[test]
    fn kit_theme_deserializes_into_gpui_kit() {
        use gpui_kit::component::ThemeConfig;
        for theme in builtin_themes() {
            for mode in [Mode::Dark, Mode::Light] {
                let value = kit_theme_json(&theme, mode).expect("resolves");
                let config: ThemeConfig = serde_json::from_value(value).expect("kit accepts it");
                assert_eq!(config.name.as_ref(), kit_theme_name(&theme.name, mode));
                assert_eq!(
                    config.colors.background.as_deref(),
                    Some(format!("{}FF", theme.mode(mode).surface.canvas).as_str())
                );
            }
        }
    }

    #[test]
    fn the_kit_chart_keys_use_the_chart_roles() {
        let mapping: MappingFile = serde_json::from_str(MAPPING_JSON).expect("parses");
        for n in 1..=5 {
            let key = format!("chart.{n}");
            assert_eq!(mapping.colors.get(&key), Some(&key), "{key}");
        }
        for theme in builtin_themes() {
            for mode in [Mode::Dark, Mode::Light] {
                let colors = theme.mode(mode);
                let kit = kit_colors(colors, mode).expect("resolves");
                for n in 1..=5 {
                    let want = colors.role(&format!("chart.{n}")).expect("a role");
                    assert_eq!(
                        kit.get(&format!("chart.{n}")).and_then(|v| v.as_str()),
                        Some(want.opaque().to_string().as_str()),
                        "{} {mode} chart.{n}",
                        theme.name
                    );
                }
            }
        }
    }

    #[test]
    fn derived_colors_follow_the_rules() {
        let theme = &builtin_themes()[0];
        let dark = theme.mode(Mode::Dark);
        let hover = eval("hover(surface.raised)", dark, Mode::Dark).expect("ok");
        assert!(hover.rgb.luminance() > dark.surface.raised.luminance());
        let light = theme.mode(Mode::Light);
        let hover = eval("hover(surface.raised)", light, Mode::Light).expect("ok");
        assert!(hover.rgb.luminance() < light.surface.raised.luminance());
        assert_eq!(eval("scrim", dark, Mode::Dark).expect("ok").a, 140);
        assert_eq!(
            eval("tint(status.done)", dark, Mode::Dark).expect("ok").a,
            41
        );
        // on() picks the darker sidebar color on a light status fill.
        assert_eq!(
            eval("on(status.done)", dark, Mode::Dark).expect("ok").rgb,
            dark.surface.sidebar
        );
    }
}
