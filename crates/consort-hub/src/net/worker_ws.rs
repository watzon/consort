//! The worker WebSocket.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::response::Response;
use consort_proto::auth::{PeerId, WorkerHello};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::execution_link::{ExecInventory, ExecSkillBundleGet, ExecutionToHub};
use consort_proto::frame::{
    Empty, Frame, FrameId, RequestFrame, SUPPORTED_PROTOCOL_VERSIONS, negotiate_version,
};
use consort_proto::ids::WorkerId;
use consort_proto::model::MachineKind;
use consort_proto::session_host_link::{
    HubToSessionHost, OFFLINE_AFTER_SECS, SessionHostToHub, SessionStop, ToolCall, ToolList,
    ToolListResult,
};
use futures_util::StreamExt;

use super::ws::{
    Outbox, PeerKind, WS_MAX_FRAME, fail, fail_protocol, handshake, send_frame, write_loop,
};
use crate::error::{HubError, Result};
use crate::hub::Hub;
use crate::link::WorkerLink;

pub(crate) async fn worker_ws(
    State(hub): State<Hub>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade
        .max_message_size(WS_MAX_FRAME)
        .max_frame_size(WS_MAX_FRAME)
        .on_upgrade(move |socket| run_worker(hub, socket, addr.ip(), MachineKind::Execution))
}

/// The session host has its own route and its own credentials. A key that
/// paired as a worker cannot connect here, and the other way round.
pub(crate) async fn session_host_ws(
    State(hub): State<Hub>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade
        .max_message_size(WS_MAX_FRAME)
        .max_frame_size(WS_MAX_FRAME)
        .on_upgrade(move |socket| run_worker(hub, socket, addr.ip(), MachineKind::SessionHost))
}

async fn run_worker(hub: Hub, mut socket: WebSocket, peer_ip: IpAddr, route: MachineKind) {
    let Some(auth) = handshake(&hub, &mut socket, PeerKind::Worker, peer_ip).await else {
        return;
    };
    let PeerId::Worker(worker_id) = auth.peer else {
        return;
    };
    if !matches!(hub.machine_kind(worker_id).await, Ok(Some(kind)) if kind == route) {
        let error = ErrorBody::unauthorized(
            "This key is paired for another kind of machine. Pair this machine again with a code of the right kind.",
        );
        fail(&mut socket, &auth.hello, error, 1008, "unauthorized").await;
        return;
    }
    let hello: WorkerHello = match auth.hello.decode_as() {
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
    if hello.worker_id != worker_id {
        let error =
            ErrorBody::unauthorized("The hello names another worker. Pair this machine again.");
        fail(&mut socket, &auth.hello, error, 1008, "unauthorized").await;
        return;
    }
    let Some(version) = negotiate_version(SUPPORTED_PROTOCOL_VERSIONS, &hello.protocol_versions)
    else {
        fail_protocol(&mut socket, &auth.hello).await;
        return;
    };
    let (result, stops) = match hub.worker_hello(worker_id, hello, version).await {
        Ok(pair) => pair,
        Err(err) => {
            let (code, reason) = if err.code() == ErrorCode::Unauthorized {
                (1008, "unauthorized")
            } else {
                (1011, "internal")
            };
            fail(&mut socket, &auth.hello, err.to_error_body(), code, reason).await;
            return;
        }
    };
    let Ok(reply) = auth.hello.respond_ok(&result) else {
        return;
    };
    if !send_frame(&mut socket, &reply).await {
        let _ = hub.worker_went_offline(worker_id).await;
        return;
    }

    let cancel = hub.shutdown.child_token();
    let (outbox, rx) = Outbox::new(cancel.clone());
    let (sink, mut stream) = socket.split();
    let writer = tokio::spawn(write_loop(sink, rx, cancel.clone()));
    let link = Arc::new(WorkerLink::new(
        hub.links.next_conn_id(),
        worker_id,
        outbox.clone(),
    ));
    if let Some(old) = hub.links.register(link.clone()) {
        // A new connection replaces an old one that did not close cleanly.
        old.outbox.close();
        old.fail_pending();
    }
    // A revoke closes only registered links, so read the revoke time again
    // now that this link is registered. This also sets the worker online
    // again if the old connection marked it offline during this hello.
    match hub.worker_link_registered(worker_id).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::info!(worker_id = %worker_id, "a removed worker finished its hello, so the hub closes the link");
            cancel.cancel();
            link.fail_pending();
            hub.links.remove(worker_id, link.conn_id);
            let _ = writer.await;
            return;
        }
        Err(err) => {
            tracing::warn!(error = %err, worker_id = %worker_id, "could not check a new worker link, so the hub closes it");
            cancel.cancel();
            link.fail_pending();
            if hub.links.remove(worker_id, link.conn_id) {
                let _ = hub.worker_went_offline(worker_id).await;
            }
            let _ = writer.await;
            return;
        }
    }
    tracing::info!(worker_id = %worker_id, "a worker connected");

    // The worker reported sessions that the hub does not know, or that have
    // ended on the hub. The worker stops them.
    for (session_id, reason) in stops {
        let link = link.clone();
        tokio::spawn(async move {
            let command = HubToSessionHost::SessionStop(SessionStop { session_id, reason });
            if let Ok(Frame::Request(request)) =
                Frame::family_request(FrameId::generate(), &command)
            {
                let _ = link
                    .call(
                        request.id,
                        request.kind,
                        request.body,
                        Duration::from_secs(60),
                    )
                    .await;
            }
        });
    }
    hub.worker_came_online(worker_id);

    let idle = Duration::from_secs(OFFLINE_AFTER_SECS);
    loop {
        let next = tokio::select! {
            () = cancel.cancelled() => break,
            next = tokio::time::timeout(idle, stream.next()) => next,
        };
        let message = match next {
            Ok(Some(Ok(message))) => message,
            Ok(_) => break,
            Err(_) => {
                tracing::warn!(worker_id = %worker_id, "a worker sent nothing for 60 seconds, so it is offline");
                break;
            }
        };
        let text = match message {
            Message::Text(text) => text,
            Message::Close(_) | Message::Binary(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
        };
        match Frame::parse(text.as_str()) {
            Ok(Frame::Request(request)) if route == MachineKind::Execution => {
                handle_execution_request(&hub, worker_id, &link, request).await;
            }
            Ok(Frame::Request(request)) => {
                handle_request(&hub, worker_id, &link, request).await;
            }
            Ok(Frame::Response(response)) => link.resolve(&response.re, response.result),
            Ok(Frame::Event(event)) if route == MachineKind::Execution => {
                if let Ok(ExecutionToHub::Inventory(ExecInventory(inventory))) =
                    event.decode::<ExecutionToHub>()
                    && let Err(err) = hub.update_inventory(worker_id, inventory).await
                {
                    tracing::warn!(error = %err, "could not store an inventory");
                }
            }
            Ok(Frame::Event(event)) => match event.decode::<SessionHostToHub>() {
                Ok(SessionHostToHub::Inventory(inventory)) => {
                    if let Err(err) = hub.update_inventory(worker_id, inventory).await {
                        tracing::warn!(error = %err, "could not store an inventory");
                    }
                }
                Ok(SessionHostToHub::SignInUpdate(update)) => {
                    if let Err(err) = hub.sign_in_update(worker_id, update).await {
                        tracing::warn!(error = %err, "could not store a sign-in update");
                    }
                }
                _ => {}
            },
            Err(err) => {
                tracing::debug!(error = %err, "a worker sent a bad frame");
                break;
            }
        }
    }

    cancel.cancel();
    link.fail_pending();
    if hub.links.remove(worker_id, link.conn_id) {
        if let Err(err) = hub.worker_went_offline(worker_id).await {
            tracing::warn!(error = %err, "could not mark a worker offline");
        }
        tracing::info!(worker_id = %worker_id, "a worker disconnected");
    }
    let _ = writer.await;
}

