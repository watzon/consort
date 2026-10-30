//! The proactive routines: the check-in, the morning brief, the research
//! pass and follow-ups. The hub makes and pauses them with the level of
//! their channel, and gates each run before a model runs.

use consort_proto::ids::{ChannelId, RoutineId};
use consort_proto::model::{
    CHECK_IN_CHANNEL_SECONDS, CHECK_IN_MIN_SECONDS, CHECK_IN_NOTE_PATH, CHECK_IN_TUTTI_SECONDS,
    Channel, ChannelKind, ExpectedHash, MemoryAuthor, MemoryScope, ProactiveKind, ProactiveOutcome,
    ProactiveWake, ProactivityLevel, ProactivitySettings, Routine, RoutineAction, RoutineKind,
    RoutineOrigin, RoutineRun, RoutineSchedule,
};
use consort_proto::time::Timestamp;
use consort_proto::turn_input::WakeEvent;
use rusqlite::Connection;

use super::gate::{self, Gate, GateInput};
use super::summary;
use crate::error::{HubError, Result};
use crate::scheduler;
use crate::store::{Tx, conversation, knowledge, meta_get, proactivity as store, routines};

/// The meta key that says the hub made the morning brief once.
const BRIEF_MADE_KEY: &str = "proactivity.brief_made";
/// The meta key that says the hub gave every channel its proactive
/// routines once, at the first start with proactivity.
const DEFAULTS_KEY: &str = "proactivity.defaults_v1";

/// The fixed prompt of a check-in. The user tunes the check-in in the
/// check-in note, not here.
pub(crate) const CHECK_IN_PROMPT: &str = "Look for things that need the user. The event has what changed in the hub since the last check-in and the check-in note. Read the note first: it says what to watch and what to skip. Post only what needs the user, with a link to its source. When nothing needs the user, call no_reply_needed with a finding.";

/// The prompt of the morning brief.
pub(crate) const BRIEF_PROMPT: &str = "Write the morning brief. Read every channel (list_channels, read_channel), the tasks, and the connected mail and calendar. Post one ranked list of at most 5 items in three groups: Needs you, Ready for review, Done since yesterday. Number the items through all groups. Link each item to its task, message or source. Prepare drafts as tasks where they help: they run in draft mode. If nothing needs the user, post nothing: call no_reply_needed with a finding. The brief may notify once.";

/// The prompt of the research pass, for the routine row. The wake event
/// itself carries no prompt; the system prompt has the rules.
pub(crate) const RESEARCH_PROMPT: &str = "The daily read-only research pass. Read the channels, memory and the connected services. Keep what you learn in memory under research/. Make at most 3 offers a day with ask_decision and offer set. Post nothing else.";

/// The first content of Tutti's check-in note.
fn starting_note(has_connection: bool) -> String {
    let watch = if has_connection {
        "Watch tasks that need me, and my mail and calendar."
    } else {
        "Watch tasks that need me."
    };
    let front = if has_connection {
        "watch: outside\n"
    } else {
        ""
    };
    format!(
        "{front}{watch}\nSkip newsletters, receipts and automated notices, unless a payment failed.\nAsk before anything outward.\n"
    )
}

fn new_routine(
    channel_id: ChannelId,
    kind: RoutineKind,
    name: &str,
    schedule: RoutineSchedule,
    prompt: &str,
    now: Timestamp,
) -> Routine {
    Routine {
        id: RoutineId::new(),
        channel_id,
        name: name.to_owned(),
        schedule,
        prompt: prompt.to_owned(),
        action: RoutineAction::CoordinatorTurn,
        enabled: true,
        next_run_at: None,
        last_run_at: None,
        last_run_status: None,
        kind,
        origin: RoutineOrigin::Hub,
        created_at: Some(now),
    }
}

/// The next run of a hub routine, from now, with its jitter for a check-in
/// and the research pass.
fn first_run(routine: &Routine, now: Timestamp) -> Result<Option<Timestamp>> {
    let next = crate::schedule::next_run_at(&routine.schedule, now)?;
    Ok(next.map(|at| match routine.kind {
        RoutineKind::CheckIn | RoutineKind::Research => at.plus_millis(gate::jitter_ms(routine.id)),
        _ => at,
    }))
}

