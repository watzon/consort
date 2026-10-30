//! The client WebSocket: handshake, requests, snapshots and the feed.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::response::Response;
use consort_proto::auth::{ClientHello, ClientHelloResult, PeerId};
use consort_proto::client_api::{
    ClientRequest, FeedEntry, FeedSkipped, HubToClient, SyncSnapshot, SyncSubscribe,
};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{
    Empty, Frame, RequestFrame, SUPPORTED_PROTOCOL_VERSIONS, negotiate_version,
};
use consort_proto::ids::{ChannelId, ClientId};
use futures_util::StreamExt;
use tokio::task::JoinHandle;

use super::ws::{
    Outbox, PeerKind, WS_MAX_FRAME, fail, fail_protocol, handshake, send_frame, write_loop,
};
use crate::api;
use crate::error::{HubError, Result};
use crate::hub::{Caller, HUB_VERSION, Hub};
use crate::store::feed;

/// Any frame, including a pong, resets this timer. The hub pings every 30 s.
const CLIENT_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const BACKLOG_BATCH: usize = 500;

pub(crate) async fn client_ws(
    State(hub): State<Hub>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade
        .max_message_size(WS_MAX_FRAME)
        .max_frame_size(WS_MAX_FRAME)
        .on_upgrade(move |socket| run_client(hub, socket, addr.ip()))
}

async fn run_client(hub: Hub, mut socket: WebSocket, peer_ip: IpAddr) {
    let Some(auth) = handshake(&hub, &mut socket, PeerKind::Client, peer_ip).await else {
        return;
    };
    let PeerId::Client(client_id) = auth.peer else {
        return;
    };
    let hello: ClientHello = match auth.hello.decode_as() {
        Ok(hello) => hello,
        Err(err) => {
            fail(
                &mut socket,
                &auth.hello,
                err.to_error_body(),
                1002,
                "bad hello",
            )
            .await;
            return;
        }
    };
    if hello.client_id != client_id {
        let error = ErrorBody::unauthorized("The hello names another client. Pair this app again.");
        fail(&mut socket, &auth.hello, error, 1008, "unauthorized").await;
        return;
    }
    let Some(version) = negotiate_version(SUPPORTED_PROTOCOL_VERSIONS, &hello.protocol_versions)
    else {
        fail_protocol(&mut socket, &auth.hello).await;
        return;
    };
    let head = hub
        .store
        .read(|conn| Ok((feed::head_seq(conn)?, crate::store::hub_epoch(conn)?)))
        .await;
    let (feed_seq, hub_epoch) = match head {
        Ok(head) => head,
        Err(err) => {
            fail(
                &mut socket,
                &auth.hello,
                err.to_error_body(),
                1011,
                "internal",
            )
            .await;
            return;
        }
    };
    // A revoke closes only registered connections. So the hub registers this
    // one first and then reads the revoke time again. A revoke that ran
    // during the handshake is then seen here, and a later one closes the
    // connection through the registry.
    let cancel = hub.shutdown.child_token();
    let connection = hub.clients.register(client_id, cancel.clone());
    match hub.client_is_active(client_id).await {
        Ok(true) => {}
        Ok(false) => {
            hub.clients.unregister(connection);
            let error =
                ErrorBody::unauthorized("This client was removed. Pair this app again to connect.");
            fail(&mut socket, &auth.hello, error, 1008, "unauthorized").await;
            return;
        }
        Err(err) => {
            hub.clients.unregister(connection);
            fail(
                &mut socket,
                &auth.hello,
                err.to_error_body(),
                1011,
                "internal",
            )
            .await;
            return;
        }
    }
    let result = ClientHelloResult {
        protocol_version: version,
        hub_version: HUB_VERSION.to_owned(),
        feed_seq,
        hub_epoch,
    };
    let sent = match auth.hello.respond_ok(&result) {
        Ok(reply) => send_frame(&mut socket, &reply).await,
        Err(_) => false,
    };
    if !sent {
        hub.clients.unregister(connection);
        return;
    }
    if let Err(err) = hub.touch_client(client_id).await {
        tracing::warn!(error = %err, "could not record that a client connected");
    }

    let (outbox, rx) = Outbox::new(cancel.clone());
    let (sink, mut stream) = socket.split();
    let writer = tokio::spawn(write_loop(sink, rx, cancel.clone()));
    tracing::debug!(client_id = %client_id, "a client connected");

    let mut forwarder: Option<JoinHandle<()>> = None;
    loop {
        let next = tokio::select! {
            () = cancel.cancelled() => break,
            next = tokio::time::timeout(CLIENT_IDLE_TIMEOUT, stream.next()) => next,
        };
        let message = match next {
            Ok(Some(Ok(message))) => message,
            _ => break,
        };
        let text = match message {
            Message::Text(text) => text,
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Binary(_) => break,
        };
        // A revoke can land while a frame waits. Do not run it.
        if cancel.is_cancelled() {
            break;
        }
        match Frame::parse(text.as_str()) {
            Ok(Frame::Request(request)) => {
                handle_request(&hub, client_id, &outbox, &mut forwarder, request).await;
            }
            Ok(_) => {}
            Err(err) => {
                tracing::debug!(error = %err, "a client sent a bad frame");
                outbox.close();
                break;
            }
        }
    }

    if let Some(handle) = forwarder {
        handle.abort();
    }
    cancel.cancel();
    hub.clients.unregister(connection);
    let _ = writer.await;
    // The time of the last connection is when the client was last seen.
    if !hub.shutdown.is_cancelled()
        && let Err(err) = hub.touch_client(client_id).await
    {
        tracing::debug!(error = %err, "could not record that a client disconnected");
    }
    tracing::debug!(client_id = %client_id, "a client disconnected");
}

