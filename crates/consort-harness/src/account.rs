//! Harness accounts: the `[[accounts]]` entries of `worker.toml`, and the
//! per-account home folders that keep two logins of one harness apart.
//!
//! The worker never reads a harness's private login files. A shadow home
//! links every entry of the shared home except those files, so two accounts
//! share settings and history but each keeps its own login. The worker only
//! reads directory entry names, never file contents.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use consort_proto::ids::AccountId;
use serde::{Deserialize, Serialize};

use crate::{HarnessError, HarnessKind};

fn default_true() -> bool {
    true
}

/// One harness account on this worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountConfig {
    /// The account ID.
    pub id: AccountId,
    /// The harness.
    pub harness: HarnessKind,
    /// The label that the user sees, such as "Codex Personal".
    pub label: String,
    /// The harness program, when it is not the default on `PATH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    /// The harness's home folder, when it is not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// A folder that links to `home` for everything except the login files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_home: Option<String>,
    /// Extra arguments for the harness program.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables for the harness program.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Whether the worker offers this account to the hub.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl AccountConfig {
    /// A default account for a harness, labelled with the harness display
    /// name.
    pub fn default_for(kind: HarnessKind) -> Self {
        Self {
            id: AccountId::new(),
            harness: kind,
            label: kind.display_name().to_owned(),
            binary: None,
            home: None,
            shadow_home: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            enabled: true,
        }
    }
}

/// The environment variable that sets a harness's home folder.
pub fn home_variable(kind: HarnessKind) -> Option<&'static str> {
    match kind {
        HarnessKind::Codex => Some("CODEX_HOME"),
        HarnessKind::ClaudeCode => Some("CLAUDE_CONFIG_DIR"),
        // OpenCode keeps its data in `$XDG_DATA_HOME/opencode`.
        HarnessKind::OpenCode => Some("XDG_DATA_HOME"),
        HarnessKind::GeminiCli | HarnessKind::Pi | HarnessKind::Acp => None,
    }
}

/// The login files that a shadow home keeps private. The worker never reads
/// these files.
pub fn private_files(kind: HarnessKind) -> &'static [&'static str] {
    match kind {
        HarnessKind::Codex | HarnessKind::Pi => &["auth.json"],
        HarnessKind::ClaudeCode => &[".credentials.json"],
        HarnessKind::OpenCode => &["auth.json"],
        HarnessKind::GeminiCli | HarnessKind::Acp => &[],
    }
}

/// The user's home folder.
pub fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Expands a leading `~` to the user's home folder.
pub fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return user_home().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = user_home()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

/// The default home folder of a harness, used as the shared home of a
/// shadow home when the account does not set `home`.
pub fn default_home(kind: HarnessKind) -> Option<PathBuf> {
    let home = user_home()?;
    match kind {
        HarnessKind::Codex => Some(home.join(".codex")),
        HarnessKind::ClaudeCode => Some(home.join(".claude")),
        HarnessKind::OpenCode => Some(home.join(".local").join("share")),
        HarnessKind::GeminiCli | HarnessKind::Pi | HarnessKind::Acp => None,
    }
}

/// Prepares the account's home folder and returns the environment variables
/// that point the harness at it, plus the account's own `env`.
///
/// With a `shadow_home`, this creates the folder and links each entry of the
/// shared home into it, except the private login files.
pub fn prepare_home(account: &AccountConfig) -> Result<BTreeMap<String, String>, HarnessError> {
    let mut env = BTreeMap::new();
    let variable = home_variable(account.harness);
    if let Some(shadow) = &account.shadow_home {
        let Some(variable) = variable else {
            return Err(HarnessError::Unsupported(
                "use a shadow home with this harness",
            ));
        };
        let shadow = expand_home(shadow);
        let shared = account
            .home
            .as_deref()
            .map(expand_home)
            .or_else(|| default_home(account.harness));
        build_shadow_home(&shadow, shared.as_deref(), private_files(account.harness))?;
        env.insert(variable.to_owned(), shadow.to_string_lossy().into_owned());
    } else if let Some(home) = &account.home {
        let Some(variable) = variable else {
            return Err(HarnessError::Unsupported(
                "set a home folder for this harness",
            ));
        };
        env.insert(
            variable.to_owned(),
            expand_home(home).to_string_lossy().into_owned(),
        );
    }
    env.extend(account.env.clone());
    Ok(env)
}

