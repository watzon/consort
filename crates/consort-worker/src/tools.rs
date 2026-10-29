//! The worker's managed tools: installs on demand, the update check, and
//! updates that wait until no session uses the tool.
//!
//! The [`ToolStore`] of `consort-tools` does the downloads and the checks.
//! This module adds what the worker needs on top:
//!
//! - [`Tools::ensure`] installs a missing tool once, also when several
//!   sessions ask for it at the same time.
//! - A [`ToolLease`] marks a tool and the tools that it needs as in use
//!   while a session, a probe or a sign-in runs it.
//! - [`Tools::request_install`] runs at once when the tool is idle, and
//!   waits otherwise. The worker never replaces a tool that a session uses.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use consort_proto::model::{ToolState as ProtoToolState, ToolStatus as ProtoToolStatus};
use consort_tools::{LaunchSpec, ToolError, ToolId, ToolState, ToolStatus, ToolStore};
use tokio::sync::Notify;

/// How often the worker checks for updates.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The managed tools of one worker.
pub struct Tools {
    store: ToolStore,
    in_use: Mutex<HashMap<ToolId, usize>>,
    pending: Mutex<BTreeSet<ToolId>>,
    ensure_locks: Mutex<HashMap<ToolId, Arc<tokio::sync::Mutex<()>>>>,
    /// Wakes the tools loop: a lease ended, or an install waits.
    pub wake: Notify,
    /// Whether the worker checks for updates by itself. Tests turn it off,
    /// so they make no network request.
    pub check_updates: bool,
}

impl std::fmt::Debug for Tools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tools")
            .field("root", &self.store.root())
            .finish_non_exhaustive()
    }
}

/// The tools that `tool` needs to run, with `tool` itself, in no order.
pub fn with_dependencies(tool: &ToolId) -> Vec<ToolId> {
    let mut out = vec![tool.clone()];
    let mut i = 0;
    while i < out.len() {
        for dependency in out[i].dependencies() {
            if !out.contains(dependency) {
                out.push(dependency.clone());
            }
        }
        i += 1;
    }
    out
}

/// Marks tools as in use while it lives.
pub struct ToolLease {
    tools: Arc<Tools>,
    held: Vec<ToolId>,
}

impl std::fmt::Debug for ToolLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolLease")
            .field("held", &self.held)
            .finish()
    }
}

impl Drop for ToolLease {
    fn drop(&mut self) {
        let mut freed = false;
        {
            let mut in_use = lock(&self.tools.in_use);
            for tool in &self.held {
                if let Some(count) = in_use.get_mut(tool) {
                    *count -= 1;
                    if *count == 0 {
                        in_use.remove(tool);
                        freed = true;
                    }
                }
            }
        }
        if freed {
            self.tools.wake.notify_one();
        }
    }
}

impl Tools {
    /// Opens the store at `root`.
    pub fn new(root: std::path::PathBuf, check_updates: bool) -> Result<Self, ToolError> {
        Ok(Self::with_store(ToolStore::new(root)?, check_updates))
    }

    /// Uses a store that the caller made. Tests pass one with local
    /// sources.
    pub fn with_store(store: ToolStore, check_updates: bool) -> Self {
        Self {
            store,
            in_use: Mutex::new(HashMap::new()),
            pending: Mutex::new(BTreeSet::new()),
            ensure_locks: Mutex::new(HashMap::new()),
            wake: Notify::new(),
            check_updates,
        }
    }

    /// The store.
    pub fn store(&self) -> &ToolStore {
        &self.store
    }

    /// Marks a tool and the tools that it needs as in use.
    pub fn lease(self: &Arc<Self>, tool: &ToolId) -> ToolLease {
        let held = with_dependencies(tool);
        {
            let mut in_use = lock(&self.in_use);
            for tool in &held {
                *in_use.entry(tool.clone()).or_insert(0) += 1;
            }
        }
        ToolLease {
            tools: self.clone(),
            held,
        }
    }

    /// Whether a session, a probe or a sign-in uses the tool now.
    pub fn in_use(&self, tool: &ToolId) -> bool {
        lock(&self.in_use).contains_key(tool)
    }

    /// The launch of a tool. A missing tool is installed first. When several
    /// callers ask for the same missing tool, it installs once.
    pub async fn ensure(&self, tool: &ToolId) -> Result<LaunchSpec, ToolError> {
        if self.store.installed(tool).is_some() {
            return self.store.launch(tool);
        }
        let gate = lock(&self.ensure_locks)
            .entry(tool.clone())
            .or_default()
            .clone();
        let _guard = gate.lock().await;
        if self.store.installed(tool).is_none() {
            self.store.install(tool, None).await?;
        }
        self.store.launch(tool)
    }

    /// Whether the tool or a tool that it needs is installing now.
    pub fn installing(&self, tool: &ToolId) -> bool {
        let needed = with_dependencies(tool);
        self.store
            .status()
            .iter()
            .any(|s| s.state == ToolState::Installing && needed.contains(&s.tool))
    }

    /// The last install error of a tool or a tool that it needs, when it is
    /// not installed.
    pub fn install_failure(&self, tool: &ToolId) -> Option<String> {
        let needed = with_dependencies(tool);
        self.store
            .status()
            .into_iter()
            .find(|s| s.state == ToolState::Failed && needed.contains(&s.tool))
            .and_then(|s| s.detail)
    }

