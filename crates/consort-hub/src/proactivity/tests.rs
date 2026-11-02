//! Unit tests of the proactive assistant: the routines that the level
//! makes, the gate before a model runs, the change summary, stale-task
//! nudges, follow-ups, the rules of a proactive turn, and the log.
//!
//! The test hub starts with Pause all on; each test ends the pause first.
//! Each test runs the schedule at a chosen time with `run_routine_at` and
//! `nudge_stale_tasks_at`, so a test never waits for the clock.

use consort_proto::ids::{AccountId, ChannelId, SessionId, TaskId, WorkerId};
use consort_proto::model::{
    ApprovalMode, CHECK_IN_NOTE_PATH, Channel, ChannelKind, ExpectedHash, Isolation, MemoryAuthor,
    MemoryScope, ModelOptions, PlacementRequest, ProactiveKind, ProactiveOutcome, ProactiveRating,
    ProactiveWake, ProactivityLevel, ProactivitySettings, QuietHours, Routine, RoutineKind,
    RoutineOrigin, RoutineSchedule, Session, SessionRole, SessionState, SnoozeChoice, TaskCreator,
    Usage,
};
use consort_proto::session_host_link::TurnEndReason;
use consort_proto::status::{Actor, TaskStatus};
use consort_proto::time::Timestamp;
use consort_proto::turn_input::WakeEvent;
use serde_json::{Value, json};

use super::routines::{pending_wake, sync_channel_routines};
use crate::hub::{Caller, Hub, test_hub};
use crate::ops::hooks::NewTask;
use crate::scheduler::RunTrigger;
use crate::scheduler::testing::wakes;
use crate::store::coordinator::CoordinatorRow;
use crate::store::{conversation, proactivity as store, routines, work};

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;

fn at(rfc3339: &str) -> Timestamp {
    Timestamp::parse_rfc3339(rfc3339).unwrap()
}

/// Ends the pause of the test hub and sets the settings for a test: UTC,
/// quiet hours from 21:00 to 07:00.
async fn settings(hub: &Hub, change: impl FnOnce(&mut ProactivitySettings) + Send + 'static) {
    hub.store
        .write(move |tx| {
            let mut settings = ProactivitySettings {
                time_zone: Some("UTC".to_owned()),
                ..ProactivitySettings::default()
            };
            change(&mut settings);
            tx.save_proactivity_settings(&settings)
        })
        .await
        .unwrap();
}

/// Settings with quiet hours that are never now: two minutes long, twelve
/// hours from now. Tools use the real clock.
async fn awake_now(hub: &Hub) {
    let now = Timestamp::now();
    let minute = |t: Timestamp| {
        let m = t.as_millis().div_euclid(MINUTE).rem_euclid(24 * 60);
        format!("{:02}:{:02}", m / 60, m % 60)
    };
    let start = minute(now.plus_millis(12 * HOUR));
    let end = minute(now.plus_millis(12 * HOUR + 2 * MINUTE));
    settings(hub, move |s| {
        s.quiet_hours = QuietHours { start, end };
    })
    .await;
}

/// A standard channel with a level, with its routines synced.
async fn channel(hub: &Hub, name: &str, level: Option<ProactivityLevel>) -> Channel {
    let now = hub.now();
    let channel = Channel {
        id: ChannelId::new(),
        kind: ChannelKind::Standard,
        name: name.to_owned(),
        icon: None,
        color: None,
        group_id: None,
        position: 1,
        purpose: String::new(),
        instructions: String::new(),
        coordinator_selection: None,
        task_selection: None,
        preferred_workers: Vec::new(),
        repo_ids: Vec::new(),
        default_isolation: Isolation::Worktree,
        default_approval_mode: ApprovalMode::Auto,
        allowed_local_mcp_servers: Vec::new(),
        memory_enabled: true,
        tags: Vec::new(),
        disabled_connections: Vec::new(),
        disabled_skills: Vec::new(),
        command_rules: Vec::new(),
        archived_at: None,
        created_at: now,
        updated_at: now,
        proactivity: level,
        proactive_tasks: Default::default(),
        coordinator_fallback: None,
        coordinator_blocked: None,
        monid: Default::default(),
        access_grants: Vec::new(),
    };
    let saved = channel.clone();
    hub.store
        .write(move |tx| {
            tx.save_channel(&saved)?;
            sync_channel_routines(tx, &saved)
        })
        .await
        .unwrap();
    channel
}

async fn set_level(hub: &Hub, channel_id: ChannelId, level: ProactivityLevel) -> Channel {
    let mut update = consort_proto::client_api::ChannelUpdate::new(channel_id);
    update.proactivity = Some(Some(level));
    hub.channel_update(&Caller::Hub, update)
        .await
        .unwrap()
        .channel
}

async fn tutti(hub: &Hub) -> Channel {
    hub.store.read(conversation::tutti_channel).await.unwrap()
}

async fn routines_of(hub: &Hub, channel_id: ChannelId) -> Vec<Routine> {
    hub.store
        .read(move |conn| routines::routines_in_channel(conn, channel_id))
        .await
        .unwrap()
}

async fn routine_of_kind(hub: &Hub, channel_id: ChannelId, kind: RoutineKind) -> Option<Routine> {
    routines_of(hub, channel_id)
        .await
        .into_iter()
        .find(|r| r.kind == kind)
}

async fn log(hub: &Hub, channel_id: Option<ChannelId>) -> Vec<ProactiveWake> {
    hub.store
        .read(move |conn| store::wakes_page(conn, channel_id, None, None, 100))
        .await
        .unwrap()
        .0
}

