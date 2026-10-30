//! Review fixes that the hub's HTTP side shows.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use consort_proto::auth::{HEADER_SIGNATURE, HEADER_TIMESTAMP, HEADER_WORKER_ID, blob_path};
use consort_proto::model::Sha256Hash;

use crate::common::World;

/// A worker signature with a time far out of range is refused with 401.
/// The time math is checked, so the request never panics its handler.
#[tokio::test]
async fn a_blob_request_with_a_time_far_out_of_range_is_refused() {
    let world = World::start().await;
    let path = blob_path(&Sha256Hash::from_bytes([3; 32]));
    for timestamp in [i64::MIN, i64::MAX] {
        let response = reqwest::Client::new()
            .get(format!("{}{path}", world.hub.url))
            .header(HEADER_WORKER_ID, world.worker.worker_id.to_string())
            .header(HEADER_TIMESTAMP, timestamp.to_string())
            .header(HEADER_SIGNATURE, "00".repeat(64))
            .send()
            .await
            .expect("an answer, not a closed connection");
        assert_eq!(response.status(), 401, "{timestamp}");
    }
    world.hub.handle.shutdown().await.unwrap();
}
