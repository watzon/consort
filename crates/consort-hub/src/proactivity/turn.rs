//! Proactive turns: which turn is one, what its tools may do, how its
//! messages are marked, and what the hub records when it ends.

use consort_proto::ids::{ChannelId, MessageId, SessionId};
use consort_proto::model::{
    CHECK_IN_BACKOFF_MAX_SECONDS, Channel, Message, ProactiveKind, ProactiveMark, ProactiveOutcome,
    RoutineKind, RoutineSchedule, SILENT_CHECK_INS_BEFORE_BACKOFF,
};
use consort_proto::time::Timestamp;
use consort_proto::tools::HubTool;
use consort_proto::turn_input::WakeEvent;
use consort_proto::worker_link::TurnEndReason;
use rusqlite::{Connection, params};

use crate::error::{HubError, Result};
use crate::store::coordinator::{self as coord_store, CoordinatorRow};
use crate::store::{Tx, list_docs, proactivity as store, routines};

/// The error of a tool that the research pass may not use.
pub(crate) const RESEARCH_REFUSAL: &str =
    "The research pass is read-only. Make an offer with ask_decision instead.";

/// The error of a tool that a proactive turn may not use.
pub(crate) const PROACTIVE_REFUSAL: &str = "A turn that you started on your own cannot widen access, add a connector, turn on scripts or change settings. Ask the user with ask_decision, or wait until the user asks.";

/// A `policy_denied` error with this text.
pub(crate) fn denied(text: impl Into<String>) -> HubError {
    HubError::new(consort_proto::error::ErrorCode::PolicyDenied, text)
}

/// What kind of turn a coordinator runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnKind {
    /// A turn with at least one event that the user or the work caused.
    Normal,
    /// A turn whose events are all proactive. The kind is the first event's.
    Proactive {
        kind: ProactiveKind,
        /// The reason of the first follow-up in the turn.
        reason: Option<String>,
    },
    /// A turn whose only events are research passes.
    Research,
}

impl TurnKind {
    /// Whether this turn started on its own.
    pub(crate) fn is_proactive(&self) -> bool {
        !matches!(self, Self::Normal)
    }

    /// The proactive kind, for the mark of a message.
    pub(crate) fn proactive_kind(&self) -> Option<ProactiveKind> {
        match self {
            Self::Normal => None,
            Self::Proactive { kind, .. } => Some(*kind),
            Self::Research => Some(ProactiveKind::Research),
        }
    }
}

/// The kind of a turn with these events.
pub(crate) fn classify(events: &[WakeEvent]) -> TurnKind {
    if events.is_empty() || events.iter().any(|e| e.proactive_kind().is_none()) {
        return TurnKind::Normal;
    }
    if events
        .iter()
        .all(|e| e.proactive_kind() == Some(ProactiveKind::Research))
    {
        return TurnKind::Research;
    }
    let reason = events.iter().find_map(|e| match e {
        WakeEvent::FollowUp { reason, .. } => Some(reason.clone()),
        _ => None,
    });
    TurnKind::Proactive {
        kind: events
            .iter()
            .find_map(WakeEvent::proactive_kind)
            .unwrap_or(ProactiveKind::CheckIn),
        reason,
    }
}

/// The kind of the open turn of a coordinator session. A session without an
/// open turn, and a task session, runs no proactive turn.
pub(crate) fn turn_of_session(
    conn: &Connection,
    session_id: SessionId,
    channel_id: ChannelId,
) -> Result<(TurnKind, CoordinatorRow)> {
    let row = coord_store::coordinator(conn, channel_id)?;
    if row.session_id != Some(session_id) || !row.turn_open {
        return Ok((TurnKind::Normal, row));
    }
    Ok((classify(&row.turn_events), row))
}

/// The tools that a proactive turn may not use: they widen access, add a
/// connector, turn on scripts, or change settings.
fn widens_access(tool: HubTool) -> bool {
    use HubTool as T;
    matches!(
        tool,
        T::ConnectService
            | T::AddSkill
            | T::RequestAccess
            | T::SetChannelTags
            | T::SetChannelConnector
            | T::SetChannelSkill
            | T::SetWorkerTags
            | T::SetAccountExtras
            | T::UpdateConnection
            | T::UpdateSkill
            | T::Disconnect
            | T::RemoveSkill
            | T::InstallTool
            | T::AddAccount
            | T::StartSignIn
            | T::CreateChannel
            | T::UpdateChannel
            | T::ArchiveChannel
            | T::SetDefaultModel
            | T::UpdateThisChannel
            | T::RoutineCreate
            | T::RoutineUpdate
            | T::RoutineDelete
    )
}

