//! The hub tools with which the assistant works on its own:
//! `follow_up_later`, `cancel_follow_up` and `set_proactivity`, and the
//! rules that `post_message`, `reply`, `ask_decision`, `start_task` and
//! `no_reply_needed` follow in a proactive turn.

use consort_proto::ids::{ChannelId, RoutineId, SessionId};
use consort_proto::model::{
    ApprovalMode, Channel, DecisionOption, FINDING_MAX_CHARS, FOLLOW_UP_MAX_DAYS,
    FOLLOW_UP_MIN_MINUTES, FOLLOW_UP_REASON_MAX_CHARS, FOLLOW_UPS_MAX_PER_CHANNEL, OFFER_DO_IT,
    OFFER_NEVER, OFFER_NOT_NOW, ProactiveKind, ProactiveMark, ProactiveTasks, ProactivityLevel,
    RESEARCH_MEMORY_FOLDER, RESEARCH_OFFERS_MAX_PER_DAY, Routine, RoutineAction, RoutineKind,
    RoutineOrigin, RoutineSchedule, SnoozeChoice, Suggestion, validate_suggestions,
};
use consort_proto::time::Timestamp;
use consort_proto::tools::{
    CancelFollowUpArgs, FollowUpLaterArgs, FollowUpResult, OkResult, SetProactivityArgs,
    SetProactivityResult, ToolAudience,
};
use serde_json::Value;

use super::gate;
use super::turn::{
    self, RESEARCH_REFUSAL, TurnKind, denied, notifying_messages_since, turn_notified,
};
use crate::error::{HubError, Result};
use crate::hub::{Hub, ToolCtx};
use crate::store::{Tx, conversation, proactivity as store, routines};

const MINUTE_MS: i64 = 60 * 1000;
const DAY_MS: i64 = 24 * 60 * MINUTE_MS;

/// The proactive mark and the suggestions of one message that a
/// coordinator posts. Call it inside the transaction that posts it.
pub(crate) struct Posting {
    pub mark: Option<ProactiveMark>,
    pub suggestions: Vec<Suggestion>,
}

/// Decides the mark of a message that a coordinator session posts now: the
/// kind of its turn, and whether it may notify. A notification needs a
/// proactive turn, the time outside quiet hours and snooze, no other
/// notification in this turn, and room under the daily limit. When the hub
/// holds a notification back, the turn's log entries say so.
pub(crate) fn posting(
    tx: &mut Tx<'_>,
    session_id: SessionId,
    channel_id: ChannelId,
    notify: bool,
    suggestions: Vec<Suggestion>,
) -> Result<Posting> {
    validate_suggestions(&suggestions)?;
    let (kind, row) = turn::turn_of_session(tx.db(), session_id, channel_id)?;
    if kind == TurnKind::Research {
        return Err(denied(RESEARCH_REFUSAL));
    }
    let mut mark = turn::mark_for(&kind);
    if let Some(mark) = mark.as_mut()
        && notify
    {
        let now = tx.now();
        let settings = store::settings(tx.db())?;
        let since = row.turn_started_at.unwrap_or(now);
        let allowed = gate::may_notify(&settings, now)
            && !turn_notified(tx.db(), channel_id, session_id, since)?
            && notifying_messages_since(tx.db(), gate::start_of_day(&settings, now))?
                < settings.notify_cap_per_day;
        if allowed {
            mark.notify = true;
        } else {
            tx.update_turn_wakes(&row.turn_events, |wake| wake.notify_held = true)?;
        }
    }
    Ok(Posting { mark, suggestions })
}

/// The three options of an offer.
pub(crate) fn offer_options() -> Vec<DecisionOption> {
    vec![
        DecisionOption {
            label: OFFER_DO_IT.to_owned(),
            consequence: "The assistant does the work it offered, in draft mode.".to_owned(),
            id: None,
        },
        DecisionOption {
            label: OFFER_NOT_NOW.to_owned(),
            consequence: "Nothing happens now. The assistant may ask again later.".to_owned(),
            id: None,
        },
        DecisionOption {
            label: OFFER_NEVER.to_owned(),
            consequence:
                "The assistant adds a skip line to the check-in note and does not offer this again."
                    .to_owned(),
            id: None,
        },
    ]
}

