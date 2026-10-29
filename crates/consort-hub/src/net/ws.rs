//! Shared WebSocket parts: the outbox, frame I/O and the auth handshake.

use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use consort_proto::auth::{
    AuthChallenge, AuthResponse, Nonce, PeerId, PublicKey, Signature, auth_signing_payload,
};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{Empty, Frame, FrameError, MAX_FRAME_BYTES, RequestFrame};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::hub::Hub;
use crate::util::random_bytes;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const OUTBOX_CAPACITY: usize = 4096;
/// The most connections that can be in the handshake at the same time. A
/// personal hub has a few peers, so this is far above normal use. It stops a
/// flood of sockets that never authenticate.
pub(crate) const MAX_PENDING_HANDSHAKES: usize = 64;

/// The slots for connections that have not finished the handshake.
pub struct HandshakeSlots(std::sync::Arc<tokio::sync::Semaphore>);

impl Default for HandshakeSlots {
    fn default() -> Self {
        Self(std::sync::Arc::new(tokio::sync::Semaphore::new(
            MAX_PENDING_HANDSHAKES,
        )))
    }
}

impl HandshakeSlots {
    /// A slot, or `None` when all slots are in use.
    pub(crate) fn try_take(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        self.0.clone().try_acquire_owned().ok()
    }
}

/// A queue of text frames for one connection. When the queue is full, the
/// peer reads too slowly. The hub then closes the connection, and the peer
/// reconnects and catches up from the feed or its spool.
#[derive(Clone)]
pub(crate) struct Outbox {
    tx: mpsc::Sender<String>,
    cancel: CancellationToken,
}

impl Outbox {
    pub(crate) fn new(cancel: CancellationToken) -> (Self, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel(OUTBOX_CAPACITY);
        (Self { tx, cancel }, rx)
    }

    pub(crate) fn send_text(&self, text: String) -> bool {
        match self.tx.try_send(text) {
            Ok(()) => true,
            Err(_) => {
                tracing::warn!(
                    "a peer reads too slowly or is gone, so the hub closes its connection"
                );
                self.cancel.cancel();
                false
            }
        }
    }

    pub(crate) fn send_frame(&self, frame: &Frame) -> bool {
        match frame.to_text() {
            Ok(text) => self.send_text(text),
            Err(err) => {
                tracing::warn!(error = %err, "the hub could not encode a frame");
                false
            }
        }
    }

    /// Sends the response to `request`. A response that does not fit in one
    /// frame cannot go out, so the peer gets an error response in its place
    /// and does not wait for ever.
    pub(crate) fn send_response(&self, request: &RequestFrame, response: &Frame) -> bool {
        match response.to_text() {
            Ok(text) => self.send_text(text),
            Err(FrameError::TooLarge { size }) => {
                tracing::warn!(kind = %request.kind, size, "a response is larger than the frame limit, so the hub sends an error");
                self.send_frame(&request.respond_error(too_large(size)))
            }
            Err(err) => {
                tracing::warn!(kind = %request.kind, error = %err, "the hub could not encode a response");
                self.send_frame(&request.respond_error(ErrorBody::internal()))
            }
        }
    }

    pub(crate) fn close(&self) {
        self.cancel.cancel();
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.cancel.is_cancelled() || self.tx.is_closed()
    }
}

/// The error for an answer that does not fit in one frame.
pub(crate) fn too_large(size: usize) -> ErrorBody {
    ErrorBody::new(
        ErrorCode::Internal,
        format!(
            "The answer has {} KiB, and a frame can have at most {} KiB, so the hub cannot send it. Ask for less, for example with a smaller limit.",
            size / 1024,
            MAX_FRAME_BYTES / 1024
        ),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerKind {
    Worker,
    Client,
}

#[derive(Debug)]
pub(crate) enum RecvError {
    Closed,
    Timeout,
    Frame(FrameError),
}

/// Reads the next text frame. Pings and pongs are skipped.
pub(crate) async fn recv_frame(
    socket: &mut WebSocket,
    timeout: Duration,
) -> Result<Frame, RecvError> {
    loop {
        let next = tokio::time::timeout(timeout, futures_util::StreamExt::next(socket))
            .await
            .map_err(|_| RecvError::Timeout)?;
        match next {
            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return Err(RecvError::Closed),
            Some(Ok(Message::Text(text))) => {
                return Frame::parse(text.as_str()).map_err(RecvError::Frame);
            }
            Some(Ok(Message::Binary(_))) => {
                return Err(RecvError::Frame(FrameError::Malformed(
                    "binary frames are not used",
                )));
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
        }
    }
}

pub(crate) async fn send_frame(socket: &mut WebSocket, frame: &Frame) -> bool {
    let Ok(text) = frame.to_text() else {
        return false;
    };
    futures_util::SinkExt::send(socket, Message::Text(text.into()))
        .await
        .is_ok()
}

async fn close_with(socket: &mut WebSocket, code: u16, reason: &str) {
    let _ = futures_util::SinkExt::send(
        socket,
        Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_owned().into(),
        })),
    )
    .await;
}

/// The result of a good handshake: who the peer is and its `hello` request.
pub(crate) struct Authenticated {
    pub peer: PeerId,
    pub hello: RequestFrame,
}

pub(crate) fn verify_signature(
    public_key: &PublicKey,
    payload: &[u8],
    signature: &Signature,
) -> bool {
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(public_key.as_bytes()) else {
        return false;
    };
    let signature = ed25519_dalek::Signature::from_bytes(signature.as_bytes());
    key.verify_strict(payload, &signature).is_ok()
}

