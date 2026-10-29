//! Where each tool comes from, and how one version is downloaded, checked
//! and unpacked.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use super::archive::{self, ArchiveKind};
use super::http::{check_sha256, find_checksum};
use super::{
    LaunchRecord, OPENCODE_REGISTRY_ID, Scratch, ToolStore, node_bin_dir, node_exe, npm_cli,
    path_with, signature, write_launch_record,
};
use crate::registry::{BinaryTarget, PackageDistribution};
use crate::{Libc, Platform, ToolError, ToolId};

/// The Node major version that Consort installs. It must be an LTS line.
const NODE_MAJOR: &str = "24";

/// The longest time that one npm install may take.
const NPM_TIMEOUT: Duration = Duration::from_secs(600);

/// The download sources. [`Sources::default`] has the public addresses.
/// Tests point them at a local server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    /// Official Node builds, with `index.json` and `v<version>/` folders.
    pub node_dist: String,
    /// Unofficial Node builds for musl Linux, with the same layout.
    pub node_musl_dist: String,
    /// The Claude Code release CDN, with `stable` and `<version>/` folders.
    pub claude_releases: String,
    /// The npm registry, for the latest version of npm based tools.
    pub npm_registry: String,
    /// The GitHub API address of the latest Codex release.
    pub codex_latest_release: String,
    /// The Codex release downloads, with `rust-v<version>/` folders.
    pub codex_downloads: String,
    /// The ACP registry file.
    pub acp_registry: String,
    /// The OpenCode version service. It is used only when the ACP registry
    /// cannot be read.
    pub opencode_version: String,
}

impl Default for Sources {
    fn default() -> Self {
        Sources {
            node_dist: "https://nodejs.org/dist".to_owned(),
            node_musl_dist: "https://unofficial-builds.nodejs.org/download/release".to_owned(),
            claude_releases: "https://downloads.claude.ai/claude-code-releases".to_owned(),
            npm_registry: "https://registry.npmjs.org".to_owned(),
            codex_latest_release: "https://api.github.com/repos/openai/codex/releases/latest"
                .to_owned(),
            codex_downloads: "https://github.com/openai/codex/releases/download".to_owned(),
            acp_registry: crate::registry::REGISTRY_URL.to_owned(),
            opencode_version: "https://opencode.ai/update/api/latest/cli/npm".to_owned(),
        }
    }
}

#[derive(Deserialize)]
struct NodeRelease {
    version: String,
    #[serde(default)]
    lts: serde_json::Value,
}

#[derive(Deserialize)]
struct ClaudeManifest {
    version: String,
    platforms: BTreeMap<String, ClaudePlatform>,
}

#[derive(Deserialize)]
struct ClaudePlatform {
    checksum: String,
}

#[derive(Deserialize)]
struct NpmVersion {
    version: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
}

#[derive(Deserialize)]
struct OpenCodeVersion {
    version: String,
}

/// The fields of `codex-package.json` at the top of a Codex package.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CodexPackage {
    #[serde(default)]
    entrypoint: Option<String>,
    #[serde(default = "default_codex_path_dir")]
    pub(super) path_dir: String,
}

fn default_codex_path_dir() -> String {
    "codex-path".to_owned()
}

impl CodexPackage {
    /// The program, relative to the package folder. A missing or unsafe
    /// entry falls back to `bin/codex`.
    pub(super) fn entrypoint(&self, platform: &Platform) -> PathBuf {
        self.entrypoint
            .as_deref()
            .and_then(|entry| archive::safe_relative(Path::new(entry), 0).ok().flatten())
            .unwrap_or_else(|| PathBuf::from(format!("bin/codex{}", platform.exe_suffix())))
    }
}

/// Reads `codex-package.json`. A missing or damaged file gives the defaults.
pub(super) fn read_codex_package(dir: &Path) -> CodexPackage {
    fs::read(dir.join("codex-package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(CodexPackage {
            entrypoint: None,
            path_dir: default_codex_path_dir(),
        })
}

/// A relative path as text with `/` separators.
fn slash_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The file name at the end of a URL.
fn url_file_name(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/').next().unwrap_or(path)
}

