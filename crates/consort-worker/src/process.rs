//! Processes of the worker. A process writes to a file, not to a pipe, so it
//! goes on when the worker stops. A record in the journal tells the next
//! worker about it. A command with an unknown result never runs again by
//! itself: the state says `unknown`, and the model decides.

use std::collections::HashMap;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use consort_proto::execution_link::MAX_OUTPUT_BYTES;
use consort_proto::ids::SessionId;
use consort_proto::time::Timestamp;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt as _;
use tokio::process::{ChildStdin, Command};
use tokio::sync::watch;

use crate::WorkerError;
use crate::journal::{Journal, ProcessRecord};

/// How often the worker checks on a process from before a restart.
const ADOPT_POLL: Duration = Duration::from_secs(2);

/// The shell of this machine, with the argument for a command line.
pub fn shell() -> (&'static str, &'static [&'static str]) {
    if cfg!(windows) {
        (
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command"],
        )
    } else {
        ("/bin/sh", &["-c"])
    }
}

/// Ends a process and its children. A failure means that it is gone already.
pub async fn kill_tree(pid: u32, force: bool) {
    if pid <= 1 {
        return;
    }
    let mut command = if cfg!(windows) {
        let mut c = Command::new("taskkill");
        c.args(["/T", "/F", "/PID", &pid.to_string()]);
        c
    } else {
        // The process leads its own group, so a negative ID names all of it.
        let mut c = Command::new("kill");
        c.args([
            "-s",
            if force { "KILL" } else { "TERM" },
            "--",
            &format!("-{pid}"),
        ]);
        c
    };
    let _ = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

/// The start time of a process as the system shows it, or `None` when it does
/// not run.
pub async fn start_mark(pid: u32) -> Option<String> {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("powershell.exe");
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-Process -Id {pid} -ErrorAction Stop).StartTime.ToFileTimeUtc()"),
        ]);
        c
    } else {
        let mut c = Command::new("ps");
        c.args(["-o", "lstart=", "-p", &pid.to_string()])
            .env("LC_ALL", "C");
        c
    };
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}

struct Live {
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    ended: watch::Receiver<bool>,
}

/// The processes of this worker.
pub struct Processes {
    journal: Arc<Journal>,
    dir: PathBuf,
    live: Mutex<HashMap<String, Arc<Live>>>,
}

impl Processes {
    /// The processes, with their output files in `dir`.
    pub fn new(journal: Arc<Journal>, dir: PathBuf) -> Self {
        Self {
            journal,
            dir,
            live: Mutex::new(HashMap::new()),
        }
    }

    fn live(&self, process_id: &str) -> Option<Arc<Live>> {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(process_id)
            .cloned()
    }

