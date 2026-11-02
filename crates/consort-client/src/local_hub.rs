//! A hub on this computer: the client side of `consort local`. This module
//! finds the program that the app carries, runs the install, reads the state
//! file, starts the service, and sees a hub that stopped. The desktop app holds
//! a [`LocalHub`] handle and reads its [`LocalHubView`]. It never spawns
//! anything and never touches the network.
//!
//! [`LocalHub::install_and_connect`] starts the install from the first screen of
//! the app, and [`InstallProgress`] holds its four steps.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use consort_proto::local::InstallStep;
pub use consort_proto::release::InstallKind;

use consort_proto::local::{
    CheckProblem, CheckReport, DIR_ENV, InstallReport, Layout, LocalState, SetupCodeReport,
    StatusReport, UpgradeOutcome, UpgradeReport,
};
use consort_proto::release::Version;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{Client, ClientError, view::Connection};

/// How often the service looks at the hub.
const POLL: Duration = Duration::from_secs(5);
/// How long the app waits between two looks at the tasks that work.
const TASK_POLL: Duration = Duration::from_secs(5);
/// A stopped hub needs this many failed looks in a row. A restart is short.
const FAILS_FOR_STOPPED: u32 = 2;

/// The Consort folder: the override, else the data folder of this system.
pub fn consort_folder() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(DIR_ENV).filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    directories::ProjectDirs::from("", "", "Consort").map(|d| d.data_dir().to_path_buf())
}

/// Where the program is for an app at `exe`, on this system. A macOS bundle has
/// it in `Contents/Helpers`, because `Contents/MacOS/consort` and
/// `Contents/MacOS/Consort` are one file on a normal Mac disk. Other builds have
/// it beside the app, and a debug build finds `target/debug/consort`.
pub fn bundled_program_path(os: crate::update::Os, exe: &Path) -> Option<PathBuf> {
    if os == crate::update::Os::MacOs
        && let Some(bundle) = exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
    {
        return Some(bundle.join("Contents/Helpers/consort"));
    }
    let name = if os == crate::update::Os::Windows {
        "consort.exe"
    } else {
        "consort"
    };
    Some(exe.parent()?.join(name))
}

/// What to do about the program of the service at a start of the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwapDecision {
    /// Both have one version, or one of them is unknown.
    Nothing,
    /// The app carries a newer program.
    Upgrade { from: Version, to: Version },
    /// The hub is newer than this app. The version states handle it.
    AppTooOld,
}

/// Compares the program in the app with the copy that the service runs.
pub fn swap_decision(bundled: Option<&Version>, installed: Option<&Version>) -> SwapDecision {
    match (bundled, installed) {
        (Some(bundled), Some(installed)) if bundled > installed => SwapDecision::Upgrade {
            from: installed.clone(),
            to: bundled.clone(),
        },
        (Some(bundled), Some(installed)) if bundled < installed => SwapDecision::AppTooOld,
        _ => SwapDecision::Nothing,
    }
}

/// Reads `consort 0.8.0`.
pub fn parse_version_line(line: &str) -> Option<Version> {
    line.split_whitespace().last().and_then(Version::parse)
}

/// What this computer can do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LocalSupport {
    /// The app has not asked yet.
    #[default]
    Unknown,
    Available,
    /// Windows has no boundary.
    NotOnThisSystem,
    /// Linux with no bubblewrap.
    NeedsBubblewrap {
        command: String,
    },
    /// The boundary program does not start.
    BoundaryFailed {
        message: String,
    },
    /// This build has no `consort` program.
    NoProgram,
}

impl From<Option<CheckProblem>> for LocalSupport {
    fn from(problem: Option<CheckProblem>) -> Self {
        match problem {
            None => LocalSupport::Available,
            Some(CheckProblem::NoBoundary) => LocalSupport::NotOnThisSystem,
            Some(CheckProblem::NeedsBubblewrap { command }) => {
                LocalSupport::NeedsBubblewrap { command }
            }
            Some(CheckProblem::BoundaryFailed { message }) => {
                LocalSupport::BoundaryFailed { message }
            }
        }
    }
}

/// The state of one step of the install.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StepState {
    #[default]
    Waiting,
    Working,
    Done,
}

/// Where the install is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstallPhase {
    #[default]
    Idle,
    Running,
    Done,
    Failed,
}

/// The install, for the screen with its four steps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstallProgress {
    pub phase: InstallPhase,
    /// The steps in the order of [`InstallStep::ALL`].
    pub steps: [StepState; 4],
    /// What went wrong and what to do next.
    pub error: Option<String>,
}

impl InstallProgress {
    /// The state when this step starts: all earlier steps are done.
    pub fn at(step: InstallStep) -> Self {
        let mut steps = [StepState::Waiting; 4];
        let mut passed = true;
        for (slot, each) in steps.iter_mut().zip(InstallStep::ALL) {
            if each == step {
                *slot = StepState::Working;
                passed = false;
            } else if passed {
                *slot = StepState::Done;
            }
        }
        Self {
            phase: InstallPhase::Running,
            steps,
            error: None,
        }
    }

    /// The state when a step of `consort local install` is done. The program
    /// says "checked" after the check, and the other names when they start.
    pub fn after_line(step: InstallStep) -> Self {
        match step {
            InstallStep::Checked => Self::at(InstallStep::StartingHub),
            other => Self::at(other),
        }
    }
}

/// The text of a step, while it waits or works and when it is done.
pub fn step_label(step: InstallStep, state: StepState) -> &'static str {
    match (step, state) {
        (InstallStep::Checked, StepState::Done) => "Checked that this computer can run agents",
        (InstallStep::Checked, _) => "Checking that this computer can run agents",
        (InstallStep::StartingHub, StepState::Done) => "Started the hub in the background",
        (InstallStep::StartingHub, _) => "Starting the hub in the background",
        (InstallStep::AddingWorker, StepState::Done) => "Added this computer as a worker",
        (InstallStep::AddingWorker, _) => "Adding this computer as a worker",
        (InstallStep::Connecting, StepState::Done) => "Connected this app",
        (InstallStep::Connecting, _) => "Connecting this app",
    }
}