/// A coordinator session of a channel with an open turn that holds
/// `events`. Returns the session.
async fn open_turn(hub: &Hub, channel_id: ChannelId, events: Vec<WakeEvent>) -> SessionId {
    let session = Session {
        execution_worker_id: None,
        id: SessionId::new(),
        role: SessionRole::Coordinator,
        channel_id,
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
    let id = session.id;
    hub.store
        .write(move |tx| {
            tx.save_session(&session)?;
            tx.save_coordinator(&CoordinatorRow {
                channel_id,
                session_id: Some(session.id),
                turn_open: true,
                turn_started_at: Some(tx.now().plus_millis(-1_000)),
                turn_events: events,
            })
        })
        .await
        .unwrap();
    id
}

/// A pending log entry and the proactive event that carries it.
async fn proactive_event(hub: &Hub, channel_id: ChannelId, kind: ProactiveKind) -> WakeEvent {
    let wake = hub
        .store
        .write(move |tx| {
            let mut wake = pending_wake(channel_id, kind, tx.now(), None);
            tx.insert_wake(&mut wake)?;
            Ok(wake)
        })
        .await
        .unwrap();
    let routine = consort_proto::ids::RoutineId::new();
    match kind {
        ProactiveKind::Research => WakeEvent::Research {
            routine,
            log: wake.id,
        },
        ProactiveKind::FollowUp => WakeEvent::FollowUp {
            routine,
            reason: "Check the quote.".into(),
            set_at: Timestamp::now(),
            log: wake.id,
        },
        _ => WakeEvent::CheckIn {
            routine,
            summary: String::new(),
            note: String::new(),
            recent_findings: Vec::new(),
            log: wake.id,
        },
    }
}

async fn call(hub: &Hub, session: SessionId, tool: &str, args: Value) -> Result<Value, String> {
    let result = hub.call_tool(session, tool, args).await;
    let value = result.structured_content.unwrap_or(Value::Null);
    if result.is_error {
        Err(value["error"]["message"].as_str().unwrap_or("").to_owned())
    } else {
        Ok(value)
    }
}

/// Ends the open turn of a channel as the worker would, and records it.
async fn end_turn(hub: &Hub, channel_id: ChannelId, session: SessionId, reason: TurnEndReason) {
    hub.store
        .write(move |tx| {
            let channel = conversation::channel_or_404(tx.db(), channel_id)?;
            let row = crate::store::coordinator::coordinator(tx.db(), channel_id)?;
            tx.finish_proactive_turn(&channel, session, &row, reason, None)?;
            tx.save_coordinator(&CoordinatorRow::empty(channel_id))
        })
        .await
        .unwrap();
}

// The routines that the level makes -----------------------------------------

#[tokio::test]
async fn the_check_in_is_made_with_the_level_and_paused_below_normal() {
    let (hub, _dir) = test_hub().await;
    let quiet = channel(&hub, "sayso", None).await;
    assert!(
        routines_of(&hub, quiet.id).await.is_empty(),
        "Quiet has none"
    );

    let normal = set_level(&hub, quiet.id, ProactivityLevel::Normal).await;
    let check_in = routine_of_kind(&hub, normal.id, RoutineKind::CheckIn)
        .await
        .expect("a check-in");
    assert!(check_in.enabled);
    assert_eq!(check_in.origin, RoutineOrigin::Hub);
    assert_eq!(
        check_in.schedule,
        RoutineSchedule::Interval { seconds: 6 * 3600 }
    );
    let due = check_in.next_run_at.unwrap().as_millis() - hub.now().as_millis();
    assert!(
        (6 * HOUR - 1_000..=6 * HOUR + 5 * MINUTE).contains(&due),
        "six hours with up to 5 minutes of jitter: {due}"
    );
    assert!(
        routine_of_kind(&hub, normal.id, RoutineKind::Research)
            .await
            .is_none()
    );

    // Eager adds the research pass. Back to Quiet pauses both.
    set_level(&hub, quiet.id, ProactivityLevel::Eager).await;
    let research = routine_of_kind(&hub, quiet.id, RoutineKind::Research)
        .await
        .unwrap();
    assert!(research.enabled);
    assert!(matches!(
        &research.schedule,
        RoutineSchedule::Cron { expression, .. } if expression == "30 6 * * *"
    ));
    set_level(&hub, quiet.id, ProactivityLevel::Quiet).await;
    for routine in routines_of(&hub, quiet.id).await {
        assert!(!routine.enabled, "{routine:?}");
        assert_eq!(routine.next_run_at, None);
    }
    // Normal again turns the same check-in on; no second one.
    set_level(&hub, quiet.id, ProactivityLevel::Normal).await;
    let all = routines_of(&hub, quiet.id).await;
    assert_eq!(
        all.iter()
            .filter(|r| r.kind == RoutineKind::CheckIn)
            .count(),
        1
    );
    assert_eq!(
        all.iter()
            .find(|r| r.kind == RoutineKind::CheckIn)
            .unwrap()
            .id,
        check_in.id
    );
}

#[tokio::test]
async fn the_defaults_come_once_and_the_brief_never_comes_back_after_a_delete() {
    let (hub, _dir) = test_hub().await;
    let tutti = tutti(&hub).await;
    hub.make_proactive_defaults().await.unwrap();
    let made = routines_of(&hub, tutti.id).await;
    let kinds: Vec<_> = made.iter().map(|r| r.kind).collect();
    assert!(kinds.contains(&RoutineKind::CheckIn) && kinds.contains(&RoutineKind::Brief));
    let check_in = made
        .iter()
        .find(|r| r.kind == RoutineKind::CheckIn)
        .unwrap();
    assert_eq!(
        check_in.schedule,
        RoutineSchedule::Interval { seconds: 3 * 3600 }
    );
    let brief = made.iter().find(|r| r.kind == RoutineKind::Brief).unwrap();
    assert!(matches!(
        &brief.schedule,
        RoutineSchedule::Cron { expression, time_zone } if expression == "30 7 * * 1-5" && time_zone == "UTC"
    ));
    // Tutti's check-in note has its three starting lines, without the watch
    // line while no connection exists.
    let note = hub
        .store
        .read(|conn| {
            crate::store::knowledge::memory_note(conn, MemoryScope::Global, CHECK_IN_NOTE_PATH)
        })
        .await
        .unwrap()
        .unwrap();
    assert!(
        note.content.starts_with("Watch tasks that need me."),
        "{}",
        note.content
    );
    assert!(note.content.contains("Skip newsletters, receipts"));
    assert!(note.content.contains("Ask before anything outward."));

    // A second start changes nothing.
    hub.make_proactive_defaults().await.unwrap();
    assert_eq!(routines_of(&hub, tutti.id).await.len(), made.len());

    // The user deletes the brief. A level change does not bring it back.
    hub.delete_routine_in(None, brief.id).await.unwrap();
    set_level(&hub, tutti.id, ProactivityLevel::Quiet).await;
    set_level(&hub, tutti.id, ProactivityLevel::Eager).await;
    let after = routines_of(&hub, tutti.id).await;
    assert!(
        after.iter().all(|r| r.kind != RoutineKind::Brief),
        "{after:?}"
    );
    assert!(after.iter().any(|r| r.kind == RoutineKind::Research));
}

#[tokio::test]
async fn a_new_time_zone_moves_the_brief_and_the_research_pass() {
    let (hub, _dir) = test_hub().await;
    let tutti = tutti(&hub).await;
    set_level(&hub, tutti.id, ProactivityLevel::Eager).await;
    let request = consort_proto::client_api::ProactivitySet {
        time_zone: Some("America/Chicago".into()),
        time_zone_if_unset: true,
        ..Default::default()
    };
    let result = hub
        .proactivity_set(&Caller::Hub, request.clone())
        .await
        .unwrap();
    assert_eq!(
        result.settings.time_zone.as_deref(),
        Some("America/Chicago")
    );
    for routine in routines_of(&hub, tutti.id).await {
        if let RoutineSchedule::Cron { time_zone, .. } = &routine.schedule {
            assert_eq!(time_zone, "America/Chicago", "{}", routine.name);
        }
    }
    // A client at start sends its zone only when the hub has none.
    let other = consort_proto::client_api::ProactivitySet {
        time_zone: Some("Europe/Berlin".into()),
        ..request
    };
    let kept = hub.proactivity_set(&Caller::Hub, other).await.unwrap();
    assert_eq!(kept.settings.time_zone.as_deref(), Some("America/Chicago"));
    // A zone that does not exist is refused.
    let bad = consort_proto::client_api::ProactivitySet {
        time_zone: Some("Mars/Olympus".into()),
        ..Default::default()
    };
    assert!(hub.proactivity_set(&Caller::Hub, bad).await.is_err());
}

#[tokio::test]
async fn the_hub_fixes_the_prompt_of_a_check_in_and_its_shortest_interval() {
    let (hub, _dir) = test_hub().await;
    let ch = channel(&hub, "house-lights", Some(ProactivityLevel::Normal)).await;
    let check_in = routine_of_kind(&hub, ch.id, RoutineKind::CheckIn)
        .await
        .unwrap();
    let mut update = consort_proto::client_api::RoutineUpdate {
        channel_id: None,
        routine_id: check_in.id,
        name: None,
        schedule: None,
        prompt: Some("Do something else.".into()),
        action: None,
        enabled: None,
        monid: None,
    };
    let err = hub
        .update_routine_in(None, update.clone())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("check-in.md"), "{err}");
    update.prompt = None;
    update.schedule = Some(RoutineSchedule::Interval { seconds: 600 });
    assert!(hub.update_routine_in(None, update.clone()).await.is_err());
    update.schedule = Some(RoutineSchedule::Interval { seconds: 3600 });
    hub.update_routine_in(None, update).await.unwrap();
}

