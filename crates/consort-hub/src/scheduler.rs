//! The routine scheduler.
//!
//! [`spawn`] starts a loop that checks for due routines once a second. Each
//! due routine runs with [`Hub::run_routine`]. The client API (`routine.run_now`)
//! calls the same function with [`RunTrigger::Manual`].
//!
//! Rules for a run:
//!
//! - The run reads the routine again inside its transaction. A routine that
//!   the user deleted, turned off or changed after the scheduler read it is
//!   handled correctly, and a stale copy never overwrites a newer one.
//! - A routine in an archived or missing channel does not run. A scheduled
//!   check still moves the routine to its next run.
//! - A run is `running` until its work ends: the coordinator turn that got
//!   the prompt, or the task that the run started (see
//!   `store::routines::RunEnd`).
//! - A scheduled run that would start while the last run of the same routine
//!   still runs is `skipped`. Run now then fails with `conflict`, and no run
//!   is recorded.
//! - A routine that was due while the hub was down runs once when the hub
//!   is back, if it is less than [`LATE_LIMIT_MS`] late. A later one is
//!   recorded as `skipped`. Either way its next run is the first one after
//!   now. There is no catch-up.
//! - An interval routine keeps the rhythm of the time when it was saved:
//!   the next run is the first multiple of the interval after now, counted
//!   from the run that was due.
//! - A `once` routine turns itself off after it runs.
//! - A `start_task` run moves the routine to its next run and records the
//!   run before it creates the task. A crash in between leaves a run that
//!   the next check ends, and never makes two tasks.

use std::time::Duration;

use consort_proto::error::ErrorCode;
use consort_proto::ids::{ChannelId, RoutineId};
use consort_proto::model::{
    Routine, RoutineAction, RoutineRun, RoutineRunStatus, RoutineSchedule, TaskCreator,
};
use consort_proto::time::Timestamp;
use consort_proto::turn_input::WakeEvent;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::error::{HubError, Result};
use crate::hub::Hub;
use crate::ops::hooks::NewTask;
use crate::schedule::next_run_at;
use crate::store::routines::{RunEnd, task_run_end};
use crate::store::{Tx, conversation, coordinator, routines, work};

/// How often the scheduler looks for due routines.
const TICK: Duration = Duration::from_secs(1);

/// A scheduled run that is this late or later does not run. The hub was
/// down at the run time (approved on 2026-10-29).
pub(crate) const LATE_LIMIT_MS: i64 = 60 * 60 * 1000;

/// A `start_task` run whose task is not there after this long lost its task
/// to a crash between the two transactions of the run.
const TASK_START_LIMIT_MS: i64 = 5 * 60 * 1000;

/// Why a routine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunTrigger {
    /// Its `next_run_at` has passed. The run moves the routine to its next run.
    Schedule,
    /// The user asked for it (`routine.run_now`). The schedule does not change.
    Manual,
}

/// Starts the scheduler loop. It stops when `hub.shutdown` is cancelled. A
/// tick that has started finishes first.
pub(crate) fn spawn(hub: &Hub) -> JoinHandle<()> {
    let hub = hub.clone();
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval(TICK);
        // A slow tick must not cause a burst of ticks after it.
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = hub.shutdown.cancelled() => break,
                _ = ticks.tick() => {}
            }
            tick(&hub).await;
        }
    })
}

/// Runs every routine that is due now. A failure of one routine is logged
/// and does not stop the others.
pub(crate) async fn tick(hub: &Hub) {
    tick_at(hub, hub.now()).await;
}

/// Runs every routine that is due at `now`, as if the clock said `now`.
/// Tests use it to run the schedule at a chosen time.
pub(crate) async fn tick_at(hub: &Hub, now: Timestamp) {
    let due = match hub
        .store
        .read(move |conn| routines::due_routines(conn, now))
        .await
    {
        Ok(due) => due,
        Err(err) => {
            tracing::warn!(error = %err, "the scheduler could not read the due routines");
            return;
        }
    };
    for routine in due {
        if hub.shutdown.is_cancelled() {
            break;
        }
        match hub
            .run_routine_at(&routine, RunTrigger::Schedule, Some(now))
            .await
        {
            Ok(run) => {
                tracing::info!(
                    routine = %routine.id,
                    name = %routine.name,
                    status = run.status.as_str(),
                    "a routine was due"
                );
            }
            // The routine changed after the read, or its channel is archived.
            // This is not a failure.
            Err(err) if matches!(err.code(), ErrorCode::Conflict | ErrorCode::NotFound) => {
                tracing::debug!(routine = %routine.id, reason = %err, "a routine did not run");
            }
            Err(err) => {
                tracing::warn!(
                    routine = %routine.id,
                    name = %routine.name,
                    error = %err,
                    "a routine failed to run"
                );
            }
        }
    }
}

/// What the first transaction of a run decided.
enum Begun {
    /// The routine did not run and nothing was recorded. The text says why.
    Refused(String),
    /// The run is complete in the store: a `skipped` run, or a
    /// `coordinator_turn` run whose prompt waits for the coordinator. The
    /// caller kicks the coordinator of `wake`.
    Recorded {
        run: RoutineRun,
        wake: Option<ChannelId>,
    },
    /// A `start_task` run is recorded as `running` and must create its task.
    Task {
        new_task: Box<NewTask>,
        run: RoutineRun,
        /// Set when the stored schedule was not valid and the hub turned the
        /// routine off.
        schedule_error: Option<String>,
    },
}

