//! The hub tools of "Write in my voice": `voice_guide` for a session that
//! drafts text in the user's name, and `voice_sample` for the Tutti
//! coordinator when the user asks for a "Try it" sample.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `voice_guide`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceGuideArgs {
    /// The context of the draft: `email.formal` (clients and new
    /// contacts), `email.close` (colleagues and friends), `chat` or
    /// `post.public`. Leave it out when you are not sure: you get the Core
    /// rules only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// The result of `voice_guide`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceGuideResult {
    /// Whether the voice is on. When it is off, write in a neutral style.
    pub enabled: bool,
    /// The context that the guide used: the one you asked for, or `core`.
    pub context: String,
    /// The rules and examples to follow.
    pub guide: String,
    /// What to do with it.
    pub message: String,
}

/// `voice_sample`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VoiceSampleArgs {
    /// The sample number from the `voice_sample` event.
    pub sample: u64,
    /// The sample draft. Nothing is sent.
    pub draft: String,
    /// The context that you used, or `core`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}
