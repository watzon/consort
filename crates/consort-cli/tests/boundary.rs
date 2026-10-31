//! A real process in the real boundary of this machine: `sandbox-exec` on a
//! Mac, `bwrap` on Linux. The test skips only where this system has no
//! boundary. `scripts/test-linux-boundary.sh` runs it in a Linux container.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use consort_harness::HarnessKind;
use consort_harness::boundary::{BoundaryConfig, detect, enter};
use consort_harness::process::ProcessSpec;

/// Folders that the test does not give to the boundary, except `session`,
/// `runtime` and `profile`.
struct Folders {
    _root: tempfile::TempDir,
    session: PathBuf,
    runtime: PathBuf,
    profile: PathBuf,
    outside: PathBuf,
}

fn folders() -> Folders {
    let root = tempfile::tempdir().expect("a temp folder");
    let root_path = std::fs::canonicalize(root.path()).expect("a real path");
    let make = |name: &str| {
        let path = root_path.join(name);
        std::fs::create_dir_all(&path).expect("a folder");
        path
    };
    Folders {
        session: make("session"),
        runtime: make("runtime"),
        profile: make("profile"),
        outside: make("outside"),
        _root: root,
    }
}

fn spec(
    program: &Path,
    args: Vec<String>,
    folders: &Folders,
    extra: &[(&str, String)],
) -> ProcessSpec {
    let mut env = BTreeMap::new();
    env.insert(
        "CLAUDE_CONFIG_DIR".to_owned(),
        folders.profile.to_string_lossy().into_owned(),
    );
    for (key, value) in extra {
        env.insert((*key).to_owned(), value.clone());
    }
    ProcessSpec {
        program: program.to_path_buf(),
        args,
        env,
        env_clear: true,
        env_remove: Vec::new(),
        cwd: Some(folders.session.clone()),
        watcher: None,
    }
}

async fn run(spec: ProcessSpec) -> String {
    let mut command = tokio::process::Command::new(&spec.program);
    command
        .args(&spec.args)
        .env_clear()
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    let output = tokio::time::timeout(Duration::from_secs(120), command.output())
        .await
        .expect("the process ends")
        .expect("the process starts");
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn config(read_only: Vec<PathBuf>) -> Option<BoundaryConfig> {
    let kind = match detect() {
        Ok(kind) => kind,
        Err(err) => {
            eprintln!("skipped: {err}");
            return None;
        }
    };
    Some(BoundaryConfig {
        kind,
        read_only,
        helper: Some(PathBuf::from(env!("CARGO_BIN_EXE_consort"))),
    })
}

/// The address of this machine on its network, when it has one. No packet
/// goes out.
fn network_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback()).then(|| ip.to_string())
}

const SCRIPT: &str = r#"
echo in > "$PWD/inside.txt" && echo WRITE_SESSION_YES || echo WRITE_SESSION_NO
cat "$SECRET" 2>/dev/null && echo READ_OUTSIDE_YES || echo READ_OUTSIDE_NO
echo x 2>/dev/null > "$OUTSIDE_FILE" && echo WRITE_OUTSIDE_YES || echo WRITE_OUTSIDE_NO
(exec 3<>"/dev/tcp/127.0.0.1/$LISTENER") 2>/dev/null && echo LOOPBACK_YES || echo LOOPBACK_NO
if [ -n "$NETWORK" ]; then
  (exec 3<>"/dev/tcp/$NETWORK/$LISTENER") 2>/dev/null && echo NETWORK_YES || echo NETWORK_NO
fi
port="${HTTPS_PROXY##*:}"
exec 3<>"/dev/tcp/127.0.0.1/$port" || echo PROXY_UNREACHABLE
printf 'CONNECT denied.example:443 HTTP/1.1\r\nHost: denied.example:443\r\n\r\n' >&3
read -r line <&3
echo "PROXY_ANSWER $line"
"#;

#[tokio::test]
async fn a_process_in_the_boundary_stays_in_it() {
    let Some(config) = config(Vec::new()) else {
        return;
    };
    let folders = folders();
    let secret = folders.outside.join("secret.txt");
    std::fs::write(&secret, "the secret").expect("a secret");
    // Listens on every address of the machine. The process must reach it by
    // neither address.
    let listener = std::net::TcpListener::bind("0.0.0.0:0").expect("a listener");
    let port = listener.local_addr().expect("a port").port();
    let mut extra = vec![
        ("SECRET", secret.to_string_lossy().into_owned()),
        (
            "OUTSIDE_FILE",
            folders
                .outside
                .join("new.txt")
                .to_string_lossy()
                .into_owned(),
        ),
        ("LISTENER", port.to_string()),
    ];
    if let Some(address) = network_address() {
        extra.push(("NETWORK", address));
    }
    let process = spec(
        Path::new("/bin/bash"),
        vec!["-c".to_owned(), SCRIPT.to_owned()],
        &folders,
        &extra,
    );
    let (process, _guard) = enter(
        &config,
        HarnessKind::ClaudeCode,
        process,
        &folders.session,
        &folders.runtime,
        &[],
    )
    .await
    .expect("the boundary starts");
    let output = run(process).await;
    println!("{output}");
    assert!(output.contains("WRITE_SESSION_YES"), "{output}");
    assert!(folders.session.join("inside.txt").exists());
    assert!(output.contains("READ_OUTSIDE_NO"), "{output}");
    assert!(!output.contains("the secret"), "{output}");
    assert!(output.contains("WRITE_OUTSIDE_NO"), "{output}");
    assert!(!folders.outside.join("new.txt").exists());
    assert!(output.contains("LOOPBACK_NO"), "{output}");
    assert!(!output.contains("NETWORK_YES"), "{output}");
    assert!(output.contains("PROXY_ANSWER HTTP/1.1 403"), "{output}");
}

/// The real `claude` in the boundary, with a key that is not valid. It must
/// reach the provider through the proxy, and the provider refuses the key, so
/// this spends nothing. Set `CONSORT_BOUNDARY_CLAUDE` to the program.
#[tokio::test]
#[ignore = "needs CONSORT_BOUNDARY_CLAUDE and the network"]
async fn the_real_cli_reaches_its_provider_through_the_proxy() {
    let program = PathBuf::from(
        std::env::var_os("CONSORT_BOUNDARY_CLAUDE").expect("CONSORT_BOUNDARY_CLAUDE"),
    );
    let program = std::fs::canonicalize(program).expect("the program");
    let tool_folder = program.parent().expect("a folder").to_path_buf();
    let Some(config) = config(vec![tool_folder]) else {
        panic!("this system has no boundary");
    };
    let folders = folders();
    let process = spec(
        &program,
        vec![
            "-p".to_owned(),
            "Say ok".to_owned(),
            "--model".to_owned(),
            "haiku".to_owned(),
        ],
        &folders,
        &[(
            "ANTHROPIC_API_KEY",
            "sk-ant-api03-not-a-valid-key".to_owned(),
        )],
    );
    let (process, _guard) = enter(
        &config,
        HarnessKind::ClaudeCode,
        process,
        &folders.session,
        &folders.runtime,
        &[],
    )
    .await
    .expect("the boundary starts");
    let output = run(process).await.to_lowercase();
    println!("{output}");
    assert!(
        output.contains("api key") || output.contains("401") || output.contains("authentication"),
        "the provider did not refuse the key, so the CLI may not have reached it: {output}"
    );
}
