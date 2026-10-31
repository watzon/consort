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

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod apply;
pub mod net;

pub use net::{UpdateConfig, Updates};

/// The GitHub repository that holds the releases, as `owner/name`. This is
/// the one place in Rust code that names it. The install scripts and the
/// install commands in the app use it too. Change it here when the
/// repository moves, and in the two install scripts.
pub const RELEASES_REPO: &str = "watzon/consort";

/// The file with the SHA-256 of every release file. One line for each file:
/// the hash, two spaces, the name.
pub const CHECKSUMS_ASSET: &str = "SHA256SUMS";

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

/// The command that downloads the worker, installs it and pairs it with a
/// hub, for the system `os`.
pub fn worker_install_command(os: crate::Os, hub_url: &str, code: &str) -> String {
    let url = install_script_url(os);
    match os {
        crate::Os::Windows => {
            format!("& ([scriptblock]::Create((irm {url}))) -Hub {hub_url} -Code {code}")
        }
        crate::Os::MacOs | crate::Os::Linux => {
            format!("curl -fsSL {url} | bash -s -- --hub {hub_url} --code {code}")
        }
    }
}

/// A release version: `major.minor.patch`, with an optional pre-release
/// part. A leading `v` is allowed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split('+').next().unwrap_or(text);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_owned())),
            Some(_) => return None,
            None => (text, None),
        };
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                // A release is newer than its pre-releases.
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => compare_pre(a, b),
            })
    }
}

/// Compares two pre-release parts, such as `rc.2` and `rc.10`, part by
/// part. A number is lower than a word.
fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
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
    Some(Candidate {
        version,
        asset,
        checksums,
    })
}

/// Reads a checksum file: the lowercase hash for each file name. A line
/// with a `*` before the name (binary mode) works too. Other lines are
/// skipped.
pub fn parse_checksums(text: &str) -> HashMap<String, String> {
    let mut sums = HashMap::new();
    for line in text.lines() {
        let Some((hash, name)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let name = name.trim().trim_start_matches('*');
        if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && !name.is_empty() {
            sums.insert(name.to_owned(), hash.to_ascii_lowercase());
        }
    }
    sums
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
}

impl UpdateView {
    /// The sentence for the sidebar notice, when an update is ready.
    pub fn notice(&self) -> Option<&'static str> {
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
    fn versions_parse_and_print() {
        assert_eq!(v("0.7.1").to_string(), "0.7.1");
        assert_eq!(v("v1.2.3").to_string(), "1.2.3");
        assert_eq!(v("1.2.3-rc.1+build5").to_string(), "1.2.3-rc.1");
        for bad in ["", "1", "1.2", "1.2.3.4", "a.b.c", "1.2.3-", "v"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.2.1") > v("0.2.0"));
        assert_eq!(v("1.0.0"), v("v1.0.0"));
    }

    #[test]
    fn a_release_is_newer_than_its_pre_releases() {
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.10") > v("1.0.0-rc.2"));
        assert!(v("1.0.0-rc.1") > v("1.0.0-beta.9"));
        assert!(v("1.0.0-1") < v("1.0.0-alpha"));
        assert!(v("1.0.0-rc") < v("1.0.0-rc.1"));
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
    fn the_install_commands_name_the_script_and_the_code() {
        let mac = worker_install_command(crate::Os::MacOs, "https://hub.example.com", "KQ7M-4XPD");
        assert_eq!(
            mac,
            "curl -fsSL https://raw.githubusercontent.com/watzon/consort/main/scripts/install/worker.sh | bash -s -- --hub https://hub.example.com --code KQ7M-4XPD"
        );
        assert_eq!(
            worker_install_command(crate::Os::Linux, "https://h", "A"),
            "curl -fsSL https://raw.githubusercontent.com/watzon/consort/main/scripts/install/worker.sh | bash -s -- --hub https://h --code A"
        );
        let win =
            worker_install_command(crate::Os::Windows, "https://hub.example.com", "KQ7M-4XPD");
        assert_eq!(
            win,
            "& ([scriptblock]::Create((irm https://raw.githubusercontent.com/watzon/consort/main/scripts/install/worker.ps1))) -Hub https://hub.example.com -Code KQ7M-4XPD"
        );
    }
}
