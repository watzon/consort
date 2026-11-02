//! Managed tools: the CLIs and runtimes that a worker installs in
//! `<consort home>/tools/<tool>/<version>/`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A managed tool. A string on the wire: `node`, `claude-code`,
/// `claude-agent-acp`, `codex`, `opencode`, `gemini-cli`,
/// `chrome-headless-shell`, or `acp:<registry id>` for an agent from the ACP
/// registry.
///
/// Any string reads, so a newer worker can report a tool that this version
/// does not know. Use [`ToolId::is_known`] to check one.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolId(String);

impl ToolId {
    /// Node.js, for the tools that are npm packages.
    pub const NODE: &'static str = "node";
    /// The Claude Code CLI (`claude`).
    pub const CLAUDE_CODE: &'static str = "claude-code";
    /// The ACP adapter for Claude Code, `@agentclientprotocol/claude-agent-acp`.
    pub const CLAUDE_AGENT_ACP: &'static str = "claude-agent-acp";
    /// The Codex CLI.
    pub const CODEX: &'static str = "codex";
    /// OpenCode version 2.
    pub const OPENCODE: &'static str = "opencode";
    /// The Gemini CLI.
    pub const GEMINI_CLI: &'static str = "gemini-cli";
    /// The preview browser (`chrome-headless-shell`). The session host
    /// renders visuals with it. It is not a provider.
    pub const CHROME_HEADLESS_SHELL: &'static str = "chrome-headless-shell";
    /// The prefix of an ACP registry agent.
    pub const ACP_PREFIX: &'static str = "acp:";
    /// Every built-in tool, without the ACP registry agents.
    pub const BUILT_IN: [&'static str; 7] = [
        Self::NODE,
        Self::CLAUDE_CODE,
        Self::CLAUDE_AGENT_ACP,
        Self::CODEX,
        Self::OPENCODE,
        Self::GEMINI_CLI,
        Self::CHROME_HEADLESS_SHELL,
    ];

    /// Wraps a string. It is not checked.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The tool of an ACP registry agent: `acp:<registry id>`.
    pub fn acp(registry_id: &str) -> Self {
        Self(format!("{}{registry_id}", Self::ACP_PREFIX))
    }

    /// `node`.
    pub fn node() -> Self {
        Self::new(Self::NODE)
    }

    /// `claude-code`.
    pub fn claude_code() -> Self {
        Self::new(Self::CLAUDE_CODE)
    }

    /// `claude-agent-acp`.
    pub fn claude_agent_acp() -> Self {
        Self::new(Self::CLAUDE_AGENT_ACP)
    }

    /// `codex`.
    pub fn codex() -> Self {
        Self::new(Self::CODEX)
    }

    /// `opencode`.
    pub fn opencode() -> Self {
        Self::new(Self::OPENCODE)
    }

    /// `gemini-cli`.
    pub fn gemini_cli() -> Self {
        Self::new(Self::GEMINI_CLI)
    }

    /// `chrome-headless-shell`.
    pub fn chrome_headless_shell() -> Self {
        Self::new(Self::CHROME_HEADLESS_SHELL)
    }

    /// The string value.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The registry ID of an ACP registry agent, or `None` for a built-in
    /// tool.
    pub fn acp_registry_id(&self) -> Option<&str> {
        self.0
            .strip_prefix(Self::ACP_PREFIX)
            .filter(|id| !id.is_empty())
    }

    /// Whether this is a built-in tool or `acp:` with a registry ID.
    pub fn is_known(&self) -> bool {
        Self::BUILT_IN.contains(&self.0.as_str()) || self.acp_registry_id().is_some()
    }
}

impl fmt::Display for ToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ToolId({:?})", self.0)
    }
}

impl From<&str> for ToolId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ToolId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Where a managed tool is on a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    /// Not installed.
    Missing,
    /// The worker is installing or updating it.
    Installing,
    /// Installed and up to date, as far as the last check knows.
    Ready,
    /// Installed, and a newer version is available.
    UpdateAvailable,
    /// The last install or update failed. `detail` says why.
    Failed,
}

/// The state of one managed tool on a worker, in the inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStatus {
    /// The tool.
    pub tool: ToolId,
    /// The installed version, if any.
    #[serde(default)]
    pub version: Option<String>,
    /// The newest version that the last update check found.
    #[serde(default)]
    pub latest: Option<String>,
    /// Where the tool is.
    pub state: ToolState,
    /// More about the state, for example why an install failed.
    #[serde(default)]
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_ids() {
        assert_eq!(ToolId::claude_code().as_str(), "claude-code");
        assert_eq!(ToolId::acp("goose").as_str(), "acp:goose");
        assert_eq!(ToolId::acp("goose").acp_registry_id(), Some("goose"));
        assert_eq!(ToolId::codex().acp_registry_id(), None);
        for name in ToolId::BUILT_IN {
            assert!(ToolId::new(name).is_known(), "{name}");
        }
        assert!(ToolId::acp("goose").is_known());
        assert_eq!(
            ToolId::chrome_headless_shell().as_str(),
            "chrome-headless-shell"
        );
        assert_eq!(
            ToolId::BUILT_IN.last(),
            Some(&ToolId::CHROME_HEADLESS_SHELL)
        );
        assert!(ToolId::chrome_headless_shell().is_known());
        assert!(!ToolId::new("acp:").is_known());
        assert!(!ToolId::new("pip").is_known());
        assert_eq!(
            serde_json::to_value(ToolId::gemini_cli()).unwrap(),
            json!("gemini-cli")
        );
        let back: ToolId = serde_json::from_value(json!("acp:goose")).unwrap();
        assert_eq!(back, ToolId::acp("goose"));
        assert_eq!(format!("{back}"), "acp:goose");
        assert_eq!(format!("{back:?}"), "ToolId(\"acp:goose\")");
    }

    #[test]
    fn tool_status_round_trips() {
        let status = ToolStatus {
            tool: ToolId::claude_code(),
            version: Some("2.1.0".into()),
            latest: Some("2.2.0".into()),
            state: ToolState::UpdateAvailable,
            detail: None,
        };
        let value = serde_json::to_value(&status).unwrap();
        assert_eq!(
            value,
            json!({"tool": "claude-code", "version": "2.1.0", "latest": "2.2.0",
                   "state": "update_available", "detail": null})
        );
        assert_eq!(serde_json::from_value::<ToolStatus>(value).unwrap(), status);
        let minimal: ToolStatus =
            serde_json::from_value(json!({"tool": "node", "state": "missing"})).unwrap();
        assert_eq!(minimal.version, None);
        for (state, text) in [
            (ToolState::Missing, "missing"),
            (ToolState::Installing, "installing"),
            (ToolState::Ready, "ready"),
            (ToolState::UpdateAvailable, "update_available"),
            (ToolState::Failed, "failed"),
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), text);
        }
    }
}