/// The rules of `ask_decision` for a coordinator: in a research turn only
/// offers, at most 3 a day; an offer only at the level Eager. Returns the
/// mark of the card.
pub(crate) fn decision_posting(
    tx: &mut Tx<'_>,
    session_id: SessionId,
    channel: &Channel,
    offer: bool,
) -> Result<Option<ProactiveMark>> {
    let (kind, _) = turn::turn_of_session(tx.db(), session_id, channel.id)?;
    if kind == TurnKind::Research && !offer {
        return Err(denied(RESEARCH_REFUSAL));
    }
    if !offer {
        return Ok(turn::mark_for(&kind));
    }
    // An offer about the style guide is the hub's (`propose_voice_rule`),
    // so every offer here follows the normal gate.
    if !kind.is_proactive() {
        return Err(HubError::invalid(
            "An offer is for a turn that you started on your own. Ask a normal decision instead.",
        ));
    }
    if channel.proactivity_level() < ProactivityLevel::Eager {
        return Err(denied(format!(
            "Offers need the level Eager in {}. Ask a normal decision, or tell the user.",
            channel.name
        )));
    }
    if kind == TurnKind::Research {
        let settings = store::settings(tx.db())?;
        let now = tx.now();
        let made = store::research_offers_since(tx.db(), gate::start_of_day(&settings, now))?;
        if made >= RESEARCH_OFFERS_MAX_PER_DAY {
            return Err(denied(format!(
                "The research pass made {RESEARCH_OFFERS_MAX_PER_DAY} offers today. Keep the rest in memory under research/ and call no_reply_needed."
            )));
        }
    }
    let mut mark = turn::mark_for(&kind);
    if let Some(mark) = mark.as_mut() {
        mark.offer = true;
    }
    Ok(mark)
}

/// The kind of the proactive turn that starts a task, and whether the task
/// runs in draft mode.
pub(crate) type TaskOrigin = (ProactiveKind, bool);

/// How a task that a coordinator starts now runs: its approval mode, and
/// its proactive origin with the draft flag. A task that a proactive turn
/// starts runs in draft mode (Ask), unless the channel lets such tasks use
/// its default. A task that a normal turn starts after the user chose "Do
/// it" on an offer runs in draft mode too, as the offer says.
pub(crate) fn task_approval_mode(
    conn: &rusqlite::Connection,
    session_id: SessionId,
    channel: &Channel,
    requested: Option<ApprovalMode>,
) -> Result<(Option<ApprovalMode>, Option<TaskOrigin>)> {
    let (kind, row) = turn::turn_of_session(conn, session_id, channel.id)?;
    let origin = match kind.proactive_kind() {
        Some(kind) => Some(kind),
        None => offer_taken(conn, &row.turn_events)?,
    };
    let Some(origin) = origin else {
        return Ok((requested, None));
    };
    let draft = channel.proactive_tasks == ProactiveTasks::Drafts;
    let mode = if draft {
        Some(ApprovalMode::Ask)
    } else {
        requested
    };
    Ok((mode, Some((origin, draft))))
}

/// The kind of the offer that the user answered with "Do it" in these
/// events, when there is one.
fn offer_taken(
    conn: &rusqlite::Connection,
    events: &[consort_proto::turn_input::WakeEvent],
) -> Result<Option<ProactiveKind>> {
    for event in events {
        let consort_proto::turn_input::WakeEvent::DecisionAnswered {
            id,
            option: Some(option),
            ..
        } = event
        else {
            continue;
        };
        if option != OFFER_DO_IT {
            continue;
        }
        let Some(decision) = crate::store::work::decision(conn, *id)? else {
            continue;
        };
        if let Some(mark) = conversation::message(conn, decision.message_id)?
            .and_then(|m| m.proactive)
            .filter(|mark| mark.offer)
        {
            return Ok(Some(mark.kind));
        }
    }
    Ok(None)
}

