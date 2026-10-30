//! Write in my voice, as a client shows it: Settings > Voice (the switch,
//! the sources, the style guide, Try it, Disclosure, Turn off and Reset),
//! the offer "Save this style guide?", and the voice parts of the card of a
//! draft or send tool (the voice line, the commitment flags, Edit and "Not
//! how I write").
//!
//! The hub decides and enforces (see `docs/VOICE-PLAN.md`). The client
//! shows the state and sends the user's choices.

use chrono::{DateTime, Local, Utc};
use consort_proto::model::{
    self, ConnectorCard, MemoryScope, VOICE_NOTE_PATH, VoiceGuide, VoiceRule, VoiceSettings,
    context_description, context_label, memory_note_entity_id,
};
use consort_proto::status::TaskStatus as ProtoStatus;

pub use consort_proto::model::{
    Disclosure, FlagKind, OutgoingKind, VOICE_PROPOSAL_QUESTION, VOICE_TASK_TITLE,
    VoiceSampleState, VoiceSource, VoiceSourceKind,
};

use crate::cache::Cache;
use crate::view::TaskStatus;

// Cache ----------------------------------------------------------------------

/// What the cache keeps about the voice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoiceCache {
    /// The settings, from the `all` snapshot and the feed. Empty from an
    /// older hub.
    pub settings: Option<VoiceSettings>,
    /// The newest "Try it" sample, from `voice.get`. Only in memory.
    pub sample: Option<model::VoiceSample>,
    /// The style guide that waits for "Do it", from `voice.get`.
    pub proposal: Option<String>,
    /// What went wrong with the last change.
    pub error: Option<String>,
}

impl VoiceCache {
    /// Applies settings from the feed. Returns whether they changed.
    pub fn apply_settings(&mut self, settings: VoiceSettings) -> bool {
        if self.settings.as_ref() == Some(&settings) {
            return false;
        }
        if settings.proposal_decision_id.is_none() {
            self.proposal = None;
        }
        self.settings = Some(settings);
        true
    }
}

// Changes --------------------------------------------------------------------

/// A change of the settings. Fields that are `None` do not change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceChanges {
    pub enabled: Option<bool>,
    pub sources: Option<Vec<VoiceSource>>,
    pub learn_from_edits: Option<bool>,
    pub disclosure: Option<Disclosure>,
}

impl VoiceChanges {
    /// The `voice.set` request.
    pub fn request(&self) -> consort_proto::client_api::VoiceSet {
        consort_proto::client_api::VoiceSet {
            enabled: self.enabled,
            sources: self.sources.clone(),
            learn_from_edits: self.learn_from_edits,
            disclosure: self.disclosure,
        }
    }
}

/// A change to the style guide from Settings > Voice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuideEdit {
    /// Remove one rule of a section (`core`, a context key or `hard`).
    RemoveRule { section: String, index: usize },
    /// Remove one example of a context.
    RemoveExample { section: String, index: usize },
    /// Add a hard rule. It is the user's rule.
    AddHardRule { text: String },
    /// Replace the whole note ("Edit as markdown").
    Markdown { content: String },
}

/// The section key of the hard rules in [`GuideEdit`].
pub const HARD_RULES: &str = "hard";

/// The note after an edit. The hub marks rules that the user added.
pub fn apply_guide_edit(content: &str, edit: &GuideEdit) -> String {
    let mut guide = VoiceGuide::parse(content);
    match edit {
        GuideEdit::RemoveRule { section, index } => {
            let rules = match section.as_str() {
                model::CORE_CONTEXT => Some(&mut guide.core),
                HARD_RULES => Some(&mut guide.hard_rules),
                key => guide
                    .contexts
                    .iter_mut()
                    .find(|c| c.key == key)
                    .map(|c| &mut c.rules),
            };
            if let Some(rules) = rules
                && *index < rules.len()
            {
                rules.remove(*index);
            }
        }
        GuideEdit::RemoveExample { section, index } => {
            if let Some(context) = guide.contexts.iter_mut().find(|c| &c.key == section)
                && *index < context.examples.len()
            {
                context.examples.remove(*index);
            }
        }
        GuideEdit::AddHardRule { text } => {
            let text = text.trim();
            if !text.is_empty() {
                guide.hard_rules.push(VoiceRule {
                    text: text.to_owned(),
                    by_user: true,
                });
            }
        }
        GuideEdit::Markdown { content } => return content.clone(),
    }
    guide.render()
}

