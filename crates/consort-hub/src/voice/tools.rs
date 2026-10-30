//! The hub tools `voice_guide` and `voice_sample`.

use consort_proto::model::{CORE_CONTEXT, VOICE_CONTEXTS, VoiceSampleState};
use consort_proto::tools::{OkResult, VoiceGuideArgs, VoiceGuideResult, VoiceSampleArgs};
use serde_json::Value;

use super::{active_guide, store};
use crate::error::{HubError, Result};
use crate::hub::{Hub, ToolCtx};

/// The most characters in a sample draft.
const SAMPLE_MAX_CHARS: usize = 4_000;

const WORDING_ONLY: &str = "Use this for wording only. Do not add a date, a time, a weekday, a price, a promise or an agreement that the request does not contain. The hub marks any that appear. Use it only for text that goes out in the user's name.";

impl Hub {
    /// `voice_guide`: the rules and examples of one context, after the Core
    /// rules. The hub records the call, so the draft card says which voice
    /// wrote it.
    pub(crate) async fn tool_voice_guide(
        &self,
        ctx: &ToolCtx,
        args: VoiceGuideArgs,
    ) -> Result<Value> {
        let session_id = ctx.session.id;
        let wanted = args
            .context
            .map(|c| c.trim().to_ascii_lowercase())
            .filter(|c| !c.is_empty());
        if let Some(context) = &wanted
            && context != CORE_CONTEXT
            && !VOICE_CONTEXTS.contains(&context.as_str())
        {
            return Err(HubError::invalid(format!(
                "There is no context {context}. Use one of: {}, or leave it out for the Core rules.",
                VOICE_CONTEXTS.join(", ")
            )));
        }
        let result = self
            .store
            .write(move |tx| {
                let Some(guide) = active_guide(tx.db())? else {
                    return Ok(VoiceGuideResult {
                        enabled: false,
                        context: CORE_CONTEXT.to_owned(),
                        guide: String::new(),
                        message: "The voice is off. Write in a neutral, short and polite style."
                            .to_owned(),
                    });
                };
                let section = wanted
                    .as_deref()
                    .and_then(|c| guide.section_text(c).map(|text| (c.to_owned(), text)));
                let (context, text, note) = match section {
                    Some((context, text)) => (context, text, ""),
                    None => {
                        let mut text = String::from("Core rules:\n");
                        for rule in &guide.core {
                            text.push_str("- ");
                            text.push_str(&rule.text);
                            text.push('\n');
                        }
                        let note = if wanted.as_deref().is_some_and(|c| c != CORE_CONTEXT) {
                            " That context has no rules yet, so use the Core rules only."
                        } else {
                            ""
                        };
                        (CORE_CONTEXT.to_owned(), text, note)
                    }
                };
                let mut text = text;
                if !guide.hard_rules.is_empty() {
                    text.push_str("Hard rules (never break them):\n");
                    for rule in &guide.hard_rules {
                        text.push_str("- ");
                        text.push_str(&rule.text);
                        text.push('\n');
                    }
                }
                tx.record_voice_use(session_id, &context)?;
                Ok(VoiceGuideResult {
                    enabled: true,
                    context,
                    guide: text,
                    message: format!("{WORDING_ONLY}{note}"),
                })
            })
            .await?;
        Ok(serde_json::to_value(result)?)
    }

    /// `voice_sample`: the Tutti coordinator returns a "Try it" sample. The
    /// hub keeps it only in memory.
    pub(crate) async fn tool_voice_sample(
        &self,
        _ctx: &ToolCtx,
        args: VoiceSampleArgs,
    ) -> Result<Value> {
        let draft = args.draft.trim().to_owned();
        if draft.is_empty() {
            return Err(HubError::invalid("The sample is empty. Write the draft."));
        }
        let draft: String = draft.chars().take(SAMPLE_MAX_CHARS).collect();
        let current = self.voice.sample();
        let Some(mut sample) = current.filter(|s| s.id == args.sample) else {
            return Err(HubError::invalid(
                "This sample is not the newest one. The user asked for another sample, so write nothing for this one.",
            ));
        };
        let id = sample.id;
        // The text goes to memory first, so a client that reads the settings
        // change finds it.
        sample.state = VoiceSampleState::Ready;
        sample.draft = Some(draft);
        sample.context = args.context.filter(|c| !c.trim().is_empty());
        self.voice.set_sample(Some(sample));
        let newest = self
            .store
            .write(move |tx| {
                let mut settings = store::settings(tx.db())?;
                if settings.sample_id != id {
                    return Ok(false);
                }
                settings.sample_state = Some(VoiceSampleState::Ready);
                tx.save_voice_settings(&settings)?;
                Ok(true)
            })
            .await?;
        if !newest {
            return Err(HubError::invalid(
                "This sample is not the newest one. Write nothing for it.",
            ));
        }
        Ok(serde_json::to_value(OkResult::OK)?)
    }
}
