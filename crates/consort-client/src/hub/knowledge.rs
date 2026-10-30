//! Routines and memory on the hub backend: what each routine and memory
//! [`Command`] asks of the hub, and how the answers go into the cache.

use consort_proto::{
    client_api::{
        MemoryDelete, MemoryHistory, MemoryList, MemoryMove, MemoryRead, MemoryRestore,
        MemoryWrite, RoutineCreate, RoutineDelete, RoutineRunNow, RoutineRuns, RoutineUpdate,
    },
    model::{ExpectedHash, MemoryNote, MemoryScope, PlacementRequest, RoutineAction},
};

use super::{Shared, commands::parse_id, invalid};
use crate::{ClientError, Command, CommandOutput, routines::RoutineForm};

/// How many runs the run history asks for. The view shows the last 30 days
/// of them.
const RUNS_LIMIT: u32 = 100;

fn scope(text: &str) -> Result<MemoryScope, ClientError> {
    text.parse()
        .map_err(|_| invalid("Consort cannot use this memory, because its scope is not valid. Restart Consort, then try again."))
}

fn hash(text: &str) -> Result<consort_proto::model::Sha256Hash, ClientError> {
    text.parse()
        .map_err(|_| invalid("Consort cannot use this note, because its hash is not valid. Open the note again, then try again."))
}

/// The action of a routine form. A `start_task` routine keeps the placement
/// that it had.
fn action(form: &RoutineForm, placement: PlacementRequest) -> RoutineAction {
    if form.starts_task {
        RoutineAction::StartTask {
            title: form.title.trim().to_owned(),
            model_selection: super::commands::selection(&form.selection),
            placement,
        }
    } else {
        RoutineAction::CoordinatorTurn
    }
}

impl Shared {
    pub(super) fn seq(&self) -> u64 {
        self.cache.lock().map(|c| c.seq).unwrap_or(0)
    }

    fn keep_note(&self, note: MemoryNote, asked_at: u64) -> crate::memory::MemoryNoteView {
        let Ok(mut cache) = self.cache.lock() else {
            return crate::memory::MemoryNoteView::default();
        };
        let view = crate::reduce::memory_note_view(&cache, &note, true);
        cache.merge_memory_note(note, asked_at);
        drop(cache);
        self.publish();
        view
    }
}