async fn handle_request(
    hub: &Hub,
    client_id: ClientId,
    outbox: &Outbox,
    forwarder: &mut Option<JoinHandle<()>>,
    request: RequestFrame,
) {
    let decoded: ClientRequest = match request.decode() {
        Ok(decoded) => decoded,
        Err(err) => {
            outbox.send_frame(&request.respond_error(err.to_error_body()));
            return;
        }
    };
    match decoded {
        ClientRequest::SyncSubscribe(subscribe) => {
            match prepare_subscription(hub, &subscribe).await {
                Ok(()) => {
                    // The response goes into the outbox before the forwarder
                    // starts, so the client sees `ok` before the first feed entry.
                    if let Ok(ok) = request.respond_ok(&Empty {}) {
                        outbox.send_frame(&ok);
                    }
                    if let Some(old) = forwarder.take() {
                        old.abort();
                    }
                    *forwarder = Some(spawn_forwarder(hub.clone(), outbox.clone(), subscribe));
                }
                Err(err) => {
                    outbox.send_frame(&request.respond_error(err.to_error_body()));
                }
            }
        }
        ClientRequest::TaskItemDetail(_) => {
            // This request waits for a worker, so it must not block the others.
            let hub = hub.clone();
            let outbox = outbox.clone();
            tokio::spawn(async move {
                let result = api::dispatch(&hub, Caller::Client(client_id), decoded).await;
                reply(&outbox, &request, result);
            });
        }
        other => {
            let result = match other {
                ClientRequest::Hello(_) => Err(HubError::conflict(
                    "The hello was already sent on this connection.",
                )),
                ClientRequest::SyncSnapshot(SyncSnapshot { scope }) => hub
                    .snapshot(scope)
                    .await
                    .and_then(|snapshot| Ok(serde_json::to_value(&snapshot)?)),
                ClientRequest::AuthHttpToken(_) => hub
                    .store
                    .read(move |conn| crate::store::machine::client(conn, client_id))
                    .await
                    .and_then(|client| match client {
                        Some(client) if client.revoked_at.is_none() => {
                            let token = hub.clients.issue_token(client_id, hub.now());
                            Ok(serde_json::to_value(&token)?)
                        }
                        _ => Err(HubError::unauthorized("This client was revoked.")),
                    }),
                other => api::dispatch(hub, Caller::Client(client_id), other).await,
            };
            reply(outbox, &request, result);
        }
    }
}

fn reply(outbox: &Outbox, request: &RequestFrame, result: Result<serde_json::Value>) {
    let frame = match result {
        Ok(value) => request
            .respond_ok(&value)
            .unwrap_or_else(|err| request.respond_error(err.to_error_body())),
        Err(err) => request.respond_error(err.to_error_body()),
    };
    outbox.send_response(request, &frame);
}

/// Checks the position before the hub says `ok`.
async fn prepare_subscription(hub: &Hub, subscribe: &SyncSubscribe) -> Result<()> {
    let (head, floor) = hub
        .store
        .read(|conn| Ok((feed::head_seq(conn)?, feed::floor_seq(conn)?)))
        .await?;
    if subscribe.after_seq < floor || subscribe.after_seq > head {
        return Err(HubError::new(
            ErrorCode::ResyncRequired,
            "Your position in the feed is not available any more. Reload the snapshot, then subscribe again.",
        ));
    }
    Ok(())
}

