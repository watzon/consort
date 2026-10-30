use super::*;
use consort_proto::ids::SessionId;
use consort_proto::model::{
    ApprovalDetails, FileChangeKind, ItemPayload, ItemStatus, RequestOptionKind,
};
use consort_proto::worker_link::RequestBody;
use serde_json::json;

fn mapper() -> AcpMapper {
    let mut m = AcpMapper::new(SessionId::new());
    m.start_turn(TurnId::new("t1"));
    m
}

fn items(events: &[SessionEvent]) -> Vec<(ItemPayload, ItemStatus)> {
    events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Item(i) => Some((i.payload.clone(), i.status)),
            _ => None,
        })
        .collect()
}

#[test]
fn recorded_claude_turn_maps_to_one_message() {
    let text = include_str!("../../tests/fixtures/acp-claude-ok-turn.jsonl");
    let mut m = mapper();
    let mut events = Vec::new();
    for line in text.lines() {
        let message: Value = serde_json::from_str(line).unwrap();
        events.extend(m.map_update(&message["params"]["update"]));
    }
    let (reason, error) = stop_reason(Some("end_turn"));
    events.extend(m.end_turn(reason, error));
    let all = items(&events);
    let (last, status) = all.last().unwrap();
    assert_eq!(*status, ItemStatus::Completed);
    assert!(matches!(last, ItemPayload::AssistantMessage(a) if a.text == "OK"));
    let ids: std::collections::HashSet<_> = events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Item(i) => Some(i.id),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 1, "one assistant message");
    let usage: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Usage(u) => Some(u.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(usage.last().unwrap().cost_usd, Some(0.349_512_9));
    assert!(matches!(
        events.last(),
        Some(SessionEvent::TurnEnded {
            reason: TurnEndReason::Completed,
            ..
        })
    ));
}

#[test]
fn chunks_accumulate_and_tool_calls_split_messages() {
    let mut m = mapper();
    let mut events = Vec::new();
    for t in ["Let me ", "look."] {
        events.extend(m.map_update(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": t}})));
    }
    events.extend(m.map_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "tc1", "title": "`cargo test`", "kind": "execute",
        "status": "pending", "rawInput": {"command": "cargo test", "description": "Run tests"}
    })));
    events.extend(m.map_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "tc1", "status": "completed",
        "content": [{"type": "content", "content": {"type": "text", "text": "test result: ok"}}]
    })));
    events.extend(m.map_update(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Done."}})));
    let all = items(&events);
    // Message in progress twice, completed once, the command twice, the new message once.
    assert!(matches!(&all[1].0, ItemPayload::AssistantMessage(a) if a.text == "Let me look."));
    assert!(matches!(
        &all[2],
        (ItemPayload::AssistantMessage(_), ItemStatus::Completed)
    ));
    match &all[3].0 {
        ItemPayload::Command(c) => assert_eq!(c.command, "cargo test"),
        other => panic!("{other:?}"),
    }
    match &all[4] {
        (ItemPayload::Command(c), ItemStatus::Completed) => assert_eq!(c.output, "test result: ok"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(&all[5].0, ItemPayload::AssistantMessage(a) if a.text == "Done."));
    let ended = m.end_turn(TurnEndReason::Completed, None);
    assert!(matches!(
        &items(&ended)[0],
        (ItemPayload::AssistantMessage(_), ItemStatus::Completed)
    ));
}

#[test]
fn edits_plans_mcp_tools_and_thoughts() {
    let mut m = mapper();
    let edit = m.map_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "e1", "title": "Edit src/lib.rs", "kind": "edit",
        "status": "in_progress",
        "content": [{"type": "diff", "path": "/w/src/lib.rs", "oldText": "a\nb", "newText": "a\nc\nd"}]
    }));
    match &items(&edit)[0].0 {
        ItemPayload::FileChange(f) => {
            assert_eq!(f.changes[0].path, "/w/src/lib.rs");
            assert_eq!(f.changes[0].change, FileChangeKind::Modify);
            assert_eq!(
                (f.changes[0].lines_added, f.changes[0].lines_removed),
                (3, 2)
            );
            // The worker keeps the diff for the task item view.
            let diff = f.changes[0].diff.as_deref().unwrap();
            assert!(diff.contains("--- a//w/src/lib.rs"), "{diff}");
            assert!(diff.contains("-b\n"), "{diff}");
            assert!(diff.contains("+c\n"), "{diff}");
            assert!(diff.contains("+d"), "{diff}");
            assert!(diff.contains(" a\n"), "{diff}");
        }
        other => panic!("{other:?}"),
    }
    let new_file = m.map_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "e2", "title": "Write a.txt", "kind": "edit",
        "content": [{"type": "diff", "path": "/w/a.txt", "oldText": null, "newText": "x"}]
    }));
    assert!(
        matches!(&items(&new_file)[0].0, ItemPayload::FileChange(f) if f.changes[0].change == FileChangeKind::Add)
    );
    let tool = m.map_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "m1", "title": "mcp__hub__post_update", "kind": "other",
        "rawInput": {"body": "Halfway."}
    }));
    match &items(&tool)[0].0 {
        ItemPayload::ToolCall(t) => {
            assert_eq!(t.server.as_deref(), Some("hub"));
            assert_eq!(t.tool, "post_update");
            assert_eq!(t.arguments["body"], "Halfway.");
        }
        other => panic!("{other:?}"),
    }
    let failed = m.map_update(&json!({"sessionUpdate": "tool_call_update", "toolCallId": "m1", "status": "failed", "rawOutput": "boom"}));
    assert!(
        matches!(&items(&failed)[0], (ItemPayload::ToolCall(t), ItemStatus::Failed) if t.is_error && t.output.as_deref() == Some("boom"))
    );
    let plan = m.map_update(&json!({"sessionUpdate": "plan", "entries": [
        {"content": "Read", "priority": "high", "status": "completed"},
        {"content": "Fix", "priority": "high", "status": "in_progress"}
    ]}));
    assert!(
        matches!(&items(&plan)[0], (ItemPayload::Plan(p), ItemStatus::InProgress) if p.steps.len() == 2)
    );
    let thought = m.map_update(&json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "Hmm"}}));
    assert!(matches!(&items(&thought)[0].0, ItemPayload::Reasoning(r) if r.text == "Hmm"));
    assert!(
        m.map_update(
            &json!({"sessionUpdate": "available_commands_update", "availableCommands": []})
        )
        .is_empty()
    );
}

#[test]
fn permission_requests_map_details_and_options() {
    let mut m = mapper();
    m.map_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "tc9", "title": "`rm -rf build`", "kind": "execute",
        "rawInput": {"command": "rm -rf build"}
    }));
    let opened = m.map_permission(
        RequestId::new("acp-1"),
        &json!({
            "sessionId": "s",
            "toolCall": {"toolCallId": "tc9"},
            "options": [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
            ]
        }),
    );
    assert_eq!(opened.summary, "`rm -rf build`");
    assert!(
        matches!(&opened.body, RequestBody::Approval(ApprovalDetails::Command { command, .. }) if command == "rm -rf build")
    );
    let kinds: Vec<_> = opened.options.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        vec![
            RequestOptionKind::AllowOnce,
            RequestOptionKind::AllowAlways,
            RequestOptionKind::DenyOnce
        ]
    );

    let edit = m.map_permission(RequestId::new("acp-2"), &json!({
        "toolCall": {"toolCallId": "x", "title": "Edit a.rs", "kind": "edit", "locations": [{"path": "/w/a.rs"}]},
        "options": []
    }));
    assert!(
        matches!(&edit.body, RequestBody::Approval(ApprovalDetails::FileChange { paths }) if paths == &vec!["/w/a.rs".to_owned()])
    );
    let tool = m.map_permission(RequestId::new("acp-3"), &json!({
        "toolCall": {"toolCallId": "y", "title": "mcp__github__create_issue", "rawInput": {"title": "Bug"}},
        "options": []
    }));
    // A title is chosen by the agent and the model, so it never names a
    // server. The worker allows a hub tool without a card by the server.
    assert!(
        matches!(&tool.body, RequestBody::Approval(ApprovalDetails::Tool { server: None, tool, .. }) if tool == "mcp__github__create_issue")
    );
    // The tool name that Claude Code sets in `_meta` does name the server.
    let claude = m.map_permission(RequestId::new("acp-4"), &json!({
        "toolCall": {"toolCallId": "z", "title": "Create an issue", "rawInput": {"title": "Bug"},
                     "_meta": {"claudeCode": {"toolName": "mcp__github__create_issue"}}},
        "options": []
    }));
    assert!(
        matches!(&claude.body, RequestBody::Approval(ApprovalDetails::Tool { server: Some(s), tool, .. }) if s == "github" && tool == "create_issue")
    );
}

#[test]
fn command_approvals_always_name_their_folder() {
    let workspace = std::env::temp_dir().join("acp-ws");
    let ws = workspace.to_string_lossy().into_owned();
    let mut m = AcpMapper::new(SessionId::new()).with_workspace(&workspace);
    m.start_turn(TurnId::new("t1"));
    let cwd_of = |m: &AcpMapper, raw: Value| {
        let opened = m.map_permission(
            RequestId::new("acp-1"),
            &json!({
                "toolCall": {"toolCallId": "c", "title": "Run", "kind": "execute", "rawInput": raw},
                "options": []
            }),
        );
        match opened.body {
            RequestBody::Approval(ApprovalDetails::Command { cwd, .. }) => cwd,
            other => panic!("not a command approval: {other:?}"),
        }
    };
    // Claude Code's `Bash` names no folder, so the command runs in the
    // workspace.
    assert_eq!(cwd_of(&m, json!({"command": "ls"})), Some(ws.clone()));
    // OpenCode names it `workdir`, and Gemini CLI `dir_path`. A relative
    // folder is in the workspace.
    let sub = workspace.join("sub").to_string_lossy().into_owned();
    assert_eq!(
        cwd_of(&m, json!({"command": "ls", "workdir": "sub"})),
        Some(sub.clone())
    );
    assert_eq!(
        cwd_of(&m, json!({"command": "ls", "dir_path": &sub})),
        Some(sub.clone())
    );
    assert_eq!(cwd_of(&m, json!({"command": "ls", "cwd": &sub})), Some(sub));
}

#[test]
fn stop_reasons() {
    assert_eq!(
        stop_reason(Some("end_turn")),
        (TurnEndReason::Completed, None)
    );
    assert_eq!(
        stop_reason(Some("cancelled")),
        (TurnEndReason::Interrupted, None)
    );
    // A turn that stops before its work is done fails, with the reason.
    for reason in [
        Some("max_tokens"),
        Some("max_turn_requests"),
        Some("refusal"),
        Some("something_new"),
        None,
    ] {
        let (end, error) = stop_reason(reason);
        assert_eq!(end, TurnEndReason::Failed, "{reason:?}");
        assert!(error.is_some_and(|e| !e.is_empty()), "{reason:?}");
    }
    assert!(
        stop_reason(Some("max_tokens"))
            .1
            .unwrap()
            .contains("token limit")
    );
}

