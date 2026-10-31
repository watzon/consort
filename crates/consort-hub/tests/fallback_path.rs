//! The fallback path: the order of the entries, the reasons to skip one, the
//! trigger of a provider outage after three tries, the return of the
//! assistant at the reset, and a task that stays where it moved.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::client_api::{
    ChannelGoBack, ChannelUpdate, PreferencesSetFallbackPath, SnapshotScope,
};
use consort_proto::ids::{AccountId, SessionId};
use consort_proto::model::{
    ApprovalMode, AuthState, FallbackPath, HarnessAccount, ModelInfo, ModelSelection, SessionState,
    TurnErrorKind,
};
use consort_proto::session_host_link::{
    HubToSessionHost, SessionEvent, SessionStart, SessionStartResult, TurnEndReason,
};
use consort_proto::status::TaskStatus;
use consort_proto::time::Timestamp;
use serde_json::{Value, json};

const A: &str = "Claude Personal";
const B: &str = "Claude Work";
const C: &str = "Codex Work";
const D: &str = "Codex Personal";

fn account(world: &World, label: &str, harness: &str, models: &[&str]) -> HarnessAccount {
    let mut account = inventory(world.worker.account_id).accounts[0].clone();
    account.account_id = AccountId::new();
    account.label = label.to_owned();
    account.harness = harness.to_owned();
    account.models = models
        .iter()
        .map(|id| ModelInfo {
            id: (*id).to_owned(),
            name: (*id).to_owned(),
            options: Vec::new(),
        })
        .collect();
    account
}

/// The accounts of the design on one worker. A and B are Claude accounts
/// with Opus. C and D are Codex accounts with GPT.
fn four_accounts(world: &World) -> Vec<HarnessAccount> {
    vec![
        account(world, A, "claude", &["opus"]),
        account(world, B, "claude", &["opus"]),
        account(world, C, "codex", &["gpt"]),
        account(world, D, "codex", &["gpt"]),
    ]
}

async fn send_four(world: &mut World) -> Vec<AccountId> {
    let accounts = four_accounts(world);
    send_accounts(world, accounts).await
}

async fn send_accounts(world: &mut World, accounts: Vec<HarnessAccount>) -> Vec<AccountId> {
    let mut inventory = inventory(world.worker.account_id);
    let ids = accounts.iter().map(|a| a.account_id).collect();
    inventory.accounts = accounts;
    world.worker.send_custom_inventory(inventory).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    ids
}

fn step(label: &str, model: &str) -> ModelSelection {
    ModelSelection {
        account_label: label.to_owned(),
        model: model.to_owned(),
        options: Default::default(),
    }
}

async fn set_path(world: &mut World, other_accounts: bool, steps: &[(&str, &str)]) {
    world
        .client
        .request(PreferencesSetFallbackPath {
            path: FallbackPath {
                other_accounts,
                steps: steps.iter().map(|(l, m)| step(l, m)).collect(),
            },
        })
        .await
        .unwrap();
}

async fn assistant_runs_on(world: &mut World, label: &str) {
    let update: ChannelUpdate = serde_json::from_value(json!({
        "channel_id": world.tutti,
        "coordinator_selection": step(label, "opus"),
    }))
    .unwrap();
    world.client.request(update).await.unwrap();
}

/// Reads commands until a `session.start`. Other commands get an answer.
async fn next_start(worker: &mut FakeWorker) -> SessionStart {
    loop {
        let (request, command) = worker.next_command().await;
        if let HubToSessionHost::SessionStart(start) = command {
            worker
                .respond_ok(
                    &request,
                    &SessionStartResult {
                        workspace_path: "/tmp/workspace".to_owned(),
                        native_ref: None,
                    },
                )
                .await;
            return start;
        }
        worker.respond_ok(&request, &json!({})).await;
    }
}

/// Reads commands until a `session.turn` for this session.
async fn next_turn(worker: &mut FakeWorker, session: SessionId) {
    loop {
        let (request, command) = worker.next_command().await;
        worker.respond_ok(&request, &json!({})).await;
        if matches!(&command, HubToSessionHost::SessionTurn(turn) if turn.session_id == session) {
            return;
        }
        assert!(
            !matches!(command, HubToSessionHost::SessionStart(_)),
            "the hub started a new session, but it had to try the turn again"
        );
    }
}