    /// Starts a shell command. The deadline ends it.
    pub async fn start(
        self: &Arc<Self>,
        session_id: SessionId,
        command_line: &str,
        cwd: &Path,
        timeout: Duration,
    ) -> Result<ProcessRecord, WorkerError> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| WorkerError::io("could not make the processes folder", e))?;
        let process_id = uuid::Uuid::now_v7().simple().to_string();
        let output_path = self.dir.join(format!("{process_id}.log"));
        let out = std::fs::File::create(&output_path)
            .map_err(|e| WorkerError::io("could not make the output file", e))?;
        let err = out
            .try_clone()
            .map_err(|e| WorkerError::io("could not open the output file", e))?;
        let (program, args) = shell();
        let mut command = Command::new(program);
        command
            .args(args)
            .arg(command_line)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|e| WorkerError::io(format!("could not start {program}"), e))?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let start_mark = match pid {
            Some(pid) => start_mark(pid).await,
            None => None,
        };
        let now = Timestamp::now();
        let deadline = Timestamp::from_millis(
            now.as_millis() + i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX / 2),
        );
        let record = ProcessRecord {
            process_id: process_id.clone(),
            session_id,
            command: command_line.to_owned(),
            cwd: cwd.to_string_lossy().into_owned(),
            pid,
            start_mark,
            state: "running".to_owned(),
            exit_code: None,
            output_path: output_path.to_string_lossy().into_owned(),
            started_at: now,
            ended_at: None,
            deadline,
        };
        self.journal.put_process(&record)?;
        let (ended_tx, ended) = watch::channel(false);
        self.live.lock().unwrap_or_else(|e| e.into_inner()).insert(
            process_id.clone(),
            Arc::new(Live {
                stdin: tokio::sync::Mutex::new(stdin),
                ended,
            }),
        );
        let journal = self.journal.clone();
        let mut done = record.clone();
        tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status.ok(),
                () = tokio::time::sleep(timeout) => {
                    if let Some(pid) = pid {
                        kill_tree(pid, true).await;
                    }
                    child.wait().await.ok()
                }
            };
            done.state = "exited".to_owned();
            done.exit_code = status.and_then(|s| s.code());
            done.ended_at = Some(Timestamp::now());
            if let Err(err) = journal.put_process(&done) {
                tracing::warn!(error = %err, "could not record the end of a process");
            }
            let _ = ended_tx.send(true);
        });
        Ok(record)
    }

    /// Looks after the processes that ran when the last worker stopped. Each
    /// one that still runs is watched. The result of one that is gone is
    /// unknown.
    pub async fn adopt(self: &Arc<Self>) -> Result<(), WorkerError> {
        for mut record in self.journal.running_processes()? {
            let alive = match record.pid {
                Some(pid) => {
                    start_mark(pid).await == record.start_mark && record.start_mark.is_some()
                }
                None => false,
            };
            if !alive {
                record.state = "unknown".to_owned();
                record.ended_at = Some(Timestamp::now());
                self.journal.put_process(&record)?;
                continue;
            }
            let journal = self.journal.clone();
            let (ended_tx, ended) = watch::channel(false);
            self.live.lock().unwrap_or_else(|e| e.into_inner()).insert(
                record.process_id.clone(),
                Arc::new(Live {
                    stdin: tokio::sync::Mutex::new(None),
                    ended,
                }),
            );
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(ADOPT_POLL).await;
                    let Some(pid) = record.pid else { break };
                    let mark = start_mark(pid).await;
                    if mark.is_none() || mark != record.start_mark {
                        break;
                    }
                    if Timestamp::now() > record.deadline {
                        kill_tree(pid, true).await;
                    }
                }
                // This worker is not the parent, so the exit code is lost.
                record.state = "unknown".to_owned();
                record.ended_at = Some(Timestamp::now());
                if let Err(err) = journal.put_process(&record) {
                    tracing::warn!(error = %err, "could not record the end of a process");
                }
                let _ = ended_tx.send(true);
            });
        }
        Ok(())
    }

    /// The record of a process of this session.
    pub fn record(
        &self,
        session_id: SessionId,
        process_id: &str,
    ) -> Result<Option<ProcessRecord>, WorkerError> {
        Ok(self
            .journal
            .process(process_id)?
            .filter(|p| p.session_id == session_id))
    }

    /// The state and the new output of a process.
    pub fn snapshot(record: &ProcessRecord, offset: u64) -> Value {
        let (output, next_offset, more) = read_output(Path::new(&record.output_path), offset);
        let mut value = json!({
            "process_id": record.process_id,
            "state": record.state,
            "output": output,
            "next_offset": next_offset,
            "more_output": more,
        });
        if let Some(code) = record.exit_code {
            value["exit_code"] = json!(code);
        }
        if let Some(ended) = record.ended_at {
            value["duration_ms"] = json!(ended.as_millis() - record.started_at.as_millis());
        }
        if record.state == "unknown" {
            value["message"] = json!(
                "The worker restarted during this command, so nobody knows its result. Check the workspace. Do not start the command again without checking."
            );
        }
        value
    }

    /// Waits for a process to end, up to `wait`.
    pub async fn wait(&self, process_id: &str, wait: Duration) {
        let Some(live) = self.live(process_id) else {
            return;
        };
        let mut ended = live.ended.clone();
        let _ = tokio::time::timeout(wait, ended.wait_for(|e| *e)).await;
    }

    /// Sends text to a process that runs. Returns false when its input is
    /// closed.
    pub async fn write_stdin(&self, process_id: &str, text: &str) -> bool {
        let Some(live) = self.live(process_id) else {
            return false;
        };
        let mut stdin = live.stdin.lock().await;
        let Some(pipe) = stdin.as_mut() else {
            return false;
        };
        pipe.write_all(text.as_bytes()).await.is_ok() && pipe.flush().await.is_ok()
    }
}

