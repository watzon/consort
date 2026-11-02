//! `consort local`: a hub on the computer of the user. One per-user service
//! runs the hub, the session host and a worker, with no admin rights. All of
//! them run as the user.
//!
//! The app calls these commands, and reads their JSON on stdout. They never log
//! a setup code. A dry run prints the plan and changes nothing.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use consort_harness::boundary::{self, BoundaryError};
use consort_proto::ids::WorkerId;
use consort_proto::local::{
    CheckProblem, CheckReport, DIR_ENV, InstallReport, InstallStep, LABEL_ENV, Layout, LocalState,
    PROGRESS_PREFIX, SERVICE_LABEL, SetupCodeReport, StatusReport, UpgradeOutcome, UpgradeReport,
    address_of,
};
use consort_proto::release::{InstallKind, Version};
use consort_session_host::config::HostToml;
use consort_worker::config::WorkerToml;

use crate::service::{
    self, PlannedCommand, Platform, Runner as _, ServiceEnv, ServiceSpec, SystemRunner,
};

/// The version of this program.
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How long a start may take until the hub answers.
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the worker and the session host may take to connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a stop may take. The service manager stops the service itself.
const STOP_TIMEOUT: Duration = Duration::from_secs(35);
/// How many backups of updates stay.
const KEEP_BACKUPS: u32 = 3;

#[derive(Subcommand)]
pub enum LocalCommand {
    /// Install the hub, the session host and a worker on this computer, in
    /// one service. A second run changes only what differs. It prints JSON
    /// with the address, a setup code and the worker ID.
    Install {
        /// Print what the install would write and run. Change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Print the steps on stderr, for the app.
        #[arg(long)]
        progress: bool,
    },
    /// Print the state of the service and the hub as JSON.
    Status,
    /// Stop the service and remove it. The data stays, unless you ask.
    Uninstall {
        /// Also delete the channels, the memory and the account profiles.
        #[arg(long)]
        delete_data: bool,
        /// Print what the command would do. Change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Run the hub, the session host and a worker until a stop signal. The
    /// service runs this command.
    Run {
        /// Write the log to this file, not to the terminal.
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    /// Make a client setup code from the data folder, and print JSON.
    SetupCode,
    /// Say whether this computer can run agents, as JSON.
    Check,
    /// Start the service.
    Start,
    /// Stop the service. It stays installed.
    Stop,
    /// Turn the start at login on or off. A running hub keeps running.
    Login {
        #[arg(value_enum)]
        state: LoginState,
    },
    /// Replace the program of the service with a newer one: a backup, a stop,
    /// the replacement and a start. If the new hub does not start, the old
    /// program and the backup come back. It prints JSON.
    Upgrade {
        /// The new `consort` program.
        #[arg(long)]
        program: PathBuf,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LoginState {
    On,
    Off,
}

/// The log file of `consort local run`, when the command has one.
pub fn log_file_of(command: &LocalCommand) -> Option<&Path> {
    match command {
        LocalCommand::Run { log_file } => log_file.as_deref(),
        _ => None,
    }
}

/// The Consort folder: the override, else the data folder of this system.
pub fn consort_folder() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(DIR_ENV).filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    directories::ProjectDirs::from("", "", "Consort")
        .map(|d| d.data_dir().to_path_buf())
        .context("could not find a data folder for Consort")
}

fn label_override() -> Option<String> {
    std::env::var(LABEL_ENV).ok().filter(|l| !l.is_empty())
}

fn check_label(label: &str) -> Result<()> {
    if label.is_empty()
        || !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        bail!("{label} is not a service name. Use letters, digits, dots and dashes");
    }
    Ok(())
}

/// The service of a local hub. The variables keep an override for the program
/// that the service runs, so it finds the same folder and name.
pub fn spec_for(
    layout: &Layout,
    dir_override: bool,
    label: Option<&str>,
    platform: Platform,
) -> ServiceSpec {
    let log = layout.log_file().to_string_lossy().into_owned();
    let mut env = vec![(InstallKind::ENV_VAR.to_owned(), "local".to_owned())];
    if dir_override {
        env.push((
            DIR_ENV.to_owned(),
            layout.root().to_string_lossy().into_owned(),
        ));
    }
    if let Some(label) = label {
        env.push((LABEL_ENV.to_owned(), label.to_owned()));
    }
    ServiceSpec {
        label: label.unwrap_or(SERVICE_LABEL).to_owned(),
        unit: label.map_or_else(
            || "consort-local.service".to_owned(),
            |label| format!("{label}.service"),
        ),
        task: "Consort hub".to_owned(),
        description: "Consort hub on this computer".to_owned(),
        noun: "hub".to_owned(),
        log_file: Some(log.clone()),
        args: Some(vec![
            "local".to_owned(),
            "run".to_owned(),
            "--log-file".to_owned(),
            log,
        ]),
        env,
        private_definition: (platform == Platform::MacOs)
            .then(|| layout.service_file().to_string_lossy().into_owned()),
    }
}

/// The `PATH` of the service. An app that starts from the Dock has a short
/// `PATH`, so the common folders of tools are added.
pub fn service_path(path: Option<&str>, home: &str) -> String {
    let mut parts: Vec<String> = path
        .unwrap_or("/usr/bin:/bin:/usr/sbin:/sbin")
        .split(':')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    for extra in [
        format!("{home}/.local/bin"),
        "/opt/homebrew/bin".to_owned(),
        "/usr/local/bin".to_owned(),
    ] {
        if !parts.contains(&extra) {
            parts.push(extra);
        }
    }
    parts.join(":")
}

struct LocalService {
    layout: Layout,
    platform: Platform,
    spec: ServiceSpec,
    env: ServiceEnv,
}

fn context() -> Result<LocalService> {
    let platform = Platform::current().context("this system has no local hub")?;
    if platform == Platform::Windows {
        bail!(
            "Not on Windows yet. Consort cannot keep an agent inside its limits here. Run the hub on a Mac or a Linux computer, and add this computer as a worker"
        );
    }
    let layout = Layout::new(consort_folder()?);
    let label = label_override();
    if let Some(label) = &label {
        check_label(label)?;
    }
    let dir_override = std::env::var_os(DIR_ENV).is_some_and(|d| !d.is_empty());
    let spec = spec_for(&layout, dir_override, label.as_deref(), platform);
    let mut env = ServiceEnv::detect(&layout.program(false), None)?;
    env.path = Some(service_path(env.path.as_deref(), &env.home));
    Ok(LocalService {
        layout,
        platform,
        spec,
        env,
    })
}

/// Writes the state file through a temporary file, with 0600 rights.
pub fn write_state(layout: &Layout, state: &LocalState) -> Result<()> {
    let path = layout.state_file();
    let dir = layout.local_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("could not make {}", dir.display()))?;
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(state)?;
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("could not write {}", tmp.display()))?;
        file.write_all(&bytes)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &path).with_context(|| format!("could not write {}", path.display()))
}

