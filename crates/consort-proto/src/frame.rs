//! The JSON frame envelope.
//!
//! Each WebSocket text frame holds one JSON object:
//!
//! - A request: `{"v": 1, "id": "...", "type": "task.create", "body": {...}}`
//! - An event (no response): `{"v": 1, "type": "feed", "body": {...}}`
//! - A response: `{"v": 1, "re": "...", "ok": true, "body": {...}}` or
//!   `{"v": 1, "re": "...", "ok": false, "error": {"code": "...", "message": "..."}}`
//!
//! [`Frame`] is the untyped envelope. Typed bodies implement [`Message`], and
//! the ones that expect a response implement [`Request`]. Each link has
//! enums of its messages ([`crate::client_api::ClientRequest`],
//! [`crate::session_host_link::SessionHostToHub`], [`crate::session_host_link::HubToSessionHost`])
//! that implement [`MessageFamily`], so a receiver can decode any frame of
//! that link with [`RequestFrame::decode`] or [`EventFrame::decode`].
//!
//! ```
//! use consort_proto::frame::{Frame, FrameId};
//! use consort_proto::client_api::{ClientRequest, TaskResolve};
//! use consort_proto::ids::TaskId;
//!
//! let task_id = TaskId::new();
//! let frame = Frame::request(FrameId::generate(), &TaskResolve { task_id }).unwrap();
//! let text = frame.to_text().unwrap();
//!
//! let Frame::Request(request) = Frame::parse(&text).unwrap() else { unreachable!() };
//! let decoded: ClientRequest = request.decode().unwrap();
//! assert_eq!(decoded, ClientRequest::TaskResolve(TaskResolve { task_id }));
//! ```

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{ErrorBody, ErrorCode};
pub use crate::ids::FrameId;

/// The frame format version, the `v` field of every frame.
pub const FRAME_VERSION: u32 = 1;

/// The protocol version that this crate implements.
pub const PROTOCOL_VERSION: u32 = 1;

/// The protocol versions that this crate can speak, oldest first. Send this
/// in `hello`.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u32] = &[PROTOCOL_VERSION];

/// The largest frame that a peer may send, in bytes (1 MiB). A larger frame
/// is a protocol error. Large data goes through the blob endpoints.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// The largest message body, in bytes as [`json_len`] measures them. The hub
/// refuses a larger body, and a client checks it before it sends. A frame
/// with such a body stays under [`MAX_FRAME_BYTES`].
pub const MESSAGE_BODY_MAX_BYTES: usize = 256 * 1024;

const _: () = assert!(MESSAGE_BODY_MAX_BYTES < MAX_FRAME_BYTES);

/// The number of bytes that `text` takes as a JSON string, without the
/// quotes. Every size limit on text counts this way, because a frame holds
/// JSON: a quote, a backslash and the common control characters count 2
/// bytes, other control characters count 6 bytes, and every other character
/// counts its UTF-8 bytes.
pub fn json_len(text: &str) -> usize {
    text.chars()
        .map(|c| match c {
            '"' | '\\' | '\u{08}' | '\u{0C}' | '\n' | '\r' | '\t' => 2,
            c if u32::from(c) < 0x20 => 6,
            c => c.len_utf8(),
        })
        .sum()
}

/// Picks the highest version that both sides support, as the hub does in the
/// `hello` exchange. `None` means the hub closes with `protocol_mismatch`.
pub fn negotiate_version(ours: &[u32], theirs: &[u32]) -> Option<u32> {
    ours.iter().copied().filter(|v| theirs.contains(v)).max()
}

/// A typed frame body with a fixed `type` string.
pub trait Message: Serialize + DeserializeOwned {
    /// The `type` of the frame, for example `task.create`.
    const TYPE: &'static str;
}

/// A message that expects a response.
pub trait Request: Message {
    /// The body of an `ok` response.
    type Response: Serialize + DeserializeOwned;
}

/// An enum of every message that one side of a link can send.
pub trait MessageFamily: Serialize + DeserializeOwned {
    /// Every `type` string in the family.
    const TYPES: &'static [&'static str];

    /// The `type` string of this value.
    fn type_name(&self) -> &'static str;
}

/// An empty body: `{}`. Reading accepts any object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

