//! Regression test for review cycle 3, finding S1.
//!
//! A new worker caps sessions at Auto (`default_max_mode` in
//! `consort-worker`). Settings tells the user what Auto does on each
//! harness: on Claude Code it asks only for destructive actions and for
//! actions outside the workspace, and on Gemini CLI and OpenCode it still
//! asks before commands. OpenCode and
//! Gemini CLI have no sandbox, so a mode that lets commands run with no
//! prompt is Full access for them. These tests make sure that only Full
//! access maps to such a configuration, for every adapter.

use consort_harness::HarnessKind;
use consort_harness::acp::static_capabilities;
use consort_harness::approval::{
    claude_mode, codex_policy, effective_mode, gemini_mode, opencode_permission,
};
use consort_proto::model::ApprovalMode;

/// The default `max_approval_mode` of a worker policy.
const DEFAULT_WORKER_CAP: ApprovalMode = ApprovalMode::Auto;

/// Whether an OpenCode configuration lets a command or a fetch run with no
/// prompt.
fn opencode_runs_without_asking(mode: ApprovalMode) -> bool {
    let config = opencode_permission(mode, true);
    let permission = &config["permission"];
    permission["bash"] == "allow" || permission["webfetch"] == "allow"
}

#[test]
fn auto_asks_before_commands_and_fetches_in_harnesses_with_no_sandbox() {
    for kind in [HarnessKind::OpenCode, HarnessKind::GeminiCli] {
        let caps = static_capabilities(kind);
        let mode = effective_mode(ApprovalMode::Auto, DEFAULT_WORKER_CAP, &caps);
        assert_eq!(mode, Some(ApprovalMode::Auto), "{kind:?}");
    }

    assert!(!opencode_runs_without_asking(ApprovalMode::Auto));
    assert_eq!(
        opencode_permission(ApprovalMode::Auto, true),
        opencode_permission(ApprovalMode::AutoEdits, true)
    );
    assert_ne!(
        opencode_permission(ApprovalMode::Auto, true),
        opencode_permission(ApprovalMode::FullAccess, true)
    );
    assert_eq!(gemini_mode(ApprovalMode::Auto), "auto_edit");
    assert_ne!(
        gemini_mode(ApprovalMode::Auto),
        gemini_mode(ApprovalMode::FullAccess)
    );

    // Claude Auto is Claude Code's own `auto` mode, and Codex Auto runs
    // commands in a `workspace-write` sandbox.
    assert_eq!(claude_mode(ApprovalMode::Auto), "auto");
    assert_eq!(
        codex_policy(ApprovalMode::Auto),
        ("never", "workspace-write")
    );
}

#[test]
fn only_full_access_runs_with_no_prompt() {
    for mode in ApprovalMode::ALL {
        let full = mode == ApprovalMode::FullAccess;
        assert_eq!(opencode_runs_without_asking(mode), full, "{mode:?}");
        assert_eq!(gemini_mode(mode) == "yolo", full, "{mode:?}");
        assert_eq!(claude_mode(mode) == "bypassPermissions", full, "{mode:?}");
        assert_eq!(
            codex_policy(mode).1 == "danger-full-access",
            full,
            "{mode:?}"
        );
    }
    // The old behavior of Gemini Auto stays possible, but only when the
    // user allows Full access in `worker.toml`.
    let gemini = static_capabilities(HarnessKind::GeminiCli);
    assert_eq!(
        effective_mode(ApprovalMode::FullAccess, ApprovalMode::FullAccess, &gemini),
        Some(ApprovalMode::FullAccess)
    );
    assert_eq!(
        effective_mode(ApprovalMode::FullAccess, DEFAULT_WORKER_CAP, &gemini),
        Some(ApprovalMode::Auto)
    );
}
