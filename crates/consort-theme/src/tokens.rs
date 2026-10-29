//! The `Tokens` view: every color role, derived color, type style, space,
//! corner, motion and layout size that Consort's own views read. View code
//! uses these and never a raw hex value or an ad hoc size.

use std::time::Duration;

use gpui_kit::{App, BoxShadow, FontWeight, Global, Hsla, Pixels, SharedString, Styled, point, px};

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
}

#[derive(Clone, Copy, Debug)]
pub struct Icons {
    pub size: Pixels,
    pub dense: Pixels,
    pub toolbar: Pixels,
}

/// How large the app sets text. The Appearance setting changes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
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
                ease_enter: s.motion.ease_enter,
                ease_exit: s.motion.ease_exit,
            },
            layout: Layout {
                sidebar_width: px(s.layout.sidebar_width),
                panel_width: px(s.layout.panel_width),
                timeline_max_width: px(s.layout.timeline_max_width),
                speaker_gutter: px(s.layout.speaker_gutter),
                min_window: (px(s.layout.min_window[0]), px(s.layout.min_window[1])),
            },
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