/// The tools that the research pass may use. Some have one more check where
/// they run: `memory_write` only under `research/`, `ask_decision` only as
/// an offer, and `run_connector_tool` only for read tools.
fn research_allows(tool: HubTool) -> bool {
    use HubTool as T;
    matches!(
        tool,
        T::MemoryList
            | T::MemoryRead
            | T::FilesList
            | T::ListWorkers
            | T::Search
            | T::ListConnectorTools
            | T::GetConnectorTool
            | T::RunConnectorTool
            | T::ListSkills
            | T::ReadSkill
            | T::FetchTimeline
            | T::FetchThread
            | T::ListTasks
            | T::ReadTask
            | T::RoutineList
            | T::ListConnections
            | T::ListInstalledSkills
            | T::ListChannels
            | T::ReadChannel
            | T::SearchConnectorCatalog
            | T::SearchSkills
            | T::MemoryWrite
            | T::AskDecision
            | T::NoReplyNeeded
    )
}

/// Checks a tool call against the kind of the turn. Returns the refusal.
pub(crate) fn check_tool(turn: &TurnKind, tool: HubTool) -> Result<()> {
    match turn {
        TurnKind::Normal => Ok(()),
        TurnKind::Research if !research_allows(tool) => Err(denied(RESEARCH_REFUSAL)),
        TurnKind::Research => Ok(()),
        TurnKind::Proactive { .. } if widens_access(tool) => Err(denied(PROACTIVE_REFUSAL)),
        TurnKind::Proactive { .. } => Ok(()),
    }
}

/// The mark of a message that a turn of this kind posts, or `None` for a
/// normal turn.
pub(crate) fn mark_for(turn: &TurnKind) -> Option<ProactiveMark> {
    let kind = turn.proactive_kind()?;
    let reason = match turn {
        TurnKind::Proactive { reason, .. } => reason.clone(),
        _ => None,
    };
    Some(ProactiveMark {
        reason,
        ..ProactiveMark::new(kind)
    })
}

/// The messages that a session posted in a channel since a time: timeline
/// messages, thread replies and decision cards.
pub(crate) fn turn_messages(
    conn: &Connection,
    channel_id: ChannelId,
    session_id: SessionId,
    since: Timestamp,
) -> Result<Vec<Message>> {
    list_docs(
        conn,
        "SELECT data FROM messages
         WHERE channel_id = ?1 AND created_at >= ?2 AND deleted_at IS NULL
           AND json_extract(data, '$.author_session_id') = ?3
           AND json_extract(data, '$.kind') IN ('text', 'decision')
         ORDER BY created_at, rowid",
        params![
            channel_id.to_string(),
            since.as_millis(),
            session_id.to_string()
        ],
    )
}

/// Whether a turn that started at `since` already posted a notifying
/// message. A proactive turn notifies at most once.
pub(crate) fn turn_notified(
    conn: &Connection,
    channel_id: ChannelId,
    session_id: SessionId,
    since: Timestamp,
) -> Result<bool> {
    Ok(turn_messages(conn, channel_id, session_id, since)?
        .iter()
        .any(|m| m.proactive.as_ref().is_some_and(|p| p.notify)))
}

/// The notifying proactive messages since a time, across the hub.
pub(crate) fn notifying_messages_since(conn: &Connection, since: Timestamp) -> Result<u32> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM messages
         WHERE created_at >= ?1 AND json_extract(data, '$.proactive.notify') = 1",
        [since.as_millis()],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

