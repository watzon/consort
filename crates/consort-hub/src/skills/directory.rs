//! The skill directories: where the user finds skills to add.
//!
//! - **Smithery** (built in). The public skills registry of the Smithery
//!   Platform API: `GET https://api.smithery.ai/skills?q=...&pageSize=...`
//!   and `GET /skills/{namespace}/{slug}`
//!   (<https://smithery.ai/docs/api-reference/skills/list-or-search-skills>,
//!   <https://smithery.ai/docs/api-reference/skills/get-a-skill>). Search
//!   needs no key. A result names its files with `gitUrl`, a GitHub page
//!   URL of the skill folder. The registry gives no audit results, so the
//!   entries have none.
//! - **Anthropic** (built in). The repo `github.com/anthropics/skills`,
//!   read as a marketplace: its `.claude-plugin/marketplace.json` lists
//!   plugins with `skills` folders, and the hub reads the `SKILL.md` of each
//!   (<https://code.claude.com/docs/en/plugin-marketplaces>,
//!   <https://code.claude.com/docs/en/plugins-reference>).
//! - **Marketplaces that the user adds**: a git repo (read the same way as
//!   Anthropic's, or as a list of every `SKILL.md` when it has no
//!   marketplace file), or the URL of a `marketplace.json` file.
//! - **skills.sh** is not a source. Its API needs a Vercel OIDC token and
//!   its robots.txt disallows `/api/`, so the app links to it and the user
//!   pastes an `owner/repo`. The hub never calls it.
//!
//! Listings and searches are cached. A source that fails is named in
//! `unavailable` and the others still answer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use consort_proto::client_api::{
    SKILL_DIRECTORY_PAGE_MAX, SkillDirectoryAddSource, SkillDirectoryRemoveSource,
    SkillDirectorySearch, SkillDirectorySearchResult, SkillDirectorySources,
    SkillDirectorySourcesResult,
};
use consort_proto::ids::SkillId;
use consort_proto::model::{
    SKILL_MANIFEST_FILE, SKILL_UPDATE_CHECK_MS, Skill, SkillDirectoryEntry, SkillDirectoryKind,
    SkillDirectorySource, SkillSource,
};
use serde_json::Value;

use super::front_matter;
use super::git::{GitOptions, Repo, TreeItem, normalize_repo_path, parse_git_url, same_repo};
use super::install::Fetched;
use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::store::{meta_get, skills as skill_store};

/// The ID of the Smithery directory.
pub(crate) const SMITHERY_ID: &str = "smithery";
/// The ID of Anthropic's skills repo.
pub(crate) const ANTHROPIC_ID: &str = "anthropic";
/// The public Smithery Platform API.
pub(crate) const SMITHERY_API_URL: &str = "https://api.smithery.ai";
/// Anthropic's skills repo.
pub(crate) const ANTHROPIC_REPO_URL: &str = "https://github.com/anthropics/skills";
/// The marketplace file in a repo.
const MARKETPLACE_PATH: &str = ".claude-plugin/marketplace.json";
/// The sources that the user added, as JSON in `meta`.
const SOURCES_META_KEY: &str = "skill_directory_sources";

/// How long a repo or marketplace listing stays fresh.
const LISTING_TTL: Duration = Duration::from_millis(SKILL_UPDATE_CHECK_MS as u64);
/// How long one Smithery search stays fresh.
const SEARCH_TTL: Duration = Duration::from_secs(60 * 60);
/// How long a failed source is not asked again.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(60);
/// How many searches the hub keeps.
const SEARCHES_KEPT: usize = 64;
/// The time limit of one HTTP request.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
/// The most bytes of one JSON answer or marketplace file.
const JSON_MAX_BYTES: usize = 2 * 1024 * 1024;
/// The most bytes of a `SKILL.md` that a listing reads.
const MANIFEST_READ_MAX_BYTES: u64 = 256 * 1024;
/// The most skills in one listing.
const LISTING_MAX_SKILLS: usize = 1_000;
/// The most results that the hub asks Smithery for.
const SMITHERY_PAGE_SIZE: u32 = 100;
/// The default page of `skill_directory.search`.
const PAGE_DEFAULT: u32 = 30;
/// How long a preview stays for the install that follows it.
pub(crate) const PREVIEW_TTL: Duration = Duration::from_secs(60 * 60);
/// How many previews the hub keeps.
const PREVIEWS_KEPT: usize = 16;

/// The addresses of the built-in directories, and whether local URLs are
/// allowed. Tests point them at local fakes.
#[derive(Debug, Clone)]
pub(crate) struct SourceConfig {
    pub smithery_url: String,
    pub anthropic_url: String,
    pub allow_local: bool,
}

impl Default for SourceConfig {
    fn default() -> Self {
        let env = |key: &str, default: &str| {
            std::env::var(key)
                .ok()
                .map(|v| v.trim().trim_end_matches('/').to_owned())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| default.to_owned())
        };
        Self {
            smithery_url: env("CONSORT_SMITHERY_URL", SMITHERY_API_URL),
            anthropic_url: env("CONSORT_ANTHROPIC_SKILLS_URL", ANTHROPIC_REPO_URL),
            allow_local: false,
        }
    }
}

struct Cached<T> {
    ok: Option<(Instant, T)>,
    failed: Option<(Instant, String)>,
}

impl<T> Default for Cached<T> {
    fn default() -> Self {
        Self {
            ok: None,
            failed: None,
        }
    }
}

