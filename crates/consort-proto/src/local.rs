//! A hub on the computer of the user: the folder layout, the state file, and
//! the JSON that the `consort local` commands print. The app and the program
//! read the same types. This is pure data, with no I/O.
//!
//! The Consort folder also holds the files of the app, and a `consort hub` or
//! a `consort worker` with their default folders can use it. So all parts of
//! the local hub live in `local/`, and the program lives in `bin/`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Uses this folder in place of the Consort folder. The tests use it.
pub const DIR_ENV: &str = "CONSORT_LOCAL_DIR";
/// Uses this service name in place of [`SERVICE_LABEL`]. The tests use it.
pub const LABEL_ENV: &str = "CONSORT_LOCAL_LABEL";
/// The launchd label of the service. The systemd unit is `consort-local.service`.
pub const SERVICE_LABEL: &str = "dev.consort.local";
/// The stderr lines of `consort local install --progress` start with this.
pub const PROGRESS_PREFIX: &str = "consort-progress: ";

/// The paths of a local hub, under one Consort folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// The layout in this Consort folder.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The Consort folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder of the program.
    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    /// The copy of the program that the service runs.
    pub fn program(&self, windows: bool) -> PathBuf {
        self.bin_dir()
            .join(if windows { "consort.exe" } else { "consort" })
    }

    /// The copy before the last replacement, for a rollback.
    pub fn previous_program(&self, windows: bool) -> PathBuf {
        self.bin_dir().join(if windows {
            "consort.previous.exe"
        } else {
            "consort.previous"
        })
    }

    /// The folder of everything that the local hub keeps.
    pub fn local_dir(&self) -> PathBuf {
        self.root.join("local")
    }

    /// The state file: [`LocalState`].
    pub fn state_file(&self) -> PathBuf {
        self.local_dir().join("state.json")
    }

    /// The data folder of the hub.
    pub fn hub_dir(&self) -> PathBuf {
        self.local_dir().join("hub")
    }

    /// The folder of the session host.
    pub fn session_host_dir(&self) -> PathBuf {
        self.local_dir().join("session-host")
    }

    /// The Consort home: the account profiles and the managed tools.
    pub fn home_dir(&self) -> PathBuf {
        self.local_dir().join("home")
    }

    /// The folder of the worker of this computer.
    pub fn worker_dir(&self) -> PathBuf {
        self.local_dir().join("worker")
    }

    /// The backups that an update makes.
    pub fn backup_dir(&self) -> PathBuf {
        self.local_dir().join("backups")
    }

    /// The log of the service.
    pub fn log_file(&self) -> PathBuf {
        self.local_dir().join("logs").join("local.log")
    }

    /// The private copy of the service definition. On macOS this file loads
    /// the service, and a copy in `~/Library/LaunchAgents` starts it at login.
    pub fn service_file(&self) -> PathBuf {
        self.local_dir().join("service.plist")
    }
}

/// The address of a hub on this port of loopback.
pub fn address_of(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// The state file. `consort local run` writes it with 0600 rights, and the app
/// reads it. It holds no secret.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalState {
    /// The address of the hub, as [`address_of`] makes it.
    pub address: String,
    /// The port. The next start uses it again when it is free.
    pub port: u16,
    /// The version of the program that wrote the file.
    pub version: String,
    /// The process of the service. `None` after a clean stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// The worker of this computer, when it is paired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    /// The worker of this computer is connected to the hub.
    #[serde(default)]
    pub worker_connected: bool,
    /// The session host is connected to the hub.
    #[serde(default)]
    pub session_host_connected: bool,
    /// The addresses that this hub had before, because its port was taken at a
    /// start. A client paired with one of them moves to [`Self::address`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub past_addresses: Vec<String>,
}

impl LocalState {
    /// The hub and the worker of this computer are up.
    pub fn ready(&self) -> bool {
        self.pid.is_some() && self.worker_connected
    }
}

/// What `consort local install` prints on stdout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReport {
    pub address: String,
    /// A client setup code. It works once and is valid for 10 minutes.
    pub setup_code: String,
    pub worker_id: String,
    pub version: String,
}

/// What `consort local setup-code` prints on stdout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupCodeReport {
    pub address: String,
    pub setup_code: String,
}

/// What `consort local status` prints on stdout.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    /// The service definition exists.
    pub installed: bool,
    /// The service manager has the service and it runs.
    pub running: bool,
    /// The hub answers on its address.
    pub healthy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// The version of the program that answers this question.
    pub version: String,
    /// The version of the running hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_version: Option<String>,
    pub start_at_login: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    pub worker_connected: bool,
    pub session_host_connected: bool,
    /// The tasks that work now. An update asks first when this is not zero.
    pub working_tasks: u32,
    /// The Consort folder holds the data of a hub.
    pub has_data: bool,
}

