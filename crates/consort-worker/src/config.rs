//! `worker.toml` and the files in the worker's configuration folder.
//!
//! The folder holds `worker.toml`, the key file `worker.key`, the database
//! `worker.db`, the `workspaces/` folder and the `runtime/` folder. The
//! format of `worker.toml` is in `docs/data-model.md` ("Worker-local
//! state"). On Unix the worker makes these folders with mode 0700 (see
//! [`create_private_dir`]).

use std::path::{Path, PathBuf};

use consort_harness::{AccountConfig, HarnessKind, process::resolve_program};
use consort_proto::ids::WorkerId;
use consort_proto::model::{ApprovalMode, CheckoutInfo, WorkerPolicy};
use serde::{Deserialize, Serialize};

use crate::WorkerError;

/// The name of the configuration file.
pub const CONFIG_FILE: &str = "worker.toml";
/// The name of the key file.
pub const KEY_FILE: &str = "worker.key";
/// The name of the database.
pub const DB_FILE: &str = "worker.db";
/// The name of the workspaces folder.
pub const WORKSPACES_DIR: &str = "workspaces";
/// The name of the folder with one runtime folder per session.
pub const RUNTIME_DIR: &str = "runtime";
/// The name of the lock file. A running worker holds an exclusive lock on
/// it, so a second worker cannot use the same folder.
pub const LOCK_FILE: &str = "worker.lock";

/// Creates a folder and its parents. On Unix the folder gets mode 0700,
/// also when it exists already, so only the user can read what the worker
/// keeps in it. Parents that exist keep their mode.
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
    }
}

/// Why a local MCP server name cannot be used, or `None` when it can.
///
/// The name `hub` (in any case) is kept for the hub tools. A name with `__`
/// could look like a hub tool: `hub__x` gives the tool `mcp__hub__x__tool`,
/// which reads as the `hub` server with the tool `x__tool`.
pub fn local_server_name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        Some("the name is empty")
    } else if name.eq_ignore_ascii_case("hub") {
        Some("Consort keeps the name `hub` for the hub tools")
    } else if name.contains("__") {
        Some("the name has `__`, which harnesses use between the server and the tool name")
    } else {
        None
    }
}

fn default_true() -> bool {
    true
}

/// The `[policy]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// Folders that sessions may use. Empty means any folder.
    #[serde(default)]
    pub allowed_roots: Vec<String>,
    /// The highest approval mode that the hub may ask for.
    #[serde(default = "default_max_mode")]
    pub max_approval_mode: ApprovalMode,
    /// Whether sessions may run shell commands.
    #[serde(default = "default_true")]
    pub allow_shell: bool,
}

/// A new worker allows Auto. Full access is a choice that the user makes
/// in `worker.toml`.
fn default_max_mode() -> ApprovalMode {
    ApprovalMode::Auto
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            allowed_roots: Vec::new(),
            max_approval_mode: default_max_mode(),
            allow_shell: true,
        }
    }
}

impl PolicyConfig {
    /// The policy as the inventory shows it.
    pub fn to_proto(&self) -> WorkerPolicy {
        WorkerPolicy {
            max_approval_mode: self.max_approval_mode,
            allowed_roots: self.allowed_roots.clone(),
            allow_shell: self.allow_shell,
        }
    }
}

/// One `[[local_mcp_servers]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalMcpServerConfig {
    /// The name.
    pub name: String,
    /// `http` (the only transport in this build).
    #[serde(default = "default_transport")]
    pub transport: String,
    /// The URL.
    pub url: String,
    /// A description for the assistant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn default_transport() -> String {
    "http".to_owned()
}

/// One `[[checkouts]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutConfig {
    /// The git remote URL.
    pub repo_url: String,
    /// The folder.
    pub path: String,
}

/// `worker.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerToml {
    /// The hub URL, set by pairing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_url: Option<String>,
    /// The worker ID, set by pairing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<WorkerId>,
    /// The display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tags for placement, added to the detected tags. Older files call
    /// this `labels`.
    #[serde(default, alias = "labels")]
    pub tags: Vec<String>,
    /// Add an account for each installed harness that has none.
    #[serde(default = "default_true")]
    pub auto_discover: bool,
    /// The worker policy.
    #[serde(default)]
    pub policy: PolicyConfig,
    /// The harness accounts.
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
    /// Local MCP servers.
    #[serde(default)]
    pub local_mcp_servers: Vec<LocalMcpServerConfig>,
    /// Repo checkouts.
    #[serde(default)]
    pub checkouts: Vec<CheckoutConfig>,
}

