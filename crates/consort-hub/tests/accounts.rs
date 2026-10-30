//! Account, sign-in and tool requests: the hub relays them to the worker
//! that owns the account, and keeps secret values out of the database, the
//! feed, the logs and the error messages.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::sync::{Mutex, Once};
use std::time::Duration;

use crate::common::*;
use consort_hub::store;
use consort_proto::client_api::{
    AccountCreate, AccountRemove, AccountSignOut, AccountUpdate, EntityPayload, SignInCancel,
    SignInStart, SignInSubmitCode, WorkerList, WorkerToolInstall, WorkerToolsCheck,
};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{EventFrame, Frame, Request};
use consort_proto::ids::{AccountId, SignInId, WorkerId};
use consort_proto::model::{
    AcpAccountSpec, SecretEnv, SignIn, SignInMethod, SignInState, ToolId, WorkerState,
};
use consort_proto::worker_link::{HubToWorker, SignInUpdate};
use serde_json::json;

// Capturing the logs --------------------------------------------------------

static LOGS: Mutex<Vec<u8>> = Mutex::new(Vec::new());

struct Capture;

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        LOGS.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Sends every log line of the process, at every level, to [`LOGS`].
fn capture_logs() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(|| Capture)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

fn logs() -> String {
    String::from_utf8_lossy(&LOGS.lock().unwrap()).into_owned()
}

/// True when the database files of the hub hold these bytes.
fn database_holds(hub: &TestHub, needle: &str) -> bool {
    ["hub.db", "hub.db-wal"].iter().any(|name| {
        std::fs::read(hub.dir.path().join(name))
            .map(|bytes| {
                bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_bytes())
            })
            .unwrap_or(false)
    })
}

// Helpers ---------------------------------------------------------------------

/// Sends a client request while the worker answers its next command. The
/// answer is `Ok` or the error that the worker sends.
async fn ask<R: Request>(
    world: &mut World,
    body: R,
    answer: Result<(), ErrorBody>,
) -> (Result<R::Response, ErrorBody>, HubToWorker) {
    let World { client, worker, .. } = world;
    tokio::join!(client.request(body), async {
        let (request, command) = worker.next_command().await;
        match answer {
            Ok(()) => worker.respond_ok(&request, &json!({})).await,
            Err(error) => worker.respond_error(&request, error).await,
        }
        command
    })
}

fn env(pairs: &[(&str, &str)]) -> SecretEnv {
    pairs.iter().copied().collect()
}

fn account_create(worker_id: WorkerId, label: &str, secrets: SecretEnv) -> AccountCreate {
    AccountCreate {
        worker_id,
        harness: "claude".to_owned(),
        label: label.to_owned(),
        acp: None,
        env: secrets,
    }
}

async fn send_update(worker: &mut FakeWorker, sign_in_id: SignInId, state: SignInState) {
    worker
        .peer
        .send(&Frame::Event(EventFrame {
            kind: "sign_in.update".to_owned(),
            body: serde_json::to_value(SignInUpdate { sign_in_id, state }).unwrap(),
        }))
        .await;
}

fn sign_in_entry(
    id: SignInId,
    wanted: impl Fn(&SignInState) -> bool,
) -> impl FnMut(&consort_proto::client_api::FeedEntry) -> bool {
    move |entry| matches!(upserted(entry), Some(EntityPayload::SignIn(s)) if s.id == id && wanted(&s.state))
}

/// The stored sign-ins, read from the database file.
fn stored_sign_ins(hub: &TestHub) -> Vec<SignIn> {
    let conn = store::open_and_migrate(hub.dir.path()).unwrap();
    store::sign_ins::sign_ins(&conn).unwrap()
}

async fn wait_offline(world: &mut World, id: WorkerId) {
    world.wait_worker(id, WorkerState::Offline).await;
}

// Accounts --------------------------------------------------------------------