/// The skill directory state of a running hub.
pub struct SkillSources {
    config: Mutex<SourceConfig>,
    tmp_root: PathBuf,
    listings: tokio::sync::Mutex<HashMap<String, Cached<Vec<SkillDirectoryEntry>>>>,
    searches: tokio::sync::Mutex<HashMap<String, Cached<Vec<SkillDirectoryEntry>>>>,
    previews: Mutex<HashMap<String, (Instant, Arc<Fetched>)>>,
    /// Upstream commits whose files are the same as the installed version,
    /// so the update check does not fetch them again.
    pub(crate) same_files: Mutex<HashMap<SkillId, String>>,
    /// One update check at a time.
    pub(crate) update_check: tokio::sync::Mutex<()>,
}

impl SkillSources {
    /// The state of a hub whose data folder is `data_dir`. Temp folders of
    /// fetches go to `<data_dir>/tmp`. Leftovers of a hub that stopped in
    /// the middle of a fetch are removed.
    pub(crate) fn open(data_dir: &Path) -> Self {
        let tmp_root = data_dir.join("tmp");
        if let Ok(entries) = std::fs::read_dir(&tmp_root) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("skill-fetch-")
                {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        Self {
            config: Mutex::new(SourceConfig::default()),
            tmp_root,
            listings: tokio::sync::Mutex::default(),
            searches: tokio::sync::Mutex::default(),
            previews: Mutex::default(),
            same_files: Mutex::default(),
            update_check: tokio::sync::Mutex::default(),
        }
    }

    pub(crate) fn config(&self) -> SourceConfig {
        self.config.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// Changes the addresses of the built-in directories, and drops every
    /// cache.
    pub(crate) fn set_config(&self, change: impl FnOnce(&mut SourceConfig)) {
        if let Ok(mut config) = self.config.lock() {
            change(&mut config);
            config.smithery_url = config.smithery_url.trim_end_matches('/').to_owned();
        }
        if let Ok(mut l) = self.listings.try_lock() {
            l.clear();
        }
        if let Ok(mut s) = self.searches.try_lock() {
            s.clear();
        }
        if let Ok(mut p) = self.previews.lock() {
            p.clear();
        }
    }

    pub(crate) fn git_options(&self) -> GitOptions {
        GitOptions {
            allow_local: self.config().allow_local,
        }
    }

    pub(crate) fn tmp_root(&self) -> &Path {
        &self.tmp_root
    }

    pub(crate) fn preview(&self, key: &str) -> Option<Arc<Fetched>> {
        let previews = self.previews.lock().ok()?;
        previews
            .get(key)
            .filter(|(at, _)| at.elapsed() < PREVIEW_TTL)
            .map(|(_, f)| f.clone())
    }

    pub(crate) fn keep_preview(&self, key: String, fetched: Arc<Fetched>) {
        if let Ok(mut previews) = self.previews.lock() {
            previews.retain(|_, (at, _)| at.elapsed() < PREVIEW_TTL);
            while previews.len() >= PREVIEWS_KEPT {
                let Some(oldest) = previews
                    .iter()
                    .min_by_key(|(_, (at, _))| *at)
                    .map(|(k, _)| k.clone())
                else {
                    break;
                };
                previews.remove(&oldest);
            }
            previews.insert(key, (Instant::now(), fetched));
        }
    }

    fn http(&self) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .https_only(!self.config().allow_local)
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent(concat!("consort-hub/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| HubError::internal("the HTTP client did not start"))
    }

    /// GETs a JSON document of at most [`JSON_MAX_BYTES`].
    async fn get_json(&self, url: &str) -> std::result::Result<Value, String> {
        let http = self.http().map_err(|e| e.to_string())?;
        let mut response = http
            .get(url)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    "it did not answer in time".to_owned()
                } else {
                    "it could not be reached".to_owned()
                }
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("it answered {}", status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|len| len > JSON_MAX_BYTES as u64)
        {
            return Err("its answer is too large".to_owned());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "its answer stopped early".to_owned())?
        {
            body.extend_from_slice(&chunk);
            if body.len() > JSON_MAX_BYTES {
                return Err("its answer is too large".to_owned());
            }
        }
        serde_json::from_slice(&body).map_err(|_| "its answer is not JSON".to_owned())
    }
}

#[cfg(test)]
impl SkillSources {
    /// Puts a listing in the cache, as if the source gave it.
    pub(crate) async fn seed_listing(&self, source_id: &str, entries: Vec<SkillDirectoryEntry>) {
        self.listings.lock().await.insert(
            source_id.to_owned(),
            Cached {
                ok: Some((Instant::now(), entries)),
                failed: None,
            },
        );
    }

    /// Drops the cached listings and searches, as when they grow old.
    pub(crate) async fn forget_cache(&self) {
        self.listings.lock().await.clear();
        self.searches.lock().await.clear();
    }
}

/// Where the entries of a user source come from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Location {
    /// A git repo, with the path of its marketplace file.
    Git {
        url: String,
        git_ref: Option<String>,
        marketplace: String,
    },
    /// A `marketplace.json` over HTTP. Only plugins with a git source give
    /// entries, because a relative path has no repo.
    Http { url: String },
}

/// Reads the URL of a source that the user adds. A `.json` URL is a
/// marketplace file. A raw GitHub URL of a marketplace file is read from
/// its repo, so relative plugin paths work. Anything else is a git repo.
fn locate(input: &str, options: GitOptions) -> Result<(String, Location)> {
    let input = input.trim();
    if input.to_ascii_lowercase().ends_with(".json") {
        let lower = input.to_ascii_lowercase();
        let allowed =
            lower.starts_with("https://") || (options.allow_local && lower.starts_with("http://"));
        if !allowed || input.contains(char::is_whitespace) {
            return Err(HubError::invalid(format!(
                "{input} is not an https URL. Use the https URL of a marketplace.json file or a git repo."
            )));
        }
        if let Some(rest) = input.strip_prefix("https://raw.githubusercontent.com/") {
            let parts: Vec<&str> = rest.split('/').collect();
            if parts.len() >= 4 {
                let repo = format!("https://github.com/{}/{}", parts[0], parts[1]);
                let file = parts[3..].join("/");
                return Ok((
                    input.to_owned(),
                    Location::Git {
                        url: repo,
                        git_ref: Some(parts[2].to_owned()),
                        marketplace: file,
                    },
                ));
            }
        }
        return Ok((
            input.to_owned(),
            Location::Http {
                url: input.to_owned(),
            },
        ));
    }
    let target = parse_git_url(input, options)?;
    Ok((
        target.url.clone(),
        Location::Git {
            url: target.url,
            git_ref: target.git_ref,
            marketplace: MARKETPLACE_PATH.to_owned(),
        },
    ))
}

/// The prefix of the entry IDs of a source: `smithery`, `anthropic`, or
/// `m` and 8 hex digits of the hash of the source ID.
pub(crate) fn entry_prefix(source_id: &str) -> String {
    match source_id {
        SMITHERY_ID | ANTHROPIC_ID => source_id.to_owned(),
        other => format!("m{}", &crate::util::sha256(other.as_bytes()).as_str()[..8]),
    }
}

/// Whether a text is an entry ID of a directory (and not a git URL).
pub(crate) fn looks_like_entry_id(text: &str) -> bool {
    let Some((prefix, rest)) = text.split_once(':') else {
        return false;
    };
    !rest.is_empty()
        && !rest.starts_with("//")
        && (prefix == SMITHERY_ID
            || prefix == ANTHROPIC_ID
            || (prefix.len() == 9
                && prefix.starts_with('m')
                && prefix[1..].chars().all(|c| c.is_ascii_hexdigit())))
}

/// Splits a Smithery `gitUrl` (a GitHub page URL of the skill folder, or a
/// repo URL) into the repo and the folder.
fn split_git_page_url(url: &str, options: GitOptions) -> Option<(String, Option<String>)> {
    let target = parse_git_url(url, options).ok()?;
    Some((target.url, target.path))
}

/// The text of a JSON field.
fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// One Smithery skill as an entry. A skill with no git URL has no files to
/// install, so it is left out.
fn smithery_entry(item: &Value, options: GitOptions) -> Option<SkillDirectoryEntry> {
    let namespace = text(item, "namespace")?;
    let slug = text(item, "slug")?;
    let (repo_url, path) = split_git_page_url(&text(item, "gitUrl")?, options)?;
    Some(SkillDirectoryEntry {
        entry_id: format!("{SMITHERY_ID}:{namespace}/{slug}"),
        directory: SkillDirectoryKind::Smithery,
        source_id: Some(SMITHERY_ID.to_owned()),
        name: text(item, "displayName").unwrap_or_else(|| slug.clone()),
        description: text(item, "description").unwrap_or_default(),
        author: Some(namespace.clone()),
        url: Some(format!("https://smithery.ai/skills/{namespace}/{slug}")),
        repo_url: Some(repo_url),
        path,
        license: None,
        installs: item
            .get("totalActivations")
            .and_then(Value::as_f64)
            .filter(|n| *n >= 0.0)
            .map(|n| n as u64),
        audits: Vec::new(),
        installed_skill_id: None,
    })
}

/// A Smithery name part: letters, digits, `-`, `_` and `.`.
fn is_smithery_name(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Builds a query string. Values are percent-encoded.
fn query(pairs: &[(&str, &str)]) -> String {
    fn encode(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The skill folders that a plugin of a marketplace names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PluginSkills {
    plugin: String,
    description: String,
    author: Option<String>,
    license: Option<String>,
    /// `None` for a plugin in the marketplace repo; the repo URL otherwise.
    repo_url: Option<String>,
    /// Folders to scan: each a skill folder or a folder of skill folders.
    folders: Vec<String>,
    /// For a plugin in another repo: the folders are listed, not scanned.
    listed_only: bool,
}

fn join_path(base: &str, rel: &str) -> Option<String> {
    let rel = rel.trim().trim_start_matches("./").trim_end_matches('/');
    let joined = match (base.is_empty(), rel.is_empty() || rel == ".") {
        (true, true) => String::new(),
        (true, false) => rel.to_owned(),
        (false, true) => base.to_owned(),
        (false, false) => format!("{base}/{rel}"),
    };
    if joined.is_empty() {
        return Some(joined);
    }
    normalize_repo_path(Some(&joined))
        .ok()
        .map(Option::unwrap_or_default)
}

/// Reads the plugins of a marketplace file. `root` is the folder of the
/// marketplace in its repo (the parent of `.claude-plugin`).
fn marketplace_plugins(doc: &Value, root: &str, options: GitOptions) -> Vec<PluginSkills> {
    let plugin_root = doc
        .get("metadata")
        .and_then(|m| text(m, "pluginRoot"))
        .unwrap_or_default();
    let mut out = Vec::new();
    let Some(plugins) = doc.get("plugins").and_then(Value::as_array) else {
        return out;
    };
    for plugin in plugins {
        let Some(name) = text(plugin, "name") else {
            continue;
        };
        let explicit: Vec<String> = match plugin.get("skills") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        };
        let author = plugin
            .get("author")
            .and_then(|a| text(a, "name").or_else(|| a.as_str().map(str::to_owned)));
        let (repo_url, plugin_dir, listed_only) = match plugin.get("source") {
            Some(Value::String(s)) => {
                let rel = if !s.contains('/') && s != "." && !plugin_root.is_empty() {
                    format!("{}/{s}", plugin_root.trim_end_matches('/'))
                } else {
                    s.clone()
                };
                let Some(dir) = join_path(root, &rel) else {
                    continue;
                };
                (None, dir, false)
            }
            Some(Value::Object(source)) => {
                let kind = source.get("source").and_then(Value::as_str).unwrap_or("");
                let (url, dir) = match kind {
                    "github" => (
                        source
                            .get("repo")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        String::new(),
                    ),
                    "url" => (
                        source.get("url").and_then(Value::as_str).map(str::to_owned),
                        String::new(),
                    ),
                    "git-subdir" => (
                        source.get("url").and_then(Value::as_str).map(str::to_owned),
                        source
                            .get("path")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    ),
                    _ => (None, String::new()),
                };
                let Some(url) = url.and_then(|u| parse_git_url(&u, options).ok()) else {
                    continue;
                };
                let Some(dir) = join_path("", &dir) else {
                    continue;
                };
                // Only folders that the plugin names can be listed: the hub
                // does not fetch another repo to scan it.
                if explicit.is_empty() {
                    continue;
                }
                (Some(url.url), dir, true)
            }
            _ => continue,
        };
        let mut folders: Vec<String> = explicit
            .iter()
            .filter_map(|rel| join_path(&plugin_dir, rel))
            .collect();
        if !listed_only && let Some(default) = join_path(&plugin_dir, "skills") {
            folders.push(default);
        }
        folders.dedup();
        out.push(PluginSkills {
            plugin: name,
            description: text(plugin, "description").unwrap_or_default(),
            author,
            license: text(plugin, "license"),
            repo_url,
            folders,
            listed_only,
        });
    }
    out
}

/// The parent folder of a `SKILL.md` path: `""` for the root.
fn skill_folder_of(path: &str) -> Option<&str> {
    if path == SKILL_MANIFEST_FILE {
        return Some("");
    }
    path.strip_suffix(SKILL_MANIFEST_FILE)
        .and_then(|p| p.strip_suffix('/'))
}

fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A skill folder found in a listing, before its `SKILL.md` is read.
struct Found {
    path: String,
    repo_url: Option<String>,
    author: Option<String>,
    license: Option<String>,
    fallback_description: String,
}

impl Hub {
    // Sources --------------------------------------------------------------

    /// The built-in sources and the ones that the user added.
    pub(crate) async fn directory_sources(&self) -> Result<Vec<SkillDirectorySource>> {
        let config = self.skill_sources.config();
        let mut sources = vec![
            SkillDirectorySource {
                id: SMITHERY_ID.to_owned(),
                kind: SkillDirectoryKind::Smithery,
                name: "Smithery".to_owned(),
                url: "https://smithery.ai/skills".to_owned(),
                removable: false,
            },
            SkillDirectorySource {
                id: ANTHROPIC_ID.to_owned(),
                kind: SkillDirectoryKind::Anthropic,
                name: "Anthropic".to_owned(),
                url: config.anthropic_url,
                removable: false,
            },
        ];
        sources.extend(self.user_sources().await?);
        Ok(sources)
    }

    async fn user_sources(&self) -> Result<Vec<SkillDirectorySource>> {
        let raw = self
            .store
            .read(|conn| meta_get(conn, SOURCES_META_KEY))
            .await?;
        Ok(raw
            .and_then(|r| serde_json::from_str(&r).ok())
            .unwrap_or_default())
    }

    async fn save_user_sources(&self, sources: Vec<SkillDirectorySource>) -> Result<()> {
        let json = serde_json::to_string(&sources)?;
        self.store
            .write(move |tx| tx.meta_set(SOURCES_META_KEY, &json))
            .await
    }

    pub(crate) async fn skill_directory_sources(
        &self,
        _caller: &Caller,
        _request: SkillDirectorySources,
    ) -> Result<SkillDirectorySourcesResult> {
        Ok(SkillDirectorySourcesResult {
            sources: self.directory_sources().await?,
        })
    }

    pub(crate) async fn skill_directory_add_source(
        &self,
        _caller: &Caller,
        request: SkillDirectoryAddSource,
    ) -> Result<SkillDirectorySourcesResult> {
        let options = self.skill_sources.git_options();
        let (id, location) = locate(&request.url, options)?;
        let existing = self.directory_sources().await?;
        if existing
            .iter()
            .any(|s| s.id == id || same_repo(&s.url, &id))
        {
            return Ok(SkillDirectorySourcesResult { sources: existing });
        }
        // Read it once: the user learns now when it does not work.
        let (name, entries) = self
            .list_location(&id, &location)
            .await
            .map_err(|message| {
                HubError::invalid(format!(
                    "The hub could not read {}: {}. Check the URL, then try again.",
                    request.url.trim(),
                    message.trim_end_matches('.')
                ))
            })?;
        if entries.is_empty() {
            return Err(HubError::invalid(format!(
                "{} has no skills that the hub can list. Add a repo with SKILL.md files or a marketplace.json with skills.",
                request.url.trim()
            )));
        }
        let source = SkillDirectorySource {
            id: id.clone(),
            kind: SkillDirectoryKind::Marketplace,
            name,
            url: id.clone(),
            removable: true,
        };
        let mut users = self.user_sources().await?;
        users.push(source);
        self.save_user_sources(users).await?;
        let mut listings = self.skill_sources.listings.lock().await;
        listings.insert(
            id,
            Cached {
                ok: Some((Instant::now(), entries)),
                failed: None,
            },
        );
        drop(listings);
        Ok(SkillDirectorySourcesResult {
            sources: self.directory_sources().await?,
        })
    }

    pub(crate) async fn skill_directory_remove_source(
        &self,
        _caller: &Caller,
        request: SkillDirectoryRemoveSource,
    ) -> Result<SkillDirectorySourcesResult> {
        let id = request.id.trim().to_owned();
        if id == SMITHERY_ID || id == ANTHROPIC_ID {
            return Err(HubError::invalid(format!(
                "The {id} directory is built in and cannot be removed."
            )));
        }
        let mut users = self.user_sources().await?;
        let before = users.len();
        users.retain(|s| s.id != id);
        if users.len() == before {
            return Err(HubError::not_found(format!(
                "There is no skill directory {id}."
            )));
        }
        self.save_user_sources(users).await?;
        self.skill_sources.listings.lock().await.remove(&id);
        Ok(SkillDirectorySourcesResult {
            sources: self.directory_sources().await?,
        })
    }

    // Listings ---------------------------------------------------------------

    /// Lists a git repo: the skills of its marketplace file, or every
    /// `SKILL.md` when it has none. Returns the marketplace name too.
    async fn list_repo(
        &self,
        source_id: &str,
        url: &str,
        git_ref: Option<&str>,
        marketplace_path: &str,
        kind: SkillDirectoryKind,
    ) -> std::result::Result<(Option<String>, Vec<SkillDirectoryEntry>), String> {
        let sources = &self.skill_sources;
        let repo = Repo::fetch(sources.tmp_root(), url, git_ref, sources.git_options())
            .await
            .map_err(|e| e.to_error_body().message)?;
        let items = repo
            .tree(None)
            .await
            .map_err(|e| e.to_error_body().message)?;
        let root = marketplace_path
            .strip_suffix(MARKETPLACE_PATH)
            .unwrap_or("")
            .trim_end_matches('/')
            .to_owned();
        let marketplace_item = items.iter().find(|i| i.path == marketplace_path).cloned();
        let manifests: HashMap<&str, &TreeItem> = items
            .iter()
            .filter_map(|i| skill_folder_of(&i.path).map(|folder| (folder, i)))
            .collect();
        let mut found: Vec<Found> = Vec::new();
        let mut name = None;
        if let Some(item) = marketplace_item {
            let read = repo
                .read_small_files(std::slice::from_ref(&item), JSON_MAX_BYTES as u64)
                .await
                .map_err(|e| e.to_error_body().message)?;
            let doc: Value = read
                .get(&item.path)
                .and_then(|b| serde_json::from_slice(b).ok())
                .ok_or_else(|| "its marketplace.json is not JSON".to_owned())?;
            name = text(&doc, "name");
            for plugin in marketplace_plugins(&doc, &root, sources.git_options()) {
                for folder in &plugin.folders {
                    let mut push = |path: String| {
                        found.push(Found {
                            path,
                            repo_url: plugin.repo_url.clone(),
                            author: plugin.author.clone(),
                            license: plugin.license.clone(),
                            fallback_description: plugin.description.clone(),
                        });
                    };
                    if plugin.listed_only || manifests.contains_key(folder.as_str()) {
                        push(folder.clone());
                    } else {
                        // A folder of skill folders: one level down.
                        let prefix = if folder.is_empty() {
                            String::new()
                        } else {
                            format!("{folder}/")
                        };
                        for path in manifests.keys() {
                            if let Some(rest) = path.strip_prefix(prefix.as_str())
                                && !rest.is_empty()
                                && !rest.contains('/')
                            {
                                push((*path).to_owned());
                            }
                        }
                    }
                }
            }
        } else {
            for path in manifests.keys() {
                found.push(Found {
                    path: (*path).to_owned(),
                    repo_url: None,
                    author: None,
                    license: None,
                    fallback_description: String::new(),
                });
            }
        }
        found.sort_by(|a, b| a.path.cmp(&b.path));
        found.dedup_by(|a, b| a.path == b.path && a.repo_url == b.repo_url);
        found.truncate(LISTING_MAX_SKILLS);
        // Read the SKILL.md of every skill in this repo.
        let local: Vec<TreeItem> = found
            .iter()
            .filter(|f| f.repo_url.is_none())
            .filter_map(|f| manifests.get(f.path.as_str()).map(|i| (*i).clone()))
            .collect();
        let texts = repo
            .read_small_files(&local, MANIFEST_READ_MAX_BYTES)
            .await
            .map_err(|e| e.to_error_body().message)?;
        let prefix = entry_prefix(source_id);
        let owner = url
            .trim_end_matches('/')
            .rsplit('/')
            .nth(1)
            .map(str::to_owned);
        let mut entries = Vec::with_capacity(found.len());
        for f in found {
            let manifest_path = if f.path.is_empty() {
                SKILL_MANIFEST_FILE.to_owned()
            } else {
                format!("{}/{SKILL_MANIFEST_FILE}", f.path)
            };
            let (skill_name, description, license) = match texts.get(&manifest_path) {
                Some(bytes) => front_matter(&String::from_utf8_lossy(bytes)),
                None if f.repo_url.is_none() => continue,
                None => (None, None, None),
            };
            let folder_name = if f.path.is_empty() {
                url.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("skill")
            } else {
                last_segment(&f.path)
            };
            let key = match &f.repo_url {
                None if f.path.is_empty() => ".".to_owned(),
                None => f.path.clone(),
                Some(repo) => format!("{repo}#{}", f.path),
            };
            let repo_url = f.repo_url.clone().unwrap_or_else(|| url.to_owned());
            entries.push(SkillDirectoryEntry {
                entry_id: format!("{prefix}:{key}"),
                directory: kind,
                source_id: Some(source_id.to_owned()),
                name: skill_name.unwrap_or_else(|| folder_name.to_owned()),
                description: description.unwrap_or(f.fallback_description),
                author: f.author.or_else(|| owner.clone()),
                url: Some(repo_page_url(&repo_url, &f.path)),
                repo_url: Some(repo_url),
                path: (!f.path.is_empty()).then_some(f.path),
                license: license.or(f.license),
                installs: None,
                audits: Vec::new(),
                installed_skill_id: None,
            });
        }
        Ok((name, entries))
    }

    /// Lists a `marketplace.json` over HTTP: only plugins in other repos.
    async fn list_http_marketplace(
        &self,
        source_id: &str,
        url: &str,
    ) -> std::result::Result<(Option<String>, Vec<SkillDirectoryEntry>), String> {
        let doc = self.skill_sources.get_json(url).await?;
        let options = self.skill_sources.git_options();
        let prefix = entry_prefix(source_id);
        let mut entries = Vec::new();
        for plugin in marketplace_plugins(&doc, "", options) {
            let Some(repo_url) = plugin.repo_url.clone() else {
                continue;
            };
            for folder in &plugin.folders {
                entries.push(SkillDirectoryEntry {
                    entry_id: format!("{prefix}:{repo_url}#{folder}"),
                    directory: SkillDirectoryKind::Marketplace,
                    source_id: Some(source_id.to_owned()),
                    name: if folder.is_empty() {
                        plugin.plugin.clone()
                    } else {
                        last_segment(folder).to_owned()
                    },
                    description: plugin.description.clone(),
                    author: plugin.author.clone(),
                    url: Some(repo_page_url(&repo_url, folder)),
                    repo_url: Some(repo_url.clone()),
                    path: (!folder.is_empty()).then(|| folder.clone()),
                    license: plugin.license.clone(),
                    installs: None,
                    audits: Vec::new(),
                    installed_skill_id: None,
                });
            }
        }
        entries.truncate(LISTING_MAX_SKILLS);
        Ok((text(&doc, "name"), entries))
    }

    /// Lists one user source. Returns its name and entries.
    async fn list_location(
        &self,
        source_id: &str,
        location: &Location,
    ) -> std::result::Result<(String, Vec<SkillDirectoryEntry>), String> {
        let (name, entries) = match location {
            Location::Git {
                url,
                git_ref,
                marketplace,
            } => {
                self.list_repo(
                    source_id,
                    url,
                    git_ref.as_deref(),
                    marketplace,
                    SkillDirectoryKind::Marketplace,
                )
                .await?
            }
            Location::Http { url } => self.list_http_marketplace(source_id, url).await?,
        };
        let fallback = source_id
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .rsplit('/')
            .find(|s| !s.is_empty() && !s.ends_with(".json") && *s != ".claude-plugin")
            .unwrap_or(source_id)
            .to_owned();
        Ok((name.unwrap_or(fallback), entries))
    }

    /// The listing of a repo source, from the cache when it is fresh. A
    /// failed fetch gives the old copy when there is one.
    async fn listing(
        &self,
        source: &SkillDirectorySource,
    ) -> std::result::Result<Vec<SkillDirectoryEntry>, String> {
        let mut listings = self.skill_sources.listings.lock().await;
        let cached = listings.entry(source.id.clone()).or_default();
        if let Some((at, entries)) = &cached.ok
            && at.elapsed() < LISTING_TTL
        {
            return Ok(entries.clone());
        }
        if let Some((at, message)) = &cached.failed
            && at.elapsed() < RETRY_AFTER_FAILURE
        {
            return cached
                .ok
                .as_ref()
                .map(|(_, e)| e.clone())
                .ok_or_else(|| message.clone());
        }
        drop(listings);
        let fetched = match source.kind {
            SkillDirectoryKind::Anthropic => self
                .list_repo(
                    &source.id,
                    &source.url,
                    None,
                    MARKETPLACE_PATH,
                    SkillDirectoryKind::Anthropic,
                )
                .await
                .map(|(_, e)| e),
            _ => match locate(&source.url, self.skill_sources.git_options()) {
                Ok((_, location)) => self
                    .list_location(&source.id, &location)
                    .await
                    .map(|(_, e)| e),
                Err(err) => Err(err.to_error_body().message),
            },
        };
        let mut listings = self.skill_sources.listings.lock().await;
        let cached = listings.entry(source.id.clone()).or_default();
        match fetched {
            Ok(entries) => {
                cached.ok = Some((Instant::now(), entries.clone()));
                cached.failed = None;
                Ok(entries)
            }
            Err(message) => {
                tracing::warn!(source = %source.id, error = %message, "a skill directory did not answer");
                cached.failed = Some((Instant::now(), message.clone()));
                cached.ok.as_ref().map(|(_, e)| e.clone()).ok_or(message)
            }
        }
    }

    /// One Smithery search, from the cache when it is fresh.
    async fn smithery_search(
        &self,
        search: &str,
    ) -> std::result::Result<Vec<SkillDirectoryEntry>, String> {
        let key = search.trim().to_lowercase();
        {
            let mut searches = self.skill_sources.searches.lock().await;
            let cached = searches.entry(key.clone()).or_default();
            if let Some((at, entries)) = &cached.ok
                && at.elapsed() < SEARCH_TTL
            {
                return Ok(entries.clone());
            }
            if let Some((at, message)) = &cached.failed
                && at.elapsed() < RETRY_AFTER_FAILURE
            {
                return cached
                    .ok
                    .as_ref()
                    .map(|(_, e)| e.clone())
                    .ok_or_else(|| message.clone());
            }
        }
        let config = self.skill_sources.config();
        let page_size = SMITHERY_PAGE_SIZE.to_string();
        let mut pairs = vec![("pageSize", page_size.as_str())];
        if !key.is_empty() {
            pairs.push(("q", key.as_str()));
        }
        let url = format!("{}/skills?{}", config.smithery_url, query(&pairs));
        let options = self.skill_sources.git_options();
        let fetched = self.skill_sources.get_json(&url).await.and_then(|doc| {
            doc.get("skills")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|i| smithery_entry(i, options))
                        .collect::<Vec<_>>()
                })
                .ok_or_else(|| "its answer has no skills".to_owned())
        });
        let mut searches = self.skill_sources.searches.lock().await;
        if searches.len() >= SEARCHES_KEPT {
            searches.retain(|_, c| {
                c.ok.as_ref()
                    .is_some_and(|(at, _)| at.elapsed() < SEARCH_TTL)
            });
        }
        let cached = searches.entry(key).or_default();
        match fetched {
            Ok(entries) => {
                cached.ok = Some((Instant::now(), entries.clone()));
                cached.failed = None;
                Ok(entries)
            }
            Err(message) => {
                tracing::warn!(error = %message, "Smithery did not answer a skill search");
                cached.failed = Some((Instant::now(), message.clone()));
                cached.ok.as_ref().map(|(_, e)| e.clone()).ok_or(message)
            }
        }
    }

