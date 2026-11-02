//! The tool store: install, update, status and launch.
//!
//! Layout under the root (`<consort home>/tools`):
//!
//! ```text
//! state.json                   last update check
//! .npm-cache/                  npm cache for npm based tools
//! <tool>/current               the ready version (text)
//! <tool>/<version>/            one installed version
//! <tool>/.tmp-<random>/        an install in progress
//! ```
//!
//! An install unpacks into `.tmp-<random>`, verifies it, renames it to
//! `<version>`, and then writes `current`. A failed install removes its
//! temporary files. The store keeps the previous version and removes older
//! ones.

mod archive;
mod chrome;
mod http;
mod signature;
mod sources;
mod state;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{CLAUDE_DISABLE_AUTOUPDATER, OPENCODE_DISABLE_AUTOUPDATE};
use crate::registry::{BinaryTarget, PackageDistribution, Registry};
use crate::{Installed, LaunchSpec, Platform, ToolError, ToolId, ToolState, ToolStatus};

pub use chrome::{ChromeFile, ChromePin};
pub use sources::Sources;

use http::Http;
use state::{StoreState, ToolCheck, is_newer, now_secs, valid_version};

/// How long the store reuses a fetched ACP registry.
const REGISTRY_TTL: Duration = Duration::from_secs(300);

/// The file in a version folder that holds the launch command of a tool
/// from the ACP registry.
const LAUNCH_RECORD: &str = ".consort-launch.json";

/// The npm packages of the npm based tools.
const CLAUDE_AGENT_ACP_PACKAGE: &str = "@agentclientprotocol/claude-agent-acp";
const GEMINI_CLI_PACKAGE: &str = "@google/gemini-cli";

/// The registry entry that Consort uses for OpenCode.
const OPENCODE_REGISTRY_ID: &str = "opencode-v2";

/// Settings for a [`ToolStore`].
#[derive(Debug, Clone)]
pub struct ToolStoreOptions {
    /// The platform to install builds for.
    pub platform: Platform,
    /// Where to download from.
    pub sources: Sources,
    /// A program to run in place of the managed npm. Tests use it. When it is
    /// `None`, the store runs `node npm-cli.js` from the managed Node.
    pub npm_program: Option<PathBuf>,
    /// The armored OpenPGP public key that signs the Claude Code manifest.
    pub claude_signing_key: String,
    /// The pinned version, sizes and hashes of the preview browser. Tests
    /// replace it with the files of a local server.
    pub chrome: ChromePin,
}

impl ToolStoreOptions {
    /// The options for this machine: the detected platform, the public
    /// sources, the embedded Claude Code key and the pinned preview
    /// browser.
    pub fn detect() -> Result<Self, ToolError> {
        let platform = Platform::current().ok_or_else(|| ToolError::UnknownPlatform {
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
        })?;
        Ok(ToolStoreOptions {
            platform,
            sources: Sources::default(),
            npm_program: None,
            claude_signing_key: signature::CLAUDE_CODE_KEY.to_owned(),
            chrome: ChromePin::pinned(),
        })
    }
}

/// Installs, updates and launches managed tools under one root folder.
///
/// The store is safe to share between tasks. Only one install of a tool runs
/// at a time. A second call for the same tool waits, and then finds the
/// version that the first call installed.
pub struct ToolStore {
    root: PathBuf,
    options: ToolStoreOptions,
    http: Http,
    locks: Mutex<HashMap<ToolId, Arc<tokio::sync::Mutex<()>>>>,
    busy: Mutex<HashMap<ToolId, usize>>,
    install_errors: Mutex<HashMap<ToolId, String>>,
    check_errors: Mutex<HashMap<ToolId, String>>,
    state: Mutex<StoreState>,
    registry_cache: tokio::sync::Mutex<Option<(Instant, Arc<Registry>)>>,
}

impl std::fmt::Debug for ToolStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolStore")
            .field("root", &self.root)
            .field("platform", &self.options.platform)
            .finish_non_exhaustive()
    }
}

/// Locks a std mutex. A panic in another thread does not make the data
/// invalid here, so a poisoned lock is used as is.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How to install an agent from the ACP registry.
enum AcpPlan {
    Binary {
        registry_version: String,
        target: BinaryTarget,
    },
    Npx {
        package: PackageDistribution,
    },
}

