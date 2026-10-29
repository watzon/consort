/// Why a client action failed. Each message says what happened and what to
/// do next, so a client can show it as it is.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ClientError {
    #[error("Consort is not connected to the hub. Wait until it reconnects, then try again")]
    NotConnected,
    #[error(
        "Consort has not loaded a channel yet. Wait until it connects to the hub, then try again"
    )]
    NoChannel,
    #[error("The hub URL is not valid: {0}. Enter a URL such as https://hub.example.com")]
    InvalidUrl(String),
    #[error("Cannot reach the hub at {url}. Check the URL and that the hub runs, then try again")]
    Unreachable { url: String },
    #[error("The hub did not accept the setup code. Check the code, or make a new one on the hub")]
    BadCode,
    #[error("{message}")]
    Hub { code: String, message: String },
    #[error(
        "Cannot read or write the client file {path}: {message}. Check that the folder exists and that you may use it, then try again"
    )]
    Storage { path: String, message: String },
    #[error("The hub did not answer in time. Check the connection, then try again")]
    TimedOut,
    #[error("The client stopped. Restart Consort")]
    Stopped,
    #[error("{0}")]
    Other(String),
}
