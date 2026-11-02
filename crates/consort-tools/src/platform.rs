//! The operating system, CPU and C library of a worker, and the asset names
//! that each tool source uses for them.

use std::fmt;

/// An operating system that Consort can install tools on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    Macos,
    Linux,
    Windows,
}

/// A CPU architecture that Consort can install tools on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    X64,
    Arm64,
}

/// The C library. Only Linux has a choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Libc {
    /// The normal C library of the system (glibc on Linux).
    System,
    /// musl, for example on Alpine Linux.
    Musl,
}

/// The platform of a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
    pub libc: Libc,
}

impl Platform {
    /// Every platform that the tool sources publish builds for.
    pub const ALL: [Platform; 8] = [
        Platform::new(Os::Macos, Arch::Arm64, Libc::System),
        Platform::new(Os::Macos, Arch::X64, Libc::System),
        Platform::new(Os::Linux, Arch::X64, Libc::System),
        Platform::new(Os::Linux, Arch::Arm64, Libc::System),
        Platform::new(Os::Linux, Arch::X64, Libc::Musl),
        Platform::new(Os::Linux, Arch::Arm64, Libc::Musl),
        Platform::new(Os::Windows, Arch::X64, Libc::System),
        Platform::new(Os::Windows, Arch::Arm64, Libc::System),
    ];

    pub const fn new(os: Os, arch: Arch, libc: Libc) -> Self {
        Platform { os, arch, libc }
    }

    /// The platform of this process, or `None` on an OS or CPU that no tool
    /// source supports.
    ///
    /// On Linux, musl is detected from `/lib/libc.musl-*` or from the output
    /// of `ldd --version`.
    pub fn current() -> Option<Platform> {
        let os = match std::env::consts::OS {
            "macos" => Os::Macos,
            "linux" => Os::Linux,
            "windows" => Os::Windows,
            _ => return None,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => Arch::X64,
            "aarch64" => Arch::Arm64,
            _ => return None,
        };
        let libc = if os == Os::Linux && detect_musl() {
            Libc::Musl
        } else {
            Libc::System
        };
        Some(Platform { os, arch, libc })
    }

    /// True on Windows.
    pub fn is_windows(&self) -> bool {
        self.os == Os::Windows
    }

