//! Harness processes.
//!
//! Each harness runs as a child process with piped stdio. On Unix the child
//! leads its own process group, and on Windows it runs in a job object, so
//! [`HarnessProcess::kill`] ends the whole process tree. Wrapper launchers
//! such as `npx` start the real agent as a grandchild, so killing only the
//! child is not enough.
//!
//! On Windows the job object kills the tree when the worker exits, even
//! when the worker is killed. On Unix a process group outlives a worker
//! that is killed. So a [`ProcessWatcher`] learns each group that starts and
//! ends, and the worker kills the groups that it recorded at the next start
//! (see [`group_start_time`], [`kill_group`] and [`members_started_since`]).
//!
//! A harness never shares the worker's console on Windows: it starts with
//! `CREATE_NEW_PROCESS_GROUP` and `CREATE_NO_WINDOW`. So Ctrl-C and a
//! console close reach only the worker, which then stops its sessions in
//! order and keeps them for the next run.

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

/// Learns about the process groups of harness processes. The worker
/// records them, so it can kill a group that outlives it.
pub trait ProcessWatcher: Send + Sync {
    /// A process group started. `pgid` is the ID of the group and of its
    /// leader. `program` is the program that the leader runs.
    fn started(&self, pgid: u32, program: &str);
    /// The process group ended, or the adapter killed it.
    fn ended(&self, pgid: u32);
}

/// A shared [`ProcessWatcher`]. Two handles are equal when they share one
/// watcher.
#[derive(Clone)]
pub struct WatcherHandle(pub Arc<dyn ProcessWatcher>);

impl std::fmt::Debug for WatcherHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WatcherHandle")
    }
}

impl PartialEq for WatcherHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// How to start a harness process.
#[derive(Debug, Clone, Default)]
pub struct ProcessSpec {
    /// The program, already resolved.
    pub program: PathBuf,
    /// The arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: BTreeMap<String, String>,
    /// Start with no variables from this process. A CLI inside the boundary
    /// gets only `env`.
    pub env_clear: bool,
    /// Environment variables to remove.
    pub env_remove: Vec<String>,
    /// The working folder.
    pub cwd: Option<PathBuf>,
    /// Learns the process group, on Unix.
    pub watcher: Option<WatcherHandle>,
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
    /// The watcher and the process group that it knows about.
    group: Option<(WatcherHandle, u32)>,
    /// What must live as long as the process, such as its boundary.
    held: Option<Box<dyn std::any::Any + Send + Sync>>,
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
    if spec.env_clear {
        cmd.env_clear();
    }
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
        use windows::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
        wrap.wrap(process_wrap::tokio::CreationFlags(
            CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW,
        ));
        wrap.wrap(process_wrap::tokio::JobObject);
    }
    wrap.wrap(KillOnDrop);
    let mut child = wrap
        .spawn()
        .map_err(|e| HarnessError::Spawn(format!("{}: {e}", spec.program.display())))?;
    let stdin = child.stdin().take();
    let stdout = child.stdout().take();
    let stderr = child.stderr().take();
    // The child leads its own process group on Unix, so the group ID is the
    // child's process ID.
    let group = if cfg!(unix) {
        spec.watcher.clone().zip(child.id())
    } else {
        None
    };
    if let Some((watcher, pgid)) = &group {
        let program = spec
            .program
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        watcher.0.started(*pgid, &program);
    }
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
        group,
        held: None,
    })
}