/// Makes and pauses the proactive routines of a channel for its level:
/// a check-in from Normal, the research pass at Eager, and (in Tutti, once)
/// the morning brief. A check-in that the user deleted comes back only when
/// the level changes.
pub(crate) fn sync_channel_routines(tx: &mut Tx<'_>, channel: &Channel) -> Result<()> {
    if channel.archived_at.is_some() {
        return Ok(());
    }
    let now = tx.now();
    let level = channel.proactivity_level();
    let settings = store::settings(tx.db())?;
    let existing = routines::routines_in_channel(tx.db(), channel.id)?;

    let want_check_in = level >= ProactivityLevel::Normal;
    match existing.iter().find(|r| r.kind == RoutineKind::CheckIn) {
        Some(routine) if routine.enabled != want_check_in => {
            let mut routine = routine.clone();
            routine.enabled = want_check_in;
            routine.next_run_at = if want_check_in {
                first_run(&routine, now)?
            } else {
                None
            };
            tx.save_routine(&routine)?;
        }
        Some(_) => {}
        None if want_check_in => {
            let seconds = match channel.kind {
                ChannelKind::Tutti => CHECK_IN_TUTTI_SECONDS,
                ChannelKind::Standard => CHECK_IN_CHANNEL_SECONDS,
            };
            let mut routine = new_routine(
                channel.id,
                RoutineKind::CheckIn,
                "Check-in",
                RoutineSchedule::Interval { seconds },
                CHECK_IN_PROMPT,
                now,
            );
            routine.next_run_at = first_run(&routine, now)?;
            tx.save_routine(&routine)?;
            if channel.is_tutti() {
                write_starting_note(tx)?;
            }
        }
        None => {}
    }

    let want_research = level >= ProactivityLevel::Eager;
    match existing.iter().find(|r| r.kind == RoutineKind::Research) {
        Some(routine) if routine.enabled != want_research => {
            let mut routine = routine.clone();
            routine.enabled = want_research;
            routine.next_run_at = if want_research {
                first_run(&routine, now)?
            } else {
                None
            };
            tx.save_routine(&routine)?;
        }
        Some(_) => {}
        None if want_research => {
            let mut routine = new_routine(
                channel.id,
                RoutineKind::Research,
                "Research",
                RoutineSchedule::Cron {
                    expression: "30 6 * * *".to_owned(),
                    time_zone: gate::zone_name(&settings),
                },
                RESEARCH_PROMPT,
                now,
            );
            routine.next_run_at = first_run(&routine, now)?;
            tx.save_routine(&routine)?;
        }
        None => {}
    }

    if channel.is_tutti()
        && level >= ProactivityLevel::Normal
        && meta_get(tx.db(), BRIEF_MADE_KEY)?.is_none()
    {
        let mut routine = new_routine(
            channel.id,
            RoutineKind::Brief,
            "Morning brief",
            RoutineSchedule::Cron {
                expression: "30 7 * * 1-5".to_owned(),
                time_zone: gate::zone_name(&settings),
            },
            BRIEF_PROMPT,
            now,
        );
        routine.next_run_at = first_run(&routine, now)?;
        tx.save_routine(&routine)?;
        tx.meta_set(BRIEF_MADE_KEY, "1")?;
    }
    Ok(())
}

/// Writes Tutti's check-in note with its three starting lines, when the
/// global memory never had one.
fn write_starting_note(tx: &mut Tx<'_>) -> Result<()> {
    let exists = knowledge::memory_note(tx.db(), MemoryScope::Global, CHECK_IN_NOTE_PATH)?
        .is_some()
        || knowledge::deleted_memory_note(tx.db(), MemoryScope::Global, CHECK_IN_NOTE_PATH)?
            .is_some();
    if exists {
        return Ok(());
    }
    let has_connection = !crate::store::connectors::connections(tx.db())?.is_empty();
    tx.write_memory(
        MemoryScope::Global,
        CHECK_IN_NOTE_PATH,
        &starting_note(has_connection),
        &ExpectedHash::NotExists,
        MemoryAuthor::User,
        None,
    )?;
    Ok(())
}

/// Gives every channel its proactive routines, once per hub. An existing
/// hub gets them at its first start with proactivity. Returns true when it
/// ran.
pub(crate) fn make_defaults_once(tx: &mut Tx<'_>) -> Result<bool> {
    if meta_get(tx.db(), DEFAULTS_KEY)?.is_some() {
        return Ok(false);
    }
    for channel in conversation::channels(tx.db())? {
        sync_channel_routines(tx, &channel)?;
    }
    tx.meta_set(DEFAULTS_KEY, "1")?;
    Ok(true)
}