/// Reads at most [`MAX_OUTPUT_BYTES`] from `offset`. Returns the text, the next
/// offset and whether more output follows.
fn read_output(path: &Path, offset: u64) -> (String, u64, bool) {
    let read = || -> std::io::Result<(String, u64, bool)> {
        let mut file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        let start = offset.min(len);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(MAX_OUTPUT_BYTES as u64).read_to_end(&mut bytes)?;
        // A cut can fall inside a character. The next read begins with it.
        let usable = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(err) if err.error_len().is_none() => err.valid_up_to(),
            Err(_) => bytes.len(),
        };
        let next = start + usable as u64;
        Ok((
            String::from_utf8_lossy(&bytes[..usable]).into_owned(),
            next,
            next < len,
        ))
    };
    read().unwrap_or((String::new(), offset, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn processes(dir: &Path) -> (Arc<Journal>, Arc<Processes>) {
        let journal = Arc::new(Journal::open(&dir.join("w.db")).unwrap());
        (
            journal.clone(),
            Arc::new(Processes::new(journal, dir.join("p"))),
        )
    }

    #[tokio::test]
    async fn a_command_runs_and_its_output_and_exit_code_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (journal, processes) = processes(dir.path());
        let session = SessionId::new();
        let record = processes
            .start(
                session,
                "echo hello; exit 3",
                dir.path(),
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        processes
            .wait(&record.process_id, Duration::from_secs(20))
            .await;
        let done = journal.process(&record.process_id).unwrap().unwrap();
        assert_eq!(done.state, "exited");
        assert_eq!(done.exit_code, Some(3));
        let snapshot = Processes::snapshot(&done, 0);
        assert!(snapshot["output"].as_str().unwrap().contains("hello"));
        assert_eq!(snapshot["exit_code"], 3);
        // Another session does not see the process.
        assert!(
            processes
                .record(SessionId::new(), &record.process_id)
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_deadline_ends_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let (journal, processes) = processes(dir.path());
        let record = processes
            .start(
                SessionId::new(),
                "sleep 60 & wait",
                dir.path(),
                Duration::from_millis(200),
            )
            .await
            .unwrap();
        processes
            .wait(&record.process_id, Duration::from_secs(10))
            .await;
        let done = journal.process(&record.process_id).unwrap().unwrap();
        assert_eq!(done.state, "exited");
        assert_eq!(done.exit_code, None);
        let pid = done.pid.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let group = std::process::Command::new("pgrep")
            .args(["-g", &pid.to_string()])
            .output()
            .unwrap();
        assert!(group.stdout.is_empty());
    }

    /// A worker restart in the middle of a command. The command runs on, and
    /// a new worker never starts it again.
    #[cfg(unix)]
    #[tokio::test]
    async fn after_a_restart_a_process_that_is_gone_has_an_unknown_result() {
        let dir = tempfile::tempdir().unwrap();
        let (journal, processes) = processes(dir.path());
        let gone = ProcessRecord {
            process_id: "gone".to_owned(),
            session_id: SessionId::new(),
            command: "x".to_owned(),
            cwd: "/".to_owned(),
            pid: Some(999_999),
            start_mark: Some("then".to_owned()),
            state: "running".to_owned(),
            exit_code: None,
            output_path: dir.path().join("none").to_string_lossy().into_owned(),
            started_at: Timestamp::now(),
            ended_at: None,
            deadline: Timestamp::now(),
        };
        journal.put_process(&gone).unwrap();
        processes.adopt().await.unwrap();
        let record = journal.process("gone").unwrap().unwrap();
        assert_eq!(record.state, "unknown");
        let snapshot = Processes::snapshot(&record, 0);
        assert!(
            snapshot["message"]
                .as_str()
                .unwrap()
                .contains("Do not start")
        );
    }

    #[test]
    fn a_read_gives_the_next_offset_and_does_not_cut_a_character() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("o");
        let mut text = "a".repeat(MAX_OUTPUT_BYTES - 1);
        text.push('é');
        text.push_str("tail");
        std::fs::write(&path, &text).unwrap();
        let (first, next, more) = read_output(&path, 0);
        assert_eq!(first.len(), MAX_OUTPUT_BYTES - 1);
        assert!(more);
        let (second, end, more) = read_output(&path, next);
        assert_eq!(second, "étail");
        assert!(!more);
        assert_eq!(end, text.len() as u64);
    }
}
