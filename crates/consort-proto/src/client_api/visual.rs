//! `visual.render`: a client asks for an image of a visual at its width,
//! scale and theme. The plan is `docs/VISUALS-PLAN.md`.

use serde::{Deserialize, Serialize};

use crate::ids::MessageId;
use crate::model::VisualStyle;

/// `visual.render`: an image of a visual. The hub answers from its image
/// cache, or asks the session host to render one. The answer is a
/// `VisualImage`; download the PNG with the HTTP token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisualRender {
    /// The message of the visual.
    pub message_id: MessageId,
    /// The width in CSS pixels, `VISUAL_RENDER_WIDTH_MIN` to
    /// `VISUAL_RENDER_WIDTH_MAX`.
    pub width: u32,
    /// Device pixels per CSS pixel, 1 to `VISUAL_SCALE_MAX`.
    pub scale: u8,
    /// The theme of the client.
    pub style: VisualStyle,
}