impl Hub {
    /// Runs the action of a routine now.
    ///
    /// The `routine` argument names the routine. The run uses the stored copy,
    /// so a stale argument is safe. It returns a `conflict` error when the
    /// routine did not run and nothing was recorded: its channel is archived
    /// or missing, (for [`RunTrigger::Schedule`]) it is no longer due, or
    /// (for [`RunTrigger::Manual`]) its last run still runs. It returns
    /// `not_found` when the routine does not exist any more. A scheduled run
    /// that the hub skips returns the `skipped` run.
    pub(crate) async fn run_routine(
        &self,
        routine: &Routine,
        trigger: RunTrigger,
    ) -> Result<RoutineRun> {
        self.run_routine_at(routine, trigger, None).await
    }

    /// [`Hub::run_routine`] with the clock at `at`. `None` is now.
    pub(crate) async fn run_routine_at(
        &self,
        routine: &Routine,
        trigger: RunTrigger,
        at: Option<Timestamp>,
    ) -> Result<RoutineRun> {
        let id = routine.id;
        let begun = self
            .store
            .write(move |tx| begin_run(tx, id, trigger, at))
            .await?;
        match begun {
            Begun::Refused(reason) => Err(HubError::conflict(reason)),
            Begun::Recorded { run, wake } => {
                if let Some(channel_id) = wake {
                    self.kick_coordinator(channel_id);
                }
                Ok(run)
            }
            Begun::Task {
                new_task,
                run,
                schedule_error,
            } => self.finish_task_run(*new_task, run, schedule_error).await,
        }
    }

    /// Creates the task of a `start_task` run, then writes it to the run.
    async fn finish_task_run(
        &self,
        new_task: NewTask,
        mut run: RoutineRun,
        schedule_error: Option<String>,
    ) -> Result<RoutineRun> {
        let failure = match self.create_task(new_task).await {
            Ok(task) => {
                run.task_id = Some(task.id);
                None
            }
            Err(err) => {
                tracing::warn!(routine = %run.routine_id, error = %err, "a routine could not start its task");
                Some(err.to_error_body().message)
            }
        };
        let failure = join_errors(failure, schedule_error);
        self.store
            .write(move |tx| {
                // The task may have ended before this transaction, for
                // example when its placement failed at once.
                let task = match run.task_id {
                    Some(task_id) => work::task(tx.db(), task_id)?,
                    None => None,
                };
                let end = match (&failure, &task) {
                    (Some(text), _) => Some(RunEnd::Error(text.clone())),
                    (None, Some(task)) => task_run_end(task),
                    (None, None) => None,
                };
                // The user may have deleted the routine meanwhile. The run
                // row stays until the delete removes it.
                if let Some(stored) = routines::running_run(tx.db(), run.routine_id)?
                    && stored.id == run.id
                {
                    match end {
                        Some(end) => {
                            // The task ID is part of the change.
                            tx.save_routine_run(&run)?;
                            tx.end_routine_run(&mut run, end)?;
                        }
                        None => tx.save_routine_run(&run)?,
                    }
                }
                Ok(run)
            })
            .await
    }
}

/// The first transaction of a run. It reads the routine, decides whether it
/// runs, and records the run.
fn begin_run(
    tx: &mut Tx<'_>,
    id: RoutineId,
    trigger: RunTrigger,
    at: Option<Timestamp>,
) -> Result<Begun> {
    let now = at.unwrap_or(tx.now());
    let mut routine = routines::routine_or_404(tx.db(), id)?;
    let scheduled = trigger == RunTrigger::Schedule;
    if scheduled && !is_due(&routine, now) {
        return Ok(Begun::Refused(
            "The routine is turned off or not due. It did not run.".to_owned(),
        ));
    }
    let channel_is_open = conversation::channel(tx.db(), routine.channel_id)?
        .is_some_and(|channel| channel.archived_at.is_none());
    if !channel_is_open {
        // Move on, so the scheduler does not look at this routine every second.
        if scheduled {
            advance(&mut routine, now);
            tx.save_routine(&routine)?;
        }
        return Ok(Begun::Refused(
            "The channel of the routine is archived or missing. The routine did not run."
                .to_owned(),
        ));
    }

    if let Some(kind) = routine.kind.proactive_kind() {
        let channel = conversation::channel_or_404(tx.db(), routine.channel_id)?;
        return Ok(
            match crate::proactivity::routines::begin_proactive_run(
                tx, routine, kind, &channel, now, scheduled,
            )? {
                Ok((run, wake)) => Begun::Recorded { run, wake },
                Err(reason) => Begun::Refused(reason),
            },
        );
    }

    if still_running(tx, &routine, now)? {
        if !scheduled {
            return Ok(Begun::Refused(format!(
                "The last run of {} has not ended yet. Wait until it ends, then select Run now again.",
                routine.name
            )));
        }
        // The last run keeps its place as the routine's last run.
        let due = routine.next_run_at.unwrap_or(now);
        let mut run = skipped_run(&routine, due, "The last run was still going.");
        add_schedule_error(&mut run, advance(&mut routine, now));
        tx.record_routine_run(&mut run)?;
        tx.save_routine(&routine)?;
        return Ok(Begun::Recorded { run, wake: None });
    }

    if scheduled
        && let Some(due) = routine.next_run_at
        && now.as_millis().saturating_sub(due.as_millis()) >= LATE_LIMIT_MS
    {
        let mut run = skipped_run(
            &routine,
            due,
            "The hub was offline at the run time and came back more than 1 hour later.",
        );
        add_schedule_error(&mut run, advance(&mut routine, now));
        note_last_run(&mut routine, &run);
        tx.record_routine_run(&mut run)?;
        tx.save_routine(&routine)?;
        return Ok(Begun::Recorded { run, wake: None });
    }

    let schedule_error = if scheduled {
        advance(&mut routine, now)
    } else {
        None
    };
    let mut run = new_run(&routine, now, !scheduled);
    match routine.action.clone() {
        RoutineAction::CoordinatorTurn => {
            tx.queue_wake(
                routine.channel_id,
                &WakeEvent::Routine {
                    id: routine.id,
                    name: routine.name.clone(),
                    prompt: routine.prompt.clone(),
                },
            )?;
            if let Some(reason) = schedule_error {
                // The prompt went out. The run still says that the routine
                // turned off.
                add_schedule_error(&mut run, Some(reason));
            }
            note_last_run(&mut routine, &run);
            tx.record_routine_run(&mut run)?;
            tx.save_routine(&routine)?;
            Ok(Begun::Recorded {
                run,
                wake: Some(routine.channel_id),
            })
        }
        RoutineAction::StartTask {
            title,
            model_selection,
            placement,
        } => {
            note_last_run(&mut routine, &run);
            tx.record_routine_run(&mut run)?;
            tx.save_routine(&routine)?;
            Ok(Begun::Task {
                new_task: Box::new(NewTask {
                    channel_id: routine.channel_id,
                    title,
                    brief: routine.prompt,
                    model_selection,
                    placement,
                    approval_mode: None,
                    start_at: None,
                    created_by: TaskCreator::Routine,
                    routine_id: Some(id),
                    attachments: Vec::new(),
                    proactive: None,
                    user_request: String::new(),
                    voice_task: false,
                }),
                run,
                schedule_error,
            })
        }
    }
}

