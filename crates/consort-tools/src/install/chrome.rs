//! The preview browser: `chrome-headless-shell` from Chrome for Testing.
//!
//! The session host renders visuals with it. No vendor manifest gives a hash
//! for these files. So Consort pins one version, and the size and the SHA-256
//! of the file of each platform, in [`PINNED_FILES`]. An update check never
//! changes the version. A new Consort release does.

use super::archive::{self, ArchiveKind};
use super::http::check_sha256;
use super::sources::extract_blocking;
use super::{Scratch, ToolStore};
use crate::{Platform, ToolError, ToolId};

/// The pinned version of Chrome for Testing.
///
/// Never pin a version below 156.0.8078.70. An older shell trusts the
/// eMudhra intermediate certificates that were compromised. A test checks
/// this rule with `MIN_VERSION`.
const PINNED_VERSION: &str = "156.0.8078.80";

/// The oldest version that Consort may pin.
#[cfg(test)]
const MIN_VERSION: &str = "156.0.8078.70";

/// The platform, the size in bytes and the SHA-256 of
/// `chrome-headless-shell-<platform>.zip` at [`PINNED_VERSION`].
///
/// To update this table:
///
/// 1. Open
///    <https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json>
///    and pick a version that has `chrome-headless-shell` downloads for every
///    platform below. It must not be older than 156.0.8078.70.
/// 2. Download each `chrome-headless-shell-<platform>.zip` from
///    `https://storage.googleapis.com/chrome-for-testing-public/<version>/<platform>/`.
/// 3. Write the size (`stat`) and the SHA-256 (`shasum -a 256`) of each file
///    here. The JSON file has no hashes, so do not skip the download.
/// 4. Set [`PINNED_VERSION`]. Run the tests of this crate. Run the ignored
///    smoke test `real_chrome_headless_shell` once.
///
/// `win32` has no Consort platform. It stays so that the table has every
/// platform of the vendor.
const PINNED_FILES: [(&str, u64, &str); 6] = [
    (
        "linux64",
        126_922_702,
        "547dcd374e02961bf998f6046a267644f8502225c36bb78ca307dab32a0b3eb4",
    ),
    (
        "linux-arm64",
        126_679_292,
        "5bb6a00a4285c13303d3f72c5afdb70f144ea7a93eed462365a52b69631c9607",
    ),
    (
        "mac-arm64",
        103_982_159,
        "ea615baa2e9caf0d02d1898a6562b5f8fa9c35239707dae0f592178df5b986ac",
    ),
    (
        "mac-x64",
        109_493_298,
        "535d417d9034cbd11e7dacd1a71aa3d6e1b525e6d5dcd2671eb2532f01481228",
    ),
    (
        "win32",
        119_036_038,
        "8d7472c5c60215a24c42e31d0e9567f1fbf41365e46c8d36c59b2a8b8b2b3a65",
    ),
    (
        "win64",
        125_772_546,
        "10a77466005e7f64e00f1a75fd1ea81f4a38a60093c65fea6895843b26e60a9a",
    ),
];

/// One pinned build of the preview browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromePin {
    /// The version of Chrome for Testing. It is the only version that the
    /// store installs.
    pub version: String,
    /// The download of each platform.
    pub files: Vec<ChromeFile>,
}

/// The pinned download of the preview browser for one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeFile {
    /// The Chrome for Testing platform name, such as `linux64`. See
    /// [`Platform::chrome_platform`].
    pub platform: String,
    /// The size of the zip file in bytes.
    pub bytes: u64,
    /// The SHA-256 of the zip file, in hex.
    pub sha256: String,
}

impl ChromePin {
    /// The pin that this release of Consort ships.
    pub fn pinned() -> ChromePin {
        ChromePin {
            version: PINNED_VERSION.to_owned(),
            files: PINNED_FILES
                .iter()
                .map(|(platform, bytes, sha256)| ChromeFile {
                    platform: (*platform).to_owned(),
                    bytes: *bytes,
                    sha256: (*sha256).to_owned(),
                })
                .collect(),
        }
    }
}

/// The program in the install folder: `chrome-headless-shell`, with `.exe`
/// on Windows.
pub(super) fn program_name(platform: &Platform) -> String {
    format!("chrome-headless-shell{}", platform.exe_suffix())
}

impl ToolStore {
    /// The pinned version. This makes no request.
    pub(super) fn chrome_latest(&self) -> String {
        self.options.chrome.version.clone()
    }