/// The errors from parsing, building or decoding frames.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The frame is larger than [`MAX_FRAME_BYTES`].
    #[error("frame is {size} bytes, more than the limit of {MAX_FRAME_BYTES}")]
    TooLarge {
        /// The size of the frame in bytes.
        size: usize,
    },
    /// The frame is not a JSON object with the envelope fields.
    #[error("frame is not valid JSON: {0}")]
    Json(#[source] serde_json::Error),
    /// The frame has a `v` that this crate does not support.
    #[error("frame version {found} is not supported; this side supports {FRAME_VERSION}")]
    UnsupportedVersion {
        /// The `v` in the frame.
        found: u32,
    },
    /// The envelope fields do not form a request, an event or a response.
    #[error("frame is malformed: {0}")]
    Malformed(&'static str),
    /// The `type` is not in the family that the receiver expects.
    #[error("unknown message type `{0}`")]
    UnknownType(String),
    /// The `type` is not the one that the receiver expects.
    #[error("expected message type `{expected}`, got `{found}`")]
    WrongType {
        /// The type that the receiver expects.
        expected: &'static str,
        /// The type in the frame.
        found: String,
    },
    /// The body does not match the schema of its type.
    #[error("body of `{kind}` is invalid: {source}")]
    InvalidBody {
        /// The `type` of the frame.
        kind: String,
        /// The serde error, which names the field.
        #[source]
        source: serde_json::Error,
    },
    /// A body did not serialize to a JSON object with `type` and `body`.
    #[error("could not serialize the body: {0}")]
    Serialize(#[source] serde_json::Error),
}

impl FrameError {
    /// The error body to send back for this error, when the frame had an `id`.
    pub fn to_error_body(&self) -> ErrorBody {
        match self {
            Self::UnsupportedVersion { .. } => {
                ErrorBody::new(ErrorCode::ProtocolMismatch, self.to_string())
            }
            Self::Serialize(_) => ErrorBody::internal(),
            _ => ErrorBody::invalid(self.to_string()),
        }
    }
}

/// The error from reading a response.
#[derive(Debug, thiserror::Error)]
pub enum ResponseError {
    /// The peer answered with an error.
    #[error(transparent)]
    Remote(#[from] ErrorBody),
    /// The `ok` body does not match the expected response type.
    #[error("response body is invalid: {0}")]
    InvalidBody(#[source] serde_json::Error),
}

/// A request frame.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestFrame {
    /// The request ID, echoed as `re` in the response.
    pub id: FrameId,
    /// The `type`.
    pub kind: String,
    /// The `body`. An absent body reads as `{}`.
    pub body: Value,
}

/// A frame that needs no response.
#[derive(Debug, Clone, PartialEq)]
pub struct EventFrame {
    /// The `type`.
    pub kind: String,
    /// The `body`. An absent body reads as `{}`.
    pub body: Value,
}

/// A response frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ResponseFrame {
    /// The ID of the request that this answers.
    pub re: FrameId,
    /// The `body` when `ok` is true, or the `error` when it is false.
    pub result: Result<Value, ErrorBody>,
}

/// One frame on a WebSocket.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// A request, which has an `id` and expects a response.
    Request(RequestFrame),
    /// A message with a `type` and no `id`.
    Event(EventFrame),
    /// A response to a request.
    Response(ResponseFrame),
}

#[derive(Serialize, Deserialize)]
struct RawFrame {
    v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<FrameId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    re: Option<FrameId>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<ErrorBody>,
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

/// Splits a serialized family value `{"type": ..., "body": ...}` into its parts.
fn split_tagged(value: Value) -> Result<(String, Value), FrameError> {
    let Value::Object(mut map) = value else {
        return Err(FrameError::Malformed(
            "a message must serialize to an object",
        ));
    };
    let kind = match map.remove("type") {
        Some(Value::String(kind)) => kind,
        _ => return Err(FrameError::Malformed("a message needs a string `type`")),
    };
    let body = map.remove("body").unwrap_or_else(empty_object);
    Ok((kind, body))
}

fn decode_family<F: MessageFamily>(kind: &str, body: &Value) -> Result<F, FrameError> {
    if !F::TYPES.contains(&kind) {
        return Err(FrameError::UnknownType(kind.to_owned()));
    }
    let mut map = Map::new();
    map.insert("type".to_owned(), Value::String(kind.to_owned()));
    map.insert("body".to_owned(), body.clone());
    serde_json::from_value(Value::Object(map)).map_err(|source| FrameError::InvalidBody {
        kind: kind.to_owned(),
        source,
    })
}

fn decode_message<M: Message>(kind: &str, body: &Value) -> Result<M, FrameError> {
    if kind != M::TYPE {
        return Err(FrameError::WrongType {
            expected: M::TYPE,
            found: kind.to_owned(),
        });
    }
    M::deserialize(body).map_err(|source| FrameError::InvalidBody {
        kind: kind.to_owned(),
        source,
    })
}

impl Frame {
    /// Parses one WebSocket text frame.
    pub fn parse(text: &str) -> Result<Self, FrameError> {
        if text.len() > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge { size: text.len() });
        }
        let value: Value = serde_json::from_str(text).map_err(FrameError::Json)?;
        if !value.is_object() {
            return Err(FrameError::Malformed("a frame must be a JSON object"));
        }
        let raw = RawFrame::deserialize(value).map_err(FrameError::Json)?;
        if raw.v != FRAME_VERSION {
            return Err(FrameError::UnsupportedVersion { found: raw.v });
        }
        if let Some(re) = raw.re {
            let result = match raw.ok {
                Some(true) => Ok(raw.body.unwrap_or_else(empty_object)),
                Some(false) => Err(raw
                    .error
                    .ok_or(FrameError::Malformed("an error response needs `error`"))?),
                None => return Err(FrameError::Malformed("a response needs `ok`")),
            };
            return Ok(Self::Response(ResponseFrame { re, result }));
        }
        let kind = raw
            .kind
            .ok_or(FrameError::Malformed("a frame needs `type` or `re`"))?;
        let body = raw.body.unwrap_or_else(empty_object);
        Ok(match raw.id {
            Some(id) => Self::Request(RequestFrame { id, kind, body }),
            None => Self::Event(EventFrame { kind, body }),
        })
    }

    /// Serializes the frame to the text of one WebSocket frame. Fails if the
    /// text is larger than [`MAX_FRAME_BYTES`].
    pub fn to_text(&self) -> Result<String, FrameError> {
        let raw = match self {
            Self::Request(r) => RawFrame {
                v: FRAME_VERSION,
                id: Some(r.id.clone()),
                re: None,
                kind: Some(r.kind.clone()),
                ok: None,
                body: Some(r.body.clone()),
                error: None,
            },
            Self::Event(e) => RawFrame {
                v: FRAME_VERSION,
                id: None,
                re: None,
                kind: Some(e.kind.clone()),
                ok: None,
                body: Some(e.body.clone()),
                error: None,
            },
            Self::Response(r) => {
                let (ok, body, error) = match &r.result {
                    Ok(body) => (true, Some(body.clone()), None),
                    Err(error) => (false, None, Some(error.clone())),
                };
                RawFrame {
                    v: FRAME_VERSION,
                    id: None,
                    re: Some(r.re.clone()),
                    kind: None,
                    ok: Some(ok),
                    body,
                    error,
                }
            }
        };
        let text = serde_json::to_string(&raw).map_err(FrameError::Serialize)?;
        if text.len() > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge { size: text.len() });
        }
        Ok(text)
    }

    /// A request frame for a typed message.
    pub fn request<M: Message>(id: FrameId, message: &M) -> Result<Self, FrameError> {
        let body = serde_json::to_value(message).map_err(FrameError::Serialize)?;
        Ok(Self::Request(RequestFrame {
            id,
            kind: M::TYPE.to_owned(),
            body,
        }))
    }

    /// An event frame for a typed message.
    pub fn event<M: Message>(message: &M) -> Result<Self, FrameError> {
        let body = serde_json::to_value(message).map_err(FrameError::Serialize)?;
        Ok(Self::Event(EventFrame {
            kind: M::TYPE.to_owned(),
            body,
        }))
    }

    /// A request frame for a value of a message family.
    pub fn family_request<F: MessageFamily>(id: FrameId, message: &F) -> Result<Self, FrameError> {
        let value = serde_json::to_value(message).map_err(FrameError::Serialize)?;
        let (kind, body) = split_tagged(value)?;
        Ok(Self::Request(RequestFrame { id, kind, body }))
    }

    /// An event frame for a value of a message family.
    pub fn family_event<F: MessageFamily>(message: &F) -> Result<Self, FrameError> {
        let value = serde_json::to_value(message).map_err(FrameError::Serialize)?;
        let (kind, body) = split_tagged(value)?;
        Ok(Self::Event(EventFrame { kind, body }))
    }

    /// An `ok` response.
    pub fn ok<T: Serialize>(re: FrameId, body: &T) -> Result<Self, FrameError> {
        let body = serde_json::to_value(body).map_err(FrameError::Serialize)?;
        Ok(Self::Response(ResponseFrame {
            re,
            result: Ok(body),
        }))
    }

    /// An error response.
    pub fn error(re: FrameId, error: ErrorBody) -> Self {
        Self::Response(ResponseFrame {
            re,
            result: Err(error),
        })
    }
}

