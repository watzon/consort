//! The errors of the link to the hub.

use consort_proto::error::ErrorCode;

/// The errors of the link to the hub.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// A file could not be read or written.
    #[error("{context}: {source}")]
    Io {
        /// What the machine was doing.
        context: String,
        /// The error.
        #[source]
        source: std::io::Error,
    },
    /// A setting is not valid.
    #[error("{0}")]
    Config(String),
    /// The key file is not valid.
    #[error("the machine key is not valid: {0}")]
    Key(String),
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
    /// The machine is not connected to the hub.
    #[error("this machine is not connected to the hub")]
    Offline,
    /// A frame is larger than the 1 MiB frame limit.
    #[error("the frame is too large to send: {0}")]
    TooLarge(String),
}

impl LinkError {
    /// An I/O error with context.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}
