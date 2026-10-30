//! Fetches skill folders from git repos with the `git` CLI.
//!
//! The method (docs/notes/step3-hub.md, "Fetch method"):
//! 1. `git init --bare` in a new temp folder, with no templates, so the
//!    repo has no hooks.
//! 2. `git fetch --depth 1 --filter=blob:none` of one ref: the commit and
//!    its trees, with no file content. The commit SHA is the pin.
//! 3. `git ls-tree` of the skill folder. The hub checks the count of
//!    entries before it fetches content.
//! 4. `git fetch` of only the blobs of that folder, then `git cat-file
//!    --batch-check` for their sizes, the checks of `package.rs`, and
//!    `git cat-file --batch` for the content.
//!
//! Nothing is checked out, so no file of the repo touches the disk as a
//! file, a link in the repo is only a blob with its target text, and no
//! filter or hook of the repo runs. Git gets no system or user config, no
//! terminal prompt, no credential helper and only the allowed protocols.
//! Every command has a time limit, and a fetch stops when the temp folder
//! grows past a size limit. The temp folder goes away with [`Repo`].
//!
//! # Private repos and ssh
//!
//! A public repo is fetched in the clean setup above. A repo that needs a
//! login is fetched with the credentials of the user that runs the hub, so
//! the hub stores no secret:
//! - An `https` repo is fetched without credentials first. When the server
//!   asks for a login, git runs again with the real home folder, so the
//!   credential helper of that user (the keychain, `gh auth`, a store file)
//!   answers. The hub never reads the answer: git does.
//! - An `ssh` repo (`ssh://git@host/owner/repo`, or `git@host:owner/repo`
//!   as the user types it) goes straight to the second setup. Git runs
//!   `ssh` with the ssh agent (`SSH_AUTH_SOCK`) and the `~/.ssh` folder of
//!   that user. `ssh` runs with `BatchMode` (no prompt) and with strict host
//!   key checking, so an unknown host fails with a message that names the
//!   fix. No agent forwarding.
//! - Only a URL that the user gave the hub may use this: a URL typed in a
//!   client request, a source that the user added, or a repo of a skill that
//!   the user installed. A URL from a directory, a marketplace plugin or
//!   an assistant tool fetches without credentials, and an `ssh` URL from
//!   there is refused. So a listing cannot make the hub offer its keys to a
//!   host that the user did not choose.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use consort_proto::error::ErrorCode;
use consort_proto::model::{SKILL_MAX_FILES, validate_relative_path};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::NewSkillFile;
use super::package::{Entry, EntryKind, LISTING_MAX_ENTRIES, check_listing, skill_invalid};
use crate::error::{HubError, Result};

/// How long one git command may take.
pub(crate) const GIT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long `ssh` waits to connect.
const SSH_CONNECT_SECONDS: u32 = 20;
/// How large the temp folder of one fetch may grow. Trees of a large repo
/// take a few MiB; the blobs of a skill take at most its size limit.
const FETCH_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// The longest link target that the hub reads.
const LINK_TARGET_MAX_BYTES: u64 = 4_096;
/// The most bytes of git's error text in a message.
const STDERR_MAX_CHARS: usize = 300;

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// The environment variables that git keeps: the path, and the proxy and
/// certificate settings of the hub machine.
/// The variables that git also keeps when it may use the credentials of the
/// user: the ssh agent, and what credential helpers need to find the keychain
/// or the session bus.
const CREDENTIAL_ENV: &[&str] = &[
    "SSH_AUTH_SOCK",
    "USER",
    "LOGNAME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
];

const KEPT_ENV: &[&str] = &[
    "PATH",
    "SYSTEMROOT",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "GIT_SSL_CAINFO",
];

/// Settings of a fetch.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GitOptions {
    /// Allow `file://` and `http://` URLs. Only tests turn this on.
    pub allow_local: bool,
    /// How long one git command may take.
    pub timeout: Duration,
    /// Whether git may use the credentials of the hub's user for this repo.
    /// See the module docs: only for a URL that the user gave the hub.
    pub credentials: bool,
    /// Git runs with the credentials now. The fetch sets it after an
    /// anonymous try met a login, or at once for an `ssh` URL.
    pub(crate) with_credentials: bool,
}

impl GitOptions {
    /// Clean setup, the default time limit, no credentials.
    pub(crate) const fn new(allow_local: bool) -> Self {
        Self {
            allow_local,
            timeout: GIT_TIMEOUT,
            credentials: false,
            with_credentials: false,
        }
    }

    /// The same, for a URL that the user gave the hub.
    pub(crate) const fn trusted(mut self) -> Self {
        self.credentials = true;
        self
    }

    fn allowed_protocols(self) -> &'static str {
        match (self.allow_local, self.with_credentials) {
            (true, true) => "https:http:file:ssh",
            (true, false) => "https:http:file",
            (false, true) => "https:ssh",
            (false, false) => "https",
        }
    }
}

