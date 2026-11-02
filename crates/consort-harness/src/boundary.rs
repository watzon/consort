//! The OS boundary of a CLI on the hub machine.
//!
//! A CLI runs with its own profile, its session folder and its temp folder.
//! It has no access to `hub.db`, blobs or other profiles. Its only network
//! is its MCP endpoint and one [`EgressProxy`]. On macOS the boundary is
//! `sandbox-exec`. On Linux it is bubblewrap, and a helper inside the
//! boundary joins loopback ports to Unix sockets. On other systems a session
//! does not start. The rules come from
//! `docs/research/hub-boundary-experiment.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::egress::{EgressProxy, provider_hosts};
use crate::process::ProcessSpec;
use crate::{HarnessError, HarnessKind};

/// The folder of the sockets inside the Linux boundary.
pub const INNER_SOCKET_DIR: &str = "/run/consort";
/// The subcommand of the `consort` binary that runs inside the Linux
/// boundary (see [`run_inside`]).
pub const INSIDE_SUBCOMMAND: &str = "boundary-run";
/// The fixed part of `PATH` in the boundary.
const FIXED_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// How a harness keeps its sessions inside the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundarySupport {
    /// Tests prove the generated configuration and the boundary for this
    /// harness. A session starts only inside the boundary.
    Proven,
    /// A fake harness of a test. It runs no CLI.
    NotNeeded,
    /// Not proven. The session host does not start this harness. The text
    /// says why, for the user.
    Off(String),
}

/// The text for a harness that has no proof of the boundary yet.
pub fn turned_off_reason(name: &str) -> String {
    format!(
        "Consort cannot yet keep {name} inside the hub boundary, so it is turned off. Use another provider until a later version turns it on."
    )
}

/// The kind of OS boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryKind {
    /// `sandbox-exec` on macOS.
    MacSandbox,
    /// `bwrap` on Linux.
    Bubblewrap,
}

/// Why this machine has no boundary. A session does not start then.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BoundaryError {
    /// This system has no boundary that Consort knows.
    #[error(
        "Consort has no process boundary for this system. Run the session host on Linux or macOS."
    )]
    UnsupportedSystem,
    /// The boundary program is missing or cannot start.
    #[error("{0}")]
    NotUsable(String),
}

/// What the session host gives each account that runs inside the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundaryConfig {
    /// The boundary program to use.
    pub kind: BoundaryKind,
    /// Folders that the CLI may read and run, such as the managed tools.
    pub read_only: Vec<PathBuf>,
    /// The `consort` binary, for the helper inside the Linux boundary.
    pub helper: Option<PathBuf>,
}

