//! The per-user service that starts the worker at login and restarts it when
//! it fails: a launchd agent on macOS, a systemd user unit on Linux and a
//! Task Scheduler task on Windows.
//!
//! The code has two layers. The first layer makes a [`Plan`] from plain
//! strings: the files to write, with their text, and the commands to run. It
//! does no I/O, so the tests check the text for every platform on every
//! platform. The second layer, [`apply`], writes the files and runs the
//! commands through a [`Runner`]. `--dry-run` prints the plan and does not
//! call the second layer.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The launchd label, and the base name of the plist.
pub const LAUNCHD_LABEL: &str = "dev.consort.worker";
/// The systemd user unit.
pub const SYSTEMD_UNIT: &str = "consort-worker.service";
/// The name of the Task Scheduler task.
pub const TASK_NAME: &str = "Consort worker";
/// The name of the log file on every system.
pub const LOG_FILE_NAME: &str = "worker.log";

/// The systems that have a service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
}

impl Platform {
    /// The system that runs this program, or `None` for an unsupported one.
    pub fn current() -> Option<Platform> {
        match std::env::consts::OS {
            "macos" => Some(Platform::MacOs),
            "linux" => Some(Platform::Linux),
            "windows" => Some(Platform::Windows),
            _ => None,
        }
    }

    /// The name of the service manager, for messages.
    pub fn manager(self) -> &'static str {
        match self {
            Platform::MacOs => "a launchd agent",
            Platform::Linux => "a systemd user unit",
            Platform::Windows => "a Task Scheduler task",
        }
    }
}

/// What the plan needs to know about the machine and the user. Every path is
/// a string in the style of the target system, so a test can plan for
/// Windows on a Mac.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceEnv {
    /// The full path of the `consort` program.
    pub exe: String,
    /// The home folder of the user.
    pub home: String,
    /// The folder for the unit file on Linux, `~/.config` by default.
    pub xdg_config_home: Option<String>,
    /// The folder for the log file on Linux, `~/.local/state` by default.
    pub xdg_state_home: Option<String>,
    /// `%LOCALAPPDATA%` on Windows.
    pub local_app_data: Option<String>,
    /// `DOMAIN\user` on Windows.
    pub windows_user: Option<String>,
    /// The user ID on macOS, for `launchctl`.
    pub uid: Option<String>,
    /// The `PATH` that the service gets. The service managers start the
    /// worker with a short `PATH`, and the harnesses need the user's own.
    pub path: Option<String>,
    /// The worker folder, when it is not the default.
    pub config_dir: Option<String>,
}

impl ServiceEnv {
    /// The values of the running system.
    pub fn detect(exe: &Path, config_dir: Option<&Path>) -> Result<ServiceEnv> {
        let home = directories::BaseDirs::new()
            .map(|d| d.home_dir().to_string_lossy().into_owned())
            .context("could not find the home folder")?;
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let windows_user = match (var("USERDOMAIN"), var("USERNAME")) {
            (Some(domain), Some(user)) => Some(format!("{domain}\\{user}")),
            (None, Some(user)) => Some(user),
            _ => None,
        };
        let uid = if cfg!(target_os = "macos") {
            let out = std::process::Command::new("id")
                .arg("-u")
                .output()
                .context("could not run `id -u`")?;
            Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        } else {
            None
        };
        Ok(ServiceEnv {
            exe: exe.to_string_lossy().into_owned(),
            home,
            xdg_config_home: var("XDG_CONFIG_HOME"),
            xdg_state_home: var("XDG_STATE_HOME"),
            local_app_data: var("LOCALAPPDATA"),
            windows_user,
            uid,
            path: var("PATH"),
            config_dir: config_dir.map(|d| d.to_string_lossy().into_owned()),
        })
    }
}

/// How a file is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    /// UTF-16 little endian with a byte order mark. `schtasks /XML` needs it.
    Utf16Le,
}

/// A file that the install writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedFile {
    pub path: String,
    pub text: String,
    pub encoding: Encoding,
}

/// A command that the install runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedCommand {
    pub program: String,
    pub args: Vec<String>,
    /// A failure does not stop the install. For example, `launchctl bootout`
    /// fails when the agent is not loaded yet.
    pub may_fail: bool,
}

impl PlannedCommand {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            may_fail: false,
        }
    }

    fn may_fail(mut self) -> Self {
        self.may_fail = true;
        self
    }

    /// The command as one line, for the dry run.
    pub fn line(&self) -> String {
        let mut line = self.program.clone();
        for arg in &self.args {
            line.push(' ');
            if arg.is_empty() || arg.contains([' ', '"', '\'']) {
                let _ = write!(line, "\"{}\"", arg.replace('"', "\\\""));
            } else {
                line.push_str(arg);
            }
        }
        line
    }
}

