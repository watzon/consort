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
    /// One frame of stop motion: the Three voices mark and the icon
    /// shimmer play at 8 frames a second.
    pub frame_ms: u64,
    /// How long the mark holds its Done pose before it goes back to idle.
    pub done_hold_ms: u64,
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
    /// The Add connector and Add skill dialogs: a search and a list that
    /// scrolls inside the dialog.
    pub dialog_catalog: f32,
    /// The channel switcher.
    pub switcher_width: f32,
    /// The space above the switcher.
    pub switcher_top: f32,
    /// The longest a channel name grows in a switcher row.
    pub switcher_name_max: f32,
    /// The tallest a block of error text grows: a toast, or the reason in a
    /// task thread. Its text scrolls past this.
    pub error_text_max_height: f32,
    /// The widest a bubble, a task card or an approval card grows in the
    /// timeline.
    pub bubble_max_width: f32,
    /// The space above the first item of a chat at rest. It reaches just
    /// below the Three voices mark and the activity pill. A test checks
    /// that it is tall enough for both.
    pub content_top: f32,
    /// The progressive tint at the top of the timeline: the canvas color at
    /// each stop's opacity, from the top down. The last stop is clear and
    /// sets the height of the tint. GPUI gradients have two stops, so the
    /// view draws one gradient layer between each pair of stops.
    pub top_tint: [TintStop; 4],
    /// The fade at the bottom of the timeline, above the composer.
    pub fade_bottom: f32,
}

/// One stop of the top tint: a distance from the top in px, and the opacity
/// of the canvas color there.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct TintStop {
    pub at: f32,
    pub alpha: f32,
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
    /// The border of a failed upload chip: `status.failed` at this share.
    pub failed_border: f32,
    /// The track of an upload progress bar: `status.working` at this share.
    pub progress_track: f32,
    /// The soft fill of the composer while files are dragged over the chat:
    /// `accent.fill` at this share, over `surface.card`.
    pub drop_fill: f32,
    /// A Previous or Next button of the light box at the first or last
    /// picture.
    pub nav_disabled: f32,
    /// The file tile in a file bubble in your lane: the scrim at this
    /// share, so the tile reads on `accent.surface`.
    pub you_tile: f32,
    /// A button that needs the hub, while the hub is offline.
    pub offline_dim: f32,
    /// The Three voices mark while the hub is offline.
    pub offline_mark: f32,
    /// The bubble of a message that waits in the outbox.
    pub queued: f32,
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
    /// The height of the model picker popover. The list of the main pane
    /// scrolls inside it.
    pub model_picker_height: f32,
    /// The width of the rail at the left of the model picker.
    pub picker_rail: f32,
    /// A square button in the rail: Favorites or an account.
    pub picker_rail_button: f32,
    /// The height and the least width of the badge on an account button.
    pub picker_rail_badge: f32,
    /// The text size of that badge.
    pub picker_rail_badge_text: f32,
    /// The width of the accent bar that marks the selected rail button.
    pub picker_rail_marker: f32,
    /// The height of that bar.
    pub picker_rail_marker_height: f32,
    /// The width of the effort menu.
    pub effort_menu: f32,
    /// The effort select next to a model field in a dialog.
    pub effort_select: f32,
    /// The height of a chip in the composer, such as the model chip.
    pub composer_chip: f32,
    /// The tile around a service logo at the top of a connector page and
    /// of the Connect dialog.
    pub connector_tile: f32,
    /// A text field or a select in a row of a connector or skill page.
    pub connection_field: f32,
    /// The height of the service grid in the Add connector dialog. It
    /// scrolls inside.
    pub catalog_list: f32,
    /// The height of the result list in the Add skill dialog. It scrolls
    /// inside.
    pub skill_results: f32,
    /// The height of the Composio usage bar.
    pub usage_bar: f32,
    /// The file list of the skill files dialog.
    pub files_list: f32,
    /// The height of the skill files dialog body.
    pub files_view: f32,
    /// The vertical padding of a text input: 36 px with its border.
    pub input_pad_y: f32,
    /// The vertical padding of a text button: 30 px high.
    pub button_pad_y: f32,
    /// The dot of a worker in a list.
    pub worker_dot: f32,
    /// The dot of a sign-in state.
    pub state_dot: f32,
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
    pub admin: Admin,
    pub files: Files,
    pub knowledge: Knowledge,
    pub sidebar: Sidebar,
    pub update_notice: UpdateNotice,
    pub chat: Chat,
    pub theme_preview: ThemePreview,
    pub convo: Convo,
}

