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
    /// The black behind the light box.
    pub lightbox: Hsla,
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
            lightbox: hsla(Rgb::BLACK.with_alpha(s.derive.lightbox_alpha)),
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
    /// One frame of stop motion (8 frames a second).
    pub frame: Duration,
    /// How long the mark holds Done.
    pub done_hold: Duration,
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
    pub bubble_max_width: Pixels,
    /// The space above the first item of a chat at rest, below the mark
    /// and the activity pill.
    pub content_top: Pixels,
    /// The stops of the top tint, from the top down. See
    /// [`Layout::top_tint_height`].
    pub top_tint: [TintStop; 4],
    pub fade_bottom: Pixels,
}

/// One stop of the top tint: a distance from the top, and the opacity of
/// the canvas color there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TintStop {
    pub at: Pixels,
    pub alpha: f32,
}

impl Layout {
    /// The height of the top tint: the place of its last stop, where it is
    /// clear.
    pub fn top_tint_height(&self) -> Pixels {
        self.top_tint[self.top_tint.len() - 1].at
    }
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
    pub failed_border: f32,
    pub progress_track: f32,
    pub drop_fill: f32,
    pub nav_disabled: f32,
    pub you_tile: f32,
    /// A button that needs the hub, while the hub is offline.
    pub offline_dim: f32,
    /// The Three voices mark while the hub is offline.
    pub offline_mark: f32,
    /// The bubble of a message that waits in the outbox.
    pub queued: f32,
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
    pub switch_inset: Pixels,
    pub notes_min_height: Pixels,
    pub brand_mark: Pixels,
    pub brand_gap: Pixels,
    /// The width of the focus ring.
    pub focus_ring: Pixels,
    pub admin: Admin,
    pub files: Files,
    pub knowledge: Knowledge,
    pub sidebar: Sidebar,
    pub update_notice: UpdateNotice,
    pub chat: Chat,
    pub proactivity: Proactivity,
    pub voice: Voice,
    pub theme_preview: ThemePreview,
    pub convo: Convo,
}

/// The sizes of phase 4. See `structure::Convo`.
#[derive(Clone, Copy, Debug)]
pub struct Convo {
    pub reaction_chip: Pixels,
    pub reaction_pad_x: Pixels,
    pub reaction_gap: Pixels,
    pub quick_emoji: Pixels,
    pub menu_width: Pixels,
    pub emoji_picker_width: Pixels,
    pub emoji_cell: Pixels,
    pub emoji_picker_height: Pixels,
    pub panel_wide: Pixels,
    pub line_number_col: Pixels,
    pub search_width: Pixels,
    pub search_row: Pixels,
    pub search_results_max: Pixels,
    pub skeleton_dot: Pixels,
    pub skeleton_bar: Pixels,
    pub skeleton_title: Pixels,
    pub skeleton_line: Pixels,
    pub dialog_settings: Pixels,
    pub settings_column_gap: Pixels,
    pub dialog_small: Pixels,
    pub dialog_group: Pixels,
    pub dialog_about: Pixels,
    pub about_mark: Pixels,
    pub icon_choice: Pixels,
    pub brief_box: Pixels,
    pub instructions_box: Pixels,
    pub root_ring: Pixels,
    pub drag_line: Pixels,
    pub reaction_emoji: Type,
    pub quick_emoji_text: Type,
    pub emoji_cell_text: Type,
    pub wordmark: Type,
}

/// The sizes of the voice screens. See `structure::Voice`.
#[derive(Clone, Copy, Debug)]
pub struct Voice {
    pub remove_slot: Pixels,
    pub rule_dot: Pixels,
    pub source_tile: Pixels,
    pub check_box: Pixels,
    pub check_icon: Pixels,
    pub radio_dot: Pixels,
    pub sample_dot: Pixels,
    pub flag_bar: Pixels,
    pub field_label: Pixels,
    pub subject_box: Pixels,
    pub body_box: Pixels,
    pub line_icon: Pixels,
    pub rule_pad_y: Pixels,
    pub example_pad_y: Pixels,
}