    /// Searches every source, or one. Returns the entries (local sources
    /// first, then Smithery) and the names of the sources that failed.
    pub(crate) async fn search_directories(
        &self,
        search: Option<&str>,
        source: Option<&str>,
    ) -> Result<(Vec<SkillDirectoryEntry>, Vec<String>)> {
        let sources = self.directory_sources().await?;
        let selected: Vec<SkillDirectorySource> =
            match source.map(str::trim).filter(|s| !s.is_empty()) {
                Some(id) => {
                    let found = sources
                        .iter()
                        .find(|s| s.id == id)
                        .cloned()
                        .ok_or_else(|| {
                            let ids: Vec<&str> = sources.iter().map(|s| s.id.as_str()).collect();
                            HubError::not_found(format!(
                                "There is no skill directory {id}. Use one of these: {}.",
                                ids.join(", ")
                            ))
                        })?;
                    vec![found]
                }
                None => sources,
            };
        let words: Vec<String> = search
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let mut local = Vec::new();
        let mut remote = Vec::new();
        let mut unavailable = Vec::new();
        for source in selected {
            if source.kind == SkillDirectoryKind::Smithery {
                match self.smithery_search(&words.join(" ")).await {
                    Ok(entries) => remote.extend(entries),
                    Err(_) => unavailable.push(source.name.clone()),
                }
                continue;
            }
            match self.listing(&source).await {
                Ok(entries) => local.extend(entries.into_iter().filter(|e| {
                    let hay = format!(
                        "{} {} {} {}",
                        e.name,
                        e.description,
                        e.author.as_deref().unwrap_or(""),
                        e.path.as_deref().unwrap_or("")
                    )
                    .to_lowercase();
                    words.iter().all(|w| hay.contains(w.as_str()))
                })),
                Err(_) => unavailable.push(source.name.clone()),
            }
        }
        local.extend(remote);
        let skills = self.store.read(skill_store::skills).await?;
        for entry in &mut local {
            entry.installed_skill_id = installed_from(&skills, entry);
        }
        Ok((local, unavailable))
    }

