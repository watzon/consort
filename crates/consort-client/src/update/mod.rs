//! App updates: look for a new release on GitHub, download the right file
//! for this system into a staging folder, check its SHA-256, and tell the
//! app that an update is ready. On a restart, the app replaces itself.
//!
//! The logic is pure where it can be. This file holds the version compare,
//! the choice of the release file, the checksum file parser and the state
//! machine. [`net`] holds the network code and the background service, and
//! [`apply`] holds the replacement of the installed app.
//!
//! The desktop app never talks to the network: it holds an [`Updates`]
//! handle and reads its [`UpdateView`].

use std::path::PathBuf;

use consort_proto::error::HubVersions;
use serde::{Deserialize, Serialize};

pub mod apply;
pub mod net;

pub use net::{UpdateConfig, Updates};

// The repository, the versions and the checksum file are in `consort-proto`,
// because the worker and `consort hub update` read them too.
pub use consort_proto::release::{
    CHECKSUMS_ASSET, RELEASE_MANIFEST_ASSET, RELEASES_REPO, ReleaseManifest, Version,
    parse_checksums,
};

/// How often the app looks for a new release.
pub const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// The address of the raw install script for a system, on the main branch of
/// the repository.
pub fn install_script_url(os: crate::Os) -> String {
    let file = match os {
        crate::Os::Windows => "worker.ps1",
        crate::Os::MacOs | crate::Os::Linux => "worker.sh",
    };
    format!("https://raw.githubusercontent.com/{RELEASES_REPO}/main/scripts/install/{file}")
}

/// The address of the raw script that installs a hub on a Linux server.
pub fn hub_install_script_url() -> String {
    format!("https://raw.githubusercontent.com/{RELEASES_REPO}/main/scripts/install/hub.sh")
}

/// The command that installs a hub on a server. It prints a setup code.
pub fn hub_install_command() -> String {
    format!("curl -fsSL {} | bash", hub_install_script_url())
}

/// The page that explains how to run a hub on a server.
pub fn server_guide_url() -> String {
    format!("https://github.com/{RELEASES_REPO}/blob/main/docs/deploy.md")
}

/// The command that downloads the worker, installs it and pairs it with a
/// hub, for the system `os`.
pub fn worker_install_command(
    os: crate::Os,
    hub_url: &str,
    code: &str,
    tls_sha256: Option<&consort_proto::model::Sha256Hash>,
) -> String {
    let url = install_script_url(os);
    match os {
        crate::Os::Windows => {
            let pin = tls_sha256
                .map(|pin| format!(" -TlsSha256 {pin}"))
                .unwrap_or_default();
            format!("& ([scriptblock]::Create((irm {url}))) -Hub {hub_url} -Code {code}{pin}")
        }
        crate::Os::MacOs | crate::Os::Linux => {
            let pin = tls_sha256
                .map(|pin| format!(" --tls-sha256 {pin}"))
                .unwrap_or_default();
            format!("curl -fsSL {url} | bash -s -- --hub {hub_url} --code {code}{pin}")
        }
    }
}

/// The operating systems that have a desktop build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    MacOs,
    Windows,
    Linux,
}

/// The processors that have a desktop build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Arch {
    Arm64,
    X86_64,
}

/// A system and a processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
}

impl Platform {
    /// The system that runs this program, or `None` when it has no desktop
    /// build.
    pub fn current() -> Option<Platform> {
        Platform::from_std(std::env::consts::OS, std::env::consts::ARCH)
    }

    /// From the names in `std::env::consts`.
    pub fn from_std(os: &str, arch: &str) -> Option<Platform> {
        let os = match os {
            "macos" => Os::MacOs,
            "windows" => Os::Windows,
            "linux" => Os::Linux,
            _ => return None,
        };
        let arch = match arch {
            "aarch64" => Arch::Arm64,
            "x86_64" => Arch::X86_64,
            _ => return None,
        };
        Some(Platform { os, arch })
    }

    /// The key in the names of the release files: `macos-arm64`,
    /// `macos-x86_64`, `linux-x86_64`, `linux-arm64` or `windows-x86_64`.
    pub fn key(self) -> String {
        let os = match self.os {
            Os::MacOs => "macos",
            Os::Windows => "windows",
            Os::Linux => "linux",
        };
        let arch = match self.arch {
            Arch::Arm64 => "arm64",
            Arch::X86_64 => "x86_64",
        };
        format!("{os}-{arch}")
    }