fn failed(kind: TurnErrorKind, resets_at: Option<Timestamp>, turn: &str) -> SessionEvent {
    SessionEvent::TurnEnded {
        turn_id: turn.into(),
        reason: TurnEndReason::Failed,
        error: Some(format!("The turn failed: {kind:?}")),
        error_kind: Some(kind),
        resets_at,
    }
}

fn limit(resets_at: Option<Timestamp>) -> SessionEvent {
    failed(TurnErrorKind::RateLimit, resets_at, "t1")
}

fn in_ms(ms: i64) -> Timestamp {
    Timestamp::now().plus_millis(ms)
}

/// The first notice of this kind in the timeline or a thread, as JSON.
async fn notice_of(
    world: &mut World,
    kind: &str,
    thread: Option<consort_proto::ids::MessageId>,
) -> Value {
    world.wait_notice(thread, &[&format!("\"{kind}\"")]).await;
    let notices = world.notices(thread).await;
    let body = notices
        .iter()
        .find(|n| n.contains(&format!("\"{kind}\"")))
        .unwrap();
    serde_json::from_str(body).unwrap()
}

async fn tutti_channel(world: &mut World) -> consort_proto::model::Channel {
    world
        .client
        .snapshot(SnapshotScope::Channel(world.tutti))
        .await
        .channels
        .into_iter()
        .find(|c| c.id == world.tutti)
        .unwrap()
}

/// The assistant runs on A and its turn ends at a limit.
async fn assistant_on_a_at_a_limit(
    world: &mut World,
    resets_at: Option<Timestamp>,
) -> (SessionStart, SessionStart) {
    assistant_runs_on(world, A).await;
    let start = world.start_coordinator("hello").await;
    world
        .worker
        .send_events(start.session_id, vec![limit(resets_at)])
        .await;
    let moved = next_start(&mut world.worker).await;
    (start, moved)
}

// Order ----------------------------------------------------------------------