    /// `.exe` on Windows, else empty.
    pub fn exe_suffix(&self) -> &'static str {
        if self.is_windows() { ".exe" } else { "" }
    }

    /// The platform folder of the Claude Code release CDN, such as
    /// `darwin-arm64` or `linux-x64-musl`.
    pub fn claude_platform(&self) -> &'static str {
        match (self.os, self.arch, self.libc) {
            (Os::Macos, Arch::Arm64, _) => "darwin-arm64",
            (Os::Macos, Arch::X64, _) => "darwin-x64",
            (Os::Linux, Arch::X64, Libc::System) => "linux-x64",
            (Os::Linux, Arch::Arm64, Libc::System) => "linux-arm64",
            (Os::Linux, Arch::X64, Libc::Musl) => "linux-x64-musl",
            (Os::Linux, Arch::Arm64, Libc::Musl) => "linux-arm64-musl",
            (Os::Windows, Arch::X64, _) => "win32-x64",
            (Os::Windows, Arch::Arm64, _) => "win32-arm64",
        }
    }

    /// The platform part of a Node archive name, such as `darwin-arm64`,
    /// `linux-x64-musl` or `win-x64`.
    pub fn node_platform(&self) -> &'static str {
        match (self.os, self.arch, self.libc) {
            (Os::Macos, Arch::Arm64, _) => "darwin-arm64",
            (Os::Macos, Arch::X64, _) => "darwin-x64",
            (Os::Linux, Arch::X64, Libc::System) => "linux-x64",
            (Os::Linux, Arch::Arm64, Libc::System) => "linux-arm64",
            (Os::Linux, Arch::X64, Libc::Musl) => "linux-x64-musl",
            (Os::Linux, Arch::Arm64, Libc::Musl) => "linux-arm64-musl",
            (Os::Windows, Arch::X64, _) => "win-x64",
            (Os::Windows, Arch::Arm64, _) => "win-arm64",
        }
    }

    /// The file extension of the Node archive: `zip` on Windows, else
    /// `tar.gz`.
    pub fn node_archive_ext(&self) -> &'static str {
        if self.is_windows() { "zip" } else { "tar.gz" }
    }

    /// The full Node archive name for a version without the leading `v`.
    pub fn node_archive(&self, version: &str) -> String {
        format!(
            "node-v{version}-{}.{}",
            self.node_platform(),
            self.node_archive_ext()
        )
    }

    /// The Rust target triple of the Codex release assets. Linux always uses
    /// the static musl build, because it runs on glibc systems too.
    pub fn codex_triple(&self) -> &'static str {
        match (self.os, self.arch) {
            (Os::Macos, Arch::Arm64) => "aarch64-apple-darwin",
            (Os::Macos, Arch::X64) => "x86_64-apple-darwin",
            (Os::Linux, Arch::X64) => "x86_64-unknown-linux-musl",
            (Os::Linux, Arch::Arm64) => "aarch64-unknown-linux-musl",
            (Os::Windows, Arch::X64) => "x86_64-pc-windows-msvc",
            (Os::Windows, Arch::Arm64) => "aarch64-pc-windows-msvc",
        }
    }

    /// The Codex release asset name, `codex-package-<triple>.tar.gz`.
    pub fn codex_asset(&self) -> String {
        format!("codex-package-{}.tar.gz", self.codex_triple())
    }

    /// The platform name of the Chrome for Testing builds, such as
    /// `mac-arm64`, `linux64` or `win64`, or `None` when there is no build.
    ///
    /// The Linux builds link glibc, so musl Linux has none. Windows on Arm
    /// uses the x64 build, which Windows runs in emulation.
    pub fn chrome_platform(&self) -> Option<&'static str> {
        match (self.os, self.arch, self.libc) {
            (Os::Macos, Arch::Arm64, _) => Some("mac-arm64"),
            (Os::Macos, Arch::X64, _) => Some("mac-x64"),
            (Os::Linux, Arch::X64, Libc::System) => Some("linux64"),
            (Os::Linux, Arch::Arm64, Libc::System) => Some("linux-arm64"),
            (Os::Linux, _, Libc::Musl) => None,
            (Os::Windows, _, _) => Some("win64"),
        }
    }

    /// The platform key of an ACP registry `binary` distribution, such as
    /// `darwin-aarch64`. The registry has no musl keys, so musl Linux uses
    /// the same key as glibc Linux.
    pub fn registry_key(&self) -> &'static str {
        match (self.os, self.arch) {
            (Os::Macos, Arch::Arm64) => "darwin-aarch64",
            (Os::Macos, Arch::X64) => "darwin-x86_64",
            (Os::Linux, Arch::Arm64) => "linux-aarch64",
            (Os::Linux, Arch::X64) => "linux-x86_64",
            (Os::Windows, Arch::Arm64) => "windows-aarch64",
            (Os::Windows, Arch::X64) => "windows-x86_64",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let os = match self.os {
            Os::Macos => "macos",
            Os::Linux => "linux",
            Os::Windows => "windows",
        };
        let arch = match self.arch {
            Arch::X64 => "x64",
            Arch::Arm64 => "arm64",
        };
        write!(f, "{os}-{arch}")?;
        if self.libc == Libc::Musl {
            f.write_str("-musl")?;
        }
        Ok(())
    }
}

