//! The theme variables of a page: the names, the roles behind them, and the
//! style of a theme.

use std::sync::LazyLock;

use consort_proto::model::{CssVariable, VisualMode, VisualStyle};
use consort_theme::{Mode, ModeColors};

/// The sans font of a page.
const FONT_SANS: &str = "\"Atkinson Hyperlegible Next\", system-ui, sans-serif";
/// The mono font of a page.
const FONT_MONO: &str = "\"Atkinson Hyperlegible Mono\", ui-monospace, monospace";

/// Makes [`VARIABLES`] and the table of the color roles behind the names from
/// one list. The two font variables come last and have no role.
macro_rules! variables {
    ($(($name:literal, $role:literal)),* $(,)?) => {
        /// The names of the CSS variables of a page, in the order of
        /// `docs/VISUALS-PLAN.md`: the colors, then `--font-sans` and
        /// `--font-mono`.
        pub const VARIABLES: &[&str] = &[$($name,)* "--font-sans", "--font-mono"];

        /// The color variables with the role that sets each one.
        const COLORS: &[(&str, &str)] = &[$(($name, $role)),*];
    };
}

variables! {
    ("--background", "surface.canvas"),
    ("--foreground", "text.primary"),
    ("--muted", "surface.card"),
    ("--muted-foreground", "text.secondary"),
    ("--subtle-foreground", "text.tertiary"),
    ("--card", "surface.card"),
    ("--card-foreground", "text.primary"),
    ("--popover", "surface.raised"),
    ("--popover-foreground", "text.primary"),
    ("--border", "border.subtle"),
    ("--input", "border.strong"),
    ("--primary", "accent.fill"),
    ("--primary-foreground", "accent.on_fill"),
    ("--accent", "accent.surface"),
    ("--accent-foreground", "accent.text"),
    ("--ring", "accent.text"),
    ("--destructive", "status.failed"),
    ("--success", "status.done"),
    // The name follows T3 Code. In Spruce, this role is blue.
    ("--warning", "status.working"),
    ("--info", "status.scheduled"),
    ("--chart-1", "chart.1"),
    ("--chart-2", "chart.2"),
    ("--chart-3", "chart.3"),
    ("--chart-4", "chart.4"),
    ("--chart-5", "chart.5"),
    ("--chart-6", "chart.6"),
    ("--chart-ink", "chart.ink"),
}

/// The style of a page for the colors of one mode of a theme. The colors are
/// `#RRGGBB`. The variables come in the order of [`VARIABLES`].
pub fn theme_style(colors: &ModeColors, mode: VisualMode) -> VisualStyle {
    let mut variables = Vec::with_capacity(VARIABLES.len());
    for (name, role) in COLORS {
        // Every role in the table is a role of `ModeColors`. The
        // `every_variable_has_a_value` test proves it.
        if let Some(color) = colors.role(role) {
            variables.push(CssVariable {
                name: (*name).to_owned(),
                value: color.to_string(),
            });
        }
    }
    variables.push(CssVariable {
        name: "--font-sans".to_owned(),
        value: FONT_SANS.to_owned(),
    });
    variables.push(CssVariable {
        name: "--font-mono".to_owned(),
        value: FONT_MONO.to_owned(),
    });
    VisualStyle { mode, variables }
}

