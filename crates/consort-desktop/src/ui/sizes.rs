//! Component sizes that `design/structure.json` does not name. Each one comes
//! from the approved Paper design or from `docs/design-system.md`. View code
//! reads sizes from the theme tokens or from here, never as a literal.

use gpui_kit::{Pixels, px};

/// The label column of a settings row ("a label column of 160 px").
pub const SETTINGS_LABEL: Pixels = px(160.0);
/// A segmented control in Settings: Mode and Text size.
pub const SETTINGS_SEGMENTED: Pixels = px(336.0);
/// One theme card in Settings > Appearance.
pub const THEME_CARD: Pixels = px(310.0);
/// Two theme cards and the gap between them.
pub const THEME_ROW: Pixels = px(632.0);
/// The height of a theme preview.
pub const THEME_PREVIEW_HEIGHT: Pixels = px(92.0);
/// The worker list in Settings > Workers.
pub const WORKER_LIST: Pixels = px(288.0);
/// The widest a settings detail page grows.
pub const SETTINGS_DETAIL_MAX: Pixels = px(760.0);
/// The sign-in column of a harness account row.
pub const ACCOUNT_STATE: Pixels = px(140.0);
/// A select in a settings row.
pub const SETTINGS_SELECT: Pixels = px(160.0);
/// The Group select in the New channel dialog.
pub const GROUP_SELECT: Pixels = px(200.0);
/// The Workspace control in the New channel dialog.
pub const WORKSPACE_CONTROL: Pixels = px(230.0);
/// The vertical padding of a text input: 36 px with its border.
pub const INPUT_PAD_Y: Pixels = px(7.0);
/// The vertical padding of a text button: 30 px high.
pub const BUTTON_PAD_Y: Pixels = px(5.0);
/// The dot of a worker in a list.
pub const WORKER_DOT: Pixels = px(7.0);
/// The dot of a sign-in state.
pub const STATE_DOT: Pixels = px(6.0);
/// The vertical padding of a worker row in the sidebar: 26 px rows.
pub const WORKER_ROW_PAD_Y: Pixels = px(3.0);
/// The inset of the switch thumb.
pub const SWITCH_INSET: Pixels = px(3.0);

/// The drawing of a theme in its card: a miniature sidebar, two lines of
/// text and a card with two buttons.
pub mod preview {
    use gpui_kit::{Pixels, px};

    pub const BAR: Pixels = px(4.0);
    pub const BAR_GAP: Pixels = px(5.0);
    pub const SIDEBAR: Pixels = px(32.0);
    pub const SIDEBAR_ITEM: f32 = 20.0;
    pub const TEXT_LINE: f32 = 76.0;
    pub const TITLE: Pixels = px(48.0);
    pub const CHIP: Pixels = px(6.0);
    pub const CARD: Pixels = px(28.0);
    pub const BUTTON: Pixels = px(22.0);
    pub const BUTTON_RADIUS: Pixels = px(2.0);
}