impl Tx<'_> {
    /// Changes the log entries of the proactive events of a turn.
    pub(crate) fn update_turn_wakes(
        &mut self,
        events: &[WakeEvent],
        change: impl Fn(&mut consort_proto::model::ProactiveWake),
    ) -> Result<()> {
        for id in events.iter().filter_map(WakeEvent::log_id) {
            if let Some(mut wake) = store::wake(self.db(), id)? {
                change(&mut wake);
                self.save_wake(&wake)?;
            }
        }
        Ok(())
    }

    /// Records what a coordinator turn did in the log entries of its
    /// proactive events, counts silent check-ins for back-off, and deletes
    /// the follow-ups that ran. Call it when the turn ends, after the routine
    /// runs ended.
    pub(crate) fn finish_proactive_turn(
        &mut self,
        channel: &Channel,
        session_id: SessionId,
        row: &CoordinatorRow,
        reason: TurnEndReason,
        error: Option<&str>,
    ) -> Result<()> {
        if !row.turn_events.iter().any(|e| e.proactive_kind().is_some()) {
            return Ok(());
        }
        let since = row.turn_started_at.unwrap_or(Timestamp::EPOCH);
        let messages = turn_messages(self.db(), channel.id, session_id, since)?;
        let outcome = match reason {
            TurnEndReason::Completed => {
                if messages
                    .iter()
                    .any(|m| m.proactive.as_ref().is_some_and(|p| p.notify))
                {
                    ProactiveOutcome::Notified
                } else if messages.is_empty() {
                    ProactiveOutcome::Silent
                } else {
                    ProactiveOutcome::Posted
                }
            }
            TurnEndReason::Interrupted | TurnEndReason::Failed => ProactiveOutcome::Failed,
        };
        let ids: Vec<MessageId> = messages.iter().map(|m| m.id).collect();
        let now = self.now();
        let failure = error.map(str::to_owned).or_else(|| {
            (outcome == ProactiveOutcome::Failed)
                .then(|| "The turn stopped before it finished.".to_owned())
        });
        self.update_turn_wakes(&row.turn_events, |wake| {
            if wake.outcome != ProactiveOutcome::Pending {
                return;
            }
            wake.outcome = outcome;
            wake.message_ids.clone_from(&ids);
            wake.ended_at = Some(now);
            if let Some(text) = &failure {
                wake.detail = Some(crate::util::truncate_chars(text, 300));
            } else if outcome != ProactiveOutcome::Silent && wake.detail.is_none() {
                wake.detail = messages
                    .first()
                    .map(|m| crate::util::truncate_chars(m.body.lines().next().unwrap_or(""), 160));
            }
        })?;

        if row
            .turn_events
            .iter()
            .any(|e| matches!(e, WakeEvent::CheckIn { .. }))
        {
            self.count_check_in(channel.id, outcome)?;
        }
        for event in &row.turn_events {
            if let WakeEvent::FollowUp { routine, .. } = event
                && let Some(routine) = routines::routine(self.db(), *routine)?
            {
                self.delete_routine(&routine)?;
                self.delete_follow_up_row(routine.id)?;
            }
        }
        Ok(())
    }

    /// Counts a check-in for back-off. After six silent check-ins in a row
    /// the interval doubles, up to 12 hours. A check-in that posted resets
    /// the count.
    fn count_check_in(&mut self, channel_id: ChannelId, outcome: ProactiveOutcome) -> Result<()> {
        let mut state = store::channel_state(self.db(), channel_id)?;
        match outcome {
            ProactiveOutcome::Silent => state.silent_streak += 1,
            ProactiveOutcome::Posted | ProactiveOutcome::Notified => state.silent_streak = 0,
            _ => return Ok(()),
        }
        if state.silent_streak >= SILENT_CHECK_INS_BEFORE_BACKOFF {
            state.silent_streak = 0;
            if let Some(mut routine) = check_in_routine(self.db(), channel_id)?
                && let RoutineSchedule::Interval { seconds } = routine.schedule
                && seconds < CHECK_IN_BACKOFF_MAX_SECONDS
            {
                let longer = seconds.saturating_mul(2).min(CHECK_IN_BACKOFF_MAX_SECONDS);
                state.base_seconds.get_or_insert(seconds);
                routine.schedule = RoutineSchedule::Interval { seconds: longer };
                let step = i64::try_from(longer).unwrap_or(i64::MAX / 1000) * 1000;
                routine.next_run_at = Some(self.now().plus_millis(step));
                self.save_routine(&routine)?;
            }
        }
        self.save_channel_state(channel_id, &state)
    }

    /// A user message in a channel resets back-off: the silent count goes
    /// to zero and a doubled check-in interval goes back to its base.
    pub(crate) fn reset_check_in_backoff(&mut self, channel_id: ChannelId) -> Result<()> {
        let mut state = store::channel_state(self.db(), channel_id)?;
        if state.silent_streak == 0 && state.base_seconds.is_none() {
            return Ok(());
        }
        state.silent_streak = 0;
        if let Some(base) = state.base_seconds.take()
            && let Some(mut routine) = check_in_routine(self.db(), channel_id)?
            && let RoutineSchedule::Interval { seconds } = routine.schedule
            && seconds != base
        {
            routine.schedule = RoutineSchedule::Interval { seconds: base };
            let step = i64::try_from(base).unwrap_or(i64::MAX / 1000) * 1000;
            let sooner = self.now().plus_millis(step);
            routine.next_run_at = Some(routine.next_run_at.map_or(sooner, |at| at.min(sooner)));
            self.save_routine(&routine)?;
        }
        self.save_channel_state(channel_id, &state)
    }
}