/// What the service manager says about the hub.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HubRun {
    #[default]
    Unknown,
    Running,
    /// The app asked it to start.
    Starting,
    /// It does not answer on its address.
    Stopped,
}

/// A replacement of the program of the service.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SwapState {
    #[default]
    Idle,
    /// Tasks work, and the app must ask first.
    NeedsChoice { working: u32 },
    /// The swap waits until these tasks finish.
    Waiting { working: u32 },
    /// The backup, the stop, the replacement and the start.
    Working,
    /// The new program runs.
    Done,
    /// It did not work. `message` says what happened and what to do next.
    Failed { message: String },
}

/// The hub on this computer, as the app shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalHubView {
    pub support: LocalSupport,
    /// The service exists.
    pub installed: bool,
    /// The folder holds the data of a hub, with or without a service.
    pub has_data: bool,
    /// The app has asked once whether a hub is on this computer. The setup gate
    /// waits for this, so it never shows the wrong first screen.
    pub probed: bool,
    pub address: Option<String>,
    pub past_addresses: Vec<String>,
    pub run: HubRun,
    pub start_at_login: bool,
    pub install: InstallProgress,
    pub swap: SwapState,
    /// The Consort folder, as the W2 screen shows it.
    pub folder: String,
    pub log_file: PathBuf,
    /// The last action that failed. A notice shows it.
    pub error: Option<String>,
}

impl LocalHubView {
    /// A view for a build or a mock with no hub on this computer.
    pub fn none() -> Self {
        Self {
            probed: true,
            ..Self::default()
        }
    }

    /// `hub_url` is the hub on this computer, at its address now or an old one.
    pub fn is_this_hub(&self, hub_url: &str) -> bool {
        let hub_url = hub_url.trim_end_matches('/');
        !hub_url.is_empty()
            && (self.address.as_deref() == Some(hub_url)
                || self.past_addresses.iter().any(|a| a == hub_url))
    }

    /// The new address, when the hub of this client moved to another port.
    pub fn moved_address(&self, hub_url: &str) -> Option<&str> {
        let address = self.address.as_deref()?;
        (address != hub_url.trim_end_matches('/') && self.is_this_hub(hub_url)).then_some(address)
    }

    /// The hub of this app is on this computer and it stopped: W3, with Start
    /// the hub and Show log. Any other offline hub keeps its normal line.
    pub fn stopped_for(&self, hub_url: &str, connection: &Connection) -> bool {
        self.installed
            && self.run == HubRun::Stopped
            && self.is_this_hub(hub_url)
            && matches!(
                connection,
                Connection::Offline { .. } | Connection::Connecting
            )
    }

    /// What the first screen about this hub shows.
    pub fn setup_stage(&self) -> LocalSetupStage {
        match (&self.install.phase, &self.support) {
            (InstallPhase::Failed, _) => LocalSetupStage::Failed {
                message: self.install.error.clone().unwrap_or_default(),
            },
            (InstallPhase::Running, _) => LocalSetupStage::Install,
            (_, LocalSupport::NotOnThisSystem) => LocalSetupStage::NotOnThisSystem,
            (_, LocalSupport::NeedsBubblewrap { command }) => LocalSetupStage::NeedsBubblewrap {
                command: command.clone(),
            },
            (_, LocalSupport::BoundaryFailed { message }) => LocalSetupStage::Failed {
                message: message.clone(),
            },
            (_, LocalSupport::NoProgram) => LocalSetupStage::Failed {
                message: "This build of Consort has no `consort` program.".to_owned(),
            },
            _ if self.installed => LocalSetupStage::Found,
            _ => LocalSetupStage::Choose,
        }
    }
}

/// What W1 and W2 show about this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalSetupStage {
    /// W1: Install here.
    Choose,
    /// The four steps.
    Install,
    NotOnThisSystem,
    NeedsBubblewrap {
        command: String,
    },
    /// The hub did not start, or the boundary does not work.
    Failed {
        message: String,
    },
    /// A hub is on this computer, and this app can connect to it.
    Found,
}

/// The text of the dialog before a restart with tasks that work (V2).
pub fn tasks_title(working: u32) -> String {
    match working {
        1 => "1 task is working".to_owned(),
        n => format!("{n} tasks are working"),
    }
}

/// Runs the `consort` program. The tests use a fake, so no test touches a
/// service manager.
pub trait Program: Send + Sync {
    /// Runs the program with `args`, and gives each line of stderr to
    /// `on_line`. Returns stdout, or a message when the program fails.
    fn run(
        &self,
        program: &Path,
        args: &[String],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<String, String>;

    /// The version that a program says it has.
    fn version(&self, program: &Path) -> Option<Version> {
        let out = self
            .run(program, &["--version".to_owned()], &mut |_| {})
            .ok()?;
        parse_version_line(&out)
    }
}

/// Runs the program on this computer.
pub struct SystemProgram;

impl Program for SystemProgram {
    fn run(
        &self,
        program: &Path,
        args: &[String],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<String, String> {
        use std::io::{BufRead as _, Read as _};
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|err| format!("Consort could not run {}: {err}", program.display()))?;
        let stdout = child.stdout.take();
        let reader = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut stdout) = stdout {
                let _ = stdout.read_to_string(&mut text);
            }
            text
        });
        let mut errors = Vec::new();
        if let Some(stderr) = child.stderr.take() {
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {
                on_line(&line);
                if !line.starts_with(consort_proto::local::PROGRESS_PREFIX) {
                    errors.push(line);
                }
            }
        }
        let status = child.wait().map_err(|err| err.to_string())?;
        let out = reader.join().unwrap_or_default();
        if status.success() {
            Ok(out)
        } else {
            // The last line has the message. `anyhow` starts it with "Error: ".
            let message = errors
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim_start_matches("Error: ").to_owned())
                .unwrap_or_else(|| format!("{} ended with {status}", program.display()));
            Err(message)
        }
    }
}

/// What the service needs to know.
#[derive(Clone)]
pub struct LocalConfig {
    pub layout: Layout,
    /// The program in the app. `None` when this build has none.
    pub bundled: Option<PathBuf>,
    pub program: Arc<dyn Program>,
    /// The folder for the program of a new app version.
    pub work: PathBuf,
    /// Asks the hub at this port for its version.
    pub health: Arc<dyn Fn(u16) -> Option<String> + Send + Sync>,
}

