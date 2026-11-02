//! The first conversation over the real links: the wake of the first sign-in,
//! the guide in the prompt, the worker card and the Composio key on the
//! connect card. The fake session host stands in for the model, so these tests
//! prove the hub and not what a real model says.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::common::*;
use consort_proto::client_api::{
    ApprovalAnswer, ComposioSetKey, EntityPayload, GuideTryAgain, MessageList,
    PreferencesSetUserName,
};
use consort_proto::ids::{AccountId, ChannelId};
use consort_proto::model::{
    ApprovalDetails, ApprovalStatus, AuthState, GuideState, Inventory, MessageKind, Preferences,
    SessionState,
};
use consort_proto::session_host_link::{SessionEvent, SessionStart, TurnEndReason};
use serde_json::{Value, json};

use crate::connectors::fake;

/// The client and the session host of a new hub, with no account signed in.
struct FirstRun {
    hub: TestHub,
    client: FakeClient,
    worker: FakeWorker,
    tutti: ChannelId,
}

fn signed_out(account_id: AccountId) -> Inventory {
    let mut inventory = inventory(account_id);
    inventory.accounts[0].auth_state = AuthState::SignedOut;
    inventory
}

async fn first_run() -> FirstRun {
    let dir = tempfile::tempdir().unwrap();
    let hub = TestHub::start_with_greeting(dir, true).await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let tutti = client.tutti().await;
    let snapshot = client
        .snapshot(consort_proto::client_api::SnapshotScope::All)
        .await;
    client.subscribe(snapshot.seq).await.unwrap();
    // `pair_and_connect` sends an inventory with a signed-in account, so the
    // greeting starts here.
    let worker = FakeWorker::pair_and_connect(&hub).await;
    FirstRun {
        hub,
        client,
        worker,
        tutti,
    }
}

async fn greeting_started(run: &mut FirstRun) -> SessionStart {
    let start = run.worker.accept_session_start().await;
    run.worker
        .send_events(
            start.session_id,
            vec![state(SessionState::Active), turn_started("t1")],
        )
        .await;
    start
}

async fn end_turn(worker: &mut FakeWorker, start: &SessionStart, reason: TurnEndReason) {
    worker
        .send_events(start.session_id, vec![turn_ended("t1", reason)])
        .await;
}

async fn messages(
    client: &mut FakeClient,
    channel_id: ChannelId,
) -> Vec<consort_proto::model::Message> {
    client
        .request(MessageList {
            channel_id,
            thread_root_id: None,
            before: None,
            limit: Some(200),
        })
        .await
        .unwrap()
        .messages
}

fn is_greeting(text: &str) -> bool {
    text.contains("<first_sign_in")
}

async fn no_more_commands(worker: &mut FakeWorker) {
    assert!(
        worker
            .try_next_command(std::time::Duration::from_millis(300))
            .await
            .is_none(),
        "no other turn may start"
    );
}

/// Clears the name, and returns the record that the hub keeps.
async fn cleared_name(client: &mut FakeClient) -> Preferences {
    client
        .request(PreferencesSetUserName {
            name: String::new(),
        })
        .await
        .unwrap()
        .preferences
}

