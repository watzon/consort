//! The execution link: messages between an execution worker and the hub.
//!
//! A worker is a machine where work runs. It holds no account and it runs no
//! harness. [`HubToExecution`] has the hub's requests, and [`ExecutionToHub`]
//! has what a worker sends. The session host has its own link
//! ([`crate::session_host_link`]), with its own route and its own credentials.
//!
//! Each file or process request is one [`ExecOp`]. It carries a workspace
//! capability, an operation ID, a hash of the arguments, a deadline and a
//! policy version. The worker runs an operation once: the journal key is the
//! session, the operation ID and the argument hash.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::{WorkerHello, WorkerHelloResult};
use crate::frame::{Empty, message_family};
use crate::ids::{SessionId, TaskId};
use crate::model::{BlobRef, FileRef, Inventory, Os, SkillBundle};
use crate::session_host_link::{SkillBundleGet, WorkspaceSpec};
use crate::time::Timestamp;
use crate::tools::ToolDefinition;

/// The longest `process_wait`, in seconds. The model calls it again for a
/// longer wait (docs/research/hub-boundary-experiment.md, "The limit on a
/// pending tool call").
pub const MAX_PROCESS_WAIT_SECS: u64 = 30;
/// The most output bytes that one `process_read` or `process_wait` returns.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// The most bytes that one `fs_read` returns.
pub const MAX_READ_BYTES: u64 = 512 * 1024;

/// What a model needs to know about one workspace. The session host's CLI
/// tells the model the hub's folder, OS and shell, and that cannot be turned
/// off. So Consort gives this to the model at the start, at a resume, at a
/// change of worker and on each turn, and says that it overrides that text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInspection {
    /// The capability of this workspace.
    pub capability: String,
    /// The worker's name, as its machine reports it.
    pub worker: String,
    /// The operating system of the worker.
    pub os: Os,
    /// The CPU architecture of the worker.
    pub arch: String,
    /// The root of the workspace, in the worker's own form.
    pub native_root: String,
    /// The path separator on the worker.
    pub path_separator: String,
    /// Whether paths on the worker ignore case.
    pub case_insensitive_paths: bool,
    /// The shell that `process_start` uses.
    pub shell: String,
    /// The runtimes that the worker found, such as `git 2.47.0`.
    #[serde(default)]
    pub runtimes: Vec<String>,
    /// What was made for this workspace: `in_place`, `worktree`, `clone` or
    /// `empty`.
    #[serde(default)]
    pub layout: String,
}

impl WorkspaceInspection {
    /// The environment message. It overrides what the CLI says.
    pub fn environment_message(&self) -> String {
        let os = match self.os {
            Os::Macos => "macOS",
            Os::Linux => "Linux",
            Os::Windows => "Windows",
        };
        let runtimes = if self.runtimes.is_empty() {
            "none found".to_owned()
        } else {
            self.runtimes.join(", ")
        };
        format!(
            "<consort_environment>\nThis text overrides any folder, platform, OS version or shell that you were told elsewhere. Your harness runs on another machine, and you cannot reach that machine. All your work happens on the worker below, through the Consort tools only.\nWorker: {worker}\nOS: {os} ({arch})\nWorkspace root on the worker: {root}\nPath rules: give paths relative to the workspace root, with `{separator}` or `/`. An absolute path is refused. Case {case}.\nShell for process_start: {shell}\nRuntimes: {runtimes}\n</consort_environment>",
            worker = self.worker,
            arch = self.arch,
            root = self.native_root,
            separator = self.path_separator,
            case = if self.case_insensitive_paths {
                "does not matter"
            } else {
                "matters"
            },
            shell = self.shell,
        )
    }
}

/// `workspace.prepare`: make or find the workspace of a session. Idempotent
/// for one key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePrepare {
    /// The capability that the hub made. Later requests name it.
    pub capability: String,
    /// The task, for a task session.
    pub task_id: Option<TaskId>,
    /// The key of the workspace: the task ID or the session ID.
    pub key: String,
    /// What to make.
    pub spec: WorkspaceSpec,
    /// The channel files. The worker puts a read-only copy in
    /// `<workspace>/.channel/files/`.
    #[serde(default)]
    pub channel_files: Vec<FileRef>,
}

