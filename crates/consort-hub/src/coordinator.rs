//! Coordinators: one per channel, one turn at a time.
//!
//! A wake event goes into the `wake_events` table. [`Hub::kick_coordinator`]
//! then calls [`Hub::run_coordinator`]. That function takes a per-channel lock
//! and does nothing while a turn is open. It takes all queued events,
//! renders them with `render_events` and sends them as one turn. When the
//! channel has no healthy session, it starts one through placement. The
//! turn ends when the worker sends `turn.ended` (see `link::events`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use consort_proto::ids::{ChannelId, SessionId};
use consort_proto::model::{
    Channel, FileRef, Isolation, PlacementRequest, Session, SessionRole, SessionState, Usage,
};
use consort_proto::turn_input::{WakeEvent, render_events};
use consort_proto::worker_link::{
    HubToWorker, SessionStart, SessionStop, SessionTurn, TurnInput, WorkspaceSpec,
};

use crate::error::Result;
use crate::hub::Hub;
use crate::placement::{PlacementInput, Subject, place};
use crate::prompts;
use crate::store::coordinator::{self as coord_store, CoordinatorRow};
use crate::store::work::{self, SessionRuntime};
use crate::store::{conversation, knowledge, machine};

const CONTEXT_MESSAGES: usize = 50;

#[derive(Default)]
pub struct Coordinators {
    /// Unit tests of other modules turn the coordinator off, so a queued wake
    /// event stays queued and no notice appears.
    disabled: std::sync::atomic::AtomicBool,
    locks: Mutex<HashMap<ChannelId, Arc<tokio::sync::Mutex<()>>>>,
    /// The last notice per channel, so the hub does not post the same one twice.
    notices: Mutex<HashMap<ChannelId, String>>,
}