#[tokio::test]
async fn the_first_sign_in_wakes_tutti_once_with_the_guide_in_the_prompt() {
    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    assert_eq!(start.channel_id, run.tutti);
    let text = &start.turn.as_ref().unwrap().text;
    assert!(
        text.contains("<first_sign_in account=\"Claude Code\"/>"),
        "{text}"
    );
    assert!(
        start.system_prompt_append.contains("# The guided start"),
        "the guide is in the prompt while it runs"
    );
    // The notice of T1 comes from the hub, and it holds the account.
    let notices: Vec<String> = messages(&mut run.client, run.tutti)
        .await
        .into_iter()
        .filter(|m| m.kind == MessageKind::Notice)
        .map(|m| m.body)
        .collect();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains("You signed in to Claude Code. The assistant can answer now."),
        "{notices:?}"
    );

    // The guide state reaches the client.
    run.client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Preferences(p)) if p.guide == GuideState::Running)
        })
        .await;

    // Another inventory, a sign-out and a new sign-in do not greet again.
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    let account = run.worker.account_id;
    run.worker.send_custom_inventory(signed_out(account)).await;
    run.worker
        .send_custom_inventory(inventory(account).with_hash())
        .await;
    no_more_commands(&mut run.worker).await;
    run.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_signed_out_account_does_not_wake_and_the_sign_in_does() {
    let dir = tempfile::tempdir().unwrap();
    let hub = TestHub::start_with_greeting(dir, true).await;
    let code = hub.handle.new_session_host_pairing_code().await.unwrap();
    let key = new_key();
    let (status, body) = pair_session_host_http(&hub, &code, &key).await;
    assert_eq!(status, 200, "{body}");
    let paired: consort_proto::auth::PairWorkerResponse = serde_json::from_value(body).unwrap();
    let account = AccountId::new();
    let mut worker = FakeWorker::connect(&hub, paired.worker_id, key, account, Vec::new()).await;
    worker.send_custom_inventory(signed_out(account)).await;
    no_more_commands(&mut worker).await;
    worker
        .send_custom_inventory(inventory(account).with_hash())
        .await;
    let start = worker.accept_session_start().await;
    assert!(is_greeting(&start.turn.as_ref().unwrap().text));
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_restart_keeps_the_flag_and_sends_no_second_greeting() {
    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    let FirstRun { hub, worker, .. } = run;
    let away = worker.go_away().await;
    let TestHub { handle, dir, .. } = hub;
    handle.shutdown().await.unwrap();

    let hub = TestHub::start_with_greeting(dir, true).await;
    let mut worker = away.come_back(&hub).await;
    worker.send_inventory().await;
    // Only a stop of the old session may come. No new session starts.
    while let Some((_, command)) = worker
        .try_next_command(std::time::Duration::from_millis(300))
        .await
    {
        assert!(
            !matches!(
                command,
                consort_proto::session_host_link::HubToSessionHost::SessionStart(_)
            ),
            "{command:?}"
        );
    }
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_restart_before_the_turn_keeps_the_queued_greeting() {
    let dir = tempfile::tempdir().unwrap();
    let hub = TestHub::start_with_greeting(dir, true).await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let snapshot = client
        .snapshot(consort_proto::client_api::SnapshotScope::All)
        .await;
    client.subscribe(snapshot.seq).await.unwrap();
    // The session host goes away right after the hub took its inventory, and
    // before it answers the start. The turn waits in the store.
    let worker = FakeWorker::pair_and_connect(&hub).await;
    client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Preferences(p)) if p.guide == GuideState::Running)
        })
        .await;
    let away = worker.go_away().await;
    let TestHub { handle, dir, .. } = hub;
    handle.shutdown().await.unwrap();
    let hub = TestHub::start_with_greeting(dir, true).await;
    let mut worker = away.come_back(&hub).await;
    worker.send_inventory().await;
    let start = worker.accept_session_start().await;
    assert!(
        is_greeting(&start.turn.as_ref().unwrap().text),
        "the event came back after the restart"
    );
    worker.send_inventory().await;
    no_more_commands(&mut worker).await;
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_hub_with_history_never_greets() {
    let dir = tempfile::tempdir().unwrap();
    let hub = TestHub::start_with_greeting(dir, false).await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    client
        .request(consort_proto::client_api::ChannelCreate {
            name: "old".to_owned(),
            purpose: String::new(),
            instructions: String::new(),
            icon: None,
            color: None,
            group_id: None,
            coordinator_selection: None,
            task_selection: None,
            repo_ids: Vec::new(),
            default_isolation: None,
            default_approval_mode: None,
            memory_enabled: None,
            tags: Vec::new(),
        })
        .await
        .unwrap();
    let TestHub { handle, dir, .. } = hub;
    handle.shutdown().await.unwrap();

    let hub = TestHub::start_with_greeting(dir, true).await;
    let mut worker = FakeWorker::pair_and_connect(&hub).await;
    // The new channel has its own guided start, from before the restart.
    while let Some((_, command)) = worker
        .try_next_command(std::time::Duration::from_millis(300))
        .await
    {
        if let consort_proto::session_host_link::HubToSessionHost::SessionStart(start) = command {
            assert!(!is_greeting(&start.turn.as_ref().unwrap().text));
        }
    }
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_second_client_changes_nothing() {
    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    let code = run.hub.handle.new_setup_code().await.unwrap();
    let _second = FakeClient::pair_with_code_and_connect(&run.hub, &code).await;
    no_more_commands(&mut run.worker).await;
    run.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn try_again_after_a_failed_first_turn_greets_once_and_a_second_click_does_nothing() {
    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    run.worker
        .send_events(
            start.session_id,
            vec![SessionEvent::TurnEnded {
                turn_id: "t1".into(),
                reason: TurnEndReason::Failed,
                error: Some("no boundary on the hub machine".to_owned()),
                error_kind: None,
                resets_at: None,
            }],
        )
        .await;
    let tutti = run.tutti;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let list = messages(&mut run.client, tutti).await;
        if list
            .iter()
            .any(|m| m.kind == MessageKind::Notice && m.body.contains("turn_failed"))
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no failure notice");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // Two clicks, one turn.
    run.client.request(GuideTryAgain {}).await.unwrap();
    run.client.request(GuideTryAgain {}).await.unwrap();
    let (_, command) = run.worker.next_command().await;
    let text = match command {
        consort_proto::session_host_link::HubToSessionHost::SessionTurn(turn) => turn.turn.text,
        consort_proto::session_host_link::HubToSessionHost::SessionStart(start) => {
            start.turn.unwrap().text
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(text.matches("<first_sign_in").count(), 1, "{text}");
    no_more_commands(&mut run.worker).await;
    run.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn try_again_does_nothing_after_the_assistant_spoke() {
    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    run.worker
        .tool_json(start.session_id, "post_message", json!({"body": "Hello"}))
        .await;
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    run.client.request(GuideTryAgain {}).await.unwrap();
    no_more_commands(&mut run.worker).await;
    run.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_guide_leaves_the_prompt_when_it_is_finished() {
    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    assert!(start.system_prompt_append.contains("# The guided start"));
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    user_turn(&mut run, start.session_id, "t2").await;
    let finished = run
        .worker
        .tool_json(
            start.session_id,
            "finish_guide",
            json!({"outcome": "closed"}),
        )
        .await;
    assert_eq!(finished["status"], "ok", "{finished}");
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    run.client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Preferences(p)) if p.guide == GuideState::Closed)
        })
        .await;

    let tutti = run.tutti;
    run.client
        .request(consort_proto::client_api::MessageSend {
            channel_id: tutti,
            client_message_id: "after-guide".to_owned(),
            body: "Hello again".to_owned(),
            attachments: Vec::new(),
            thread_root_id: None,
            in_timeline: false,
        })
        .await
        .unwrap();
    // The prompt changed, so the session ends and a new one starts.
    let next = loop {
        let (request, command) = run.worker.next_command().await;
        match command {
            consort_proto::session_host_link::HubToSessionHost::SessionStart(next) => {
                run.worker
                    .respond_ok(
                        &request,
                        &consort_proto::session_host_link::SessionStartResult {
                            workspace_path: "/tmp/workspace".to_owned(),
                            native_ref: None,
                        },
                    )
                    .await;
                break next;
            }
            _ => run.worker.respond_ok(&request, &json!({})).await,
        }
    };
    assert!(
        !next.system_prompt_append.contains("# The guided start"),
        "the guide is gone"
    );
    run.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn not_now_makes_one_more_offer_and_a_second_not_now_ends_it() {
    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    let session = start.session_id;
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    user_turn(&mut run, session, "t2").await;
    let first = run
        .worker
        .tool_json(session, "finish_guide", json!({"outcome": "later"}))
        .await;
    assert!(
        first["summary"].as_str().unwrap().contains("tomorrow"),
        "{first}"
    );
    let routines = run
        .client
        .snapshot(consort_proto::client_api::SnapshotScope::All)
        .await
        .routines;
    let offers: Vec<_> = routines
        .iter()
        .filter(|r| r.prompt.contains("start_guide"))
        .collect();
    assert_eq!(offers.len(), 1, "one follow-up tomorrow");
    let at = offers[0].next_run_at.unwrap().as_millis();
    let now = consort_proto::time::Timestamp::now().as_millis();
    assert!(
        at > now + 23 * 3_600_000 && at <= now + 24 * 3_600_000,
        "{}",
        at - now
    );

    // The prompt says this, and the words of the reason fit a follow-up.
    assert!(offers[0].prompt.chars().count() <= consort_proto::model::FOLLOW_UP_REASON_MAX_CHARS);

    let second = run
        .worker
        .tool_json(session, "finish_guide", json!({"outcome": "later"}))
        .await;
    assert!(
        second["summary"].as_str().unwrap().contains("no offer"),
        "{second}"
    );
    let routines = run
        .client
        .snapshot(consort_proto::client_api::SnapshotScope::All)
        .await
        .routines;
    assert!(!routines.iter().any(|r| r.prompt.contains("start_guide")));
    run.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn declined_makes_no_offer_and_start_guide_brings_the_rules_back() {
    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    let session = start.session_id;
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    user_turn(&mut run, session, "t2").await;
    run.worker
        .tool_json(session, "finish_guide", json!({"outcome": "declined"}))
        .await;
    let routines = run
        .client
        .snapshot(consort_proto::client_api::SnapshotScope::All)
        .await
        .routines;
    assert!(!routines.iter().any(|r| r.prompt.contains("start_guide")));
    run.client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Preferences(p)) if p.guide == GuideState::Stopped)
        })
        .await;
    let started = run
        .worker
        .tool_json(session, "start_guide", json!({}))
        .await;
    assert!(
        started["guide"]
            .as_str()
            .unwrap()
            .contains("# The guided start"),
        "{started}"
    );
    run.client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Preferences(p)) if p.guide == GuideState::Running)
        })
        .await;
    run.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_assistant_keeps_the_name_for_every_client() {
    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    let session = start.session_id;
    // A greeting is a proactive turn, and it cannot change the name.
    let refused = run
        .worker
        .tool_call(session, "set_user_name", json!({"name": "Chris"}))
        .await;
    assert!(refused.is_error, "{refused:?}");
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;

    let tutti = run.tutti;
    run.client
        .request(consort_proto::client_api::MessageSend {
            channel_id: tutti,
            client_message_id: "name".to_owned(),
            body: "Chris. I build software.".to_owned(),
            attachments: Vec::new(),
            thread_root_id: None,
            in_timeline: false,
        })
        .await
        .unwrap();
    let (request, _) = run.worker.next_command().await;
    run.worker.respond_ok(&request, &json!({})).await;
    run.worker
        .send_events(session, vec![turn_started("t2")])
        .await;
    let named = run
        .worker
        .tool_json(session, "set_user_name", json!({"name": " Chris "}))
        .await;
    assert_eq!(named["name"], "Chris");
    run.client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Preferences(p)) if p.user_name.as_deref() == Some("Chris"))
        })
        .await;
    // The client API clears it, and refuses a name that does not fit.
    assert_eq!(cleared_name(&mut run.client).await.user_name, None);
    assert!(
        run.client
            .request(PreferencesSetUserName {
                name: "x".repeat(81),
            })
            .await
            .is_err()
    );
    run.hub.handle.shutdown().await.unwrap();
}

