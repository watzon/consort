//! A task session: the harness runs on the session host, and each tool call
//! goes through the hub to the worker.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::FakeHub;

use consort_harness::{AccountConfig, Harness, HarnessKind};
use consort_proto::execution_link::ExecOpResult;
use consort_proto::ids::{AccountId, ChannelId, SessionId, TaskId, WorkerId};
use consort_proto::model::{ApprovalDetails, ApprovalMode, Isolation, ItemPayload, SessionRole};
use consort_proto::session_host_link::{
    RequestAnswer, RequestBody, RequestOpened, SessionEvent, SessionExecution, SessionRespond,
    SessionStart, ToolDecision, TurnInput, WorkspaceSpec,
};
use consort_session_host::bridge::McpHttpClient;
use consort_session_host::config::HostToml;
use consort_session_host::{Harnesses, HostConfig};
use consort_testkit::{FakeHarness, FakeRecord, FakeScript, FakeStep};
use serde_json::{Value, json};

struct Fixture {
    hub: FakeHub,
    record: Arc<std::sync::Mutex<FakeRecord>>,
    account_id: AccountId,
    _dir: tempfile::TempDir,
    _host: consort_session_host::HostHandle,
    dir: std::path::PathBuf,
}

async fn fixture() -> Fixture {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    consort_session_host::pair(&hub.url, "abcd-efgh", dir.path(), None, vec![])
        .await
        .unwrap();
    let mut toml = HostToml::load(dir.path()).unwrap();
    let account = AccountConfig::default_for(HarnessKind::ClaudeCode);
    let account_id = account.id;
    toml.accounts = vec![account];
    toml.save(dir.path()).unwrap();
    let fake = FakeHarness::new(
        HarnessKind::ClaudeCode,
        FakeScript {
            turns: vec![vec![FakeStep::Text("Working.".into())]],
        },
    );
    let record = fake.record();
    let mut harnesses: Harnesses = HashMap::new();
    harnesses.insert(HarnessKind::ClaudeCode, Arc::new(fake) as Arc<dyn Harness>);
    let host = consort_session_host::run_with(
        HostConfig {
            ws_path: consort_proto::auth::SESSION_HOST_WS_PATH,
            config_dir: dir.path().to_path_buf(),
            consort_home: Some(dir.path().join("home")),
            check_tool_updates: false,
        },
        harnesses,
    )
    .await
    .unwrap();
    assert!(host.wait_connected(Duration::from_secs(10)).await);
    hub.wait_connected().await;
    Fixture {
        hub,
        record,
        account_id,
        dir: dir.path().to_path_buf(),
        _dir: dir,
        _host: host,
    }
}

fn start(session_id: SessionId, task_id: TaskId, account_id: AccountId) -> SessionStart {
    SessionStart {
        execution: Some(SessionExecution {
            worker_id: WorkerId::new(),
            worker_name: "bigboi".into(),
        }),
        session_id,
        role: SessionRole::Task,
        channel_id: ChannelId::new(),
        task_id: Some(task_id),
        account_id,
        model: "fake-model".into(),
        options: Default::default(),
        approval_mode: ApprovalMode::Auto,
        workspace: WorkspaceSpec {
            repos: vec![],
            isolation: Isolation::Clone,
            carry_uncommitted: false,
        },
        system_prompt_append: "You are a task.".into(),
        allowed_local_mcp_servers: vec![],
        channel_files: vec![],
        extensions: Default::default(),
        resume: None,
        turn: Some(TurnInput {
            text: "Run the tests".into(),
            attachments: vec![],
        }),
        turn_approval_mode: None,
    }
}

/// The MCP client of the session, as the harness would use it.
async fn mcp_client(fixture: &Fixture) -> McpHttpClient {
    let (url, token) = {
        let record = fixture.record.lock().unwrap();
        let server = &record.opened[0].mcp_servers[0];
        assert_eq!(server.name, "consort");
        (server.url.clone(), server.bearer_token.clone().unwrap())
    };
    let client = McpHttpClient::new(&url, &token).unwrap();
    client
        .post(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"},
            }}),
        )
        .await
        .unwrap();
    client
        .post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await
        .unwrap();
    client
}

async fn call(client: &McpHttpClient, name: &str, arguments: Value) -> Value {
    let answer = client
        .post(
            &json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {
                "name": name,
                "arguments": arguments,
            }}),
        )
        .await
        .unwrap();
    answer
        .into_iter()
        .find(|m| m.get("result").is_some())
        .expect("a result")["result"]
        .clone()
}

