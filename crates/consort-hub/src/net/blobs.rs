//! Blob upload and download.

use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use consort_proto::auth::{
    HEADER_SIGNATURE, HEADER_TIMESTAMP, HEADER_WORKER_ID, HTTP_SIGNATURE_MAX_SKEW_MS, Signature,
    http_signing_payload,
};
use consort_proto::ids::WorkerId;
use consort_proto::model::Sha256Hash;
use consort_proto::time::Timestamp;
use serde_json::json;

use super::pairing::error_response;
use super::ws::verify_signature;
use crate::error::{HubError, Result};
use crate::hub::Hub;
use crate::util::{random_token, sha256};

/// The largest blob that the hub accepts.
pub(crate) const MAX_BLOB_BYTES: usize = 256 * 1024 * 1024;

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

impl Hub {
    /// Checks a blob request: a client bearer token, or a worker signature.
    async fn authorize_http(&self, method: &Method, uri: &Uri, headers: &HeaderMap) -> Result<()> {
        let denied = || {
            HubError::unauthorized(
                "Blob requests need a client token or a worker signature. Send auth.http_token first.",
            )
        };
        let now = Timestamp::now();
        if let Some(auth) = header_text(headers, header::AUTHORIZATION.as_str()) {
            let token = auth.strip_prefix("Bearer ").ok_or_else(denied)?;
            let client = self.clients.check_token(token, now).ok_or_else(denied)?;
            // A revoked client keeps no access.
            if self.client_is_active(client).await? {
                return Ok(());
            }
            return Err(denied());
        }
        let worker: WorkerId = header_text(headers, HEADER_WORKER_ID)
            .ok_or_else(denied)?
            .parse()
            .map_err(|_| denied())?;
        let timestamp: i64 = header_text(headers, HEADER_TIMESTAMP)
            .and_then(|v| v.parse().ok())
            .ok_or_else(denied)?;
        if (now.as_millis() - timestamp).abs() > HTTP_SIGNATURE_MAX_SKEW_MS {
            return Err(HubError::unauthorized(
                "The request time is too far from the hub time. Check the clock on this machine.",
            ));
        }
        let signature: Signature = header_text(headers, HEADER_SIGNATURE)
            .and_then(|v| v.parse().ok())
            .ok_or_else(denied)?;
        let key = self
            .peer_public_key(consort_proto::auth::PeerId::Worker(worker))
            .await?
            .ok_or_else(denied)?;
        let payload = http_signing_payload(
            method.as_str(),
            &self.settings.origin,
            uri.path(),
            Timestamp::from_millis(timestamp),
            worker,
        );
        if verify_signature(&key, &payload, &signature) {
            Ok(())
        } else {
            Err(denied())
        }
    }

    fn blob_file(&self, hash: &Sha256Hash) -> PathBuf {
        self.store.blobs_dir().join(hash.as_str())
    }
}

fn parse_hash(text: &str) -> Result<Sha256Hash> {
    text.parse().map_err(|_| {
        HubError::invalid("The blob name must be a SHA-256 hash of 64 hex characters.")
    })
}

pub(crate) async fn put_blob(
    State(hub): State<Hub>,
    Path(name): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result: Result<Response> = async {
        hub.authorize_http(&method, &uri, &headers).await?;
        let expected = parse_hash(&name)?;
        if body.len() > MAX_BLOB_BYTES {
            return Err(HubError::invalid(
                "The blob is too large. The limit is 256 MiB.",
            ));
        }
        if sha256(&body) != expected {
            return Err(HubError::invalid(
                "The content does not match the hash in the URL. Upload the file again.",
            ));
        }
        let target = hub.blob_file(&expected);
        if !target.exists() {
            let temp = target.with_extension(format!("tmp-{}", &random_token()[..12]));
            tokio::fs::write(&temp, &body).await?;
            if let Err(err) = tokio::fs::rename(&temp, &target).await {
                let _ = tokio::fs::remove_file(&temp).await;
                return Err(err.into());
            }
        }
        Ok((
            StatusCode::OK,
            axum::Json(json!({"sha256": expected, "size": body.len()})),
        )
            .into_response())
    }
    .await;
    result.unwrap_or_else(|err| error_response(&err))
}

pub(crate) async fn get_blob(
    State(hub): State<Hub>,
    Path(name): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let result: Result<Response> = async {
        hub.authorize_http(&method, &uri, &headers).await?;
        let hash = parse_hash(&name)?;
        let bytes = match tokio::fs::read(hub.blob_file(&hash)).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(HubError::not_found("The hub has no blob with this hash."));
            }
            Err(err) => return Err(err.into()),
        };
        Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response())
    }
    .await;
    result.unwrap_or_else(|err| error_response(&err))
}