/// Reads the state file. A missing or damaged file is no state.
pub fn read_state(layout: &Layout) -> Option<LocalState> {
    std::fs::read(layout.state_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// The port for this start: the saved port when it is free, else a free port
/// that the system picks.
pub fn choose_port(saved: Option<u16>) -> Result<u16> {
    if let Some(port) = saved.filter(|p| *p != 0)
        && std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
    {
        return Ok(port);
    }
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).context("could not find a free port")?;
    Ok(listener.local_addr()?.port())
}

/// The state at the start of the service, from the state before it.
pub fn starting_state(previous: Option<&LocalState>, port: u16, pid: u32) -> LocalState {
    let address = address_of(port);
    let mut past = previous
        .map(|s| s.past_addresses.clone())
        .unwrap_or_default();
    if let Some(previous) = previous
        && previous.address != address
        && !previous.address.is_empty()
        && !past.contains(&previous.address)
    {
        past.push(previous.address.clone());
    }
    past.retain(|a| *a != address);
    LocalState {
        address,
        port,
        version: VERSION.to_owned(),
        pid: Some(pid),
        worker_id: previous.and_then(|s| s.worker_id.clone()),
        worker_connected: false,
        session_host_connected: false,
        past_addresses: past,
    }
}

/// What the start of the service does with a saved pairing.
#[derive(Debug, PartialEq, Eq)]
pub enum PairStep {
    /// It is paired with this address.
    Keep,
    /// It is paired with this hub at its old address. Only the address changes.
    NewAddress,
    /// It is not paired.
    Pair,
}

/// Decides from what a worker or a session host keeps. A local hub is always
/// the same hub, so a new port keeps the ID and the key.
pub fn pair_step(hub_url: Option<&str>, has_id: bool, has_key: bool, url: &str) -> PairStep {
    match hub_url {
        Some(saved) if has_id && has_key => {
            if consort_link::http::base_url(saved) == consort_link::http::base_url(url) {
                PairStep::Keep
            } else {
                PairStep::NewAddress
            }
        }
        _ => PairStep::Pair,
    }
}

async fn pair_session_host(hub: &consort_hub::HubHandle, dir: &Path) -> Result<Option<WorkerId>> {
    let url = consort_link::http::base_url(&hub.public_url());
    let mut toml = HostToml::load(dir)?;
    let has_key = dir.join(consort_session_host::config::KEY_FILE).exists();
    match pair_step(
        toml.hub_url.as_deref(),
        toml.worker_id.is_some(),
        has_key,
        &url,
    ) {
        PairStep::Keep => {}
        PairStep::NewAddress => {
            toml.hub_url = Some(url);
            toml.save(dir)?;
        }
        PairStep::Pair => {
            let code = hub
                .new_session_host_pairing_code()
                .await
                .context("the hub could not make a session host code")?;
            consort_session_host::pair(&url, &code, dir, Some("hub".to_owned()), Vec::new())
                .await
                .context("could not pair the session host")?;
        }
    }
    Ok(HostToml::load(dir)?.worker_id)
}

async fn pair_worker(hub: &consort_hub::HubHandle, dir: &Path) -> Result<()> {
    let url = consort_link::http::base_url(&hub.public_url());
    let mut toml = WorkerToml::load(dir)?;
    let has_key = dir.join(consort_worker::config::KEY_FILE).exists();
    match pair_step(
        toml.hub_url.as_deref(),
        toml.worker_id.is_some(),
        has_key,
        &url,
    ) {
        PairStep::Keep => {}
        PairStep::NewAddress => {
            toml.hub_url = Some(url);
            toml.save(dir)?;
        }
        PairStep::Pair => {
            let code = hub
                .new_worker_pairing_code()
                .await
                .context("the hub could not make a pairing code")?;
            consort_worker::pair(&url, &code, dir, None, vec!["local".to_owned()])
                .await
                .context("could not pair the worker of this computer")?;
        }
    }
    Ok(())
}

/// `consort local run`.
pub async fn run() -> Result<()> {
    let layout = Layout::new(consort_folder()?);
    // From here a stop signal ends the wait below, and not the process, so a
    // stop during the start still closes everything.
    let stop = crate::StopSignals::listen().wait();
    tokio::pin!(stop);
    std::fs::create_dir_all(layout.local_dir())?;
    let previous = read_state(&layout);
    let port = choose_port(previous.as_ref().map(|s| s.port))?;
    let hub = consort_hub::serve(consort_hub::HubConfig {
        data_dir: layout.hub_dir(),
        listen: std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        public_url: None,
        install_kind: InstallKind::Local,
        dev: false,
        first_greeting: true,
    })
    .await
    .context("could not start the hub")?;
    let mut state = starting_state(previous.as_ref(), port, std::process::id());
    write_state(&layout, &state)?;

    let host_dir = layout.session_host_dir();
    let host_id = pair_session_host(&hub, &host_dir).await?;
    let mut session_host = crate::spawn_session_host(&host_dir, &layout.home_dir())?;
    let worker_dir = layout.worker_dir();
    pair_worker(&hub, &worker_dir).await?;
    let worker = consort_worker::run(consort_worker::WorkerConfig {
        config_dir: worker_dir,
        // The app replaces this program.
        self_update: None,
    })
    .await?;
    state.worker_id = Some(worker.worker_id().to_string());
    write_state(&layout, &state)?;
    tracing::info!(address = %state.address, "the hub of this computer runs");

    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            () = &mut stop => break,
            status = session_host.wait() => {
                // A session host that ends does not stop the hub. The turns of
                // that moment fail, and new ones work when it is back.
                tracing::warn!(?status, "the session host ended. It starts again in 5 seconds");
                tokio::select! {
                    () = &mut stop => {
                        state.session_host_connected = false;
                        break;
                    }
                    () = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
                session_host = crate::spawn_session_host(&host_dir, &layout.home_dir())?;
            }
            _ = tick.tick() => {
                let worker_connected = worker.is_connected();
                let host_connected = host_id.is_some_and(|id| hub.peer_online(id));
                if worker_connected != state.worker_connected
                    || host_connected != state.session_host_connected
                {
                    state.worker_connected = worker_connected;
                    state.session_host_connected = host_connected;
                    write_state(&layout, &state)?;
                }
            }
        }
    }
    crate::stop_session_host(session_host).await;
    worker.shutdown().await?;
    hub.shutdown()
        .await
        .context("the hub did not stop cleanly")?;
    state.pid = None;
    state.worker_connected = false;
    state.session_host_connected = false;
    write_state(&layout, &state)?;
    Ok(())
}