#[tokio::test]
async fn the_harness_runs_here_and_the_workspace_is_on_the_worker() {
    let fixture = fixture().await;
    let (session_id, task_id) = (SessionId::new(), TaskId::new());
    let response = fixture
        .hub
        .command(
            "cmd-1",
            start(session_id, task_id, fixture.account_id).into(),
        )
        .await;
    let body = response.result.expect("session.start succeeds");
    assert_eq!(body["workspace_path"], "C:\\work\\task");

    let record = fixture.record.lock().unwrap();
    let opened = &record.opened[0];
    // The CLI runs in the folder of the task on this machine.
    assert_eq!(
        opened.workspace,
        fixture.dir.join("sessions").join(task_id.to_string())
    );
    assert!(opened.workspace.is_dir());
    // The environment message overrides what the CLI says, in the prompt and
    // before the first turn.
    assert!(opened.system_prompt_append.contains("You are a task."));
    assert!(opened.system_prompt_append.contains("C:\\work\\task"));
    assert!(opened.system_prompt_append.contains("AGENTS.md"));
    assert!(record.turns[0].text.contains("C:\\work\\task"));
    assert!(record.turns[0].text.ends_with("Run the tests"));
    drop(record);
    assert_eq!(fixture.hub.state.workspace_opens.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_tool_of_the_worker_goes_through_the_hub_and_shows_a_command() {
    let fixture = fixture().await;
    let session_id = SessionId::new();
    fixture
        .hub
        .command(
            "cmd-1",
            start(session_id, TaskId::new(), fixture.account_id).into(),
        )
        .await;
    // The first event of the turn gives the items their turn.
    fixture
        .hub
        .wait_for("turn.started", |h| {
            h.events(session_id)
                .iter()
                .any(|e| matches!(e.event, SessionEvent::TurnStarted { .. }))
                .then_some(())
        })
        .await;
    fixture
        .hub
        .state
        .exec_results
        .lock()
        .unwrap()
        .push_back(ExecOpResult {
            result: json!({"process_id": "p1", "state": "running"}),
            replayed_from_journal: false,
            is_error: false,
        });
    let client = mcp_client(&fixture).await;
    let result = call(
        &client,
        "process_start",
        json!({"command": "cargo test", "operation_id": "op-1"}),
    )
    .await;
    assert_eq!(result["isError"], false, "{result}");
    {
        let authorized = fixture.hub.state.authorized.lock().unwrap();
        assert_eq!(authorized[0].tool, "process_start");
        assert_eq!(authorized[0].mode, ApprovalMode::Auto);
        let execs = fixture.hub.state.execs.lock().unwrap();
        assert_eq!(execs[0].operation_id, "op-1");
        assert!(execs[0].arguments.get("operation_id").is_none());
    }
    fixture
        .hub
        .wait_for("the command item", |h| {
            h.events(session_id).iter().find_map(|e| match &e.event {
                SessionEvent::Item(item) => match &item.payload {
                    ItemPayload::Command(c) if c.command == "cargo test" => Some(()),
                    _ => None,
                },
                _ => None,
            })
        })
        .await;
}

#[tokio::test]
async fn a_call_that_asks_waits_for_the_answer_of_the_user() {
    let fixture = fixture().await;
    let session_id = SessionId::new();
    fixture
        .hub
        .command(
            "cmd-1",
            start(session_id, TaskId::new(), fixture.account_id).into(),
        )
        .await;
    fixture
        .hub
        .state
        .decisions
        .lock()
        .unwrap()
        .push_back(ToolDecision::Ask {
            note: Some("Consort asks.".into()),
            blocked: vec![],
            check: None,
        });
    let client = mcp_client(&fixture).await;
    let running = tokio::spawn(async move {
        call(
            &client,
            "process_start",
            json!({"command": "git push", "operation_id": "op-2"}),
        )
        .await
    });
    let request: RequestOpened = fixture
        .hub
        .wait_for("request.opened", |h| {
            h.events(session_id)
                .into_iter()
                .find_map(|e| match e.event {
                    SessionEvent::RequestOpened(r) => Some(r),
                    _ => None,
                })
        })
        .await;
    assert!(matches!(
        &request.body,
        RequestBody::Approval(ApprovalDetails::Command { command, check_note, .. })
            if command == "git push" && check_note.as_deref() == Some("Consort asks.")
    ));
    assert!(fixture.hub.state.execs.lock().unwrap().is_empty());

    // The answer goes to the tool call, not to the harness.
    let answer = fixture
        .hub
        .command(
            "cmd-2",
            SessionRespond {
                session_id,
                request_id: request.request_id.clone(),
                answer: RequestAnswer {
                    option_id: Some("allow".into()),
                    text: None,
                },
            }
            .into(),
        )
        .await;
    assert!(answer.result.is_ok(), "{answer:?}");
    let result = running.await.unwrap();
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(fixture.hub.state.execs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn publish_artifact_uploads_from_the_worker() {
    let fixture = fixture().await;
    let session_id = SessionId::new();
    fixture
        .hub
        .command(
            "cmd-1",
            start(session_id, TaskId::new(), fixture.account_id).into(),
        )
        .await;
    let sha = "ab".repeat(32);
    fixture
        .hub
        .state
        .exec_results
        .lock()
        .unwrap()
        .push_back(ExecOpResult {
            result: json!({"sha256": sha, "size": 12}),
            replayed_from_journal: false,
            is_error: false,
        });
    let client = mcp_client(&fixture).await;
    let result = call(
        &client,
        "publish_artifact",
        json!({"path": "out/report.md", "title": "Report"}),
    )
    .await;
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(fixture.hub.state.execs.lock().unwrap()[0].tool, "blob_put");
    let calls = fixture.hub.state.tool_calls.lock().unwrap();
    assert_eq!(calls[0].name, "publish_artifact");
    assert_eq!(calls[0].arguments["blob_sha256"], sha);
    assert_eq!(calls[0].arguments["size"], 12);
    assert_eq!(calls[0].arguments["mime"], "text/markdown");
}
