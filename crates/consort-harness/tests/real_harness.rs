//! Smoke tests against the real harnesses on this machine.
//!
//! They are ignored by default, because they need the harness installed and
//! signed in, and a turn uses the user's own plan. Run one with:
//!
//! ```text
//! CARGO_TARGET_DIR=target/worker cargo test -p consort-harness --test real_harness -- --ignored --nocapture claude_probe
//! ```
//!
//! Set `CONSORT_HARNESS_RECORD=/tmp/acp.jsonl` to record the raw harness
//! messages for mapper fixtures.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::time::Duration;

use consort_harness::{
    AccountConfig, ApprovalMode, HarnessKind, SessionEvent, SessionSpec, TurnInput, adapter_for,
};
use consort_proto::ids::SessionId;
use consort_proto::worker_link::TurnEndReason;

fn spec(dir: &std::path::Path, mode: ApprovalMode) -> SessionSpec {
    SessionSpec {
        session_id: SessionId::new(),
        workspace: dir.to_path_buf(),
        runtime_dir: dir.to_path_buf(),
        // OpenCode's default model can need a provider that is not signed
        // in. Set CONSORT_SMOKE_MODEL to pick one, such as
        // `deepseek/deepseek-flash`.
        model: std::env::var("CONSORT_SMOKE_MODEL").unwrap_or_default(),
        options: Default::default(),
        approval_mode: mode,
        system_prompt_append: "You are a smoke test. Keep answers short.".into(),
        mcp_servers: Vec::new(),
        env: BTreeMap::new(),
        resume: None,
    }
}

async fn probe(kind: HarnessKind) {
    let harness = adapter_for(kind).unwrap();
    let status = harness.probe(&AccountConfig::default_for(kind)).await;
    println!("{kind}: {status:#?}");
}

async fn one_turn(kind: HarnessKind) {
    let harness = adapter_for(kind).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let account = AccountConfig::default_for(kind);
    let (mut session, mut events) = harness
        .open(&account, spec(dir.path(), ApprovalMode::Ask))
        .await
        .unwrap();
    println!("native ref: {:?}", session.native_ref());
    let turn = session
        .turn(TurnInput {
            text: "Reply with the single word OK".into(),
            attachments: vec![],
        })
        .await
        .unwrap();
    let mut text = String::new();
    let ended = tokio::time::timeout(Duration::from_secs(240), async {
        while let Some(event) = events.recv().await {
            println!("{}", serde_json::to_string(&event).unwrap());
            if let SessionEvent::Item(item) = &event
                && let consort_proto::model::ItemPayload::AssistantMessage(m) = &item.payload
            {
                text = m.text.clone();
            }
            if let SessionEvent::TurnEnded {
                turn_id, reason, ..
            } = event
                && turn_id == turn
            {
                return reason;
            }
        }
        panic!("events ended before the turn ended");
    })
    .await
    .unwrap();
    assert_eq!(ended, TurnEndReason::Completed);
    assert!(text.to_uppercase().contains("OK"), "reply was {text:?}");
    session.close().await.unwrap();
}

#[tokio::test]
#[ignore = "needs claude-agent-acp through npx; no model call"]
async fn claude_probe() {
    probe(HarnessKind::ClaudeCode).await;
}

#[tokio::test]
#[ignore = "needs opencode; no model call"]
async fn opencode_probe() {
    probe(HarnessKind::OpenCode).await;
}

#[tokio::test]
#[ignore = "needs codex; no model call"]
async fn codex_probe() {
    probe(HarnessKind::Codex).await;
}

#[tokio::test]
#[ignore = "uses the user's Claude plan"]
async fn claude_turn() {
    one_turn(HarnessKind::ClaudeCode).await;
}

#[tokio::test]
#[ignore = "uses the user's OpenCode provider"]
async fn opencode_turn() {
    one_turn(HarnessKind::OpenCode).await;
}

#[tokio::test]
#[ignore = "uses the user's Codex plan"]
async fn codex_turn() {
    one_turn(HarnessKind::Codex).await;
}
