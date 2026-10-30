//! Where the files of a directory skill come from: a git repo, a zip
//! archive, an npm package, or a command (which the hub refuses).
//!
//! The source of a plugin in a `marketplace.json` has these types
//! (<https://code.claude.com/docs/en/plugins/marketplace-reference>). The hub
//! keeps the one location of a skill as one text, the `repo_url` of a
//! directory entry and the `url` of a `SkillSource::Directory`, so the proto
//! does not change:
//!
//! | Type | Text | Pin (`commit`) |
//! |---|---|---|
//! | git | `https://host/repo`, with `#ref` for a branch, tag or commit | the commit |
//! | `archive` | `archive+https://host/file.zip`, with `#sha256=<64 hex>` | SHA-256 of the zip |
//! | `npm` | `npm:<package>[@<version or tag>]`, with `?registry=<https url>` | the `integrity` of the version |
//! | `command` | `command:<plugin name>` | none |
//!
//! Decisions (docs/notes/smaller-items.md):
//! - **`archive`** is supported. Only `https`, at most 25 MiB, and a host that
//!   is not this machine, a private network, a link-local address or a cloud
//!   metadata address. With a `sha256`, a download that does not match is
//!   refused. The zip goes through the checks of an upload.
//! - **`npm`** is supported through the registry's HTTP API. The hub never
//!   runs npm, so no install script can run. The version must be exact or a
//!   tag, because the hub has no semver resolver. The tarball must match the
//!   `integrity` (SHA-512) that the registry gives. Only a package name:
//!   a tarball link, a git address, a folder or an alias is refused.
//! - **`command`** is not supported. It runs a shell command, and the hub
//!   would run it on its own machine with the hub's rights, when a marketplace
//!   says so. Claude Code asks the user to review the command first and has
//!   an admin switch to turn it off. The hub has neither, so it refuses with
//!   a message.

use std::net::IpAddr;

use consort_proto::error::ErrorCode;
use consort_proto::model::{SKILL_UPLOAD_MAX_BYTES, Sha256Hash};
use serde_json::Value;
use sha2::{Digest, Sha512};

use super::NewSkillFile;
use super::git::{
    GitOptions, Repo, check_ref, normalize_repo_path, parse_git_url, remote_commit, same_repo,
};
use super::package::skill_invalid;
use super::tarball::untar_folder;
use super::upload::unpack_zip_folder;
use crate::error::{HubError, Result};
use crate::hub::Hub;
use crate::store::skills as skill_store;

/// The default npm registry.
pub(crate) const NPM_REGISTRY: &str = "https://registry.npmjs.org";
/// The largest version document or packument that the hub reads.
const NPM_DOC_MAX_BYTES: usize = 2 * 1024 * 1024;
/// The longest npm package name.
const NPM_NAME_MAX: usize = 214;

/// The location of a directory skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Remote {
    Git(String),
    Archive {
        url: String,
        sha256: Option<Sha256Hash>,
    },
    Npm {
        package: String,
        version: Option<String>,
        registry: String,
    },
    Command {
        plugin: String,
    },
}

impl Remote {
    /// Reads the text of a `repo_url`. Anything that is not a marker is a
    /// git URL.
    pub(crate) fn parse(text: &str) -> Self {
        let text = text.trim();
        if let Some(rest) = text.strip_prefix("archive+") {
            let (url, fragment) = match rest.split_once('#') {
                Some((url, fragment)) => (url, Some(fragment)),
                None => (rest, None),
            };
            let sha256 = fragment
                .and_then(|f| f.strip_prefix("sha256="))
                .and_then(|h| h.to_ascii_lowercase().parse().ok());
            return Self::Archive {
                url: url.to_owned(),
                sha256,
            };
        }
        if let Some(rest) = text.strip_prefix("npm:") {
            let (spec, registry) = match rest.split_once("?registry=") {
                Some((spec, registry)) => (spec, registry.to_owned()),
                None => (rest, NPM_REGISTRY.to_owned()),
            };
            // `@scope/name@1.2.3`: the version starts at the last `@` that
            // is not the first character.
            let (package, version) = match spec.rfind('@') {
                Some(at) if at > 0 => (&spec[..at], Some(spec[at + 1..].to_owned())),
                _ => (spec, None),
            };
            return Self::Npm {
                package: package.to_owned(),
                version: version.filter(|v| !v.is_empty()),
                registry,
            };
        }
        if let Some(plugin) = text.strip_prefix("command:") {
            return Self::Command {
                plugin: plugin.to_owned(),
            };
        }
        Self::Git(text.to_owned())
    }

