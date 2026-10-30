//! Pairing, authentication and `hello`.
//!
//! This module has the data shapes and the exact bytes to sign. It does not
//! do cryptography: the hub, the worker and the client sign and verify with
//! an Ed25519 crate. Keys, signatures and nonces travel as lowercase hex.
//!
//! The connect sequence on both links:
//!
//! 1. The peer opens [`WORKER_WS_PATH`] or [`CLIENT_WS_PATH`].
//! 2. The hub sends the event [`AuthChallenge`].
//! 3. The peer sends the request [`AuthResponse`], with a signature over
//!    [`auth_signing_payload`]. The hub answers `ok` with an empty body, or
//!    `unauthorized` and closes.
//! 4. The peer sends `hello` ([`WorkerHello`] or [`ClientHello`]).

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::frame::{Empty, Message, Request};
use crate::ids::{ClientId, SessionId, WorkerId};
use crate::model::{ClientKind, Os, SessionState};
use crate::time::Timestamp;

/// The WebSocket path for workers.
pub const WORKER_WS_PATH: &str = "/v1/worker";
/// The WebSocket path for clients.
pub const CLIENT_WS_PATH: &str = "/v1/client";
/// The HTTP path to pair a worker.
pub const PAIR_WORKER_PATH: &str = "/v1/pair/worker";
/// The HTTP path to pair a client.
pub const PAIR_CLIENT_PATH: &str = "/v1/pair/client";
/// How long a pairing code is valid, in milliseconds (10 minutes).
pub const PAIRING_CODE_TTL_MS: i64 = 10 * 60 * 1_000;
/// How long a token from `auth.http_token` is valid, in milliseconds (10 minutes).
pub const HTTP_TOKEN_TTL_MS: i64 = 10 * 60 * 1_000;

/// The HTTP path of a blob.
pub fn blob_path(sha256: &crate::model::Sha256Hash) -> String {
    format!("/v1/blobs/{sha256}")
}

/// The error when a hex value has the wrong length or characters.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("expected {expected} hex characters for {what}")]
pub struct HexValueError {
    /// What the value is, for example `an Ed25519 public key`.
    pub what: &'static str,
    /// The number of hex characters that the value needs.
    pub expected: usize,
}

