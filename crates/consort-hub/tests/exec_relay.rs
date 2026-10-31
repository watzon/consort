//! The hub between a session host and an execution worker: the tool list, the
//! approval engine and the relay of each tool call.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::common::*;
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::execution_link::{
    ExecOp, ExecOpResult, ExecTool, HubToExecution, WorkspaceInspection, WorkspacePrepare,
};
use consort_proto::frame::{Frame, FrameId, RequestFrame};
use consort_proto::model::{ApprovalMode, CommandCheckVerdict, Os};
use consort_proto::session_host_link::{
    CommandCheckReply, ToolAuthorize, ToolDecision, ToolExec, WorkspaceOpen, WorkspaceSpec,
};
use consort_proto::tools::ToolDefinition;
use serde_json::json;

fn exec_result() -> ExecOpResult {
    ExecOpResult {
        result: json!({"text": "hello"}),
        replayed_from_journal: false,
        is_error: false,
    }
}

async fn tool_names(world: &mut World, session_id: consort_proto::ids::SessionId) -> Vec<String> {
    let value = world
        .worker
        .peer
        .call_raw("tool.list", json!({"session_id": session_id}))
        .await
        .unwrap();
    let tools: Vec<ToolDefinition> = serde_json::from_value(value["tools"].clone()).unwrap();
    tools.into_iter().map(|t| t.name).collect()
}

