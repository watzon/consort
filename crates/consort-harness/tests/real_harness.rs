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
    let dir = tempfile::tempdir().unwrap();
    let input = TurnInput {
        text: "Reply with the single word OK".into(),
        attachments: vec![],
    };
    let text = turn_reply(kind, dir.path(), input).await;
    assert!(text.to_uppercase().contains("OK"), "reply was {text:?}");
}

/// A PNG of 32 by 32 red pixels, with stored (not compressed) deflate
/// blocks, so the test needs no image crate.
fn red_png() -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFF_u32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let side = 32_u32;
    // Each row: filter type 0 (none), then red pixels.
    let row: Vec<u8> = std::iter::once(0)
        .chain([255_u8, 0, 0].repeat(side as usize))
        .collect();
    let raw = row.repeat(side as usize);
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    let mut zlib = vec![0x78, 0x01, 0x01];
    let len = raw.len() as u16;
    zlib.extend_from_slice(&len.to_le_bytes());
    zlib.extend_from_slice(&(!len).to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&side.to_be_bytes());
    ihdr.extend_from_slice(&side.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

/// Saves a red PNG where the worker saves attachments, and asks the model
/// for its color. The adapter sends it as image input.
async fn image_turn(kind: HarnessKind) {
    let dir = tempfile::tempdir().unwrap();
    let png = red_png();
    // The hash only names the saved file here, so any value works.
    let blob = consort_proto::model::BlobRef {
        sha256: consort_proto::model::Sha256Hash::from_bytes([0x5a; 32]),
        name: "square.png".into(),
        mime: "image/png".into(),
        size: png.len() as u64,
    };
    let path = consort_harness::attachments::attachment_path(dir.path(), &blob);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &png).unwrap();
    let input = TurnInput {
        text: "What color is the attached image? Do not open any file or run any tool. Reply with one word.".into(),
        attachments: vec![blob],
    };
    let text = turn_reply(kind, dir.path(), input).await;
    assert!(text.to_lowercase().contains("red"), "reply was {text:?}");
}

/// Plays one turn in a new session and returns the last assistant text.
async fn turn_reply(kind: HarnessKind, dir: &std::path::Path, input: TurnInput) -> String {
    let harness = adapter_for(kind).unwrap();
    let account = AccountConfig::default_for(kind);
    let (mut session, mut events) = harness
        .open(&account, spec(dir, ApprovalMode::Ask))
        .await
        .unwrap();
    println!("native ref: {:?}", session.native_ref());
    println!("images: {}", session.capabilities().images);
    let turn = session.turn(input).await.unwrap();
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
    session.close().await.unwrap();
    text
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

#[tokio::test]
#[ignore = "uses the user's Claude plan"]
async fn claude_image_turn() {
    image_turn(HarnessKind::ClaudeCode).await;
}

#[tokio::test]
#[ignore = "uses the user's Codex plan"]
async fn codex_image_turn() {
    image_turn(HarnessKind::Codex).await;
}

#[tokio::test]
#[ignore = "uses the user's Gemini account"]
async fn gemini_image_turn() {
    image_turn(HarnessKind::GeminiCli).await;
}

#[tokio::test]
#[ignore = "uses the user's OpenCode provider; set CONSORT_SMOKE_MODEL to a model with image input"]
async fn opencode_image_turn() {
    image_turn(HarnessKind::OpenCode).await;
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
