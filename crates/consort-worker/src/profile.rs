//! The Consort home and the profile folders of accounts.
//!
//! The Consort home is `~/.consort` (`%USERPROFILE%\.consort` on Windows),
//! or the folder in `CONSORT_HOME`. It holds `profiles/<account id>/` for
//! each account that Consort made, and `tools/` with the managed tools.
//!
//! A profile folder holds the harness's own config and login, and
//! `env.json` with the account's secret variables (mode 0600 on Unix). The
//! worker derives each harness's home settings from the profile folder (see
//! `docs/harness-adapters.md`, "Profile folders"). It never reads the login
//! files that the harness writes there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use consort_harness::HarnessKind;
use consort_proto::ids::AccountId;

use crate::WorkerError;
use crate::config::create_private_dir;

/// The variable that sets the Consort home.
pub const HOME_VAR: &str = "CONSORT_HOME";
/// The folder of the profile folders, in the Consort home.
pub const PROFILES_DIR: &str = "profiles";
/// The folder of the managed tools, in the Consort home.
pub const TOOLS_DIR: &str = "tools";
/// The file with the secret variables, in a profile folder.
pub const ENV_FILE: &str = "env.json";

/// The Consort home: `CONSORT_HOME` when it is set, else `.consort` in the
/// user's home folder.
pub fn consort_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os(HOME_VAR).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(home));
    }
    directories::BaseDirs::new().map(|d| d.home_dir().join(".consort"))
}

/// The most characters in a profile folder name.
pub const SLUG_MAX_CHARS: usize = 48;

/// The profile folder name for an account label: the label in lowercase,
/// each run of characters other than `a` to `z` and `0` to `9` as one `-`,
/// no `-` at the start or the end, at most [`SLUG_MAX_CHARS`] characters.
/// An empty result is `account`. The desktop app shows the same name before
/// the account exists, so the two rules must stay the same.
pub fn profile_slug(label: &str) -> String {
    let mut slug = String::new();
    for c in label.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug
        .trim_matches('-')
        .chars()
        .take(SLUG_MAX_CHARS)
        .collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "account".to_owned()
    } else {
        slug.to_owned()
    }
}

/// Whether a name can be a profile folder name: what [`profile_slug`]
/// makes, with an optional `-<n>` suffix. A name from a hand-edited
/// `worker.toml` such as `../x` is refused.
pub fn valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= SLUG_MAX_CHARS + 8
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The profile folder of an account: `profiles/<profile_dir>`, or
/// `profiles/<account id>` for an account without a valid `profile_dir`.
pub fn profile_dir(consort_home: &Path, account: AccountId, name: Option<&str>) -> PathBuf {
    let profiles = consort_home.join(PROFILES_DIR);
    match name {
        Some(name) if valid_profile_name(name) => profiles.join(name),
        _ => profiles.join(account.to_string()),
    }
}

/// A free profile folder name for a new account: the slug of the label,
/// or the slug with `-2`, `-3` and so on when a folder with that name
/// exists or another account uses it.
pub fn free_profile_name(consort_home: &Path, label: &str, taken: &[String]) -> String {
    let base = profile_slug(label);
    let profiles = consort_home.join(PROFILES_DIR);
    let free = |name: &str| !taken.iter().any(|t| t == name) && !profiles.join(name).exists();
    if free(&base) {
        return base;
    }
    (2u32..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| free(name))
        .unwrap_or(base)
}

