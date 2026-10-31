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
    /// A provider's sign-in flow failed. The message says why.
    SignInFailed,
    /// A worker could not install or update a managed tool. The message
    /// says why.
    ToolInstallFailed,
    /// A tag is not 1 to 32 lowercase letters, digits or `-`. The message
    /// names the tag.
    InvalidTag,
    /// The peer does not support this request yet. The message names the
    /// request type.
    Unsupported,
    /// The connection does not exist, or the session's channel cannot use
    /// it. The message names the connectors that the channel can use.
    ConnectorNotAvailable,
    /// The hub has no Composio key. The user adds one in Settings >
    /// Connectors.
    ComposioNotConfigured,
    /// Composio refused a request or did not answer. The message says why.
    ComposioError,
    /// The hub has no Monid key. The user adds one in Settings > Connectors.
    MonidNotConfigured,
    /// Monid refuses the key.
    MonidKeyRefused,
    /// Monid refused a request or did not answer. The message says why.
    MonidError,
    /// The action needs the user first, for example a connection that needs
    /// a new sign-in. The message says what the user must do.
    NeedsUser,
    /// The skill is blocked, for example because its audit failed. The
    /// message says why.
    SkillBlocked,
    /// The skill does not follow the Agent Skills format or breaks a limit.
    /// The message names the file or the limit.
    SkillInvalid,
    /// A code that this version does not know. A newer peer sent it. Treat it
    /// like [`ErrorCode::Internal`] and show the message.
    Unknown,
}

impl ErrorCode {
    /// Every known code, in the order of protocol.md.
    pub const ALL: [ErrorCode; 25] = [
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
        Self::SignInFailed,
        Self::ToolInstallFailed,
        Self::InvalidTag,
        Self::Unsupported,
        Self::ConnectorNotAvailable,
        Self::ComposioNotConfigured,
        Self::ComposioError,
        Self::MonidNotConfigured,
        Self::MonidKeyRefused,
        Self::MonidError,
        Self::NeedsUser,
        Self::SkillBlocked,
        Self::SkillInvalid,
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
            Self::SignInFailed => "sign_in_failed",
            Self::ToolInstallFailed => "tool_install_failed",
            Self::InvalidTag => "invalid_tag",
            Self::Unsupported => "unsupported",
            Self::ConnectorNotAvailable => "connector_not_available",
            Self::ComposioNotConfigured => "composio_not_configured",
            Self::ComposioError => "composio_error",
            Self::MonidNotConfigured => "monid_not_configured",
            Self::MonidKeyRefused => "monid_key_refused",
            Self::MonidError => "monid_error",
            Self::NeedsUser => "needs_user",
            Self::SkillBlocked => "skill_blocked",
            Self::SkillInvalid => "skill_invalid",
            Self::Unknown => "unknown",
        }
    }

    /// The HTTP status for this code on the pairing and blob endpoints.
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Unauthorized => 401,
            Self::ProtocolMismatch | Self::Invalid | Self::InvalidTag | Self::SkillInvalid => 400,
            Self::NotFound | Self::WorkerUnknown | Self::ConnectorNotAvailable => 404,
            Self::StaleWrite | Self::Conflict | Self::ResyncRequired | Self::NeedsUser => 409,
            Self::PolicyDenied | Self::SkillBlocked => 403,
            Self::WorkerOffline
            | Self::NoMatchingAccount
            | Self::ComposioNotConfigured
            | Self::MonidNotConfigured => 503,
            Self::SignInFailed
            | Self::ToolInstallFailed
            | Self::ComposioError
            | Self::MonidKeyRefused
            | Self::MonidError => 502,
            Self::Unsupported => 501,
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

    /// An `unsupported` error for a request type that this peer accepts but
    /// does not carry out yet, for example `account.create`.
    pub fn not_built(request_type: &str) -> Self {
        Self::new(
            ErrorCode::Unsupported,
            format!(
                "Not built yet: {request_type}. Update the hub and the worker, then try again."
            ),
        )
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
    fn new_codes_have_their_wire_strings() {
        for (code, text) in [
            (ErrorCode::SignInFailed, "sign_in_failed"),
            (ErrorCode::ToolInstallFailed, "tool_install_failed"),
            (ErrorCode::InvalidTag, "invalid_tag"),
            (ErrorCode::Unsupported, "unsupported"),
        ] {
            assert_eq!(serde_json::to_value(code).unwrap(), text);
            assert_eq!(text.parse::<ErrorCode>().unwrap(), code);
        }
        let body = ErrorBody::not_built("account.create");
        assert_eq!(body.code, ErrorCode::Unsupported);
        assert!(body.message.starts_with("Not built yet: account.create."));
    }

    #[test]
    fn connector_and_skill_codes_have_their_wire_strings() {
        for (code, text, status) in [
            (
                ErrorCode::ConnectorNotAvailable,
                "connector_not_available",
                404,
            ),
            (
                ErrorCode::ComposioNotConfigured,
                "composio_not_configured",
                503,
            ),
            (ErrorCode::ComposioError, "composio_error", 502),
            (ErrorCode::NeedsUser, "needs_user", 409),
            (ErrorCode::SkillBlocked, "skill_blocked", 403),
            (ErrorCode::SkillInvalid, "skill_invalid", 400),
        ] {
            assert_eq!(serde_json::to_value(code).unwrap(), text);
            assert_eq!(text.parse::<ErrorCode>().unwrap(), code);
            assert_eq!(code.http_status(), status);
            assert!(ErrorCode::ALL.contains(&code));
            let body = ErrorBody::new(code, "The user must reconnect GitHub Work.");
            let back: ErrorBody =
                serde_json::from_value(serde_json::to_value(&body).unwrap()).unwrap();
            assert_eq!(back, body);
        }
    }

    #[test]
    fn http_statuses_are_errors() {
        for code in ErrorCode::ALL {
            assert!((400..600).contains(&code.http_status()));
        }
    }
}