/// The saved launch command of a registry tool. `program` is relative to the
/// version folder, with `/` separators.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LaunchRecord {
    program: String,
    /// Run `program` with the managed Node.
    #[serde(default)]
    node: bool,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// Temporary paths of one install. Dropping it removes them, and removes the
/// tool folder when it is empty.
struct Scratch {
    tool_dir: PathBuf,
    staging: PathBuf,
    download: PathBuf,
}

impl Scratch {
    fn new(tool_dir: &Path) -> Result<Scratch, ToolError> {
        fs::create_dir_all(tool_dir).map_err(ToolError::io(format!(
            "cannot create {}",
            tool_dir.display()
        )))?;
        let tag = format!("{:016x}", rand::random::<u64>());
        let staging = tool_dir.join(format!(".tmp-{tag}"));
        fs::create_dir(&staging).map_err(ToolError::io(format!(
            "cannot create {}",
            staging.display()
        )))?;
        Ok(Scratch {
            tool_dir: tool_dir.to_owned(),
            staging,
            download: tool_dir.join(format!(".tmp-{tag}.download")),
        })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // After a good install, `staging` was renamed and `download` was
        // moved or unpacked, so these calls fail without harm.
        let _ = fs::remove_dir_all(&self.staging);
        let _ = fs::remove_file(&self.download);
        // This fails unless the folder is empty.
        let _ = fs::remove_dir(&self.tool_dir);
    }
}

/// Marks a tool as installing while it lives.
struct BusyGuard<'a> {
    busy: &'a Mutex<HashMap<ToolId, usize>>,
    tool: ToolId,
}