impl RequestFrame {
    /// Decodes the request as one message of a family. An unknown `type`
    /// gives [`FrameError::UnknownType`].
    pub fn decode<F: MessageFamily>(&self) -> Result<F, FrameError> {
        decode_family(&self.kind, &self.body)
    }

    /// Decodes the request as one message type.
    pub fn decode_as<M: Message>(&self) -> Result<M, FrameError> {
        decode_message(&self.kind, &self.body)
    }

    /// An `ok` response to this request.
    pub fn respond_ok<T: Serialize>(&self, body: &T) -> Result<Frame, FrameError> {
        Frame::ok(self.id.clone(), body)
    }

    /// An error response to this request.
    pub fn respond_error(&self, error: ErrorBody) -> Frame {
        Frame::error(self.id.clone(), error)
    }
}

impl EventFrame {
    /// Decodes the event as one message of a family.
    pub fn decode<F: MessageFamily>(&self) -> Result<F, FrameError> {
        decode_family(&self.kind, &self.body)
    }

    /// Decodes the event as one message type.
    pub fn decode_as<M: Message>(&self) -> Result<M, FrameError> {
        decode_message(&self.kind, &self.body)
    }
}

impl ResponseFrame {
    /// Reads the response as the response type of request `R`.
    pub fn into_response<R: Request>(self) -> Result<R::Response, ResponseError> {
        self.into_body()
    }

