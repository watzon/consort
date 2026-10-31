//! Runs one [`ExecOp`]: finds the workspace by its capability, checks the
//! worker policy, asks the journal, runs the tool and records the result.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use consort_proto::execution_link::{ExecOp, ExecOpResult, ExecTool, MAX_PROCESS_WAIT_SECS};
use consort_proto::ids::SessionId;
use serde_json::{Value, json};

use crate::config::LocalMcpServerConfig;
use crate::journal::{Begin, Journal, StoredWorkspace};
use crate::limits::Limits;
use crate::local_mcp;
use crate::paths::{PathRules, contain};
use crate::process::{Processes, kill_tree};
use crate::tools::{self, Check, ToolError, ToolOutcome, sha256_hex};

/// The time limit of a command when the model gives none, in seconds.
/// The most commands that `process_list` returns.
const PROCESS_LIST_MAX: u32 = 50;

const DEFAULT_COMMAND_SECS: u64 = 30 * 60;
/// The longest time limit of a command, in seconds.
const MAX_COMMAND_SECS: u64 = 24 * 60 * 60;

/// What the tools ask from the hub.
#[async_trait]
pub trait HubFiles: Send + Sync {
    /// Uploads a blob. Returns its sha256.
    async fn blob_put(&self, bytes: Vec<u8>) -> Result<String, String>;
    /// Downloads a blob to a new file.
    async fn blob_get(&self, sha256: &str, dest: &Path) -> Result<(), String>;
    /// Makes a skill version ready. Returns its folder on this machine.
    async fn skill_folder(&self, name: &str, content_hash: &str) -> Result<PathBuf, String>;
}

/// The operation runner of a worker.
pub struct Exec {
    /// The journal.
    pub journal: Arc<Journal>,
    /// The processes.
    pub processes: Arc<Processes>,
    /// The limits of the worker.
    pub limits: Arc<Limits>,
    /// The local MCP servers.
    pub local_servers: Vec<LocalMcpServerConfig>,
    /// The rules for paths.
    pub rules: PathRules,
    /// The hub.
    pub hub: Arc<dyn HubFiles>,
    /// The operations that run now.
    running: Mutex<HashSet<(SessionId, String)>>,
}

/// The hash of a tool name and its arguments. `serde_json` keeps object keys
/// sorted in this build (no `preserve_order` feature), so the same arguments
/// give the same hash.
pub fn argument_hash(tool: &str, arguments: &Value) -> String {
    sha256_hex(format!("{tool}\n{arguments}").as_bytes())
}

fn failure(code: &'static str, message: impl Into<String>) -> ExecOpResult {
    ExecOpResult {
        result: ToolError::new(code, message).to_value(),
        replayed_from_journal: false,
        is_error: true,
    }
}

impl Exec {
    /// A new runner.
    pub fn new(
        journal: Arc<Journal>,
        processes: Arc<Processes>,
        limits: Arc<Limits>,
        local_servers: Vec<LocalMcpServerConfig>,
        rules: PathRules,
        hub: Arc<dyn HubFiles>,
    ) -> Self {
        Self {
            journal,
            processes,
            limits,
            local_servers,
            rules,
            hub,
            running: Mutex::new(HashSet::new()),
        }
    }

    fn mark_running(&self, key: (SessionId, String), running: bool) {
        let mut set = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if running {
            set.insert(key);
        } else {
            set.remove(&key);
        }
    }

    fn is_running(&self, key: &(SessionId, String)) -> bool {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(key)
    }

