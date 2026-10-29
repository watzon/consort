//! The `Tokens` view: every color role, derived color, type style, space,
//! corner, motion and layout size that Consort's own views read. View code
//! uses these and never a raw hex value or an ad hoc size.

use std::time::Duration;

use gpui_kit::{App, BoxShadow, FontWeight, Global, Hsla, Pixels, SharedString, Styled, point, px};
use serde::{Deserialize, Serialize};

use crate::{
    color::{Rgb, Rgba},
    file::{Mode, ModeColors, ThemeFile},
    mapping::on,
    structure::{self, TypeStyle, structure},
};

fn hsla(rgba: Rgba) -> Hsla {
    gpui_kit::Rgba {
        r: f32::from(rgba.rgb.r) / 255.0,
        g: f32::from(rgba.rgb.g) / 255.0,
        b: f32::from(rgba.rgb.b) / 255.0,
        a: rgba.alpha(),
    }
    .into()
}

fn solid(rgb: Rgb) -> Hsla {
    hsla(rgb.opaque())
}

/// Every color that Consort's views use, for the active theme and mode.
#[derive(Clone, Debug)]
pub struct Palette {
    pub surface_sidebar: Hsla,
    pub surface_canvas: Hsla,
    pub surface_card: Hsla,
    pub surface_raised: Hsla,
    pub surface_selected: Hsla,
    pub border_subtle: Hsla,
    pub border_strong: Hsla,
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub accent_fill: Hsla,
    pub accent_text: Hsla,
    pub accent_on_fill: Hsla,
    pub accent_surface: Hsla,
    pub status_working: Hsla,
    pub status_done: Hsla,
    pub status_failed: Hsla,
    pub status_scheduled: Hsla,
    pub syntax_keyword: Hsla,
    pub syntax_string: Hsla,
    pub syntax_number: Hsla,
    pub syntax_function: Hsla,
    pub syntax_type: Hsla,
    pub syntax_tag: Hsla,
    pub syntax_attribute: Hsla,

    /// Hover on a row or a ghost button over `surface.sidebar`.
    pub hover_sidebar: Hsla,
    /// Hover on a row or a ghost button over `surface.canvas`.
    pub hover_canvas: Hsla,
    /// Hover on a row over `surface.card`.
    pub hover_card: Hsla,
    pub hover_raised: Hsla,
    pub pressed_raised: Hsla,
    pub hover_selected: Hsla,
    pub hover_accent_fill: Hsla,
    pub pressed_accent_fill: Hsla,
    pub tint_working: Hsla,
    pub tint_done: Hsla,
    pub tint_failed: Hsla,
    pub tint_scheduled: Hsla,
    pub tint_accent: Hsla,
    /// Text selection: `accent.fill` at 28 percent.
    pub selection: Hsla,
    /// The scrim behind dialogs.
    pub scrim: Hsla,
    /// Text or a glyph on a `status.failed` fill.
    pub on_failed: Hsla,
    /// The color of `shadow.float`.
    pub shadow: Hsla,
    /// Transparent `surface.canvas`, the end stop of the timeline fades.
    pub canvas_clear: Hsla,
    /// Transparent `surface.sidebar`, the end stop of the panel fades.
    pub sidebar_clear: Hsla,
    /// Transparent `surface.raised`, the end stop of the fades in a list
    /// that scrolls inside a popover.
    pub raised_clear: Hsla,
}

