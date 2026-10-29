//! The worker against a minimal fake hub (`common/mod.rs`), with the fake
//! harness.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{CODE, FakeHub};

use consort_harness::{AccountConfig, Harness, HarnessKind};
use consort_proto::ids::{AccountId, ChannelId, SessionId, TaskId};
use consort_proto::model::{ApprovalMode, AuthState, Isolation, ItemPayload, SessionRole};
use consort_proto::worker_link::{
    RequestAnswer, SessionEvent, SessionRespond, SessionStart, SessionStop, SessionTurn,
    TurnEndReason, TurnInput, WorkspaceSpec,
};
use consort_testkit::{FakeHarness, FakeScript, FakeStep};
use consort_worker::bridge::McpHttpClient;
use consort_worker::config::WorkerToml;
use consort_worker::{Harnesses, WorkerConfig};
use serde_json::json;

fn session_start(session_id: SessionId, account_id: AccountId) -> SessionStart {
    SessionStart {
        session_id,
        role: SessionRole::Task,
        channel_id: ChannelId::new(),
        task_id: Some(TaskId::new()),
        account_id,
        model: "fake-model".into(),
        options: Default::default(),
        approval_mode: ApprovalMode::Auto,
        workspace: WorkspaceSpec {
            repos: vec![],
            isolation: Isolation::Clone,
            carry_uncommitted: false,
        },
        system_prompt_append: "You are a task.".into(),
        allowed_local_mcp_servers: vec![],
        channel_files: vec![],
        resume: None,
        turn: Some(TurnInput {
            text: "Run the tests".into(),
            attachments: vec![],
        }),
    }
}

