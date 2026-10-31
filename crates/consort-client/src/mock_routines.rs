//! Mock routines and memory: the routines of house-lights, Tutti and
//! homelab, the run history of Sunset check, the memory notes of
//! house-lights, global memory and sayso, and the routine and memory
//! commands on the mock state. Times are relative to now, so the rows say
//! "yesterday" and "today" as in the designs.

use chrono::{Duration, NaiveTime, TimeZone};
use chrono_tz::Tz;
use consort_proto::model::{RoutineRunStatus, RoutineSchedule};
use consort_proto::time::Timestamp;

use crate::{
    ClientError, Command, CommandOutput,
    memory::{GLOBAL, MemoryNoteView},
    routines::{RoutineActionView, RoutineForm, RoutineRunView, RoutineView},
    view::{MemorySavedView, NoticeView, SelectionOption, SelectionView, TimelineItem, ViewState},
};

pub const LIGHTS: &str = "chn_house_lights";
pub const TUTTI: &str = "chn_tutti";
pub const HOMELAB: &str = "chn_homelab";
pub const SAYSO: &str = "chn_sayso";
pub const SUNSET: &str = "rtn_sunset_check";
pub const PRICE_WATCH: &str = "rtn_weekly_price_watch";
pub const CLIPS: &str = "rtn_order_clips";
const ROOF_TASK: &str = "tsk_0192f0c4-roofline";
const BERLIN: &str = "Europe/Berlin";
const DAY: i64 = 86_400_000;

fn berlin() -> Tz {
    BERLIN.parse().unwrap_or(Tz::UTC)
}

