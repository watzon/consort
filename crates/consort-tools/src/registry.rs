//! Types and a parser for the ACP agent registry.
//!
//! The registry is a JSON file at [`REGISTRY_URL`]. Its schema is
//! `https://cdn.agentclientprotocol.com/registry/v1/latest/agent.schema.json`.
//! This module has no network and no install code, so the hub can use it
//! with `default-features = false`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Platform;

/// The address of the current registry file.
pub const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

/// The registry file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub version: String,
    pub agents: Vec<RegistryAgent>,
    #[serde(default)]
    pub extensions: Vec<serde_json::Value>,
}

impl Registry {
    /// Parses a registry file. Unknown fields are ignored, so a newer
    /// registry still loads.
    pub fn parse(bytes: &[u8]) -> Result<Registry, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// The agent with this id.
    pub fn agent(&self, id: &str) -> Option<&RegistryAgent> {
        self.agents.iter().find(|agent| agent.id == id)
    }
}

/// One agent in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_url: Option<String>,
    /// The address of the agent icon (an SVG file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub distribution: Distribution,
}

/// The ways to get an agent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Distribution {
    /// Archives or raw binaries, by registry platform key (see
    /// [`Platform::registry_key`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<BTreeMap<String, BinaryTarget>>,
    /// An npm package to run with Node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npx: Option<PackageDistribution>,
    /// A Python package for `uvx`. Consort does not support it yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uvx: Option<PackageDistribution>,
}

/// A binary build for one platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryTarget {
    /// The download address: a `.zip`, `.tar.gz`, `.tgz`, `.tar.bz2` or
    /// `.tbz2` archive, or a raw binary.
    pub archive: String,
    /// The program to run, relative to the unpacked archive.
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The SHA-256 of the archive in hex. Consort refuses a binary without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// A package for `npx` or `uvx`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDistribution {
    /// The package name, with an optional `@version`.
    pub package: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl PackageDistribution {
    /// The package name without the version, such as `@scope/name`.
    pub fn package_name(&self) -> &str {
        split_package(&self.package).0
    }

    /// The version in the package spec, if it has one.
    pub fn package_version(&self) -> Option<&str> {
        split_package(&self.package).1
    }
}

/// Splits `name@version` and `@scope/name@version`.
fn split_package(spec: &str) -> (&str, Option<&str>) {
    let start = usize::from(spec.starts_with('@'));
    match spec[start..].find('@') {
        Some(at) => (&spec[..start + at], Some(&spec[start + at + 1..])),
        None => (spec, None),
    }
}

impl RegistryAgent {
    /// The binary build for a platform, if it has a SHA-256.
    pub fn binary_for(&self, platform: &Platform) -> Option<&BinaryTarget> {
        self.distribution
            .binary
            .as_ref()?
            .get(platform.registry_key())
            .filter(|target| target.sha256.is_some())
    }

    /// True when Consort can install this agent on the platform: a binary
    /// with a SHA-256 for the platform, or an `npx` package.
    pub fn supported_on(&self, platform: &Platform) -> bool {
        self.distribution.npx.is_some() || self.binary_for(platform).is_some()
    }

    /// True when Consort can install this agent on at least one platform.
    pub fn supported_anywhere(&self) -> bool {
        Platform::ALL
            .iter()
            .any(|platform| self.supported_on(platform))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Arch, Libc, Os};

    const SNAPSHOT: &[u8] = include_bytes!("../tests/fixtures/registry-2026-10-29.json");

    #[test]
    fn parses_the_real_registry() {
        let registry = Registry::parse(SNAPSHOT).unwrap();
        assert_eq!(registry.version, "1.0.0");
        assert!(registry.agents.len() > 40);

        let mac = Platform::new(Os::Macos, Arch::Arm64, Libc::System);
        let musl = Platform::new(Os::Linux, Arch::X64, Libc::Musl);

        let opencode = registry.agent("opencode-v2").unwrap();
        assert!(opencode.supported_on(&mac));
        assert!(opencode.supported_on(&musl));
        let target = opencode.binary_for(&mac).unwrap();
        assert_eq!(target.cmd, "./opencode");
        assert_eq!(target.args, ["acp"]);
        assert!(opencode.icon.as_deref().unwrap().ends_with(".svg"));

        // A binary distribution without sha256 is not supported.
        let cursor = registry.agent("cursor").unwrap();
        assert!(cursor.distribution.binary.is_some());
        assert!(!cursor.supported_on(&mac));
        assert!(!cursor.supported_anywhere());

        // npx is supported everywhere.
        let gemini = registry.agent("gemini").unwrap();
        assert!(gemini.supported_on(&musl));
        let npx = gemini.distribution.npx.as_ref().unwrap();
        assert_eq!(npx.package_name(), "@google/gemini-cli");
        assert_eq!(npx.package_version(), Some("0.66.0"));
        assert_eq!(npx.args, ["--acp"]);

        // uvx alone is not supported.
        let fast_agent = registry.agent("fast-agent").unwrap();
        assert!(fast_agent.distribution.uvx.is_some());
        assert!(!fast_agent.supported_anywhere());
    }

    #[test]
    fn splits_package_specs() {
        assert_eq!(split_package("cline@3.0.74"), ("cline", Some("3.0.74")));
        assert_eq!(split_package("@a/b@1.0.0"), ("@a/b", Some("1.0.0")));
        assert_eq!(split_package("@a/b"), ("@a/b", None));
        assert_eq!(split_package("pi-acp"), ("pi-acp", None));
    }

    #[test]
    fn ignores_unknown_fields() {
        let json = br#"{"version":"2.0.0","agents":[{"id":"x","name":"X","version":"1.0.0",
            "description":"d","preview":{"version":"1.1.0"},"new_field":1,
            "distribution":{"npx":{"package":"x@1.0.0"}}}]}"#;
        let registry = Registry::parse(json).unwrap();
        assert!(registry.agents[0].supported_anywhere());
        assert!(registry.extensions.is_empty());
    }
}