/// The sizes of the proactivity screens. See `structure::Proactivity`.
#[derive(Clone, Copy, Debug)]
pub struct Proactivity {
    pub level_select: Pixels,
    pub time_box: Pixels,
    pub zone_select: Pixels,
    pub filter_select: Pixels,
    pub log_time: Pixels,
    pub log_place: Pixels,
    pub log_result: Pixels,
    pub log_pad_y: Pixels,
    pub count_bar: Pixels,
    pub mark_icon: Pixels,
    pub mark_gap: Pixels,
    pub mark_indent: Pixels,
    pub reason_indent: Pixels,
    pub chip_pad_x: Pixels,
    pub chip_pad_y: Pixels,
    pub chip_gap: Pixels,
    pub feedback_pad_x: Pixels,
    pub feedback_pad_y: Pixels,
    pub dab: Pixels,
    pub dab_gap: Pixels,
    pub dab_rise: Pixels,
    pub dab_drop: Pixels,
    pub writing_pad_x: Pixels,
    pub writing_pad_y: Pixels,
    pub offer_pad_top: Pixels,
    pub offer_pad: Pixels,
    pub brief_group_gap: Pixels,
}

/// The sizes of the Hub, Clients and Notifications pages. See
/// `structure::Admin`.
#[derive(Clone, Copy, Debug)]
pub struct Admin {
    pub storage_bar: Pixels,
    pub legend_swatch: Pixels,
    pub swatch_radius: Pixels,
    pub legend_gap: Pixels,
    pub status_col: Pixels,
    pub action_col: Pixels,
    pub client_tile: Pixels,
    pub client_icon: Pixels,
    pub row_gap: Pixels,
    pub confirm_indent: Pixels,
    pub code_text: Type,
    pub code_pad_x: Pixels,
    pub qr: Pixels,
    pub qr_pad: Pixels,
    pub countdown_bar: Pixels,
    pub time_select: Pixels,
    pub copy_button: Pixels,
    pub copy_small: Pixels,
    pub dock_tile: Pixels,
    pub dock_mark: Pixels,
    pub dock_badge: Pixels,
    pub dock_badge_offset: Pixels,
    pub dock_badge_text: Type,
    pub banner_width: Pixels,
    pub banner_tile: Pixels,
    pub banner_tile_radius: Pixels,
    pub banner_mark: Pixels,
    pub banner_gap: Pixels,
    pub banner_title: Type,
    pub banner_text: Type,
}

/// The sizes of routines and memory. See `structure::Knowledge`.
#[derive(Clone, Copy, Debug)]
pub struct Knowledge {
    pub switch_w: Pixels,
    pub switch_h: Pixels,
    pub switch_thumb: Pixels,
    pub status_dot: Pixels,
    pub row_pad_y: Pixels,
    pub summary_label: Pixels,
    pub run_duration: Pixels,
    pub run_link: Pixels,
    pub channel_field: Pixels,
    pub time_field: Pixels,
    pub every_field: Pixels,
    pub unit_field: Pixels,
    pub day_button: Pixels,
    pub zone_popover: Pixels,
    pub zone_list: Pixels,
    pub calendar_cell: Pixels,
    pub tree_indent: Pixels,
    pub change_bar: Pixels,
    pub editor_min_height: Pixels,
    pub version_list: Pixels,
    pub line_icon: Pixels,
}

/// The sizes of attachments and channel files. See `structure::Files`.
#[derive(Clone, Copy, Debug)]
pub struct Files {
    pub chip_image: Pixels,
    pub chip_file: Pixels,
    pub chip_pad: Pixels,
    pub chip_gap: Pixels,
    pub chip_tile: Pixels,
    pub chip_badge: Pixels,
    pub chip_close: Pixels,
    pub chip_close_icon: Pixels,
    pub progress: Pixels,
    pub row_progress: Pixels,
    pub image_tile_w: Pixels,
    pub image_tile_h: Pixels,
    pub file_bubble_you: Pixels,
    pub file_bubble: Pixels,
    pub bubble_pad_y: Pixels,
    pub bubble_pad_left: Pixels,
    pub bubble_action: Pixels,
    pub drop_icon_tile: Pixels,
    pub drop_icon: Pixels,
    pub lightbox_bar: Pixels,
    pub lightbox_nav: Pixels,
    pub lightbox_image_w: Pixels,
    pub lightbox_image_h: Pixels,
    pub row_pad_y: Pixels,
    pub size_col: Pixels,
    pub date_col: Pixels,
    pub more: Pixels,
    pub artifact_indent: Pixels,
}

/// The sizes of the sidebar and the settings rail. See
/// `structure::Sidebar`.
#[derive(Clone, Copy, Debug)]
pub struct Sidebar {
    pub inset: Pixels,
    pub row_pad_y: Pixels,
    pub row_pad_x: Pixels,
    pub row_gap: Pixels,
    pub row_icon: Pixels,
    pub row_text: Type,
    pub label_text: Type,
    pub label_pad_y: Pixels,
    pub group_top: Pixels,
    pub title_bar: Pixels,
    pub worker_text: Type,
    pub worker_pad_y: Pixels,
    pub avatar: Pixels,
    pub account_pad_top: Pixels,
    pub account_pad_bottom: Pixels,
}