impl<'a> BusyGuard<'a> {
    fn new(busy: &'a Mutex<HashMap<ToolId, usize>>, tool: &ToolId) -> Self {
        *lock(busy).entry(tool.clone()).or_insert(0) += 1;
        BusyGuard {
            busy,
            tool: tool.clone(),
        }
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        let mut busy = lock(self.busy);
        if let Some(count) = busy.get_mut(&self.tool) {
            *count -= 1;
            if *count == 0 {
                busy.remove(&self.tool);
            }
        }
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl ToolStore {
    /// Opens the store at `root` (normally `<consort home>/tools`) for this
    /// machine. The folder is created when it does not exist.
    pub fn new(root: PathBuf) -> Result<Self, ToolError> {
        Self::with_options(root, ToolStoreOptions::detect()?)
    }

    /// Opens the store with explicit options.
    pub fn with_options(root: PathBuf, options: ToolStoreOptions) -> Result<Self, ToolError> {
        fs::create_dir_all(&root)
            .map_err(ToolError::io(format!("cannot create {}", root.display())))?;
        let state = StoreState::load(&root.join("state.json"));
        Ok(ToolStore {
            root,
            options,
            http: Http::new()?,
            locks: Mutex::new(HashMap::new()),
            busy: Mutex::new(HashMap::new()),
            install_errors: Mutex::new(HashMap::new()),
            check_errors: Mutex::new(HashMap::new()),
            state: Mutex::new(state),
            registry_cache: tokio::sync::Mutex::new(None),
        })
    }

    /// The root folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The platform that the store installs builds for.
    pub fn platform(&self) -> Platform {
        self.options.platform
    }

    /// The time of the last [`ToolStore::check_updates`], also from an
    /// earlier run of the worker.
    pub fn last_check(&self) -> Option<SystemTime> {
        lock(&self.state)
            .last_check
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
    }

    /// The ready version of a tool: the version that `current` names.
    pub fn installed(&self, tool: &ToolId) -> Option<Installed> {
        let tool_dir = self.root.join(tool.dir_name());
        let version = fs::read_to_string(tool_dir.join("current")).ok()?;
        let version = version.trim();
        if !valid_version(version) {
            return None;
        }
        let dir = tool_dir.join(version);
        dir.is_dir().then(|| Installed {
            tool: tool.clone(),
            version: version.to_owned(),
            dir,
        })
    }

    fn require(&self, tool: &ToolId) -> Result<Installed, ToolError> {
        self.installed(tool)
            .ok_or_else(|| ToolError::NotInstalled { tool: tool.clone() })
    }

    /// The newest version that the tool's source offers now.
    pub async fn latest(&self, tool: &ToolId) -> Result<String, ToolError> {
        let version = match tool {
            ToolId::Node => self.node_latest().await?,
            ToolId::ClaudeCode => self.claude_latest().await?,
            ToolId::ClaudeAgentAcp => self.npm_latest(CLAUDE_AGENT_ACP_PACKAGE).await?,
            ToolId::Codex => self.codex_latest().await?,
            ToolId::OpenCode => self.opencode_latest().await?,
            ToolId::GeminiCli => self.npm_latest(GEMINI_CLI_PACKAGE).await?,
            // The pinned version. An update check makes no request for it.
            ToolId::ChromeHeadlessShell => self.chrome_latest(),
            ToolId::Acp(id) => {
                let registry = self.registry().await?;
                let agent = registry
                    .agent(id)
                    .ok_or_else(|| ToolError::UnknownAgent { id: id.clone() })?;
                agent.version.clone()
            }
        };
        if valid_version(&version) {
            Ok(version)
        } else {
            Err(ToolError::InvalidResponse {
                url: tool.to_string(),
                message: format!("\"{version}\" is not a valid version"),
            })
        }
    }

    /// Installs a version of a tool (the latest when `version` is `None`)
    /// and makes it the ready version. Missing dependencies are installed
    /// first. Installing a version that is already on disk only makes it
    /// the ready version.
    pub async fn install(
        &self,
        tool: &ToolId,
        version: Option<&str>,
    ) -> Result<Installed, ToolError> {
        let _busy = BusyGuard::new(&self.busy, tool);
        let result = self.install_inner(tool, version).await;
        let mut errors = lock(&self.install_errors);
        match &result {
            Ok(_) => {
                errors.remove(tool);
            }
            Err(err) => {
                errors.insert(tool.clone(), err.to_string());
            }
        }
        result
    }

    fn install_inner<'a>(
        &'a self,
        tool: &'a ToolId,
        version: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Installed, ToolError>> {
        Box::pin(async move {
            let acp_plan = match tool {
                ToolId::Acp(id) => Some(self.acp_plan(id).await?),
                _ => None,
            };
            let mut dependencies = tool.dependencies().to_vec();
            if matches!(acp_plan, Some(AcpPlan::Npx { .. })) {
                dependencies.push(ToolId::Node);
            }
            for dependency in &dependencies {
                if self.installed(dependency).is_none() {
                    self.install(dependency, None).await?;
                }
            }

            let tool_lock = {
                let mut locks = lock(&self.locks);
                locks.entry(tool.clone()).or_default().clone()
            };
            let _guard = tool_lock.lock().await;

            let version = match version {
                Some(version) => version.to_owned(),
                None => self.latest(tool).await?,
            };
            if !valid_version(&version) {
                return Err(ToolError::Unsupported {
                    tool: tool.clone(),
                    reason: format!("\"{version}\" is not a valid version"),
                });
            }
            let tool_dir = self.root.join(tool.dir_name());
            let target = tool_dir.join(&version);
            let previous = self.installed(tool).map(|installed| installed.version);

            if !target.is_dir() {
                let scratch = Scratch::new(&tool_dir)?;
                self.stage(tool, &version, acp_plan, &scratch).await?;
                fs::rename(&scratch.staging, &target).map_err(ToolError::io(format!(
                    "cannot move the install to {}",
                    target.display()
                )))?;
            }
            write_current(&tool_dir, &version)?;
            prune(&tool_dir, &version, previous.as_deref());
            Ok(Installed {
                tool: tool.clone(),
                version,
                dir: target,
            })
        })
    }

    /// Downloads, verifies and unpacks one version into `scratch.staging`.
    async fn stage(
        &self,
        tool: &ToolId,
        version: &str,
        acp_plan: Option<AcpPlan>,
        scratch: &Scratch,
    ) -> Result<(), ToolError> {
        match tool {
            ToolId::Node => self.stage_node(version, scratch).await,
            ToolId::ClaudeCode => self.stage_claude(version, scratch).await,
            ToolId::ClaudeAgentAcp => {
                self.stage_npm_tool(tool, CLAUDE_AGENT_ACP_PACKAGE, version, scratch)
                    .await
            }
            ToolId::GeminiCli => {
                self.stage_npm_tool(tool, GEMINI_CLI_PACKAGE, version, scratch)
                    .await
            }
            ToolId::Codex => self.stage_codex(version, scratch).await,
            ToolId::ChromeHeadlessShell => self.stage_chrome_headless_shell(version, scratch).await,
            ToolId::OpenCode => {
                let registry = self.registry().await?;
                let agent = registry.agent(OPENCODE_REGISTRY_ID).ok_or_else(|| {
                    ToolError::UnknownAgent {
                        id: OPENCODE_REGISTRY_ID.to_owned(),
                    }
                })?;
                let target = agent
                    .distribution
                    .binary
                    .as_ref()
                    .and_then(|binary| binary.get(self.options.platform.registry_key()));
                let target = target.ok_or_else(|| ToolError::UnsupportedPlatform {
                    tool: tool.clone(),
                    platform: self.options.platform.to_string(),
                })?;
                self.stage_registry_binary(tool, version, &agent.version, target, scratch)
                    .await
            }
            ToolId::Acp(_) => match acp_plan {
                Some(AcpPlan::Binary {
                    registry_version,
                    target,
                }) => {
                    self.stage_registry_binary(tool, version, &registry_version, &target, scratch)
                        .await
                }
                Some(AcpPlan::Npx { package }) => {
                    self.stage_npx(tool, version, &package, scratch).await
                }
                None => Err(ToolError::Unsupported {
                    tool: tool.clone(),
                    reason: "no install plan".to_owned(),
                }),
            },
        }
    }

    /// Chooses how to install a registry agent: a binary with a SHA-256 for
    /// this platform, else `npx`. A binary without a SHA-256 is refused.
    async fn acp_plan(&self, id: &str) -> Result<AcpPlan, ToolError> {
        let registry = self.registry().await?;
        let agent = registry
            .agent(id)
            .ok_or_else(|| ToolError::UnknownAgent { id: id.to_owned() })?;
        let tool = ToolId::Acp(id.to_owned());
        let distribution = &agent.distribution;
        let binary = distribution
            .binary
            .as_ref()
            .and_then(|binary| binary.get(self.options.platform.registry_key()));
        if let Some(target) = binary.filter(|target| target.sha256.is_some()) {
            return Ok(AcpPlan::Binary {
                registry_version: agent.version.clone(),
                target: target.clone(),
            });
        }
        if let Some(package) = &distribution.npx {
            return Ok(AcpPlan::Npx {
                package: package.clone(),
            });
        }
        if let Some(target) = binary {
            return Err(ToolError::MissingChecksum {
                name: target.archive.clone(),
            });
        }
        if distribution.uvx.is_some() {
            return Err(ToolError::Unsupported {
                tool,
                reason: "uvx packages are not supported yet".to_owned(),
            });
        }
        Err(ToolError::UnsupportedPlatform {
            tool,
            platform: self.options.platform.to_string(),
        })
    }

    /// The ACP registry, fetched at most every few minutes.
    async fn registry(&self) -> Result<Arc<Registry>, ToolError> {
        let mut cache = self.registry_cache.lock().await;
        if let Some((fetched, registry)) = cache.as_ref()
            && fetched.elapsed() < REGISTRY_TTL
        {
            return Ok(registry.clone());
        }
        let url = &self.options.sources.acp_registry;
        let bytes = self.http.bytes(url).await?;
        let registry =
            Arc::new(
                Registry::parse(&bytes).map_err(|err| ToolError::InvalidResponse {
                    url: url.clone(),
                    message: err.to_string(),
                })?,
            );
        *cache = Some((Instant::now(), registry.clone()));
        Ok(registry)
    }

    /// Every tool to report: the built-in tools, and each registry agent
    /// that is on disk or that an install has touched.
    fn known_tools(&self) -> Vec<ToolId> {
        let mut tools: Vec<ToolId> = ToolId::builtin().to_vec();
        let mut agents: Vec<ToolId> = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                if let Some(tool @ ToolId::Acp(_)) =
                    ToolId::from_dir_name(&entry.file_name().to_string_lossy())
                {
                    agents.push(tool);
                }
            }
        }
        agents.extend(lock(&self.busy).keys().cloned());
        agents.extend(lock(&self.install_errors).keys().cloned());
        agents.retain(|tool| matches!(tool, ToolId::Acp(_)));
        agents.sort();
        agents.dedup();
        tools.extend(agents);
        tools
    }

