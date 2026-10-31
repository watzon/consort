//! Worker errors.

use consort_proto::error::{ErrorBody, ErrorCode};

/// The errors of the execution worker.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// A file could not be read or written.
    #[error("{context}: {source}")]
    Io {
        /// What the worker was doing.
        context: String,
        /// The error.
        #[source]
        source: std::io::Error,
    },
    /// The configuration of the worker is not valid.
    #[error("the worker configuration is not valid: {0}")]
    Config(String),
    /// Another worker runs with the same config folder.
    #[error(
        "another worker runs with the folder {0}. Stop that worker first, or give this worker its own folder with --config-dir"
    )]
    AlreadyRunning(String),
    /// The worker is not paired yet.
    #[error("this worker is not paired. Run `consort worker pair <hub-url> --code <code>` first")]
    NotPaired,
    /// The worker database failed.
    #[error("the worker database failed: {0}")]
    Store(#[from] rusqlite::Error),
    /// The link to the hub failed.
    #[error(transparent)]
    Link(#[from] consort_link::LinkError),
    /// The worker policy does not allow this.
    #[error("{0}")]
    Policy(String),
    /// A workspace could not be made.
    #[error("{0}")]
    Workspace(String),
    /// The hub could not be reached.
    #[error("could not reach the hub: {0}")]
    Network(String),
}

impl WorkerError {
    /// An I/O error with context.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    /// The error body to send to the hub.
    pub fn to_error_body(&self) -> ErrorBody {
        match self {
            Self::Policy(message) => ErrorBody::new(ErrorCode::PolicyDenied, message.clone()),
            Self::Workspace(message) => ErrorBody::new(ErrorCode::Invalid, message.clone()),
            other => ErrorBody::new(ErrorCode::Internal, other.to_string()),
        }
    }
}
