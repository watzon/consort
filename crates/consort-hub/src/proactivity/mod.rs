//! The assistant on its own: check-ins, the morning brief, follow-ups,
//! stale-task nudges, the research pass, the guided start of a new channel,
//! and the limits that the hub enforces on all of them (ADR 0013).
//!
//! The rules:
//!
//! - The hub decides before a model runs ([`gate`]): the level of the
//!   channel, Pause all, snooze, quiet hours, the daily turn limit and, for
//!   a check-in, "nothing changed" ([`summary`]). A skipped wake costs no
//!   tokens and gets a row in the proactivity log, except a wake that the
//!   level stops.
//! - A turn whose events are all proactive is a proactive turn ([`turn`]).
//!   Its assistant text without a posting call is dropped, its messages are
//!   marked, its tasks run in draft mode, it cannot widen access, and a
//!   research turn may only read.
//! - Every proactive wake is in the log with its outcome.

use std::time::Duration;

use consort_proto::client_api::{
    MessageRate, PROACTIVITY_LOG_PAGE_MAX, ProactivityCancelFollowUps,
    ProactivityCancelFollowUpsResult, ProactivityGet, ProactivityLog, ProactivityLogResult,
    ProactivityResult, ProactivitySet, ProactivitySnooze,
};
use consort_proto::frame::Empty;
use consort_proto::ids::ChannelId;
use consort_proto::model::{
    Channel, PROACTIVE_LOG_KEEP_DAYS, ProactiveKind, ProactiveRating, RoutineKind,
};
use consort_proto::turn_input::WakeEvent;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::store::{Tx, conversation, proactivity as store, routines as routine_store};

pub(crate) mod gate;
pub(crate) mod guide;
pub(crate) mod routines;
mod stale;
pub(crate) mod summary;
#[cfg(test)]
mod tests;
pub(crate) mod tools;
pub(crate) mod turn;

/// How often the hub looks for stale tasks.
const STALE_TICK: Duration = Duration::from_secs(30);
/// The most notifications a day that a user can allow.
const NOTIFY_CAP_MAX: u32 = 20;
const LOG_KEEP_MS: i64 = PROACTIVE_LOG_KEEP_DAYS * 24 * 60 * 60 * 1000;

/// Starts the loop that looks for stale tasks every 30 seconds and purges
/// the log every hour. It stops when the hub shuts down.
pub(crate) fn spawn(hub: &Hub) -> JoinHandle<()> {
    let hub = hub.clone();
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval(STALE_TICK);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut count: u64 = 0;
        loop {
            tokio::select! {
                () = hub.shutdown.cancelled() => break,
                _ = ticks.tick() => {}
            }
            count += 1;
            if let Err(err) = hub.nudge_stale_tasks_at(hub.now()).await {
                tracing::warn!(error = %err, "could not look for stale tasks");
            }
            if count % 120 == 1 {
                let before = hub.now().plus_millis(-LOG_KEEP_MS);
                match hub.store.write(move |tx| tx.purge_wakes(before)).await {
                    Ok(0) => {}
                    Ok(purged) => {
                        tracing::info!(purged, "removed proactivity log entries older than 30 days")
                    }
                    Err(err) => tracing::warn!(error = %err, "could not purge the proactivity log"),
                }
            }
        }
    })
}

/// Queues the guided start of a new channel, when its level and Pause all
/// allow it. Returns true when the caller must kick the coordinator.
pub(crate) fn channel_created(tx: &mut Tx<'_>, channel: &Channel) -> Result<bool> {
    if channel.is_tutti() {
        return Ok(false);
    }
    let settings = store::settings(tx.db())?;
    let now = tx.now();
    let decision = gate::decide(&gate::GateInput {
        kind: ProactiveKind::ChannelCreated,
        level: channel.proactivity_level(),
        channel_name: &channel.name,
        channel_kind: channel.kind,
        settings: &settings,
        now,
        turns_today: 0,
    });
    match decision {
        gate::Gate::Run => {
            let mut wake =
                routines::pending_wake(channel.id, ProactiveKind::ChannelCreated, now, None);
            tx.insert_wake(&mut wake)?;
            tx.queue_wake(
                channel.id,
                &WakeEvent::ChannelCreated {
                    name: channel.name.clone(),
                    kind: channel.kind,
                    log: wake.id,
                },
            )?;
            Ok(true)
        }
        gate::Gate::Skip(outcome, detail) => {
            let mut wake =
                routines::pending_wake(channel.id, ProactiveKind::ChannelCreated, now, None);
            wake.outcome = outcome;
            wake.detail = Some(detail);
            wake.ended_at = Some(now);
            tx.insert_wake(&mut wake)?;
            Ok(false)
        }
        gate::Gate::Quiet => Ok(false),
    }
}

impl Hub {
    /// Gives every channel its proactive routines, once per hub, and the
    /// brief routine its current prompt. The hub calls it at start.
    pub(crate) async fn make_proactive_defaults(&self) -> Result<()> {
        let made = self.store.write(routines::make_defaults_once).await?;
        if made {
            tracing::info!("made the check-in and the morning brief");
        }
        if self.store.write(routines::update_brief_prompt).await? {
            tracing::info!("gave the morning brief its new prompt");
        }
        Ok(())
    }

    /// Looks for stale tasks at `now` and kicks the channels that got a
    /// nudge.
    pub(crate) async fn nudge_stale_tasks_at(
        &self,
        now: consort_proto::time::Timestamp,
    ) -> Result<()> {
        let kick = self
            .store
            .write(move |tx| stale::nudge_stale_tasks(tx, now))
            .await?;
        for channel_id in kick {
            self.kick_coordinator(channel_id);
        }
        Ok(())
    }