/// Asks the hub on this port. Its version, or `None` when it does not answer.
pub fn health(port: u16) -> Option<String> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .write_all(b"GET /healthz HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .ok()?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;
    let (_, body) = answer.split_once("\r\n\r\n")?;
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if value.get("ok")?.as_bool()? {
        value.get("version")?.as_str().map(str::to_owned)
    } else {
        None
    }
}

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The hub answers on the address of the state file, with this version when
/// one is named. The state must be new: an old file can name a stopped hub.
fn hub_answers(layout: &Layout, version: Option<&str>) -> bool {
    let Some(state) = read_state(layout) else {
        return false;
    };
    if state.pid.is_none() {
        return false;
    }
    match health(state.port) {
        Some(found) => version.is_none_or(|v| v == found),
        None => false,
    }
}

fn run_all(commands: &[PlannedCommand]) -> Result<()> {
    for command in commands {
        let outcome = SystemRunner.run(command)?;
        if !outcome.success && !command.may_fail {
            bail!(
                "`{}` failed: {}",
                command.line(),
                outcome.output.trim().lines().next().unwrap_or("no message")
            );
        }
    }
    Ok(())
}

/// Puts `source` in place as the program of the service. A reader always finds
/// a whole file: the new file gets its final name with a rename, and the old
/// one keeps a name as the previous program. Returns false when the program is
/// the same file already.
pub fn install_program(source: &Path, layout: &Layout, windows: bool) -> Result<bool> {
    let target = layout.program(windows);
    let previous = layout.previous_program(windows);
    let dir = layout.bin_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("could not make {}", dir.display()))?;
    if same_file(source, &target) {
        return Ok(false);
    }
    let new = dir.join(".consort.new");
    std::fs::copy(source, &new)
        .with_context(|| format!("could not copy {} to {}", source.display(), new.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
    }
    if target.exists() {
        let link = dir.join(".consort.previous");
        let _ = std::fs::remove_file(&link);
        std::fs::hard_link(&target, &link)
            .or_else(|_| std::fs::copy(&target, &link).map(|_| ()))?;
        std::fs::rename(&link, &previous)?;
    }
    if cfg!(target_os = "macos") {
        // The app is signed and the user opened it. A background service has
        // no window for a question about a file from the internet.
        let _ = std::process::Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(&new)
            .output();
    }
    std::fs::rename(&new, &target)
        .with_context(|| format!("could not replace {}", target.display()))?;
    Ok(true)
}

/// Both paths exist and hold the same bytes.
fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(meta_a), Ok(meta_b)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if meta_a.len() != meta_b.len() {
        return false;
    }
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(a), std::fs::canonicalize(b))
        && a == b
    {
        return true;
    }
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn progress(on: bool, step: InstallStep) {
    if on {
        eprintln!("{PROGRESS_PREFIX}{}", step.as_str());
    }
}

/// The file that starts the service at login on macOS.
fn login_file(ctx: &LocalService) -> Result<String> {
    service::definition_path_for(&ctx.spec, ctx.platform, &ctx.env)
}

fn start_at_login(ctx: &LocalService) -> bool {
    match ctx.platform {
        Platform::MacOs => login_file(ctx).is_ok_and(|p| Path::new(&p).exists()),
        Platform::Linux => {
            let check = PlannedCommand {
                program: "systemctl".to_owned(),
                args: vec![
                    "--user".to_owned(),
                    "is-enabled".to_owned(),
                    ctx.spec.unit.clone(),
                ],
                may_fail: true,
            };
            SystemRunner
                .run(&check)
                .is_ok_and(|o| o.output.trim() == "enabled")
        }
        Platform::Windows => false,
    }
}

fn is_installed(ctx: &LocalService) -> bool {
    let definition = login_file(ctx).is_ok_and(|p| Path::new(&p).exists());
    definition || ctx.layout.service_file().exists()
}

fn service_running(ctx: &LocalService) -> bool {
    let Ok(plan) = service::plan_status_for(&ctx.spec, ctx.platform, &ctx.env) else {
        return false;
    };
    let Some(first) = plan.commands.first() else {
        return false;
    };
    let Ok(outcome) = SystemRunner.run(first) else {
        return false;
    };
    match ctx.platform {
        Platform::MacOs => {
            outcome.success
                && outcome
                    .output
                    .lines()
                    .any(|l| l.trim() == "state = running")
        }
        Platform::Linux => outcome.output.trim() == "active",
        Platform::Windows => false,
    }
}

