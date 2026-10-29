use super::*;
use consort_proto::ids::SessionId;
use consort_proto::model::{
    ApprovalDetails, FileChangeKind, ItemPayload, ItemStatus, RequestOptionKind,
};
use consort_proto::worker_link::RequestBody;
use protocol::UserInputQuestion;

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
fn recorded_turn_maps_to_events() {
    let text = include_str!("../../tests/fixtures/codex-ok-turn.jsonl");
    let mut m = CodexMapper::new(SessionId::new());
    let mut events = Vec::new();
    for line in text.lines() {
        let message: Value = serde_json::from_str(line).unwrap();
        events.extend(m.map_notification(message["method"].as_str().unwrap(), &message["params"]));
    }
    assert!(matches!(events[0], SessionEvent::TurnStarted { .. }));
    let all = items(&events);
    let (last, status) = all.last().unwrap();
    assert!(matches!(last, ItemPayload::AssistantMessage(a) if a.text == "OK"));
    assert_eq!(*status, ItemStatus::Completed);
    let usage = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::Usage(u) => Some(u.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.cached_tokens),
        (25186, 5, 14720)
    );
    assert!(matches!(
        &events[events.len() - 2],
        SessionEvent::TurnEnded {
            reason: TurnEndReason::Completed,
            error: None,
            ..
        }
    ));
    assert!(matches!(
        events.last(),
        Some(SessionEvent::State {
            state: SessionState::Idle
        })
    ));
    assert!(m.current_turn().is_none());
}

#[test]
fn commands_output_and_file_changes() {
    let mut m = CodexMapper::new(SessionId::new());
    m.turn_accepted("t1");
    let started = m.map_notification("item/started", &json!({
        "threadId": "th", "turnId": "t1",
        "item": {"type": "commandExecution", "id": "c1", "command": "cargo test", "cwd": "/w", "commandActions": [], "status": "inProgress"}
    }));
    assert!(
        matches!(&items(&started)[0], (ItemPayload::Command(c), ItemStatus::InProgress) if c.command == "cargo test")
    );
    let delta = m.map_notification(
        "item/commandExecution/outputDelta",
        &json!({"threadId": "th", "turnId": "t1", "itemId": "c1", "delta": "running 3 tests\n"}),
    );
    assert!(
        matches!(&delta[0], SessionEvent::OutputDelta { text, .. } if text == "running 3 tests\n")
    );
    let done = m.map_notification("item/completed", &json!({
        "threadId": "th", "turnId": "t1",
        "item": {"type": "commandExecution", "id": "c1", "command": "cargo test", "cwd": "/w", "commandActions": [],
                 "aggregatedOutput": "running 3 tests\nok\n", "exitCode": 0, "durationMs": 1200, "status": "completed"}
    }));
    match &items(&done)[0] {
        (ItemPayload::Command(c), ItemStatus::Completed) => {
            assert_eq!(c.exit_code, Some(0));
            assert_eq!(c.duration_ms, Some(1200));
            assert!(c.output.ends_with("ok\n"));
        }
        other => panic!("{other:?}"),
    }
    let files = m.map_notification("item/started", &json!({
        "threadId": "th", "turnId": "t1",
        "item": {"type": "fileChange", "id": "f1", "status": "inProgress", "changes": [
            {"path": "a.rs", "kind": {"type": "update", "move_path": null}, "diff": "@@ -1 +1,2 @@\n-a\n+b\n+c\n"},
            {"path": "old.rs", "kind": {"type": "update", "move_path": "new.rs"}, "diff": ""},
            {"path": "n.rs", "kind": {"type": "add"}, "diff": "x\ny\n"}
        ]}
    }));
    match &items(&files)[0].0 {
        ItemPayload::FileChange(f) => {
            assert_eq!(
                (f.changes[0].lines_added, f.changes[0].lines_removed),
                (2, 1)
            );
            assert_eq!(f.changes[1].change, FileChangeKind::Rename);
            assert_eq!(f.changes[1].path, "new.rs");
            assert_eq!(f.changes[1].from_path.as_deref(), Some("old.rs"));
            assert_eq!(
                (f.changes[2].change, f.changes[2].lines_added),
                (FileChangeKind::Add, 2)
            );
        }
        other => panic!("{other:?}"),
    }
    let approval = m.file_change_approval(
        RequestId::new("codex-1"),
        &FileChangeApprovalParams {
            turn_id: "t1".into(),
            item_id: "f1".into(),
            reason: None,
            grant_root: None,
        },
    );
    assert!(
        matches!(&approval.body, RequestBody::Approval(ApprovalDetails::FileChange { paths }) if paths.len() == 3)
    );
    assert_eq!(approval.summary, "Edit 3 files");
    let mcp = m.map_notification("item/completed", &json!({
        "threadId": "th", "turnId": "t1",
        "item": {"type": "mcpToolCall", "id": "m1", "server": "hub", "tool": "post_update", "arguments": {"body": "hi"},
                 "result": {"content": [{"type": "text", "text": "{\"ok\":true}"}]}, "status": "completed"}
    }));
    assert!(
        matches!(&items(&mcp)[0], (ItemPayload::ToolCall(t), ItemStatus::Completed) if t.server.as_deref() == Some("hub") && t.output.as_deref() == Some("{\"ok\":true}"))
    );
}

