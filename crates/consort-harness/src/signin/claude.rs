//! Claude Code sign-in: `claude auth login` in a pseudo-terminal,
//! `claude auth status --json` and `claude auth logout`.
//!
//! `claude auth login` prints a sign-in URL, tries to open a browser, and
//! then waits for the user to paste a code. The worker runs it in a
//! pseudo-terminal (`portable-pty`, which uses ConPTY on Windows), because
//! the command reads the code from a terminal. `BROWSER` points to a program
//! that does nothing, so no browser opens on the worker: the user opens the
//! URL in the app. Tested with Claude Code 2.1.312 on macOS: it runs
//! `$BROWSER <url>`, then prints the URL and "Paste code here if prompted".

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::account::prepare_home;
use crate::process::resolve_program;
use crate::{AccountConfig, HarnessError};

/// How long `claude auth status` and `claude auth logout` may take.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// The most bytes of login output that the worker keeps.
const OUTPUT_MAX: usize = 64 * 1024;

/// What `claude auth status --json` reports. It has no credential.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClaudeAuthStatus {
    /// Whether someone is signed in.
    pub logged_in: bool,
    /// `claude.ai`, `api_key`, `none` or another method.
    pub auth_method: Option<String>,
    /// The signed-in email address.
    pub email: Option<String>,
    /// The subscription, such as `max` or `pro`.
    pub subscription_type: Option<String>,
    /// The organization name.
    pub org_name: Option<String>,
}

impl ClaudeAuthStatus {
    /// Reads the JSON object in the output of `claude auth status --json`.
    pub fn parse(output: &str) -> Option<Self> {
        let start = output.find('{')?;
        let end = output.rfind('}')?;
        let value: Value = serde_json::from_str(output.get(start..=end)?).ok()?;
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Some(Self {
            logged_in: value.get("loggedIn")?.as_bool()?,
            auth_method: text("authMethod"),
            email: text("email"),
            subscription_type: text("subscriptionType"),
            org_name: text("orgName"),
        })
    }

    /// The plan name for the inventory, such as "Max" or "API key".
    pub fn plan(&self) -> Option<String> {
        if let Some(subscription) = &self.subscription_type {
            return Some(crate::codex::capitalize(subscription));
        }
        match self.auth_method.as_deref() {
            Some("api_key") => Some("API key".to_owned()),
            Some("console") => Some("Console".to_owned()),
            _ => None,
        }
    }

    /// Who is signed in, for a sign-in that succeeded: the email and the
    /// plan, as far as Claude Code tells them.
    pub fn detail(&self) -> Option<String> {
        match (&self.email, self.plan()) {
            (Some(email), Some(plan)) => Some(format!("{email}, {plan} plan")),
            (Some(email), None) => Some(email.clone()),
            (None, Some(plan)) => Some(format!("{plan} plan")),
            (None, None) => None,
        }
    }
}

/// How to run `claude` for one account: the program, and the environment
/// with the account's home folder.
#[derive(Clone, PartialEq, Eq)]
pub struct ClaudeCommand {
    /// The `claude` program.
    pub program: PathBuf,
    /// Variables to set.
    pub env: BTreeMap<String, String>,
    /// Variables to remove.
    pub env_remove: Vec<String>,
}

impl std::fmt::Debug for ClaudeCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeCommand")
            .field("program", &self.program)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("env_remove", &self.env_remove)
            .finish()
    }
}

impl ClaudeCommand {
    /// The command for an account: the managed `claude` (from the
    /// `CLAUDE_CODE_EXECUTABLE` of the managed launch), else `claude` on
    /// `PATH`. The environment turns off the self-update.
    pub fn for_account(account: &AccountConfig) -> Result<Self, HarnessError> {
        let managed = account.managed_launch();
        let program = managed
            .and_then(|l| l.env.get("CLAUDE_CODE_EXECUTABLE"))
            .map(PathBuf::from)
            .or_else(|| resolve_program("claude"))
            .ok_or_else(|| HarnessError::Spawn("`claude` is not on PATH".to_owned()))?;
        let mut env = prepare_home(account)?;
        if let Some(launch) = managed {
            env.extend(launch.env.clone());
        }
        env.insert("DISABLE_AUTOUPDATER".to_owned(), "1".to_owned());
        Ok(Self {
            program,
            env,
            env_remove: account.runtime.env_remove.clone(),
        })
    }

