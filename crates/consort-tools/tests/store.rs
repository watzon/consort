//! Tool store tests against a local HTTP server. No test uses the internet.

#![cfg(feature = "install")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use common::*;
use consort_tools::{ToolError, ToolId, ToolState, ToolStatus};

fn status_of(statuses: &[ToolStatus], tool: &ToolId) -> ToolStatus {
    statuses
        .iter()
        .find(|status| &status.tool == tool)
        .cloned()
        .unwrap_or_else(|| panic!("no status for {tool}"))
}

#[tokio::test]
async fn checksum_mismatch_is_refused_and_leaves_nothing() {
    let server = Server::start().await;
    server.put_node();
    let name = format!("node-v{NODE_VERSION}-darwin-arm64.tar.gz");
    server.put(
        &format!("/node/v{NODE_VERSION}/SHASUMS256.txt"),
        format!("{}  {name}\n", "ab".repeat(32)),
    );
    let temp = tempfile::tempdir().unwrap();
    let store = store(&server, temp.path(), None);

    let result = store.install(&ToolId::Node, None).await;
    assert!(
        matches!(result, Err(ToolError::ChecksumMismatch { .. })),
        "{result:?}"
    );
    assert!(!temp.path().join("node").exists());
    assert_eq!(store.installed(&ToolId::Node), None);

    let status = status_of(&store.status(), &ToolId::Node);
    assert_eq!(status.state, ToolState::Failed);
    assert!(status.detail.unwrap().contains("checksum mismatch"));
}

#[tokio::test]
async fn path_traversal_in_an_archive_is_refused() {
    let server = Server::start().await;
    server.put_registry();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("tools");
    let store = store(&server, &root, None);

    let tool = ToolId::Acp("evil".into());
    let result = store.install(&tool, None).await;
    assert!(
        matches!(result, Err(ToolError::UnsafePath { .. })),
        "{result:?}"
    );
    assert!(!root.join("acp-evil").exists());
    assert!(!root.join("evil.txt").exists());
    assert!(!temp.path().join("evil.txt").exists());
}

#[tokio::test]
async fn install_then_status_is_ready() {
    let server = Server::start().await;
    server.put_codex("1.0.0");
    let temp = tempfile::tempdir().unwrap();
    let store = store(&server, temp.path(), None);

    let installed = store.install(&ToolId::Codex, None).await.unwrap();
    assert_eq!(installed.version, "1.0.0");
    assert_eq!(installed.dir, temp.path().join("codex/1.0.0"));
    assert!(installed.dir.join("bin/codex").is_file());
    assert_eq!(names(&temp.path().join("codex")), ["1.0.0", "current"]);
    assert_eq!(store.installed(&ToolId::Codex), Some(installed));

    let statuses = store.status();
    let codex = status_of(&statuses, &ToolId::Codex);
    assert_eq!(codex.state, ToolState::Ready);
    assert_eq!(codex.version.as_deref(), Some("1.0.0"));
    assert_eq!(
        status_of(&statuses, &ToolId::Node).state,
        ToolState::Missing
    );
    // The built-in tools are always listed, in a fixed order.
    let order: Vec<ToolId> = statuses.iter().map(|status| status.tool.clone()).collect();
    assert_eq!(order, ToolId::builtin());
}

#[tokio::test]
async fn newer_latest_gives_update_available_and_is_saved() {
    let server = Server::start().await;
    server.put_codex("1.0.0");
    let temp = tempfile::tempdir().unwrap();
    let store = store(&server, temp.path(), None);
    store.install(&ToolId::Codex, None).await.unwrap();
    assert_eq!(store.last_check(), None);

    server.set_codex_latest("1.1.0");
    let statuses = store.check_updates().await;
    let codex = status_of(&statuses, &ToolId::Codex);
    assert_eq!(codex.state, ToolState::UpdateAvailable);
    assert_eq!(codex.version.as_deref(), Some("1.0.0"));
    assert_eq!(codex.latest.as_deref(), Some("1.1.0"));
    assert!(store.last_check().is_some());

    // Node has no fixture, so its check fails, and the status says so.
    let node = status_of(&statuses, &ToolId::Node);
    assert_eq!(node.state, ToolState::Missing);
    assert!(node.detail.unwrap().starts_with("The update check failed"));

    // A new store on the same root reads the result from state.json.
    let again = common::store(&server, temp.path(), None);
    let codex = status_of(&again.status(), &ToolId::Codex);
    assert_eq!(codex.state, ToolState::UpdateAvailable);
    assert_eq!(codex.latest.as_deref(), Some("1.1.0"));
    assert!(again.last_check().is_some());
}