/// `consort local check`.
pub fn check() -> CheckReport {
    CheckReport {
        problem: problem_of(
            boundary::detect().err().as_ref(),
            std::env::consts::OS,
            &has_program,
        ),
    }
}

fn has_program(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The problem for an answer of the boundary.
pub fn problem_of(
    error: Option<&BoundaryError>,
    os: &str,
    has: &dyn Fn(&str) -> bool,
) -> Option<CheckProblem> {
    match error? {
        BoundaryError::UnsupportedSystem => Some(CheckProblem::NoBoundary),
        BoundaryError::NotUsable(_) if os == "linux" && !has("bwrap") => {
            Some(CheckProblem::NeedsBubblewrap {
                command: bubblewrap_command(has),
            })
        }
        BoundaryError::NotUsable(message) => Some(CheckProblem::BoundaryFailed {
            message: message.clone(),
        }),
    }
}

/// The command that installs bubblewrap on this system.
pub fn bubblewrap_command(has: &dyn Fn(&str) -> bool) -> String {
    for (manager, command) in [
        ("apt", "sudo apt install bubblewrap"),
        ("dnf", "sudo dnf install bubblewrap"),
        ("pacman", "sudo pacman -S bubblewrap"),
        ("zypper", "sudo zypper install bubblewrap"),
        ("apk", "sudo apk add bubblewrap"),
    ] {
        if has(manager) {
            return command.to_owned();
        }
    }
    "sudo apt install bubblewrap".to_owned()
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// The text of `consort local install --dry-run`.
pub fn describe_install(plan: &service::Plan, layout: &Layout, source: &Path) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Dry run. Consort would copy {} to {}, and keep the older copy as {}.\n",
        source.display(),
        layout.program(false).display(),
        layout.previous_program(false).display()
    ));
    out.push_str(&format!(
        "The hub data goes in {}, with the account profiles and tools in {}.\n",
        layout.hub_dir().display(),
        layout.home_dir().display()
    ));
    out.push_str(&service::describe(plan, "install"));
    out
}

/// `consort local install`.
pub fn install(dry_run: bool, show_progress: bool) -> Result<()> {
    let ctx = context()?;
    let source = std::env::current_exe().context("could not find this program")?;
    let plan = service::plan_install_for(&ctx.spec, ctx.platform, &ctx.env)?;
    if dry_run {
        print!("{}", describe_install(&plan, &ctx.layout, &source));
        return Ok(());
    }
    if let Some(problem) = check().problem {
        bail!("this computer cannot run agents: {problem:?}");
    }
    progress(show_progress, InstallStep::Checked);
    let was_installed = is_installed(&ctx);
    let login_was_on = !was_installed || start_at_login(&ctx);

    std::fs::create_dir_all(ctx.layout.local_dir())?;
    let replaced = install_program(&source, &ctx.layout, false)?;
    progress(show_progress, InstallStep::StartingHub);
    // A second install leaves a healthy hub alone, so the tasks keep working.
    let unchanged = !replaced && was_installed && hub_answers(&ctx.layout, Some(VERSION));
    if !unchanged {
        let results = service::apply(&plan, &mut SystemRunner);
        if let Err(err) = results {
            remove_failed_service(&ctx);
            return Err(err);
        }
        if !login_was_on {
            set_login(&ctx, false)?;
        }
    }
    if !wait_until(START_TIMEOUT, || hub_answers(&ctx.layout, Some(VERSION))) {
        remove_failed_service(&ctx);
        bail!(
            "The hub did not start. Nothing else changed on this computer. The log is {}",
            ctx.layout.log_file().display()
        );
    }
    progress(show_progress, InstallStep::AddingWorker);
    let connected = wait_until(CONNECT_TIMEOUT, || {
        read_state(&ctx.layout).is_some_and(|s| s.ready() && s.session_host_connected)
    });
    if !connected {
        remove_failed_service(&ctx);
        bail!(
            "The hub started, but its worker or its session host did not connect. The log is {}",
            ctx.layout.log_file().display()
        );
    }
    let state = read_state(&ctx.layout).context("the hub wrote no state file")?;
    let setup_code = consort_hub::create_setup_code(&ctx.layout.hub_dir())
        .context("could not make a setup code")?;
    print_json(&InstallReport {
        address: state.address,
        setup_code,
        worker_id: state.worker_id.unwrap_or_default(),
        version: VERSION.to_owned(),
    })
}

/// A failed install leaves no service that starts again and again.
fn remove_failed_service(ctx: &LocalService) {
    if let Ok(plan) = service::plan_uninstall_for(&ctx.spec, ctx.platform, &ctx.env)
        && let Err(err) = service::apply(&plan, &mut SystemRunner)
    {
        eprintln!("Consort could not remove the service: {err}");
    }
}

/// `consort local status`.
pub fn status() -> Result<()> {
    let ctx = context()?;
    print_json(&status_report(&ctx))
}

fn status_report(ctx: &LocalService) -> StatusReport {
    let state = read_state(&ctx.layout);
    let hub_version = state
        .as_ref()
        .filter(|s| s.pid.is_some())
        .and_then(|s| health(s.port));
    let has_data = ctx.layout.hub_dir().join("hub.db").exists();
    let working_tasks = if has_data {
        consort_hub::admin::working_tasks(&ctx.layout.hub_dir()).unwrap_or(0)
    } else {
        0
    };
    let live = state.as_ref().filter(|s| s.pid.is_some());
    StatusReport {
        installed: is_installed(ctx),
        running: service_running(ctx),
        healthy: hub_version.is_some(),
        address: state.as_ref().map(|s| s.address.clone()),
        version: VERSION.to_owned(),
        hub_version,
        start_at_login: start_at_login(ctx),
        worker_id: state.as_ref().and_then(|s| s.worker_id.clone()),
        worker_connected: live.is_some_and(|s| s.worker_connected),
        session_host_connected: live.is_some_and(|s| s.session_host_connected),
        working_tasks: u32::try_from(working_tasks).unwrap_or(u32::MAX),
        has_data,
    }
}

