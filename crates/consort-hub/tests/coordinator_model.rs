//! The model of the assistant: a change to a channel's coordinator selection
//! reaches the next coordinator turn. An idle session ends at once. A turn
//! that runs is not cut. Tutti's selection is the default for channels
//! without their own.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::client_api::{ChannelCreate, ChannelUpdate};
use consort_proto::frame::MessageFamily as _;
use consort_proto::ids::{ChannelId, SessionId};
use consort_proto::model::{ModelSelection, OptionValue, SessionState};
use consort_proto::session_host_link::{
    HubToSessionHost, SessionStart, SessionStartResult, TurnEndReason,
};
use serde_json::json;

const QUIET: Duration = Duration::from_millis(400);

fn selection(model: &str, effort: &str) -> ModelSelection {
    ModelSelection {
        account_label: "Claude Code".to_owned(),
        model: model.to_owned(),
        options: [("effort".to_owned(), OptionValue::Text(effort.to_owned()))]
            .into_iter()
            .collect(),
    }
}

fn effort(start: &SessionStart) -> Option<&OptionValue> {
    start.options.get("effort")
}

async fn set_selection(world: &mut World, channel_id: ChannelId, value: Option<ModelSelection>) {
    let update: ChannelUpdate = serde_json::from_value(json!({
        "channel_id": channel_id,
        "coordinator_selection": value,
    }))
    .unwrap();
    world.client.request(update).await.unwrap();
}

async fn channel(world: &mut World, name: &str, own: Option<ModelSelection>) -> ChannelId {
    world
        .client
        .request(ChannelCreate {
            name: name.to_owned(),
            purpose: String::new(),
            instructions: String::new(),
            icon: None,
            color: None,
            group_id: None,
            coordinator_selection: own,
            task_selection: None,
            repo_ids: Vec::new(),
            default_isolation: None,
            default_approval_mode: None,
            memory_enabled: None,
            tags: Vec::new(),
        })
        .await
        .unwrap()
        .channel
        .id
}

/// Answers the next command, which must be `session.stop` for `session_id`.
async fn expect_stop(worker: &mut FakeWorker, session_id: SessionId) {
    let (request, command) = worker.next_command().await;
    let HubToSessionHost::SessionStop(stop) = command else {
        panic!("expected session.stop, got {}", command.type_name());
    };
    assert_eq!(stop.session_id, session_id);
    worker.respond_ok(&request, &json!({})).await;
}

/// Answers the next `session.start` of each listed channel, in any order.
async fn starts_for(worker: &mut FakeWorker, channels: &[ChannelId]) -> Vec<SessionStart> {
    let mut out: Vec<SessionStart> = Vec::new();
    while out.len() < channels.len() {
        let (request, command) = worker.next_command().await;
        let HubToSessionHost::SessionStart(start) = command else {
            panic!("expected session.start, got {}", command.type_name());
        };
        worker
            .respond_ok(
                &request,
                &SessionStartResult {
                    workspace_path: "/tmp/workspace".to_owned(),
                    native_ref: None,
                },
            )
            .await;
        assert!(channels.contains(&start.channel_id));
        out.push(start);
    }
    out.sort_by_key(|s| channels.iter().position(|c| *c == s.channel_id));
    out
}