/// Why this computer cannot run a local hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckProblem {
    /// This system has no boundary. Windows is in this group.
    NoBoundary,
    /// Linux with no bubblewrap.
    NeedsBubblewrap {
        /// The command that installs it, when the system tells.
        command: String,
    },
    /// The boundary program exists, but it does not start.
    BoundaryFailed { message: String },
}

/// What `consort local check` prints on stdout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckReport {
    /// `None` when this computer can run agents.
    pub problem: Option<CheckProblem>,
}

/// A step of the install, as the first screen of the app shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStep {
    /// Checks that this computer can run agents.
    Checked,
    /// Starts the hub in the background.
    StartingHub,
    /// Adds this computer as a worker.
    AddingWorker,
    /// Connects this app. The app does this step itself.
    Connecting,
}

impl InstallStep {
    /// The steps in their order.
    pub const ALL: [InstallStep; 4] = [
        InstallStep::Checked,
        InstallStep::StartingHub,
        InstallStep::AddingWorker,
        InstallStep::Connecting,
    ];

    /// The name on the progress line.
    pub fn as_str(self) -> &'static str {
        match self {
            InstallStep::Checked => "checked",
            InstallStep::StartingHub => "starting_hub",
            InstallStep::AddingWorker => "adding_worker",
            InstallStep::Connecting => "connecting",
        }
    }

    /// The step of a progress line, or `None` for any other line.
    pub fn from_progress_line(line: &str) -> Option<InstallStep> {
        let name = line.trim().strip_prefix(PROGRESS_PREFIX)?;
        Self::ALL.into_iter().find(|step| step.as_str() == name)
    }
}

/// How `consort local upgrade` ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeOutcome {
    /// The new program runs.
    Replaced,
    /// The installed program has this version already.
    Unchanged,
    /// The new hub did not start. The previous program and the backup are back,
    /// and the hub runs.
    RolledBack,
    /// The new hub did not start, and the previous one did not start either.
    RollbackFailed,
}

/// What `consort local upgrade` prints on stdout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeReport {
    pub outcome: UpgradeOutcome,
    /// The version before.
    pub from: String,
    /// The version of the new program.
    pub to: String,
    /// The backup of this upgrade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    /// What went wrong and what to do next. Empty after a replacement.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_part_of_the_hub_is_inside_one_folder_of_its_own() {
        let layout = Layout::new("/c");
        let local = layout.local_dir();
        for path in [
            layout.state_file(),
            layout.hub_dir(),
            layout.session_host_dir(),
            layout.home_dir(),
            layout.worker_dir(),
            layout.backup_dir(),
            layout.log_file(),
            layout.service_file(),
        ] {
            assert!(path.starts_with(&local), "{}", path.display());
        }
        assert_eq!(layout.program(false), PathBuf::from("/c/bin/consort"));
        assert_eq!(layout.program(true), PathBuf::from("/c/bin/consort.exe"));
        assert_eq!(
            layout.previous_program(false),
            PathBuf::from("/c/bin/consort.previous")
        );
    }

    #[test]
    fn a_progress_line_names_a_step_and_other_lines_do_not() {
        for step in InstallStep::ALL {
            let line = format!("{PROGRESS_PREFIX}{}\n", step.as_str());
            assert_eq!(InstallStep::from_progress_line(&line), Some(step));
        }
        assert_eq!(InstallStep::from_progress_line("checked"), None);
        assert_eq!(
            InstallStep::from_progress_line("consort-progress: flying"),
            None
        );
    }

    #[test]
    fn the_state_needs_a_process_and_a_connected_worker_to_be_ready() {
        let mut state = LocalState {
            pid: Some(4),
            ..LocalState::default()
        };
        assert!(!state.ready());
        state.worker_connected = true;
        assert!(state.ready());
        state.pid = None;
        assert!(!state.ready(), "a clean stop clears the process");
    }

    #[test]
    fn an_older_state_file_reads_with_defaults() {
        let state: LocalState =
            serde_json::from_str(r#"{"address":"http://127.0.0.1:9","port":9,"version":"0.1.0"}"#)
                .unwrap();
        assert_eq!(state.port, 9);
        assert!(state.past_addresses.is_empty());
        assert_eq!(state.pid, None);
    }

    #[test]
    fn the_address_uses_loopback() {
        assert_eq!(address_of(7601), "http://127.0.0.1:7601");
    }
}
