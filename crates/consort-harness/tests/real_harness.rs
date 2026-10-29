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
        allow_shell: true,
        system_prompt_append: "You are a smoke test. Keep answers short.".into(),
        mcp_servers: Vec::new(),
        env: BTreeMap::new(),
        extras_enabled: true,
        skills: None,
        resume: None,
        process_watcher: None,
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

/// Runs the real `claude auth login --claudeai` in a pseudo-terminal with a
/// new, empty `CLAUDE_CONFIG_DIR`, waits for the sign-in URL, and stops the
/// process without a sign-in. Set `CONSORT_CLAUDE` to the `claude` program,
/// or have `claude` on PATH. No browser opens: `BROWSER` is a no-op.
#[tokio::test]
#[ignore = "needs Claude Code and the network; no sign-in happens"]
async fn claude_login_shows_a_url_and_waits_for_a_code() {
    use consort_harness::signin::claude::{ClaudeCommand, ClaudeLogin, LoginKind};
    let dir = tempfile::tempdir().unwrap();
    let mut account = AccountConfig::default_for(HarnessKind::ClaudeCode);
    account.profile = true;
    account.runtime.env.insert(
        "CLAUDE_CONFIG_DIR".into(),
        dir.path().join("claude").to_string_lossy().into_owned(),
    );
    account.runtime.env_remove = consort_harness::account::LOGIN_VARIABLES
        .iter()
        .map(|v| (*v).to_owned())
        .collect();
    let mut command = ClaudeCommand::for_account(&account).unwrap();
    if let Some(program) = std::env::var_os("CONSORT_CLAUDE") {
        command.program = program.into();
    }
    let status = command.status().await.unwrap();
    assert!(!status.logged_in, "a new folder has no login");
    let mut login = ClaudeLogin::start(&command, LoginKind::Plan, dir.path()).unwrap();
    let url = login.url(Duration::from_secs(60)).await.unwrap();
    println!("sign-in URL: {}...", &url[..url.len().min(60)]);
    assert!(url.starts_with("https://"), "{url}");
    assert!(url.contains("oauth"), "{url}");
    // It waits for the code: it has not exited after a few seconds.
    let waited = login.wait(Duration::from_secs(3)).await;
    assert!(waited.is_err(), "the login waits for a code: {waited:?}");
    login.kill();
}

/// Starts a real ChatGPT sign-in by device code in a new, empty
/// `CODEX_HOME`, checks that Codex gives a URL and a code, and cancels it.
/// Then it signs out (nothing to sign out) and reads the account. Set
/// `CONSORT_CODEX` to the `codex` program, or have `codex` on PATH.
#[tokio::test]
#[ignore = "needs Codex and the network; no sign-in happens"]
async fn codex_device_code_sign_in_starts_and_cancels() {
    use consort_harness::codex::login::{ChatgptLogin, ChatgptStep, logout, read_account};
    let dir = tempfile::tempdir().unwrap();
    let mut account = AccountConfig::default_for(HarnessKind::Codex);
    account.profile = true;
    account.binary = std::env::var("CONSORT_CODEX").ok();
    account.runtime.env.insert(
        "CODEX_HOME".into(),
        dir.path().to_string_lossy().into_owned(),
    );
    let (login, step) = ChatgptLogin::start(&account, true).await.unwrap();
    let ChatgptStep::DeviceCode { url, user_code } = step else {
        panic!("{step:?}")
    };
    println!("device URL {url}, code length {}", user_code.len());
    assert!(url.starts_with("https://"));
    assert!(!user_code.is_empty());
    login.cancel().await;
    logout(&account).await.unwrap();
    let info = read_account(&account).await.unwrap();
    assert!(info.account.is_none());
}