#[tokio::test]
async fn keeps_the_previous_version_and_removes_older_ones() {
    let server = Server::start().await;
    for version in ["1.0.0", "1.1.0", "1.2.0"] {
        server.put_codex(version);
    }
    let temp = tempfile::tempdir().unwrap();
    let store = store(&server, temp.path(), None);
    let codex_dir = temp.path().join("codex");

    for version in ["1.0.0", "1.1.0", "1.2.0"] {
        store.install(&ToolId::Codex, Some(version)).await.unwrap();
    }
    assert_eq!(names(&codex_dir), ["1.1.0", "1.2.0", "current"]);
    assert_eq!(store.installed(&ToolId::Codex).unwrap().version, "1.2.0");

    // Going back to the kept version needs no download.
    let asset = "/github/download/rust-v1.1.0/codex-package-aarch64-apple-darwin.tar.gz";
    let hits = server.hits(asset);
    store.install(&ToolId::Codex, Some("1.1.0")).await.unwrap();
    assert_eq!(server.hits(asset), hits);
    assert_eq!(store.installed(&ToolId::Codex).unwrap().version, "1.1.0");
    assert_eq!(names(&codex_dir), ["1.1.0", "1.2.0", "current"]);
}

#[tokio::test]
async fn a_binary_without_sha256_is_refused() {
    let server = Server::start().await;
    server.put_registry();
    let temp = tempfile::tempdir().unwrap();
    let store = store(&server, temp.path(), None);

    let result = store.install(&ToolId::Acp("nosha".into()), None).await;
    assert!(
        matches!(result, Err(ToolError::MissingChecksum { .. })),
        "{result:?}"
    );
    assert_eq!(server.hits("/files/nosha.tar.gz"), 0);
    assert!(!temp.path().join("acp-nosha").exists());
}