impl Palette {
    pub fn new(colors: &ModeColors, mode: Mode) -> Palette {
        let s = structure();
        let sign = if mode.is_dark() { 1.0 } else { -1.0 };
        let hover = |c: Rgb| solid(c.shift_lightness(sign * s.derive.hover_delta_l));
        let pressed = |c: Rgb| solid(c.shift_lightness(sign * s.derive.active_delta_l));
        let tint = |c: Rgb| hsla(c.with_alpha(s.derive.status_tint_alpha));
        let scrim_alpha = if mode.is_dark() {
            s.derive.scrim_alpha.dark
        } else {
            s.derive.scrim_alpha.light
        };
        let shadow_spec = if mode.is_dark() {
            &s.shadow.float.dark
        } else {
            &s.shadow.float.light
        };
        let shadow = Rgba::parse(&shadow_spec.color).unwrap_or(Rgb::BLACK.with_alpha(0.3));
        Palette {
            surface_sidebar: solid(colors.surface.sidebar),
            surface_canvas: solid(colors.surface.canvas),
            surface_card: solid(colors.surface.card),
            surface_raised: solid(colors.surface.raised),
            surface_selected: solid(colors.surface.selected),
            border_subtle: solid(colors.border.subtle),
            border_strong: solid(colors.border.strong),
            text_primary: solid(colors.text.primary),
            text_secondary: solid(colors.text.secondary),
            text_tertiary: solid(colors.text.tertiary),
            accent_fill: solid(colors.accent.fill),
            accent_text: solid(colors.accent.text),
            accent_on_fill: solid(colors.accent.on_fill),
            accent_surface: solid(colors.accent.surface),
            status_working: solid(colors.status.working),
            status_done: solid(colors.status.done),
            status_failed: solid(colors.status.failed),
            status_scheduled: solid(colors.status.scheduled),
            syntax_keyword: solid(colors.syntax.keyword),
            syntax_string: solid(colors.syntax.string),
            syntax_number: solid(colors.syntax.number),
            syntax_function: solid(colors.syntax.function),
            syntax_type: solid(colors.syntax.type_),
            syntax_tag: solid(colors.syntax.tag),
            syntax_attribute: solid(colors.syntax.attribute),
            hover_sidebar: hover(colors.surface.sidebar),
            hover_canvas: hover(colors.surface.canvas),
            hover_card: hover(colors.surface.card),
            hover_raised: hover(colors.surface.raised),
            pressed_raised: pressed(colors.surface.raised),
            hover_selected: hover(colors.surface.selected),
            hover_accent_fill: hover(colors.accent.fill),
            pressed_accent_fill: pressed(colors.accent.fill),
            tint_working: tint(colors.status.working),
            tint_done: tint(colors.status.done),
            tint_failed: tint(colors.status.failed),
            tint_scheduled: tint(colors.status.scheduled),
            tint_accent: tint(colors.accent.fill),
            selection: hsla(colors.accent.fill.with_alpha(s.derive.selection_alpha)),
            scrim: hsla(Rgb::BLACK.with_alpha(scrim_alpha)),
            on_failed: solid(on(colors.status.failed, colors)),
            shadow: hsla(shadow),
            canvas_clear: hsla(colors.surface.canvas.with_alpha(0.0)),
            sidebar_clear: hsla(colors.surface.sidebar.with_alpha(0.0)),
            raised_clear: hsla(colors.surface.raised.with_alpha(0.0)),
        }
    }
}

/// A text style in pixels, after the text size setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Type {
    pub size: Pixels,
    pub line: Pixels,
    pub weight: FontWeight,
}

impl Type {
    fn new(style: &TypeStyle, scale: f32) -> Type {
        Type {
            size: px((style.size * scale).round()),
            line: px((style.line * scale / 2.0).round() * 2.0),
            weight: FontWeight(f32::from(style.weight)),
        }
    }
}

/// The type scale from `structure.json`.
#[derive(Clone, Copy, Debug)]
pub struct TypeScale {
    pub display: Type,
    pub heading: Type,
    pub title: Type,
    pub body: Type,
    pub ui: Type,
    pub ui_strong: Type,
    pub label: Type,
    pub caption: Type,
    pub mono: Type,
    /// Inline code is this share of the surrounding text size.
    pub mono_inline_scale: f32,
}

/// The spacing scale, on a 4 px grid.
#[derive(Clone, Copy, Debug)]
pub struct Space {
    pub xxs: Pixels,
    pub xs: Pixels,
    pub sm: Pixels,
    pub md: Pixels,
    pub lg: Pixels,
    pub xl: Pixels,
    pub xxl: Pixels,
    pub page: Pixels,
}

#[derive(Clone, Copy, Debug)]
pub struct Radius {
    pub sm: Pixels,
    pub md: Pixels,
    pub lg: Pixels,
    pub xl: Pixels,
    pub full: Pixels,
}

