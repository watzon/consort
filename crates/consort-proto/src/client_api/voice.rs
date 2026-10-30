//! Client requests for "Write in my voice": the settings (`voice.get`,
//! `voice.set`), learning (`voice.learn`), Reset (`voice.reset`), "Try it"
//! (`voice.try`), "Not how I write" (`voice.feedback`), and Edit on the
//! card of a draft or send tool (`approval.edit`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::ApprovalId;
use crate::model::{Disclosure, VoiceSample, VoiceSettings, VoiceSource};

/// The most characters in a "Try it" request.
pub const VOICE_TRY_MAX_CHARS: usize = 500;

/// `voice.get`: the settings, and the newest "Try it" sample while the hub
/// still holds it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceGet {}

/// `voice.set`. Fields that you leave out do not change. Turning the voice
/// on starts the voice task, so the request needs at least one source that
/// can be read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceSet {
    /// "Write in my voice". Turning it off keeps the style guide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// The sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<VoiceSource>>,
    /// Learn from the edits to drafts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learn_from_edits: Option<bool>,
    /// The disclosure line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disclosure: Option<Disclosure>,
}

/// `voice.learn`: start the voice task again ("Learn again").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceLearn {}

/// `voice.reset`: delete the style guide and turn the voice off. The old
/// version stays 30 days and `memory.restore` brings it back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceReset {}

/// `voice.try`: write a sample draft for a request. Nothing is sent, and
/// the hub does not save the sample. The settings say when it is ready;
/// `voice.get` then returns it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceTry {
    /// What to write, for example "Reply to Dana, say yes".
    pub request: String,
}

/// The response to the voice requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceResult {
    /// The settings after the change.
    pub settings: VoiceSettings,
    /// The newest "Try it" sample, while the hub holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<VoiceSample>,
    /// For `voice.get`: the style guide that waits for "Do it" on the offer
    /// card "Save this style guide?", in the `voice.md` format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<String>,
}

/// `voice.feedback`: "Not how I write" on the card of a draft. The hub
/// marks the draft as strong feedback, and the Tutti coordinator asks what
/// is wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceFeedback {
    /// The connector card.
    pub approval_id: ApprovalId,
}

/// `approval.edit`: change the text fields of a connector card before an
/// answer. The hub checks the new text again, sets new commitment flags,
/// and sends these arguments when the user allows the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalEdit {
    /// The connector card.
    pub approval_id: ApprovalId,
    /// The new text of each field that changed, by argument key. Only the
    /// card's `editable` keys are allowed.
    pub fields: BTreeMap<String, String>,
}