/// What an install, an uninstall or a status does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub platform: Platform,
    /// The folders to make before the files are written.
    pub dirs: Vec<String>,
    pub files: Vec<PlannedFile>,
    /// The files to remove.
    pub remove: Vec<String>,
    pub commands: Vec<PlannedCommand>,
    /// The log file that the service writes.
    pub log_file: String,
}

/// The paths of one install.
struct Paths {
    /// The file that tells the service manager what to run.
    definition: String,
    log_file: String,
    /// The folder of the definition and the folder of the log file.
    dirs: Vec<String>,
}

fn paths(platform: Platform, env: &ServiceEnv) -> Result<Paths> {
    Ok(match platform {
        Platform::MacOs => {
            let log_dir = format!("{}/Library/Logs/Consort", env.home);
            let agents = format!("{}/Library/LaunchAgents", env.home);
            Paths {
                definition: format!("{agents}/{LAUNCHD_LABEL}.plist"),
                log_file: format!("{log_dir}/{LOG_FILE_NAME}"),
                dirs: vec![agents, log_dir],
            }
        }
        Platform::Linux => {
            let config = env
                .xdg_config_home
                .clone()
                .unwrap_or_else(|| format!("{}/.config", env.home));
            let state = env
                .xdg_state_home
                .clone()
                .unwrap_or_else(|| format!("{}/.local/state", env.home));
            let units = format!("{config}/systemd/user");
            let log_dir = format!("{state}/consort");
            Paths {
                definition: format!("{units}/{SYSTEMD_UNIT}"),
                log_file: format!("{log_dir}/{LOG_FILE_NAME}"),
                dirs: vec![units, log_dir],
            }
        }
        Platform::Windows => {
            let Some(base) = env.local_app_data.clone() else {
                bail!("LOCALAPPDATA is not set, so the task and its log have no folder");
            };
            let root = format!("{base}\\Consort");
            let logs = format!("{root}\\logs");
            Paths {
                definition: format!("{root}\\worker-task.xml"),
                log_file: format!("{logs}\\{LOG_FILE_NAME}"),
                dirs: vec![root, logs],
            }
        }
    })
}

/// The arguments of `consort worker run` for the service.
fn run_args(env: &ServiceEnv, log_file: &str) -> Vec<String> {
    let mut args = vec!["worker".to_owned(), "run".to_owned()];
    if let Some(dir) = &env.config_dir {
        args.push("--config-dir".to_owned());
        args.push(dir.clone());
    }
    args.push("--log-file".to_owned());
    args.push(log_file.to_owned());
    args
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The text of the launchd plist.
pub fn launchd_plist(env: &ServiceEnv, log_file: &str) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n");
    out.push_str("<plist version=\"1.0\">\n<dict>\n");
    let _ = writeln!(
        out,
        "  <key>Label</key>\n  <string>{LAUNCHD_LABEL}</string>"
    );
    out.push_str("  <key>ProgramArguments</key>\n  <array>\n");
    let _ = writeln!(out, "    <string>{}</string>", xml_escape(&env.exe));
    for arg in run_args(env, log_file) {
        let _ = writeln!(out, "    <string>{}</string>", xml_escape(&arg));
    }
    out.push_str("  </array>\n");
    // Start at login, and again after a crash or any exit with an error. A
    // clean exit, for example `launchctl bootout`, stays stopped.
    out.push_str("  <key>RunAtLoad</key>\n  <true/>\n");
    out.push_str("  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n");
    out.push_str("  <key>ThrottleInterval</key>\n  <integer>30</integer>\n");
    out.push_str("  <key>ProcessType</key>\n  <string>Background</string>\n");
    if let Some(path) = &env.path {
        let _ = writeln!(
            out,
            "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    <string>{}</string>\n  </dict>",
            xml_escape(path)
        );
    }
    let _ = writeln!(
        out,
        "  <key>StandardOutPath</key>\n  <string>{0}</string>\n  <key>StandardErrorPath</key>\n  <string>{0}</string>",
        xml_escape(log_file)
    );
    out.push_str("</dict>\n</plist>\n");
    out
}

/// A word in a systemd `Exec` line. systemd splits the line at spaces, so a
/// word with a space, a quote or a backslash gets double quotes. A `%` is
/// always written as `%%`, because systemd expands specifiers.
fn systemd_word(word: &str) -> String {
    let word = word.replace('%', "%%");
    if word.is_empty() || word.contains([' ', '\t', '"', '\'', '\\']) {
        let escaped = word.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        word
    }
}