/// The text of a git failure that means "git needs a login".
fn is_login_failure(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    [
        "terminal prompts disabled",
        "could not read username",
        "could not read password",
        "authentication failed",
        "permission denied (publickey",
        "permission denied, please try again",
        "http basic: access denied",
        "the requested url returned error: 401",
        "the requested url returned error: 403",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// The fix for the failure, when the text names one.
pub(crate) fn failure_hint(url: &str, detail: &str, credentials: bool) -> Option<String> {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("host key verification failed") {
        let host = ssh_host(url).unwrap_or("the host");
        return Some(format!(
            "The hub does not know the host key of {host}. Run `ssh {host}` once on the hub machine, as the user that runs the hub, and accept the key. Then try again."
        ));
    }
    if lower.contains("permission denied (publickey") {
        return Some(
            "The ssh agent of the hub user has no key that this host accepts. Run `ssh-add -l` on the hub machine, as the user that runs the hub, to see the keys. Then try again."
                .to_owned(),
        );
    }
    if lower.contains("took longer than") {
        return Some(format!(
            "The server did not finish in time ({}). It may be slow or down. Try again later.",
            detail.trim_start_matches("git ")
        ));
    }
    if is_login_failure(detail) {
        return Some(if credentials {
            "The repo needs a login, and the hub's git credentials did not give one. Check that `git ls-remote <url>` works on the hub machine, as the user that runs the hub. The hub keeps no password. Then try again.".to_owned()
        } else {
            "The repo needs a login. The hub uses git credentials only for a repo that you add yourself in Settings > Skills. Add the URL there, then try again.".to_owned()
        });
    }
    None
}

fn fetch_failed(url: &str, detail: &str) -> HubError {
    fetch_failed_with(url, detail, false)
}

fn fetch_failed_with(url: &str, detail: &str, credentials: bool) -> HubError {
    let detail: String = detail.trim().chars().take(STDERR_MAX_CHARS).collect();
    let url = shown_url(url);
    if let Some(hint) = failure_hint(&url, &detail, credentials) {
        return HubError::new(
            ErrorCode::NotFound,
            format!("The hub could not fetch {url}. {hint}"),
        );
    }
    HubError::new(
        ErrorCode::NotFound,
        if detail.is_empty() {
            format!("The hub could not fetch {url}. Check the URL and the branch, then try again.")
        } else {
            format!(
                "The hub could not fetch {url}: {detail}. Check the URL and the branch, then try again."
            )
        },
    )
}

/// The URL for a message: without the `#ref` suffix.
fn shown_url(url: &str) -> String {
    url.split('#').next().unwrap_or(url).to_owned()
}

/// The host of an ssh URL, in either form.
fn ssh_host(url: &str) -> Option<&str> {
    if let Some(rest) = url.strip_prefix("ssh://") {
        let authority = rest.split('/').next()?;
        let host = authority.rsplit('@').next()?;
        return Some(host.split(':').next().unwrap_or(host));
    }
    let (left, _) = url.split_once(':').filter(|_| !url.contains("://"))?;
    Some(left.rsplit('@').next().unwrap_or(left))
}

/// A git URL with the parts that a GitHub page URL can give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitTarget {
    /// The clone URL.
    pub url: String,
    /// The branch or tag from a `/tree/<ref>/` URL.
    pub git_ref: Option<String>,
    /// The folder from a `/tree/<ref>/<path>` URL.
    pub path: Option<String>,
}

fn is_github_name(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Reads a git source as the user gives it: a clone URL, `owner/repo` for
/// GitHub, a GitHub page URL such as
/// `https://github.com/owner/repo/tree/main/skills/pdf`, or an `ssh` URL
/// (`ssh://git@host/owner/repo`, or `git@host:owner/repo`). A `#ref` at the
/// end names a branch, a tag or a commit. Only `https` is allowed, and
/// `http` and `file` with `allow_local`, and `ssh` with `credentials` (see
/// the module docs). A URL with a password is refused, so no secret reaches
/// a log or a skill.
pub(crate) fn parse_git_url(input: &str, options: GitOptions) -> Result<GitTarget> {
    let input = input.trim();
    let invalid = |why: &str| {
        HubError::invalid(format!(
            "{input} is not a git URL that the hub can use: {why}. Use an https URL or owner/repo."
        ))
    };
    if input.is_empty() {
        return Err(HubError::invalid(
            "The git URL is empty. Use an https URL or owner/repo.",
        ));
    }
    if input.starts_with('-') || input.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid(
            "it has a space, a control character or a leading -",
        ));
    }
    // A ref after `#`, as in `https://host/repo#v2`.
    let (body, fragment_ref) = match input.split_once('#') {
        Some((body, git_ref)) => {
            check_ref(git_ref)?;
            (body, Some(git_ref.to_owned()))
        }
        None => (input, None),
    };
    let mut target = parse_git_body(body, options, &invalid)?;
    if fragment_ref.is_some() {
        target.git_ref = fragment_ref;
    }
    Ok(target)
}