    pub(crate) async fn skill_directory_search(
        &self,
        _caller: &Caller,
        request: SkillDirectorySearch,
    ) -> Result<SkillDirectorySearchResult> {
        let (entries, unavailable) = self
            .search_directories(request.search.as_deref(), request.source.as_deref())
            .await?;
        let offset: usize = match request.cursor.as_deref() {
            None | Some("") => 0,
            Some(c) => c
                .parse()
                .map_err(|_| HubError::invalid("The cursor is not one that this hub gave."))?,
        };
        let limit = request
            .limit
            .unwrap_or(PAGE_DEFAULT)
            .clamp(1, SKILL_DIRECTORY_PAGE_MAX) as usize;
        let page: Vec<SkillDirectoryEntry> =
            entries.iter().skip(offset).take(limit).cloned().collect();
        let next = offset + page.len();
        Ok(SkillDirectorySearchResult {
            entries: page,
            next_cursor: (next < entries.len()).then(|| next.to_string()),
            unavailable,
        })
    }

    /// One directory entry by its ID. A Smithery entry is read from
    /// Smithery; any other from its source's listing.
    pub(crate) async fn directory_entry(
        &self,
        source: Option<&str>,
        entry_id: &str,
    ) -> Result<SkillDirectoryEntry> {
        let entry_id = entry_id.trim();
        let not_found = || {
            HubError::not_found(format!(
                "There is no skill {entry_id} in the skill directories. Search again."
            ))
        };
        let (prefix, rest) = entry_id.split_once(':').ok_or_else(not_found)?;
        if prefix == SMITHERY_ID {
            let (namespace, slug) = rest.split_once('/').ok_or_else(not_found)?;
            if !is_smithery_name(namespace) || !is_smithery_name(slug) {
                return Err(not_found());
            }
            let config = self.skill_sources.config();
            let url = format!("{}/skills/{namespace}/{slug}", config.smithery_url);
            let doc = self.skill_sources.get_json(&url).await.map_err(|message| {
                HubError::not_found(format!(
                    "Smithery did not give the skill {namespace}/{slug}: {}.",
                    message.trim_end_matches('.')
                ))
            })?;
            return smithery_entry(&doc, self.skill_sources.git_options()).ok_or_else(|| {
                HubError::not_found(format!(
                    "The Smithery skill {namespace}/{slug} has no git repo, so the hub cannot install it."
                ))
            });
        }
        let sources = self.directory_sources().await?;
        let source = match source.map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => sources.into_iter().find(|s| s.id == id),
            None => sources.into_iter().find(|s| entry_prefix(&s.id) == prefix),
        }
        .ok_or_else(not_found)?;
        let entries = self.listing(&source).await.map_err(|message| {
            HubError::not_found(format!(
                "The skill directory {} did not answer: {}. Try again later.",
                source.name,
                message.trim_end_matches('.')
            ))
        })?;
        entries
            .into_iter()
            .find(|e| e.entry_id == entry_id)
            .ok_or_else(not_found)
    }
}