/// The text of the systemd user unit.
pub fn systemd_unit(env: &ServiceEnv, log_file: &str) -> String {
    let mut exec = systemd_word(&env.exe);
    for arg in run_args(env, log_file) {
        exec.push(' ');
        exec.push_str(&systemd_word(&arg));
    }
    let mut out = String::new();
    out.push_str("[Unit]\nDescription=Consort worker\nAfter=network-online.target\nWants=network-online.target\n\n");
    out.push_str("[Service]\nType=simple\n");
    let _ = writeln!(out, "ExecStart={exec}");
    // Restart after a crash or an exit with an error, not after a clean stop.
    out.push_str("Restart=on-failure\nRestartSec=10\n");
    // SIGTERM goes to the worker only. The worker stops its harnesses itself.
    out.push_str("KillMode=mixed\nTimeoutStopSec=30\n");
    if let Some(path) = &env.path {
        let _ = writeln!(out, "Environment={}", systemd_word(&format!("PATH={path}")));
    }
    let _ = writeln!(out, "StandardOutput=append:{log_file}");
    let _ = writeln!(out, "StandardError=append:{log_file}");
    out.push_str("\n[Install]\nWantedBy=default.target\n");
    out
}

/// Single quotes for a PowerShell string.
fn powershell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The task definition for `schtasks /Create /XML`.
pub fn task_xml(env: &ServiceEnv, log_file: &str) -> String {
    // A console program shows a window when it starts at logon. PowerShell
    // with a hidden window starts it without one and passes the exit code
    // on, so a failed worker still counts as a failure for the restart rule.
    let mut command = format!("& {}", powershell_quote(&env.exe));
    for arg in run_args(env, log_file) {
        command.push(' ');
        command.push_str(&powershell_quote(&arg));
    }
    command.push_str("; exit $LASTEXITCODE");
    let arguments =
        format!("-NoProfile -NonInteractive -WindowStyle Hidden -Command \"{command}\"");
    let user = env
        .windows_user
        .as_deref()
        .map(|u| format!("      <UserId>{}</UserId>\n", xml_escape(u)))
        .unwrap_or_default();
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n");
    out.push_str(
        "<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n",
    );
    out.push_str("  <RegistrationInfo>\n    <Description>Starts the Consort worker when you sign in, and starts it again after a failure.</Description>\n  </RegistrationInfo>\n");
    let _ = write!(
        out,
        "  <Triggers>\n    <LogonTrigger>\n      <Enabled>true</Enabled>\n{user}    </LogonTrigger>\n  </Triggers>\n"
    );
    let principal_user = env
        .windows_user
        .as_deref()
        .map(|u| format!("      <UserId>{}</UserId>\n", xml_escape(u)))
        .unwrap_or_default();
    let _ = write!(
        out,
        "  <Principals>\n    <Principal id=\"Author\">\n{principal_user}      <LogonType>InteractiveToken</LogonType>\n      <RunLevel>LeastPrivilege</RunLevel>\n    </Principal>\n  </Principals>\n"
    );
    out.push_str("  <Settings>\n");
    out.push_str("    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n");
    out.push_str("    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n");
    out.push_str("    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n");
    out.push_str("    <StartWhenAvailable>true</StartWhenAvailable>\n");
    out.push_str("    <AllowStartOnDemand>true</AllowStartOnDemand>\n");
    out.push_str("    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\n");
    out.push_str("    <Hidden>false</Hidden>\n");
    out.push_str("    <Enabled>true</Enabled>\n");
    out.push_str("    <RestartOnFailure>\n      <Interval>PT1M</Interval>\n      <Count>999</Count>\n    </RestartOnFailure>\n");
    out.push_str("  </Settings>\n");
    let _ = write!(
        out,
        "  <Actions Context=\"Author\">\n    <Exec>\n      <Command>powershell.exe</Command>\n      <Arguments>{}</Arguments>\n    </Exec>\n  </Actions>\n",
        xml_escape(&arguments)
    );
    out.push_str("</Task>\n");
    out
}