#[tokio::test]
async fn the_built_in_row_comes_before_the_steps() {
    let mut world = World::start().await;
    let ids = send_four(&mut world).await;
    set_path(&mut world, true, &[(C, "gpt")]).await;
    let (start, moved) = assistant_on_a_at_a_limit(&mut world, Some(in_ms(3_600_000))).await;
    assert_eq!(start.account_id, ids[0]);
    assert_eq!(moved.account_id, ids[1], "Claude Work has Opus too");
    assert_eq!(moved.model, "opus");
    assert!(moved.turn.unwrap().text.contains("hello"));
    let notice = notice_of(&mut world, "account_fallback", None).await;
    assert_eq!(notice["scope"], "assistant");
    assert_eq!(notice["cause"], "usage_limit");
    assert_eq!(notice["from"]["account_label"], A);
    assert_eq!(notice["to"]["account_label"], B);
    assert!(
        notice["provider"].is_null(),
        "the same harness names no provider"
    );
    assert!(notice["returns_at"].is_i64() || notice["returns_at"].is_u64());
    // The first choice and the entry that took the work.
    assert_eq!(notice["lines"].as_array().unwrap().len(), 2);
    assert_eq!(notice["lines"][0]["place"], "first");
    assert_eq!(notice["lines"][1]["place"], "other_account");
    assert_eq!(notice["lines"][1]["outcome"], "chosen");
    // The channel says that the assistant runs on a fallback.
    let channel = tutti_channel(&mut world).await;
    let record = channel.coordinator_fallback.unwrap();
    assert_eq!(record.from.account_label, A);
    assert_eq!(record.to.account_label, B);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn with_the_built_in_row_off_the_first_step_takes_the_work_and_the_notice_names_the_provider()
{
    let mut world = World::start().await;
    let ids = send_four(&mut world).await;
    set_path(&mut world, false, &[(C, "gpt"), (D, "gpt")]).await;
    let (_, moved) = assistant_on_a_at_a_limit(&mut world, Some(in_ms(3_600_000))).await;
    assert_eq!(moved.account_id, ids[2]);
    assert_eq!(moved.model, "gpt");
    let notice = notice_of(&mut world, "account_fallback", None).await;
    assert_eq!(notice["provider"], "OpenAI");
    assert!(
        notice["text"].as_str().unwrap().contains("OpenAI"),
        "{notice}"
    );
    assert_eq!(notice["lines"][1]["place"], "step");
    assert_eq!(notice["lines"][1]["step"], 1);
    world.hub.handle.shutdown().await.unwrap();
}

// Skips ----------------------------------------------------------------------

/// A signed-out account is skipped, the next step takes the work, and the
/// timeline says which account the move skipped.
#[tokio::test]
async fn a_move_skips_a_signed_out_account_and_says_so_in_the_timeline() {
    let mut world = World::start().await;
    let mut accounts = four_accounts(&world);
    accounts[2].auth_state = AuthState::SignedOut;
    let ids = send_accounts(&mut world, accounts).await;
    set_path(&mut world, false, &[(C, "gpt"), (D, "gpt")]).await;
    let (_, moved) = assistant_on_a_at_a_limit(&mut world, None).await;
    assert_eq!(moved.account_id, ids[3], "Codex Personal takes the work");
    let notice = notice_of(&mut world, "account_fallback", None).await;
    assert_eq!(notice["lines"][1]["reason"], "signed_out");
    assert_eq!(notice["lines"][1]["outcome"], "skipped");
    assert_eq!(notice["lines"][1]["account_label"], C);
    let signed_out = notice_of(&mut world, "fallback_signed_out", None).await;
    assert_eq!(signed_out["account_label"], C);
    world.hub.handle.shutdown().await.unwrap();
}

/// A step whose account is at a limit is skipped, with the time.
#[tokio::test]
async fn a_held_account_is_skipped_with_its_reason_and_the_time() {
    let mut world = World::start().await;
    let ids = send_four(&mut world).await;
    set_path(&mut world, true, &[(C, "gpt")]).await;
    let until = in_ms(3_600_000);
    let (_, moved) = assistant_on_a_at_a_limit(&mut world, Some(until)).await;
    assert_eq!(moved.account_id, ids[1]);
    // Claude Work reaches its limit too. The walk starts from the first
    // choice again: A and B are held, so Codex Work takes the work.
    world
        .worker
        .send_events(
            moved.session_id,
            vec![
                state(SessionState::Active),
                turn_started("t2"),
                failed(TurnErrorKind::RateLimit, Some(until), "t2"),
            ],
        )
        .await;
    let third = next_start(&mut world.worker).await;
    assert_eq!(third.account_id, ids[2]);
    world
        .wait_notice(None, &["account_fallback", "Codex Work"])
        .await;
    let notices = world.notices(None).await;
    let second: Value = notices
        .iter()
        .filter_map(|n| serde_json::from_str::<Value>(n).ok())
        .find(|n| n["kind"] == "account_fallback" && n["to"]["account_label"] == C)
        .unwrap();
    assert_eq!(second["from"]["account_label"], B, "the model that failed");
    let held = &second["lines"][1];
    assert_eq!(held["account_label"], B);
    assert_eq!(held["reason"], "limited");
    assert_eq!(held["cause"], "usage_limit");
    assert!(held["until"].is_number(), "{held}");
    // The record keeps the first choice, so the assistant goes back to A.
    let record = tutti_channel(&mut world)
        .await
        .coordinator_fallback
        .unwrap();
    assert_eq!(record.from.account_label, A);
    assert_eq!(record.to.account_label, C);
    world.hub.handle.shutdown().await.unwrap();
}

/// A step never lowers the approval mode: a task in Auto skips an account
/// that can run only in Ask or Auto edits, and the next account takes it.
#[tokio::test]
async fn a_step_that_cannot_keep_the_approval_mode_is_skipped_and_the_task_is_not_lowered() {
    let mut world = World::start().await;
    let mut accounts = four_accounts(&world);
    accounts[2].capabilities.approval_modes = vec![ApprovalMode::Ask, ApprovalMode::AutoEdits];
    accounts[3].capabilities.approval_modes = vec![ApprovalMode::Ask, ApprovalMode::Auto];
    let ids = send_accounts(&mut world, accounts).await;
    set_path(&mut world, false, &[(C, "gpt"), (D, "gpt")]).await;
    assistant_runs_on(&mut world, A).await;
    let (task_id, start) = world.start_task("Find the warranty").await;
    assert_eq!(start.account_id, ids[0]);
    assert_eq!(start.approval_mode, ApprovalMode::Auto);
    world
        .worker
        .send_events(start.session_id, vec![limit(None)])
        .await;
    let moved = next_start(&mut world.worker).await;
    assert_eq!(moved.account_id, ids[3], "Codex Personal keeps Auto");
    assert_eq!(moved.approval_mode, ApprovalMode::Auto);
    let task = world.task(task_id).await;
    let notice = notice_of(&mut world, "account_fallback", Some(task.card_message_id)).await;
    assert_eq!(notice["scope"], "task");
    assert_eq!(notice["lines"][1]["account_label"], C);
    assert_eq!(notice["lines"][1]["reason"], "mode");
    world.hub.handle.shutdown().await.unwrap();
}

/// When no entry can take the work, the failed turn says what the path
/// tried, one line for each entry.
#[tokio::test]
async fn when_every_entry_fails_the_error_lists_each_step_and_its_reason() {
    let mut world = World::start().await;
    let mut accounts = four_accounts(&world);
    accounts[3].auth_state = AuthState::SignedOut;
    let ids = send_accounts(&mut world, accounts).await;
    set_path(&mut world, true, &[(D, "gpt")]).await;
    let (_, moved) = assistant_on_a_at_a_limit(&mut world, Some(in_ms(3_600_000))).await;
    assert_eq!(moved.account_id, ids[1]);
    world
        .worker
        .send_events(
            moved.session_id,
            vec![
                state(SessionState::Active),
                turn_started("t2"),
                failed(TurnErrorKind::RateLimit, Some(in_ms(3_600_000)), "t2"),
            ],
        )
        .await;
    let failure = notice_of(&mut world, "turn_failed", None).await;
    assert_eq!(failure["error_kind"], "rate_limit");
    let path = failure["path"].as_array().unwrap();
    let reasons: Vec<_> = path.iter().map(|l| l["reason"].clone()).collect();
    assert_eq!(path.len(), 3, "{path:?}");
    assert_eq!(path[0]["place"], "first");
    assert_eq!(reasons[1], "limited");
    assert_eq!(reasons[2], "signed_out");
    world.hub.handle.shutdown().await.unwrap();
}

/// The built-in row off and no step: the limit stays an error, as before.
#[tokio::test]
async fn with_no_entry_a_task_at_a_limit_fails_and_the_error_has_the_path() {
    let mut world = World::start().await;
    send_four(&mut world).await;
    set_path(&mut world, false, &[]).await;
    assistant_runs_on(&mut world, A).await;
    let (task_id, start) = world.start_task("Find the warranty").await;
    world
        .worker
        .send_events(start.session_id, vec![limit(None)])
        .await;
    let task = world.wait_task(task_id, TaskStatus::Failed).await;
    let failure = notice_of(&mut world, "turn_failed", Some(task.card_message_id)).await;
    assert_eq!(failure["path"].as_array().unwrap().len(), 1);
    world.hub.handle.shutdown().await.unwrap();
}

/// A sign-in problem is not a trigger.
#[tokio::test]
async fn an_expired_sign_in_does_not_move_the_work() {
    let mut world = World::start().await;
    send_four(&mut world).await;
    set_path(&mut world, true, &[(C, "gpt")]).await;
    assistant_runs_on(&mut world, A).await;
    let (task_id, start) = world.start_task("Find the warranty").await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![failed(TurnErrorKind::ExpiredSignIn, None, "t1")],
        )
        .await;
    let task = world.wait_task(task_id, TaskStatus::Failed).await;
    assert!(
        world
            .notices(Some(task.card_message_id))
            .await
            .iter()
            .all(|n| !n.contains("account_fallback")),
        "no move"
    );
    world.hub.handle.shutdown().await.unwrap();
}