/// `consort local uninstall`. It leaves the files of the app in the Consort
/// folder alone.
pub async fn uninstall(delete_data: bool, dry_run: bool) -> Result<()> {
    let ctx = context()?;
    let plan = service::plan_uninstall_for(&ctx.spec, ctx.platform, &ctx.env)?;
    if dry_run {
        print!("{}", service::describe(&plan, "remove"));
        println!(
            "The program in {} goes away. {}",
            ctx.layout.bin_dir().display(),
            if delete_data {
                format!("{} goes away too.", ctx.layout.local_dir().display())
            } else {
                format!("The data in {} stays.", ctx.layout.local_dir().display())
            }
        );
        return Ok(());
    }
    // A Serve entry must not point at a port where no hub listens. A hub that
    // never used Tailscale does not run the program here.
    if ctx.layout.hub_dir().join("hub.db").exists()
        && let Err(err) = consort_hub::reach::cli::off(&ctx.layout.hub_dir(), None).await
    {
        eprintln!("Consort could not turn off the reach of the hub: {err}. Check it in Tailscale.");
    }
    service::apply(&plan, &mut SystemRunner)?;
    wait_for_stop(&ctx.layout);
    let _ = std::fs::remove_dir_all(ctx.layout.bin_dir());
    if delete_data {
        match std::fs::remove_dir_all(ctx.layout.local_dir()) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("could not delete {}", ctx.layout.local_dir().display())
                });
            }
        }
    }
    println!("{{\"removed\":true,\"data_kept\":{}}}", !delete_data);
    Ok(())
}

/// Waits until the hub no longer answers on its address.
fn wait_for_stop(layout: &Layout) {
    let Some(state) = read_state(layout) else {
        return;
    };
    wait_until(STOP_TIMEOUT, || health(state.port).is_none());
}

/// `consort local setup-code`.
pub fn setup_code() -> Result<()> {
    let ctx = context()?;
    let state = read_state(&ctx.layout).context("no hub is on this computer")?;
    let setup_code = consort_hub::create_setup_code(&ctx.layout.hub_dir())
        .context("could not make a setup code")?;
    print_json(&SetupCodeReport {
        address: state.address,
        setup_code,
    })
}

/// `consort local start`. It returns when the hub answers.
pub fn start() -> Result<()> {
    let ctx = context()?;
    if !is_installed(&ctx) {
        bail!("no hub is on this computer. Run `consort local install`");
    }
    run_all(&service::start_commands(&ctx.spec, ctx.platform, &ctx.env)?)?;
    if !wait_until(START_TIMEOUT, || hub_answers(&ctx.layout, None)) {
        bail!(
            "The hub did not start. The log is {}",
            ctx.layout.log_file().display()
        );
    }
    // The hub answers first. The status is more useful when the session host and
    // the worker are in it too, but a slow part does not make the start fail.
    wait_until(CONNECT_TIMEOUT, || {
        read_state(&ctx.layout).is_some_and(|s| s.worker_connected && s.session_host_connected)
    });
    print_json(&status_report(&ctx))
}

/// `consort local stop`.
pub fn stop() -> Result<()> {
    let ctx = context()?;
    run_all(&service::stop_commands(&ctx.spec, ctx.platform, &ctx.env)?)?;
    wait_for_stop(&ctx.layout);
    print_json(&status_report(&ctx))
}

fn set_login(ctx: &LocalService, on: bool) -> Result<()> {
    match ctx.platform {
        Platform::MacOs => {
            let file = login_file(ctx)?;
            if on {
                if let Some(parent) = Path::new(&file).parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(ctx.layout.service_file(), &file)
                    .with_context(|| format!("could not write {file}"))?;
            } else {
                match std::fs::remove_file(&file) {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => {
                        return Err(err).with_context(|| format!("could not remove {file}"));
                    }
                }
            }
            Ok(())
        }
        Platform::Linux => run_all(&service::login_commands(&ctx.spec, on)),
        Platform::Windows => bail!("Windows has no local hub"),
    }
}

/// `consort local login`.
pub fn login(state: LoginState) -> Result<()> {
    let ctx = context()?;
    if !is_installed(&ctx) {
        bail!("no hub is on this computer. Run `consort local install`");
    }
    set_login(&ctx, state == LoginState::On)?;
    print_json(&status_report(&ctx))
}

/// The steps of an upgrade. The tests use a fake, so no test stops a service.
pub trait UpgradeSteps {
    /// Makes the backup and returns its path.
    fn back_up(&mut self) -> Result<String>;
    fn stop(&mut self) -> Result<()>;
    fn replace_program(&mut self) -> Result<()>;
    fn start(&mut self) -> Result<()>;
    /// The hub with this version answers.
    fn wait_healthy(&mut self, version: Option<&str>) -> bool;
    fn restore_program(&mut self) -> Result<()>;
    fn restore_backup(&mut self, backup: &str) -> Result<()>;
}