impl LocalConfig {
    /// The configuration of the desktop app.
    pub fn desktop() -> Option<LocalConfig> {
        let layout = Layout::new(consort_folder()?);
        let bundled = std::env::current_exe().ok().and_then(|exe| {
            let os = crate::update::Platform::current()?.os;
            let path = bundled_program_path(os, &exe)?;
            (path.is_file() && !same_path(&path, &exe)).then_some(path)
        });
        Some(LocalConfig {
            work: layout.local_dir().join("update"),
            layout,
            bundled,
            program: Arc::new(SystemProgram),
            health: Arc::new(http_health),
        })
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Asks for `/healthz` on loopback, over plain HTTP with no proxy.
fn http_health(port: u16) -> Option<String> {
    use std::io::{Read as _, Write as _};
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
    value
        .get("ok")?
        .as_bool()?
        .then(|| value.get("version")?.as_str().map(str::to_owned))?
}

type ActionReply = oneshot::Sender<Result<(), ClientError>>;

enum Action {
    Install {
        client: Client,
    },
    Connect {
        client: Client,
    },
    /// Asks again whether this computer can run agents.
    Recheck,
    Start,
    SetLogin(bool),
    Remove {
        keep_data: bool,
    },
    /// The update of the app: the program comes from this archive.
    PrepareUpdate {
        archive: PathBuf,
    },
    /// The choice in the dialog: `wait` means when the tasks finish.
    ChooseSwap {
        wait: bool,
    },
    /// The user closed the dialog. Nothing changes.
    CancelSwap,
    ShowLog,
}

/// A handle to the local hub service. Cheap to clone.
#[derive(Clone)]
pub struct LocalHub {
    view: watch::Receiver<LocalHubView>,
    actions: Option<mpsc::UnboundedSender<(Action, ActionReply)>>,
    fixed: Option<Arc<watch::Sender<LocalHubView>>>,
    _runtime: Option<Arc<crate::RuntimeThread>>,
}

impl LocalHub {
    /// Starts the service.
    pub fn start(config: LocalConfig) -> Result<LocalHub, ClientError> {
        let initial = LocalHubView {
            folder: config.layout.root().display().to_string(),
            log_file: config.layout.log_file(),
            ..LocalHubView::default()
        };
        let (tx, view) = watch::channel(initial);
        let (actions, rx) = mpsc::unbounded_channel();
        let runtime = crate::spawn_runtime("consort-local", move |_stop| async move {
            Service::new(config, tx).run(rx).await;
        })?;
        Ok(LocalHub {
            view,
            actions: Some(actions),
            fixed: None,
            _runtime: Some(Arc::new(runtime)),
        })
    }

    /// A handle that shows `view` and runs nothing. The mock mode uses it.
    pub fn fixed(view: LocalHubView) -> LocalHub {
        let (tx, rx) = watch::channel(view);
        LocalHub {
            view: rx,
            actions: None,
            fixed: Some(Arc::new(tx)),
            _runtime: None,
        }
    }

    pub fn view(&self) -> LocalHubView {
        self.view.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<LocalHubView> {
        self.view.clone()
    }

    async fn act(&self, action: Action) -> Result<(), ClientError> {
        let Some(actions) = &self.actions else {
            if let Some(fixed) = &self.fixed {
                fixed.send_modify(|view| apply_to_fixed(view, &action));
            }
            return Ok(());
        };
        let (reply, answer) = oneshot::channel();
        actions
            .send((action, reply))
            .map_err(|_| ClientError::Other("The local hub service stopped.".to_owned()))?;
        answer
            .await
            .map_err(|_| ClientError::Other("The local hub service stopped.".to_owned()))?
    }

    /// Runs the install and pairs this app. The view holds the four steps.
    pub async fn install_and_connect(&self, client: Client) -> Result<(), ClientError> {
        self.act(Action::Install { client }).await
    }

    /// Pairs this app with a hub that is on this computer, with no typed code.
    pub async fn connect(&self, client: Client) -> Result<(), ClientError> {
        self.act(Action::Connect { client }).await
    }

    /// Asks again whether this computer can run agents, after the user changed
    /// something, such as an install of bubblewrap.
    pub async fn recheck(&self) -> Result<(), ClientError> {
        self.act(Action::Recheck).await
    }

    /// Starts the service, and returns when the hub answers.
    pub async fn start_hub(&self) -> Result<(), ClientError> {
        self.act(Action::Start).await
    }

    /// Turns the start at login on or off. A running hub keeps running.
    pub async fn set_start_at_login(&self, on: bool) -> Result<(), ClientError> {
        self.act(Action::SetLogin(on)).await
    }

    /// Stops the service and removes it.
    pub async fn remove(&self, keep_data: bool) -> Result<(), ClientError> {
        self.act(Action::Remove { keep_data }).await
    }

    /// Gets the program of the new app from its archive, and replaces the
    /// program of the service. When tasks work, `swap` says
    /// [`SwapState::NeedsChoice`] and the app asks.
    pub async fn prepare_update(&self, archive: PathBuf) -> Result<(), ClientError> {
        self.act(Action::PrepareUpdate { archive }).await
    }

    /// The answer of the user to [`SwapState::NeedsChoice`].
    pub async fn choose_swap(&self, wait: bool) -> Result<(), ClientError> {
        self.act(Action::ChooseSwap { wait }).await
    }

    /// The user closed the dialog with no choice. The swap does not run.
    pub async fn cancel_swap(&self) -> Result<(), ClientError> {
        self.act(Action::CancelSwap).await
    }

    /// Opens the log of the hub.
    pub async fn show_log(&self) -> Result<(), ClientError> {
        self.act(Action::ShowLog).await
    }
}

/// What a mock does for an action, so a screen answers a click.
fn apply_to_fixed(view: &mut LocalHubView, action: &Action) {
    match action {
        Action::SetLogin(on) => view.start_at_login = *on,
        Action::Start => view.run = HubRun::Running,
        Action::Remove { .. } => {
            view.installed = false;
            view.run = HubRun::Unknown;
        }
        Action::Install { .. } => {
            view.install = InstallProgress {
                phase: InstallPhase::Done,
                steps: [StepState::Done; 4],
                error: None,
            };
        }
        Action::ChooseSwap { wait } => {
            let working = match view.swap {
                SwapState::NeedsChoice { working } => working,
                _ => 0,
            };
            view.swap = if *wait {
                SwapState::Waiting { working }
            } else {
                SwapState::Done
            };
        }
        Action::CancelSwap => view.swap = SwapState::Idle,
        Action::Recheck => view.support = LocalSupport::Available,
        Action::Connect { .. } | Action::PrepareUpdate { .. } | Action::ShowLog => {}
    }
}

/// Opens a file with the program of this system.
fn open_file(path: &Path) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("Consort could not open {}: {err}", path.display()))
}

fn read_state(layout: &Layout) -> Option<LocalState> {
    std::fs::read(layout.state_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// The arguments of a command.
fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|a| (*a).to_owned()).collect()
}

struct Service {
    config: LocalConfig,
    view: watch::Sender<LocalHubView>,
    fails: u32,
    /// The program of a swap that waits for a choice or for the tasks.
    pending: Option<PathBuf>,
    waiting: bool,
}

impl Service {
    fn new(config: LocalConfig, view: watch::Sender<LocalHubView>) -> Self {
        Self {
            config,
            view,
            fails: 0,
            pending: None,
            waiting: false,
        }
    }