/// The time `days` days from today at `hour:minute` in Berlin. Negative
/// days are in the past.
fn at(days: i64, hour: u32, minute: u32) -> i64 {
    let zone = berlin();
    let now = zone.timestamp_millis_opt(crate::format::now_ms()).single();
    let Some(now) = now else { return 0 };
    let date = now.date_naive() + Duration::days(days);
    let time = NaiveTime::from_hms_opt(hour, minute, 0).unwrap_or_default();
    zone.from_local_datetime(&date.and_time(time))
        .earliest()
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

/// The time `days` days from today at `hour:minute` on this computer's
/// clock. A Once routine shows its time in this computer's time zone.
fn local_at(days: i64, hour: u32, minute: u32) -> i64 {
    let now = chrono::Local::now().date_naive() + Duration::days(days);
    let time = NaiveTime::from_hms_opt(hour, minute, 0).unwrap_or_default();
    chrono::Local
        .from_local_datetime(&now.and_time(time))
        .earliest()
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

/// The first day from now (today counts while its time is ahead) whose
/// weekday is `weekday` (Monday 0) at `hour:minute` in Berlin.
fn next_weekday(weekday: i64, hour: u32, minute: u32) -> i64 {
    use chrono::Datelike;
    let now = crate::format::now_ms();
    for days in 0..8 {
        let candidate = at(days, hour, minute);
        let day = berlin()
            .timestamp_millis_opt(candidate)
            .single()
            .map(|t| i64::from(t.weekday().num_days_from_monday()))
            .unwrap_or(-1);
        if day == weekday && candidate > now {
            return candidate;
        }
    }
    at(7, hour, minute)
}

/// The last day before now whose weekday is `weekday` at `hour:minute`.
fn last_weekday(weekday: i64, hour: u32, minute: u32) -> i64 {
    next_weekday(weekday, hour, minute) - 7 * DAY
}

fn cron(expression: &str) -> RoutineSchedule {
    RoutineSchedule::Cron {
        expression: expression.into(),
        time_zone: BERLIN.into(),
    }
}

/// A run of Sunset check.
fn run(
    id: u64,
    started_at: i64,
    seconds: i64,
    status: RoutineRunStatus,
    error: Option<&str>,
) -> RoutineRunView {
    RoutineRunView {
        id,
        routine_id: SUNSET.into(),
        started_at,
        ended_at: (status != RoutineRunStatus::Skipped).then_some(started_at + seconds * 1_000),
        status,
        manual: false,
        message_id: (status == RoutineRunStatus::Ok).then(|| format!("msg_sunset_{id}")),
        task_id: None,
        error: error.map(str::to_owned),
    }
}

/// The run history of Sunset check, as the design "Routine run history"
/// shows it.
fn sunset_runs() -> Vec<RoutineRunView> {
    let mut manual = run(1, at(-6, 14, 12), 64, RoutineRunStatus::Ok, None);
    manual.manual = true;
    vec![
        run(5, at(-1, 17, 30), 48, RoutineRunStatus::Ok, None),
        run(4, at(-2, 17, 30), 52, RoutineRunStatus::Ok, None),
        run(
            3,
            at(-3, 17, 30),
            3,
            RoutineRunStatus::Error,
            Some(
                "Claude Work is signed out. Sign in again in Settings > Providers, then select Run now.",
            ),
        ),
        run(
            2,
            at(-4, 17, 30),
            0,
            RoutineRunStatus::Skipped,
            Some("The last run was still going."),
        ),
        manual,
    ]
}

fn sonnet_medium() -> SelectionView {
    SelectionView {
        account_label: "Claude Work".into(),
        model_id: "claude-sonnet-5-5".into(),
        model_name: "Sonnet 5.5".into(),
        options: vec![(
            crate::picker::EFFORT.to_owned(),
            SelectionOption::Text("medium".into()),
        )],
    }
}

fn routine(
    id: &str,
    channel: &str,
    name: &str,
    schedule: RoutineSchedule,
    prompt: &str,
    action: RoutineActionView,
) -> RoutineView {
    RoutineView {
        id: id.into(),
        channel_id: channel.into(),
        name: name.into(),
        enabled: true,
        schedule,
        prompt: prompt.into(),
        action,
        next_run_at: None,
        last_run_at: None,
        last_run_status: None,
        monid: None,
        runs: Vec::new(),
        runs_loaded: false,
        created_at: None,
        kind: Default::default(),
        origin: Default::default(),
    }
}

/// The routines of the designs "Channel panel, Routines" and "Settings,
/// Routines".
pub fn routines() -> Vec<RoutineView> {
    let now = crate::format::now_ms();
    let today_or_tomorrow = |hour, minute| {
        let today = at(0, hour, minute);
        if today > now {
            today
        } else {
            at(1, hour, minute)
        }
    };
    let mut sunset = routine(
        SUNSET,
        LIGHTS,
        "Sunset check",
        cron("30 17 * * *"),
        "Check today's sunset time. If the porch lights are not set to turn on 10 minutes before it, change the schedule and tell me.",
        RoutineActionView::Ask,
    );
    sunset.last_run_at = Some(at(-1, 17, 30));
    sunset.last_run_status = Some(RoutineRunStatus::Ok);
    sunset.next_run_at = Some(today_or_tomorrow(17, 30));
    sunset.runs = sunset_runs();
    sunset.runs_loaded = true;

    let mut price = routine(
        PRICE_WATCH,
        LIGHTS,
        "Weekly price watch",
        cron("0 8 * * 1"),
        "Check the price of the three quotes in channel files against the vendors' sites. Write the changes to `quote-comparison.xlsx`.",
        RoutineActionView::Task {
            title: "Check roofline light prices".into(),
            selection: Some(sonnet_medium()),
        },
    );
    price.last_run_at = Some(last_weekday(0, 8, 0));
    price.last_run_status = Some(RoutineRunStatus::Error);
    price.next_run_at = Some(next_weekday(0, 8, 0));

    let mut clips = routine(
        CLIPS,
        LIGHTS,
        "Remind me to order the clips",
        RoutineSchedule::Once {
            at: Timestamp::from_millis(local_at(4, 9, 0)),
        },
        "Remind me to order the gutter clips for the roofline lights.",
        RoutineActionView::Ask,
    );
    clips.enabled = false;

    let mut brief = routine(
        "rtn_morning_brief",
        TUTTI,
        "Morning brief",
        cron("30 7 * * 1-5"),
        "Summarize my calendar, my open tasks and what changed overnight.",
        RoutineActionView::Ask,
    );
    brief.last_run_at = Some(at(0, 7, 30));
    brief.last_run_status = Some(RoutineRunStatus::Ok);
    brief.next_run_at = (0..5).map(|d| next_weekday(d, 7, 30)).min();

    let mut review = routine(
        "rtn_weekly_review",
        TUTTI,
        "Weekly review",
        cron("0 16 * * 5"),
        "Review the week: what got done, what is stuck, and what needs me next week.",
        RoutineActionView::Ask,
    );
    review.last_run_at = Some(last_weekday(4, 16, 0));
    review.last_run_status = Some(RoutineRunStatus::Ok);
    review.next_run_at = Some(next_weekday(4, 16, 0));

    let mut backup = routine(
        "rtn_nightly_backup",
        HOMELAB,
        "Nightly backup check",
        cron("0 2 * * *"),
        "Check that last night's backups finished on every machine, and restore one file from each to prove it.",
        RoutineActionView::Task {
            title: "Check the nightly backups".into(),
            selection: None,
        },
    );
    backup.last_run_at = Some(at(0, 2, 0));
    backup.last_run_status = Some(RoutineRunStatus::Ok);
    backup.next_run_at = Some(at(1, 2, 0));

    vec![sunset, price, clips, brief, review, backup]
}

fn note(
    scope: &str,
    path: &str,
    version: u64,
    by_user: bool,
    updated_at: i64,
    content: &str,
) -> MemoryNoteView {
    MemoryNoteView {
        scope: scope.into(),
        path: path.into(),
        content: Some(content.into()),
        sha256: format!("{:064x}", fxhash(&format!("{scope}:{path}:{version}"))),
        version,
        by_user,
        updated_at,
        suggested_by: None,
        deleted_at: None,
    }
}

/// A small stable hash for the mock note hashes.
fn fxhash(text: &str) -> u128 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325_u128, |h, b| {
        (h ^ u128::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

pub const ROOFLINE: &str = "# Roofline lights\n\n## Decision\nUse **Lumen Co** for the front and the garage side.\nTotal run: 208 ft (186 ft front, 22 ft garage).\n\n## Why\n- $21 a foot, with a 3-year warranty.\n- Brightline has a 5-year warranty, but costs $26 a foot.\n\n## Open\n- Ask Lumen Co about warm white, 2700 K.\n- Order the gutter clips before Nov 3.";

const KEVIN: &str = "# Kevin\n\nChris's brother. He lives in Leipzig and works on CI pipelines at Acme.\n\n## Good to know\n- Birthday: March 14.\n- Prefers Signal to email.\n- Knows Windows build runners well. Ask him first about CI on Windows.\n\n## Recent\n- Oct 27: offered to test Sayso on his old laptop, which has no AVX2.";

/// The versions of people/kevin.md, newest first, as the design "Settings,
/// Memory, version history" shows them.
fn kevin_versions() -> Vec<MemoryNoteView> {
    let texts = [
        "## Kevin\n\nChris's brother.",
        "## Kevin\n\nChris's brother. He lives in Leipzig.\n\n## Good to know\n- Birthday: March 14.",
        "## Kevin\n\nChris's brother. He lives in Leipzig.\n\n## Good to know\n- Birthday: March 14.\n- Prefers email.",
        "## Kevin\n\nChris's brother. He lives in Leipzig.\n\n## Good to know\n- Birthday: March 14.\n- Prefers email. Signal is for urgent things.\n- Knows Windows build runners well.",
        "## Kevin\n\nChris's brother. He lives in Leipzig and works on CI pipelines at Acme.\n\n## Good to know\n- Birthday: March 14.\n- Prefers Signal to email.\n- Knows Windows build runners well.",
        KEVIN,
    ];
    let days = [89, 77, 58, 39, 27, 3];
    let users = [false, false, false, true, false, false];
    let mut out: Vec<MemoryNoteView> = texts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            note(
                GLOBAL,
                "people/kevin.md",
                i as u64 + 1,
                users[i],
                at(-days[i], 9 + i as u32 * 2, 14),
                text,
            )
        })
        .collect();
    out[5].suggested_by = Some((SAYSO.into(), "sayso".into()));
    out.reverse();
    out
}

/// The memory notes of the designs "Channel panel, Memory" and "Settings,
/// Memory", with one note of sayso and one note in Recently deleted.
pub fn memory_notes() -> Vec<MemoryNoteView> {
    let now = crate::format::now_ms();
    let mut roofline = note(
        LIGHTS,
        "decisions/roofline.md",
        4,
        false,
        now - 2 * 60_000,
        ROOFLINE,
    );
    roofline.suggested_by = Some((ROOF_TASK.into(), "Price out roofline lighting".into()));
    let mut deleted = note(
        GLOBAL,
        "people/old-landlord.md",
        2,
        false,
        at(-40, 10, 0),
        "# Old landlord\n\nMr. Becker. The deposit came back in full.",
    );
    deleted.deleted_at = Some(at(-3, 11, 5));
    let mut kevin = kevin_versions().remove(0);
    kevin.content = Some(KEVIN.into());
    let mut notes = vec![
        roofline,
        note(
            LIGHTS,
            "decisions/porch-switches.md",
            2,
            false,
            at(-9, 10, 0),
            "# Porch switches\n\nThe porch lights run on the Shelly switch by the door. The schedule lives in the Shelly app.",
        ),
        note(
            LIGHTS,
            "vendors/brightline.md",
            3,
            false,
            at(0, 8, 10),
            "# Brightline\n\n$4,850 for 186 ft. 5-year warranty, dimmers included.",
        ),
        note(
            LIGHTS,
            "vendors/lumen-co.md",
            1,
            false,
            at(-3, 18, 0),
            "# Lumen Co\n\n$21 a foot. 3-year warranty. Ask about warm white.",
        ),
        note(
            LIGHTS,
            "house.md",
            2,
            false,
            at(-18, 12, 0),
            "# The house\n\nTwo floors, a garage on the east side, gutters all around.",
        ),
        note(
            LIGHTS,
            "preferences.md",
            1,
            true,
            at(-18, 12, 30),
            "# Preferences\n\n- Warm white only.\n- Lights off at midnight.",
        ),
        kevin,
        note(
            GLOBAL,
            "people/mom.md",
            3,
            false,
            at(-27, 9, 40),
            "# Mom\n\nLives in Hamburg. Calls on Sundays.",
        ),
        note(
            GLOBAL,
            "people/sam.md",
            2,
            false,
            at(-42, 11, 0),
            "# Sam\n\nFriend from university. Runs the homelab Discord.",
        ),
        note(
            GLOBAL,
            "places/berlin.md",
            1,
            false,
            at(-30, 9, 0),
            "# Berlin\n\nHome.",
        ),
        note(
            GLOBAL,
            "places/hamburg.md",
            1,
            false,
            at(-30, 9, 5),
            "# Hamburg\n\nMom lives here.",
        ),
        note(
            GLOBAL,
            "places/leipzig.md",
            1,
            false,
            at(-30, 9, 10),
            "# Leipzig\n\nKevin lives here.",
        ),
        note(
            GLOBAL,
            "places/office.md",
            1,
            false,
            at(-30, 9, 15),
            "# The office\n\nCoworking space in Kreuzberg.",
        ),
        note(
            GLOBAL,
            "about-me.md",
            5,
            false,
            at(-10, 9, 0),
            "# About me\n\nChris. Builds Consort and Sayso. Lives in Berlin.",
        ),
        note(
            GLOBAL,
            "schedule.md",
            9,
            false,
            at(0, 7, 31),
            "# Schedule\n\n- Deep work 9:00 to 12:00.\n- No meetings on Fridays.",
        ),
        note(
            GLOBAL,
            "writing-style.md",
            1,
            true,
            at(-58, 16, 0),
            "# Writing style\n\nShort sentences. No em dashes.",
        ),
        note(
            SAYSO,
            "release.md",
            4,
            false,
            at(-2, 15, 0),
            "# Release\n\nShip from the release branch. Notarize on the MacBook Pro.",
        ),
        note(
            SAYSO,
            "testers.md",
            2,
            false,
            at(-6, 15, 0),
            "# Testers\n\nKevin tests on an old laptop with no AVX2.",
        ),
        deleted,
    ];
    for n in &mut notes {
        if n.path == "people/kevin.md" {
            n.suggested_by = Some((SAYSO.into(), "sayso".into()));
        }
    }
    notes
}

/// The memory saved line under the last bubble of house-lights.
pub fn saved_line() -> TimelineItem {
    TimelineItem::Notice(NoticeView {
        message_id: "msg_lights_memory_saved".into(),
        time_label: "8:27 PM".into(),
        created_at: crate::format::now_ms(),
        kind: "memory_saved".into(),
        text: "Updated the memory note decisions/roofline.md, from a suggestion by Price out roofline lighting".into(),
        memory: Some(MemorySavedView {
            scope: LIGHTS.into(),
            path: "decisions/roofline.md".into(),
            version: 4,
            created: false,
            suggested_by: "Price out roofline lighting".into(),
        }), draft: None, fallback: None, error: None, monid: None,
    })
}

/// Adds the routines and the memory to a mock state.
pub fn with_routines_and_memory(mut state: ViewState) -> ViewState {
    state.routines = routines();
    state.memory_notes = memory_notes();
    state.memory_loaded = vec![GLOBAL.into(), LIGHTS.into(), SAYSO.into()];
    state.memory_deleted_loaded = vec![GLOBAL.into(), LIGHTS.into(), SAYSO.into()];
    state
}

fn invalid(message: &str) -> ClientError {
    ClientError::Hub {
        code: "invalid".into(),
        message: message.into(),
    }
}

fn not_found(message: &str) -> ClientError {
    ClientError::Hub {
        code: "not_found".into(),
        message: message.into(),
    }
}

fn routine_mut<'a>(state: &'a mut ViewState, id: &str) -> Result<&'a mut RoutineView, ClientError> {
    state
        .routines
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| not_found("This routine no longer exists."))
}