/// Whether the last run of a routine still runs. A run whose work is gone
/// (its prompt left the coordinator queue without a turn, or its task is
/// gone or ended) ends here, so it never blocks the routine.
pub(crate) fn still_running(tx: &mut Tx<'_>, routine: &Routine, now: Timestamp) -> Result<bool> {
    let Some(mut run) = routines::running_run(tx.db(), routine.id)? else {
        return Ok(false);
    };
    if let Some(task_id) = run.task_id {
        let end = match work::task(tx.db(), task_id)? {
            Some(task) => task_run_end(&task),
            None => Some(RunEnd::Error(
                "The task of this run was deleted.".to_owned(),
            )),
        };
        return match end {
            Some(end) => {
                tx.end_routine_run(&mut run, end)?;
                Ok(false)
            }
            None => Ok(true),
        };
    }
    if matches!(routine.action, RoutineAction::StartTask { .. }) && !prompt_waits(tx, &run)? {
        // The run makes its task now, or a crash lost it.
        if now.as_millis() - run.started_at.as_millis() < TASK_START_LIMIT_MS {
            return Ok(true);
        }
        tx.end_routine_run(
            &mut run,
            RunEnd::Error("The hub stopped before the task of this run started.".to_owned()),
        )?;
        return Ok(false);
    }
    if prompt_waits(tx, &run)? {
        return Ok(true);
    }
    tx.end_routine_run(
        &mut run,
        RunEnd::Error(
            "The assistant did not get the prompt of this run. Select Run now to try again."
                .to_owned(),
        ),
    )?;
    Ok(false)
}

/// Whether the prompt of a run waits in the coordinator queue of its
/// channel, or is in the open turn.
fn prompt_waits(tx: &Tx<'_>, run: &RoutineRun) -> Result<bool> {
    let is_this =
        |event: &WakeEvent| crate::proactivity::routines::routine_of(event) == Some(run.routine_id);
    if coordinator::pending_wakes(tx.db(), run.channel_id)?
        .iter()
        .any(|(_, event)| is_this(event))
    {
        return Ok(true);
    }
    let row = coordinator::coordinator(tx.db(), run.channel_id)?;
    Ok(row.turn_open && row.turn_events.iter().any(is_this))
}

pub(crate) fn new_run(routine: &Routine, started_at: Timestamp, manual: bool) -> RoutineRun {
    RoutineRun {
        id: 0,
        routine_id: routine.id,
        channel_id: routine.channel_id,
        started_at,
        ended_at: None,
        status: RoutineRunStatus::Running,
        manual,
        message_id: None,
        task_id: None,
        error: None,
    }
}

pub(crate) fn skipped_run(routine: &Routine, due: Timestamp, reason: &str) -> RoutineRun {
    RoutineRun {
        ended_at: Some(due),
        status: RoutineRunStatus::Skipped,
        error: Some(reason.to_owned()),
        ..new_run(routine, due, false)
    }
}

fn is_due(routine: &Routine, now: Timestamp) -> bool {
    routine.enabled && routine.next_run_at.is_some_and(|at| at <= now)
}

/// Moves a routine past a scheduled run at `now`. The next run is the first
/// one after `now`, so a routine that was due for a long time runs once. An
/// interval routine counts from the run that was due, so it keeps the rhythm
/// of the time when it was saved.
///
/// A `once` routine turns off. A routine whose stored schedule is not valid
/// any more (for example a time zone name that the hub no longer knows) also
/// turns off. In that case the function returns the reason.
pub(crate) fn advance(routine: &mut Routine, now: Timestamp) -> Option<String> {
    if matches!(routine.schedule, RoutineSchedule::Once { .. }) {
        routine.enabled = false;
        routine.next_run_at = None;
        return None;
    }
    if let RoutineSchedule::Interval { seconds } = routine.schedule
        && let Some(due) = routine.next_run_at
        && let Some(next) = next_interval_run(due, seconds, now)
    {
        routine.next_run_at = Some(next);
        return None;
    }
    match next_run_at(&routine.schedule, now) {
        Ok(next) => {
            // The research pass starts a few minutes after its time, so
            // routines that share a minute do not all start at once.
            routine.next_run_at = match routine.kind {
                consort_proto::model::RoutineKind::Research => {
                    next.map(|at| at.plus_millis(crate::proactivity::gate::jitter_ms(routine.id)))
                }
                _ => next,
            };
            None
        }
        Err(err) => {
            routine.enabled = false;
            routine.next_run_at = None;
            tracing::warn!(
                routine = %routine.id,
                error = %err,
                "the schedule of a routine is not valid, so the hub turned the routine off"
            );
            Some(err.to_error_body().message)
        }
    }
}