    /// The name of the desktop app file in a release for this platform, or
    /// `None` when the release workflow builds none.
    pub fn desktop_asset(self) -> Option<String> {
        match (self.os, self.arch) {
            (Os::MacOs, _) => Some(format!("Consort-{}.zip", self.key())),
            // GitHub compares file names without case, so these names must
            // differ from the `consort-<key>` files of the CLI by more than case.
            (Os::Windows, Arch::X86_64) => Some(format!("Consort-desktop-{}.zip", self.key())),
            (Os::Linux, Arch::X86_64) => Some(format!("Consort-desktop-{}.tar.gz", self.key())),
            _ => None,
        }
    }
}

/// One file of a release.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
    #[serde(default)]
    pub size: u64,
}

/// A release, as GitHub's `releases/latest` answer gives it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Release {
    #[serde(rename = "tag_name")]
    pub tag: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// A newer release with the files that this platform needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub version: Version,
    pub asset: Asset,
    pub checksums: Asset,
    /// The `release.json` of the release. An older release has none.
    pub manifest: Option<Asset>,
}

/// Chooses the update for `current`, or `None` when the app is up to date or
/// the release has no file for this platform.
pub fn choose_update(
    current: &Version,
    release: &Release,
    platform: Platform,
) -> Option<Candidate> {
    if release.draft {
        return None;
    }
    let version = Version::parse(&release.tag)?;
    // A stable build never moves to a pre-release.
    if (release.prerelease || version.pre.is_some()) && current.pre.is_none() {
        return None;
    }
    if version <= *current {
        return None;
    }
    let wanted = platform.desktop_asset()?;
    let asset = release.assets.iter().find(|a| a.name == wanted)?.clone();
    let checksums = release
        .assets
        .iter()
        .find(|a| a.name == CHECKSUMS_ASSET)?
        .clone();
    let manifest = release
        .assets
        .iter()
        .find(|a| a.name == RELEASE_MANIFEST_ASSET)
        .cloned();
    Some(Candidate {
        version,
        asset,
        checksums,
        manifest,
    })
}

/// Where the update process is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    /// No check ran yet.
    Idle,
    /// A check runs.
    Checking,
    /// The app is the newest version. `checked_at` is in Unix milliseconds.
    UpToDate { checked_at: i64 },
    /// A newer version exists, and the app waits for the user to download
    /// it (the switch "Get updates on their own" is off).
    Available { version: Version },
    /// The file downloads.
    Downloading { version: Version },
    /// The file is in the staging folder and its checksum is right.
    Ready { version: Version, archive: PathBuf },
    /// The last check or download failed. The app tries again at the next
    /// check.
    Failed { message: String },
}

/// What happens to the state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateEvent {
    CheckStarted,
    /// The check found no newer version.
    NoUpdate {
        checked_at: i64,
    },
    /// The check found a newer version. With `auto`, the download starts.
    Found {
        version: Version,
        auto: bool,
    },
    DownloadStarted {
        version: Version,
    },
    Downloaded {
        version: Version,
        archive: PathBuf,
    },
    Failed {
        message: String,
    },
}

impl UpdateState {
    /// The state after `event`. A state that is Ready stays Ready for a
    /// version until a newer one is Ready, so a failed check never takes the
    /// notice away.
    pub fn apply(self, event: UpdateEvent) -> UpdateState {
        use UpdateEvent as E;
        use UpdateState as S;
        match (self, event) {
            // A ready update stays while the next checks run, and after one
            // fails or finds the same version.
            (ready @ S::Ready { .. }, E::CheckStarted | E::NoUpdate { .. } | E::Failed { .. }) => {
                ready
            }
            (
                S::Ready {
                    version: have,
                    archive,
                },
                E::Found { version, auto },
            ) => {
                if version > have {
                    if auto {
                        S::Downloading { version }
                    } else {
                        S::Available { version }
                    }
                } else {
                    S::Ready {
                        version: have,
                        archive,
                    }
                }
            }
            (_, E::CheckStarted) => S::Checking,
            (_, E::NoUpdate { checked_at }) => S::UpToDate { checked_at },
            (_, E::Found { version, auto }) => {
                if auto {
                    S::Downloading { version }
                } else {
                    S::Available { version }
                }
            }
            (_, E::DownloadStarted { version }) => S::Downloading { version },
            (_, E::Downloaded { version, archive }) => S::Ready { version, archive },
            (_, E::Failed { message }) => S::Failed { message },
        }
    }

