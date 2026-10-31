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
/// `claude-agent-acp`). It is `default` for every approval mode, and never
/// `bypassPermissions`. The CLI has only Consort tools, and the approval
/// engine of the hub decides at the MCP endpoint.
pub fn claude_mode(_mode: ApprovalMode) -> &'static str {
    "default"
}

/// The permission mode to use when the adapter does not offer `mode`, or
/// when the account or the model does not support it. `claude-agent-acp`
/// 0.99.0 offers `auto` in every session, but it sets `acceptEdits` instead
/// when the model does not support it (`supportsAutoMode`), and Claude Code
/// can refuse it for an account. Only `auto` has a fallback: the
/// session must never fall back to a mode that is above the one that the
/// user chose.
pub fn claude_mode_fallback(mode: &str) -> Option<&'static str> {
    (mode == "auto").then_some("acceptEdits")
}

/// How much a Claude Code permission mode lets run without a prompt. A
/// higher rank allows more. `auto` allows no more than the Auto approval mode,
/// which maps to it, so it is allowed only from Auto up. `plan` and `dontAsk` never allow more than
/// `default`. An unknown mode ranks highest, so the adapter treats it as
/// above every limit.
pub fn claude_mode_rank(mode: &str) -> u8 {
    match mode {
        "plan" | "dontAsk" | "default" => 0,
        "acceptEdits" => 1,
        "auto" => 2,
        _ => 3,
    }
}

/// Whether a Claude Code permission mode stays at or below the mode of
/// `limit`.
pub fn claude_mode_allowed(mode: &str, limit: ApprovalMode) -> bool {
    claude_mode_rank(mode) <= claude_mode_rank(claude_mode(limit))
}

/// The permission mode that a `claude-agent-acp` permission option sets,
/// when the option sets one. Checked against `claude-agent-acp` 0.99.0
/// (`PERMISSION_OPTION_ID` and `applyExitPlanModeSelection`).
///
/// `allow-with-updates` applies the changes that Claude Code suggests. For
/// a file change these can set `acceptEdits` for the session, and the
/// option does not say which changes it holds. So for a file change the
/// adapter treats the option as `acceptEdits`.
pub fn claude_option_mode(option_id: &str, file_change: bool) -> Option<&'static str> {
    match option_id {
        "exit-plan-bypass" | "exit-plan-clear-bypass" => Some("bypassPermissions"),
        "exit-plan-auto" | "exit-plan-clear-auto" => Some("auto"),
        "exit-plan-accept-edits" | "exit-plan-clear-accept-edits" => Some("acceptEdits"),
        "exit-plan-default" => Some("default"),
        "allow-with-updates" if file_change => Some("acceptEdits"),
        _ => None,
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

/// The Claude Code tools that reach the web. A coordinator session does no
/// web work, so the adapter gives these to `claude-agent-acp` as
/// `disallowedTools` too. Checked against `claude-agent-acp` 0.99.0, which
/// names them `WebFetch` and `WebSearch`.
pub const CLAUDE_WEB_TOOLS: [&str; 2] = ["WebFetch", "WebSearch"];

/// The Codex `thread/start` config key that turns the web search tool off,
/// and its value. Checked with codex-cli 0.165.0: the schema of
/// `app-server` has `web_search` with the modes `disabled`, `cached`,
/// `indexed` and `live`.
pub const CODEX_WEB_SEARCH_OFF: (&str, &str) = ("web_search", "disabled");

/// The Gemini CLI tools that a coordinator may not use: the web, and the
/// `codebase_investigator` agent, which Gemini CLI 0.66.0 runs with no
/// prompt in `default` mode (`policies/read-only.toml`).
pub const GEMINI_COORDINATOR_DENIED: [&str; 3] =
    ["google_web_search", "web_fetch", "codebase_investigator"];

/// The Gemini CLI tool that runs shell commands.
pub const GEMINI_SHELL_TOOL: &str = "run_shell_command";

/// The name of the MCP server that carries the hub tools.
const HUB_SERVER_NAME: &str = "consort";

/// The Gemini CLI policy file of a session, or `None` when the session
/// needs none. The adapter passes it with `--admin-policy`, the highest
/// tier, so no setting of the user or the repo can allow what it denies
/// (Gemini CLI 0.66.0, `docs/reference/policy-engine.md`).
///
/// - With `allow_shell` off, `run_shell_command` is denied.
/// - A coordinator may not use the web or `codebase_investigator`.
/// - A coordinator gets its hub tools with no approval. Gemini CLI names
///   an MCP tool in its permission request by the tool name and its
///   arguments, with no server name, so the worker cannot tell a hub tool
///   from another tool there. The policy names the server instead.
pub fn gemini_policy(allow_shell: bool, coordinator: bool) -> Option<String> {
    let mut denied: Vec<&str> = Vec::new();
    if !allow_shell {
        denied.push(GEMINI_SHELL_TOOL);
    }
    if coordinator {
        denied.extend(GEMINI_COORDINATOR_DENIED);
    }
    if denied.is_empty() && !coordinator {
        return None;
    }
    let mut out = String::new();
    if !denied.is_empty() {
        let names: Vec<String> = denied.iter().map(|n| format!("\"{n}\"")).collect();
        out.push_str(&format!(
            "[[rule]]\ntoolName = [{}]\ndecision = \"deny\"\npriority = 999\ndenyMessage = \"Consort does not allow this tool in this session. A coordinator starts a task with start_task, and the task does this work.\"\n\n",
            names.join(", ")
        ));
    }
    if coordinator {
        out.push_str(&format!(
            "[[rule]]\nmcpName = \"{HUB_SERVER_NAME}\"\ntoolName = \"*\"\ndecision = \"allow\"\npriority = 500\n"
        ));
    }
    Some(out)
}

/// Turns the web off in the OpenCode config of a coordinator, and allows its
/// hub tools. `webfetch` and `websearch` are `permission` keys of OpenCode
/// 2.2.1 (the config schema accepts only `ask`, `allow` or `deny` for
/// `websearch`). The hub tools are named `hub_<tool>`; OpenCode matches
/// the keys of `permission` against tool names with `*`, so `hub_*` covers
/// them. That match is not tested end to end with a real model.
pub fn opencode_coordinator_permission(config: &mut Value) {
    if let Some(permission) = config.get_mut("permission").and_then(Value::as_object_mut) {
        permission.insert("webfetch".to_owned(), json!("deny"));
        permission.insert("websearch".to_owned(), json!("deny"));
        permission.insert(format!("{HUB_SERVER_NAME}_*"), json!("allow"));
    }
}

/// The Gemini CLI `--approval-mode` value.
///
/// Gemini CLI has no sandbox and no mode between `auto_edit` and `yolo`.
/// `yolo` runs every command and every fetch with no prompt, so only Full
/// access gets it. Auto asks before commands, like Auto edits.
pub fn gemini_mode(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "default",
        ApprovalMode::AutoEdits | ApprovalMode::Auto => "auto_edit",
        ApprovalMode::FullAccess => "yolo",
    }
}