/// The sizes of Settings > Hub, Settings > Clients, the Pair a new app
/// dialog and Settings > Notifications. From the phase 3 Paper artboards.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Admin {
    /// The height of the storage bar, and the legend swatches.
    pub storage_bar: f32,
    pub legend_swatch: f32,
    pub swatch_radius: f32,
    pub legend_gap: f32,
    /// The status and the action columns of a client row.
    pub status_col: f32,
    pub action_col: f32,
    /// The tile and the icon of a client row.
    pub client_tile: f32,
    pub client_icon: f32,
    pub row_gap: f32,
    /// The indent of the inline Revoke confirm, under the name.
    pub confirm_indent: f32,
    /// The setup code in the pairing dialog, and its panel padding.
    pub code_text: TypeStyle,
    pub code_pad_x: f32,
    /// The QR code and its padding in its tile.
    pub qr: f32,
    pub qr_pad: f32,
    /// The countdown bar of a pairing code.
    pub countdown_bar: f32,
    /// The time select of the backup schedule.
    pub time_select: f32,
    /// The copy buttons: in a row, and in the address field.
    pub copy_button: f32,
    pub copy_small: f32,
    /// The Dock icon preview and its badge.
    pub dock_tile: f32,
    pub dock_mark: f32,
    pub dock_badge: f32,
    pub dock_badge_offset: f32,
    pub dock_badge_text: TypeStyle,
    /// The example banner.
    pub banner_width: f32,
    pub banner_tile: f32,
    pub banner_tile_radius: f32,
    pub banner_mark: f32,
    pub banner_gap: f32,
    pub banner_title: TypeStyle,
    pub banner_text: TypeStyle,
}

/// The sizes of routines and memory: the routine rows, the routine dialog
/// with its schedule picker, the run history, the memory tree, the note
/// editor and the version history. From the phase 2 Paper artboards.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Knowledge {
    /// The small switch of a routine row and of the routine page header.
    pub switch_w: f32,
    pub switch_h: f32,
    pub switch_thumb: f32,
    /// The dot before the result of a routine and the "Just now" of a note.
    pub status_dot: f32,
    /// The padding of a routine row.
    pub row_pad_y: f32,
    /// The label column of the routine summary.
    pub summary_label: f32,
    /// The duration and the link columns of the run history.
    pub run_duration: f32,
    pub run_link: f32,
    /// The Channel select of the routine dialog.
    pub channel_field: f32,
    /// The time field, the interval number and its unit select.
    pub time_field: f32,
    pub every_field: f32,
    pub unit_field: f32,
    /// A day button of the Weekly preset.
    pub day_button: f32,
    /// The time zone popover and the height of its list.
    pub zone_popover: f32,
    pub zone_list: f32,
    /// A day of the calendar popover of the date field.
    pub calendar_cell: f32,
    /// The indent of one level of the memory tree.
    pub tree_indent: f32,
    /// The bar that marks a changed line in a version preview.
    pub change_bar: f32,
    /// The least height of the note editor.
    pub editor_min_height: f32,
    /// The version list of the version history.
    pub version_list: f32,
    /// The icon of the memory saved line.
    pub line_icon: f32,
}