    fn command(&self, args: &[&str]) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(args);
        for key in &self.env_remove {
            cmd.env_remove(key);
        }
        cmd.envs(&self.env);
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        cmd
    }

    /// Runs `claude auth status --json`. The command exits with 1 when
    /// nobody is signed in, so the exit code does not count: the JSON does.
    pub async fn status(&self) -> Result<ClaudeAuthStatus, HarnessError> {
        let output = tokio::time::timeout(
            COMMAND_TIMEOUT,
            self.command(&["auth", "status", "--json"]).output(),
        )
        .await
        .map_err(|_| HarnessError::Timeout("claude auth status".to_owned()))?
        .map_err(|e| HarnessError::Spawn(format!("{}: {e}", self.program.display())))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        ClaudeAuthStatus::parse(&stdout).ok_or_else(|| {
            HarnessError::Protocol(format!(
                "`claude auth status --json` gave no status ({})",
                output.status
            ))
        })
    }

    /// Runs `claude auth logout`.
    pub async fn logout(&self) -> Result<(), HarnessError> {
        let output =
            tokio::time::timeout(COMMAND_TIMEOUT, self.command(&["auth", "logout"]).output())
                .await
                .map_err(|_| HarnessError::Timeout("claude auth logout".to_owned()))?
                .map_err(|e| HarnessError::Spawn(format!("{}: {e}", self.program.display())))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(HarnessError::Protocol(format!(
                "`claude auth logout` failed: {}",
                last_line(&String::from_utf8_lossy(&output.stderr))
                    .or_else(|| last_line(&String::from_utf8_lossy(&output.stdout)))
                    .unwrap_or_else(|| output.status.to_string())
            )))
        }
    }
}

/// The sign-in state of an account from `claude auth status`, or `None`
/// when the command cannot run or gives no status.
pub async fn status_for(account: &AccountConfig) -> Option<ClaudeAuthStatus> {
    let command = ClaudeCommand::for_account(account).ok()?;
    match command.status().await {
        Ok(status) => Some(status),
        Err(err) => {
            tracing::debug!(error = %err, "claude auth status gave no answer; the probe uses ACP");
            None
        }
    }
}

/// Which `claude auth login` flow to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginKind {
    /// A Claude subscription (`--claudeai`).
    Plan,
    /// An Anthropic Console account (`--console`).
    Console,
}

/// A program that does nothing with its arguments, for `BROWSER`. Claude
/// Code runs `$BROWSER <url>`, so no browser opens on the worker.
pub fn no_browser_program() -> String {
    #[cfg(windows)]
    {
        // `where.exe` prints that it found nothing and exits. Not verified
        // with Claude Code on Windows (see docs/DECISIONS.md).
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned());
        format!("{root}\\System32\\where.exe")
    }
    #[cfg(not(windows))]
    {
        resolve_program("true")
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/usr/bin/true".to_owned())
    }
}

/// Removes terminal control sequences: CSI (`ESC [ ... final`), OSC
/// (`ESC ] ... BEL` or `ESC ] ... ESC \`) and other two-byte escapes.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if c == '\u{7}' {
                continue;
            }
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The first `https://` URL in the text, without control sequences. A URL
/// ends at white space.
pub fn find_url(text: &str) -> Option<String> {
    let clean = strip_ansi(text);
    let start = clean.find("https://")?;
    let url: String = clean[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && !c.is_control())
        .collect();
    // A URL that the output has not finished yet has no white space after
    // it. Wait for the rest.
    let complete = clean[start + url.len()..].chars().next().is_some();
    (complete && url.len() > "https://".len()).then_some(url)
}

/// The last line of the text that has words, without control sequences.
pub fn last_line(text: &str) -> Option<String> {
    strip_ansi(text)
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_owned)
}

