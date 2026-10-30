//! Write in my voice (ADR 0014, docs/VOICE-PLAN.md).
//!
//! The rules that the hub enforces in code:
//!
//! - The voice is off by default. Turning it on starts the voice task in
//!   Tutti with a fixed brief that lists only the chosen sources
//!   ([`learn`]). Every voice task keeps its mark, so an old one keeps its
//!   limits; Learn again, Turn off and Reset stop the earlier ones. A
//!   voice task can run only read tools of those connections, and its
//!   proposal comes back as an offer card.
//! - The hub owns `voice.md`. It writes the note only after "Do it" on an
//!   offer, or for the user's own edit in Settings > Voice. Sessions cannot
//!   write or delete it; a learned rule change goes through
//!   `propose_voice_rule`. The user's rules are hub data in the settings,
//!   and the hub renders them with the mark `(you)`. Raw text never
//!   becomes memory: every write goes through the style guide format and
//!   the screening of [`consort_proto::model::VoiceGuide`].
//! - Only a session that can reach a connector tool that drafts or sends
//!   gets the voice in its prompt ([`Hub::voice_prompt_section`]).
//! - The card of a draft or send tool says which voice wrote it, has Edit,
//!   and marks commitments that the request did not contain ([`card`],
//!   [`scan`]). An allow needs "I checked the marked lines" for the flags
//!   that the user saw. While the voice is on, every send asks. The voice
//!   never loosens the approval rule.
//! - Edits teach the voice: the hub keeps draft pairs for 90 days and wakes
//!   Tutti after 5 edited pairs of one context ([`edits`]).

use std::sync::Mutex;

use consort_proto::client_api::{
    VOICE_TRY_MAX_CHARS, VoiceFeedback, VoiceGet, VoiceLearn, VoiceReset, VoiceResult, VoiceSet,
    VoiceTry,
};
use consort_proto::frame::Empty;
use consort_proto::model::{
    Channel, Connection, DRAFT_PAIR_KEEP_DAYS, MemoryAuthor, MemoryScope, VOICE_NOTE_MAX_BYTES,
    VOICE_NOTE_PATH, VoiceGuide, VoiceSample, VoiceSampleState, VoiceSettings, VoiceSourceKind,
    mark_user_rules, outgoing_kind,
};
use consort_proto::turn_input::WakeEvent;
use rusqlite::Connection as Db;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::store::{Tx, connectors as con_store, conversation, knowledge, voice as store};

pub(crate) mod card;
pub(crate) mod edits;
pub(crate) mod learn;
pub(crate) mod scan;
#[cfg(test)]
mod tests;
pub(crate) mod tools;

/// How long the hub keeps draft pairs.
pub(crate) const DRAFT_PAIR_KEEP_MS: i64 = DRAFT_PAIR_KEEP_DAYS * 24 * 60 * 60 * 1000;

/// Services that post in the user's name, besides mail and chat.
const SOCIAL_SERVICES: &[&str] = &[
    "linkedin",
    "twitter",
    "x",
    "reddit",
    "mastodon",
    "bluesky",
    "facebook",
    "instagram",
    "threads",
];

/// The voice state that lives only in the hub's memory: the newest "Try
/// it" sample. It is never written to the database or the feed.
#[derive(Default)]
pub struct VoiceState {
    sample: Mutex<Option<VoiceSample>>,
}

impl VoiceState {
    fn sample(&self) -> Option<VoiceSample> {
        self.sample.lock().ok().and_then(|s| s.clone())
    }

    fn set_sample(&self, sample: Option<VoiceSample>) {
        if let Ok(mut slot) = self.sample.lock() {
            *slot = sample;
        }
    }
}

/// The error of a session that writes or deletes `voice.md`.
pub(crate) const VOICE_NOTE_REFUSAL: &str = "voice.md belongs to the hub. Sessions cannot write or delete it. To change a rule after a voice_edits event, call propose_voice_rule with the edits that agree; the user approves it as an offer. To add a rule that the user asked for, tell the user to add it in Settings > Voice.";