    /// Runs an operation.
    pub async fn run(self: &Arc<Self>, op: ExecOp) -> ExecOpResult {
        let workspace = match self.journal.workspace_of(&op.capability) {
            Ok(Some(workspace)) => workspace,
            Ok(None) => {
                return failure(
                    "workspace_unknown",
                    "This worker has no workspace for this session. Ask the user to start the task again.",
                );
            }
            Err(err) => return failure("journal_failed", err.to_string()),
        };
        let Some(tool) = ExecTool::from_name(&op.tool) else {
            return failure(
                "unknown_tool",
                format!("This worker has no tool `{}`.", op.tool),
            );
        };
        // The limits can change while a session runs.
        let limits = self.limits.get();
        if matches!(tool, ExecTool::ProcessStart | ExecTool::ProcessWriteStdin)
            && !limits.allow_shell
        {
            return failure(
                "policy_denied",
                "The limits of this worker do not allow shell commands. The user can change this in Settings > Workers.",
            );
        }
        if let Err(err) = crate::workspace::check_record_allowed(&workspace.record, &limits) {
            return failure("policy_denied", err.to_string());
        }
        if !tool.needs_operation_id() {
            return match self.run_tool(tool, &op, &workspace).await {
                Ok((result, _)) => ExecOpResult {
                    result,
                    replayed_from_journal: false,
                    is_error: false,
                },
                Err(err) => ExecOpResult {
                    result: err.to_value(),
                    replayed_from_journal: false,
                    is_error: true,
                },
            };
        }
        let hash = argument_hash(&op.tool, &op.arguments);
        let key = (op.session_id, op.operation_id.clone());
        // The hub sends an operation again when a link drops. The first call
        // may still run.
        while self.is_running(&key) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let begin = self
            .journal
            .begin(op.session_id, &op.operation_id, &hash, &op.tool);
        match begin {
            Err(err) => failure("journal_failed", err.to_string()),
            Ok(Begin::Reused) => failure(
                "operation_id_reused",
                format!(
                    "The operation_id `{}` was used with other arguments in this session. Pass a new operation_id for a new call.",
                    op.operation_id
                ),
            ),
            Ok(Begin::Unknown) => failure(
                "result_unknown",
                "This operation began before the worker restarted, and nobody knows its result. It did not run again. Check the workspace. To run it again, pass a new operation_id.",
            ),
            Ok(Begin::Replay { result, check }) => self.replay(&workspace, result, check),
            Ok(Begin::Run) => {
                self.mark_running(key.clone(), true);
                let outcome = self.run_tool(tool, &op, &workspace).await;
                let out = match outcome {
                    Ok((result, check)) => {
                        let recorded = self.journal.finish(
                            op.session_id,
                            &op.operation_id,
                            &result,
                            check.as_ref().map(|(p, s)| (p.as_str(), s.as_str())),
                        );
                        if let Err(err) = recorded {
                            tracing::warn!(error = %err, "could not record an operation");
                        }
                        ExecOpResult {
                            result,
                            replayed_from_journal: false,
                            is_error: false,
                        }
                    }
                    Err(err) => {
                        // A refusal changed nothing, so the same ID can run.
                        if let Err(e) = self.journal.forget(op.session_id, &op.operation_id) {
                            tracing::warn!(error = %e, "could not forget a refused operation");
                        }
                        ExecOpResult {
                            result: err.to_value(),
                            replayed_from_journal: false,
                            is_error: true,
                        }
                    }
                };
                self.mark_running(key, false);
                out
            }
        }
    }

    /// The stored result, after a check that the file is as the operation left
    /// it.
    fn replay(
        &self,
        workspace: &StoredWorkspace,
        result: Value,
        check: Option<(String, String)>,
    ) -> ExecOpResult {
        if let Some((path, expected)) = check {
            let actual = contain(Path::new(&workspace.path), &path, self.rules)
                .ok()
                .and_then(|p| std::fs::read(p).ok())
                .map(|bytes| sha256_hex(&bytes));
            if actual.as_deref() != Some(expected.as_str()) {
                return failure(
                    "state_changed_since_operation",
                    format!(
                        "This operation ran before, but `{path}` changed since then. Read the file again, and pass a new operation_id for a new change."
                    ),
                );
            }
        }
        ExecOpResult {
            result,
            replayed_from_journal: true,
            is_error: false,
        }
    }