#[tokio::test]
async fn only_a_task_gets_the_tools_of_the_worker() {
    let mut world = World::start().await;
    let (_, start) = world.start_task("Fix it.").await;
    let coordinator = world.coordinator_session.unwrap();
    let task_tools = tool_names(&mut world, start.session_id).await;
    for tool in ExecTool::ALL {
        assert!(task_tools.contains(&tool.name().to_owned()), "{tool:?}");
    }
    let coordinator_tools = tool_names(&mut world, coordinator).await;
    assert!(!coordinator_tools.contains(&"process_start".to_owned()));
    assert!(!coordinator_tools.contains(&"fs_write".to_owned()));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_tool_call_reaches_the_worker_with_the_capability_of_the_task() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Fix it.").await;
    let call = ToolExec {
        session_id: start.session_id,
        tool: "fs_read".to_owned(),
        arguments: json!({"path": "a.txt"}),
        operation_id: "op-1".to_owned(),
    };
    let (answer, command) = world
        .worker
        .relay_exec("tool.exec", serde_json::to_value(&call).unwrap(), |_| {
            serde_json::to_value(exec_result()).unwrap()
        })
        .await;
    let answer: ExecOpResult = serde_json::from_value(answer.unwrap()).unwrap();
    assert_eq!(answer.result["text"], "hello");
    let HubToExecution::ExecOp(ExecOp {
        capability,
        operation_id,
        tool,
        session_id,
        ..
    }) = command
    else {
        panic!("expected exec.op, got {command:?}");
    };
    assert_eq!(capability, format!("cap-{task_id}"));
    assert_eq!(operation_id, "op-1");
    assert_eq!(tool, "fs_read");
    assert_eq!(session_id, start.session_id);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn opening_a_workspace_asks_the_worker_and_returns_its_description() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Fix it.").await;
    let open = WorkspaceOpen {
        session_id: start.session_id,
        spec: WorkspaceSpec {
            repos: vec![],
            isolation: consort_proto::model::Isolation::Clone,
            carry_uncommitted: false,
        },
        channel_files: vec![],
    };
    let (answer, command) = world
        .worker
        .relay_exec(
            "workspace.open",
            serde_json::to_value(&open).unwrap(),
            |_| {
                serde_json::to_value(WorkspaceInspection {
                    capability: "cap".to_owned(),
                    worker: "bigboi".to_owned(),
                    os: Os::Windows,
                    arch: "x86_64".to_owned(),
                    native_root: "C:\\work".to_owned(),
                    path_separator: "\\".to_owned(),
                    case_insensitive_paths: true,
                    shell: "powershell".to_owned(),
                    runtimes: vec![],
                    layout: "empty".to_owned(),
                })
                .unwrap()
            },
        )
        .await;
    let answer: WorkspaceInspection = serde_json::from_value(answer.unwrap()).unwrap();
    assert_eq!(answer.native_root, "C:\\work");
    let HubToExecution::WorkspacePrepare(WorkspacePrepare {
        key, task_id: t, ..
    }) = command
    else {
        panic!("expected workspace.prepare, got {command:?}");
    };
    assert_eq!(key, task_id.to_string());
    assert_eq!(t, Some(task_id));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_offline_worker_gives_a_retry_error_and_the_session_stays() {
    let mut world = World::start().await;
    let (_, start) = world.start_task("Fix it.").await;
    world.worker.exec_goes_away().await;
    let exec_id = world.worker.exec_id;
    world
        .wait_worker(exec_id, consort_proto::model::WorkerState::Offline)
        .await;
    let call = ToolExec {
        session_id: start.session_id,
        tool: "fs_read".to_owned(),
        arguments: json!({"path": "a.txt"}),
        operation_id: "op-1".to_owned(),
    };
    let err = world
        .worker
        .peer
        .call_raw("tool.exec", serde_json::to_value(&call).unwrap())
        .await
        .unwrap_err();
    assert_eq!(err.code, consort_proto::error::ErrorCode::WorkerOffline);
    world.hub.handle.shutdown().await.unwrap();
}

async fn authorize(world: &mut World, request: &ToolAuthorize) -> ToolDecision {
    let value = world
        .worker
        .peer
        .call_raw("tool.authorize", serde_json::to_value(request).unwrap())
        .await
        .unwrap();
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn the_approval_engine_follows_the_table_for_a_task() {
    let mut world = World::start().await;
    let (_, start) = world.start_task("Fix it.").await;
    let request = |tool: &str, mode| ToolAuthorize {
        session_id: start.session_id,
        tool: tool.to_owned(),
        arguments: json!({"command": "ls", "path": "a"}),
        mode,
    };
    // The task itself runs in a mode, and the lower of the two counts. A new
    // task from a coordinator runs in Auto here.
    assert!(matches!(
        authorize(&mut world, &request("fs_read", ApprovalMode::Ask)).await,
        ToolDecision::Allow { .. }
    ));
    assert!(matches!(
        authorize(&mut world, &request("fs_write", ApprovalMode::Ask)).await,
        ToolDecision::Ask { .. }
    ));
    assert!(matches!(
        authorize(&mut world, &request("fs_write", ApprovalMode::AutoEdits)).await,
        ToolDecision::Allow { .. }
    ));
    assert!(matches!(
        authorize(
            &mut world,
            &request("process_start", ApprovalMode::AutoEdits)
        )
        .await,
        ToolDecision::Ask { .. }
    ));
    // Auto: the command check decides. It runs through the session host.
    let auto = request("process_start", ApprovalMode::Auto);
    let decision = authorize_with_check(&mut world, &auto, &[Ok("ALLOW")]).await;
    let ToolDecision::Allow { check: Some(check) } = decision else {
        panic!("expected an allow, got {decision:?}");
    };
    assert_eq!(check.verdict, CommandCheckVerdict::Allowed);
    let decision = authorize_with_check(
        &mut world,
        &auto,
        &[
            Ok("FLAG"),
            Ok("{\"verdict\": \"block\", \"reason\": \"It runs a script.\"}"),
        ],
    )
    .await;
    let ToolDecision::Deny { reason, check } = decision else {
        panic!("expected a denial, got {decision:?}");
    };
    assert!(reason.contains("It runs a script."), "{reason}");
    assert_eq!(check.unwrap().verdict, CommandCheckVerdict::Blocked);
    // A check that fails never allows: the user decides.
    let decision = authorize_with_check(&mut world, &auto, &[Err("It failed.")]).await;
    let ToolDecision::Ask { note, check, .. } = decision else {
        panic!("expected a question, got {decision:?}");
    };
    assert!(note.is_some());
    assert_eq!(check.unwrap().verdict, CommandCheckVerdict::Asked);
    world.hub.handle.shutdown().await.unwrap();
}

/// Sends `tool.authorize` and gives the answers to the command check that the
/// hub asks the session host to run.
async fn authorize_with_check(
    world: &mut World,
    request: &ToolAuthorize,
    answers: &[Result<&str, &str>],
) -> ToolDecision {
    let id = FrameId::generate();
    let peer = &mut world.worker.peer;
    peer.send(&Frame::Request(RequestFrame {
        id: id.clone(),
        kind: "tool.authorize".to_owned(),
        body: serde_json::to_value(request).unwrap(),
    }))
    .await;
    let mut answers = answers.iter();
    loop {
        match peer.next_frame(WAIT).await.expect("an answer from the hub") {
            Frame::Response(response) if response.re == id => {
                return serde_json::from_value(response.result.unwrap()).unwrap();
            }
            Frame::Request(hub_request) if hub_request.kind == "command_check.run" => {
                let frame = match answers.next().expect("one answer for each question") {
                    Ok(text) => hub_request
                        .respond_ok(&CommandCheckReply {
                            text: (*text).to_owned(),
                        })
                        .unwrap(),
                    Err(message) => {
                        hub_request.respond_error(ErrorBody::new(ErrorCode::Internal, *message))
                    }
                };
                peer.send(&frame).await;
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn a_worker_key_does_not_open_the_route_of_a_session_host() {
    let hub = TestHub::start().await;
    let code = hub.handle.new_worker_pairing_code().await.unwrap();
    let key = new_key();
    let (status, body) = pair_worker_http(&hub, &code, &key).await;
    assert_eq!(status, 200, "{body}");
    let worker_id: consort_proto::ids::WorkerId =
        serde_json::from_value(body["worker_id"].clone()).unwrap();

    // A worker key on the route of a session host fails.
    let mut peer = Peer::connect(&hub.ws_url(consort_proto::auth::SESSION_HOST_WS_PATH)).await;
    authenticate(
        &mut peer,
        &hub.origin(),
        consort_proto::auth::PeerId::Worker(worker_id),
        &key,
    )
    .await
    .unwrap();
    let hello = peer
        .call_raw(
            "hello",
            json!({"worker_id": worker_id, "version": "test", "protocol_versions": [1], "sessions": []}),
        )
        .await;
    assert_eq!(hello.unwrap_err().code, ErrorCode::Unauthorized);

    // A session host code does not pair a worker.
    let code = hub.handle.new_session_host_pairing_code().await.unwrap();
    let (status, _) = pair_worker_http(&hub, &code, &new_key()).await;
    assert_ne!(status, 200);
    hub.handle.shutdown().await.unwrap();
}
