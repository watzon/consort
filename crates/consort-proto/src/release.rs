//! What a release of Consort is: the repository that holds it, its version,
//! the names of its files, and the small file that says which protocol
//! versions it speaks. This is pure data. The downloads are in
//! `consort_link::update` and `consort_client::update`.

use std::cmp::Ordering;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The GitHub repository that holds the releases, as `owner/name`. This is
/// the one place in Rust code that names it. A peer never sends a repository
/// or an address, so a hub cannot point a worker at other files. Change it
/// here when the repository moves, and in the two install scripts.
pub const RELEASES_REPO: &str = "watzon/consort";

/// The file with the SHA-256 of every release file. One line for each file:
/// the hash, two spaces, the name.
pub const CHECKSUMS_ASSET: &str = "SHA256SUMS";

/// The file with the version and the protocol versions of a release. See
/// [`ReleaseManifest`].
pub const RELEASE_MANIFEST_ASSET: &str = "release.json";

/// The body of `release.json`. Each release has it from the version that
/// added this file. An older release has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    /// The version of the release, such as `0.8.0`.
    pub version: String,
    /// The protocol versions that this release speaks, oldest first.
    pub protocol_versions: Vec<u32>,
}

impl ReleaseManifest {
    /// The manifest of this build.
    pub fn of_this_build(version: &str) -> Self {
        Self {
            version: version.to_owned(),
            protocol_versions: crate::frame::SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
        }
    }
}

/// How a hub was installed. The app uses it to show the right update command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    /// On the same computer as the app. The app updates it.
    Local,
    /// A systemd service on a server.
    Service,
    /// A Docker container.
    Docker,
    /// Installed by hand. A value from a newer hub also reads as this.
    #[default]
    #[serde(other)]
    Manual,
}

impl InstallKind {
    /// The variable that says how the hub was installed. The systemd unit and
    /// the `Dockerfile` set it. The local install sets it for its service.
    pub const ENV_VAR: &'static str = "CONSORT_INSTALL_KIND";

    /// The kind for a value of [`Self::ENV_VAR`]. No value, or a value that
    /// this build does not know, means [`Self::Manual`].
    pub fn from_env_value(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("local") => Self::Local,
            Some("service") => Self::Service,
            Some("docker") => Self::Docker,
            _ => Self::Manual,
        }
    }
}

/// A release version: `major.minor.patch`, with an optional pre-release
/// part. A leading `v` is allowed. A worker builds a download address from a
/// version that the hub names, so the pre-release part can hold only ASCII
/// letters, digits, `.` and `-`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Version {
    /// The first number.
    pub major: u64,
    /// The second number.
    pub minor: u64,
    /// The third number.
    pub patch: u64,
    /// The part after the `-`, such as `rc.1`.
    pub pre: Option<String>,
}

impl Version {
    /// Reads a version, or gives `None` when the text is not one.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split('+').next().unwrap_or(text);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre))
                if !pre.is_empty()
                    && pre
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') =>
            {
                (core, Some(pre.to_owned()))
            }
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

