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
use crate::store::machine::{self, Origin, PairingKind};
use crate::util::random_pairing_code;

/// Attempts from one address in one window.
const MAX_ATTEMPTS: usize = 10;
/// Attempts from all addresses together in one window. Every address in
/// the limiter has an attempt in the window, so this also caps the number
/// of addresses that the limiter keeps.
const MAX_GLOBAL_ATTEMPTS: usize = 60;
const WINDOW: Duration = Duration::from_secs(60);

/// Limits pairing requests: 10 in 60 seconds from one address, and 60 in 60
/// seconds from all addresses together. An IPv6 address counts by its /64
/// network, because one host usually has a whole /64. Behind a reverse proxy
/// every request shares the proxy address, so the limit then applies to all
/// pairing requests together. That is the safe direction.
#[derive(Default)]
pub struct PairingLimiter {
    state: Mutex<LimiterState>,
}

#[derive(Default)]
struct LimiterState {
    by_address: HashMap<IpAddr, VecDeque<Instant>>,
    all: VecDeque<Instant>,
}

/// The key of an address: IPv4 as it is (also when mapped into IPv6), and
/// the /64 network of an IPv6 address.
pub(crate) fn limit_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
        v4 => v4,
    }
}

fn drop_old(times: &mut VecDeque<Instant>, now: Instant) {
    while times
        .front()
        .is_some_and(|t| now.duration_since(*t) > WINDOW)
    {
        times.pop_front();
    }
}

impl PairingLimiter {
    /// Records an attempt. Returns false when the address made too many, or
    /// when all addresses together made too many.
    pub fn allow(&self, ip: IpAddr) -> bool {
        self.allow_at(ip, Instant::now())
    }

    fn allow_at(&self, ip: IpAddr, now: Instant) -> bool {
        let key = limit_key(ip);
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let state = &mut *state;
        drop_old(&mut state.all, now);
        state.by_address.retain(|_, times| {
            drop_old(times, now);
            !times.is_empty()
        });
        if state.all.len() >= MAX_GLOBAL_ATTEMPTS {
            return false;
        }
        let times = state.by_address.entry(key).or_default();
        if times.len() >= MAX_ATTEMPTS {
            return false;
        }
        times.push_back(now);
        state.all.push_back(now);
        true
    }

    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.state.lock().map(|s| s.by_address.len()).unwrap_or(0)
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
    /// Makes a single-use pairing code that is valid for 10 minutes, and
    /// records who made it. Removing that client cancels the code.
    pub(crate) async fn new_pairing_code(
        &self,
        kind: PairingKind,
        created_by: Origin,
    ) -> Result<(PairingCode, Timestamp)> {
        let code = random_pairing_code();
        let stored = code.clone();
        let expires = self
            .store
            .write(move |tx| {
                machine::insert_pairing_code(
                    tx.db(),
                    kind,
                    &stored,
                    tx.now(),
                    PAIRING_CODE_TTL_MS,
                    created_by,
                )
            })
            .await?;
        Ok((code, expires))
    }

    /// Refuses a code that cannot work with a read, so a wrong code does not
    /// wait for the writer or hold it up.
    async fn check_code(&self, kind: PairingKind, code: &PairingCode) -> Result<()> {
        let code = code.clone();
        let usable = self
            .store
            .read(move |conn| machine::pairing_code_usable(conn, kind, &code, Timestamp::now()))
            .await?;
        if usable { Ok(()) } else { Err(bad_code()) }
    }

    pub(crate) async fn pair_worker(
        &self,
        request: PairWorkerRequest,
    ) -> Result<PairWorkerResponse> {
        self.check_code(PairingKind::Worker, &request.code).await?;
        let id = self
            .store
            .write(move |tx| {
                let origin =
                    machine::pairing_code_origin(tx.db(), PairingKind::Worker, &request.code)?;
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
                    tags: Vec::new(),
                    paused: false,
                    state: WorkerState::Offline,
                    last_seen_at: None,
                    inventory: None,
                    paired_at: now,
                    revoked_at: None,
                };
                tx.save_worker(&worker)?;
                tx.set_worker_paired_by(worker.id, origin)?;
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
        self.check_code(PairingKind::Client, &request.code).await?;
        let id = self
            .store
            .write(move |tx| {
                let origin =
                    machine::pairing_code_origin(tx.db(), PairingKind::Client, &request.code)?;
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
                    os: request.os,
                    paired_by: origin.map(Origin::paired_by),
                };
                tx.save_client(&client)?;
                tx.set_client_paired_by(client.id, origin)?;
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
    fn an_ipv6_address_counts_by_its_64_network() {
        let limiter = PairingLimiter::default();
        let start = Instant::now();
        for i in 0..MAX_ATTEMPTS {
            let ip: IpAddr = format!("2001:db8:1:2::{:x}", i + 1).parse().unwrap();
            assert!(limiter.allow_at(ip, start));
        }
        // Another host in the same /64 is the same address.
        assert!(!limiter.allow_at("2001:db8:1:2:ffff::9".parse().unwrap(), start));
        // Another /64 is not.
        assert!(limiter.allow_at("2001:db8:1:3::1".parse().unwrap(), start));
    }

    #[test]
    fn all_addresses_together_have_a_limit_that_also_caps_the_map() {
        let limiter = PairingLimiter::default();
        let start = Instant::now();
        let ip = |i: usize| -> IpAddr { format!("10.1.{}.{}", i / 200, i % 200).parse().unwrap() };
        for i in 0..MAX_GLOBAL_ATTEMPTS {
            assert!(limiter.allow_at(ip(i), start));
        }
        // A new address is refused too, and the map does not grow.
        for i in MAX_GLOBAL_ATTEMPTS..1000 {
            assert!(!limiter.allow_at(ip(i), start));
        }
        assert_eq!(limiter.tracked(), MAX_GLOBAL_ATTEMPTS);
        // The window moves on.
        assert!(limiter.allow_at(ip(5000), start + WINDOW + Duration::from_secs(1)));
        assert_eq!(limiter.tracked(), 1);
    }

    #[tokio::test]
    async fn a_wrong_code_is_refused_without_waiting_for_the_writer() {
        let (hub, _dir) = crate::hub::test_hub().await;
        // A long write holds the writer.
        let store = hub.store.clone();
        let busy = tokio::spawn(async move {
            store
                .write(|_| {
                    std::thread::sleep(Duration::from_secs(2));
                    Ok(())
                })
                .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let started = Instant::now();
        let result = hub
            .pair_client(PairClientRequest {
                code: random_pairing_code(),
                public_key: consort_proto::auth::PublicKey::from_bytes([7; 32]),
                name: "Guess".to_owned(),
                kind: consort_proto::model::ClientKind::Desktop,
                os: None,
            })
            .await;
        assert_eq!(result.unwrap_err().code(), ErrorCode::Unauthorized);
        assert!(started.elapsed() < Duration::from_secs(1));
        busy.await.unwrap().unwrap();
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
