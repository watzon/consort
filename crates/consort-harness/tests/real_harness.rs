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
use consort_proto::session_host_link::TurnEndReason;

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
        coordinator: false,
        system_prompt_append: "You are a smoke test. Keep answers short.".into(),
        mcp_servers: Vec::new(),
        env: BTreeMap::new(),
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

/// One HTTP port that plays the model and the `consort` MCP server. The model
/// asks for `apply_patch`, then finds an MCP tool and calls it, then answers.
struct LocalModel {
    port: u16,
    tools_called: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    requests: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

impl LocalModel {
    /// `first_step` 3 answers at once, with no tool.
    fn start(first_step: usize) -> Self {
        use std::io::{Read, Write};
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let tools_called = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (called, seen) = (tools_called.clone(), requests.clone());
        std::thread::spawn(move || {
            let step = std::sync::atomic::AtomicUsize::new(first_step);
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut raw = Vec::new();
                let mut buffer = [0_u8; 65536];
                let (path, body) = loop {
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        break (String::new(), Vec::new());
                    }
                    raw.extend_from_slice(&buffer[..read]);
                    let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                    let length: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    if raw.len() >= end + 4 + length {
                        let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
                        break (path, raw[end + 4..end + 4 + length].to_vec());
                    }
                };
                let value: serde_json::Value =
                    serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                let (status, kind, reply) = if path.starts_with("/mcp") {
                    let result = match value["method"].as_str().unwrap_or("") {
                        "initialize" => serde_json::json!({
                            "protocolVersion": value["params"]["protocolVersion"],
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "consort", "version": "1"},
                        }),
                        "tools/list" => serde_json::json!({"tools": [{
                            "name": "echo",
                            "description": "Echo",
                            "inputSchema": {"type": "object", "properties": {}},
                        }]}),
                        "tools/call" => {
                            called.fetch_add(1, Ordering::SeqCst);
                            serde_json::json!({"content": [{"type": "text", "text": "echoed"}]})
                        }
                        _ => serde_json::json!({}),
                    };
                    if value.get("id").is_some() {
                        let reply = serde_json::json!({"jsonrpc": "2.0", "id": value["id"], "result": result});
                        ("200 OK", "application/json", reply.to_string())
                    } else {
                        ("202 Accepted", "application/json", String::new())
                    }
                } else if path.ends_with("/responses") {
                    seen.lock().unwrap().push(value);
                    let items = match step.fetch_add(1, Ordering::SeqCst) {
                        0 => {
                            serde_json::json!({"type": "custom_tool_call", "call_id": "c1", "name": "apply_patch", "input": "*** Begin Patch\n*** Add File: outside.txt\n+x\n*** End Patch\n"})
                        }
                        1 => {
                            serde_json::json!({"type": "tool_search_call", "call_id": "s1", "execution": "client", "status": "completed", "arguments": {"query": "echo", "limit": 5}})
                        }
                        2 => {
                            serde_json::json!({"type": "function_call", "call_id": "c2", "name": "echo", "namespace": "mcp__consort", "arguments": "{}"})
                        }
                        _ => {
                            serde_json::json!({"type": "message", "role": "assistant", "id": "m1", "content": [{"type": "output_text", "text": "done"}]})
                        }
                    };
                    let usage = serde_json::json!({"input_tokens": 1, "output_tokens": 1, "total_tokens": 2, "input_tokens_details": null, "output_tokens_details": null});
                    let events = [
                        serde_json::json!({"type": "response.created", "response": {"id": "r"}}),
                        serde_json::json!({"type": "response.output_item.done", "item": items}),
                        serde_json::json!({"type": "response.completed", "response": {"id": "r", "usage": usage}}),
                    ];
                    let text: String = events
                        .iter()
                        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
                        .collect();
                    ("200 OK", "text/event-stream", text)
                } else {
                    ("404 Not Found", "text/plain", String::new())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Self {
            port,
            tools_called,
            requests,
        }
    }
}

/// The native binary of the npm package of `codex`, which needs no Node.js.
/// `CONSORT_CODEX_BINARY` picks another one, such as a managed install.
fn codex_native() -> std::path::PathBuf {
    if let Some(binary) = std::env::var_os("CONSORT_CODEX_BINARY") {
        return binary.into();
    }
    let wrapper = consort_harness::process::resolve_program("codex").expect("codex on PATH");
    let wrapper = std::fs::canonicalize(wrapper).unwrap();
    let package = wrapper.parent().unwrap().parent().unwrap();
    let (arch, triple) = if cfg!(target_arch = "aarch64") {
        ("arm64", format!("aarch64-{}", codex_system()))
    } else {
        ("x64", format!("x86_64-{}", codex_system()))
    };
    let system = if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    package
        .join(format!("node_modules/@openai/codex-{system}-{arch}/vendor"))
        .join(triple)
        .join("bin/codex")
}

fn codex_system() -> &'static str {
    if cfg!(target_os = "macos") {
        "apple-darwin"
    } else {
        "unknown-linux-musl"
    }
}

/// An account of real Codex in the real boundary, with a home folder in
/// `root` that uses the local model and needs no sign-in.
fn local_codex(root: &std::path::Path, model: &LocalModel) -> AccountConfig {
    let home = root.join("profile/codex");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        format!(
            "cli_auth_credentials_store = \"file\"\ncheck_for_update_on_startup = false\nmodel_provider = \"local\"\n[model_providers.local]\nname = \"local\"\nbase_url = \"http://127.0.0.1:{}/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\n",
            model.port
        ),
    )
    .unwrap();
    let native = codex_native();
    let mut account = AccountConfig::default_for(HarnessKind::Codex);
    account.binary = Some(native.to_string_lossy().into_owned());
    account.home = Some(home.to_string_lossy().into_owned());
    account.runtime.boundary = Some(consort_harness::boundary::BoundaryConfig {
        kind: consort_harness::boundary::detect().unwrap(),
        read_only: vec![native.parent().unwrap().parent().unwrap().to_path_buf()],
        helper: std::env::current_exe().ok(),
    });
    account
}