// Outage ---------------------------------------------------------------------

/// A provider outage is a try. The third failed try in a row moves the
/// work. The first two run the turn again on the same session.
#[tokio::test]
async fn an_outage_moves_the_assistant_after_three_failed_tries_and_not_before() {
    let mut world = World::start().await;
    let ids = send_four(&mut world).await;
    set_path(&mut world, false, &[(C, "gpt")]).await;
    assistant_runs_on(&mut world, A).await;
    let start = world.start_coordinator("hello").await;
    let session = start.session_id;
    for (index, turn) in ["t1", "t2"].iter().enumerate() {
        world
            .worker
            .send_events(
                session,
                vec![failed(TurnErrorKind::ProviderOutage, None, turn)],
            )
            .await;
        // The hub runs the same turn again on the same session.
        next_turn(&mut world.worker, session).await;
        world
            .worker
            .send_events(session, vec![turn_started(&format!("t{}", index + 2))])
            .await;
    }
    assert!(
        world
            .notices(None)
            .await
            .iter()
            .all(|n| !n.contains("account_fallback") && !n.contains("turn_failed")),
        "two failed tries show nothing"
    );
    world
        .worker
        .send_events(
            session,
            vec![failed(TurnErrorKind::ProviderOutage, None, "t3")],
        )
        .await;
    let moved = next_start(&mut world.worker).await;
    assert_eq!(moved.account_id, ids[2]);
    assert!(
        moved.turn.unwrap().text.contains("hello"),
        "the same message"
    );
    let notice = notice_of(&mut world, "account_fallback", None).await;
    assert_eq!(notice["cause"], "outage");
    assert!(
        notice["text"].as_str().unwrap().contains("3 tries"),
        "{notice}"
    );
    assert!(notice["returns_at"].is_null(), "the provider gave no time");
    world.hub.handle.shutdown().await.unwrap();
}

