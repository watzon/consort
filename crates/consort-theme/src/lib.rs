//! Consort themes: theme files, contrast checks, the GPUI Kit theme that the
//! desktop app uses, and the [`Tokens`] view for Consort's own components.
//!
//! A theme sets color roles only. Type, spacing, corners, motion and layout
//! come from `design/structure.json` and are the same in every theme. See
//! `docs/design-system.md` and ADR 0009.

pub mod color;
pub mod file;
pub mod mapping;
pub mod runtime;
pub mod structure;
pub mod tokens;

pub use color::{Rgb, Rgba};
pub use file::{
    CONTRAST_RULES, ContrastResult, Mode, ModeColors, ROLES, ThemeError, ThemeFile, check_contrast,
    contrast_report, load_theme, parse_theme,
};
pub use runtime::{
    Appearance, LoadedTheme, RejectedTheme, SavedChoice, ThemeLibrary, ThemeSource, apply,
    choice_path, choose, choose_reduce_motion, choose_without_saving, init, read_choice,
    read_user_themes, resolve_mode, start_reduce_motion, user_themes_dir, watch_user_themes,
    write_choice,
};
pub use structure::{Structure, structure};
pub use tokens::{ActiveTokens, Palette, TextSize, TintStop, Tokens, Type, TypeExt};

/// The built-in themes, parsed. Spruce comes first.
pub fn builtin_themes() -> Vec<ThemeFile> {
    runtime::builtin_theme_sources()
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
        let sources = runtime::builtin_theme_sources();
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
        let spruce = parse_theme(runtime::builtin_theme_sources()[0]).expect("parses");
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
            serde_json::from_str(runtime::builtin_theme_sources()[0]).expect("json");
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
            serde_json::from_str(runtime::builtin_theme_sources()[0]).expect("json");
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

    #[test]
    fn user_themes_load_and_bad_files_are_named() {
        let dir = std::env::temp_dir().join(format!("consort-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let mut good: serde_json::Value =
            serde_json::from_str(runtime::builtin_theme_sources()[0]).expect("json");
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
}