// The gate before a model runs -----------------------------------------------

/// The check-in of a channel at Normal, due at `now`.
async fn due_check_in(hub: &Hub, channel_id: ChannelId, now: Timestamp) -> Routine {
    let mut routine = routine_of_kind(hub, channel_id, RoutineKind::CheckIn)
        .await
        .unwrap();
    routine.next_run_at = Some(now.plus_millis(-1_000));
    let saved = routine.clone();
    hub.store
        .write(move |tx| tx.save_routine(&saved))
        .await
        .unwrap();
    routine
}

async fn run(hub: &Hub, routine: &Routine, now: Timestamp) {
    hub.run_routine_at(routine, RunTrigger::Schedule, Some(now))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_check_in_with_no_change_skips_the_model_and_logs_it() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "house-lights", Some(ProactivityLevel::Normal)).await;
    let noon = hub.now().plus_millis(HOUR);
    let noon = at(&format!("{}T12:00:00Z", &noon.to_rfc3339()[..10]));
    let routine = due_check_in(&hub, ch.id, noon).await;
    run(&hub, &routine, noon).await;

    assert!(
        wakes(&hub, ch.id).await.is_empty(),
        "no wake event, no model"
    );
    let entries = log(&hub, Some(ch.id)).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].outcome, ProactiveOutcome::SkippedNoChange);
    assert_eq!(entries[0].kind, ProactiveKind::CheckIn);
    assert_eq!(
        entries[0].detail.as_deref(),
        Some("No change since the last check-in.")
    );
    let run = &crate::scheduler::testing::runs(&hub, routine.id).await[0];
    assert_eq!(run.status, consort_proto::model::RoutineRunStatus::Skipped);
    // The routine moved on.
    let after = routine_of_kind(&hub, ch.id, RoutineKind::CheckIn)
        .await
        .unwrap();
    assert!(after.next_run_at.unwrap() > noon);
}

#[tokio::test]
async fn a_note_that_watches_outside_runs_the_check_in_with_the_note() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "house-lights", Some(ProactivityLevel::Normal)).await;
    let scope = MemoryScope::Channel(ch.id);
    hub.store
        .write(move |tx| {
            tx.write_memory(
                scope,
                CHECK_IN_NOTE_PATH,
                "watch: outside\nWatch the Brightline mail. Skip receipts.",
                &ExpectedHash::NotExists,
                MemoryAuthor::User,
                None,
            )
            .map(|_| ())
        })
        .await
        .unwrap();
    let noon = at("2030-01-02T12:00:00Z");
    let routine = due_check_in(&hub, ch.id, noon).await;
    run(&hub, &routine, noon).await;
    let events = wakes(&hub, ch.id).await;
    let [
        WakeEvent::CheckIn {
            summary, note, log, ..
        },
    ] = events.as_slice()
    else {
        panic!("{events:?}");
    };
    assert!(summary.is_empty());
    assert!(note.contains("Watch the Brightline mail."));
    let entry = &self::log(&hub, Some(ch.id)).await[0];
    assert_eq!((entry.id, entry.outcome), (*log, ProactiveOutcome::Pending));
}

#[tokio::test]
async fn the_change_summary_names_status_changes_failed_runs_and_waiting_decisions() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "sayso", Some(ProactivityLevel::Normal)).await;
    let other = channel(&hub, "reading", None).await;
    let since = hub.now().plus_millis(-1);
    for (channel_id, title) in [(ch.id, "Fix the build"), (other.id, "Read a book")] {
        let task = hub
            .create_task(NewTask {
                channel_id,
                title: title.to_owned(),
                brief: "Do it.".to_owned(),
                model_selection: None,
                placement: PlacementRequest::default(),
                approval_mode: None,
                start_at: Some(Timestamp::now().plus_millis(86_400_000)),
                created_by: TaskCreator::User,
                routine_id: None,
                attachments: Vec::new(),
                proactive: None,
                user_request: String::new(),
                voice_task: false,
            })
            .await
            .unwrap();
        hub.store
            .write(move |tx| {
                let mut task = work::task_or_404(tx.db(), task.id)?;
                task.status = TaskStatus::Working;
                crate::ops::flow::transition(
                    tx,
                    &mut task,
                    TaskStatus::Failed,
                    Actor::System,
                    Some("The tests failed.".into()),
                    None,
                )?;
                tx.save_task(&task)
            })
            .await
            .unwrap();
    }
    let (summary, tutti_summary) = hub
        .store
        .read(move |conn| {
            let ch = conversation::channel_or_404(conn, ch.id)?;
            let tutti = conversation::tutti_channel(conn)?;
            Ok((
                super::summary::change_summary(conn, &ch, since)?,
                super::summary::change_summary(conn, &tutti, since)?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(summary, "- Task \"Fix the build\" is now Failed.");
    assert!(
        tutti_summary.contains("- Task \"Fix the build\" in sayso is now Failed."),
        "{tutti_summary}"
    );
    assert!(
        tutti_summary.contains("- Task \"Read a book\" in reading is now Failed."),
        "{tutti_summary}"
    );
}

#[tokio::test]
async fn quiet_hours_snooze_pause_and_the_budget_skip_a_check_in_before_the_model() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "house-lights", Some(ProactivityLevel::Normal)).await;
    // A note that watches outside, so "nothing changed" never skips.
    let scope = MemoryScope::Channel(ch.id);
    hub.store
        .write(move |tx| {
            tx.write_memory(
                scope,
                CHECK_IN_NOTE_PATH,
                "watch: outside\nMail.",
                &ExpectedHash::NotExists,
                MemoryAuthor::User,
                None,
            )
            .map(|_| ())
        })
        .await
        .unwrap();

    // 23:00 UTC is inside 21:00 to 07:00.
    let night = at("2030-01-02T23:00:00Z");
    let routine = due_check_in(&hub, ch.id, night).await;
    run(&hub, &routine, night).await;
    let entry = log(&hub, Some(ch.id)).await.remove(0);
    assert_eq!(entry.outcome, ProactiveOutcome::SkippedQuietHours);
    assert_eq!(
        entry.detail.as_deref(),
        Some("Quiet hours until Thu 7:00 AM.")
    );

    // Snoozed at noon.
    let noon = at("2030-01-02T12:00:00Z");
    settings(&hub, move |s| {
        s.snoozed_until = Some(noon.plus_millis(2 * HOUR))
    })
    .await;
    let routine = due_check_in(&hub, ch.id, noon).await;
    run(&hub, &routine, noon).await;
    assert_eq!(
        log(&hub, Some(ch.id)).await[0].outcome,
        ProactiveOutcome::SkippedSnoozed
    );

    // Pause all.
    settings(&hub, |s| s.paused = true).await;
    let routine = due_check_in(&hub, ch.id, noon).await;
    run(&hub, &routine, noon).await;
    assert_eq!(
        log(&hub, Some(ch.id)).await[0].outcome,
        ProactiveOutcome::SkippedPaused
    );
    assert!(
        wakes(&hub, ch.id).await.is_empty(),
        "nothing reached the model"
    );

    // Three turns a day in a channel. The fourth check-in is skipped.
    settings(&hub, |_| {}).await;
    for i in 0..3 {
        let t = noon.plus_millis(i * MINUTE);
        let routine = due_check_in(&hub, ch.id, t).await;
        run(&hub, &routine, t).await;
        crate::scheduler::testing::end_running(&hub, routine.id).await;
        hub.store
            .write(move |tx| {
                tx.delete_wakes(
                    &crate::store::coordinator::pending_wakes(tx.db(), ch.id)?
                        .iter()
                        .map(|(id, _)| *id)
                        .collect::<Vec<_>>(),
                )
            })
            .await
            .unwrap();
    }
    let t = noon.plus_millis(10 * MINUTE);
    let routine = due_check_in(&hub, ch.id, t).await;
    run(&hub, &routine, t).await;
    let entry = log(&hub, Some(ch.id)).await.remove(0);
    assert_eq!(entry.outcome, ProactiveOutcome::SkippedBudget);
    assert_eq!(
        entry.detail.as_deref(),
        Some("3 of 3 proactive turns used in house-lights.")
    );
    assert!(wakes(&hub, ch.id).await.is_empty());
    // The next day the budget is back.
    let tomorrow = noon.plus_millis(24 * HOUR);
    let routine = due_check_in(&hub, ch.id, tomorrow).await;
    run(&hub, &routine, tomorrow).await;
    assert_eq!(
        log(&hub, Some(ch.id)).await[0].outcome,
        ProactiveOutcome::Pending
    );
}

#[tokio::test]
async fn a_check_in_whose_last_one_still_waits_is_skipped_as_busy() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "house-lights", Some(ProactivityLevel::Normal)).await;
    let scope = MemoryScope::Channel(ch.id);
    hub.store
        .write(move |tx| {
            tx.write_memory(
                scope,
                CHECK_IN_NOTE_PATH,
                "watch: outside",
                &ExpectedHash::NotExists,
                MemoryAuthor::User,
                None,
            )
            .map(|_| ())
        })
        .await
        .unwrap();
    let noon = at("2030-01-02T12:00:00Z");
    let routine = due_check_in(&hub, ch.id, noon).await;
    run(&hub, &routine, noon).await;
    // A turn runs in the channel, so the check-in waits in the queue. The
    // next one comes due an interval later.
    let later = noon.plus_millis(6 * HOUR);
    let routine = due_check_in(&hub, ch.id, later).await;
    run(&hub, &routine, later).await;
    let entries = log(&hub, Some(ch.id)).await;
    assert_eq!(entries[0].outcome, ProactiveOutcome::SkippedBusy);
    assert_eq!(entries[1].outcome, ProactiveOutcome::Pending);
    assert_eq!(wakes(&hub, ch.id).await.len(), 1, "no second wake");
}

