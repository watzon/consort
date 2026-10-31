//! Worker errors.

use consort_proto::error::{ErrorBody, ErrorCode};

/// The errors of the session host.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// A file could not be read or written.
    #[error("{context}: {source}")]
    Io {
        /// What the worker was doing.
        context: String,
        /// The error.
        #[source]
        source: std::io::Error,
    },
    /// `session-host.toml` is not valid.
    #[error("session-host.toml is not valid: {0}")]
    Config(String),
    /// Another worker runs with the same config folder.
    #[error(
        "another worker runs with the folder {0}. Stop that worker first, or give this worker its own folder with --config-dir"
    )]
    AlreadyRunning(String),
    /// The worker is not paired yet.
    #[error("this worker is not paired. Run `consort worker pair <hub-url> --code <code>` first")]
    NotPaired,
    /// The key file is not valid.
    #[error("the worker key is not valid: {0}")]
    Key(String),
    /// The worker database failed.
    #[error("the worker database failed: {0}")]
    Store(#[from] rusqlite::Error),
    /// The hub refused or failed a request.
    #[error("the hub answered {}: {message}", code.as_str())]
    Hub {
        /// The error code.
        code: ErrorCode,
        /// The message.
        message: String,
    },
    /// The hub could not be reached.
    #[error("could not reach the hub: {0}")]
    Network(String),
    /// The worker is not connected to the hub.
    #[error("the worker is not connected to the hub")]
    Offline,
    /// The worker policy does not allow this.
    #[error("{0}")]
    Policy(String),
    /// A workspace could not be made.
    #[error("{0}")]
    Workspace(String),
    /// A harness failed.
    #[error(transparent)]
    Harness(#[from] consort_harness::HarnessError),
    /// The worker is shutting down.
    #[error("the worker is shutting down")]
    ShuttingDown,
    /// A frame is larger than the 1 MiB frame limit.
    #[error("the frame is too large to send: {0}")]
    TooLarge(String),
}

impl From<consort_link::LinkError> for HostError {
    fn from(err: consort_link::LinkError) -> Self {
        use consort_link::LinkError as L;
        match err {
            L::Io { context, source } => Self::Io { context, source },
            L::Config(message) => Self::Config(message),
            L::Key(message) => Self::Key(message),
            L::Hub { code, message } => Self::Hub { code, message },
            L::Network(message) => Self::Network(message),
            L::Offline => Self::Offline,
            L::TooLarge(message) => Self::TooLarge(message),
        }
    }
}

impl HostError {
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
            Self::Hub { code, message } => ErrorBody::new(*code, message.clone()),
            Self::Harness(consort_harness::HarnessError::Unsupported(what)) => ErrorBody::new(
                ErrorCode::PolicyDenied,
                format!("this harness session cannot {what}"),
            ),
            Self::Harness(consort_harness::HarnessError::UnknownRequest(id)) => ErrorBody::new(
                ErrorCode::NotFound,
                format!("no open harness request has the ID `{id}`"),
            ),
            Self::Harness(consort_harness::HarnessError::InvalidAnswer(m)) => {
                ErrorBody::new(ErrorCode::Invalid, m.clone())
            }
            Self::Workspace(message) => ErrorBody::new(ErrorCode::Invalid, message.clone()),
            other => ErrorBody::new(ErrorCode::Internal, other.to_string()),
        }
    }
}
