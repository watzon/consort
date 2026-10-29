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
//! - A routine that was due for a long time (the hub was down) runs once.
//!   Its next run is the first one after now. There is no catch-up.
//! - A `once` routine turns itself off after it runs.
//! - A `start_task` run moves the routine to its next run before it creates
//!   the task. A crash in between loses one run and never makes two tasks.

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
use crate::store::{Tx, conversation, routines};

/// How often the scheduler looks for due routines.
const TICK: Duration = Duration::from_secs(1);

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
    let now = hub.now();
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
        match hub.run_routine(&routine, RunTrigger::Schedule).await {
            Ok(run) => {
                tracing::info!(
                    routine = %routine.id,
                    name = %routine.name,
                    failed = run.error.is_some(),
                    "a routine ran"
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
    /// The routine did not run. The text says why.
    Skipped(&'static str),
    /// A `coordinator_turn` run is complete. The caller must kick the
    /// coordinator of the channel.
    Woke {
        run: RoutineRun,
        channel_id: ChannelId,
    },
    /// A `start_task` run must create its task.
    Task {
        new_task: Box<NewTask>,
        started_at: Timestamp,
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
    /// routine did not run: its channel is archived or missing, or (for
    /// [`RunTrigger::Schedule`]) it is no longer due. It returns `not_found`
    /// when the routine does not exist any more.
    pub(crate) async fn run_routine(
        &self,
        routine: &Routine,
        trigger: RunTrigger,
    ) -> Result<RoutineRun> {
        let id = routine.id;
        let begun = self
            .store
            .write(move |tx| begin_run(tx, id, trigger))
            .await?;
        match begun {
            Begun::Skipped(reason) => Err(HubError::conflict(reason)),
            Begun::Woke { run, channel_id } => {
                self.kick_coordinator(channel_id);
                Ok(run)
            }
            Begun::Task {
                new_task,
                started_at,
                schedule_error,
            } => {
                self.finish_task_run(id, *new_task, started_at, schedule_error)
                    .await
            }
        }
    }

    /// Creates the task of a `start_task` run, then records the run.
    async fn finish_task_run(
        &self,
        routine_id: RoutineId,
        new_task: NewTask,
        started_at: Timestamp,
        schedule_error: Option<String>,
    ) -> Result<RoutineRun> {
        let mut run = RoutineRun {
            routine_id,
            started_at,
            message_id: None,
            task_id: None,
            error: None,
        };
        match self.create_task(new_task).await {
            Ok(task) => run.task_id = Some(task.id),
            Err(err) => {
                tracing::warn!(routine = %routine_id, error = %err, "a routine could not start its task");
                run.error = Some(err.to_error_body().message);
            }
        }
        add_schedule_error(&mut run, schedule_error);
        let recorded = run.clone();
        self.store
            .write(move |tx| {
                // The user may have deleted the routine while the task was
                // created. Then there is nothing to record. If the routine
                // changed, only the fields of the run change.
                if let Some(mut routine) = routines::routine(tx.db(), routine_id)? {
                    note_last_run(&mut routine, &run);
                    tx.record_routine_run(&run)?;
                    tx.save_routine(&routine)?;
                }
                Ok(())
            })
            .await?;
        Ok(recorded)
    }
}

/// The first transaction of a run. It reads the routine, decides whether it
/// runs, and for a `coordinator_turn` does all the work. For a `start_task` it
/// only moves the routine to its next run.
fn begin_run(tx: &mut Tx<'_>, id: RoutineId, trigger: RunTrigger) -> Result<Begun> {
    let now = tx.now();
    let mut routine = routines::routine_or_404(tx.db(), id)?;
    if trigger == RunTrigger::Schedule && !is_due(&routine, now) {
        return Ok(Begun::Skipped(
            "The routine is turned off or not due. It did not run.",
        ));
    }
    let channel_is_open = conversation::channel(tx.db(), routine.channel_id)?
        .is_some_and(|channel| channel.archived_at.is_none());
    if !channel_is_open {
        // Move on, so the scheduler does not look at this routine every second.
        if trigger == RunTrigger::Schedule {
            advance(&mut routine, now);
            tx.save_routine(&routine)?;
        }
        return Ok(Begun::Skipped(
            "The channel of the routine is archived or missing. The routine did not run.",
        ));
    }

    let scheduled = trigger == RunTrigger::Schedule;
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
            let mut run = new_run(id, now);
            if scheduled {
                add_schedule_error(&mut run, advance(&mut routine, now));
            }
            note_last_run(&mut routine, &run);
            tx.record_routine_run(&run)?;
            tx.save_routine(&routine)?;
            Ok(Begun::Woke {
                run,
                channel_id: routine.channel_id,
            })
        }
        RoutineAction::StartTask {
            title,
            model_selection,
            placement,
        } => {
            let schedule_error = if scheduled {
                let error = advance(&mut routine, now);
                tx.save_routine(&routine)?;
                error
            } else {
                None
            };
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
                }),
                started_at: now,
                schedule_error,
            })
        }
    }
}

