//! The visual tools: `preview_visual`, which the session host answers
//! itself, and `show_visual`, which the session host prepares and the hub
//! posts. The plan is `docs/VISUALS-PLAN.md`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::MessageId;
use crate::model::{BlobRef, VisualHeight, VisualMode};

/// What `show_visual` tells the assistant after the visual is posted.
pub const SHOW_VISUAL_SHOWN: &str =
    "The visual is in the conversation. Do not describe it or repeat its data in your reply.";

/// What `preview_visual` says while the browser installs.
pub const PREVIEW_VISUAL_INSTALLING: &str =
    "The preview browser is installing. Try again in a minute.";

/// `preview_visual`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PreviewVisualArgs {
    /// The HTML of the page. It can be a whole document or a fragment.
    pub html: String,
    /// The width of the page in CSS pixels, 320 to 1144. The timeline
    /// column is 720.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// `dark` or `light`. The default is `dark`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<VisualMode>,
}

/// One console message or page error from `preview_visual`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisualConsoleMessage {
    /// `log`, `info`, `warning` or `error`. A script error is `error`.
    pub level: String,
    /// The text, cut to `VISUAL_CONSOLE_MAX_CHARS`.
    pub text: String,
}

/// The text part of the result of `preview_visual`. The image part is the
/// PNG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewVisualResult {
    /// The width of the page.
    pub width: u32,
    /// The height of the content at that width.
    pub height: u32,
    /// The console messages and page errors, at most
    /// `VISUAL_CONSOLE_MAX_MESSAGES`.
    #[serde(default)]
    pub console: Vec<VisualConsoleMessage>,
    /// How many console messages did not fit.
    #[serde(default)]
    pub omitted: u32,
}

/// `show_visual`. The session host prepares the page, uploads it, and adds
/// `page` and `heights` before it forwards the call. It sends `html` empty.
/// The model does not see those fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShowVisualArgs {
    /// The HTML of the page. It can be a whole document or a fragment. It
    /// fills the width and grows to fit its content.
    pub html: String,
    /// A short title, such as "Tokens by week".
    pub title: String,
    /// The height of the page in CSS pixels, 80 to 2000, if you know it.
    /// The measured height wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Added by the session host: the prepared page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub page: Option<BlobRef>,
    /// Added by the session host: the height table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub heights: Option<Vec<VisualHeight>>,
}

/// The result of `show_visual`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowVisualResult {
    /// The message that holds the visual.
    pub message_id: MessageId,
    /// What to do next: [`SHOW_VISUAL_SHOWN`].
    pub message: String,
}
