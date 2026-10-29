//! The error type of managed tools.

use std::io;

use crate::ToolId;

/// An error from a managed tool operation.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// A download or a request failed, or the server answered with an error
    /// status.
    #[error("could not get {url}: {message}")]
    Network { url: String, message: String },

    /// A server answered with data that Consort cannot read.
    #[error("unexpected answer from {url}: {message}")]
    InvalidResponse { url: String, message: String },

    /// A downloaded file does not have the published SHA-256.
    #[error("checksum mismatch for {name}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        name: String,
        expected: String,
        actual: String,
    },

    /// The source publishes no checksum for a file, so Consort does not
    /// install it.
    #[error("no SHA-256 checksum is published for {name}")]
    MissingChecksum { name: String },

    /// The worker runs on an OS or CPU that no tool source supports.
    #[error("this platform ({os}, {arch}) is not supported")]
    UnknownPlatform { os: String, arch: String },

    /// The tool has no build for this platform.
    #[error("{tool} has no build for {platform}")]
    UnsupportedPlatform { tool: ToolId, platform: String },

    /// The tool or this way to install it is not supported.
    #[error("cannot install {tool}: {reason}")]
    Unsupported { tool: ToolId, reason: String },

    /// The OpenPGP signature check failed.
    #[error("signature check failed: {message}")]
    Signature { message: String },

    /// An archive is damaged or uses a format that Consort cannot unpack.
    #[error("cannot unpack archive: {message}")]
    Archive { message: String },

    /// An archive entry or link points outside the install folder.
    #[error("archive entry {path} points outside the install folder")]
    UnsafePath { path: String },

    /// npm failed.
    #[error("npm failed: {message}")]
    Npm { message: String },

    /// The tool is not installed.
    #[error("{tool} is not installed")]
    NotInstalled { tool: ToolId },

    /// The ACP registry has no agent with this id.
    #[error("the ACP registry has no agent \"{id}\"")]
    UnknownAgent { id: String },

    /// A file system operation failed.
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
}

impl ToolError {
    /// Wraps an I/O error with a short description of the operation.
    #[cfg_attr(not(feature = "install"), allow(dead_code))]
    pub(crate) fn io(context: impl Into<String>) -> impl FnOnce(io::Error) -> ToolError {
        let context = context.into();
        move |source| ToolError::Io { context, source }
    }
}