/// Creates `shadow` and links each entry of `shared` into it, except the
/// names in `private`. Existing entries in `shadow` stay as they are, so a
/// login that the user made in the shadow home is kept.
pub fn build_shadow_home(
    shadow: &Path,
    shared: Option<&Path>,
    private: &[&str],
) -> Result<(), HarnessError> {
    std::fs::create_dir_all(shadow)?;
    let Some(shared) = shared else {
        return Ok(());
    };
    let Ok(entries) = std::fs::read_dir(shared) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if private.iter().any(|p| name == std::ffi::OsStr::new(p)) {
            continue;
        }
        let target = shadow.join(&name);
        if target.symlink_metadata().is_ok() {
            continue;
        }
        let source = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if let Err(err) = link(&source, &target, is_dir) {
            tracing::warn!(
                source = %source.display(),
                error = %err,
                "could not link a shared harness file into the shadow home"
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
fn link(source: &Path, target: &Path, _is_dir: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn link(source: &Path, target: &Path, is_dir: bool) -> std::io::Result<()> {
    // Symbolic links on Windows need Developer Mode. Without it, copy plain
    // files, so settings still reach the shadow home.
    let result = if is_dir {
        std::os::windows::fs::symlink_dir(source, target)
    } else {
        std::os::windows::fs::symlink_file(source, target)
    };
    match result {
        Ok(()) => Ok(()),
        Err(_) if !is_dir => std::fs::copy(source, target).map(|_| ()),
        Err(err) => Err(err),
    }
}

#[cfg(not(any(unix, windows)))]
fn link(_source: &Path, _target: &Path, _is_dir: bool) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "links are not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_home_skips_private_files() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(shared.join("sessions")).unwrap();
        std::fs::write(shared.join("config.toml"), "model = \"x\"").unwrap();
        std::fs::write(shared.join("auth.json"), "secret").unwrap();
        let shadow = dir.path().join("shadow");
        build_shadow_home(&shadow, Some(&shared), &["auth.json"]).unwrap();
        assert!(shadow.join("config.toml").exists());
        assert!(shadow.join("sessions").exists());
        assert!(shadow.join("auth.json").symlink_metadata().is_err());
        // A second run keeps what is there.
        build_shadow_home(&shadow, Some(&shared), &["auth.json"]).unwrap();
    }

    #[test]
    fn prepare_home_sets_the_variable() {
        let dir = tempfile::tempdir().unwrap();
        let mut account = AccountConfig::default_for(HarnessKind::Codex);
        account.home = Some(dir.path().to_string_lossy().into_owned());
        account.env.insert("X".into(), "1".into());
        let env = prepare_home(&account).unwrap();
        assert_eq!(env["CODEX_HOME"], dir.path().to_string_lossy());
        assert_eq!(env["X"], "1");

        let mut account = AccountConfig::default_for(HarnessKind::ClaudeCode);
        account.home = Some(dir.path().join("a").to_string_lossy().into_owned());
        account.shadow_home = Some(dir.path().join("b").to_string_lossy().into_owned());
        let env = prepare_home(&account).unwrap();
        assert_eq!(
            env["CLAUDE_CONFIG_DIR"],
            dir.path().join("b").to_string_lossy()
        );

        let mut account = AccountConfig::default_for(HarnessKind::GeminiCli);
        account.home = Some("/x".into());
        assert!(prepare_home(&account).is_err());
    }

    #[test]
    fn account_toml_shape() {
        let text = r#"
id = "acc_00000000-0000-0000-0000-000000000001"
harness = "codex"
label = "Codex Personal"
binary = "codex"
home = "~/.codex"
"#;
        let account: AccountConfig = toml_like(text);
        assert_eq!(account.harness, HarnessKind::Codex);
        assert!(account.enabled);
        assert!(account.args.is_empty());
    }

    fn toml_like(text: &str) -> AccountConfig {
        // The harness crate has no TOML dependency. Parse the same keys as
        // JSON to check the serde shape.
        let mut map = serde_json::Map::new();
        for line in text.lines().filter(|l| l.contains('=')) {
            let (k, v) = line.split_once('=').unwrap();
            map.insert(
                k.trim().to_owned(),
                serde_json::Value::String(v.trim().trim_matches('"').to_owned()),
            );
        }
        serde_json::from_value(serde_json::Value::Object(map)).unwrap()
    }

    #[test]
    fn expand_tilde() {
        if let Some(home) = user_home() {
            assert_eq!(expand_home("~/.codex"), home.join(".codex"));
        }
        assert_eq!(expand_home("/abs"), PathBuf::from("/abs"));
    }
}