fn detect_musl() -> bool {
    if cfg!(target_env = "musl") {
        return true;
    }
    let musl_loader = std::fs::read_dir("/lib").ok().is_some_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("libc.musl-")
        })
    });
    if musl_loader {
        return true;
    }
    // `ldd --version` prints to stderr on musl and to stdout on glibc.
    std::process::Command::new("ldd")
        .arg("--version")
        .output()
        .ok()
        .is_some_and(|out| {
            let text = [out.stdout, out.stderr].concat();
            String::from_utf8_lossy(&text)
                .to_lowercase()
                .contains("musl")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_for_every_platform() {
        // (platform, claude, node archive, codex asset, registry key, exe)
        let expected = [
            (
                "macos-arm64",
                "darwin-arm64",
                "node-v24.1.0-darwin-arm64.tar.gz",
                "codex-package-aarch64-apple-darwin.tar.gz",
                "darwin-aarch64",
                "",
            ),
            (
                "macos-x64",
                "darwin-x64",
                "node-v24.1.0-darwin-x64.tar.gz",
                "codex-package-x86_64-apple-darwin.tar.gz",
                "darwin-x86_64",
                "",
            ),
            (
                "linux-x64",
                "linux-x64",
                "node-v24.1.0-linux-x64.tar.gz",
                "codex-package-x86_64-unknown-linux-musl.tar.gz",
                "linux-x86_64",
                "",
            ),
            (
                "linux-arm64",
                "linux-arm64",
                "node-v24.1.0-linux-arm64.tar.gz",
                "codex-package-aarch64-unknown-linux-musl.tar.gz",
                "linux-aarch64",
                "",
            ),
            (
                "linux-x64-musl",
                "linux-x64-musl",
                "node-v24.1.0-linux-x64-musl.tar.gz",
                "codex-package-x86_64-unknown-linux-musl.tar.gz",
                "linux-x86_64",
                "",
            ),
            (
                "linux-arm64-musl",
                "linux-arm64-musl",
                "node-v24.1.0-linux-arm64-musl.tar.gz",
                "codex-package-aarch64-unknown-linux-musl.tar.gz",
                "linux-aarch64",
                "",
            ),
            (
                "windows-x64",
                "win32-x64",
                "node-v24.1.0-win-x64.zip",
                "codex-package-x86_64-pc-windows-msvc.tar.gz",
                "windows-x86_64",
                ".exe",
            ),
            (
                "windows-arm64",
                "win32-arm64",
                "node-v24.1.0-win-arm64.zip",
                "codex-package-aarch64-pc-windows-msvc.tar.gz",
                "windows-aarch64",
                ".exe",
            ),
        ];
        assert_eq!(expected.len(), Platform::ALL.len());
        for (platform, row) in Platform::ALL.iter().zip(expected) {
            assert_eq!(platform.to_string(), row.0);
            assert_eq!(platform.claude_platform(), row.1, "{platform}");
            assert_eq!(platform.node_archive("24.1.0"), row.2, "{platform}");
            assert_eq!(platform.codex_asset(), row.3, "{platform}");
            assert_eq!(platform.registry_key(), row.4, "{platform}");
            assert_eq!(platform.exe_suffix(), row.5, "{platform}");
        }
    }

    #[test]
    fn chrome_platforms() {
        let expected = [
            ("macos-arm64", Some("mac-arm64")),
            ("macos-x64", Some("mac-x64")),
            ("linux-x64", Some("linux64")),
            ("linux-arm64", Some("linux-arm64")),
            ("linux-x64-musl", None),
            ("linux-arm64-musl", None),
            ("windows-x64", Some("win64")),
            ("windows-arm64", Some("win64")),
        ];
        assert_eq!(expected.len(), Platform::ALL.len());
        for (platform, (name, chrome)) in Platform::ALL.iter().zip(expected) {
            assert_eq!(platform.to_string(), name);
            assert_eq!(platform.chrome_platform(), chrome, "{platform}");
        }
    }

    #[test]
    fn current_platform_is_detected_here() {
        // Every CI and dev machine of this project is one of the 8 platforms.
        let platform = Platform::current().unwrap();
        assert!(Platform::ALL.contains(&platform));
    }
}