/// Moves the cron routines that the hub made (the brief and the research
/// pass) to the user's new time zone.
pub(crate) fn retime_hub_routines(tx: &mut Tx<'_>, settings: &ProactivitySettings) -> Result<()> {
    let zone = gate::zone_name(settings);
    let now = tx.now();
    for mut routine in routines::routines(tx.db())? {
        if routine.origin != RoutineOrigin::Hub {
            continue;
        }
        let RoutineSchedule::Cron { time_zone, .. } = &mut routine.schedule else {
            continue;
        };
        if *time_zone == zone {
            continue;
        }
        time_zone.clone_from(&zone);
        if routine.enabled {
            routine.next_run_at = first_run(&routine, now)?;
        }
        tx.save_routine(&routine)?;
    }
    Ok(())
}

/// Checks a change that a client or a session makes to a routine of a
/// proactive kind. The hub fixes the prompt and the action. A check-in
/// keeps an interval of at least 30 minutes, and a new interval ends
/// back-off.
pub(crate) fn check_routine_change(
    tx: &mut Tx<'_>,
    before: &Routine,
    after: &Routine,
) -> Result<()> {
    if before.kind == RoutineKind::Custom {
        return Ok(());
    }
    if after.prompt != before.prompt || after.action != before.action {
        return Err(HubError::invalid(match before.kind {
            RoutineKind::CheckIn => {
                "The hub sets the prompt of a check-in. Change what it watches in check-in.md."
            }
            _ => {
                "The hub sets the prompt and the action of this routine. Change its schedule or turn it off."
            }
        }));
    }
    if before.kind == RoutineKind::CheckIn && after.schedule != before.schedule {
        match after.schedule {
            RoutineSchedule::Interval { seconds } if seconds >= CHECK_IN_MIN_SECONDS => {
                let mut state = store::channel_state(tx.db(), after.channel_id)?;
                state.base_seconds = None;
                state.silent_streak = 0;
                tx.save_channel_state(after.channel_id, &state)?;
            }
            _ => {
                return Err(HubError::invalid(
                    "A check-in runs on an interval of at least 30 minutes. Choose a longer interval.",
                ));
            }
        }
    }
    Ok(())
}

/// The note of a channel's check-in: Tutti's note in global memory, or the
/// channel's own note. Empty when there is none.
pub(crate) fn check_in_note(conn: &Connection, channel: &Channel) -> Result<String> {
    Ok(
        knowledge::memory_note(conn, channel.memory_scope(), CHECK_IN_NOTE_PATH)?
            .map(|note| note.content)
            .unwrap_or_default(),
    )
}

/// A log entry that waits for its turn.
pub(crate) fn pending_wake(
    channel_id: ChannelId,
    kind: ProactiveKind,
    at: Timestamp,
    routine_id: Option<RoutineId>,
) -> ProactiveWake {
    ProactiveWake {
        id: 0,
        channel_id,
        kind,
        at,
        outcome: ProactiveOutcome::Pending,
        detail: None,
        message_ids: Vec::new(),
        task_id: None,
        routine_id,
        notify_held: false,
        ended_at: None,
    }
}