#[tokio::test]
async fn six_silent_check_ins_double_the_interval_and_a_user_message_resets_it() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "house-lights", Some(ProactivityLevel::Normal)).await;
    for _ in 0..6 {
        let event = proactive_event(&hub, ch.id, ProactiveKind::CheckIn).await;
        let session = open_turn(&hub, ch.id, vec![event]).await;
        call(
            &hub,
            session,
            "no_reply_needed",
            json!({"finding": "The quote did not change."}),
        )
        .await
        .unwrap();
        end_turn(&hub, ch.id, session, TurnEndReason::Completed).await;
    }
    let check_in = routine_of_kind(&hub, ch.id, RoutineKind::CheckIn)
        .await
        .unwrap();
    assert_eq!(
        check_in.schedule,
        RoutineSchedule::Interval { seconds: 12 * 3600 }
    );
    let state = hub
        .store
        .read(move |conn| store::channel_state(conn, ch.id))
        .await
        .unwrap();
    assert_eq!(
        (state.silent_streak, state.base_seconds),
        (0, Some(6 * 3600))
    );
    let entries = log(&hub, Some(ch.id)).await;
    assert!(
        entries
            .iter()
            .all(|e| e.outcome == ProactiveOutcome::Silent)
    );
    assert_eq!(
        entries[0].detail.as_deref(),
        Some("The quote did not change.")
    );

    // Back-off stops at 12 hours.
    for _ in 0..6 {
        let event = proactive_event(&hub, ch.id, ProactiveKind::CheckIn).await;
        let session = open_turn(&hub, ch.id, vec![event]).await;
        end_turn(&hub, ch.id, session, TurnEndReason::Completed).await;
    }
    let check_in = routine_of_kind(&hub, ch.id, RoutineKind::CheckIn)
        .await
        .unwrap();
    assert_eq!(
        check_in.schedule,
        RoutineSchedule::Interval { seconds: 12 * 3600 }
    );

    // A user message brings back the base interval.
    hub.message_send(
        &Caller::Hub,
        consort_proto::client_api::MessageSend {
            channel_id: ch.id,
            thread_root_id: None,
            body: "I am back.".into(),
            client_message_id: "m1".into(),
            attachments: Vec::new(),
            in_timeline: false,
        },
    )
    .await
    .unwrap();
    let check_in = routine_of_kind(&hub, ch.id, RoutineKind::CheckIn)
        .await
        .unwrap();
    assert_eq!(
        check_in.schedule,
        RoutineSchedule::Interval { seconds: 6 * 3600 }
    );
    let state = hub
        .store
        .read(move |conn| store::channel_state(conn, ch.id))
        .await
        .unwrap();
    assert_eq!((state.silent_streak, state.base_seconds), (0, None));
}

#[tokio::test]
async fn the_brief_and_follow_ups_run_while_snoozed_and_in_quiet_hours_but_not_while_paused() {
    let (hub, _dir) = test_hub().await;
    let night = at("2030-01-02T23:00:00Z");
    settings(&hub, move |s| {
        s.snoozed_until = Some(night.plus_millis(HOUR))
    })
    .await;
    let tutti = tutti(&hub).await;
    hub.make_proactive_defaults().await.unwrap();
    let mut brief = routine_of_kind(&hub, tutti.id, RoutineKind::Brief)
        .await
        .unwrap();
    brief.next_run_at = Some(night.plus_millis(-1_000));
    let saved = brief.clone();
    hub.store
        .write(move |tx| tx.save_routine(&saved))
        .await
        .unwrap();
    run(&hub, &brief, night).await;
    assert!(matches!(
        wakes(&hub, tutti.id).await.as_slice(),
        [WakeEvent::Brief { .. }]
    ));

    // Pause all stops the brief too, with a row in the log.
    settings(&hub, |s| s.paused = true).await;
    let mut brief = routine_of_kind(&hub, tutti.id, RoutineKind::Brief)
        .await
        .unwrap();
    crate::scheduler::testing::end_running(&hub, brief.id).await;
    brief.next_run_at = Some(night.plus_millis(-1_000));
    let saved = brief.clone();
    hub.store
        .write(move |tx| tx.save_routine(&saved))
        .await
        .unwrap();
    run(&hub, &brief, night).await;
    let entries = log(&hub, Some(tutti.id)).await;
    assert_eq!(entries[0].outcome, ProactiveOutcome::SkippedPaused);
    assert_eq!(entries[0].kind, ProactiveKind::Brief);
}

// Follow-ups -----------------------------------------------------------------