/// Finds the boundary of this machine, and tests that it can start.
pub fn detect() -> Result<BoundaryKind, BoundaryError> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => Ok(BoundaryKind::MacSandbox),
            _ => Err(BoundaryError::NotUsable(
                "`sandbox-exec` did not start, so Consort cannot start a session. Check that /usr/bin/sandbox-exec exists.".to_owned(),
            )),
        }
    }
    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("bwrap")
            .args([
                "--unshare-all",
                "--die-with-parent",
                "--ro-bind",
                "/usr",
                "/usr",
                "--symlink",
                "usr/bin",
                "/bin",
                "--symlink",
                "usr/lib",
                "/lib",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--",
                "/usr/bin/true",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output();
        match output {
            Ok(output) if output.status.success() => Ok(BoundaryKind::Bubblewrap),
            Ok(output) => Err(BoundaryError::NotUsable(format!(
                "bubblewrap cannot start here, so Consort cannot start a session. In Docker, use the seccomp profile in deploy/session-host. The error was: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))),
            Err(_) => Err(BoundaryError::NotUsable(
                "bubblewrap is not installed, so Consort cannot start a session. Install the `bubblewrap` package.".to_owned(),
            )),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(BoundaryError::UnsupportedSystem)
    }
}

/// The paths and ports of one boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundaryPaths {
    /// The profile of the account.
    pub profile: PathBuf,
    /// The session folder. It is the CLI's `cwd`.
    pub session: PathBuf,
    /// The folder of generated files of the session, and the start folder.
    pub runtime: PathBuf,
    /// The temp folder of the session.
    pub tmp: PathBuf,
    /// Folders to read and run.
    pub read_only: Vec<PathBuf>,
    /// The real home folder of the user, for the macOS keychain.
    pub home: PathBuf,
    /// The MCP endpoint ports, then the proxy port.
    pub ports: Vec<u16>,
    /// Whether the CLI keeps its login in the macOS keychain.
    pub keychain: bool,
}

fn quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The `sandbox-exec` profile. It denies all that it does not list.
pub fn mac_profile(paths: &BoundaryPaths) -> String {
    let mut lines: Vec<String> = [
        "(version 1)",
        "(deny default)",
        "(allow process-fork)",
        "(allow process-exec)",
        "(allow signal (target same-sandbox))",
        "(allow process-info* (target same-sandbox))",
        "(allow sysctl-read)",
        "(allow file-read-metadata)",
        "(allow ipc-posix-sem)",
        "(allow pseudo-tty)",
        r#"(allow mach-lookup (global-name "com.apple.system.opendirectoryd.libinfo") (global-name "com.apple.system.notification_center") (global-name "com.apple.logd") (global-name "com.apple.system.logger") (global-name "com.apple.diagnosticd") (global-name "com.apple.trustd.agent") (global-name "com.apple.SystemConfiguration.configd") (global-name "com.apple.FSEvents"))"#,
        // A CLI that checks TLS certificates with the macOS Security framework
        // needs the security server, also when its login is in a file. Without
        // it, Codex 0.166.0 reaches chatgpt.com through the proxy, but each
        // request fails ("workspace routing discovery failed"). It gives no
        // keychain: the keychain folders stay closed below.
        r#"(allow mach-lookup (global-name "com.apple.SecurityServer"))"#,
        // Without /private/var/db/timezone the CLI spins at full CPU.
        r#"(allow file-read* (subpath "/usr") (subpath "/bin") (subpath "/sbin") (subpath "/System") (subpath "/private/var/db/dyld") (subpath "/private/var/db/timezone") (subpath "/private/var/select") (subpath "/Library/Preferences/com.apple.security") (subpath "/private/etc") (subpath "/etc") (literal "/") (literal "/dev/null") (literal "/dev/zero") (literal "/dev/random") (literal "/dev/urandom") (literal "/dev/tty") (subpath "/dev/fd") (literal "/dev/dtracehelper"))"#,
        r#"(allow file-write* (literal "/dev/null") (literal "/dev/zero") (literal "/dev/tty") (subpath "/dev/fd") (literal "/dev/dtracehelper"))"#,
    ]
    .iter()
    .map(|line| (*line).to_owned())
    .collect();
    if !paths.read_only.is_empty() {
        let list: Vec<String> = paths
            .read_only
            .iter()
            .map(|p| format!("(subpath {})", quoted(p)))
            .collect();
        lines.push(format!("(allow file-read* {})", list.join(" ")));
    }
    lines.push(format!(
        "(allow file-read* file-write* (subpath {}) (subpath {}) (subpath {}))",
        quoted(&paths.session),
        quoted(&paths.runtime),
        quoted(&paths.tmp)
    ));
    // Claude Code takes a lock in its config folder, and one beside it, before
    // it refreshes a token. Without them the login expires after some hours.
    // Bubblewrap also gives the CLI its profile to write.
    let beside = PathBuf::from(format!("{}.lock", paths.profile.to_string_lossy()));
    lines.push(format!(
        "(allow file-read* file-write* (subpath {}) (subpath {}))",
        quoted(&paths.profile),
        quoted(&beside)
    ));
    if !paths.ports.is_empty() {
        let ports: Vec<String> = paths
            .ports
            .iter()
            .map(|port| format!("(remote tcp \"localhost:{port}\")"))
            .collect();
        lines.push(format!("(allow network-outbound {})", ports.join(" ")));
    }
    if paths.keychain {
        // The login of Claude Code is in the keychain. These rules do not
        // separate keychain items per profile (see the warning in Settings > Hub).
        lines.push(r#"(allow mach-lookup (global-name "com.apple.securityd.xpc") (global-name "com.apple.security.agent") (global-name "com.apple.CoreServices.coreservicesd") (global-name "com.apple.lsd.mapdb"))"#.to_owned());
        let keychains = paths.home.join("Library/Keychains");
        let plist = paths
            .home
            .join("Library/Preferences/com.apple.security.plist");
        lines.push(format!(
            "(allow file-read* (subpath {}) (subpath \"/Library/Keychains\") (literal {}))",
            quoted(&keychains),
            quoted(&plist)
        ));
        lines.push(format!(
            "(allow file-write* (subpath {}))",
            quoted(&keychains)
        ));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The arguments of `bwrap`, up to the `--` before the program.
pub fn bwrap_args(paths: &BoundaryPaths, socket_dir: &Path, helper: &Path) -> Vec<String> {
    let text = |p: &Path| p.to_string_lossy().into_owned();
    let mut args: Vec<String> = [
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--ro-bind",
        "/usr",
        "/usr",
        "--symlink",
        "usr/bin",
        "/bin",
        "--symlink",
        "usr/lib",
        "/lib",
        "--symlink",
        "usr/sbin",
        "/sbin",
        "--ro-bind-try",
        "/lib64",
        "/lib64",
        "--ro-bind-try",
        "/etc/ssl",
        "/etc/ssl",
        "--ro-bind-try",
        "/etc/ca-certificates",
        "/etc/ca-certificates",
        "--ro-bind-try",
        "/etc/passwd",
        "/etc/passwd",
        "--ro-bind-try",
        "/etc/group",
        "/etc/group",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
    ]
    .iter()
    .map(|arg| (*arg).to_owned())
    .collect();
    let mut bind = |flag: &str, path: &Path, target: &str| {
        args.extend([flag.to_owned(), text(path), target.to_owned()]);
    };
    for path in &paths.read_only {
        bind("--ro-bind", path, &text(path));
    }
    bind("--ro-bind", helper, &text(helper));
    bind("--bind", &paths.profile, &text(&paths.profile));
    bind("--bind", &paths.session, &text(&paths.session));
    bind("--bind", &paths.runtime, &text(&paths.runtime));
    bind("--bind", &paths.tmp, &text(&paths.tmp));
    bind("--bind", socket_dir, INNER_SOCKET_DIR);
    args.extend(["--chdir".to_owned(), text(&paths.session)]);
    args
}

/// The whole environment of a CLI in the boundary. The adapter copies its
/// own environment to the CLI, so nothing else may reach it. `overlay` has
/// the variables of the account and of the launch. `PATH` from the managed
/// launch goes in front of the fixed part.
pub fn clean_env(
    kind: BoundaryKind,
    paths: &BoundaryPaths,
    user: &str,
    proxy_port: u16,
    overlay: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = overlay.clone();
    let path = match env.get("PATH") {
        Some(path) => format!("{path}:{FIXED_PATH}"),
        None => FIXED_PATH.to_owned(),
    };
    let tmp = paths.tmp.to_string_lossy().into_owned();
    let proxy = format!("http://127.0.0.1:{proxy_port}");
    let home = match kind {
        BoundaryKind::MacSandbox => paths.home.to_string_lossy().into_owned(),
        BoundaryKind::Bubblewrap => paths.session.to_string_lossy().into_owned(),
    };
    for (key, value) in [
        ("PATH", path),
        ("LANG", "en_US.UTF-8".to_owned()),
        ("USER", user.to_owned()),
        ("HOME", home),
        ("TMPDIR", tmp.clone()),
        ("CLAUDE_CODE_TMPDIR", tmp),
        ("DISABLE_AUTOUPDATER", "1".to_owned()),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".to_owned()),
        ("HTTPS_PROXY", proxy.clone()),
        ("HTTP_PROXY", proxy),
        ("NO_PROXY", "127.0.0.1,localhost".to_owned()),
    ] {
        env.insert(key.to_owned(), value);
    }
    env
}

/// Holds what a boundary needs while the CLI runs: the proxy, and on Linux
/// the socket folder and the socket tasks. They stop when this value drops.
#[derive(Debug)]
pub struct ActiveBoundary {
    _proxy: EgressProxy,
    bridges: Vec<tokio::task::JoinHandle<()>>,
    socket_dir: Option<PathBuf>,
}

impl Drop for ActiveBoundary {
    fn drop(&mut self) {
        for bridge in &self.bridges {
            bridge.abort();
        }
        if let Some(dir) = &self.socket_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn canonical(path: &Path) -> Result<PathBuf, HarnessError> {
    std::fs::canonicalize(path).map_err(|err| {
        HarnessError::Spawn(format!(
            "could not find {} for the boundary: {err}",
            path.display()
        ))
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn loopback_port(url: &str) -> Option<u16> {
    let rest = url.strip_prefix("http://127.0.0.1:")?;
    let port = rest.split('/').next()?;
    port.parse().ok()
}

/// The variable that holds the profile folder of a harness that can run in
/// the boundary.
fn profile_var(harness: HarnessKind) -> Result<&'static str, HarnessError> {
    match harness {
        HarnessKind::ClaudeCode => Ok("CLAUDE_CONFIG_DIR"),
        HarnessKind::Codex => Ok("CODEX_HOME"),
        other => Err(HarnessError::Spawn(format!(
            "{} cannot run inside the boundary",
            other.display_name()
        ))),
    }
}

/// Puts a process inside the boundary. The caller keeps the returned guard
/// for as long as the process runs. `Err` means that the process must not
/// start.
pub async fn enter(
    config: &BoundaryConfig,
    harness: HarnessKind,
    process: ProcessSpec,
    session: &Path,
    runtime: &Path,
    mcp_urls: &[String],
) -> Result<(ProcessSpec, ActiveBoundary), HarnessError> {
    let profile_var = profile_var(harness)?;
    let profile = process
        .env
        .get(profile_var)
        .map(PathBuf::from)
        .ok_or_else(|| {
            HarnessError::Spawn(
                "the account has no profile folder, so it cannot run inside the boundary"
                    .to_owned(),
            )
        })?;
    let tmp = runtime.join("tmp");
    std::fs::create_dir_all(&tmp)
        .map_err(|err| HarnessError::Spawn(format!("could not make the temp folder: {err}")))?;
    let mut ports = Vec::new();
    for url in mcp_urls {
        let port = loopback_port(url).ok_or_else(|| {
            HarnessError::Spawn(format!(
                "the MCP endpoint {url} is not on loopback, so it cannot run inside the boundary"
            ))
        })?;
        ports.push(port);
    }
    let proxy = EgressProxy::start(provider_hosts(harness))
        .await
        .map_err(|err| HarnessError::Spawn(format!("could not start the egress proxy: {err}")))?;
    ports.push(proxy.port());
    let mut read_only = Vec::new();
    for path in &config.read_only {
        read_only.push(canonical(path)?);
    }
    let home = home_dir().ok_or_else(|| {
        HarnessError::Spawn("could not find the home folder of the user".to_owned())
    })?;
    let paths = BoundaryPaths {
        profile: canonical(&profile)?,
        session: canonical(session)?,
        runtime: canonical(runtime)?,
        tmp: canonical(&tmp)?,
        read_only,
        home,
        ports: ports.clone(),
        keychain: harness == HarnessKind::ClaudeCode,
    };
    let user = std::env::var("USER").unwrap_or_else(|_| "consort".to_owned());
    let mut env = clean_env(config.kind, &paths, &user, proxy.port(), &process.env);
    env.insert(
        profile_var.to_owned(),
        paths.profile.to_string_lossy().into_owned(),
    );
    let mut guard = ActiveBoundary {
        _proxy: proxy,
        bridges: Vec::new(),
        socket_dir: None,
    };
    let (program, args) = match config.kind {
        BoundaryKind::MacSandbox => {
            let file = paths.runtime.join("boundary.sb");
            std::fs::write(&file, mac_profile(&paths)).map_err(|err| {
                HarnessError::Spawn(format!("could not write the boundary profile: {err}"))
            })?;
            let mut args = vec![
                "-f".to_owned(),
                file.to_string_lossy().into_owned(),
                process.program.to_string_lossy().into_owned(),
            ];
            args.extend(process.args.iter().cloned());
            (PathBuf::from("/usr/bin/sandbox-exec"), args)
        }
        BoundaryKind::Bubblewrap => {
            let helper = config.helper.as_deref().ok_or_else(|| {
                HarnessError::Spawn(
                    "the boundary helper is missing, so the session cannot start".to_owned(),
                )
            })?;
            let socket_dir = short_socket_dir()?;
            guard.socket_dir = Some(socket_dir.clone());
            guard.bridges = host_bridges(&socket_dir, &ports)?;
            let mut args = bwrap_args(&paths, &socket_dir, helper);
            args.push("--".to_owned());
            args.push(helper.to_string_lossy().into_owned());
            args.push(INSIDE_SUBCOMMAND.to_owned());
            for port in &ports {
                args.push("--bridge".to_owned());
                args.push(format!("{port}={INNER_SOCKET_DIR}/{port}.sock"));
            }
            args.push("--".to_owned());
            args.push(process.program.to_string_lossy().into_owned());
            args.extend(process.args.iter().cloned());
            (PathBuf::from("bwrap"), args)
        }
    };
    Ok((
        ProcessSpec {
            program,
            args,
            env,
            env_clear: true,
            env_remove: Vec::new(),
            cwd: Some(paths.session.clone()),
            watcher: process.watcher,
        },
        guard,
    ))
}

/// A Unix socket path has a small limit, so the sockets get a short folder
/// of their own.
fn short_socket_dir() -> Result<PathBuf, HarnessError> {
    let id = uuid::Uuid::now_v7().simple().to_string();
    let dir = std::env::temp_dir().join(format!("csb-{}", &id[id.len() - 12..]));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|err| {
                HarnessError::Spawn(format!("could not make the socket folder: {err}"))
            })?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(&dir)
        .map_err(|err| HarnessError::Spawn(format!("could not make the socket folder: {err}")))?;
    Ok(dir)
}

/// Joins a Unix socket in `dir` to each loopback port of this machine.
#[cfg(unix)]
fn host_bridges(
    dir: &Path,
    ports: &[u16],
) -> Result<Vec<tokio::task::JoinHandle<()>>, HarnessError> {
    let mut tasks = Vec::new();
    for port in ports.iter().copied() {
        let listener = tokio::net::UnixListener::bind(dir.join(format!("{port}.sock")))
            .map_err(|err| HarnessError::Spawn(format!("could not open a socket: {err}")))?;
        tasks.push(tokio::spawn(async move {
            while let Ok((mut inner, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Ok(mut outer) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await
                    {
                        let _ = tokio::io::copy_bidirectional(&mut inner, &mut outer).await;
                    }
                });
            }
        }));
    }
    Ok(tasks)
}

#[cfg(not(unix))]
fn host_bridges(
    _dir: &Path,
    _ports: &[u16],
) -> Result<Vec<tokio::task::JoinHandle<()>>, HarnessError> {
    Err(HarnessError::Spawn(
        BoundaryError::UnsupportedSystem.to_string(),
    ))
}

/// Runs inside the Linux boundary. Each bridge listens on a loopback port
/// and joins it to a Unix socket. Then the program runs with the stdio of
/// this process. Returns the exit code of the program.
#[cfg(unix)]
pub async fn run_inside(bridges: &[(u16, PathBuf)], program: &str, args: &[String]) -> i32 {
    for (port, socket) in bridges {
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", *port)).await {
            Ok(listener) => listener,
            Err(err) => {
                eprintln!("could not open port {port} in the boundary: {err}");
                return 70;
            }
        };
        let socket = socket.clone();
        tokio::spawn(async move {
            while let Ok((mut inner, _)) = listener.accept().await {
                let socket = socket.clone();
                tokio::spawn(async move {
                    if let Ok(mut outer) = tokio::net::UnixStream::connect(&socket).await {
                        let _ = tokio::io::copy_bidirectional(&mut inner, &mut outer).await;
                    }
                });
            }
        });
    }
    match tokio::process::Command::new(program)
        .args(args)
        .status()
        .await
    {
        Ok(status) => status.code().unwrap_or(1),
        Err(err) => {
            eprintln!("could not start {program} in the boundary: {err}");
            127
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> BoundaryPaths {
        BoundaryPaths {
            profile: "/hub/profiles/a".into(),
            session: "/hub/sessions/s1".into(),
            runtime: "/hub/runtime/s1".into(),
            tmp: "/hub/runtime/s1/tmp".into(),
            read_only: vec!["/hub/tools".into()],
            home: "/Users/u".into(),
            ports: vec![5001, 5002],
            keychain: true,
        }
    }

    #[test]
    fn a_cli_with_its_login_in_a_file_gets_no_keychain() {
        let with = mac_profile(&paths());
        assert!(with.contains("Library/Keychains"));
        assert!(with.contains("com.apple.securityd.xpc"));
        let mut file_login = paths();
        file_login.keychain = false;
        let without = mac_profile(&file_login);
        assert!(!without.contains("Keychains"));
        assert!(!without.contains("com.apple.securityd.xpc"));
        assert!(!without.contains("com.apple.security.agent"));
    }

    /// Codex 0.166.0 checks the certificate of chatgpt.com with the Security
    /// framework. Without the security server, every request fails in the
    /// boundary although the proxy lets it through.
    #[test]
    fn every_cli_can_check_tls_certificates() {
        for keychain in [true, false] {
            let mut p = paths();
            p.keychain = keychain;
            let text = mac_profile(&p);
            assert_eq!(
                text.matches(r#"(global-name "com.apple.SecurityServer")"#)
                    .count(),
                1,
                "keychain {keychain}: {text}"
            );
        }
    }

    #[test]
    fn the_mac_profile_denies_by_default_and_lists_only_this_session() {
        let text = mac_profile(&paths());
        assert!(text.starts_with("(version 1)\n(deny default)\n"));
        assert!(text.contains(r#"(allow file-read* (subpath "/hub/tools"))"#));
        assert!(text.contains(r#"(subpath "/hub/sessions/s1")"#));
        // A token refresh takes these locks. It does not run without them.
        assert!(text.contains(
            r#"(allow file-read* file-write* (subpath "/hub/profiles/a") (subpath "/hub/profiles/a.lock"))"#
        ));
        assert!(text.contains(
            r#"(allow network-outbound (remote tcp "localhost:5001") (remote tcp "localhost:5002"))"#
        ));
        assert!(!text.contains("(allow network*"));
        assert!(!text.contains("(allow default"));
        // The timezone database stops a spin at full CPU.
        assert!(text.contains("/private/var/db/timezone"));
    }

    #[test]
    fn a_quote_in_a_path_cannot_end_the_rule() {
        let mut p = paths();
        p.session = PathBuf::from("/hub/a\") (allow default) (\"b");
        let text = mac_profile(&p);
        assert!(!text.contains("(allow default)  ") && text.contains("\\\")"));
    }

    #[test]
    fn bubblewrap_gets_no_network_and_only_this_session() {
        let args = bwrap_args(&paths(), Path::new("/tmp/csb-1"), Path::new("/hub/consort"));
        let line = args.join(" ");
        assert!(line.contains("--unshare-all"));
        assert!(line.contains("--die-with-parent"));
        assert!(!line.contains("--share-net"));
        assert!(line.contains("--bind /hub/sessions/s1 /hub/sessions/s1"));
        assert!(line.contains("--bind /hub/profiles/a /hub/profiles/a"));
        assert!(line.contains("--ro-bind /hub/tools /hub/tools"));
        assert!(line.contains("--bind /tmp/csb-1 /run/consort"));
        assert!(!line.contains("hub.db"));
        assert!(line.ends_with("--chdir /hub/sessions/s1"));
    }

    #[test]
    fn the_environment_is_exactly_the_clean_set() {
        let overlay = BTreeMap::from([
            (
                "CLAUDE_CODE_EXECUTABLE".to_owned(),
                "/hub/tools/claude".to_owned(),
            ),
            ("PATH".to_owned(), "/hub/tools/node/bin".to_owned()),
        ]);
        let env = clean_env(BoundaryKind::MacSandbox, &paths(), "u", 5002, &overlay);
        assert_eq!(env["HTTPS_PROXY"], "http://127.0.0.1:5002");
        assert_eq!(env["NO_PROXY"], "127.0.0.1,localhost");
        assert_eq!(env["HOME"], "/Users/u");
        assert_eq!(
            env["PATH"],
            "/hub/tools/node/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        );
        assert_eq!(env["CLAUDE_CODE_EXECUTABLE"], "/hub/tools/claude");
        let linux = clean_env(BoundaryKind::Bubblewrap, &paths(), "u", 5002, &overlay);
        assert_eq!(linux["HOME"], "/hub/sessions/s1");
    }

    #[test]
    fn an_mcp_url_must_be_on_loopback() {
        assert_eq!(loopback_port("http://127.0.0.1:5001/mcp/x"), Some(5001));
        assert_eq!(loopback_port("http://10.0.0.1:5001/mcp/x"), None);
    }

    // A real run of the boundary on this Mac: the CLI side cannot read hub
    // data or another profile, cannot reach a loopback service, and can write
    // to its own folder.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_boundary_holds_for_a_real_process() {
        use std::process::Command;
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        let make = |name: &str| {
            let path = root_path.join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        };
        let data = make("hub-data");
        std::fs::write(data.join("hub.db"), "secret").unwrap();
        let other = make("profiles/other");
        std::fs::write(other.join("secret.txt"), "secret").unwrap();
        let hub = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let hub_port = hub.local_addr().unwrap().port();
        let paths = BoundaryPaths {
            profile: make("profiles/mine"),
            session: make("session"),
            runtime: make("runtime"),
            tmp: make("runtime/tmp"),
            keychain: true,
            read_only: Vec::new(),
            home: root_path.clone(),
            ports: Vec::new(),
        };
        let file = paths.runtime.join("b.sb");
        std::fs::write(&file, mac_profile(&paths)).unwrap();
        let run = |script: String| {
            Command::new("/usr/bin/sandbox-exec")
                .arg("-f")
                .arg(&file)
                .args(["/bin/sh", "-c", &script])
                .status()
                .unwrap()
                .success()
        };
        assert!(run(format!("echo ok > {}/f", paths.session.display())));
        assert!(run(format!("echo ok > {}/f", paths.tmp.display())));
        assert!(!run(format!("cat {}/hub.db", data.display())));
        assert!(!run(format!("cat {}/secret.txt", other.display())));
        assert!(!run(format!("echo x > {}/hub.db", data.display())));
        // A token refresh writes in the profile and beside it. Other profiles
        // stay closed.
        assert!(run(format!("echo x > {}/new", paths.profile.display())));
        assert!(run(format!("mkdir {}.lock", paths.profile.display())));
        assert!(!run(format!("echo x > {}/new", other.display())));
        assert!(!run(format!("/usr/bin/nc -z -G 2 127.0.0.1 {hub_port}")));
        drop(hub);
    }
}