macro_rules! hex_bytes {
    ($(#[$meta:meta])* $name:ident, $len:literal, $what:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name([u8; $len]);

        impl $name {
            /// Wraps raw bytes.
            pub const fn from_bytes(bytes: [u8; $len]) -> Self {
                Self(bytes)
            }

            /// The raw bytes.
            pub const fn to_bytes(&self) -> [u8; $len] {
                self.0
            }

            /// The raw bytes, borrowed.
            pub const fn as_bytes(&self) -> &[u8; $len] {
                &self.0
            }

            /// Lowercase hex.
            pub fn to_hex(&self) -> String {
                crate::hex::encode(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = HexValueError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                crate::hex::decode_array::<$len>(s)
                    .map(Self)
                    .ok_or(HexValueError { what: $what, expected: $len * 2 })
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.to_hex())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.to_hex())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.to_hex())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = Cow::<'de, str>::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

hex_bytes!(
    /// An Ed25519 public key (32 bytes, 64 hex characters).
    PublicKey, 32, "an Ed25519 public key"
);
hex_bytes!(
    /// An Ed25519 signature (64 bytes, 128 hex characters).
    Signature, 64, "an Ed25519 signature"
);
hex_bytes!(
    /// The 32 random bytes of an `auth.challenge` (64 hex characters).
    Nonce, 32, "a 32-byte nonce"
);

/// A pairing code: 8 characters from the Crockford base32 alphabet, shown as
/// `ABCD-EFGH`. Single use and valid for 10 minutes.
///
/// Parsing ignores case, hyphens and spaces, and reads `O` as `0` and `I`
/// or `L` as `1`, so a code that the user types still matches.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PairingCode([u8; 8]);

/// The alphabet of pairing codes: Crockford base32.
pub const PAIRING_CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The error when a string is not a pairing code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a pairing code has 8 letters and digits, for example ABCD-EFGH")]
pub struct PairingCodeError;

impl PairingCode {
    /// Makes a code from 8 random bytes. The hub gives the bytes from a
    /// secure random source. Each byte picks one of 32 characters, so no
    /// character is more likely than another.
    pub fn from_random_bytes(bytes: [u8; 8]) -> Self {
        Self(bytes.map(|b| PAIRING_CODE_ALPHABET[usize::from(b & 31)]))
    }

    /// The code without the hyphen, for storage and comparison.
    pub fn canonical(&self) -> String {
        self.0.iter().map(|b| char::from(*b)).collect()
    }
}

impl FromStr for PairingCode {
    type Err = PairingCodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut out = [0_u8; 8];
        let mut n = 0;
        for c in s.chars() {
            if c == '-' || c.is_whitespace() {
                continue;
            }
            let upper = match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                other => other,
            };
            let byte = u8::try_from(upper).map_err(|_| PairingCodeError)?;
            if !PAIRING_CODE_ALPHABET.contains(&byte) || n == 8 {
                return Err(PairingCodeError);
            }
            out[n] = byte;
            n += 1;
        }
        if n == 8 {
            Ok(Self(out))
        } else {
            Err(PairingCodeError)
        }
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.canonical();
        write!(f, "{}-{}", &s[..4], &s[4..])
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PairingCode({self})")
    }
}

impl Serialize for PairingCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PairingCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// The body of `POST /v1/pair/worker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairWorkerRequest {
    /// The pairing code from a client.
    pub code: PairingCode,
    /// The worker's new Ed25519 public key.
    pub public_key: PublicKey,
    /// The hostname.
    pub hostname: String,
    /// The operating system.
    pub os: Os,
    /// The CPU architecture.
    pub arch: String,
    /// The name from `worker.toml`. Empty means the hostname.
    pub name: Option<String>,
}

/// The `200` body of `POST /v1/pair/worker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairWorkerResponse {
    /// The new worker ID. The worker writes it to `worker.toml`.
    pub worker_id: WorkerId,
}

/// The body of `POST /v1/pair/client`. The first client uses the setup code
/// that the hub prints on its first start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairClientRequest {
    /// The pairing code or the setup code.
    pub code: PairingCode,
    /// The client's new Ed25519 public key.
    pub public_key: PublicKey,
    /// The name, for example "MacBook Pro".
    pub name: String,
    /// The kind of client.
    pub kind: ClientKind,
    /// The operating system of the client. An older client leaves it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<Os>,
}

/// The `200` body of `POST /v1/pair/client`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairClientResponse {
    /// The new client ID.
    pub client_id: ClientId,
}

/// What a paired client shows as a QR code so that a new client can pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingInvite {
    /// The hub URL.
    pub hub_url: String,
    /// A new pairing code.
    pub code: PairingCode,
}

/// The first frame on a link: an event from the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthChallenge {
    /// 32 random bytes.
    pub nonce: Nonce,
}

impl Message for AuthChallenge {
    const TYPE: &'static str = "auth.challenge";
}

/// The ID that a peer proves: a worker ID or a client ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerId {
    /// A worker.
    Worker(WorkerId),
    /// A client.
    Client(ClientId),
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Worker(id) => id.fmt(f),
            Self::Client(id) => id.fmt(f),
        }
    }
}

impl Serialize for PeerId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PeerId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        if s.starts_with(WorkerId::PREFIX) {
            s.parse()
                .map(Self::Worker)
                .map_err(serde::de::Error::custom)
        } else {
            s.parse()
                .map(Self::Client)
                .map_err(serde::de::Error::custom)
        }
    }
}

/// The peer's answer to [`AuthChallenge`]: a request. The hub answers `ok`
/// with an empty body, or `unauthorized`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthResponse {
    /// The worker ID or client ID.
    pub peer_id: PeerId,
    /// The Ed25519 signature over [`auth_signing_payload`].
    pub signature: Signature,
}

