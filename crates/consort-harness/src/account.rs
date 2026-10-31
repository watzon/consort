//! Harness accounts: the `[[accounts]]` entries of `session-host.toml`, and the
//! per-account home folders that keep two logins of one harness apart.
//!
//! The worker never reads a harness's private login files. A shadow home
//! links every entry of the shared home except those files, so two accounts
//! share settings and history but each keeps its own login. The worker only
//! reads directory entry names, never file contents.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use consort_proto::ids::AccountId;
use consort_proto::model::AcpAccountSpec;
use consort_tools::ToolId;
use serde::{Deserialize, Serialize};

use crate::{HarnessError, HarnessKind, Launch};

fn default_true() -> bool {
    true
}

/// One harness account on this worker.
///
/// `Debug` shows the names of the environment variables, never their
/// values, because `env` and the runtime environment can hold secrets.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountConfig {
    /// The account ID.
    pub id: AccountId,
    /// The harness.
    pub harness: HarnessKind,
    /// The label that the user sees, such as "Codex Personal".
    pub label: String,
    /// The harness program, when it is not the managed tool. An account with
    /// a `binary` always runs that program.
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
    /// True for an account that Consort made, with a profile folder in
    /// `<consort home>/profiles/<account id>/`. The worker derives its home
    /// folders from the profile folder.
    #[serde(default, skip_serializing_if = "is_false")]
    pub profile: bool,
    /// The name of the profile folder in `<consort home>/profiles/`, chosen
    /// from the label when the account was made, such as `claude-work`. A
    /// later rename does not move the folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_dir: Option<String>,
    /// The ACP agent of an `acp` account that Consort made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp: Option<AcpAccountSpec>,
    /// What the worker adds at run time. Never written to `session-host.toml`.
    #[serde(skip)]
    pub runtime: AccountRuntime,
}

/// What the worker adds to an account before it gives it to an adapter:
/// the managed tool launch, the profile variables and the secret variables.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AccountRuntime {
    /// How to start the managed tool of the account. An adapter uses it when
    /// the account has no `binary`.
    pub launch: Option<Launch>,
    /// The profile variables, then the secret variables from `env.json`.
    /// They go after the account's own `env`.
    pub env: BTreeMap<String, String>,
    /// Variables to remove from the worker's environment, so a profile
    /// account does not pick up a login that the machine already has.
    pub env_remove: Vec<String>,
    /// The OS boundary that sessions of this account run in. The session
    /// host sets it. `None` only for a harness that needs none.
    pub boundary: Option<crate::boundary::BoundaryConfig>,
}

impl std::fmt::Debug for AccountRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountRuntime")
            .field("launch", &self.launch.as_ref().map(|l| &l.program))
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("env_remove", &self.env_remove)
            .field("boundary", &self.boundary)
            .finish()
    }
}

impl std::fmt::Debug for AccountConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountConfig")
            .field("id", &self.id)
            .field("harness", &self.harness)
            .field("label", &self.label)
            .field("binary", &self.binary)
            .field("home", &self.home)
            .field("shadow_home", &self.shadow_home)
            .field("args", &self.args)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("enabled", &self.enabled)
            .field("profile", &self.profile)
            .field("profile_dir", &self.profile_dir)
            .field("acp", &self.acp)
            .field("runtime", &self.runtime)
            .finish()
    }
}

fn is_false(value: &bool) -> bool {
    !*value
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
            profile: false,
            profile_dir: None,
            acp: None,
            runtime: AccountRuntime::default(),
        }
    }

    /// The managed tool that runs this account, or `None` when the account
    /// names its own `binary` or has no managed tool (Pi, a custom ACP
    /// command).
    pub fn managed_tool(&self) -> Option<ToolId> {
        if self.binary.is_some() {
            return None;
        }
        match self.harness {
            HarnessKind::ClaudeCode => Some(ToolId::ClaudeAgentAcp),
            HarnessKind::Codex => Some(ToolId::Codex),
            HarnessKind::OpenCode => Some(ToolId::OpenCode),
            HarnessKind::GeminiCli => Some(ToolId::GeminiCli),
            HarnessKind::Acp => match &self.acp {
                Some(AcpAccountSpec::Registry { id }) => Some(ToolId::Acp(id.clone())),
                _ => None,
            },
            HarnessKind::Pi => None,
        }
    }

    /// The program of the managed launch, when the account uses one.
    pub fn managed_launch(&self) -> Option<&Launch> {
        if self.binary.is_some() {
            return None;
        }
        self.runtime.launch.as_ref()
    }
}

/// The variables that hold a login or an API key in the environment. The
/// worker removes them from the environment of a profile account, so the
/// account uses only its own login and its own `env.json`.
pub const LOGIN_VARIABLES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CONFIG_DIR",
    "OPENAI_API_KEY",
    "CODEX_API_KEY",
    "CODEX_HOME",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GEMINI_CLI_HOME",
    "OPENROUTER_API_KEY",
    "FIREWORKS_API_KEY",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
];

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