/// A worker sends only these. It has no sessions and no accounts.
async fn handle_execution_request(
    hub: &Hub,
    worker_id: WorkerId,
    link: &Arc<WorkerLink>,
    request: RequestFrame,
) {
    let decoded: ExecutionToHub = match request.decode() {
        Ok(decoded) => decoded,
        Err(err) => {
            link.outbox
                .send_frame(&request.respond_error(err.to_error_body()));
            return;
        }
    };
    match decoded {
        ExecutionToHub::Hello(_) => {
            link.outbox
                .send_frame(&request.respond_error(ErrorBody::conflict(
                    "The hello was already sent on this connection.",
                )));
        }
        ExecutionToHub::Inventory(ExecInventory(inventory)) => {
            let result = hub
                .update_inventory(worker_id, inventory)
                .await
                .map(|()| Empty {});
            reply(link, &request, result);
        }
        ExecutionToHub::SkillBundleGet(ExecSkillBundleGet(get)) => {
            let result = hub.skill_bundle_get(worker_id, get).await;
            reply(link, &request, result);
        }
    }
}

async fn handle_request(
    hub: &Hub,
    worker_id: WorkerId,
    link: &Arc<WorkerLink>,
    request: RequestFrame,
) {
    let decoded: SessionHostToHub = match request.decode() {
        Ok(decoded) => decoded,
        Err(err) => {
            link.outbox
                .send_frame(&request.respond_error(err.to_error_body()));
            return;
        }
    };
    match decoded {
        SessionHostToHub::Hello(_) => {
            link.outbox
                .send_frame(&request.respond_error(ErrorBody::conflict(
                    "The hello was already sent on this connection.",
                )));
        }
        SessionHostToHub::Inventory(inventory) => {
            let result = hub
                .update_inventory(worker_id, inventory)
                .await
                .map(|()| Empty {});
            reply(link, &request, result);
        }
        SessionHostToHub::SessionEvents(batch) => {
            let result = hub.ingest_events(worker_id, batch).await;
            reply(link, &request, result);
        }
        SessionHostToHub::ToolList(ToolList { session_id }) => {
            let result = async {
                check_session_owner(hub, worker_id, session_id).await?;
                Ok(ToolListResult {
                    tools: hub.tool_definitions(session_id).await?,
                })
            }
            .await;
            reply(link, &request, result);
        }
        SessionHostToHub::SignInUpdate(update) => {
            // `sign_in.update` is an event. A worker that sends it as a
            // request gets an answer anyway.
            let result = hub
                .sign_in_update(worker_id, update)
                .await
                .map(|()| Empty {});
            reply(link, &request, result);
        }
        SessionHostToHub::SkillBundleGet(get) => {
            let result = hub.skill_bundle_get(worker_id, get).await;
            reply(link, &request, result);
        }
        // These wait on a worker or on a model, so they must not block other
        // frames.
        SessionHostToHub::WorkspaceOpen(open) => {
            let hub = hub.clone();
            let link = link.clone();
            tokio::spawn(async move {
                let result = hub.workspace_open(worker_id, open).await;
                reply(&link, &request, result);
            });
        }
        SessionHostToHub::WorkspaceFiles(files) => {
            let hub = hub.clone();
            let link = link.clone();
            tokio::spawn(async move {
                let result = hub.workspace_files(worker_id, files).await;
                reply(&link, &request, result);
            });
        }
        SessionHostToHub::ToolAuthorize(authorize) => {
            let hub = hub.clone();
            let link = link.clone();
            tokio::spawn(async move {
                let result = hub.tool_authorize(worker_id, authorize).await;
                reply(&link, &request, result);
            });
        }
        SessionHostToHub::ToolExec(exec) => {
            let hub = hub.clone();
            let link = link.clone();
            tokio::spawn(async move {
                let result = hub.tool_exec(worker_id, exec).await;
                reply(&link, &request, result);
            });
        }
        SessionHostToHub::ToolCall(call) => {
            // A tool call can take long, so it must not block other frames.
            let hub = hub.clone();
            let link = link.clone();
            tokio::spawn(run_tool_call(hub, worker_id, link, request, call));
        }
    }
}