/// The routine of a saved form.
fn from_form(id: String, form: &RoutineForm, now: i64) -> Result<RoutineView, ClientError> {
    if let Some(error) = form.errors(now).into_iter().next() {
        return Err(invalid(&error.message));
    }
    let schedule = form
        .schedule
        .schedule(now)
        .map_err(|errors| invalid(&errors[0].message))?;
    Ok(RoutineView {
        id,
        channel_id: form.channel_id.clone(),
        name: form.name.trim().to_owned(),
        enabled: form.enabled,
        schedule,
        prompt: form.prompt.clone(),
        action: if form.starts_task {
            RoutineActionView::Task {
                title: form.title.trim().to_owned(),
                selection: form.selection.clone(),
            }
        } else {
            RoutineActionView::Ask
        },
        next_run_at: if form.enabled {
            form.schedule.next_runs(now, 1).first().copied()
        } else {
            None
        },
        last_run_at: None,
        last_run_status: None,
        monid: None,
        runs: Vec::new(),
        runs_loaded: true,
        created_at: None,
        kind: Default::default(),
        origin: Default::default(),
    })
}

fn note_mut<'a>(
    state: &'a mut ViewState,
    scope: &str,
    path: &str,
) -> Option<&'a mut MemoryNoteView> {
    state
        .memory_notes
        .iter_mut()
        .find(|n| n.scope == scope && n.path == path)
}

