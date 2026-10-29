//! `design/structure.json`: type, spacing, corners, shadows, motion, icons
//! and layout. The same for every theme.

use std::sync::LazyLock;

use serde::Deserialize;

pub(crate) const STRUCTURE_JSON: &str = include_str!("../../../design/structure.json");

#[derive(Clone, Debug, Deserialize)]
pub struct Structure {
    pub font: Fonts,
    #[serde(rename = "type")]
    pub type_scale: TypeScale,
    pub space: Space,
    pub radius: Radius,
    pub border_width: f32,
    pub shadow: Shadows,
    pub motion: Motion,
    pub icon: Icons,
    pub layout: Layout,
    pub opacity: Opacity,
    pub component: Components,
    pub derive: Derive,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Fonts {
    pub sans: String,
    pub mono: String,
    pub license: String,
}

/// One text style: size and line height in px, weight, and tracking in em.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct TypeStyle {
    pub size: f32,
    pub line: f32,
    pub weight: u16,
    pub tracking: f32,
    #[serde(default)]
    pub max_chars: Option<u16>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct TypeScale {
    pub display: TypeStyle,
    pub heading: TypeStyle,
    pub title: TypeStyle,
    pub body: TypeStyle,
    pub ui: TypeStyle,
    pub ui_strong: TypeStyle,
    pub label: TypeStyle,
    pub caption: TypeStyle,
    pub mono: TypeStyle,
    pub mono_inline_scale: f32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Space {
    pub xxs: f32,
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
    pub xxl: f32,
    pub page: f32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Radius {
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
    pub full: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Shadows {
    pub float: ShadowModes,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ShadowModes {
    pub dark: Shadow,
    pub light: Shadow,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Motion {
    pub fast_ms: u64,
    pub normal_ms: u64,
    pub slow_ms: u64,
    /// One turn of the spinner on a working task.
    pub spin_ms: u64,
    pub ease_enter: [f32; 4],
    pub ease_exit: [f32; 4],
}

#[derive(Clone, Debug, Deserialize)]
pub struct Icons {
    pub set: String,
    pub size: f32,
    pub dense_size: f32,
    pub toolbar_size: f32,
    pub stroke: f32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Layout {
    pub sidebar_width: f32,
    pub panel_width: f32,
    pub timeline_max_width: f32,
    pub speaker_gutter: f32,
    pub min_window: [f32; 2],
    /// The size of a new window, in px, before the user resizes it.
    pub default_window: [f32; 2],
    /// Below this timeline column width, the speaker and the time sit on one
    /// line above each message.
    pub wide_timeline: f32,
    /// How far, in px, a virtual list lays out rows beyond its edges.
    pub list_overdraw: f32,
    /// The usual dialog: rename, labels and the confirmations.
    pub dialog_width: f32,
    /// The wide dialogs: New channel and Instructions.
    pub dialog_wide: f32,
    /// The Add worker dialog.
    pub dialog_worker: f32,
    /// The Add account dialog: the provider grid, the name and the sign-in.
    pub dialog_account: f32,
    /// The channel switcher.
    pub switcher_width: f32,
    /// The space above the switcher.
    pub switcher_top: f32,
    /// The longest a channel name grows in a switcher row.
    pub switcher_name_max: f32,
    /// The tallest a block of error text grows: a toast, or the reason in a
    /// task thread. Its text scrolls past this.
    pub error_text_max_height: f32,
}

/// Opacities of dimmed parts, as shares of full color.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Opacity {
    /// An option that was not chosen in an answered decision.
    pub answered_dim: f32,
    /// A button that waits for the hub to answer.
    pub busy: f32,
    /// A danger button under the pointer.
    pub danger_hover: f32,
}

/// Sizes of single components that no other token covers. Each one comes
/// from the approved Paper design or from `docs/design-system.md`.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Components {
    /// The label column of a settings row.
    pub settings_label: f32,
    /// A segmented control in Settings: Mode and Text size.
    pub settings_segmented: f32,
    /// One theme card in Settings > Appearance.
    pub theme_card: f32,
    /// Two theme cards and the gap between them.
    pub theme_row: f32,
    /// The height of a theme preview.
    pub theme_preview_height: f32,
    /// The worker list in Settings > Workers.
    pub worker_list: f32,
    /// The widest a settings detail page grows.
    pub settings_detail_max: f32,
    /// The status column of an account row: a dot and a word.
    pub account_state: f32,
    /// The action column of an account row: Sign out, Sign in.
    pub account_action: f32,
    /// The version column of a tool row.
    pub tool_version: f32,
    /// The state column of a tool row: Up to date, Update to X.
    pub tool_state: f32,
    /// A provider logo in a row, a card or a field.
    pub provider_logo: f32,
    /// The tile around the icon of an ACP registry agent.
    pub agent_tile: f32,
    /// The tallest the ACP registry list grows before it scrolls: three rows.
    pub agent_list_max: f32,
    /// The tile around the provider logo in the sign-in step.
    pub sign_in_tile: f32,
    /// The provider logo in that tile.
    pub sign_in_logo: f32,
    /// The round marker of a numbered step in a sign-in flow.
    pub step_marker: f32,
    /// The label column of the Tags and Detected rows of a worker.
    pub tag_label: f32,
    /// A select in a settings row.
    pub settings_select: f32,
    /// The Group select in the New channel dialog.
    pub group_select: f32,
    /// The Workspace control in the New channel dialog.
    pub workspace_control: f32,
    /// The width of the model picker popover.
    pub model_picker: f32,
    /// The tallest the model list in the picker grows before it scrolls.
    pub model_list_max: f32,
    /// The width of the effort menu.
    pub effort_menu: f32,
    /// The effort select next to a model field in a dialog.
    pub effort_select: f32,
    /// The height of a chip in the composer, such as the model chip.
    pub composer_chip: f32,
    /// The vertical padding of a text input: 36 px with its border.
    pub input_pad_y: f32,
    /// The vertical padding of a text button: 30 px high.
    pub button_pad_y: f32,
    /// The dot of a worker in a list.
    pub worker_dot: f32,
    /// The dot of a sign-in state.
    pub state_dot: f32,
    /// The vertical padding of a worker row in the sidebar: 26 px rows.
    pub worker_row_pad_y: f32,
    /// The inset of the switch thumb.
    pub switch_inset: f32,
    /// The least height of the notes box in the Instructions dialog.
    pub notes_min_height: f32,
    /// The mark in the Consort logo on onboarding and the start-up error.
    pub brand_mark: f32,
    /// The gap between the mark and the word in the logo.
    pub brand_gap: f32,
    /// The width of the focus ring that a control shows when the keyboard
    /// focuses it. The ring is `accent.text`.
    pub focus_ring: f32,
    pub theme_preview: ThemePreview,
}

/// The drawing of a theme in its card: a miniature sidebar, two lines of
/// text and a card with two buttons.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ThemePreview {
    pub bar: f32,
    pub bar_gap: f32,
    pub sidebar: f32,
    pub sidebar_item: f32,
    pub text_line: f32,
    pub title: f32,
    pub chip: f32,
    pub card: f32,
    pub button: f32,
    pub button_radius: f32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Derive {
    pub hover_delta_l: f64,
    pub active_delta_l: f64,
    pub selection_alpha: f32,
    pub status_tint_alpha: f32,
    pub scrim_alpha: ScrimAlpha,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ScrimAlpha {
    pub dark: f32,
    pub light: f32,
}

static STRUCTURE: LazyLock<Structure> = LazyLock::new(|| {
    // The file is embedded at build time, and the `structure_parses` test
    // proves that it parses, so this cannot fail at run time.
    #[allow(clippy::expect_used)]
    serde_json::from_str(STRUCTURE_JSON).expect("design/structure.json parses")
});

/// The parsed `design/structure.json`.
pub fn structure() -> &'static Structure {
    &STRUCTURE
}

#[cfg(test)]
mod tests {
    #[test]
    fn structure_parses() {
        let s: super::Structure = serde_json::from_str(super::STRUCTURE_JSON).expect("parses");
        assert_eq!(s.layout.sidebar_width, 248.0);
        assert_eq!(s.type_scale.body.size, 15.0);
        assert_eq!(s.space.page, 48.0);
    }

    #[test]
    fn layout_tokens_for_the_window_and_the_lists_are_present() {
        let s = super::structure();
        assert_eq!(s.layout.default_window, [1440.0, 900.0]);
        assert_eq!(s.layout.wide_timeline, 600.0);
        assert_eq!(s.layout.list_overdraw, 400.0);
        let [min_w, min_h] = s.layout.min_window;
        let [default_w, default_h] = s.layout.default_window;
        assert!(default_w >= min_w && default_h >= min_h);
    }

    /// Review cycle 2, finding 14: view code had ratios such as
    /// `panel_width * 1.4`. Each one is a token now, with the value it had.
    #[test]
    fn the_sizes_that_were_ratios_in_view_code_are_tokens() {
        let s = super::structure();
        let p = s.layout.panel_width;
        assert_eq!(s.layout.dialog_width, p + s.space.xxl * 4.0);
        assert!((s.layout.dialog_wide - p * 1.4).abs() < 0.01);
        assert_eq!(s.layout.dialog_wide, p + s.space.xxl * 5.0);
        assert_eq!(s.layout.dialog_worker, s.layout.dialog_width + s.space.sm);
        assert_eq!(s.layout.switcher_width, s.layout.dialog_wide);
        assert_eq!(s.layout.switcher_top, s.space.page * 2.0);
        assert!((s.layout.switcher_name_max - p * 0.6).abs() < 0.01);
        assert_eq!(s.component.notes_min_height, s.space.page * 4.0);
        assert_eq!(s.motion.spin_ms, 1200);
        // The logo mark is about 1.2 times and its gap about a third of the
        // heading size, in whole pixels.
        assert_eq!(s.component.brand_mark, 26.0);
        assert_eq!(s.component.brand_gap, 8.0);
        assert!((s.component.brand_mark / s.type_scale.heading.size - 1.2).abs() < 0.05);
        assert!(s.layout.error_text_max_height > s.type_scale.ui.line * 3.0);
    }

    #[test]
    fn opacity_tokens_are_shares_between_zero_and_one() {
        let o = super::structure().opacity;
        assert_eq!(o.answered_dim, 0.6);
        assert_eq!(o.busy, 0.7);
        assert_eq!(o.danger_hover, 0.9);
        for share in [o.answered_dim, o.busy, o.danger_hover] {
            assert!(share > 0.0 && share < 1.0);
        }
    }

    #[test]
    fn component_tokens_keep_the_values_of_the_approved_design() {
        let c = super::structure().component;
        assert_eq!(c.settings_label, 160.0);
        assert_eq!(c.settings_segmented, 336.0);
        assert_eq!(c.theme_card, 310.0);
        // Two cards and the 12 px gap between them.
        assert_eq!(c.theme_row, c.theme_card * 2.0 + 12.0);
        assert_eq!(c.theme_preview_height, 92.0);
        assert_eq!(c.worker_list, 288.0);
        assert_eq!(c.settings_detail_max, 760.0);
        assert_eq!(c.account_state, 112.0);
        assert_eq!(
            (c.account_action, c.tool_version, c.tool_state),
            (96.0, 112.0, 150.0)
        );
        assert_eq!(
            (c.provider_logo, c.agent_tile, c.agent_list_max),
            (18.0, 32.0, 152.0)
        );
        assert_eq!(
            (c.sign_in_tile, c.sign_in_logo, c.step_marker),
            (40.0, 22.0, 22.0)
        );
        assert_eq!(c.tag_label, 92.0);
        assert_eq!(c.settings_select, 160.0);
        assert_eq!(c.group_select, 200.0);
        assert_eq!(c.workspace_control, 230.0);
        assert_eq!(
            (c.model_picker, c.model_list_max, c.effort_menu),
            (400.0, 340.0, 240.0)
        );
        assert_eq!((c.effort_select, c.composer_chip), (150.0, 28.0));
        // A 20 px line, the padding and a 1 px border make 36 px and 30 px.
        assert_eq!(c.input_pad_y * 2.0 + 20.0 + 2.0, 36.0);
        assert_eq!(c.button_pad_y * 2.0 + 20.0, 30.0);
        assert_eq!(c.worker_dot, 7.0);
        assert_eq!(c.state_dot, 6.0);
        assert_eq!(c.worker_row_pad_y * 2.0 + 20.0, 26.0);
        assert_eq!(c.switch_inset, 3.0);
        assert_eq!(c.focus_ring, 2.0);
        let p = c.theme_preview;
        assert_eq!(
            (p.bar, p.bar_gap, p.sidebar, p.sidebar_item),
            (4.0, 5.0, 32.0, 20.0)
        );
        assert_eq!(
            (
                p.text_line,
                p.title,
                p.chip,
                p.card,
                p.button,
                p.button_radius
            ),
            (76.0, 48.0, 6.0, 28.0, 22.0, 2.0)
        );
    }
}
