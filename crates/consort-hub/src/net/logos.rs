//! `GET /v1/logos/{key}`: a service logo from the logo cache.
//!
//! The route needs no login, because a client loads it as a plain image. It
//! serves only a key that the hub registered, so it is no proxy for other
//! URLs (see `connectors/logos.rs`).

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::pairing::error_response;
use crate::connectors::logos::LogoCache;
use crate::error::HubError;
use crate::hub::Hub;

pub(crate) async fn get_logo(State(hub): State<Hub>, Path(key): Path<String>) -> Response {
    if !LogoCache::is_key(&key) {
        return error_response(&HubError::not_found("The hub has no logo with this key."));
    }
    let Some(logo) = hub.connectors.logos.get(&key).await else {
        return error_response(&HubError::not_found("The hub has no logo with this key."));
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, logo.content_type),
            // A logo changes at most weekly.
            (header::CACHE_CONTROL, "public, max-age=86400"),
            // An SVG that a browser opens must not run script or load files.
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; sandbox",
            ),
        ],
        logo.bytes,
    )
        .into_response()
}
