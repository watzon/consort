//! The real hub, the real session host and the real worker, each with its own
//! key and link. Only the harness is a fake: this test uses the MCP endpoint of
//! the session as the CLI would. `real_claude` runs the real CLI.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

#[path = "../common/mod.rs"]
mod common;
mod real_claude;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use consort_harness::{AccountConfig, Harness, HarnessKind};
use consort_proto::client_api::{ChannelUpdate, TaskCreate};
use consort_proto::ids::{ChannelId, TaskId};
use consort_proto::model::{ApprovalMode, PlacementRequest};
use consort_session_host::bridge::McpHttpClient;
use consort_session_host::config::HostToml;
use consort_session_host::{Harnesses, HostConfig, HostHandle};
use consort_testkit::{FakeHarness, FakeRecord, FakeScript, FakeStep};
use consort_worker::{WorkerConfig, WorkerHandle};
use serde_json::{Value, json};

pub(crate) struct Stack {
    pub hub: TestHub,
    pub client: FakeClient,
    pub tutti: ChannelId,
    pub host: Option<HostHandle>,
    pub worker: Option<WorkerHandle>,
    pub host_dir: tempfile::TempDir,
    pub worker_dir: tempfile::TempDir,
}

impl Stack {
    /// A hub, a client, and a session host and a worker that are paired.
    pub async fn pair() -> Self {
        Self::pair_on(TestHub::start().await).await
    }

    /// The same, on a hub that the caller started.
    pub async fn pair_on(hub: TestHub) -> Self {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
        let mut client = FakeClient::pair_and_connect(&hub).await;
        let tutti = client.tutti().await;
        client
            .request(consort_proto::client_api::ProactivitySet {
                paused: Some(true),
                ..Default::default()
            })
            .await
            .unwrap();
        let host_dir = tempfile::tempdir().unwrap();
        let worker_dir = tempfile::tempdir().unwrap();
        let code = hub.handle.new_session_host_pairing_code().await.unwrap();
        consort_session_host::pair(&hub.url, &code, host_dir.path(), None, vec![])
            .await
            .unwrap();
        let code = hub.handle.new_worker_pairing_code().await.unwrap();
        consort_worker::pair(&hub.url, &code, worker_dir.path(), None, vec![])
            .await
            .unwrap();
        Self {
            hub,
            client,
            tutti,
            host: None,
            worker: None,
            host_dir,
            worker_dir,
        }
    }

    pub fn write_account(&self, account: AccountConfig) {
        let mut toml = HostToml::load(self.host_dir.path()).unwrap();
        toml.accounts = vec![account];
        toml.save(self.host_dir.path()).unwrap();
    }

    pub async fn run_worker(&mut self) {
        // A worker allows at most Auto by default. These tests answer no
        // question, so they run in Full access.
        let journal =
            consort_worker::journal::Journal::open(&self.worker_dir.path().join("worker.db"))
                .unwrap();
        journal
            .put_policy(&consort_worker::config::PolicyConfig {
                max_approval_mode: ApprovalMode::FullAccess,
                ..Default::default()
            })
            .unwrap();
        let worker = consort_worker::run(WorkerConfig {
            config_dir: self.worker_dir.path().to_path_buf(),
            self_update: None,
        })
        .await
        .unwrap();
        assert!(worker.wait_connected(Duration::from_secs(10)).await);
        self.worker = Some(worker);
    }

    /// The only workspace folder on the worker that holds `file`.
    pub fn find_on_worker(&self, file: &str) -> Option<PathBuf> {
        find_file(&self.worker_dir.path().join("workspaces"), file)
    }

    pub async fn start_task(
        &mut self,
        brief: &str,
        selection: Option<consort_proto::model::ModelSelection>,
        mode: ApprovalMode,
    ) -> TaskId {
        self.client
            .request(TaskCreate {
                channel_id: self.tutti,
                title: "Full stack".to_owned(),
                brief: brief.to_owned(),
                model_selection: selection,
                placement: PlacementRequest::default(),
                approval_mode: Some(mode),
                start_at: None,
            })
            .await
            .unwrap()
            .task
            .id
    }