// Views ----------------------------------------------------------------------

/// One source row of Settings > Voice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceSourceRow {
    pub connection_id: String,
    pub kind: Option<VoiceSourceKind>,
    /// The letter in the tile, such as "G".
    pub monogram: String,
    /// "Gmail, chris@example.com".
    pub title: String,
    /// "Sent mail, last 90 days".
    pub detail: String,
    pub ticked: bool,
    /// For chat: Direct messages are ticked.
    pub include_private: bool,
}

/// A rule of the style guide.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceRuleView {
    pub text: String,
    /// The user wrote it: "(you)".
    pub by_user: bool,
}

/// What a section of the style guide is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GuideSectionKind {
    #[default]
    Core,
    Context,
    Hard,
}

/// One card of the style guide.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuideSectionView {
    /// `core`, a context key or `hard`.
    pub key: String,
    pub kind: GuideSectionKind,
    /// "Formal email".
    pub title: String,
    /// "Clients and new contacts".
    pub subtitle: String,
    /// "Learned from 96 mails".
    pub learned: Option<String>,
    /// Why a context is empty.
    pub note: Option<String>,
    pub rules: Vec<VoiceRuleView>,
    pub examples: Vec<String>,
}

/// The "Try it" sample.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceSampleView {
    pub request: String,
    pub state: VoiceSampleState,
    pub draft: Option<String>,
    /// "Written in your voice (Formal email)".
    pub voice_line: String,
}

/// The voice task while it runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceLearningView {
    pub task_id: String,
    pub title: String,
    /// "Learning from your sources".
    pub line: String,
    pub status: TaskStatus,
}

/// Settings > Voice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoiceView {
    /// The hub has the voice settings. An older hub has none.
    pub available: bool,
    pub enabled: bool,
    /// The connection sources, mail first.
    pub sources: Vec<VoiceSourceRow>,
    /// The documents that the user picked.
    pub documents: Vec<String>,
    pub learn_from_edits: bool,
    /// "Learned from 214 sent mails and 380 Slack messages on Oct 30".
    pub learned_line: Option<String>,
    /// "Gmail, chris@... (sent mail, last 90 days) and Slack, Work (public
    /// channels). Your edits to drafts also teach it."
    pub sources_line: String,
    pub learning: Option<VoiceLearningView>,
    /// The style guide in sections, when `voice.md` is loaded.
    pub guide: Vec<GuideSectionView>,
    /// "The note voice.md, saved on Oct 30."
    pub guide_caption: String,
    /// The text and hash of `voice.md`, for Edit as markdown.
    pub note: Option<(String, String)>,
    pub disclosure: Disclosure,
    pub sample: Option<VoiceSampleView>,
    /// The style guide that waits for Do it.
    pub proposal: Option<String>,
    pub error: Option<String>,
}

impl VoiceView {
    /// Whether "Turn on and learn" can start: at least one source is
    /// ticked.
    pub fn can_turn_on(&self) -> bool {
        self.sources.iter().any(|s| s.ticked) || !self.documents.is_empty()
    }

    /// The sources as the hub takes them.
    pub fn source_list(&self) -> Vec<VoiceSource> {
        let mut out: Vec<VoiceSource> = self
            .sources
            .iter()
            .filter(|s| s.ticked)
            .filter_map(|s| {
                Some(VoiceSource {
                    kind: s.kind?,
                    connection_id: s.connection_id.parse().ok(),
                    include_private: s.include_private,
                    paths: Vec::new(),
                })
            })
            .collect();
        if !self.documents.is_empty() {
            out.push(VoiceSource {
                kind: VoiceSourceKind::Documents,
                connection_id: None,
                include_private: false,
                paths: self.documents.clone(),
            });
        }
        out
    }
}

/// "Sent mail, last 90 days" or "Messages that you wrote."
fn source_detail(kind: VoiceSourceKind) -> &'static str {
    match kind {
        VoiceSourceKind::Mail => "Sent mail, last 90 days",
        VoiceSourceKind::Chat => "Messages that you wrote.",
        VoiceSourceKind::Documents => "Documents that you pick.",
    }
}

/// The source words for the learned line: "Gmail, chris@... (sent mail,
/// last 90 days)".
fn source_phrase(row: &VoiceSourceRow) -> String {
    match row.kind {
        Some(VoiceSourceKind::Mail) => format!("{} (sent mail, last 90 days)", row.title),
        Some(VoiceSourceKind::Chat) if row.include_private => {
            format!("{} (channels and direct messages)", row.title)
        }
        Some(VoiceSourceKind::Chat) => format!("{} (public channels)", row.title),
        _ => row.title.clone(),
    }
}

