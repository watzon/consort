//! Real-download smoke tests. They use the internet, so they are ignored by
//! default. Run them by hand:
//!
//! ```text
//! cargo test -p consort-tools --test smoke -- --ignored --nocapture --test-threads 1
//! ```
//!
//! Each test installs into a new temporary folder. Set
//! `CONSORT_TOOLS_SMOKE_DIR` to use one folder for all tests and keep it.

#![cfg(feature = "install")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use consort_tools::{LaunchSpec, ToolId, ToolStore};

fn root() -> (Option<tempfile::TempDir>, PathBuf) {
    match std::env::var_os("CONSORT_TOOLS_SMOKE_DIR") {
        Some(dir) => (None, PathBuf::from(dir)),
        None => {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().to_owned();
            (Some(temp), path)
        }
    }
}

/// Runs the launch program with other arguments and returns its output.
async fn run(spec: &LaunchSpec, args: &[String]) -> String {
    let mut command = tokio::process::Command::new(&spec.program);
    command.args(args).envs(&spec.env).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(120), command.output())
        .await
        .expect("the program did not finish in 120 s")
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{:?} failed: {text}", spec.program);
    text
}

/// Installs the latest version, prints the layout, and runs the program
/// with `check_args` in place of its launch arguments. Node based tools keep
/// their script as the first argument.
async fn smoke(tool: ToolId, check_args: &[&str]) -> (LaunchSpec, String) {
    let (_temp, root) = root();
    let store = ToolStore::new(root).unwrap();
    let latest = store.latest(&tool).await.unwrap();
    let installed = store.install(&tool, None).await.unwrap();
    assert_eq!(installed.version, latest);
    let mut top: Vec<String> = std::fs::read_dir(&installed.dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    top.sort();
    let spec = store.launch(&tool).unwrap();
    println!(
        "{tool} {} in {}",
        installed.version,
        installed.dir.display()
    );
    println!("  top level: {top:?}");
    println!("  launch: {:?} {:?}", spec.program, spec.args);
    println!("  env keys: {:?}", spec.env.keys().collect::<Vec<_>>());
    assert!(spec.program.is_file(), "{:?} is missing", spec.program);

    let script = spec
        .args
        .first()
        .filter(|arg| arg.ends_with(".js"))
        .cloned();
    let args: Vec<String> = script
        .into_iter()
        .chain(check_args.iter().map(|arg| (*arg).to_owned()))
        .collect();
    let output = run(&spec, &args).await;
    println!(
        "  {}: {}",
        args.join(" "),
        output.lines().next().unwrap_or("")
    );
    let status = store.status();
    let row = status.iter().find(|row| row.tool == tool).unwrap();
    println!("  status: {:?}", row.state);
    (spec, output)
}

#[tokio::test]
#[ignore = "downloads Node from nodejs.org"]
async fn real_node() {
    let (_, output) = smoke(ToolId::Node, &["--version"]).await;
    assert!(output.starts_with("v24."), "{output}");
}

#[tokio::test]
#[ignore = "downloads Claude Code from downloads.claude.ai"]
async fn real_claude_code() {
    let (spec, output) = smoke(ToolId::ClaudeCode, &["--version"]).await;
    assert_eq!(spec.env["DISABLE_AUTOUPDATER"], "1");
    assert!(output.contains("Claude Code"), "{output}");
}

#[tokio::test]
#[ignore = "downloads Codex from github.com"]
async fn real_codex() {
    let (spec, output) = smoke(ToolId::Codex, &["--version"]).await;
    assert_eq!(spec.args, ["app-server"]);
    assert!(output.contains("codex"), "{output}");
}

#[tokio::test]
#[ignore = "downloads OpenCode through the ACP registry"]
async fn real_opencode() {
    let (spec, output) = smoke(ToolId::OpenCode, &["--version"]).await;
    assert_eq!(spec.args, ["acp"]);
    assert!(!output.trim().is_empty());
}

#[tokio::test]
#[ignore = "downloads Node, Claude Code and the ACP adapter from npm"]
async fn real_claude_agent_acp() {
    let (_temp, root) = root();
    let store = ToolStore::new(root).unwrap();
    let installed = store.install(&ToolId::ClaudeAgentAcp, None).await.unwrap();
    let spec = store.launch(&ToolId::ClaudeAgentAcp).unwrap();
    println!(
        "claude-agent-acp {} in {}",
        installed.version,
        installed.dir.display()
    );
    println!("  launch: {:?} {:?}", spec.program, spec.args);
    assert!(std::path::Path::new(&spec.args[0]).is_file());
    assert!(std::path::Path::new(&spec.env["CLAUDE_CODE_EXECUTABLE"]).is_file());
}

#[tokio::test]
#[ignore = "downloads Node and Gemini CLI from npm"]
async fn real_gemini_cli() {
    let (spec, output) = smoke(ToolId::GeminiCli, &["--help"]).await;
    assert_eq!(spec.args.last().map(String::as_str), Some("--acp"));
    // The help text must list the flag that the launch uses.
    assert!(output.contains("--acp"), "{output}");
}

#[tokio::test]
#[ignore = "downloads the preview browser (about 100 MB) from storage.googleapis.com"]
async fn real_chrome_headless_shell() {
    let (spec, output) = smoke(ToolId::ChromeHeadlessShell, &["--version"]).await;
    assert!(spec.args.is_empty());
    assert!(spec.env.is_empty());
    // The version in the output is the pinned version.
    let pinned = consort_tools::ChromePin::pinned().version;
    assert!(output.contains(&pinned), "{output} is not {pinned}");
}