/// The sizes of the update notice. See `structure::UpdateNotice`.
#[derive(Clone, Copy, Debug)]
pub struct UpdateNotice {
    pub pad_y: Pixels,
    pub pad_left: Pixels,
    pub pad_right: Pixels,
    pub gap: Pixels,
    pub margin_bottom: Pixels,
    pub action_pad_x: Pixels,
    pub action_pad_y: Pixels,
}

/// The sizes of the chat. See `structure::Chat`.
#[derive(Clone, Copy, Debug)]
pub struct Chat {
    pub bubble_radius: Pixels,
    pub bubble_tail: Pixels,
    pub bubble_pad_x: Pixels,
    pub bubble_pad_y: Pixels,
    pub composer_radius: Pixels,
    pub send_button: Pixels,
    pub top_action: Pixels,
    pub top_action_icon: Pixels,
    pub task_connector: Pixels,
    pub mark: Pixels,
    pub mark_small: Pixels,
    pub mark_top: Pixels,
    pub pill_pad_x: Pixels,
    pub pill_pad_y: Pixels,
    pub brief_number: Pixels,
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
    fn new(c: &structure::Components, scale: f32) -> Components {
        let p = &c.theme_preview;
        let sb = &c.sidebar;
        let f = &c.files;
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
            switch_inset: px(c.switch_inset),
            notes_min_height: px(c.notes_min_height),
            brand_mark: px(c.brand_mark),
            brand_gap: px(c.brand_gap),
            focus_ring: px(c.focus_ring),
            admin: {
                let a = &c.admin;
                Admin {
                    storage_bar: px(a.storage_bar),
                    legend_swatch: px(a.legend_swatch),
                    swatch_radius: px(a.swatch_radius),
                    legend_gap: px(a.legend_gap),
                    status_col: px(a.status_col),
                    action_col: px(a.action_col),
                    client_tile: px(a.client_tile),
                    client_icon: px(a.client_icon),
                    row_gap: px(a.row_gap),
                    confirm_indent: px(a.confirm_indent),
                    code_text: Type::new(&a.code_text, scale),
                    code_pad_x: px(a.code_pad_x),
                    qr: px(a.qr),
                    qr_pad: px(a.qr_pad),
                    countdown_bar: px(a.countdown_bar),
                    time_select: px(a.time_select),
                    copy_button: px(a.copy_button),
                    copy_small: px(a.copy_small),
                    dock_tile: px(a.dock_tile),
                    dock_mark: px(a.dock_mark),
                    dock_badge: px(a.dock_badge),
                    dock_badge_offset: px(a.dock_badge_offset),
                    dock_badge_text: Type::new(&a.dock_badge_text, scale),
                    banner_width: px(a.banner_width),
                    banner_tile: px(a.banner_tile),
                    banner_tile_radius: px(a.banner_tile_radius),
                    banner_mark: px(a.banner_mark),
                    banner_gap: px(a.banner_gap),
                    banner_title: Type::new(&a.banner_title, scale),
                    banner_text: Type::new(&a.banner_text, scale),
                }
            },
            files: Files {
                chip_image: px(f.chip_image),
                chip_file: px(f.chip_file),
                chip_pad: px(f.chip_pad),
                chip_gap: px(f.chip_gap),
                chip_tile: px(f.chip_tile),
                chip_badge: px(f.chip_badge),
                chip_close: px(f.chip_close),
                chip_close_icon: px(f.chip_close_icon),
                progress: px(f.progress),
                row_progress: px(f.row_progress),
                image_tile_w: px(f.image_tile_w),
                image_tile_h: px(f.image_tile_h),
                file_bubble_you: px(f.file_bubble_you),
                file_bubble: px(f.file_bubble),
                bubble_pad_y: px(f.bubble_pad_y),
                bubble_pad_left: px(f.bubble_pad_left),
                bubble_action: px(f.bubble_action),
                drop_icon_tile: px(f.drop_icon_tile),
                drop_icon: px(f.drop_icon),
                lightbox_bar: px(f.lightbox_bar),
                lightbox_nav: px(f.lightbox_nav),
                lightbox_image_w: px(f.lightbox_image_w),
                lightbox_image_h: px(f.lightbox_image_h),
                row_pad_y: px(f.row_pad_y),
                size_col: px(f.size_col),
                date_col: px(f.date_col),
                more: px(f.more),
                artifact_indent: px(f.artifact_indent),
            },
            knowledge: Knowledge {
                switch_w: px(c.knowledge.switch_w),
                switch_h: px(c.knowledge.switch_h),
                switch_thumb: px(c.knowledge.switch_thumb),
                status_dot: px(c.knowledge.status_dot),
                row_pad_y: px(c.knowledge.row_pad_y),
                summary_label: px(c.knowledge.summary_label),
                run_duration: px(c.knowledge.run_duration),
                run_link: px(c.knowledge.run_link),
                channel_field: px(c.knowledge.channel_field),
                time_field: px(c.knowledge.time_field),
                every_field: px(c.knowledge.every_field),
                unit_field: px(c.knowledge.unit_field),
                day_button: px(c.knowledge.day_button),
                zone_popover: px(c.knowledge.zone_popover),
                zone_list: px(c.knowledge.zone_list),
                calendar_cell: px(c.knowledge.calendar_cell),
                tree_indent: px(c.knowledge.tree_indent),
                change_bar: px(c.knowledge.change_bar),
                editor_min_height: px(c.knowledge.editor_min_height),
                version_list: px(c.knowledge.version_list),
                line_icon: px(c.knowledge.line_icon),
            },
            sidebar: Sidebar {
                inset: px(sb.inset),
                row_pad_y: px(sb.row_pad_y),
                row_pad_x: px(sb.row_pad_x),
                row_gap: px(sb.row_gap),
                row_icon: px(sb.row_icon),
                row_text: Type::new(&sb.row_text, scale),
                label_text: Type::new(&sb.label_text, scale),
                label_pad_y: px(sb.label_pad_y),
                group_top: px(sb.group_top),
                title_bar: px(sb.title_bar),
                worker_text: Type::new(&sb.worker_text, scale),
                worker_pad_y: px(sb.worker_pad_y),
                avatar: px(sb.avatar),
                account_pad_top: px(sb.account_pad_top),
                account_pad_bottom: px(sb.account_pad_bottom),
            },
            convo: {
                let v = &c.convo;
                Convo {
                    reaction_chip: px(v.reaction_chip),
                    reaction_pad_x: px(v.reaction_pad_x),
                    reaction_gap: px(v.reaction_gap),
                    quick_emoji: px(v.quick_emoji),
                    menu_width: px(v.menu_width),
                    emoji_picker_width: px(v.emoji_picker_width),
                    emoji_cell: px(v.emoji_cell),
                    emoji_picker_height: px(v.emoji_picker_height),
                    panel_wide: px(v.panel_wide),
                    line_number_col: px(v.line_number_col),
                    search_width: px(v.search_width),
                    search_row: px(v.search_row),
                    search_results_max: px(v.search_results_max),
                    skeleton_dot: px(v.skeleton_dot),
                    skeleton_bar: px(v.skeleton_bar),
                    skeleton_title: px(v.skeleton_title),
                    skeleton_line: px(v.skeleton_line),
                    dialog_settings: px(v.dialog_settings),
                    settings_column_gap: px(v.settings_column_gap),
                    dialog_small: px(v.dialog_small),
                    dialog_group: px(v.dialog_group),
                    dialog_about: px(v.dialog_about),
                    about_mark: px(v.about_mark),
                    icon_choice: px(v.icon_choice),
                    brief_box: px(v.brief_box),
                    instructions_box: px(v.instructions_box),
                    root_ring: px(v.root_ring),
                    drag_line: px(v.drag_line),
                    reaction_emoji: Type::new(&v.reaction_emoji, scale),
                    quick_emoji_text: Type::new(&v.quick_emoji_text, scale),
                    emoji_cell_text: Type::new(&v.emoji_cell_text, scale),
                    wordmark: Type::new(&v.wordmark, scale),
                }
            },
            update_notice: UpdateNotice {
                pad_y: px(c.update_notice.pad_y),
                pad_left: px(c.update_notice.pad_left),
                pad_right: px(c.update_notice.pad_right),
                gap: px(c.update_notice.gap),
                margin_bottom: px(c.update_notice.margin_bottom),
                action_pad_x: px(c.update_notice.action_pad_x),
                action_pad_y: px(c.update_notice.action_pad_y),
            },
            chat: Chat {
                bubble_radius: px(c.chat.bubble_radius),
                bubble_tail: px(c.chat.bubble_tail),
                bubble_pad_x: px(c.chat.bubble_pad_x),
                bubble_pad_y: px(c.chat.bubble_pad_y),
                composer_radius: px(c.chat.composer_radius),
                send_button: px(c.chat.send_button),
                top_action: px(c.chat.top_action),
                top_action_icon: px(c.chat.top_action_icon),
                task_connector: px(c.chat.task_connector),
                mark: px(c.chat.mark),
                mark_small: px(c.chat.mark_small),
                mark_top: px(c.chat.mark_top),
                pill_pad_x: px(c.chat.pill_pad_x),
                pill_pad_y: px(c.chat.pill_pad_y),
                brief_number: px(c.chat.brief_number),
            },
            proactivity: {
                let p = &c.proactivity;
                Proactivity {
                    level_select: px(p.level_select),
                    time_box: px(p.time_box),
                    zone_select: px(p.zone_select),
                    filter_select: px(p.filter_select),
                    log_time: px(p.log_time),
                    log_place: px(p.log_place),
                    log_result: px(p.log_result),
                    log_pad_y: px(p.log_pad_y),
                    count_bar: px(p.count_bar),
                    mark_icon: px(p.mark_icon),
                    mark_gap: px(p.mark_gap),
                    mark_indent: px(p.mark_indent),
                    reason_indent: px(p.reason_indent),
                    chip_pad_x: px(p.chip_pad_x),
                    chip_pad_y: px(p.chip_pad_y),
                    chip_gap: px(p.chip_gap),
                    feedback_pad_x: px(p.feedback_pad_x),
                    feedback_pad_y: px(p.feedback_pad_y),
                    dab: px(p.dab),
                    dab_gap: px(p.dab_gap),
                    dab_rise: px(p.dab_rise),
                    dab_drop: px(p.dab_drop),
                    writing_pad_x: px(p.writing_pad_x),
                    writing_pad_y: px(p.writing_pad_y),
                    offer_pad_top: px(p.offer_pad_top),
                    offer_pad: px(p.offer_pad),
                    brief_group_gap: px(p.brief_group_gap),
                }
            },
            voice: {
                let v = &c.voice;
                Voice {
                    remove_slot: px(v.remove_slot),
                    rule_dot: px(v.rule_dot),
                    source_tile: px(v.source_tile),
                    check_box: px(v.check_box),
                    check_icon: px(v.check_icon),
                    radio_dot: px(v.radio_dot),
                    sample_dot: px(v.sample_dot),
                    flag_bar: px(v.flag_bar),
                    field_label: px(v.field_label),
                    subject_box: px(v.subject_box),
                    body_box: px(v.body_box),
                    line_icon: px(v.line_icon),
                    rule_pad_y: px(v.rule_pad_y),
                    example_pad_y: px(v.example_pad_y),
                }
            },
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
                frame: Duration::from_millis(s.motion.frame_ms),
                done_hold: Duration::from_millis(s.motion.done_hold_ms),
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
                bubble_max_width: px(s.layout.bubble_max_width),
                content_top: px(s.layout.content_top),
                top_tint: s.layout.top_tint.map(|stop| TintStop {
                    at: px(stop.at),
                    alpha: stop.alpha,
                }),
                fade_bottom: px(s.layout.fade_bottom),
            },
            opacity: Opacity {
                answered_dim: s.opacity.answered_dim,
                busy: s.opacity.busy,
                danger_hover: s.opacity.danger_hover,
                failed_border: s.opacity.failed_border,
                progress_track: s.opacity.progress_track,
                drop_fill: s.opacity.drop_fill,
                nav_disabled: s.opacity.nav_disabled,
                you_tile: s.opacity.you_tile,
                offline_dim: s.opacity.offline_dim,
                offline_mark: s.opacity.offline_mark,
                queued: s.opacity.queued,
            },
            component: Components::new(&s.component, scale),
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
        assert_eq!(t.layout.sidebar_width, px(288.0));
        assert_eq!(t.component.sidebar.row_text.size, px(15.0));
        assert_eq!(t.component.sidebar.title_bar, px(52.0));
        assert_eq!(t.layout.content_top, px(98.0));
        assert_eq!(t.layout.top_tint_height(), px(108.0));
    }

    /// The sidebar text follows the text size setting, like every other
    /// text style.
    #[test]
    fn the_sidebar_text_follows_the_text_size() {
        let theme = builtin_themes().into_iter().next().expect("a theme");
        let larger = Tokens::new(&theme, Mode::Dark, TextSize::Larger);
        assert_eq!(larger.component.sidebar.row_text.size, px(17.0));
        assert_eq!(larger.component.sidebar.row_icon, px(18.0));
    }
}