#[tokio::test]
async fn account_create_sends_the_command_with_an_id_that_the_hub_chose() {
    let mut world = World::start().await;
    let worker_id = world.worker.worker_id;
    let request = AccountCreate {
        worker_id,
        harness: "acp".to_owned(),
        label: "  Goose  ".to_owned(),
        acp: Some(AcpAccountSpec::Registry {
            id: "goose".to_owned(),
        }),
        env: env(&[("GOOSE_KEY", "value-1")]),
    };
    let (result, command) = ask(&mut world, request, Ok(())).await;
    let HubToWorker::AccountCreate(command) = command else {
        panic!("expected account.create, got {command:?}");
    };
    assert_eq!(result.unwrap().account_id, command.account_id);
    assert_eq!(command.harness, "acp");
    assert_eq!(command.label, "Goose");
    assert_eq!(
        command.acp,
        Some(AcpAccountSpec::Registry {
            id: "goose".to_owned()
        })
    );
    assert_eq!(command.env.get("GOOSE_KEY"), Some("value-1"));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn account_create_checks_the_request_before_it_asks_the_worker() {
    let mut world = World::start().await;
    let worker_id = world.worker.worker_id;
    let bad = [
        account_create(worker_id, "   ", SecretEnv::new()),
        account_create(worker_id, "Name", env(&[("1BAD", "x")])),
        AccountCreate {
            harness: "acp".to_owned(),
            ..account_create(worker_id, "Name", SecretEnv::new())
        },
        AccountCreate {
            acp: Some(AcpAccountSpec::Registry { id: "x".into() }),
            ..account_create(worker_id, "Name", SecretEnv::new())
        },
    ];
    for request in bad {
        let error = world.client.request(request).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Invalid, "{error}");
    }
    assert!(
        world
            .worker
            .try_next_command(Duration::from_millis(200))
            .await
            .is_none()
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn account_update_remove_and_sign_out_reach_the_worker() {
    let mut world = World::start().await;
    let worker_id = world.worker.worker_id;
    let account_id = AccountId::new();

    let update = AccountUpdate {
        worker_id,
        account_id,
        label: Some("Renamed".to_owned()),
        env_set: env(&[("KEY", "new-value")]),
        env_remove: vec!["OLD".to_owned()],
        extras_enabled: None,
    };
    let (result, command) = ask(&mut world, update, Ok(())).await;
    result.unwrap();
    let HubToWorker::AccountUpdate(command) = command else {
        panic!("expected account.update, got {command:?}");
    };
    assert_eq!(command.account_id, account_id);
    assert_eq!(command.label.as_deref(), Some("Renamed"));
    assert_eq!(command.env_set.get("KEY"), Some("new-value"));
    assert_eq!(command.env_remove, ["OLD"]);

    let (result, command) = ask(
        &mut world,
        AccountSignOut {
            worker_id,
            account_id,
        },
        Ok(()),
    )
    .await;
    result.unwrap();
    assert!(
        matches!(command, HubToWorker::AccountSignOut(c) if c.account_id == account_id),
        "wrong command"
    );

    let (result, command) = ask(
        &mut world,
        AccountRemove {
            worker_id,
            account_id,
        },
        Ok(()),
    )
    .await;
    result.unwrap();
    assert!(
        matches!(command, HubToWorker::AccountRemove(c) if c.account_id == account_id),
        "wrong command"
    );

    // An update that changes nothing is refused at the hub.
    let nothing = AccountUpdate {
        worker_id,
        account_id,
        label: None,
        env_set: SecretEnv::new(),
        env_remove: Vec::new(),
        extras_enabled: None,
    };
    let error = world.client.request(nothing).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::Invalid);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn tool_requests_reach_the_worker() {
    let mut world = World::start().await;
    let worker_id = world.worker.worker_id;
    let (result, command) = ask(&mut world, WorkerToolsCheck { worker_id }, Ok(())).await;
    result.unwrap();
    assert!(matches!(command, HubToWorker::ToolsCheck(_)));

    let install = WorkerToolInstall {
        worker_id,
        tool: ToolId::claude_code(),
    };
    let (result, command) = ask(&mut world, install, Ok(())).await;
    result.unwrap();
    assert!(matches!(command, HubToWorker::ToolsInstall(c) if c.tool == ToolId::claude_code()));

    let unknown = WorkerToolInstall {
        worker_id,
        tool: ToolId::new("vim"),
    };
    let error = world.client.request(unknown).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::Invalid);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_request_for_an_offline_worker_fails_and_is_never_queued() {
    let mut world = World::start().await;
    let worker_id = world.worker.worker_id;
    let placeholder = placeholder_worker(&world.hub).await;
    let gone = std::mem::replace(&mut world.worker, placeholder)
        .go_away()
        .await;
    wait_offline(&mut world, worker_id).await;

    let account_id = AccountId::new();
    let errors = [
        world
            .client
            .request(account_create(worker_id, "Claude", SecretEnv::new()))
            .await
            .unwrap_err(),
        world
            .client
            .request(AccountSignOut {
                worker_id,
                account_id,
            })
            .await
            .unwrap_err(),
        world
            .client
            .request(WorkerToolsCheck { worker_id })
            .await
            .unwrap_err(),
        world
            .client
            .request(SignInStart {
                worker_id,
                account_id,
                method: SignInMethod::ClaudePlan,
            })
            .await
            .unwrap_err(),
    ];
    for error in &errors {
        assert_eq!(error.code, ErrorCode::WorkerOffline, "{error}");
    }
    // No sign-in record exists for a worker that could not run it.
    assert!(stored_sign_ins(&world.hub).is_empty());

    // When the worker comes back, it finds nothing that waited for it.
    let mut back = gone.come_back(&world.hub).await;
    assert!(
        back.try_next_command(Duration::from_millis(500))
            .await
            .is_none()
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_request_for_an_unknown_worker_fails_with_worker_unknown() {
    let mut world = World::start().await;
    let error = world
        .client
        .request(WorkerToolsCheck {
            worker_id: WorkerId::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::WorkerUnknown);
    let error = world
        .client
        .request(account_create(WorkerId::new(), "A", SecretEnv::new()))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::WorkerUnknown);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn worker_errors_become_client_errors() {
    let mut world = World::start().await;
    let worker_id = world.worker.worker_id;

    // A code that a client understands passes through with its message.
    let refused = ErrorBody::new(ErrorCode::Conflict, "An account named Claude exists.");
    let (result, _) = ask(
        &mut world,
        account_create(worker_id, "Claude", SecretEnv::new()),
        Err(refused.clone()),
    )
    .await;
    assert_eq!(result.unwrap_err(), refused);

    // An internal failure of the worker takes the code of the request.
    let broken = ErrorBody::new(ErrorCode::Internal, "The disk is full.");
    let (result, _) = ask(
        &mut world,
        account_create(worker_id, "Claude", SecretEnv::new()),
        Err(broken.clone()),
    )
    .await;
    assert_eq!(result.unwrap_err(), broken);

    let install = WorkerToolInstall {
        worker_id,
        tool: ToolId::codex(),
    };
    let (result, _) = ask(&mut world, install, Err(broken.clone())).await;
    let error = result.unwrap_err();
    assert_eq!(error.code, ErrorCode::ToolInstallFailed);
    assert_eq!(error.message, "The disk is full.");

    // A code that only means something on the link also becomes the code
    // of the request.
    let stale = ErrorBody::new(ErrorCode::Unauthorized, "Not allowed.");
    let (result, _) = ask(&mut world, WorkerToolsCheck { worker_id }, Err(stale)).await;
    assert_eq!(result.unwrap_err().code, ErrorCode::ToolInstallFailed);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn secret_values_stay_out_of_the_database_the_feed_the_logs_and_errors() {
    capture_logs();
    let mut world = World::start().await;
    let worker_id = world.worker.worker_id;
    let created_secret = "sk-created-7f3a91c2d4e8";
    let updated_secret = "sk-updated-52b1e07a96cd";
    let echoed_secret = "sk-echoed-0d9c4b8e1f37";

    let (result, command) = ask(
        &mut world,
        account_create(
            worker_id,
            "API",
            env(&[("ANTHROPIC_API_KEY", created_secret)]),
        ),
        Ok(()),
    )
    .await;
    let created = result.unwrap().account_id;
    // The value reaches the worker. That is the point of the request.
    let HubToWorker::AccountCreate(command) = command else {
        panic!("expected account.create");
    };
    assert_eq!(command.env.get("ANTHROPIC_API_KEY"), Some(created_secret));

    let update = AccountUpdate {
        worker_id,
        account_id: created,
        label: None,
        env_set: env(&[("ANTHROPIC_API_KEY", updated_secret)]),
        env_remove: Vec::new(),
        extras_enabled: None,
    };
    let (result, command) = ask(&mut world, update, Ok(())).await;
    result.unwrap();
    let HubToWorker::AccountUpdate(command) = command else {
        panic!("expected account.update");
    };
    assert_eq!(
        command.env_set.get("ANTHROPIC_API_KEY"),
        Some(updated_secret)
    );

    // A worker that repeats the value in its error does not make the hub
    // repeat it to the client.
    let echo = ErrorBody::new(
        ErrorCode::Invalid,
        format!("The key {echoed_secret} is not accepted."),
    );
    let (result, _) = ask(
        &mut world,
        account_create(worker_id, "Echo", env(&[("OPENAI_API_KEY", echoed_secret)])),
        Err(echo),
    )
    .await;
    let error = result.unwrap_err();
    assert_eq!(error.code, ErrorCode::Invalid);
    assert!(!error.message.contains(echoed_secret), "{}", error.message);
    assert!(error.message.contains("<redacted>"), "{}", error.message);

    // The client feed carries no secret.
    let mut feed = String::new();
    while let Some(frame) = world
        .client
        .peer
        .next_frame(Duration::from_millis(300))
        .await
    {
        feed.push_str(&frame.to_text().unwrap());
    }
    // Give the writer a moment to flush the log and the WAL.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let all_logs = logs();
    for secret in [created_secret, updated_secret, echoed_secret] {
        assert!(!all_logs.contains(secret), "a log line holds {secret}");
        assert!(!feed.contains(secret), "the feed holds {secret}");
        assert!(
            !database_holds(&world.hub, secret),
            "the database holds {secret}"
        );
    }
    // The logs name the variable.
    assert!(all_logs.contains("ANTHROPIC_API_KEY"));
    world.hub.handle.shutdown().await.unwrap();
}

// Sign-ins --------------------------------------------------------------------

/// Starts a sign-in and returns it. The worker accepts the start command.
async fn start_sign_in(world: &mut World, method: SignInMethod) -> SignIn {
    let worker_id = world.worker.worker_id;
    let account_id = world.worker.account_id;
    let request = SignInStart {
        worker_id,
        account_id,
        method: method.clone(),
    };
    let (result, command) = ask(world, request, Ok(())).await;
    let sign_in = result.unwrap().sign_in;
    let HubToWorker::SignInStart(command) = command else {
        panic!("expected sign_in.start, got {command:?}");
    };
    assert_eq!(command.sign_in_id, sign_in.id);
    assert_eq!(command.account_id, account_id);
    assert_eq!(command.method, method);
    sign_in
}

#[tokio::test]
async fn sign_in_start_stores_a_starting_record_and_sends_a_feed_upsert() {
    let mut world = World::start().await;
    let sign_in = start_sign_in(&mut world, SignInMethod::ClaudePlan).await;
    assert_eq!(sign_in.state, SignInState::Starting);
    assert_eq!(sign_in.worker_id, world.worker.worker_id);
    assert_eq!(sign_in.method, SignInMethod::ClaudePlan);
    assert!(sign_in.expires_at > sign_in.updated_at);
    assert!(sign_in.id.to_string().starts_with("sgn_"));
    world
        .client
        .wait_feed(sign_in_entry(sign_in.id, |s| *s == SignInState::Starting))
        .await;
    let stored = stored_sign_ins(&world.hub);
    assert_eq!(stored, vec![sign_in]);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn updates_from_the_worker_change_the_record_and_reach_the_feed() {
    let mut world = World::start().await;
    let sign_in = start_sign_in(&mut world, SignInMethod::ClaudePlan).await;
    let needs_code = SignInState::NeedsCode {
        url: "https://claude.ai/oauth".to_owned(),
    };
    send_update(&mut world.worker, sign_in.id, needs_code.clone()).await;
    world
        .client
        .wait_feed(sign_in_entry(sign_in.id, |s| *s == needs_code.clone()))
        .await;
    let stored = stored_sign_ins(&world.hub);
    assert_eq!(stored[0].state, needs_code);
    assert!(stored[0].updated_at >= sign_in.updated_at);
    assert!(stored[0].expires_at >= sign_in.expires_at);

    let done = SignInState::Succeeded {
        detail: Some("me@example.com".to_owned()),
    };
    send_update(&mut world.worker, sign_in.id, done.clone()).await;
    world
        .client
        .wait_feed(sign_in_entry(sign_in.id, |s| *s == done.clone()))
        .await;
    // A late update after the end changes nothing.
    send_update(
        &mut world.worker,
        sign_in.id,
        SignInState::Failed {
            message: "late".to_owned(),
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(stored_sign_ins(&world.hub)[0].state, done);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_snapshot_has_the_running_sign_ins_and_not_the_ended_ones() {
    use consort_proto::client_api::SnapshotScope;
    let mut world = World::start().await;
    let running = start_sign_in(&mut world, SignInMethod::ClaudePlan).await;
    let needs_code = SignInState::NeedsCode {
        url: "https://claude.ai/oauth".to_owned(),
    };
    send_update(&mut world.worker, running.id, needs_code.clone()).await;
    world
        .client
        .wait_feed(sign_in_entry(running.id, |s| *s == needs_code.clone()))
        .await;
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    assert_eq!(snapshot.sign_ins.len(), 1);
    assert_eq!(snapshot.sign_ins[0].id, running.id);
    assert_eq!(snapshot.sign_ins[0].state, needs_code);

    // The sign-in ends, and the next snapshot leaves it out.
    let done = SignInState::Succeeded { detail: None };
    send_update(&mut world.worker, running.id, done.clone()).await;
    world
        .client
        .wait_feed(sign_in_entry(running.id, |s| *s == done.clone()))
        .await;
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    assert!(snapshot.sign_ins.is_empty(), "{:?}", snapshot.sign_ins);
    // A channel snapshot never has sign-ins.
    let tutti = world.client.tutti().await;
    let channel = world.client.snapshot(SnapshotScope::Channel(tutti)).await;
    assert!(channel.sign_ins.is_empty());
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn updates_for_unknown_sign_ins_or_from_another_worker_are_ignored() {
    let mut world = World::start().await;
    let mut other = FakeWorker::pair_and_connect(&world.hub).await;
    let sign_in = start_sign_in(&mut world, SignInMethod::ClaudePlan).await;

    // Another worker cannot change the sign-in. An unknown ID changes
    // nothing.
    send_update(&mut other, sign_in.id, SignInState::Verifying).await;
    send_update(&mut world.worker, SignInId::new(), SignInState::Verifying).await;
    let real = SignInState::OpenUrl {
        url: "https://example.test/login".to_owned(),
    };
    send_update(&mut world.worker, sign_in.id, real.clone()).await;
    world
        .client
        .wait_feed(sign_in_entry(sign_in.id, |s| *s == real.clone()))
        .await;

    // The feed shows the start and the real update. Nothing else.
    let states: Vec<SignInState> = world
        .client
        .feed
        .iter()
        .filter_map(|entry| match upserted(entry) {
            Some(EntityPayload::SignIn(s)) => Some(s.state.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(states, vec![SignInState::Starting, real]);
    assert_eq!(stored_sign_ins(&world.hub).len(), 1);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn submit_code_forwards_the_code_and_never_stores_or_logs_it() {
    capture_logs();
    let mut world = World::start().await;
    let sign_in = start_sign_in(&mut world, SignInMethod::ClaudePlan).await;

    // A code before the page asks for it is refused.
    let early = SignInSubmitCode {
        sign_in_id: sign_in.id,
        code: "too-early".to_owned(),
    };
    let error = world.client.request(early).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);

    send_update(
        &mut world.worker,
        sign_in.id,
        SignInState::NeedsCode {
            url: "https://claude.ai/oauth".to_owned(),
        },
    )
    .await;
    world
        .client
        .wait_feed(sign_in_entry(sign_in.id, |s| {
            matches!(s, SignInState::NeedsCode { .. })
        }))
        .await;

    let code = "code-4d1f8a7b9c20e3";
    let request = SignInSubmitCode {
        sign_in_id: sign_in.id,
        code: code.to_owned(),
    };
    let (result, command) = ask(&mut world, request, Ok(())).await;
    result.unwrap();
    let HubToWorker::SignInCode(command) = command else {
        panic!("expected sign_in.code, got {command:?}");
    };
    assert_eq!(command.sign_in_id, sign_in.id);
    assert_eq!(command.code, code);

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!logs().contains(code), "a log line holds the code");
    assert!(
        !database_holds(&world.hub, code),
        "the database holds the code"
    );
    assert!(!database_holds(&world.hub, "too-early"));

    // An unknown sign-in is not found.
    let missing = SignInSubmitCode {
        sign_in_id: SignInId::new(),
        code: "x".to_owned(),
    };
    let error = world.client.request(missing).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancel_forwards_to_the_worker_and_sets_cancelled() {
    let mut world = World::start().await;
    let sign_in = start_sign_in(&mut world, SignInMethod::ChatgptDevice).await;
    let (result, command) = ask(
        &mut world,
        SignInCancel {
            sign_in_id: sign_in.id,
        },
        Ok(()),
    )
    .await;
    result.unwrap();
    assert!(matches!(command, HubToWorker::SignInCancel(c) if c.sign_in_id == sign_in.id));
    world
        .client
        .wait_feed(sign_in_entry(sign_in.id, |s| *s == SignInState::Cancelled))
        .await;
    assert_eq!(stored_sign_ins(&world.hub)[0].state, SignInState::Cancelled);

    // Cancelling again is allowed and sends nothing.
    world
        .client
        .request(SignInCancel {
            sign_in_id: sign_in.id,
        })
        .await
        .unwrap();
    assert!(
        world
            .worker
            .try_next_command(Duration::from_millis(300))
            .await
            .is_none()
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_sign_in_that_the_worker_refuses_fails_with_the_message() {
    let mut world = World::start().await;
    let worker_id = world.worker.worker_id;
    let account_id = world.worker.account_id;
    let refused = ErrorBody::new(ErrorCode::SignInFailed, "Claude Code is not installed yet.");
    let request = SignInStart {
        worker_id,
        account_id,
        method: SignInMethod::ClaudePlan,
    };
    let (result, _) = ask(&mut world, request, Err(refused.clone())).await;
    assert_eq!(result.unwrap_err(), refused);
    let stored = stored_sign_ins(&world.hub);
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].state,
        SignInState::Failed {
            message: "Claude Code is not installed yet.".to_owned()
        }
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_worker_that_goes_offline_fails_its_open_sign_ins() {
    let mut world = World::start().await;
    let first = start_sign_in(&mut world, SignInMethod::ClaudePlan).await;
    let finished = start_sign_in(&mut world, SignInMethod::ChatgptBrowser).await;
    send_update(
        &mut world.worker,
        finished.id,
        SignInState::Succeeded { detail: None },
    )
    .await;
    world
        .client
        .wait_feed(sign_in_entry(finished.id, |s| {
            matches!(s, SignInState::Succeeded { .. })
        }))
        .await;

    let worker_id = world.worker.worker_id;
    let placeholder = placeholder_worker(&world.hub).await;
    let _gone = std::mem::replace(&mut world.worker, placeholder)
        .go_away()
        .await;
    wait_offline(&mut world, worker_id).await;

    let failed = SignInState::Failed {
        message: "The worker went offline. Start the sign-in again.".to_owned(),
    };
    world
        .client
        .wait_feed(sign_in_entry(first.id, |s| *s == failed.clone()))
        .await;
    let stored = stored_sign_ins(&world.hub);
    let state_of = |id: SignInId| stored.iter().find(|s| s.id == id).unwrap().state.clone();
    assert_eq!(state_of(first.id), failed);
    // A sign-in that had ended keeps its state.
    assert_eq!(
        state_of(finished.id),
        SignInState::Succeeded { detail: None }
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_client_that_lists_workers_still_works_after_account_requests() {
    // The account requests change nothing in the worker list by themselves.
    let mut world = World::start().await;
    let worker_id = world.worker.worker_id;
    let (result, _) = ask(&mut world, WorkerToolsCheck { worker_id }, Ok(())).await;
    result.unwrap();
    let list = world.client.request(WorkerList {}).await.unwrap();
    assert_eq!(list.workers.len(), 1);
    world.hub.handle.shutdown().await.unwrap();
}