    /// The program to run: the installed copy, else the one in the app.
    fn program(&self) -> Option<PathBuf> {
        let installed = self.config.layout.program(cfg!(windows));
        if installed.is_file() {
            return Some(installed);
        }
        self.config.bundled.clone()
    }

    fn publish(&self, change: impl FnOnce(&mut LocalHubView)) {
        self.view.send_modify(change);
    }

    async fn run_program(&self, command: Vec<String>) -> Result<String, String> {
        let Some(program) = self.program() else {
            return Err("This build of Consort has no `consort` program.".to_owned());
        };
        let runner = self.config.program.clone();
        tokio::task::spawn_blocking(move || runner.run(&program, &command, &mut |_| {}))
            .await
            .map_err(|err| err.to_string())?
    }

    async fn run_json<T: serde::de::DeserializeOwned>(
        &self,
        command: Vec<String>,
    ) -> Result<T, String> {
        let out = self.run_program(command).await?;
        serde_json::from_str(&out)
            .map_err(|_| "The consort program answered in a form this app cannot read.".to_owned())
    }

    async fn refresh_status(&self) -> Option<StatusReport> {
        let status: Result<StatusReport, String> = self.run_json(args(&["local", "status"])).await;
        let status = status.ok()?;
        self.publish(|view| {
            view.installed = status.installed;
            view.has_data = status.has_data;
            view.start_at_login = status.start_at_login;
            view.run = if status.healthy {
                HubRun::Running
            } else if status.installed {
                HubRun::Stopped
            } else {
                HubRun::Unknown
            };
        });
        Some(status)
    }

    async fn check_support(&self) {
        if cfg!(windows) {
            self.publish(|view| view.support = LocalSupport::NotOnThisSystem);
            return;
        }
        let support = match self.config.bundled.is_some() || self.program().is_some() {
            false => LocalSupport::NoProgram,
            true => {
                let report: Result<CheckReport, String> =
                    self.run_json(args(&["local", "check"])).await;
                match report {
                    Ok(report) => report.problem.into(),
                    Err(message) => LocalSupport::BoundaryFailed { message },
                }
            }
        };
        self.publish(|view| view.support = support);
    }

    /// Reads the state file and asks the hub.
    fn poll(&mut self) {
        let state = read_state(&self.config.layout);
        let installed = self.view.borrow().installed;
        let Some(state) = state else {
            return;
        };
        let answers = (self.config.health)(state.port).is_some();
        self.fails = if answers { 0 } else { self.fails + 1 };
        let fails = self.fails;
        self.publish(|view| {
            view.address = Some(state.address.clone());
            view.past_addresses.clone_from(&state.past_addresses);
            if answers {
                view.run = HubRun::Running;
            } else if installed && fails >= FAILS_FOR_STOPPED && view.run != HubRun::Starting {
                view.run = HubRun::Stopped;
            }
        });
    }

    async fn run(mut self, mut actions: mpsc::UnboundedReceiver<(Action, ActionReply)>) {
        self.check_support().await;
        self.refresh_status().await;
        self.poll();
        self.publish(|view| view.probed = true);
        self.reconcile().await;
        let mut tick = tokio::time::interval(POLL);
        let mut task_tick = tokio::time::interval(TASK_POLL);
        loop {
            tokio::select! {
                _ = tick.tick() => self.poll(),
                _ = task_tick.tick(), if self.waiting => self.run_waiting_swap().await,
                action = actions.recv() => {
                    let Some((action, reply)) = action else { return };
                    let result = self.handle(action).await;
                    if let Err(err) = &result {
                        tracing::warn!(error = %err, "a local hub action failed");
                    }
                    let _ = reply.send(result);
                }
            }
        }
    }

    /// At a start of the app: a program in the app that is newer than the copy
    /// of the service replaces it, for example after a manual app replacement.
    async fn reconcile(&mut self) {
        if !self.view.borrow().installed {
            return;
        }
        let Some(bundled) = self.config.bundled.clone() else {
            return;
        };
        let runner = self.config.program.clone();
        let installed_path = self.config.layout.program(cfg!(windows));
        let bundled_path = bundled.clone();
        let versions = tokio::task::spawn_blocking(move || {
            (
                runner.version(&bundled_path),
                runner.version(&installed_path),
            )
        })
        .await;
        let Ok((new, old)) = versions else {
            return;
        };
        if matches!(
            swap_decision(new.as_ref(), old.as_ref()),
            SwapDecision::Upgrade { .. }
        ) {
            self.begin_swap(bundled).await;
        }
    }

    /// Starts a swap, or asks first when tasks work.
    async fn begin_swap(&mut self, program: PathBuf) {
        let working = self.refresh_status().await.map_or(0, |s| s.working_tasks);
        if working > 0 {
            self.pending = Some(program);
            self.publish(|view| view.swap = SwapState::NeedsChoice { working });
            return;
        }
        self.swap_now(program).await;
    }