/// Runs a routine or memory command.
pub(crate) async fn run(shared: &Shared, command: Command) -> Result<CommandOutput, ClientError> {
    match command {
        Command::SaveRoutine { routine_id, form } => {
            let now = crate::format::now_ms();
            if let Some(error) = form.errors(now).into_iter().next() {
                return Err(invalid(&error.message));
            }
            let schedule = form
                .schedule
                .schedule(now)
                .map_err(|errors| invalid(&errors[0].message))?;
            let channel_id = parse_id(&form.channel_id, "channel")?;
            let routine = match routine_id {
                None => {
                    shared
                        .call(RoutineCreate {
                            channel_id,
                            name: form.name.trim().to_owned(),
                            schedule,
                            prompt: form.prompt.clone(),
                            action: action(&form, PlacementRequest::default()),
                            enabled: form.enabled,
                        })
                        .await?
                        .routine
                }
                Some(id) => {
                    let placement = shared
                        .cache
                        .lock()
                        .ok()
                        .and_then(|c| c.routines.get(&id).map(|r| r.action.clone()))
                        .and_then(|a| match a {
                            RoutineAction::StartTask { placement, .. } => Some(placement),
                            RoutineAction::CoordinatorTurn => None,
                        })
                        .unwrap_or_default();
                    shared
                        .call(RoutineUpdate {
                            routine_id: parse_id(&id, "routine")?,
                            channel_id: Some(channel_id),
                            name: Some(form.name.trim().to_owned()),
                            schedule: Some(schedule),
                            prompt: Some(form.prompt.clone()),
                            action: Some(action(&form, placement)),
                            enabled: Some(form.enabled),
                        })
                        .await?
                        .routine
                }
            };
            let id = routine.id.to_string();
            if let Ok(mut cache) = shared.cache.lock() {
                cache.routines.insert(id.clone(), routine);
            }
            shared.publish();
            return Ok(CommandOutput::RoutineSaved(id));
        }
        Command::SetRoutineEnabled {
            routine_id,
            enabled,
        } => {
            let routine = shared
                .call(RoutineUpdate {
                    routine_id: parse_id(&routine_id, "routine")?,
                    channel_id: None,
                    name: None,
                    schedule: None,
                    prompt: None,
                    action: None,
                    enabled: Some(enabled),
                })
                .await?
                .routine;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.routines.insert(routine_id, routine);
            }
            shared.publish();
        }
        Command::DeleteRoutine { routine_id } => {
            shared
                .call(RoutineDelete {
                    routine_id: parse_id(&routine_id, "routine")?,
                })
                .await?;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.routines.remove(&routine_id);
                cache.routine_runs.remove(&routine_id);
                cache.runs_loaded.remove(&routine_id);
            }
            shared.publish();
        }
        Command::RunRoutineNow { routine_id } => {
            shared
                .call(RoutineRunNow {
                    routine_id: parse_id(&routine_id, "routine")?,
                })
                .await?;
        }
        Command::LoadRoutineRuns { routine_id } => {
            let asked_at = shared.seq();
            let runs = shared
                .call(RoutineRuns {
                    routine_id: parse_id(&routine_id, "routine")?,
                    limit: Some(RUNS_LIMIT),
                })
                .await?
                .runs;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.load_routine_runs(&routine_id, runs, asked_at);
            }
            shared.publish();
        }
        Command::LoadMemory {
            scope: text,
            deleted,
        } => {
            let asked_at = shared.seq();
            let notes = shared
                .call(MemoryList {
                    scope: Some(scope(&text)?),
                    deleted,
                })
                .await?
                .notes;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.load_memory(&text, notes, deleted, asked_at);
            }
            shared.publish();
        }
        Command::ReadMemoryNote { scope: text, path } => {
            let asked_at = shared.seq();
            let note = shared
                .call(MemoryRead {
                    scope: scope(&text)?,
                    path,
                })
                .await?
                .note;
            return Ok(CommandOutput::MemoryNote(shared.keep_note(note, asked_at)));
        }
        Command::WriteMemoryNote {
            scope: text,
            path,
            content,
            expected_sha256,
        } => {
            let path = crate::memory::note_path(&path).map_err(|m| invalid(&m))?;
            let expected = match expected_sha256 {
                Some(sha) => ExpectedHash::Sha256(hash(&sha)?),
                None => ExpectedHash::NotExists,
            };
            let asked_at = shared.seq();
            let note = shared
                .call(MemoryWrite {
                    scope: scope(&text)?,
                    path,
                    content,
                    expected_sha256: expected,
                })
                .await?
                .note;
            return Ok(CommandOutput::MemoryNote(shared.keep_note(note, asked_at)));
        }
        Command::DeleteMemoryNote {
            scope: text,
            path,
            sha256,
        } => {
            shared
                .call(MemoryDelete {
                    scope: scope(&text)?,
                    path,
                    expected_sha256: hash(&sha256)?,
                })
                .await?;
            // The feed brings the note with its deletion time.
        }
        Command::RestoreMemoryNote { scope: text, path } => {
            let asked_at = shared.seq();
            let note = shared
                .call(MemoryRestore {
                    scope: scope(&text)?,
                    path,
                })
                .await?
                .note;
            shared.keep_note(note, asked_at);
        }
        Command::MoveMemoryNote {
            scope: text,
            from_path,
            to_path,
            sha256,
        } => {
            let to_path = crate::memory::note_path(&to_path).map_err(|m| invalid(&m))?;
            let asked_at = shared.seq();
            let note = shared
                .call(MemoryMove {
                    scope: scope(&text)?,
                    from_path: from_path.clone(),
                    to_path,
                    expected_sha256: hash(&sha256)?,
                })
                .await?
                .note;
            if let Ok(mut cache) = shared.cache.lock() {
                cache.memory_notes.remove(&format!("{text}:{from_path}"));
            }
            return Ok(CommandOutput::MemoryNote(shared.keep_note(note, asked_at)));
        }
        Command::LoadMemoryHistory { scope: text, path } => {
            let versions = shared
                .call(MemoryHistory {
                    scope: scope(&text)?,
                    path,
                })
                .await?
                .versions;
            let Ok(cache) = shared.cache.lock() else {
                return Ok(CommandOutput::MemoryHistory(Vec::new()));
            };
            return Ok(CommandOutput::MemoryHistory(
                versions
                    .iter()
                    .map(|v| crate::reduce::memory_note_view(&cache, v, true))
                    .collect(),
            ));
        }
        other => {
            return Err(ClientError::Other(format!(
                "Consort cannot run {other:?} here. Restart Consort, then try again"
            )));
        }
    }
    Ok(CommandOutput::Done)
}