    pub async fn set_auto(&mut self) {
        let mut update = ChannelUpdate::new(self.tutti);
        update.default_approval_mode = Some(ApprovalMode::FullAccess);
        self.client.request(update).await.unwrap();
    }
}

fn find_file(dir: &Path, file: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, file) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|n| n == file) {
            return Some(path);
        }
    }
    None
}

async fn fake_host(stack: &mut Stack) -> Arc<Mutex<FakeRecord>> {
    let fake = FakeHarness::new(
        HarnessKind::ClaudeCode,
        FakeScript {
            turns: vec![vec![FakeStep::Text("Working.".into())]],
        },
    );
    let record = fake.record();
    stack.write_account(AccountConfig::default_for(HarnessKind::ClaudeCode));
    let mut harnesses: Harnesses = HashMap::new();
    harnesses.insert(HarnessKind::ClaudeCode, Arc::new(fake) as Arc<dyn Harness>);
    let host = consort_session_host::run_with(
        HostConfig {
            ws_path: consort_proto::auth::SESSION_HOST_WS_PATH,
            config_dir: stack.host_dir.path().to_path_buf(),
            consort_home: Some(stack.host_dir.path().join("home")),
            check_tool_updates: false,
        },
        harnesses,
    )
    .await
    .unwrap();
    assert!(host.wait_connected(Duration::from_secs(10)).await);
    stack.host = Some(host);
    record
}