#[test]
fn interrupted_and_failed_turns() {
    let mut m = CodexMapper::new(SessionId::new());
    m.turn_accepted("t1");
    m.map_notification(
        "item/agentMessage/delta",
        &json!({"threadId": "th", "turnId": "t1", "itemId": "a1", "delta": "Hal"}),
    );
    let ended = m.map_notification(
        "turn/completed",
        &json!({"threadId": "th", "turn": {"id": "t1", "items": [], "status": "interrupted"}}),
    );
    assert!(
        matches!(&items(&ended)[0], (ItemPayload::AssistantMessage(a), ItemStatus::Completed) if a.text == "Hal")
    );
    assert!(ended.iter().any(|e| matches!(
        e,
        SessionEvent::TurnEnded {
            reason: TurnEndReason::Interrupted,
            ..
        }
    )));
    m.turn_accepted("t2");
    let failed = m.map_notification("turn/completed", &json!({"threadId": "th", "turn": {"id": "t2", "items": [], "status": "failed", "error": {"message": "Rate limited."}}}));
    assert!(failed.iter().any(|e| matches!(e, SessionEvent::TurnEnded { reason: TurnEndReason::Failed, error: Some(msg), .. } if msg == "Rate limited.")));
    let err = m.map_notification("error", &json!({"threadId": "th", "turnId": "t2", "willRetry": false, "error": {"message": "boom"}}));
    assert!(matches!(&err[0], SessionEvent::Error { message, .. } if message == "boom"));
    assert!(m.map_notification("error", &json!({"threadId": "th", "turnId": "t2", "willRetry": true, "error": {"message": "retrying"}})).is_empty());
}

#[test]
fn approvals_and_questions() {
    let m = CodexMapper::new(SessionId::new());
    let cmd = m.command_approval(
        RequestId::new("codex-1"),
        &CommandApprovalParams {
            turn_id: "t".into(),
            item_id: "c".into(),
            command: Some("rm -rf build".into()),
            cwd: Some("/w".into()),
            reason: None,
            network_approval_context: None,
        },
    );
    assert_eq!(cmd.summary, "Run `rm -rf build`");
    assert!(
        matches!(&cmd.body, RequestBody::Approval(ApprovalDetails::Command { command, cwd: Some(c) }) if command == "rm -rf build" && c == "/w")
    );
    let ids: Vec<_> = cmd
        .options
        .iter()
        .map(|o| (o.id.as_str(), o.kind))
        .collect();
    assert_eq!(
        ids,
        vec![
            ("accept", RequestOptionKind::AllowOnce),
            ("acceptForSession", RequestOptionKind::AllowAlways),
            ("decline", RequestOptionKind::DenyOnce)
        ]
    );
    let net = m.command_approval(
        RequestId::new("codex-2"),
        &CommandApprovalParams {
            turn_id: "t".into(),
            item_id: "c".into(),
            command: Some("curl x".into()),
            cwd: None,
            reason: Some("Needs network".into()),
            network_approval_context: Some(json!({"host": "example.com"})),
        },
    );
    assert!(
        matches!(&net.body, RequestBody::Approval(ApprovalDetails::Network { target }) if target == "example.com")
    );
    let q: UserInputQuestion = serde_json::from_value(json!({
        "id": "db", "header": "Database", "question": "Which database?", "isOther": true,
        "options": [{"label": "Postgres", "description": "Use Postgres."}, {"label": "SQLite", "description": "Use SQLite."}]
    })).unwrap();
    let opened = CodexMapper::question(RequestId::new("codex-3"), &q);
    assert!(
        matches!(&opened.body, RequestBody::Question(d) if d.allow_free_text && d.question == "Which database?")
    );
    assert_eq!(opened.options[1].label, "SQLite");
    assert_eq!(opened.options[1].kind, RequestOptionKind::Choice);
}