/// The style guide, when `voice.md` exists, with the user's rules from the
/// settings. A `(you)` mark in the note that the settings do not have is
/// not the user's.
pub(crate) fn guide(conn: &Db) -> Result<Option<VoiceGuide>> {
    let Some(note) = knowledge::memory_note(conn, MemoryScope::Global, VOICE_NOTE_PATH)?
        .filter(|n| n.deleted_at.is_none())
    else {
        return Ok(None);
    };
    let settings = store::settings(conn)?;
    Ok(Some(
        VoiceGuide::parse(&note.content).with_user_rules(&settings.user_rules),
    ))
}

/// The style guide when the voice is on and the guide says something.
/// `None` means "write in a neutral style".
pub(crate) fn active_guide(conn: &Db) -> Result<Option<VoiceGuide>> {
    if !store::settings(conn)?.enabled {
        return Ok(None);
    }
    Ok(guide(conn)?.filter(|g| !g.is_empty()))
}

/// Checks and normalizes a write of `voice.md` from outside the hub. Only
/// the user may write it: a session gets [`VOICE_NOTE_REFUSAL`]. The
/// user's write marks every new or changed rule as the user's, and the
/// rules with the mark become the user's rules in the settings. The text
/// goes through the format and the screening, and must fit in 6 KiB.
pub(crate) fn screen_note_write(
    tx: &mut Tx<'_>,
    by: MemoryAuthor,
    content: &str,
) -> Result<String> {
    if by != MemoryAuthor::User {
        return Err(crate::proactivity::turn::denied(VOICE_NOTE_REFUSAL));
    }
    let before = guide(tx.db())?.unwrap_or_else(VoiceGuide::empty);
    let after = mark_user_rules(&before, VoiceGuide::parse(content).screened());
    let mut settings = store::settings(tx.db())?;
    let user_rules = after.user_rule_list();
    if settings.user_rules != user_rules {
        settings.user_rules = user_rules;
        tx.save_voice_settings(&settings)?;
    }
    let text = after.render();
    if text.len() > VOICE_NOTE_MAX_BYTES {
        return Err(HubError::invalid(format!(
            "voice.md is larger than {} KiB. Remove examples or rules.",
            VOICE_NOTE_MAX_BYTES / 1024
        )));
    }
    Ok(text)
}

/// Whether a connection can reach a tool that drafts or sends text in the
/// user's name: a mail, chat or social service, or a service whose tool
/// list (from the cache) has such a tool.
fn can_send(connection: &Connection, cached: Option<&[crate::connectors::composio::Tool]>) -> bool {
    let service = connection.service.to_ascii_lowercase();
    if VoiceSourceKind::for_service(&service).is_some()
        || SOCIAL_SERVICES.contains(&service.as_str())
    {
        return true;
    }
    cached.is_some_and(|tools| {
        tools
            .iter()
            .any(|t| outgoing_kind(&t.slug, crate::connectors::tool_kind(t)).is_some())
    })
}

/// The rules for the session prompt, with the guide's Core, hard rules and
/// contexts.
fn prompt_text(guide: &VoiceGuide) -> String {
    format!(
        "\n# The user's voice is on\n\n\
The user turned on \"Write in my voice\". Use it only for text that goes out in the user's name through a connector: mail, chat messages and posts. Never use it for your replies to the user, task briefs, code or commits. Never write in the voice of anyone else.\n\n\
Before you write such a draft, choose the context from the recipient and the channel, and call voice_guide with it. When you are not sure, use the Core rules only.\n\n\
The voice changes only the wording. Never add a date, a time, a weekday, a price, a promise or an agreement that the request does not contain. The hub marks any that appear, and the user checks them before anything goes out. Every send still waits for the user's approval.\n\n{}",
        guide.prompt_block()
    )
}

impl Hub {
    /// The voice part of a session prompt, or `None`: the voice is off,
    /// the guide is empty, or the channel has no connection that can draft
    /// or send.
    pub(crate) async fn voice_prompt_section(&self, channel: &Channel) -> Result<Option<String>> {
        let channel_id = channel.id;
        let found = self
            .store
            .read(move |conn| {
                let Some(guide) = active_guide(conn)? else {
                    return Ok(None);
                };
                let channel = conversation::channel_or_404(conn, channel_id)?;
                let connections = con_store::effective_connections(conn, &channel)?;
                Ok(Some((guide, connections)))
            })
            .await?;
        let Some((guide, connections)) = found else {
            return Ok(None);
        };
        let cache = self.connectors.cached_tools().await;
        let reaches = connections
            .iter()
            .any(|c| can_send(c, cache.get(&c.service).map(Vec::as_slice)));
        Ok(reaches.then(|| prompt_text(&guide)))
    }