#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub fast: Duration,
    pub normal: Duration,
    pub slow: Duration,
    /// One turn of the spinner on a working task.
    pub spin: Duration,
    pub ease_enter: [f32; 4],
    pub ease_exit: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub sidebar_width: Pixels,
    pub panel_width: Pixels,
    pub timeline_max_width: Pixels,
    pub speaker_gutter: Pixels,
    pub min_window: (Pixels, Pixels),
    /// The size of a new window.
    pub default_window: (Pixels, Pixels),
    /// Below this timeline column width, the speaker and the time share one
    /// line above each message.
    pub wide_timeline: Pixels,
    /// How far a virtual list lays out rows beyond its edges.
    pub list_overdraw: Pixels,
    /// The usual dialog: rename, labels and the confirmations.
    pub dialog_width: Pixels,
    /// The wide dialogs: New channel and Instructions.
    pub dialog_wide: Pixels,
    /// The Add worker dialog.
    pub dialog_worker: Pixels,
    pub dialog_account: Pixels,
    pub dialog_catalog: Pixels,
    /// The channel switcher.
    pub switcher_width: Pixels,
    /// The space above the switcher.
    pub switcher_top: Pixels,
    /// The longest a channel name grows in a switcher row.
    pub switcher_name_max: Pixels,
    /// The tallest a block of error text grows: a toast, or the reason in a
    /// task thread. Its text scrolls past this.
    pub error_text_max_height: Pixels,
}

/// Opacities of dimmed parts, as shares of full color.
#[derive(Clone, Copy, Debug)]
pub struct Opacity {
    /// An option that was not chosen in an answered decision.
    pub answered_dim: f32,
    /// A button that waits for the hub to answer.
    pub busy: f32,
    /// A danger button under the pointer.
    pub danger_hover: f32,
}

/// Sizes of single components, from `structure.json`.
#[derive(Clone, Copy, Debug)]
pub struct Components {
    pub settings_label: Pixels,
    pub settings_segmented: Pixels,
    pub theme_card: Pixels,
    pub theme_row: Pixels,
    pub theme_preview_height: Pixels,
    pub worker_list: Pixels,
    pub settings_detail_max: Pixels,
    pub account_state: Pixels,
    pub account_action: Pixels,
    pub tool_version: Pixels,
    pub tool_state: Pixels,
    pub provider_logo: Pixels,
    pub agent_tile: Pixels,
    pub agent_list_max: Pixels,
    pub sign_in_tile: Pixels,
    pub sign_in_logo: Pixels,
    pub step_marker: Pixels,
    pub tag_label: Pixels,
    pub settings_select: Pixels,
    pub group_select: Pixels,
    pub workspace_control: Pixels,
    pub model_picker: Pixels,
    pub model_picker_height: Pixels,
    pub picker_rail: Pixels,
    pub picker_rail_button: Pixels,
    pub picker_rail_badge: Pixels,
    pub picker_rail_badge_text: Pixels,
    pub picker_rail_marker: Pixels,
    pub picker_rail_marker_height: Pixels,
    pub effort_menu: Pixels,
    pub effort_select: Pixels,
    pub composer_chip: Pixels,
    pub connector_tile: Pixels,
    pub connection_field: Pixels,
    pub catalog_list: Pixels,
    pub skill_results: Pixels,
    pub usage_bar: Pixels,
    pub files_list: Pixels,
    pub files_view: Pixels,
    pub input_pad_y: Pixels,
    pub button_pad_y: Pixels,
    pub worker_dot: Pixels,
    pub state_dot: Pixels,
    pub worker_row_pad_y: Pixels,
    pub switch_inset: Pixels,
    pub notes_min_height: Pixels,
    pub brand_mark: Pixels,
    pub brand_gap: Pixels,
    /// The width of the focus ring.
    pub focus_ring: Pixels,
    pub theme_preview: ThemePreview,
}

/// The drawing of a theme in its card.
#[derive(Clone, Copy, Debug)]
pub struct ThemePreview {
    pub bar: Pixels,
    pub bar_gap: Pixels,
    pub sidebar: Pixels,
    pub sidebar_item: Pixels,
    pub text_line: Pixels,
    pub title: Pixels,
    pub chip: Pixels,
    pub card: Pixels,
    pub button: Pixels,
    pub button_radius: Pixels,
}

