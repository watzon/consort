//! Harness processes.
//!
//! Each harness runs as a child process with piped stdio. On Unix the child
//! leads its own process group, and on Windows it runs in a job object, so
//! [`HarnessProcess::kill`] ends the whole process tree. Wrapper launchers
//! such as `npx` start the real agent as a grandchild, so killing only the
//! child is not enough.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

use crate::HarnessError;

/// How much of a harness's stderr the adapter keeps for error messages.
const STDERR_TAIL_BYTES: usize = 8 * 1024;

/// How to start a harness process.
#[derive(Debug, Clone, Default)]
pub struct ProcessSpec {
    /// The program, already resolved.
    pub program: PathBuf,
    /// The arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: BTreeMap<String, String>,
    /// Environment variables to remove.
    pub env_remove: Vec<String>,
    /// The working folder.
    pub cwd: Option<PathBuf>,
}

/// Finds a program on `PATH`, or checks an explicit path. On Windows this
/// finds `npx.cmd` for `npx`.
pub fn resolve_program(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 || path.is_absolute() {
        return path.exists().then(|| path.to_path_buf());
    }
    which::which(name).ok()
}

/// A running harness process with piped stdio.
pub struct HarnessProcess {
    child: Box<dyn ChildWrapper>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr_tail: Arc<Mutex<String>>,
}

impl std::fmt::Debug for HarnessProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HarnessProcess")
            .field("pid", &self.child.id())
            .finish()
    }
}

fn build_command(spec: &ProcessSpec) -> tokio::process::Command {
    let is_script = spec
        .program
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ps1"));
    let mut cmd = if is_script {
        // A PowerShell script launcher on Windows.
        let mut cmd = tokio::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
        cmd.arg(&spec.program);
        cmd
    } else {
        tokio::process::Command::new(&spec.program)
    };
    cmd.args(&spec.args);
    for key in &spec.env_remove {
        cmd.env_remove(key);
    }
    cmd.envs(&spec.env);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd
}

/// Starts a harness process.
pub fn spawn(spec: &ProcessSpec) -> Result<HarnessProcess, HarnessError> {
    let mut wrap = CommandWrap::from(build_command(spec));
    #[cfg(unix)]
    {
        wrap.wrap(process_wrap::tokio::ProcessGroup::leader());
    }
    #[cfg(windows)]
    {
        wrap.wrap(process_wrap::tokio::JobObject);
    }
    wrap.wrap(KillOnDrop);
    let mut child = wrap
        .spawn()
        .map_err(|e| HarnessError::Spawn(format!("{}: {e}", spec.program.display())))?;
    let stdin = child.stdin().take();
    let stdout = child.stdout().take();
    let stderr = child.stderr().take();
    let stderr_tail = Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = stderr {
        let tail = stderr_tail.clone();
        let program = spec.program.display().to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "consort_harness::stderr", program = %program, "{line}");
                if let Ok(mut tail) = tail.lock() {
                    tail.push_str(&line);
                    tail.push('\n');
                    if tail.len() > STDERR_TAIL_BYTES {
                        let (kept, _) = consort_proto::model::tail_of(&tail, STDERR_TAIL_BYTES);
                        *tail = kept.to_owned();
                    }
                }
            }
        });
    }
    Ok(HarnessProcess {
        child,
        stdin,
        stdout,
        stderr_tail,
    })
}

impl HarnessProcess {
    /// Takes the child's stdin.
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.stdin.take()
    }

    /// Takes the child's stdout.
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }

    /// The process ID.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// The last lines of the child's stderr.
    pub fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .map(|t| t.clone())
            .unwrap_or_default()
    }

    /// A handle that reads the stderr tail later.
    pub fn stderr_handle(&self) -> StderrTail {
        StderrTail(self.stderr_tail.clone())
    }

    /// Whether the process has exited.
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Waits for the process to exit.
    pub async fn wait(&mut self) -> Option<std::process::ExitStatus> {
        self.child.wait().await.ok()
    }

    /// Kills the whole process tree and waits for the child.
    pub async fn kill(&mut self) {
        if let Err(err) = Box::into_pin(self.child.kill()).await {
            tracing::debug!(error = %err, "harness process kill failed; it may have exited");
        }
    }

    /// Waits up to `grace` for the process to exit, then kills the tree.
    pub async fn shutdown(&mut self, grace: Duration) {
        drop(self.stdin.take());
        if tokio::time::timeout(grace, self.child.wait())
            .await
            .is_err()
        {
            self.kill().await;
        } else {
            // The child exited. Kill the rest of the group, if any.
            self.kill().await;
        }
    }
}

/// A shared view of a process's stderr tail.
#[derive(Debug, Clone)]
pub struct StderrTail(Arc<Mutex<String>>);

impl StderrTail {
    /// A tail with no output, for a harness that is not a child process.
    pub fn empty() -> Self {
        Self(Arc::new(Mutex::new(String::new())))
    }

    /// The last lines of stderr.
    pub fn get(&self) -> String {
        self.0.lock().map(|t| t.clone()).unwrap_or_default()
    }
}

/// Runs a short command, such as `codex --version`, and returns its stdout.
pub async fn run_capture(
    program: &Path,
    args: &[&str],
    env: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<String, HarnessError> {
    let spec = ProcessSpec {
        program: program.to_path_buf(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        env: env.clone(),
        env_remove: Vec::new(),
        cwd: None,
    };
    let mut cmd = build_command(&spec);
    cmd.stdin(std::process::Stdio::null());
    cmd.kill_on_drop(true);
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| HarnessError::Timeout(format!("{} {}", program.display(), args.join(" "))))?
        .map_err(|e| HarnessError::Spawn(format!("{}: {e}", program.display())))?;
    if !output.status.success() {
        return Err(HarnessError::Protocol(format!(
            "{} exited with {}",
            program.display(),
            output.status
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Pulls a version number such as `0.165.0` out of `--version` output.
pub fn parse_version(output: &str) -> Option<String> {
    output
        .split(|c: char| c.is_whitespace() || c == 'v' || c == '(' || c == ')')
        .find(|word| {
            word.split('.').count() >= 2
                && word
                    .split('.')
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        })
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing() {
        assert_eq!(
            parse_version("codex-cli 0.165.0").as_deref(),
            Some("0.165.0")
        );
        assert_eq!(
            parse_version("2.1.312 (Claude Code)").as_deref(),
            Some("2.1.312")
        );
        assert_eq!(parse_version("opencode v2.2.0").as_deref(), Some("2.2.0"));
        assert_eq!(parse_version("0.46.0").as_deref(), Some("0.46.0"));
        assert_eq!(parse_version("no version"), None);
    }

    #[tokio::test]
    async fn spawned_process_tree_is_killed() {
        let Some(sh) = resolve_program(if cfg!(windows) { "cmd" } else { "sh" }) else {
            return;
        };
        let args = if cfg!(windows) {
            vec!["/C".to_owned(), "ping -n 30 127.0.0.1 > NUL".to_owned()]
        } else {
            vec!["-c".to_owned(), "sleep 30 & sleep 30".to_owned()]
        };
        let mut process = spawn(&ProcessSpec {
            program: sh,
            args,
            ..ProcessSpec::default()
        })
        .unwrap();
        assert!(process.id().is_some());
        assert!(!process.has_exited());
        let started = std::time::Instant::now();
        process.kill().await;
        assert!(process.has_exited());
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
