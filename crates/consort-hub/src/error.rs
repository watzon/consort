//! One error type for the whole crate.
//!
//! Handlers return [`Result`]. The transport layer turns a [`HubError`] into
//! the `{code, message}` body of an error response with
//! [`HubError::to_error_body`]. Only [`HubError::Api`] reaches a peer as is.
//! Every other variant is logged and shows as `internal`.

use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::model::ValidationError;
use consort_proto::status::TransitionError;

pub type Result<T> = std::result::Result<T, HubError>;

#[derive(Debug, thiserror::Error)]
pub enum HubError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// An error that a peer may see.
    #[error("{0}")]
    Api(#[from] ErrorBody),
    #[error("the hub could not start: {0}")]
    Startup(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl HubError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Api(ErrorBody::new(code, message))
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Api(ErrorBody::invalid(message))
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::Api(ErrorBody::not_found(message))
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Api(ErrorBody::conflict(message))
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Api(ErrorBody::unauthorized(message))
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    /// The body that a peer sees. Non-API errors are logged and hidden.
    pub fn to_error_body(&self) -> ErrorBody {
        match self {
            Self::Api(body) => body.clone(),
            other => {
                tracing::error!(error = %other, "request failed with an internal error");
                ErrorBody::internal()
            }
        }
    }

    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Api(body) => body.code,
            _ => ErrorCode::Internal,
        }
    }
}

impl From<ValidationError> for HubError {
    fn from(err: ValidationError) -> Self {
        Self::Api(err.into())
    }
}

impl From<consort_proto::model::InvalidTag> for HubError {
    fn from(err: consort_proto::model::InvalidTag) -> Self {
        Self::Api(err.into())
    }
}

impl From<TransitionError> for HubError {
    fn from(err: TransitionError) -> Self {
        Self::Api(err.into())
    }
}

impl From<consort_proto::frame::FrameError> for HubError {
    fn from(err: consort_proto::frame::FrameError) -> Self {
        Self::Api(err.to_error_body())
    }
}

impl From<consort_proto::ids::IdError> for HubError {
    fn from(err: consort_proto::ids::IdError) -> Self {
        Self::invalid(err.to_string())
    }
}

impl From<tokio::task::JoinError> for HubError {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::Internal(format!("a background task failed: {err}"))
    }
}