impl WorkerToml {
    /// The path of `worker.toml` in a folder.
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join(CONFIG_FILE)
    }

    /// Reads `worker.toml`, or returns the defaults when it does not exist.
    pub fn load(config_dir: &Path) -> Result<Self, WorkerError> {
        let path = Self::path(config_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| WorkerError::Config(e.to_string())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                auto_discover: true,
                ..Self::default()
            }),
            Err(err) => Err(WorkerError::io(
                format!("could not read {}", path.display()),
                err,
            )),
        }
    }

    /// Writes `worker.toml` through a temporary file, so a crash never
    /// leaves half a file.
    pub fn save(&self, config_dir: &Path) -> Result<(), WorkerError> {
        create_private_dir(config_dir).map_err(|e| {
            WorkerError::io(format!("could not create {}", config_dir.display()), e)
        })?;
        let text = toml::to_string_pretty(self).map_err(|e| WorkerError::Config(e.to_string()))?;
        let path = Self::path(config_dir);
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)
            .map_err(|e| WorkerError::io(format!("could not write {}", tmp.display()), e))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| WorkerError::io(format!("could not write {}", path.display()), e))
    }

    /// The checkouts as the inventory shows them.
    pub fn checkout_infos(&self) -> Vec<CheckoutInfo> {
        self.checkouts
            .iter()
            .map(|c| CheckoutInfo {
                repo_url: c.repo_url.clone(),
                path: c.path.clone(),
                managed: false,
            })
            .collect()
    }

    /// Adds a default account for each installed harness that has no
    /// account yet. Returns whether anything changed.
    pub fn discover_accounts(&mut self, installed: &dyn Fn(HarnessKind) -> bool) -> bool {
        if !self.auto_discover {
            return false;
        }
        let mut changed = false;
        for kind in HarnessKind::ALL {
            if kind == HarnessKind::Acp {
                continue;
            }
            if self.accounts.iter().any(|a| a.harness == kind) {
                continue;
            }
            if installed(kind) {
                self.accounts.push(AccountConfig::default_for(kind));
                changed = true;
            }
        }
        changed
    }
}

/// Whether a harness looks installed on this machine.
pub fn harness_installed(kind: HarnessKind) -> bool {
    match kind {
        // `claude-agent-acp` runs through npx or bunx.
        HarnessKind::ClaudeCode => {
            resolve_program("npx").is_some() || resolve_program("bunx").is_some()
        }
        HarnessKind::Codex => resolve_program("codex").is_some(),
        HarnessKind::OpenCode => resolve_program("opencode").is_some(),
        HarnessKind::GeminiCli => resolve_program("gemini").is_some(),
        HarnessKind::Pi => resolve_program("pi").is_some(),
        HarnessKind::Acp => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_model_example_parses() {
        let text = r#"
hub_url = "https://hub.example.com"
name = "bigboi"
tags = ["windows-test", "gpu"]

[policy]
allowed_roots = ["C:/Users/chris/Code"]
max_approval_mode = "auto_edits"
allow_shell = true

[[accounts]]
id = "acc_0192f0c4-7a1e-7c3d-9b4a-2f1e0d9c8b7a"
harness = "codex"
label = "Codex Personal"
binary = "codex"
home = "~/.codex"
shadow_home = "~/.local/share/app/homes/codex-personal"
args = []
env = {}
enabled = true

[[local_mcp_servers]]
name = "home-assistant"
transport = "http"
url = "http://192.168.1.20:8123/mcp"

[[checkouts]]
repo_url = "git@github.com:watzon/sayso.git"
path = "C:/Users/chris/Code/sayso"
"#;
        let config: WorkerToml = toml::from_str(text).unwrap();
        assert_eq!(config.policy.max_approval_mode, ApprovalMode::AutoEdits);
        assert_eq!(config.accounts[0].harness, HarnessKind::Codex);
        assert_eq!(config.accounts[0].label, "Codex Personal");
        assert_eq!(config.tags, ["windows-test", "gpu"]);
        assert!(config.auto_discover);
        // A file from before the rename still loads its labels as tags.
        let old: WorkerToml = toml::from_str(r#"labels = ["gpu"]"#).unwrap();
        assert_eq!(old.tags, ["gpu"]);
        assert_eq!(config.local_mcp_servers[0].name, "home-assistant");
        assert_eq!(config.checkout_infos()[0].path, "C:/Users/chris/Code/sayso");
    }

    #[test]
    fn local_server_names_that_look_like_hub_tools_are_refused() {
        assert!(local_server_name_problem("home-assistant").is_none());
        assert!(local_server_name_problem("github_tools").is_none());
        for bad in ["hub", "HUB", "Hub", "hub__x", "a__b", " "] {
            assert!(local_server_name_problem(bad).is_some(), "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn private_folders_have_mode_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("a").join("worker");
        create_private_dir(&folder).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&folder), 0o700);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        WorkerToml::default().save(&folder).unwrap();
        assert_eq!(mode(&folder), 0o700, "an existing folder becomes private");
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = WorkerToml::load(dir.path()).unwrap();
        assert!(config.auto_discover);
        config.hub_url = Some("http://127.0.0.1:7600".into());
        config.worker_id = Some(WorkerId::new());
        assert!(config.discover_accounts(&|k| k == HarnessKind::Codex));
        assert!(!config.discover_accounts(&|k| k == HarnessKind::Codex));
        config.save(dir.path()).unwrap();
        let back = WorkerToml::load(dir.path()).unwrap();
        assert_eq!(back, config);
        assert_eq!(back.accounts[0].label, "Codex");
    }
}