fn parse_git_body(
    input: &str,
    options: GitOptions,
    invalid: &dyn Fn(&str) -> HubError,
) -> Result<GitTarget> {
    if !input.contains(':') {
        let parts: Vec<&str> = input.trim_end_matches('/').split('/').collect();
        if parts.len() == 2 && parts.iter().all(|p| is_github_name(p)) {
            return Ok(GitTarget {
                url: format!("https://github.com/{}/{}", parts[0], parts[1]),
                git_ref: None,
                path: None,
            });
        }
        return Err(invalid("it is not owner/repo"));
    }
    let Some((scheme, rest)) = input.split_once("://") else {
        // `git@host:owner/repo`, the form that GitHub shows for ssh.
        return parse_scp_url(input, options, invalid);
    };
    let scheme = scheme.to_ascii_lowercase();
    match scheme.as_str() {
        "https" => {}
        "http" | "file" if options.allow_local => {}
        "ssh" => return parse_ssh_url(rest, options, invalid),
        _ => return Err(invalid("only https and ssh URLs are allowed")),
    }
    if scheme != "file" {
        let authority = rest.split('/').next().unwrap_or_default();
        if authority.is_empty() || authority.contains('@') {
            return Err(invalid("it has no host, or it has a user name"));
        }
        if rest.contains('?') {
            return Err(invalid("it has a query"));
        }
    }
    // A GitHub page URL of a folder: /owner/repo/tree/<ref>/<path>.
    if scheme == "https" {
        let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
        if parts.len() >= 3
            && parts[0].eq_ignore_ascii_case("github.com")
            && is_github_name(parts[1])
            && is_github_name(parts[2])
        {
            let url = format!("https://github.com/{}/{}", parts[1], parts[2]);
            if parts.len() >= 5 && parts[3] == "tree" {
                let path = parts[5..].join("/");
                return Ok(GitTarget {
                    url,
                    git_ref: Some(parts[4].to_owned()),
                    path: (!path.is_empty()).then_some(path),
                });
            }
            if parts.len() == 3 {
                return Ok(GitTarget {
                    url,
                    git_ref: None,
                    path: None,
                });
            }
        }
    }
    Ok(GitTarget {
        url: format!("{scheme}://{rest}"),
        git_ref: None,
        path: None,
    })
}

fn is_ssh_user(user: &str) -> bool {
    !user.is_empty()
        && user.len() <= 64
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn is_ssh_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['-', '.'])
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

fn ssh_refused(invalid: &dyn Fn(&str) -> HubError) -> HubError {
    invalid(
        "it is an ssh URL. The hub uses ssh only for a repo that you add yourself in Settings > Skills",
    )
}

/// `ssh://[user@]host[:port]/path`, after the scheme.
fn parse_ssh_url(
    rest: &str,
    options: GitOptions,
    invalid: &dyn Fn(&str) -> HubError,
) -> Result<GitTarget> {
    if !options.credentials {
        return Err(ssh_refused(invalid));
    }
    let (authority, path) = rest
        .split_once('/')
        .ok_or_else(|| invalid("it has no path"))?;
    if path.is_empty() || path.starts_with('-') || rest.contains('?') {
        return Err(invalid(
            "its path is empty or starts with -, or it has a query",
        ));
    }
    let (user, host_port) = match authority.rsplit_once('@') {
        Some((user, host_port)) => (Some(user), host_port),
        None => (None, authority),
    };
    if user.is_some_and(|u| !is_ssh_user(u)) {
        return Err(invalid(
            "its ssh user name has a character that is not allowed",
        ));
    }
    let (host, port) = match host_port.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (host_port, None),
    };
    if !is_ssh_host(host) {
        return Err(invalid("its host name is not valid"));
    }
    if port.is_some_and(|p| p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit())) {
        return Err(invalid("its port is not a number"));
    }
    let user = user.map(|u| format!("{u}@")).unwrap_or_default();
    let port = port.map(|p| format!(":{p}")).unwrap_or_default();
    Ok(GitTarget {
        url: format!("ssh://{user}{host}{port}/{path}"),
        git_ref: None,
        path: None,
    })
}

/// `[user@]host:path`, the scp-like form of an ssh URL. Git reads this form
/// itself: a path without a leading `/` is in the home folder of the ssh
/// user. The hub keeps the text as the user typed it, after the checks.
fn parse_scp_url(
    input: &str,
    options: GitOptions,
    invalid: &dyn Fn(&str) -> HubError,
) -> Result<GitTarget> {
    let Some((left, path)) = input.split_once(':') else {
        return Err(invalid("it has no scheme"));
    };
    let (user, host) = match left.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, left),
    };
    // `ext::command`, `C:\folder` and the like are not ssh URLs.
    if left.contains('/')
        || path.is_empty()
        || path.starts_with(':')
        || !(user.is_some() || host.contains('.'))
        || !is_ssh_host(host)
    {
        return Err(invalid("it has no scheme"));
    }
    if !options.credentials {
        return Err(ssh_refused(invalid));
    }
    if user.is_some_and(|u| !is_ssh_user(u)) {
        return Err(invalid(
            "its ssh user name has a character that is not allowed",
        ));
    }
    if path.starts_with('-') || path.contains("..") {
        return Err(invalid("its path starts with - or climbs out of a folder"));
    }
    Ok(GitTarget {
        url: input.to_owned(),
        git_ref: None,
        path: None,
    })
}