/// Runs the challenge, the signature check and the wait for `hello`. On
/// failure it tells the peer and closes the socket.
pub(crate) async fn handshake(
    hub: &Hub,
    socket: &mut WebSocket,
    kind: PeerKind,
) -> Option<Authenticated> {
    // The slot is free again when this function returns.
    let Some(_slot) = hub.handshakes.try_take() else {
        tracing::warn!("too many connections are in the handshake, so the hub refuses a new one");
        close_with(socket, 1013, "too many connections, try again later").await;
        return None;
    };
    let nonce = Nonce::from_bytes(random_bytes::<32>());
    let challenge = Frame::event(&AuthChallenge { nonce }).ok()?;
    if !send_frame(socket, &challenge).await {
        return None;
    }

    let request = match recv_request(socket, "auth.response").await {
        Some(request) => request,
        None => return None,
    };
    let response: AuthResponse = match request.decode_as() {
        Ok(response) => response,
        Err(err) => {
            fail(
                socket,
                &request,
                err.to_error_body(),
                1008,
                "bad auth.response",
            )
            .await;
            return None;
        }
    };

    let wrong_kind = matches!(
        (kind, response.peer_id),
        (PeerKind::Worker, PeerId::Client(_)) | (PeerKind::Client, PeerId::Worker(_))
    );
    let public_key = if wrong_kind {
        None
    } else {
        match hub.peer_public_key(response.peer_id).await {
            Ok(key) => key,
            Err(err) => {
                tracing::error!(error = %err, "could not look up a peer key");
                None
            }
        }
    };
    let payload = auth_signing_payload(&hub.settings.origin, &nonce);
    let valid = public_key
        .as_ref()
        .is_some_and(|key| verify_signature(key, &payload, &response.signature));
    if !valid {
        // The same message for every failure, so a peer learns nothing about
        // which IDs exist.
        let error = ErrorBody::unauthorized(
            "The key or the signature is not valid. Pair this machine again.",
        );
        fail(socket, &request, error, 1008, "unauthorized").await;
        return None;
    }
    let ok = request.respond_ok(&Empty {}).ok()?;
    if !send_frame(socket, &ok).await {
        return None;
    }

    let hello = match recv_request(socket, "hello").await {
        Some(hello) => hello,
        None => return None,
    };
    Some(Authenticated {
        peer: response.peer_id,
        hello,
    })
}

async fn recv_request(socket: &mut WebSocket, expected: &str) -> Option<RequestFrame> {
    match recv_frame(socket, HANDSHAKE_TIMEOUT).await {
        Ok(Frame::Request(request)) if request.kind == expected => Some(request),
        Ok(_) => {
            close_with(socket, 1002, "unexpected frame").await;
            None
        }
        Err(RecvError::Frame(err)) => {
            let _ = send_frame(
                socket,
                &Frame::error(
                    consort_proto::frame::FrameId::generate(),
                    err.to_error_body(),
                ),
            )
            .await;
            close_with(socket, 1002, "protocol error").await;
            None
        }
        Err(_) => {
            close_with(socket, 1008, "handshake timed out").await;
            None
        }
    }
}

pub(crate) async fn fail(
    socket: &mut WebSocket,
    request: &RequestFrame,
    error: ErrorBody,
    code: u16,
    reason: &str,
) {
    let _ = send_frame(socket, &request.respond_error(error)).await;
    close_with(socket, code, reason).await;
}

pub(crate) async fn fail_protocol(socket: &mut WebSocket, request: &RequestFrame) {
    let error = ErrorBody::new(
        ErrorCode::ProtocolMismatch,
        format!(
            "This hub supports protocol versions {:?}. Update the app.",
            consort_proto::frame::SUPPORTED_PROTOCOL_VERSIONS
        ),
    );
    fail(socket, request, error, 1002, "protocol_mismatch").await;
}

pub(crate) const WS_MAX_FRAME: usize = MAX_FRAME_BYTES;

/// Sends queued text from the outbox to the socket until the connection ends.
pub(crate) async fn write_loop(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<String>,
    cancel: CancellationToken,
) {
    use futures_util::SinkExt;
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await;
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    cancel.cancel();
                    break;
                }
            }
            () = cancel.cancelled() => {
                let _ = sink.send(Message::Close(Some(CloseFrame { code: 1000, reason: "closing".into() }))).await;
                break;
            }
            next = rx.recv() => {
                let Some(text) = next else { break };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    cancel.cancel();
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::frame::FrameId;
    use serde_json::json;

    #[test]
    fn a_response_that_does_not_fit_goes_out_as_an_error_response() {
        let (outbox, mut rx) = Outbox::new(CancellationToken::new());
        let request = RequestFrame {
            id: FrameId::generate(),
            kind: "message.list".to_owned(),
            body: json!({}),
        };
        let big = request
            .respond_ok(&json!({"pad": "x".repeat(MAX_FRAME_BYTES)}))
            .unwrap();
        assert!(outbox.send_response(&request, &big));
        let Ok(Frame::Response(response)) = Frame::parse(&rx.try_recv().unwrap()) else {
            panic!("expected a response");
        };
        assert_eq!(response.re, request.id);
        let error = response.result.unwrap_err();
        assert!(error.message.contains("KiB"), "{}", error.message);
    }
}