/// "Oct 30" in this computer's time zone.
pub fn short_date(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        .with_timezone(&Local)
        .format("%b %-d")
        .to_string()
}

/// The source rows: one per mail or chat connection. With no source chosen
/// yet, mail and the public chat channels are ticked, as the plan's
/// defaults say.
fn source_rows(cache: &Cache, settings: &VoiceSettings) -> Vec<VoiceSourceRow> {
    let fresh = settings.sources.is_empty() && !settings.enabled && settings.task_id.is_none();
    let mut rows: Vec<VoiceSourceRow> = cache
        .connections
        .values()
        .filter_map(|c| {
            let kind = VoiceSourceKind::for_service(&c.service)?;
            let chosen = settings
                .sources
                .iter()
                .find(|s| s.connection_id == Some(c.id));
            let who = c
                .identity
                .clone()
                .filter(|i| !i.trim().is_empty())
                .unwrap_or_else(|| {
                    c.name
                        .strip_prefix(&c.service_name)
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .unwrap_or(&c.name)
                        .to_owned()
                });
            Some(VoiceSourceRow {
                connection_id: c.id.to_string(),
                kind: Some(kind),
                monogram: c
                    .service_name
                    .chars()
                    .next()
                    .map(|ch| ch.to_uppercase().to_string())
                    .unwrap_or_default(),
                title: format!("{}, {who}", c.service_name),
                detail: source_detail(kind).to_owned(),
                ticked: chosen.is_some() || fresh,
                include_private: chosen.is_some_and(|s| s.include_private),
            })
        })
        .collect();
    rows.sort_by_key(|r| {
        (
            r.kind != Some(VoiceSourceKind::Mail),
            r.title.to_lowercase(),
        )
    });
    rows
}

/// The style guide in sections: Core, the four contexts, Hard rules.
pub fn guide_sections(guide: &VoiceGuide) -> Vec<GuideSectionView> {
    let rules = |rules: &[VoiceRule]| -> Vec<VoiceRuleView> {
        rules
            .iter()
            .map(|r| VoiceRuleView {
                text: r.text.clone(),
                by_user: r.by_user,
            })
            .collect()
    };
    let mut out = vec![GuideSectionView {
        key: model::CORE_CONTEXT.to_owned(),
        kind: GuideSectionKind::Core,
        title: "Core rules".to_owned(),
        subtitle: "Apply to every draft".to_owned(),
        rules: rules(&guide.core),
        ..Default::default()
    }];
    for section in &guide.contexts {
        let note = if section.is_empty() {
            Some(section.note.clone().unwrap_or_else(|| {
                "Not enough text yet. Add documents to learn this context.".to_owned()
            }))
        } else {
            section.note.clone()
        };
        out.push(GuideSectionView {
            key: section.key.clone(),
            kind: GuideSectionKind::Context,
            title: context_label(&section.key),
            subtitle: context_description(&section.key).to_owned(),
            learned: section
                .learned_from
                .as_ref()
                .map(|l| format!("Learned from {l}")),
            note,
            rules: rules(&section.rules),
            examples: section.examples.clone(),
        });
    }
    out.push(GuideSectionView {
        key: HARD_RULES.to_owned(),
        kind: GuideSectionKind::Hard,
        title: "Hard rules".to_owned(),
        subtitle: "Consort checks every draft against these and marks a draft that breaks one."
            .to_owned(),
        rules: rules(&guide.hard_rules),
        ..Default::default()
    });
    out
}