/// The check-in routine of a channel, when it has one.
pub(crate) fn check_in_routine(
    conn: &Connection,
    channel_id: ChannelId,
) -> Result<Option<consort_proto::model::Routine>> {
    Ok(routines::routines_in_channel(conn, channel_id)?
        .into_iter()
        .find(|r| r.kind == RoutineKind::CheckIn))
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::ids::RoutineId;

    fn user() -> WakeEvent {
        WakeEvent::UserMessage {
            id: MessageId::new(),
            thread: None,
            at: Timestamp::from_millis(1),
            text: "hi".into(),
        }
    }

    fn research() -> WakeEvent {
        WakeEvent::Research {
            routine: RoutineId::new(),
            log: 1,
        }
    }

    fn follow_up() -> WakeEvent {
        WakeEvent::FollowUp {
            routine: RoutineId::new(),
            reason: "Check the quote.".into(),
            set_at: Timestamp::from_millis(1),
            log: 2,
        }
    }

    #[test]
    fn a_turn_is_proactive_only_when_every_event_is() {
        assert_eq!(classify(&[]), TurnKind::Normal);
        assert_eq!(classify(&[user(), follow_up()]), TurnKind::Normal);
        assert_eq!(classify(&[research()]), TurnKind::Research);
        assert_eq!(
            classify(&[follow_up(), research()]),
            TurnKind::Proactive {
                kind: ProactiveKind::FollowUp,
                reason: Some("Check the quote.".into())
            }
        );
        let mark = mark_for(&classify(&[follow_up()])).unwrap();
        assert_eq!(mark.kind, ProactiveKind::FollowUp);
        assert_eq!(mark.reason.as_deref(), Some("Check the quote."));
        assert_eq!(mark_for(&TurnKind::Normal), None);
    }

    #[test]
    fn research_allows_only_reading_and_proactive_turns_cannot_widen_access() {
        use HubTool as T;
        for tool in [
            T::PostMessage,
            T::Reply,
            T::StartTask,
            T::FilesWrite,
            T::MessageTask,
        ] {
            let err = check_tool(&TurnKind::Research, tool).unwrap_err();
            assert_eq!(err.to_error_body().message, RESEARCH_REFUSAL);
        }
        for tool in [
            T::Search,
            T::ReadChannel,
            T::MemoryWrite,
            T::AskDecision,
            T::NoReplyNeeded,
        ] {
            assert!(check_tool(&TurnKind::Research, tool).is_ok(), "{tool:?}");
        }
        let check_in = TurnKind::Proactive {
            kind: ProactiveKind::CheckIn,
            reason: None,
        };
        for tool in [
            T::ConnectService,
            T::AddSkill,
            T::SetChannelTags,
            T::UpdateSkill,
            T::RequestAccess,
        ] {
            assert!(check_tool(&check_in, tool).is_err(), "{tool:?}");
        }
        for tool in [
            T::PostMessage,
            T::StartTask,
            T::SetProactivity,
            T::FollowUpLater,
        ] {
            assert!(check_tool(&check_in, tool).is_ok(), "{tool:?}");
        }
        for tool in HubTool::ALL {
            assert!(check_tool(&TurnKind::Normal, *tool).is_ok());
        }
    }
}