#[tokio::test]
async fn a_new_model_ends_an_idle_session_and_the_next_turn_uses_it() {
    let mut world = World::start().await;
    let first = world.start_coordinator("First").await;
    assert_eq!(first.model, "opus");
    world
        .worker
        .send_events(
            first.session_id,
            vec![turn_ended("t1", TurnEndReason::Completed)],
        )
        .await;

    let tutti = world.tutti;
    set_selection(&mut world, tutti, Some(selection("sonnet", "high"))).await;
    // No turn runs, so the session ends at once.
    expect_stop(&mut world.worker, first.session_id).await;

    world.say(tutti, "Second").await;
    let next = world.worker.accept_session_start().await;
    assert_ne!(next.session_id, first.session_id);
    assert_eq!(next.model, "sonnet");
    assert_eq!(effort(&next), Some(&OptionValue::Text("high".into())));
    assert!(
        next.turn.as_ref().unwrap().text.contains("<context>"),
        "the new session gets the context from the hub"
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_new_effort_alone_also_reaches_the_next_session() {
    let mut world = World::start().await;
    let tutti = world.tutti;
    set_selection(&mut world, tutti, Some(selection("sonnet", "low"))).await;
    let first = world.start_coordinator("First").await;
    assert_eq!(effort(&first), Some(&OptionValue::Text("low".into())));
    world
        .worker
        .send_events(
            first.session_id,
            vec![turn_ended("t1", TurnEndReason::Completed)],
        )
        .await;
    set_selection(&mut world, tutti, Some(selection("sonnet", "high"))).await;
    expect_stop(&mut world.worker, first.session_id).await;
    world.say(tutti, "Think harder").await;
    let next = world.worker.accept_session_start().await;
    assert_eq!(next.model, "sonnet");
    assert_eq!(effort(&next), Some(&OptionValue::Text("high".into())));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_running_turn_is_not_cut_and_the_turn_after_it_uses_the_new_model() {
    let mut world = World::start().await;
    // The first turn is open: the worker sent `turn.started` and no end.
    let first = world.start_coordinator("First").await;
    let tutti = world.tutti;
    set_selection(&mut world, tutti, Some(selection("sonnet", "high"))).await;
    if let Some((_, command)) = world.worker.try_next_command(QUIET).await {
        panic!(
            "the running turn must go on, but the worker got {}",
            command.type_name()
        );
    }

    // A message waits for the turn. When the turn ends, the hub replaces the
    // session, and the waiting message goes to the new one.
    world.say(tutti, "Second").await;
    world
        .worker
        .send_events(
            first.session_id,
            vec![
                assistant_text(
                    first.session_id,
                    "Done.",
                    consort_proto::model::ItemStatus::Completed,
                ),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    expect_stop(&mut world.worker, first.session_id).await;
    let next = world.worker.accept_session_start().await;
    assert_eq!(next.model, "sonnet");
    assert_eq!(effort(&next), Some(&OptionValue::Text("high".into())));
    assert!(next.turn.as_ref().unwrap().text.contains("Second"));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn tutti_sets_the_default_for_channels_without_their_own_model() {
    let mut world = World::start().await;
    let tutti = world.tutti;
    let inherits = channel(&mut world, "Inherits", None).await;
    let own = channel(&mut world, "Own", Some(selection("opus", "low"))).await;
    set_selection(&mut world, tutti, Some(selection("sonnet", "high"))).await;

    world.say(inherits, "Hello").await;
    world.say(own, "Hello").await;
    let starts = starts_for(&mut world.worker, &[inherits, own]).await;
    assert_eq!(starts[0].model, "sonnet", "the Tutti default");
    assert_eq!(effort(&starts[0]), Some(&OptionValue::Text("high".into())));
    assert_eq!(starts[1].model, "opus", "the channel's own model");
    assert_eq!(effort(&starts[1]), Some(&OptionValue::Text("low".into())));
    for start in &starts {
        world
            .worker
            .send_events(
                start.session_id,
                vec![
                    state(SessionState::Active),
                    turn_started("t1"),
                    turn_ended("t1", TurnEndReason::Completed),
                ],
            )
            .await;
    }

    // A new Tutti default moves only the channel that inherits it.
    set_selection(&mut world, tutti, Some(selection("opus", "high"))).await;
    expect_stop(&mut world.worker, starts[0].session_id).await;
    if let Some((_, command)) = world.worker.try_next_command(QUIET).await {
        panic!(
            "the channel with its own model must keep its session, but the worker got {}",
            command.type_name()
        );
    }

    // Clearing the channel's own model makes it use the Tutti model.
    set_selection(&mut world, own, None).await;
    expect_stop(&mut world.worker, starts[1].session_id).await;
    world.say(own, "Again").await;
    let next = world.worker.accept_session_start().await;
    assert_eq!(next.channel_id, own);
    assert_eq!(next.model, "opus");
    assert_eq!(effort(&next), Some(&OptionValue::Text("high".into())));
    world.hub.handle.shutdown().await.unwrap();
}