async fn extract_blocking(
    archive_path: &Path,
    kind: ArchiveKind,
    dest: &Path,
    strip: usize,
) -> Result<(), ToolError> {
    let archive_path = archive_path.to_owned();
    let dest = dest.to_owned();
    tokio::task::spawn_blocking(move || archive::extract(&archive_path, kind, &dest, strip))
        .await
        .map_err(|err| ToolError::Archive {
            message: err.to_string(),
        })?
}

/// True when a program file is a Node script: a `.js`, `.cjs` or `.mjs`
/// file, or a file with a `node` shebang line.
fn is_node_script(path: &Path) -> bool {
    let by_extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext, "js" | "cjs" | "mjs"));
    if by_extension {
        return true;
    }
    fs::read(path).ok().is_some_and(|bytes| {
        let head = &bytes[..bytes.len().min(128)];
        let line = head.split(|b| *b == b'\n').next().unwrap_or(head);
        line.starts_with(b"#!") && String::from_utf8_lossy(line).contains("node")
    })
}

/// Chooses the program from the `bin` field of a `package.json`: a single
/// entry, else the entry with the package's own name, else the first one.
fn pick_bin(manifest: &serde_json::Value, package: &str) -> Option<String> {
    match manifest.get("bin")? {
        serde_json::Value::String(path) => Some(path.clone()),
        serde_json::Value::Object(bins) => {
            let own_name = package.rsplit('/').next().unwrap_or(package);
            let chosen = if bins.len() == 1 {
                bins.values().next()
            } else {
                bins.get(own_name).or_else(|| bins.values().next())
            };
            chosen?.as_str().map(str::to_owned)
        }
        _ => None,
    }
}

/// True for an npm package name that is safe in a path:
/// `name` or `@scope/name` with lower-case letters, digits, `-`, `.`, `_`.
fn valid_package_name(name: &str) -> bool {
    let body = name.strip_prefix('@').unwrap_or(name);
    let parts: Vec<&str> = body.split('/').collect();
    let expected_parts = if name.starts_with('@') { 2 } else { 1 };
    parts.len() == expected_parts
        && parts.iter().all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && part.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_')
                })
        })
}

impl ToolStore {
    fn node_base(&self) -> &str {
        if self.options.platform.libc == Libc::Musl {
            &self.options.sources.node_musl_dist
        } else {
            &self.options.sources.node_dist
        }
    }

    pub(super) async fn node_latest(&self) -> Result<String, ToolError> {
        let url = format!("{}/index.json", self.node_base());
        let releases: Vec<NodeRelease> = self.http.json(&url).await?;
        let prefix = format!("v{NODE_MAJOR}.");
        releases
            .iter()
            .find(|release| release.version.starts_with(&prefix) && release.lts.is_string())
            .map(|release| release.version.trim_start_matches('v').to_owned())
            .ok_or_else(|| ToolError::InvalidResponse {
                url,
                message: format!("no Node {NODE_MAJOR} LTS release"),
            })
    }

    pub(super) async fn stage_node(
        &self,
        version: &str,
        scratch: &Scratch,
    ) -> Result<(), ToolError> {
        let platform = &self.options.platform;
        let base = format!("{}/v{version}", self.node_base());
        let file = platform.node_archive(version);
        let sums = self.http.text(&format!("{base}/SHASUMS256.txt")).await?;
        let expected = find_checksum(&sums, &file)
            .ok_or_else(|| ToolError::MissingChecksum { name: file.clone() })?;
        let got = self
            .http
            .download(&format!("{base}/{file}"), &scratch.download)
            .await?;
        check_sha256(&file, &expected, &got.sha256)?;
        extract_blocking(
            &scratch.download,
            ArchiveKind::from_name(&file),
            &scratch.staging,
            1,
        )
        .await?;
        if !node_exe(&scratch.staging, platform).is_file() {
            return Err(ToolError::Archive {
                message: format!("{file} has no node program"),
            });
        }
        Ok(())
    }

    pub(super) async fn claude_latest(&self) -> Result<String, ToolError> {
        let url = format!("{}/stable", self.options.sources.claude_releases);
        Ok(self.http.text(&url).await?.trim().to_owned())
    }