    /// The name of the release tag: `v0.8.0`.
    pub fn tag(&self) -> String {
        format!("v{self}")
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

/// The version that a machine with version `current` must follow, when the
/// hub runs `hub`. `None` when either text is not a version, or when the hub
/// is not newer: a machine never moves to an older version.
pub fn version_to_follow(hub: &str, current: &str) -> Option<Version> {
    let hub = Version::parse(hub)?;
    let current = Version::parse(current)?;
    (hub > current).then_some(hub)
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

/// The name of the file with the `consort` program for a system, from the
/// names in `std::env::consts`. `None` when the release has no such file.
pub fn program_asset(os: &str, arch: &str) -> Option<String> {
    let key = match (os, arch) {
        ("macos", "aarch64") => "macos-arm64",
        ("macos", "x86_64") => "macos-x86_64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "x86_64") => "linux-x86_64",
        ("windows", "x86_64") => "windows-x86_64",
        _ => return None,
    };
    let extension = if os == "windows" { "zip" } else { "tar.gz" };
    Some(format!("consort-{key}.{extension}"))
}

/// The address of a file in the release of `version`, in [`RELEASES_REPO`].
pub fn release_file_url(version: &Version, asset: &str) -> String {
    format!(
        "https://github.com/{RELEASES_REPO}/releases/download/{}/{asset}",
        version.tag()
    )
}

/// The address that says which release is the latest: the GitHub API.
pub fn latest_release_url() -> String {
    format!("https://api.github.com/repos/{RELEASES_REPO}/releases/latest")
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
        assert_eq!(v("0.8.0").tag(), "v0.8.0");
        for bad in ["", "1", "1.2", "1.2.3.4", "a.b.c", "1.2.3-", "v"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    /// A worker builds a download address from a version that the hub names.
    #[test]
    fn a_version_cannot_hold_a_path_or_an_address() {
        for bad in [
            "1.2.3-../../evil",
            "1.2.3-a/b",
            "1.2.3-a?b=c",
            "1.2.3-a b",
            "1.2.3-a%2f",
            "1.2.3-a#b",
        ] {
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
    fn a_machine_follows_only_a_newer_hub() {
        assert_eq!(version_to_follow("0.8.0", "0.7.1"), Some(v("0.8.0")));
        assert_eq!(version_to_follow("0.8.0", "0.8.0"), None);
        assert_eq!(version_to_follow("0.7.0", "0.8.0"), None, "no downgrade");
        assert_eq!(version_to_follow("0.8.0", "test"), None);
        assert_eq!(version_to_follow("dev", "0.8.0"), None);
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

    /// `scripts/install/worker.sh` and `worker.ps1` use the same names.
    #[test]
    fn each_system_has_one_program_file_name() {
        for (os, arch, name) in [
            ("macos", "aarch64", "consort-macos-arm64.tar.gz"),
            ("macos", "x86_64", "consort-macos-x86_64.tar.gz"),
            ("linux", "aarch64", "consort-linux-arm64.tar.gz"),
            ("linux", "x86_64", "consort-linux-x86_64.tar.gz"),
            ("windows", "x86_64", "consort-windows-x86_64.zip"),
        ] {
            assert_eq!(program_asset(os, arch).as_deref(), Some(name));
        }
        assert_eq!(program_asset("windows", "aarch64"), None);
        assert_eq!(program_asset("freebsd", "x86_64"), None);
    }

    #[test]
    fn a_release_file_comes_from_the_fixed_repository() {
        assert_eq!(
            release_file_url(&v("0.8.0"), "SHA256SUMS"),
            "https://github.com/watzon/consort/releases/download/v0.8.0/SHA256SUMS"
        );
    }

    #[test]
    fn the_install_kind_comes_from_the_variable() {
        assert_eq!(
            InstallKind::from_env_value(Some("service")),
            InstallKind::Service
        );
        assert_eq!(
            InstallKind::from_env_value(Some("docker")),
            InstallKind::Docker
        );
        assert_eq!(
            InstallKind::from_env_value(Some("local")),
            InstallKind::Local
        );
        assert_eq!(InstallKind::from_env_value(None), InstallKind::Manual);
        assert_eq!(
            InstallKind::from_env_value(Some("mystery")),
            InstallKind::Manual
        );
    }

    #[test]
    fn an_install_kind_from_a_newer_hub_reads_as_manual() {
        let kind: InstallKind = serde_json::from_str("\"nix\"").unwrap();
        assert_eq!(kind, InstallKind::Manual);
        assert_eq!(
            serde_json::to_value(InstallKind::Docker).unwrap(),
            serde_json::json!("docker")
        );
    }

    #[test]
    fn the_manifest_has_the_protocols_of_this_build() {
        let manifest = ReleaseManifest::of_this_build("0.8.0");
        assert_eq!(
            serde_json::to_value(&manifest).unwrap(),
            serde_json::json!({
                "version": "0.8.0",
                "protocol_versions": crate::frame::SUPPORTED_PROTOCOL_VERSIONS,
            })
        );
    }
}
