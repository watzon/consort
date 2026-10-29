//! The hub tools reach each real harness on this machine.
//!
//! Each test starts a worker with the real adapters against the fake hub,
//! starts a session with no turn, and waits until the harness lists the hub
//! tools through the session's MCP endpoint (the fake hub sees `tool.list`).
//! Harnesses list MCP tools when they start a session, so no model call
//! runs. They are ignored by default because they need the harness
//! installed and signed in. Run one with:
//!
//! ```text
//! CARGO_TARGET_DIR=target/worker cargo test -p consort-worker --test real_harness_mcp -- --ignored --nocapture codex
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{CODE, FakeHub};
use consort_harness::{AccountConfig, HarnessKind};
use consort_proto::ids::{ChannelId, SessionId, TaskId};
use consort_proto::model::{ApprovalMode, Isolation, SessionRole};
use consort_proto::worker_link::{SessionEvent, SessionStart, SessionStop, WorkspaceSpec};
use consort_worker::WorkerConfig;
use consort_worker::config::WorkerToml;

async fn hub_tools_reach(kind: HarnessKind) {
    hub_tools_reach_with(kind, true).await;
}

/// Like `hub_tools_reach`, with the worker's `allow_shell` policy. With the
/// shell off, the session starts with the harness's shell tools removed.
async fn hub_tools_reach_with(kind: HarnessKind, allow_shell: bool) {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    consort_worker::pair(&hub.url, CODE, dir.path(), None, vec![])
        .await
        .unwrap();
    let mut toml = WorkerToml::load(dir.path()).unwrap();
    toml.policy.allow_shell = allow_shell;
    let account = AccountConfig::default_for(kind);
    let account_id = account.id;
    toml.accounts = vec![account];
    toml.save(dir.path()).unwrap();
    // The default Consort home keeps the managed tools between runs.
    let worker = consort_worker::run(WorkerConfig::new(dir.path().to_path_buf()))
        .await
        .unwrap();
    hub.wait_connected().await;

    let session_id = SessionId::new();
    let start = SessionStart {
        session_id,
        role: SessionRole::Task,
        channel_id: ChannelId::new(),
        task_id: Some(TaskId::new()),
        account_id,
        model: String::new(),
        options: Default::default(),
        approval_mode: ApprovalMode::Ask,
        workspace: WorkspaceSpec {
            repos: vec![],
            isolation: Isolation::Clone,
            carry_uncommitted: false,
        },
        system_prompt_append: String::new(),
        allowed_local_mcp_servers: vec![],
        channel_files: vec![],
        extensions: Default::default(),
        resume: None,
        turn: None,
    };
    let response = hub.command("start", start.into()).await;
    println!("session.start: {:?}", response.result);
    assert!(response.result.is_ok());
    let saw = async {
        loop {
            let listed = hub.state.tool_lists.lock().unwrap().contains(&session_id);
            if listed {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let ok = tokio::time::timeout(Duration::from_secs(90), saw)
        .await
        .is_ok();
    for e in hub.events(session_id) {
        if let SessionEvent::Error { message, .. } = &e.event {
            println!("error event: {message}");
        }
    }
    let stop = hub
        .command(
            "stop",
            SessionStop {
                session_id,
                reason: "test".into(),
            }
            .into(),
        )
        .await;
    assert!(stop.result.is_ok());
    worker.shutdown().await.unwrap();
    assert!(ok, "{kind} did not list the hub tools");
    println!("{kind} listed the hub tools through the local MCP endpoint");
}

#[tokio::test]
#[ignore = "needs codex signed in; no model call"]
async fn codex() {
    hub_tools_reach(HarnessKind::Codex).await;
}

#[tokio::test]
#[ignore = "needs claude-agent-acp through npx; no model call"]
async fn claude() {
    hub_tools_reach(HarnessKind::ClaudeCode).await;
}

#[tokio::test]
#[ignore = "needs opencode; no model call"]
async fn opencode() {
    hub_tools_reach(HarnessKind::OpenCode).await;
}

#[tokio::test]
#[ignore = "needs codex signed in; no model call"]
async fn codex_shell_off() {
    hub_tools_reach_with(HarnessKind::Codex, false).await;
}

#[tokio::test]
#[ignore = "needs claude-agent-acp through npx; no model call"]
async fn claude_shell_off() {
    hub_tools_reach_with(HarnessKind::ClaudeCode, false).await;
}