/// The first time after `now` that is `due` plus a whole number of
/// intervals. `None` when the numbers do not fit.
fn next_interval_run(due: Timestamp, seconds: u64, now: Timestamp) -> Option<Timestamp> {
    let step = i64::try_from(seconds).ok()?.checked_mul(1_000)?;
    if step <= 0 {
        return None;
    }
    let behind = now.as_millis().checked_sub(due.as_millis())?;
    let steps = if behind < 0 { 0 } else { behind / step + 1 };
    Some(Timestamp::from_millis(
        due.as_millis().checked_add(steps.checked_mul(step)?)?,
    ))
}

/// Adds the reason from [`advance`] to an error text.
fn join_errors(previous: Option<String>, schedule_error: Option<String>) -> Option<String> {
    let Some(reason) = schedule_error else {
        return previous;
    };
    let text =
        format!("The hub turned the routine off because its schedule is not valid. {reason}");
    Some(match previous {
        Some(previous) => format!("{previous} {text}"),
        None => text,
    })
}

/// Adds the reason from [`advance`] to the error of a run.
pub(crate) fn add_schedule_error(run: &mut RoutineRun, schedule_error: Option<String>) {
    run.error = join_errors(run.error.take(), schedule_error);
}

/// Copies a new run to the routine as its last run.
pub(crate) fn note_last_run(routine: &mut Routine, run: &RoutineRun) {
    routine.last_run_at = Some(run.started_at);
    routine.last_run_status = Some(run.status);
}

/// Helpers that the tests of the routine handlers and tools share.
#[cfg(test)]
pub(crate) mod testing {
    use consort_proto::ids::ChannelId;
    use consort_proto::model::{
        ApprovalMode, Channel, ChannelKind, Isolation, Routine, RoutineAction, RoutineRun,
        RoutineSchedule,
    };
    use consort_proto::turn_input::WakeEvent;

    use crate::hub::Hub;
    use crate::store::{coordinator, routines};

    /// Saves a standard channel, archived or not.
    pub(crate) async fn channel(hub: &Hub, archived: bool) -> Channel {
        let now = hub.now();
        let channel = Channel {
            id: ChannelId::new(),
            kind: ChannelKind::Standard,
            name: "Home".to_owned(),
            icon: None,
            color: None,
            group_id: None,
            position: 1,
            purpose: "Test channel".to_owned(),
            instructions: String::new(),
            coordinator_selection: None,
            task_selection: None,
            preferred_workers: Vec::new(),
            repo_ids: Vec::new(),
            default_isolation: Isolation::Worktree,
            default_approval_mode: ApprovalMode::Ask,
            allowed_local_mcp_servers: Vec::new(),
            memory_enabled: true,
            disabled_connections: Vec::new(),
            disabled_skills: Vec::new(),
            archived_at: archived.then_some(now),
            created_at: now,
            updated_at: now,
            tags: Vec::new(),
            proactive_tasks: Default::default(),
            coordinator_fallback: None,
            coordinator_blocked: None,
            monid: Default::default(),
            access_grants: Vec::new(),
            proactivity: None,
        };
        let saved = channel.clone();
        hub.store
            .write(move |tx| tx.save_channel(&saved))
            .await
            .unwrap();
        channel
    }

    /// An enabled routine that is not saved yet.
    pub(crate) fn routine(
        channel_id: ChannelId,
        schedule: RoutineSchedule,
        action: RoutineAction,
    ) -> Routine {
        Routine {
            id: consort_proto::ids::RoutineId::new(),
            channel_id,
            name: "Morning brief".to_owned(),
            schedule,
            prompt: "Summarize my day.".to_owned(),
            action,
            enabled: true,
            next_run_at: None,
            last_run_at: None,
            last_run_status: None,
            monid: None,
            kind: Default::default(),
            origin: Default::default(),
            created_at: None,
        }
    }

    pub(crate) async fn save(hub: &Hub, routine: &Routine) {
        let routine = routine.clone();
        hub.store
            .write(move |tx| tx.save_routine(&routine))
            .await
            .unwrap();
    }

    pub(crate) async fn load(hub: &Hub, id: consort_proto::ids::RoutineId) -> Routine {
        hub.store
            .read(move |conn| routines::routine_or_404(conn, id))
            .await
            .unwrap()
    }

    /// The queued wake events of a channel, oldest first.
    pub(crate) async fn wakes(hub: &Hub, channel_id: ChannelId) -> Vec<WakeEvent> {
        hub.store
            .read(move |conn| coordinator::pending_wakes(conn, channel_id))
            .await
            .unwrap()
            .into_iter()
            .map(|(_, event)| event)
            .collect()
    }

    /// Ends the run of a routine that still runs, as a turn that finished.
    pub(crate) async fn end_running(hub: &Hub, id: consort_proto::ids::RoutineId) {
        hub.store
            .write(move |tx| {
                if let Some(mut run) = routines::running_run(tx.db(), id)? {
                    tx.end_routine_run(&mut run, routines::RunEnd::Ok)?;
                }
                Ok(())
            })
            .await
            .unwrap();
    }