/// Runs an upgrade: back up, stop, replace, start. A failure after the stop
/// puts the previous program and the backup back. A failure before the stop
/// returns an error, and nothing has changed.
pub fn run_upgrade(steps: &mut dyn UpgradeSteps, from: &str, to: &str) -> Result<UpgradeReport> {
    let backup = steps.back_up().context("could not make a backup")?;
    let report = |outcome, message: String| UpgradeReport {
        outcome,
        from: from.to_owned(),
        to: to.to_owned(),
        backup: Some(backup.clone()),
        message,
    };
    steps.stop().context("could not stop the hub")?;
    let went_up = steps
        .replace_program()
        .and_then(|()| steps.start())
        .map(|()| steps.wait_healthy(Some(to)));
    let reason = match went_up {
        Ok(true) => return Ok(report(UpgradeOutcome::Replaced, String::new())),
        Ok(false) => format!("The hub {to} did not start."),
        Err(err) => format!("The hub {to} did not start: {err}."),
    };
    let _ = steps.stop();
    let rolled_back = steps
        .restore_program()
        .and_then(|()| steps.restore_backup(&backup))
        .and_then(|()| steps.start())
        .map(|()| steps.wait_healthy(Some(from)));
    Ok(match rolled_back {
        Ok(true) => report(
            UpgradeOutcome::RolledBack,
            format!(
                "{reason} Consort put back version {from} and your data. The app did not update."
            ),
        ),
        Ok(false) => report(
            UpgradeOutcome::RollbackFailed,
            format!(
                "{reason} Version {from} did not start again either. Your backup is {backup}. Open the log, and restore the backup with `consort hub restore`."
            ),
        ),
        Err(err) => report(
            UpgradeOutcome::RollbackFailed,
            format!(
                "{reason} Consort could not put back version {from}: {err}. Your backup is {backup}. Restore it with `consort hub restore`."
            ),
        ),
    })
}

struct SystemUpgrade<'a> {
    ctx: &'a LocalService,
    program: &'a Path,
}

impl UpgradeSteps for SystemUpgrade<'_> {
    fn back_up(&mut self) -> Result<String> {
        let layout = &self.ctx.layout;
        let path =
            consort_hub::admin::back_up_now(&layout.hub_dir(), &layout.backup_dir(), KEEP_BACKUPS)
                .map_err(|err| anyhow::anyhow!("{err}"))?;
        Ok(path.display().to_string())
    }

    fn stop(&mut self) -> Result<()> {
        run_all(&service::stop_commands(
            &self.ctx.spec,
            self.ctx.platform,
            &self.ctx.env,
        )?)?;
        wait_for_stop(&self.ctx.layout);
        Ok(())
    }

    fn replace_program(&mut self) -> Result<()> {
        install_program(self.program, &self.ctx.layout, false).map(|_| ())
    }

    fn start(&mut self) -> Result<()> {
        run_all(&service::start_commands(
            &self.ctx.spec,
            self.ctx.platform,
            &self.ctx.env,
        )?)
    }

    fn wait_healthy(&mut self, version: Option<&str>) -> bool {
        wait_until(START_TIMEOUT, || hub_answers(&self.ctx.layout, version))
    }

    fn restore_program(&mut self) -> Result<()> {
        let layout = &self.ctx.layout;
        std::fs::rename(layout.previous_program(false), layout.program(false))
            .context("the previous program is gone")
    }

    fn restore_backup(&mut self, backup: &str) -> Result<()> {
        consort_hub::admin::restore(Path::new(backup), &self.ctx.layout.hub_dir(), true)
            .map(|_| ())
            .map_err(|err| anyhow::anyhow!("{err}"))
    }
}