    /// The text of an archive location.
    pub(crate) fn archive_text(url: &str, sha256: Option<&str>) -> String {
        match sha256 {
            Some(hash) => format!("archive+{url}#sha256={hash}"),
            None => format!("archive+{url}"),
        }
    }

    /// The text of an npm location.
    pub(crate) fn npm_text(package: &str, version: Option<&str>, registry: Option<&str>) -> String {
        let mut text = format!("npm:{package}");
        if let Some(version) = version.filter(|v| !v.is_empty()) {
            text.push('@');
            text.push_str(version);
        }
        if let Some(registry) = registry.filter(|r| r.trim_end_matches('/') != NPM_REGISTRY) {
            text.push_str("?registry=");
            text.push_str(registry.trim_end_matches('/'));
        }
        text
    }

    /// The text of a command location.
    pub(crate) fn command_text(plugin: &str) -> String {
        format!("command:{plugin}")
    }
}

/// The message for a plugin that needs a command.
pub(crate) fn command_refused(plugin: &str) -> HubError {
    HubError::new(
        ErrorCode::SkillInvalid,
        format!(
            "The plugin {plugin} builds its files with a command. The hub does not run commands from a marketplace, because the command would run on the hub machine with the rights of the hub. Install the skills from the git repo of the plugin instead, or upload them as a zip."
        ),
    )
}

/// Whether a host may be fetched: not this machine, not a private network,
/// not a link-local or cloud metadata address.
pub(crate) fn host_is_blocked(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".localdomain")
        || host == "metadata.google.internal"
    {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let [a, b, ..] = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || (a == 100 && (64..128).contains(&b))
        }
        Ok(IpAddr::V6(ip)) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return host_is_blocked(&v4.to_string());
            }
            let first = ip.segments()[0];
            ip.is_loopback()
                || ip.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