    /// Reads the `ok` body as `T`, or returns the remote error.
    pub fn into_body<T: DeserializeOwned>(self) -> Result<T, ResponseError> {
        let body = self.result?;
        serde_json::from_value(body).map_err(ResponseError::InvalidBody)
    }
}

/// Defines an enum of messages with `{type, body}` adjacent tagging, and
/// implements [`Message`], [`Request`] (when a response type is given),
/// [`MessageFamily`] and `From<Body>` for each variant.
macro_rules! message_family {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$vmeta:meta])*
                $variant:ident($ty:literal, $body:ty $(=> $resp:ty)?)
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        // A value lives for one frame, so a large variant costs little.
        #[allow(clippy::large_enum_variant)]
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(tag = "type", content = "body")]
        $vis enum $name {
            $(
                $(#[$vmeta])*
                #[serde(rename = $ty)]
                $variant($body),
            )*
        }

        impl $crate::frame::MessageFamily for $name {
            const TYPES: &'static [&'static str] = &[$($ty),*];

            fn type_name(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => $ty,)*
                }
            }
        }

        $(
            impl $crate::frame::Message for $body {
                const TYPE: &'static str = $ty;
            }

            $(
                impl $crate::frame::Request for $body {
                    type Response = $resp;
                }
            )?

            impl From<$body> for $name {
                fn from(value: $body) -> Self {
                    Self::$variant(value)
                }
            }
        )*
    };
}

