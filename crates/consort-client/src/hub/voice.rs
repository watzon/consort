//! Settings > Voice and the draft card on the hub backend: what each voice
//! [`Command`] asks of the hub, and how the answers go into the cache.

use std::sync::Arc;

use consort_proto::client_api::{
    ApprovalEdit, MemoryRead, MemoryWrite, VoiceFeedback, VoiceGet, VoiceLearn, VoiceReset,
    VoiceResult, VoiceTry,
};
use consort_proto::model::{ExpectedHash, MemoryScope, VOICE_NOTE_PATH, memory_note_entity_id};

use super::{Shared, commands::parse_id};
use crate::voice::apply_guide_edit;
use crate::{ClientError, Command, CommandOutput};

/// An `unsupported` answer in words that say what the hub cannot do yet.
fn older_hub(err: ClientError) -> ClientError {
    match err {
        ClientError::Hub { code, .. } if code == "unsupported" => ClientError::Hub {
            code,
            message: "This hub is older than the app and cannot write in your voice yet. Update the hub, then try again.".to_owned(),
        },
        other => other,
    }
}

/// Keeps the error of a change for the page, and gives it back.
fn note_error(shared: &Arc<Shared>, err: ClientError) -> ClientError {
    let err = older_hub(err);
    if let Ok(mut cache) = shared.cache.lock() {
        cache.voice.error = Some(err.to_string());
    }
    shared.publish();
    err
}

fn take_result(shared: &Arc<Shared>, result: VoiceResult, with_proposal: bool) {
    if let Ok(mut cache) = shared.cache.lock() {
        cache.voice.error = None;
        cache.voice.apply_settings(result.settings);
        if result.sample.is_some() {
            cache.voice.sample = result.sample;
        }
        if with_proposal {
            cache.voice.proposal = result.proposal;
        }
    }
}

/// Reads `voice.md` into the cache. A guide that does not exist is not an
/// error.
async fn load_note(shared: &Arc<Shared>) -> Result<(), ClientError> {
    let asked_at = shared.seq();
    match shared
        .call(MemoryRead {
            scope: MemoryScope::Global,
            path: VOICE_NOTE_PATH.to_owned(),
        })
        .await
    {
        Ok(result) => {
            if let Ok(mut cache) = shared.cache.lock() {
                cache.merge_memory_note(result.note, asked_at);
            }
            Ok(())
        }
        Err(ClientError::Hub { code, .. }) if code == "not_found" => Ok(()),
        Err(err) => Err(err),
    }
}

/// Runs one voice command.
pub(super) async fn run(
    shared: &Arc<Shared>,
    command: Command,
) -> Result<CommandOutput, ClientError> {
    match command {
        Command::LoadVoice => {
            let result = shared
                .call(VoiceGet {})
                .await
                .map_err(|err| note_error(shared, err))?;
            take_result(shared, result, true);
            load_note(shared).await?;
        }
        Command::SetVoice(changes) => {
            let result = shared
                .call(changes.request())
                .await
                .map_err(|err| note_error(shared, err))?;
            take_result(shared, result, false);
        }
        Command::LearnVoice => {
            let result = shared
                .call(VoiceLearn {})
                .await
                .map_err(|err| note_error(shared, err))?;
            take_result(shared, result, false);
        }
        Command::ResetVoice => {
            let result = shared
                .call(VoiceReset {})
                .await
                .map_err(|err| note_error(shared, err))?;
            take_result(shared, result, false);
            if let Ok(mut cache) = shared.cache.lock() {
                cache.voice.sample = None;
            }
        }
        Command::TryVoice { request } => {
            let result = shared
                .call(VoiceTry { request })
                .await
                .map_err(|err| note_error(shared, err))?;
            take_result(shared, result, false);
        }
        Command::EditVoiceGuide(edit) => {
            let current = shared.cache.lock().ok().and_then(|c| {
                c.memory_notes
                    .get(&memory_note_entity_id(MemoryScope::Global, VOICE_NOTE_PATH))
                    .filter(|n| n.has_content && n.note.deleted_at.is_none())
                    .map(|n| n.note.clone())
            });
            let Some(note) = current else {
                return Err(note_error(
                    shared,
                    ClientError::Other(
                        "The style guide is not loaded. Open Settings > Voice again.".to_owned(),
                    ),
                ));
            };
            let content = apply_guide_edit(&note.content, &edit);
            let asked_at = shared.seq();
            let written = shared
                .call(MemoryWrite {
                    scope: MemoryScope::Global,
                    path: VOICE_NOTE_PATH.to_owned(),
                    content,
                    expected_sha256: ExpectedHash::Sha256(note.sha256),
                })
                .await
                .map_err(|err| note_error(shared, err))?;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.voice.error = None;
                cache.merge_memory_note(written.note, asked_at);
            }
        }
        Command::VoiceFeedback { approval_id } => {
            shared
                .call(VoiceFeedback {
                    approval_id: parse_id(&approval_id, "approval")?,
                })
                .await
                .map_err(older_hub)?;
        }
        Command::EditApproval {
            approval_id,
            fields,
        } => {
            let result = shared
                .call(ApprovalEdit {
                    approval_id: parse_id(&approval_id, "approval")?,
                    fields: fields.into_iter().collect(),
                })
                .await
                .map_err(older_hub)?;
            if let Ok(mut cache) = shared.cache.lock() {
                cache
                    .approvals
                    .insert(result.approval.id.to_string(), result.approval);
            }
        }
        other => {
            return Err(ClientError::Other(format!(
                "{other:?} is not a voice command"
            )));
        }
    }
    shared.publish();
    Ok(CommandOutput::Done)
}
