//! Periodic work: due tasks, revokes from the hub machine, the feed
//! retention, the purge of old deleted memory notes, stuck coordinator turns, quiet task turns and expired
//! commands and idle sign-ins.

use std::time::Duration;

use tokio::task::JoinHandle;

use crate::hub::Hub;

const STUCK_TURN_MS: i64 = 2 * 60 * 60 * 1000;

/// How long a deleted memory note stays in Recently deleted.
const MEMORY_DELETED_KEEP_MS: i64 =
    consort_proto::model::MEMORY_DELETED_KEEP_DAYS * 24 * 60 * 60 * 1000;
/// How often, in seconds, the hub reads the revokes that a command on the
/// hub machine wrote.
const REVOKE_CHECK_TICKS: u64 = 2;

/// How often, in seconds, the hub ends the sign-ins that got no update.
const SIGN_IN_CHECK_TICKS: u64 = 5;

pub(crate) fn spawn(hub: &Hub) -> JoinHandle<()> {
    let hub = hub.clone();
    tokio::spawn(async move {
        let mut second = tokio::time::interval(Duration::from_secs(1));
        second.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut ticks: u64 = 0;
        loop {
            tokio::select! {
                () = hub.shutdown.cancelled() => break,
                _ = second.tick() => {}
            }
            ticks += 1;
            if let Err(err) = hub.start_due_tasks().await {
                tracing::warn!(error = %err, "could not start due tasks");
            }
            // The first tick, then every 2 s: clients and workers that
            // `consort hub client revoke` or `worker revoke` removed.
            if ticks % REVOKE_CHECK_TICKS == 1
                && let Err(err) = hub.apply_revokes_from_disk().await
            {
                tracing::warn!(error = %err, "could not check for removed clients and workers");
            }
            if ticks.is_multiple_of(SIGN_IN_CHECK_TICKS)
                && let Err(err) = hub.expire_sign_ins().await
            {
                tracing::warn!(error = %err, "could not expire sign-ins");
            }
            if ticks.is_multiple_of(60) {
                if let Err(err) = hub.expire_stuck_turns(STUCK_TURN_MS).await {
                    tracing::warn!(error = %err, "could not check coordinator turns");
                }
                if let Err(err) = hub.expire_quiet_task_turns(STUCK_TURN_MS).await {
                    tracing::warn!(error = %err, "could not check task turns");
                }
                if let Err(err) = hub.expire_queued_commands().await {
                    tracing::warn!(error = %err, "could not expire queued commands");
                }
            }
            if ticks.is_multiple_of(3600) {
                let result = hub
                    .store
                    .write(|tx| {
                        let pruned = tx.prune_feed()?;
                        tx.delete_expired_pairing_codes()?;
                        let before = tx.now().plus_millis(-MEMORY_DELETED_KEEP_MS);
                        let purged = tx.purge_deleted_memory(before)?;
                        if purged > 0 {
                            tracing::info!(
                                purged,
                                "removed memory notes that were deleted 30 days ago"
                            );
                        }
                        Ok(pruned)
                    })
                    .await;
                match result {
                    Ok(pruned) if pruned > 0 => tracing::info!(pruned, "removed old feed entries"),
                    Ok(_) => {}
                    Err(err) => tracing::warn!(error = %err, "could not prune the feed"),
                }
            }
        }
    })
}