/// A user turn on the session of the greeting, so the tools of a normal turn
/// work.
async fn user_turn(run: &mut FirstRun, session: consort_proto::ids::SessionId, id: &str) {
    run.client
        .request(consort_proto::client_api::MessageSend {
            channel_id: run.tutti,
            client_message_id: id.to_owned(),
            body: "Yes".to_owned(),
            attachments: Vec::new(),
            thread_root_id: None,
            in_timeline: false,
        })
        .await
        .unwrap();
    let (request, _) = run.worker.next_command().await;
    run.worker.respond_ok(&request, &json!({})).await;
    run.worker
        .send_events(session, vec![turn_started(id)])
        .await;
}

#[tokio::test]
async fn the_worker_card_closes_when_a_worker_comes_online_and_holds_no_code() {
    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    let session = start.session_id;
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    user_turn(&mut run, session, "t2").await;

    let posted = run
        .worker
        .tool_json(session, "add_worker", json!({"this_computer": true}))
        .await;
    assert_eq!(posted["status"], "waiting_for_user");
    let posted_text = posted.to_string();

    // The app makes the code. It is in no message and no approval.
    let pairing = run
        .client
        .request(consort_proto::client_api::WorkerPairingCode {})
        .await
        .unwrap();
    let code = pairing.code.to_string();
    assert!(!posted_text.contains(&code));
    let card = run
        .client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Approval(a)) if matches!(a.details, ApprovalDetails::AddWorker(_)))
        })
        .await;
    let card_text = serde_json::to_string(&card).unwrap();
    assert!(!card_text.contains(&code));
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;

    let exec = FakeExec::pair(&run.hub).await;
    let closed = run
        .client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Approval(a))
                if a.status == ApprovalStatus::Allowed && matches!(a.details, ApprovalDetails::AddWorker(_)))
        })
        .await;
    let Some(EntityPayload::Approval(closed)) = upserted(&closed) else {
        panic!("not an approval");
    };
    let ApprovalDetails::AddWorker(details) = &closed.details else {
        panic!("not a worker card");
    };
    assert_eq!(details.worker_id, Some(exec.worker_id));

    // Tutti gets told, in a normal turn.
    let (request, command) = run.worker.next_command().await;
    let text = match command {
        consort_proto::session_host_link::HubToSessionHost::SessionTurn(turn) => turn.turn.text,
        other => panic!("{other:?}"),
    };
    run.worker.respond_ok(&request, &json!({})).await;
    assert!(text.contains("is online"), "{text}");
    assert!(text.contains("card_answered"), "{text}");
    assert!(!text.contains(&code), "{text}");
    run.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn not_now_on_the_worker_card_tells_tutti() {
    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    let session = start.session_id;
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    user_turn(&mut run, session, "t2").await;
    run.worker.tool_json(session, "add_worker", json!({})).await;
    let card = run
        .client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Approval(a)) if matches!(a.details, ApprovalDetails::AddWorker(_)))
        })
        .await;
    let Some(EntityPayload::Approval(card)) = upserted(&card) else {
        panic!("not an approval");
    };
    let id = card.id;
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    run.client
        .request(ApprovalAnswer {
            approval_id: id,
            option_id: consort_proto::model::HUB_CARD_DENY.to_owned(),
            checked: Default::default(),
            checked_hash: None,
        })
        .await
        .unwrap();
    let (request, command) = run.worker.next_command().await;
    let text = match command {
        consort_proto::session_host_link::HubToSessionHost::SessionTurn(turn) => turn.turn.text,
        other => panic!("{other:?}"),
    };
    run.worker.respond_ok(&request, &json!({})).await;
    assert!(text.contains("chose not to add a worker"), "{text}");
    run.hub.handle.shutdown().await.unwrap();
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn the_composio_key_of_the_first_connect_card_reaches_no_transcript_and_no_log() {
    let logs = Captured::default();
    tracing_subscriber::fmt()
        // The link library logs whole frames at its own trace level, for every
        // request. That is outside what the hub and the proto write.
        .with_env_filter("info,consort_hub=trace,consort_proto=trace")
        .with_writer(logs.clone())
        .init();

    let mut run = first_run().await;
    let start = greeting_started(&mut run).await;
    let session = start.session_id;
    let fake = fake::FakeComposio::start().await;
    run.hub.handle.set_composio_url(&fake.url);
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;
    user_turn(&mut run, session, "t2").await;

    // With no key, the card still comes, with the name that the model gave.
    let asked = run
        .worker
        .tool_json(
            session,
            "connect_service",
            json!({"service": "gmail", "name": "Gmail"}),
        )
        .await;
    assert_eq!(asked["status"], "waiting_for_user", "{asked}");
    let card = run
        .client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::Approval(a)) if matches!(a.details, ApprovalDetails::ConnectService(_)))
        })
        .await;
    let Some(EntityPayload::Approval(card)) = upserted(&card) else {
        panic!("not an approval");
    };
    assert_eq!(card.summary, "Connect Gmail");
    let id = card.id;
    end_turn(&mut run.worker, &start, TurnEndReason::Completed).await;

    // The card sends the key to the hub, and then answers.
    let key = fake::GOOD_KEY;
    run.client
        .request(ComposioSetKey {
            api_key: key.to_owned(),
        })
        .await
        .unwrap();
    run.client
        .request(ApprovalAnswer {
            approval_id: id,
            option_id: consort_proto::model::HUB_CARD_ALLOW.to_owned(),
            checked: Default::default(),
            checked_hash: None,
        })
        .await
        .unwrap();
    let (request, command) = run.worker.next_command().await;
    let text = match command {
        consort_proto::session_host_link::HubToSessionHost::SessionTurn(turn) => turn.turn.text,
        other => panic!("{other:?}"),
    };
    run.worker.respond_ok(&request, &json!({})).await;
    assert!(text.contains("started to connect Gmail"), "{text}");
    assert!(!text.contains(key), "the model gets no key: {text}");

    // The tools of the model and the timeline do not hold it.
    let all = messages(&mut run.client, run.tutti).await;
    assert!(!serde_json::to_string(&all).unwrap().contains(key));
    let connections: Value = run
        .worker
        .tool_json(session, "list_connections", json!({}))
        .await;
    assert!(!connections.to_string().contains(key));
    let snapshot = run
        .client
        .snapshot(consort_proto::client_api::SnapshotScope::All)
        .await;
    assert!(!serde_json::to_string(&snapshot).unwrap().contains(key));

    // No log line and no byte of the store holds it, in the clear.
    let hub_dir = run.hub.dir.path().to_path_buf();
    run.hub.handle.shutdown().await.unwrap();
    let log = String::from_utf8_lossy(&logs.0.lock().unwrap()).into_owned();
    assert!(log.contains("the Composio key changed"), "the log ran");
    let leaked: Vec<&str> = log.lines().filter(|l| l.contains(key)).collect();
    assert!(leaked.is_empty(), "the key is in a log line: {leaked:?}");
    for entry in std::fs::read_dir(&hub_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !bytes.windows(key.len()).any(|w| w == key.as_bytes()),
                "the key is in {}",
                path.display()
            );
        }
    }
}