    /// Whether a session belongs to a voice task: any task that learned
    /// the voice, also one that a newer voice task replaced.
    pub(crate) async fn is_voice_task(
        &self,
        task: Option<&consort_proto::model::Task>,
    ) -> Result<bool> {
        let Some(task) = task else {
            return Ok(false);
        };
        let id = task.id;
        self.store
            .read(move |conn| store::is_voice_task(conn, id))
            .await
    }

    /// The settings with the sample that the hub holds.
    fn voice_result(&self, settings: VoiceSettings) -> VoiceResult {
        let sample = self
            .voice
            .sample()
            .filter(|s| s.id == settings.sample_id)
            .map(|mut s| {
                if let Some(state) = settings.sample_state {
                    s.state = state;
                }
                s
            });
        VoiceResult {
            settings,
            sample,
            proposal: None,
        }
    }

    pub(crate) async fn voice_get(
        &self,
        _caller: &Caller,
        _request: VoiceGet,
    ) -> Result<VoiceResult> {
        let (settings, proposal) = self
            .store
            .read(|conn| Ok((store::settings(conn)?, store::proposal(conn)?)))
            .await?;
        let mut result = self.voice_result(settings);
        result.proposal = proposal;
        Ok(result)
    }

    pub(crate) async fn voice_set(
        &self,
        _caller: &Caller,
        request: VoiceSet,
    ) -> Result<VoiceResult> {
        if let Some(sources) = &request.sources {
            learn::validate_sources(sources)?;
        }
        let turn_off = request.enabled == Some(false);
        let hub = self.clone();
        let (settings, start) = self
            .store
            .write(move |tx| {
                let before = store::settings(tx.db())?;
                let mut settings = before.clone();
                if let Some(sources) = request.sources {
                    for source in &sources {
                        if let Some(id) = source.connection_id {
                            con_store::connection_or_404(tx.db(), id)?;
                        }
                    }
                    settings.sources = sources;
                }
                if let Some(edits) = request.learn_from_edits {
                    settings.learn_from_edits = edits;
                }
                if let Some(disclosure) = request.disclosure {
                    settings.disclosure = disclosure;
                }
                let mut start = false;
                if let Some(enabled) = request.enabled {
                    if enabled && !before.enabled {
                        if !settings.has_readable_source() {
                            return Err(HubError::invalid(
                                "Tick at least one source, then turn the voice on.",
                            ));
                        }
                        // A guide from before stays in use. With none, the
                        // voice task learns one.
                        start = guide(tx.db())?.is_none_or(|g| g.is_empty());
                    }
                    settings.enabled = enabled;
                }
                if settings != before {
                    tx.save_voice_settings(&settings)?;
                }
                let _ = &hub;
                Ok((settings, start))
            })
            .await?;
        if turn_off {
            self.stop_voice_tasks("The voice was turned off.").await?;
        }
        if start {
            let settings = self.start_voice_task().await?;
            return Ok(self.voice_result(settings));
        }
        Ok(self.voice_result(settings))
    }

    pub(crate) async fn voice_learn(
        &self,
        _caller: &Caller,
        _request: VoiceLearn,
    ) -> Result<VoiceResult> {
        let settings = self.store.read(store::settings).await?;
        if !settings.enabled {
            return Err(HubError::invalid(
                "The voice is off. Turn it on in Settings > Voice first.",
            ));
        }
        if !settings.has_readable_source() {
            return Err(HubError::invalid(
                "Tick at least one source, then learn again.",
            ));
        }
        let settings = self.start_voice_task().await?;
        Ok(self.voice_result(settings))
    }

