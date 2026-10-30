//! `item.detail` against the fake hub: the worker keeps the whole output of
//! a command and uploads it as a blob when the hub asks.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use common::{CODE, FakeHub};

use consort_harness::{AccountConfig, Harness, HarnessKind};
use consort_proto::ids::{ChannelId, ItemId, SessionId};
use consort_proto::model::{ApprovalMode, Isolation, ItemPayload, SessionRole};
use consort_proto::worker_link::{
    ItemDetail, ItemDetailResult, SessionEvent, SessionStart, TurnInput, WorkspaceSpec,
};
use consort_testkit::{FakeHarness, FakeScript, FakeStep};
use consort_worker::config::WorkerToml;
use consort_worker::{Harnesses, WorkerConfig};

#[tokio::test]
async fn the_hub_gets_the_whole_output_of_a_command_as_a_blob() {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    consort_worker::pair(&hub.url, CODE, dir.path(), None, vec![])
        .await
        .unwrap();
    let mut toml = WorkerToml::load(dir.path()).unwrap();
    let account = AccountConfig::default_for(HarnessKind::ClaudeCode);
    let account_id = account.id;
    toml.accounts = vec![account];
    toml.save(dir.path()).unwrap();
    let output = (1..=500).map(|n| format!("line {n}\n")).collect::<String>();
    let fake = FakeHarness::new(
        HarnessKind::ClaudeCode,
        FakeScript {
            turns: vec![vec![
                FakeStep::Command {
                    command: "cargo build".into(),
                    output: output.clone(),
                },
                FakeStep::Text("Built.".into()),
            ]],
        },
    );
    let mut harnesses: Harnesses = HashMap::new();
    harnesses.insert(HarnessKind::ClaudeCode, Arc::new(fake) as Arc<dyn Harness>);
    let _worker = consort_worker::run_with(
        WorkerConfig {
            config_dir: dir.path().to_path_buf(),
            consort_home: Some(dir.path().join("home")),
            check_tool_updates: false,
        },
        harnesses,
    )
    .await
    .unwrap();
    hub.wait_connected().await;

    let session_id = SessionId::new();
    let start = SessionStart {
        session_id,
        role: SessionRole::Task,
        channel_id: ChannelId::new(),
        task_id: None,
        account_id,
        model: "fake-model".into(),
        options: Default::default(),
        approval_mode: ApprovalMode::Auto,
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
        turn: Some(TurnInput {
            text: "Build it".into(),
            attachments: vec![],
        }),
        turn_approval_mode: None,
    };
    let started = hub.command("start-1", start.into()).await;
    assert!(started.result.is_ok(), "{:?}", started.result);

    let item_id: ItemId = hub
        .wait_for("the command item", |hub| {
            hub.events(session_id)
                .into_iter()
                .find_map(|e| match e.event {
                    SessionEvent::Item(item) => match item.payload {
                        ItemPayload::Command(_) => Some(item.id),
                        _ => None,
                    },
                    _ => None,
                })
        })
        .await;

    let answer = hub
        .command(
            "detail-1",
            ItemDetail {
                session_id,
                item_id,
            }
            .into(),
        )
        .await;
    let result: ItemDetailResult =
        serde_json::from_value(answer.result.expect("the worker has the output")).unwrap();
    assert_eq!(result.mime, "text/plain");
    assert_eq!(result.size, output.len() as u64);
    let uploaded = hub.state.blobs.lock().unwrap().clone();
    assert!(
        uploaded.iter().any(|p| p.ends_with(result.sha256.as_str())),
        "{uploaded:?}"
    );

    // An item with nothing kept is not found, with a sentence for the app.
    let missing = hub
        .command(
            "detail-2",
            ItemDetail {
                session_id,
                item_id: ItemId::new(),
            }
            .into(),
        )
        .await;
    let error = missing.result.expect_err("an unknown item fails");
    assert_eq!(error.code, consort_proto::error::ErrorCode::NotFound);
}