/// The login files that a shadow home keeps private, as paths relative to
/// the home with `/` between parts. The worker never reads these files.
pub fn private_files(kind: HarnessKind) -> &'static [&'static str] {
    match kind {
        HarnessKind::Codex | HarnessKind::Pi => &["auth.json"],
        HarnessKind::ClaudeCode => &[".credentials.json"],
        // The home is `$XDG_DATA_HOME`, and OpenCode keeps its login in the
        // `opencode` folder inside it.
        HarnessKind::OpenCode => &["opencode/auth.json"],
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
    env.extend(account.runtime.env.clone());
    Ok(env)
}

/// Creates `shadow` and links each entry of `shared` into it, except the
/// paths in `private`. Existing entries in `shadow` stay as they are, so a
/// login that the user made in the shadow home is kept.
///
/// A private path can be inside a folder, such as `opencode/auth.json`. That
/// folder is then a real folder in `shadow`, and each of its entries except
/// the private file is linked. An older link to the whole folder is replaced
/// by a real folder; only the link is removed, never the shared folder.
pub fn build_shadow_home(
    shadow: &Path,
    shared: Option<&Path>,
    private: &[&str],
) -> Result<(), HarnessError> {
    std::fs::create_dir_all(shadow)?;
    let Some(shared) = shared else {
        return Ok(());
    };
    let private: Vec<Vec<&str>> = private
        .iter()
        .map(|p| p.split('/').filter(|part| !part.is_empty()).collect())
        .filter(|parts: &Vec<&str>| !parts.is_empty())
        .collect();
    shadow_folder(shadow, shared, &private)
}

/// Links the entries of one folder of the shared home. See
/// [`build_shadow_home`].
fn shadow_folder(shadow: &Path, shared: &Path, private: &[Vec<&str>]) -> Result<(), HarnessError> {
    let Ok(entries) = std::fs::read_dir(shared) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let here = |part: &str| name == std::ffi::OsStr::new(part);
        if private.iter().any(|p| p.len() == 1 && here(p[0])) {
            continue;
        }
        let inner: Vec<Vec<&str>> = private
            .iter()
            .filter(|p| p.len() > 1 && here(p[0]))
            .map(|p| p[1..].to_vec())
            .collect();
        let target = shadow.join(&name);
        let source = entry.path();
        if !inner.is_empty() && source.is_dir() {
            let is_link = target
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink());
            if is_link {
                remove_link(&target)?;
            }
            std::fs::create_dir_all(&target)?;
            shadow_folder(&target, &source, &inner)?;
            continue;
        }
        if target.symlink_metadata().is_ok() {
            continue;
        }
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

/// Removes a link, not what it points to.
#[cfg(windows)]
fn remove_link(path: &Path) -> std::io::Result<()> {
    // A folder link on Windows is a folder entry, so remove it as one.
    std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path))
}

/// Removes a link, not what it points to.
#[cfg(not(windows))]
fn remove_link(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
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
    fn opencode_shadow_home_keeps_the_nested_login_private() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("share");
        std::fs::create_dir_all(shared.join("opencode").join("storage")).unwrap();
        std::fs::create_dir_all(shared.join("other-app")).unwrap();
        // A stand-in login file. The test only checks that it is not linked.
        std::fs::write(shared.join("opencode").join("auth.json"), "x").unwrap();
        std::fs::write(shared.join("opencode").join("log.txt"), "y").unwrap();
        let shadow = dir.path().join("shadow");
        // An older build linked the whole `opencode` folder.
        std::fs::create_dir_all(&shadow).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(shared.join("opencode"), shadow.join("opencode")).unwrap();

        build_shadow_home(&shadow, Some(&shared), private_files(HarnessKind::OpenCode)).unwrap();
        let opencode = shadow.join("opencode");
        let meta = opencode.symlink_metadata().unwrap();
        assert!(meta.is_dir() && !meta.file_type().is_symlink());
        assert!(opencode.join("storage").exists());
        assert!(opencode.join("log.txt").exists());
        assert!(
            opencode.join("auth.json").symlink_metadata().is_err(),
            "the OpenCode login is not shared"
        );
        assert!(shadow.join("other-app").exists());
        assert!(
            shared.join("opencode").join("auth.json").exists(),
            "the shared login stays"
        );
        // A second run keeps the real folder.
        build_shadow_home(&shadow, Some(&shared), private_files(HarnessKind::OpenCode)).unwrap();
        assert!(opencode.join("auth.json").symlink_metadata().is_err());
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
    fn debug_shows_variable_names_only() {
        let mut account = AccountConfig::default_for(HarnessKind::ClaudeCode);
        account.env.insert("HAND_SECRET".into(), "hunter2".into());
        account
            .runtime
            .env
            .insert("ANTHROPIC_API_KEY".into(), "sk-ant-secret".into());
        let debug = format!("{account:?}");
        assert!(debug.contains("HAND_SECRET") && debug.contains("ANTHROPIC_API_KEY"));
        assert!(
            !debug.contains("hunter2") && !debug.contains("sk-ant-secret"),
            "{debug}"
        );
    }

    #[test]
    fn the_runtime_env_goes_after_the_account_env() {
        let mut account = AccountConfig::default_for(HarnessKind::ClaudeCode);
        account.env.insert("A".into(), "hand".into());
        account.runtime.env.insert("A".into(), "profile".into());
        account.runtime.env.insert("B".into(), "secret".into());
        let env = prepare_home(&account).unwrap();
        assert_eq!(env["A"], "profile");
        assert_eq!(env["B"], "secret");
    }

    #[test]
    fn expand_tilde() {
        if let Some(home) = user_home() {
            assert_eq!(expand_home("~/.codex"), home.join(".codex"));
        }
        assert_eq!(expand_home("/abs"), PathBuf::from("/abs"));
    }
}
