//! A worker that sends no frame for 60 seconds is offline.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::client_api::{EntityPayload, SnapshotScope};
use consort_proto::model::WorkerState;

#[tokio::test(start_paused = true)]
async fn a_silent_worker_goes_offline_after_sixty_seconds() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let worker = FakeWorker::pair_and_connect(&hub).await;
    let id = worker.worker_id;
    let snapshot = client.snapshot(SnapshotScope::All).await;
    client.subscribe(snapshot.seq).await.unwrap();

    // The worker is online while it is quiet for a short time.
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(!client.feed.iter().any(
        |e| matches!(upserted(e), Some(EntityPayload::Worker(w)) if w.id == id && w.state == WorkerState::Offline)
    ));

    // After 60 seconds with no frame, the hub marks it offline.
    tokio::time::sleep(Duration::from_secs(40)).await;
    client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Worker(w)) if w.id == id && w.state == WorkerState::Offline))
        .await;
    drop(worker);
    hub.handle.shutdown().await.unwrap();
}
