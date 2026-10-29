//! Pairing endpoints and the per-IP attempt limit.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use consort_proto::auth::{
    PAIRING_CODE_TTL_MS, PairClientRequest, PairClientResponse, PairWorkerRequest,
    PairWorkerResponse, PairingCode,
};
use consort_proto::error::{ErrorBody, ErrorCode, HttpErrorBody};
use consort_proto::ids::{ClientId, WorkerId};
use consort_proto::model::{Client, Worker, WorkerState};
use consort_proto::time::Timestamp;
use serde::de::DeserializeOwned;

use crate::error::{HubError, Result};
use crate::hub::Hub;
use crate::store::machine::{self, PairingKind};
use crate::util::random_pairing_code;

const MAX_ATTEMPTS: usize = 10;
const WINDOW: Duration = Duration::from_secs(60);

/// Limits pairing requests per client IP address: 10 in 60 seconds. Behind a
/// reverse proxy every request shares the proxy address, so the limit then
/// applies to all pairing requests together. That is the safe direction.
#[derive(Default)]
pub struct PairingLimiter {
    attempts: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl PairingLimiter {
    /// Records an attempt. Returns false when the address made too many.
    pub fn allow(&self, ip: IpAddr) -> bool {
        self.allow_at(ip, Instant::now())
    }

    fn allow_at(&self, ip: IpAddr, now: Instant) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
            v4 => v4,
        };
        let Ok(mut attempts) = self.attempts.lock() else {
            return false;
        };
        attempts.retain(|_, times| {
            while times
                .front()
                .is_some_and(|t| now.duration_since(*t) > WINDOW)
            {
                times.pop_front();
            }
            !times.is_empty()
        });
        let times = attempts.entry(ip).or_default();
        if times.len() >= MAX_ATTEMPTS {
            return false;
        }
        times.push_back(now);
        true
    }
}

fn http_error(error: ErrorBody) -> Response {
    let status =
        StatusCode::from_u16(error.code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(HttpErrorBody { error })).into_response()
}

pub(crate) fn error_response(err: &HubError) -> Response {
    http_error(err.to_error_body())
}

fn parse_body<T: DeserializeOwned>(body: &Bytes) -> std::result::Result<T, Box<Response>> {
    serde_json::from_slice(body).map_err(|e| {
        Box::new(http_error(ErrorBody::invalid(format!(
            "The request body is not valid: {e}. Check the fields and try again."
        ))))
    })
}

fn too_many() -> Response {
    http_error(ErrorBody::new(
        ErrorCode::PolicyDenied,
        "Too many pairing attempts. Wait a minute, then try again.",
    ))
    .into_response_with_status(StatusCode::TOO_MANY_REQUESTS)
}

trait WithStatus {
    fn into_response_with_status(self, status: StatusCode) -> Response;
}

impl WithStatus for Response {
    fn into_response_with_status(mut self, status: StatusCode) -> Response {
        *self.status_mut() = status;
        self
    }
}

pub(crate) async fn pair_worker(
    State(hub): State<Hub>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: Bytes,
) -> Response {
    if !hub.pairing_limiter.allow(addr.ip()) {
        return too_many();
    }
    let request: PairWorkerRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(response) => return *response,
    };
    match hub.pair_worker(request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => error_response(&err),
    }
}

pub(crate) async fn pair_client(
    State(hub): State<Hub>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: Bytes,
) -> Response {
    if !hub.pairing_limiter.allow(addr.ip()) {
        return too_many();
    }
    let request: PairClientRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(response) => return *response,
    };
    match hub.pair_client(request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => error_response(&err),
    }
}

fn bad_code() -> HubError {
    HubError::unauthorized("The pairing code is not valid or has expired. Ask for a new code.")
}

impl Hub {
    /// Makes a single-use pairing code that is valid for 10 minutes.
    pub(crate) async fn new_pairing_code(
        &self,
        kind: PairingKind,
    ) -> Result<(PairingCode, Timestamp)> {
        let code = random_pairing_code();
        let stored = code.clone();
        let expires = self
            .store
            .write(move |tx| {
                machine::insert_pairing_code(tx.db(), kind, &stored, tx.now(), PAIRING_CODE_TTL_MS)
            })
            .await?;
        Ok((code, expires))
    }

    pub(crate) async fn pair_worker(
        &self,
        request: PairWorkerRequest,
    ) -> Result<PairWorkerResponse> {
        let id = self
            .store
            .write(move |tx| {
                if !machine::consume_pairing_code(
                    tx.db(),
                    PairingKind::Worker,
                    &request.code,
                    tx.now(),
                )? {
                    return Err(bad_code());
                }
                let now = tx.now();
                let name = request
                    .name
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| request.hostname.clone());
                let worker = Worker {
                    id: WorkerId::new(),
                    name,
                    hostname: request.hostname,
                    os: request.os,
                    arch: request.arch,
                    public_key: request.public_key,
                    version: String::new(),
                    protocol_version: 0,
                    labels: Vec::new(),
                    paused: false,
                    state: WorkerState::Offline,
                    last_seen_at: None,
                    inventory: None,
                    paired_at: now,
                    revoked_at: None,
                };
                tx.save_worker(&worker)?;
                Ok(worker.id)
            })
            .await?;
        tracing::info!(worker_id = %id, "paired a worker");
        Ok(PairWorkerResponse { worker_id: id })
    }

    pub(crate) async fn pair_client(
        &self,
        request: PairClientRequest,
    ) -> Result<PairClientResponse> {
        let id = self
            .store
            .write(move |tx| {
                if !machine::consume_pairing_code(
                    tx.db(),
                    PairingKind::Client,
                    &request.code,
                    tx.now(),
                )? {
                    return Err(bad_code());
                }
                let client = Client {
                    id: ClientId::new(),
                    name: request.name,
                    kind: request.kind,
                    public_key: request.public_key,
                    push_provider: None,
                    paired_at: tx.now(),
                    last_seen_at: None,
                    revoked_at: None,
                };
                tx.save_client(&client)?;
                Ok(client.id)
            })
            .await?;
        self.setup.mark_paired();
        tracing::info!(client_id = %id, "paired a client");
        Ok(PairClientResponse { client_id: id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limiter_blocks_the_eleventh_attempt_in_a_minute() {
        let limiter = PairingLimiter::default();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let start = Instant::now();
        for _ in 0..MAX_ATTEMPTS {
            assert!(limiter.allow_at(ip, start));
        }
        assert!(!limiter.allow_at(ip, start));
        // Another address is not affected.
        assert!(limiter.allow_at("10.0.0.2".parse().unwrap(), start));
        // The window moves on.
        assert!(limiter.allow_at(ip, start + WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn an_ipv4_mapped_address_counts_as_ipv4() {
        let limiter = PairingLimiter::default();
        let start = Instant::now();
        let v4: IpAddr = "10.0.0.1".parse().unwrap();
        let mapped: IpAddr = "::ffff:10.0.0.1".parse().unwrap();
        for _ in 0..MAX_ATTEMPTS {
            assert!(limiter.allow_at(v4, start));
        }
        assert!(!limiter.allow_at(mapped, start));
    }
}