/// The GitHub page of a folder, or the repo URL for another host.
fn repo_page_url(repo: &str, path: &str) -> String {
    if repo.starts_with("https://github.com/") && !path.is_empty() {
        format!("{}/tree/HEAD/{path}", repo.trim_end_matches('/'))
    } else {
        repo.to_owned()
    }
}

/// The installed skill that came from this entry: the same entry ID, or the
/// same repo and folder.
fn installed_from(skills: &[Skill], entry: &SkillDirectoryEntry) -> Option<SkillId> {
    skills
        .iter()
        .find(|s| match &s.source {
            SkillSource::Directory { entry_id, .. } if *entry_id == entry.entry_id => true,
            SkillSource::Directory { url, path, .. } | SkillSource::Git { url, path, .. } => {
                entry.repo_url.as_deref().is_some_and(|r| same_repo(r, url))
                    && path.as_deref().unwrap_or("") == entry.path.as_deref().unwrap_or("")
            }
            SkillSource::Upload { .. } => false,
        })
        .map(|s| s.id)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const LOCAL: GitOptions = GitOptions { allow_local: true };

    #[test]
    fn entry_ids_are_told_apart_from_git_urls() {
        assert!(looks_like_entry_id("smithery:openclaw/nano-pdf"));
        assert!(looks_like_entry_id("anthropic:skills/pdf"));
        let prefix = entry_prefix("https://example.com/marketplace.json");
        assert!(looks_like_entry_id(&format!("{prefix}:skills/x")));
        assert!(!looks_like_entry_id("https://github.com/a/b"));
        assert!(!looks_like_entry_id("anthropics/skills"));
        assert!(!looks_like_entry_id("file:///tmp/x"));
    }

    #[test]
    fn smithery_results_become_entries() {
        let item = json!({
            "namespace": "openclaw", "slug": "nano-pdf", "displayName": "nano-pdf",
            "description": "Edit PDFs.", "gitUrl": "https://github.com/openclaw/openclaw/tree/main/skills/nano-pdf",
            "totalActivations": 6, "verified": false
        });
        let entry = smithery_entry(&item, LOCAL).unwrap();
        assert_eq!(entry.entry_id, "smithery:openclaw/nano-pdf");
        assert_eq!(
            entry.repo_url.as_deref(),
            Some("https://github.com/openclaw/openclaw")
        );
        assert_eq!(entry.path.as_deref(), Some("skills/nano-pdf"));
        assert_eq!(entry.installs, Some(6));
        assert!(entry.audits.is_empty(), "Smithery gives no audits");
        let no_git = json!({"namespace": "a", "slug": "b", "gitUrl": null});
        assert!(smithery_entry(&no_git, LOCAL).is_none());
        assert_eq!(query(&[("q", "pdf forms&x")]), "q=pdf%20forms%26x");
    }

    #[test]
    fn marketplace_plugins_name_their_skill_folders() {
        let doc = json!({
            "name": "example",
            "metadata": {"pluginRoot": "./plugins"},
            "plugins": [
                {"name": "docs", "source": "./", "skills": ["./skills/pdf", "./skills/xlsx"], "description": "Docs."},
                {"name": "bare", "source": "tidy"},
                {"name": "remote", "source": {"source": "github", "repo": "acme/tools"}, "skills": ["./skills/lint"]},
                {"name": "remote-unlisted", "source": {"source": "url", "url": "https://git.example.com/x.git"}},
                {"name": "npm", "source": {"source": "npm", "package": "x"}, "skills": ["a"]},
                {"name": "escape", "source": "./../outside"}
            ]
        });
        let plugins = marketplace_plugins(&doc, "", LOCAL);
        let names: Vec<&str> = plugins.iter().map(|p| p.plugin.as_str()).collect();
        assert_eq!(names, vec!["docs", "bare", "remote"]);
        assert_eq!(
            plugins[0].folders,
            vec!["skills/pdf", "skills/xlsx", "skills"]
        );
        assert_eq!(plugins[1].folders, vec!["plugins/tidy/skills"]);
        assert_eq!(
            plugins[2].repo_url.as_deref(),
            Some("https://github.com/acme/tools")
        );
        assert_eq!(plugins[2].folders, vec!["skills/lint"]);
        assert!(plugins[2].listed_only);
    }

    #[test]
    fn sources_are_located() {
        let (id, loc) = locate("acme/skills", LOCAL).unwrap();
        assert_eq!(id, "https://github.com/acme/skills");
        assert!(
            matches!(loc, Location::Git { ref marketplace, .. } if marketplace == MARKETPLACE_PATH)
        );
        let (_, loc) = locate(
            "https://raw.githubusercontent.com/acme/skills/main/.claude-plugin/marketplace.json",
            GitOptions { allow_local: false },
        )
        .unwrap();
        assert_eq!(
            loc,
            Location::Git {
                url: "https://github.com/acme/skills".to_owned(),
                git_ref: Some("main".to_owned()),
                marketplace: MARKETPLACE_PATH.to_owned(),
            }
        );
        assert!(matches!(
            locate(
                "https://example.com/m.json",
                GitOptions { allow_local: false }
            )
            .unwrap()
            .1,
            Location::Http { .. }
        ));
        assert!(
            locate(
                "http://example.com/m.json",
                GitOptions { allow_local: false }
            )
            .is_err()
        );
    }
}