/// Applies a routine or memory command to the mock state.
pub(crate) fn apply(
    state: &mut ViewState,
    command: Command,
    next_id: &mut u64,
) -> Result<CommandOutput, ClientError> {
    let now = crate::format::now_ms();
    match command {
        Command::SaveRoutine { routine_id, form } => {
            let id = routine_id.clone().unwrap_or_else(|| {
                *next_id += 1;
                format!("rtn_mock_{next_id}")
            });
            let mut saved = from_form(id.clone(), &form, now)?;
            match routine_id {
                Some(id) => {
                    let old = routine_mut(state, &id)?;
                    saved.last_run_at = old.last_run_at;
                    saved.last_run_status = old.last_run_status;
                    saved.runs = std::mem::take(&mut old.runs);
                    *old = saved;
                }
                None => state.routines.push(saved),
            }
            return Ok(CommandOutput::RoutineSaved(id));
        }
        Command::SetRoutineEnabled {
            routine_id,
            enabled,
        } => {
            let routine = routine_mut(state, &routine_id)?;
            routine.enabled = enabled;
            routine.next_run_at = if enabled {
                let form = crate::routines::ScheduleForm::from_schedule(
                    &routine.schedule,
                    &crate::routines::local_time_zone(),
                    now,
                );
                form.next_runs(now, 1).first().copied()
            } else {
                None
            };
        }
        Command::DeleteRoutine { routine_id } => {
            routine_mut(state, &routine_id)?;
            state.routines.retain(|r| r.id != routine_id);
        }
        Command::RunRoutineNow { routine_id } => {
            let routine = routine_mut(state, &routine_id)?;
            if routine
                .runs
                .first()
                .is_some_and(|r| r.ended_at.is_none() && r.status == RoutineRunStatus::Running)
            {
                return Err(ClientError::Hub {
                    code: "conflict".into(),
                    message: format!(
                        "The last run of {} has not ended yet. Wait until it ends, then select Run now again.",
                        routine.name
                    ),
                });
            }
            *next_id += 1;
            let task = matches!(routine.action, RoutineActionView::Task { .. });
            routine.runs.insert(
                0,
                RoutineRunView {
                    id: 100 + *next_id,
                    routine_id: routine_id.clone(),
                    started_at: now,
                    ended_at: Some(now + 2_000),
                    status: RoutineRunStatus::Ok,
                    manual: true,
                    message_id: (!task).then(|| format!("msg_run_{next_id}")),
                    task_id: task.then(|| ROOF_TASK.to_owned()),
                    error: None,
                },
            );
            routine.last_run_at = Some(now);
            routine.last_run_status = Some(RoutineRunStatus::Ok);
        }
        Command::LoadRoutineRuns { routine_id } => {
            routine_mut(state, &routine_id)?.runs_loaded = true;
        }
        Command::LoadMemory { scope, deleted } => {
            let list = if deleted {
                &mut state.memory_deleted_loaded
            } else {
                &mut state.memory_loaded
            };
            if !list.contains(&scope) {
                list.push(scope);
            }
        }
        Command::ReadMemoryNote { scope, path } => {
            let note = note_mut(state, &scope, &path)
                .filter(|n| n.deleted_at.is_none())
                .ok_or_else(|| not_found("There is no memory note at that path."))?;
            return Ok(CommandOutput::MemoryNote(note.clone()));
        }
        Command::WriteMemoryNote {
            scope,
            path,
            content,
            expected_sha256,
        } => {
            let path = crate::memory::note_path(&path).map_err(|m| invalid(&m))?;
            let current = note_mut(state, &scope, &path)
                .filter(|n| n.deleted_at.is_none())
                .map(|n| n.sha256.clone());
            if current != expected_sha256 {
                return Err(ClientError::Hub {
                    code: "stale_write".into(),
                    message:
                        "The note changed since you read it. Read it again, then retry the write."
                            .into(),
                });
            }
            let version = note_mut(state, &scope, &path).map_or(0, |n| n.version) + 1;
            let mut written = note(&scope, &path, version, true, now, &content);
            written.sha256 = format!("{:064x}", fxhash(&format!("{scope}:{path}:{content}")));
            state
                .memory_notes
                .retain(|n| !(n.scope == scope && n.path == path));
            state.memory_notes.push(written.clone());
            return Ok(CommandOutput::MemoryNote(written));
        }
        Command::DeleteMemoryNote { scope, path, .. } => {
            let note = note_mut(state, &scope, &path)
                .ok_or_else(|| not_found("There is no memory note at that path."))?;
            note.deleted_at = Some(now);
        }
        Command::RestoreMemoryNote { scope, path } => {
            let note = note_mut(state, &scope, &path)
                .filter(|n| n.deleted_at.is_some())
                .ok_or_else(|| not_found("The note is not in Recently deleted."))?;
            note.deleted_at = None;
        }
        Command::MoveMemoryNote {
            scope,
            from_path,
            to_path,
            ..
        } => {
            let to_path = crate::memory::note_path(&to_path).map_err(|m| invalid(&m))?;
            if note_mut(state, &scope, &to_path).is_some_and(|n| n.deleted_at.is_none()) {
                return Err(ClientError::Hub {
                    code: "conflict".into(),
                    message: format!("A note at {to_path} exists already. Choose another name."),
                });
            }
            let note = note_mut(state, &scope, &from_path)
                .ok_or_else(|| not_found("There is no memory note at that path."))?;
            note.path = to_path;
            return Ok(CommandOutput::MemoryNote(note.clone()));
        }
        Command::LoadMemoryHistory { scope, path } => {
            if scope == GLOBAL && path == "people/kevin.md" {
                return Ok(CommandOutput::MemoryHistory(kevin_versions()));
            }
            let note = note_mut(state, &scope, &path)
                .ok_or_else(|| not_found("There is no memory note at that path."))?
                .clone();
            let mut versions = vec![note.clone()];
            let content = note.content.clone().unwrap_or_default();
            for version in (1..note.version).rev() {
                let lines: Vec<&str> = content.lines().collect();
                let keep = lines
                    .len()
                    .saturating_sub((note.version - version) as usize)
                    .max(1);
                let mut old = note.clone();
                old.version = version;
                old.content = Some(lines[..keep.min(lines.len())].join("\n"));
                old.updated_at = note.updated_at - (note.version - version) as i64 * 5 * DAY;
                old.suggested_by = None;
                versions.push(old);
            }
            return Ok(CommandOutput::MemoryHistory(versions));
        }
        other => {
            return Err(ClientError::Other(format!(
                "The mock cannot run {other:?}."
            )));
        }
    }
    Ok(CommandOutput::Done)
}

