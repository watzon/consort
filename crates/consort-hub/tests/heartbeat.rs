//! A worker that sends no frame for 60 seconds is offline.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::client_api::{EntityPayload, SnapshotScope};
use consort_proto::model::WorkerState;

#[tokio::test(start_paused = true)]
async fn a_silent_worker_goes_offline_after_sixty_seconds() {
    // The clock is paused, and the sockets and the store use real time. Only
    // `clock.advance` may move the clock, so no timer fires while a real
    // frame is on its way.
    let clock = FrozenClock::freeze();
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let worker = FakeWorker::pair_and_connect(&hub).await;
    let id = worker.worker_id;
    let snapshot = client.snapshot(SnapshotScope::All).await;
    client.subscribe(snapshot.seq).await.unwrap();

    // The worker is online while it is quiet for 59 seconds.
    clock.advance(Duration::from_secs(59)).await;
    let snapshot = client.snapshot(SnapshotScope::All).await;
    assert_eq!(worker_state(&snapshot.workers, id), WorkerState::Online);

    // After 60 seconds with no frame, the hub marks it offline.
    clock.advance(Duration::from_secs(1)).await;
    client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Worker(w)) if w.id == id && w.state == WorkerState::Offline))
        .await;
    let snapshot = client.snapshot(SnapshotScope::All).await;
    assert_eq!(worker_state(&snapshot.workers, id), WorkerState::Offline);
    clock.assert_held();
    drop(worker);
    hub.handle.shutdown().await.unwrap();
}