/// Whether a clone URL goes over ssh: `ssh://...` or the scp-like
/// `user@host:path`. Every other form that the hub accepts has `://`.
pub(crate) fn is_ssh_url(url: &str) -> bool {
    url.starts_with("ssh://") || (!url.contains("://") && url.contains(':'))
}

/// Two clone URLs name the same repo: the same text without case in the
/// host, a trailing `/` or `.git`.
pub(crate) fn same_repo(a: &str, b: &str) -> bool {
    fn key(url: &str) -> String {
        url.trim()
            .split('#')
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .to_ascii_lowercase()
    }
    key(a) == key(b)
}

/// Checks a branch, a tag or a commit SHA.
pub(crate) fn check_ref(git_ref: &str) -> Result<()> {
    let ok = !git_ref.is_empty()
        && git_ref.len() <= 200
        && !git_ref.starts_with(['-', '/'])
        && !git_ref.ends_with(['/', '.'])
        && !git_ref.contains("..")
        && !git_ref.contains("//")
        && !git_ref.contains("@{")
        && git_ref
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    if ok {
        Ok(())
    } else {
        Err(HubError::invalid(format!(
            "{git_ref} is not a branch, a tag or a commit that the hub can use."
        )))
    }
}

/// Checks the folder of a skill in a repo and gives its normal form. An
/// empty path or `.` is the repo root.
pub(crate) fn normalize_repo_path(path: Option<&str>) -> Result<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if path.trim().starts_with(['/', '\\']) {
        return Err(HubError::invalid(format!(
            "The path {path} is absolute. Use a folder in the repo, such as skills/pdf."
        )));
    }
    let trimmed = path.trim().trim_start_matches("./").trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return Ok(None);
    }
    validate_relative_path("path", trimmed).map_err(|e| {
        HubError::invalid(format!(
            "The path {path} {}. Use a folder in the repo, such as skills/pdf.",
            e.message
        ))
    })?;
    Ok(Some(trimmed.to_owned()))
}

/// The size of a folder, without following links.
fn folder_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

/// The home folder of the user that runs the hub, for the setup with
/// credentials. Tests set another one for their thread.
fn user_home() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = TEST_HOME.with(|h| h.borrow().clone()) {
        return Some(home);
    }
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|key| std::env::var_os(key).filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

#[cfg(test)]
thread_local! {
    static TEST_HOME: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Makes the setup with credentials use `home` in this test thread.
#[cfg(test)]
pub(crate) fn set_test_home(home: Option<PathBuf>) {
    TEST_HOME.with(|h| *h.borrow_mut() = home);
}

/// Reads a pipe of git to its end.
async fn read_all<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) -> Vec<u8> {
    use tokio::io::AsyncReadExt as _;
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut bytes).await;
    }
    bytes
}

/// Stops git and waits until it has ended. On Windows, `git.exe` starts
/// helper programs (`git-remote-http.exe`) that stay alive when only `git.exe`
/// ends, so the whole tree is stopped.
async fn stop(
    child: &mut tokio::process::Child,
    out_task: &tokio::task::JoinHandle<Vec<u8>>,
    err_task: &tokio::task::JoinHandle<Vec<u8>>,
) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let _ = child.kill().await;
    out_task.abort();
    err_task.abort();
}