impl Message for AuthResponse {
    const TYPE: &'static str = "auth.response";
}

impl Request for AuthResponse {
    type Response = Empty;
}

/// The error when a URL has no usable origin.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not an http, https, ws or wss URL with a host")]
pub struct OriginError(pub String);

/// The origin of a hub URL, as both sides sign it: `scheme://host[:port]`.
///
/// The scheme and host are lowercase. `ws` becomes `http` and `wss` becomes
/// `https`. A default port (80 for http, 443 for https) is dropped. The path,
/// query and fragment are dropped. A URL with user info is refused.
///
/// The peer uses the hub URL from its own configuration. The hub uses its
/// configured public URL. A peer that reaches the hub through another name
/// fails to authenticate, which is the point of signing the origin.
pub fn normalize_origin(url: &str) -> Result<String, OriginError> {
    let err = || OriginError(url.to_owned());
    let (scheme, rest) = url.trim().split_once("://").ok_or_else(err)?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "http" | "ws" => "http",
        "https" | "wss" => "https",
        _ => return Err(err()),
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|a| !a.is_empty())
        .ok_or_else(err)?;
    if authority.contains('@') {
        return Err(err());
    }
    let (host, port) = if let Some(after_bracket) = authority.strip_prefix('[') {
        let (inside, after) = after_bracket.split_once(']').ok_or_else(err)?;
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':').ok_or_else(err)?),
        };
        (format!("[{}]", inside.to_ascii_lowercase()), port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_ascii_lowercase(), Some(port)),
            None => (authority.to_ascii_lowercase(), None),
        }
    };
    if host.is_empty() || host == "[]" {
        return Err(err());
    }
    let port = match port {
        None => None,
        Some(p) => Some(p.parse::<u16>().map_err(|_| err())?),
    };
    let default_port = if scheme == "http" { 80 } else { 443 };
    Ok(match port {
        Some(p) if p != default_port => format!("{scheme}://{host}:{p}"),
        _ => format!("{scheme}://{host}"),
    })
}

/// The bytes that a peer signs to answer an `auth.challenge`:
/// `consort-auth-v1\n<origin>\n<nonce hex>` in UTF-8. `origin` must come
/// from [`normalize_origin`].
pub fn auth_signing_payload(origin: &str, nonce: &Nonce) -> Vec<u8> {
    format!("consort-auth-v1\n{origin}\n{}", nonce.to_hex()).into_bytes()
}

/// The header that names the worker on a signed HTTP request.
pub const HEADER_WORKER_ID: &str = "x-consort-worker";
/// The header with the request time in Unix milliseconds.
pub const HEADER_TIMESTAMP: &str = "x-consort-timestamp";
/// The header with the hex Ed25519 signature over [`http_signing_payload`].
pub const HEADER_SIGNATURE: &str = "x-consort-signature";
/// How far the timestamp of a signed HTTP request may be from the hub's
/// clock, in milliseconds (5 minutes).
pub const HTTP_SIGNATURE_MAX_SKEW_MS: i64 = 5 * 60 * 1_000;

/// The bytes that a worker signs for a blob request:
/// `consort-http-v1\n<METHOD>\n<origin>\n<path>\n<timestamp ms>\n<worker id>`.
/// `method` is uppercased. `path` is the request path without the query, for
/// example `/v1/blobs/<sha256>`.
pub fn http_signing_payload(
    method: &str,
    origin: &str,
    path: &str,
    timestamp: Timestamp,
    worker_id: WorkerId,
) -> Vec<u8> {
    format!(
        "consort-http-v1\n{}\n{origin}\n{path}\n{}\n{worker_id}",
        method.to_ascii_uppercase(),
        timestamp.as_millis()
    )
    .into_bytes()
}

/// One open session in a worker's `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenSession {
    /// The session.
    pub session_id: SessionId,
    /// The highest event sequence number in the worker's spool.
    pub last_seq: u64,
    /// The session state on the worker.
    pub state: SessionState,
}