/// The sizes of attachments and channel files: the chips of the composer,
/// the attachments in bubbles, the drop state, the light box and the rows
/// of the Files tab. From the phase 1 Paper artboards.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Files {
    /// An image chip in the composer: a square thumbnail.
    pub chip_image: f32,
    /// A file chip in the composer.
    pub chip_file: f32,
    pub chip_pad: f32,
    pub chip_gap: f32,
    /// The tile with the file icon in a chip or a file bubble.
    pub chip_tile: f32,
    /// The round remove badge on an image chip.
    pub chip_badge: f32,
    /// The remove button of a file chip, and its icon.
    pub chip_close: f32,
    pub chip_close_icon: f32,
    /// The height of the progress bar of an upload in a chip.
    pub progress: f32,
    /// The height of the progress bar of an upload in the Files tab.
    pub row_progress: f32,
    /// A picture in a bubble group.
    pub image_tile_w: f32,
    pub image_tile_h: f32,
    /// A file bubble in your lane, and a file or artifact bubble in the
    /// assistant lane.
    pub file_bubble_you: f32,
    pub file_bubble: f32,
    pub bubble_pad_y: f32,
    pub bubble_pad_left: f32,
    /// The Save and Open buttons of a file bubble on hover.
    pub bubble_action: f32,
    /// The round tile and the icon of the drop state of the composer.
    pub drop_icon_tile: f32,
    pub drop_icon: f32,
    /// The top and bottom bars of the light box.
    pub lightbox_bar: f32,
    /// The round Previous and Next buttons of the light box.
    pub lightbox_nav: f32,
    /// The largest picture in the light box.
    pub lightbox_image_w: f32,
    pub lightbox_image_h: f32,
    /// A row of the Files tab.
    pub row_pad_y: f32,
    /// The size and date columns of a row of the Files tab.
    pub size_col: f32,
    pub date_col: f32,
    /// The "more" button of a row.
    pub more: f32,
    /// The indent of an artifact under its task.
    pub artifact_indent: f32,
}

/// The sizes of the sidebar and the settings rail. From the approved Paper
/// artboard "Base: Tutti with top tint and larger sidebar, dark".
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Sidebar {
    /// The space between the sidebar edges and its rows.
    pub inset: f32,
    /// The padding of a row: 36 px rows with the row text.
    pub row_pad_y: f32,
    pub row_pad_x: f32,
    /// The gap between the icon and the name of a row.
    pub row_gap: f32,
    /// The icon and the status glyph of a row.
    pub row_icon: f32,
    /// The text of a row: Search, Tutti, a channel, a settings section.
    pub row_text: TypeStyle,
    /// A group label, the Workers label and a settings group label.
    pub label_text: TypeStyle,
    pub label_pad_y: f32,
    /// The space above a group.
    pub group_top: f32,
    /// The title bar strip at the top, where the macOS window buttons sit.
    pub title_bar: f32,
    /// The name of a worker row.
    pub worker_text: TypeStyle,
    /// The padding of a worker row: 30 px rows.
    pub worker_pad_y: f32,
    /// The avatar in the account row.
    pub avatar: f32,
    pub account_pad_top: f32,
    pub account_pad_bottom: f32,
}

/// The sizes of phase 4: reactions, the message menu, the emoji picker,
/// the wide panel of a task item, the search palette, and the New task,
/// channel settings, New channel, group and About dialogs. From the phase 4
/// Paper artboards.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Convo {
    /// The height of a reaction chip under a bubble, and its padding.
    pub reaction_chip: f32,
    pub reaction_pad_x: f32,
    /// The gap between the chips, and above them.
    pub reaction_gap: f32,
    /// A quick emoji button at the top of the message menu.
    pub quick_emoji: f32,
    /// The message menu with the quick emoji.
    pub menu_width: f32,
    /// The emoji picker behind More, and one cell of its grid.
    pub emoji_picker_width: f32,
    pub emoji_cell: f32,
    pub emoji_picker_height: f32,
    /// The side panel while it shows a task item in full.
    pub panel_wide: f32,
    /// The line numbers of command output, and of each side of a diff.
    pub line_number_col: f32,
    /// The search palette.
    pub search_width: f32,
    /// A Go to row of the search palette.
    pub search_row: f32,
    /// The most height of the result list before it scrolls.
    pub search_results_max: f32,
    /// A skeleton row while the hub searches: the dot and the bars.
    pub skeleton_dot: f32,
    pub skeleton_bar: f32,
    pub skeleton_title: f32,
    pub skeleton_line: f32,
    /// The channel settings dialog: two columns.
    pub dialog_settings: f32,
    pub settings_column_gap: f32,
    /// The New channel dialog.
    pub dialog_small: f32,
    /// The New group and Delete group dialogs.
    pub dialog_group: f32,
    /// The About dialog, its mark and its wordmark.
    pub dialog_about: f32,
    pub about_mark: f32,
    /// An icon choice of the New channel dialog and the channel settings.
    pub icon_choice: f32,
    /// The least height of the brief box of the New task dialog.
    pub brief_box: f32,
    /// The least height of the instructions box of the channel settings.
    pub instructions_box: f32,
    /// The accent ring around the root of an open thread.
    pub root_ring: f32,
    /// The line that shows where a dragged group goes.
    pub drag_line: f32,
    pub reaction_emoji: TypeStyle,
    pub quick_emoji_text: TypeStyle,
    pub emoji_cell_text: TypeStyle,
    pub wordmark: TypeStyle,
}

