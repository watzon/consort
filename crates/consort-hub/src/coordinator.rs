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
use std::time::{Duration, Instant};

use consort_proto::ids::{ChannelId, SessionId};
use consort_proto::model::{
    BlobRef, Channel, Isolation, PlacementRequest, Session, SessionRole, SessionState, Usage,
};
use consort_proto::turn_input::{WakeEvent, render_events};
use consort_proto::worker_link::{
    HubToWorker, SessionStart, SessionTurn, TurnInput, WorkspaceSpec,
};

use crate::error::Result;
use crate::hub::Hub;
use crate::limits::{TURN_TEXT_MAX_BYTES, TURN_TEXT_MIN_BYTES, json_len, turn_room};
use crate::link::events::{Effects, queue_stop};
use crate::placement::{PlacementInput, Subject, place};
use crate::prompts;
use crate::store::coordinator::{self as coord_store, CoordinatorRow};
use crate::store::work::{self, SessionRuntime};
use crate::store::{conversation, knowledge, machine};

const CONTEXT_MESSAGES: usize = 50;
/// The wait after the first failed start of a coordinator session.
const START_BACKOFF_FIRST: Duration = Duration::from_secs(10);
/// The longest wait between two starts of a coordinator session.
const START_BACKOFF_MAX: Duration = Duration::from_secs(10 * 60);

/// The wait before the next start after `failures` failed starts in a row:
/// 10 s, then twice as long after each new failure, at most 10 minutes.
pub(crate) fn start_backoff(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let factor = 2_u32.saturating_pow(failures - 1);
    START_BACKOFF_FIRST
        .saturating_mul(factor)
        .min(START_BACKOFF_MAX)
}

/// The failed starts of one channel's coordinator.
#[derive(Debug, Clone, Copy)]
struct StartFailures {
    count: u32,
    last: Instant,
    /// A run is scheduled for the end of the wait.
    retry_scheduled: bool,
}

#[derive(Default)]
pub struct Coordinators {
    /// Unit tests of other modules turn the coordinator off, so a queued wake
    /// event stays queued and no notice appears.
    disabled: std::sync::atomic::AtomicBool,
    locks: Mutex<HashMap<ChannelId, Arc<tokio::sync::Mutex<()>>>>,
    /// The last notice per channel, so the hub does not post the same one twice.
    notices: Mutex<HashMap<ChannelId, String>>,
    /// Failed starts per channel, for the backoff. They live in memory, so
    /// a hub restart tries again at once.
    start_failures: Mutex<HashMap<ChannelId, StartFailures>>,
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

    /// Records a failed start of a coordinator session in a channel.
    pub(crate) fn start_failed(&self, channel_id: ChannelId) {
        if let Ok(mut failures) = self.start_failures.lock() {
            let entry = failures.entry(channel_id).or_insert(StartFailures {
                count: 0,
                last: Instant::now(),
                retry_scheduled: false,
            });
            entry.count = entry.count.saturating_add(1);
            entry.last = Instant::now();
        }
    }

    /// Records that a coordinator session in a channel started.
    pub(crate) fn start_worked(&self, channel_id: ChannelId) {
        if let Ok(mut failures) = self.start_failures.lock() {
            failures.remove(&channel_id);
        }
    }

    /// How long the channel must still wait before it starts a new session.
    /// `None` when it may start now. When it must wait and no run is
    /// scheduled yet, the second value is true: the caller schedules one.
    fn start_wait(&self, channel_id: ChannelId, now: Instant) -> Option<(Duration, bool)> {
        let mut failures = self.start_failures.lock().ok()?;
        let entry = failures.get_mut(&channel_id)?;
        let until = entry.last + start_backoff(entry.count);
        if now >= until {
            return None;
        }
        let schedule = !entry.retry_scheduled;
        entry.retry_scheduled = true;
        Some((until - now, schedule))
    }

    fn retry_ran(&self, channel_id: ChannelId) {
        if let Ok(mut failures) = self.start_failures.lock()
            && let Some(entry) = failures.get_mut(&channel_id)
        {
            entry.retry_scheduled = false;
        }
    }
}

