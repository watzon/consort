//! Error codes and the `{code, message}` error body.
//!
//! An error response frame has `"ok": false` and an `error` object with a
//! `code` from [`ErrorCode`] and a human `message`. HTTP endpoints (pairing and
//! blobs) send the same object as `{"error": {...}}` with the status from
//! [`ErrorCode::http_status`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The error codes in protocol.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The key, signature or token is not valid, or the worker or client is revoked.
    Unauthorized,
    /// No common protocol version.
    ProtocolMismatch,
    /// The body does not match the schema. The message names the field.
    Invalid,
    /// The entity does not exist.
    NotFound,
    /// The expected memory note hash does not match the stored hash.
    StaleWrite,
    /// The worker that this action needs is offline.
    WorkerOffline,
    /// No worker has this ID.
    WorkerUnknown,
    /// No online worker has a harness account with this label and model.
    NoMatchingAccount,
    /// The worker policy does not allow this action. The message names the rule.
    PolicyDenied,
    /// The client's feed position is older than the oldest kept feed entry.
    ResyncRequired,
    /// The action does not apply to the entity's current state.
    Conflict,
    /// An unexpected error on the hub. The hub logs the details.
    Internal,
    /// A code that this version does not know. A newer peer sent it. Treat it
    /// like [`ErrorCode::Internal`] and show the message.
    Unknown,
}

impl ErrorCode {
    /// Every known code, in the order of protocol.md.
    pub const ALL: [ErrorCode; 12] = [
        Self::Unauthorized,
        Self::ProtocolMismatch,
        Self::Invalid,
        Self::NotFound,
        Self::StaleWrite,
        Self::WorkerOffline,
        Self::WorkerUnknown,
        Self::NoMatchingAccount,
        Self::PolicyDenied,
        Self::ResyncRequired,
        Self::Conflict,
        Self::Internal,
    ];

    /// The wire string, for example `worker_offline`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::ProtocolMismatch => "protocol_mismatch",
            Self::Invalid => "invalid",
            Self::NotFound => "not_found",
            Self::StaleWrite => "stale_write",
            Self::WorkerOffline => "worker_offline",
            Self::WorkerUnknown => "worker_unknown",
            Self::NoMatchingAccount => "no_matching_account",
            Self::PolicyDenied => "policy_denied",
            Self::ResyncRequired => "resync_required",
            Self::Conflict => "conflict",
            Self::Internal => "internal",
            Self::Unknown => "unknown",
        }
    }

    /// The HTTP status for this code on the pairing and blob endpoints.
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Unauthorized => 401,
            Self::ProtocolMismatch | Self::Invalid => 400,
            Self::NotFound | Self::WorkerUnknown => 404,
            Self::StaleWrite | Self::Conflict | Self::ResyncRequired => 409,
            Self::PolicyDenied => 403,
            Self::WorkerOffline | Self::NoMatchingAccount => 503,
            Self::Internal | Self::Unknown => 500,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ErrorCode {
    type Err = std::convert::Infallible;

    /// Never fails. An unknown string gives [`ErrorCode::Unknown`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::ALL
            .into_iter()
            .find(|code| code.as_str() == s)
            .unwrap_or(Self::Unknown))
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        Ok(s.parse().unwrap_or(Self::Unknown))
    }
}

/// The `error` object of an error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ErrorBody {
    /// What kind of error this is.
    pub code: ErrorCode,
    /// A sentence for logs and for the user. For `invalid`, it names the
    /// field. For `policy_denied`, it names the rule.
    pub message: String,
}

impl ErrorBody {
    /// Makes an error body.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// An `invalid` error.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Invalid, message)
    }

    /// A `not_found` error.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// A `conflict` error.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    /// An `unauthorized` error.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unauthorized, message)
    }

    /// An `internal` error. Do not put internal details in the message.
    pub fn internal() -> Self {
        Self::new(
            ErrorCode::Internal,
            "Something went wrong on the hub. Try again.",
        )
    }
}

/// The JSON body of an HTTP error response: `{"error": {"code", "message"}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpErrorBody {
    /// The error.
    pub error: ErrorBody,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_as_snake_case() {
        for code in ErrorCode::ALL {
            let json = serde_json::to_string(&code).unwrap();
            assert_eq!(json, format!("\"{}\"", code.as_str()));
            assert_eq!(serde_json::from_str::<ErrorCode>(&json).unwrap(), code);
        }
        assert_eq!(
            serde_json::to_string(&ErrorCode::NoMatchingAccount).unwrap(),
            "\"no_matching_account\""
        );
    }

    #[test]
    fn unknown_codes_are_tolerated() {
        let code: ErrorCode = serde_json::from_str("\"rate_limited\"").unwrap();
        assert_eq!(code, ErrorCode::Unknown);
    }

    #[test]
    fn error_body_shape() {
        let body = ErrorBody::new(ErrorCode::WorkerOffline, "bigboi is offline.");
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({"code": "worker_offline", "message": "bigboi is offline."})
        );
        assert_eq!(body.to_string(), "worker_offline: bigboi is offline.");
    }

    #[test]
    fn http_statuses_are_errors() {
        for code in ErrorCode::ALL {
            assert!((400..600).contains(&code.http_status()));
        }
    }
}