    async fn run_tool(
        self: &Arc<Self>,
        tool: ExecTool,
        op: &ExecOp,
        workspace: &StoredWorkspace,
    ) -> ToolOutcome {
        let root = PathBuf::from(&workspace.path);
        let args = &op.arguments;
        let rules = self.rules;
        match tool {
            ExecTool::WorkspaceInspect => {
                let inspection = crate::core::inspect(workspace).await;
                let value = serde_json::to_value(&inspection)
                    .map_err(|e| ToolError::new("internal", e.to_string()))?;
                Ok((value, None))
            }
            ExecTool::FsList => {
                blocking(root, args, move |r, a| {
                    tools::fs_list(r, a, rules).map(|v| (v, None))
                })
                .await
            }
            ExecTool::FsStat => {
                blocking(root, args, move |r, a| {
                    tools::fs_stat(r, a, rules).map(|v| (v, None))
                })
                .await
            }
            ExecTool::FsRead => {
                blocking(root, args, move |r, a| {
                    tools::fs_read(r, a, rules).map(|v| (v, None))
                })
                .await
            }
            ExecTool::FsSearch => {
                blocking(root, args, move |r, a| {
                    tools::fs_search(r, a, rules).map(|v| (v, None))
                })
                .await
            }
            ExecTool::FsApplyPatch => {
                blocking(root, args, move |r, a| tools::fs_apply_patch(r, a, rules)).await
            }
            ExecTool::FsWrite => {
                blocking(root, args, move |r, a| tools::fs_write(r, a, rules)).await
            }
            ExecTool::FsMove => blocking(root, args, move |r, a| tools::fs_move(r, a, rules)).await,
            ExecTool::FsRemove => {
                blocking(root, args, move |r, a| tools::fs_remove(r, a, rules)).await
            }
            ExecTool::ProcessStart => self.process_start(op, &root).await,
            ExecTool::ProcessRead => self.process_read(op).map(|v| (v, None)),
            ExecTool::ProcessWait => self.process_wait(op).await.map(|v| (v, None)),
            ExecTool::ProcessList => self.process_list(op).map(|v| (v, None)),
            ExecTool::ProcessWriteStdin => self.process_stdin(op).await.map(|v| (v, None)),
            ExecTool::ProcessSignal => self.process_signal(op).await.map(|v| (v, None)),
            ExecTool::BlobPut => self.blob_put(op, &root).await.map(|v| (v, None)),
            ExecTool::BlobGet => self.blob_get(op, &root).await,
            ExecTool::SkillPrepare => self.skill_prepare(op).await.map(|v| (v, None)),
            ExecTool::LocalMcpListTools => self.local_list(op).await.map(|v| (v, None)),
            ExecTool::LocalMcpCall => self.local_call(op).await.map(|v| (v, None)),
        }
    }