/// The sizes of the update notice above the account row in the sidebar.
/// From the approved Paper artboard "App update in the sidebar, dark".
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct UpdateNotice {
    pub pad_y: f32,
    pub pad_left: f32,
    pub pad_right: f32,
    /// The gap between the icon, the text and the button.
    pub gap: f32,
    /// The space between the notice and the account row.
    pub margin_bottom: f32,
    /// The padding of the Restart button.
    pub action_pad_x: f32,
    pub action_pad_y: f32,
}

/// The sizes of the chat: bubbles, the composer, the top actions and the
/// Three voices mark. From the approved Paper artboard "Tutti, bubbles,
/// dark".
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Chat {
    /// The corners of a bubble, a card in the timeline and the composer.
    pub bubble_radius: f32,
    /// The small corner on the speaker's side of the last bubble of a
    /// group, and the top-left corner of a card under its task line.
    pub bubble_tail: f32,
    pub bubble_pad_x: f32,
    pub bubble_pad_y: f32,
    /// The corners of the composer field.
    pub composer_radius: f32,
    /// The send key inside the composer field.
    pub send_button: f32,
    /// A button at the top right of the timeline: Tasks, New task,
    /// Connectors.
    pub top_action: f32,
    /// The icon in a top action.
    pub top_action_icon: f32,
    /// The curved connector at the start of a task line.
    pub task_connector: f32,
    /// The Three voices mark over the timeline.
    pub mark: f32,
    /// The mark in the sidebar, sharp.
    pub mark_small: f32,
    /// The space above the mark.
    pub mark_top: f32,
    pub pill_pad_x: f32,
    pub pill_pad_y: f32,
    /// The number column of a list in a brief.
    pub brief_number: f32,
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
    /// The black behind the light box.
    pub lightbox_alpha: f32,
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
        assert_eq!(s.layout.sidebar_width, 288.0);
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

    /// The chat redesign: the bubble width, the fades and the frame rate of
    /// the stop motion.
    #[test]
    fn chat_layout_and_motion_tokens_keep_the_design_values() {
        let s = super::structure();
        assert_eq!(s.layout.bubble_max_width, 560.0);
        assert!(s.layout.bubble_max_width < s.layout.timeline_max_width);
        assert_eq!((s.layout.content_top, s.layout.fade_bottom), (98.0, 32.0));
        // 8 frames a second.
        assert_eq!(s.motion.frame_ms, 125);
        assert_eq!(s.motion.done_hold_ms, 1500);
        // At rest, the first item starts below the mark, the gap and the
        // pill, with a margin. The pill is one line of the label size, its
        // padding and its border. The activity pill is not drawn when the
        // assistant idles, but the space stays, so the chat does not jump.
        let chat = &s.component.chat;
        let pill = s.type_scale.label.line + 2.0 * chat.pill_pad_y + 2.0 * s.border_width;
        let needed = chat.mark_top + chat.mark + s.space.xxs + pill + s.space.xs;
        assert!(
            s.layout.content_top >= needed,
            "{} < {needed}",
            s.layout.content_top
        );
        assert!(s.layout.content_top - needed < s.space.md);
    }

    /// The phase 4 sizes come from the Paper artboards: the reaction chips
    /// and the quick emoji of `9I9-0`, the 560 px item view of `CA5-0`, the
    /// 640 px search palette of `A0N-0`, and the 880 px channel settings of
    /// `CYJ-0`. The wide panel is wider than the usual one, and the two
    /// columns of the settings fit in the dialog.
    #[test]
    fn convo_tokens_keep_the_values_of_the_phase_4_designs() {
        let s = super::structure();
        let v = s.component.convo;
        assert_eq!((v.reaction_chip, v.reaction_pad_x), (24.0, 8.0));
        assert_eq!((v.quick_emoji, v.menu_width), (30.0, 248.0));
        assert_eq!(v.reaction_emoji.size, 13.0);
        assert_eq!(v.panel_wide, 560.0);
        assert!(v.panel_wide > s.layout.panel_width);
        assert_eq!(v.search_width, 640.0);
        assert_eq!(v.dialog_settings, 880.0);
        assert!(v.dialog_settings > 2.0 * s.layout.dialog_width * 0.8);
        assert_eq!(
            (v.dialog_small, v.dialog_group, v.dialog_about),
            (460.0, 400.0, 440.0)
        );
        assert_eq!(v.wordmark.weight, 700);
        // Six quick emoji and More fit in the menu.
        assert!(6.0 * v.quick_emoji < v.menu_width);
    }

    /// The top tint. The user asked for a tint that is much darker from the
    /// mark up, like Claude Duet: the canvas at 97 percent at the top, 94 at
    /// the bottom of the mark (60 px), 80 at the bottom of the pill (86 px),
    /// and clear at 108 px, just below the place where the first item
    /// starts at rest. The tint gets lighter all the way down, and content
    /// that moves up under the mark stays faintly visible.
    #[test]
    fn the_top_tint_gets_lighter_from_the_top_to_clear() {
        let s = super::structure();
        let stops = s.layout.top_tint;
        assert_eq!(
            stops.map(|t| (t.at, t.alpha)),
            [(0.0, 0.97), (60.0, 0.94), (86.0, 0.8), (108.0, 0.0)]
        );
        for pair in stops.windows(2) {
            assert!(pair[0].at < pair[1].at);
            assert!(pair[0].alpha > pair[1].alpha);
        }
        assert!(stops[0].alpha < 1.0, "content stays faintly visible");
        assert!(stops[3].at > s.layout.content_top);
    }

    #[test]
    fn opacity_tokens_are_shares_between_zero_and_one() {
        let o = super::structure().opacity;
        assert_eq!(o.answered_dim, 0.6);
        assert_eq!(o.busy, 0.7);
        assert_eq!(o.danger_hover, 0.9);
        assert_eq!(
            (
                o.failed_border,
                o.progress_track,
                o.drop_fill,
                o.nav_disabled
            ),
            (0.45, 0.22, 0.06, 0.4)
        );
        assert_eq!((o.offline_dim, o.offline_mark, o.queued), (0.45, 0.7, 0.75));
        for share in [
            o.answered_dim,
            o.busy,
            o.danger_hover,
            o.failed_border,
            o.progress_track,
            o.drop_fill,
            o.nav_disabled,
            o.you_tile,
            o.offline_dim,
            o.offline_mark,
            o.queued,
        ] {
            assert!(share > 0.0 && share < 1.0);
        }
    }

    /// The phase 1 artboards: composer chips, attachments in bubbles, the
    /// light box and the rows of the Files tab.
    #[test]
    fn admin_tokens_keep_the_values_of_the_phase_3_designs() {
        let a = super::structure().component.admin;
        assert_eq!(
            (a.storage_bar, a.legend_swatch, a.legend_gap),
            (6.0, 8.0, 28.0)
        );
        assert_eq!(
            (a.status_col, a.action_col, a.client_tile),
            (150.0, 72.0, 32.0)
        );
        // The confirm text starts under the client name: the row padding,
        // the tile and the gap.
        let s = super::structure();
        assert_eq!(s.space.lg + a.client_tile + a.row_gap, a.confirm_indent);
        assert_eq!((a.code_text.size, a.code_text.line), (34.0, 42.0));
        assert_eq!((a.qr, a.qr_pad), (148.0, 8.0));
        assert_eq!(
            (a.dock_tile, a.dock_badge, a.banner_width),
            (40.0, 18.0, 360.0)
        );
    }

    #[test]
    fn file_tokens_keep_the_values_of_the_phase_1_designs() {
        let f = super::structure().component.files;
        assert_eq!(
            (f.chip_image, f.chip_file, f.chip_tile),
            (56.0, 240.0, 38.0)
        );
        // A file chip is as high as an image chip.
        assert_eq!(f.chip_tile + 2.0 * f.chip_pad + 2.0, f.chip_image);
        assert_eq!((f.image_tile_w, f.image_tile_h), (228.0, 168.0));
        // Two pictures and their gap fit in a bubble.
        let s = super::structure();
        assert!(2.0 * f.image_tile_w + s.space.xs <= s.layout.bubble_max_width);
        assert_eq!((f.file_bubble_you, f.file_bubble), (280.0, 360.0));
        assert_eq!((f.lightbox_bar, f.lightbox_nav), (56.0, 40.0));
        assert_eq!((f.size_col, f.date_col, f.more), (64.0, 52.0, 20.0));
        assert!(s.derive.lightbox_alpha > s.derive.scrim_alpha.dark);
    }

    /// The phase 2 artboards: routine rows, the routine dialog, the run
    /// history, the memory tree and the version history.
    #[test]
    fn knowledge_tokens_keep_the_values_of_the_phase_2_designs() {
        let s = super::structure();
        let k = s.component.knowledge;
        assert_eq!((k.switch_w, k.switch_h, k.switch_thumb), (32.0, 18.0, 14.0));
        // The thumb sits 2 px inside the track.
        assert_eq!(k.switch_h - k.switch_thumb, 2.0 * s.space.xxs);
        assert_eq!(
            (k.summary_label, k.run_duration, k.run_link),
            (80.0, 64.0, 76.0)
        );
        assert_eq!((k.channel_field, k.time_field), (190.0, 84.0));
        // The Name field and the Channel field fill the dialog body, inside
        // the dialog's border and padding.
        assert_eq!(
            s.layout.dialog_wide
                - 2.0 * (s.space.xl + s.border_width)
                - s.space.md
                - k.channel_field,
            308.0
        );
        assert_eq!((k.tree_indent, k.change_bar), (22.0, 3.0));
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
            (c.model_picker, c.model_picker_height, c.effort_menu),
            (440.0, 420.0, 240.0)
        );
        // The rail button sits in the rail with equal room on each side, and
        // the marker sits on the rail's right edge, centred on the button.
        assert_eq!((c.picker_rail, c.picker_rail_button), (48.0, 36.0));
        assert_eq!((c.picker_rail_badge, c.picker_rail_badge_text), (14.0, 9.0));
        assert_eq!(
            (c.picker_rail_marker, c.picker_rail_marker_height),
            (3.0, 20.0)
        );
        assert_eq!((c.effort_select, c.composer_chip), (150.0, 28.0));
        // A 20 px line, the padding and a 1 px border make 36 px and 30 px.
        assert_eq!(c.input_pad_y * 2.0 + 20.0 + 2.0, 36.0);
        assert_eq!(c.button_pad_y * 2.0 + 20.0, 30.0);
        assert_eq!(c.worker_dot, 7.0);
        assert_eq!(c.state_dot, 6.0);
        // The larger sidebar: 288 px, 36 px rows with 15 px text and 18 px
        // icons, 13 px labels, a 52 px title bar, 30 px worker rows with
        // 14 px names and a 28 px avatar.
        let sb = c.sidebar;
        assert_eq!(sb.row_pad_y * 2.0 + sb.row_text.line, 36.0);
        assert_eq!((sb.row_text.size, sb.row_icon), (15.0, 18.0));
        assert_eq!((sb.row_pad_x, sb.row_gap, sb.inset), (10.0, 10.0, 10.0));
        assert_eq!((sb.label_text.size, sb.label_text.weight), (13.0, 500));
        assert_eq!(sb.title_bar, 52.0);
        assert_eq!(sb.worker_text.size, 14.0);
        assert_eq!(sb.worker_pad_y * 2.0 + sb.worker_text.line, 30.0);
        assert_eq!(sb.avatar, 28.0);
        assert_eq!(c.switch_inset, 3.0);
        assert_eq!(c.focus_ring, 2.0);
        let chat = c.chat;
        assert_eq!((chat.bubble_radius, chat.bubble_tail), (16.0, 4.0));
        assert_eq!((chat.bubble_pad_x, chat.bubble_pad_y), (14.0, 10.0));
        assert_eq!((chat.composer_radius, chat.send_button), (16.0, 32.0));
        assert_eq!((chat.top_action, chat.top_action_icon), (32.0, 18.0));
        assert_eq!(
            (chat.mark, chat.mark_small, chat.mark_top),
            (56.0, 16.0, 4.0)
        );
        assert_eq!((chat.pill_pad_x, chat.pill_pad_y), (10.0, 3.0));
        assert_eq!((chat.task_connector, chat.brief_number), (14.0, 16.0));
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
