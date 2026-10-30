//! The hub tools of "Write in my voice": `voice_guide` for a session that
//! drafts text in the user's name, `voice_sample` for the Tutti coordinator
//! when the user asks for a "Try it" sample, and `propose_voice_rule` for a
//! learned rule change that the user approves as an offer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::ApprovalId;

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

/// The most characters of the `title` of `show_draft`.
pub const SHOW_DRAFT_TITLE_MAX_CHARS: usize = 200;
/// The most bytes of the `to` line of `show_draft`.
pub const SHOW_DRAFT_TO_MAX_BYTES: usize = 1_000;
/// The most bytes of the `body` of `show_draft` (64 KiB).
pub const SHOW_DRAFT_BODY_MAX_BYTES: usize = 64 * 1024;

/// `show_draft`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShowDraftArgs {
    /// A short title for the card, such as the subject of an email.
    pub title: String,
    /// Who the text is for, in plain words, such as "The landlord" or "Dr.
    /// Meyer's office". Leave it out when the text has no addressee.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// The text that the user sends themself, as it should go out.
    pub body: String,
}

/// The result of `show_draft`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowDraftResult {
    /// The message that holds the draft.
    pub message_id: crate::ids::MessageId,
    /// What happens next.
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

/// `propose_voice_rule`: one learned rule change for the style guide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProposeVoiceRuleArgs {
    /// The rule, as one short sentence, such as "Do not open with 'Hope
    /// you are well'.".
    pub rule: String,
    /// The context of the `voice_edits` event: `core`, `email.formal`,
    /// `email.close`, `chat` or `post.public`. The rule goes into that
    /// section.
    pub context: String,
    /// The `pair` IDs from the `voice_edits` event that show the change:
    /// at least 3 distinct edited pairs of this context.
    pub pair_ids: Vec<ApprovalId>,
    /// A learned rule of that section that the new rule replaces, word for
    /// word. A rule that the user wrote cannot be replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
}