pub(crate) use message_family;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_len_counts_the_bytes_of_a_json_string() {
        assert_eq!(json_len(""), 0);
        assert_eq!(json_len("abc"), 3);
        // A quote, a backslash and the common control characters count 2.
        assert_eq!(json_len("a\"b\\c"), 7);
        assert_eq!(json_len("\u{08}\u{0C}\n\r\t"), 10);
        // Other control characters count 6.
        assert_eq!(json_len("\u{0}\u{1f}"), 12);
        // Other characters count their UTF-8 bytes.
        assert_eq!(json_len("\u{e9}"), 2);
        assert_eq!(json_len("\u{20ac}"), 3);
        assert_eq!(json_len("\u{1f600}"), 4);
        // The size is the length of the JSON string without its quotes.
        let text = "a\"\n\u{1}\u{e9}<\u{1f600}";
        assert_eq!(
            json_len(text) + 2,
            serde_json::to_string(text).unwrap().len()
        );
    }

    #[test]
    fn parses_the_three_shapes() {
        let req =
            Frame::parse(r#"{"v":1,"id":"01JC5","type":"task.create","body":{"a":1}}"#).unwrap();
        assert_eq!(
            req,
            Frame::Request(RequestFrame {
                id: FrameId::new("01JC5"),
                kind: "task.create".into(),
                body: json!({"a": 1}),
            })
        );

        let ok = Frame::parse(r#"{"v":1,"re":"01JC5","ok":true,"body":{}}"#).unwrap();
        assert_eq!(
            ok,
            Frame::Response(ResponseFrame {
                re: FrameId::new("01JC5"),
                result: Ok(json!({})),
            })
        );

        let err = Frame::parse(
            r#"{"v":1,"re":"01JC5","ok":false,"error":{"code":"worker_offline","message":"bigboi is offline."}}"#,
        )
        .unwrap();
        assert_eq!(
            err,
            Frame::Response(ResponseFrame {
                re: FrameId::new("01JC5"),
                result: Err(ErrorBody::new(
                    ErrorCode::WorkerOffline,
                    "bigboi is offline."
                )),
            })
        );

        let event = Frame::parse(r#"{"v":1,"type":"inventory.refresh"}"#).unwrap();
        assert_eq!(
            event,
            Frame::Event(EventFrame {
                kind: "inventory.refresh".into(),
                body: json!({}),
            })
        );
    }

    #[test]
    fn serializes_exact_envelopes() {
        let req = Frame::Request(RequestFrame {
            id: FrameId::new("01JC5"),
            kind: "task.create".into(),
            body: json!({}),
        });
        let value: Value = serde_json::from_str(&req.to_text().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({"v": 1, "id": "01JC5", "type": "task.create", "body": {}})
        );

        let ok = Frame::ok(FrameId::new("01JC5"), &Empty {}).unwrap();
        let value: Value = serde_json::from_str(&ok.to_text().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({"v": 1, "re": "01JC5", "ok": true, "body": {}})
        );

        let err = Frame::error(
            FrameId::new("01JC5"),
            ErrorBody::new(ErrorCode::WorkerOffline, "bigboi is offline."),
        );
        let value: Value = serde_json::from_str(&err.to_text().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({"v": 1, "re": "01JC5", "ok": false,
                   "error": {"code": "worker_offline", "message": "bigboi is offline."}})
        );
    }

    #[test]
    fn rejects_bad_frames() {
        assert!(matches!(
            Frame::parse(r#"{"v":2,"type":"x"}"#),
            Err(FrameError::UnsupportedVersion { found: 2 })
        ));
        assert!(matches!(
            Frame::parse(r#"{"v":1}"#),
            Err(FrameError::Malformed(_))
        ));
        assert!(matches!(
            Frame::parse(r#"{"v":1,"re":"a"}"#),
            Err(FrameError::Malformed(_))
        ));
        assert!(matches!(
            Frame::parse(r#"{"v":1,"re":"a","ok":false}"#),
            Err(FrameError::Malformed(_))
        ));
        assert!(matches!(Frame::parse("[1]"), Err(FrameError::Malformed(_))));
        assert!(matches!(Frame::parse("{nope"), Err(FrameError::Json(_))));
        assert!(matches!(
            Frame::parse(r#"{"v":"1","type":"x"}"#),
            Err(FrameError::Json(_))
        ));
        let big = format!(
            r#"{{"v":1,"type":"x","body":{{"s":"{}"}}}}"#,
            "a".repeat(MAX_FRAME_BYTES)
        );
        assert!(matches!(
            Frame::parse(&big),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn unknown_envelope_fields_are_ignored() {
        let frame = Frame::parse(r#"{"v":1,"type":"x","trace":"abc"}"#).unwrap();
        assert!(matches!(frame, Frame::Event(_)));
    }

    #[test]
    fn version_negotiation_picks_highest_common() {
        assert_eq!(negotiate_version(&[1, 2, 3], &[2, 3, 4]), Some(3));
        assert_eq!(negotiate_version(&[1], &[2]), None);
        assert_eq!(
            negotiate_version(SUPPORTED_PROTOCOL_VERSIONS, SUPPORTED_PROTOCOL_VERSIONS),
            Some(PROTOCOL_VERSION)
        );
    }

    #[test]
    fn error_mapping() {
        assert_eq!(
            FrameError::UnsupportedVersion { found: 9 }
                .to_error_body()
                .code,
            ErrorCode::ProtocolMismatch
        );
        assert_eq!(
            FrameError::UnknownType("x".into()).to_error_body().code,
            ErrorCode::Invalid
        );
    }
}