/// The time of a new follow-up from `after_minutes` or `at`, checked: at
/// least 15 minutes and at most 30 days ahead of `now`. The math is
/// checked, so a value far out of range is an error and never a panic.
pub(crate) fn follow_up_time(
    now: Timestamp,
    after_minutes: Option<u64>,
    at: Option<Timestamp>,
) -> Result<Timestamp> {
    let too_late = || {
        HubError::invalid(format!(
            "A follow-up can be at most {FOLLOW_UP_MAX_DAYS} days ahead. Choose an earlier time."
        ))
    };
    let at = match (after_minutes, at) {
        (Some(minutes), None) => i64::try_from(minutes)
            .ok()
            .and_then(|m| m.checked_mul(MINUTE_MS))
            .and_then(|ms| now.as_millis().checked_add(ms))
            .map(Timestamp::from_millis)
            .ok_or_else(too_late)?,
        (None, Some(at)) => at,
        _ => {
            return Err(HubError::invalid(
                "Give after_minutes or at, not both and not neither.",
            ));
        }
    };
    let min = i64::try_from(FOLLOW_UP_MIN_MINUTES).unwrap_or(15) * MINUTE_MS;
    let max = i64::try_from(FOLLOW_UP_MAX_DAYS).unwrap_or(30) * DAY_MS;
    let Some(ahead) = at.as_millis().checked_sub(now.as_millis()) else {
        // Only a time far outside the range of real times gets here.
        return Err(HubError::invalid(format!(
            "The time {} is out of range. Give a time from {FOLLOW_UP_MIN_MINUTES} minutes to {FOLLOW_UP_MAX_DAYS} days ahead.",
            at.as_millis()
        )));
    };
    if ahead < min {
        return Err(HubError::invalid(format!(
            "A follow-up must be at least {FOLLOW_UP_MIN_MINUTES} minutes ahead. Choose a later time."
        )));
    }
    if ahead > max {
        return Err(too_late());
    }
    Ok(at)
}

/// The research pass may write memory only under `research/`. Returns
/// whether the write is a research write, which posts no line.
pub(crate) fn check_memory_write(
    conn: &rusqlite::Connection,
    session_id: SessionId,
    channel_id: ChannelId,
    path: &str,
) -> Result<bool> {
    let (kind, _) = turn::turn_of_session(conn, session_id, channel_id)?;
    if kind != TurnKind::Research {
        return Ok(false);
    }
    if !path.starts_with(RESEARCH_MEMORY_FOLDER) {
        return Err(denied(RESEARCH_REFUSAL));
    }
    Ok(true)
}

impl Hub {
    /// Refuses a tool that the kind of the session's turn does not allow.
    pub(crate) async fn check_turn_tool(
        &self,
        ctx: &ToolCtx,
        tool: consort_proto::tools::HubTool,
    ) -> Result<()> {
        if ctx.audience == ToolAudience::TaskSession {
            return Ok(());
        }
        let (session_id, channel_id) = (ctx.session.id, ctx.channel.id);
        let kind = self
            .store
            .read(move |conn| Ok(turn::turn_of_session(conn, session_id, channel_id)?.0))
            .await?;
        turn::check_tool(&kind, tool)
    }

    /// The kind of the open turn of a coordinator session. A task session
    /// runs no coordinator turn, so its kind is normal.
    pub(crate) async fn turn_kind(&self, ctx: &ToolCtx) -> Result<TurnKind> {
        if ctx.audience == ToolAudience::TaskSession {
            return Ok(TurnKind::Normal);
        }
        let (session_id, channel_id) = (ctx.session.id, ctx.channel.id);
        self.store
            .read(move |conn| Ok(turn::turn_of_session(conn, session_id, channel_id)?.0))
            .await
    }

    /// `message_task` and `retry_task` in a proactive turn reach only a task
    /// that a proactive turn of this channel started in draft mode. Other
    /// tasks may act outward with their own mode, so the user must steer
    /// them.
    pub(crate) async fn check_steer_task(
        &self,
        ctx: &ToolCtx,
        task_id: consort_proto::ids::TaskId,
    ) -> Result<()> {
        if !self.turn_kind(ctx).await?.is_proactive() {
            return Ok(());
        }
        let channel_id = ctx.channel.id;
        let origin = self
            .store
            .read(move |conn| store::proactive_task(conn, task_id))
            .await?;
        match origin {
            Some(origin) if origin.draft && origin.channel_id == channel_id => Ok(()),
            _ => Err(denied(turn::STEER_TASK_REFUSAL)),
        }
    }

    /// Whether a connector call must ask the user each time: in a proactive
    /// turn, and in a task that runs in draft mode. Earlier allows do not
    /// count there.
    pub(crate) async fn connector_calls_ask_each_time(&self, ctx: &ToolCtx) -> Result<bool> {
        if let Some(task) = &ctx.task {
            let task_id = task.id;
            let origin = self
                .store
                .read(move |conn| store::proactive_task(conn, task_id))
                .await?;
            return Ok(origin.is_some_and(|o| o.draft));
        }
        Ok(self.turn_kind(ctx).await?.is_proactive())
    }