#[tokio::test]
async fn follow_ups_have_limits_replace_by_key_and_delete_themselves_after_they_run() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "house-lights", None).await;
    let session = open_turn(&hub, ch.id, Vec::new()).await;

    let err = call(
        &hub,
        session,
        "follow_up_later",
        json!({"after_minutes": 10, "reason": "Too soon"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("at least 15 minutes"), "{err}");
    let err = call(
        &hub,
        session,
        "follow_up_later",
        json!({"after_minutes": 31 * 24 * 60, "reason": "Too late"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("at most 30 days"), "{err}");
    let err = call(
        &hub,
        session,
        "follow_up_later",
        json!({"after_minutes": 60, "reason": " "}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("reason is empty"), "{err}");

    let first = call(&hub, session, "follow_up_later", json!({"after_minutes": 60, "reason": "Check whether Brightline replied.", "key": "brightline"})).await.unwrap();
    let second = call(
        &hub,
        session,
        "follow_up_later",
        json!({"after_minutes": 120, "reason": "Check Brightline again.", "key": "brightline"}),
    )
    .await
    .unwrap();
    assert_eq!(second["replaced"], first["routine_id"]);
    let follow_ups: Vec<_> = routines_of(&hub, ch.id)
        .await
        .into_iter()
        .filter(|r| r.kind == RoutineKind::FollowUp)
        .collect();
    assert_eq!(follow_ups.len(), 1);
    assert_eq!(follow_ups[0].prompt, "Check Brightline again.");
    assert_eq!(follow_ups[0].origin, RoutineOrigin::Assistant);
    assert!(follow_ups[0].created_at.is_some());

    // At most 20 wait in a channel.
    for i in 0..19 {
        call(
            &hub,
            session,
            "follow_up_later",
            json!({"after_minutes": 60 + i, "reason": format!("Check {i}.")}),
        )
        .await
        .unwrap();
    }
    let err = call(
        &hub,
        session,
        "follow_up_later",
        json!({"after_minutes": 90, "reason": "One more."}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("20 follow-ups wait"), "{err}");

    // Cancel by key.
    call(
        &hub,
        session,
        "cancel_follow_up",
        json!({"key": "brightline"}),
    )
    .await
    .unwrap();
    assert!(
        call(
            &hub,
            session,
            "cancel_follow_up",
            json!({"key": "brightline"})
        )
        .await
        .is_err()
    );

    // A follow-up comes due: it runs, with its reason, and is gone after
    // its turn. Its message carries the reason under the mark.
    let mut due = routines_of(&hub, ch.id)
        .await
        .into_iter()
        .find(|r| r.kind == RoutineKind::FollowUp)
        .unwrap();
    let night = at("2030-01-02T23:30:00Z");
    due.next_run_at = Some(night.plus_millis(-1_000));
    let saved = due.clone();
    hub.store
        .write(move |tx| tx.save_routine(&saved))
        .await
        .unwrap();
    run(&hub, &due, night).await;
    let events = wakes(&hub, ch.id).await;
    let [
        WakeEvent::FollowUp {
            reason, routine, ..
        },
    ] = events.as_slice()
    else {
        panic!("{events:?}");
    };
    assert_eq!(*routine, due.id);
    assert_eq!(reason, &due.prompt);
    let turn = open_turn(&hub, ch.id, events.clone()).await;
    let posted = call(
        &hub,
        turn,
        "post_message",
        json!({"body": "Brightline replied.", "notify": true}),
    )
    .await
    .unwrap();
    let id: consort_proto::ids::MessageId = posted["message_id"].as_str().unwrap().parse().unwrap();
    let message = hub
        .store
        .read(move |conn| conversation::message_or_404(conn, id))
        .await
        .unwrap();
    let mark = message.proactive.unwrap();
    assert_eq!(mark.kind, ProactiveKind::FollowUp);
    assert_eq!(mark.reason.as_deref(), Some(due.prompt.as_str()));
    end_turn(&hub, ch.id, turn, TurnEndReason::Completed).await;
    assert!(
        routines_of(&hub, ch.id)
            .await
            .iter()
            .all(|r| r.id != due.id),
        "the follow-up is gone"
    );

    // Cancel all follow-ups from Settings.
    let canceled = hub
        .proactivity_cancel_follow_ups(
            &Caller::Hub,
            consort_proto::client_api::ProactivityCancelFollowUps { channel_id: None },
        )
        .await
        .unwrap()
        .canceled;
    assert_eq!(canceled, 18);
    assert!(routines_of(&hub, ch.id).await.is_empty());

    // A channel at Off cannot set one.
    set_level(&hub, ch.id, ProactivityLevel::Off).await;
    let err = call(
        &hub,
        session,
        "follow_up_later",
        json!({"after_minutes": 60, "reason": "Later."}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("is Off"), "{err}");
}

#[tokio::test]
async fn a_follow_up_that_comes_due_while_paused_is_skipped_logged_and_gone() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "house-lights", None).await;
    let session = open_turn(&hub, ch.id, Vec::new()).await;
    call(
        &hub,
        session,
        "follow_up_later",
        json!({"after_minutes": 60, "reason": "Check."}),
    )
    .await
    .unwrap();
    settings(&hub, |s| s.paused = true).await;
    let mut due = routines_of(&hub, ch.id).await.remove(0);
    let noon = at("2030-01-02T12:00:00Z");
    due.next_run_at = Some(noon.plus_millis(-1_000));
    let saved = due.clone();
    hub.store
        .write(move |tx| tx.save_routine(&saved))
        .await
        .unwrap();
    run(&hub, &due, noon).await;
    assert!(wakes(&hub, ch.id).await.is_empty());
    assert!(routines_of(&hub, ch.id).await.is_empty());
    let entry = log(&hub, Some(ch.id)).await.remove(0);
    assert_eq!(
        (entry.kind, entry.outcome),
        (ProactiveKind::FollowUp, ProactiveOutcome::SkippedPaused)
    );
}

// Stale tasks ----------------------------------------------------------------

async fn task_in(hub: &Hub, channel_id: ChannelId, status: TaskStatus) -> TaskId {
    let task = hub
        .create_task(NewTask {
            channel_id,
            title: "Go through open issues".to_owned(),
            brief: "Do it.".to_owned(),
            model_selection: None,
            placement: PlacementRequest::default(),
            approval_mode: None,
            start_at: Some(Timestamp::now().plus_millis(86_400_000)),
            created_by: TaskCreator::User,
            routine_id: None,
            attachments: Vec::new(),
            proactive: None,
            user_request: String::new(),
            voice_task: false,
        })
        .await
        .unwrap();
    hub.store
        .write(move |tx| {
            let mut task = work::task_or_404(tx.db(), task.id)?;
            task.status = TaskStatus::Working;
            crate::ops::flow::transition(tx, &mut task, status, Actor::System, None, None)?;
            tx.save_task(&task)?;
            tx.delete_wakes(
                &crate::store::coordinator::pending_wakes(tx.db(), channel_id)?
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .unwrap();
    task.id
}

#[tokio::test]
async fn a_stale_task_gets_one_nudge_per_status_entry() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "sayso", None).await;
    let task_id = task_in(&hub, ch.id, TaskStatus::NeedsInput).await;
    let now = hub.now();
    // 23 hours: not yet. 24 hours, at noon: one nudge.
    let noon = at(&format!(
        "{}T12:00:00Z",
        &now.plus_millis(2 * 24 * HOUR).to_rfc3339()[..10]
    ));
    hub.nudge_stale_tasks_at(now.plus_millis(23 * HOUR))
        .await
        .unwrap();
    assert!(wakes(&hub, ch.id).await.is_empty());
    hub.nudge_stale_tasks_at(noon).await.unwrap();
    let events = wakes(&hub, ch.id).await;
    let [WakeEvent::StaleTask { task, status, .. }] = events.as_slice() else {
        panic!("{events:?}");
    };
    assert_eq!((*task, *status), (task_id, TaskStatus::NeedsInput));
    // Never twice for the same status entry.
    hub.nudge_stale_tasks_at(noon.plus_millis(HOUR))
        .await
        .unwrap();
    assert_eq!(wakes(&hub, ch.id).await.len(), 1);
    assert_eq!(log(&hub, Some(ch.id)).await[0].task_id, Some(task_id));

    // A new status entry (Ready for review) waits 72 hours, then gets its own.
    hub.store
        .write(move |tx| {
            let mut task = work::task_or_404(tx.db(), task_id)?;
            task.status = TaskStatus::Working;
            crate::ops::flow::transition(
                tx,
                &mut task,
                TaskStatus::Review,
                Actor::System,
                None,
                None,
            )?;
            tx.save_task(&task)?;
            tx.delete_wakes(
                &crate::store::coordinator::pending_wakes(tx.db(), ch.id)?
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .unwrap();
    let entered = hub.now();
    let at_noon = |hours: i64| {
        at(&format!(
            "{}T12:00:00Z",
            &entered.plus_millis(hours * HOUR).to_rfc3339()[..10]
        ))
    };
    hub.nudge_stale_tasks_at(entered.plus_millis(71 * HOUR))
        .await
        .unwrap();
    assert!(wakes(&hub, ch.id).await.is_empty());
    hub.nudge_stale_tasks_at(at_noon(4 * 24)).await.unwrap();
    assert!(matches!(
        wakes(&hub, ch.id).await.as_slice(),
        [WakeEvent::StaleTask {
            status: TaskStatus::Review,
            ..
        }]
    ));
}

#[tokio::test]
async fn a_stale_nudge_waits_for_quiet_hours_and_snooze_and_the_level() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "sayso", None).await;
    task_in(&hub, ch.id, TaskStatus::NeedsInput).await;
    let day = &hub.now().plus_millis(3 * 24 * HOUR).to_rfc3339()[..10];
    // At night it waits, with no row.
    hub.nudge_stale_tasks_at(at(&format!("{day}T23:00:00Z")))
        .await
        .unwrap();
    assert!(wakes(&hub, ch.id).await.is_empty());
    assert!(log(&hub, Some(ch.id)).await.is_empty());
    // Snoozed: one row, and it waits.
    let noon = at(&format!("{day}T12:00:00Z"));
    settings(&hub, move |s| {
        s.snoozed_until = Some(noon.plus_millis(HOUR))
    })
    .await;
    hub.nudge_stale_tasks_at(noon).await.unwrap();
    hub.nudge_stale_tasks_at(noon.plus_millis(MINUTE))
        .await
        .unwrap();
    let entries = log(&hub, Some(ch.id)).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].outcome, ProactiveOutcome::SkippedSnoozed);
    assert!(wakes(&hub, ch.id).await.is_empty());
    // Level Off: nothing, and no row.
    set_level(&hub, ch.id, ProactivityLevel::Off).await;
    hub.nudge_stale_tasks_at(noon.plus_millis(2 * HOUR))
        .await
        .unwrap();
    assert!(wakes(&hub, ch.id).await.is_empty());
    assert_eq!(log(&hub, Some(ch.id)).await.len(), 1);
    // After the snooze, at Quiet, the nudge comes.
    set_level(&hub, ch.id, ProactivityLevel::Quiet).await;
    hub.nudge_stale_tasks_at(noon.plus_millis(2 * HOUR))
        .await
        .unwrap();
    assert_eq!(wakes(&hub, ch.id).await.len(), 1);
}

// The rules of a proactive turn ----------------------------------------------

#[tokio::test]
async fn the_research_pass_is_read_only_in_code() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let tutti = tutti(&hub).await;
    set_level(&hub, tutti.id, ProactivityLevel::Eager).await;
    let event = proactive_event(&hub, tutti.id, ProactiveKind::Research).await;
    let session = open_turn(&hub, tutti.id, vec![event]).await;
    let refusal = "The research pass is read-only. Make an offer with ask_decision instead.";
    for (tool, args) in [
        ("post_message", json!({"body": "Hi"})),
        ("start_task", json!({"title": "T", "brief": "B"})),
        ("files_write", json!({"path": "a.md", "content": "x"})),
        (
            "memory_write",
            json!({"path": "people/a.md", "content": "x", "expected_sha256": "not_exists"}),
        ),
        (
            "ask_decision",
            json!({"question": "Q?", "options": [{"label": "A", "consequence": "a"}, {"label": "B", "consequence": "b"}]}),
        ),
        (
            "message_channel",
            json!({"channel_id": tutti.id, "body": "x"}),
        ),
        (
            "routine_create",
            json!({"name": "x", "schedule": {"kind": "interval", "seconds": 3600}, "prompt": "p", "action": {"kind": "coordinator_turn"}}),
        ),
    ] {
        assert_eq!(
            call(&hub, session, tool, args).await.unwrap_err(),
            refusal,
            "{tool}"
        );
    }
    // Reads, memory under research/, offers and no_reply_needed work.
    call(&hub, session, "list_channels", json!({}))
        .await
        .unwrap();
    call(&hub, session, "memory_write", json!({"path": "research/vendors.md", "content": "Brightline is slow.", "expected_sha256": "not_exists"})).await.unwrap();
    for i in 0..3 {
        let offer = call(&hub, session, "ask_decision", json!({"question": format!("Want me to draft reply {i}?"), "context": "I will only write a draft. I will not send it.", "offer": true})).await.unwrap();
        assert!(offer["decision_id"].is_string());
    }
    let err = call(
        &hub,
        session,
        "ask_decision",
        json!({"question": "A fourth?", "offer": true}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("3 offers today"), "{err}");
    call(
        &hub,
        session,
        "no_reply_needed",
        json!({"finding": "Dana asked about the venue."}),
    )
    .await
    .unwrap();
    // The offer cards are marked, with the three fixed options.
    let decisions = hub
        .store
        .read(move |conn| work::pending_decisions(conn, Some(tutti.id)))
        .await
        .unwrap();
    assert_eq!(decisions.len(), 3);
    let labels: Vec<_> = decisions[0]
        .options
        .iter()
        .map(|o| o.label.as_str())
        .collect();
    assert_eq!(labels, ["Do it", "Not now", "Never about this"]);
    let card_id = decisions[0].message_id;
    let card = hub
        .store
        .read(move |conn| conversation::message_or_404(conn, card_id))
        .await
        .unwrap();
    let mark = card.proactive.unwrap();
    assert!(mark.offer);
    assert_eq!(mark.kind, ProactiveKind::Research);
}

#[tokio::test]
async fn offers_need_eager_and_a_proactive_turn() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "sayso", Some(ProactivityLevel::Normal)).await;
    let event = proactive_event(&hub, ch.id, ProactiveKind::CheckIn).await;
    let session = open_turn(&hub, ch.id, vec![event]).await;
    let err = call(
        &hub,
        session,
        "ask_decision",
        json!({"question": "Draft it?", "offer": true}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("need the level Eager"), "{err}");
    let normal_turn = open_turn(&hub, ch.id, Vec::new()).await;
    set_level(&hub, ch.id, ProactivityLevel::Eager).await;
    let err = call(
        &hub,
        normal_turn,
        "ask_decision",
        json!({"question": "Draft it?", "offer": true}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("turn that you started on your own"), "{err}");
}

#[tokio::test]
async fn a_task_that_a_proactive_turn_starts_runs_in_draft_mode() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "sayso", Some(ProactivityLevel::Normal)).await;
    assert_eq!(ch.default_approval_mode, ApprovalMode::Auto);
    let event = proactive_event(&hub, ch.id, ProactiveKind::CheckIn).await;
    let log_id = event.log_id().unwrap();
    let session = open_turn(&hub, ch.id, vec![event]).await;
    let started = call(&hub, session, "start_task", json!({"title": "Draft a reply", "brief": "Write a draft.", "approval_mode": "full_access"})).await.unwrap();
    let task_id: TaskId = started["task_id"].as_str().unwrap().parse().unwrap();
    let task = hub
        .store
        .read(move |conn| work::task_or_404(conn, task_id))
        .await
        .unwrap();
    assert_eq!(task.approval_mode, ApprovalMode::Ask, "draft mode");
    assert!(started["notes"].to_string().contains("draft mode"));
    let wake = hub
        .store
        .read(move |conn| store::wake(conn, log_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wake.task_id, Some(task_id));

    // A channel that lets these tasks use its default.
    let mut update = consort_proto::client_api::ChannelUpdate::new(ch.id);
    update.proactive_tasks = Some(consort_proto::model::ProactiveTasks::ChannelDefault);
    hub.channel_update(&Caller::Hub, update).await.unwrap();
    let started = call(
        &hub,
        session,
        "start_task",
        json!({"title": "Another", "brief": "B"}),
    )
    .await
    .unwrap();
    let task_id: TaskId = started["task_id"].as_str().unwrap().parse().unwrap();
    let task = hub
        .store
        .read(move |conn| work::task_or_404(conn, task_id))
        .await
        .unwrap();
    assert_eq!(task.approval_mode, ApprovalMode::Auto);

    // A normal turn keeps what it asked for, within the channel.
    let normal = open_turn(&hub, ch.id, Vec::new()).await;
    let started = call(
        &hub,
        normal,
        "start_task",
        json!({"title": "Normal", "brief": "B"}),
    )
    .await
    .unwrap();
    let task_id: TaskId = started["task_id"].as_str().unwrap().parse().unwrap();
    let task = hub
        .store
        .read(move |conn| work::task_or_404(conn, task_id))
        .await
        .unwrap();
    assert_eq!(task.approval_mode, ApprovalMode::Auto);
}

#[tokio::test]
async fn a_proactive_turn_cannot_widen_access_or_change_settings() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "sayso", Some(ProactivityLevel::Normal)).await;
    let event = proactive_event(&hub, ch.id, ProactiveKind::CheckIn).await;
    let session = open_turn(&hub, ch.id, vec![event]).await;
    for (tool, args) in [
        ("set_channel_tags", json!({"tags": ["work"]})),
        ("update_this_channel", json!({"purpose": "x"})),
        (
            "connect_service",
            json!({"service": "gmail", "name": "Gmail"}),
        ),
        (
            "routine_delete",
            json!({"routine_id": consort_proto::ids::RoutineId::new()}),
        ),
    ] {
        let err = call(&hub, session, tool, args).await.unwrap_err();
        assert!(err.contains("cannot widen access"), "{tool}: {err}");
    }
}

#[tokio::test]
async fn notifications_need_a_proactive_turn_and_stop_at_the_daily_limit() {
    let (hub, _dir) = test_hub().await;
    awake_now(&hub).await;
    let ch = channel(&hub, "sayso", Some(ProactivityLevel::Normal)).await;
    let mut notified = 0;
    for i in 0..4 {
        let event = proactive_event(&hub, ch.id, ProactiveKind::CheckIn).await;
        let log_id = event.log_id().unwrap();
        let session = open_turn(&hub, ch.id, vec![event]).await;
        let posted = call(
            &hub,
            session,
            "post_message",
            json!({"body": format!("Needs you {i}."), "notify": true}),
        )
        .await
        .unwrap();
        // A second notify in the same turn is held back.
        let again = call(
            &hub,
            session,
            "post_message",
            json!({"body": "And this.", "notify": true}),
        )
        .await
        .unwrap();
        let (first, second) = {
            let (a, b): (consort_proto::ids::MessageId, consort_proto::ids::MessageId) = (
                posted["message_id"].as_str().unwrap().parse().unwrap(),
                again["message_id"].as_str().unwrap().parse().unwrap(),
            );
            hub.store
                .read(move |conn| {
                    Ok((
                        conversation::message_or_404(conn, a)?,
                        conversation::message_or_404(conn, b)?,
                    ))
                })
                .await
                .unwrap()
        };
        assert!(!second.proactive.unwrap().notify);
        end_turn(&hub, ch.id, session, TurnEndReason::Completed).await;
        let wake = hub
            .store
            .read(move |conn| store::wake(conn, log_id))
            .await
            .unwrap()
            .unwrap();
        if first.proactive.unwrap().notify {
            notified += 1;
            assert_eq!(wake.outcome, ProactiveOutcome::Notified);
        } else {
            assert_eq!(i, 3, "the fourth one is over the limit");
            assert_eq!(wake.outcome, ProactiveOutcome::Posted);
            assert!(wake.notify_held);
        }
        assert_eq!(wake.message_ids.len(), 2);
    }
    assert_eq!(notified, 3);
    // In a normal turn, notify does nothing and the message has no mark.
    let normal = open_turn(&hub, ch.id, Vec::new()).await;
    let posted = call(
        &hub,
        normal,
        "post_message",
        json!({"body": "Hi", "notify": true}),
    )
    .await
    .unwrap();
    let id: consort_proto::ids::MessageId = posted["message_id"].as_str().unwrap().parse().unwrap();
    let message = hub
        .store
        .read(move |conn| conversation::message_or_404(conn, id))
        .await
        .unwrap();
    assert_eq!(message.proactive, None);
}

#[tokio::test]
async fn nothing_notifies_while_snoozed() {
    let (hub, _dir) = test_hub().await;
    let ch = channel(&hub, "sayso", Some(ProactivityLevel::Normal)).await;
    awake_now(&hub).await;
    hub.proactivity_snooze(
        &Caller::Hub,
        consort_proto::client_api::ProactivitySnooze {
            choice: SnoozeChoice::TwoHours,
        },
    )
    .await
    .unwrap();
    let event = proactive_event(&hub, ch.id, ProactiveKind::FollowUp).await;
    let session = open_turn(&hub, ch.id, vec![event]).await;
    let posted = call(
        &hub,
        session,
        "post_message",
        json!({"body": "Due soon.", "notify": true}),
    )
    .await
    .unwrap();
    let id: consort_proto::ids::MessageId = posted["message_id"].as_str().unwrap().parse().unwrap();
    let message = hub
        .store
        .read(move |conn| conversation::message_or_404(conn, id))
        .await
        .unwrap();
    assert!(!message.proactive.unwrap().notify);
}

#[tokio::test]
async fn suggestions_are_checked_and_stored_on_the_message() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "reading", None).await;
    let session = open_turn(&hub, ch.id, Vec::new()).await;
    let posted = call(
        &hub,
        session,
        "post_message",
        json!({"body": "What is reading for?", "suggestions": [
            {"label": "Books I want to read"}, {"label": "I will type it", "action": "type"}
        ]}),
    )
    .await
    .unwrap();
    let id: consort_proto::ids::MessageId = posted["message_id"].as_str().unwrap().parse().unwrap();
    let message = hub
        .store
        .read(move |conn| conversation::message_or_404(conn, id))
        .await
        .unwrap();
    assert_eq!(message.suggestions.len(), 2);
    assert_eq!(
        message.suggestions[1].action,
        consort_proto::model::SuggestionAction::Type
    );
    let five = json!({"body": "x", "suggestions": [{"label":"a"},{"label":"b"},{"label":"c"},{"label":"d"},{"label":"e"}]});
    assert!(call(&hub, session, "post_message", five).await.is_err());
    let reply = call(
        &hub,
        session,
        "reply",
        json!({"message_id": id, "body": "y", "suggestions": [{"label": "x".repeat(41)}]}),
    )
    .await;
    assert!(reply.is_err());
}

