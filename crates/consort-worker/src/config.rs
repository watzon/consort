//! `worker.toml` and the files in the worker's configuration folder.
//!
//! The folder holds `worker.toml`, the key file `worker.key`, the database
//! `worker.db` and the `workspaces/` folder. The format of `worker.toml` is
//! in `docs/data-model.md` ("Worker-local state").

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
    /// Labels for placement.
    #[serde(default)]
    pub labels: Vec<String>,
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
        std::fs::create_dir_all(config_dir).map_err(|e| {
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
labels = ["windows-test", "gpu"]

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
        assert!(config.auto_discover);
        assert_eq!(config.local_mcp_servers[0].name, "home-assistant");
        assert_eq!(config.checkout_infos()[0].path, "C:/Users/chris/Code/sayso");
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