/// The queued events that go into one turn, with their IDs and the turn
/// text. Events go in arrival order while the text takes at most `room`
/// bytes as a JSON string. The other events wait for the next turn, which
/// starts when this one ends. A first event that alone does not fit is cut,
/// with a note that tells the coordinator how to read all of it.
/// The attachments of the user messages in a batch of wake events, in the
/// order of the events. The hub reads them from the stored messages, so a
/// message that the user deleted in the meantime gives none.
fn event_attachments(conn: &rusqlite::Connection, events: &[WakeEvent]) -> Result<Vec<BlobRef>> {
    let mut out: Vec<BlobRef> = Vec::new();
    for event in events {
        let WakeEvent::UserMessage { id, .. } = event else {
            continue;
        };
        if let Some(message) = conversation::message(conn, *id)?
            && message.deleted_at.is_none()
        {
            for blob in message.attachments {
                if !out.contains(&blob) {
                    out.push(blob);
                }
            }
        }
    }
    Ok(out)
}

fn turn_batch(events: &[(i64, WakeEvent)], room: usize) -> (Vec<i64>, Vec<WakeEvent>, String) {
    let shell = json_len(&render_events(&[]));
    let mut used = shell;
    let mut ids = Vec::new();
    let mut taken = Vec::new();
    for (id, event) in events {
        let cost = json_len(&render_events(std::slice::from_ref(event))) - shell;
        if used + cost <= room {
            used += cost;
            ids.push(*id);
            taken.push(event.clone());
            continue;
        }
        if taken.is_empty() {
            ids.push(*id);
            taken.push(cut_event(event, room));
        }
        break;
    }
    let rendered = render_events(&taken);
    (ids, taken, rendered)
}

/// The main text of an event: a message, a summary, a prompt or a note.
fn main_text(event: &mut WakeEvent) -> Option<&mut String> {
    match event {
        WakeEvent::UserMessage { text, .. }
        | WakeEvent::TaskUpdate { text, .. }
        | WakeEvent::TuttiMessage { text, .. }
        | WakeEvent::CardAnswered { text, .. } => Some(text),
        WakeEvent::Routine { prompt, .. } | WakeEvent::Brief { prompt, .. } => Some(prompt),
        WakeEvent::MemorySuggestion { note, .. } => Some(note),
        WakeEvent::DecisionAnswered { text, .. } => text.as_mut(),
        WakeEvent::CheckIn { summary, .. } => Some(summary),
        WakeEvent::FollowUp { reason, .. } => Some(reason),
        WakeEvent::StaleTask { .. }
        | WakeEvent::Research { .. }
        | WakeEvent::ProactiveFeedback { .. }
        | WakeEvent::ChannelCreated { .. } => None,
    }
}

/// Cuts the main text of an event so that the event alone renders in at
/// most `room` bytes as a JSON string.
fn cut_event(event: &WakeEvent, room: usize) -> WakeEvent {
    let mark = match event {
        WakeEvent::UserMessage { id, .. } | WakeEvent::TuttiMessage { id, .. } => format!(
            "\n\n[The hub cut this message here, because it is too long for one turn. Call fetch_thread with message_id \"{id}\" to read all of it.]"
        ),
        _ => "\n\n[The hub cut this text here, because it is too long for one turn.]".to_owned(),
    };
    let mut cut = event.clone();
    let full = match main_text(&mut cut) {
        Some(text) => std::mem::take(text),
        None => return cut,
    };
    let bounds: Vec<usize> = full
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(full.len()))
        .collect();
    let fits = |end: usize, cut: &mut WakeEvent| {
        if let Some(text) = main_text(cut) {
            *text = format!("{}{mark}", &full[..end]);
        }
        json_len(&render_events(std::slice::from_ref(cut))) <= room
    };
    // The longest start of the text that fits, by binary search over the
    // character boundaries.
    let (mut low, mut high) = (0_usize, bounds.len() - 1);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if fits(bounds[mid], &mut cut) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    fits(bounds[low], &mut cut);
    cut
}