#[test]
fn thread_params_follow_the_approval_table_and_mcp_config() {
    let spec = SessionSpec {
        session_id: SessionId::new(),
        workspace: "/w".into(),
        runtime_dir: "/r".into(),
        model: "gpt-5.5".into(),
        options: Default::default(),
        approval_mode: ApprovalMode::AutoEdits,
        system_prompt_append: "Be brief.".into(),
        mcp_servers: vec![McpServerSpec {
            name: "hub".into(),
            url: "http://127.0.0.1:1/mcp/ses".into(),
            bearer_token: Some("tok".into()),
            bridge: None,
        }],
        env: BTreeMap::new(),
        resume: None,
    };
    let (config, env) = mcp_config(&spec.mcp_servers);
    assert_eq!(env[&token_variable(0)], "tok");
    let params = serde_json::to_value(thread_params(&spec, config)).unwrap();
    assert_eq!(params["approvalPolicy"], "on-request");
    assert_eq!(params["sandbox"], "workspace-write");
    assert_eq!(params["developerInstructions"], "Be brief.");
    assert_eq!(
        params["config"]["mcp_servers"]["hub"]["bearer_token_env_var"],
        "CONSORT_MCP_TOKEN_0"
    );
    assert_eq!(
        params["config"]["mcp_servers"]["hub"]["url"],
        "http://127.0.0.1:1/mcp/ses"
    );
    assert!(
        !params.to_string().contains("tok\""),
        "the token is not in the request"
    );
    assert!(params.get("threadId").is_none());
}

#[test]
fn model_list_maps_efforts() {
    let models: Vec<protocol::Model> = serde_json::from_value(json!([
        {"id": "gpt-5.5", "model": "gpt-5.5", "displayName": "GPT-5.5", "description": "", "hidden": false, "isDefault": true,
         "defaultReasoningEffort": "medium", "supportedReasoningEfforts": [{"reasoningEffort": "low", "description": ""}, {"reasoningEffort": "high", "description": ""}]},
        {"id": "secret", "model": "secret", "displayName": "", "description": "", "hidden": true, "isDefault": false,
         "defaultReasoningEffort": "low", "supportedReasoningEfforts": []}
    ])).unwrap();
    let mapped = models_from_list(&models);
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].name, "GPT-5.5");
    assert!(
        matches!(&mapped[0].options[0], OptionDescriptor::Select { choices, default: Some(d), .. } if choices[1].label == "High" && d == "medium")
    );
}