/// The mock for a screen of the phase 2 designs, by its `--screen` name.
/// `None` for a screen that is not one of them.
pub fn screen_state(screen: &str) -> Option<ViewState> {
    let base = || {
        let mut state = with_routines_and_memory(crate::mock::view_state_with_files());
        if let Some(timeline) = state.timelines.iter_mut().find(|t| t.channel_id == LIGHTS) {
            timeline.items.push(saved_line());
        }
        state
    };
    match screen {
        "routines" | "routine-runs" | "routine-new" | "routine-edit" | "schedule-picker"
        | "schedule-interval" | "schedule-once" | "schedule-once-past" | "schedule-cron"
        | "settings-routines" | "memory" | "memory-conflict" | "settings-memory"
        | "memory-history" | "memory-deleted" | "routines-empty" | "memory-empty" => Some(base()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_routines_read_as_the_designs() {
        let all = routines();
        assert_eq!(all.iter().filter(|r| r.channel_id == LIGHTS).count(), 3);
        let now = crate::format::now_ms();
        let sunset = &all[0];
        assert_eq!(
            crate::routines::summary_line(sunset),
            "Every day at 17:30, asks the assistant"
        );
        assert_eq!(
            crate::routines::status_line(sunset, now).result,
            "Succeeded yesterday."
        );
        assert_eq!(
            crate::routines::status_line(&all[2], now).result,
            crate::routines::OFF_TEXT
        );
        let runs = crate::routines::recent_runs(sunset, now);
        assert_eq!(runs.len(), 5);
        assert!(runs[4].manual);
    }

    #[test]
    fn mock_commands_change_the_state() {
        let mut state = with_routines_and_memory(ViewState::default());
        let mut next = 0;
        apply(
            &mut state,
            Command::SetRoutineEnabled {
                routine_id: CLIPS.into(),
                enabled: true,
            },
            &mut next,
        )
        .unwrap();
        assert!(state.routines[2].enabled);
        let kevin = state
            .memory_notes
            .iter()
            .find(|n| n.path == "people/kevin.md")
            .unwrap()
            .clone();
        let stale = apply(
            &mut state,
            Command::WriteMemoryNote {
                scope: GLOBAL.into(),
                path: kevin.path.clone(),
                content: "x".into(),
                expected_sha256: Some("0".repeat(64)),
            },
            &mut next,
        )
        .unwrap_err();
        assert!(matches!(stale, ClientError::Hub { ref code, .. } if code == "stale_write"));
        let history = apply(
            &mut state,
            Command::LoadMemoryHistory {
                scope: GLOBAL.into(),
                path: kevin.path.clone(),
            },
            &mut next,
        )
        .unwrap();
        let CommandOutput::MemoryHistory(versions) = history else {
            panic!("no history");
        };
        assert_eq!(versions.len(), 6);
        assert_eq!(versions[0].version, 6);
    }
}