fn spawn_forwarder(hub: Hub, outbox: Outbox, subscribe: SyncSubscribe) -> JoinHandle<()> {
    let filter: Option<HashSet<ChannelId>> =
        subscribe.channel_ids.map(|ids| ids.into_iter().collect());
    let after_seq = subscribe.after_seq;
    tokio::spawn(async move {
        // Subscribe to live entries first, so none falls between the backlog
        // read and the live stream.
        let mut live = hub.store.subscribe();
        let mut last = after_seq;
        if !catch_up(&hub, &outbox, filter.as_ref(), &mut last).await {
            return;
        }
        loop {
            match live.recv().await {
                Ok(entry) => {
                    if entry.seq <= last {
                        continue;
                    }
                    last = entry.seq;
                    if passes(filter.as_ref(), &entry) && !send_entry(&outbox, &entry) {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if !catch_up(&hub, &outbox, filter.as_ref(), &mut last).await {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}

/// Sends every stored entry after `last`. Returns false when the connection
/// is gone or the read failed.
async fn catch_up(
    hub: &Hub,
    outbox: &Outbox,
    filter: Option<&HashSet<ChannelId>>,
    last: &mut u64,
) -> bool {
    loop {
        let from = *last;
        let batch = match hub
            .store
            .read(move |conn| feed::entries_after(conn, from, BACKLOG_BATCH))
            .await
        {
            Ok(batch) => batch,
            Err(err) => {
                tracing::warn!(error = %err, "could not read the feed backlog");
                outbox.close();
                return false;
            }
        };
        let done = batch.len() < BACKLOG_BATCH;
        for entry in batch {
            *last = entry.seq;
            if passes(filter, &entry) && !send_entry(outbox, &entry) {
                return false;
            }
        }
        if done {
            return true;
        }
    }
}

fn passes(filter: Option<&HashSet<ChannelId>>, entry: &FeedEntry) -> bool {
    match (filter, entry.channel_id) {
        (Some(set), Some(channel)) => set.contains(&channel),
        _ => true,
    }
}

/// Sends one feed entry. Returns false when the connection is gone.
///
/// An entry that does not fit in one frame goes out as a small
/// `feed.skipped` event in its place, so the client knows what it missed and
/// the feed goes on.
fn send_entry(outbox: &Outbox, entry: &FeedEntry) -> bool {
    let encoded =
        Frame::family_event(&HubToClient::Feed(entry.clone())).and_then(|frame| frame.to_text());
    match encoded {
        Ok(text) => outbox.send_text(text),
        Err(err) => {
            tracing::warn!(seq = entry.seq, entity_id = %entry.entity_id, error = %err, "a feed entry does not fit in a frame, so the hub sends feed.skipped");
            let skipped = HubToClient::FeedSkipped(FeedSkipped {
                seq: entry.seq,
                channel_id: entry.channel_id,
                entity: entry.change.kind(),
                entity_id: entry.entity_id.clone(),
            });
            match Frame::family_event(&skipped) {
                Ok(frame) => outbox.send_frame(&frame),
                Err(err) => {
                    tracing::warn!(error = %err, "could not encode feed.skipped");
                    true
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::client_api::{EntityKind, EntityPayload};
    use consort_proto::frame::MAX_FRAME_BYTES;
    use consort_proto::ids::MessageId;
    use consort_proto::model::{Author, Message, MessageKind};
    use consort_proto::time::Timestamp;
    use tokio_util::sync::CancellationToken;

    fn message(body: String) -> Message {
        Message {
            id: MessageId::new(),
            channel_id: ChannelId::new(),
            thread_root_id: None,
            author: Author::Assistant,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            kind: MessageKind::Text,
            body,
            attachments: vec![],
            in_timeline: false,
            client_message_id: None,
            reply_count: 0,
            last_reply_at: None,
            reactions: vec![],
            edited_at: None,
            deleted_at: None,
            created_at: Timestamp::now(),
            proactive: None,
            suggestions: Vec::new(),
        }
    }

    fn next_event(rx: &mut tokio::sync::mpsc::Receiver<String>) -> HubToClient {
        let text = rx.try_recv().expect("a frame");
        let Ok(Frame::Event(event)) = Frame::parse(&text) else {
            panic!("not an event: {text}");
        };
        event.decode().unwrap()
    }

    #[test]
    fn a_feed_entry_that_does_not_fit_goes_out_as_feed_skipped_and_the_feed_goes_on() {
        let (outbox, mut rx) = Outbox::new(CancellationToken::new());
        let big = message("x".repeat(MAX_FRAME_BYTES));
        let entry = FeedEntry::upsert(7, Timestamp::now(), EntityPayload::Message(big.clone()));
        assert!(send_entry(&outbox, &entry), "the forwarder must go on");
        let HubToClient::FeedSkipped(skipped) = next_event(&mut rx) else {
            panic!("expected feed.skipped");
        };
        assert_eq!(skipped.seq, 7);
        assert_eq!(skipped.entity, EntityKind::Message);
        assert_eq!(skipped.entity_id, big.id.to_string());
        assert_eq!(skipped.channel_id, Some(big.channel_id));

        // The next entry goes out as usual.
        let small = FeedEntry::upsert(
            8,
            Timestamp::now(),
            EntityPayload::Message(message("hi".to_owned())),
        );
        assert!(send_entry(&outbox, &small));
        assert!(matches!(next_event(&mut rx), HubToClient::Feed(e) if e.seq == 8));
    }
}
