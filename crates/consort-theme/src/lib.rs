//! Consort themes: theme files, contrast checks, the fonts, the GPUI Kit
//! theme that the desktop app uses, and the [`Tokens`] view for Consort's own
//! components.
//!
//! A theme sets color roles only. Type, spacing, corners, motion and layout
//! come from `design/structure.json` and are the same in every theme. See
//! `docs/design-system.md` and ADR 0009.
//!
//! GPUI is behind the `gpui` feature, which is on by default. Without it, the
//! crate has the theme files, the roles, the contrast checks, the structure
//! and the fonts. The modules `runtime` and `tokens` need the feature.

pub mod builtin;
pub mod color;
pub mod file;
pub mod fonts;
pub mod mapping;
#[cfg(feature = "gpui")]
pub mod runtime;
pub mod structure;
#[cfg(feature = "gpui")]
pub mod tokens;

pub use builtin::builtin_theme_sources;
pub use color::{Rgb, Rgba};
pub use file::{
    CONTRAST_RULES, Chart, ContrastResult, Mode, ModeColors, ROLES, ThemeError, ThemeFile,
    check_contrast, contrast_report, load_theme, parse_theme,
};
#[cfg(feature = "gpui")]
pub use runtime::{
    Appearance, LoadedTheme, RejectedTheme, SavedChoice, ThemeLibrary, ThemeSource, apply,
    choice_path, choose, choose_reduce_motion, choose_without_saving, init, read_choice,
    read_user_themes, resolve_mode, start_reduce_motion, user_themes_dir, watch_user_themes,
    write_choice,
};
pub use structure::{Structure, structure};
#[cfg(feature = "gpui")]
pub use tokens::{ActiveTokens, Palette, TextSize, TintStop, Tokens, Type, TypeExt};