    /// The version that is ready to install.
    pub fn ready_version(&self) -> Option<&Version> {
        match self {
            UpdateState::Ready { version, .. } => Some(version),
            _ => None,
        }
    }
}

/// What the app shows about updates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateView {
    pub state: UpdateState,
    /// The switch "Get updates on their own".
    pub auto: bool,
    /// The version of this app.
    pub current: Version,
    /// This copy of the app can replace itself. A build that runs from a
    /// cargo `target` folder cannot.
    pub installable: bool,
    /// The protocol versions of the newest release, from its `release.json`.
    /// `None` for a release without that file: it holds nothing back.
    pub release_protocols: Option<Vec<u32>>,
    /// The versions of the connected hub. `None` until a hub answers.
    pub hub: Option<HubVersions>,
}

/// An update that is ready and waits for the hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateHold {
    pub version: Version,
    pub hub_version: String,
}

impl UpdateHold {
    /// The first line of the notice.
    pub fn title(&self) -> String {
        format!("Update {} waits for your hub", self.version)
    }

    /// The rest of the notice.
    pub fn body(&self) -> String {
        format!(
            "This version cannot talk to hub {}. Update the hub first, and this app restarts when you are ready.",
            self.hub_version
        )
    }
}

impl UpdateView {
    /// The update that is ready, when it cannot talk to the hub because the
    /// hub is too old. Installing it would cut this app off from the hub. A
    /// hub that is newer than the release holds nothing: the update does no
    /// harm there.
    pub fn hold(&self) -> Option<UpdateHold> {
        let version = self.state.ready_version()?;
        let release = self.release_protocols.as_ref()?;
        let hub = self.hub.as_ref()?;
        // A hub on this computer updates with the app, so they stay together.
        if hub.install == Some(consort_proto::release::InstallKind::Local) {
            return None;
        }
        if consort_proto::frame::negotiate_version(release, &hub.protocol_versions).is_some() {
            return None;
        }
        let too_old = consort_proto::frame::who_is_too_old(release, Some(&hub.protocol_versions));
        (too_old == consort_proto::frame::TooOld::Hub).then(|| UpdateHold {
            version: version.clone(),
            hub_version: hub.hub_version.clone(),
        })
    }

    /// The newest version that the updater knows of.
    pub fn latest(&self) -> Option<&Version> {
        match &self.state {
            UpdateState::Available { version }
            | UpdateState::Downloading { version }
            | UpdateState::Ready { version, .. } => Some(version),
            _ => None,
        }
    }

    /// The sentence for the sidebar notice, when an update is ready and no
    /// hold is on it.
    pub fn notice(&self) -> Option<&'static str> {
        if self.hold().is_some() {
            return None;
        }
        self.state
            .ready_version()
            .map(|_| "Update ready. Restart to install.")
    }

    /// The line for the About dialog.
    pub fn summary(&self) -> String {
        match &self.state {
            UpdateState::Idle => "Consort checks for updates in the background.".to_owned(),
            UpdateState::Checking => "Looking for a new version.".to_owned(),
            UpdateState::UpToDate { .. } => format!("Version {} is the newest.", self.current),
            UpdateState::Available { version } => {
                format!("Version {version} is available.")
            }
            UpdateState::Downloading { version } => {
                format!("Downloading version {version}.")
            }
            UpdateState::Ready { .. } if self.hold().is_some() => {
                self.hold().map(|hold| hold.title()).unwrap_or_default()
            }
            UpdateState::Ready { version, .. } => {
                format!("Version {version} is ready. Restart to install.")
            }
            UpdateState::Failed { message } => message.clone(),
        }
    }
}

/// The settings of the updater, kept in the client's config folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSettings {
    /// The switch "Get updates on their own". On by default.
    #[serde(default = "default_auto")]
    pub auto: bool,
}