/// A session and the highest contiguous event sequence number that the hub
/// stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSeq {
    /// The session.
    pub session_id: SessionId,
    /// The highest contiguous `seq` that the hub stored. 0 means none.
    pub last_seq: u64,
}

/// `hello` from a worker, after authentication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerHello {
    /// The worker ID.
    pub worker_id: WorkerId,
    /// The worker's software version.
    pub version: String,
    /// The protocol versions that the worker supports.
    pub protocol_versions: Vec<u32>,
    /// The sessions that are open on the worker.
    #[serde(default)]
    pub sessions: Vec<OpenSession>,
}

/// The response to [`WorkerHello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerHelloResult {
    /// The protocol version that the hub picked.
    pub protocol_version: u32,
    /// The hub's software version.
    pub hub_version: String,
    /// The hub's stored `last_seq` for each session in the hello that the
    /// hub knows. The worker sends every event after that number. The hub
    /// sends `session.stop` for a session that it does not know.
    #[serde(default)]
    pub sessions: Vec<SessionSeq>,
}

/// `hello` from a client, after authentication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    /// The client ID.
    pub client_id: ClientId,
    /// The client's software version.
    pub version: String,
    /// The protocol versions that the client supports.
    pub protocol_versions: Vec<u32>,
}

/// The response to [`ClientHello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHelloResult {
    /// The protocol version that the hub picked.
    pub protocol_version: u32,
    /// The hub's software version.
    pub hub_version: String,
    /// The current feed sequence number.
    pub feed_seq: u64,
}

/// `auth.http_token`: ask for a bearer token for the blob endpoints.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthHttpToken {}

