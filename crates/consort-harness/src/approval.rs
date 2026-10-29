//! How each adapter maps the four approval modes onto its harness.
//!
//! The effective mode of a session is the lower of the requested mode and
//! the worker policy's `max_approval_mode`, and then the highest mode at or
//! below that which the adapter can enforce. See the table in
//! `docs/harness-adapters.md`.

use consort_proto::model::{ApprovalMode, Capabilities};
use serde_json::{Value, json};

/// The effective approval mode: the request capped by the worker policy,
/// then lowered to a mode that the adapter supports. `None` means the
/// adapter supports no mode at or below the cap.
pub fn effective_mode(
    requested: ApprovalMode,
    policy_max: ApprovalMode,
    capabilities: &Capabilities,
) -> Option<ApprovalMode> {
    capabilities.best_mode_up_to(requested.capped_by(policy_max))
}

/// The Claude Code permission mode (an ACP session mode in
/// `claude-agent-acp`).
pub fn claude_mode(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "default",
        // Claude Code has no mode between `acceptEdits` and
        // `bypassPermissions`, so Auto asks before commands, like Auto edits.
        ApprovalMode::AutoEdits | ApprovalMode::Auto => "acceptEdits",
        ApprovalMode::FullAccess => "bypassPermissions",
    }
}

/// The Codex `approvalPolicy` and `sandbox` values.
pub fn codex_policy(mode: ApprovalMode) -> (&'static str, &'static str) {
    match mode {
        // Read-only, so every edit and every command outside the trusted set
        // asks first.
        ApprovalMode::Ask => ("untrusted", "read-only"),
        // Edits in the workspace are allowed. Commands that leave the
        // sandbox ask.
        ApprovalMode::AutoEdits => ("on-request", "workspace-write"),
        ApprovalMode::Auto => ("never", "workspace-write"),
        ApprovalMode::FullAccess => ("never", "danger-full-access"),
    }
}

/// The Claude Code tools that run shell commands. With `allow_shell` off,
/// the adapter gives them to `claude-agent-acp` as `disallowedTools`, so the
/// model never sees them. Checked against Claude Agent SDK 0.3.312
/// (`BUILTIN_TOOL_NAMES`): `Monitor` runs a shell command too.
pub const CLAUDE_SHELL_TOOLS: [&str; 3] = ["Bash", "PowerShell", "Monitor"];

/// The Codex `features` that give the model a shell. With `allow_shell` off,
/// the adapter sets both to false in the `thread/start` config. Tested with
/// codex-cli 0.165.0: the model request then has no `exec_command` and no
/// `write_stdin` tool.
pub const CODEX_SHELL_FEATURES: [&str; 2] = ["shell_tool", "unified_exec"];

/// The Gemini CLI `--approval-mode` value.
pub fn gemini_mode(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "default",
        ApprovalMode::AutoEdits => "auto_edit",
        ApprovalMode::Auto | ApprovalMode::FullAccess => "yolo",
    }
}

/// The OpenCode `permission` configuration, passed in
/// `OPENCODE_CONFIG_CONTENT`. With `allow_shell` off, `bash` is `deny`.
pub fn opencode_permission(mode: ApprovalMode, allow_shell: bool) -> Value {
    let (edit, bash, webfetch) = match mode {
        ApprovalMode::Ask => ("ask", "ask", "ask"),
        ApprovalMode::AutoEdits => ("allow", "ask", "ask"),
        ApprovalMode::Auto | ApprovalMode::FullAccess => ("allow", "allow", "allow"),
    };
    let bash = if allow_shell { bash } else { "deny" };
    json!({ "permission": { "edit": edit, "bash": bash, "webfetch": webfetch } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_follow_the_table() {
        assert_eq!(claude_mode(ApprovalMode::Ask), "default");
        assert_eq!(claude_mode(ApprovalMode::AutoEdits), "acceptEdits");
        assert_eq!(claude_mode(ApprovalMode::FullAccess), "bypassPermissions");
        assert_eq!(codex_policy(ApprovalMode::Ask), ("untrusted", "read-only"));
        assert_eq!(
            codex_policy(ApprovalMode::FullAccess),
            ("never", "danger-full-access")
        );
        assert_eq!(gemini_mode(ApprovalMode::AutoEdits), "auto_edit");
        assert_eq!(
            opencode_permission(ApprovalMode::AutoEdits, true)["permission"]["bash"],
            "ask"
        );
        let off = opencode_permission(ApprovalMode::AutoEdits, false);
        assert_eq!(off["permission"]["bash"], "deny");
        assert_eq!(off["permission"]["edit"], "allow");
    }

    #[test]
    fn effective_mode_is_capped() {
        let caps = Capabilities {
            approval_modes: ApprovalMode::ALL.to_vec(),
            ..Capabilities::default()
        };
        assert_eq!(
            effective_mode(ApprovalMode::FullAccess, ApprovalMode::AutoEdits, &caps),
            Some(ApprovalMode::AutoEdits)
        );
        let only_ask = Capabilities {
            approval_modes: vec![ApprovalMode::Ask],
            ..Capabilities::default()
        };
        assert_eq!(
            effective_mode(ApprovalMode::Auto, ApprovalMode::FullAccess, &only_ask),
            Some(ApprovalMode::Ask)
        );
        let full_only = Capabilities {
            approval_modes: vec![ApprovalMode::FullAccess],
            ..Capabilities::default()
        };
        assert_eq!(
            effective_mode(ApprovalMode::Auto, ApprovalMode::FullAccess, &full_only),
            None
        );
    }
}