/// The version that a program says it has.
fn program_version(program: &Path) -> Result<Version> {
    let output = std::process::Command::new(program)
        .arg("--version")
        .output()
        .with_context(|| format!("could not run {}", program.display()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .last()
        .and_then(Version::parse)
        .with_context(|| format!("{} did not say its version", program.display()))
}

/// `consort local upgrade`. The installed program runs this command. It never
/// replaces itself with an older program.
pub fn upgrade(program: &Path) -> Result<()> {
    let ctx = context()?;
    if !is_installed(&ctx) {
        bail!("no hub is on this computer. Run `consort local install`");
    }
    let to = program_version(program)?;
    let from = program_version(&ctx.layout.program(false))?;
    if to < from {
        bail!(
            "{} has version {to}, which is older than the hub {from}",
            program.display()
        );
    }
    if to == from {
        return print_json(&UpgradeReport {
            outcome: UpgradeOutcome::Unchanged,
            from: from.to_string(),
            to: to.to_string(),
            backup: None,
            message: String::new(),
        });
    }
    let mut steps = SystemUpgrade { ctx: &ctx, program };
    let report = run_upgrade(&mut steps, &from.to_string(), &to.to_string())?;
    print_json(&report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac_env(home: &str) -> ServiceEnv {
        ServiceEnv {
            exe: format!("{home}/Consort/bin/consort"),
            home: home.to_owned(),
            xdg_config_home: None,
            xdg_state_home: None,
            local_app_data: None,
            windows_user: None,
            uid: Some("501".into()),
            path: Some("/usr/bin".into()),
            config_dir: None,
        }
    }

    #[test]
    fn the_local_service_is_not_the_worker_service() {
        let layout = Layout::new("/Users/ann/Consort");
        let spec = spec_for(&layout, false, None, Platform::MacOs);
        let plan =
            service::plan_install_for(&spec, Platform::MacOs, &mac_env("/Users/ann")).unwrap();
        assert_eq!(
            plan.files[0].path,
            "/Users/ann/Library/LaunchAgents/dev.consort.local.plist"
        );
        assert_eq!(plan.files[1].path, "/Users/ann/Consort/local/service.plist");
        let text = &plan.files[0].text;
        assert!(
            text.contains("<string>dev.consort.local</string>"),
            "{text}"
        );
        assert!(
            text.contains("<string>local</string>\n    <string>run</string>"),
            "{text}"
        );
        assert!(
            text.contains("<key>CONSORT_INSTALL_KIND</key>\n    <string>local</string>"),
            "{text}"
        );
        assert!(!text.contains("CONSORT_LOCAL_DIR"), "{text}");
        assert_eq!(plan.log_file, "/Users/ann/Consort/local/logs/local.log");
        let lines: Vec<_> = plan.commands.iter().map(|c| c.line()).collect();
        assert_eq!(
            lines,
            [
                "launchctl bootout gui/501/dev.consort.local",
                "launchctl bootstrap gui/501 /Users/ann/Consort/local/service.plist",
            ]
        );
    }

    #[test]
    fn the_linux_unit_has_its_own_name_and_keeps_an_override() {
        let layout = Layout::new("/tmp/x y");
        let spec = spec_for(&layout, true, None, Platform::Linux);
        assert_eq!(spec.unit, "consort-local.service");
        let mut env = mac_env("/home/ann");
        env.uid = None;
        let plan = service::plan_install_for(&spec, Platform::Linux, &env).unwrap();
        assert_eq!(plan.files.len(), 1, "only macOS has a private copy");
        let text = &plan.files[0].text;
        assert!(
            text.contains("Description=Consort hub on this computer"),
            "{text}"
        );
        assert!(
            text.contains("Environment=CONSORT_INSTALL_KIND=local"),
            "{text}"
        );
        assert!(
            text.contains("Environment=\"CONSORT_LOCAL_DIR=/tmp/x y\""),
            "{text}"
        );
        let custom = spec_for(&layout, true, Some("test.label"), Platform::Linux);
        assert_eq!(custom.unit, "test.label.service");
    }

    #[test]
    fn the_start_and_the_stop_use_the_label_of_the_spec() {
        let layout = Layout::new("/c");
        let spec = spec_for(&layout, false, Some("t.x"), Platform::MacOs);
        let env = mac_env("/h");
        let start: Vec<_> = service::start_commands(&spec, Platform::MacOs, &env)
            .unwrap()
            .iter()
            .map(|c| c.line())
            .collect();
        assert_eq!(
            start,
            [
                "launchctl bootstrap gui/501 /c/local/service.plist",
                "launchctl kickstart gui/501/t.x",
            ]
        );
        let stop = service::stop_commands(&spec, Platform::MacOs, &env).unwrap();
        assert_eq!(stop[0].line(), "launchctl bootout gui/501/t.x");
        assert!(stop[0].may_fail);
        let login = service::login_commands(&spec, false);
        assert_eq!(login[0].line(), "systemctl --user disable t.x.service");
    }

    #[test]
    fn the_uninstall_removes_both_copies_of_the_definition() {
        let layout = Layout::new("/c");
        let spec = spec_for(&layout, false, None, Platform::MacOs);
        let plan = service::plan_uninstall_for(&spec, Platform::MacOs, &mac_env("/h")).unwrap();
        assert_eq!(
            plan.remove,
            [
                "/h/Library/LaunchAgents/dev.consort.local.plist",
                "/c/local/service.plist"
            ]
        );
    }

    #[test]
    fn a_label_must_be_a_plain_name() {
        assert!(check_label("dev.consort.test-1").is_ok());
        assert!(check_label("a/b").is_err());
        assert!(check_label("").is_err());
    }

    #[test]
    fn the_service_path_gets_the_folders_of_common_tools() {
        assert_eq!(
            service_path(Some("/usr/bin:/opt/homebrew/bin"), "/h"),
            "/usr/bin:/opt/homebrew/bin:/h/.local/bin:/usr/local/bin"
        );
        assert!(service_path(None, "/h").starts_with("/usr/bin:/bin"));
    }

    #[test]
    fn the_state_file_is_private_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path());
        assert_eq!(read_state(&layout), None);
        let state = starting_state(None, 7700, 12);
        write_state(&layout, &state).unwrap();
        assert_eq!(read_state(&layout), Some(state));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(layout.state_file())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(layout.state_file(), "not json").unwrap();
        assert_eq!(read_state(&layout), None, "a damaged file is no state");
    }

    #[test]
    fn the_saved_port_stays_while_it_is_free() {
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let busy = held.local_addr().unwrap().port();
        let other = choose_port(Some(busy)).unwrap();
        assert_ne!(other, busy, "a busy port is not used");
        drop(held);
        assert_eq!(choose_port(Some(busy)).unwrap(), busy);
        assert_ne!(choose_port(None).unwrap(), 0);
        assert_ne!(choose_port(Some(0)).unwrap(), 0);
    }

    #[test]
    fn a_new_port_remembers_the_old_address_and_keeps_the_worker() {
        let mut first = starting_state(None, 7700, 1);
        first.worker_id = Some("wrk_1".into());
        let same = starting_state(Some(&first), 7700, 2);
        assert!(same.past_addresses.is_empty());
        let moved = starting_state(Some(&first), 7801, 3);
        assert_eq!(moved.past_addresses, ["http://127.0.0.1:7700"]);
        assert_eq!(moved.worker_id.as_deref(), Some("wrk_1"));
        assert!(!moved.worker_connected);
        let back = starting_state(Some(&moved), 7700, 4);
        assert_eq!(back.past_addresses, ["http://127.0.0.1:7801"]);
    }

    #[test]
    fn a_new_address_keeps_the_pairing() {
        let url = "http://127.0.0.1:7801";
        assert_eq!(pair_step(Some(url), true, true, url), PairStep::Keep);
        assert_eq!(
            pair_step(Some("http://127.0.0.1:7700"), true, true, url),
            PairStep::NewAddress
        );
        assert_eq!(pair_step(Some(url), true, false, url), PairStep::Pair);
        assert_eq!(pair_step(Some(url), false, true, url), PairStep::Pair);
        assert_eq!(pair_step(None, false, false, url), PairStep::Pair);
    }

    #[test]
    fn the_program_goes_in_place_and_the_old_one_stays_for_a_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path().join("Consort"));
        let new = dir.path().join("new");
        std::fs::write(&new, b"one").unwrap();
        assert!(install_program(&new, &layout, false).unwrap());
        assert_eq!(std::fs::read(layout.program(false)).unwrap(), b"one");
        assert!(!layout.previous_program(false).exists());
        assert!(
            !install_program(&new, &layout, false).unwrap(),
            "the same bytes change nothing"
        );
        std::fs::write(&new, b"two!").unwrap();
        assert!(install_program(&new, &layout, false).unwrap());
        assert_eq!(std::fs::read(layout.program(false)).unwrap(), b"two!");
        assert_eq!(
            std::fs::read(layout.previous_program(false)).unwrap(),
            b"one"
        );
        let names: Vec<_> = std::fs::read_dir(layout.bin_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.starts_with('.')), "{names:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(layout.program(false))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "the copy can run");
        }
    }

    #[test]
    fn the_health_check_reads_the_answer_of_the_hub_and_needs_ok() {
        fn answer_once(body: &'static str) -> u16 {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 256];
                let _ = stream.read(&mut buf);
                let _ = write!(stream, "HTTP/1.1 200 OK\r\n\r\n{body}");
            });
            port
        }
        assert_eq!(
            health(answer_once(r#"{"ok":true,"version":"0.8.0"}"#)).as_deref(),
            Some("0.8.0")
        );
        assert_eq!(
            health(answer_once(r#"{"ok":false,"version":"0.8.0"}"#)),
            None
        );
        assert_eq!(health(answer_once("not json")), None);
        let closed = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        assert_eq!(health(port), None);
    }

    #[test]
    fn the_check_names_what_this_system_needs() {
        let none = |_: &str| false;
        assert_eq!(problem_of(None, "macos", &none), None);
        assert_eq!(
            problem_of(Some(&BoundaryError::UnsupportedSystem), "windows", &none),
            Some(CheckProblem::NoBoundary)
        );
        let dnf = |name: &str| name == "dnf";
        assert_eq!(
            problem_of(Some(&BoundaryError::NotUsable("no".into())), "linux", &dnf),
            Some(CheckProblem::NeedsBubblewrap {
                command: "sudo dnf install bubblewrap".into()
            })
        );
        let has_bwrap = |name: &str| name == "bwrap";
        assert_eq!(
            problem_of(
                Some(&BoundaryError::NotUsable("no user namespaces".into())),
                "linux",
                &has_bwrap
            ),
            Some(CheckProblem::BoundaryFailed {
                message: "no user namespaces".into()
            })
        );
    }

    /// Records the steps and fails where a test says.
    #[derive(Default)]
    struct FakeSteps {
        seen: Vec<String>,
        fail_backup: bool,
        new_hub_starts: bool,
        old_hub_starts: bool,
        fail_replace: bool,
    }

    impl UpgradeSteps for FakeSteps {
        fn back_up(&mut self) -> Result<String> {
            self.seen.push("back up".into());
            if self.fail_backup {
                bail!("the disk is full");
            }
            Ok("/b/consort.tar".into())
        }
        fn stop(&mut self) -> Result<()> {
            self.seen.push("stop".into());
            Ok(())
        }
        fn replace_program(&mut self) -> Result<()> {
            self.seen.push("replace".into());
            if self.fail_replace {
                bail!("the disk is full");
            }
            Ok(())
        }
        fn start(&mut self) -> Result<()> {
            self.seen.push("start".into());
            Ok(())
        }
        fn wait_healthy(&mut self, version: Option<&str>) -> bool {
            self.seen.push(format!("wait {}", version.unwrap_or("any")));
            if version == Some("2.0.0") {
                self.new_hub_starts
            } else {
                self.old_hub_starts
            }
        }
        fn restore_program(&mut self) -> Result<()> {
            self.seen.push("restore program".into());
            Ok(())
        }
        fn restore_backup(&mut self, backup: &str) -> Result<()> {
            self.seen.push(format!("restore {backup}"));
            Ok(())
        }
    }

    #[test]
    fn an_upgrade_backs_up_stops_replaces_and_starts_in_that_order() {
        let mut steps = FakeSteps {
            new_hub_starts: true,
            ..FakeSteps::default()
        };
        let report = run_upgrade(&mut steps, "1.0.0", "2.0.0").unwrap();
        assert_eq!(report.outcome, UpgradeOutcome::Replaced);
        assert_eq!(report.backup.as_deref(), Some("/b/consort.tar"));
        assert_eq!(
            steps.seen,
            ["back up", "stop", "replace", "start", "wait 2.0.0"]
        );
    }

    #[test]
    fn a_failed_backup_changes_nothing() {
        let mut steps = FakeSteps {
            fail_backup: true,
            ..FakeSteps::default()
        };
        assert!(run_upgrade(&mut steps, "1.0.0", "2.0.0").is_err());
        assert_eq!(steps.seen, ["back up"]);
    }

    #[test]
    fn a_new_hub_that_does_not_start_brings_back_the_program_and_the_backup() {
        let mut steps = FakeSteps {
            old_hub_starts: true,
            ..FakeSteps::default()
        };
        let report = run_upgrade(&mut steps, "1.0.0", "2.0.0").unwrap();
        assert_eq!(report.outcome, UpgradeOutcome::RolledBack);
        assert!(
            report.message.contains("put back version 1.0.0"),
            "{}",
            report.message
        );
        assert_eq!(
            steps.seen,
            [
                "back up",
                "stop",
                "replace",
                "start",
                "wait 2.0.0",
                "stop",
                "restore program",
                "restore /b/consort.tar",
                "start",
                "wait 1.0.0",
            ]
        );
    }

    #[test]
    fn a_rollback_that_fails_names_the_backup() {
        let mut steps = FakeSteps::default();
        let report = run_upgrade(&mut steps, "1.0.0", "2.0.0").unwrap();
        assert_eq!(report.outcome, UpgradeOutcome::RollbackFailed);
        assert!(
            report.message.contains("/b/consort.tar"),
            "{}",
            report.message
        );
    }

    #[test]
    fn a_failed_replacement_rolls_back_too() {
        let mut steps = FakeSteps {
            fail_replace: true,
            old_hub_starts: true,
            ..FakeSteps::default()
        };
        let report = run_upgrade(&mut steps, "1.0.0", "2.0.0").unwrap();
        assert_eq!(report.outcome, UpgradeOutcome::RolledBack);
        assert!(!steps.seen.contains(&"wait 2.0.0".to_owned()));
    }
}