/// The response to [`AuthHttpToken`]. Send it as `Authorization: Bearer <token>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpToken {
    /// The bearer token.
    pub token: String,
    /// When the token stops working, 10 minutes after it was made.
    pub expires_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_values() {
        let key = PublicKey::from_bytes([7; 32]);
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, format!("\"{}\"", "07".repeat(32)));
        assert_eq!(serde_json::from_str::<PublicKey>(&json).unwrap(), key);
        assert!(serde_json::from_str::<PublicKey>("\"0707\"").is_err());
        let sig: Signature = "AB".repeat(64).parse().unwrap();
        assert_eq!(sig.to_bytes(), [0xab; 64]);
        assert!("ab".repeat(32).parse::<Signature>().is_err());
    }

    #[test]
    fn pairing_codes() {
        let code = PairingCode::from_random_bytes([0, 1, 2, 3, 28, 29, 30, 255]);
        assert_eq!(code.to_string(), "0123-WXYZ");
        assert_eq!(code.canonical(), "0123WXYZ");
        let typed: PairingCode = "o123 wxyz".parse().unwrap();
        assert_eq!(typed, code);
        let with_i: PairingCode = "OIL3-WXYZ".parse().unwrap();
        assert_eq!(with_i.canonical(), "0113WXYZ");
        assert!("ABCD-EFG".parse::<PairingCode>().is_err());
        assert!("ABCD-EFGHJ".parse::<PairingCode>().is_err());
        assert!("ABCD-EFGU".parse::<PairingCode>().is_err());
        assert!("ABCD-EFGé".parse::<PairingCode>().is_err());
        assert_eq!(serde_json::to_string(&code).unwrap(), "\"0123-WXYZ\"");
    }

    #[test]
    fn origins() {
        let cases = [
            ("https://hub.example.com", "https://hub.example.com"),
            (
                "https://Hub.Example.com:443/v1/client",
                "https://hub.example.com",
            ),
            ("wss://hub.example.com/v1/worker", "https://hub.example.com"),
            ("ws://127.0.0.1:7600/v1/worker", "http://127.0.0.1:7600"),
            ("http://localhost:80", "http://localhost"),
            ("HTTP://[::1]:7600/", "http://[::1]:7600"),
            ("https://hub.example.com?x=1", "https://hub.example.com"),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_origin(input).unwrap(), expected, "{input}");
        }
        for bad in [
            "hub.example.com",
            "ftp://hub.example.com",
            "https://",
            "https://user@hub.example.com",
            "https://hub.example.com:99999",
            "https://[::1",
        ] {
            assert!(normalize_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn signing_payloads_are_exact() {
        let nonce = Nonce::from_bytes([0xab; 32]);
        let payload = auth_signing_payload("https://hub.example.com", &nonce);
        assert_eq!(
            String::from_utf8(payload).unwrap(),
            format!(
                "consort-auth-v1\nhttps://hub.example.com\n{}",
                "ab".repeat(32)
            )
        );
        let worker: WorkerId = "wrk_00000000-0000-0000-0000-000000000001".parse().unwrap();
        let payload = http_signing_payload(
            "put",
            "https://hub.example.com",
            "/v1/blobs/abc",
            Timestamp::from_millis(42),
            worker,
        );
        assert_eq!(
            String::from_utf8(payload).unwrap(),
            "consort-http-v1\nPUT\nhttps://hub.example.com\n/v1/blobs/abc\n42\nwrk_00000000-0000-0000-0000-000000000001"
        );
    }

    #[test]
    fn peer_ids() {
        let w = PeerId::Worker(WorkerId::new());
        let c = PeerId::Client(ClientId::new());
        for id in [w, c] {
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(serde_json::from_str::<PeerId>(&json).unwrap(), id);
        }
        assert!(
            serde_json::from_str::<PeerId>("\"tsk_00000000-0000-0000-0000-000000000001\"").is_err()
        );
    }

    #[test]
    fn auth_and_hello_frames() {
        use crate::frame::{Frame, FrameId};
        use serde_json::{Value, json};

        let challenge = Frame::event(&AuthChallenge {
            nonce: Nonce::from_bytes([1; 32]),
        })
        .unwrap();
        let value: Value = serde_json::from_str(&challenge.to_text().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({"v": 1, "type": "auth.challenge", "body": {"nonce": "01".repeat(32)}})
        );

        let worker: WorkerId = "wrk_00000000-0000-0000-0000-000000000001".parse().unwrap();
        let response = Frame::request(
            FrameId::new("a1"),
            &AuthResponse {
                peer_id: PeerId::Worker(worker),
                signature: Signature::from_bytes([2; 64]),
            },
        )
        .unwrap();
        let value: Value = serde_json::from_str(&response.to_text().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({"v": 1, "id": "a1", "type": "auth.response",
                   "body": {"peer_id": "wrk_00000000-0000-0000-0000-000000000001",
                            "signature": "02".repeat(64)}})
        );

        let hello = Frame::request(
            FrameId::new("h1"),
            &WorkerHello {
                worker_id: worker,
                version: "0.1.0".into(),
                protocol_versions: vec![1],
                sessions: vec![],
            },
        )
        .unwrap();
        let value: Value = serde_json::from_str(&hello.to_text().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({"v": 1, "id": "h1", "type": "hello",
                   "body": {"worker_id": "wrk_00000000-0000-0000-0000-000000000001",
                            "version": "0.1.0", "protocol_versions": [1], "sessions": []}})
        );
        let reply = WorkerHelloResult {
            protocol_version: 1,
            hub_version: "0.1.0".into(),
            sessions: vec![SessionSeq {
                session_id: "ses_00000000-0000-0000-0000-000000000002".parse().unwrap(),
                last_seq: 12,
            }],
        };
        assert_eq!(
            serde_json::to_value(&reply).unwrap(),
            json!({"protocol_version": 1, "hub_version": "0.1.0",
                   "sessions": [{"session_id": "ses_00000000-0000-0000-0000-000000000002", "last_seq": 12}]})
        );
    }

    #[test]
    fn pairing_bodies() {
        let req: PairWorkerRequest = serde_json::from_value(serde_json::json!({
            "code": "ABCD-EFGH",
            "public_key": "11".repeat(32),
            "hostname": "bigboi",
            "os": "windows",
            "arch": "x86_64"
        }))
        .unwrap();
        assert_eq!(req.name, None);
        assert_eq!(req.code.canonical(), "ABCDEFGH");
    }
}