impl HarnessProcess {
    /// Keeps `value` for as long as this process value lives.
    pub fn hold(&mut self, value: Box<dyn std::any::Any + Send + Sync>) {
        self.held = Some(value);
    }

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
        self.forget_group();
    }

    /// Tells the watcher that the process group is gone.
    fn forget_group(&mut self) {
        if let Some((watcher, pgid)) = self.group.take() {
            watcher.0.ended(pgid);
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

impl Drop for HarnessProcess {
    fn drop(&mut self) {
        // `KillOnDrop` kills the process tree when the child drops.
        self.forget_group();
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
        env_clear: false,
        program: program.to_path_buf(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        env: env.clone(),
        env_remove: Vec::new(),
        cwd: None,
        watcher: None,
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

/// The start time of a process, as `ps -o lstart` shows it, or `None` when
/// no such process runs. The worker stores it with a process group, so it
/// never kills a group whose ID the system gave to a new process. This
/// blocks, so call it from a blocking task.
#[cfg(unix)]
pub fn group_start_time(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}

/// Sends SIGKILL to every process in a process group. Returns whether the
/// signal went out. This blocks, so call it from a blocking task.
#[cfg(unix)]
pub fn kill_group(pgid: u32) -> bool {
    if pgid <= 1 {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{pgid}")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Sends SIGKILL to one process. Returns whether the signal went out. This
/// blocks, so call it from a blocking task.
#[cfg(unix)]
pub fn kill_pid(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-s", "KILL", "--", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A start time from `ps -o lstart` (with `LC_ALL=C`), such as
/// `Mon Jan  1 00:00:00 2001`, as a value that sorts in time order:
/// year, month, day, hour, minute, second. The time is local time, as
/// `ps` shows it.
pub fn parse_lstart(text: &str) -> Option<(u32, u32, u32, u32, u32, u32)> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [_, month, day, clock, year] = parts.as_slice() else {
        return None;
    };
    let month = u32::try_from(MONTHS.iter().position(|m| m == month)? + 1).ok()?;
    let mut clock = clock.split(':').map(str::parse::<u32>);
    let (Some(Ok(h)), Some(Ok(m)), Some(Ok(s)), None) =
        (clock.next(), clock.next(), clock.next(), clock.next())
    else {
        return None;
    };
    Some((year.parse().ok()?, month, day.parse().ok()?, h, m, s))
}

/// The processes of a group: on Linux from `/proc`, elsewhere from `ps`.
#[cfg(target_os = "linux")]
fn group_pids(pgid: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            // The fields after the program name, which ends with `)`:
            // state, parent, then the process group.
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                stat.rsplit_once(')').is_some_and(|(_, rest)| {
                    rest.split_whitespace().nth(2) == Some(pgid.to_string().as_str())
                })
            })
        })
        .collect()
}

/// The processes of a group: on Linux from `/proc`, elsewhere from `ps`.
#[cfg(all(unix, not(target_os = "linux")))]
fn group_pids(pgid: u32) -> Vec<u32> {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,pgid="])
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid = words.next()?.parse::<u32>().ok()?;
            (words.next()? == pgid.to_string()).then_some(pid)
        })
        .collect()
}

/// The processes of a process group whose leader is gone, that started at
/// or after `started` (the leader's start time as [`group_start_time`]
/// gave it). The worker kills them at its next start: when a harness
/// leader exits, the processes that it started keep the group. A process
/// that started before the leader cannot belong to the leader's group.
/// This blocks, so call it from a blocking task.
#[cfg(unix)]
pub fn members_started_since(pgid: u32, started: &str) -> Vec<u32> {
    let Some(since) = parse_lstart(started) else {
        return Vec::new();
    };
    group_pids(pgid)
        .into_iter()
        .filter(|pid| *pid > 1)
        .filter(|pid| {
            group_start_time(*pid)
                .as_deref()
                .and_then(parse_lstart)
                .is_some_and(|at| at >= since)
        })
        .collect()
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

    #[test]
    fn lstart_sorts_in_time_order() {
        let a = parse_lstart("Mon Jan  1 00:00:00 2001").unwrap();
        let b = parse_lstart("Tue Oct 29 13:05:09 2026").unwrap();
        assert_eq!(b, (2026, 10, 29, 13, 5, 9));
        assert!(a < b);
        assert!(parse_lstart("Tue Oct 29 13:05 2026").is_none());
        assert!(parse_lstart("").is_none());
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

    #[cfg(unix)]
    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, u32)>>);

    #[cfg(unix)]
    impl ProcessWatcher for Recorder {
        fn started(&self, pgid: u32, program: &str) {
            self.0
                .lock()
                .unwrap()
                .push((format!("started {program}"), pgid));
        }
        fn ended(&self, pgid: u32) {
            self.0.lock().unwrap().push(("ended".to_owned(), pgid));
        }
    }

    #[cfg(unix)]
    fn group_members(pgid: u32) -> Vec<String> {
        let out = std::process::Command::new("ps")
            .args(["-A", "-o", "pid=,pgid=,stat="])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| {
                let parts: Vec<&str> = l.split_whitespace().collect();
                parts.get(1) == Some(&pgid.to_string().as_str())
                    && !parts.get(2).is_some_and(|s| s.starts_with('Z'))
            })
            .map(str::to_owned)
            .collect()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_watcher_learns_the_group_and_a_stale_group_can_be_killed() {
        let recorder = Arc::new(Recorder::default());
        let watcher = WatcherHandle(recorder.clone());
        let spec = ProcessSpec {
            program: resolve_program("sh").unwrap(),
            args: vec!["-c".to_owned(), "sleep 30 & sleep 30".to_owned()],
            watcher: Some(watcher.clone()),
            ..ProcessSpec::default()
        };
        // A killed process tells the watcher that its group ended.
        let mut process = spawn(&spec).unwrap();
        let pgid = process.id().unwrap();
        process.kill().await;
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![("started sh".to_owned(), pgid), ("ended".to_owned(), pgid)]
        );

        // A process that the worker loses (as when the worker is killed)
        // keeps its group. The worker kills the group at its next start, but
        // only when the leader still has the start time that it recorded.
        let process = spawn(&spec).unwrap();
        let pgid = process.id().unwrap();
        std::mem::forget(process);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(group_members(pgid).len() >= 2, "{:?}", group_members(pgid));
        let started = tokio::task::spawn_blocking(move || group_start_time(pgid))
            .await
            .unwrap()
            .expect("the leader runs");
        assert!(!started.is_empty());
        assert!(
            tokio::task::spawn_blocking(move || kill_group(pgid))
                .await
                .unwrap()
        );
        for _ in 0..50 {
            if group_members(pgid).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(group_members(pgid).is_empty(), "{:?}", group_members(pgid));
        assert!(!kill_group(1), "the worker never signals init");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn members_of_a_group_whose_leader_is_gone_are_found() {
        // The leader starts a child in its group and exits.
        let mut process = spawn(&ProcessSpec {
            program: resolve_program("sh").unwrap(),
            args: vec!["-c".to_owned(), "sleep 30 & exit 0".to_owned()],
            ..ProcessSpec::default()
        })
        .unwrap();
        let pgid = process.id().unwrap();
        let started = tokio::task::spawn_blocking(move || group_start_time(pgid))
            .await
            .unwrap()
            .unwrap();
        process.wait().await;
        // Dropping the process would kill its group, as the worker does.
        // A killed worker does not, so forget it.
        std::mem::forget(process);
        let s = started.clone();
        let members = tokio::task::spawn_blocking(move || members_started_since(pgid, &s))
            .await
            .unwrap();
        assert_eq!(members.len(), 1, "{members:?}");
        assert!(!members.contains(&pgid));
        // A time after the members started finds none.
        let none = tokio::task::spawn_blocking(move || {
            members_started_since(pgid, "Fri Dec 31 23:59:59 2100")
        })
        .await
        .unwrap();
        assert!(none.is_empty());
        for pid in members {
            assert!(kill_pid(pid));
        }
    }
}