/// Real `codex app-server` in the real boundary, with a local model and no
/// account. The model calls `apply_patch` and a Consort tool. Only the Consort
/// tool runs, and it asks for nothing.
#[tokio::test]
#[ignore = "needs codex; no model call"]
async fn codex_in_the_boundary_runs_only_consort_tools() {
    let root = tempfile::tempdir().unwrap();
    let root_path = std::fs::canonicalize(root.path()).unwrap();
    let (session, runtime) = (root_path.join("session"), root_path.join("runtime"));
    for dir in [&session, &runtime] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let model = LocalModel::start(0);
    let account = local_codex(&root_path, &model);
    let mut spec = spec(&session, ApprovalMode::Ask);
    spec.runtime_dir = runtime.clone();
    spec.model = "gpt-5.5".into();
    spec.mcp_servers = vec![consort_harness::McpServerSpec {
        name: "consort".into(),
        url: format!("http://127.0.0.1:{}/mcp", model.port),
        bearer_token: Some("token".into()),
        bridge: None,
    }];
    let harness = adapter_for(HarnessKind::Codex).unwrap();
    let started = std::time::Instant::now();
    let (mut session_handle, mut events) = harness.open(&account, spec).await.unwrap();
    let turn = session_handle
        .turn(TurnInput {
            text: "Go".into(),
            attachments: vec![],
        })
        .await
        .unwrap();
    let mut text = String::new();
    let mut asked = false;
    let reason = tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(event) = events.recv().await {
            println!(
                "{:?} {}",
                started.elapsed(),
                serde_json::to_string(&event).unwrap()
            );
            match event {
                SessionEvent::RequestOpened(_) => asked = true,
                SessionEvent::Item(item) => {
                    if let consort_proto::model::ItemPayload::AssistantMessage(m) = item.payload {
                        text = m.text;
                    }
                }
                SessionEvent::TurnEnded {
                    turn_id, reason, ..
                } if turn_id == turn => return reason,
                _ => {}
            }
        }
        panic!("events ended before the turn ended");
    })
    .await
    .unwrap();
    assert_eq!(reason, TurnEndReason::Completed);
    assert_eq!(text, "done");
    assert!(!asked, "Codex asks for nothing");
    let called = model.tools_called.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(called, 1, "the Consort tool ran");
    assert!(
        !session.join("outside.txt").exists(),
        "apply_patch wrote a file"
    );
    for request in model.requests.lock().unwrap().iter() {
        let names: Vec<String> = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                t["name"]
                    .as_str()
                    .unwrap_or(t["type"].as_str().unwrap())
                    .to_owned()
            })
            .collect();
        println!("tools: {names:?}");
        for off in [
            "exec_command",
            "write_stdin",
            "shell",
            "view_image",
            "web_search",
        ] {
            assert!(!names.iter().any(|n| n == off), "{off} in {names:?}");
        }
    }
    session_handle.close().await.ok();
}

/// The boundary gives real Codex no port that it does not list. One question
/// to the local model, which is not an MCP endpoint here, ends in an error.
#[tokio::test]
#[ignore = "needs codex; no model call"]
async fn codex_in_the_boundary_cannot_reach_a_port_that_is_not_listed() {
    let root = tempfile::tempdir().unwrap();
    let root_path = std::fs::canonicalize(root.path()).unwrap();
    let folder = root_path.join("check");
    std::fs::create_dir_all(&folder).unwrap();
    let model = LocalModel::start(3);
    let account = local_codex(&root_path, &model);
    let harness = adapter_for(HarnessKind::Codex).unwrap();
    // Codex keeps trying, so a limit ends the wait. The caller of a check has
    // its own limit too.
    let answer = tokio::time::timeout(
        Duration::from_secs(30),
        harness.one_shot(
            &account,
            &folder,
            consort_harness::OneShot {
                model: "gpt-5.5".into(),
                system: "Answer in one word.".into(),
                prompt: "Go".into(),
            },
        ),
    )
    .await;
    println!("{answer:?}");
    assert!(!matches!(answer, Ok(Ok(_))));
    assert!(model.requests.lock().unwrap().is_empty());
}