impl Coordinators {
    /// Starts a background run for a channel. It returns at once.
    pub(crate) fn kick(&self, hub: &Hub, channel_id: ChannelId) {
        if self.disabled.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let hub = hub.clone();
        tokio::spawn(async move {
            if let Err(err) = hub.run_coordinator(channel_id).await {
                tracing::warn!(error = %err, channel_id = %channel_id, "could not run a coordinator");
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn set_disabled(&self, disabled: bool) {
        self.disabled
            .store(disabled, std::sync::atomic::Ordering::Relaxed);
    }

    fn lock_for(&self, channel_id: ChannelId) -> Arc<tokio::sync::Mutex<()>> {
        let Ok(mut locks) = self.locks.lock() else {
            return Arc::new(tokio::sync::Mutex::new(()));
        };
        locks.entry(channel_id).or_default().clone()
    }

    /// True when `text` differs from the last notice for the channel.
    fn is_new_notice(&self, channel_id: ChannelId, text: &str) -> bool {
        let Ok(mut notices) = self.notices.lock() else {
            return false;
        };
        if notices.get(&channel_id).is_some_and(|last| last == text) {
            return false;
        }
        notices.insert(channel_id, text.to_owned());
        true
    }

    fn clear_notice(&self, channel_id: ChannelId) {
        if let Ok(mut notices) = self.notices.lock() {
            notices.remove(&channel_id);
        }
    }
}

/// A hash of what the system prompt of a coordinator says about its channel.
/// When it changes, the hub replaces the session, so the new purpose,
/// instructions or model reach the coordinator.
fn prompt_fingerprint(channel: &Channel, everything: &Channel) -> String {
    let selection = channel.effective_coordinator_selection(everything);
    let text = format!(
        "{}\n{}\n{}\n{}",
        channel.name,
        channel.purpose,
        channel.instructions,
        serde_json::to_string(&selection).unwrap_or_default()
    );
    crate::util::sha256(text.as_bytes()).to_string()
}

struct Inputs {
    channel: Channel,
    everything: Channel,
    events: Vec<(i64, WakeEvent)>,
    session: Option<Session>,
    workers: Vec<consort_proto::model::Worker>,
    open: HashMap<consort_proto::ids::WorkerId, u32>,
    global_index: String,
    channel_index: String,
    context: String,
    files: Vec<FileRef>,
}

impl Hub {
    /// The memory index text of the global scope and of one channel.
    pub(crate) async fn memory_indexes(&self, channel: &Channel) -> Result<(String, String)> {
        let scope = channel.memory_scope();
        let is_everything = channel.is_everything();
        self.store
            .read(move |conn| {
                let global =
                    knowledge::memory_notes(conn, consort_proto::model::MemoryScope::Global)?;
                let own = if is_everything {
                    Vec::new()
                } else {
                    knowledge::memory_notes(conn, scope)?
                };
                Ok((
                    prompts::memory_index_lines(&global),
                    prompts::memory_index_lines(&own),
                ))
            })
            .await
    }

    /// Sends the queued wake events of a channel as one turn, if no turn
    /// runs and a worker can take it.
    pub(crate) async fn run_coordinator(&self, channel_id: ChannelId) -> Result<()> {
        let lock = self.coordinators.lock_for(channel_id);
        let _guard = lock.lock().await;

        let Some(inputs) = self.read_coordinator_inputs(channel_id).await? else {
            return Ok(());
        };
        let event_ids: Vec<i64> = inputs.events.iter().map(|(id, _)| *id).collect();
        let events: Vec<WakeEvent> = inputs.events.iter().map(|(_, e)| e.clone()).collect();
        let rendered = render_events(&events);

        let fingerprint = prompt_fingerprint(&inputs.channel, &inputs.everything);
        let mut inputs = inputs;
        if let Some(session) = inputs.session.clone() {
            let current = self
                .store
                .read(move |conn| work::session_runtime(conn, session.id))
                .await?;
            if current.prompt_fingerprint.as_deref() != Some(fingerprint.as_str()) {
                // The settings of the channel changed. Retire this session.
                self.retire_coordinator_session(channel_id, session.id)
                    .await?;
                inputs.session = None;
                let (workers, open) = self
                    .store
                    .read(|conn| Ok((machine::workers(conn)?, work::open_session_counts(conn)?)))
                    .await?;
                inputs.workers = workers;
                inputs.open = open;
                let (context, files) = self.fresh_context(channel_id).await?;
                inputs.context = context;
                inputs.files = files;
                let (global, own) = self.memory_indexes(&inputs.channel).await?;
                inputs.global_index = global;
                inputs.channel_index = own;
            }
        }
        if let Some(session) = inputs.session.clone() {
            let worker_id = session.worker_id;
            let started = self
                .store
                .write(move |tx| {
                    let row = coord_store::coordinator(tx.db(), channel_id)?;
                    if row.turn_open || row.session_id != Some(session.id) {
                        return Ok(false);
                    }
                    tx.delete_wakes(&event_ids)?;
                    tx.save_coordinator(&CoordinatorRow {
                        channel_id,
                        session_id: Some(session.id),
                        turn_open: true,
                        turn_started_at: Some(tx.now()),
                        turn_events: events,
                    })?;
                    tx.save_session_runtime(
                        session.id,
                        &SessionRuntime {
                            turn_id: None,
                            turn_open: true,
                            prompt_fingerprint: Some(fingerprint),
                            ..SessionRuntime::default()
                        },
                    )?;
                    tx.queue_command(
                        session.worker_id,
                        &HubToWorker::SessionTurn(SessionTurn {
                            session_id: session.id,
                            turn: TurnInput {
                                text: rendered,
                                attachments: Vec::new(),
                            },
                        }),
                    )?;
                    Ok(true)
                })
                .await?;
            if started {
                self.pump_worker(worker_id);
            }
            return Ok(());
        }

        // No healthy session: place a new one.
        let selection = inputs
            .channel
            .effective_coordinator_selection(&inputs.everything)
            .cloned();
        let request = PlacementRequest::default();
        let placement = place(&PlacementInput {
            subject: Subject::Coordinator,
            workers: &inputs.workers,
            open_sessions: &inputs.open,
            selection: selection.as_ref(),
            request: &request,
            approval_mode: inputs.channel.default_approval_mode,
            preferred_workers: &inputs.channel.preferred_workers,
            repos: &[],
            isolation: Isolation::Worktree,
        });
        let placement = match placement {
            Ok(placement) => placement,
            Err(err) => {
                return self
                    .coordinator_cannot_start(&inputs.channel, &err.reason())
                    .await;
            }
        };
        self.coordinators.clear_notice(channel_id);

        let system_prompt = prompts::coordinator_system_prompt(
            &inputs.channel,
            &inputs.global_index,
            &inputs.channel_index,
        );
        let first_turn = format!("{}\n\n{rendered}", inputs.context);
        let worker_id = placement.worker_id;
        let files = inputs.files.clone();
        let channel = inputs.channel.clone();
        let started = self
            .store
            .write(move |tx| {
                let row = coord_store::coordinator(tx.db(), channel_id)?;
                if row.turn_open {
                    return Ok(false);
                }
                let session = Session {
                    id: SessionId::new(),
                    role: SessionRole::Coordinator,
                    channel_id,
                    task_id: None,
                    worker_id,
                    account_id: placement.account_id,
                    harness: placement.harness.clone(),
                    model: placement.model.clone(),
                    options: placement.options.clone(),
                    workspace_path: String::new(),
                    native_ref: None,
                    state: SessionState::Starting,
                    last_seq: 0,
                    usage: Usage::default(),
                    started_at: tx.now(),
                    ended_at: None,
                    end_reason: None,
                };
                tx.save_session(&session)?;
                tx.save_session_runtime(
                    session.id,
                    &SessionRuntime {
                        turn_open: true,
                        prompt_fingerprint: Some(fingerprint),
                        ..SessionRuntime::default()
                    },
                )?;
                tx.delete_wakes(&event_ids)?;
                tx.save_coordinator(&CoordinatorRow {
                    channel_id,
                    session_id: Some(session.id),
                    turn_open: true,
                    turn_started_at: Some(tx.now()),
                    turn_events: events,
                })?;
                tx.queue_command(
                    worker_id,
                    &HubToWorker::SessionStart(SessionStart {
                        session_id: session.id,
                        role: SessionRole::Coordinator,
                        channel_id,
                        task_id: None,
                        account_id: placement.account_id,
                        model: placement.model,
                        options: placement.options,
                        approval_mode: channel.default_approval_mode,
                        workspace: WorkspaceSpec {
                            repos: Vec::new(),
                            isolation: Isolation::Worktree,
                            carry_uncommitted: false,
                        },
                        system_prompt_append: system_prompt,
                        allowed_local_mcp_servers: Vec::new(),
                        channel_files: files,
                        resume: None,
                        turn: Some(TurnInput {
                            text: first_turn,
                            attachments: Vec::new(),
                        }),
                    }),
                )?;
                Ok(true)
            })
            .await?;
        if started {
            self.pump_worker(worker_id);
        }
        Ok(())
    }

    /// The `<context>` block and the channel files for a new session.
    async fn fresh_context(&self, channel_id: ChannelId) -> Result<(String, Vec<FileRef>)> {
        self.store
            .read(move |conn| {
                let tasks = work::tasks_in_channel(conn, channel_id, false)?;
                let (mut messages, _) =
                    conversation::messages_page(conn, channel_id, None, None, CONTEXT_MESSAGES)?;
                messages.reverse();
                let files = knowledge::channel_files(conn, channel_id, None)?
                    .into_iter()
                    .map(|f| FileRef {
                        path: f.path,
                        sha256: f.blob_sha256,
                    })
                    .collect();
                Ok((prompts::context_pack(&tasks, &messages), files))
            })
            .await
    }

    /// Ends a healthy coordinator session on purpose, for example because the
    /// channel settings changed. The worker stops the session. The next turn
    /// starts a new one.
    async fn retire_coordinator_session(
        &self,
        channel_id: ChannelId,
        session_id: SessionId,
    ) -> Result<()> {
        let worker = self
            .store
            .write(move |tx| {
                let Some(mut session) = work::session(tx.db(), session_id)? else {
                    return Ok(None);
                };
                let mut row = coord_store::coordinator(tx.db(), channel_id)?;
                if row.turn_open || row.session_id != Some(session_id) {
                    return Ok(None);
                }
                row.session_id = None;
                tx.save_coordinator(&row)?;
                if !session.state.is_final() {
                    session.state = SessionState::Ended;
                    session.ended_at = Some(tx.now());
                    session.end_reason = Some("The channel settings changed.".to_owned());
                    tx.save_session(&session)?;
                    tx.queue_command(
                        session.worker_id,
                        &HubToWorker::SessionStop(SessionStop {
                            session_id,
                            reason: "The channel settings changed.".to_owned(),
                        }),
                    )?;
                    return Ok(Some(session.worker_id));
                }
                Ok(None)
            })
            .await?;
        if let Some(worker_id) = worker {
            self.pump_worker(worker_id);
        }
        Ok(())
    }

    async fn read_coordinator_inputs(&self, channel_id: ChannelId) -> Result<Option<Inputs>> {
        let Some(mut partial) = self
            .store
            .read(move |conn| {
                let Some(channel) = conversation::channel(conn, channel_id)? else {
                    return Ok(None);
                };
                if channel.archived_at.is_some() {
                    return Ok(None);
                }
                let row = coord_store::coordinator(conn, channel_id)?;
                if row.turn_open {
                    return Ok(None);
                }
                let events = coord_store::pending_wakes(conn, channel_id)?;
                if events.is_empty() {
                    return Ok(None);
                }
                let session = match row.session_id {
                    Some(id) => work::session(conn, id)?.filter(|s| !s.state.is_final()),
                    None => None,
                };
                let everything = conversation::everything_channel(conn)?;
                let (workers, open) = if session.is_none() {
                    (machine::workers(conn)?, work::open_session_counts(conn)?)
                } else {
                    (Vec::new(), HashMap::new())
                };
                let (context, files) = if session.is_none() {
                    let tasks = work::tasks_in_channel(conn, channel_id, false)?;
                    let (mut messages, _) = conversation::messages_page(
                        conn,
                        channel_id,
                        None,
                        None,
                        CONTEXT_MESSAGES,
                    )?;
                    messages.reverse();
                    let files = knowledge::channel_files(conn, channel_id, None)?
                        .into_iter()
                        .map(|f| FileRef {
                            path: f.path,
                            sha256: f.blob_sha256,
                        })
                        .collect();
                    (prompts::context_pack(&tasks, &messages), files)
                } else {
                    (String::new(), Vec::new())
                };
                Ok(Some(Inputs {
                    channel,
                    everything,
                    events,
                    session,
                    workers,
                    open,
                    global_index: String::new(),
                    channel_index: String::new(),
                    context,
                    files,
                }))
            })
            .await?
        else {
            return Ok(None);
        };
        if partial.session.is_none() {
            let (global, own) = self.memory_indexes(&partial.channel).await?;
            partial.global_index = global;
            partial.channel_index = own;
        }
        Ok(Some(partial))
    }

    /// The coordinator cannot start now. The events stay queued. The hub says
    /// why in the channel, once for each reason.
    async fn coordinator_cannot_start(&self, channel: &Channel, reason: &str) -> Result<()> {
        let text = format!("The assistant cannot answer yet. {reason}");
        if !self.coordinators.is_new_notice(channel.id, &text) {
            return Ok(());
        }
        let channel_id = channel.id;
        self.store
            .write(move |tx| {
                tx.post_notice(channel_id, None, None, "assistant_unavailable", &text)
                    .map(|_| ())
            })
            .await
    }

    /// Closes a coordinator turn that has run too long. Call it from the
    /// maintenance loop. The session is then lost, and the next wake event
    /// starts a new one.
    pub(crate) async fn expire_stuck_turns(&self, max_age_ms: i64) -> Result<()> {
        let stuck = self
            .store
            .read(move |conn| {
                let now = consort_proto::time::Timestamp::now().as_millis();
                let mut out = Vec::new();
                for channel_id in coord_store::channels_with_open_turn(conn)? {
                    let row = coord_store::coordinator(conn, channel_id)?;
                    if row
                        .turn_started_at
                        .is_some_and(|t| now - t.as_millis() > max_age_ms)
                        && let Some(session_id) = row.session_id
                    {
                        out.push(session_id);
                    }
                }
                Ok(out)
            })
            .await?;
        for session_id in stuck {
            tracing::warn!(session_id = %session_id, "a coordinator turn ran for too long, so the hub replaces the session");
            self.mark_session_lost(session_id, "The turn ran for too long.")
                .await?;
        }
        Ok(())
    }
}