/// An outage is the fault of the provider, so another account of the same
/// harness would fail the same way: the built-in row skips it.
#[tokio::test]
async fn an_outage_holds_every_account_of_the_harness() {
    let mut world = World::start().await;
    let ids = send_four(&mut world).await;
    set_path(&mut world, true, &[(C, "gpt")]).await;
    assistant_runs_on(&mut world, A).await;
    let start = world.start_coordinator("hello").await;
    let session = start.session_id;
    for turn in ["t1", "t2"] {
        world
            .worker
            .send_events(
                session,
                vec![failed(TurnErrorKind::ProviderOutage, None, turn)],
            )
            .await;
        next_turn(&mut world.worker, session).await;
        world
            .worker
            .send_events(session, vec![turn_started(turn)])
            .await;
    }
    world
        .worker
        .send_events(
            session,
            vec![failed(TurnErrorKind::ProviderOutage, None, "t3")],
        )
        .await;
    let moved = next_start(&mut world.worker).await;
    assert_eq!(moved.account_id, ids[2], "Codex Work, not Claude Work");
    let notice = notice_of(&mut world, "account_fallback", None).await;
    let skipped = &notice["lines"][1];
    assert_eq!(skipped["account_label"], B);
    assert_eq!(skipped["reason"], "limited");
    assert_eq!(skipped["cause"], "outage");
    assert_eq!(skipped["provider"], "Anthropic");
    world.hub.handle.shutdown().await.unwrap();
}

/// A turn that ends well clears the count, so failures that are not in a
/// row never add up.
#[tokio::test]
async fn a_turn_that_ends_well_clears_the_count_of_failed_tries() {
    let mut world = World::start().await;
    send_four(&mut world).await;
    set_path(&mut world, false, &[(C, "gpt")]).await;
    assistant_runs_on(&mut world, A).await;
    let start = world.start_coordinator("hello").await;
    let session = start.session_id;
    let ok = || SessionEvent::TurnEnded {
        turn_id: "ok".into(),
        reason: TurnEndReason::Completed,
        error: None,
        error_kind: None,
        resets_at: None,
    };
    for _ in 0..2 {
        // Two failed tries, then a good turn.
        for turn in ["a", "b"] {
            world
                .worker
                .send_events(
                    session,
                    vec![failed(TurnErrorKind::ProviderOutage, None, turn)],
                )
                .await;
            next_turn(&mut world.worker, session).await;
            world
                .worker
                .send_events(session, vec![turn_started(turn)])
                .await;
        }
        world.worker.send_events(session, vec![ok()]).await;
        world.say(world.tutti, "again").await;
        next_turn(&mut world.worker, session).await;
        world
            .worker
            .send_events(session, vec![turn_started("n")])
            .await;
    }
    assert!(
        world
            .notices(None)
            .await
            .iter()
            .all(|n| !n.contains("account_fallback")),
        "four failures in all, none moved the assistant"
    );
    world.hub.handle.shutdown().await.unwrap();
}