/// The plan for `consort worker install`.
pub fn plan_install(platform: Platform, env: &ServiceEnv) -> Result<Plan> {
    let p = paths(platform, env)?;
    let (text, encoding) = match platform {
        Platform::MacOs => (launchd_plist(env, &p.log_file), Encoding::Utf8),
        Platform::Linux => (systemd_unit(env, &p.log_file), Encoding::Utf8),
        Platform::Windows => (task_xml(env, &p.log_file), Encoding::Utf16Le),
    };
    let commands = match platform {
        Platform::MacOs => {
            let domain = launchd_domain(env)?;
            vec![
                // A second install replaces the first one.
                PlannedCommand::new(
                    "launchctl",
                    &["bootout", &format!("{domain}/{LAUNCHD_LABEL}")],
                )
                .may_fail(),
                PlannedCommand::new("launchctl", &["bootstrap", &domain, &p.definition]),
            ]
        }
        Platform::Linux => vec![
            PlannedCommand::new("systemctl", &["--user", "daemon-reload"]),
            PlannedCommand::new("systemctl", &["--user", "enable", SYSTEMD_UNIT]),
            PlannedCommand::new("systemctl", &["--user", "restart", SYSTEMD_UNIT]),
        ],
        Platform::Windows => vec![
            PlannedCommand::new(
                "schtasks",
                &["/Create", "/TN", TASK_NAME, "/XML", &p.definition, "/F"],
            ),
            PlannedCommand::new("schtasks", &["/Run", "/TN", TASK_NAME]),
        ],
    };
    Ok(Plan {
        platform,
        dirs: p.dirs,
        files: vec![PlannedFile {
            path: p.definition,
            text,
            encoding,
        }],
        remove: Vec::new(),
        commands,
        log_file: p.log_file,
    })
}

/// The plan for `consort worker uninstall`. It keeps the log file and the
/// worker folder.
pub fn plan_uninstall(platform: Platform, env: &ServiceEnv) -> Result<Plan> {
    let p = paths(platform, env)?;
    let commands = match platform {
        Platform::MacOs => {
            let domain = launchd_domain(env)?;
            vec![
                PlannedCommand::new(
                    "launchctl",
                    &["bootout", &format!("{domain}/{LAUNCHD_LABEL}")],
                )
                .may_fail(),
            ]
        }
        Platform::Linux => vec![
            PlannedCommand::new("systemctl", &["--user", "disable", "--now", SYSTEMD_UNIT])
                .may_fail(),
            PlannedCommand::new("systemctl", &["--user", "daemon-reload"]).may_fail(),
        ],
        Platform::Windows => vec![
            PlannedCommand::new("schtasks", &["/End", "/TN", TASK_NAME]).may_fail(),
            PlannedCommand::new("schtasks", &["/Delete", "/TN", TASK_NAME, "/F"]).may_fail(),
        ],
    };
    Ok(Plan {
        platform,
        dirs: Vec::new(),
        files: Vec::new(),
        remove: vec![p.definition],
        commands,
        log_file: p.log_file,
    })
}

/// The commands that show the state of the service.
pub fn plan_status(platform: Platform, env: &ServiceEnv) -> Result<Plan> {
    let p = paths(platform, env)?;
    let commands = match platform {
        Platform::MacOs => {
            let domain = launchd_domain(env)?;
            vec![PlannedCommand::new(
                "launchctl",
                &["print", &format!("{domain}/{LAUNCHD_LABEL}")],
            )]
        }
        Platform::Linux => vec![
            PlannedCommand::new("systemctl", &["--user", "is-active", SYSTEMD_UNIT]),
            PlannedCommand::new("systemctl", &["--user", "is-enabled", SYSTEMD_UNIT]),
        ],
        Platform::Windows => vec![PlannedCommand::new(
            "schtasks",
            &["/Query", "/TN", TASK_NAME, "/FO", "LIST", "/V"],
        )],
    };
    Ok(Plan {
        platform,
        dirs: Vec::new(),
        files: Vec::new(),
        remove: Vec::new(),
        commands,
        log_file: p.log_file,
    })
}

fn launchd_domain(env: &ServiceEnv) -> Result<String> {
    match &env.uid {
        Some(uid) if !uid.is_empty() => Ok(format!("gui/{uid}")),
        _ => bail!("could not find the user ID for launchctl"),
    }
}

/// The path of the definition file, to see whether the service is installed.
pub fn definition_path(platform: Platform, env: &ServiceEnv) -> Result<String> {
    Ok(paths(platform, env)?.definition)
}

/// The text of a dry run: what the install would write and run.
pub fn describe(plan: &Plan, verb: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Dry run. Consort would {verb} {}.",
        plan.platform.manager()
    );
    for dir in &plan.dirs {
        let _ = writeln!(out, "\nMake the folder {dir}");
    }
    for file in &plan.files {
        let _ = writeln!(out, "\nWrite {}:\n", file.path);
        for line in file.text.lines() {
            let _ = writeln!(out, "    {line}");
        }
    }
    for path in &plan.remove {
        let _ = writeln!(out, "\nRemove {path}");
    }
    if !plan.commands.is_empty() {
        out.push_str("\nRun:\n");
        for command in &plan.commands {
            let _ = writeln!(
                out,
                "    {}{}",
                command.line(),
                if command.may_fail {
                    "  (a failure is fine)"
                } else {
                    ""
                }
            );
        }
    }
    let _ = writeln!(out, "\nThe worker writes its log to {}", plan.log_file);
    out
}