#[tokio::test]
async fn set_proactivity_makes_a_channel_quieter_only_and_snoozes() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "house-lights", Some(ProactivityLevel::Normal)).await;
    let session = open_turn(&hub, ch.id, Vec::new()).await;
    let err = call(&hub, session, "set_proactivity", json!({"level": "eager"}))
        .await
        .unwrap_err();
    assert!(err.contains("Only the user can"), "{err}");
    let result = call(
        &hub,
        session,
        "set_proactivity",
        json!({"level": "quiet", "snooze": "until_tomorrow"}),
    )
    .await
    .unwrap();
    assert_eq!(result["level"], "quiet");
    assert!(result["snoozed_until"].is_number());
    let check_in = routine_of_kind(&hub, ch.id, RoutineKind::CheckIn)
        .await
        .unwrap();
    assert!(!check_in.enabled, "the check-in pauses below Normal");
    // A channel coordinator cannot change another channel.
    let other = channel(&hub, "sayso", None).await;
    assert!(
        call(
            &hub,
            session,
            "set_proactivity",
            json!({"level": "off", "channel_id": other.id})
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn too_noisy_wakes_the_coordinator_once_and_useful_is_only_recorded() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "sayso", Some(ProactivityLevel::Normal)).await;
    let event = proactive_event(&hub, ch.id, ProactiveKind::CheckIn).await;
    let session = open_turn(&hub, ch.id, vec![event]).await;
    let posted = call(
        &hub,
        session,
        "post_message",
        json!({"body": "Your dentist moved."}),
    )
    .await
    .unwrap();
    let id: consort_proto::ids::MessageId = posted["message_id"].as_str().unwrap().parse().unwrap();
    end_turn(&hub, ch.id, session, TurnEndReason::Completed).await;

    let rate = |rating| consort_proto::client_api::MessageRate {
        message_id: id,
        rating,
    };
    hub.message_rate(&Caller::Hub, rate(Some(ProactiveRating::Useful)))
        .await
        .unwrap();
    assert!(wakes(&hub, ch.id).await.is_empty());
    hub.message_rate(&Caller::Hub, rate(Some(ProactiveRating::TooNoisy)))
        .await
        .unwrap();
    hub.message_rate(&Caller::Hub, rate(Some(ProactiveRating::TooNoisy)))
        .await
        .unwrap();
    let events = wakes(&hub, ch.id).await;
    assert!(
        matches!(events.as_slice(), [WakeEvent::ProactiveFeedback { message, rating: ProactiveRating::TooNoisy, .. }] if *message == id)
    );
    let message = hub
        .store
        .read(move |conn| conversation::message_or_404(conn, id))
        .await
        .unwrap();
    assert_eq!(
        message.proactive.unwrap().rating,
        Some(ProactiveRating::TooNoisy)
    );
    assert_eq!(
        log(&hub, Some(ch.id)).await[0].kind,
        ProactiveKind::Feedback
    );

    // A message from a normal turn cannot be rated.
    let normal = open_turn(&hub, ch.id, Vec::new()).await;
    let posted = call(&hub, normal, "post_message", json!({"body": "Hi"}))
        .await
        .unwrap();
    let plain: consort_proto::ids::MessageId =
        posted["message_id"].as_str().unwrap().parse().unwrap();
    let rate = consort_proto::client_api::MessageRate {
        message_id: plain,
        rating: Some(ProactiveRating::Useful),
    };
    assert!(hub.message_rate(&Caller::Hub, rate).await.is_err());
}