/// Starts a run of a proactive routine: the gate, the log entry, the wake
/// event and the routine run. Called by the scheduler for a routine whose
/// kind is not `custom`. Returns the run and the channel to kick.
pub(crate) fn begin_proactive_run(
    tx: &mut Tx<'_>,
    mut routine: Routine,
    kind: ProactiveKind,
    channel: &Channel,
    now: Timestamp,
    scheduled: bool,
) -> Result<std::result::Result<(RoutineRun, Option<ChannelId>), String>> {
    let settings = store::settings(tx.db())?;
    let busy = scheduler::still_running(tx, &routine, now)?;
    if busy && !scheduled {
        return Ok(Err(format!(
            "The last run of {} has not ended yet. Wait until it ends, then select Run now again.",
            routine.name
        )));
    }
    let decision = if scheduled {
        let turns_today =
            store::turns_since(tx.db(), channel.id, gate::start_of_day(&settings, now))?;
        let decided = gate::decide(&GateInput {
            kind,
            level: channel.proactivity_level(),
            channel_name: &channel.name,
            channel_kind: channel.kind,
            settings: &settings,
            now,
            turns_today,
        });
        match decided {
            Gate::Run if busy => Gate::Skip(
                ProactiveOutcome::SkippedBusy,
                "The last one still waited for its turn.".to_owned(),
            ),
            other => other,
        }
    } else {
        Gate::Run
    };

    let due = if scheduled {
        routine.next_run_at.unwrap_or(now)
    } else {
        now
    };
    let mut decision = decision;
    let mut check_in: Option<(String, String, Vec<String>)> = None;
    if decision == Gate::Run && kind == ProactiveKind::CheckIn {
        let mut state = store::channel_state(tx.db(), channel.id)?;
        let since = state.last_check_in_at.unwrap_or(channel.created_at);
        let changes = summary::change_summary(tx.db(), channel, since)?;
        let note = check_in_note(tx.db(), channel)?;
        state.last_check_in_at = Some(now);
        tx.save_channel_state(channel.id, &state)?;
        if scheduled && changes.is_empty() && !summary::watches_outside(&note) {
            decision = Gate::Skip(
                ProactiveOutcome::SkippedNoChange,
                "No change since the last check-in.".to_owned(),
            );
        } else {
            let findings = store::recent_findings(tx.db(), channel.id, 3)?;
            check_in = Some((changes, summary::note_for_check_in(&note), findings));
        }
    }

    let schedule_error = if scheduled {
        scheduler::advance(&mut routine, now)
    } else {
        None
    };
    match decision {
        Gate::Quiet | Gate::Skip(..) => {
            let reason = match &decision {
                Gate::Skip(outcome, detail) => {
                    let mut wake = pending_wake(channel.id, kind, due, Some(routine.id));
                    wake.outcome = *outcome;
                    wake.detail = Some(detail.clone());
                    wake.ended_at = Some(now);
                    tx.insert_wake(&mut wake)?;
                    format!("{} {detail}", outcome.label())
                }
                _ => format!(
                    "The proactivity level of {} is {}.",
                    channel.name,
                    channel.proactivity_level().label()
                ),
            };
            let mut run = scheduler::skipped_run(&routine, due, &reason);
            scheduler::add_schedule_error(&mut run, schedule_error);
            if !busy {
                scheduler::note_last_run(&mut routine, &run);
            }
            tx.record_routine_run(&mut run)?;
            if routine.kind == RoutineKind::FollowUp {
                tx.delete_routine(&routine)?;
                tx.delete_follow_up_row(routine.id)?;
            } else {
                tx.save_routine(&routine)?;
            }
            Ok(Ok((run, None)))
        }
        Gate::Run => {
            let mut wake = pending_wake(channel.id, kind, due, Some(routine.id));
            tx.insert_wake(&mut wake)?;
            let log = wake.id;
            let event = match kind {
                ProactiveKind::CheckIn => {
                    let (summary, note, recent_findings) = check_in.unwrap_or_default();
                    WakeEvent::CheckIn {
                        routine: routine.id,
                        summary,
                        note,
                        recent_findings,
                        log,
                    }
                }
                ProactiveKind::Brief => WakeEvent::Brief {
                    routine: routine.id,
                    prompt: routine.prompt.clone(),
                    log,
                },
                ProactiveKind::Research => WakeEvent::Research {
                    routine: routine.id,
                    log,
                },
                _ => {
                    let set_at = store::follow_up(tx.db(), routine.id)?
                        .map(|f| f.set_at)
                        .or(routine.created_at)
                        .unwrap_or(now);
                    WakeEvent::FollowUp {
                        routine: routine.id,
                        reason: routine.prompt.clone(),
                        set_at,
                        log,
                    }
                }
            };
            tx.queue_wake(channel.id, &event)?;
            let mut run = scheduler::new_run(&routine, now, !scheduled);
            scheduler::add_schedule_error(&mut run, schedule_error);
            scheduler::note_last_run(&mut routine, &run);
            tx.record_routine_run(&mut run)?;
            tx.save_routine(&routine)?;
            Ok(Ok((run, Some(channel.id))))
        }
    }
}

/// The routine ID in a proactive event that a routine caused.
pub(crate) fn routine_of(event: &WakeEvent) -> Option<RoutineId> {
    match event {
        WakeEvent::Routine { id, .. } => Some(*id),
        WakeEvent::CheckIn { routine, .. }
        | WakeEvent::Brief { routine, .. }
        | WakeEvent::FollowUp { routine, .. }
        | WakeEvent::Research { routine, .. } => Some(*routine),
        _ => None,
    }
}