/// Settings > Voice from the cache.
pub fn voice_view(cache: &Cache) -> VoiceView {
    let Some(settings) = cache.voice.settings.clone() else {
        return VoiceView::default();
    };
    let sources = source_rows(cache, &settings);
    let documents: Vec<String> = settings
        .sources
        .iter()
        .filter(|s| s.kind == VoiceSourceKind::Documents)
        .flat_map(|s| s.paths.clone())
        .collect();
    let note = cache
        .memory_notes
        .get(&memory_note_entity_id(MemoryScope::Global, VOICE_NOTE_PATH))
        .filter(|n| n.has_content && n.note.deleted_at.is_none())
        .map(|n| n.note.clone());
    let learned_line = settings.last_learned_at.map(|at| {
        let summary = settings
            .learned_summary
            .clone()
            .unwrap_or_else(|| "your sources".to_owned());
        format!("Learned from {summary} on {}", short_date(at.as_millis()))
    });
    let chosen: Vec<String> = sources
        .iter()
        .filter(|s| s.ticked)
        .map(source_phrase)
        .collect();
    let mut sources_line = if chosen.is_empty() {
        String::new()
    } else {
        format!("{}.", crate::connectors::join_and(&chosen))
    };
    if settings.learn_from_edits {
        if !sources_line.is_empty() {
            sources_line.push(' ');
        }
        sources_line.push_str("Your edits to drafts also teach it.");
    }
    let learning = settings.task_id.and_then(|id| {
        let task = cache.tasks.get(&id.to_string())?;
        matches!(
            task.status,
            ProtoStatus::UpNext | ProtoStatus::Working | ProtoStatus::NeedsInput
        )
        .then(|| VoiceLearningView {
            task_id: task.id.to_string(),
            title: task.title.clone(),
            line: "Learning from your sources".to_owned(),
            status: crate::reduce::task_status(task.status),
        })
    });
    let sample = cache.voice.sample.as_ref().map(|s| VoiceSampleView {
        request: s.request.clone(),
        state: if s.id == settings.sample_id {
            settings.sample_state.unwrap_or(s.state)
        } else {
            s.state
        },
        draft: s.draft.clone(),
        voice_line: voice_line(Some(s.context.as_deref().unwrap_or(model::CORE_CONTEXT))),
    });
    VoiceView {
        available: true,
        enabled: settings.enabled,
        sources,
        documents,
        learn_from_edits: settings.learn_from_edits,
        learned_line,
        sources_line,
        learning,
        guide: note
            .as_ref()
            .map(|n| guide_sections(&VoiceGuide::parse(&n.content)))
            .unwrap_or_default(),
        guide_caption: note
            .as_ref()
            .map(|n| {
                format!(
                    "The note voice.md, saved on {}. A rule with (you) is one that you wrote. A learned change never replaces it.",
                    short_date(n.updated_at.as_millis())
                )
            })
            .unwrap_or_default(),
        note: note.map(|n| (n.content, n.sha256.to_string())),
        disclosure: settings.disclosure,
        sample,
        proposal: cache.voice.proposal.clone(),
        error: cache.voice.error.clone(),
    }
}

// The draft card ---------------------------------------------------------------

/// A phrase that the user must check, on a draft card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlagView {
    /// The field label, such as "Body".
    pub field: String,
    pub text: String,
    /// "Date", "Agreement".
    pub kind_label: String,
    /// "Not in your request. Check it."
    pub note: String,
    /// Byte offsets in the field value.
    pub start: usize,
    pub end: usize,
}

impl FlagView {
    /// The line under the text of the draft: the phrase and why it is
    /// marked. "Thursday at 9 AM is not in your request. Check it."
    pub fn line(&self) -> String {
        match self.note.as_str() {
            "Not in your request. Check it." => {
                format!("{} is not in your request. Check it.", self.text)
            }
            note => format!("{}. {note}", self.text.trim_end_matches('.')),
        }
    }
}

/// A text field that Edit changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditableFieldView {
    /// The argument key, such as `body`.
    pub key: String,
    /// "Body".
    pub label: String,
    pub value: String,
}

/// The voice parts of the card of a draft or send tool.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftCardView {
    /// The tool saves a draft, not a send.
    pub draft_only: bool,
    /// "Written in your voice (Formal email)" or "Neutral style".
    pub voice_line: String,
    pub in_voice: bool,
    pub flags: Vec<FlagView>,
    pub editable: Vec<EditableFieldView>,
    /// The user changed the text.
    pub edited: bool,
    /// The user said "Not how I write".
    pub not_my_voice: bool,
    /// The hash of the arguments and flags that the user checks. A tick
    /// holds only for this hash, so a new scan clears it.
    pub review_hash: String,
    /// The subject, or "Email to <name>" when there is none. See
    /// [`crate::drafts::draft_title`].
    pub title: String,
    /// The value of the To field, when the tool has one.
    pub to: Option<String>,
    /// "Sends with Gmail Work", or "Saves a draft in Gmail Work".
    pub connection_line: String,
    /// The subject, when the tool has one.
    pub subject: Option<String>,
    /// The text of the message, as it is now (after any edit).
    pub body: String,
    /// The words of the answer that runs the tool: "Send", or "Save draft"
    /// for a tool that saves a draft.
    pub allow_label: String,
    /// The words of the other answer: "Not now".
    pub deny_label: String,
    /// What happened, after the card closes. `None` while it waits.
    pub outcome: Option<crate::drafts::DraftOutcomeView>,
}