#[test]
fn launch_recipes() {
    let found = |name: &str| -> Option<PathBuf> {
        matches!(name, "npx" | "opencode" | "gemini" | "bunx" | "my-agent")
            .then(|| PathBuf::from(format!("/bin/{name}")))
    };
    let no_npx =
        |name: &str| -> Option<PathBuf> { (name == "bunx").then(|| PathBuf::from("/bin/bunx")) };
    let claude = AccountConfig::default_for(HarnessKind::ClaudeCode);
    let l = launch_recipe(
        HarnessKind::ClaudeCode,
        &claude,
        ApprovalMode::Ask,
        true,
        &found,
    )
    .unwrap();
    assert_eq!(l.program, PathBuf::from("/bin/npx"));
    assert_eq!(l.args, vec!["-y", CLAUDE_AGENT_ACP]);
    let l = launch_recipe(
        HarnessKind::ClaudeCode,
        &claude,
        ApprovalMode::Ask,
        true,
        &no_npx,
    )
    .unwrap();
    assert_eq!(l.args, vec![CLAUDE_AGENT_ACP]);
    assert!(
        launch_recipe(
            HarnessKind::ClaudeCode,
            &claude,
            ApprovalMode::Ask,
            true,
            &|_| { None }
        )
        .is_err()
    );

    let mut open = AccountConfig::default_for(HarnessKind::OpenCode);
    open.args = vec!["--print-logs".into()];
    let l = launch_recipe(
        HarnessKind::OpenCode,
        &open,
        ApprovalMode::AutoEdits,
        true,
        &found,
    )
    .unwrap();
    assert_eq!(l.args, vec!["acp", "--print-logs"]);
    let config: Value = serde_json::from_str(&l.env["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert_eq!(config["permission"]["edit"], "allow");
    assert_eq!(config["permission"]["bash"], "ask");
    // A shell-off worker denies OpenCode's shell tool.
    let l = launch_recipe(
        HarnessKind::OpenCode,
        &open,
        ApprovalMode::AutoEdits,
        false,
        &found,
    )
    .unwrap();
    let config: Value = serde_json::from_str(&l.env["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert_eq!(config["permission"]["bash"], "deny");

    // Below Full access, OpenCode does not load the repo's configuration.
    for key in crate::approval::OPENCODE_NO_PROJECT_CONFIG {
        assert_eq!(l.env.get(key).map(String::as_str), Some("1"), "{key}");
    }
    let full = launch_recipe(
        HarnessKind::OpenCode,
        &open,
        ApprovalMode::FullAccess,
        true,
        &found,
    )
    .unwrap();
    for key in crate::approval::OPENCODE_NO_PROJECT_CONFIG {
        assert!(!full.env.contains_key(key), "{key}");
    }

    // OpenCode Auto asks before commands and fetches. Only Full access
    // allows them with no prompt.
    let auto = launch_recipe(
        HarnessKind::OpenCode,
        &open,
        ApprovalMode::Auto,
        true,
        &found,
    )
    .unwrap();
    let config: Value = serde_json::from_str(&auto.env["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert_eq!(
        config["permission"],
        json!({"edit": "allow", "bash": "ask", "webfetch": "ask"})
    );
    let config: Value = serde_json::from_str(&full.env["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert_eq!(
        config["permission"],
        json!({"edit": "allow", "bash": "allow", "webfetch": "allow"})
    );

    let gemini = AccountConfig::default_for(HarnessKind::GeminiCli);
    let gemini_args = |mode| {
        launch_recipe(HarnessKind::GeminiCli, &gemini, mode, true, &found)
            .unwrap()
            .args
    };
    assert_eq!(
        gemini_args(ApprovalMode::Auto),
        vec!["--experimental-acp", "--approval-mode", "auto_edit"]
    );
    assert_eq!(
        gemini_args(ApprovalMode::FullAccess),
        vec!["--experimental-acp", "--approval-mode", "yolo"]
    );

    let mut generic = AccountConfig::default_for(HarnessKind::Acp);
    assert!(launch_recipe(HarnessKind::Acp, &generic, ApprovalMode::Ask, true, &found).is_err());
    generic.binary = Some("my-agent".into());
    generic.args = vec!["acp".into()];
    let l = launch_recipe(HarnessKind::Acp, &generic, ApprovalMode::Ask, true, &found).unwrap();
    assert_eq!(
        (l.program, l.args),
        (PathBuf::from("/bin/my-agent"), vec!["acp".to_owned()])
    );
}

#[test]
fn mcp_servers_use_http_or_the_bridge() {
    let servers = vec![McpServerSpec {
        name: "hub".into(),
        url: "http://127.0.0.1:5000/mcp/ses_x".into(),
        bearer_token: Some("tok".into()),
        bridge: Some(crate::StdioCommand {
            command: PathBuf::from("/bin/consort"),
            args: vec![
                "mcp-bridge".into(),
                "--url".into(),
                "http://127.0.0.1:5000/mcp/ses_x".into(),
            ],
            env: BTreeMap::from([("CONSORT_MCP_TOKEN".to_owned(), "tok".to_owned())]),
        }),
    }];
    let http = mcp_servers_json(&servers, true);
    assert_eq!(http[0]["type"], "http");
    assert_eq!(
        http[0]["headers"][0],
        json!({"name": "Authorization", "value": "Bearer tok"})
    );
    let stdio = mcp_servers_json(&servers, false);
    assert!(stdio[0].get("type").is_none());
    assert_eq!(stdio[0]["command"], "/bin/consort");
    assert_eq!(
        stdio[0]["env"][0],
        json!({"name": "CONSORT_MCP_TOKEN", "value": "tok"})
    );
}

#[test]
fn models_come_from_config_options() {
    let options = vec![
        json!({"id": "mode", "category": "mode", "type": "select", "currentValue": "default", "options": [{"value": "default", "name": "Default"}]}),
        json!({"id": "model", "category": "model", "type": "select", "currentValue": "opus", "options": [
            {"value": "opus", "name": "Opus"}, {"value": "sonnet", "name": "Sonnet"}
        ]}),
        json!({"id": "effort", "category": "thought_level", "type": "select", "currentValue": "high", "options": [
            {"group": "g", "name": "Levels", "options": [{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]}
        ]}),
    ];
    let models = models_from_config(&options, &BTreeMap::new());
    assert_eq!(models.len(), 2);
    assert_eq!(models[1].id, "sonnet");
    match &models[0].options[0] {
        OptionDescriptor::Select {
            id,
            choices,
            default,
            ..
        } => {
            assert_eq!(id, "effort");
            assert_eq!(choices.len(), 2);
            assert_eq!(default.as_deref(), Some("high"));
        }
        other => panic!("{other:?}"),
    }
    assert!(models_from_config(&[], &BTreeMap::new()).is_empty());
}

/// `claude-agent-acp` 0.99.0 lists `default` first among the effort levels
/// and has it as the current value. OpenCode 2.2.1 lists it last. It is not
/// a level, so it leaves the list, and the provider default is unknown.
/// The levels also differ by model.
#[test]
fn effort_levels_come_per_model_without_the_default_value() {
    let model = json!({"id": "model", "category": "model", "type": "select", "currentValue": "default", "options": [
        {"value": "default", "name": "Default (recommended)"}, {"value": "opus", "name": "Opus 5.5"},
        {"value": "claude-haiku-4-5", "name": "Haiku 4.5"}, {"value": "sonnet", "name": "Sonnet"}
    ]});
    let effort = |levels: &[&str], current: &str| {
        let options: Vec<Value> = levels
            .iter()
            .map(|v| json!({"value": v, "name": v.to_uppercase()}))
            .collect();
        json!({"id": "effort", "category": "thought_level", "type": "select", "currentValue": current, "options": options})
    };
    let first = vec![
        model.clone(),
        effort(&["default", "low", "high", "xhigh", "max"], "default"),
    ];
    let mut per_model = BTreeMap::new();
    per_model.insert("opus".to_owned(), first.clone());
    // Haiku 4.5 has no effort option.
    per_model.insert("claude-haiku-4-5".to_owned(), vec![model.clone()]);
    per_model.insert(
        "sonnet".to_owned(),
        vec![model, effort(&["low", "high", "default"], "high")],
    );
    let models = models_from_config(&first, &per_model);
    let levels = |m: &ModelInfo| match m.options.first() {
        Some(OptionDescriptor::Select {
            choices, default, ..
        }) => Some((
            choices.iter().map(|c| c.value.clone()).collect::<Vec<_>>(),
            default.clone(),
        )),
        _ => None,
    };
    let by_id = |id: &str| models.iter().find(|m| m.id == id).unwrap();
    assert_eq!(
        levels(by_id("opus")),
        Some((
            vec!["low".into(), "high".into(), "xhigh".into(), "max".into()],
            None
        ))
    );
    assert_eq!(levels(by_id("claude-haiku-4-5")), None);
    assert_eq!(
        levels(by_id("sonnet")),
        Some((vec!["low".into(), "high".into()], Some("high".into())))
    );
    // A model that the probe did not reach gets the first model's levels.
    assert_eq!(levels(by_id("default")), levels(by_id("opus")));
}

#[test]
fn auth_label_ignores_email() {
    let params = json!({"authStatus": {"kind": "account", "label": "Claude Max", "account": {"email": "a@b.c"}}});
    assert_eq!(auth_label(&params).as_deref(), Some("Claude Max"));
    assert_eq!(auth_label(&json!({"authStatus": {"kind": "none"}})), None);
}

#[test]
fn recorded_opencode_turn_maps_to_one_message() {
    let text = include_str!("../../tests/fixtures/acp-opencode-ok-turn.jsonl");
    let mut m = mapper();
    let mut events = Vec::new();
    for line in text.lines() {
        let message: Value = serde_json::from_str(line).unwrap();
        events.extend(m.map_update(&message["params"]["update"]));
    }
    let (reason, error) = stop_reason(Some("end_turn"));
    events.extend(m.end_turn(reason, error));
    let all = items(&events);
    assert!(
        matches!(all.last(), Some((ItemPayload::AssistantMessage(a), ItemStatus::Completed)) if a.text.trim() == "OK")
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::Usage(u) if u.cost_usd.is_some()))
    );
}

mod fake_agent {
    use super::*;
    use crate::process::StderrTail;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn spec() -> SessionSpec {
        // An empty workspace, so no CLAUDE.md is in it.
        let workspace = tempfile::tempdir().unwrap().keep();
        SessionSpec {
            session_id: consort_proto::ids::SessionId::new(),
            workspace: workspace.clone(),
            runtime_dir: workspace,
            model: "sonnet".into(),
            options: [("effort".to_owned(), OptionValue::Text("high".into()))]
                .into_iter()
                .collect(),
            approval_mode: ApprovalMode::Ask,
            allow_shell: true,
            coordinator: false,
            system_prompt_append: "You are a coordinator.".into(),
            mcp_servers: vec![McpServerSpec {
                name: "hub".into(),
                url: "http://127.0.0.1:9/mcp/ses".into(),
                bearer_token: Some("tok".into()),
                bridge: None,
            }],
            env: BTreeMap::new(),
            extras_enabled: true,
            skills: None,
            resume: None,
            process_watcher: None,
        }
    }

    /// How the fake agent misbehaves. The default behaves well.
    #[derive(Debug, Clone, Copy, Default)]
    struct Behavior {
        /// `session/set_config_option` for `mode` answers but keeps the
        /// old mode.
        ignore_mode: bool,
        /// The session has no mode option.
        no_mode: bool,
        /// `_session/steering` never answers.
        ignore_steer: bool,
        /// `session/cancel` does not end the turn.
        ignore_cancel: bool,
        /// The mode option offers `bypassPermissions`.
        bypass: bool,
        /// The mode option does not offer `auto`.
        no_auto: bool,
        /// `session/set_config_option` for `mode` fails when the value is
        /// `auto`, like Claude Code does for an account without it.
        refuse_auto: bool,
        /// The first permission request is the one to leave plan mode.
        plan_exit: bool,
        /// After the first permission answer, the agent reports this mode.
        escalate: Option<&'static str>,
        /// A `session/set_config_option` for `mode` after the open fails.
        refuse_reset: bool,
        /// On `session/cancel`, the agent asks for a permission before the
        /// turn ends.
        ask_after_cancel: bool,
        /// The agent never answers.
        silent: bool,
        /// The agent reports `promptCapabilities.image`.
        images: bool,
        /// The model `haiku` has no `auto` mode: choosing it drops `auto`
        /// to `acceptEdits`, and the mode option does not offer `auto`
        /// while it is the model, like the Claude Code adapter.
        no_auto_on_haiku: bool,
        /// Choosing this model fails.
        refuse_model: Option<&'static str>,
    }

    /// The config options of the fake session.
    fn fake_options(mode: &str, behavior: Behavior) -> Vec<Value> {
        let mut options = vec![
            json!({"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "opus",
             "options": [{"value": "opus", "name": "Opus"}, {"value": "sonnet", "name": "Sonnet"}, {"value": "haiku", "name": "Haiku"}]}),
            json!({"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "medium",
             "options": [{"value": "medium", "name": "Medium"}, {"value": "high", "name": "High"}]}),
        ];
        if !behavior.no_mode {
            let mut choices = vec![
                json!({"value": "default", "name": "Default"}),
                json!({"value": "acceptEdits", "name": "Accept edits"}),
                json!({"value": "plan", "name": "Plan"}),
            ];
            if !behavior.no_auto {
                choices.push(json!({"value": "auto", "name": "Auto"}));
            }
            if behavior.bypass {
                choices.push(json!({"value": "bypassPermissions", "name": "Bypass permissions"}));
            }
            options.insert(0, json!({"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": mode,
             "options": choices}));
        }
        options
    }

    /// A scripted ACP agent over an in-memory pipe.
    async fn run_agent(stream: tokio::io::DuplexStream, log: Arc<Mutex<Vec<Value>>>) {
        run_agent_with(stream, log, Behavior::default()).await;
    }

    async fn run_agent_with(
        stream: tokio::io::DuplexStream,
        log: Arc<Mutex<Vec<Value>>>,
        behavior: Behavior,
    ) {
        let mut mode = "acceptEdits".to_owned();
        let mut model_now = "opus".to_owned();
        let mut mode_sets = 0;
        let (read, mut write) = tokio::io::split(stream);
        let mut lines = BufReader::new(read).lines();
        let mut prompts = 0;
        let mut open_prompt: Option<Value> = None;
        if behavior.silent {
            while let Ok(Some(line)) = lines.next_line().await {
                log.lock()
                    .unwrap()
                    .push(serde_json::from_str(&line).unwrap());
            }
            return;
        }
        let send = async |w: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>, v: Value| {
            w.write_all(format!("{v}\n").as_bytes()).await.unwrap();
            w.flush().await.unwrap();
        };
        let update = |text: &str| json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}}});
        while let Ok(Some(line)) = lines.next_line().await {
            let msg: Value = serde_json::from_str(&line).unwrap();
            log.lock().unwrap().push(msg.clone());
            let id = msg.get("id").cloned();
            let reply =
                |result: Value| json!({"jsonrpc": "2.0", "id": id.clone(), "result": result});
            match msg.get("method").and_then(Value::as_str) {
                Some("initialize") => {
                    send(&mut write, reply(json!({
                        "protocolVersion": 1,
                        "agentCapabilities": {"loadSession": true, "mcpCapabilities": {"http": true}, "promptCapabilities": {"image": behavior.images, "embeddedContext": true}},
                        "agentInfo": {"name": "fake", "version": "9.9.9"},
                        "authMethods": [],
                        "_meta": {"steering": {"supported": true}}
                    }))).await;
                }
                Some("session/new") => {
                    send(&mut write, reply(json!({"sessionId": "s1", "configOptions": fake_options(&mode, behavior)}))).await;
                }
                Some("session/load") => {
                    send(
                        &mut write,
                        reply(json!({"configOptions": fake_options(&mode, behavior)})),
                    )
                    .await;
                }
                Some("session/set_config_option") => {
                    if msg["params"]["configId"] == "model" {
                        let value = msg["params"]["value"].as_str().unwrap_or_default();
                        if behavior.refuse_model == Some(value) {
                            send(&mut write, json!({"jsonrpc": "2.0", "id": id.clone(), "error": {"code": -32602, "message": "Model is not available"}})).await;
                            continue;
                        }
                        model_now = value.to_owned();
                        if behavior.no_auto_on_haiku && model_now == "haiku" && mode == "auto" {
                            mode = "acceptEdits".to_owned();
                        }
                    }
                    let behavior = Behavior {
                        no_auto: behavior.no_auto
                            || (behavior.no_auto_on_haiku && model_now == "haiku"),
                        ..behavior
                    };
                    if msg["params"]["configId"] == "mode" {
                        if behavior.no_auto_on_haiku
                            && model_now == "haiku"
                            && msg["params"]["value"] == "auto"
                        {
                            send(&mut write, json!({"jsonrpc": "2.0", "id": id.clone(), "error": {"code": -32602, "message": "Auto mode is not available"}})).await;
                            continue;
                        }
                        mode_sets += 1;
                        if behavior.refuse_auto && msg["params"]["value"] == "auto" {
                            send(&mut write, json!({"jsonrpc": "2.0", "id": id.clone(), "error": {"code": -32602, "message": "Auto mode is not available"}})).await;
                            continue;
                        }
                        if behavior.refuse_reset && mode_sets > 1 {
                            send(&mut write, json!({"jsonrpc": "2.0", "id": id.clone(), "error": {"code": -32603, "message": "Internal error"}})).await;
                            continue;
                        }
                    }
                    if msg["params"]["configId"] == "model" && msg["params"]["value"] == "haiku" {
                        // Haiku has no effort option in the fake agent.
                        let options: Vec<Value> = fake_options(&mode, behavior)
                            .into_iter()
                            .filter(|o| o["id"] != "effort")
                            .collect();
                        send(&mut write, reply(json!({"configOptions": options}))).await;
                        continue;
                    }
                    if msg["params"]["configId"] == "mode" && !behavior.ignore_mode {
                        mode = msg["params"]["value"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned();
                    }
                    send(
                        &mut write,
                        reply(json!({"configOptions": fake_options(&mode, behavior)})),
                    )
                    .await
                }
                Some("session/prompt") => {
                    // A prompt that the test wants to fail. -32000 is the ACP
                    // code for "authentication required".
                    let text = msg["params"]["prompt"][0]["text"].as_str().unwrap_or("");
                    let failure = match text {
                        "Expire" => Some((-32000, "Authentication required")),
                        "Break" => Some((-32603, "Internal error")),
                        // What `claude-agent-acp` sends for an expired
                        // OAuth token: an internal error that carries the
                        // API error in its data.
                        "Expire401" => Some((-32603, "Internal error")),
                        // A rate limit, as the API error in the data.
                        "Limit" => Some((-32603, "Internal error")),
                        _ => None,
                    };
                    if let Some((code, message)) = failure {
                        let mut error = json!({"code": code, "message": message});
                        if text == "Limit" {
                            error["data"] = json!({
                                "details": "API Error: 429 {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"Weekly limit reached\"}} {\"resetsAt\": 1760000500}"
                            });
                        }
                        if text == "Expire401" {
                            error["data"] = json!({
                                "kind": "authentication_failed",
                                "details": "Failed to authenticate. API Error: 401 {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"OAuth token has expired. Please obtain a new token or refresh your existing token.\"}}"
                            });
                        }
                        send(
                            &mut write,
                            json!({"jsonrpc": "2.0", "id": id.clone(), "error": error}),
                        )
                        .await;
                        continue;
                    }
                    prompts += 1;
                    open_prompt = id.clone();
                    if prompts == 1 && behavior.plan_exit {
                        // The options of `claude-agent-acp` 0.99.0 to leave
                        // plan mode.
                        send(&mut write, json!({"jsonrpc": "2.0", "id": "perm-1", "method": "session/request_permission", "params": {
                            "sessionId": "s1",
                            "toolCall": {"toolCallId": "tc1", "title": "Ready to code?", "kind": "switch_mode", "rawInput": {"plan": "Do it."}},
                            "options": [
                                {"optionId": "exit-plan-clear-auto", "name": "Yes, clear context and use auto mode", "kind": "allow_always"},
                                {"optionId": "exit-plan-auto", "name": "Yes, and use auto mode", "kind": "allow_always"},
                                {"optionId": "exit-plan-bypass", "name": "Yes, and bypass permissions", "kind": "allow_always"},
                                {"optionId": "exit-plan-accept-edits", "name": "Yes, auto-accept edits", "kind": "allow_always"},
                                {"optionId": "exit-plan-default", "name": "Yes, manually approve edits", "kind": "allow_once"},
                                {"optionId": "reject", "name": "No, keep planning", "kind": "reject_once"}
                            ]
                        }})).await;
                    } else if prompts == 1 {
                        send(&mut write, update("Checking. ")).await;
                        send(&mut write, json!({"jsonrpc": "2.0", "id": "perm-1", "method": "session/request_permission", "params": {
                            "sessionId": "s1",
                            "toolCall": {"toolCallId": "tc1", "title": "`git push`", "kind": "execute", "rawInput": {"command": "git push"}},
                            "options": [
                                {"optionId": "yes", "name": "Allow", "kind": "allow_once"},
                                {"optionId": "no", "name": "Reject", "kind": "reject_once"}
                            ]
                        }})).await;
                    }
                }
                Some("_session/steering") => {
                    if behavior.ignore_steer {
                        // No answer, but the turn goes on.
                        send(&mut write, update("Still working. ")).await;
                    } else {
                        send(&mut write, reply(json!({"outcome": "injected"}))).await
                    }
                }
                Some("session/cancel") if behavior.ignore_cancel => {}
                Some("session/cancel") if behavior.ask_after_cancel => {
                    send(&mut write, json!({"jsonrpc": "2.0", "id": "perm-2", "method": "session/request_permission", "params": {
                        "sessionId": "s1",
                        "toolCall": {"toolCallId": "tc2", "title": "`rm -rf /`", "kind": "execute", "rawInput": {"command": "rm -rf /"}},
                        "options": [
                            {"optionId": "yes", "name": "Allow", "kind": "allow_once"},
                            {"optionId": "no", "name": "Reject", "kind": "reject_once"}
                        ]
                    }})).await;
                }
                None if msg.get("id") == Some(&json!("perm-2")) => {
                    if let Some(pid) = open_prompt.take() {
                        send(&mut write, json!({"jsonrpc": "2.0", "id": pid, "result": {"stopReason": "cancelled"}})).await;
                    }
                }
                Some("session/cancel") => {
                    if let Some(pid) = open_prompt.take() {
                        send(&mut write, json!({"jsonrpc": "2.0", "id": pid, "result": {"stopReason": "cancelled"}})).await;
                    }
                }
                None if msg.get("id") == Some(&json!("perm-1")) => {
                    let chosen = msg["result"]["outcome"]["optionId"]
                        .as_str()
                        .unwrap_or("none")
                        .to_owned();
                    if let Some(higher) = behavior.escalate {
                        mode = higher.to_owned();
                        send(&mut write, json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1",
                            "update": {"sessionUpdate": "current_mode_update", "currentModeId": higher}}})).await;
                        send(&mut write, json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1",
                            "update": {"sessionUpdate": "config_option_update", "configOptions": fake_options(&mode, behavior)}}})).await;
                    }
                    send(&mut write, update(&format!("Chose {chosen}."))).await;
                    if let Some(pid) = open_prompt.take() {
                        send(&mut write, json!({"jsonrpc": "2.0", "id": pid, "result": {"stopReason": "end_turn"}})).await;
                    }
                }
                _ => {}
            }
        }
    }

    async fn next_matching(
        events: &mut EventReceiver,
        f: impl Fn(&SessionEvent) -> bool,
    ) -> SessionEvent {
        loop {
            let e = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("an event in time")
                .expect("events stay open");
            if f(&e) {
                return e;
            }
        }
    }

    #[tokio::test]
    async fn session_configures_round_trips_an_approval_steers_and_interrupts() {
        let (client_side, agent_side) = tokio::io::duplex(1 << 16);
        let log = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(run_agent(agent_side, log.clone()));
        let (read, write) = tokio::io::split(client_side);
        let (session, mut events, ready) = connect(
            HarnessKind::ClaudeCode,
            spec(),
            write,
            read,
            None,
            StderrTail::empty(),
        )
        .await
        .unwrap();
        assert_eq!(ready.agent_version.as_deref(), Some("9.9.9"));
        assert!(
            ready.capabilities.steer && ready.capabilities.resume && ready.capabilities.mcp_http
        );
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        assert_eq!(session.native_ref(), Some(NativeRef::new("s1")));

        {
            let log = log.lock().unwrap();
            let new = log.iter().find(|m| m["method"] == "session/new").unwrap();
            assert_eq!(new["params"]["mcpServers"][0]["type"], "http");
            assert_eq!(
                new["params"]["mcpServers"][0]["headers"][0]["value"],
                "Bearer tok"
            );
            assert_eq!(
                new["params"]["_meta"]["systemPrompt"]["append"],
                "You are a coordinator."
            );
            let sets: Vec<(String, String)> = log
                .iter()
                .filter(|m| m["method"] == "session/set_config_option")
                .map(|m| {
                    (
                        m["params"]["configId"].as_str().unwrap().to_owned(),
                        m["params"]["value"].as_str().unwrap().to_owned(),
                    )
                })
                .collect();
            assert!(
                sets.contains(&("mode".into(), "default".into())),
                "Ask maps to the default mode: {sets:?}"
            );
            assert!(sets.contains(&("model".into(), "sonnet".into())));
            assert!(sets.contains(&("effort".into(), "high".into())));
        }

        let turn = session
            .turn(TurnInput {
                text: "Push it".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        let SessionEvent::RequestOpened(opened) =
            next_matching(&mut events, |e| matches!(e, SessionEvent::RequestOpened(_))).await
        else {
            unreachable!()
        };
        assert!(
            matches!(&opened.body, consort_proto::worker_link::RequestBody::Approval(ApprovalDetails::Command { command, .. }) if command == "git push")
        );
        assert!(
            session
                .respond(
                    RequestId::new("nope"),
                    Answer {
                        option_id: Some("yes".into()),
                        text: None
                    }
                )
                .await
                .is_err()
        );
        assert!(
            session
                .respond(
                    opened.request_id.clone(),
                    Answer {
                        option_id: Some("maybe".into()),
                        text: None
                    }
                )
                .await
                .is_err()
        );
        session
            .respond(
                opened.request_id.clone(),
                Answer {
                    option_id: Some("yes".into()),
                    text: None,
                },
            )
            .await
            .unwrap();
        let closed = next_matching(&mut events, |e| {
            matches!(e, SessionEvent::RequestClosed { .. })
        })
        .await;
        assert!(matches!(
            closed,
            SessionEvent::RequestClosed {
                outcome: consort_proto::worker_link::RequestOutcome::Allowed,
                ..
            }
        ));
        let mut text = String::new();
        loop {
            match next_matching(&mut events, |e| {
                matches!(e, SessionEvent::Item(_) | SessionEvent::TurnEnded { .. })
            })
            .await
            {
                SessionEvent::Item(i) => {
                    if let ItemPayload::AssistantMessage(m) = &i.payload {
                        text = m.text.clone();
                    }
                }
                SessionEvent::TurnEnded {
                    turn_id, reason, ..
                } => {
                    assert_eq!(turn_id, turn);
                    assert_eq!(reason, TurnEndReason::Completed);
                    break;
                }
                _ => unreachable!(),
            }
        }
        assert_eq!(text, "Checking. Chose yes.");

        let second = session
            .turn(TurnInput {
                text: "Wait".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        assert!(
            session.turn(TurnInput::default()).await.is_err(),
            "one turn at a time"
        );
        session
            .steer(TurnInput {
                text: "Also this".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        session.interrupt().await.unwrap();
        let ended =
            next_matching(&mut events, |e| matches!(e, SessionEvent::TurnEnded { .. })).await;
        assert!(
            matches!(ended, SessionEvent::TurnEnded { reason: TurnEndReason::Interrupted, turn_id, .. } if turn_id == second)
        );
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|m| m["method"] == "_session/steering"
                    && m["params"]["prompt"][0]["text"] == "Also this")
        );
        session.close().await.unwrap();
    }

    /// Plays one turn and returns the events from `TurnStarted` up to and
    /// including `TurnEnded`.
    async fn events_of_turn(
        session: &mut Box<dyn HarnessSession>,
        events: &mut EventReceiver,
        text: &str,
    ) -> Vec<SessionEvent> {
        session
            .turn(TurnInput {
                text: text.into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        let mut out = Vec::new();
        loop {
            let event = next_matching(events, |_| true).await;
            let ended = matches!(event, SessionEvent::TurnEnded { .. });
            out.push(event);
            if ended {
                return out;
            }
        }
    }

    #[tokio::test]
    async fn an_expired_claude_login_that_the_probe_cannot_see_ends_the_turn_with_login_expired() {
        let (client_side, agent_side) = tokio::io::duplex(1 << 16);
        let log = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(run_agent(agent_side, log));
        let (read, write) = tokio::io::split(client_side);
        let (session, mut events, _ready) = connect(
            HarnessKind::ClaudeCode,
            spec(),
            write,
            read,
            None,
            StderrTail::empty(),
        )
        .await
        .unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        let turn = events_of_turn(&mut session, &mut events, "Expire401").await;
        let at = turn
            .iter()
            .position(|e| {
                matches!(e, SessionEvent::Error { code, message }
                    if code == crate::LOGIN_EXPIRED_ERROR_CODE && message == crate::LOGIN_EXPIRED_MESSAGE)
            })
            .expect("a login_expired error event");
        assert_eq!(at, turn.len() - 2, "it comes right before TurnEnded");
        assert!(matches!(
            turn.last(),
            Some(SessionEvent::TurnEnded { reason: TurnEndReason::Failed, error: Some(error), .. })
                if error == crate::LOGIN_EXPIRED_MESSAGE
        ));
        // It is not a sign-out: that event is for a login that is gone.
        assert!(!turn.iter().any(
            |e| matches!(e, SessionEvent::Error { code, .. } if code == crate::SIGNED_OUT_ERROR_CODE)
        ));
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn a_prompt_that_needs_a_login_reports_signed_out_and_the_next_step() {
        let (client_side, agent_side) = tokio::io::duplex(1 << 16);
        let log = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(run_agent(agent_side, log));
        let (read, write) = tokio::io::split(client_side);
        let (session, mut events, _ready) = connect(
            HarnessKind::ClaudeCode,
            spec(),
            write,
            read,
            None,
            StderrTail::empty(),
        )
        .await
        .unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        let hint = "Authentication required. Run `claude` on this machine and sign in with /login.";

        let turn = events_of_turn(&mut session, &mut events, "Expire").await;
        let signed_out = turn
            .iter()
            .position(|e| {
                matches!(e, SessionEvent::Error { code, message }
                    if code == crate::SIGNED_OUT_ERROR_CODE && message == hint)
            })
            .expect("a signed_out error event");
        assert_eq!(
            signed_out,
            turn.len() - 2,
            "it comes right before TurnEnded"
        );
        assert!(matches!(
            turn.last(),
            Some(SessionEvent::TurnEnded { reason: TurnEndReason::Failed, error: Some(error), .. })
                if error == hint
        ));

        // Another failure is not a sign-out. The turn fails with its own text.
        let turn = events_of_turn(&mut session, &mut events, "Break").await;
        assert!(
            !turn
                .iter()
                .any(|e| matches!(e, SessionEvent::Error { code, .. } if code == crate::SIGNED_OUT_ERROR_CODE))
        );
        assert!(matches!(
            turn.last(),
            Some(SessionEvent::TurnEnded { reason: TurnEndReason::Failed, error: Some(error), error_kind: None, resets_at: None, .. })
                if error.contains("Internal error")
        ));

        // An expired sign-in has its kind.
        let turn = events_of_turn(&mut session, &mut events, "Expire").await;
        assert!(matches!(
            turn.last(),
            Some(SessionEvent::TurnEnded { error_kind: Some(kind), .. })
                if *kind == consort_proto::model::TurnErrorKind::ExpiredSignIn
        ));

        // A rate limit that the API error names has its kind and its reset
        // time, read from the data of the error.
        let turn = events_of_turn(&mut session, &mut events, "Limit").await;
        assert!(matches!(
            turn.last(),
            Some(SessionEvent::TurnEnded { reason: TurnEndReason::Failed, error_kind: Some(kind), resets_at: Some(at), .. })
                if *kind == consort_proto::model::TurnErrorKind::RateLimit
                    && at.as_millis() == 1_760_000_500_000
        ));
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn other_agents_get_the_system_prompt_in_the_first_turn() {
        let (client_side, agent_side) = tokio::io::duplex(1 << 16);
        let log = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(run_agent(agent_side, log.clone()));
        let (read, write) = tokio::io::split(client_side);
        let mut s = spec();
        s.approval_mode = ApprovalMode::FullAccess;
        let (session, _events, ready) = connect(
            HarnessKind::OpenCode,
            s,
            write,
            read,
            None,
            StderrTail::empty(),
        )
        .await
        .unwrap();
        assert!(!ready.capabilities.steer, "only Claude steers");
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        assert!(matches!(
            session.steer(TurnInput::default()).await,
            Err(HarnessError::Unsupported(_))
        ));
        session
            .turn(TurnInput {
                text: "Hello".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let log = log.lock().unwrap();
        let new = log.iter().find(|m| m["method"] == "session/new").unwrap();
        assert!(new["params"].get("_meta").is_none());
        assert!(
            !log.iter()
                .any(|m| m["method"] == "session/set_config_option"
                    && m["params"]["configId"] == "mode"),
            "OpenCode modes come from OPENCODE_CONFIG_CONTENT"
        );
        let prompt = log
            .iter()
            .find(|m| m["method"] == "session/prompt")
            .unwrap();
        assert_eq!(
            prompt["params"]["prompt"][0]["text"],
            "You are a coordinator.\n\nHello"
        );
    }

    async fn connect_with(
        kind: HarnessKind,
        spec: SessionSpec,
        behavior: Behavior,
    ) -> (
        Result<(AcpSession, EventReceiver, Ready), HarnessError>,
        Arc<Mutex<Vec<Value>>>,
    ) {
        let (client_side, agent_side) = tokio::io::duplex(1 << 16);
        let log = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(run_agent_with(agent_side, log.clone(), behavior));
        let (read, write) = tokio::io::split(client_side);
        let result = connect(kind, spec, write, read, None, StderrTail::empty()).await;
        (result, log)
    }

    /// The `model`, `effort` and `mode` values that were set, in order, as
    /// `model=x`.
    fn config_sets(log: &Arc<Mutex<Vec<Value>>>) -> Vec<String> {
        log.lock()
            .unwrap()
            .iter()
            .filter(|m| m["method"] == "session/set_config_option")
            .map(|m| {
                format!(
                    "{}={}",
                    m["params"]["configId"].as_str().unwrap(),
                    m["params"]["value"].as_str().unwrap()
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn the_open_sets_the_model_before_the_mode() {
        let mut auto = spec();
        auto.approval_mode = ApprovalMode::Auto;
        let (result, log) = connect_with(HarnessKind::ClaudeCode, auto, Behavior::default()).await;
        let (session, _events, _ready) = result.unwrap();
        let session: Box<dyn HarnessSession> = Box::new(session);
        assert_eq!(
            config_sets(&log),
            vec!["model=sonnet", "effort=high", "mode=auto"]
        );
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn a_model_without_auto_opens_in_accept_edits_and_does_not_fail() {
        // The model comes first, so the adapter has dropped `auto` before the
        // mode is set. The open asks for nothing it cannot get.
        let mut auto = spec();
        auto.approval_mode = ApprovalMode::Auto;
        auto.model = "haiku".into();
        let behavior = Behavior {
            no_auto_on_haiku: true,
            ..Behavior::default()
        };
        let (result, log) = connect_with(HarnessKind::ClaudeCode, auto, behavior).await;
        let (session, _events, _ready) = result.unwrap();
        let session: Box<dyn HarnessSession> = Box::new(session);
        assert_eq!(config_sets(&log), vec!["model=haiku"]);
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn a_model_change_in_the_session_sets_the_mode_again() {
        let mut auto = spec();
        auto.approval_mode = ApprovalMode::Auto;
        let behavior = Behavior {
            no_auto_on_haiku: true,
            ..Behavior::default()
        };
        let (result, log) = connect_with(HarnessKind::ClaudeCode, auto, behavior).await;
        let (session, _events, _ready) = result.unwrap();
        assert!(session.capabilities().switch_model);
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        assert_eq!(
            config_sets(&log),
            vec!["model=sonnet", "effort=high", "mode=auto"]
        );

        // To a model without `auto`: the adapter drops the mode and refuses
        // `auto`. The change works, and the session goes on in
        // `acceptEdits`.
        session
            .set_model("haiku", &ModelOptions::new())
            .await
            .unwrap();
        let sets = config_sets(&log);
        assert_eq!(sets[3..], ["model=haiku", "mode=auto"], "{sets:?}");

        // Back to a model with `auto`: the mode is set again, and now it
        // sticks. Without this, the session would stay in `acceptEdits`.
        let options: ModelOptions = [("effort".to_owned(), OptionValue::Text("high".into()))]
            .into_iter()
            .collect();
        session.set_model("sonnet", &options).await.unwrap();
        let sets = config_sets(&log);
        assert_eq!(
            sets[5..],
            ["model=sonnet", "effort=high", "mode=auto"],
            "{sets:?}"
        );
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn a_model_change_that_the_agent_refuses_fails_and_the_session_stays_open() {
        let behavior = Behavior {
            refuse_model: Some("haiku"),
            ..Behavior::default()
        };
        let (result, _log) = connect_with(HarnessKind::ClaudeCode, spec(), behavior).await;
        let (session, _events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        let err = session
            .set_model("haiku", &ModelOptions::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("haiku"), "{err}");
        // A model that the option does not offer fails too.
        let err = session
            .set_model("gpt-9", &ModelOptions::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("gpt-9"), "{err}");
        session.close().await.unwrap();
    }

    #[test]
    fn only_the_checked_agents_report_a_model_switch() {
        assert!(static_capabilities(HarnessKind::ClaudeCode).switch_model);
        assert!(static_capabilities(HarnessKind::OpenCode).switch_model);
        assert!(!static_capabilities(HarnessKind::GeminiCli).switch_model);
        assert!(!static_capabilities(HarnessKind::Acp).switch_model);
    }

    #[tokio::test]
    async fn a_claude_mode_that_does_not_stick_fails_the_open() {
        let behavior = Behavior {
            ignore_mode: true,
            ..Behavior::default()
        };
        let (result, _log) = connect_with(HarnessKind::ClaudeCode, spec(), behavior).await;
        let Err(err) = result else {
            panic!("the open must fail when the mode stays acceptEdits")
        };
        assert!(err.to_string().contains("permission mode"), "{err}");
        assert!(err.to_string().contains("acceptEdits"), "{err}");
    }

    #[tokio::test]
    async fn a_claude_session_without_a_mode_fails_the_open() {
        let behavior = Behavior {
            no_mode: true,
            ..Behavior::default()
        };
        let (result, _log) = connect_with(HarnessKind::ClaudeCode, spec(), behavior).await;
        let Err(err) = result else {
            panic!("the open must fail when the agent has no mode")
        };
        assert!(err.to_string().contains("no permission mode"), "{err}");
    }

    #[tokio::test]
    async fn session_load_sends_the_system_prompt_and_the_limits() {
        let mut s = spec();
        s.resume = Some(NativeRef::new("s1"));
        s.allow_shell = false;
        let (result, log) = connect_with(HarnessKind::ClaudeCode, s, Behavior::default()).await;
        let (session, _events, ready) = result.unwrap();
        assert_eq!(ready.native_ref, NativeRef::new("s1"));
        {
            let log = log.lock().unwrap();
            let load = log.iter().find(|m| m["method"] == "session/load").unwrap();
            let meta = &load["params"]["_meta"];
            assert_eq!(meta["systemPrompt"]["append"], "You are a coordinator.");
            assert_eq!(
                meta["claudeCode"]["options"]["settingSources"],
                json!(["user"])
            );
            assert_eq!(
                meta["claudeCode"]["options"]["disallowedTools"],
                json!(["Bash", "PowerShell", "Monitor"])
            );
            assert!(
                log.iter()
                    .any(|m| m["method"] == "session/set_config_option"
                        && m["params"]["configId"] == "mode"
                        && m["params"]["value"] == "default"),
                "the resumed session gets its mode again"
            );
        }
        Box::new(session).close().await.unwrap();
    }

    #[tokio::test]
    async fn extras_off_and_skills_reach_session_new_and_session_load() {
        let dir = tempfile::tempdir().unwrap();
        let skills =
            crate::skills::SessionSkills::new(dir.path().join("skills"), vec!["pdf-forms".into()]);
        let plugin = crate::skills::claude_plugin_dir(&skills)
            .to_string_lossy()
            .into_owned();
        for resume in [None, Some(NativeRef::new("s1"))] {
            let mut s = spec();
            s.extras_enabled = false;
            s.skills = Some(skills.clone());
            s.resume = resume.clone();
            let (result, log) = connect_with(HarnessKind::ClaudeCode, s, Behavior::default()).await;
            let (session, _events, _ready) = result.unwrap();
            {
                let log = log.lock().unwrap();
                let method = if resume.is_some() {
                    "session/load"
                } else {
                    "session/new"
                };
                let request = log.iter().find(|m| m["method"] == method).unwrap();
                let options = &request["params"]["_meta"]["claudeCode"]["options"];
                assert_eq!(
                    options["settings"],
                    json!({
                        "disableClaudeAiConnectors": true,
                        "syncClaudeAiSkills": false,
                        "syncClaudeAiPlugins": false
                    }),
                    "{method}"
                );
                assert_eq!(
                    options["env"],
                    json!({"ENABLE_CLAUDEAI_MCP_SERVERS": "false"}),
                    "{method}"
                );
                assert_eq!(
                    options["plugins"],
                    json!([{"type": "local", "path": plugin}]),
                    "{method}"
                );
            }
            Box::new(session).close().await.unwrap();
        }
    }

    #[tokio::test]
    async fn extras_on_send_no_settings_or_plugins() {
        let (result, log) =
            connect_with(HarnessKind::ClaudeCode, spec(), Behavior::default()).await;
        let (session, _events, _ready) = result.unwrap();
        {
            let log = log.lock().unwrap();
            let new = log.iter().find(|m| m["method"] == "session/new").unwrap();
            let options = &new["params"]["_meta"]["claudeCode"]["options"];
            assert!(options.get("settings").is_none(), "{options}");
            assert!(options.get("env").is_none(), "{options}");
            assert!(options.get("plugins").is_none(), "{options}");
        }
        Box::new(session).close().await.unwrap();
    }

    #[tokio::test]
    async fn a_steer_without_an_answer_does_not_block_the_session() {
        let behavior = Behavior {
            ignore_steer: true,
            ..Behavior::default()
        };
        let (result, log) = connect_with(HarnessKind::ClaudeCode, spec(), behavior).await;
        let (session, mut events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        // The first turn of the fake agent asks for a permission.
        session
            .turn(TurnInput {
                text: "Push it".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        let SessionEvent::RequestOpened(opened) =
            next_matching(&mut events, |e| matches!(e, SessionEvent::RequestOpened(_))).await
        else {
            unreachable!()
        };
        // While the steer waits, the agent's updates still arrive.
        let (steered, update) = tokio::join!(
            session.steer(TurnInput {
                text: "Also this".into(),
                attachments: vec![],
            }),
            next_matching(&mut events, |e| matches!(
                e,
                SessionEvent::Item(i) if matches!(&i.payload, ItemPayload::AssistantMessage(m) if m.text.contains("Still working"))
            ))
        );
        assert!(
            matches!(steered, Err(HarnessError::Timeout(_))),
            "{steered:?}"
        );
        assert!(matches!(update, SessionEvent::Item(_)));
        // The approval still works after the steer.
        session
            .respond(
                opened.request_id,
                Answer {
                    option_id: Some("yes".into()),
                    text: None,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            next_matching(&mut events, |e| matches!(e, SessionEvent::TurnEnded { .. })).await,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
                ..
            }
        ));
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|m| m["method"] == "_session/steering")
        );
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn an_ignored_cancel_loses_the_session() {
        let behavior = Behavior {
            ignore_cancel: true,
            ..Behavior::default()
        };
        let (result, _log) = connect_with(HarnessKind::ClaudeCode, spec(), behavior).await;
        let (session, mut events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        // The first turn of the fake agent asks for a permission. The second
        // turn runs until a cancel.
        session
            .turn(TurnInput {
                text: "Push it".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        let SessionEvent::RequestOpened(opened) =
            next_matching(&mut events, |e| matches!(e, SessionEvent::RequestOpened(_))).await
        else {
            unreachable!()
        };
        session
            .respond(
                opened.request_id,
                Answer {
                    option_id: Some("yes".into()),
                    text: None,
                },
            )
            .await
            .unwrap();
        next_matching(&mut events, |e| matches!(e, SessionEvent::TurnEnded { .. })).await;
        session
            .turn(TurnInput {
                text: "Wait".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        session.interrupt().await.unwrap();
        assert!(matches!(
            next_matching(&mut events, |e| matches!(e, SessionEvent::TurnEnded { .. })).await,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Failed,
                ..
            }
        ));
        assert!(matches!(
            next_matching(&mut events, |e| matches!(e, SessionEvent::Error { .. })).await,
            SessionEvent::Error { code, .. } if code == "harness_unresponsive"
        ));
        assert!(matches!(
            next_matching(&mut events, |e| matches!(e, SessionEvent::State { .. })).await,
            SessionEvent::State {
                state: SessionState::Lost
            }
        ));
    }

    #[test]
    fn agents_start_in_the_runtime_folder() {
        let mut s = spec();
        s.runtime_dir = PathBuf::from("/runtime/ses_1");
        let account = AccountConfig::default_for(HarnessKind::ClaudeCode);
        let process = agent_process_spec(HarnessKind::ClaudeCode, &account, &s, &|name| {
            Some(PathBuf::from(format!("/bin/{name}")))
        })
        .unwrap();
        assert_eq!(process.cwd.as_deref(), Some(Path::new("/runtime/ses_1")));
        assert_ne!(process.cwd.as_deref(), Some(s.workspace.as_path()));
    }

    /// The first turn of the fake agent with `plan_exit` asks to leave plan
    /// mode. Returns the request.
    async fn plan_exit_request(
        mode: ApprovalMode,
    ) -> (
        Box<dyn HarnessSession>,
        EventReceiver,
        consort_proto::worker_link::RequestOpened,
    ) {
        let mut s = spec();
        s.approval_mode = mode;
        let behavior = Behavior {
            plan_exit: true,
            bypass: true,
            ..Behavior::default()
        };
        let (result, _log) = connect_with(HarnessKind::ClaudeCode, s, behavior).await;
        let (session, mut events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        session
            .turn(TurnInput {
                text: "Plan it".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        let SessionEvent::RequestOpened(opened) =
            next_matching(&mut events, |e| matches!(e, SessionEvent::RequestOpened(_))).await
        else {
            unreachable!()
        };
        (session, events, opened)
    }

    #[tokio::test]
    async fn options_that_raise_the_mode_are_not_offered() {
        let ids = |opened: &consort_proto::worker_link::RequestOpened| -> Vec<String> {
            opened.options.iter().map(|o| o.id.clone()).collect()
        };
        // Ask: only "manually approve edits" and "keep planning" stay. The
        // client's "Allow for task" picks the first allow-always option, so
        // it can no longer move the session to auto mode.
        let (mut session, mut events, opened) = plan_exit_request(ApprovalMode::Ask).await;
        assert_eq!(ids(&opened), vec!["exit-plan-default", "reject"]);
        assert!(matches!(
            session
                .respond(
                    opened.request_id.clone(),
                    Answer {
                        option_id: Some("exit-plan-auto".into()),
                        text: None
                    }
                )
                .await,
            Err(HarnessError::InvalidAnswer(_))
        ));
        session
            .respond(
                opened.request_id,
                Answer {
                    option_id: Some("exit-plan-default".into()),
                    text: None,
                },
            )
            .await
            .unwrap();
        next_matching(&mut events, |e| matches!(e, SessionEvent::TurnEnded { .. })).await;
        session.close().await.unwrap();

        // Auto edits maps to `acceptEdits`, so auto mode and bypass go, and
        // "auto-accept edits" stays.
        let (session, _events, opened) = plan_exit_request(ApprovalMode::AutoEdits).await;
        assert_eq!(
            ids(&opened),
            vec!["exit-plan-accept-edits", "exit-plan-default", "reject"]
        );
        session.close().await.unwrap();

        // Auto maps to Claude Code's `auto` mode, so auto mode stays and
        // only bypass goes.
        let (session, _events, opened) = plan_exit_request(ApprovalMode::Auto).await;
        assert_eq!(
            ids(&opened),
            vec![
                "exit-plan-clear-auto",
                "exit-plan-auto",
                "exit-plan-accept-edits",
                "exit-plan-default",
                "reject"
            ]
        );
        session.close().await.unwrap();

        // Full access keeps every option.
        let (session, _events, opened) = plan_exit_request(ApprovalMode::FullAccess).await;
        assert_eq!(opened.options.len(), 6);
        session.close().await.unwrap();
    }

    /// Plays the first turn of the fake agent, which asks for a permission,
    /// and allows it.
    async fn allow_first_request(
        session: &mut Box<dyn HarnessSession>,
        events: &mut EventReceiver,
    ) {
        session
            .turn(TurnInput {
                text: "Push it".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        let SessionEvent::RequestOpened(opened) =
            next_matching(events, |e| matches!(e, SessionEvent::RequestOpened(_))).await
        else {
            unreachable!()
        };
        session
            .respond(
                opened.request_id,
                Answer {
                    option_id: Some("yes".into()),
                    text: None,
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_mode_above_the_limit_is_set_back_at_once() {
        let behavior = Behavior {
            escalate: Some("auto"),
            ..Behavior::default()
        };
        let (result, log) = connect_with(HarnessKind::ClaudeCode, spec(), behavior).await;
        let (session, mut events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        allow_first_request(&mut session, &mut events).await;
        let reset = next_matching(&mut events, |e| matches!(e, SessionEvent::Error { .. })).await;
        assert!(
            matches!(&reset, SessionEvent::Error { code, message } if code == "mode_reset" && message.contains("`auto`") && message.contains("`default`")),
            "{reset:?}"
        );
        let sets: Vec<String> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|m| {
                m["method"] == "session/set_config_option" && m["params"]["configId"] == "mode"
            })
            .map(|m| m["params"]["value"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(sets, vec!["default", "default"], "the open, then the reset");
        // The session goes on.
        assert!(
            session
                .turn(TurnInput {
                    text: "Next".into(),
                    attachments: vec![],
                })
                .await
                .is_ok()
        );
        session.close().await.unwrap();
    }

    /// The values of the `mode` option that the worker set, in order.
    fn mode_sets(log: &Arc<Mutex<Vec<Value>>>) -> Vec<String> {
        log.lock()
            .unwrap()
            .iter()
            .filter(|m| {
                m["method"] == "session/set_config_option" && m["params"]["configId"] == "mode"
            })
            .map(|m| m["params"]["value"].as_str().unwrap().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn auto_opens_claude_in_its_own_auto_mode() {
        let mut auto = spec();
        auto.approval_mode = ApprovalMode::Auto;
        let (result, log) = connect_with(HarnessKind::ClaudeCode, auto, Behavior::default()).await;
        let (session, _events, _ready) = result.unwrap();
        let session: Box<dyn HarnessSession> = Box::new(session);
        assert_eq!(mode_sets(&log), vec!["auto"]);
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn auto_falls_back_to_accept_edits_when_claude_has_no_auto_mode() {
        // The adapter does not offer `auto`: the open asks for nothing it
        // cannot get, and the session goes on in `acceptEdits`.
        let mut auto = spec();
        auto.approval_mode = ApprovalMode::Auto;
        let behavior = Behavior {
            no_auto: true,
            ..Behavior::default()
        };
        let (result, log) = connect_with(HarnessKind::ClaudeCode, auto.clone(), behavior).await;
        let (session, _events, _ready) = result.unwrap();
        let session: Box<dyn HarnessSession> = Box::new(session);
        assert!(mode_sets(&log).is_empty(), "{:?}", mode_sets(&log));
        session.close().await.unwrap();

        // The account refuses `auto`: the open does not fail.
        let behavior = Behavior {
            refuse_auto: true,
            ..Behavior::default()
        };
        let (result, log) = connect_with(HarnessKind::ClaudeCode, auto.clone(), behavior).await;
        let (session, _events, _ready) = result.unwrap();
        let session: Box<dyn HarnessSession> = Box::new(session);
        assert_eq!(mode_sets(&log), vec!["auto"]);
        session.close().await.unwrap();

        // The adapter answers but keeps `acceptEdits`, as it does when the
        // model does not support `auto`.
        let behavior = Behavior {
            ignore_mode: true,
            ..Behavior::default()
        };
        let (result, _log) = connect_with(HarnessKind::ClaudeCode, auto, behavior).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn a_mode_change_to_auto_accepts_the_fallback_of_the_adapter() {
        // The fake keeps `acceptEdits` for every change after the open, as
        // the adapter does for a model without `auto` mode.
        let behavior = Behavior {
            ignore_mode: true,
            ..Behavior::default()
        };
        let mut edits = spec();
        edits.approval_mode = ApprovalMode::AutoEdits;
        let (result, _log) = connect_with(HarnessKind::ClaudeCode, edits, behavior).await;
        let (session, _events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        assert!(session.set_approval_mode(ApprovalMode::Auto).await.is_ok());
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn auto_mode_is_only_a_move_up_from_auto() {
        // Auto edits: a move to `auto` is set back.
        let behavior = Behavior {
            escalate: Some("auto"),
            ..Behavior::default()
        };
        let mut edits = spec();
        edits.approval_mode = ApprovalMode::AutoEdits;
        let (result, log) = connect_with(HarnessKind::ClaudeCode, edits, behavior).await;
        let (session, mut events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        allow_first_request(&mut session, &mut events).await;
        let reset = next_matching(&mut events, |e| matches!(e, SessionEvent::Error { .. })).await;
        assert!(
            matches!(&reset, SessionEvent::Error { code, message } if code == "mode_reset" && message.contains("`auto`") && message.contains("`acceptEdits`")),
            "{reset:?}"
        );
        assert_eq!(mode_sets(&log), vec!["acceptEdits"]);
        session.close().await.unwrap();

        // Auto: the session is in `auto` already, and nothing is set back.
        let behavior = Behavior {
            escalate: Some("auto"),
            ..Behavior::default()
        };
        let mut auto = spec();
        auto.approval_mode = ApprovalMode::Auto;
        let (result, log) = connect_with(HarnessKind::ClaudeCode, auto, behavior).await;
        let (session, mut events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        allow_first_request(&mut session, &mut events).await;
        next_matching(&mut events, |e| matches!(e, SessionEvent::TurnEnded { .. })).await;
        assert_eq!(mode_sets(&log), vec!["auto"], "only the open sets a mode");
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn a_claude_session_changes_its_mode_for_a_turn_and_back() {
        let mut auto = spec();
        auto.approval_mode = ApprovalMode::Auto;
        let (result, log) = connect_with(HarnessKind::ClaudeCode, auto, Behavior::default()).await;
        let (session, _events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        session.set_approval_mode(ApprovalMode::Ask).await.unwrap();
        session.set_approval_mode(ApprovalMode::Auto).await.unwrap();
        let sets: Vec<String> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|m| {
                m["method"] == "session/set_config_option" && m["params"]["configId"] == "mode"
            })
            .map(|m| m["params"]["value"].as_str().unwrap().to_owned())
            .collect();
        // The fake agent starts in `acceptEdits`, so the open sets `auto`.
        assert_eq!(sets, vec!["auto", "default", "auto"]);
        session.close().await.unwrap();

        // A mode that the agent does not take fails, so the worker refuses
        // the turn.
        let mut auto = spec();
        auto.approval_mode = ApprovalMode::AutoEdits;
        let behavior = Behavior {
            refuse_reset: true,
            ..Behavior::default()
        };
        let (result, _log) = connect_with(HarnessKind::ClaudeCode, auto, behavior).await;
        let (session, _events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        // The fake refuses every mode change after the first one.
        session.set_approval_mode(ApprovalMode::Ask).await.unwrap();
        assert!(
            session
                .set_approval_mode(ApprovalMode::AutoEdits)
                .await
                .is_err()
        );
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn other_agents_cannot_change_their_mode_in_a_session() {
        let mut auto = spec();
        auto.approval_mode = ApprovalMode::Auto;
        let (result, _log) = connect_with(HarnessKind::OpenCode, auto, Behavior::default()).await;
        let (session, _events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        assert!(matches!(
            session.set_approval_mode(ApprovalMode::Ask).await,
            Err(HarnessError::Unsupported(_))
        ));
        // The same mode needs no change.
        assert!(session.set_approval_mode(ApprovalMode::Auto).await.is_ok());
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn a_mode_that_cannot_be_set_back_loses_the_session() {
        let behavior = Behavior {
            escalate: Some("bypassPermissions"),
            refuse_reset: true,
            bypass: true,
            ..Behavior::default()
        };
        let (result, _log) = connect_with(HarnessKind::ClaudeCode, spec(), behavior).await;
        let (session, mut events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        allow_first_request(&mut session, &mut events).await;
        let error = next_matching(&mut events, |e| matches!(e, SessionEvent::Error { .. })).await;
        assert!(
            matches!(&error, SessionEvent::Error { code, .. } if code == "mode_reset_failed"),
            "{error:?}"
        );
        assert!(matches!(
            next_matching(&mut events, |e| matches!(e, SessionEvent::State { .. })).await,
            SessionEvent::State {
                state: SessionState::Lost
            }
        ));
    }

    #[tokio::test]
    async fn a_permission_request_after_a_cancel_is_cancelled_at_once() {
        let behavior = Behavior {
            ask_after_cancel: true,
            ..Behavior::default()
        };
        let (result, log) = connect_with(HarnessKind::ClaudeCode, spec(), behavior).await;
        let (session, mut events, _ready) = result.unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        allow_first_request(&mut session, &mut events).await;
        next_matching(&mut events, |e| matches!(e, SessionEvent::TurnEnded { .. })).await;
        session
            .turn(TurnInput {
                text: "Wait".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        session.interrupt().await.unwrap();
        // The agent asks once more before it ends the turn. The user never
        // sees that request.
        loop {
            match next_matching(&mut events, |_| true).await {
                SessionEvent::RequestOpened(opened) => {
                    panic!("a request after the cancel reached the user: {opened:?}")
                }
                SessionEvent::TurnEnded { reason, .. } => {
                    assert_eq!(reason, TurnEndReason::Interrupted);
                    break;
                }
                _ => {}
            }
        }
        let answer = log
            .lock()
            .unwrap()
            .iter()
            .find(|m| m.get("id") == Some(&json!("perm-2")) && m.get("method").is_none())
            .cloned()
            .expect("the request was answered");
        assert_eq!(answer["result"]["outcome"]["outcome"], "cancelled");
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn full_access_without_bypass_is_unavailable() {
        let mut s = spec();
        s.approval_mode = ApprovalMode::FullAccess;
        let (result, _log) =
            connect_with(HarnessKind::ClaudeCode, s.clone(), Behavior::default()).await;
        assert!(
            matches!(&result, Err(HarnessError::ModeUnavailable(_))),
            "{:?}",
            result.as_ref().err()
        );
        let behavior = Behavior {
            bypass: true,
            ..Behavior::default()
        };
        let (result, _log) = connect_with(HarnessKind::ClaudeCode, s, behavior).await;
        let (session, _events, ready) = result.unwrap();
        assert_eq!(ready.bypass, Some(true));
        Box::new(session).close().await.unwrap();
    }

    #[tokio::test]
    async fn full_access_without_bypass_opens_in_auto_with_its_limits() {
        let mut s = spec();
        s.approval_mode = ApprovalMode::FullAccess;
        let logs = Arc::new(Mutex::new(Vec::new()));
        let seen = logs.clone();
        let (session, _events) = open_with_fallback(HarnessKind::ClaudeCode, s, |spec| {
            let seen = seen.clone();
            async move {
                let (result, log) =
                    connect_with(HarnessKind::ClaudeCode, spec, Behavior::default()).await;
                seen.lock().unwrap().push(log);
                result
            }
        })
        .await
        .unwrap();
        assert!(
            !session
                .capabilities()
                .approval_modes
                .contains(&ApprovalMode::FullAccess)
        );
        let logs = logs.lock().unwrap().clone();
        assert_eq!(logs.len(), 2, "a second agent opens the session");
        let second = logs[1].lock().unwrap().clone();
        let new = second
            .iter()
            .find(|m| m["method"] == "session/new")
            .unwrap();
        assert_eq!(
            new["params"]["_meta"]["claudeCode"]["options"]["settingSources"],
            json!(["user"]),
            "the lower mode does not load the repo's settings"
        );
        assert_eq!(
            new["params"]["_meta"]["claudeCode"]["options"]["allowDangerouslySkipPermissions"],
            false
        );
        Box::new(session).close().await.unwrap();
    }

    #[tokio::test]
    async fn a_probe_sees_whether_bypass_is_offered() {
        let (client_side, agent_side) = tokio::io::duplex(1 << 16);
        let log = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(run_agent_with(agent_side, log.clone(), Behavior::default()));
        let (read, write) = tokio::io::split(client_side);
        let (session, _events, ready) = connect_for(
            HarnessKind::ClaudeCode,
            spec(),
            write,
            read,
            None,
            StderrTail::empty(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(ready.bypass, Some(false));
        // The probe read the options of each model.
        let models = models_from_config(&ready.config_options, &ready.model_options);
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].options.len(), 1, "Opus has an effort option");
        assert!(models[2].options.is_empty(), "Haiku has none");
        let new = log
            .lock()
            .unwrap()
            .iter()
            .find(|m| m["method"] == "session/new")
            .cloned()
            .unwrap();
        let options = &new["params"]["_meta"]["claudeCode"]["options"];
        assert_eq!(options["settingSources"], json!(["user"]));
        assert!(
            options.get("allowDangerouslySkipPermissions").is_none(),
            "a probe lets Claude Code show its bypass mode: {options}"
        );
        Box::new(session).close().await.unwrap();
    }

    #[tokio::test]
    async fn initialize_has_its_own_limit() {
        let behavior = Behavior {
            silent: true,
            ..Behavior::default()
        };
        let started = std::time::Instant::now();
        let (result, _log) = connect_with(HarnessKind::ClaudeCode, spec(), behavior).await;
        let Err(HarnessError::Timeout(why)) = result else {
            panic!("the open must time out")
        };
        assert!(why.contains("initialize"), "{why}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Whether a process runs. A zombie does not count.
    #[cfg(unix)]
    fn runs(pid: u32) -> bool {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        !stat.is_empty() && !stat.starts_with('Z')
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_dropped_start_stops_the_agent() {
        // An agent that never answers, as a hung probe has.
        let mut child = crate::process::spawn(&crate::process::ProcessSpec {
            program: crate::process::resolve_program("sh").unwrap(),
            args: vec!["-c".into(), "sleep 30".into()],
            ..Default::default()
        })
        .unwrap();
        let pid = child.id().unwrap();
        let stdin = child.take_stdin().unwrap();
        let stdout = child.take_stdout().unwrap();
        let stderr = child.stderr_handle();
        // The probe limit runs out before the agent answers.
        let dropped = tokio::time::timeout(
            Duration::from_millis(100),
            connect_for(
                HarnessKind::ClaudeCode,
                spec(),
                stdin,
                stdout,
                Some(child),
                stderr,
                true,
            ),
        )
        .await;
        assert!(dropped.is_err());
        for _ in 0..50 {
            if !runs(pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!runs(pid), "the agent still runs");
    }

    /// Saves an attachment where the worker saves it, and returns its
    /// reference.
    fn save_attachment(
        workspace: &std::path::Path,
        name: &str,
        bytes: &[u8],
    ) -> consort_proto::model::BlobRef {
        let blob = consort_proto::model::BlobRef {
            sha256: consort_proto::model::Sha256Hash::from_bytes([bytes.len() as u8; 32]),
            name: name.into(),
            mime: "application/octet-stream".into(),
            size: bytes.len() as u64,
        };
        let path = crate::attachments::attachment_path(workspace, &blob);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        blob
    }

    #[tokio::test]
    async fn claude_gets_images_as_image_blocks_in_a_turn_and_a_steer() {
        let s = spec();
        let workspace = s.workspace.clone();
        let behavior = Behavior {
            images: true,
            ..Behavior::default()
        };
        let (result, log) = connect_with(HarnessKind::ClaudeCode, s, behavior).await;
        let (session, _events, ready) = result.unwrap();
        assert!(ready.capabilities.images);
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        assert!(session.capabilities().images);
        let png = save_attachment(&workspace, "shot.png", crate::attachments::tests::PNG);
        let pdf = save_attachment(&workspace, "spec.pdf", b"%PDF-1.7 tiny");
        session
            .turn(TurnInput {
                text: "What is in these?".into(),
                attachments: vec![png.clone(), pdf.clone()],
            })
            .await
            .unwrap();
        session
            .steer(TurnInput {
                text: "And this one.".into(),
                attachments: vec![png.clone()],
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let log = log.lock().unwrap();
        let prompt = &log
            .iter()
            .find(|m| m["method"] == "session/prompt")
            .unwrap()["params"]["prompt"];
        let blocks = prompt.as_array().unwrap();
        assert_eq!(blocks.len(), 2, "{prompt}");
        let text = blocks[0]["text"].as_str().unwrap();
        let png_path = crate::attachments::attachment_path(&workspace, &png);
        let pdf_path = crate::attachments::attachment_path(&workspace, &pdf);
        assert!(
            text.starts_with("What is in these?\n\nAttached files."),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "also in this message as an image): {}",
                png_path.display()
            )),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "spec.pdf (application/octet-stream, 13 bytes): {}",
                pdf_path.display()
            )),
            "{text}"
        );
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["mimeType"], "image/png");
        assert_eq!(
            blocks[1]["data"],
            crate::attachments::base64(crate::attachments::tests::PNG)
        );
        let steer = &log
            .iter()
            .find(|m| m["method"] == "_session/steering")
            .unwrap()["params"]["prompt"];
        assert_eq!(steer[1]["type"], "image", "{steer}");
    }

    #[tokio::test]
    async fn an_agent_without_image_input_gets_paths_only() {
        let s = spec();
        let workspace = s.workspace.clone();
        let (result, log) = connect_with(HarnessKind::GeminiCli, s, Behavior::default()).await;
        let (session, _events, ready) = result.unwrap();
        assert!(
            !ready.capabilities.images,
            "the agent did not report images"
        );
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        let png = save_attachment(&workspace, "shot.png", crate::attachments::tests::PNG);
        session
            .turn(TurnInput {
                text: "Look".into(),
                attachments: vec![png],
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let log = log.lock().unwrap();
        let prompt = &log
            .iter()
            .find(|m| m["method"] == "session/prompt")
            .unwrap()["params"]["prompt"];
        assert_eq!(prompt.as_array().unwrap().len(), 1, "{prompt}");
        let text = prompt[0]["text"].as_str().unwrap();
        assert!(
            text.contains("shot.png (application/octet-stream, 16 bytes): "),
            "{text}"
        );
        assert!(!text.contains("as an image"));
    }

    #[test]
    fn the_static_image_capability_is_on_only_for_checked_agents() {
        assert!(static_capabilities(HarnessKind::ClaudeCode).images);
        assert!(static_capabilities(HarnessKind::GeminiCli).images);
        assert!(static_capabilities(HarnessKind::OpenCode).images);
        assert!(!static_capabilities(HarnessKind::Acp).images);
    }
}

#[test]
fn the_skill_folder_goes_into_the_opencode_config() {
    let found = |name: &str| (name == "opencode").then(|| PathBuf::from("/bin/opencode"));
    let account = AccountConfig::default_for(HarnessKind::OpenCode);
    let skills = crate::skills::SessionSkills::new(
        PathBuf::from("/runtime/ses_1/skills"),
        vec!["pdf-forms".into()],
    );
    let with = launch_recipe_with(
        HarnessKind::OpenCode,
        &account,
        ApprovalMode::Auto,
        true,
        Some(&skills),
        &found,
    )
    .unwrap();
    let config: Value = serde_json::from_str(&with.env["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert_eq!(
        config["skills"],
        json!([PathBuf::from("/runtime/ses_1/skills")
            .join("skills")
            .to_string_lossy()])
    );
    // The permissions stay.
    assert_eq!(config["permission"]["bash"], "ask");
    let without = launch_recipe(
        HarnessKind::OpenCode,
        &account,
        ApprovalMode::Auto,
        true,
        &found,
    )
    .unwrap();
    let config: Value = serde_json::from_str(&without.env["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert!(config.get("skills").is_none(), "{config}");

    // The agent process gets the same config from the session spec.
    let mut spec = SessionSpec {
        session_id: SessionId::new(),
        workspace: PathBuf::from("/ws"),
        runtime_dir: PathBuf::from("/runtime/ses_1"),
        model: String::new(),
        options: Default::default(),
        approval_mode: ApprovalMode::Auto,
        allow_shell: true,
        coordinator: false,
        system_prompt_append: String::new(),
        mcp_servers: vec![],
        env: BTreeMap::new(),
        extras_enabled: true,
        skills: Some(skills),
        resume: None,
        process_watcher: None,
    };
    let process = agent_process_spec(HarnessKind::OpenCode, &account, &spec, &found).unwrap();
    // The config is JSON, so the JSON text of the path shows in it. A
    // Windows path has backslashes, and JSON escapes them.
    let folder = json!(
        PathBuf::from("/runtime/ses_1/skills")
            .join("skills")
            .to_string_lossy()
    )
    .to_string();
    assert!(process.env["OPENCODE_CONFIG_CONTENT"].contains(&folder));
    spec.skills = None;
    let process = agent_process_spec(HarnessKind::OpenCode, &account, &spec, &found).unwrap();
    assert!(!process.env["OPENCODE_CONFIG_CONTENT"].contains("skills"));
}

#[test]
fn claude_meta_keeps_repo_settings_and_the_shell_out() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("repo");
    std::fs::create_dir_all(workspace.join(".claude")).unwrap();
    std::fs::write(workspace.join("CLAUDE.md"), "Use tabs.\n").unwrap();
    let outside = dir.path().join("outside.txt");
    std::fs::write(&outside, "do not read me").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, workspace.join(".claude").join("CLAUDE.md")).unwrap();
    let mut spec = SessionSpec {
        session_id: SessionId::new(),
        workspace: workspace.clone(),
        runtime_dir: dir.path().to_path_buf(),
        model: String::new(),
        options: Default::default(),
        approval_mode: ApprovalMode::AutoEdits,
        allow_shell: false,
        coordinator: false,
        system_prompt_append: "You are a task.".into(),
        mcp_servers: vec![],
        env: BTreeMap::new(),
        extras_enabled: true,
        skills: None,
        resume: None,
        process_watcher: None,
    };
    let meta = claude_meta(&spec);
    let append = meta["systemPrompt"]["append"].as_str().unwrap();
    assert!(append.starts_with("You are a task."), "{append}");
    assert!(append.contains("Use tabs."), "{append}");
    assert!(
        !append.contains("do not read me"),
        "a linked CLAUDE.md is not read: {append}"
    );
    let options = &meta["claudeCode"]["options"];
    assert_eq!(options["settingSources"], json!(["user"]));
    assert_eq!(options["allowDangerouslySkipPermissions"], false);
    assert_eq!(
        options["disallowedTools"],
        json!(["Bash", "PowerShell", "Monitor"])
    );

    // Full access with the shell on keeps Claude Code's own settings, and
    // Claude Code loads CLAUDE.md itself.
    spec.approval_mode = ApprovalMode::FullAccess;
    spec.allow_shell = true;
    let meta = claude_meta(&spec);
    assert_eq!(meta["systemPrompt"]["append"], "You are a task.");
    assert!(meta.get("claudeCode").is_none(), "{meta}");
}

/// A workspace folder that is one git repo.
fn repo_workspace() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("repo");
    std::fs::create_dir_all(workspace.join(".git")).unwrap();
    (dir, workspace)
}

#[test]
fn claude_md_imports_are_followed_inside_the_workspace() {
    let (dir, ws) = repo_workspace();
    std::fs::write(ws.join("CLAUDE.md"), "@AGENTS.md\n").unwrap();
    std::fs::create_dir_all(ws.join("docs")).unwrap();
    std::fs::write(
        ws.join("AGENTS.md"),
        "Use tabs. See @docs/style.md, @../outside.md and @link.md.\n```\n@secret.md\n```\nNot `@secret.md` either.\n",
    )
    .unwrap();
    // A cycle back to AGENTS.md.
    std::fs::write(ws.join("docs/style.md"), "Short lines.\n@../AGENTS.md\n").unwrap();
    std::fs::write(ws.join("secret.md"), "do not read: fenced").unwrap();
    std::fs::write(dir.path().join("outside.md"), "do not read: outside").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path().join("outside.md"), ws.join("link.md")).unwrap();
    let text = project_instructions(&ws).unwrap();
    assert!(
        text.contains("Instructions from CLAUDE.md in the workspace:\n\n@AGENTS.md"),
        "{text}"
    );
    assert!(
        text.contains("Instructions from AGENTS.md in the workspace, imported by CLAUDE.md:"),
        "{text}"
    );
    assert!(text.contains("Use tabs."), "{text}");
    assert!(
        text.contains("imported by AGENTS.md:\n\nShort lines."),
        "{text}"
    );
    assert!(!text.contains("do not read"), "{text}");
    assert_eq!(
        text.matches("Use tabs.").count(),
        1,
        "a cycle reads a file once"
    );
}

#[test]
fn imports_stop_at_the_depth_limit() {
    let (_dir, ws) = repo_workspace();
    std::fs::write(ws.join("CLAUDE.md"), "@f1.md").unwrap();
    let words = ["one", "two", "three", "four", "five", "six"];
    for (i, word) in words.iter().enumerate() {
        let n = i + 1;
        std::fs::write(
            ws.join(format!("f{n}.md")),
            format!("Step {word}. @f{}.md", n + 1),
        )
        .unwrap();
    }
    let text = project_instructions(&ws).unwrap();
    assert!(text.contains("Step five."), "{text}");
    assert!(!text.contains("Step six."), "{text}");
}

#[test]
fn a_multi_repo_workspace_reads_each_repo() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    for name in ["a", "b", "notes"] {
        std::fs::create_dir_all(root.join(name).join(".claude")).unwrap();
    }
    std::fs::create_dir_all(root.join("a/.git")).unwrap();
    // A worktree has a `.git` file.
    std::fs::write(root.join("b/.git"), "gitdir: /x").unwrap();
    std::fs::write(root.join("a/CLAUDE.md"), "From a. @../b/shared.md").unwrap();
    std::fs::write(root.join("b/shared.md"), "Shared by b.").unwrap();
    std::fs::write(root.join("b/.claude/CLAUDE.md"), "From b.").unwrap();
    std::fs::write(root.join("notes/CLAUDE.md"), "do not read: not a repo").unwrap();
    let text = project_instructions(&root).unwrap();
    assert!(text.contains("Instructions from a/CLAUDE.md"), "{text}");
    assert!(text.contains("From a."), "{text}");
    assert!(text.contains("Shared by b."), "{text}");
    assert!(
        text.contains("Instructions from b/.claude/CLAUDE.md"),
        "{text}"
    );
    assert!(!text.contains("do not read"), "{text}");
    // In a single repo, a subfolder with `.git` (a submodule) is not read.
    let (_dir, ws) = repo_workspace();
    std::fs::create_dir_all(ws.join("sub/.git")).unwrap();
    std::fs::write(ws.join("sub/CLAUDE.md"), "Submodule.").unwrap();
    assert_eq!(project_instructions(&ws), None);
}

#[test]
fn instructions_stay_in_the_budget() {
    let (_dir, ws) = repo_workspace();
    std::fs::write(
        ws.join("CLAUDE.md"),
        format!("@big.md\n{}", "a".repeat(40 * 1024)),
    )
    .unwrap();
    std::fs::write(ws.join("big.md"), "b".repeat(40 * 1024)).unwrap();
    let text = project_instructions(&ws).unwrap();
    assert!(text.len() <= 64 * 1024, "{}", text.len());
    assert!(text.contains("bbbb"));
}

#[test]
fn managed_launches_are_used_unless_the_account_names_a_binary() {
    let nothing = |_: &str| -> Option<PathBuf> { None };
    let managed = |program: &str, args: &[&str]| crate::Launch {
        program: PathBuf::from(program),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        env: BTreeMap::from([("PATH".to_owned(), "/tools/node/bin".to_owned())]),
    };

    let mut claude = AccountConfig::default_for(HarnessKind::ClaudeCode);
    claude.runtime.launch = Some(managed("/tools/node", &["/tools/acp/dist/index.js"]));
    let l = launch_recipe(
        HarnessKind::ClaudeCode,
        &claude,
        ApprovalMode::Ask,
        true,
        &nothing,
    )
    .unwrap();
    assert_eq!(l.program, PathBuf::from("/tools/node"));
    assert_eq!(l.args, ["/tools/acp/dist/index.js"]);
    assert_eq!(l.env["PATH"], "/tools/node/bin");

    let mut gemini = AccountConfig::default_for(HarnessKind::GeminiCli);
    gemini.runtime.launch = Some(managed("/tools/node", &["gemini.js", "--acp"]));
    let l = launch_recipe(
        HarnessKind::GeminiCli,
        &gemini,
        ApprovalMode::Auto,
        true,
        &nothing,
    )
    .unwrap();
    assert_eq!(
        l.args,
        ["gemini.js", "--acp", "--approval-mode", "auto_edit"]
    );

    let mut opencode = AccountConfig::default_for(HarnessKind::OpenCode);
    opencode.runtime.launch = Some(managed("/tools/opencode", &["acp"]));
    let l = launch_recipe(
        HarnessKind::OpenCode,
        &opencode,
        ApprovalMode::Ask,
        true,
        &nothing,
    )
    .unwrap();
    assert_eq!(l.program, PathBuf::from("/tools/opencode"));
    assert_eq!(l.args, ["acp"]);
    assert!(l.env.contains_key("OPENCODE_CONFIG_CONTENT"));

    let mut agent = AccountConfig::default_for(HarnessKind::Acp);
    agent.acp = Some(consort_proto::model::AcpAccountSpec::Registry { id: "goose".into() });
    assert_eq!(
        agent.managed_tool(),
        Some(crate::ToolId::Acp("goose".into()))
    );
    agent.runtime.launch = Some(managed("/tools/goose", &["acp"]));
    let l = launch_recipe(HarnessKind::Acp, &agent, ApprovalMode::Ask, true, &nothing).unwrap();
    assert_eq!(l.program, PathBuf::from("/tools/goose"));

    // A hand-written `binary` wins over the managed tool.
    let found = |name: &str| -> Option<PathBuf> { Some(PathBuf::from(format!("/bin/{name}"))) };
    claude.binary = Some("my-claude-acp".into());
    assert_eq!(claude.managed_tool(), None);
    let l = launch_recipe(
        HarnessKind::ClaudeCode,
        &claude,
        ApprovalMode::Ask,
        true,
        &found,
    )
    .unwrap();
    assert_eq!(l.program, PathBuf::from("/bin/my-claude-acp"));
    assert!(!l.env.contains_key("PATH"));
}

#[test]
fn auth_methods_become_sign_in_methods() {
    let init = json!({
        "authMethods": [
            {"id": "oauth-personal", "name": "Log in with Google"},
            {"id": "login", "name": "Log in", "type": "terminal", "args": ["--login"]},
            {"id": "key", "name": "API key", "type": "env_var", "varName": "AGENT_API_KEY"},
            {"id": "x", "name": "Something new", "type": "future_kind"}
        ],
        "agentCapabilities": {"auth": {"logout": {}}}
    });
    let methods = sign_in_methods_from_init(&init, "/tools/agent acp");
    assert_eq!(
        methods,
        vec![
            SignInMethod::AcpAgent {
                method_id: "oauth-personal".into(),
                name: "Log in with Google".into()
            },
            SignInMethod::Terminal {
                command: "/tools/agent acp --login".into()
            },
            SignInMethod::ApiKey {
                variable: "AGENT_API_KEY".into()
            },
        ]
    );
    assert!(offers_logout(&init));
    assert!(!offers_logout(&json!({"agentCapabilities": {}})));
    assert!(sign_in_methods_from_init(&json!({}), "x").is_empty());
}

/// A spec for the tests of the coordinator rules.
fn coordinator_spec(dir: &std::path::Path, allow_shell: bool, coordinator: bool) -> SessionSpec {
    SessionSpec {
        session_id: SessionId::new(),
        workspace: dir.to_path_buf(),
        runtime_dir: dir.join("runtime"),
        model: String::new(),
        options: Default::default(),
        approval_mode: ApprovalMode::Ask,
        allow_shell,
        coordinator,
        system_prompt_append: String::new(),
        mcp_servers: vec![],
        env: BTreeMap::new(),
        extras_enabled: true,
        skills: None,
        resume: None,
        process_watcher: None,
    }
}

#[test]
fn a_claude_coordinator_loses_the_shell_and_the_web() {
    let dir = tempfile::tempdir().unwrap();
    let meta = claude_meta(&coordinator_spec(dir.path(), false, true));
    assert_eq!(
        meta["claudeCode"]["options"]["disallowedTools"],
        json!(["Bash", "PowerShell", "Monitor", "WebFetch", "WebSearch"])
    );
    // A task keeps the web.
    let meta = claude_meta(&coordinator_spec(dir.path(), false, false));
    assert_eq!(
        meta["claudeCode"]["options"]["disallowedTools"],
        json!(["Bash", "PowerShell", "Monitor"])
    );
}

#[test]
fn a_gemini_coordinator_gets_a_policy_that_denies_shell_and_web_and_allows_hub_tools() {
    let dir = tempfile::tempdir().unwrap();
    let found = |name: &str| (name == "gemini").then(|| PathBuf::from("/bin/gemini"));
    let account = AccountConfig::default_for(HarnessKind::GeminiCli);
    let spec = coordinator_spec(dir.path(), false, true);
    let process = agent_process_spec(HarnessKind::GeminiCli, &account, &spec, &found).unwrap();
    let flag = process
        .args
        .iter()
        .find_map(|a| a.strip_prefix("--admin-policy="))
        .expect("an admin policy flag");
    let policy = std::fs::read_to_string(flag).unwrap();
    for tool in [
        "run_shell_command",
        "google_web_search",
        "web_fetch",
        "codebase_investigator",
    ] {
        assert!(policy.contains(&format!("\"{tool}\"")), "{policy}");
    }
    assert!(policy.contains("decision = \"deny\""), "{policy}");
    assert!(
        policy.contains("mcpName = \"hub\"\ntoolName = \"*\"\ndecision = \"allow\""),
        "{policy}"
    );
    // The mode flag stays, and Gemini still asks in Ask mode for the rest.
    assert!(process.args.contains(&"default".to_owned()));

    // A task with the shell on needs no policy.
    let spec = coordinator_spec(dir.path(), true, false);
    let process = agent_process_spec(HarnessKind::GeminiCli, &account, &spec, &found).unwrap();
    assert!(
        !process.args.iter().any(|a| a.starts_with("--admin-policy")),
        "{:?}",
        process.args
    );
    // A task without the shell only loses the shell.
    let policy = crate::approval::gemini_policy(false, false).unwrap();
    assert!(policy.contains("run_shell_command"), "{policy}");
    assert!(!policy.contains("google_web_search"), "{policy}");
    assert!(!policy.contains("mcpName"), "{policy}");
}

#[test]
fn an_opencode_coordinator_denies_the_web_and_allows_hub_tools() {
    let found = |name: &str| (name == "opencode").then(|| PathBuf::from("/bin/opencode"));
    let account = AccountConfig::default_for(HarnessKind::OpenCode);
    let dir = tempfile::tempdir().unwrap();
    let spec = coordinator_spec(dir.path(), false, true);
    let process = agent_process_spec(HarnessKind::OpenCode, &account, &spec, &found).unwrap();
    let config: Value = serde_json::from_str(&process.env["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert_eq!(config["permission"]["bash"], "deny");
    assert_eq!(config["permission"]["webfetch"], "deny");
    assert_eq!(config["permission"]["websearch"], "deny");
    assert_eq!(config["permission"]["hub_*"], "allow");
    // A task keeps the web, as its mode says.
    let spec = coordinator_spec(dir.path(), true, false);
    let process = agent_process_spec(HarnessKind::OpenCode, &account, &spec, &found).unwrap();
    let config: Value = serde_json::from_str(&process.env["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert_eq!(config["permission"]["webfetch"], "ask");
    assert!(config["permission"].get("websearch").is_none());
}