fn new_run(routine_id: RoutineId, started_at: Timestamp) -> RoutineRun {
    RoutineRun {
        routine_id,
        started_at,
        message_id: None,
        task_id: None,
        error: None,
    }
}

fn is_due(routine: &Routine, now: Timestamp) -> bool {
    routine.enabled && routine.next_run_at.is_some_and(|at| at <= now)
}

/// Moves a routine past a scheduled run at `now`. The next run is the first
/// one after `now`, so a routine that was due for a long time runs once.
///
/// A `once` routine turns off. A routine whose stored schedule is not valid
/// any more (for example a time zone name that the hub no longer knows) also
/// turns off. In that case the function returns the reason.
fn advance(routine: &mut Routine, now: Timestamp) -> Option<String> {
    if matches!(routine.schedule, RoutineSchedule::Once { .. }) {
        routine.enabled = false;
        routine.next_run_at = None;
        return None;
    }
    match next_run_at(&routine.schedule, now) {
        Ok(next) => {
            routine.next_run_at = next;
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

/// Adds the reason from [`advance`] to the error of a run.
fn add_schedule_error(run: &mut RoutineRun, schedule_error: Option<String>) {
    let Some(reason) = schedule_error else {
        return;
    };
    let text =
        format!("The hub turned the routine off because its schedule is not valid. {reason}");
    run.error = Some(match run.error.take() {
        Some(previous) => format!("{previous} {text}"),
        None => text,
    });
}

/// Copies the result of a run to the routine.
fn note_last_run(routine: &mut Routine, run: &RoutineRun) {
    routine.last_run_at = Some(run.started_at);
    routine.last_run_status = Some(if run.error.is_some() {
        RoutineRunStatus::Error
    } else {
        RoutineRunStatus::Ok
    });
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
            archived_at: archived.then_some(now),
            created_at: now,
            updated_at: now,
            tags: Vec::new(),
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
    use super::testing::{channel, load, routine, runs, save, wakes};
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
    async fn a_coordinator_turn_queues_one_wake_records_one_run_and_advances() {
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
        assert_eq!(run.error, None);
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
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Ok));
        assert!(saved.enabled);
        // The next run is one interval after the run.
        assert_eq!(saved.next_run_at, Some(run.started_at.plus_millis(HOUR_MS)));
    }

    #[tokio::test]
    async fn a_run_adds_a_routine_entry_to_the_feed() {
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

        hub.run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();

        let entry = feed.recv().await.unwrap();
        assert_eq!(entry.entity_id, routine.id.to_string());
        assert_eq!(entry.channel_id, Some(channel.id));
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

        assert_eq!(wakes(&hub, channel.id).await.len(), 1);
        assert_eq!(runs(&hub, routine.id).await.len(), 1);
        let saved = load(&hub, routine.id).await;
        assert_eq!(saved.next_run_at, Some(later));
        assert!(saved.enabled);
        assert_eq!(saved.last_run_at, Some(run.started_at));
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Ok));
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
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Ok));
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
    async fn a_missed_run_runs_once_and_the_next_run_is_in_the_future() {
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
        routine.next_run_at = Some(now.plus_millis(-30 * 24 * HOUR_MS));
        save(&hub, &routine).await;

        tick(&hub).await;
        // A second tick finds nothing due.
        tick(&hub).await;

        assert_eq!(wakes(&hub, channel.id).await.len(), 1);
        assert_eq!(runs(&hub, routine.id).await.len(), 1);
        let next = load(&hub, routine.id).await.next_run_at.unwrap();
        assert!(next > now, "{next:?} should be after {now:?}");
        // A daily routine runs again within a day and a bit (daylight saving).
        assert!(next <= now.plus_millis(25 * HOUR_MS), "{next:?}");
    }

    #[tokio::test]
    async fn a_start_task_run_creates_the_task_and_records_it() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let routine = due(channel.id, interval(3_600), start_task(), hub.now());
        save(&hub, &routine).await;

        let run = hub
            .run_routine(&routine, RunTrigger::Schedule)
            .await
            .unwrap();

        assert_eq!(run.error, None);
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
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Ok));
        assert_eq!(saved.next_run_at, Some(run.started_at.plus_millis(HOUR_MS)));
        // A task routine does not wake the coordinator.
        assert!(wakes(&hub, channel.id).await.is_empty());
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
        assert_eq!(run.task_id, None);
        let saved = load(&hub, routine.id).await;
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Error));
        // The schedule moved on, so the failure does not repeat every second.
        assert_eq!(saved.next_run_at, Some(run.started_at.plus_millis(HOUR_MS)));
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
        let saved = load(&hub, routine.id).await;
        assert_eq!(saved.next_run_at, Some(later));
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Ok));
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
        assert_eq!(saved.last_run_status, Some(RoutineRunStatus::Error));
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
