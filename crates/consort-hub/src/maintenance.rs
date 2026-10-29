//! Periodic work: due tasks, the feed retention, stuck turns and expired
//! commands.

use std::time::Duration;

use tokio::task::JoinHandle;

use crate::hub::Hub;

const STUCK_TURN_MS: i64 = 2 * 60 * 60 * 1000;

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
            if ticks.is_multiple_of(60) {
                if let Err(err) = hub.expire_stuck_turns(STUCK_TURN_MS).await {
                    tracing::warn!(error = %err, "could not check coordinator turns");
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
