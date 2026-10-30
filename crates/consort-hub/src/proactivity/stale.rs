//! Stale-task nudges: a hub timer, with no model poll. A task in Needs input
//! for 24 hours, or in Ready for review for 72 hours, adds one `stale_task`
//! wake event to its channel, once per status entry.

use consort_proto::ids::ChannelId;
use consort_proto::model::{
    ProactiveKind, ProactiveOutcome, STALE_NEEDS_INPUT_MS, STALE_REVIEW_MS,
};
use consort_proto::status::TaskStatus;
use consort_proto::time::Timestamp;
use consort_proto::turn_input::WakeEvent;

use super::gate::{self, Gate, GateInput};
use super::routines::pending_wake;
use crate::error::Result;
use crate::store::{Tx, conversation, proactivity as store, work};

/// How long a task in this status waits before its nudge, or `None` for a
/// status that never gets one.
fn stale_after(status: TaskStatus) -> Option<i64> {
    match status {
        TaskStatus::NeedsInput => Some(STALE_NEEDS_INPUT_MS),
        TaskStatus::Review => Some(STALE_REVIEW_MS),
        _ => None,
    }
}

/// Looks at every task in Needs input or Ready for review at `now`. Returns
/// the channels whose coordinators must be kicked.
pub(crate) fn nudge_stale_tasks(tx: &mut Tx<'_>, now: Timestamp) -> Result<Vec<ChannelId>> {
    let settings = store::settings(tx.db())?;
    let mut tasks = work::tasks_with_status(tx.db(), TaskStatus::NeedsInput)?;
    tasks.extend(work::tasks_with_status(tx.db(), TaskStatus::Review)?);
    let mut kick: Vec<ChannelId> = Vec::new();
    for task in tasks {
        if task.archived_at.is_some() {
            continue;
        }
        let Some(after) = stale_after(task.status) else {
            continue;
        };
        let row = match store::status_since(tx.db(), task.id)? {
            Some(row) if row.status == task.status => row,
            _ => {
                // A task from before this hub kept the time: its last change
                // is the best guess of when it entered its status.
                tx.note_task_status_at(&task, task.updated_at)?;
                match store::status_since(tx.db(), task.id)? {
                    Some(row) => row,
                    None => continue,
                }
            }
        };
        if row.nudged || now.as_millis() - row.since.as_millis() < after {
            continue;
        }
        let Some(channel) = conversation::channel(tx.db(), task.channel_id)? else {
            continue;
        };
        if channel.archived_at.is_some() {
            continue;
        }
        let turns_today =
            store::turns_since(tx.db(), channel.id, gate::start_of_day(&settings, now))?;
        let decision = gate::decide(&GateInput {
            kind: ProactiveKind::StaleTask,
            level: channel.proactivity_level(),
            channel_name: &channel.name,
            channel_kind: channel.kind,
            settings: &settings,
            now,
            turns_today,
        });
        match decision {
            // The level stops it, or it waits for the end of quiet hours.
            Gate::Quiet => {}
            Gate::Skip(outcome, detail) => {
                let held = matches!(
                    outcome,
                    ProactiveOutcome::SkippedSnoozed | ProactiveOutcome::SkippedPaused
                );
                // A nudge that waits for the end of a snooze or Pause all
                // gets one row. A nudge over the turn limit is skipped for
                // good.
                if held && row.held_logged {
                    continue;
                }
                let mut wake = pending_wake(channel.id, ProactiveKind::StaleTask, now, None);
                wake.outcome = outcome;
                wake.detail = Some(detail);
                wake.task_id = Some(task.id);
                wake.ended_at = Some(now);
                tx.insert_wake(&mut wake)?;
                if held {
                    tx.mark_status_held_logged(task.id)?;
                } else {
                    tx.mark_status_nudged(task.id)?;
                }
            }
            Gate::Run => {
                let mut wake = pending_wake(channel.id, ProactiveKind::StaleTask, now, None);
                wake.task_id = Some(task.id);
                tx.insert_wake(&mut wake)?;
                tx.queue_wake(
                    channel.id,
                    &WakeEvent::StaleTask {
                        task: task.id,
                        title: task.title.clone(),
                        status: task.status,
                        since: row.since,
                        log: wake.id,
                    },
                )?;
                tx.mark_status_nudged(task.id)?;
                if !kick.contains(&channel.id) {
                    kick.push(channel.id);
                }
            }
        }
    }
    Ok(kick)
}