/// The home settings of a harness, derived from the profile folder.
pub fn profile_env(kind: HarnessKind, dir: &Path) -> BTreeMap<String, String> {
    let text = |p: PathBuf| p.to_string_lossy().into_owned();
    let mut env = BTreeMap::new();
    match kind {
        HarnessKind::ClaudeCode => {
            env.insert("CLAUDE_CONFIG_DIR".to_owned(), text(dir.join("claude")));
        }
        HarnessKind::Codex => {
            env.insert("CODEX_HOME".to_owned(), text(dir.join("codex")));
        }
        HarnessKind::GeminiCli => {
            env.insert("GEMINI_CLI_HOME".to_owned(), text(dir.join("gemini")));
        }
        HarnessKind::OpenCode => {
            // OpenCode reads `~/.claude/skills`, `~/.agents/skills` and
            // `~/.claude/CLAUDE.md` from the home folder. A home folder
            // inside the profile keeps them out. `tool_path_pins` keeps the
            // tools of the user where they are.
            let home = text(dir.join(PROFILE_HOME_DIR));
            #[cfg(windows)]
            env.insert("USERPROFILE".to_owned(), home.clone());
            env.insert("HOME".to_owned(), home);
            let xdg = dir.join("xdg");
            for (key, sub) in [
                ("XDG_DATA_HOME", "data"),
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_STATE_HOME", "state"),
                ("XDG_CACHE_HOME", "cache"),
            ] {
                env.insert(key.to_owned(), text(xdg.join(sub)));
            }
        }
        // An ACP agent keeps `HOME`. Only the env is set.
        HarnessKind::Pi | HarnessKind::Acp => {}
    }
    env
}

/// The folder in a profile that is the `HOME` of an OpenCode account. It
/// holds nothing that Consort reads.
pub const PROFILE_HOME_DIR: &str = "home";

/// The variables that keep the user's own tools where they are, when an
/// account runs with another `HOME`. Each entry is a variable, the path in
/// the real home that it names, and whether the path is a file. A variable
/// is set only when the path exists and the variable is not set already, so
/// a tool that the user does not have gets no variable.
const TOOL_PATHS: &[(&str, &str, bool)] = &[
    // git reads `~/.gitconfig`.
    ("GIT_CONFIG_GLOBAL", ".gitconfig", true),
    // gpg signs commits with the keys in `~/.gnupg`.
    ("GNUPGHOME", ".gnupg", false),
    ("CARGO_HOME", ".cargo", false),
    ("RUSTUP_HOME", ".rustup", false),
    ("BUN_INSTALL", ".bun", false),
    ("npm_config_cache", ".npm", false),
];

/// The environment variables that point the tools in `real_home` at their
/// real places. `is_set` tells whether a variable has a value already, in
/// the worker's environment, so the worker never overrides the user.
pub fn tool_path_pins(real_home: &Path, is_set: &dyn Fn(&str) -> bool) -> BTreeMap<String, String> {
    TOOL_PATHS
        .iter()
        .filter_map(|(name, rel, is_file)| {
            let path = real_home.join(rel);
            let exists = if *is_file {
                path.is_file()
            } else {
                path.is_dir()
            };
            (exists && !is_set(name))
                .then(|| ((*name).to_owned(), path.to_string_lossy().into_owned()))
        })
        .collect()
}

/// The top-level `config.toml` line that keeps the Codex login in a file in
/// `CODEX_HOME`, not in the system keychain, so each profile has its own.
pub const CODEX_FILE_STORE_LINE: &str = "cli_auth_credentials_store = \"file\"";

