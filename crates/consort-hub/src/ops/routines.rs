//! Client API handlers for routines. Owned by agent B.
//!
//! The handlers and the routine hub tools share three operations:
//! [`Hub::create_routine_in`], [`Hub::update_routine_in`] and
//! [`Hub::delete_routine_in`]. They hold the rules, so a client and the
//! assistant cannot make a routine that the other could not.

use consort_proto::client_api::{
    RoutineCreate, RoutineDelete, RoutineRunNow, RoutineRuns, RoutineRunsResult, RoutineUpdate,
};
use consort_proto::frame::Empty;
use consort_proto::ids::{ChannelId, RoutineId};
use consort_proto::model::{Routine, RoutineAction, RoutineSchedule};
use consort_proto::time::Timestamp;
use consort_proto::tools::RoutineResult;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::schedule::{next_run_at, validate_schedule};
use crate::scheduler::RunTrigger;
use crate::store::{conversation, routines};

/// The longest routine name, in characters.
const MAX_NAME_CHARS: usize = 120;

/// The number of runs that `routine.runs` returns when the client sends no
/// limit. This is the default that `RoutineRuns` documents.
const DEFAULT_RUNS_LIMIT: u32 = 20;

impl Hub {
    pub(crate) async fn routine_create(
        &self,
        _caller: &Caller,
        request: RoutineCreate,
    ) -> Result<RoutineResult> {
        let routine = self
            .create_routine_in(
                request.channel_id,
                request.name,
                request.schedule,
                request.prompt,
                request.action,
                request.enabled,
            )
            .await?;
        Ok(RoutineResult {
            routine,
            notes: Vec::new(),
        })
    }

    pub(crate) async fn routine_update(
        &self,
        _caller: &Caller,
        request: RoutineUpdate,
    ) -> Result<RoutineResult> {
        let routine = self.update_routine_in(None, request).await?;
        Ok(RoutineResult {
            routine,
            notes: Vec::new(),
        })
    }

    pub(crate) async fn routine_delete(
        &self,
        _caller: &Caller,
        request: RoutineDelete,
    ) -> Result<Empty> {
        self.delete_routine_in(None, request.routine_id).await?;
        Ok(Empty {})
    }

    pub(crate) async fn routine_run_now(
        &self,
        _caller: &Caller,
        request: RoutineRunNow,
    ) -> Result<Empty> {
        let id = request.routine_id;
        let routine = self
            .store
            .read(move |conn| routines::routine_or_404(conn, id))
            .await?;
        self.run_routine(&routine, RunTrigger::Manual).await?;
        Ok(Empty {})
    }

    pub(crate) async fn routine_runs(
        &self,
        _caller: &Caller,
        request: RoutineRuns,
    ) -> Result<RoutineRunsResult> {
        let id = request.routine_id;
        let limit =
            usize::try_from(request.limit.unwrap_or(DEFAULT_RUNS_LIMIT)).unwrap_or(usize::MAX);
        let runs = self
            .store
            .read(move |conn| {
                routines::routine_or_404(conn, id)?;
                routines::routine_runs(conn, id, limit)
            })
            .await?;
        Ok(RoutineRunsResult { runs })
    }

    /// Creates a routine in a channel. The channel must exist and must not be
    /// archived. An enabled routine gets its first `next_run_at`.
    pub(crate) async fn create_routine_in(
        &self,
        channel_id: ChannelId,
        name: String,
        schedule: RoutineSchedule,
        prompt: String,
        action: RoutineAction,
        enabled: bool,
    ) -> Result<Routine> {
        let name = name.trim().to_owned();
        check_text(&name, &prompt, &action)?;
        validate_schedule(&schedule)?;
        self.store
            .write(move |tx| {
                let now = tx.now();
                let channel = conversation::channel_or_404(tx.db(), channel_id)?;
                if channel.archived_at.is_some() {
                    return Err(HubError::conflict(format!(
                        "The channel {} is archived. Restore the channel before you add a routine.",
                        channel.name
                    )));
                }
                // A one-time routine in the past is a mistake even when it is off.
                require_future_once(&schedule, now)?;
                let next_run_at = if enabled {
                    Some(first_run(&schedule, now)?)
                } else {
                    None
                };
                let routine = Routine {
                    id: RoutineId::new(),
                    channel_id,
                    name,
                    schedule,
                    prompt,
                    action,
                    enabled,
                    next_run_at,
                    last_run_at: None,
                    last_run_status: None,
                };
                tx.save_routine(&routine)?;
                Ok(routine)
            })
            .await
    }

