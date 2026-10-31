//! Tasks, placement requests, isolation and approval modes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::{ModelSelection, Os};
use crate::ids::{ChannelId, MessageId, RepoId, SessionId, TaskId, WorkerId};
use crate::status::TaskStatus;
use crate::time::Timestamp;

/// How a task's workspace relates to a checkout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// Run in the user's own checkout, with its uncommitted work.
    InPlace,
    /// Make a git worktree from the checkout's `HEAD`. The default.
    #[default]
    Worktree,
    /// Make a fresh clone in the worker's `workspaces/` folder.
    Clone,
}

/// How much a session may do without asking.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Ask before edits and commands. The safe fallback of the type. New
    /// channels and Tutti are created in Auto (see DECISIONS 699).
    #[default]
    Ask,
    /// Edit files without asking. Ask before commands.
    AutoEdits,
    /// Run without asking where the harness has a middle mode. Claude Code
    /// asks only for destructive actions and for actions outside the
    /// workspace. Codex runs commands in its sandbox. Gemini CLI and OpenCode
    /// still ask before commands.
    Auto,
    /// Run without asking and without a sandbox.
    FullAccess,
}

impl ApprovalMode {
    /// Every mode, from the least to the most access.
    pub const ALL: [ApprovalMode; 4] = [Self::Ask, Self::AutoEdits, Self::Auto, Self::FullAccess];

    /// The effective mode: the lower of this requested mode and the worker
    /// policy's `max_approval_mode`. The variant order is from the least to
    /// the most access, so the lower mode is the stricter one.
    pub fn capped_by(self, max: ApprovalMode) -> ApprovalMode {
        self.min(max)
    }
}

/// Constraints on the worker for a task.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlacementRequire {
    /// The worker's operating system.
    #[serde(default)]
    pub os: Option<Os>,
    /// Tags that the worker must have, for example `gpu`. Older peers send
    /// this as `labels`.
    #[serde(default, alias = "labels")]
    pub tags: Vec<String>,
    /// Local MCP servers that the worker must have, by name.
    #[serde(default)]
    pub local_mcp_servers: Vec<String>,
}

impl PlacementRequire {
    /// Whether a worker with this OS, these tags and these local MCP server
    /// names meets every constraint.
    pub fn is_satisfied_by<'a>(
        &self,
        os: Os,
        tags: impl IntoIterator<Item = &'a str> + Clone,
        local_mcp_servers: impl IntoIterator<Item = &'a str> + Clone,
    ) -> bool {
        if self.os.is_some_and(|wanted| wanted != os) {
            return false;
        }
        let has_tags = self
            .tags
            .iter()
            .all(|wanted| tags.clone().into_iter().any(|t| t == wanted));
        let has_servers = self
            .local_mcp_servers
            .iter()
            .all(|wanted| local_mcp_servers.clone().into_iter().any(|s| s == wanted));
        has_tags && has_servers
    }
}

/// Where a task may run. Empty fields take the channel's defaults when the
/// hub creates the task.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlacementRequest {
    /// A fixed worker. Placement fails clearly if it cannot use this worker.
    #[serde(default)]
    pub worker_id: Option<WorkerId>,
    /// Constraints on the worker.
    #[serde(default)]
    pub require: Option<PlacementRequire>,
    /// The repos to put in the workspace. Empty means the channel repos.
    #[serde(default)]
    pub repos: Option<Vec<RepoId>>,
    /// The isolation. Empty means the channel's default.
    #[serde(default)]
    pub isolation: Option<Isolation>,
}

/// Who created a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCreator {
    /// The user, from a client.
    User,
    /// A coordinator, with `start_task`.
    Assistant,
    /// A routine with a `start_task` action.
    Routine,
}