/// `workspace.inspect`, from the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInspect {
    /// The capability.
    pub capability: String,
}

/// `workspace.release`: the task no longer needs its workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecWorkspaceRelease {
    /// The key of the workspace.
    pub key: String,
    /// Delete the worktree or clone. A checkout used in place is never
    /// deleted.
    pub delete: bool,
}

/// `exec.op`: one tool call on a worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecOp {
    /// The session.
    pub session_id: SessionId,
    /// The workspace capability.
    pub capability: String,
    /// The retry key. The model picks it for a change, and the session host
    /// picks it for a read.
    pub operation_id: String,
    /// The tool, by its [`ExecTool::name`].
    pub tool: String,
    /// The arguments. They hold no `operation_id`.
    pub arguments: Value,
    /// When the worker stops waiting for this operation.
    pub deadline: Timestamp,
    /// The version of the worker policy that the hub saw. A worker with a
    /// newer policy checks the operation against its own.
    pub policy_version: u64,
}

/// The result of [`ExecOp`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecOpResult {
    /// The result of the tool.
    pub result: Value,
    /// True when this comes from the journal: the operation had run before,
    /// and it did not run again.
    #[serde(default)]
    pub replayed_from_journal: bool,
    /// True when the tool refused or failed. Then `result` has `error_code`
    /// and `message`, and the model reads them.
    #[serde(default)]
    pub is_error: bool,
}

/// `workspace.sync`: put the files of a turn in the workspace, before the turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSync {
    /// The capability.
    pub capability: String,
    /// The attachments of the turn.
    #[serde(default)]
    pub attachments: Vec<BlobRef>,
    /// The channel files, when the hub wants the copy to change.
    #[serde(default)]
    pub channel_files: Option<Vec<FileRef>>,
}

/// The result of [`WorkspaceSync`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSyncResult {
    /// A message for each file that the session did not get.
    #[serde(default)]
    pub failures: Vec<String>,
}

/// `policy.set`: change the limits of the worker. A missing field keeps its
/// value. The worker saves the result, applies it at once and answers with
/// the policy that it uses now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySet {
    /// The highest approval mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_approval_mode: Option<crate::model::ApprovalMode>,
    /// The folders that tasks may use. Empty means no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_roots: Option<Vec<String>>,
    /// Whether tasks may run shell commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_shell: Option<bool>,
}

/// `inventory.refresh`, on the execution link.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecInventoryRefresh {}

/// The `hello` of a worker. It holds no sessions, because a worker has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecHello(pub WorkerHello);

/// The inventory of a worker. Its `accounts` and `tools` are empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecInventory(pub Inventory);

/// `skill_bundle.get` on the execution link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecSkillBundleGet(pub SkillBundleGet);

message_family! {
    /// Everything that a worker sends to the hub after authentication.
    pub enum ExecutionToHub {
        /// The first message after authentication.
        Hello("hello", ExecHello => WorkerHelloResult),
        /// A full inventory. No response.
        Inventory("inventory", ExecInventory),
        /// The files of a skill version, for `skill_prepare`.
        SkillBundleGet("skill_bundle.get", ExecSkillBundleGet => SkillBundle),
    }
}

message_family! {
    /// The hub requests to a worker.
    pub enum HubToExecution {
        /// Make or find a workspace.
        WorkspacePrepare("workspace.prepare", WorkspacePrepare => WorkspaceInspection),
        /// Describe a workspace.
        WorkspaceInspect("workspace.inspect", WorkspaceInspect => WorkspaceInspection),
        /// Release a workspace.
        WorkspaceRelease("workspace.release", ExecWorkspaceRelease => Empty),
        /// Put the files of a turn in a workspace.
        WorkspaceSync("workspace.sync", WorkspaceSync => WorkspaceSyncResult),
        /// Run one tool call.
        ExecOp("exec.op", ExecOp => ExecOpResult),
        /// Send a new inventory.
        InventoryRefresh("inventory.refresh", ExecInventoryRefresh => Empty),
        /// Change the limits of the worker.
        PolicySet("policy.set", PolicySet => crate::model::WorkerPolicy),
    }
}