/// The style of the built-in Spruce theme. The renderer uses it when a client
/// gives no style.
pub fn default_style(mode: VisualMode) -> VisualStyle {
    static STYLES: LazyLock<Option<[VisualStyle; 2]>> = LazyLock::new(|| {
        let spruce = consort_theme::builtin_themes().into_iter().next()?;
        Some([
            theme_style(spruce.mode(Mode::Dark), VisualMode::Dark),
            theme_style(spruce.mode(Mode::Light), VisualMode::Light),
        ])
    });
    let index = match mode {
        VisualMode::Dark => 0,
        VisualMode::Light => 1,
    };
    // The built-in themes are embedded and tested, so the list is never empty.
    // An empty style is still a valid style.
    STYLES
        .as_ref()
        .map(|styles| styles[index].clone())
        .unwrap_or(VisualStyle {
            mode,
            variables: Vec::new(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_theme::{ROLES, builtin_themes};

    #[test]
    fn the_variables_come_in_the_order_of_the_plan() {
        let expected = [
            "--background",
            "--foreground",
            "--muted",
            "--muted-foreground",
            "--subtle-foreground",
            "--card",
            "--card-foreground",
            "--popover",
            "--popover-foreground",
            "--border",
            "--input",
            "--primary",
            "--primary-foreground",
            "--accent",
            "--accent-foreground",
            "--ring",
            "--destructive",
            "--success",
            "--warning",
            "--info",
            "--chart-1",
            "--chart-2",
            "--chart-3",
            "--chart-4",
            "--chart-5",
            "--chart-6",
            "--chart-ink",
            "--font-sans",
            "--font-mono",
        ];
        assert_eq!(VARIABLES, expected);
        for mode in [VisualMode::Dark, VisualMode::Light] {
            let names: Vec<String> = default_style(mode)
                .variables
                .into_iter()
                .map(|v| v.name)
                .collect();
            assert_eq!(names, expected);
        }
    }

    #[test]
    fn every_variable_has_a_value_from_a_real_role() {
        for (name, role) in COLORS {
            assert!(ROLES.contains(role), "{name}: {role} is not a role");
        }
        assert_eq!(COLORS.len() + 2, VARIABLES.len());
        for theme in builtin_themes() {
            for (mode, visual) in [
                (Mode::Dark, VisualMode::Dark),
                (Mode::Light, VisualMode::Light),
            ] {
                let style = theme_style(theme.mode(mode), visual);
                assert_eq!(style.variables.len(), VARIABLES.len(), "{}", theme.name);
                assert_eq!(style.mode, visual);
            }
        }
    }

    #[test]
    fn colors_are_hex_and_follow_the_roles() {
        let themes = builtin_themes();
        let colors = themes[0].mode(Mode::Dark);
        let style = theme_style(colors, VisualMode::Dark);
        let value = |name: &str| {
            style
                .variables
                .iter()
                .find(|v| v.name == name)
                .map(|v| v.value.clone())
                .unwrap_or_default()
        };
        assert_eq!(value("--background"), "#131E1A");
        assert_eq!(value("--foreground"), "#F0EBDF");
        assert_eq!(value("--muted"), "#1B2623");
        assert_eq!(value("--card"), "#1B2623");
        assert_eq!(value("--popover"), "#222E2A");
        assert_eq!(value("--border"), "#27322E");
        assert_eq!(value("--input"), "#384440");
        assert_eq!(value("--primary"), "#E7BE62");
        assert_eq!(value("--primary-foreground"), "#2B2005");
        assert_eq!(value("--accent"), "#3E3519");
        assert_eq!(value("--accent-foreground"), "#EAC673");
        assert_eq!(value("--ring"), "#EAC673");
        assert_eq!(value("--destructive"), "#F1735A");
        assert_eq!(value("--success"), "#80C7A8");
        assert_eq!(value("--warning"), "#77B4ED");
        assert_eq!(value("--info"), "#B2A0D6");
        assert_eq!(value("--chart-1"), "#77B4ED");
        assert_eq!(value("--chart-6"), "#8E9A94");
        assert_eq!(value("--chart-ink"), "#131E1A");
        for variable in &style.variables[..VARIABLES.len() - 2] {
            let hex = variable.value.strip_prefix('#').unwrap_or("");
            assert_eq!(hex.len(), 6, "{}", variable.name);
            assert!(
                hex.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
            );
        }
    }

    #[test]
    fn the_fonts_name_the_atkinson_families_and_a_system_fallback() {
        let style = default_style(VisualMode::Dark);
        let sans = style
            .variables
            .iter()
            .find(|v| v.name == "--font-sans")
            .map(|v| v.value.as_str());
        let mono = style
            .variables
            .iter()
            .find(|v| v.name == "--font-mono")
            .map(|v| v.value.as_str());
        assert_eq!(
            sans,
            Some("\"Atkinson Hyperlegible Next\", system-ui, sans-serif")
        );
        assert_eq!(
            mono,
            Some("\"Atkinson Hyperlegible Mono\", ui-monospace, monospace")
        );
    }

    #[test]
    fn the_default_style_is_spruce_and_passes_the_style_check() {
        for mode in [VisualMode::Dark, VisualMode::Light] {
            let style = default_style(mode);
            assert_eq!(style.mode, mode);
            assert_eq!(style.check(), Ok(()));
        }
        let dark = default_style(VisualMode::Dark);
        let light = default_style(VisualMode::Light);
        assert_ne!(dark.variables, light.variables);
        assert_eq!(dark.variables[0].value, "#131E1A");
        assert_eq!(light.variables[0].value, "#F8FCF8");
        for theme in builtin_themes() {
            for (mode, visual) in [
                (Mode::Dark, VisualMode::Dark),
                (Mode::Light, VisualMode::Light),
            ] {
                assert_eq!(theme_style(theme.mode(mode), visual).check(), Ok(()));
            }
        }
    }
}