/// A unit of work that runs in its own session and reports in its own thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// The ID.
    pub id: TaskId,
    /// The channel.
    pub channel_id: ChannelId,
    /// The title.
    pub title: String,
    /// The first instruction to the task session, in markdown.
    pub brief: String,
    /// The task status.
    pub status: TaskStatus,
    /// Why the task is in this status, for example "Waiting for bigboi to
    /// come online".
    pub status_reason: Option<String>,
    /// Who created the task.
    pub created_by: TaskCreator,
    /// The task card in the timeline. The task thread hangs from it.
    pub card_message_id: MessageId,
    /// The model selection. Empty means the channel's `task_selection`.
    pub model_selection: Option<ModelSelection>,
    /// Where the task may run.
    #[serde(default)]
    pub placement: PlacementRequest,
    /// The requested approval mode.
    pub approval_mode: ApprovalMode,
    /// When a Scheduled task starts.
    pub start_at: Option<Timestamp>,
    /// The current session.
    pub current_session_id: Option<SessionId>,
    /// The worker of the current session, so the task card can show when it
    /// is offline.
    pub worker_id: Option<WorkerId>,
    /// The text that the task gave when it went to Ready for review.
    pub review_summary: Option<String>,
    /// When the user resolved the task.
    pub resolved_at: Option<Timestamp>,
    /// The current session of the task has ended, so the task cannot take a
    /// message. A task in Ready for review or Resolved can then only
    /// retry. A new session sets it back to false. Older hubs leave it out.
    #[serde(default)]
    pub session_ended: bool,
    /// How many Monid calls of this task ran.
    #[serde(default)]
    pub monid_calls: u32,
    /// What they cost, in millionths of a dollar.
    #[serde(default)]
    pub monid_cost_micros: i64,
    /// When the task was archived. Archive hides a task in any status.
    pub archived_at: Option<Timestamp>,
    /// When the task was created.
    pub created_at: Timestamp,
    /// When the task last changed.
    pub updated_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_mode_order_and_cap() {
        assert!(ApprovalMode::Ask < ApprovalMode::AutoEdits);
        assert!(ApprovalMode::Auto < ApprovalMode::FullAccess);
        assert_eq!(
            ApprovalMode::FullAccess.capped_by(ApprovalMode::AutoEdits),
            ApprovalMode::AutoEdits
        );
        assert_eq!(
            ApprovalMode::Ask.capped_by(ApprovalMode::FullAccess),
            ApprovalMode::Ask
        );
        assert_eq!(
            serde_json::to_string(&ApprovalMode::FullAccess).unwrap(),
            "\"full_access\""
        );
        assert_eq!(
            serde_json::to_string(&ApprovalMode::AutoEdits).unwrap(),
            "\"auto_edits\""
        );
    }

    #[test]
    fn isolation_strings() {
        assert_eq!(
            serde_json::to_string(&Isolation::InPlace).unwrap(),
            "\"in_place\""
        );
        assert_eq!(Isolation::default(), Isolation::Worktree);
    }

    #[test]
    fn require_matching() {
        let req = PlacementRequire {
            os: Some(Os::Windows),
            tags: vec!["gpu".into()],
            local_mcp_servers: vec!["home-assistant".into()],
        };
        assert!(req.is_satisfied_by(Os::Windows, ["gpu", "windows-test"], ["home-assistant"]));
        assert!(!req.is_satisfied_by(Os::Linux, ["gpu"], ["home-assistant"]));
        assert!(!req.is_satisfied_by(Os::Windows, ["windows-test"], ["home-assistant"]));
        assert!(!req.is_satisfied_by(Os::Windows, ["gpu"], []));
        assert!(PlacementRequire::default().is_satisfied_by(Os::Macos, [], []));
    }

    #[test]
    fn require_reads_old_labels() {
        let old: PlacementRequire =
            serde_json::from_value(serde_json::json!({"labels": ["gpu"]})).unwrap();
        assert_eq!(old.tags, ["gpu"]);
        let value = serde_json::to_value(&old).unwrap();
        assert_eq!(value["tags"], serde_json::json!(["gpu"]));
        assert!(value.get("labels").is_none());
    }
}