fn default_auto() -> bool {
    true
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings { auto: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn a_desktop_file_name_differs_from_the_cli_file_names_without_case() {
        // The release workflow packs the CLI as a zip only for Windows.
        for (os, arch, extension) in [
            ("macos", "aarch64", "tar.gz"),
            ("macos", "x86_64", "tar.gz"),
            ("windows", "x86_64", "zip"),
            ("linux", "x86_64", "tar.gz"),
        ] {
            let platform = Platform::from_std(os, arch).unwrap();
            let desktop = platform.desktop_asset().unwrap().to_lowercase();
            assert_ne!(desktop, format!("consort-{}.{extension}", platform.key()));
        }
    }

    #[test]
    fn each_platform_has_one_desktop_file_name() {
        let p = |os, arch| Platform::from_std(os, arch).unwrap();
        assert_eq!(
            p("macos", "aarch64").desktop_asset().as_deref(),
            Some("Consort-macos-arm64.zip")
        );
        assert_eq!(
            p("macos", "x86_64").desktop_asset().as_deref(),
            Some("Consort-macos-x86_64.zip")
        );
        assert_eq!(
            p("windows", "x86_64").desktop_asset().as_deref(),
            Some("Consort-desktop-windows-x86_64.zip")
        );
        assert_eq!(
            p("linux", "x86_64").desktop_asset().as_deref(),
            Some("Consort-desktop-linux-x86_64.tar.gz")
        );
        assert_eq!(p("linux", "aarch64").desktop_asset(), None);
        assert_eq!(p("windows", "aarch64").desktop_asset(), None);
        assert_eq!(p("linux", "aarch64").key(), "linux-arm64");
        assert!(Platform::from_std("freebsd", "x86_64").is_none());
        assert!(Platform::from_std("linux", "riscv64").is_none());
    }

    fn release(tag: &str, names: &[&str]) -> Release {
        Release {
            tag: tag.to_owned(),
            draft: false,
            prerelease: false,
            assets: names
                .iter()
                .map(|n| Asset {
                    name: (*n).to_owned(),
                    url: format!("https://example.test/{n}"),
                    size: 10,
                })
                .collect(),
        }
    }

    const MAC: Platform = Platform {
        os: Os::MacOs,
        arch: Arch::Arm64,
    };

    #[test]
    fn a_newer_release_gives_the_file_for_this_platform() {
        let r = release(
            "v0.2.0",
            &[
                "Consort-macos-arm64.zip",
                "Consort-macos-x86_64.zip",
                "SHA256SUMS",
            ],
        );
        let c = choose_update(&v("0.1.0"), &r, MAC).unwrap();
        assert_eq!(c.version, v("0.2.0"));
        assert_eq!(c.asset.name, "Consort-macos-arm64.zip");
        assert_eq!(c.checksums.name, "SHA256SUMS");
    }

    #[test]
    fn no_update_for_the_same_or_an_older_version() {
        let r = release("v0.1.0", &["Consort-macos-arm64.zip", "SHA256SUMS"]);
        assert!(choose_update(&v("0.1.0"), &r, MAC).is_none());
        assert!(choose_update(&v("0.3.0"), &r, MAC).is_none());
    }

    #[test]
    fn no_update_without_the_file_or_the_checksums() {
        let no_file = release("v0.2.0", &["Consort-macos-x86_64.zip", "SHA256SUMS"]);
        assert!(choose_update(&v("0.1.0"), &no_file, MAC).is_none());
        let no_sums = release("v0.2.0", &["Consort-macos-arm64.zip"]);
        assert!(choose_update(&v("0.1.0"), &no_sums, MAC).is_none());
    }

    #[test]
    fn a_draft_or_a_pre_release_is_not_offered_to_a_stable_build() {
        let names = ["Consort-macos-arm64.zip", "SHA256SUMS"];
        let mut draft = release("v0.2.0", &names);
        draft.draft = true;
        assert!(choose_update(&v("0.1.0"), &draft, MAC).is_none());
        let mut pre = release("v0.2.0", &names);
        pre.prerelease = true;
        assert!(choose_update(&v("0.1.0"), &pre, MAC).is_none());
        let tagged = release("v0.2.0-rc.1", &names);
        assert!(choose_update(&v("0.1.0"), &tagged, MAC).is_none());
        // A pre-release build follows pre-releases.
        assert!(choose_update(&v("0.1.0-rc.1"), &tagged, MAC).is_some());
        let junk = release("nightly", &names);
        assert!(choose_update(&v("0.1.0"), &junk, MAC).is_none());
    }

    #[test]
    fn the_github_answer_parses() {
        let json = r#"{"tag_name":"v0.2.0","draft":false,"prerelease":false,"name":"x",
            "assets":[{"name":"SHA256SUMS","size":5,"browser_download_url":"https://x/y","id":1}]}"#;
        let release: Release = serde_json::from_str(json).unwrap();
        assert_eq!(release.tag, "v0.2.0");
        assert_eq!(release.assets[0].url, "https://x/y");
    }

    #[test]
    fn the_checksum_file_parses() {
        let hash_a = "a".repeat(64);
        let hash_b = "B".repeat(64);
        let text = format!(
            "{hash_a}  Consort-macos-arm64.zip\n{hash_b} *consort-linux-x86_64.tar.gz\nnot a line\n{} short.zip\n\n",
            "c".repeat(10)
        );
        let sums = parse_checksums(&text);
        assert_eq!(sums.len(), 2);
        assert_eq!(sums["Consort-macos-arm64.zip"], hash_a);
        assert_eq!(sums["consort-linux-x86_64.tar.gz"], "b".repeat(64));
    }

    #[test]
    fn the_state_goes_from_check_to_download_to_ready() {
        use UpdateEvent as E;
        let mut state = UpdateState::Idle;
        state = state.apply(E::CheckStarted);
        assert_eq!(state, UpdateState::Checking);
        state = state.apply(E::Found {
            version: v("0.2.0"),
            auto: true,
        });
        assert_eq!(
            state,
            UpdateState::Downloading {
                version: v("0.2.0")
            }
        );
        state = state.apply(E::Downloaded {
            version: v("0.2.0"),
            archive: PathBuf::from("/x/a.zip"),
        });
        assert_eq!(state.ready_version(), Some(&v("0.2.0")));
    }

    #[test]
    fn with_updates_off_the_state_waits_for_the_user() {
        use UpdateEvent as E;
        let state = UpdateState::Checking.apply(E::Found {
            version: v("0.2.0"),
            auto: false,
        });
        assert_eq!(
            state,
            UpdateState::Available {
                version: v("0.2.0")
            }
        );
        let state = state.apply(E::DownloadStarted {
            version: v("0.2.0"),
        });
        assert!(matches!(state, UpdateState::Downloading { .. }));
    }

    #[test]
    fn a_ready_update_survives_later_checks_and_failures() {
        use UpdateEvent as E;
        let ready = UpdateState::Ready {
            version: v("0.2.0"),
            archive: PathBuf::from("/x/a.zip"),
        };
        for event in [
            E::CheckStarted,
            E::NoUpdate { checked_at: 5 },
            E::Failed {
                message: "no network".into(),
            },
            E::Found {
                version: v("0.2.0"),
                auto: true,
            },
        ] {
            assert_eq!(ready.clone().apply(event), ready);
        }
        // A newer version replaces it.
        let newer = ready.apply(E::Found {
            version: v("0.3.0"),
            auto: true,
        });
        assert_eq!(
            newer,
            UpdateState::Downloading {
                version: v("0.3.0")
            }
        );
    }

    #[test]
    fn a_failure_shows_until_the_next_check() {
        use UpdateEvent as E;
        let state = UpdateState::Downloading {
            version: v("0.2.0"),
        }
        .apply(E::Failed {
            message: "Consort could not download the update.".into(),
        });
        assert!(matches!(state, UpdateState::Failed { .. }));
        assert_eq!(state.apply(E::CheckStarted), UpdateState::Checking);
    }

    #[test]
    fn the_notice_shows_only_for_a_ready_update() {
        let view = |state| UpdateView {
            state,
            auto: true,
            current: v("0.1.0"),
            installable: true,
            release_protocols: None,
            hub: None,
        };
        assert_eq!(view(UpdateState::Idle).notice(), None);
        assert_eq!(
            view(UpdateState::Available {
                version: v("0.2.0")
            })
            .notice(),
            None
        );
        let ready = view(UpdateState::Ready {
            version: v("0.2.0"),
            archive: PathBuf::new(),
        });
        assert_eq!(ready.notice(), Some("Update ready. Restart to install."));
        assert_eq!(
            ready.summary(),
            "Version 0.2.0 is ready. Restart to install."
        );
        assert_eq!(
            view(UpdateState::UpToDate { checked_at: 0 }).summary(),
            "Version 0.1.0 is the newest."
        );
    }

    #[test]
    fn the_settings_default_to_on_and_read_an_empty_file() {
        assert!(UpdateSettings::default().auto);
        let s: UpdateSettings = serde_json::from_str("{}").unwrap();
        assert!(s.auto);
        let s: UpdateSettings = serde_json::from_str(r#"{"auto":false}"#).unwrap();
        assert!(!s.auto);
    }

    #[test]
    fn the_hub_command_is_the_one_in_the_design() {
        assert_eq!(
            hub_install_command(),
            "curl -fsSL https://raw.githubusercontent.com/watzon/consort/main/scripts/install/hub.sh | bash"
        );
        assert!(server_guide_url().ends_with("/docs/deploy.md"));
    }

    #[test]
    fn the_install_commands_carry_the_pin_of_a_hub_on_this_network() {
        let pin = consort_proto::model::Sha256Hash::from_bytes([7; 32]);
        let hex = "07".repeat(32);
        let mac = worker_install_command(crate::Os::MacOs, "https://m.local:7601", "A", Some(&pin));
        assert!(mac.ends_with(&format!(
            "--hub https://m.local:7601 --code A --tls-sha256 {hex}"
        )));
        let win =
            worker_install_command(crate::Os::Windows, "https://m.local:7601", "A", Some(&pin));
        assert!(win.ends_with(&format!(
            "-Hub https://m.local:7601 -Code A -TlsSha256 {hex}"
        )));
    }

    #[test]
    fn the_install_commands_name_the_script_and_the_code() {
        let mac = worker_install_command(
            crate::Os::MacOs,
            "https://hub.example.com",
            "KQ7M-4XPD",
            None,
        );
        assert_eq!(
            mac,
            "curl -fsSL https://raw.githubusercontent.com/watzon/consort/main/scripts/install/worker.sh | bash -s -- --hub https://hub.example.com --code KQ7M-4XPD"
        );
        assert_eq!(
            worker_install_command(crate::Os::Linux, "https://h", "A", None),
            "curl -fsSL https://raw.githubusercontent.com/watzon/consort/main/scripts/install/worker.sh | bash -s -- --hub https://h --code A"
        );
        let win = worker_install_command(
            crate::Os::Windows,
            "https://hub.example.com",
            "KQ7M-4XPD",
            None,
        );
        assert_eq!(
            win,
            "& ([scriptblock]::Create((irm https://raw.githubusercontent.com/watzon/consort/main/scripts/install/worker.ps1))) -Hub https://hub.example.com -Code KQ7M-4XPD"
        );
    }

    #[test]
    fn a_hub_on_this_computer_never_holds_an_update() {
        let mut view = held_view(Some(&[2]), Some(&[1]));
        assert!(view.hold().is_some());
        if let Some(hub) = view.hub.as_mut() {
            hub.install = Some(consort_proto::release::InstallKind::Local);
        }
        assert_eq!(view.hold(), None);
    }

    fn held_view(release: Option<&[u32]>, hub: Option<&[u32]>) -> UpdateView {
        UpdateView {
            state: UpdateState::Ready {
                version: v("0.8.0"),
                archive: PathBuf::new(),
            },
            auto: true,
            current: v("0.7.0"),
            installable: true,
            release_protocols: release.map(<[u32]>::to_vec),
            hub: hub.map(|protocols| HubVersions {
                hub_version: "0.6.2".into(),
                protocol_versions: protocols.to_vec(),
                install: None,
            }),
        }
    }

    #[test]
    fn an_update_that_cannot_talk_to_the_hub_is_held() {
        let held = held_view(Some(&[2, 3]), Some(&[1]));
        let hold = held.hold().unwrap();
        assert_eq!(hold.title(), "Update 0.8.0 waits for your hub");
        assert!(hold.body().contains("hub 0.6.2"));
        assert_eq!(held.notice(), None);
        assert_eq!(held.summary(), "Update 0.8.0 waits for your hub");
    }

    #[test]
    fn an_update_is_not_held_when_it_fits_or_nothing_is_known() {
        for view in [
            held_view(Some(&[1, 2]), Some(&[1])),
            // A release from before `release.json`.
            held_view(None, Some(&[1])),
            // No hub answered in this run.
            held_view(Some(&[2]), None),
            // A hub that is newer than the release: the update does no harm.
            held_view(Some(&[1]), Some(&[3])),
        ] {
            assert_eq!(view.hold(), None, "{view:?}");
            assert!(view.notice().is_some(), "{view:?}");
        }
    }

    #[test]
    fn the_release_manifest_goes_with_the_candidate_when_the_release_has_one() {
        let platform = Platform::from_std("macos", "aarch64").unwrap();
        let with = release(
            "v0.8.0",
            &["Consort-macos-arm64.zip", "SHA256SUMS", "release.json"],
        );
        let candidate = choose_update(&v("0.7.0"), &with, platform).unwrap();
        assert_eq!(candidate.manifest.unwrap().name, "release.json");
        let without = release("v0.8.0", &["Consort-macos-arm64.zip", "SHA256SUMS"]);
        let candidate = choose_update(&v("0.7.0"), &without, platform).unwrap();
        assert_eq!(candidate.manifest, None);
    }
}