    pub(crate) async fn voice_reset(
        &self,
        _caller: &Caller,
        _request: VoiceReset,
    ) -> Result<VoiceResult> {
        self.stop_voice_tasks("The voice was reset.").await?;
        let settings = self
            .store
            .write(|tx| {
                if let Some(note) =
                    knowledge::memory_note(tx.db(), MemoryScope::Global, VOICE_NOTE_PATH)?
                        .filter(|n| n.deleted_at.is_none())
                {
                    tx.delete_memory(MemoryScope::Global, VOICE_NOTE_PATH, &note.sha256)?;
                }
                let mut settings = store::settings(tx.db())?;
                learn::close_proposal(tx, &mut settings)?;
                settings.enabled = false;
                settings.last_learned_at = None;
                settings.learned_summary = None;
                settings.user_rules.clear();
                tx.save_voice_settings(&settings)?;
                Ok(settings)
            })
            .await?;
        self.voice.set_sample(None);
        Ok(self.voice_result(settings))
    }

    pub(crate) async fn voice_try(
        &self,
        _caller: &Caller,
        request: VoiceTry,
    ) -> Result<VoiceResult> {
        let text = request.request.trim().to_owned();
        if text.is_empty() {
            return Err(HubError::invalid("Write what the sample should say."));
        }
        if text.chars().count() > VOICE_TRY_MAX_CHARS {
            return Err(HubError::invalid(format!(
                "The request is longer than {VOICE_TRY_MAX_CHARS} characters. Shorten it."
            )));
        }
        let wake_text = text.clone();
        let (settings, tutti) = self
            .store
            .write(move |tx| {
                if active_guide(tx.db())?.is_none() {
                    return Err(HubError::invalid(
                        "Turn the voice on and save a style guide first.",
                    ));
                }
                let mut settings = store::settings(tx.db())?;
                settings.sample_id += 1;
                settings.sample_state = Some(VoiceSampleState::Writing);
                tx.save_voice_settings(&settings)?;
                let tutti = conversation::tutti_channel(tx.db())?;
                tx.queue_wake(
                    tutti.id,
                    &WakeEvent::VoiceSample {
                        sample: settings.sample_id,
                        request: wake_text,
                    },
                )?;
                Ok((settings, tutti.id))
            })
            .await?;
        self.voice.set_sample(Some(VoiceSample {
            id: settings.sample_id,
            request: text,
            state: VoiceSampleState::Writing,
            draft: None,
            context: None,
        }));
        self.kick_coordinator(tutti);
        Ok(self.voice_result(settings))
    }

    pub(crate) async fn voice_feedback(
        &self,
        _caller: &Caller,
        request: VoiceFeedback,
    ) -> Result<Empty> {
        let tutti = self
            .store
            .write(move |tx| card::not_my_voice(tx, request.approval_id))
            .await?;
        if let Some(tutti) = tutti {
            self.kick_coordinator(tutti);
        }
        Ok(Empty {})
    }

    /// Deletes draft pairs older than 90 days. The maintenance loop calls
    /// it.
    pub(crate) async fn purge_voice(&self) -> Result<usize> {
        let before = self.now().plus_millis(-DRAFT_PAIR_KEEP_MS);
        self.store.write(move |tx| tx.purge_voice(before)).await
    }
}

/// At the end of a coordinator turn: a sample that the turn did not write
/// failed.
pub(crate) fn turn_ended(tx: &mut Tx<'_>, events: &[WakeEvent]) -> Result<()> {
    let samples: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            WakeEvent::VoiceSample { sample, .. } => Some(*sample),
            _ => None,
        })
        .collect();
    if samples.is_empty() {
        return Ok(());
    }
    let mut settings = store::settings(tx.db())?;
    if samples.contains(&settings.sample_id)
        && settings.sample_state == Some(VoiceSampleState::Writing)
    {
        settings.sample_state = Some(VoiceSampleState::Failed);
        tx.save_voice_settings(&settings)?;
    }
    Ok(())
}

/// Whether every event of a turn is a voice sample. Such a turn posts no
/// fallback message.
pub(crate) fn only_samples(events: &[WakeEvent]) -> bool {
    !events.is_empty() && events.iter().all(WakeEvent::is_voice_sample)
}
