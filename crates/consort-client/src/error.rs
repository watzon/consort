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
    /// A self-signed hub, and no pin from a code or a link.
    #[error(
        "The hub at {url} has its own certificate, so the address alone is not enough. Scan the code in the app on the hub, or paste its pairing link"
    )]
    UntrustedHub { url: String },
    /// The hub showed another certificate than the pin.
    #[error(
        "The hub at {url} did not show the certificate of this pairing link. Someone may be in the way, or the hub has a new certificate. Make a new code on the hub, then try again"
    )]
    PinMismatch { url: String },
    /// The hub and this app have no common protocol version.
    #[error("{title}. {detail}")]
    Incompatible { title: String, detail: String },
    #[error("{message}")]
    Hub { code: String, message: String },
    /// The hub cannot do this now, and can later. A visual that cannot be
    /// drawn because no session host is online, or because the browser for
    /// visuals is still installing, gives this. The message says to try
    /// again. The app may try again by itself after a while.
    #[error("{message}")]
    Unavailable { message: String },
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