    async fn process_start(&self, op: &ExecOp, root: &Path) -> ToolOutcome {
        let args = &op.arguments;
        let command = required(args, "command")?;
        let cwd = match args.get("cwd").and_then(Value::as_str) {
            Some(given) if !given.trim().is_empty() && given.trim() != "." => {
                let path = contain(root, given, self.rules)
                    .map_err(|e| ToolError::new("path_refused", e.to_string()))?;
                if !path.is_dir() {
                    return Err(ToolError::new(
                        "not_found",
                        format!("`{given}` is not a folder in the workspace."),
                    ));
                }
                path
            }
            _ => root.to_path_buf(),
        };
        let secs = args
            .get("timeout_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_COMMAND_SECS)
            .clamp(1, MAX_COMMAND_SECS);
        let processes = self.processes.clone();
        let record = processes
            .start(op.session_id, command, &cwd, Duration::from_secs(secs))
            .await
            .map_err(|e| ToolError::new("start_failed", e.to_string()))?;
        Ok((
            json!({"process_id": record.process_id, "state": "running", "pid": record.pid}),
            None,
        ))
    }

    fn known_process(&self, op: &ExecOp) -> Result<crate::journal::ProcessRecord, ToolError> {
        let id = required(&op.arguments, "process_id")?;
        self.processes
            .record(op.session_id, id)
            .map_err(|e| ToolError::new("journal_failed", e.to_string()))?
            .ok_or_else(|| {
                ToolError::new("not_found", format!("This session has no process `{id}`."))
            })
    }

    fn process_read(&self, op: &ExecOp) -> Result<Value, ToolError> {
        let record = self.known_process(op)?;
        let offset = op
            .arguments
            .get("offset")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        Ok(Processes::snapshot(&record, offset))
    }

    /// The commands of the session and the operations with no known result.
    /// An operation that runs now has a result to come, so it is not listed.
    fn process_list(&self, op: &ExecOp) -> Result<Value, ToolError> {
        let journal_error = |e: crate::WorkerError| ToolError::new("journal_failed", e.to_string());
        let processes = self
            .journal
            .session_processes(op.session_id, PROCESS_LIST_MAX)
            .map_err(journal_error)?;
        let unknown: Vec<Value> = self
            .journal
            .open_operations(op.session_id)
            .map_err(journal_error)?
            .into_iter()
            .filter(|(id, _)| !self.is_running(&(op.session_id, id.clone())))
            .map(|(operation_id, tool)| json!({"operation_id": operation_id, "tool": tool}))
            .collect();
        let processes: Vec<Value> = processes
            .iter()
            .map(|p| {
                json!({
                    "process_id": p.process_id,
                    "command": p.command,
                    "state": p.state,
                    "exit_code": p.exit_code,
                })
            })
            .collect();
        Ok(json!({"processes": processes, "unknown_operations": unknown}))
    }

    async fn process_wait(&self, op: &ExecOp) -> Result<Value, ToolError> {
        let record = self.known_process(op)?;
        let secs = op
            .arguments
            .get("timeout_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(MAX_PROCESS_WAIT_SECS)
            .min(MAX_PROCESS_WAIT_SECS);
        self.processes
            .wait(&record.process_id, Duration::from_secs(secs))
            .await;
        self.process_read(op)
    }

    async fn process_stdin(&self, op: &ExecOp) -> Result<Value, ToolError> {
        let record = self.known_process(op)?;
        let text = required(&op.arguments, "text")?;
        if self.processes.write_stdin(&record.process_id, text).await {
            Ok(json!({"written": text.len()}))
        } else {
            Err(ToolError::new(
                "stdin_closed",
                "The input of this process is closed. The process ended, or the worker restarted after it began.",
            ))
        }
    }

    async fn process_signal(&self, op: &ExecOp) -> Result<Value, ToolError> {
        let record = self.known_process(op)?;
        if record.state != "running" {
            return Ok(json!({"state": record.state, "signalled": false}));
        }
        let Some(pid) = record.pid else {
            return Ok(json!({"state": record.state, "signalled": false}));
        };
        let force = op.arguments.get("signal").and_then(Value::as_str) == Some("kill");
        kill_tree(pid, force).await;
        Ok(json!({"signalled": true}))
    }

    async fn blob_put(&self, op: &ExecOp, root: &Path) -> Result<Value, ToolError> {
        let given = required(&op.arguments, "path")?;
        let path = contain(root, given, self.rules)
            .map_err(|e| ToolError::new("path_refused", e.to_string()))?;
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| ToolError::new("not_found", format!("Could not read `{given}`: {e}")))?;
        let size = bytes.len();
        let sha256 = self
            .hub
            .blob_put(bytes)
            .await
            .map_err(|e| ToolError::new("upload_failed", e))?;
        Ok(json!({"sha256": sha256, "size": size}))
    }

    async fn blob_get(&self, op: &ExecOp, root: &Path) -> ToolOutcome {
        let given = required(&op.arguments, "path")?;
        let sha256 = required(&op.arguments, "sha256")?;
        let path = contain(root, given, self.rules)
            .map_err(|e| ToolError::new("path_refused", e.to_string()))?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::new("io_error", e.to_string()))?;
        }
        self.hub
            .blob_get(sha256, &path)
            .await
            .map_err(|e| ToolError::new("download_failed", e))?;
        Ok((
            json!({"path": given, "sha256": sha256}),
            Some((given.to_owned(), sha256.to_owned())),
        ))
    }

    /// The hub adds `content_hash`, the version that this session uses.
    async fn skill_prepare(&self, op: &ExecOp) -> Result<Value, ToolError> {
        let name = required(&op.arguments, "name")?;
        let hash = required(&op.arguments, "content_hash")?;
        let folder = self
            .hub
            .skill_folder(name, hash)
            .await
            .map_err(|e| ToolError::new("skill_failed", e))?;
        Ok(json!({"name": name, "path": folder.to_string_lossy()}))
    }

    fn local_server(&self, name: &str) -> Result<&LocalMcpServerConfig, ToolError> {
        self.local_servers
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| {
                ToolError::new(
                    "not_found",
                    format!("This worker has no local MCP server `{name}`."),
                )
            })
    }

    async fn local_list(&self, op: &ExecOp) -> Result<Value, ToolError> {
        let wanted = op.arguments.get("server").and_then(Value::as_str);
        let mut out = Vec::new();
        for server in &self.local_servers {
            if wanted.is_some_and(|w| w != server.name) {
                continue;
            }
            match local_mcp::list_tools(&server.name, &server.url).await {
                Ok(tools) => out.push(json!({"server": server.name, "tools": tools})),
                Err(err) => out.push(json!({"server": server.name, "error": err.message})),
            }
        }
        Ok(json!({"servers": out}))
    }

    async fn local_call(&self, op: &ExecOp) -> Result<Value, ToolError> {
        let name = required(&op.arguments, "server")?;
        let tool = required(&op.arguments, "tool")?;
        let server = self.local_server(name)?;
        let empty = json!({});
        let arguments = op.arguments.get("arguments").unwrap_or(&empty);
        local_mcp::call_tool(name, &server.url, tool, arguments).await
    }
}