/// A hash of what the system prompt of a coordinator says about its channel.
/// When it changes, the hub replaces the session, so the new purpose,
/// instructions or model reach the coordinator.
fn prompt_fingerprint(channel: &Channel, tutti: &Channel, extensions_key: &str) -> String {
    let selection = channel.effective_coordinator_selection(tutti);
    let text = format!(
        "{}\n{}\n{}\n{}\n{extensions_key}\n{}",
        channel.name,
        channel.purpose,
        channel.instructions,
        serde_json::to_string(&selection).unwrap_or_default(),
        channel.proactivity_level().as_str()
    );
    crate::util::sha256(text.as_bytes()).to_string()
}

/// The channels whose effective coordinator selection differs after one
/// channel changed from `before` to `after`. A change to Tutti moves
/// every channel that has no selection of its own.
pub(crate) fn moved_selections(
    before: &Channel,
    after: &Channel,
    tutti: &Channel,
    channels: &[Channel],
) -> Vec<ChannelId> {
    let (tutti_before, tutti_after) = if after.is_tutti() {
        (before, after)
    } else {
        (tutti, tutti)
    };
    channels
        .iter()
        .filter(|c| c.archived_at.is_none())
        .filter_map(|c| {
            let (old, new) = if c.id == after.id {
                (before, after)
            } else {
                (c, c)
            };
            let moved = old.effective_coordinator_selection(tutti_before)
                != new.effective_coordinator_selection(tutti_after);
            moved.then_some(c.id)
        })
        .collect()
}

struct Inputs {
    channel: Channel,
    tutti: Channel,
    events: Vec<(i64, WakeEvent)>,
    session: Option<Session>,
    workers: Vec<consort_proto::model::Worker>,
    open: HashMap<consort_proto::ids::WorkerId, u32>,
    global_index: String,
    channel_index: String,
    context: String,
    /// The effective set of connectors and skills, for the fingerprint.
    extensions_key: String,
}