/// An outage of a task: the first two failed turns ask the session to go
/// on, the third moves the task.
#[tokio::test]
async fn an_outage_moves_a_task_after_three_failed_tries() {
    let mut world = World::start().await;
    let ids = send_four(&mut world).await;
    set_path(&mut world, false, &[(C, "gpt")]).await;
    assistant_runs_on(&mut world, A).await;
    let (task_id, start) = world.start_task("Find the warranty").await;
    let session = start.session_id;
    for turn in ["t1", "t2"] {
        world
            .worker
            .send_events(
                session,
                vec![failed(TurnErrorKind::ProviderOutage, None, turn)],
            )
            .await;
        next_turn(&mut world.worker, session).await;
        world
            .worker
            .send_events(session, vec![turn_started(turn)])
            .await;
        assert_eq!(world.task(task_id).await.status, TaskStatus::Working);
    }
    world
        .worker
        .send_events(
            session,
            vec![failed(TurnErrorKind::ProviderOutage, None, "t3")],
        )
        .await;
    let moved = next_start(&mut world.worker).await;
    assert_eq!(moved.account_id, ids[2]);
    let task = world.task(task_id).await;
    assert_eq!(task.status, TaskStatus::Working);
    assert_eq!(task.model_selection.unwrap().account_label, C);
    world.hub.handle.shutdown().await.unwrap();
}

// The way back ---------------------------------------------------------------

/// The assistant goes back to its first choice on its own when the limit
/// resets, with a notice. The next message starts a session on it.
#[tokio::test]
async fn the_assistant_goes_back_on_its_own_at_the_reset() {
    let mut world = World::start().await;
    let ids = send_four(&mut world).await;
    set_path(&mut world, true, &[]).await;
    let (_, moved) = assistant_on_a_at_a_limit(&mut world, Some(in_ms(1_500))).await;
    assert_eq!(moved.account_id, ids[1]);
    // The turn on the fallback ends, so the assistant is idle.
    world
        .worker
        .send_events(
            moved.session_id,
            vec![
                state(SessionState::Active),
                turn_started("t2"),
                SessionEvent::TurnEnded {
                    turn_id: "t2".into(),
                    reason: TurnEndReason::Completed,
                    error: None,
                    error_kind: None,
                    resets_at: None,
                },
            ],
        )
        .await;
    assert!(
        tutti_channel(&mut world)
            .await
            .coordinator_fallback
            .is_some()
    );
    let returned = notice_of(&mut world, "fallback_returned", None).await;
    assert_eq!(returned["to"]["account_label"], A);
    assert!(
        returned["text"].as_str().unwrap().contains("has reset"),
        "{returned}"
    );
    assert!(
        tutti_channel(&mut world)
            .await
            .coordinator_fallback
            .is_none()
    );
    // The fallback session stopped. The next message starts on the first
    // choice.
    world.say(world.tutti, "back again").await;
    let back = next_start(&mut world.worker).await;
    assert_eq!(back.account_id, ids[0]);
    world.hub.handle.shutdown().await.unwrap();
}

/// A turn that runs is not cut: the assistant goes back after it.
#[tokio::test]
async fn the_assistant_does_not_go_back_while_a_turn_runs() {
    let mut world = World::start().await;
    let ids = send_four(&mut world).await;
    set_path(&mut world, true, &[]).await;
    let (_, moved) = assistant_on_a_at_a_limit(&mut world, Some(in_ms(1_000))).await;
    world
        .worker
        .send_events(
            moved.session_id,
            vec![state(SessionState::Active), turn_started("t2")],
        )
        .await;
    tokio::time::sleep(Duration::from_millis(3_500)).await;
    assert!(
        tutti_channel(&mut world)
            .await
            .coordinator_fallback
            .is_some(),
        "the turn still runs"
    );
    world
        .worker
        .send_events(
            moved.session_id,
            vec![SessionEvent::TurnEnded {
                turn_id: "t2".into(),
                reason: TurnEndReason::Completed,
                error: None,
                error_kind: None,
                resets_at: None,
            }],
        )
        .await;
    notice_of(&mut world, "fallback_returned", None).await;
    world.say(world.tutti, "back again").await;
    assert_eq!(next_start(&mut world.worker).await.account_id, ids[0]);
    world.hub.handle.shutdown().await.unwrap();
}

