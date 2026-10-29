//! The shared types of managed tools.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One managed tool.
///
/// The string form is `node`, `claude-code`, `claude-agent-acp`, `codex`,
/// `opencode`, `gemini-cli` or `acp:<registry id>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ToolId {
    /// Node.js 24 LTS. The npm based tools run on it.
    Node,
    /// The native Claude Code binary.
    ClaudeCode,
    /// The `@agentclientprotocol/claude-agent-acp` adapter.
    ClaudeAgentAcp,
    /// The Codex CLI, launched as `codex app-server`.
    Codex,
    /// OpenCode version 2, launched as `opencode acp`.
    OpenCode,
    /// Gemini CLI, launched with `--acp`.
    GeminiCli,
    /// An agent from the ACP registry. The value is the registry id.
    Acp(String),
}

impl ToolId {
    /// The tools that Consort knows by name, in display order.
    pub fn builtin() -> [ToolId; 6] {
        [
            ToolId::Node,
            ToolId::ClaudeCode,
            ToolId::ClaudeAgentAcp,
            ToolId::Codex,
            ToolId::OpenCode,
            ToolId::GeminiCli,
        ]
    }

    /// The tools that must be ready before this tool can install or run.
    ///
    /// For an ACP registry agent, the answer depends on its registry entry,
    /// so this list is empty. The store adds Node for an `npx` agent.
    pub fn dependencies(&self) -> &'static [ToolId] {
        match self {
            ToolId::ClaudeAgentAcp => &[ToolId::Node, ToolId::ClaudeCode],
            ToolId::GeminiCli => &[ToolId::Node],
            _ => &[],
        }
    }

    /// A name for the UI, such as "Claude Code".
    pub fn display_name(&self) -> &str {
        match self {
            ToolId::Node => "Node.js",
            ToolId::ClaudeCode => "Claude Code",
            ToolId::ClaudeAgentAcp => "Claude agent for ACP",
            ToolId::Codex => "Codex",
            ToolId::OpenCode => "OpenCode",
            ToolId::GeminiCli => "Gemini CLI",
            ToolId::Acp(id) => id,
        }
    }

    /// The folder name under the tools root.
    ///
    /// An ACP agent uses `acp-<id>`, because `:` is not valid in a Windows
    /// file name. No built-in tool name starts with `acp-`.
    pub fn dir_name(&self) -> String {
        match self {
            ToolId::Acp(id) => format!("acp-{id}"),
            other => other.to_string(),
        }
    }

    /// The tool that owns a folder name from [`ToolId::dir_name`].
    pub fn from_dir_name(name: &str) -> Option<ToolId> {
        match name.strip_prefix("acp-") {
            Some(id) if valid_registry_id(id) => Some(ToolId::Acp(id.to_owned())),
            Some(_) => None,
            None => match name.parse::<ToolId>() {
                Ok(ToolId::Acp(_)) | Err(_) => None,
                Ok(tool) => Some(tool),
            },
        }
    }
}

/// True when `id` matches the registry pattern `^[a-z][a-z0-9-]*$`
/// (at most 64 characters).
pub(crate) fn valid_registry_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl fmt::Display for ToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolId::Node => f.write_str("node"),
            ToolId::ClaudeCode => f.write_str("claude-code"),
            ToolId::ClaudeAgentAcp => f.write_str("claude-agent-acp"),
            ToolId::Codex => f.write_str("codex"),
            ToolId::OpenCode => f.write_str("opencode"),
            ToolId::GeminiCli => f.write_str("gemini-cli"),
            ToolId::Acp(id) => write!(f, "acp:{id}"),
        }
    }
}

/// The error for a tool name that is not known.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown tool \"{0}\"")]
pub struct ParseToolIdError(pub String);

impl FromStr for ToolId {
    type Err = ParseToolIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "node" => Ok(ToolId::Node),
            "claude-code" => Ok(ToolId::ClaudeCode),
            "claude-agent-acp" => Ok(ToolId::ClaudeAgentAcp),
            "codex" => Ok(ToolId::Codex),
            "opencode" => Ok(ToolId::OpenCode),
            "gemini-cli" => Ok(ToolId::GeminiCli),
            other => match other.strip_prefix("acp:") {
                Some(id) if valid_registry_id(id) => Ok(ToolId::Acp(id.to_owned())),
                _ => Err(ParseToolIdError(other.to_owned())),
            },
        }
    }
}

impl Serialize for ToolId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ToolId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The state of one tool on one worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    /// No version is installed.
    Missing,
    /// An install or an update runs now.
    Installing,
    /// A version is installed, and no newer version is known.
    Ready,
    /// A version is installed, and the last update check found a newer one.
    UpdateAvailable,
    /// No version is installed, and the last install failed.
    Failed,
}

/// The status of one tool, for the worker inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStatus {
    pub tool: ToolId,
    /// The installed version.
    pub version: Option<String>,
    /// The newest version from the last update check.
    pub latest: Option<String>,
    pub state: ToolState,
    /// An error message from the last install or update check.
    pub detail: Option<String>,
}

/// One installed version of a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub tool: ToolId,
    pub version: String,
    /// The version folder, `<root>/<tool dir>/<version>`.
    pub dir: PathBuf,
}

/// How to start a tool: the program, its arguments, and the environment
/// variables to add to the session environment.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_id_string_forms_round_trip() {
        let cases = [
            (ToolId::Node, "node"),
            (ToolId::ClaudeCode, "claude-code"),
            (ToolId::ClaudeAgentAcp, "claude-agent-acp"),
            (ToolId::Codex, "codex"),
            (ToolId::OpenCode, "opencode"),
            (ToolId::GeminiCli, "gemini-cli"),
            (ToolId::Acp("goose".into()), "acp:goose"),
        ];
        for (tool, text) in cases {
            assert_eq!(tool.to_string(), text);
            assert_eq!(text.parse::<ToolId>().unwrap(), tool);
            let json = serde_json::to_string(&tool).unwrap();
            assert_eq!(json, format!("\"{text}\""));
            assert_eq!(serde_json::from_str::<ToolId>(&json).unwrap(), tool);
        }
    }

    #[test]
    fn tool_id_refuses_bad_names() {
        for bad in [
            "",
            "Node",
            "acp:",
            "acp:Goose",
            "acp:../x",
            "acp:1abc",
            "acp:a b",
        ] {
            assert!(bad.parse::<ToolId>().is_err(), "{bad} parsed");
        }
        assert!(serde_json::from_str::<ToolId>("\"nope\"").is_err());
    }

    #[test]
    fn dir_names_round_trip_and_avoid_colons() {
        let mut tools = ToolId::builtin().to_vec();
        tools.push(ToolId::Acp("claude-acp".into()));
        for tool in tools {
            let dir = tool.dir_name();
            assert!(!dir.contains(':'));
            assert_eq!(ToolId::from_dir_name(&dir), Some(tool));
        }
        assert_eq!(ToolId::from_dir_name("acp-"), None);
        assert_eq!(ToolId::from_dir_name(".tmp-1"), None);
        assert_eq!(ToolId::from_dir_name("state.json"), None);
    }

    #[test]
    fn tool_state_uses_snake_case() {
        let json = serde_json::to_string(&ToolState::UpdateAvailable).unwrap();
        assert_eq!(json, "\"update_available\"");
    }
}