    /// The runs of a routine, newest first.
    pub(crate) async fn runs(hub: &Hub, id: consort_proto::ids::RoutineId) -> Vec<RoutineRun> {
        hub.store
            .read(move |conn| routines::routine_runs(conn, id, 50))
            .await
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{channel, end_running, load, routine, runs, save, wakes};
    use super::*;
    use crate::hub::test_hub;
    use consort_proto::model::PlacementRequest;

    const HOUR_MS: i64 = 3_600_000;

    fn interval(seconds: u64) -> RoutineSchedule {
        RoutineSchedule::Interval { seconds }
    }

    fn start_task() -> RoutineAction {
        RoutineAction::StartTask {
            title: "Nightly check".to_owned(),
            model_selection: None,
            placement: PlacementRequest::default(),
        }
    }

    /// A due routine: it should have run one minute ago.
    fn due(
        channel_id: ChannelId,
        schedule: RoutineSchedule,
        action: RoutineAction,
        now: Timestamp,
    ) -> Routine {
        let mut routine = routine(channel_id, schedule, action);
        routine.next_run_at = Some(now.plus_millis(-60_000));
        routine
    }

    #[tokio::test]
    async fn a_coordinator_turn_queues_one_wake_records_one_running_run_and_advances() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let before = hub.now();
        let routine = due(
            channel.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            before,
        );
        save(&hub, &routine).await;

        let run = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();

        assert_eq!(run.routine_id, routine.id);
        assert_eq!(run.channel_id, channel.id);
        assert_eq!(run.error, None);
        assert_eq!(run.status, RoutineRunStatus::Running);
        assert!(!run.manual);
        assert!(run.id > 0);
        assert_eq!(
            wakes(&hub, channel.id).await,
            vec![WakeEvent::Routine {
                id: routine.id,
                name: "Morning brief".to_owned(),
                prompt: "Summarize my day.".to_owned(),
            }]
        );
        assert_eq!(runs(&hub, routine.id).await, vec![run.clone()]);
        let saved = load(&hub, routine.id).await;
        assert_eq!(saved.last_run_at, Some(run.started_at));
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Running));
        assert!(saved.enabled);
        // The next run keeps the rhythm of the run that was due.
        assert_eq!(
            saved.next_run_at,
            Some(routine.next_run_at.unwrap().plus_millis(HOUR_MS))
        );
    }

    #[tokio::test]
    async fn a_run_adds_a_run_and_a_routine_entry_to_the_feed() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let routine = due(
            channel.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            hub.now(),
        );
        save(&hub, &routine).await;
        let mut feed = hub.store.subscribe();

        let run = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();

        let first = feed.recv().await.unwrap();
        assert_eq!(first.entity_id, run.id.to_string());
        assert_eq!(
            first.change.kind(),
            consort_proto::client_api::EntityKind::RoutineRun
        );
        assert_eq!(first.channel_id, Some(channel.id));
        let second = feed.recv().await.unwrap();
        assert_eq!(second.entity_id, routine.id.to_string());
    }

    #[tokio::test]
    async fn a_manual_run_does_not_change_the_schedule() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let later = hub.now().plus_millis(10 * HOUR_MS);
        let mut routine = routine(channel.id, interval(3_600), RoutineAction::CoordinatorTurn);
        routine.next_run_at = Some(later);
        save(&hub, &routine).await;

        let run = hub.run_routine(&routine, RunTrigger::Manual).await.unwrap();

        assert!(run.manual);
        assert_eq!(wakes(&hub, channel.id).await.len(), 1);
        assert_eq!(runs(&hub, routine.id).await.len(), 1);
        let saved = load(&hub, routine.id).await;
        assert_eq!(saved.next_run_at, Some(later));
        assert!(saved.enabled);
        assert_eq!(saved.last_run_at, Some(run.started_at));
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Running));
    }

    #[tokio::test]
    async fn a_manual_run_works_for_a_routine_that_is_turned_off() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let mut routine = routine(channel.id, interval(3_600), RoutineAction::CoordinatorTurn);
        routine.enabled = false;
        save(&hub, &routine).await;

        hub.run_routine(&routine, RunTrigger::Manual).await.unwrap();

        assert_eq!(wakes(&hub, channel.id).await.len(), 1);
        let saved = load(&hub, routine.id).await;
        assert!(!saved.enabled);
        assert_eq!(saved.next_run_at, None);
    }

    #[tokio::test]
    async fn a_once_routine_turns_itself_off_after_it_runs() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let now = hub.now();
        let at = now.plus_millis(-1_000);
        let mut routine = routine(
            channel.id,
            RoutineSchedule::Once { at },
            RoutineAction::CoordinatorTurn,
        );
        routine.next_run_at = Some(at);
        save(&hub, &routine).await;

        hub.run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();

        let saved = load(&hub, routine.id).await;
        assert!(!saved.enabled);
        assert_eq!(saved.next_run_at, None);
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Running));
        assert_eq!(wakes(&hub, channel.id).await.len(), 1);
    }

    #[tokio::test]
    async fn a_manual_run_of_a_once_routine_keeps_it_on() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let at = hub.now().plus_millis(HOUR_MS);
        let mut routine = routine(
            channel.id,
            RoutineSchedule::Once { at },
            RoutineAction::CoordinatorTurn,
        );
        routine.next_run_at = Some(at);
        save(&hub, &routine).await;

        hub.run_routine(&routine, RunTrigger::Manual).await.unwrap();

        let saved = load(&hub, routine.id).await;
        assert!(saved.enabled);
        assert_eq!(saved.next_run_at, Some(at));
    }

    #[tokio::test]
    async fn a_run_less_than_an_hour_late_runs_once_and_the_next_run_is_in_the_future() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let now = hub.now();
        // The hub was down for 50 minutes.
        let mut routine = routine(
            channel.id,
            RoutineSchedule::Cron {
                expression: "*/10 * * * *".to_owned(),
                time_zone: "America/Chicago".to_owned(),
            },
            RoutineAction::CoordinatorTurn,
        );
        routine.next_run_at = Some(now.plus_millis(-50 * 60_000));
        save(&hub, &routine).await;

        tick(&hub).await;
        // A second tick finds nothing due.
        tick(&hub).await;

        assert_eq!(wakes(&hub, channel.id).await.len(), 1);
        let recorded = runs(&hub, routine.id).await;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].status, RoutineRunStatus::Running);
        let next = load(&hub, routine.id).await.next_run_at.unwrap();
        assert!(next > now, "{next:?} should be after {now:?}");
        assert!(next <= now.plus_millis(10 * 60_000), "{next:?}");
    }

    #[tokio::test]
    async fn a_run_an_hour_late_or_more_is_skipped_and_the_next_run_is_in_the_future() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let now = hub.now();
        // The hub was down for 30 days. A cron routine would have run 30 times.
        let mut routine = routine(
            channel.id,
            RoutineSchedule::Cron {
                expression: "0 8 * * *".to_owned(),
                time_zone: "America/Chicago".to_owned(),
            },
            RoutineAction::CoordinatorTurn,
        );
        let missed = now.plus_millis(-30 * 24 * HOUR_MS);
        routine.next_run_at = Some(missed);
        save(&hub, &routine).await;

        tick(&hub).await;
        tick(&hub).await;

        assert!(wakes(&hub, channel.id).await.is_empty());
        let recorded = runs(&hub, routine.id).await;
        assert_eq!(recorded.len(), 1);
        let skipped = &recorded[0];
        assert_eq!(skipped.status, RoutineRunStatus::Skipped);
        assert_eq!(skipped.started_at, missed);
        assert!(skipped.error.as_deref().unwrap().contains("offline"));
        let saved = load(&hub, routine.id).await;
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Skipped));
        assert_eq!(saved.last_run_at, Some(missed));
        let next = saved.next_run_at.unwrap();
        assert!(next > now, "{next:?} should be after {now:?}");
        // A daily routine runs again within a day and a bit (daylight saving).
        assert!(next <= now.plus_millis(25 * HOUR_MS), "{next:?}");
    }

    #[tokio::test]
    async fn a_scheduled_run_while_the_last_run_still_runs_is_skipped() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let routine = due(
            channel.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            hub.now(),
        );
        save(&hub, &routine).await;
        let first = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();
        // The coordinator has not taken the prompt yet, and the routine is
        // due again.
        let mut again = load(&hub, routine.id).await;
        again.next_run_at = Some(hub.now().plus_millis(-1_000));
        save(&hub, &again).await;

        let second = hub.run_routine(&again, RunTrigger::Schedule).await.unwrap();

        assert_eq!(second.status, RoutineRunStatus::Skipped);
        assert_eq!(
            second.error.as_deref(),
            Some("The last run was still going.")
        );
        assert_eq!(wakes(&hub, channel.id).await.len(), 1);
        let saved = load(&hub, routine.id).await;
        // The run that still runs stays the last run.
        assert_eq!(saved.last_run_at, Some(first.started_at));
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Running));
        assert!(saved.next_run_at.unwrap() > hub.now());
        // Run now says why it cannot start, and records nothing.
        let err = hub
            .run_routine(&again, RunTrigger::Manual)
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::Conflict);
        assert!(err.to_string().contains("has not ended"), "{err}");
        assert_eq!(runs(&hub, routine.id).await.len(), 2);
    }

    #[tokio::test]
    async fn a_run_whose_prompt_left_the_queue_without_a_turn_ends_and_does_not_block() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let routine = due(
            channel.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            hub.now(),
        );
        save(&hub, &routine).await;
        let first = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();
        // The queue loses the prompt, as when the channel was cleared.
        hub.store
            .write(move |tx| {
                let ids: Vec<i64> = coordinator::pending_wakes(tx.db(), channel.id)?
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();
                tx.delete_wakes(&ids)
            })
            .await
            .unwrap();

        let second = hub.run_routine(&routine, RunTrigger::Manual).await.unwrap();

        assert_eq!(second.status, RoutineRunStatus::Running);
        let all = runs(&hub, routine.id).await;
        let old = all.iter().find(|r| r.id == first.id).unwrap();
        assert_eq!(old.status, RoutineRunStatus::Error);
        assert!(old.ended_at.is_some());
    }

    #[tokio::test]
    async fn an_interval_routine_keeps_the_rhythm_of_its_save_time() {
        let due_at = Timestamp::from_millis(1_000_000);
        // Ten minutes late with a 4 minute interval: the next run is the
        // first multiple after now, counted from the run that was due.
        let now = due_at.plus_millis(10 * 60_000);
        assert_eq!(
            next_interval_run(due_at, 240, now),
            Some(due_at.plus_millis(12 * 60_000))
        );
        // On time: one interval later.
        assert_eq!(
            next_interval_run(due_at, 240, due_at),
            Some(due_at.plus_millis(4 * 60_000))
        );
        assert_eq!(next_interval_run(due_at, 0, now), None);
    }

    #[tokio::test]
    async fn a_start_task_run_creates_the_task_and_ends_when_the_task_is_ready() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let routine = due(channel.id, interval(3_600), start_task(), hub.now());
        save(&hub, &routine).await;

        let run = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();

        assert_eq!(run.error, None);
        assert_eq!(run.status, RoutineRunStatus::Running);
        let task_id = run.task_id.unwrap();
        let task = hub
            .store
            .read(move |conn| crate::store::work::task_or_404(conn, task_id))
            .await
            .unwrap();
        assert_eq!(task.title, "Nightly check");
        assert_eq!(task.created_by, consort_proto::model::TaskCreator::Routine);
        assert_eq!(runs(&hub, routine.id).await, vec![run.clone()]);
        let saved = load(&hub, routine.id).await;
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Running));
        assert_eq!(
            saved.next_run_at,
            Some(routine.next_run_at.unwrap().plus_millis(HOUR_MS))
        );
        // A task routine does not wake the coordinator.
        assert!(wakes(&hub, channel.id).await.is_empty());

        // The task becomes ready for review: the run ends well.
        hub.store
            .write(move |tx| {
                let mut task = crate::store::work::task_or_404(tx.db(), task_id)?;
                crate::ops::flow::transition(
                    tx,
                    &mut task,
                    consort_proto::status::TaskStatus::Working,
                    consort_proto::status::Actor::System,
                    None,
                    None,
                )?;
                crate::ops::flow::transition(
                    tx,
                    &mut task,
                    consort_proto::status::TaskStatus::Review,
                    consort_proto::status::Actor::System,
                    None,
                    None,
                )?;
                tx.save_task(&task)
            })
            .await
            .unwrap();
        let ended = runs(&hub, routine.id).await.remove(0);
        assert_eq!(ended.status, RoutineRunStatus::Ok);
        assert!(ended.ended_at.is_some());
        assert_eq!(
            load(&hub, routine.id).await.last_run_status,
            Some(RoutineRunStatus::Ok)
        );
    }

    #[tokio::test]
    async fn a_start_task_run_records_the_error_when_the_task_cannot_be_created() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let action = RoutineAction::StartTask {
            title: "  ".to_owned(),
            model_selection: None,
            placement: PlacementRequest::default(),
        };
        let routine = due(channel.id, interval(3_600), action, hub.now());
        save(&hub, &routine).await;

        let run = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();

        assert!(run.error.is_some());
        assert_eq!(run.status, RoutineRunStatus::Error);
        assert_eq!(run.task_id, None);
        let saved = load(&hub, routine.id).await;
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Error));
        // The schedule moved on, so the failure does not repeat every second.
        assert_eq!(
            saved.next_run_at,
            Some(routine.next_run_at.unwrap().plus_millis(HOUR_MS))
        );
        assert!(saved.enabled);
    }

    #[tokio::test]
    async fn a_manual_start_task_run_keeps_the_schedule() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let later = hub.now().plus_millis(HOUR_MS);
        let mut routine = routine(channel.id, interval(3_600), start_task());
        routine.next_run_at = Some(later);
        save(&hub, &routine).await;

        let run = hub.run_routine(&routine, RunTrigger::Manual).await.unwrap();

        assert!(run.task_id.is_some());
        assert!(run.manual);
        let saved = load(&hub, routine.id).await;
        assert_eq!(saved.next_run_at, Some(later));
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Running));
    }

    #[tokio::test]
    async fn a_deleted_task_ends_its_run() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let routine = due(channel.id, interval(3_600), start_task(), hub.now());
        save(&hub, &routine).await;
        let run = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();
        let task_id = run.task_id.unwrap();

        hub.store
            .write(move |tx| {
                let task = crate::store::work::task_or_404(tx.db(), task_id)?;
                tx.purge_task(&task)
            })
            .await
            .unwrap();

        let ended = runs(&hub, routine.id).await.remove(0);
        assert_eq!(ended.status, RoutineRunStatus::Error);
        assert!(ended.error.unwrap().contains("deleted"));
    }

    #[tokio::test]
    async fn a_routine_in_an_archived_channel_does_not_run() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, true).await;
        let now = hub.now();
        let routine = due(
            channel.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            now,
        );
        save(&hub, &routine).await;

        let err = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap_err();

        assert_eq!(err.code(), ErrorCode::Conflict);
        assert!(wakes(&hub, channel.id).await.is_empty());
        assert!(runs(&hub, routine.id).await.is_empty());
        let saved = load(&hub, routine.id).await;
        // The next run moved on and nothing else changed.
        assert!(saved.next_run_at.unwrap() > now);
        assert_eq!(saved.last_run_at, None);
        assert_eq!(saved.last_run_status, None);
        assert!(saved.enabled);
    }

    #[tokio::test]
    async fn a_manual_run_in_an_archived_channel_changes_nothing() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, true).await;
        let later = hub.now().plus_millis(HOUR_MS);
        let mut routine = routine(channel.id, interval(3_600), RoutineAction::CoordinatorTurn);
        routine.next_run_at = Some(later);
        save(&hub, &routine).await;

        let err = hub
            .run_routine(&routine, RunTrigger::Manual)
            .await
            .unwrap_err();

        assert_eq!(err.code(), ErrorCode::Conflict);
        assert!(wakes(&hub, channel.id).await.is_empty());
        assert!(runs(&hub, routine.id).await.is_empty());
        assert_eq!(load(&hub, routine.id).await, routine);
    }

    #[tokio::test]
    async fn a_routine_in_a_missing_channel_does_not_run() {
        let (hub, _dir) = test_hub().await;
        let missing = ChannelId::new();
        let routine = due(
            missing,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            hub.now(),
        );
        save(&hub, &routine).await;

        let err = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap_err();

        assert_eq!(err.code(), ErrorCode::Conflict);
        assert!(wakes(&hub, missing).await.is_empty());
        assert!(runs(&hub, routine.id).await.is_empty());
    }

    #[tokio::test]
    async fn a_routine_with_a_schedule_that_is_not_valid_turns_off_and_says_why() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let routine = due(
            channel.id,
            RoutineSchedule::Cron {
                expression: "0 8 * * *".to_owned(),
                time_zone: "Mars/Olympus".to_owned(),
            },
            RoutineAction::CoordinatorTurn,
            hub.now(),
        );
        save(&hub, &routine).await;

        let run = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();

        // The action still ran, and the run says what went wrong.
        assert_eq!(wakes(&hub, channel.id).await.len(), 1);
        let error = run.error.clone().unwrap();
        assert!(error.contains("Mars/Olympus"), "{error}");
        let saved = load(&hub, routine.id).await;
        assert!(!saved.enabled);
        assert_eq!(saved.next_run_at, None);
        // The turn ends, and the run that holds the error did not end well.
        end_running(&hub, routine.id).await;
        assert_eq!(
            load(&hub, routine.id).await.last_run_status,
            Some(RoutineRunStatus::Error)
        );
    }

    #[tokio::test]
    async fn a_stale_copy_does_not_run_a_routine_that_was_turned_off() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let routine = due(
            channel.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            hub.now(),
        );
        save(&hub, &routine).await;
        // The user turns the routine off after the scheduler read it.
        let mut off = routine.clone();
        off.enabled = false;
        save(&hub, &off).await;

        let err = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap_err();

        assert_eq!(err.code(), ErrorCode::Conflict);
        assert!(wakes(&hub, channel.id).await.is_empty());
        assert_eq!(load(&hub, routine.id).await, off);
    }

    #[tokio::test]
    async fn a_run_uses_the_stored_prompt_and_keeps_stored_edits() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let routine = due(
            channel.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            hub.now(),
        );
        save(&hub, &routine).await;
        let mut edited = routine.clone();
        edited.prompt = "A new prompt.".to_owned();
        edited.name = "Renamed".to_owned();
        save(&hub, &edited).await;

        hub.run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();

        assert_eq!(
            wakes(&hub, channel.id).await,
            vec![WakeEvent::Routine {
                id: routine.id,
                name: "Renamed".to_owned(),
                prompt: "A new prompt.".to_owned(),
            }]
        );
        let saved = load(&hub, routine.id).await;
        assert_eq!(saved.prompt, "A new prompt.");
        assert_eq!(saved.name, "Renamed");
    }

    #[tokio::test]
    async fn a_deleted_routine_is_not_found() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let routine = due(
            channel.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            hub.now(),
        );

        let err = hub
            .run_routine(&routine, RunTrigger::Manual)
            .await
            .unwrap_err();

        assert_eq!(err.code(), ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn a_tick_runs_only_routines_that_are_due_and_enabled() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let now = hub.now();

        let ready = due(
            channel.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            now,
        );
        let mut off = due(
            channel.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            now,
        );
        off.enabled = false;
        let mut later = routine(channel.id, interval(3_600), RoutineAction::CoordinatorTurn);
        later.next_run_at = Some(now.plus_millis(HOUR_MS));
        let no_time = routine(channel.id, interval(3_600), RoutineAction::CoordinatorTurn);
        for r in [&ready, &off, &later, &no_time] {
            save(&hub, r).await;
        }

        tick(&hub).await;

        assert_eq!(runs(&hub, ready.id).await.len(), 1);
        for other in [&off, &later, &no_time] {
            assert!(runs(&hub, other.id).await.is_empty());
            assert_eq!(load(&hub, other.id).await, *other);
        }
        assert_eq!(wakes(&hub, channel.id).await.len(), 1);

        // The routine moved on, so the next tick does nothing.
        tick(&hub).await;
        assert_eq!(runs(&hub, ready.id).await.len(), 1);
    }

    #[tokio::test]
    async fn a_failed_routine_does_not_stop_the_tick() {
        let (hub, _dir) = test_hub().await;
        let archived = channel(&hub, true).await;
        let open = channel(&hub, false).await;
        let now = hub.now();
        // The first routine is the oldest, so it runs first and fails.
        let mut first = due(
            archived.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            now,
        );
        first.next_run_at = Some(now.plus_millis(-120_000));
        let second = due(
            open.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            now,
        );
        let third = due(open.id, interval(3_600), start_task(), now);
        for r in [&first, &second, &third] {
            save(&hub, r).await;
        }

        tick(&hub).await;

        assert!(runs(&hub, first.id).await.is_empty());
        assert_eq!(runs(&hub, second.id).await.len(), 1);
        assert_eq!(runs(&hub, third.id).await.len(), 1);
    }

    #[tokio::test]
    async fn the_loop_runs_due_routines_and_stops_on_shutdown() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let routine = due(
            channel.id,
            interval(3_600),
            RoutineAction::CoordinatorTurn,
            hub.now(),
        );
        save(&hub, &routine).await;

        let handle = spawn(&hub);

        // The first tick runs at once.
        tokio::time::timeout(Duration::from_secs(5), async {
            while runs(&hub, routine.id).await.is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the loop should run the due routine");

        hub.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("the loop should stop")
            .unwrap();
        assert_eq!(runs(&hub, routine.id).await.len(), 1);
    }

    #[test]
    fn advance_turns_a_once_routine_off() {
        let mut routine = routine(
            ChannelId::new(),
            RoutineSchedule::Once {
                at: Timestamp::EPOCH,
            },
            RoutineAction::CoordinatorTurn,
        );
        routine.next_run_at = Some(Timestamp::EPOCH);
        assert_eq!(advance(&mut routine, Timestamp::from_millis(5)), None);
        assert!(!routine.enabled);
        assert_eq!(routine.next_run_at, None);
    }
}