/// A running `claude auth login` in a pseudo-terminal.
///
/// Dropping it kills the process.
pub struct ClaudeLogin {
    writer: Box<dyn Write + Send>,
    /// Asks the waiter thread to kill the process.
    kill: Arc<AtomicBool>,
    output: mpsc::UnboundedReceiver<Vec<u8>>,
    exit: Option<oneshot::Receiver<bool>>,
    exited: Option<bool>,
    text: String,
    /// The code that was typed. The terminal echoes it, so lines that hold
    /// it never go into a message.
    typed: Option<String>,
    // Keeps the pseudo-terminal open while the login runs.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl std::fmt::Debug for ClaudeLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeLogin").finish_non_exhaustive()
    }
}

impl ClaudeLogin {
    /// Starts `claude auth login` in a pseudo-terminal in `cwd`.
    pub fn start(
        command: &ClaudeCommand,
        kind: LoginKind,
        cwd: &Path,
    ) -> Result<Self, HarnessError> {
        let flag = match kind {
            LoginKind::Plan => "--claudeai",
            LoginKind::Console => "--console",
        };
        Self::spawn(&command.program, &["auth", "login", flag], command, cwd)
    }

    /// Starts any program in a pseudo-terminal with the environment of
    /// `command`. Tests run a fake `claude` with it.
    pub fn spawn(
        program: &Path,
        args: &[&str],
        command: &ClaudeCommand,
        cwd: &Path,
    ) -> Result<Self, HarnessError> {
        let spawn_err = |e: &dyn std::fmt::Display| HarnessError::Spawn(format!("{e}"));
        let pty = native_pty_system();
        // A wide terminal, so the long URL is not wrapped.
        let pair = pty
            .openpty(PtySize {
                rows: 40,
                cols: 2000,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| spawn_err(&e))?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        for key in &command.env_remove {
            cmd.env_remove(key);
        }
        for (key, value) in &command.env {
            cmd.env(key, value);
        }
        cmd.env("BROWSER", no_browser_program());
        if std::env::var_os("TERM").is_none() {
            cmd.env("TERM", "xterm-256color");
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| spawn_err(&e))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| spawn_err(&e))?;
        let writer = pair.master.take_writer().map_err(|e| spawn_err(&e))?;

        let (out_tx, output) = mpsc::unbounded_channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if out_tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let (exit_tx, exit) = oneshot::channel();
        let kill = Arc::new(AtomicBool::new(false));
        let kill_flag = kill.clone();
        std::thread::spawn(move || {
            // Poll, so a kill can come between two checks. `Child::kill`
            // sends SIGHUP on Unix, waits a moment, and then kills.
            let success = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status.success(),
                    Ok(None) => {}
                    Err(_) => break false,
                }
                if kill_flag.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    break child.wait().map(|s| s.success()).unwrap_or(false);
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            let _ = exit_tx.send(success);
        });
        Ok(Self {
            writer,
            kill,
            output,
            exit: Some(exit),
            exited: None,
            text: String::new(),
            typed: None,
            _master: pair.master,
        })
    }

    fn take_output(&mut self, bytes: &[u8]) {
        self.text.push_str(&String::from_utf8_lossy(bytes));
        if self.text.len() > OUTPUT_MAX {
            let mut cut = self.text.len() - OUTPUT_MAX;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
        }
    }

    /// Waits until the output has the sign-in URL.
    pub async fn url(&mut self, limit: Duration) -> Result<String, HarnessError> {
        let wait = async {
            loop {
                if let Some(url) = find_url(&self.text) {
                    return Ok(url);
                }
                match self.output.recv().await {
                    Some(bytes) => self.take_output(&bytes),
                    None => {
                        return Err(HarnessError::Protocol(match self.last_line() {
                            Some(line) => format!("`claude auth login` ended: {line}"),
                            None => "`claude auth login` ended before it showed a URL".to_owned(),
                        }));
                    }
                }
            }
        };
        tokio::time::timeout(limit, wait).await.map_err(|_| {
            HarnessError::Timeout("`claude auth login` did not show a sign-in URL".to_owned())
        })?
    }

    /// Types the code and Enter. The code is not kept or logged.
    pub fn send_code(&mut self, code: &str) -> Result<(), HarnessError> {
        let code = code.trim();
        self.typed = Some(code.to_owned());
        self.writer
            .write_all(code.as_bytes())
            .and_then(|()| self.writer.write_all(b"\r"))
            .and_then(|()| self.writer.flush())
            .map_err(|e| HarnessError::Protocol(format!("could not type the code: {e}")))
    }

    /// Waits for the process to exit, and reads its output meanwhile.
    /// Returns whether it exited with code 0.
    pub async fn wait(&mut self, limit: Duration) -> Result<bool, HarnessError> {
        if let Some(done) = self.exited {
            return Ok(done);
        }
        let Some(mut exit) = self.exit.take() else {
            return Ok(false);
        };
        let wait = async {
            loop {
                tokio::select! {
                    done = &mut exit => {
                        // Read what is left of the output.
                        while let Ok(bytes) = self.output.try_recv() {
                            self.take_output(&bytes);
                        }
                        return done.unwrap_or(false);
                    }
                    bytes = self.output.recv() => match bytes {
                        Some(bytes) => self.take_output(&bytes),
                        None => {
                            return (&mut exit).await.unwrap_or(false);
                        }
                    }
                }
            }
        };
        match tokio::time::timeout(limit, wait).await {
            Ok(done) => {
                self.exited = Some(done);
                Ok(done)
            }
            Err(_) => Err(HarnessError::Timeout(
                "`claude auth login` did not finish".to_owned(),
            )),
        }
    }

    /// The last line of the output with words, such as "Login failed: ...".
    /// A line that holds the typed code is skipped.
    pub fn last_line(&self) -> Option<String> {
        let clean = strip_ansi(&self.text);
        clean
            .lines()
            .map(str::trim)
            .rfind(|l| {
                !l.is_empty()
                    && self
                        .typed
                        .as_deref()
                        .is_none_or(|code| code.is_empty() || !l.contains(code))
            })
            .map(str::to_owned)
    }

    /// Stops the process.
    pub fn kill(&mut self) {
        if self.exited.is_none() {
            self.kill.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for ClaudeLogin {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_json_reads_the_sign_in_fields() {
        let out = r#"{
  "loggedIn": true,
  "authMethod": "claude.ai",
  "apiProvider": "firstParty",
  "email": "a@example.com",
  "orgName": "A",
  "subscriptionType": "max"
}"#;
        let status = ClaudeAuthStatus::parse(out).unwrap();
        assert!(status.logged_in);
        assert_eq!(status.email.as_deref(), Some("a@example.com"));
        assert_eq!(status.plan().as_deref(), Some("Max"));
        assert_eq!(status.detail().as_deref(), Some("a@example.com, Max plan"));
        // The real output for nobody signed in (Claude Code 2.1.312).
        let none = ClaudeAuthStatus::parse(
            r#"{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"}"#,
        )
        .unwrap();
        assert!(!none.logged_in);
        assert_eq!(none.plan(), None);
        let key = ClaudeAuthStatus {
            logged_in: true,
            auth_method: Some("api_key".into()),
            ..Default::default()
        };
        assert_eq!(key.plan().as_deref(), Some("API key"));
        assert!(ClaudeAuthStatus::parse("not json").is_none());
    }

    #[test]
    fn the_url_comes_out_of_the_terminal_codes() {
        // The shape of Claude Code 2.1.312's output, with a shorter URL.
        let out = "Opening browser to sign in\u{2026}\r\nIf the browser didn't open, visit: \u{1b}]8;;https://claude.com/cai/oauth/authorize?code=true&state=x\u{7}\u{1b}[94mhttps://claude.com/cai/oauth/authorize?code=true&state=x\u{1b}[39m\u{1b}]8;;\u{7}\r\nPaste code here if prompted > ";
        assert_eq!(
            find_url(out).as_deref(),
            Some("https://claude.com/cai/oauth/authorize?code=true&state=x")
        );
        // A URL that is not complete yet is not taken.
        assert_eq!(find_url("visit: https://claude.com/cai/oau"), None);
        assert_eq!(
            last_line("x\r\n\u{1b}[31mLogin failed: bad code\u{1b}[0m\r\n\u{1b}[?25h").as_deref(),
            Some("Login failed: bad code")
        );
    }
}
