//! HTTP and WebSocket transport.

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use consort_proto::auth::{CLIENT_WS_PATH, PAIR_CLIENT_PATH, PAIR_WORKER_PATH, WORKER_WS_PATH};
use serde_json::{Value, json};

use crate::hub::{HUB_VERSION, Hub};

pub(crate) mod blobs;
pub(crate) mod client_ws;
pub mod clients;
pub mod pairing;
pub(crate) mod worker_ws;
pub(crate) mod ws;

async fn healthz() -> Json<Value> {
    Json(json!({"ok": true, "version": HUB_VERSION}))
}

pub(crate) fn router(hub: Hub) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route(PAIR_WORKER_PATH, post(pairing::pair_worker))
        .route(PAIR_CLIENT_PATH, post(pairing::pair_client))
        .route(CLIENT_WS_PATH, get(client_ws::client_ws))
        .route(WORKER_WS_PATH, get(worker_ws::worker_ws))
        .route(
            "/v1/blobs/{sha256}",
            put(blobs::put_blob)
                .get(blobs::get_blob)
                .layer(DefaultBodyLimit::max(blobs::MAX_BLOB_BYTES + 1024)),
        )
        .with_state(hub)
}