/// Checks the URL of an archive or a tarball. `https`, a host that may be
/// fetched, no user name.
fn check_download_url(url: &str, allow_local: bool) -> Result<reqwest::Url> {
    let bad = |why: &str| {
        HubError::invalid(format!(
            "{url} is not a download address that the hub can use: {why}. Use an https address."
        ))
    };
    let parsed = reqwest::Url::parse(url).map_err(|_| bad("it is not a URL"))?;
    let scheme_ok = parsed.scheme() == "https" || (allow_local && parsed.scheme() == "http");
    if !scheme_ok {
        return Err(bad("only https is allowed"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(bad("it has a user name or a password"));
    }
    let host = parsed.host_str().ok_or_else(|| bad("it has no host"))?;
    if !allow_local && host_is_blocked(host) {
        return Err(bad(
            "it points to this machine, a private network or a metadata address",
        ));
    }
    Ok(parsed)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = u32::from(chunk[0]) << 16
            | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(TABLE[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The SHA-512 `integrity` string of npm for some bytes.
pub(crate) fn npm_integrity(bytes: &[u8]) -> String {
    format!("sha512-{}", base64(&Sha512::digest(bytes)))
}

/// A valid npm package name: lowercase letters, digits and `-._~`, with an
/// optional `@scope/` first.
fn check_npm_package(package: &str) -> Result<()> {
    let ok_part = |part: &str| {
        !part.is_empty()
            && !part.starts_with(['.', '_'])
            && part.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_' | '~')
            })
    };
    let valid = package.len() <= NPM_NAME_MAX
        && match package.strip_prefix('@') {
            Some(rest) => rest
                .split_once('/')
                .is_some_and(|(scope, name)| ok_part(scope) && ok_part(name)),
            None => ok_part(package),
        };
    if valid {
        Ok(())
    } else {
        Err(HubError::invalid(format!(
            "{package} is not an npm package name. The hub reads only a package name, such as @scope/name. A tarball link, a git address, a folder or an alias is not supported."
        )))
    }
}

/// An exact version (`1.2.3`, `1.2.3-beta.1`) or a tag (`latest`).
fn check_npm_version(version: &str) -> Result<()> {
    let exact = {
        let core = version.split(['-', '+']).next().unwrap_or("");
        let parts: Vec<&str> = core.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    };
    let tag = version
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase())
        && version
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
    let clean = version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'));
    if clean && (exact || tag) {
        Ok(())
    } else {
        Err(HubError::invalid(format!(
            "The npm version {version} is a range. The hub reads an exact version, such as 2.1.0, or a tag, such as latest."
        )))
    }
}

/// What the registry says about one version.
#[derive(Debug, Clone)]
struct NpmVersion {
    tarball: String,
    integrity: String,
}

/// The files of a fetched location.
#[derive(Debug)]
pub(crate) struct RemoteFiles {
    /// The pin: a commit, an archive hash or an npm integrity.
    pub commit: String,
    pub files: Vec<NewSkillFile>,
    /// The location without a ref (git), or the whole marker (other types).
    pub url: String,
    /// The branch, tag or commit that the fetch followed (git only).
    pub git_ref: Option<String>,
}

fn download_failed(url: &str, why: &str) -> HubError {
    HubError::new(
        ErrorCode::NotFound,
        format!("The hub could not download {url}: {why}. Check the address, then try again."),
    )
}

impl Hub {
    /// The git options for `url`. Credentials are allowed when the user gave
    /// the hub this URL: `typed_by_user`, a source that the user added, or a
    /// repo of a skill that the user installed from git.
    pub(crate) async fn git_options_for(
        &self,
        url: &str,
        typed_by_user: bool,
    ) -> Result<GitOptions> {
        let base = self.skill_sources.git_options();
        if typed_by_user {
            return Ok(base.trusted());
        }
        let sources = self.directory_sources().await?;
        if sources
            .iter()
            .any(|s| s.removable && same_repo(&s.url, url))
        {
            return Ok(base.trusted());
        }
        let skills = self.store.read(skill_store::skills).await?;
        let installed = skills.iter().any(|skill| {
            matches!(&skill.source, consort_proto::model::SkillSource::Git { url: u, .. } if same_repo(u, url))
        });
        Ok(if installed { base.trusted() } else { base })
    }

    /// Fetches the files of a skill folder from a location. `git_ref`
    /// overrides the ref that the location names (a git commit that a card
    /// shows, for example).
    pub(crate) async fn fetch_remote(
        &self,
        location: &str,
        path: Option<&str>,
        git_ref: Option<&str>,
        typed_by_user: bool,
    ) -> Result<RemoteFiles> {
        match Remote::parse(location) {
            Remote::Command { plugin } => Err(command_refused(&plugin)),
            Remote::Git(text) => {
                let options = self.git_options_for(&text, typed_by_user).await?;
                let target = parse_git_url(&text, options)?;
                let path = normalize_repo_path(path.or(target.path.as_deref()))?;
                let wanted = git_ref.map(str::to_owned).or(target.git_ref.clone());
                if let Some(r) = &wanted {
                    check_ref(r)?;
                }
                let repo = Repo::fetch(
                    self.skill_sources.tmp_root(),
                    &target.url,
                    wanted.as_deref(),
                    options,
                )
                .await?;
                let files = repo.skill_files(path.as_deref()).await?;
                Ok(RemoteFiles {
                    commit: repo.commit.clone(),
                    files,
                    url: target.url,
                    // A commit that a card pins is not a branch to follow.
                    git_ref: target
                        .git_ref
                        .or_else(|| git_ref.map(str::to_owned))
                        .or(wanted),
                })
            }
            Remote::Archive { url, sha256 } => {
                let path = normalize_repo_path(path)?;
                let (hash, bytes) = self.download_archive(&url, sha256.as_ref()).await?;
                let folder = path.clone();
                let files = tokio::task::spawn_blocking(move || {
                    unpack_zip_folder(&bytes, folder.as_deref())
                })
                .await
                .map_err(|_| HubError::internal("the zip reader stopped"))??;
                Ok(RemoteFiles {
                    commit: hash.as_str().to_owned(),
                    files,
                    url: location.to_owned(),
                    git_ref: None,
                })
            }
            Remote::Npm {
                package,
                version,
                registry,
            } => {
                let path = normalize_repo_path(path)?;
                let found = self
                    .npm_version(&package, version.as_deref(), &registry)
                    .await?;
                let bytes = self
                    .download(&found.tarball, SKILL_UPLOAD_MAX_BYTES)
                    .await?;
                if npm_integrity(&bytes) != found.integrity {
                    return Err(skill_invalid(format!(
                        "The npm package {package} does not match the integrity hash that the registry gives. The hub did not use it."
                    )));
                }
                let folder = path.clone();
                let files =
                    tokio::task::spawn_blocking(move || untar_folder(&bytes, folder.as_deref()))
                        .await
                        .map_err(|_| HubError::internal("the tarball reader stopped"))??;
                Ok(RemoteFiles {
                    commit: found.integrity,
                    files,
                    url: location.to_owned(),
                    git_ref: None,
                })
            }
        }
    }

    /// The pin that a location has now, with the least work: the commit of a
    /// git ref, the integrity of an npm version, or the hash of an archive.
    pub(crate) async fn remote_pin(
        &self,
        location: &str,
        git_ref: Option<&str>,
        typed_by_user: bool,
    ) -> Result<String> {
        match Remote::parse(location) {
            Remote::Command { plugin } => Err(command_refused(&plugin)),
            Remote::Git(text) => {
                let options = self.git_options_for(&text, typed_by_user).await?;
                let target = parse_git_url(&text, options)?;
                let wanted = git_ref.map(str::to_owned).or(target.git_ref);
                remote_commit(
                    self.skill_sources.tmp_root(),
                    &target.url,
                    wanted.as_deref(),
                    options,
                )
                .await
            }
            // The marketplace pinned the bytes. They cannot change.
            Remote::Archive {
                sha256: Some(hash), ..
            } => Ok(hash.as_str().to_owned()),
            Remote::Archive { url, sha256: None } => Ok(self
                .download_archive(&url, None)
                .await?
                .0
                .as_str()
                .to_owned()),
            Remote::Npm {
                package,
                version,
                registry,
            } => Ok(self
                .npm_version(&package, version.as_deref(), &registry)
                .await?
                .integrity),
        }
    }

    /// Downloads a zip, and checks its hash when the location has one.
    async fn download_archive(
        &self,
        url: &str,
        expected: Option<&Sha256Hash>,
    ) -> Result<(Sha256Hash, Vec<u8>)> {
        let bytes = self.download(url, SKILL_UPLOAD_MAX_BYTES).await?;
        let hash = crate::util::sha256(&bytes);
        if let Some(expected) = expected
            && *expected != hash
        {
            return Err(skill_invalid(format!(
                "The archive at {url} does not match the sha256 that the marketplace gives. The hub did not use it."
            )));
        }
        Ok((hash, bytes))
    }

    fn download_client(&self) -> Result<reqwest::Client> {
        let config = self.skill_sources.config();
        let allow_local = config.allow_local;
        // A redirect goes through the same check as the first address.
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 3 {
                return attempt.error("too many redirects");
            }
            let blocked = !allow_local
                && (attempt.url().scheme() != "https"
                    || attempt.url().host_str().is_none_or(host_is_blocked));
            if blocked {
                attempt.error("the download address moved to a place the hub does not fetch")
            } else {
                attempt.follow()
            }
        });
        reqwest::Client::builder()
            .timeout(config.download_timeout)
            .https_only(!allow_local)
            .redirect(policy)
            .user_agent(concat!("consort-hub/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| HubError::internal("the HTTP client did not start"))
    }

    /// Downloads at most `max_bytes` from an https address.
    async fn download(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>> {
        let config = self.skill_sources.config();
        let parsed = check_download_url(url, config.allow_local)?;
        let http = self.download_client()?;
        let limit = crate::util::duration_text(config.download_timeout);
        let mut response = http.get(parsed).send().await.map_err(|e| {
            if e.is_timeout() {
                download_failed(url, &format!("the server did not answer within {limit}"))
            } else {
                download_failed(url, "the server could not be reached")
            }
        })?;
        if !response.status().is_success() {
            return Err(download_failed(
                url,
                &format!("the server answered {}", response.status().as_u16()),
            ));
        }
        if response.content_length().is_some_and(|len| len > max_bytes) {
            return Err(skill_invalid(format!(
                "The download at {url} is larger than {max_bytes} bytes."
            )));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| {
            if e.is_timeout() {
                download_failed(url, &format!("the server did not finish within {limit}"))
            } else {
                download_failed(url, "the answer stopped early")
            }
        })? {
            body.extend_from_slice(&chunk);
            if body.len() as u64 > max_bytes {
                return Err(skill_invalid(format!(
                    "The download at {url} is larger than {max_bytes} bytes."
                )));
            }
        }
        Ok(body)
    }

    /// Reads the document of one npm version: its tarball address and its
    /// integrity hash.
    async fn npm_version(
        &self,
        package: &str,
        version: Option<&str>,
        registry: &str,
    ) -> Result<NpmVersion> {
        check_npm_package(package)?;
        let version = version.unwrap_or("latest");
        check_npm_version(version)?;
        let config = self.skill_sources.config();
        let registry = registry.trim_end_matches('/');
        // The registry must be https, like any download.
        check_download_url(registry, config.allow_local)?;
        let encoded = package.replace('/', "%2F");
        let url = format!("{registry}/{encoded}/{version}");
        let http = self.download_client()?;
        let limit = crate::util::duration_text(config.download_timeout);
        let mut response = http
            .get(&url)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    download_failed(&url, &format!("the registry did not answer within {limit}"))
                } else {
                    download_failed(&url, "the registry could not be reached")
                }
            })?;
        if response.status().as_u16() == 404 {
            return Err(HubError::not_found(format!(
                "The npm registry has no package {package} at {version}. Check the name and the version."
            )));
        }
        if !response.status().is_success() {
            return Err(download_failed(
                &url,
                &format!("the registry answered {}", response.status().as_u16()),
            ));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| download_failed(&url, "the answer stopped early"))?
        {
            body.extend_from_slice(&chunk);
            if body.len() > NPM_DOC_MAX_BYTES {
                return Err(download_failed(&url, "the answer is too large"));
            }
        }
        let doc: Value = serde_json::from_slice(&body)
            .map_err(|_| download_failed(&url, "the answer is not JSON"))?;
        let tarball = doc
            .pointer("/dist/tarball")
            .and_then(Value::as_str)
            .ok_or_else(|| download_failed(&url, "the version has no tarball address"))?;
        let integrity = doc
            .pointer("/dist/integrity")
            .and_then(Value::as_str)
            .and_then(|all| all.split_whitespace().find(|i| i.starts_with("sha512-")))
            .ok_or_else(|| {
                skill_invalid(format!(
                    "The npm package {package} has no sha512 integrity hash, so the hub cannot check it."
                ))
            })?;
        Ok(NpmVersion {
            tarball: tarball.to_owned(),
            integrity: integrity.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_round_trip() {
        let hash = "a".repeat(64);
        let archive = Remote::archive_text("https://x.example/p.zip", Some(&hash));
        assert_eq!(
            Remote::parse(&archive),
            Remote::Archive {
                url: "https://x.example/p.zip".to_owned(),
                sha256: hash.parse().ok(),
            }
        );
        let npm = Remote::npm_text("@acme/skills", Some("2.1.0"), Some("https://npm.example/"));
        assert_eq!(npm, "npm:@acme/skills@2.1.0?registry=https://npm.example");
        assert_eq!(
            Remote::parse(&npm),
            Remote::Npm {
                package: "@acme/skills".to_owned(),
                version: Some("2.1.0".to_owned()),
                registry: "https://npm.example".to_owned(),
            }
        );
        assert_eq!(
            Remote::parse("npm:tool"),
            Remote::Npm {
                package: "tool".to_owned(),
                version: None,
                registry: NPM_REGISTRY.to_owned(),
            }
        );
        assert_eq!(
            Remote::parse(&Remote::command_text("fmt")),
            Remote::Command {
                plugin: "fmt".to_owned()
            }
        );
        assert_eq!(
            Remote::parse("https://github.com/a/b#v2"),
            Remote::Git("https://github.com/a/b#v2".to_owned())
        );
    }

    #[test]
    fn hosts_on_this_machine_or_a_private_network_are_blocked() {
        for blocked in [
            "localhost",
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.5",
            "172.16.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "[::1]",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "metadata.google.internal",
            "printer.local",
        ] {
            assert!(host_is_blocked(blocked), "{blocked}");
        }
        for open in [
            "github.com",
            "8.8.8.8",
            "registry.npmjs.org",
            "2606:4700::1",
        ] {
            assert!(!host_is_blocked(open), "{open}");
        }
        assert!(check_download_url("https://169.254.169.254/x.zip", false).is_err());
        assert!(check_download_url("http://example.com/x.zip", false).is_err());
        assert!(check_download_url("https://user:pw@example.com/x.zip", false).is_err());
        assert!(check_download_url("https://example.com/x.zip", false).is_ok());
    }

    #[test]
    fn npm_names_and_versions_are_checked() {
        for good in ["tool", "@acme/skills", "a-b.c_d"] {
            assert!(check_npm_package(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "Tool",
            "../x",
            "a/b",
            "file:../x",
            "git+https://x/y",
            "npm:other",
            "https://x.example/p.tgz",
            "@scope",
        ] {
            assert!(check_npm_package(bad).is_err(), "{bad}");
        }
        for good in ["1.2.3", "1.2.3-beta.1", "latest", "next"] {
            assert!(check_npm_version(good).is_ok(), "{good}");
        }
        for range in ["^1.2.3", "~1", ">=1.0.0", "1.x", "*", "1.2", "1 || 2"] {
            assert!(check_npm_version(range).is_err(), "{range}");
        }
    }

    #[test]
    fn the_integrity_is_the_npm_form() {
        // sha512 of the empty input, as npm writes it.
        assert_eq!(
            npm_integrity(b""),
            "sha512-z4PhNX7vuL3xVChQ1m2AB9Yg5AULVxXcg/SpIdNs6c5H0NE8XYXysP+DGNKHfuwvY7kxvUdBeoGlODJ6+SfaPg=="
        );
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn a_command_is_refused_with_the_reason() {
        let message = command_refused("fmt").to_error_body().message;
        assert!(message.contains("does not run commands"), "{message}");
        assert!(message.contains("git repo"), "{message}");
    }
}