    fn status_of(&self, tool: &ToolId) -> ToolStatus {
        let installed = self.installed(tool);
        let latest = lock(&self.state)
            .tools
            .get(&tool.to_string())
            .map(|check| check.latest.clone());
        let busy = lock(&self.busy).contains_key(tool);
        let install_error = lock(&self.install_errors).get(tool).cloned();
        let check_error = lock(&self.check_errors).get(tool).cloned();

        let state = match (&installed, busy) {
            (_, true) => ToolState::Installing,
            (Some(installed), false) => match &latest {
                Some(latest) if is_newer(latest, &installed.version) => ToolState::UpdateAvailable,
                _ => ToolState::Ready,
            },
            (None, false) if install_error.is_some() => ToolState::Failed,
            (None, false) => ToolState::Missing,
        };
        let detail = match (install_error, check_error) {
            (Some(err), _) if installed.is_some() => Some(format!("The last update failed: {err}")),
            (Some(err), _) => Some(err),
            (None, Some(err)) => Some(format!("The update check failed: {err}")),
            (None, None) => None,
        };
        ToolStatus {
            tool: tool.clone(),
            version: installed.map(|installed| installed.version),
            latest,
            state,
            detail,
        }
    }

    /// The status of every known tool, from disk and the last update check.
    /// This makes no network request.
    pub fn status(&self) -> Vec<ToolStatus> {
        self.known_tools()
            .iter()
            .map(|tool| self.status_of(tool))
            .collect()
    }

