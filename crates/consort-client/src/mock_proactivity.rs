//! Mock proactivity: the content of the proactivity designs (Settings >
//! Proactivity, the proactive messages and the offer in Tutti, the
//! Routines tab with a check-in and follow-ups) and of the guided start of
//! a new channel (reading), and the proactivity commands on the mock state.
//!
//! The designs' day is a Thursday at about 11:50 AM. Times are relative to
//! today on this computer's clock, so the rows say "Today" as in the
//! designs.

use chrono::{Duration, NaiveTime, TimeZone};
use consort_proto::model::{
    ChannelKind, ProactiveKind, ProactivityLevel, RoutineKind, RoutineOrigin, RoutineRunStatus,
    RoutineSchedule,
};

use crate::proactivity::{
    ChannelLevelView, FollowUpView, LogGroupView, LogRowView, ProactiveMarkView, ProactivityView,
    ResultTone, SnoozeOption, SuggestionView, follow_up_when, level_meaning,
};
use crate::routines::{RoutineActionView, RoutineView};
use crate::view::{
    ChannelView, DayView, DecisionCardView, DecisionOption, DecisionView, MessageView, NoticeView,
    RequestState, Speaker, TimelineItem, TimelineView, ViewState,
};
use crate::{ClientError, Command, CommandOutput};

pub const TUTTI: &str = "chn_tutti";
pub const LIGHTS: &str = "chn_house_lights";
pub const SAYSO: &str = "chn_sayso";
pub const LUCY: &str = "chn_miss_lucy_tg";
pub const HOMELAB: &str = "chn_homelab";
pub const READING: &str = "chn_reading";
pub const CHECK_IN: &str = "rtn_lights_check_in";
pub const FOLLOW_UP_QUOTE: &str = "rtn_follow_up_brightline";
pub const FOLLOW_UP_SWITCH: &str = "rtn_follow_up_porch_switch";
pub const BRIEF_ROUTINE: &str = "rtn_morning_brief";
pub const OFFER_MESSAGE: &str = "msg_offer_dana";
pub const BRIEF_MESSAGE: &str = "msg_morning_brief";
pub const CHECK_IN_MESSAGE: &str = "msg_check_in_dentist";
pub const FOLLOW_UP_MESSAGE: &str = "msg_follow_up_vendor";
pub const GREETING: &str = "msg_reading_greeting";
pub const SETUP_OFFER: &str = "msg_reading_setup";