async fn mcp(record: &Arc<Mutex<FakeRecord>>) -> McpHttpClient {
    let deadline = tokio::time::Instant::now() + WAIT;
    let (url, token) = loop {
        if let Some(spec) = record
            .lock()
            .unwrap()
            .opened
            .iter()
            .find(|spec| !spec.coordinator)
        {
            let server = &spec.mcp_servers[0];
            break (server.url.clone(), server.bearer_token.clone().unwrap());
        }
        assert!(tokio::time::Instant::now() < deadline, "no session opened");
        tokio::time::sleep(Duration::from_millis(25)).await;
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
    let result = answer
        .into_iter()
        .find(|m| m.get("result").is_some())
        .expect("a result")["result"]
        .clone();
    assert_eq!(result["isError"], false, "{name}: {result}");
    result["structuredContent"].clone()
}

/// The real worker and the real hub: the app's request changes the limits of
/// the worker at once, and the worker keeps them across a restart.
#[tokio::test]
async fn the_app_changes_the_limits_of_a_worker_and_the_worker_keeps_them() {
    use consort_proto::client_api::{WorkerList, WorkerSetPolicy};
    let mut stack = Stack::pair().await;
    stack.run_worker().await;
    let worker_id = stack
        .client
        .request(WorkerList {})
        .await
        .unwrap()
        .workers
        .iter()
        .find(|w| w.kind == consort_proto::model::MachineKind::Execution)
        .unwrap()
        .id;
    let root = stack.worker_dir.path().to_string_lossy().into_owned();
    let changed = stack
        .client
        .request(WorkerSetPolicy {
            worker_id,
            max_approval_mode: Some(ApprovalMode::Ask),
            allowed_roots: Some(vec![root.clone()]),
            allow_shell: Some(false),
        })
        .await
        .unwrap()
        .worker;
    let policy = changed.inventory.unwrap().policy;
    assert_eq!(policy.max_approval_mode, ApprovalMode::Ask);
    assert_eq!(policy.allowed_roots, vec![root.clone()]);
    assert!(!policy.allow_shell);

    // The worker refuses a folder that it cannot call a full path.
    let refused = stack
        .client
        .request(WorkerSetPolicy {
            worker_id,
            max_approval_mode: None,
            allowed_roots: Some(vec!["not/a/full/path".to_owned()]),
            allow_shell: None,
        })
        .await
        .unwrap_err();
    assert!(format!("{refused:?}").contains("full path"), "{refused:?}");

    // After a restart, the worker reports the same limits.
    stack.worker.take().unwrap().shutdown().await.unwrap();
    let worker = consort_worker::run(WorkerConfig {
        config_dir: stack.worker_dir.path().to_path_buf(),
        self_update: None,
    })
    .await
    .unwrap();
    assert!(worker.wait_connected(Duration::from_secs(10)).await);
    stack.worker = Some(worker);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let list = stack.client.request(WorkerList {}).await.unwrap();
        let w = list.workers.iter().find(|w| w.id == worker_id).unwrap();
        if w.state == consort_proto::model::WorkerState::Online {
            let policy = &w.inventory.as_ref().unwrap().policy;
            assert_eq!(policy.max_approval_mode, ApprovalMode::Ask);
            assert_eq!(policy.allowed_roots, vec![root.clone()]);
            assert!(!policy.allow_shell);
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no reconnect");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    stack.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_task_writes_files_and_runs_commands_on_the_worker() {
    let mut stack = Stack::pair().await;
    let record = fake_host(&mut stack).await;
    stack.run_worker().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    stack
        .start_task("Write a file.", None, ApprovalMode::FullAccess)
        .await;
    let client = mcp(&record).await;

    // The workspace is on the worker, and the session sees its description.
    let inspection = call(&client, "workspace_inspect", json!({})).await;
    let root = inspection["native_root"].as_str().unwrap().to_owned();
    assert!(
        root.starts_with(&*stack.worker_dir.path().to_string_lossy()),
        "{root}"
    );

    // A write, and the same operation again: the journal answers.
    let write = json!({"path": "made.txt", "content": "one\n", "operation_id": "op-1"});
    let first = call(&client, "fs_write", write.clone()).await;
    assert!(first.get("replayed_from_journal").is_none(), "{first}");
    let second = call(&client, "fs_write", write).await;
    assert_eq!(second["replayed_from_journal"], true, "{second}");
    let made = stack
        .find_on_worker("made.txt")
        .expect("the file on the worker");
    assert_eq!(std::fs::read_to_string(&made).unwrap(), "one\n");
    // The harness folder on the session host has no such file.
    assert!(find_file(stack.host_dir.path(), "made.txt").is_none());

    // A command runs on the worker.
    let started = call(
        &client,
        "process_start",
        json!({"command": "echo two >> made.txt; cat made.txt", "operation_id": "op-2"}),
    )
    .await;
    let process = started["process_id"].as_str().unwrap().to_owned();
    let waited = call(
        &client,
        "process_wait",
        json!({"process_id": process, "timeout_seconds": 20}),
    )
    .await;
    assert_eq!(waited["state"], "exited", "{waited}");
    assert!(
        waited["output"].as_str().unwrap().contains("two"),
        "{waited}"
    );
    assert_eq!(std::fs::read_to_string(&made).unwrap(), "one\ntwo\n");

    stack.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_command_goes_on_through_a_restart_of_the_hub() {
    let mut stack = Stack::pair().await;
    let record = fake_host(&mut stack).await;
    stack.run_worker().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    stack
        .start_task("Run a long command.", None, ApprovalMode::FullAccess)
        .await;
    let client = mcp(&record).await;
    let started = call(
        &client,
        "process_start",
        json!({"command": "sleep 3; echo finished > done.txt; echo finished", "operation_id": "op-1"}),
    )
    .await;
    let process = started["process_id"].as_str().unwrap().to_owned();

    // The hub stops and comes back on the same port and data.
    let Stack { hub, .. } = &mut stack;
    let addr = hub.addr;
    let dir = tempfile::tempdir().unwrap();
    let old = std::mem::replace(hub, TestHub::start_in(dir).await);
    let TestHub { handle, dir, .. } = old;
    handle.shutdown().await.unwrap();
    let again = consort_hub::serve(consort_hub::HubConfig {
        data_dir: dir.path().to_path_buf(),
        listen: addr,
        public_url: None,
        install_kind: consort_proto::release::InstallKind::Manual,
        dev: false,
        first_greeting: false,
    })
    .await
    .unwrap();

    // `process_wait` retries with the link down, and it gets the result.
    let mut state = Value::Null;
    for _ in 0..12 {
        let waited = call(
            &client,
            "process_wait",
            json!({"process_id": process, "timeout_seconds": 5}),
        )
        .await;
        state = waited["state"].clone();
        if state == "exited" {
            break;
        }
    }
    assert_eq!(state, "exited");
    assert!(stack.find_on_worker("done.txt").is_some());
    again.shutdown().await.unwrap();
}