    /// Changes a routine. Fields that the update leaves out do not change.
    ///
    /// `scope` limits the change to the routines of one channel. A routine in
    /// another channel is then `not_found`. A hub tool passes the channel of
    /// its session. A client passes `None`.
    ///
    /// The routine gets a new `next_run_at` (counted from now) when the
    /// schedule changes or when `enabled` changes. Turning a routine off
    /// clears it.
    pub(crate) async fn update_routine_in(
        &self,
        scope: Option<ChannelId>,
        update: RoutineUpdate,
    ) -> Result<Routine> {
        if let Some(schedule) = &update.schedule {
            validate_schedule(schedule)?;
        }
        self.store
            .write(move |tx| {
                let now = tx.now();
                let mut routine = routine_in_scope(tx.db(), scope, update.routine_id)?;

                let schedule_changed = update
                    .schedule
                    .as_ref()
                    .is_some_and(|schedule| *schedule != routine.schedule);
                let enabled_changed = update
                    .enabled
                    .is_some_and(|enabled| enabled != routine.enabled);

                if let Some(name) = update.name {
                    routine.name = name.trim().to_owned();
                }
                if let Some(schedule) = update.schedule {
                    routine.schedule = schedule;
                }
                if let Some(prompt) = update.prompt {
                    routine.prompt = prompt;
                }
                if let Some(action) = update.action {
                    routine.action = action;
                }
                if let Some(enabled) = update.enabled {
                    routine.enabled = enabled;
                }
                check_text(&routine.name, &routine.prompt, &routine.action)?;

                if schedule_changed {
                    require_future_once(&routine.schedule, now)?;
                }
                if schedule_changed || enabled_changed {
                    routine.next_run_at = if routine.enabled {
                        Some(first_run(&routine.schedule, now)?)
                    } else {
                        None
                    };
                }
                tx.save_routine(&routine)?;
                Ok(routine)
            })
            .await
    }

    /// Deletes a routine and its runs. `scope` works as in
    /// [`Hub::update_routine_in`].
    pub(crate) async fn delete_routine_in(
        &self,
        scope: Option<ChannelId>,
        routine_id: RoutineId,
    ) -> Result<()> {
        self.store
            .write(move |tx| {
                let routine = routine_in_scope(tx.db(), scope, routine_id)?;
                tx.delete_routine(&routine)
            })
            .await
    }
}

/// Reads a routine. A routine outside `scope` is `not_found`, the same as a
/// routine that does not exist, so a session cannot probe other channels.
fn routine_in_scope(
    conn: &rusqlite::Connection,
    scope: Option<ChannelId>,
    id: RoutineId,
) -> Result<Routine> {
    let routine = routines::routine_or_404(conn, id)?;
    if scope.is_some_and(|channel_id| channel_id != routine.channel_id) {
        return Err(HubError::not_found(format!(
            "The routine {id} does not exist in this channel."
        )));
    }
    Ok(routine)
}