    /// Asks for an install or an update. It waits until no session uses
    /// the tool. The tools loop runs it.
    pub fn request_install(&self, tool: ToolId) {
        lock(&self.pending).insert(tool);
        self.wake.notify_one();
    }

    /// The waiting installs whose tools are idle now. They leave the
    /// waiting list.
    pub fn take_ready(&self) -> Vec<ToolId> {
        let mut pending = lock(&self.pending);
        let ready: Vec<ToolId> = pending
            .iter()
            .filter(|tool| !self.in_use(tool))
            .cloned()
            .collect();
        for tool in &ready {
            pending.remove(tool);
        }
        ready
    }

    /// Whether the update check is due: never done, or done more than a day
    /// ago.
    pub fn check_due(&self) -> bool {
        match self.store.last_check() {
            None => true,
            Some(at) => SystemTime::now()
                .duration_since(at)
                .map_or(true, |age| age >= CHECK_INTERVAL),
        }
    }

    /// The status of every tool for the inventory.
    pub fn statuses(&self) -> Vec<ProtoToolStatus> {
        let pending = lock(&self.pending).clone();
        self.store
            .status()
            .into_iter()
            .map(|status| {
                let waits = pending.contains(&status.tool) && status.state != ToolState::Installing;
                to_proto(status, waits)
            })
            .collect()
    }
}

/// A name for a tool in a status text, such as "Claude Code".
pub fn tool_name(tool: &ToolId) -> String {
    match tool {
        // The ACP adapter is part of Claude Code for the user.
        ToolId::ClaudeAgentAcp => "Claude Code".to_owned(),
        other => other.display_name().to_owned(),
    }
}

fn to_proto(status: ToolStatus, waits: bool) -> ProtoToolStatus {
    let state = match status.state {
        ToolState::Missing => ProtoToolState::Missing,
        ToolState::Installing => ProtoToolState::Installing,
        ToolState::Ready => ProtoToolState::Ready,
        ToolState::UpdateAvailable => ProtoToolState::UpdateAvailable,
        ToolState::Failed => ProtoToolState::Failed,
    };
    let detail = if waits {
        Some(format!(
            "The install waits until no session uses {}.",
            tool_name(&status.tool)
        ))
    } else {
        status.detail
    };
    ProtoToolStatus {
        tool: consort_proto::model::ToolId::new(status.tool.to_string()),
        version: status.version,
        latest: status.latest,
        state,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools(dir: &std::path::Path) -> Arc<Tools> {
        Arc::new(Tools::new(dir.join("tools"), false).unwrap())
    }

    /// Makes `tool` look installed at `version`, as the store lays it out.
    fn fake_install(dir: &std::path::Path, tool: &ToolId, version: &str) {
        let tool_dir = dir.join("tools").join(tool.dir_name());
        std::fs::create_dir_all(tool_dir.join(version)).unwrap();
        std::fs::write(tool_dir.join("current"), version).unwrap();
    }

    #[test]
    fn dependencies_are_in_use_with_the_tool() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools(dir.path());
        let lease = tools.lease(&ToolId::ClaudeAgentAcp);
        assert!(tools.in_use(&ToolId::ClaudeAgentAcp));
        assert!(tools.in_use(&ToolId::Node));
        assert!(tools.in_use(&ToolId::ClaudeCode));
        assert!(!tools.in_use(&ToolId::Codex));
        let second = tools.lease(&ToolId::GeminiCli);
        drop(lease);
        assert!(tools.in_use(&ToolId::Node), "Gemini CLI still runs on Node");
        assert!(!tools.in_use(&ToolId::ClaudeCode));
        drop(second);
        assert!(!tools.in_use(&ToolId::Node));
    }

    #[test]
    fn an_install_waits_until_the_tool_is_idle() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools(dir.path());
        let lease = tools.lease(&ToolId::Codex);
        tools.request_install(ToolId::Codex);
        tools.request_install(ToolId::OpenCode);
        assert_eq!(tools.take_ready(), [ToolId::OpenCode]);
        let codex = tools
            .statuses()
            .into_iter()
            .find(|s| s.tool.as_str() == "codex")
            .unwrap();
        assert!(
            codex
                .detail
                .unwrap()
                .contains("waits until no session uses Codex"),
            "the inventory says why"
        );
        assert!(tools.take_ready().is_empty());
        drop(lease);
        assert_eq!(tools.take_ready(), [ToolId::Codex]);
    }

    #[tokio::test]
    async fn an_installed_tool_launches_without_a_download() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools(dir.path());
        fake_install(dir.path(), &ToolId::ClaudeCode, "2.1.308");
        let launch = tools.ensure(&ToolId::ClaudeCode).await.unwrap();
        assert!(
            launch.program.ends_with(
                std::path::Path::new("claude-code")
                    .join("2.1.308")
                    .join(format!("claude{}", std::env::consts::EXE_SUFFIX))
            )
        );
        assert_eq!(launch.env["DISABLE_AUTOUPDATER"], "1");
        let statuses = tools.statuses();
        let claude = statuses
            .iter()
            .find(|s| s.tool.as_str() == "claude-code")
            .unwrap();
        assert_eq!(claude.state, ProtoToolState::Ready);
        assert_eq!(claude.version.as_deref(), Some("2.1.308"));
        let node = statuses.iter().find(|s| s.tool.as_str() == "node").unwrap();
        assert_eq!(node.state, ProtoToolState::Missing);
        assert!(tools.check_due());
        assert_eq!(tool_name(&ToolId::ClaudeAgentAcp), "Claude Code");
    }
}