/// Sets a top-level key of a TOML text to `line`. The line goes before the
/// first table header. An existing top-level line for the key is replaced.
pub fn set_top_level_toml(text: &str, key: &str, line: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut top = true;
    let mut done = false;
    for current in text.lines() {
        let trimmed = current.trim_start();
        if trimmed.starts_with('[') {
            top = false;
        }
        let is_key = trimmed
            .strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='));
        if top && is_key {
            if !done {
                out.push(line.to_owned());
                done = true;
            }
            continue;
        }
        out.push(current.to_owned());
    }
    if !done {
        out.insert(0, line.to_owned());
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Writes a file through a temporary file and a rename, so a crash never
/// leaves half a file. With `private`, the file has mode 0600 on Unix from
/// the start.
fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::now_v7().simple()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let result = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Makes the profile folder and the harness's folders and settings in it:
/// the Codex `config.toml` lines, the Gemini `settings.json`, and the
/// OpenCode XDG folders. Settings that the user changed stay. Safe to run
/// again.
pub fn prepare_profile(kind: HarnessKind, dir: &Path) -> Result<(), WorkerError> {
    let io = |what: &Path| {
        let what = what.display().to_string();
        move |e| WorkerError::io(format!("could not prepare {what}"), e)
    };
    create_private_dir(dir).map_err(io(dir))?;
    for folder in profile_env(kind, dir).values() {
        std::fs::create_dir_all(folder).map_err(io(Path::new(folder)))?;
    }
    match kind {
        HarnessKind::Codex => {
            let path = dir.join("codex").join("config.toml");
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let text =
                set_top_level_toml(&text, "cli_auth_credentials_store", CODEX_FILE_STORE_LINE);
            let text = consort_tools::config::codex_disable_update_check(&text);
            write_atomic(&path, text.as_bytes(), false).map_err(io(&path))?;
        }
        HarnessKind::GeminiCli => {
            // Gemini CLI keeps its settings in `$GEMINI_CLI_HOME/.gemini`.
            let folder = dir.join("gemini").join(".gemini");
            std::fs::create_dir_all(&folder).map_err(io(&folder))?;
            let path = folder.join("settings.json");
            let mut value = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(serde_json::Value::Null);
            consort_tools::config::gemini_disable_auto_update(&mut value);
            let text = serde_json::to_vec_pretty(&value).map_err(|e| {
                WorkerError::Config(format!("could not write {}: {e}", path.display()))
            })?;
            write_atomic(&path, &text, false).map_err(io(&path))?;
        }
        _ => {}
    }
    Ok(())
}

/// Reads the secret variables of a profile folder. A missing file is an
/// empty set.
pub fn read_secrets(dir: &Path) -> Result<BTreeMap<String, String>, WorkerError> {
    let path = dir.join(ENV_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            // The error of serde_json names a position, never a value.
            WorkerError::Config(format!("{} is not valid: {e}", path.display()))
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(err) => Err(WorkerError::io(
            format!("could not read {}", path.display()),
            err,
        )),
    }
}

/// Writes the secret variables of a profile folder to `env.json` with mode
/// 0600 on Unix. An empty set removes the file.
pub fn write_secrets(dir: &Path, secrets: &BTreeMap<String, String>) -> Result<(), WorkerError> {
    let path = dir.join(ENV_FILE);
    if secrets.is_empty() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(WorkerError::io(
                format!("could not remove {}", path.display()),
                err,
            )),
        };
    }
    create_private_dir(dir)
        .map_err(|e| WorkerError::io(format!("could not create {}", dir.display()), e))?;
    let bytes = serde_json::to_vec_pretty(secrets)
        .map_err(|_| WorkerError::Config("could not encode the secret variables".to_owned()))?;
    write_atomic(&path, &bytes, true)
        .map_err(|e| WorkerError::io(format!("could not write {}", path.display()), e))
}