    async fn swap_now(&mut self, program: PathBuf) {
        self.waiting = false;
        self.publish(|view| view.swap = SwapState::Working);
        let command = vec![
            "local".to_owned(),
            "upgrade".to_owned(),
            "--program".to_owned(),
            program.display().to_string(),
        ];
        let result: Result<UpgradeReport, String> = self.run_json(command).await;
        let swap = match result {
            Ok(report) => match report.outcome {
                UpgradeOutcome::Replaced | UpgradeOutcome::Unchanged => SwapState::Done,
                UpgradeOutcome::RolledBack | UpgradeOutcome::RollbackFailed => SwapState::Failed {
                    message: report.message,
                },
            },
            Err(message) => SwapState::Failed { message },
        };
        self.publish(|view| view.swap = swap);
        // The program is large, and the service has its own copy now.
        let _ = std::fs::remove_dir_all(&self.config.work);
        self.refresh_status().await;
    }

    async fn run_waiting_swap(&mut self) {
        let working = self.refresh_status().await.map_or(0, |s| s.working_tasks);
        if working > 0 {
            self.publish(|view| view.swap = SwapState::Waiting { working });
            return;
        }
        match self.pending.take() {
            Some(program) => self.swap_now(program).await,
            None => self.waiting = false,
        }
    }

    async fn handle(&mut self, action: Action) -> Result<(), ClientError> {
        let other = ClientError::Other;
        match action {
            Action::Install { client } => self.install(client).await,
            Action::Recheck => {
                self.check_support().await;
                Ok(())
            }
            Action::Connect { client } => {
                // A hub that stopped cannot pair this app.
                self.poll();
                if self.view.borrow().run != HubRun::Running {
                    self.publish(|view| view.run = HubRun::Starting);
                    let started = self.run_program(args(&["local", "start"])).await;
                    self.fails = 0;
                    self.refresh_status().await;
                    self.poll();
                    started.map_err(other)?;
                }
                let report: SetupCodeReport = self
                    .run_json(args(&["local", "setup-code"]))
                    .await
                    .map_err(other)?;
                client.pair(report.address, report.setup_code).await?;
                self.poll();
                Ok(())
            }
            Action::Start => {
                self.publish(|view| view.run = HubRun::Starting);
                let result = self.run_program(args(&["local", "start"])).await;
                self.fails = 0;
                self.refresh_status().await;
                self.poll();
                result.map(|_| ()).map_err(other)
            }
            Action::SetLogin(on) => {
                let state = if on { "on" } else { "off" };
                self.run_program(args(&["local", "login", state]))
                    .await
                    .map_err(other)?;
                self.refresh_status().await;
                Ok(())
            }
            Action::Remove { keep_data } => {
                let mut command = args(&["local", "uninstall"]);
                if !keep_data {
                    command.push("--delete-data".to_owned());
                }
                // The program removes its own file. A running program on a Mac or
                // on Linux keeps working after that.
                self.run_program(command).await.map_err(other)?;
                self.publish(|view| {
                    view.installed = false;
                    view.run = HubRun::Unknown;
                    view.address = None;
                    view.past_addresses.clear();
                    view.has_data = keep_data;
                    view.install = InstallProgress::default();
                });
                Ok(())
            }
            Action::PrepareUpdate { archive } => {
                if !self.view.borrow().installed {
                    self.publish(|view| view.swap = SwapState::Done);
                    return Ok(());
                }
                let work = self.config.work.clone();
                let extracted = tokio::task::spawn_blocking(move || {
                    crate::update::apply::extract_program(&archive, &work)
                })
                .await
                .map_err(|err| other(err.to_string()))?;
                match extracted.map_err(|err| other(err.to_string()))? {
                    Some(program) => self.begin_swap(program).await,
                    // A release from before the app carried the program.
                    None => self.publish(|view| view.swap = SwapState::Done),
                }
                Ok(())
            }
            Action::ChooseSwap { wait } => {
                let working = self.view.borrow().swap.clone();
                if wait {
                    self.waiting = true;
                    if let SwapState::NeedsChoice { working } = working {
                        self.publish(|view| view.swap = SwapState::Waiting { working });
                    }
                    return Ok(());
                }
                if let Some(program) = self.pending.take() {
                    self.swap_now(program).await;
                }
                Ok(())
            }
            Action::CancelSwap => {
                self.waiting = false;
                self.pending = None;
                self.publish(|view| view.swap = SwapState::Idle);
                Ok(())
            }
            Action::ShowLog => {
                let path = self.config.layout.log_file();
                open_file(&path).map_err(other)
            }
        }
    }

    async fn install(&mut self, client: Client) -> Result<(), ClientError> {
        self.publish(|view| view.install = InstallProgress::at(InstallStep::Checked));
        // The program of the app runs the install: it copies itself.
        let Some(bundled) = self.config.bundled.clone().or_else(|| self.program()) else {
            let message = "This build of Consort has no `consort` program.".to_owned();
            self.fail_install(message.clone());
            return Err(ClientError::Other(message));
        };
        let runner = self.config.program.clone();
        let view = self.view.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut on_line = |line: &str| {
                if let Some(step) = InstallStep::from_progress_line(line) {
                    view.send_modify(|v| v.install = InstallProgress::after_line(step));
                }
            };
            runner.run(
                &bundled,
                &args(&["local", "install", "--progress"]),
                &mut on_line,
            )
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|r| r);
        let out = match result {
            Ok(out) => out,
            Err(message) => {
                self.fail_install(message.clone());
                return Err(ClientError::Other(message));
            }
        };
        let report: InstallReport = match serde_json::from_str(&out) {
            Ok(report) => report,
            Err(_) => {
                let message =
                    "The consort program answered in a form this app cannot read.".to_owned();
                self.fail_install(message.clone());
                return Err(ClientError::Other(message));
            }
        };
        self.publish(|view| {
            view.install = InstallProgress::at(InstallStep::Connecting);
            view.address = Some(report.address.clone());
        });
        if let Err(err) = client.pair(report.address, report.setup_code).await {
            self.fail_install(err.to_string());
            return Err(err);
        }
        self.publish(|view| {
            view.install = InstallProgress {
                phase: InstallPhase::Done,
                steps: [StepState::Done; 4],
                error: None,
            };
        });
        self.refresh_status().await;
        self.poll();
        Ok(())
    }