fn required<'a>(args: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("invalid_arguments", format!("`{name}` is required.")))
}

async fn blocking(
    root: PathBuf,
    args: &Value,
    work: impl FnOnce(&Path, &Value) -> Result<(Value, Check), ToolError> + Send + 'static,
) -> ToolOutcome {
    let args = args.clone();
    tokio::task::spawn_blocking(move || work(&root, &args))
        .await
        .map_err(|e| ToolError::new("internal", e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{WorkspaceEntry, WorkspaceRecord};
    use consort_proto::time::Timestamp;

    struct NoHub;

    #[async_trait]
    impl HubFiles for NoHub {
        async fn blob_put(&self, _: Vec<u8>) -> Result<String, String> {
            Err("no hub".to_owned())
        }
        async fn blob_get(&self, _: &str, _: &Path) -> Result<(), String> {
            Err("no hub".to_owned())
        }
        async fn skill_folder(&self, _: &str, _: &str) -> Result<PathBuf, String> {
            Err("no hub".to_owned())
        }
    }

    fn runner(dir: &Path, policy: crate::config::PolicyConfig) -> Arc<Exec> {
        let journal = Arc::new(Journal::open(&dir.join("w.db")).unwrap());
        let workspace = dir.join("w");
        std::fs::create_dir_all(&workspace).unwrap();
        let path = workspace.to_string_lossy().into_owned();
        journal
            .put_workspace(&StoredWorkspace {
                key: "k".to_owned(),
                capability: "cap".to_owned(),
                path: path.clone(),
                record: WorkspaceRecord {
                    task_id: None,
                    root: path.clone(),
                    entries: vec![WorkspaceEntry::Empty { path }],
                },
            })
            .unwrap();
        let processes = Arc::new(Processes::new(journal.clone(), dir.join("p")));
        let limits = Arc::new(Limits::starting_with(journal.clone(), policy));
        Arc::new(Exec::new(
            journal,
            processes,
            limits,
            Vec::new(),
            PathRules::Unix,
            Arc::new(NoHub),
        ))
    }

    fn op(session: SessionId, tool: &str, id: &str, arguments: Value) -> ExecOp {
        ExecOp {
            session_id: session,
            capability: "cap".to_owned(),
            operation_id: id.to_owned(),
            tool: tool.to_owned(),
            arguments,
            deadline: Timestamp::now().plus_millis(60_000),
            policy_version: 0,
        }
    }

    #[tokio::test]
    async fn the_same_write_replays_and_other_arguments_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let exec = runner(dir.path(), crate::config::PolicyConfig::default());
        let s = SessionId::new();
        let args = json!({"path": "a.txt", "content": "one"});
        let first = exec.run(op(s, "fs_write", "op1", args.clone())).await;
        assert!(!first.is_error);
        assert!(!first.replayed_from_journal);
        let again = exec.run(op(s, "fs_write", "op1", args)).await;
        assert!(again.replayed_from_journal);
        assert_eq!(again.result, first.result);
        let other = exec
            .run(op(
                s,
                "fs_write",
                "op1",
                json!({"path": "b.txt", "content": "two"}),
            ))
            .await;
        assert_eq!(other.result["error_code"], "operation_id_reused");
    }

    #[tokio::test]
    async fn a_replay_checks_the_file_that_the_patch_left() {
        let dir = tempfile::tempdir().unwrap();
        let exec = runner(dir.path(), crate::config::PolicyConfig::default());
        let s = SessionId::new();
        let args = json!({"path": "a.txt", "content": "one"});
        exec.run(op(s, "fs_write", "op1", args.clone())).await;
        std::fs::write(dir.path().join("w/a.txt"), "changed").unwrap();
        let replay = exec.run(op(s, "fs_write", "op1", args)).await;
        assert_eq!(replay.result["error_code"], "state_changed_since_operation");
    }

    /// The defect of the experiment: a model picks the same ID in a new
    /// session, and a shared journal gives the old result.
    #[tokio::test]
    async fn a_command_with_a_repeated_id_in_a_new_session_runs() {
        let dir = tempfile::tempdir().unwrap();
        let exec = runner(dir.path(), crate::config::PolicyConfig::default());
        let args = json!({"command": "echo x >> count"});
        let a = exec
            .run(op(SessionId::new(), "process_start", "run-1", args.clone()))
            .await;
        let b = exec
            .run(op(SessionId::new(), "process_start", "run-1", args))
            .await;
        assert!(!a.is_error);
        assert!(!b.is_error);
        assert!(!b.replayed_from_journal);
        assert_ne!(a.result["process_id"], b.result["process_id"]);
    }

    #[tokio::test]
    async fn a_command_runs_once_when_the_call_comes_twice() {
        let dir = tempfile::tempdir().unwrap();
        let exec = runner(dir.path(), crate::config::PolicyConfig::default());
        let s = SessionId::new();
        let args = json!({"command": "echo x >> count"});
        let first = exec.run(op(s, "process_start", "op", args.clone())).await;
        let second = exec.run(op(s, "process_start", "op", args)).await;
        assert!(second.replayed_from_journal);
        let id = first.result["process_id"].as_str().unwrap();
        let waited = exec
            .run(op(
                s,
                "process_wait",
                "w",
                json!({"process_id": id, "timeout_seconds": 20}),
            ))
            .await;
        assert_eq!(waited.result["state"], "exited");
        // Windows PowerShell writes UTF-16 with `>>`, so count the bytes.
        let bytes = std::fs::read(dir.path().join("w/count")).unwrap();
        assert_eq!(bytes.iter().filter(|b| **b == b'x').count(), 1);
    }

    #[tokio::test]
    async fn an_operation_that_began_before_a_restart_does_not_run_again() {
        let dir = tempfile::tempdir().unwrap();
        let exec = runner(dir.path(), crate::config::PolicyConfig::default());
        let s = SessionId::new();
        let args = json!({"command": "echo x >> count"});
        let hash = argument_hash("process_start", &args);
        exec.journal.begin(s, "op", &hash, "process_start").unwrap();
        let out = exec.run(op(s, "process_start", "op", args)).await;
        assert_eq!(out.result["error_code"], "result_unknown");
        assert!(!dir.path().join("w/count").exists());
    }

    #[tokio::test]
    async fn the_process_list_shows_the_commands_and_the_unknown_operations_of_a_session() {
        let dir = tempfile::tempdir().unwrap();
        let exec = runner(dir.path(), crate::config::PolicyConfig::default());
        let s = SessionId::new();
        let started = exec
            .run(op(s, "process_start", "op", json!({"command": "echo x"})))
            .await;
        let args = json!({"path": "a.txt", "content": "x"});
        let hash = argument_hash("fs_write", &args);
        exec.journal.begin(s, "lost", &hash, "fs_write").unwrap();
        let list = exec.run(op(s, "process_list", "l", json!({}))).await;
        assert!(!list.is_error, "{:?}", list.result);
        assert_eq!(
            list.result["processes"][0]["process_id"],
            started.result["process_id"]
        );
        assert_eq!(list.result["processes"][0]["command"], "echo x");
        assert_eq!(
            list.result["unknown_operations"],
            json!([{"operation_id": "lost", "tool": "fs_write"}])
        );
        // Another session sees nothing of this one.
        let other = exec
            .run(op(SessionId::new(), "process_list", "l", json!({})))
            .await;
        assert_eq!(other.result["processes"], json!([]));
    }

    #[tokio::test]
    async fn a_refused_change_can_run_again_with_the_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let exec = runner(dir.path(), crate::config::PolicyConfig::default());
        let s = SessionId::new();
        let args = json!({"path": "/etc/passwd", "content": "x"});
        let refused = exec.run(op(s, "fs_write", "op", args.clone())).await;
        assert_eq!(refused.result["error_code"], "path_refused");
        let again = exec.run(op(s, "fs_write", "op", args)).await;
        assert!(!again.replayed_from_journal);
    }

    #[tokio::test]
    async fn a_change_of_the_limits_applies_to_the_next_operation() {
        let dir = tempfile::tempdir().unwrap();
        let exec = runner(dir.path(), crate::config::PolicyConfig::default());
        let s = SessionId::new();
        exec.limits
            .change(consort_proto::execution_link::PolicySet {
                allow_shell: Some(false),
                ..Default::default()
            })
            .unwrap();
        let out = exec
            .run(op(s, "process_start", "op", json!({"command": "echo x"})))
            .await;
        assert_eq!(out.result["error_code"], "policy_denied");
        exec.limits
            .change(consort_proto::execution_link::PolicySet {
                allow_shell: Some(true),
                ..Default::default()
            })
            .unwrap();
        let out = exec
            .run(op(s, "process_start", "op2", json!({"command": "echo x"})))
            .await;
        assert!(!out.is_error, "{:?}", out.result);
    }

    #[tokio::test]
    async fn the_policy_of_the_worker_can_refuse_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        let policy = crate::config::PolicyConfig {
            allow_shell: false,
            ..crate::config::PolicyConfig::default()
        };
        let exec = runner(dir.path(), policy);
        let out = exec
            .run(op(
                SessionId::new(),
                "process_start",
                "op",
                json!({"command": "echo"}),
            ))
            .await;
        assert_eq!(out.result["error_code"], "policy_denied");
    }

    #[tokio::test]
    async fn an_unknown_capability_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let exec = runner(dir.path(), crate::config::PolicyConfig::default());
        let mut call = op(SessionId::new(), "fs_list", "op", json!({}));
        call.capability = "other".to_owned();
        assert_eq!(
            exec.run(call).await.result["error_code"],
            "workspace_unknown"
        );
    }
}