#[tokio::test]
async fn a_new_channel_queues_the_guided_start_unless_paused() {
    let (hub, _dir) = test_hub().await;
    let create = |name: &str| consort_proto::client_api::ChannelCreate {
        name: name.to_owned(),
        purpose: String::new(),
        instructions: String::new(),
        icon: None,
        color: None,
        group_id: None,
        coordinator_selection: None,
        task_selection: None,
        repo_ids: Vec::new(),
        default_isolation: None,
        default_approval_mode: None,
        memory_enabled: None,
        tags: Vec::new(),
    };
    // Paused: a row in the log, no wake.
    let paused = hub
        .channel_create(&Caller::Hub, create("paused"))
        .await
        .unwrap()
        .channel;
    assert!(wakes(&hub, paused.id).await.is_empty());
    assert_eq!(
        log(&hub, Some(paused.id)).await[0].outcome,
        ProactiveOutcome::SkippedPaused
    );
    settings(&hub, |_| {}).await;
    let reading = hub
        .channel_create(&Caller::Hub, create("reading"))
        .await
        .unwrap()
        .channel;
    let events = wakes(&hub, reading.id).await;
    assert!(
        matches!(events.as_slice(), [WakeEvent::ChannelCreated { name, kind: ChannelKind::Standard, .. }] if name == "reading")
    );
    let entry = log(&hub, Some(reading.id)).await.remove(0);
    assert_eq!(
        (entry.kind, entry.outcome),
        (ProactiveKind::ChannelCreated, ProactiveOutcome::Pending)
    );
    // The greeting is marked as a new-channel message with its chips.
    let session = open_turn(&hub, reading.id, events).await;
    let posted = call(&hub, session, "post_message", json!({"body": "Do you want me to help you set it up?", "suggestions": [{"label": "Yes"}, {"label": "Not now"}]})).await.unwrap();
    let id: consort_proto::ids::MessageId = posted["message_id"].as_str().unwrap().parse().unwrap();
    let message = hub
        .store
        .read(move |conn| conversation::message_or_404(conn, id))
        .await
        .unwrap();
    assert_eq!(
        message.proactive.unwrap().kind,
        ProactiveKind::ChannelCreated
    );
    end_turn(&hub, reading.id, session, TurnEndReason::Completed).await;
    let entry = log(&hub, Some(reading.id)).await.remove(0);
    assert_eq!(entry.outcome, ProactiveOutcome::Posted);
    assert_eq!(entry.message_ids, vec![id]);
}

