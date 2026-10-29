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
    assert!(
        matches!(&tool.body, RequestBody::Approval(ApprovalDetails::Tool { server: Some(s), tool, .. }) if s == "github" && tool == "create_issue")
    );
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

    let gemini = AccountConfig::default_for(HarnessKind::GeminiCli);
    let l = launch_recipe(
        HarnessKind::GeminiCli,
        &gemini,
        ApprovalMode::Auto,
        true,
        &found,
    )
    .unwrap();
    assert_eq!(
        l.args,
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
    let models = models_from_config(&options);
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
    assert!(models_from_config(&[]).is_empty());
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
            system_prompt_append: "You are a coordinator.".into(),
            mcp_servers: vec![McpServerSpec {
                name: "hub".into(),
                url: "http://127.0.0.1:9/mcp/ses".into(),
                bearer_token: Some("tok".into()),
                bridge: None,
            }],
            env: BTreeMap::new(),
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
    }

    /// The config options of the fake session.
    fn fake_options(mode: &str, behavior: Behavior) -> Vec<Value> {
        let mut options = vec![
            json!({"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "opus",
             "options": [{"value": "opus", "name": "Opus"}, {"value": "sonnet", "name": "Sonnet"}]}),
            json!({"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "medium",
             "options": [{"value": "medium", "name": "Medium"}, {"value": "high", "name": "High"}]}),
        ];
        if !behavior.no_mode {
            options.insert(0, json!({"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": mode,
             "options": [{"value": "default", "name": "Default"}, {"value": "acceptEdits", "name": "Accept edits"}]}));
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
        let (read, mut write) = tokio::io::split(stream);
        let mut lines = BufReader::new(read).lines();
        let mut prompts = 0;
        let mut open_prompt: Option<Value> = None;
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
                        "agentCapabilities": {"loadSession": true, "mcpCapabilities": {"http": true}},
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
                        _ => None,
                    };
                    if let Some((code, message)) = failure {
                        send(&mut write, json!({"jsonrpc": "2.0", "id": id.clone(), "error": {"code": code, "message": message}})).await;
                        continue;
                    }
                    prompts += 1;
                    open_prompt = id.clone();
                    if prompts == 1 {
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
            Some(SessionEvent::TurnEnded { reason: TurnEndReason::Failed, error: Some(error), .. })
                if error.contains("Internal error")
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
        system_prompt_append: "You are a task.".into(),
        mcp_servers: vec![],
        env: BTreeMap::new(),
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