    pub(super) async fn stage_claude(
        &self,
        version: &str,
        scratch: &Scratch,
    ) -> Result<(), ToolError> {
        let platform = &self.options.platform;
        let base = format!("{}/{version}", self.options.sources.claude_releases);
        let manifest_url = format!("{base}/manifest.json");
        let manifest_bytes = self.http.bytes(&manifest_url).await?;
        let signature_bytes = self.http.bytes(&format!("{manifest_url}.sig")).await?;
        signature::verify_detached(
            &self.options.claude_signing_key,
            &manifest_bytes,
            &signature_bytes,
        )?;
        let manifest: ClaudeManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|err| ToolError::InvalidResponse {
                url: manifest_url.clone(),
                message: err.to_string(),
            })?;
        // A signed manifest of another version must not pass as this one.
        if manifest.version != version {
            return Err(ToolError::InvalidResponse {
                url: manifest_url,
                message: format!("the manifest is for version {}", manifest.version),
            });
        }
        let platform_name = platform.claude_platform();
        let entry = manifest.platforms.get(platform_name).ok_or_else(|| {
            ToolError::UnsupportedPlatform {
                tool: ToolId::ClaudeCode,
                platform: platform.to_string(),
            }
        })?;
        let file = format!("claude{}", platform.exe_suffix());
        let got = self
            .http
            .download(&format!("{base}/{platform_name}/{file}"), &scratch.download)
            .await?;
        check_sha256(&file, &entry.checksum, &got.sha256)?;
        let program = scratch.staging.join(&file);
        fs::rename(&scratch.download, &program)
            .map_err(ToolError::io(format!("cannot move {}", program.display())))?;
        archive::set_mode(&program, 0o755)
    }

    pub(super) async fn codex_latest(&self) -> Result<String, ToolError> {
        let url = &self.options.sources.codex_latest_release;
        let release: GithubRelease = self.http.json(url).await?;
        release
            .tag_name
            .strip_prefix("rust-v")
            .map(str::to_owned)
            .ok_or_else(|| ToolError::InvalidResponse {
                url: url.clone(),
                message: format!("unexpected release tag {}", release.tag_name),
            })
    }

    pub(super) async fn stage_codex(
        &self,
        version: &str,
        scratch: &Scratch,
    ) -> Result<(), ToolError> {
        let platform = &self.options.platform;
        let base = format!("{}/rust-v{version}", self.options.sources.codex_downloads);
        let asset = platform.codex_asset();
        let sums = self
            .http
            .text(&format!("{base}/codex-package_SHA256SUMS"))
            .await?;
        let expected = find_checksum(&sums, &asset).ok_or_else(|| ToolError::MissingChecksum {
            name: asset.clone(),
        })?;
        let got = self
            .http
            .download(&format!("{base}/{asset}"), &scratch.download)
            .await?;
        check_sha256(&asset, &expected, &got.sha256)?;
        extract_blocking(&scratch.download, ArchiveKind::TarGz, &scratch.staging, 0).await?;
        let entry = read_codex_package(&scratch.staging).entrypoint(platform);
        if !scratch.staging.join(&entry).is_file() {
            return Err(ToolError::Archive {
                message: format!("{asset} has no {}", entry.display()),
            });
        }
        Ok(())
    }

    pub(super) async fn opencode_latest(&self) -> Result<String, ToolError> {
        let from_registry = match self.registry().await {
            Ok(registry) => registry
                .agent(OPENCODE_REGISTRY_ID)
                .map(|agent| agent.version.clone()),
            Err(err) => {
                tracing::warn!(error = %err, "cannot read the ACP registry, using the OpenCode version service");
                None
            }
        };
        if let Some(version) = from_registry {
            return Ok(version);
        }
        let reply: OpenCodeVersion = self
            .http
            .json(&self.options.sources.opencode_version)
            .await?;
        Ok(reply.version)
    }

    pub(super) async fn npm_latest(&self, package: &str) -> Result<String, ToolError> {
        let url = format!(
            "{}/{}/latest",
            self.options.sources.npm_registry,
            package.replace('/', "%2f")
        );
        let reply: NpmVersion = self.http.json(&url).await?;
        Ok(reply.version)
    }

    /// Runs npm with the given arguments, through the managed Node or the
    /// program in [`super::ToolStoreOptions::npm_program`].
    async fn run_npm(&self, args: Vec<OsString>) -> Result<(), ToolError> {
        let platform = &self.options.platform;
        let (program, mut full_args) = match &self.options.npm_program {
            Some(program) => (program.clone(), Vec::new()),
            None => {
                let node = self.require(&ToolId::Node)?;
                (
                    node_exe(&node.dir, platform),
                    vec![npm_cli(&node.dir, platform).into_os_string()],
                )
            }
        };
        full_args.extend(args);
        let mut command = tokio::process::Command::new(&program);
        command
            .args(&full_args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .env("npm_config_cache", self.root.join(".npm-cache"))
            .env("npm_config_update_notifier", "false")
            .env("npm_config_fund", "false")
            .env("npm_config_audit", "false");
        if let Some(node) = self.installed(&ToolId::Node)
            && let Some(path) = path_with(&node_bin_dir(&node.dir, platform))
        {
            command.env("PATH", path);
        }
        let output = tokio::time::timeout(NPM_TIMEOUT, command.output())
            .await
            .map_err(|_| ToolError::Npm {
                message: format!("npm did not finish in {} seconds", NPM_TIMEOUT.as_secs()),
            })?
            .map_err(ToolError::io(format!("cannot run {}", program.display())))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lines: Vec<&str> = stderr.lines().collect();
        let tail = lines[lines.len().saturating_sub(20)..].join("\n");
        Err(ToolError::Npm {
            message: format!("{} ({})", tail.trim(), output.status),
        })
    }

    /// Installs a built-in npm tool with optional packages and install
    /// scripts turned off, and checks that `entry` exists.
    pub(super) async fn stage_npm_tool(
        &self,
        tool: &ToolId,
        package: &str,
        version: &str,
        scratch: &Scratch,
    ) -> Result<(), ToolError> {
        let entry = match tool {
            ToolId::ClaudeAgentAcp => "dist/index.js",
            ToolId::GeminiCli => "bundle/gemini.js",
            _ => {
                return Err(ToolError::Unsupported {
                    tool: tool.clone(),
                    reason: "not an npm tool".to_owned(),
                });
            }
        };
        self.run_npm(vec![
            "install".into(),
            "--omit=optional".into(),
            "--ignore-scripts".into(),
            "--no-audit".into(),
            "--no-fund".into(),
            "--prefix".into(),
            scratch.staging.clone().into_os_string(),
            format!("{package}@{version}").into(),
        ])
        .await?;
        let entry_path = scratch
            .staging
            .join("node_modules")
            .join(package)
            .join(entry);
        if !entry_path.is_file() {
            return Err(ToolError::Npm {
                message: format!("{package}@{version} has no {entry}"),
            });
        }
        Ok(())
    }

    /// Installs an `npx` registry agent like `npx` would: with optional
    /// packages and install scripts. Saves the launch command.
    pub(super) async fn stage_npx(
        &self,
        tool: &ToolId,
        version: &str,
        package: &PackageDistribution,
        scratch: &Scratch,
    ) -> Result<(), ToolError> {
        let name = package.package_name();
        if !valid_package_name(name) {
            return Err(ToolError::Unsupported {
                tool: tool.clone(),
                reason: format!("\"{name}\" is not a valid npm package name"),
            });
        }
        self.run_npm(vec![
            "install".into(),
            "--no-audit".into(),
            "--no-fund".into(),
            "--prefix".into(),
            scratch.staging.clone().into_os_string(),
            format!("{name}@{version}").into(),
        ])
        .await?;
        let package_dir = Path::new("node_modules").join(name);
        let manifest_path = scratch.staging.join(&package_dir).join("package.json");
        let manifest: serde_json::Value = fs::read(&manifest_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| ToolError::Npm {
                message: format!("cannot read {}", manifest_path.display()),
            })?;
        let bin = pick_bin(&manifest, name).ok_or_else(|| ToolError::Npm {
            message: format!("{name} has no program (no bin in package.json)"),
        })?;
        let bin = archive::safe_relative(Path::new(&bin), 0)?
            .ok_or_else(|| ToolError::UnsafePath { path: bin.clone() })?;
        let program = package_dir.join(bin);
        let full = scratch.staging.join(&program);
        if !full.is_file() {
            return Err(ToolError::Npm {
                message: format!("{name} has no {}", program.display()),
            });
        }
        let record = LaunchRecord {
            program: slash_path(&program),
            node: is_node_script(&full),
            args: package.args.clone(),
            env: package.env.clone(),
        };
        write_launch_record(&scratch.staging, &record)
    }

    /// Installs a registry binary: downloads it, checks the SHA-256, unpacks
    /// it, and saves the launch command.
    pub(super) async fn stage_registry_binary(
        &self,
        tool: &ToolId,
        version: &str,
        registry_version: &str,
        target: &BinaryTarget,
        scratch: &Scratch,
    ) -> Result<(), ToolError> {
        // The registry has one address per platform, for its own version.
        if version != registry_version {
            return Err(ToolError::Unsupported {
                tool: tool.clone(),
                reason: format!("the ACP registry offers only version {registry_version}"),
            });
        }
        let name = url_file_name(&target.archive).to_owned();
        let expected = target
            .sha256
            .as_deref()
            .ok_or_else(|| ToolError::MissingChecksum { name: name.clone() })?;
        let command = archive::safe_relative(Path::new(&target.cmd), 0)?.ok_or_else(|| {
            ToolError::UnsafePath {
                path: target.cmd.clone(),
            }
        })?;
        let got = self
            .http
            .download(&target.archive, &scratch.download)
            .await?;
        check_sha256(&name, expected, &got.sha256)?;
        let program = scratch.staging.join(&command);
        match ArchiveKind::from_name(&target.archive) {
            ArchiveKind::Raw => {
                if let Some(parent) = program.parent() {
                    fs::create_dir_all(parent)
                        .map_err(ToolError::io(format!("cannot create {}", parent.display())))?;
                }
                fs::rename(&scratch.download, &program)
                    .map_err(ToolError::io(format!("cannot move {}", program.display())))?;
            }
            kind => extract_blocking(&scratch.download, kind, &scratch.staging, 0).await?,
        }
        if !program.is_file() {
            return Err(ToolError::Archive {
                message: format!("{name} has no {}", target.cmd),
            });
        }
        archive::set_mode(&program, 0o755)?;
        let record = LaunchRecord {
            program: slash_path(&command),
            node: false,
            args: target.args.clone(),
            env: target.env.clone(),
        };
        write_launch_record(&scratch.staging, &record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_the_program_of_a_package() {
        assert_eq!(
            pick_bin(&json!({ "bin": "cli.js" }), "x"),
            Some("cli.js".into())
        );
        assert_eq!(
            pick_bin(
                &json!({ "bin": { "a": "a.js", "pi-acp": "main.js" } }),
                "pi-acp"
            ),
            Some("main.js".into())
        );
        assert_eq!(
            pick_bin(&json!({ "bin": { "other": "o.js" } }), "@s/name"),
            Some("o.js".into())
        );
        assert_eq!(pick_bin(&json!({}), "x"), None);
    }

    #[test]
    fn checks_package_names() {
        assert!(valid_package_name("cline"));
        assert!(valid_package_name("@google/gemini-cli"));
        for bad in ["", "@x", "../x", "@a/../b", "a/b", "A", "@a/b/c"] {
            assert!(!valid_package_name(bad), "{bad}");
        }
    }

    #[test]
    fn codex_package_defaults() {
        let platform = Platform::new(crate::Os::Windows, crate::Arch::X64, Libc::System);
        let package = read_codex_package(Path::new("/nonexistent"));
        assert_eq!(
            package.entrypoint(&platform),
            PathBuf::from("bin/codex.exe")
        );
        assert_eq!(package.path_dir, "codex-path");
        let unsafe_entry = CodexPackage {
            entrypoint: Some("../evil".into()),
            path_dir: "codex-path".into(),
        };
        assert_eq!(
            unsafe_entry.entrypoint(&platform),
            PathBuf::from("bin/codex.exe")
        );
    }

    #[test]
    fn file_names_from_urls() {
        assert_eq!(
            url_file_name("https://x/y/opencode.zip?a=1"),
            "opencode.zip"
        );
        assert_eq!(url_file_name("amp-acp"), "amp-acp");
    }
}