/// Checks the name of a secret variable: ASCII letters, digits and `_`, not
/// starting with a digit.
pub fn valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Deletes a profile folder. A missing folder is not an error.
pub fn remove_profile(dir: &Path) -> Result<(), WorkerError> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(WorkerError::io(
            format!("could not delete {}", dir.display()),
            err,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_folder_names_follow_the_label() {
        assert_eq!(profile_slug("Claude Work"), "claude-work");
        assert_eq!(profile_slug("  Ünïcode & co  "), "n-code-co");
        assert_eq!(profile_slug("!!!"), "account");
        assert_eq!(profile_slug(""), "account");
        assert_eq!(profile_slug("Codex--Personal 2"), "codex-personal-2");
        let long = profile_slug(&"ab ".repeat(40));
        assert!(
            long.len() <= SLUG_MAX_CHARS && !long.ends_with('-'),
            "{long}"
        );
        assert!(valid_profile_name("claude-work-2"));
        for bad in ["", "../x", "A", "-a", "a-", "a/b"] {
            assert!(!valid_profile_name(bad), "{bad}");
        }

        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        assert_eq!(free_profile_name(home, "Claude Work", &[]), "claude-work");
        std::fs::create_dir_all(home.join(PROFILES_DIR).join("claude-work")).unwrap();
        assert_eq!(free_profile_name(home, "Claude Work", &[]), "claude-work-2");
        assert_eq!(
            free_profile_name(home, "Claude Work", &["claude-work-2".into()]),
            "claude-work-3"
        );
        let id = AccountId::new();
        assert_eq!(
            profile_dir(home, id, Some("claude-work")),
            home.join(PROFILES_DIR).join("claude-work")
        );
        assert_eq!(
            profile_dir(home, id, Some("../x")),
            home.join(PROFILES_DIR).join(id.to_string())
        );
    }

    #[test]
    fn each_harness_gets_its_home_in_the_profile_folder() {
        let dir = Path::new("/p");
        let claude = profile_env(HarnessKind::ClaudeCode, dir);
        assert_eq!(
            claude["CLAUDE_CONFIG_DIR"],
            dir.join("claude").to_string_lossy()
        );
        assert_eq!(
            profile_env(HarnessKind::Codex, dir)["CODEX_HOME"],
            dir.join("codex").to_string_lossy()
        );
        assert_eq!(
            profile_env(HarnessKind::GeminiCli, dir)["GEMINI_CLI_HOME"],
            dir.join("gemini").to_string_lossy()
        );
        let opencode = profile_env(HarnessKind::OpenCode, dir);
        assert_eq!(opencode.len(), 5 + usize::from(cfg!(windows)));
        assert_eq!(opencode["HOME"], dir.join("home").to_string_lossy());
        assert_eq!(
            opencode["XDG_STATE_HOME"],
            dir.join("xdg").join("state").to_string_lossy()
        );
        assert!(profile_env(HarnessKind::Acp, dir).is_empty());
    }

    #[test]
    fn opencode_runs_with_a_home_inside_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("opencode-profile");
        prepare_profile(HarnessKind::OpenCode, &profile).unwrap();
        let env = profile_env(HarnessKind::OpenCode, &profile);
        let home = std::path::PathBuf::from(&env["HOME"]);
        assert_eq!(home, profile.join("home"));
        assert!(home.is_dir(), "the worker makes the home folder");
        // `~/.claude/skills` and `~/.agents/skills` are not in it.
        assert!(!home.join(".claude").exists() && !home.join(".agents").exists());
        // The other harnesses keep their home.
        for kind in [
            HarnessKind::ClaudeCode,
            HarnessKind::Codex,
            HarnessKind::GeminiCli,
            HarnessKind::Acp,
        ] {
            assert!(!profile_env(kind, &profile).contains_key("HOME"), "{kind}");
        }
    }

    /// OpenSSH finds `~/.ssh` through the password database, not through
    /// `HOME`. So the own `HOME` of an OpenCode account hides the skills of
    /// the real home, but not the ssh keys, the ssh config and `known_hosts`
    /// of the user. This test shows it with `ssh -G`, which reads the user's
    /// config and connects to nothing. It runs where `ssh` is installed and
    /// the user has a password database entry (Linux and macOS).
    #[cfg(unix)]
    #[test]
    fn openssh_ignores_the_own_home_of_an_opencode_account() {
        let Ok(output) = std::process::Command::new("ssh").arg("-V").output() else {
            return;
        };
        if !output.status.success() && output.stderr.is_empty() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        prepare_profile(HarnessKind::OpenCode, &profile).unwrap();
        let env = profile_env(HarnessKind::OpenCode, &profile);
        let own_home = std::path::PathBuf::from(&env["HOME"]);
        // A config in the own home that sets a user name nobody else sets.
        std::fs::create_dir_all(own_home.join(".ssh")).unwrap();
        std::fs::write(
            own_home.join(".ssh/config"),
            "Host *\n  User user-from-the-own-home\n",
        )
        .unwrap();
        let run = |home: &std::path::Path| {
            let out = std::process::Command::new("ssh")
                .args(["-G", "host.example.invalid"])
                .env("HOME", home)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let config = run(&own_home);
        assert!(
            !config.contains("user-from-the-own-home"),
            "ssh read ~/.ssh/config from HOME, not from the password database:\n{config}"
        );
    }

    #[test]
    fn tools_of_the_user_keep_their_real_places() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path();
        std::fs::write(real.join(".gitconfig"), "[user]\n").unwrap();
        std::fs::create_dir_all(real.join(".cargo")).unwrap();
        std::fs::create_dir_all(real.join(".gnupg")).unwrap();
        // A folder with the name of a file entry does not count, and a tool
        // that the user does not have gets no variable.
        std::fs::create_dir_all(real.join(".npm")).unwrap();
        let pins = tool_path_pins(real, &|name| name == "npm_config_cache");
        assert_eq!(
            pins["GIT_CONFIG_GLOBAL"],
            real.join(".gitconfig").to_string_lossy()
        );
        assert_eq!(pins["CARGO_HOME"], real.join(".cargo").to_string_lossy());
        assert_eq!(pins["GNUPGHOME"], real.join(".gnupg").to_string_lossy());
        assert!(!pins.contains_key("RUSTUP_HOME"), "{pins:?}");
        assert!(!pins.contains_key("BUN_INSTALL"), "{pins:?}");
        assert!(
            !pins.contains_key("npm_config_cache"),
            "a variable that the user set stays: {pins:?}"
        );
        // The pins never name the folders of the harnesses.
        assert!(
            pins.values()
                .all(|v| !v.contains(".claude") && !v.contains(".agents"))
        );
    }

    #[test]
    fn codex_and_gemini_settings_are_written_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("codex-profile");
        prepare_profile(HarnessKind::Codex, &codex).unwrap();
        let config = codex.join("codex").join("config.toml");
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains(CODEX_FILE_STORE_LINE), "{text}");
        assert!(
            text.contains(consort_tools::config::CODEX_CONFIG_LINE),
            "{text}"
        );
        // A setting of the user stays, and a second run adds nothing.
        std::fs::write(
            &config,
            format!("{text}model = \"o3\"\n[profiles.x]\na = 1\n"),
        )
        .unwrap();
        prepare_profile(HarnessKind::Codex, &codex).unwrap();
        let again = std::fs::read_to_string(&config).unwrap();
        assert_eq!(again.matches("cli_auth_credentials_store").count(), 1);
        assert!(again.contains("model = \"o3\"") && again.contains("[profiles.x]"));
        let first_table = again.find('[').unwrap();
        assert!(again.find("cli_auth").unwrap() < first_table);

        let gemini = dir.path().join("gemini-profile");
        prepare_profile(HarnessKind::GeminiCli, &gemini).unwrap();
        let settings = gemini.join("gemini").join(".gemini").join("settings.json");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
        assert_eq!(value["general"]["enableAutoUpdate"], false);

        let opencode = dir.path().join("opencode-profile");
        prepare_profile(HarnessKind::OpenCode, &opencode).unwrap();
        assert!(opencode.join("xdg").join("data").is_dir());
        assert!(opencode.join("xdg").join("cache").is_dir());
    }

    #[test]
    fn secrets_round_trip_with_mode_0600() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("p");
        assert!(read_secrets(&profile).unwrap().is_empty());
        let secrets = BTreeMap::from([("ANTHROPIC_API_KEY".to_owned(), "sk-x".to_owned())]);
        write_secrets(&profile, &secrets).unwrap();
        assert_eq!(read_secrets(&profile).unwrap(), secrets);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(profile.join(ENV_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        write_secrets(&profile, &BTreeMap::new()).unwrap();
        assert!(!profile.join(ENV_FILE).exists());
        assert!(valid_variable_name("OPENAI_API_KEY"));
        assert!(
            !valid_variable_name("1X") && !valid_variable_name("A-B") && !valid_variable_name("")
        );
    }

    #[test]
    fn the_consort_home_follows_the_variable() {
        // The test cannot set the variable (that needs `unsafe` in Rust
        // 2024), so it checks the fallback shape only.
        if std::env::var_os(HOME_VAR).is_none()
            && let Some(home) = consort_home()
        {
            assert!(home.ends_with(".consort"));
        }
    }
}