    fn fail_install(&self, message: String) {
        self.publish(|view| {
            view.install.phase = InstallPhase::Failed;
            view.install.error = Some(message);
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::update::Os;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn a_bundle_keeps_the_program_where_it_cannot_overwrite_the_app() {
        assert_eq!(
            bundled_program_path(
                Os::MacOs,
                Path::new("/Applications/Consort.app/Contents/MacOS/Consort")
            ),
            Some(PathBuf::from(
                "/Applications/Consort.app/Contents/Helpers/consort"
            ))
        );
        assert_eq!(
            bundled_program_path(Os::Linux, Path::new("/opt/Consort/consort-desktop")),
            Some(PathBuf::from("/opt/Consort/consort"))
        );
        assert_eq!(
            bundled_program_path(Os::Windows, Path::new("C:/Consort/consort-desktop.exe")),
            Some(PathBuf::from("C:/Consort/consort.exe"))
        );
    }

    #[test]
    fn a_debug_build_finds_the_program_beside_it() {
        assert_eq!(
            bundled_program_path(Os::MacOs, Path::new("/r/target/debug/consort-desktop")),
            Some(PathBuf::from("/r/target/debug/consort"))
        );
    }

    #[test]
    fn only_a_newer_program_in_the_app_replaces_the_copy() {
        assert_eq!(
            swap_decision(Some(&v("0.9.0")), Some(&v("0.8.0"))),
            SwapDecision::Upgrade {
                from: v("0.8.0"),
                to: v("0.9.0")
            }
        );
        assert_eq!(
            swap_decision(Some(&v("0.8.0")), Some(&v("0.9.0"))),
            SwapDecision::AppTooOld
        );
        assert_eq!(
            swap_decision(Some(&v("0.8.0")), Some(&v("0.8.0"))),
            SwapDecision::Nothing
        );
        assert_eq!(
            swap_decision(None, Some(&v("0.8.0"))),
            SwapDecision::Nothing
        );
        assert_eq!(
            swap_decision(Some(&v("0.8.0")), None),
            SwapDecision::Nothing
        );
    }

    #[test]
    fn the_version_line_of_the_program_reads() {
        assert_eq!(parse_version_line("consort 0.8.0\n"), Some(v("0.8.0")));
        assert_eq!(parse_version_line("nothing"), None);
    }

    #[test]
    fn the_steps_of_the_install_fill_up_in_order() {
        let p = InstallProgress::at(InstallStep::Checked);
        assert_eq!(p.steps[0], StepState::Working);
        assert_eq!(p.steps[1], StepState::Waiting);
        let p = InstallProgress::after_line(InstallStep::Checked);
        assert_eq!(
            p.steps,
            [
                StepState::Done,
                StepState::Working,
                StepState::Waiting,
                StepState::Waiting
            ]
        );
        let p = InstallProgress::after_line(InstallStep::AddingWorker);
        assert_eq!(p.steps[1], StepState::Done);
        assert_eq!(p.steps[2], StepState::Working);
        assert_eq!(p.phase, InstallPhase::Running);
    }

    #[test]
    fn the_step_texts_match_the_design() {
        assert_eq!(
            step_label(InstallStep::Checked, StepState::Done),
            "Checked that this computer can run agents"
        );
        assert_eq!(
            step_label(InstallStep::StartingHub, StepState::Working),
            "Starting the hub in the background"
        );
        assert_eq!(
            step_label(InstallStep::Connecting, StepState::Waiting),
            "Connecting this app"
        );
    }

    #[test]
    fn a_stopped_hub_shows_only_for_the_hub_of_this_app() {
        let view = LocalHubView {
            installed: true,
            run: HubRun::Stopped,
            address: Some("http://127.0.0.1:7700".into()),
            past_addresses: vec!["http://127.0.0.1:7600".into()],
            ..LocalHubView::default()
        };
        let offline = Connection::Offline {
            retry_in_ms: 1000,
            reason: "refused".into(),
        };
        assert!(view.stopped_for("http://127.0.0.1:7700/", &offline));
        assert!(view.stopped_for("http://127.0.0.1:7600", &offline));
        assert!(
            !view.stopped_for("https://hub.example.com", &offline),
            "a hub on a server is not this one"
        );
        assert!(
            !view.stopped_for("http://127.0.0.1:7700", &Connection::Online),
            "a link that works has no card"
        );
        let running = LocalHubView {
            run: HubRun::Running,
            ..view.clone()
        };
        assert!(!running.stopped_for("http://127.0.0.1:7700", &offline));
        assert!(!LocalHubView::default().stopped_for("", &offline));
    }

    #[test]
    fn a_client_follows_only_the_old_address_of_this_hub() {
        let view = LocalHubView {
            address: Some("http://127.0.0.1:7700".into()),
            past_addresses: vec!["http://127.0.0.1:7600".into()],
            ..LocalHubView::default()
        };
        assert_eq!(
            view.moved_address("http://127.0.0.1:7600"),
            Some("http://127.0.0.1:7700")
        );
        assert_eq!(view.moved_address("http://127.0.0.1:7700"), None);
        assert_eq!(
            view.moved_address("http://127.0.0.1:9999"),
            None,
            "a dev hub on another port stays"
        );
    }

    #[test]
    fn the_title_counts_the_tasks() {
        assert_eq!(tasks_title(1), "1 task is working");
        assert_eq!(tasks_title(2), "2 tasks are working");
    }

    #[test]
    fn the_check_of_the_program_maps_to_what_the_app_shows() {
        assert_eq!(LocalSupport::from(None), LocalSupport::Available);
        assert_eq!(
            LocalSupport::from(Some(CheckProblem::NoBoundary)),
            LocalSupport::NotOnThisSystem
        );
        assert_eq!(
            LocalSupport::from(Some(CheckProblem::NeedsBubblewrap {
                command: "sudo apt install bubblewrap".into()
            })),
            LocalSupport::NeedsBubblewrap {
                command: "sudo apt install bubblewrap".into()
            }
        );
    }

    /// Answers from a table of commands, and records them.
    #[derive(Default)]
    struct FakeProgram {
        seen: Mutex<Vec<String>>,
        status: Mutex<StatusReport>,
        fail: Option<&'static str>,
        upgrade: Mutex<Option<UpgradeOutcome>>,
        bundled_version: &'static str,
        installed_version: &'static str,
    }

    impl Program for FakeProgram {
        fn run(
            &self,
            program: &Path,
            args: &[String],
            on_line: &mut dyn FnMut(&str),
        ) -> Result<String, String> {
            if args == ["--version"] {
                let installed = program.ends_with("bin/consort");
                return Ok(format!(
                    "consort {}",
                    if installed {
                        self.installed_version
                    } else {
                        self.bundled_version
                    }
                ));
            }
            let line = args.join(" ");
            self.seen.lock().unwrap().push(line.clone());
            let verb = args.get(1).map(String::as_str).unwrap_or_default();
            if self.fail == Some(verb) {
                return Err("The hub did not start. Nothing else changed.".to_owned());
            }
            match verb {
                "check" => Ok(r#"{"problem":null}"#.to_owned()),
                "status" => Ok(serde_json::to_string(&*self.status.lock().unwrap()).unwrap()),
                "install" => {
                    on_line("consort-progress: checked");
                    on_line("consort-progress: starting_hub");
                    on_line("consort-progress: adding_worker");
                    Ok(serde_json::to_string(&InstallReport {
                        address: "http://127.0.0.1:7700".into(),
                        setup_code: "ABCD-EFGH".into(),
                        worker_id: "wrk_1".into(),
                        version: "0.8.0".into(),
                    })
                    .unwrap())
                }
                "upgrade" => {
                    let outcome = self
                        .upgrade
                        .lock()
                        .unwrap()
                        .unwrap_or(UpgradeOutcome::Replaced);
                    Ok(serde_json::to_string(&UpgradeReport {
                        outcome,
                        from: "0.8.0".into(),
                        to: "0.9.0".into(),
                        backup: None,
                        message: if outcome == UpgradeOutcome::Replaced {
                            String::new()
                        } else {
                            "Consort put back version 0.8.0.".to_owned()
                        },
                    })
                    .unwrap())
                }
                _ => Ok("{}".to_owned()),
            }
        }
    }

    fn service(fake: Arc<FakeProgram>, dir: &Path) -> (Service, watch::Receiver<LocalHubView>) {
        let layout = Layout::new(dir);
        std::fs::create_dir_all(layout.bin_dir()).unwrap();
        std::fs::write(layout.program(false), b"x").unwrap();
        let (tx, rx) = watch::channel(LocalHubView::default());
        (
            Service::new(
                LocalConfig {
                    layout,
                    bundled: Some(dir.join("bundled-consort")),
                    program: fake,
                    work: dir.join("work"),
                    health: Arc::new(|_| None),
                },
                tx,
            ),
            rx,
        )
    }

    #[tokio::test]
    async fn connect_starts_a_hub_that_stopped_before_it_asks_for_a_code() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeProgram::default());
        fake.status.lock().unwrap().installed = true;
        let (mut svc, rx) = service(fake.clone(), dir.path());
        assert_ne!(rx.borrow().run, HubRun::Running);
        let client = Client::mock().unwrap();
        let _ = svc.handle(Action::Connect { client }).await;
        let seen = fake.seen.lock().unwrap();
        let start = seen.iter().position(|c| c.contains("local start"));
        assert!(start.is_some(), "the hub starts first: {seen:?}");
    }

    #[tokio::test]
    async fn the_program_of_the_app_runs_the_install_and_the_app_pairs_with_its_code() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeProgram::default());
        let (mut svc, rx) = service(fake.clone(), dir.path());
        let client = Client::mock().unwrap();
        svc.install(client).await.unwrap();
        let view = rx.borrow().clone();
        assert_eq!(view.install.phase, InstallPhase::Done);
        assert_eq!(view.install.steps, [StepState::Done; 4]);
        assert_eq!(view.address.as_deref(), Some("http://127.0.0.1:7700"));
        assert!(
            fake.seen.lock().unwrap()[0].contains("local install --progress"),
            "{:?}",
            fake.seen
        );
    }