    /// Asks every source for its latest version, saves the result in
    /// `state.json`, and returns the new status. A failed check keeps the
    /// earlier result and sets the detail of that tool.
    pub async fn check_updates(&self) -> Vec<ToolStatus> {
        let tools = self.known_tools();
        let mut results = Vec::with_capacity(tools.len());
        for tool in tools {
            let result = self.latest(&tool).await;
            results.push((tool, result));
        }
        let now = now_secs();
        let snapshot = {
            let mut state = lock(&self.state);
            let mut errors = lock(&self.check_errors);
            for (tool, result) in results {
                match result {
                    Ok(latest) => {
                        errors.remove(&tool);
                        state.tools.insert(
                            tool.to_string(),
                            ToolCheck {
                                latest,
                                checked_at: now,
                            },
                        );
                    }
                    Err(err) => {
                        errors.insert(tool, err.to_string());
                    }
                }
            }
            state.last_check = Some(now);
            state.clone()
        };
        if let Err(err) = snapshot.save(&self.root.join("state.json")) {
            tracing::warn!(error = %err, "cannot save the tool update check");
        }
        self.status()
    }

    /// How to start the ready version of a tool. The environment includes the
    /// variables that turn off self-updates.
    pub fn launch(&self, tool: &ToolId) -> Result<LaunchSpec, ToolError> {
        let installed = self.require(tool)?;
        let dir = &installed.dir;
        let platform = &self.options.platform;
        let mut spec = LaunchSpec::default();
        match tool {
            ToolId::Node => {
                spec.program = node_exe(dir, platform);
            }
            ToolId::ClaudeCode => {
                spec.program = dir.join(format!("claude{}", platform.exe_suffix()));
                insert_env(&mut spec, CLAUDE_DISABLE_AUTOUPDATER);
            }
            ToolId::ClaudeAgentAcp => {
                let claude = self.launch(&ToolId::ClaudeCode)?;
                self.node_spec(&mut spec)?;
                spec.args.push(path_arg(&dir.join(
                    "node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
                )));
                spec.env.insert(
                    "CLAUDE_CODE_EXECUTABLE".to_owned(),
                    claude.program.to_string_lossy().into_owned(),
                );
                insert_env(&mut spec, CLAUDE_DISABLE_AUTOUPDATER);
            }
            ToolId::Codex => {
                let package = sources::read_codex_package(dir);
                spec.program = dir.join(package.entrypoint(platform));
                spec.args.push("app-server".to_owned());
                let path_dir = dir.join(&package.path_dir);
                if path_dir.is_dir()
                    && let Some(path) = path_with(&path_dir)
                {
                    spec.env.insert("PATH".to_owned(), path);
                }
            }
            ToolId::GeminiCli => {
                self.node_spec(&mut spec)?;
                spec.args.push(path_arg(
                    &dir.join("node_modules/@google/gemini-cli/bundle/gemini.js"),
                ));
                spec.args.push("--acp".to_owned());
            }
            ToolId::ChromeHeadlessShell => {
                spec.program = dir.join(chrome::program_name(platform));
            }
            ToolId::OpenCode | ToolId::Acp(_) => {
                let record = read_launch_record(dir)?;
                let program =
                    archive::safe_relative(Path::new(&record.program), 0)?.ok_or_else(|| {
                        ToolError::UnsafePath {
                            path: record.program.clone(),
                        }
                    })?;
                if record.node {
                    self.node_spec(&mut spec)?;
                    spec.args.push(path_arg(&dir.join(program)));
                } else {
                    spec.program = dir.join(program);
                }
                spec.args.extend(record.args);
                spec.env.extend(record.env);
                if *tool == ToolId::OpenCode {
                    insert_env(&mut spec, OPENCODE_DISABLE_AUTOUPDATE);
                }
            }
        }
        Ok(spec)
    }