    pub(crate) async fn proactivity_get(
        &self,
        _caller: &Caller,
        _request: ProactivityGet,
    ) -> Result<ProactivityResult> {
        let settings = self.store.read(store::settings).await?;
        Ok(ProactivityResult { settings })
    }

    pub(crate) async fn proactivity_set(
        &self,
        _caller: &Caller,
        request: ProactivitySet,
    ) -> Result<ProactivityResult> {
        if let Some(hours) = &request.quiet_hours {
            hours.validate()?;
        }
        if let Some(zone) = &request.time_zone {
            crate::schedule::parse_zone(zone)?;
        }
        if request
            .notify_cap_per_day
            .is_some_and(|cap| cap > NOTIFY_CAP_MAX)
        {
            return Err(HubError::invalid(format!(
                "The notification limit must be from 0 to {NOTIFY_CAP_MAX} a day."
            )));
        }
        let settings = self
            .store
            .write(move |tx| {
                let before = store::settings(tx.db())?;
                let mut settings = before.clone();
                if let Some(paused) = request.paused {
                    settings.paused = paused;
                }
                if let Some(hours) = request.quiet_hours {
                    settings.quiet_hours = hours;
                }
                if let Some(zone) = request.time_zone
                    && (!request.time_zone_if_unset || settings.time_zone.is_none())
                {
                    settings.time_zone = Some(zone.trim().to_owned());
                }
                if let Some(cap) = request.notify_cap_per_day {
                    settings.notify_cap_per_day = cap;
                }
                if settings != before {
                    tx.save_proactivity_settings(&settings)?;
                    if settings.time_zone != before.time_zone {
                        routines::retime_hub_routines(tx, &settings)?;
                    }
                }
                Ok(settings)
            })
            .await?;
        Ok(ProactivityResult { settings })
    }

    pub(crate) async fn proactivity_snooze(
        &self,
        _caller: &Caller,
        request: ProactivitySnooze,
    ) -> Result<ProactivityResult> {
        let settings = self
            .store
            .write(move |tx| {
                let now = tx.now();
                let mut settings = store::settings(tx.db())?;
                settings.snoozed_until = gate::snooze_until(&settings, request.choice, now);
                tx.save_proactivity_settings(&settings)?;
                Ok(settings)
            })
            .await?;
        Ok(ProactivityResult { settings })
    }

    pub(crate) async fn proactivity_log(
        &self,
        _caller: &Caller,
        request: ProactivityLog,
    ) -> Result<ProactivityLogResult> {
        let limit = usize::try_from(
            request
                .limit
                .unwrap_or(100)
                .clamp(1, PROACTIVITY_LOG_PAGE_MAX),
        )
        .unwrap_or(100);
        let (wakes, has_more) = self
            .store
            .read(move |conn| {
                store::wakes_page(
                    conn,
                    request.channel_id,
                    request.kind,
                    request.before,
                    limit,
                )
            })
            .await?;
        Ok(ProactivityLogResult { wakes, has_more })
    }

    pub(crate) async fn proactivity_cancel_follow_ups(
        &self,
        _caller: &Caller,
        request: ProactivityCancelFollowUps,
    ) -> Result<ProactivityCancelFollowUpsResult> {
        let canceled = self
            .store
            .write(move |tx| {
                let mut canceled = 0_u32;
                for routine in routine_store::routines(tx.db())? {
                    if routine.kind != RoutineKind::FollowUp
                        || request.channel_id.is_some_and(|c| c != routine.channel_id)
                    {
                        continue;
                    }
                    tx.delete_routine(&routine)?;
                    tx.delete_follow_up_row(routine.id)?;
                    canceled += 1;
                }
                Ok(canceled)
            })
            .await?;
        Ok(ProactivityCancelFollowUpsResult { canceled })
    }

    /// `message.rate`. Useful is only recorded. Too noisy also wakes the
    /// coordinator once, which adds a skip line to the check-in note and
    /// confirms in one reply.
    pub(crate) async fn message_rate(
        &self,
        _caller: &Caller,
        request: MessageRate,
    ) -> Result<Empty> {
        let kick: Option<ChannelId> = self
            .store
            .write(move |tx| {
                let mut message = conversation::message_or_404(tx.db(), request.message_id)?;
                let Some(mark) = message.proactive.as_mut() else {
                    return Err(HubError::invalid(
                        "Only a message that the assistant posted on its own can be rated.",
                    ));
                };
                if mark.rating == request.rating {
                    return Ok(None);
                }
                mark.rating = request.rating;
                tx.save_message(&message)?;
                if request.rating != Some(ProactiveRating::TooNoisy) {
                    return Ok(None);
                }
                let channel = conversation::channel_or_404(tx.db(), message.channel_id)?;
                if channel.archived_at.is_some() {
                    return Ok(None);
                }
                let mut wake =
                    routines::pending_wake(channel.id, ProactiveKind::Feedback, tx.now(), None);
                wake.message_ids = vec![message.id];
                tx.insert_wake(&mut wake)?;
                tx.queue_wake(
                    channel.id,
                    &WakeEvent::ProactiveFeedback {
                        message: message.id,
                        rating: ProactiveRating::TooNoisy,
                        log: wake.id,
                    },
                )?;
                Ok(Some(channel.id))
            })
            .await?;
        if let Some(channel_id) = kick {
            self.kick_coordinator(channel_id);
        }
        Ok(Empty {})
    }
}