/// A limit with no reset time holds the account for an hour, so the
/// assistant does not go back soon, and the notice has no time.
#[tokio::test]
async fn a_limit_with_no_reset_time_names_no_time_and_the_assistant_stays() {
    let mut world = World::start().await;
    send_four(&mut world).await;
    set_path(&mut world, true, &[]).await;
    let (_, moved) = assistant_on_a_at_a_limit(&mut world, None).await;
    let notice = notice_of(&mut world, "account_fallback", None).await;
    assert!(notice["returns_at"].is_null());
    assert!(!notice["text"].as_str().unwrap().contains("returns at"));
    let record = tutti_channel(&mut world)
        .await
        .coordinator_fallback
        .unwrap();
    assert_eq!(record.returns_at, None);
    world
        .worker
        .send_events(
            moved.session_id,
            vec![SessionEvent::TurnEnded {
                turn_id: "t1".into(),
                reason: TurnEndReason::Completed,
                error: None,
                error_kind: None,
                resets_at: None,
            }],
        )
        .await;
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert!(
        tutti_channel(&mut world)
            .await
            .coordinator_fallback
            .is_some()
    );
    // "Go back to <model>" is refused until the hold ends.
    let refused = world
        .client
        .request(ChannelGoBack {
            channel_id: world.tutti,
        })
        .await;
    assert!(refused.is_err());
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn go_back_is_refused_when_the_assistant_is_not_on_a_fallback() {
    let mut world = World::start().await;
    let refused = world
        .client
        .request(ChannelGoBack {
            channel_id: world.tutti,
        })
        .await;
    assert!(refused.is_err());
    world.hub.handle.shutdown().await.unwrap();
}

/// A task stays where it moved, also after the limit reset and after the
/// assistant went back.
#[tokio::test]
async fn a_task_stays_where_it_moved_when_the_limit_resets() {
    let mut world = World::start().await;
    let ids = send_four(&mut world).await;
    set_path(&mut world, true, &[]).await;
    assistant_runs_on(&mut world, A).await;
    let (task_id, start) = world.start_task("Find the warranty").await;
    assert_eq!(start.account_id, ids[0]);
    world
        .worker
        .send_events(start.session_id, vec![limit(Some(in_ms(1_000)))])
        .await;
    let moved = next_start(&mut world.worker).await;
    assert_eq!(moved.account_id, ids[1]);
    let task = world.task(task_id).await;
    let notice = notice_of(&mut world, "account_fallback", Some(task.card_message_id)).await;
    assert_eq!(notice["scope"], "task");
    assert!(
        notice["text"].as_str().unwrap().contains("It stays there"),
        "{notice}"
    );
    // The reset passes, and the hub checks for returns a few times.
    tokio::time::sleep(Duration::from_millis(4_500)).await;
    let task = world.task(task_id).await;
    assert_eq!(task.model_selection.unwrap().account_label, B);
    assert_eq!(task.current_session_id, Some(moved.session_id));
    assert_eq!(task.status, TaskStatus::Working);
    assert!(
        tutti_channel(&mut world)
            .await
            .coordinator_fallback
            .is_none(),
        "a task never sets the record of the assistant"
    );
    world.hub.handle.shutdown().await.unwrap();
}

// The settings ----------------------------------------------------------------

#[tokio::test]
async fn the_hub_stores_the_path_sends_it_in_the_snapshot_and_refuses_a_bad_one() {
    let mut world = World::start().await;
    set_path(&mut world, false, &[(C, "gpt"), (B, "opus")]).await;
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    let path = snapshot.preferences.unwrap().fallback_path;
    assert!(!path.other_accounts);
    assert_eq!(path.steps[0].account_label, C);
    assert_eq!(path.steps[1].account_label, B, "the order is kept");
    let repeat = world
        .client
        .request(PreferencesSetFallbackPath {
            path: FallbackPath {
                other_accounts: true,
                steps: vec![step(C, "gpt"), step(C, "gpt")],
            },
        })
        .await;
    assert!(repeat.is_err());
    world.hub.handle.shutdown().await.unwrap();
}