#[tokio::test]
async fn pair_connect_run_a_session_and_resend_after_reconnect() {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();

    // Pair.
    let bad = consort_worker::pair(&hub.url, "ZZZZ-ZZZZ", dir.path(), None, vec![]).await;
    assert!(bad.is_err());
    let worker_id = consort_worker::pair(
        &hub.url,
        "abcd-efgh",
        dir.path(),
        Some("test-worker".into()),
        vec!["ci".into()],
    )
    .await
    .unwrap();
    assert_eq!(Some(worker_id), *hub.state.worker_id.lock().unwrap());

    // Configure one account that the fake harness serves.
    let mut toml = WorkerToml::load(dir.path()).unwrap();
    assert_eq!(toml.worker_id, Some(worker_id));
    assert_eq!(toml.tags, vec!["ci".to_owned()]);
    toml.auto_discover = false;
    let account = AccountConfig::default_for(HarnessKind::ClaudeCode);
    let account_id = account.id;
    toml.accounts = vec![account];
    toml.save(dir.path()).unwrap();

    let script = FakeScript {
        turns: vec![
            vec![
                FakeStep::Command {
                    command: "cargo test".into(),
                    output: "ok\n".into(),
                },
                FakeStep::Approval {
                    request_id: "r1".into(),
                    command: "git push".into(),
                },
                FakeStep::Text("Done.".into()),
            ],
            vec![FakeStep::Text("Second turn.".into())],
        ],
    };
    let fake = FakeHarness::new(HarnessKind::ClaudeCode, script);
    let record = fake.record();
    let mut harnesses: Harnesses = HashMap::new();
    harnesses.insert(
        HarnessKind::ClaudeCode,
        Arc::new(fake.clone()) as Arc<dyn Harness>,
    );
    let worker = consort_worker::run_with(
        WorkerConfig {
            config_dir: dir.path().to_path_buf(),
        },
        harnesses,
    )
    .await
    .unwrap();
    assert_eq!(worker.worker_id(), worker_id);
    assert!(worker.wait_connected(Duration::from_secs(10)).await);
    hub.wait_connected().await;

    // Inventory after the probe.
    let inventory = hub
        .wait_for("a signed-in inventory", |h| {
            h.state
                .inventories
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|i| {
                    i.accounts
                        .first()
                        .is_some_and(|a| a.auth_state == AuthState::SignedIn)
                })
                .cloned()
        })
        .await;
    assert_eq!(inventory.accounts[0].label, "Claude Code");
    assert_eq!(inventory.accounts[0].harness, "claude");
    assert_eq!(inventory.accounts[0].models[0].id, "fake-model");
    assert_eq!(inventory.tags, vec!["ci".to_owned()]);
    assert_eq!(inventory.hash, inventory.compute_hash());

    // Start a session with a first turn.
    let session_id = SessionId::new();
    let start = session_start(session_id, account_id);
    let response = hub.command("cmd-1", start.clone().into()).await;
    let body = response.result.expect("session.start succeeds");
    let workspace_path = body["workspace_path"].as_str().unwrap().to_owned();
    assert!(std::path::Path::new(&workspace_path).is_dir());
    assert_eq!(body["native_ref"], format!("fake-{session_id}"));

    // The same command ID returns the stored result and does not run again.
    let again = hub.command("cmd-1", start.into()).await;
    assert_eq!(again.result.unwrap(), body);
    assert_eq!(record.lock().unwrap().opened.len(), 1);
    {
        let opened = &record.lock().unwrap().opened[0];
        assert_eq!(opened.approval_mode, ApprovalMode::Auto);
        assert_eq!(opened.system_prompt_append, "You are a task.");
        assert_eq!(opened.mcp_servers[0].name, "hub");
    }

    // The approval opens, the hub answers it, and the turn ends.
    let request = hub
        .wait_for("request.opened", |h| {
            h.events(session_id)
                .into_iter()
                .find_map(|e| match e.event {
                    SessionEvent::RequestOpened(r) => Some(r),
                    _ => None,
                })
        })
        .await;
    assert_eq!(request.request_id.as_str(), "r1");
    let answer = hub
        .command(
            "cmd-2",
            SessionRespond {
                session_id,
                request_id: request.request_id.clone(),
                answer: RequestAnswer {
                    option_id: Some("allow".into()),
                    text: None,
                },
            }
            .into(),
        )
        .await;
    assert!(answer.result.is_ok());
    hub.wait_for("turn.ended", |h| {
        h.events(session_id)
            .iter()
            .any(|e| {
                matches!(
                    e.event,
                    SessionEvent::TurnEnded {
                        reason: TurnEndReason::Completed,
                        ..
                    }
                )
            })
            .then_some(())
    })
    .await;
    let events = hub.events(session_id);
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(
        seqs,
        (1..=seqs.len() as u64).collect::<Vec<_>>(),
        "sequence numbers start at 1 with no gaps"
    );
    assert!(matches!(
        events[0].event,
        SessionEvent::State {
            state: consort_proto::model::SessionState::Starting
        }
    ));
    let texts: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.event {
            SessionEvent::Item(i) => match &i.payload {
                ItemPayload::AssistantMessage(m) => Some(m.text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["Done.".to_owned()]);
    assert_eq!(record.lock().unwrap().answers.len(), 1);

    // A hub tool call through the session's MCP endpoint.
    let (url, token) = {
        let r = record.lock().unwrap();
        let hub_server = &r.opened[0].mcp_servers[0];
        (
            hub_server.url.clone(),
            hub_server.bearer_token.clone().unwrap(),
        )
    };
    let client = McpHttpClient::new(&url, &token).unwrap();
    client
        .post(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}))
        .await
        .unwrap();
    let tools = client
        .post(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .await
        .unwrap();
    assert_eq!(tools[0]["result"]["tools"].as_array().unwrap().len(), 2);
    let call = client
        .post(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "post_update", "arguments": {"body": "Halfway."}}}))
        .await
        .unwrap();
    assert_eq!(call[0]["result"]["structuredContent"]["ok"], true);
    {
        let calls = hub.state.tool_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].session_id, session_id);
        assert_eq!(calls[0].name, "post_update");
        assert_eq!(calls[0].arguments["body"], "Halfway.");
    }

    // The hub stops storing events. A second turn spools on the worker.
    hub.state.reject_events.store(true, Ordering::SeqCst);
    let stored_before = hub.events(session_id).len();
    let turn = hub
        .command(
            "cmd-3",
            SessionTurn {
                session_id,
                turn: TurnInput {
                    text: "Again".into(),
                    attachments: vec![],
                },
            }
            .into(),
        )
        .await;
    assert!(turn.result.is_ok());
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        hub.events(session_id).len(),
        stored_before,
        "rejected events are not stored"
    );

    // The connection drops. The worker reconnects, lists the session in
    // hello, and resends what the hub does not have.
    hub.state.reject_events.store(false, Ordering::SeqCst);
    hub.state.kick.notify_waiters();
    hub.wait_for("a second hello", |h| {
        (h.state.hellos.lock().unwrap().len() >= 2).then_some(())
    })
    .await;
    let hello = hub.state.hellos.lock().unwrap()[1].clone();
    let open = hello
        .sessions
        .iter()
        .find(|s| s.session_id == session_id)
        .expect("hello lists the open session");
    assert!(open.last_seq > stored_before as u64);
    hub.wait_for("the resent second turn", |h| {
        h.events(session_id).iter().any(|e| matches!(&e.event, SessionEvent::Item(i) if matches!(&i.payload, ItemPayload::AssistantMessage(m) if m.text == "Second turn."))).then_some(())
    })
    .await;
    let events = hub.events(session_id);
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());

    // An unknown session gets not_found. Stopping the session closes the harness.
    let unknown = hub
        .command(
            "cmd-4",
            SessionTurn {
                session_id: SessionId::new(),
                turn: TurnInput::default(),
            }
            .into(),
        )
        .await;
    assert_eq!(
        unknown.result.unwrap_err().code,
        consort_proto::error::ErrorCode::NotFound
    );
    let stop = hub
        .command(
            "cmd-5",
            SessionStop {
                session_id,
                reason: "done".into(),
            }
            .into(),
        )
        .await;
    assert!(stop.result.is_ok());
    assert_eq!(record.lock().unwrap().closed, 1);

    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn sessions_resume_after_a_worker_restart() {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    consort_worker::pair(&hub.url, CODE, dir.path(), None, vec![])
        .await
        .unwrap();
    let mut toml = WorkerToml::load(dir.path()).unwrap();
    toml.auto_discover = false;
    let account = AccountConfig::default_for(HarnessKind::Codex);
    let account_id = account.id;
    toml.accounts = vec![account];
    toml.save(dir.path()).unwrap();

    let fake = FakeHarness::new(HarnessKind::Codex, FakeScript::reply("Hi."));
    let record = fake.record();
    let harnesses = || {
        let mut h: Harnesses = HashMap::new();
        h.insert(
            HarnessKind::Codex,
            Arc::new(fake.clone()) as Arc<dyn Harness>,
        );
        h
    };
    let config = WorkerConfig {
        config_dir: dir.path().to_path_buf(),
    };
    let worker = consort_worker::run_with(config.clone(), harnesses())
        .await
        .unwrap();
    hub.wait_connected().await;
    // A second worker on the same folder refuses to start.
    let second = consort_worker::run_with(config.clone(), harnesses()).await;
    assert!(
        matches!(second, Err(consort_worker::WorkerError::AlreadyRunning(_))),
        "{second:?}"
    );
    let session_id = SessionId::new();
    let mut start = session_start(session_id, account_id);
    start.turn = None;
    assert!(hub.command("start-1", start.into()).await.result.is_ok());
    hub.wait_for("the native ref", |h| {
        h.events(session_id)
            .iter()
            .any(|e| matches!(e.event, SessionEvent::NativeRef { .. }))
            .then_some(())
    })
    .await;
    worker.shutdown().await.unwrap();
    hub.wait_for("the old link to close", |h| {
        h.state.conn.lock().unwrap().is_none().then_some(())
    })
    .await;

    let worker = consort_worker::run_with(config, harnesses()).await.unwrap();
    hub.wait_connected().await;
    hub.wait_for("the resumed open", |_| {
        (record.lock().unwrap().opened.len() == 2).then_some(())
    })
    .await;
    let resumed = record.lock().unwrap().opened[1].clone();
    assert_eq!(
        resumed.resume.as_ref().map(|r| r.as_str().to_owned()),
        Some(format!("fake-{session_id}"))
    );
    assert_eq!(resumed.session_id, session_id);
    // The hub learns that the old harness process and its turn are gone.
    hub.wait_for("session reopened", |h| {
        h.events(session_id)
            .iter()
            .any(|e| matches!(e.event, SessionEvent::Reopened))
            .then_some(())
    })
    .await;
    let turn = hub
        .command(
            "turn-1",
            SessionTurn {
                session_id,
                turn: TurnInput {
                    text: "Still there?".into(),
                    attachments: vec![],
                },
            }
            .into(),
        )
        .await;
    assert!(turn.result.is_ok(), "{:?}", turn.result);
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_worker_allows_hub_tools_without_asking_the_hub() {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    consort_worker::pair(&hub.url, CODE, dir.path(), None, vec![])
        .await
        .unwrap();
    let mut toml = WorkerToml::load(dir.path()).unwrap();
    toml.auto_discover = false;
    let account = AccountConfig::default_for(HarnessKind::ClaudeCode);
    let account_id = account.id;
    toml.accounts = vec![account];
    toml.save(dir.path()).unwrap();

    let script = FakeScript {
        turns: vec![vec![
            FakeStep::ToolApproval {
                request_id: "hub-1".into(),
                server: "hub".into(),
                tool: "post_update".into(),
            },
            // `mcp__hub__x__post_update` from a local server named `hub__x`
            // parses as the `hub` server. It is not a listed hub tool.
            FakeStep::ToolApproval {
                request_id: "spoof-1".into(),
                server: "hub".into(),
                tool: "x__post_update".into(),
            },
            FakeStep::ToolApproval {
                request_id: "other-1".into(),
                server: "github".into(),
                tool: "create_issue".into(),
            },
            FakeStep::Text("Posted.".into()),
        ]],
    };
    let fake = FakeHarness::new(HarnessKind::ClaudeCode, script);
    let record = fake.record();
    let mut harnesses: Harnesses = HashMap::new();
    harnesses.insert(
        HarnessKind::ClaudeCode,
        Arc::new(fake.clone()) as Arc<dyn Harness>,
    );
    let worker = consort_worker::run_with(
        WorkerConfig {
            config_dir: dir.path().to_path_buf(),
        },
        harnesses,
    )
    .await
    .unwrap();
    hub.wait_connected().await;
    let session_id = SessionId::new();
    let mut start = session_start(session_id, account_id);
    start.turn = None;
    assert!(hub.command("start-1", start.into()).await.result.is_ok());

    // The harness lists the hub tools, as a real harness does when it
    // starts. The worker allows only the tools in this list.
    let (url, token) = {
        let r = record.lock().unwrap();
        let hub_server = &r.opened[0].mcp_servers[0];
        (
            hub_server.url.clone(),
            hub_server.bearer_token.clone().unwrap(),
        )
    };
    let client = McpHttpClient::new(&url, &token).unwrap();
    client
        .post(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}))
        .await
        .unwrap();
    client
        .post(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .await
        .unwrap();
    let turn = hub
        .command(
            "turn-1",
            SessionTurn {
                session_id,
                turn: TurnInput {
                    text: "Post it".into(),
                    attachments: vec![],
                },
            }
            .into(),
        )
        .await;
    assert!(turn.result.is_ok());

    // The listed hub tool is allowed on the worker. The hub sees the
    // spoofed one first.
    let opened = |h: &FakeHub| -> Vec<String> {
        h.events(session_id)
            .into_iter()
            .filter_map(|e| match e.event {
                SessionEvent::RequestOpened(r) => Some(r.request_id.as_str().to_owned()),
                _ => None,
            })
            .collect()
    };
    hub.wait_for("the spoofed request", |h| {
        (!opened(h).is_empty()).then_some(())
    })
    .await;
    assert_eq!(opened(&hub), vec!["spoof-1".to_owned()]);
    let answer = hub
        .command(
            "answer-1",
            SessionRespond {
                session_id,
                request_id: consort_proto::ids::RequestId::new("spoof-1"),
                answer: RequestAnswer {
                    option_id: Some("deny".into()),
                    text: None,
                },
            }
            .into(),
        )
        .await;
    assert!(answer.result.is_ok());
    hub.wait_for("the second request", |h| {
        (opened(h).len() == 2).then_some(())
    })
    .await;
    assert_eq!(
        opened(&hub),
        vec!["spoof-1".to_owned(), "other-1".to_owned()]
    );
    let hub_tool_closed = hub
        .events(session_id)
        .into_iter()
        .find_map(|e| match e.event {
            SessionEvent::RequestClosed {
                request_id,
                outcome,
            } if request_id.as_str() == "hub-1" => Some(outcome),
            _ => None,
        });
    assert_eq!(
        hub_tool_closed,
        Some(consort_proto::worker_link::RequestOutcome::Allowed)
    );
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_signed_out_session_makes_the_worker_send_a_signed_out_inventory() {
    let hub = FakeHub::start().await;
    let dir = tempfile::tempdir().unwrap();
    consort_worker::pair(&hub.url, CODE, dir.path(), None, vec![])
        .await
        .unwrap();
    let mut toml = WorkerToml::load(dir.path()).unwrap();
    toml.auto_discover = false;
    let account = AccountConfig::default_for(HarnessKind::ClaudeCode);
    let account_id = account.id;
    toml.accounts = vec![account];
    toml.save(dir.path()).unwrap();

    // The probe says "signed in", like the real probe of an expired Claude
    // Code login. The first prompt then reports that the login is gone.
    let hint = "Authentication required. Run `claude` on this machine and sign in with /login.";
    let script = FakeScript {
        turns: vec![vec![
            FakeStep::Event(SessionEvent::Error {
                code: consort_harness::SIGNED_OUT_ERROR_CODE.to_owned(),
                message: hint.to_owned(),
            }),
            FakeStep::Fail(hint.to_owned()),
        ]],
    };
    let fake = FakeHarness::new(HarnessKind::ClaudeCode, script);
    let mut harnesses: Harnesses = HashMap::new();
    harnesses.insert(
        HarnessKind::ClaudeCode,
        Arc::new(fake.clone()) as Arc<dyn Harness>,
    );
    let worker = consort_worker::run_with(
        WorkerConfig {
            config_dir: dir.path().to_path_buf(),
        },
        harnesses,
    )
    .await
    .unwrap();
    hub.wait_connected().await;
    hub.wait_for("a signed-in inventory", |h| {
        h.state
            .inventories
            .lock()
            .unwrap()
            .iter()
            .any(|i| {
                i.accounts
                    .first()
                    .is_some_and(|a| a.auth_state == AuthState::SignedIn)
            })
            .then_some(())
    })
    .await;

    let session_id = SessionId::new();
    let start = session_start(session_id, account_id);
    assert!(hub.command("start-1", start.into()).await.result.is_ok());

    // The newest inventory shows the account as signed out, with the hint.
    let inventory = hub
        .wait_for("a signed-out inventory", |h| {
            h.state
                .inventories
                .lock()
                .unwrap()
                .last()
                .filter(|i| {
                    i.accounts
                        .first()
                        .is_some_and(|a| a.auth_state == AuthState::SignedOut)
                })
                .cloned()
        })
        .await;
    let account = &inventory.accounts[0];
    assert_eq!(account.auth_detail.as_deref(), Some(hint));
    assert_eq!(account.models[0].id, "fake-model", "the models stay");
    assert!(
        !account.can_run("fake-model"),
        "the hub does not place work on a signed-out account"
    );
    assert_eq!(inventory.hash, inventory.compute_hash());

    // The hub also gets the error event and the failed turn with the hint.
    hub.wait_for("the failed turn", |h| {
        let events = h.events(session_id);
        let error = events.iter().any(|e| {
            matches!(&e.event, SessionEvent::Error { code, message }
                if code == consort_harness::SIGNED_OUT_ERROR_CODE && message == hint)
        });
        let failed = events.iter().any(|e| {
            matches!(&e.event, SessionEvent::TurnEnded { reason: TurnEndReason::Failed, error: Some(text), .. }
                if text == hint)
        });
        (error && failed).then_some(())
    })
    .await;
    worker.shutdown().await.unwrap();
}