    /// Sets the program to the managed Node, and puts its folder first on
    /// `PATH`, so child processes that run `node` find it.
    fn node_spec(&self, spec: &mut LaunchSpec) -> Result<(), ToolError> {
        let node = self.require(&ToolId::Node)?;
        spec.program = node_exe(&node.dir, &self.options.platform);
        if let Some(path) = path_with(&node_bin_dir(&node.dir, &self.options.platform)) {
            spec.env.insert("PATH".to_owned(), path);
        }
        Ok(())
    }
}

fn insert_env(spec: &mut LaunchSpec, (key, value): (&str, &str)) {
    spec.env.insert(key.to_owned(), value.to_owned());
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The `PATH` of this process with `dir` first.
fn path_with(dir: &Path) -> Option<String> {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&current));
    std::env::join_paths(paths).ok()?.into_string().ok()
}

/// The Node program in a Node install folder.
fn node_exe(dir: &Path, platform: &Platform) -> PathBuf {
    if platform.is_windows() {
        dir.join("node.exe")
    } else {
        dir.join("bin/node")
    }
}

/// The folder with `node` and `npm` in a Node install folder.
fn node_bin_dir(dir: &Path, platform: &Platform) -> PathBuf {
    if platform.is_windows() {
        dir.to_path_buf()
    } else {
        dir.join("bin")
    }
}

/// The npm entry script in a Node install folder.
fn npm_cli(dir: &Path, platform: &Platform) -> PathBuf {
    if platform.is_windows() {
        dir.join("node_modules/npm/bin/npm-cli.js")
    } else {
        dir.join("lib/node_modules/npm/bin/npm-cli.js")
    }
}

fn read_launch_record(dir: &Path) -> Result<LaunchRecord, ToolError> {
    let path = dir.join(LAUNCH_RECORD);
    let bytes =
        fs::read(&path).map_err(ToolError::io(format!("cannot read {}", path.display())))?;
    serde_json::from_slice(&bytes).map_err(|err| ToolError::Io {
        context: format!("cannot read {}", path.display()),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, err),
    })
}

fn write_launch_record(dir: &Path, record: &LaunchRecord) -> Result<(), ToolError> {
    let path = dir.join(LAUNCH_RECORD);
    let json = serde_json::to_vec_pretty(record).map_err(|err| ToolError::Io {
        context: format!("cannot write {}", path.display()),
        source: std::io::Error::other(err),
    })?;
    fs::write(&path, json).map_err(ToolError::io(format!("cannot write {}", path.display())))
}

/// Writes `<tool dir>/current` through a temporary file and a rename.
fn write_current(tool_dir: &Path, version: &str) -> Result<(), ToolError> {
    let tmp = tool_dir.join(format!(".tmp-{:016x}.current", rand::random::<u64>()));
    fs::write(&tmp, version).map_err(ToolError::io(format!("cannot write {}", tmp.display())))?;
    fs::rename(&tmp, tool_dir.join("current")).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        ToolError::Io {
            context: format!("cannot write {}", tool_dir.join("current").display()),
            source: err,
        }
    })
}

/// Removes version folders other than `current` and `previous`, and stale
/// temporary files. The caller holds the tool lock, so no other install uses
/// the temporary files.
fn prune(tool_dir: &Path, current: &str, previous: Option<&str>) {
    let Ok(entries) = fs::read_dir(tool_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "current" || name == current || Some(name.as_str()) == previous {
            continue;
        }
        let path = entry.path();
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        let result = if name.starts_with(".tmp-") {
            if is_dir {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            }
        } else if is_dir && valid_version(&name) {
            fs::remove_dir_all(&path)
        } else {
            continue;
        };
        if let Err(err) = result {
            tracing::warn!(path = %path.display(), error = %err, "cannot remove an old tool version");
        }
    }
}