impl DraftCardView {
    /// Whether an allow button is off: there are flags and the box is not
    /// ticked.
    pub fn allow_blocked(&self, checked: bool) -> bool {
        !self.flags.is_empty() && !checked
    }

    /// The words of the tick that takes the place of the hint.
    pub fn check_label(&self) -> &'static str {
        if self.flags.len() > 1 {
            "I checked the marked lines"
        } else {
            "I checked the marked line"
        }
    }

    /// The flag ranges inside one field, by its label.
    pub fn ranges(&self, label: &str) -> Vec<(usize, usize)> {
        self.flags
            .iter()
            .filter(|f| f.field == label)
            .map(|f| (f.start, f.end))
            .collect()
    }
}

/// "Written in your voice (Formal email)", or "Neutral style" for `None`.
pub fn voice_line(context: Option<&str>) -> String {
    match context {
        Some(context) => model::VoiceUse {
            context: context.to_owned(),
        }
        .line(),
        None => model::NEUTRAL_STYLE_LINE.to_owned(),
    }
}

/// The voice parts of a connector card. `None` for a tool that does not
/// draft or send.
pub fn draft_card(card: &ConnectorCard, status: model::ApprovalStatus) -> Option<DraftCardView> {
    let outgoing = card.outgoing?;
    let (to, subject) = crate::drafts::card_to_and_subject(card);
    let label_of = |key: &str| {
        card.fields
            .iter()
            .find(|f| f.label.eq_ignore_ascii_case(key) || (key != "subject" && f.label == "Body"))
            .map(|f| f.label.clone())
            .unwrap_or_else(|| {
                let mut chars = key.replace('_', " ").chars().collect::<Vec<_>>();
                if let Some(first) = chars.first_mut() {
                    *first = first.to_ascii_uppercase();
                }
                chars.into_iter().collect()
            })
    };
    Some(DraftCardView {
        draft_only: outgoing == OutgoingKind::Draft,
        voice_line: voice_line(card.voice.as_ref().map(|v| v.context.as_str())),
        in_voice: card.voice.is_some(),
        flags: card
            .flags
            .iter()
            .map(|f| FlagView {
                field: f.label.clone(),
                text: f.text.clone(),
                kind_label: f.kind.label().to_owned(),
                note: f.note(),
                start: f.start,
                end: f.end,
            })
            .collect(),
        editable: card
            .editable
            .iter()
            .filter_map(|key| {
                Some(EditableFieldView {
                    key: key.clone(),
                    label: label_of(key),
                    value: card.arguments.get(key)?.as_str()?.to_owned(),
                })
            })
            .collect(),
        edited: card.edited(),
        not_my_voice: card.not_my_voice,
        review_hash: card.review_hash(),
        title: crate::drafts::draft_title(subject.as_deref(), to.as_deref()),
        to,
        connection_line: match outgoing {
            OutgoingKind::Send => format!("Sends with {}", card.connection_name),
            OutgoingKind::Draft => format!("Saves a draft in {}", card.connection_name),
        },
        subject,
        body: crate::drafts::card_body(card),
        allow_label: match outgoing {
            OutgoingKind::Send => "Send".to_owned(),
            OutgoingKind::Draft => "Save draft".to_owned(),
        },
        deny_label: "Not now".to_owned(),
        outcome: crate::drafts::outcome(card, status),
    })
}

/// The steps of the voice task's card: the lines of its review summary
/// that start with "- ", and the rest as its summary.
pub fn voice_task_steps(summary: &str) -> (String, Vec<String>) {
    let mut steps = Vec::new();
    let mut rest = Vec::new();
    for line in summary.lines() {
        match line.trim().strip_prefix("- ") {
            Some(step) => steps.push(step.trim().to_owned()),
            None if !line.trim().is_empty() => rest.push(line.trim().to_owned()),
            None => {}
        }
    }
    (rest.join(" "), steps)
}

/// The note at the end of the voice task's card.
pub const VOICE_TASK_NOTE: &str =
    "Raw text stays in this task. Consort keeps only the style guide, after you agree.";

#[cfg(test)]
mod tests;