/// Runs one `tool.call` and sends the answer. It waits while the link has
/// [`MAX_TOOL_CALLS_PER_LINK`](crate::link::MAX_TOOL_CALLS_PER_LINK) calls
/// running.
async fn run_tool_call(
    hub: Hub,
    worker_id: WorkerId,
    link: Arc<WorkerLink>,
    request: RequestFrame,
    call: ToolCall,
) {
    let Ok(_permit) = link.tool_calls.clone().acquire_owned().await else {
        // The semaphore is never closed. A closed one means the link is gone.
        reply::<Empty>(
            &link,
            &request,
            Err(HubError::internal("the tool call limit closed")),
        );
        return;
    };
    let result = match check_session_owner(&hub, worker_id, call.session_id).await {
        Ok(()) => Ok(hub
            .call_tool(call.session_id, &call.name, call.arguments)
            .await),
        Err(err) => Err(err),
    };
    reply(&link, &request, result);
}

async fn check_session_owner(
    hub: &Hub,
    worker_id: WorkerId,
    session_id: consort_proto::ids::SessionId,
) -> Result<()> {
    let owner = hub
        .store
        .read(move |conn| crate::store::work::session(conn, session_id))
        .await?;
    match owner {
        Some(session) if session.worker_id == worker_id => Ok(()),
        _ => Err(HubError::new(
            ErrorCode::NotFound,
            "The hub does not know this session for this worker.",
        )),
    }
}

fn reply<T: serde::Serialize>(link: &WorkerLink, request: &RequestFrame, result: Result<T>) {
    let frame = match result {
        Ok(value) => request
            .respond_ok(&value)
            .unwrap_or_else(|err| request.respond_error(err.to_error_body())),
        Err(err) => request.respond_error(err.to_error_body()),
    };
    link.outbox.send_response(request, &frame);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::MAX_TOOL_CALLS_PER_LINK;
    use crate::net::ws::Outbox;
    use consort_proto::ids::SessionId;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn a_tool_call_waits_while_the_link_runs_its_most_calls() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let (outbox, mut rx) = Outbox::new(CancellationToken::new());
        let link = Arc::new(WorkerLink::new(1, WorkerId::new(), outbox));
        let held: Vec<_> = (0..MAX_TOOL_CALLS_PER_LINK)
            .map(|_| link.tool_calls.clone().try_acquire_owned().unwrap())
            .collect();
        let call = ToolCall {
            session_id: SessionId::new(),
            call_id: "c1".to_owned(),
            name: "memory_list".to_owned(),
            arguments: json!({}),
        };
        let request = RequestFrame {
            id: FrameId::generate(),
            kind: "tool.call".to_owned(),
            body: json!({}),
        };
        let running = tokio::spawn(run_tool_call(
            hub.clone(),
            link.worker_id,
            link.clone(),
            request,
            call,
        ));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(rx.try_recv().is_err(), "the call ran past the limit");
        drop(held);
        running.await.unwrap();
        // The session does not exist, so the answer is an error. It came.
        assert!(rx.try_recv().is_ok());
    }
}