/// What a command printed and how it ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub success: bool,
    pub output: String,
}

/// Runs the commands of a plan. The tests use a recording runner, so no test
/// touches a service manager.
pub trait Runner {
    fn run(&mut self, command: &PlannedCommand) -> Result<Outcome>;
}

/// Runs commands on this machine.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&mut self, command: &PlannedCommand) -> Result<Outcome> {
        let out = std::process::Command::new(&command.program)
            .args(&command.args)
            .output()
            .with_context(|| format!("could not run `{}`", command.line()))?;
        let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
        output.push_str(&String::from_utf8_lossy(&out.stderr));
        Ok(Outcome {
            success: out.status.success(),
            output,
        })
    }
}

fn encode(file: &PlannedFile) -> Vec<u8> {
    match file.encoding {
        Encoding::Utf8 => file.text.clone().into_bytes(),
        Encoding::Utf16Le => {
            let mut bytes = vec![0xFF, 0xFE];
            for unit in file.text.encode_utf16() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            bytes
        }
    }
}

/// Writes the files and runs the commands of a plan. Returns the output of
/// the commands, one entry for each.
pub fn apply(plan: &Plan, runner: &mut dyn Runner) -> Result<Vec<(PlannedCommand, Outcome)>> {
    for dir in &plan.dirs {
        std::fs::create_dir_all(dir).with_context(|| format!("could not make the folder {dir}"))?;
    }
    for file in &plan.files {
        let path = PathBuf::from(&file.path);
        std::fs::write(&path, encode(file))
            .with_context(|| format!("could not write {}", file.path))?;
    }
    for path in &plan.remove {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("could not remove {path}")),
        }
    }
    let mut results = Vec::new();
    for command in &plan.commands {
        let outcome = runner.run(command)?;
        if !outcome.success && !command.may_fail {
            bail!(
                "`{}` failed: {}",
                command.line(),
                outcome.output.trim().lines().next().unwrap_or("no message")
            );
        }
        results.push((command.clone(), outcome));
    }
    Ok(results)
}

/// A line that says whether the service runs, from the output of the status
/// commands.
pub fn status_summary(platform: Platform, results: &[(PlannedCommand, Outcome)]) -> String {
    let first = results.first().map(|(_, o)| o);
    match platform {
        Platform::MacOs => match first {
            Some(o) if o.success => {
                let pid = o
                    .output
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("pid = "))
                    .map(str::trim);
                let state = o
                    .output
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("state = "))
                    .map(str::trim)
                    .unwrap_or("unknown");
                match pid {
                    Some(pid) => {
                        format!("The launchd agent is loaded and {state} (process {pid}).")
                    }
                    None => format!("The launchd agent is loaded and {state}."),
                }
            }
            _ => "The launchd agent is not loaded.".to_owned(),
        },
        Platform::Linux => {
            let active = first.map(|o| o.output.trim()).unwrap_or("unknown");
            let enabled = results
                .get(1)
                .map(|(_, o)| o.output.trim())
                .unwrap_or("unknown");
            format!("The systemd user unit is {active} and {enabled}.")
        }
        Platform::Windows => match first {
            Some(o) if o.success => {
                let status = o
                    .output
                    .lines()
                    .find_map(|l| l.strip_prefix("Status:"))
                    .map(str::trim)
                    .unwrap_or("unknown");
                format!("The Task Scheduler task exists. Its status is {status}.")
            }
            _ => "The Task Scheduler task does not exist.".to_owned(),
        },
    }
}