/// Checks the text fields of a routine.
fn check_text(name: &str, prompt: &str, action: &RoutineAction) -> Result<()> {
    if name.trim().is_empty() {
        return Err(HubError::invalid(
            "The name of the routine is empty. Give the routine a name.",
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(HubError::invalid(format!(
            "The name of the routine is longer than {MAX_NAME_CHARS} characters. Use a shorter name."
        )));
    }
    if prompt.trim().is_empty() {
        return Err(HubError::invalid(
            "The prompt of the routine is empty. Write what the routine must do.",
        ));
    }
    // A start_task routine makes the prompt the brief of its task.
    crate::limits::check_text("prompt", prompt, crate::limits::BRIEF_MAX_BYTES)?;
    if let RoutineAction::StartTask { title, .. } = action
        && title.trim().is_empty()
    {
        return Err(HubError::invalid(
            "The task title of the routine is empty. Give the task a title.",
        ));
    }
    Ok(())
}

/// A one-time routine must run in the future.
fn require_future_once(schedule: &RoutineSchedule, now: Timestamp) -> Result<()> {
    if let RoutineSchedule::Once { at } = schedule
        && *at <= now
    {
        return Err(HubError::invalid(format!(
            "The time of a one-time routine ({at}) is in the past. Choose a time in the future."
        )));
    }
    Ok(())
}

/// The first run of an enabled routine, counted from `now`. Only a one-time
/// routine that is already past has none.
fn first_run(schedule: &RoutineSchedule, now: Timestamp) -> Result<Timestamp> {
    next_run_at(schedule, now)?.ok_or_else(|| {
        HubError::invalid(
            "The one-time routine has no run in the future. Set a new time in the schedule.",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::test_hub;
    use crate::scheduler::testing::{channel, load, routine, runs, save, wakes};
    use consort_proto::error::ErrorCode;
    use consort_proto::model::{PlacementRequest, RoutineRun};
    use consort_proto::turn_input::WakeEvent;

    const HOUR_MS: i64 = 3_600_000;

    fn interval(seconds: u64) -> RoutineSchedule {
        RoutineSchedule::Interval { seconds }
    }

    fn cron(expression: &str) -> RoutineSchedule {
        RoutineSchedule::Cron {
            expression: expression.to_owned(),
            time_zone: "America/Chicago".to_owned(),
        }
    }

    fn start_task(title: &str) -> RoutineAction {
        RoutineAction::StartTask {
            title: title.to_owned(),
            model_selection: None,
            placement: PlacementRequest::default(),
        }
    }

    fn create_request(channel_id: ChannelId) -> RoutineCreate {
        RoutineCreate {
            channel_id,
            name: "Morning brief".to_owned(),
            schedule: interval(3_600),
            prompt: "Summarize my day.".to_owned(),
            action: RoutineAction::CoordinatorTurn,
            enabled: true,
        }
    }

    fn update_request(routine_id: RoutineId) -> RoutineUpdate {
        RoutineUpdate {
            routine_id,
            name: None,
            schedule: None,
            prompt: None,
            action: None,
            enabled: None,
        }
    }

    async fn create(hub: &Hub, request: RoutineCreate) -> Result<Routine> {
        hub.routine_create(&Caller::Hub, request)
            .await
            .map(|result| result.routine)
    }

    async fn update(hub: &Hub, request: RoutineUpdate) -> Result<Routine> {
        hub.routine_update(&Caller::Hub, request)
            .await
            .map(|result| result.routine)
    }

    #[tokio::test]
    async fn create_saves_an_enabled_routine_with_its_first_run() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let before = hub.now();

        let created = create(&hub, create_request(channel.id)).await.unwrap();

        assert_eq!(created.channel_id, channel.id);
        assert!(created.enabled);
        assert_eq!(created.last_run_at, None);
        assert_eq!(created.last_run_status, None);
        let next = created.next_run_at.unwrap();
        assert!(next.as_millis() >= before.as_millis() + 3_600_000);
        assert!(next.as_millis() <= hub.now().as_millis() + 3_600_000);
        assert_eq!(load(&hub, created.id).await, created);
    }

    #[tokio::test]
    async fn create_in_a_disabled_state_has_no_next_run() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let mut request = create_request(channel.id);
        request.enabled = false;

        let created = create(&hub, request).await.unwrap();

        assert!(!created.enabled);
        assert_eq!(created.next_run_at, None);
    }

    #[tokio::test]
    async fn create_uses_the_time_zone_of_a_cron_schedule() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let mut request = create_request(channel.id);
        request.schedule = cron("0 8 * * *");

        let created = create(&hub, request).await.unwrap();

        let next = created.next_run_at.unwrap();
        assert!(next > hub.now());
        // 08:00 in Chicago is 13:00 or 14:00 UTC.
        let hour = (next.as_millis() / HOUR_MS) % 24;
        assert!(hour == 13 || hour == 14, "hour {hour}");
    }

    #[tokio::test]
    async fn create_trims_the_name() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let mut request = create_request(channel.id);
        request.name = "  Morning brief \n".to_owned();

        assert_eq!(create(&hub, request).await.unwrap().name, "Morning brief");
    }

    #[tokio::test]
    async fn create_checks_the_channel() {
        let (hub, _dir) = test_hub().await;
        let missing = create(&hub, create_request(ChannelId::new()))
            .await
            .unwrap_err();
        assert_eq!(missing.code(), ErrorCode::NotFound);

        let archived = channel(&hub, true).await;
        let err = create(&hub, create_request(archived.id)).await.unwrap_err();
        assert_eq!(err.code(), ErrorCode::Conflict);
        assert!(err.to_string().contains("archived"), "{err}");
    }

    #[tokio::test]
    async fn create_rejects_bad_text() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;

        let mut cases = Vec::new();
        let mut request = create_request(channel.id);
        request.name = "   ".to_owned();
        cases.push(("name", request));
        let mut request = create_request(channel.id);
        request.name = "x".repeat(121);
        cases.push(("120", request));
        let mut request = create_request(channel.id);
        request.prompt = " \n".to_owned();
        cases.push(("prompt", request));
        let mut request = create_request(channel.id);
        request.action = start_task("  ");
        cases.push(("title", request));

        for (word, request) in cases {
            let err = create(&hub, request).await.unwrap_err();
            assert_eq!(err.code(), ErrorCode::Invalid, "{word}");
            assert!(err.to_string().contains(word), "{err}");
        }
        // A name of exactly 120 characters is fine.
        let mut request = create_request(channel.id);
        request.name = "x".repeat(120);
        assert!(create(&hub, request).await.is_ok());
    }

    #[tokio::test]
    async fn create_rejects_a_bad_schedule() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        for schedule in [
            interval(30),
            cron("not a cron at all"),
            RoutineSchedule::Cron {
                expression: "0 8 * * *".to_owned(),
                time_zone: "Mars/Olympus".to_owned(),
            },
        ] {
            let mut request = create_request(channel.id);
            request.schedule = schedule;
            let err = create(&hub, request).await.unwrap_err();
            assert_eq!(err.code(), ErrorCode::Invalid);
        }
        // Nothing was saved.
        let all = hub.store.read(routines::routines).await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn create_checks_that_a_once_time_is_in_the_future() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        for enabled in [true, false] {
            let mut request = create_request(channel.id);
            request.schedule = RoutineSchedule::Once {
                at: hub.now().plus_millis(-1_000),
            };
            request.enabled = enabled;
            let err = create(&hub, request).await.unwrap_err();
            assert_eq!(err.code(), ErrorCode::Invalid);
            assert!(err.to_string().contains("in the past"), "{err}");
        }

        let at = hub.now().plus_millis(HOUR_MS);
        let mut request = create_request(channel.id);
        request.schedule = RoutineSchedule::Once { at };
        let created = create(&hub, request).await.unwrap();
        assert_eq!(created.next_run_at, Some(at));
    }

    #[tokio::test]
    async fn update_changes_only_the_fields_that_it_names() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let created = create(&hub, create_request(channel.id)).await.unwrap();

        let mut request = update_request(created.id);
        request.name = Some("Evening brief".to_owned());
        let updated = update(&hub, request).await.unwrap();

        assert_eq!(updated.name, "Evening brief");
        // The rest, including the next run, stays the same.
        assert_eq!(
            updated,
            Routine {
                name: "Evening brief".to_owned(),
                ..created.clone()
            }
        );
        assert_eq!(load(&hub, created.id).await, updated);

        let mut request = update_request(created.id);
        request.prompt = Some("New prompt.".to_owned());
        request.action = Some(start_task("Nightly"));
        let updated = update(&hub, request).await.unwrap();
        assert_eq!(updated.prompt, "New prompt.");
        assert_eq!(updated.action, start_task("Nightly"));
        assert_eq!(updated.name, "Evening brief");
        assert_eq!(updated.next_run_at, created.next_run_at);
    }

    #[tokio::test]
    async fn update_recomputes_the_next_run_when_the_schedule_changes() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let created = create(&hub, create_request(channel.id)).await.unwrap();

        let mut request = update_request(created.id);
        request.schedule = Some(interval(7_200));
        let updated = update(&hub, request).await.unwrap();

        let next = updated.next_run_at.unwrap().as_millis();
        assert!(next > created.next_run_at.unwrap().as_millis() + HOUR_MS / 2);
        assert!(next <= hub.now().as_millis() + 2 * HOUR_MS);

        // The same schedule again is not a change.
        let mut request = update_request(created.id);
        request.schedule = Some(interval(7_200));
        let again = update(&hub, request).await.unwrap();
        assert_eq!(again.next_run_at, updated.next_run_at);
    }

    #[tokio::test]
    async fn update_clears_the_next_run_when_it_turns_a_routine_off_and_sets_it_when_on() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let created = create(&hub, create_request(channel.id)).await.unwrap();

        let mut request = update_request(created.id);
        request.enabled = Some(false);
        let off = update(&hub, request).await.unwrap();
        assert!(!off.enabled);
        assert_eq!(off.next_run_at, None);

        let mut request = update_request(created.id);
        request.enabled = Some(true);
        let on = update(&hub, request).await.unwrap();
        assert!(on.enabled);
        let next = on.next_run_at.unwrap().as_millis();
        assert!(next > hub.now().as_millis());
        assert!(next <= hub.now().as_millis() + HOUR_MS);
    }

    #[tokio::test]
    async fn update_of_a_disabled_routine_keeps_it_without_a_next_run() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let mut request = create_request(channel.id);
        request.enabled = false;
        let created = create(&hub, request).await.unwrap();

        let mut request = update_request(created.id);
        request.schedule = Some(interval(120));
        let updated = update(&hub, request).await.unwrap();

        assert!(!updated.enabled);
        assert_eq!(updated.next_run_at, None);
        assert_eq!(updated.schedule, interval(120));
    }

    #[tokio::test]
    async fn update_checks_the_new_values() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let created = create(&hub, create_request(channel.id)).await.unwrap();

        let mut bad = Vec::new();
        let mut request = update_request(created.id);
        request.name = Some(String::new());
        bad.push(request);
        let mut request = update_request(created.id);
        request.prompt = Some(" ".to_owned());
        bad.push(request);
        let mut request = update_request(created.id);
        request.action = Some(start_task(""));
        bad.push(request);
        let mut request = update_request(created.id);
        request.schedule = Some(interval(1));
        bad.push(request);
        let mut request = update_request(created.id);
        request.schedule = Some(cron("bad"));
        bad.push(request);
        let mut request = update_request(created.id);
        request.schedule = Some(RoutineSchedule::Once {
            at: hub.now().plus_millis(-5_000),
        });
        bad.push(request);

        for request in bad {
            let err = update(&hub, request).await.unwrap_err();
            assert_eq!(err.code(), ErrorCode::Invalid, "{err}");
        }
        // A failed update changes nothing.
        assert_eq!(load(&hub, created.id).await, created);
    }

    #[tokio::test]
    async fn a_fired_once_routine_needs_a_new_time_before_it_turns_on() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let at = hub.now().plus_millis(HOUR_MS);
        let mut request = create_request(channel.id);
        request.schedule = RoutineSchedule::Once { at };
        let created = create(&hub, request).await.unwrap();
        // The routine ran and turned itself off.
        let mut fired = created.clone();
        fired.enabled = false;
        fired.next_run_at = None;
        fired.schedule = RoutineSchedule::Once {
            at: hub.now().plus_millis(-1_000),
        };
        save(&hub, &fired).await;

        let mut request = update_request(created.id);
        request.enabled = Some(true);
        let err = update(&hub, request).await.unwrap_err();
        assert_eq!(err.code(), ErrorCode::Invalid);

        // A new time and the switch together work.
        let new_at = hub.now().plus_millis(2 * HOUR_MS);
        let mut request = update_request(created.id);
        request.enabled = Some(true);
        request.schedule = Some(RoutineSchedule::Once { at: new_at });
        let updated = update(&hub, request).await.unwrap();
        assert!(updated.enabled);
        assert_eq!(updated.next_run_at, Some(new_at));
    }

    #[tokio::test]
    async fn update_and_delete_of_a_missing_routine_are_not_found() {
        let (hub, _dir) = test_hub().await;
        let err = update(&hub, update_request(RoutineId::new()))
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);
        let err = hub
            .routine_delete(
                &Caller::Hub,
                RoutineDelete {
                    routine_id: RoutineId::new(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn delete_removes_the_routine_and_its_runs() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let created = create(&hub, create_request(channel.id)).await.unwrap();
        hub.routine_run_now(
            &Caller::Hub,
            RoutineRunNow {
                routine_id: created.id,
            },
        )
        .await
        .unwrap();
        assert_eq!(runs(&hub, created.id).await.len(), 1);
        let mut feed = hub.store.subscribe();

        hub.routine_delete(
            &Caller::Hub,
            RoutineDelete {
                routine_id: created.id,
            },
        )
        .await
        .unwrap();

        let stored = hub
            .store
            .read(move |conn| routines::routine(conn, created.id))
            .await
            .unwrap();
        assert!(stored.is_none());
        assert!(runs(&hub, created.id).await.is_empty());
        let entry = feed.recv().await.unwrap();
        assert_eq!(entry.entity_id, created.id.to_string());
        assert_eq!(entry.channel_id, Some(channel.id));
    }

    #[tokio::test]
    async fn run_now_runs_the_action_and_keeps_the_schedule() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let created = create(&hub, create_request(channel.id)).await.unwrap();

        let empty = hub
            .routine_run_now(
                &Caller::Hub,
                RoutineRunNow {
                    routine_id: created.id,
                },
            )
            .await
            .unwrap();

        assert_eq!(empty, Empty {});
        assert!(matches!(
            wakes(&hub, channel.id).await.as_slice(),
            [WakeEvent::Routine { id, .. }] if *id == created.id
        ));
        let saved = load(&hub, created.id).await;
        assert_eq!(saved.next_run_at, created.next_run_at);
        assert!(saved.last_run_at.is_some());
    }

    #[tokio::test]
    async fn run_now_reports_a_missing_routine_and_an_archived_channel() {
        let (hub, _dir) = test_hub().await;
        let err = hub
            .routine_run_now(
                &Caller::Hub,
                RoutineRunNow {
                    routine_id: RoutineId::new(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);

        let archived = channel(&hub, true).await;
        let stored = routine(archived.id, interval(3_600), RoutineAction::CoordinatorTurn);
        save(&hub, &stored).await;
        let err = hub
            .routine_run_now(
                &Caller::Hub,
                RoutineRunNow {
                    routine_id: stored.id,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::Conflict);
    }

    #[tokio::test]
    async fn runs_lists_the_newest_first_and_honors_the_limit() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let created = create(&hub, create_request(channel.id)).await.unwrap();
        for _ in 0..3 {
            hub.routine_run_now(
                &Caller::Hub,
                RoutineRunNow {
                    routine_id: created.id,
                },
            )
            .await
            .unwrap();
        }
        let all = hub
            .routine_runs(
                &Caller::Hub,
                RoutineRuns {
                    routine_id: created.id,
                    limit: None,
                },
            )
            .await
            .unwrap()
            .runs;
        assert_eq!(all.len(), 3);
        assert!(
            all.windows(2)
                .all(|pair| pair[0].started_at >= pair[1].started_at)
        );
        assert_eq!(all, runs(&hub, created.id).await);

        let two: Vec<RoutineRun> = hub
            .routine_runs(
                &Caller::Hub,
                RoutineRuns {
                    routine_id: created.id,
                    limit: Some(2),
                },
            )
            .await
            .unwrap()
            .runs;
        assert_eq!(two, all[..2]);

        let err = hub
            .routine_runs(
                &Caller::Hub,
                RoutineRuns {
                    routine_id: RoutineId::new(),
                    limit: None,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn update_in_scope_hides_routines_of_other_channels() {
        let (hub, _dir) = test_hub().await;
        let mine = channel(&hub, false).await;
        let other = channel(&hub, false).await;
        let created = create(&hub, create_request(other.id)).await.unwrap();

        let mut request = update_request(created.id);
        request.name = Some("Hijacked".to_owned());
        let err = hub
            .update_routine_in(Some(mine.id), request)
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);
        let err = hub
            .delete_routine_in(Some(mine.id), created.id)
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);
        assert_eq!(load(&hub, created.id).await, created);

        hub.delete_routine_in(Some(other.id), created.id)
            .await
            .unwrap();
    }
}