#[tokio::test]
async fn a_failed_proactive_turn_is_logged_as_failed() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "sayso", Some(ProactivityLevel::Normal)).await;
    let event = proactive_event(&hub, ch.id, ProactiveKind::CheckIn).await;
    let session = open_turn(&hub, ch.id, vec![event]).await;
    end_turn(&hub, ch.id, session, TurnEndReason::Failed).await;
    let entry = log(&hub, Some(ch.id)).await.remove(0);
    assert_eq!(entry.outcome, ProactiveOutcome::Failed);
    assert!(entry.detail.is_some());
}

#[tokio::test]
async fn the_log_pages_newest_first_with_filters() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let a = channel(&hub, "a", None).await;
    let b = channel(&hub, "b", None).await;
    for (channel_id, kind) in [
        (a.id, ProactiveKind::CheckIn),
        (b.id, ProactiveKind::FollowUp),
        (a.id, ProactiveKind::FollowUp),
    ] {
        proactive_event(&hub, channel_id, kind).await;
    }
    let page = |channel_id, kind, before| {
        hub.proactivity_log(
            &Caller::Hub,
            consort_proto::client_api::ProactivityLog {
                channel_id,
                kind,
                before,
                limit: Some(1),
            },
        )
    };
    let first = page(None, None, None).await.unwrap();
    assert_eq!(first.wakes.len(), 1);
    assert!(first.has_more);
    assert_eq!(first.wakes[0].channel_id, a.id);
    let next = page(None, None, Some(first.wakes[0].id)).await.unwrap();
    assert_eq!(next.wakes[0].channel_id, b.id);
    let only_a = page(Some(a.id), Some(ProactiveKind::CheckIn), None)
        .await
        .unwrap();
    assert_eq!(only_a.wakes[0].kind, ProactiveKind::CheckIn);
    assert!(!only_a.has_more);
    // The hourly purge removes entries older than 30 days.
    let purged = hub
        .store
        .write(|tx| tx.purge_wakes(Timestamp::now().plus_millis(HOUR)))
        .await
        .unwrap();
    assert_eq!(purged, 3);
}

mod review;