    /// Whether the session runs the research pass now.
    pub(crate) async fn in_research_turn(&self, ctx: &ToolCtx) -> Result<bool> {
        if ctx.audience == ToolAudience::TaskSession {
            return Ok(false);
        }
        let (session_id, channel_id) = (ctx.session.id, ctx.channel.id);
        self.store
            .read(move |conn| {
                Ok(turn::turn_of_session(conn, session_id, channel_id)?.0 == TurnKind::Research)
            })
            .await
    }

    /// `no_reply_needed` with a finding: in a proactive turn, the finding goes
    /// into the log entries of the turn.
    pub(crate) async fn note_finding(&self, ctx: &ToolCtx, finding: Option<String>) -> Result<()> {
        let Some(finding) = finding
            .map(|f| f.trim().to_owned())
            .filter(|f| !f.is_empty())
        else {
            return Ok(());
        };
        let finding = crate::util::truncate_chars(&finding, FINDING_MAX_CHARS);
        let (session_id, channel_id) = (ctx.session.id, ctx.channel.id);
        self.store
            .write(move |tx| {
                let (kind, row) = turn::turn_of_session(tx.db(), session_id, channel_id)?;
                if kind.is_proactive() {
                    tx.update_turn_wakes(&row.turn_events, |wake| {
                        wake.detail = Some(finding.clone());
                    })?;
                }
                Ok(())
            })
            .await
    }

    /// Records the task that a proactive turn started in its log entries.
    pub(crate) async fn note_turn_task(
        &self,
        ctx: &ToolCtx,
        task_id: consort_proto::ids::TaskId,
    ) -> Result<()> {
        let (session_id, channel_id) = (ctx.session.id, ctx.channel.id);
        self.store
            .write(move |tx| {
                let (kind, row) = turn::turn_of_session(tx.db(), session_id, channel_id)?;
                if kind.is_proactive() {
                    tx.update_turn_wakes(&row.turn_events, |wake| {
                        wake.task_id.get_or_insert(task_id);
                    })?;
                }
                Ok(())
            })
            .await
    }