/// One git command, in a clean environment. `home` is the temp folder of
/// the fetch. With `watch`, the command stops when that folder grows past
/// [`FETCH_MAX_BYTES`]. With `options.with_credentials`, git also gets the
/// real home folder, the user's git config and the ssh agent.
async fn run_git(
    options: GitOptions,
    home: &Path,
    git_dir: Option<&Path>,
    args: &[&str],
    stdin: Option<Vec<u8>>,
    watch: bool,
) -> std::result::Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command.env_clear();
    for key in KEPT_ENV {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", options.allowed_protocols())
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("LC_ALL", "C")
        .current_dir(home);
    if options.with_credentials {
        for key in CREDENTIAL_ENV {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        // The user's own `~/.gitconfig` and system config give the
        // credential helper. The settings below still win, as they come on
        // the command line. `GIT_SSH_COMMAND` wins over `core.sshCommand`.
        command
            .env("HOME", user_home().unwrap_or_else(|| home.to_path_buf()))
            .env(
                "GIT_SSH_COMMAND",
                format!(
                    "ssh -o BatchMode=yes -o ConnectTimeout={SSH_CONNECT_SECONDS} -o StrictHostKeyChecking=yes -o ForwardAgent=no -o ForwardX11=no"
                ),
            );
    } else {
        command
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", NULL_DEVICE);
    }
    let mut settings = vec![
        format!("core.hooksPath={NULL_DEVICE}"),
        "core.fsmonitor=false".to_owned(),
        "core.symlinks=false".to_owned(),
        "transfer.fsckObjects=true".to_owned(),
        "protocol.version=2".to_owned(),
        "init.defaultBranch=main".to_owned(),
        "fetch.recurseSubmodules=false".to_owned(),
        "http.lowSpeedLimit=1024".to_owned(),
        "http.lowSpeedTime=30".to_owned(),
    ];
    if !options.with_credentials {
        settings.push("credential.helper=".to_owned());
    }
    for setting in settings {
        command.arg("-c").arg(setting);
    }
    if let Some(dir) = git_dir {
        command.arg(format!("--git-dir={}", dir.display()));
    }
    command
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            "git is not installed on the hub machine".to_owned()
        } else {
            format!("git did not start ({err})")
        }
    })?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = pipe.write_all(&bytes).await;
            let _ = pipe.shutdown().await;
        });
    }
    // Read the pipes in tasks and wait for the process itself. A stop then
    // waits until git has ended. On Windows a folder cannot be removed while
    // a process runs in it, so the temp folder needs a git that is gone.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_task = tokio::spawn(read_all(stdout));
    let err_task = tokio::spawn(read_all(stderr));
    let deadline = tokio::time::sleep(options.timeout);
    tokio::pin!(deadline);
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let status = loop {
        tokio::select! {
            out = child.wait() => break out.map_err(|e| format!("git failed ({e})"))?,
            () = &mut deadline => {
                stop(&mut child, &out_task, &err_task).await;
                return Err(format!("git took longer than {}", crate::util::duration_text(options.timeout)));
            }
            _ = tick.tick(), if watch => {
                let dir = home.to_path_buf();
                let size = tokio::task::spawn_blocking(move || folder_size(&dir)).await.unwrap_or(0);
                if size > FETCH_MAX_BYTES {
                    stop(&mut child, &out_task, &err_task).await;
                    return Err(format!("the repo is larger than {} MiB", FETCH_MAX_BYTES / (1024 * 1024)));
                }
            }
        }
    };
    let output = std::process::Output {
        status,
        stdout: out_task.await.unwrap_or_default(),
        stderr: err_task.await.unwrap_or_default(),
    };
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // A login or host key problem is the useful line, though git ends
        // with "Could not read from remote repository."
        let line = stderr
            .lines()
            .find(|l| {
                is_login_failure(l)
                    || l.to_ascii_lowercase()
                        .contains("host key verification failed")
            })
            .or_else(|| {
                stderr
                    .lines()
                    .rev()
                    .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
            })
            .or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()))
            .unwrap_or("git failed");
        Err(line
            .trim_start_matches("fatal: ")
            .trim_start_matches("error: ")
            .to_owned())
    }
}

/// The options for the first try at `url`: an `ssh` URL goes straight to the
/// setup with credentials, any other URL starts anonymous.
fn first_try(url: &str, options: GitOptions) -> GitOptions {
    GitOptions {
        with_credentials: options.credentials && is_ssh_url(url),
        ..options
    }
}

/// The commit that a branch or tag points to now, without a fetch
/// (`git ls-remote`). A full SHA is its own answer. A repo that needs a
/// login is asked again with the credentials of the hub's user when
/// `options.credentials` allows it.
pub(crate) async fn remote_commit(
    tmp_root: &Path,
    url: &str,
    git_ref: Option<&str>,
    options: GitOptions,
) -> Result<String> {
    if let Some(r) = git_ref {
        check_ref(r)?;
        if is_full_sha(r) {
            return Ok(r.to_ascii_lowercase());
        }
    }
    let home = new_temp(tmp_root)?;
    let wanted = git_ref.unwrap_or("HEAD");
    let args = ["ls-remote", "--end-of-options", url, wanted];
    let mut tried = first_try(url, options);
    let mut result = run_git(tried, home.path(), None, &args, None, false).await;
    if let Err(detail) = &result
        && options.credentials
        && !tried.with_credentials
        && is_login_failure(detail)
    {
        tried.with_credentials = true;
        result = run_git(tried, home.path(), None, &args, None, false).await;
    }
    let out = result.map_err(|e| fetch_failed_with(url, &e, tried.with_credentials))?;
    let text = String::from_utf8_lossy(&out);
    // An exact match first: a branch, then a tag, then anything.
    let rows: Vec<(&str, &str)> = text.lines().filter_map(|l| l.split_once('\t')).collect();
    let pick = rows
        .iter()
        .find(|(_, name)| *name == wanted || *name == format!("refs/heads/{wanted}"))
        .or_else(|| {
            rows.iter()
                .find(|(_, name)| *name == format!("refs/tags/{wanted}^{{}}"))
        })
        .or_else(|| rows.first())
        .map(|(sha, _)| sha.to_string());
    pick.filter(|sha| is_full_sha(sha))
        .ok_or_else(|| fetch_failed(url, &format!("it has no branch or tag {wanted}")))
}