/// What a tool does, for the approval engine. See the table in
/// `docs/HUB-PLAN.md`, "Approval modes".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    /// A read, or the output of a process.
    Read,
    /// A write or a patch in the workspace.
    Write,
    /// A removal.
    Remove,
    /// A shell command.
    Shell,
    /// A tool of a local MCP server. Its own class comes from its
    /// annotations, and an unknown one asks.
    LocalMcp,
}

/// The tools that run on a worker. The model sees each as
/// `mcp__consort__<name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecTool {
    /// `workspace_inspect`.
    WorkspaceInspect,
    /// `fs_list`.
    FsList,
    /// `fs_stat`.
    FsStat,
    /// `fs_read`.
    FsRead,
    /// `fs_search`.
    FsSearch,
    /// `fs_apply_patch`.
    FsApplyPatch,
    /// `fs_write`.
    FsWrite,
    /// `fs_move`.
    FsMove,
    /// `fs_remove`.
    FsRemove,
    /// `process_start`.
    ProcessStart,
    /// `process_read`.
    ProcessRead,
    /// `process_write_stdin`.
    ProcessWriteStdin,
    /// `process_signal`.
    ProcessSignal,
    /// `process_wait`.
    ProcessWait,
    /// `process_list`.
    ProcessList,
    /// `blob_put`.
    BlobPut,
    /// `blob_get`.
    BlobGet,
    /// `skill_prepare`.
    SkillPrepare,
    /// `local_mcp_list_tools`.
    LocalMcpListTools,
    /// `local_mcp_call`.
    LocalMcpCall,
}

impl ExecTool {
    /// Every tool.
    pub const ALL: [Self; 20] = [
        Self::WorkspaceInspect,
        Self::FsList,
        Self::FsStat,
        Self::FsRead,
        Self::FsSearch,
        Self::FsApplyPatch,
        Self::FsWrite,
        Self::FsMove,
        Self::FsRemove,
        Self::ProcessStart,
        Self::ProcessRead,
        Self::ProcessWriteStdin,
        Self::ProcessSignal,
        Self::ProcessWait,
        Self::ProcessList,
        Self::BlobPut,
        Self::BlobGet,
        Self::SkillPrepare,
        Self::LocalMcpListTools,
        Self::LocalMcpCall,
    ];