    pub(crate) async fn tool_follow_up_later(
        &self,
        ctx: &ToolCtx,
        args: FollowUpLaterArgs,
    ) -> Result<Value> {
        let reason = args.reason.trim().to_owned();
        if reason.is_empty() {
            return Err(HubError::invalid(
                "The reason is empty. Say in one line why you will wake.",
            ));
        }
        if reason.chars().count() > FOLLOW_UP_REASON_MAX_CHARS {
            return Err(HubError::invalid(format!(
                "The reason is longer than {FOLLOW_UP_REASON_MAX_CHARS} characters. Use one short line."
            )));
        }
        let key = args
            .key
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty());
        if key.as_ref().is_some_and(|k| k.chars().count() > 80) {
            return Err(HubError::invalid(
                "The key is longer than 80 characters. Use a short key.",
            ));
        }
        let (channel_id, session_id) = (ctx.channel.id, ctx.session.id);
        let result = self
            .store
            .write(move |tx| {
                let now = tx.now();
                let at = follow_up_time(now, args.after_minutes, args.at)?;
                let channel = conversation::channel_or_404(tx.db(), channel_id)?;
                if channel.proactivity_level() == ProactivityLevel::Off {
                    return Err(denied(format!(
                        "The proactivity level of {} is Off, so you cannot wake on your own here. Tell the user what to check instead.",
                        channel.name
                    )));
                }
                let replaced = match &key {
                    Some(key) => store::follow_up_by_key(tx.db(), channel_id, key)?,
                    None => None,
                };
                if let Some(old) = &replaced {
                    if let Some(routine) = routines::routine(tx.db(), old.routine_id)? {
                        tx.delete_routine(&routine)?;
                    }
                    tx.delete_follow_up_row(old.routine_id)?;
                }
                let waiting = routines::routines_in_channel(tx.db(), channel_id)?
                    .iter()
                    .filter(|r| r.kind == RoutineKind::FollowUp && r.enabled)
                    .count();
                if waiting >= FOLLOW_UPS_MAX_PER_CHANNEL {
                    return Err(HubError::conflict(format!(
                        "{FOLLOW_UPS_MAX_PER_CHANNEL} follow-ups wait in this channel already. Cancel one with cancel_follow_up first."
                    )));
                }
                let routine = Routine {
                    id: RoutineId::new(),
                    channel_id,
                    name: "Follow-up".to_owned(),
                    schedule: RoutineSchedule::Once { at },
                    prompt: reason,
                    action: RoutineAction::CoordinatorTurn,
                    enabled: true,
                    next_run_at: Some(at),
                    last_run_at: None,
                    last_run_status: None,
                    kind: RoutineKind::FollowUp,
                    origin: RoutineOrigin::Assistant,
                    created_at: Some(now),
                };
                // A follow-up that a proactive turn sets counts against the
                // daily turn budget, so the assistant cannot chain them.
                let set_in_proactive_turn = turn::turn_of_session(tx.db(), session_id, channel_id)?
                    .0
                    .is_proactive();
                tx.save_routine(&routine)?;
                tx.save_follow_up(&store::FollowUpRow {
                    routine_id: routine.id,
                    channel_id,
                    key,
                    set_at: now,
                    set_in_proactive_turn,
                })?;
                Ok(FollowUpResult {
                    routine_id: routine.id,
                    at,
                    replaced: replaced.map(|r| r.routine_id),
                })
            })
            .await?;
        Ok(serde_json::to_value(result)?)
    }

    pub(crate) async fn tool_cancel_follow_up(
        &self,
        ctx: &ToolCtx,
        args: CancelFollowUpArgs,
    ) -> Result<Value> {
        let channel_id = ctx.channel.id;
        self.store
            .write(move |tx| {
                let id = match (args.routine_id, args.key.as_deref()) {
                    (Some(id), _) => id,
                    (None, Some(key)) => store::follow_up_by_key(tx.db(), channel_id, key.trim())?
                        .map(|row| row.routine_id)
                        .ok_or_else(|| {
                            HubError::not_found(format!(
                                "No follow-up with the key \"{key}\" waits in this channel."
                            ))
                        })?,
                    (None, None) => {
                        return Err(HubError::invalid("Give routine_id or key."));
                    }
                };
                let routine = routines::routine(tx.db(), id)?
                    .filter(|r| r.channel_id == channel_id && r.kind == RoutineKind::FollowUp)
                    .ok_or_else(|| {
                        HubError::not_found(format!(
                            "The follow-up {id} does not wait in this channel."
                        ))
                    })?;
                tx.delete_routine(&routine)?;
                tx.delete_follow_up_row(routine.id)
            })
            .await?;
        Ok(serde_json::to_value(OkResult::OK)?)
    }

    pub(crate) async fn tool_set_proactivity(
        &self,
        ctx: &ToolCtx,
        args: SetProactivityArgs,
    ) -> Result<Value> {
        if args.level.is_none() && args.snooze.is_none() {
            return Err(HubError::invalid("Give level, snooze or both."));
        }
        let target = match args.channel_id {
            Some(id) if id != ctx.channel.id => {
                if ctx.audience != ToolAudience::TuttiCoordinator {
                    return Err(denied(
                        "You can change only this channel. Tutti can change other channels.",
                    ));
                }
                id
            }
            _ => ctx.channel.id,
        };
        let result = self
            .store
            .write(move |tx| {
                let now = tx.now();
                let mut channel = conversation::channel_or_404(tx.db(), target)?;
                if let Some(level) = args.level {
                    let current = channel.proactivity_level();
                    if level > current {
                        return Err(denied(format!(
                            "You cannot raise the level of {} from {} to {}. Only the user can, in Settings > Proactivity or in the channel settings.",
                            channel.name,
                            current.label(),
                            level.label()
                        )));
                    }
                    if level != current {
                        channel.proactivity = Some(level);
                        channel.updated_at = now;
                        tx.save_channel(&channel)?;
                        super::routines::sync_channel_routines(tx, &channel)?;
                    }
                }
                let mut settings = store::settings(tx.db())?;
                if let Some(choice) = args.snooze {
                    // The tool only makes things quieter: it starts or
                    // extends a snooze, never ends or shortens one. It has no
                    // way to end Pause all.
                    if choice == SnoozeChoice::Off {
                        return Err(denied(
                            "You cannot end a snooze. Only the user can, in Settings > Proactivity.",
                        ));
                    }
                    let current = settings.snoozed_until.filter(|until| *until > now);
                    let wanted = gate::snooze_until(&settings, choice, now);
                    let until = match (current, wanted) {
                        (Some(current), Some(wanted)) => Some(current.max(wanted)),
                        (current, wanted) => wanted.or(current),
                    };
                    if until != settings.snoozed_until {
                        settings.snoozed_until = until;
                        tx.save_proactivity_settings(&settings)?;
                    }
                }
                Ok(SetProactivityResult {
                    level: channel.proactivity_level(),
                    snoozed_until: settings.snoozed_until.filter(|until| *until > now),
                })
            })
            .await?;
        Ok(serde_json::to_value(result)?)
    }
}
