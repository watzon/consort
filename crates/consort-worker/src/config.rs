//! The files of the worker: `worker.toml`, the key, `worker.db` and
//! `workspaces/`.

use std::path::{Path, PathBuf};

use consort_proto::ids::WorkerId;
use consort_proto::model::{ApprovalMode, CheckoutInfo, LocalMcpServerInfo, WorkerPolicy};
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
/// The name of the folder with the output of each process.
pub const PROCESSES_DIR: &str = "processes";
/// The name of the folder with the blob and skill caches.
pub const CACHE_DIR: &str = "cache";
/// The name of the lock file. A running worker holds an exclusive lock on
/// it, so a second worker cannot use the same folder.
pub const LOCK_FILE: &str = "worker.lock";

/// Creates a folder that only this user can read.
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// A path with `~` read as the user's home folder.
pub fn expand_home(path: &str) -> PathBuf {
    let home = || directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    if path == "~" {
        return home().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = home()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

fn default_true() -> bool {
    true
}

/// A new worker allows Auto. Full access is a choice that the user makes in
/// the app.
fn default_max_mode() -> ApprovalMode {
    ApprovalMode::Auto
}

/// The limits of this worker. They are in `worker.db`, and they change only
/// through the app (see [`crate::limits`]).
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
    /// The SHA-256 of the certificate of a hub on This network, set by
    /// pairing. The worker accepts this certificate and no other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_sha256: Option<consort_proto::model::Sha256Hash>,
    /// The worker ID, set by pairing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<WorkerId>,
    /// The display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tags for placement, added to the detected tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Local MCP servers.
    #[serde(default)]
    pub local_mcp_servers: Vec<LocalMcpServerConfig>,
    /// Repo checkouts.
    #[serde(default)]
    pub checkouts: Vec<CheckoutConfig>,
}

impl WorkerToml {
    /// The path of the file in a config folder.
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join(CONFIG_FILE)
    }

    /// Reads the file. A missing file gives the defaults.
    pub fn load(config_dir: &Path) -> Result<Self, WorkerError> {
        let path = Self::path(config_dir);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| WorkerError::Config(e.to_string())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(WorkerError::io(
                format!("could not read {}", path.display()),
                err,
            )),
        }
    }

    /// Writes the file.
    pub fn save(&self, config_dir: &Path) -> Result<(), WorkerError> {
        let text = toml::to_string_pretty(self).map_err(|e| WorkerError::Config(e.to_string()))?;
        let path = Self::path(config_dir);
        std::fs::write(&path, text)
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

    /// The local MCP servers as the inventory shows them.
    pub fn local_server_infos(&self) -> Vec<LocalMcpServerInfo> {
        self.local_mcp_servers
            .iter()
            .map(|s| LocalMcpServerInfo {
                name: s.name.clone(),
                description: s.description.clone(),
                tools: Vec::new(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults_and_a_saved_file_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let empty = WorkerToml::load(dir.path()).unwrap();
        assert_eq!(empty, WorkerToml::default());
        let mut toml = empty;
        toml.name = Some("bigboi".to_owned());
        toml.tags.push("gpu".to_owned());
        toml.save(dir.path()).unwrap();
        assert_eq!(WorkerToml::load(dir.path()).unwrap(), toml);
    }
}