/// Moves a log file that is larger than `limit` bytes to `<name>.1`, so the
/// log stays small without a log rotation service.
pub fn rotate_log(path: &Path, limit: u64) -> std::io::Result<()> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > limit => {
            let mut old = path.as_os_str().to_owned();
            old.push(".1");
            std::fs::rename(path, old)
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(platform: Platform) -> ServiceEnv {
        match platform {
            Platform::MacOs => ServiceEnv {
                exe: "/Users/ann/.local/bin/consort".into(),
                home: "/Users/ann".into(),
                xdg_config_home: None,
                xdg_state_home: None,
                local_app_data: None,
                windows_user: None,
                uid: Some("501".into()),
                path: Some("/opt/homebrew/bin:/usr/bin:/bin".into()),
                config_dir: None,
            },
            Platform::Linux => ServiceEnv {
                exe: "/home/ann/.local/bin/consort".into(),
                home: "/home/ann".into(),
                xdg_config_home: None,
                xdg_state_home: None,
                local_app_data: None,
                windows_user: None,
                uid: None,
                path: Some("/home/ann/.local/bin:/usr/bin".into()),
                config_dir: None,
            },
            Platform::Windows => ServiceEnv {
                exe: "C:\\Users\\Ann Lee\\AppData\\Local\\Consort\\bin\\consort.exe".into(),
                home: "C:\\Users\\Ann Lee".into(),
                xdg_config_home: None,
                xdg_state_home: None,
                local_app_data: Some("C:\\Users\\Ann Lee\\AppData\\Local".into()),
                windows_user: Some("BIGBOI\\Ann Lee".into()),
                uid: None,
                path: None,
                config_dir: None,
            },
        }
    }

    #[test]
    fn the_launchd_plist_starts_at_login_and_restarts_after_a_failure() {
        let plan = plan_install(Platform::MacOs, &env(Platform::MacOs)).unwrap();
        assert_eq!(plan.files.len(), 1);
        let file = &plan.files[0];
        assert_eq!(
            file.path,
            "/Users/ann/Library/LaunchAgents/dev.consort.worker.plist"
        );
        assert_eq!(plan.log_file, "/Users/ann/Library/Logs/Consort/worker.log");
        let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.consort.worker</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Users/ann/.local/bin/consort</string>
    <string>worker</string>
    <string>run</string>
    <string>--log-file</string>
    <string>/Users/ann/Library/Logs/Consort/worker.log</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>30</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/opt/homebrew/bin:/usr/bin:/bin</string>
  </dict>
  <key>StandardOutPath</key>
  <string>/Users/ann/Library/Logs/Consort/worker.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/ann/Library/Logs/Consort/worker.log</string>
</dict>
</plist>
"#;
        assert_eq!(file.text, expected);
        let lines: Vec<_> = plan.commands.iter().map(|c| c.line()).collect();
        assert_eq!(
            lines,
            [
                "launchctl bootout gui/501/dev.consort.worker",
                "launchctl bootstrap gui/501 /Users/ann/Library/LaunchAgents/dev.consort.worker.plist",
            ]
        );
        assert!(
            plan.commands[0].may_fail,
            "bootout fails on a first install"
        );
        assert!(!plan.commands[1].may_fail);
    }

    #[test]
    fn the_plist_escapes_xml_and_keeps_a_custom_worker_folder() {
        let mut e = env(Platform::MacOs);
        e.config_dir = Some("/Users/ann/R&D <x>".into());
        let text = launchd_plist(&e, "/tmp/w.log");
        assert!(text.contains("<string>--config-dir</string>"));
        assert!(text.contains("<string>/Users/ann/R&amp;D &lt;x&gt;</string>"));
    }

    #[test]
    fn the_systemd_unit_restarts_on_failure_and_logs_to_a_file() {
        let plan = plan_install(Platform::Linux, &env(Platform::Linux)).unwrap();
        let file = &plan.files[0];
        assert_eq!(
            file.path,
            "/home/ann/.config/systemd/user/consort-worker.service"
        );
        assert_eq!(plan.log_file, "/home/ann/.local/state/consort/worker.log");
        let expected = "[Unit]
Description=Consort worker
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/home/ann/.local/bin/consort worker run --log-file /home/ann/.local/state/consort/worker.log
Restart=on-failure
RestartSec=10
KillMode=mixed
TimeoutStopSec=30
Environment=PATH=/home/ann/.local/bin:/usr/bin
StandardOutput=append:/home/ann/.local/state/consort/worker.log
StandardError=append:/home/ann/.local/state/consort/worker.log

[Install]
WantedBy=default.target
";
        assert_eq!(file.text, expected);
        let lines: Vec<_> = plan.commands.iter().map(|c| c.line()).collect();
        assert_eq!(
            lines,
            [
                "systemctl --user daemon-reload",
                "systemctl --user enable consort-worker.service",
                "systemctl --user restart consort-worker.service",
            ]
        );
    }

    #[test]
    fn the_systemd_unit_follows_the_xdg_folders_and_quotes_odd_paths() {
        let mut e = env(Platform::Linux);
        e.xdg_config_home = Some("/cfg".into());
        e.xdg_state_home = Some("/state".into());
        e.exe = "/opt/my tools/consort".into();
        e.config_dir = Some("/data/50%/worker".into());
        let plan = plan_install(Platform::Linux, &e).unwrap();
        assert_eq!(
            plan.files[0].path,
            "/cfg/systemd/user/consort-worker.service"
        );
        assert_eq!(plan.log_file, "/state/consort/worker.log");
        assert!(plan.files[0].text.contains(
            "ExecStart=\"/opt/my tools/consort\" worker run --config-dir /data/50%%/worker --log-file /state/consort/worker.log\n"
        ));
    }

    #[test]
    fn the_windows_task_runs_at_logon_and_restarts_on_failure() {
        let plan = plan_install(Platform::Windows, &env(Platform::Windows)).unwrap();
        let file = &plan.files[0];
        assert_eq!(
            file.path,
            "C:\\Users\\Ann Lee\\AppData\\Local\\Consort\\worker-task.xml"
        );
        assert_eq!(file.encoding, Encoding::Utf16Le);
        assert_eq!(
            plan.log_file,
            "C:\\Users\\Ann Lee\\AppData\\Local\\Consort\\logs\\worker.log"
        );
        let expected = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts the Consort worker when you sign in, and starts it again after a failure.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>BIGBOI\Ann Lee</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>BIGBOI\Ann Lee</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Hidden>false</Hidden>
    <Enabled>true</Enabled>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>999</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>powershell.exe</Command>
      <Arguments>-NoProfile -NonInteractive -WindowStyle Hidden -Command &quot;&amp; 'C:\Users\Ann Lee\AppData\Local\Consort\bin\consort.exe' 'worker' 'run' '--log-file' 'C:\Users\Ann Lee\AppData\Local\Consort\logs\worker.log'; exit $LASTEXITCODE&quot;</Arguments>
    </Exec>
  </Actions>
</Task>
"#;
        assert_eq!(file.text, expected);
        let lines: Vec<_> = plan.commands.iter().map(|c| c.line()).collect();
        assert_eq!(
            lines,
            [
                "schtasks /Create /TN \"Consort worker\" /XML \"C:\\Users\\Ann Lee\\AppData\\Local\\Consort\\worker-task.xml\" /F",
                "schtasks /Run /TN \"Consort worker\"",
            ]
        );
    }

    #[test]
    fn the_windows_task_quotes_a_single_quote_in_a_path() {
        let mut e = env(Platform::Windows);
        e.exe = "C:\\Users\\O'Neil\\consort.exe".into();
        let text = task_xml(&e, "C:\\logs\\w.log");
        assert!(text.contains("'C:\\Users\\O''Neil\\consort.exe'"));
    }

    #[test]
    fn windows_needs_local_app_data() {
        let mut e = env(Platform::Windows);
        e.local_app_data = None;
        assert!(plan_install(Platform::Windows, &e).is_err());
    }

    #[test]
    fn macos_needs_the_user_id() {
        let mut e = env(Platform::MacOs);
        e.uid = None;
        assert!(plan_install(Platform::MacOs, &e).is_err());
    }

    #[test]
    fn uninstall_stops_the_service_and_removes_its_file_on_every_system() {
        let mac = plan_uninstall(Platform::MacOs, &env(Platform::MacOs)).unwrap();
        assert_eq!(
            mac.remove,
            ["/Users/ann/Library/LaunchAgents/dev.consort.worker.plist"]
        );
        assert_eq!(
            mac.commands[0].line(),
            "launchctl bootout gui/501/dev.consort.worker"
        );
        let linux = plan_uninstall(Platform::Linux, &env(Platform::Linux)).unwrap();
        assert_eq!(
            linux.commands[0].line(),
            "systemctl --user disable --now consort-worker.service"
        );
        assert_eq!(linux.remove.len(), 1);
        let win = plan_uninstall(Platform::Windows, &env(Platform::Windows)).unwrap();
        assert_eq!(
            win.commands.iter().map(|c| c.line()).collect::<Vec<_>>(),
            [
                "schtasks /End /TN \"Consort worker\"",
                "schtasks /Delete /TN \"Consort worker\" /F",
            ]
        );
        for plan in [&mac, &linux, &win] {
            assert!(plan.files.is_empty(), "uninstall writes nothing");
            assert!(plan.commands.iter().all(|c| c.may_fail));
        }
    }

    #[test]
    fn status_asks_the_service_manager() {
        let mac = plan_status(Platform::MacOs, &env(Platform::MacOs)).unwrap();
        assert_eq!(
            mac.commands[0].line(),
            "launchctl print gui/501/dev.consort.worker"
        );
        let linux = plan_status(Platform::Linux, &env(Platform::Linux)).unwrap();
        assert_eq!(linux.commands.len(), 2);
        let win = plan_status(Platform::Windows, &env(Platform::Windows)).unwrap();
        assert_eq!(
            win.commands[0].line(),
            "schtasks /Query /TN \"Consort worker\" /FO LIST /V"
        );
    }

    #[test]
    fn the_status_line_reads_the_output_of_each_manager() {
        let ok = |text: &str| Outcome {
            success: true,
            output: text.to_owned(),
        };
        let cmd = PlannedCommand::new("x", &[]);
        let mac = [(cmd.clone(), ok("  state = running\n  pid = 812\n"))];
        assert_eq!(
            status_summary(Platform::MacOs, &mac),
            "The launchd agent is loaded and running (process 812)."
        );
        let gone = [(
            cmd.clone(),
            Outcome {
                success: false,
                output: String::new(),
            },
        )];
        assert_eq!(
            status_summary(Platform::MacOs, &gone),
            "The launchd agent is not loaded."
        );
        let linux = [
            (cmd.clone(), ok("active\n")),
            (cmd.clone(), ok("enabled\n")),
        ];
        assert_eq!(
            status_summary(Platform::Linux, &linux),
            "The systemd user unit is active and enabled."
        );
        let win = [(
            cmd,
            ok("TaskName: \\Consort worker\nStatus:                Running\n"),
        )];
        assert_eq!(
            status_summary(Platform::Windows, &win),
            "The Task Scheduler task exists. Its status is Running."
        );
    }

    #[test]
    fn the_dry_run_text_lists_the_files_and_the_commands() {
        let plan = plan_install(Platform::Linux, &env(Platform::Linux)).unwrap();
        let text = describe(&plan, "install");
        assert!(text.starts_with("Dry run. Consort would install a systemd user unit."));
        assert!(text.contains("Write /home/ann/.config/systemd/user/consort-worker.service:"));
        assert!(text.contains("    Restart=on-failure"));
        assert!(text.contains("    systemctl --user enable consort-worker.service"));
        assert!(
            text.contains("The worker writes its log to /home/ann/.local/state/consort/worker.log")
        );
    }

    /// Records the commands and answers from a script.
    struct Recorder {
        seen: Vec<String>,
        fail: Option<&'static str>,
    }

    impl Runner for Recorder {
        fn run(&mut self, command: &PlannedCommand) -> Result<Outcome> {
            self.seen.push(command.line());
            Ok(Outcome {
                success: self.fail != Some(command.args[0].as_str()),
                output: "refused".into(),
            })
        }
    }

    #[test]
    fn apply_writes_the_files_and_runs_the_commands_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        let mut e = env(Platform::Linux);
        e.home = root.clone();
        let plan = plan_install(Platform::Linux, &e).unwrap();
        let mut recorder = Recorder {
            seen: Vec::new(),
            fail: None,
        };
        let results = apply(&plan, &mut recorder).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(recorder.seen.len(), 3);
        let unit = std::fs::read_to_string(&plan.files[0].path).unwrap();
        assert_eq!(unit, plan.files[0].text);
        assert!(Path::new(&plan.log_file).parent().unwrap().is_dir());
    }

    #[test]
    fn apply_stops_at_a_failed_command_unless_it_may_fail() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = env(Platform::MacOs);
        e.home = dir.path().to_string_lossy().into_owned();
        let plan = plan_install(Platform::MacOs, &e).unwrap();
        // bootout may fail, bootstrap may not.
        let mut recorder = Recorder {
            seen: Vec::new(),
            fail: Some("bootout"),
        };
        apply(&plan, &mut recorder).unwrap();
        assert_eq!(recorder.seen.len(), 2);
        let mut recorder = Recorder {
            seen: Vec::new(),
            fail: Some("bootstrap"),
        };
        let err = apply(&plan, &mut recorder).unwrap_err().to_string();
        assert!(err.contains("launchctl bootstrap"), "{err}");
        assert!(err.contains("refused"), "{err}");
    }

    #[test]
    fn apply_removes_the_file_and_accepts_one_that_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = env(Platform::Linux);
        e.home = dir.path().to_string_lossy().into_owned();
        let install = plan_install(Platform::Linux, &e).unwrap();
        let mut recorder = Recorder {
            seen: Vec::new(),
            fail: None,
        };
        apply(&install, &mut recorder).unwrap();
        let uninstall = plan_uninstall(Platform::Linux, &e).unwrap();
        apply(&uninstall, &mut recorder).unwrap();
        assert!(!Path::new(&install.files[0].path).exists());
        apply(&uninstall, &mut recorder).unwrap();
    }

    #[test]
    fn the_task_file_is_written_as_utf16_with_a_byte_order_mark() {
        let file = PlannedFile {
            path: String::new(),
            text: "<a>é</a>".into(),
            encoding: Encoding::Utf16Le,
        };
        let bytes = encode(&file);
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "<a>é</a>");
    }

    #[test]
    fn a_large_log_moves_aside_at_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("worker.log");
        std::fs::write(&log, vec![b'x'; 100]).unwrap();
        rotate_log(&log, 1000).unwrap();
        assert!(log.exists(), "a small log stays");
        rotate_log(&log, 50).unwrap();
        assert!(!log.exists());
        assert!(dir.path().join("worker.log.1").exists());
        rotate_log(&log, 50).unwrap();
    }
}