/// The built-in themes, parsed. Spruce comes first.
pub fn builtin_themes() -> Vec<ThemeFile> {
    builtin_theme_sources()
        .into_iter()
        // The built-in files are embedded at build time, and the
        // `builtin_themes_pass_every_rule` test proves that they load.
        .filter_map(|json| load_theme(json).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_pass_every_rule_in_both_modes() {
        let sources = builtin_theme_sources();
        assert_eq!(builtin_themes().len(), sources.len());
        for json in sources {
            let theme = parse_theme(json).expect("parses");
            let report = contrast_report(&theme);
            assert!(!report.is_empty());
            for result in &report {
                assert!(
                    result.passes(),
                    "{} {}: {} on {} is {:.2}, below {}",
                    theme.name,
                    result.mode,
                    result.foreground,
                    result.background,
                    result.ratio,
                    result.minimum
                );
            }
            assert!(report.iter().any(|r| r.mode == Mode::Dark));
            assert!(report.iter().any(|r| r.mode == Mode::Light));
        }
    }

    #[test]
    fn the_lowest_text_ratio_is_spruce_light_as_documented() {
        let spruce = parse_theme(builtin_theme_sources()[0]).expect("parses");
        let lowest = contrast_report(&spruce)
            .into_iter()
            .filter(|r| r.minimum == 4.5)
            .min_by(|a, b| a.ratio.total_cmp(&b.ratio))
            .expect("has results");
        assert_eq!(lowest.mode, Mode::Light);
        assert!((lowest.ratio - 4.51).abs() < 0.01, "{:?}", lowest);
    }

    #[test]
    fn a_failing_pair_is_rejected_with_its_role_and_ratio() {
        let mut theme: serde_json::Value =
            serde_json::from_str(builtin_theme_sources()[0]).expect("json");
        // Tertiary text almost the color of the dark sidebar.
        theme["modes"]["dark"]["text"]["tertiary"] = "#1A2420".into();
        let err = load_theme(&theme.to_string()).expect_err("must fail");
        match err {
            ThemeError::Contrast {
                mode,
                foreground,
                background,
                ratio,
                minimum,
            } => {
                assert_eq!(mode, Mode::Dark);
                assert_eq!(foreground, "text.tertiary");
                assert_eq!(background, "surface.sidebar");
                let expected = Rgb::from_u32(0x1A2420).contrast(Rgb::from_u32(0x0F1815));
                assert!((ratio - expected).abs() < 1e-9);
                assert_eq!(minimum, 4.5);
            }
            other => panic!("wrong error: {other}"),
        }
        let message = load_theme(&theme.to_string())
            .expect_err("must fail")
            .to_string();
        assert!(
            message.contains("text.tertiary on surface.sidebar"),
            "{message}"
        );
        assert!(message.contains("1.1"), "{message}");
    }

    #[test]
    fn schema_errors_are_reported() {
        assert!(matches!(
            load_theme("{\"name\": \"x\"}"),
            Err(ThemeError::Schema(_))
        ));
        let mut theme: serde_json::Value =
            serde_json::from_str(builtin_theme_sources()[0]).expect("json");
        theme["modes"]["dark"]["surface"]["canvas"] = "red".into();
        let err = load_theme(&theme.to_string()).expect_err("bad color");
        assert!(err.to_string().contains("#RRGGBB"), "{err}");
        theme["modes"]["dark"]["surface"]["canvas"] = "#131E1A".into();
        theme["modes"]["dark"]["surface"]["extra"] = "#131E1A".into();
        assert!(matches!(
            load_theme(&theme.to_string()),
            Err(ThemeError::Schema(_))
        ));
    }

    #[cfg(feature = "gpui")]
    #[test]
    fn user_themes_load_and_bad_files_are_named() {
        let dir = std::env::temp_dir().join(format!("consort-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let mut good: serde_json::Value =
            serde_json::from_str(builtin_theme_sources()[0]).expect("json");
        good["name"] = "Moss".into();
        good["default"] = false.into();
        std::fs::write(dir.join("moss.json"), good.to_string()).expect("write");
        std::fs::write(dir.join("broken.json"), "{").expect("write");
        std::fs::write(dir.join("notes.txt"), "ignored").expect("write");
        let (loaded, rejected) = read_user_themes(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].file.name, "Moss");
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].file_name, "broken.json");
        std::fs::remove_dir_all(&dir).ok();
    }
    fn spruce_json() -> serde_json::Value {
        serde_json::from_str(builtin_theme_sources()[0]).expect("json")
    }

    #[test]
    fn every_role_path_resolves_and_the_count_is_documented() {
        assert_eq!(ROLES.len(), 32);
        for theme in builtin_themes() {
            for mode in [Mode::Dark, Mode::Light] {
                for path in ROLES {
                    assert!(
                        theme.mode(mode).role(path).is_some(),
                        "{} {mode}: no role {path}",
                        theme.name
                    );
                }
            }
        }
        assert!(
            builtin_themes()[0]
                .mode(Mode::Dark)
                .role("chart.7")
                .is_none()
        );
    }

    #[test]
    fn spruce_has_chart_roles() {
        let themes = builtin_themes();
        let spruce = &themes[0];
        let dark = spruce.mode(Mode::Dark);
        assert_eq!(dark.role("chart.1"), Rgb::parse("#77B4ED"));
        assert_eq!(dark.role("chart.3"), Rgb::parse("#E7BE62"));
        assert_eq!(dark.role("chart.6"), Rgb::parse("#8E9A94"));
        assert_eq!(dark.role("chart.ink"), Rgb::parse("#131E1A"));
        let light = spruce.mode(Mode::Light);
        assert_eq!(light.role("chart.1"), Rgb::parse("#2F74C0"));
        assert_eq!(light.role("chart.6"), Rgb::parse("#7A8781"));
        assert_eq!(light.role("chart.ink"), Rgb::parse("#FFFFFF"));
    }

    #[test]
    fn velvet_shares_the_pigments_and_has_its_own_slate_and_ink() {
        let themes = builtin_themes();
        let (spruce, velvet) = (&themes[0], &themes[1]);
        assert_eq!(velvet.name, "Velvet");
        for mode in [Mode::Dark, Mode::Light] {
            for path in ["chart.1", "chart.2", "chart.3", "chart.4", "chart.5"] {
                assert_eq!(
                    velvet.mode(mode).role(path),
                    spruce.mode(mode).role(path),
                    "{mode} {path}"
                );
            }
            assert_ne!(
                velvet.mode(mode).role("chart.6"),
                spruce.mode(mode).role("chart.6")
            );
        }
        assert_eq!(
            velvet.mode(Mode::Dark).role("chart.6"),
            Rgb::parse("#968E97")
        );
        assert_eq!(
            velvet.mode(Mode::Light).role("chart.6"),
            Rgb::parse("#857B85")
        );
        assert_eq!(
            velvet.mode(Mode::Dark).role("chart.ink"),
            Rgb::parse("#1F181F")
        );
    }

    #[test]
    fn the_chart_rules_are_checked_for_both_themes_in_both_modes() {
        for theme in builtin_themes() {
            let report = contrast_report(&theme);
            for mode in [Mode::Dark, Mode::Light] {
                let pigments = report
                    .iter()
                    .filter(|r| {
                        r.mode == mode
                            && r.background == "surface.canvas"
                            && r.foreground.starts_with("chart.")
                    })
                    .count();
                assert_eq!(pigments, 6, "{} {mode}", theme.name);
                let inks = report
                    .iter()
                    .filter(|r| {
                        r.mode == mode
                            && r.foreground == "chart.ink"
                            && r.background.starts_with("chart.")
                    })
                    .count();
                assert_eq!(inks, 6, "{} {mode}", theme.name);
            }
            assert!(
                report
                    .iter()
                    .filter(|r| r.foreground.starts_with("chart."))
                    .all(|r| r.minimum == 3.0 && r.passes())
            );
        }
    }

    #[test]
    fn a_pigment_that_is_too_faint_on_the_canvas_is_rejected() {
        let mut theme = spruce_json();
        // Nearly the color of the light canvas.
        theme["modes"]["light"]["chart"]["3"] = "#F0F4F0".into();
        match load_theme(&theme.to_string()).expect_err("must fail") {
            ThemeError::Contrast {
                mode,
                foreground,
                background,
                minimum,
                ..
            } => {
                assert_eq!(mode, Mode::Light);
                assert_eq!(foreground, "chart.3");
                assert_eq!(background, "surface.canvas");
                assert_eq!(minimum, 3.0);
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn chart_ink_must_be_readable_on_every_pigment() {
        let mut theme = spruce_json();
        // Ink with the color of the first pigment has a ratio of 1.0 on it.
        // The pigment rules pass, so the ink rule must be the one that fails.
        theme["modes"]["dark"]["chart"]["ink"] = "#77B4ED".into();
        match load_theme(&theme.to_string()).expect_err("must fail") {
            ThemeError::Contrast {
                mode, foreground, ..
            } => {
                assert_eq!(mode, Mode::Dark);
                assert_eq!(foreground, "chart.ink");
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn a_user_theme_with_no_chart_group_gets_the_chart_of_spruce_in_each_mode() {
        let mut theme = spruce_json();
        theme["name"] = "Moss".into();
        theme["default"] = false.into();
        for mode in ["dark", "light"] {
            theme["modes"][mode]
                .as_object_mut()
                .expect("a mode")
                .remove("chart");
            // Prove that the chart really left the file.
            assert!(theme["modes"][mode].get("chart").is_none());
        }
        let loaded = load_theme(&theme.to_string()).expect("loads with no chart group");
        let themes = builtin_themes();
        for mode in [Mode::Dark, Mode::Light] {
            for path in [
                "chart.1",
                "chart.2",
                "chart.3",
                "chart.4",
                "chart.5",
                "chart.6",
                "chart.ink",
            ] {
                assert_eq!(
                    loaded.mode(mode).role(path),
                    themes[0].mode(mode).role(path),
                    "{mode} {path}"
                );
            }
        }
        // The two modes get different values.
        assert_ne!(
            loaded.mode(Mode::Dark).role("chart.1"),
            loaded.mode(Mode::Light).role("chart.1")
        );
    }

    #[test]
    fn a_user_theme_may_set_the_chart_group_of_one_mode_only() {
        let mut theme = spruce_json();
        theme["modes"]["light"]
            .as_object_mut()
            .expect("a mode")
            .remove("chart");
        theme["modes"]["dark"]["chart"]["1"] = "#A0C8F0".into();
        let loaded = load_theme(&theme.to_string()).expect("loads");
        assert_eq!(
            loaded.mode(Mode::Dark).role("chart.1"),
            Rgb::parse("#A0C8F0")
        );
        assert_eq!(
            loaded.mode(Mode::Light).role("chart.1"),
            builtin_themes()[0].mode(Mode::Light).role("chart.1")
        );
    }

    #[test]
    fn a_partial_or_unknown_chart_group_is_a_schema_error() {
        let mut theme = spruce_json();
        theme["modes"]["dark"]["chart"]
            .as_object_mut()
            .expect("a chart")
            .remove("ink");
        assert!(matches!(
            load_theme(&theme.to_string()),
            Err(ThemeError::Schema(_))
        ));
        let mut theme = spruce_json();
        theme["modes"]["dark"]["chart"]["7"] = "#000000".into();
        assert!(matches!(
            load_theme(&theme.to_string()),
            Err(ThemeError::Schema(_))
        ));
    }

    #[test]
    fn the_schema_file_lists_the_chart_group_and_makes_it_optional() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../design/theme.schema.json")).expect("json");
        let mode = &schema["$defs"]["mode"];
        let required: Vec<&str> = mode["required"]
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(!required.contains(&"chart"));
        let chart = &mode["properties"]["chart"];
        let keys: Vec<&str> = chart["required"]
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(keys, ["1", "2", "3", "4", "5", "6", "ink"]);
        assert_eq!(chart["additionalProperties"], false);
    }
}