mod fake_server {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// A scripted `codex app-server` over an in-memory pipe.
    async fn run_server(stream: tokio::io::DuplexStream, log: Arc<Mutex<Vec<Value>>>) {
        let (read, mut write) = tokio::io::split(stream);
        let mut lines = BufReader::new(read).lines();
        let mut turns = 0;
        let send = async |w: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>, v: Value| {
            w.write_all(format!("{v}\n").as_bytes()).await.unwrap();
            w.flush().await.unwrap();
        };
        let note = |method: &str, params: Value| json!({"method": method, "params": params});
        while let Ok(Some(line)) = lines.next_line().await {
            let msg: Value = serde_json::from_str(&line).unwrap();
            log.lock().unwrap().push(msg.clone());
            let id = msg.get("id").cloned();
            match msg.get("method").and_then(Value::as_str) {
                Some("thread/start") => {
                    send(
                        &mut write,
                        json!({"id": id, "result": {"thread": {"id": "th1"}, "model": "gpt-5.5"}}),
                    )
                    .await
                }
                Some("turn/start") => {
                    turns += 1;
                    let turn = format!("t{turns}");
                    send(&mut write, json!({"id": id, "result": {"turn": {"id": turn, "status": "inProgress", "items": []}}})).await;
                    send(&mut write, note("turn/started", json!({"threadId": "th1", "turn": {"id": turn, "status": "inProgress", "items": []}}))).await;
                    if turns == 1 {
                        send(&mut write, note("item/started", json!({"threadId": "th1", "turnId": "t1", "startedAtMs": 0,
                            "item": {"type": "commandExecution", "id": "c1", "command": "git push", "cwd": "/w", "commandActions": [], "status": "inProgress"}}))).await;
                        send(&mut write, json!({"id": 50, "method": "item/commandExecution/requestApproval", "params": {
                            "threadId": "th1", "turnId": "t1", "itemId": "c1", "startedAtMs": 0, "command": "git push", "cwd": "/w"}})).await;
                    } else {
                        send(&mut write, json!({"id": 51, "method": "item/tool/requestUserInput", "params": {
                            "threadId": "th1", "turnId": turn, "itemId": "q", "isBlocking": true,
                            "questions": [{"id": "db", "header": "Database", "question": "Which one?", "options": [{"label": "Postgres", "description": ""}, {"label": "SQLite", "description": ""}]}]}})).await;
                    }
                }
                Some("turn/steer") => {
                    send(
                        &mut write,
                        json!({"id": id, "result": {"turnId": msg["params"]["expectedTurnId"]}}),
                    )
                    .await
                }
                Some("turn/interrupt") => {
                    send(&mut write, json!({"id": id, "result": {}})).await;
                    let turn = msg["params"]["turnId"].clone();
                    send(
                        &mut write,
                        note(
                            "serverRequest/resolved",
                            json!({"threadId": "th1", "requestId": 51}),
                        ),
                    )
                    .await;
                    send(&mut write, note("turn/completed", json!({"threadId": "th1", "turn": {"id": turn, "status": "interrupted", "items": []}}))).await;
                }
                None if id == Some(json!(50)) => {
                    let decision = msg["result"]["decision"].as_str().unwrap().to_owned();
                    send(&mut write, note("item/completed", json!({"threadId": "th1", "turnId": "t1", "completedAtMs": 1,
                        "item": {"type": "commandExecution", "id": "c1", "command": "git push", "cwd": "/w", "commandActions": [],
                                 "aggregatedOutput": decision, "exitCode": 0, "status": "completed"}}))).await;
                    send(&mut write, note("thread/tokenUsage/updated", json!({"threadId": "th1", "turnId": "t1",
                        "tokenUsage": {"total": {"inputTokens": 10, "cachedInputTokens": 2, "outputTokens": 3, "reasoningOutputTokens": 0, "totalTokens": 13},
                                       "last": {"inputTokens": 10, "cachedInputTokens": 2, "outputTokens": 3, "reasoningOutputTokens": 0, "totalTokens": 13}}}))).await;
                    send(&mut write, note("turn/completed", json!({"threadId": "th1", "turn": {"id": "t1", "status": "completed", "items": []}}))).await;
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
    async fn approvals_questions_steer_and_interrupt() {
        let (client_side, server_side) = tokio::io::duplex(1 << 16);
        let log = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(run_server(server_side, log.clone()));
        let (read, write) = tokio::io::split(client_side);
        let (client, inbound) = RpcClient::start(write, read);
        let spec = SessionSpec {
            session_id: SessionId::new(),
            workspace: "/w".into(),
            runtime_dir: "/r".into(),
            model: String::new(),
            options: [("effort".to_owned(), OptionValue::Text("high".into()))]
                .into_iter()
                .collect(),
            approval_mode: ApprovalMode::Ask,
            system_prompt_append: String::new(),
            mcp_servers: vec![],
            env: BTreeMap::new(),
            resume: None,
        };
        let (session, mut events) =
            start_thread(spec, serde_json::Map::new(), client, inbound, None)
                .await
                .unwrap();
        let mut session: Box<dyn HarnessSession> = Box::new(session);
        assert_eq!(session.native_ref(), Some(NativeRef::new("th1")));
        {
            let log = log.lock().unwrap();
            let start = log.iter().find(|m| m["method"] == "thread/start").unwrap();
            assert_eq!(start["params"]["approvalPolicy"], "untrusted");
            assert_eq!(start["params"]["sandbox"], "read-only");
            assert!(
                start["params"].get("model").is_none(),
                "an empty model means the Codex default"
            );
        }

        let turn = session
            .turn(TurnInput {
                text: "Push".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        assert_eq!(turn, TurnId::new("t1"));
        let SessionEvent::RequestOpened(opened) =
            next_matching(&mut events, |e| matches!(e, SessionEvent::RequestOpened(_))).await
        else {
            unreachable!()
        };
        assert!(
            matches!(&opened.body, RequestBody::Approval(ApprovalDetails::Command { command, .. }) if command == "git push")
        );
        session
            .respond(
                opened.request_id.clone(),
                Answer {
                    option_id: Some("acceptForSession".into()),
                    text: None,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            next_matching(&mut events, |e| matches!(
                e,
                SessionEvent::RequestClosed { .. }
            ))
            .await,
            SessionEvent::RequestClosed {
                outcome: RequestOutcome::Allowed,
                ..
            }
        ));
        let usage = next_matching(&mut events, |e| matches!(e, SessionEvent::Usage(_))).await;
        assert!(
            matches!(usage, SessionEvent::Usage(u) if u.input_tokens == 10 && u.cached_tokens == 2)
        );
        assert!(matches!(
            next_matching(&mut events, |e| matches!(e, SessionEvent::TurnEnded { .. })).await,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
                ..
            }
        ));
        {
            let log = log.lock().unwrap();
            let answer = log
                .iter()
                .find(|m| m.get("id") == Some(&json!(50)) && m.get("result").is_some())
                .unwrap();
            assert_eq!(answer["result"]["decision"], "acceptForSession");
            let turn_start = log.iter().find(|m| m["method"] == "turn/start").unwrap();
            assert_eq!(turn_start["params"]["effort"], "high");
        }

        // A question, a steer, then an interrupt that withdraws the question.
        let second = session
            .turn(TurnInput {
                text: "Pick".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        let SessionEvent::RequestOpened(question) =
            next_matching(&mut events, |e| matches!(e, SessionEvent::RequestOpened(_))).await
        else {
            unreachable!()
        };
        assert!(matches!(&question.body, RequestBody::Question(q) if q.question == "Which one?"));
        assert!(
            session
                .respond(
                    question.request_id.clone(),
                    Answer {
                        option_id: None,
                        text: None
                    }
                )
                .await
                .is_err(),
            "an empty answer is rejected"
        );
        session
            .steer(TurnInput {
                text: "Prefer SQLite".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        session.interrupt().await.unwrap();
        assert!(matches!(
            next_matching(&mut events, |e| matches!(
                e,
                SessionEvent::RequestClosed { .. }
            ))
            .await,
            SessionEvent::RequestClosed {
                outcome: RequestOutcome::Canceled,
                ..
            }
        ));
        assert!(
            matches!(next_matching(&mut events, |e| matches!(e, SessionEvent::TurnEnded { .. })).await,
            SessionEvent::TurnEnded { reason: TurnEndReason::Interrupted, turn_id, .. } if turn_id == second)
        );
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|m| m["method"] == "turn/steer" && m["params"]["expectedTurnId"] == "t2")
        );
        session.close().await.unwrap();
    }
}