    /// The name on the wire and in the tool list.
    pub const fn name(self) -> &'static str {
        match self {
            Self::WorkspaceInspect => "workspace_inspect",
            Self::FsList => "fs_list",
            Self::FsStat => "fs_stat",
            Self::FsRead => "fs_read",
            Self::FsSearch => "fs_search",
            Self::FsApplyPatch => "fs_apply_patch",
            Self::FsWrite => "fs_write",
            Self::FsMove => "fs_move",
            Self::FsRemove => "fs_remove",
            Self::ProcessStart => "process_start",
            Self::ProcessRead => "process_read",
            Self::ProcessWriteStdin => "process_write_stdin",
            Self::ProcessSignal => "process_signal",
            Self::ProcessWait => "process_wait",
            Self::ProcessList => "process_list",
            Self::BlobPut => "blob_put",
            Self::BlobGet => "blob_get",
            Self::SkillPrepare => "skill_prepare",
            Self::LocalMcpListTools => "local_mcp_list_tools",
            Self::LocalMcpCall => "local_mcp_call",
        }
    }

    /// The tool with this name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The class of the tool.
    pub const fn class(self) -> ToolClass {
        match self {
            Self::WorkspaceInspect
            | Self::FsList
            | Self::FsStat
            | Self::FsRead
            | Self::FsSearch
            | Self::ProcessRead
            | Self::ProcessWait
            | Self::ProcessList
            | Self::BlobPut
            | Self::SkillPrepare
            | Self::LocalMcpListTools => ToolClass::Read,
            // A signal changes only a process that was allowed before.
            // `blob_get` writes one file in the workspace.
            Self::FsApplyPatch
            | Self::FsWrite
            | Self::FsMove
            | Self::ProcessSignal
            | Self::BlobGet => ToolClass::Write,
            Self::FsRemove => ToolClass::Remove,
            // Text to a running shell is a new command.
            Self::ProcessStart | Self::ProcessWriteStdin => ToolClass::Shell,
            Self::LocalMcpCall => ToolClass::LocalMcp,
        }
    }

    /// Whether the model must pick an `operation_id`, because the call
    /// changes something.
    pub const fn needs_operation_id(self) -> bool {
        matches!(
            self,
            Self::FsApplyPatch
                | Self::FsWrite
                | Self::FsMove
                | Self::FsRemove
                | Self::ProcessStart
                | Self::ProcessWriteStdin
                | Self::ProcessSignal
                | Self::BlobGet
                | Self::LocalMcpCall
        )
    }

    /// The tool definition that the model sees.
    pub fn definition(self) -> ToolDefinition {
        const OPERATION_ID: &str = " Pass a new operation_id for each new call. If a call fails with no clear result, call it again with the same operation_id and the same arguments, so that the work does not run twice.";
        let path =
            json!({"type": "string", "description": "A path relative to the workspace root."});
        let (description, mut properties, mut required): (&str, Value, Vec<&str>) = match self {
            Self::WorkspaceInspect => (
                "Describe the workspace: worker, OS, root, path rules, shell and runtimes.",
                json!({}),
                vec![],
            ),
            Self::FsList => (
                "List a folder in the workspace.",
                json!({"path": path, "recursive": {"type": "boolean"}}),
                vec![],
            ),
            Self::FsStat => (
                "Describe one file or folder in the workspace: kind, size and sha256.",
                json!({"path": path}),
                vec!["path"],
            ),
            Self::FsRead => (
                "Read a text file in the workspace. Returns the text and its sha256. Use offset and limit, in bytes, for a large file.",
                json!({"path": path, "offset": {"type": "integer"}, "limit": {"type": "integer"}}),
                vec!["path"],
            ),
            Self::FsSearch => (
                "Search the text files of the workspace for a regular expression. Returns the path, the line number and the line.",
                json!({"pattern": {"type": "string"}, "path": path, "max_results": {"type": "integer"}}),
                vec!["pattern"],
            ),
            Self::FsApplyPatch => (
                "Replace text in one file in the workspace. expected_sha256 must be the hash from fs_read.",
                json!({"path": path, "expected_sha256": {"type": "string"}, "edits": {"type": "array", "items": {"type": "object", "properties": {"old": {"type": "string"}, "new": {"type": "string"}}, "required": ["old", "new"]}}}),
                vec!["path", "expected_sha256", "edits"],
            ),
            Self::FsWrite => (
                "Write a whole file in the workspace. For a file that exists, expected_sha256 must be the hash from fs_read.",
                json!({"path": path, "content": {"type": "string"}, "expected_sha256": {"type": "string"}}),
                vec!["path", "content"],
            ),
            Self::FsMove => (
                "Move or rename a file or folder inside the workspace.",
                json!({"from": path, "to": path}),
                vec!["from", "to"],
            ),
            Self::FsRemove => (
                "Remove a file or a folder from the workspace.",
                json!({"path": path, "recursive": {"type": "boolean"}}),
                vec!["path"],
            ),
            Self::ProcessStart => (
                "Start a shell command in the workspace on the worker. Then call process_wait.",
                json!({"command": {"type": "string"}, "cwd": path, "timeout_seconds": {"type": "integer", "description": "The command ends after this time. The default is 1800."}}),
                vec!["command"],
            ),
            Self::ProcessRead => (
                "Read the new output of a process. Give the offset that the last read returned.",
                json!({"process_id": {"type": "string"}, "offset": {"type": "integer"}}),
                vec!["process_id"],
            ),
            Self::ProcessWriteStdin => (
                "Send text to the standard input of a running process.",
                json!({"process_id": {"type": "string"}, "text": {"type": "string"}}),
                vec!["process_id", "text"],
            ),
            Self::ProcessSignal => (
                "Stop a running process. signal is term or kill.",
                json!({"process_id": {"type": "string"}, "signal": {"type": "string", "enum": ["term", "kill"]}}),
                vec!["process_id", "signal"],
            ),
            Self::ProcessWait => (
                "Wait up to 30 seconds for a process to end. Returns the state, the new output and the exit code. Call it again while the state is running. A state of unknown means that nobody knows the result: do not start the command again without checking first.",
                json!({"process_id": {"type": "string"}, "timeout_seconds": {"type": "integer"}, "offset": {"type": "integer"}}),
                vec!["process_id"],
            ),
            Self::ProcessList => (
                "List the commands of this session on the worker, with their state, and the operations with no known result. Call it to check what ran after an interruption.",
                json!({}),
                vec![],
            ),
            Self::BlobPut => (
                "Upload a file of the workspace to the hub. Returns its sha256.",
                json!({"path": path}),
                vec!["path"],
            ),
            Self::BlobGet => (
                "Download a blob from the hub into a file of the workspace.",
                json!({"sha256": {"type": "string"}, "path": path}),
                vec!["sha256", "path"],
            ),
            Self::SkillPrepare => (
                "Put the script files of a skill on the worker. Returns the real path of the skill folder on the worker. Read the skill with read_skill first.",
                json!({"name": {"type": "string"}}),
                vec!["name"],
            ),
            Self::LocalMcpListTools => (
                "List the tools of the local MCP servers on the worker.",
                json!({"server": {"type": "string"}}),
                vec![],
            ),
            Self::LocalMcpCall => (
                "Call a tool of a local MCP server on the worker.",
                json!({"server": {"type": "string"}, "tool": {"type": "string"}, "arguments": {"type": "object"}}),
                vec!["server", "tool"],
            ),
        };
        let mut text = description.to_owned();
        if self.needs_operation_id() {
            text.push_str(OPERATION_ID);
            if let Value::Object(map) = &mut properties {
                map.insert("operation_id".to_owned(), json!({"type": "string"}));
            }
            required.push("operation_id");
        }
        ToolDefinition {
            name: self.name().to_owned(),
            description: text,
            input_schema: json!({"type": "object", "properties": properties, "required": required}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Frame, FrameId, MessageFamily};

    #[test]
    fn every_tool_has_a_unique_name_and_a_schema() {
        let mut names: Vec<_> = ExecTool::ALL.iter().map(|t| t.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ExecTool::ALL.len());
        for tool in ExecTool::ALL {
            let def = tool.definition();
            assert_eq!(ExecTool::from_name(&def.name), Some(tool));
            assert_eq!(def.input_schema["type"], "object");
            let required = def.input_schema["required"].as_array().unwrap();
            assert_eq!(
                tool.needs_operation_id(),
                required.iter().any(|r| r == "operation_id"),
                "{}",
                def.name
            );
        }
    }

    #[test]
    fn every_change_needs_an_operation_id() {
        for tool in ExecTool::ALL {
            if !matches!(tool.class(), ToolClass::Read) {
                assert!(tool.needs_operation_id(), "{}", tool.name());
            }
        }
    }

    #[test]
    fn the_environment_message_says_that_it_overrides_the_cli() {
        let text = WorkspaceInspection {
            capability: "cap".into(),
            worker: "bigboi".into(),
            os: Os::Windows,
            arch: "x86_64".into(),
            native_root: r"C:\Users\ann\w".into(),
            path_separator: "\\".into(),
            case_insensitive_paths: true,
            shell: "powershell.exe".into(),
            runtimes: vec!["git 2.47.0".into()],
            layout: "empty".into(),
        }
        .environment_message();
        assert!(text.contains("overrides"));
        assert!(text.contains(r"C:\Users\ann\w"));
        assert!(text.contains("Windows"));
        assert!(text.contains("powershell.exe"));
    }

    #[test]
    fn hub_requests_round_trip() {
        let op = ExecOp {
            session_id: SessionId::new(),
            capability: "cap".into(),
            operation_id: "op-1".into(),
            tool: "fs_read".into(),
            arguments: json!({"path": "a"}),
            deadline: Timestamp::from_millis(5),
            policy_version: 1,
        };
        let message: HubToExecution = op.into();
        let frame = Frame::family_request(FrameId::generate(), &message).unwrap();
        let Frame::Request(request) = Frame::parse(&frame.to_text().unwrap()).unwrap() else {
            panic!("expected a request");
        };
        assert_eq!(request.kind, message.type_name());
        assert_eq!(request.decode::<HubToExecution>().unwrap(), message);
    }
}