    /// Downloads the pinned zip, checks its size and SHA-256 while it
    /// streams, and unpacks it without its top folder.
    pub(super) async fn stage_chrome_headless_shell(
        &self,
        version: &str,
        scratch: &Scratch,
    ) -> Result<(), ToolError> {
        let tool = ToolId::ChromeHeadlessShell;
        let platform = &self.options.platform;
        let pin = &self.options.chrome;
        // The hashes are for one version. Another version has no pin.
        if version != pin.version {
            return Err(ToolError::Unsupported {
                tool,
                reason: format!("Consort pins version {}", pin.version),
            });
        }
        let no_build = || ToolError::UnsupportedPlatform {
            tool: ToolId::ChromeHeadlessShell,
            platform: platform.to_string(),
        };
        let chrome_platform = platform.chrome_platform().ok_or_else(no_build)?;
        let file = pin
            .files
            .iter()
            .find(|file| file.platform == chrome_platform)
            .ok_or_else(no_build)?;

        let name = format!("chrome-headless-shell-{chrome_platform}.zip");
        let url = format!(
            "{}/{version}/{chrome_platform}/{name}",
            self.options.sources.chrome_for_testing
        );
        let got = self
            .http
            .download_sized(&url, &scratch.download, &name, file.bytes)
            .await?;
        check_sha256(&name, &file.sha256, &got.sha256)?;
        extract_blocking(&scratch.download, ArchiveKind::Zip, &scratch.staging, 1).await?;
        let program = scratch.staging.join(program_name(platform));
        if !program.is_file() {
            return Err(ToolError::Archive {
                message: format!("{name} has no {}", program_name(platform)),
            });
        }
        // A zip made on Windows has no mode bits.
        archive::set_mode(&program, 0o755)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::install::state::is_newer;

    /// The names of the six builds that the vendor publishes.
    const VENDOR_PLATFORMS: [&str; 6] = [
        "linux-arm64",
        "linux64",
        "mac-arm64",
        "mac-x64",
        "win32",
        "win64",
    ];

    #[test]
    fn the_pinned_table_has_every_platform_once() {
        let pin = ChromePin::pinned();
        let names: BTreeSet<&str> = pin.files.iter().map(|f| f.platform.as_str()).collect();
        assert_eq!(pin.files.len(), VENDOR_PLATFORMS.len());
        assert_eq!(names, BTreeSet::from(VENDOR_PLATFORMS));
        for file in &pin.files {
            assert!(file.bytes > 50_000_000, "{}: {}", file.platform, file.bytes);
            assert_eq!(file.sha256.len(), 64, "{}", file.platform);
            assert!(
                file.sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{} is not lower-case hex",
                file.platform
            );
        }
        let hashes: BTreeSet<&str> = pin.files.iter().map(|f| f.sha256.as_str()).collect();
        assert_eq!(hashes.len(), pin.files.len(), "two platforms share a hash");
    }

    #[test]
    fn every_platform_with_a_build_has_a_pinned_file() {
        let pin = ChromePin::pinned();
        for platform in Platform::ALL {
            let Some(name) = platform.chrome_platform() else {
                continue;
            };
            assert!(
                pin.files.iter().any(|file| file.platform == name),
                "{platform} has no pinned file"
            );
        }
        // Only musl Linux has no build.
        let without: Vec<String> = Platform::ALL
            .iter()
            .filter(|platform| platform.chrome_platform().is_none())
            .map(ToString::to_string)
            .collect();
        assert_eq!(without, ["linux-x64-musl", "linux-arm64-musl"]);
    }

    #[test]
    fn the_pinned_version_is_not_older_than_the_floor() {
        let pin = ChromePin::pinned();
        assert!(
            !is_newer(MIN_VERSION, &pin.version),
            "{} is older than {MIN_VERSION}",
            pin.version
        );
        assert!(crate::install::state::valid_version(&pin.version));
        assert_eq!(pin.version, PINNED_VERSION);
    }

    #[test]
    fn the_program_name_follows_the_platform() {
        use crate::{Arch, Libc, Os};
        let windows = Platform::new(Os::Windows, Arch::X64, Libc::System);
        let linux = Platform::new(Os::Linux, Arch::X64, Libc::System);
        assert_eq!(program_name(&windows), "chrome-headless-shell.exe");
        assert_eq!(program_name(&linux), "chrome-headless-shell");
    }
}