#[tokio::test]
async fn uvx_and_unknown_agents_are_refused() {
    let server = Server::start().await;
    server.put_registry();
    let temp = tempfile::tempdir().unwrap();
    let store = store(&server, temp.path(), None);

    let result = store.install(&ToolId::Acp("uv-only".into()), None).await;
    assert!(
        matches!(result, Err(ToolError::Unsupported { .. })),
        "{result:?}"
    );
    let result = store.install(&ToolId::Acp("nobody".into()), None).await;
    assert!(
        matches!(result, Err(ToolError::UnknownAgent { .. })),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_bad_version_text_is_refused() {
    let server = Server::start().await;
    let temp = tempfile::tempdir().unwrap();
    let store = store(&server, temp.path(), None);
    let result = store.install(&ToolId::Codex, Some("../x")).await;
    assert!(
        matches!(result, Err(ToolError::Unsupported { .. })),
        "{result:?}"
    );
    assert!(common::names(temp.path()).is_empty());
}

#[tokio::test]
async fn a_claude_manifest_with_a_bad_signature_is_refused() {
    let server = Server::start().await;
    server.put_claude();
    // Change one byte of the signed manifest.
    let mut changed = TEST_MANIFEST.to_vec();
    let at = changed.iter().position(|b| *b == b'5').unwrap();
    changed[at] = b'6';
    server.put(&format!("/claude/{CLAUDE_VERSION}/manifest.json"), changed);
    let temp = tempfile::tempdir().unwrap();
    let store = store(&server, temp.path(), None);

    let result = store.install(&ToolId::ClaudeCode, None).await;
    assert!(
        matches!(result, Err(ToolError::Signature { .. })),
        "{result:?}"
    );
    assert!(!temp.path().join("claude-code").exists());
    assert_eq!(
        server.hits(&format!("/claude/{CLAUDE_VERSION}/darwin-arm64/claude")),
        0
    );
}

#[tokio::test]
async fn a_signed_manifest_of_another_version_is_refused() {
    let server = Server::start().await;
    server.put_claude();
    server.put("/claude/9.9.2/manifest.json", TEST_MANIFEST);
    server.put("/claude/9.9.2/manifest.json.sig", TEST_MANIFEST_SIG);
    let temp = tempfile::tempdir().unwrap();
    let store = store(&server, temp.path(), None);

    let result = store.install(&ToolId::ClaudeCode, Some("9.9.2")).await;
    assert!(
        matches!(result, Err(ToolError::InvalidResponse { .. })),
        "{result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_installs_of_one_tool_download_once() {
    let server = Server::start().await;
    server.put_codex("1.0.0");
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(store(&server, temp.path(), None));

    let mut tasks = Vec::new();
    for _ in 0..4 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            store.install(&ToolId::Codex, None).await
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap().version, "1.0.0");
    }
    let asset = "/github/download/rust-v1.0.0/codex-package-aarch64-apple-darwin.tar.gz";
    assert_eq!(server.hits(asset), 1);
    assert_eq!(names(&temp.path().join("codex")), ["1.0.0", "current"]);
}

#[tokio::test]
async fn launch_of_a_missing_tool_fails() {
    let server = Server::start().await;
    let temp = tempfile::tempdir().unwrap();
    let store = store(&server, temp.path(), None);
    let result = store.launch(&ToolId::Codex);
    assert!(
        matches!(
            result,
            Err(ToolError::NotInstalled {
                tool: ToolId::Codex
            })
        ),
        "{result:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn launch_specs_for_every_tool() {
    let server = Server::start().await;
    server.put_node();
    server.put_claude();
    server.put_codex("1.0.0");
    server.put_registry();
    server.put_npm_latest("@agentclientprotocol/claude-agent-acp", "0.99.0");
    server.put_npm_latest("@google/gemini-cli", "0.66.0");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("tools");
    let log = temp.path().join("npm.log");
    let npm = fake_npm(temp.path(), &log);
    let store = store(&server, &root, Some(npm));

    // The ACP adapter needs Node and Claude Code. They install first.
    store.install(&ToolId::ClaudeAgentAcp, None).await.unwrap();
    assert!(store.installed(&ToolId::Node).is_some());
    assert!(store.installed(&ToolId::ClaudeCode).is_some());
    for tool in [
        ToolId::GeminiCli,
        ToolId::Codex,
        ToolId::OpenCode,
        ToolId::Acp("fake-npx".into()),
        ToolId::Acp("rawbin".into()),
    ] {
        store.install(&tool, None).await.unwrap();
    }

    let node_dir = root.join(format!("node/{NODE_VERSION}"));
    let node = node_dir.join("bin/node");
    let node_path_prefix = node_dir.join("bin").display().to_string();

    let spec = store.launch(&ToolId::Node).unwrap();
    assert_eq!(spec.program, node);
    assert!(spec.args.is_empty());
    // The npm link in the Node archive is kept.
    assert!(node_dir.join("bin/npm").exists());

    let claude = store.launch(&ToolId::ClaudeCode).unwrap();
    assert_eq!(
        claude.program,
        root.join(format!("claude-code/{CLAUDE_VERSION}/claude"))
    );
    assert_eq!(std::fs::read(&claude.program).unwrap(), FAKE_CLAUDE);
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&claude.program)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);
    }
    assert_eq!(claude.env["DISABLE_AUTOUPDATER"], "1");

    let acp = store.launch(&ToolId::ClaudeAgentAcp).unwrap();
    assert_eq!(acp.program, node);
    assert_eq!(
        acp.args,
        [root
            .join("claude-agent-acp/0.99.0/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js")
            .display()
            .to_string()]
    );
    assert_eq!(
        acp.env["CLAUDE_CODE_EXECUTABLE"],
        claude.program.display().to_string()
    );
    assert_eq!(acp.env["DISABLE_AUTOUPDATER"], "1");
    assert!(acp.env["PATH"].starts_with(&node_path_prefix));

    let codex = store.launch(&ToolId::Codex).unwrap();
    assert_eq!(codex.program, root.join("codex/1.0.0/bin/codex"));
    assert_eq!(codex.args, ["app-server"]);
    assert!(
        codex.env["PATH"].starts_with(&root.join("codex/1.0.0/codex-path").display().to_string())
    );

    let opencode = store.launch(&ToolId::OpenCode).unwrap();
    assert_eq!(opencode.program, root.join("opencode/2.2.1/opencode"));
    assert_eq!(opencode.args, ["acp"]);
    assert_eq!(opencode.env["OPENCODE_DISABLE_AUTOUPDATE"], "true");

    let gemini = store.launch(&ToolId::GeminiCli).unwrap();
    assert_eq!(gemini.program, node);
    assert_eq!(
        gemini.args,
        [
            root.join("gemini-cli/0.66.0/node_modules/@google/gemini-cli/bundle/gemini.js")
                .display()
                .to_string(),
            "--acp".to_owned()
        ]
    );

    let npx = store.launch(&ToolId::Acp("fake-npx".into())).unwrap();
    assert_eq!(npx.program, node);
    assert_eq!(
        npx.args,
        [
            root.join("acp-fake-npx/1.0.0/node_modules/fake-npx-agent/bin/cli")
                .display()
                .to_string(),
            "--acp".to_owned()
        ]
    );
    assert_eq!(npx.env["FOO"], "bar");

    let raw = store.launch(&ToolId::Acp("rawbin".into())).unwrap();
    assert_eq!(raw.program, root.join("acp-rawbin/1.0.0/rawbin"));
    assert_eq!(raw.args, ["serve"]);
    assert_eq!(raw.env["RAW"], "1");

    // The built-in npm tools turn off optional packages and scripts. An npx
    // agent installs like npx would.
    let log = std::fs::read_to_string(&log).unwrap();
    let line = |needle: &str| {
        log.lines()
            .find(|line| line.contains(needle))
            .unwrap()
            .to_owned()
    };
    let acp_line = line("claude-agent-acp@0.99.0");
    assert!(acp_line.contains("--omit=optional") && acp_line.contains("--ignore-scripts"));
    assert!(line("gemini-cli@0.66.0").contains("--ignore-scripts"));
    assert!(!line("fake-npx-agent@1.0.0").contains("--ignore-scripts"));

    let statuses = store.status();
    for status in &statuses {
        assert_eq!(status.state, ToolState::Ready, "{status:?}");
    }
    let tools: Vec<String> = statuses
        .iter()
        .map(|status| status.tool.to_string())
        .collect();
    assert!(tools.contains(&"acp:fake-npx".to_owned()));
    assert!(tools.contains(&"acp:rawbin".to_owned()));
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_npm_install_leaves_nothing() {
    let server = Server::start().await;
    server.put_node();
    server.put_registry();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("tools");
    let npm = fake_npm(temp.path(), &temp.path().join("npm.log"));
    let store = store(&server, &root, Some(npm));

    let tool = ToolId::Acp("broken-npx".into());
    let result = store.install(&tool, None).await;
    match result {
        Err(ToolError::Npm { message }) => assert!(message.contains("404"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert!(!root.join("acp-broken-npx").exists());
    // Node was installed first, as a dependency.
    assert!(store.installed(&ToolId::Node).is_some());
    let status = status_of(&store.status(), &tool);
    assert_eq!(status.state, ToolState::Failed);
}