/// The environment variables that stop OpenCode from loading the
/// project's configuration: `opencode.json`, and the `.opencode`,
/// `.claude` and `.agents` folders of the workspace and of the folders
/// above it. OpenCode 2.2.0 reads the first name, and the second one when
/// the first is not set. Both go to the agent below Full access.
pub const OPENCODE_NO_PROJECT_CONFIG: [&str; 2] = [
    "OPENCODE_CONFIG_PROJECT_DISABLE",
    "OPENCODE_DISABLE_PROJECT_CONFIG",
];

/// The OpenCode `permission` configuration, passed in
/// `OPENCODE_CONFIG_CONTENT`. With `allow_shell` off, `bash` is `deny`.
///
/// OpenCode has no sandbox, so `bash` and `webfetch` on `allow` let every
/// command and every fetch run with no prompt. Only Full access gets that.
/// Auto asks before commands and fetches, like Auto edits.
pub fn opencode_permission(mode: ApprovalMode, allow_shell: bool) -> Value {
    let (edit, bash, webfetch) = match mode {
        ApprovalMode::Ask => ("ask", "ask", "ask"),
        ApprovalMode::AutoEdits | ApprovalMode::Auto => ("allow", "ask", "ask"),
        ApprovalMode::FullAccess => ("allow", "allow", "allow"),
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
        assert_eq!(claude_mode(ApprovalMode::AutoEdits), "default");
        assert_eq!(claude_mode(ApprovalMode::Auto), "default");
        assert_eq!(claude_mode(ApprovalMode::FullAccess), "default");
        assert_eq!(codex_policy(ApprovalMode::Ask), ("untrusted", "read-only"));
        assert_eq!(
            codex_policy(ApprovalMode::FullAccess),
            ("never", "danger-full-access")
        );
        assert_eq!(gemini_mode(ApprovalMode::AutoEdits), "auto_edit");
        assert_eq!(gemini_mode(ApprovalMode::Auto), "auto_edit");
        assert_eq!(gemini_mode(ApprovalMode::FullAccess), "yolo");
        let auto = opencode_permission(ApprovalMode::Auto, true);
        assert_eq!(auto["permission"]["edit"], "allow");
        assert_eq!(auto["permission"]["bash"], "ask");
        assert_eq!(auto["permission"]["webfetch"], "ask");
        assert_eq!(
            opencode_permission(ApprovalMode::FullAccess, true)["permission"]["bash"],
            "allow"
        );
        assert_eq!(
            opencode_permission(ApprovalMode::AutoEdits, true)["permission"]["bash"],
            "ask"
        );
        let off = opencode_permission(ApprovalMode::AutoEdits, false);
        assert_eq!(off["permission"]["bash"], "deny");
        assert_eq!(off["permission"]["edit"], "allow");
    }

    #[test]
    fn claude_modes_have_a_rank() {
        // The limit is `default` in every approval mode, so the CLI cannot
        // raise its own mode.
        for limit in ApprovalMode::ALL {
            assert!(claude_mode_allowed("default", limit));
            assert!(claude_mode_allowed("plan", limit));
            assert!(!claude_mode_allowed("acceptEdits", limit));
            assert!(!claude_mode_allowed("auto", limit));
            assert!(!claude_mode_allowed("bypassPermissions", limit));
            assert!(!claude_mode_allowed("somethingNew", limit));
        }
        assert_eq!(claude_mode_fallback("auto"), Some("acceptEdits"));
        assert_eq!(claude_mode_fallback("acceptEdits"), None);
        assert_eq!(claude_mode_fallback("bypassPermissions"), None);
        assert_eq!(claude_option_mode("exit-plan-auto", false), Some("auto"));
        assert_eq!(
            claude_option_mode("allow-with-updates", true),
            Some("acceptEdits")
        );
        assert_eq!(claude_option_mode("allow-with-updates", false), None);
        assert_eq!(claude_option_mode("allow-once", true), None);
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