fn is_full_sha(text: &str) -> bool {
    text.len() == 40 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// A new temp folder for one fetch. The parent folder is private to the
/// hub's user, like the rest of the data folder.
fn new_temp(tmp_root: &Path) -> Result<tempfile::TempDir> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(tmp_root)?;
    Ok(tempfile::Builder::new()
        .prefix("skill-fetch-")
        .tempdir_in(tmp_root)?)
}

/// One entry of `git ls-tree -r`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeItem {
    pub mode: String,
    pub oid: String,
    pub path: String,
}

/// A fetched commit in a temp bare repo. The temp folder goes away when
/// this is dropped.
pub(crate) struct Repo {
    dir: tempfile::TempDir,
    git_dir: PathBuf,
    url: String,
    options: GitOptions,
    /// The fetched commit: the pin.
    pub commit: String,
}

impl Repo {
    /// Fetches the commit of `git_ref` (the default branch when empty),
    /// with its trees and no file content.
    pub async fn fetch(
        tmp_root: &Path,
        url: &str,
        git_ref: Option<&str>,
        options: GitOptions,
    ) -> Result<Self> {
        if let Some(r) = git_ref {
            check_ref(r)?;
        }
        let dir = new_temp(tmp_root)?;
        let git_dir = dir.path().join("repo.git");
        let git_dir_text = git_dir.display().to_string();
        run_git(
            options,
            dir.path(),
            None,
            &["init", "--bare", "--quiet", "--template=", &git_dir_text],
            None,
            false,
        )
        .await
        .map_err(|e| fetch_failed(url, &e))?;
        let wanted = git_ref.unwrap_or("HEAD");
        let fetch_args = [
            "fetch",
            "--quiet",
            "--no-tags",
            "--depth=1",
            "--filter=blob:none",
            "--end-of-options",
            url,
            wanted,
        ];
        // Anonymous first (except for ssh). A login asks for the setup with
        // the credentials of the hub's user, when the URL allows it.
        let mut options = first_try(url, options);
        let mut fetched =
            run_git(options, dir.path(), Some(&git_dir), &fetch_args, None, true).await;
        if let Err(detail) = &fetched
            && options.credentials
            && !options.with_credentials
            && is_login_failure(detail)
        {
            options.with_credentials = true;
            fetched = run_git(options, dir.path(), Some(&git_dir), &fetch_args, None, true).await;
        }
        fetched.map_err(|e| fetch_failed_with(url, &e, options.with_credentials))?;
        let out = run_git(
            options,
            dir.path(),
            Some(&git_dir),
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                "FETCH_HEAD^{commit}",
            ],
            None,
            false,
        )
        .await
        .map_err(|e| fetch_failed(url, &e))?;
        let commit = String::from_utf8_lossy(&out).trim().to_owned();
        if !is_full_sha(&commit) {
            return Err(fetch_failed(url, "git gave no commit"));
        }
        Ok(Self {
            dir,
            git_dir,
            url: url.to_owned(),
            options,
            commit,
        })
    }

    async fn git(&self, args: &[&str], stdin: Option<Vec<u8>>, watch: bool) -> Result<Vec<u8>> {
        run_git(
            self.options,
            self.dir.path(),
            Some(&self.git_dir),
            args,
            stdin,
            watch,
        )
        .await
        .map_err(|e| fetch_failed_with(&self.url, &e, self.options.with_credentials))
    }

    /// Every file, link and submodule under `path` (the whole tree when
    /// empty), with paths relative to `path`. Fails with `not_found` when
    /// the commit has no such folder.
    pub async fn tree(&self, path: Option<&str>) -> Result<Vec<TreeItem>> {
        let spec = match path {
            Some(p) => format!("{}:{p}", self.commit),
            None => format!("{}^{{tree}}", self.commit),
        };
        let kind = self
            .git(&["cat-file", "-t", "--end-of-options", &spec], None, false)
            .await
            .map_err(|_| {
                HubError::not_found(format!(
                    "The repo {} has no folder {} at commit {}.",
                    self.url,
                    path.unwrap_or("."),
                    &self.commit[..12]
                ))
            })?;
        if String::from_utf8_lossy(&kind).trim() != "tree" {
            return Err(HubError::not_found(format!(
                "{} in the repo {} is not a folder.",
                path.unwrap_or("."),
                self.url
            )));
        }
        let out = self
            .git(
                &["ls-tree", "-r", "-z", "--end-of-options", &spec],
                None,
                false,
            )
            .await?;
        let mut items = Vec::new();
        for record in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let text = std::str::from_utf8(record).map_err(|_| {
                skill_invalid("A path in the repo is not UTF-8 text. The hub cannot store it.")
            })?;
            let (meta, path) = text
                .split_once('\t')
                .ok_or_else(|| HubError::internal("git ls-tree gave a line with no tab"))?;
            let mut parts = meta.split(' ');
            let (Some(mode), Some(_kind), Some(oid)) = (parts.next(), parts.next(), parts.next())
            else {
                return Err(HubError::internal("git ls-tree gave a line with no object"));
            };
            items.push(TreeItem {
                mode: mode.to_owned(),
                oid: oid.to_owned(),
                path: path.to_owned(),
            });
        }
        Ok(items)
    }

    /// Fetches the blobs that the commit fetch left out.
    async fn fetch_blobs(&self, oids: &[&str]) -> Result<()> {
        let mut missing = Vec::new();
        let check = self.batch_check(oids).await?;
        for oid in oids {
            if !check.contains_key(*oid) {
                missing.push(*oid);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let mut args = vec![
            "-c",
            "fetch.negotiationAlgorithm=noop",
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--filter=blob:none",
            "--end-of-options",
            self.url.as_str(),
        ];
        args.extend(missing.iter().copied());
        self.git(&args, None, true).await?;
        Ok(())
    }

    /// The sizes of the blobs that the repo has.
    async fn batch_check(&self, oids: &[&str]) -> Result<HashMap<String, u64>> {
        let input = oids.iter().map(|o| format!("{o}\n")).collect::<String>();
        let out = self
            .git(
                &[
                    "cat-file",
                    "--batch-check=%(objectname) %(objecttype) %(objectsize)",
                ],
                Some(input.into_bytes()),
                false,
            )
            .await?;
        let mut sizes = HashMap::new();
        for line in String::from_utf8_lossy(&out).lines() {
            let parts: Vec<&str> = line.split(' ').collect();
            if let [oid, "blob", size] = parts.as_slice()
                && let Ok(size) = size.parse::<u64>()
            {
                sizes.insert((*oid).to_owned(), size);
            }
        }
        Ok(sizes)
    }

    /// The content of blobs whose sizes are checked already.
    async fn read_blobs(&self, oids: &[&str]) -> Result<HashMap<String, Vec<u8>>> {
        let input = oids.iter().map(|o| format!("{o}\n")).collect::<String>();
        let out = self
            .git(&["cat-file", "--batch"], Some(input.into_bytes()), false)
            .await?;
        let mut blobs = HashMap::new();
        let mut rest = out.as_slice();
        while !rest.is_empty() {
            let line_end = rest
                .iter()
                .position(|b| *b == b'\n')
                .ok_or_else(|| HubError::internal("git cat-file gave a header with no end"))?;
            let header = String::from_utf8_lossy(&rest[..line_end]).to_string();
            rest = &rest[line_end + 1..];
            let parts: Vec<&str> = header.split(' ').collect();
            let [oid, _kind, size] = parts.as_slice() else {
                return Err(fetch_failed(&self.url, "a file of the skill is missing"));
            };
            let size: usize = size
                .parse()
                .map_err(|_| HubError::internal("git cat-file gave a size that is not a number"))?;
            if rest.len() < size + 1 {
                return Err(HubError::internal("git cat-file stopped early"));
            }
            blobs.insert((*oid).to_owned(), rest[..size].to_vec());
            rest = &rest[size + 1..];
        }
        Ok(blobs)
    }

    /// The files of the skill folder at `path`, after every check of
    /// `package.rs`. Content is fetched and read only after the checks of
    /// the listing.
    pub async fn skill_files(&self, path: Option<&str>) -> Result<Vec<NewSkillFile>> {
        let items = self.tree(path).await?;
        // Count before any content is fetched. Submodules are refused later
        // with their own message.
        let files = items.iter().filter(|i| i.mode != "160000").count();
        if items.len() > LISTING_MAX_ENTRIES || files > SKILL_MAX_FILES {
            return Err(skill_invalid(format!(
                "The skill has more than {SKILL_MAX_FILES} files."
            )));
        }
        let blob_oids: Vec<&str> = items
            .iter()
            .filter(|i| i.mode != "160000")
            .map(|i| i.oid.as_str())
            .collect();
        self.fetch_blobs(&blob_oids).await?;
        let sizes = self.batch_check(&blob_oids).await?;
        let size_of = |oid: &str| {
            sizes
                .get(oid)
                .copied()
                .ok_or_else(|| fetch_failed(&self.url, "a file of the skill is missing"))
        };
        // Link targets are small blobs. Read them first.
        let mut link_oids = Vec::new();
        for item in items.iter().filter(|i| i.mode == "120000") {
            if size_of(&item.oid)? > LINK_TARGET_MAX_BYTES {
                return Err(skill_invalid(format!(
                    "{} is a link with a target that is too long.",
                    item.path
                )));
            }
            link_oids.push(item.oid.as_str());
        }
        let targets = if link_oids.is_empty() {
            HashMap::new()
        } else {
            self.read_blobs(&link_oids).await?
        };
        let mut entries = Vec::with_capacity(items.len());
        let mut oid_of_path = HashMap::new();
        for item in &items {
            let kind = match item.mode.as_str() {
                "100644" | "100664" | "100755" => {
                    oid_of_path.insert(item.path.clone(), item.oid.clone());
                    EntryKind::File {
                        size: size_of(&item.oid)?,
                        executable: item.mode == "100755",
                    }
                }
                "120000" => EntryKind::Link {
                    target: String::from_utf8_lossy(
                        targets
                            .get(&item.oid)
                            .map(Vec::as_slice)
                            .unwrap_or_default(),
                    )
                    .into_owned(),
                },
                "160000" => EntryKind::Submodule,
                _ => EntryKind::Special,
            };
            entries.push(Entry {
                path: item.path.clone(),
                kind,
            });
        }
        let plan = check_listing(&entries)?;
        let wanted: Vec<String> = plan.content_paths().into_iter().collect();
        let oids: Vec<&str> = wanted
            .iter()
            .filter_map(|p| oid_of_path.get(p).map(String::as_str))
            .collect();
        let blobs = self.read_blobs(&oids).await?;
        let mut contents = BTreeMap::new();
        for path in wanted {
            if let Some(bytes) = oid_of_path.get(&path).and_then(|oid| blobs.get(oid)) {
                contents.insert(path, bytes.clone());
            }
        }
        plan.assemble(&contents)
    }

    /// Reads small text files at these repo paths, for directory listings.
    /// A missing file or one over `max_bytes` is left out.
    pub async fn read_small_files(
        &self,
        items: &[TreeItem],
        max_bytes: u64,
    ) -> Result<HashMap<String, Vec<u8>>> {
        let oids: Vec<&str> = items.iter().map(|i| i.oid.as_str()).collect();
        if oids.is_empty() {
            return Ok(HashMap::new());
        }
        self.fetch_blobs(&oids).await?;
        let sizes = self.batch_check(&oids).await?;
        let small: Vec<&str> = oids
            .iter()
            .copied()
            .filter(|o| sizes.get(*o).is_some_and(|s| *s <= max_bytes))
            .collect();
        let blobs = self.read_blobs(&small).await?;
        Ok(items
            .iter()
            .filter_map(|i| blobs.get(&i.oid).map(|b| (i.path.clone(), b.clone())))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: GitOptions = GitOptions::new(true);
    const REMOTE: GitOptions = GitOptions::new(false);

    #[test]
    fn git_urls_are_read_and_checked() {
        let t = parse_git_url("anthropics/skills", REMOTE).unwrap();
        assert_eq!(t.url, "https://github.com/anthropics/skills");
        let t = parse_git_url(
            "https://github.com/openclaw/openclaw/tree/main/skills/nano-pdf",
            REMOTE,
        )
        .unwrap();
        assert_eq!(t.url, "https://github.com/openclaw/openclaw");
        assert_eq!(t.git_ref.as_deref(), Some("main"));
        assert_eq!(t.path.as_deref(), Some("skills/nano-pdf"));
        let t = parse_git_url("https://git.example.com/a/b.git", REMOTE).unwrap();
        assert_eq!(t.url, "https://git.example.com/a/b.git");
        for bad in [
            "",
            "-uhttps://x",
            "file:///etc",
            "http://example.com/a",
            "ssh://git@github.com/a/b",
            "git@github.com:a/b",
            "ext::sh -c touch% /tmp/x",
            "https://user:pass@github.com/a/b",
            "https://github.com/a/b?x=1",
            "a/b/c",
            "../b",
        ] {
            assert!(parse_git_url(bad, REMOTE).is_err(), "{bad}");
        }
        assert!(parse_git_url("file:///tmp/repo", LOCAL).is_ok());
        assert!(same_repo(
            "https://GitHub.com/a/b.git/",
            "https://github.com/a/b"
        ));
    }

    #[test]
    fn refs_and_paths_are_checked() {
        for good in ["main", "v1.2.0", "release/2026", &"a".repeat(40)] {
            assert!(check_ref(good).is_ok(), "{good}");
        }
        for bad in ["", "-x", "a..b", "a b", "a@{1}", "x/", "--upload-pack=evil"] {
            assert!(check_ref(bad).is_err(), "{bad}");
        }
        assert_eq!(
            normalize_repo_path(Some("./skills/pdf/"))
                .unwrap()
                .as_deref(),
            Some("skills/pdf")
        );
        assert_eq!(normalize_repo_path(Some(".")).unwrap(), None);
        assert!(normalize_repo_path(Some("../x")).is_err());
        assert!(normalize_repo_path(Some("a/../../x")).is_err());
    }
}