/// Today, or `days` days from today, at `hour:minute` on this computer's
/// clock.
fn local_at(days: i64, hour: u32, minute: u32) -> i64 {
    let date = chrono::Local::now().date_naive() + Duration::days(days);
    let time = NaiveTime::from_hms_opt(hour, minute, 0).unwrap_or_default();
    chrono::Local
        .from_local_datetime(&date.and_time(time))
        .earliest()
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

fn mark(kind: ProactiveKind, reason: Option<&str>) -> ProactiveMarkView {
    ProactiveMarkView {
        kind: Some(kind),
        label: crate::proactivity::mark_label(kind, false).to_owned(),
        icon: crate::proactivity::mark_icon(kind, false).to_owned(),
        reason: reason.map(Into::into),
        offer: false,
        notify: false,
        rating: None,
        show_caption: true,
    }
}

fn assistant(id: &str, channel: &str, at: i64, body: &str) -> MessageView {
    MessageView {
        id: id.into(),
        channel_id: channel.into(),
        speaker: Speaker::Consort,
        time_label: crate::format::time_label(at),
        created_at: at,
        body: body.into(),
        ..MessageView::default()
    }
}

fn you(id: &str, channel: &str, at: i64, body: &str) -> MessageView {
    MessageView {
        speaker: Speaker::You,
        ..assistant(id, channel, at, body)
    }
}

/// A morning brief in Markdown. The coordinator chooses the format, so this
/// one uses bold, links to tasks, strikethrough, a table and a task list.
pub const BRIEF_BODY: &str = "Five things for today. **Two need you.**\n\n1. **[Go through open Sayso issues](task:tsk_0192f0c4-sayso-issues).** Codex needs your OK to build on bigboi.\n2. **Dana, about the venue.** The offer to draft a reply waits.\n3. **[Tighten the spam rules](task:tsk_0192f0c4-spam-rules)** is ready for review. The new rules for miss-lucy-tg ~~block links~~ hold links for review.\n4. **[Price out roofline lighting](task:tsk_0192f0c4-roofline)** is ready for review:\n\n| Quote | Price | Install |\n|---|---:|---|\n| Bright Eaves | $1,850 | Nov 14 |\n| Peak Lights | $2,100 | Nov 9 |\n| HomeGlow | $1,640 | Nov 21 |\n\n5. *Done:* [Move backups to homelab](task:tsk_0192f0c4-nightly-backups). The copy from truenas ended at 4:10 AM.\n\n- [ ] Answer Dana\n- [x] Check the backup";

/// Tutti's timeline in G2C-0: an offer, the morning brief, a check-in and a
/// follow-up.
pub fn tutti_items() -> Vec<TimelineItem> {
    let offer_at = local_at(0, 6, 30);
    let brief_at = local_at(0, 7, 30);
    let check_in_at = local_at(0, 10, 40);
    let follow_up_at = local_at(0, 11, 20);
    let mut offer_mark = mark(ProactiveKind::Research, None);
    offer_mark.offer = true;
    offer_mark.label = "Offer".into();
    offer_mark.icon = "compose".into();
    let mut brief = assistant(BRIEF_MESSAGE, TUTTI, brief_at, BRIEF_BODY);
    brief.proactive = Some(mark(ProactiveKind::Brief, None));
    let mut check_in = assistant(
        CHECK_IN_MESSAGE,
        TUTTI,
        check_in_at,
        "Your dentist moved Friday's visit to 8:30 AM. It now overlaps with your 8:30 stand-up. I changed nothing.",
    );
    check_in.proactive = Some(mark(ProactiveKind::CheckIn, None));
    let mut follow_up = assistant(
        FOLLOW_UP_MESSAGE,
        TUTTI,
        follow_up_at,
        "The vendor has not replied about the venue deposit. Their last mail came on Tuesday at 4:12 PM. I will look again tomorrow morning.",
    );
    follow_up.proactive = Some(mark(
        ProactiveKind::FollowUp,
        Some("You asked me to check the vendor reply today."),
    ));
    vec![
        TimelineItem::Day(DayView {
            key: "day-proactive-today".into(),
            label: "Today".into(),
        }),
        TimelineItem::Decision(DecisionCardView {
            message_id: OFFER_MESSAGE.into(),
            speaker: Speaker::Consort,
            time_label: crate::format::time_label(offer_at),
            created_at: offer_at,
            decision: DecisionView {
                id: "dec_offer_dana".into(),
                task_id: None,
                question: "Want me to draft a reply to Dana?".into(),
                context: Some("They asked about the venue on Tuesday, and I found no reply from you. I will only write a draft. I will not send it.".into()),
                options: ["Do it", "Not now", "Never about this"]
                    .into_iter()
                    .map(|label| DecisionOption {
                        label: label.into(),
                        consequence: String::new(),
                    })
                    .collect(),
                recommended: None,
                allow_free_text: false,
                state: RequestState::Pending,
                chosen: None,
                answer_text: None,
            },
            proactive: Some(offer_mark),
        }),
        TimelineItem::Message(brief),
        TimelineItem::Message(check_in),
        TimelineItem::Message(follow_up),
    ]
}

/// The routines of house-lights in G8V-0: the check-in and two follow-ups.
pub fn routines() -> Vec<RoutineView> {
    let base = RoutineView {
        id: CHECK_IN.into(),
        channel_id: LIGHTS.into(),
        name: "Check-in".into(),
        enabled: true,
        schedule: RoutineSchedule::Interval { seconds: 6 * 3600 },
        prompt: "Look for things that need the user.".into(),
        action: RoutineActionView::Ask,
        next_run_at: Some(local_at(0, 13, 45)),
        last_run_at: Some(local_at(0, 7, 25)),
        last_run_status: Some(RoutineRunStatus::Ok),
        monid: None,
        runs: Vec::new(),
        runs_loaded: false,
        kind: RoutineKind::CheckIn,
        origin: RoutineOrigin::Hub,
        created_at: Some(local_at(-20, 9, 0)),
    };
    let quote = RoutineView {
        id: FOLLOW_UP_QUOTE.into(),
        name: "Follow-up".into(),
        schedule: RoutineSchedule::Once {
            at: consort_proto::time::Timestamp::from_millis(local_at(0, 15, 0)),
        },
        prompt: "Check whether Brightline replied to the quote.".into(),
        next_run_at: Some(local_at(0, 15, 0)),
        last_run_at: None,
        last_run_status: None,
        monid: None,
        kind: RoutineKind::FollowUp,
        origin: RoutineOrigin::Assistant,
        created_at: Some(local_at(0, 9, 10)),
        ..base.clone()
    };
    let switch = RoutineView {
        id: FOLLOW_UP_SWITCH.into(),
        prompt: "See whether the porch switch order shipped.".into(),
        schedule: RoutineSchedule::Once {
            at: consort_proto::time::Timestamp::from_millis(local_at(1, 9, 0)),
        },
        next_run_at: Some(local_at(1, 9, 0)),
        created_at: Some(local_at(-1, 16, 30)),
        ..quote.clone()
    };
    let brief = RoutineView {
        id: BRIEF_ROUTINE.into(),
        channel_id: TUTTI.into(),
        name: "Morning brief".into(),
        schedule: RoutineSchedule::Cron {
            expression: "30 7 * * 1-5".into(),
            time_zone: "America/Chicago".into(),
        },
        prompt: "Write the morning brief.".into(),
        next_run_at: Some(local_at(1, 7, 30)),
        last_run_at: Some(local_at(0, 7, 30)),
        kind: RoutineKind::Brief,
        origin: RoutineOrigin::Hub,
        ..base.clone()
    };
    vec![base, quote, switch, brief]
}

fn level_row(
    id: &str,
    name: &str,
    icon: &str,
    kind: ChannelKind,
    level: ProactivityLevel,
    is_default: bool,
) -> ChannelLevelView {
    ChannelLevelView {
        channel_id: id.into(),
        name: name.into(),
        icon: icon.into(),
        tutti: kind == ChannelKind::Tutti,
        level: (!is_default).then_some(level),
        effective: Some(level),
        is_default,
        meaning: level_meaning(level, kind, if kind == ChannelKind::Tutti { 3 } else { 6 }),
    }
}

// One row of the design's log, field by field.
#[allow(clippy::too_many_arguments)]
fn log_row(
    id: u64,
    at: i64,
    channel: (&str, &str),
    kind: ProactiveKind,
    result: &str,
    tone: ResultTone,
    detail: &str,
    message: Option<&str>,
) -> LogRowView {
    LogRowView {
        id,
        channel_id: channel.0.into(),
        channel_name: channel.1.into(),
        kind: Some(kind),
        kind_label: kind.label().into(),
        time_label: crate::format::time_label(at),
        at,
        result: result.into(),
        tone,
        detail: detail.into(),
        message: message.map(|m| (m.into(), None)),
    }
}

/// Settings > Proactivity in FYE-0.
pub fn proactivity_view() -> ProactivityView {
    let now = crate::format::now_ms();
    let tutti = (TUTTI, "Tutti");
    let lights = (LIGHTS, "house-lights");
    let sayso = (SAYSO, "sayso");
    let reading = (READING, "reading");
    use ProactiveKind as K;
    use ResultTone as T;
    let today = vec![
        log_row(
            14,
            local_at(0, 11, 48),
            sayso,
            K::StaleTask,
            "Skipped: snoozed",
            T::Quiet,
            "Snoozed until 1:45 PM.",
            None,
        ),
        log_row(
            13,
            local_at(0, 11, 20),
            tutti,
            K::FollowUp,
            "Posted",
            T::Posted,
            "The vendor has not replied about the deposit.",
            Some(FOLLOW_UP_MESSAGE),
        ),
        log_row(
            12,
            local_at(0, 11, 12),
            tutti,
            K::StaleTask,
            "Notified",
            T::Notified,
            "Go through open Sayso issues has waited for your OK since yesterday.",
            Some(BRIEF_MESSAGE),
        ),
        log_row(
            11,
            local_at(0, 10, 40),
            tutti,
            K::CheckIn,
            "Posted",
            T::Posted,
            "Your dentist moved Friday's visit to 8:30 AM.",
            Some(CHECK_IN_MESSAGE),
        ),
        log_row(
            10,
            local_at(0, 10, 5),
            lights,
            K::FollowUp,
            "Posted",
            T::Posted,
            "Brightline has not replied to the quote.",
            Some("msg_lights_follow_up"),
        ),
        log_row(
            9,
            local_at(0, 7, 40),
            tutti,
            K::CheckIn,
            "Skipped: nothing changed",
            T::Quiet,
            "No change since the brief.",
            None,
        ),
        log_row(
            8,
            local_at(0, 7, 30),
            tutti,
            K::Brief,
            "Notified",
            T::Notified,
            "Five items. Two need you.",
            Some(BRIEF_MESSAGE),
        ),
        log_row(
            7,
            local_at(0, 7, 25),
            lights,
            K::CheckIn,
            "Stayed silent",
            T::Quiet,
            "The quotes did not change. Brightline is still open.",
            None,
        ),
        log_row(
            6,
            local_at(0, 6, 30),
            tutti,
            K::Research,
            "Posted",
            T::Posted,
            "Dana asked about the venue on Tuesday. One offer to draft a reply.",
            Some(OFFER_MESSAGE),
        ),
    ];
    let yesterday = vec![
        log_row(
            5,
            local_at(-1, 21, 40),
            tutti,
            K::CheckIn,
            "Skipped: quiet hours",
            T::Quiet,
            "Quiet hours until 6:00 AM.",
            None,
        ),
        log_row(
            4,
            local_at(-1, 16, 20),
            lights,
            K::CheckIn,
            "Failed",
            T::Failed,
            "The turn timed out. The next check-in runs as planned.",
            None,
        ),
        log_row(
            3,
            local_at(-1, 14, 10),
            sayso,
            K::StaleTask,
            "Skipped: over the turn limit",
            T::Quiet,
            "3 of 3 proactive turns used in sayso.",
            None,
        ),
        log_row(
            2,
            local_at(-1, 9, 15),
            reading,
            K::ChannelCreated,
            "Posted",
            T::Posted,
            "Greeted you and asked what it is for.",
            Some(GREETING),
        ),
    ];
    let follow_ups: Vec<FollowUpView> = routines()
        .into_iter()
        .filter(|r| r.kind == RoutineKind::FollowUp)
        .map(|r| FollowUpView {
            routine_id: r.id,
            channel_id: r.channel_id,
            reason: r.prompt,
            when: follow_up_when(r.next_run_at.unwrap_or_default(), r.created_at, now),
            at: r.next_run_at.unwrap_or_default(),
        })
        .collect();
    ProactivityView {
        available: true,
        paused: false,
        quiet_start: "21:00".into(),
        quiet_end: "06:00".into(),
        time_zone: "America/Chicago".into(),
        snooze: SnoozeOption::TwoHours,
        snooze_line: format!(
            "Now: snoozed until {}. Follow-ups that the assistant set still run, but they do not notify.",
            crate::format::time_label(local_at(0, 13, 45))
        ),
        follow_ups,
        follow_ups_line: "2 are waiting, both in house-lights. You can cancel one in the Routines tab of its channel.".into(),
        channels: vec![
            level_row(TUTTI, "Tutti", "brace", ChannelKind::Tutti, ProactivityLevel::Eager, false),
            level_row(SAYSO, "sayso", "mic", ChannelKind::Standard, ProactivityLevel::Quiet, true),
            level_row(LUCY, "miss-lucy-tg", "shield", ChannelKind::Standard, ProactivityLevel::Quiet, true),
            level_row(HOMELAB, "homelab", "server", ChannelKind::Standard, ProactivityLevel::Off, false),
            level_row(LIGHTS, "house-lights", "lightbulb", ChannelKind::Standard, ProactivityLevel::Normal, false),
            level_row(READING, "reading", "book-open", ChannelKind::Standard, ProactivityLevel::Quiet, true),
        ],
        turns_today: 9,
        turns_line: "7 ran a model. 2 were skipped before a model ran.".into(),
        notified_today: 2,
        notify_cap: 3,
        notify_line: "After 3, the assistant posts without a notification.".into(),
        brief: Some((
            BRIEF_ROUTINE.into(),
            format!(
                "Weekdays at 7:30 AM in Tutti. Today's brief came at {}. Change the time or the days in the routine.",
                crate::format::time_label(local_at(0, 7, 30))
            ),
        )),
        log: vec![
            LogGroupView {
                label: "Today".into(),
                rows: today,
            },
            LogGroupView {
                label: "Yesterday".into(),
                rows: yesterday,
            },
        ],
        log_load: crate::connectors::LoadState::Loaded,
        log_has_more: false,
        error: None,
    }
}

/// The new channel reading, for F6N-0, FDX-0 and FJC-0.
fn reading_channel(state: &ViewState) -> Option<ChannelView> {
    let mut channel = state.channels.iter().find(|c| c.id == SAYSO)?.clone();
    channel.id = READING.into();
    channel.name = "reading".into();
    channel.purpose = String::new();
    channel.instructions = String::new();
    channel.icon = "book-open".into();
    channel.repos = Vec::new();
    channel.tags = Vec::new();
    channel.preferred_workers = Vec::new();
    Some(channel)
}

/// The timeline of reading at each step of the guided start.
pub fn reading_items(step: &str) -> Vec<TimelineItem> {
    let made = local_at(0, 20, 14);
    let mut greeting = assistant(
        GREETING,
        READING,
        made + 4_000,
        "Hello. This is reading, a new channel. Here we keep one subject together: its tasks, notes, files and memory.",
    );
    greeting.proactive = Some(ProactiveMarkView {
        show_caption: false,
        ..mark(ProactiveKind::ChannelCreated, None)
    });
    let mut setup = assistant(
        SETUP_OFFER,
        READING,
        made + 6_000,
        "Do you want me to help you set it up? I ask a few short questions: what the channel is for, which connectors and repos it uses, and which model I use here.",
    );
    setup.proactive = greeting.proactive.clone();
    let mut items = vec![
        TimelineItem::Day(DayView {
            key: "day-reading-today".into(),
            label: "Today".into(),
        }),
        TimelineItem::Notice(NoticeView {
            message_id: crate::proactivity::made_line_key(READING),
            time_label: crate::format::time_label(made),
            created_at: made,
            kind: "channel_created".into(),
            text: "You made the channel reading.".into(),
            memory: None,
            draft: None,
            fallback: None,
            error: None,
            monid: None,
            visual: None,
        }),
        TimelineItem::Message(greeting),
    ];
    match step {
        "channel-greeting" => {
            setup.suggestions = ["Yes", "Not now", "No, I set it up myself"]
                .into_iter()
                .map(|label| SuggestionView {
                    label: label.into(),
                    type_it: false,
                })
                .collect();
            items.push(TimelineItem::Message(setup));
        }
        "channel-writing" => {
            items.push(TimelineItem::Message(setup));
            items.push(TimelineItem::Message(you(
                "msg_reading_yes",
                READING,
                made + 40_000,
                "Yes",
            )));
        }
        _ => {
            items.push(TimelineItem::Message(setup));
            items.push(TimelineItem::Message(you(
                "msg_reading_yes",
                READING,
                made + 40_000,
                "Yes",
            )));
            items.push(TimelineItem::Message(assistant(
                "msg_reading_first",
                READING,
                made + 50_000,
                "Good. First question, of four.",
            )));
            let mut question = assistant(
                "msg_reading_purpose",
                READING,
                made + 52_000,
                "What is reading for? I write your answer as the channel's purpose, and I read it before every turn here.",
            );
            question.suggestions = vec![
                SuggestionView {
                    label: "Books I want to read".into(),
                    type_it: false,
                },
                SuggestionView {
                    label: "Articles and papers".into(),
                    type_it: false,
                },
                SuggestionView {
                    label: "Work research".into(),
                    type_it: false,
                },
                SuggestionView {
                    label: "I will type it".into(),
                    type_it: true,
                },
            ];
            items.push(TimelineItem::Message(question));
        }
    }
    items
}

/// The view state of a proactivity screen, or `None` for another screen.
pub fn screen_state(screen: &str) -> Option<ViewState> {
    let base = || {
        let mut state =
            crate::mock_routines::with_routines_and_memory(crate::mock::view_state_with_files());
        state.proactivity = proactivity_view();
        state.routines.retain(|r| {
            !(r.channel_id == LIGHTS || r.channel_id == TUTTI) || r.kind == RoutineKind::Custom
        });
        state.routines.extend(routines());
        if let Some(channel) = reading_channel(&state) {
            state.channels.push(channel);
            state.timelines.push(TimelineView {
                channel_id: READING.into(),
                items: reading_items("channel-greeting"),
                has_older: false,
                loaded: true,
            });
        }
        if let Some(tutti) = state.timelines.iter_mut().find(|t| t.channel_id == TUTTI) {
            tutti.items = tutti_items();
        }
        crate::derive::refresh(&mut state);
        state
    };
    match screen {
        "proactivity" | "proactive-messages" | "routines-follow-ups" => Some(base()),
        "channel-greeting" | "channel-writing" | "channel-question" => {
            let mut state = base();
            if let Some(timeline) = state.timelines.iter_mut().find(|t| t.channel_id == READING) {
                timeline.items = reading_items(screen);
            }
            if screen == "channel-writing" {
                state.activity.push(crate::activity::ChannelActivity {
                    channel_id: READING.into(),
                    activity: crate::activity::ActivityView {
                        state: crate::activity::MarkState::Working,
                        line: Some("Writing in reading".into()),
                    },
                    own_state: crate::activity::MarkState::Working,
                });
            }
            Some(state)
        }
        _ => None,
    }
}

fn not_found(message: &str) -> ClientError {
    ClientError::Hub {
        code: "not_found".into(),
        message: message.into(),
    }
}

/// Runs a proactivity command on the mock state.
pub fn apply(state: &mut ViewState, command: Command) -> Result<CommandOutput, ClientError> {
    let view = &mut state.proactivity;
    match command {
        Command::LoadProactivityLog => {
            view.log_load = crate::connectors::LoadState::Loaded;
        }
        Command::SetProactivity(changes) => {
            if let Some(paused) = changes.paused {
                view.paused = paused;
            }
            for (field, value) in [
                (&mut view.quiet_start, changes.quiet_start),
                (&mut view.quiet_end, changes.quiet_end),
            ] {
                if let Some(value) = value {
                    if consort_proto::client_api::parse_time(&value).is_none() {
                        return Err(ClientError::Hub {
                            code: "invalid".into(),
                            message: "quiet_hours.start must be HH:MM from 00:00 to 23:59".into(),
                        });
                    }
                    *field = value;
                }
            }
            if let Some(zone) = changes.time_zone {
                view.time_zone = zone;
            }
        }
        Command::SnoozeProactivity(option) => {
            view.snooze = option;
            view.snooze_line = match option {
                SnoozeOption::Off => "Check-ins, nudges and the research pass wait. Follow-ups and the brief still run, but they do not notify.".into(),
                other => format!(
                    "Now: snoozed ({}). Follow-ups that the assistant set still run, but they do not notify.",
                    other.label().to_lowercase()
                ),
            };
        }
        Command::CancelAllFollowUps { channel_id } => {
            let matches = |c: &str| channel_id.as_deref().is_none_or(|id| id == c);
            view.follow_ups.retain(|f| !matches(&f.channel_id));
            view.follow_ups_line =
                "None are waiting. The assistant sets one when it says it will check again.".into();
            state
                .routines
                .retain(|r| !(r.kind == RoutineKind::FollowUp && matches(&r.channel_id)));
        }
        Command::SetChannelProactivity { channel_id, level } => {
            let row = view
                .channels
                .iter_mut()
                .find(|c| c.channel_id == channel_id)
                .ok_or_else(|| not_found("This channel no longer exists."))?;
            let kind = if row.tutti {
                ChannelKind::Tutti
            } else {
                ChannelKind::Standard
            };
            let effective = level.unwrap_or(if row.tutti {
                ProactivityLevel::Normal
            } else {
                ProactivityLevel::Quiet
            });
            row.level = level;
            row.is_default = level.is_none();
            row.effective = Some(effective);
            row.meaning = level_meaning(effective, kind, if row.tutti { 3 } else { 6 });
        }
        Command::RateMessage { message_id, rating } => {
            for timeline in &mut state.timelines {
                for item in &mut timeline.items {
                    if let TimelineItem::Message(m) = item
                        && m.id == message_id
                        && let Some(mark) = m.proactive.as_mut()
                    {
                        mark.rating = rating;
                    }
                }
            }
        }
        other => {
            return Err(ClientError::Other(format!(
                "{other:?} is not a proactivity command"
            )));
        }
    }
    Ok(CommandOutput::Done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_screens_hold_the_designs_content() {
        let state = screen_state("proactivity").unwrap();
        let view = &state.proactivity;
        assert!(view.available);
        assert_eq!(view.channels.len(), 6);
        assert_eq!(
            view.channels[0].meaning,
            "Normal, plus a daily research pass and offers."
        );
        assert_eq!(view.channels[3].meaning, "The assistant only answers here.");
        assert_eq!(view.follow_ups.len(), 2);
        assert_eq!(view.log[0].rows.len(), 9);
        assert_eq!(view.log[1].label, "Yesterday");
        let tutti = state.timeline(TUTTI).unwrap();
        assert!(
            matches!(&tutti.items[1], TimelineItem::Decision(d) if d.proactive.as_ref().is_some_and(|m| m.offer))
        );
        let question = screen_state("channel-question").unwrap();
        let reading = question.timeline(READING).unwrap();
        let TimelineItem::Message(last) = reading.items.last().unwrap() else {
            panic!("a message");
        };
        assert_eq!(last.suggestions.len(), 4);
        assert!(last.suggestions[3].type_it);
        let writing = screen_state("channel-writing").unwrap();
        assert!(crate::proactivity::is_writing(&writing, READING));
        assert!(!crate::proactivity::is_writing(&question, READING));
        assert!(screen_state("tutti").is_none());
    }

    #[test]
    fn the_mock_commands_change_the_page() {
        let mut state = screen_state("proactivity").unwrap();
        apply(
            &mut state,
            Command::SetChannelProactivity {
                channel_id: HOMELAB.into(),
                level: Some(ProactivityLevel::Normal),
            },
        )
        .unwrap();
        assert_eq!(
            state.proactivity.channels[3].meaning,
            "Quiet, plus a check-in every 6 hours."
        );
        apply(&mut state, Command::CancelAllFollowUps { channel_id: None }).unwrap();
        assert!(state.proactivity.follow_ups.is_empty());
        assert!(
            state
                .routines
                .iter()
                .all(|r| r.kind != RoutineKind::FollowUp)
        );
        apply(&mut state, Command::SnoozeProactivity(SnoozeOption::Off)).unwrap();
        assert_eq!(state.proactivity.snooze, SnoozeOption::Off);
        let bad = apply(
            &mut state,
            Command::SetProactivity(crate::proactivity::ProactivityChanges {
                quiet_start: Some("25:00".into()),
                ..Default::default()
            }),
        );
        assert!(bad.is_err());
    }
}