impl Hub {
    /// The memory index text of the global scope and of one channel.
    pub(crate) async fn memory_indexes(&self, channel: &Channel) -> Result<(String, String)> {
        let scope = channel.memory_scope();
        let is_tutti = channel.is_tutti();
        self.store
            .read(move |conn| {
                let global =
                    knowledge::memory_notes(conn, consort_proto::model::MemoryScope::Global)?;
                let own = if is_tutti {
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

        let fingerprint =
            prompt_fingerprint(&inputs.channel, &inputs.tutti, &inputs.extensions_key);
        let mut inputs = inputs;
        if let Some(session) = inputs.session.clone() {
            let current = self
                .store
                .read(move |conn| work::session_runtime(conn, session.id))
                .await?;
            if current.prompt_fingerprint.as_deref() != Some(fingerprint.as_str()) {
                // The settings of the channel changed. Retire this session.
                self.retire_coordinator_session(
                    channel_id,
                    Some(session.id),
                    "The channel settings changed.",
                )
                .await?;
                inputs.session = None;
                let (workers, open) = self
                    .store
                    .read(|conn| Ok((machine::workers(conn)?, work::open_session_counts(conn)?)))
                    .await?;
                inputs.workers = workers;
                inputs.open = open;
                inputs.context = self.fresh_context(channel_id).await?;
                let (global, own) = self.memory_indexes(&inputs.channel).await?;
                inputs.global_index = global;
                inputs.channel_index = own;
            }
        }
        if let Some(session) = inputs.session.clone() {
            let worker_id = session.worker_id;
            let (event_ids, events, rendered) = turn_batch(&inputs.events, TURN_TEXT_MAX_BYTES);
            let started = self
                .store
                .write(move |tx| {
                    let row = coord_store::coordinator(tx.db(), channel_id)?;
                    if row.turn_open || row.session_id != Some(session.id) {
                        return Ok(false);
                    }
                    let attachments = event_attachments(tx.db(), &events)?;
                    let channel_files = tx.channel_files_for_turn(session.id, channel_id, None)?;
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
                                attachments,
                            },
                            channel_files,
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

        // No healthy session: place a new one. After failed starts, the
        // channel waits first. A run at the end of the wait takes the events
        // that came in the meantime.
        if self.retry_coordinator_later(channel_id) {
            return Ok(());
        }
        let selection = inputs
            .channel
            .effective_coordinator_selection(&inputs.tutti)
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
            channel_tags: &inputs.channel.tags,
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
        // The first turn also holds the context pack, and the command holds
        // the system prompt, so the events get the room that is left.
        let room = turn_room(json_len(&system_prompt) + json_len(&inputs.context))
            .max(TURN_TEXT_MIN_BYTES);
        let (event_ids, events, rendered) = turn_batch(&inputs.events, room);
        let first_turn = format!("{}\n\n{rendered}", inputs.context);
        let worker_id = placement.worker_id;
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
                let extensions = tx.session_extensions(worker_id, channel_id)?;
                let channel_files = tx.channel_files_for_start(session.id, channel_id, None)?;
                let attachments = event_attachments(tx.db(), &events)?;
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
                        channel_files,
                        extensions,
                        resume: None,
                        turn: Some(TurnInput {
                            text: first_turn,
                            attachments,
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

    /// After failed starts or refused turns, schedules one run of the
    /// coordinator of a channel at the end of the backoff. Returns true
    /// while the channel must wait.
    pub(crate) fn retry_coordinator_later(&self, channel_id: ChannelId) -> bool {
        let Some((wait, schedule)) = self.coordinators.start_wait(channel_id, Instant::now())
        else {
            return false;
        };
        if schedule {
            tracing::info!(channel_id = %channel_id, wait_ms = wait.as_millis(), "the coordinator failed before, so the hub waits before it tries again");
            let hub = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(wait).await;
                hub.coordinators.retry_ran(channel_id);
                hub.kick_coordinator(channel_id);
            });
        }
        true
    }

    /// The `<context>` block for a new session.
    async fn fresh_context(&self, channel_id: ChannelId) -> Result<String> {
        self.store
            .read(move |conn| {
                let tasks = work::tasks_in_channel(conn, channel_id, false)?;
                let (mut messages, _) = conversation::messages_page(
                    conn,
                    channel_id,
                    None,
                    None,
                    CONTEXT_MESSAGES,
                    usize::MAX,
                )?;
                messages.reverse();
                Ok(prompts::context_pack(&tasks, &messages))
            })
            .await
    }

    /// Ends a healthy coordinator session on purpose, for example because the
    /// channel settings changed. The worker stops the session. The next turn
    /// starts a new one. With `None`, it ends the channel's current session.
    /// A session with an open turn stays: the turn is not cut, and the next
    /// run sees that the prompt fingerprint changed.
    async fn retire_coordinator_session(
        &self,
        channel_id: ChannelId,
        session_id: Option<SessionId>,
        reason: &'static str,
    ) -> Result<()> {
        let hub = self.clone();
        let fx = self
            .store
            .write(move |tx| {
                let mut fx = Effects::default();
                let mut row = coord_store::coordinator(tx.db(), channel_id)?;
                let Some(session_id) = session_id.or(row.session_id) else {
                    return Ok(fx);
                };
                let Some(mut session) = work::session(tx.db(), session_id)? else {
                    return Ok(fx);
                };
                if row.turn_open || row.session_id != Some(session_id) {
                    return Ok(fx);
                }
                row.session_id = None;
                tx.save_coordinator(&row)?;
                if !session.state.is_final() {
                    session.state = SessionState::Ended;
                    session.ended_at = Some(tx.now());
                    session.end_reason = Some(reason.to_owned());
                    tx.save_session(&session)?;
                    queue_stop(&hub, tx, &mut fx, &session, reason)?;
                }
                Ok(fx)
            })
            .await?;
        self.run_effects(fx);
        Ok(())
    }

    /// The model selection of these channels' coordinators changed. An idle
    /// session ends now, so the next turn starts a session with the new
    /// model and options. A session whose turn runs keeps the turn, and the
    /// next run of the channel replaces it (see [`prompt_fingerprint`]).
    pub(crate) async fn coordinator_selection_changed(&self, channels: Vec<ChannelId>) {
        for channel_id in channels {
            if let Err(err) = self
                .retire_coordinator_session(
                    channel_id,
                    None,
                    "The model for the assistant changed.",
                )
                .await
            {
                tracing::warn!(error = %err, channel_id = %channel_id, "could not end a coordinator session after a model change");
            }
        }
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
                let tutti = conversation::tutti_channel(conn)?;
                let (workers, open) = if session.is_none() {
                    (machine::workers(conn)?, work::open_session_counts(conn)?)
                } else {
                    (Vec::new(), HashMap::new())
                };
                let context = if session.is_none() {
                    let tasks = work::tasks_in_channel(conn, channel_id, false)?;
                    let (mut messages, _) = conversation::messages_page(
                        conn,
                        channel_id,
                        None,
                        None,
                        CONTEXT_MESSAGES,
                        usize::MAX,
                    )?;
                    messages.reverse();
                    prompts::context_pack(&tasks, &messages)
                } else {
                    String::new()
                };
                let extensions_key =
                    crate::connectors::extensions::effective_set_key(conn, channel_id)?;
                Ok(Some(Inputs {
                    channel,
                    tutti,
                    events,
                    session,
                    workers,
                    open,
                    global_index: String::new(),
                    channel_index: String::new(),
                    context,
                    extensions_key,
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
    ///
    /// Only the time that the worker is online counts, as for task turns:
    /// the events of an offline worker wait in its spool, and the turn may
    /// have acted already.
    pub(crate) async fn expire_stuck_turns(&self, max_age_ms: i64) -> Result<()> {
        let candidates = self
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
                        let worker = work::session(conn, session_id)?.map(|s| s.worker_id);
                        out.push((session_id, worker));
                    }
                }
                Ok(out)
            })
            .await?;
        let max_age = Duration::from_millis(u64::try_from(max_age_ms).unwrap_or(0));
        let stuck = candidates.into_iter().filter_map(|(session_id, worker)| {
            // A row whose session is gone has no worker to wait for.
            let online_long_enough = worker.is_none_or(|worker| {
                self.links
                    .get(worker)
                    .is_some_and(|l| !l.outbox.is_closed() && l.connected_at.elapsed() > max_age)
            });
            online_long_enough.then_some(session_id)
        });
        // One session that fails does not stop the others.
        for session_id in stuck.collect::<Vec<_>>() {
            tracing::warn!(session_id = %session_id, "a coordinator turn ran for too long, so the hub replaces the session");
            if let Err(err) = self
                .mark_session_lost(session_id, "The turn ran for too long.")
                .await
            {
                tracing::warn!(error = %err, session_id = %session_id, "could not replace a coordinator session with a stuck turn");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::ids::{AccountId, MessageId, WorkerId};
    use consort_proto::model::ModelOptions;
    use consort_proto::time::Timestamp;

    use super::*;

    fn said(text: &str) -> WakeEvent {
        WakeEvent::UserMessage {
            id: MessageId::new(),
            thread: None,
            at: Timestamp::now(),
            text: text.to_owned(),
        }
    }

    #[tokio::test]
    async fn a_change_to_tutti_moves_only_the_channels_that_inherit_it() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let tutti = hub.store.read(conversation::tutti_channel).await.unwrap();
        let sel = |model: &str| consort_proto::model::ModelSelection {
            account_label: "Claude".to_owned(),
            model: model.to_owned(),
            options: ModelOptions::new(),
        };
        let mut inherits = tutti.clone();
        inherits.id = ChannelId::new();
        inherits.kind = consort_proto::model::ChannelKind::Standard;
        inherits.coordinator_selection = None;
        let mut own = inherits.clone();
        own.id = ChannelId::new();
        own.coordinator_selection = Some(sel("own"));
        let mut archived = inherits.clone();
        archived.id = ChannelId::new();
        archived.archived_at = Some(Timestamp::now());
        let mut after = tutti.clone();
        after.coordinator_selection = Some(sel("new"));
        let all = vec![after.clone(), inherits.clone(), own.clone(), archived];
        let moved = moved_selections(&tutti, &after, &tutti, &all);
        assert_eq!(moved, [tutti.id, inherits.id]);

        // A channel's own change moves only that channel, and only when the
        // effective selection changes.
        let mut own_after = own.clone();
        own_after.coordinator_selection = Some(sel("other"));
        assert_eq!(moved_selections(&own, &own_after, &after, &all), [own.id]);
        let mut same = inherits.clone();
        same.coordinator_selection = Some(sel("new"));
        assert!(moved_selections(&inherits, &same, &after, &all).is_empty());
    }

    #[test]
    fn the_start_backoff_doubles_up_to_ten_minutes() {
        assert_eq!(start_backoff(0), Duration::ZERO);
        assert_eq!(start_backoff(1), Duration::from_secs(10));
        assert_eq!(start_backoff(2), Duration::from_secs(20));
        assert_eq!(start_backoff(3), Duration::from_secs(40));
        assert_eq!(start_backoff(7), Duration::from_secs(600));
        assert_eq!(start_backoff(u32::MAX), Duration::from_secs(600));
    }

    #[test]
    fn a_channel_waits_after_a_failed_start_and_schedules_one_run() {
        let coordinators = Coordinators::default();
        let channel = ChannelId::new();
        let now = Instant::now();
        assert!(coordinators.start_wait(channel, now).is_none());
        coordinators.start_failed(channel);
        let (wait, schedule) = coordinators.start_wait(channel, Instant::now()).unwrap();
        assert!(wait <= Duration::from_secs(10) && schedule);
        // A second trigger in the wait schedules nothing more.
        let (_, schedule) = coordinators.start_wait(channel, Instant::now()).unwrap();
        assert!(!schedule);
        // After the wait, the channel may start.
        assert!(
            coordinators
                .start_wait(channel, Instant::now() + Duration::from_secs(11))
                .is_none()
        );
        coordinators.start_worked(channel);
        assert!(coordinators.start_wait(channel, Instant::now()).is_none());
    }

    #[test]
    fn a_turn_takes_events_in_order_while_they_fit() {
        let events: Vec<(i64, WakeEvent)> = (1..=3).map(|i| (i, said(&"a".repeat(100)))).collect();
        let two: Vec<WakeEvent> = events[..2].iter().map(|(_, e)| e.clone()).collect();
        let room = json_len(&render_events(&two));
        let (ids, taken, text) = turn_batch(&events, room);
        assert_eq!(ids, [1, 2]);
        assert_eq!(taken, two);
        assert!(json_len(&text) <= room);
        let (ids, _, _) = turn_batch(&events, usize::MAX);
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn a_first_event_that_does_not_fit_alone_is_cut_with_a_pointer() {
        let event = said(&"<b>".repeat(10_000));
        let WakeEvent::UserMessage { id, .. } = event.clone() else {
            unreachable!()
        };
        let events = vec![(1, event), (2, said("next"))];
        let (ids, taken, text) = turn_batch(&events, 4_000);
        assert_eq!(ids, [1]);
        assert!(json_len(&text) <= 4_000, "{}", json_len(&text));
        assert!(text.contains(&format!("message_id \"{id}\"")), "{text}");
        assert!(text.contains("&lt;b&gt;"));
        let WakeEvent::UserMessage { text: cut, .. } = &taken[0] else {
            unreachable!()
        };
        assert!(cut.starts_with("<b><b>"));
    }

    /// Registers a link for `worker` that connected `online_for` ago. Keep
    /// the receiver, so the link stays open.
    fn link_for(
        hub: &Hub,
        worker: WorkerId,
        online_for: Duration,
    ) -> tokio::sync::mpsc::Receiver<String> {
        let (outbox, rx) = crate::net::ws::Outbox::new(tokio_util::sync::CancellationToken::new());
        let mut link = crate::link::WorkerLink::new(hub.links.next_conn_id(), worker, outbox);
        link.connected_at = Instant::now().checked_sub(online_for).unwrap();
        hub.links.register(Arc::new(link));
        rx
    }

    /// A coordinator session with a turn that opened 10 s ago.
    async fn session_with_old_turn(hub: &Hub) -> Session {
        let tutti = hub.store.read(conversation::tutti_channel).await.unwrap();
        let session = Session {
            id: SessionId::new(),
            role: SessionRole::Coordinator,
            channel_id: tutti.id,
            task_id: None,
            worker_id: WorkerId::new(),
            account_id: AccountId::new(),
            harness: "fake".to_owned(),
            model: "fake-1".to_owned(),
            options: ModelOptions::new(),
            workspace_path: String::new(),
            native_ref: None,
            state: SessionState::Active,
            last_seq: 0,
            usage: Usage::default(),
            started_at: Timestamp::now(),
            ended_at: None,
            end_reason: None,
        };
        let stored = session.clone();
        let old = Timestamp::now().plus_millis(-10_000);
        hub.store
            .write(move |tx| {
                tx.save_session(&stored)?;
                tx.save_coordinator(&CoordinatorRow {
                    channel_id: stored.channel_id,
                    session_id: Some(stored.id),
                    turn_open: true,
                    turn_started_at: Some(old),
                    turn_events: Vec::new(),
                })
            })
            .await
            .unwrap();
        session
    }

    /// Review 3, finding 6: the coordinator expiry counted the time that
    /// the worker was offline, while its events waited in the spool.
    #[tokio::test]
    async fn a_long_coordinator_turn_of_a_worker_that_just_came_back_waits() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let session = session_with_old_turn(&hub).await;
        let state = |hub: &Hub| {
            let (hub, id) = (hub.clone(), session.id);
            async move {
                hub.store
                    .read(move |conn| work::session_or_404(conn, id))
                    .await
                    .unwrap()
                    .state
            }
        };
        // Offline: the turn waits.
        hub.expire_stuck_turns(1_000).await.unwrap();
        assert_eq!(state(&hub).await, SessionState::Active);
        // Online again just now: the turn still waits.
        let _rx = link_for(&hub, session.worker_id, Duration::ZERO);
        hub.expire_stuck_turns(1_000).await.unwrap();
        assert_eq!(state(&hub).await, SessionState::Active);
        // Online for longer than the limit: the hub replaces the session.
        let _rx2 = link_for(&hub, session.worker_id, Duration::from_secs(60));
        hub.expire_stuck_turns(1_000).await.unwrap();
        assert_eq!(state(&hub).await, SessionState::Lost);
    }

    /// Review 3, finding 3: a refused coordinator turn waited for the next
    /// wake event. It now runs again after the start backoff.
    #[tokio::test]
    async fn a_refused_coordinator_turn_runs_again_after_the_start_backoff() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let channel = ChannelId::new();
        hub.run_effects(Effects {
            turn_refused: vec![channel],
            ..Effects::default()
        });
        let (wait, schedule) = hub
            .coordinators
            .start_wait(channel, Instant::now())
            .expect("the channel waits");
        assert!(wait <= START_BACKOFF_FIRST, "{wait:?}");
        assert!(
            !schedule,
            "a run at the end of the wait is scheduled already"
        );
    }

    #[tokio::test]
    async fn one_stuck_turn_that_fails_does_not_stop_the_others() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let tutti = hub.store.read(conversation::tutti_channel).await.unwrap();
        let session = Session {
            id: SessionId::new(),
            role: SessionRole::Coordinator,
            channel_id: tutti.id,
            task_id: None,
            worker_id: WorkerId::new(),
            account_id: AccountId::new(),
            harness: "fake".to_owned(),
            model: "fake-1".to_owned(),
            options: ModelOptions::new(),
            workspace_path: String::new(),
            native_ref: None,
            state: SessionState::Active,
            last_seq: 0,
            usage: Usage::default(),
            started_at: Timestamp::now(),
            ended_at: None,
            end_reason: None,
        };
        let good = session.id;
        let _rx = link_for(&hub, session.worker_id, Duration::from_secs(60));
        let old = Timestamp::now().plus_millis(-10_000);
        hub.store
            .write(move |tx| {
                // A row whose session is gone fails. It comes first.
                tx.save_coordinator(&CoordinatorRow {
                    channel_id: ChannelId::new(),
                    session_id: Some(SessionId::new()),
                    turn_open: true,
                    turn_started_at: Some(old),
                    turn_events: Vec::new(),
                })?;
                tx.save_session(&session)?;
                tx.save_coordinator(&CoordinatorRow {
                    channel_id: tutti.id,
                    session_id: Some(good),
                    turn_open: true,
                    turn_started_at: Some(old),
                    turn_events: Vec::new(),
                })
            })
            .await
            .unwrap();
        hub.expire_stuck_turns(1_000).await.unwrap();
        let state = hub
            .store
            .read(move |conn| work::session_or_404(conn, good))
            .await
            .unwrap()
            .state;
        assert_eq!(state, SessionState::Lost);
    }
}