    #[tokio::test]
    async fn a_failed_install_keeps_the_steps_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeProgram {
            fail: Some("install"),
            ..FakeProgram::default()
        });
        let (mut svc, rx) = service(fake, dir.path());
        assert!(svc.install(Client::mock().unwrap()).await.is_err());
        let view = rx.borrow().clone();
        assert_eq!(view.install.phase, InstallPhase::Failed);
        assert!(
            view.install
                .error
                .as_deref()
                .is_some_and(|e| e.contains("did not start"))
        );
    }

    #[tokio::test]
    async fn working_tasks_make_the_app_ask_before_the_swap() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeProgram::default());
        fake.status.lock().unwrap().working_tasks = 2;
        fake.status.lock().unwrap().installed = true;
        let (mut svc, rx) = service(fake.clone(), dir.path());
        svc.begin_swap(PathBuf::from("/new/consort")).await;
        assert_eq!(rx.borrow().swap, SwapState::NeedsChoice { working: 2 });
        assert!(
            !fake
                .seen
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("upgrade")),
            "nothing changes before the choice"
        );
        // Restart when they finish: it waits while tasks work.
        svc.handle(Action::ChooseSwap { wait: true }).await.unwrap();
        assert_eq!(rx.borrow().swap, SwapState::Waiting { working: 2 });
        svc.run_waiting_swap().await;
        assert_eq!(rx.borrow().swap, SwapState::Waiting { working: 2 });
        fake.status.lock().unwrap().working_tasks = 0;
        svc.run_waiting_swap().await;
        assert_eq!(rx.borrow().swap, SwapState::Done);
        assert!(!svc.waiting);
        assert!(
            fake.seen
                .lock()
                .unwrap()
                .iter()
                .any(|l| l == "local upgrade --program /new/consort")
        );
    }

    #[tokio::test]
    async fn restart_now_swaps_at_once_and_a_rollback_is_a_failure_with_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeProgram::default());
        fake.status.lock().unwrap().working_tasks = 1;
        *fake.upgrade.lock().unwrap() = Some(UpgradeOutcome::RolledBack);
        let (mut svc, rx) = service(fake.clone(), dir.path());
        svc.begin_swap(PathBuf::from("/new/consort")).await;
        svc.handle(Action::ChooseSwap { wait: false })
            .await
            .unwrap();
        assert_eq!(
            rx.borrow().swap,
            SwapState::Failed {
                message: "Consort put back version 0.8.0.".into()
            }
        );
    }

    #[tokio::test]
    async fn a_start_with_a_newer_program_in_the_app_swaps_and_an_older_one_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let newer = Arc::new(FakeProgram {
            bundled_version: "0.9.0",
            installed_version: "0.8.0",
            ..FakeProgram::default()
        });
        newer.status.lock().unwrap().installed = true;
        let (mut svc, rx) = service(newer.clone(), dir.path());
        svc.refresh_status().await;
        svc.reconcile().await;
        assert_eq!(rx.borrow().swap, SwapState::Done);

        let older = Arc::new(FakeProgram {
            bundled_version: "0.7.0",
            installed_version: "0.8.0",
            ..FakeProgram::default()
        });
        older.status.lock().unwrap().installed = true;
        let (mut svc, rx) = service(older.clone(), dir.path());
        svc.refresh_status().await;
        svc.reconcile().await;
        assert_eq!(rx.borrow().swap, SwapState::Idle);
        assert!(
            !older
                .seen
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("upgrade")),
            "an app that is too old never replaces a newer hub"
        );
    }

    #[tokio::test]
    async fn a_hub_that_stops_shows_after_two_failed_looks_and_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeProgram::default());
        let (mut svc, rx) = service(fake, dir.path());
        std::fs::create_dir_all(svc.config.layout.local_dir()).unwrap();
        std::fs::write(
            svc.config.layout.state_file(),
            r#"{"address":"http://127.0.0.1:9","port":9,"version":"0.8.0","pid":1}"#,
        )
        .unwrap();
        svc.publish(|view| {
            view.installed = true;
            view.run = HubRun::Running;
        });
        svc.poll();
        assert_eq!(
            rx.borrow().run,
            HubRun::Running,
            "one failed look is a restart"
        );
        svc.poll();
        assert_eq!(rx.borrow().run, HubRun::Stopped);
        svc.config.health = Arc::new(|_| Some("0.8.0".into()));
        svc.poll();
        assert_eq!(rx.borrow().run, HubRun::Running);
        assert_eq!(rx.borrow().address.as_deref(), Some("http://127.0.0.1:9"));
    }

    #[tokio::test]
    async fn the_remove_keeps_or_deletes_the_data_as_asked() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeProgram::default());
        let (mut svc, rx) = service(fake.clone(), dir.path());
        svc.handle(Action::Remove { keep_data: true })
            .await
            .unwrap();
        assert!(rx.borrow().has_data);
        assert!(!rx.borrow().installed);
        svc.handle(Action::Remove { keep_data: false })
            .await
            .unwrap();
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0], "local uninstall");
        assert_eq!(seen[1], "local uninstall --delete-data");
    }

    #[tokio::test]
    async fn closing_the_dialog_cancels_the_swap() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeProgram::default());
        fake.status.lock().unwrap().working_tasks = 1;
        let (mut svc, rx) = service(fake.clone(), dir.path());
        svc.begin_swap(PathBuf::from("/new/consort")).await;
        svc.handle(Action::CancelSwap).await.unwrap();
        assert_eq!(rx.borrow().swap, SwapState::Idle);
        svc.handle(Action::ChooseSwap { wait: false })
            .await
            .unwrap();
        assert!(
            !fake
                .seen
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("upgrade")),
            "a cancelled swap never runs"
        );
    }

    #[test]
    fn the_setup_screen_follows_the_state() {
        let base = LocalHubView {
            support: LocalSupport::Available,
            ..LocalHubView::default()
        };
        assert_eq!(base.setup_stage(), LocalSetupStage::Choose);
        let running = LocalHubView {
            install: InstallProgress::at(InstallStep::Checked),
            ..base.clone()
        };
        assert_eq!(running.setup_stage(), LocalSetupStage::Install);
        // An install that ended long ago says nothing about this screen.
        let done = LocalHubView {
            install: InstallProgress {
                phase: InstallPhase::Done,
                ..InstallProgress::default()
            },
            ..base.clone()
        };
        assert_eq!(done.setup_stage(), LocalSetupStage::Choose);
        let found = LocalHubView {
            installed: true,
            ..base.clone()
        };
        assert_eq!(found.setup_stage(), LocalSetupStage::Found);
        let windows = LocalHubView {
            support: LocalSupport::NotOnThisSystem,
            ..base.clone()
        };
        assert_eq!(windows.setup_stage(), LocalSetupStage::NotOnThisSystem);
        let failed = LocalHubView {
            install: InstallProgress {
                phase: InstallPhase::Failed,
                error: Some("no".into()),
                ..InstallProgress::default()
            },
            ..windows
        };
        assert_eq!(
            failed.setup_stage(),
            LocalSetupStage::Failed {
                message: "no".into()
            }
        );
    }

    #[test]
    fn only_a_probed_view_says_where_the_hub_is() {
        use crate::setup::LocalPresence;
        assert_eq!(
            LocalPresence::from(&LocalHubView::default()),
            LocalPresence::Unknown,
            "the service has not asked yet"
        );
        assert_eq!(
            LocalPresence::from(&LocalHubView::none()),
            LocalPresence::None,
            "a build with no local hub service knows"
        );
        let found = LocalHubView {
            probed: true,
            installed: true,
            ..LocalHubView::default()
        };
        assert_eq!(LocalPresence::from(&found), LocalPresence::Installed);
    }

    #[tokio::test]
    async fn a_mock_handle_answers_a_click() {
        let hub = LocalHub::fixed(LocalHubView {
            start_at_login: true,
            run: HubRun::Stopped,
            ..LocalHubView::default()
        });
        hub.set_start_at_login(false).await.unwrap();
        assert!(!hub.view().start_at_login);
        hub.start_hub().await.unwrap();
        assert_eq!(hub.view().run, HubRun::Running);
    }
}