impl Components {
    fn new(c: &structure::Components) -> Components {
        let p = &c.theme_preview;
        Components {
            settings_label: px(c.settings_label),
            settings_segmented: px(c.settings_segmented),
            theme_card: px(c.theme_card),
            theme_row: px(c.theme_row),
            theme_preview_height: px(c.theme_preview_height),
            worker_list: px(c.worker_list),
            settings_detail_max: px(c.settings_detail_max),
            account_state: px(c.account_state),
            account_action: px(c.account_action),
            tool_version: px(c.tool_version),
            tool_state: px(c.tool_state),
            provider_logo: px(c.provider_logo),
            agent_tile: px(c.agent_tile),
            agent_list_max: px(c.agent_list_max),
            sign_in_tile: px(c.sign_in_tile),
            sign_in_logo: px(c.sign_in_logo),
            step_marker: px(c.step_marker),
            tag_label: px(c.tag_label),
            settings_select: px(c.settings_select),
            group_select: px(c.group_select),
            workspace_control: px(c.workspace_control),
            model_picker: px(c.model_picker),
            model_picker_height: px(c.model_picker_height),
            picker_rail: px(c.picker_rail),
            picker_rail_button: px(c.picker_rail_button),
            picker_rail_badge: px(c.picker_rail_badge),
            picker_rail_badge_text: px(c.picker_rail_badge_text),
            picker_rail_marker: px(c.picker_rail_marker),
            picker_rail_marker_height: px(c.picker_rail_marker_height),
            effort_menu: px(c.effort_menu),
            effort_select: px(c.effort_select),
            composer_chip: px(c.composer_chip),
            connector_tile: px(c.connector_tile),
            connection_field: px(c.connection_field),
            catalog_list: px(c.catalog_list),
            skill_results: px(c.skill_results),
            usage_bar: px(c.usage_bar),
            files_list: px(c.files_list),
            files_view: px(c.files_view),
            input_pad_y: px(c.input_pad_y),
            button_pad_y: px(c.button_pad_y),
            worker_dot: px(c.worker_dot),
            state_dot: px(c.state_dot),
            worker_row_pad_y: px(c.worker_row_pad_y),
            switch_inset: px(c.switch_inset),
            notes_min_height: px(c.notes_min_height),
            brand_mark: px(c.brand_mark),
            brand_gap: px(c.brand_gap),
            focus_ring: px(c.focus_ring),
            theme_preview: ThemePreview {
                bar: px(p.bar),
                bar_gap: px(p.bar_gap),
                sidebar: px(p.sidebar),
                sidebar_item: px(p.sidebar_item),
                text_line: px(p.text_line),
                title: px(p.title),
                chip: px(p.chip),
                card: px(p.card),
                button: px(p.button),
                button_radius: px(p.button_radius),
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Icons {
    pub size: Pixels,
    pub dense: Pixels,
    pub toolbar: Pixels,
}

/// How large the app sets text. The Appearance setting changes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    Smaller,
    #[default]
    Default,
    Larger,
}

impl TextSize {
    pub fn scale(self) -> f32 {
        match self {
            TextSize::Smaller => 0.9,
            TextSize::Default => 1.0,
            TextSize::Larger => 1.12,
        }
    }
}

/// The resolved tokens for the active theme, mode and text size. A GPUI
/// global: read it with [`ActiveTokens::tokens`].
#[derive(Clone, Debug)]
pub struct Tokens {
    pub theme_name: SharedString,
    pub mode: Mode,
    pub colors: Palette,
    pub text: TypeScale,
    pub space: Space,
    pub radius: Radius,
    pub border_width: Pixels,
    /// The stroke of icons and glyphs, for rings drawn as borders.
    pub stroke: Pixels,
    pub motion: Motion,
    pub layout: Layout,
    pub opacity: Opacity,
    pub component: Components,
    pub icon: Icons,
    pub font_sans: SharedString,
    pub font_mono: SharedString,
    pub text_size: TextSize,
    shadow_float: (f32, f32, f32, f32),
}

impl Global for Tokens {}

impl Tokens {
    pub fn new(theme: &ThemeFile, mode: Mode, text_size: TextSize) -> Tokens {
        let s = structure();
        let scale = text_size.scale();
        let t = &s.type_scale;
        let shadow = if mode.is_dark() {
            &s.shadow.float.dark
        } else {
            &s.shadow.float.light
        };
        Tokens {
            theme_name: theme.name.clone().into(),
            mode,
            colors: Palette::new(theme.mode(mode), mode),
            text: TypeScale {
                display: Type::new(&t.display, scale),
                heading: Type::new(&t.heading, scale),
                title: Type::new(&t.title, scale),
                body: Type::new(&t.body, scale),
                ui: Type::new(&t.ui, scale),
                ui_strong: Type::new(&t.ui_strong, scale),
                label: Type::new(&t.label, scale),
                caption: Type::new(&t.caption, scale),
                mono: Type::new(&t.mono, scale),
                mono_inline_scale: t.mono_inline_scale,
            },
            space: space(&s.space),
            radius: Radius {
                sm: px(s.radius.sm),
                md: px(s.radius.md),
                lg: px(s.radius.lg),
                xl: px(s.radius.xl),
                full: px(s.radius.full),
            },
            border_width: px(s.border_width),
            stroke: px(s.icon.stroke),
            motion: Motion {
                fast: Duration::from_millis(s.motion.fast_ms),
                normal: Duration::from_millis(s.motion.normal_ms),
                slow: Duration::from_millis(s.motion.slow_ms),
                spin: Duration::from_millis(s.motion.spin_ms),
                ease_enter: s.motion.ease_enter,
                ease_exit: s.motion.ease_exit,
            },
            layout: Layout {
                sidebar_width: px(s.layout.sidebar_width),
                panel_width: px(s.layout.panel_width),
                timeline_max_width: px(s.layout.timeline_max_width),
                speaker_gutter: px(s.layout.speaker_gutter),
                min_window: (px(s.layout.min_window[0]), px(s.layout.min_window[1])),
                default_window: (
                    px(s.layout.default_window[0]),
                    px(s.layout.default_window[1]),
                ),
                wide_timeline: px(s.layout.wide_timeline),
                list_overdraw: px(s.layout.list_overdraw),
                dialog_width: px(s.layout.dialog_width),
                dialog_wide: px(s.layout.dialog_wide),
                dialog_worker: px(s.layout.dialog_worker),
                dialog_account: px(s.layout.dialog_account),
                dialog_catalog: px(s.layout.dialog_catalog),
                switcher_width: px(s.layout.switcher_width),
                switcher_top: px(s.layout.switcher_top),
                switcher_name_max: px(s.layout.switcher_name_max),
                error_text_max_height: px(s.layout.error_text_max_height),
            },
            opacity: Opacity {
                answered_dim: s.opacity.answered_dim,
                busy: s.opacity.busy,
                danger_hover: s.opacity.danger_hover,
            },
            component: Components::new(&s.component),
            icon: Icons {
                size: px(s.icon.size),
                dense: px(s.icon.dense_size),
                toolbar: px(s.icon.toolbar_size),
            },
            font_sans: s.font.sans.clone().into(),
            font_mono: s.font.mono.clone().into(),
            text_size,
            shadow_float: (shadow.x, shadow.y, shadow.blur, shadow.spread),
        }
    }

    pub fn is_dark(&self) -> bool {
        self.mode.is_dark()
    }

    /// The focus ring of a control that the keyboard focuses: a ring of
    /// `accent.text` just outside the control. It takes no room in the
    /// layout, so a control does not move when it gets the focus.
    pub fn focus_ring(&self) -> Vec<BoxShadow> {
        vec![BoxShadow {
            color: self.colors.accent_text,
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: self.component.focus_ring,
            inset: false,
        }]
    }

    /// `shadow.float`: menus, popovers, dialogs and toasts.
    pub fn shadow_float(&self) -> Vec<BoxShadow> {
        let (x, y, blur, spread) = self.shadow_float;
        vec![BoxShadow {
            color: self.colors.shadow,
            offset: point(px(x), px(y)),
            blur_radius: px(blur),
            spread_radius: px(spread),
            inset: false,
        }]
    }
}

fn space(s: &structure::Space) -> Space {
    Space {
        xxs: px(s.xxs),
        xs: px(s.xs),
        sm: px(s.sm),
        md: px(s.md),
        lg: px(s.lg),
        xl: px(s.xl),
        xxl: px(s.xxl),
        page: px(s.page),
    }
}

/// Read the active tokens from any GPUI context.
pub trait ActiveTokens {
    fn tokens(&self) -> &Tokens;
}

impl ActiveTokens for App {
    fn tokens(&self) -> &Tokens {
        self.global::<Tokens>()
    }
}

/// Apply a [`Type`] style to an element.
pub trait TypeExt: Styled + Sized {
    fn type_style(self, style: Type) -> Self {
        self.text_size(style.size)
            .line_height(style.line)
            .font_weight(style.weight)
    }
}

impl<T: Styled> TypeExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_themes;

    fn tokens() -> Tokens {
        let theme = builtin_themes()
            .into_iter()
            .next()
            .expect("a built-in theme");
        Tokens::new(&theme, Mode::Dark, TextSize::Default)
    }

    #[test]
    fn layout_tokens_reach_the_views() {
        let t = tokens();
        assert_eq!(t.layout.default_window, (px(1440.0), px(900.0)));
        assert_eq!(t.layout.wide_timeline, px(600.0));
        assert_eq!(t.layout.list_overdraw, px(400.0));
        assert_eq!(t.layout.dialog_width, px(528.0));
        assert_eq!(t.layout.dialog_wide, px(560.0));
        assert_eq!(t.layout.dialog_worker, px(536.0));
        assert_eq!(t.layout.dialog_account, px(600.0));
        assert_eq!(t.layout.switcher_width, px(560.0));
        assert_eq!(t.layout.switcher_top, px(96.0));
        assert_eq!(t.layout.switcher_name_max, px(240.0));
        assert_eq!(t.layout.error_text_max_height, px(240.0));
        assert_eq!(t.motion.spin, Duration::from_millis(1200));
    }

    /// Review cycle 3, finding 3: a control that the keyboard focuses shows a
    /// ring. It is the accent text color, drawn outside the control.
    #[test]
    fn the_focus_ring_is_the_accent_text_color_outside_the_control() {
        let t = tokens();
        let ring = t.focus_ring();
        assert_eq!(ring.len(), 1);
        assert_eq!(ring[0].color, t.colors.accent_text);
        assert_eq!(ring[0].spread_radius, px(2.0));
        assert_eq!(ring[0].blur_radius, px(0.0));
        assert_eq!(ring[0].offset, point(px(0.0), px(0.0)));
        assert!(!ring[0].inset);
        // Both modes give the ring a color that reads on the surface.
        for mode in [Mode::Dark, Mode::Light] {
            let theme = builtin_themes().into_iter().next().expect("a theme");
            let t = Tokens::new(&theme, mode, TextSize::Default);
            assert_ne!(t.focus_ring()[0].color, t.colors.surface_canvas);
        }
    }

    #[test]
    fn opacity_and_component_tokens_reach_the_views() {
        let t = tokens();
        assert_eq!(t.opacity.answered_dim, 0.6);
        assert_eq!(t.opacity.busy, 0.7);
        assert_eq!(t.opacity.danger_hover, 0.9);
        assert_eq!(t.component.settings_label, px(160.0));
        assert_eq!(t.component.theme_row, px(632.0));
        assert_eq!(t.component.button_pad_y, px(5.0));
        assert_eq!(t.component.notes_min_height, px(192.0));
        assert_eq!(t.component.brand_mark, px(26.0));
        assert_eq!(t.component.brand_gap, px(8.0));
        assert_eq!(t.component.focus_ring, px(2.0));
        assert_eq!(t.component.theme_preview.sidebar_item, px(20.0));
        assert_eq!(t.component.theme_preview.button_radius, px(2.0));
    }
}
