//! The assistant on its own, through a real hub, a client and the fake
//! worker: a check-in that the hub skips sends nothing to the worker, a
//! proactive turn drops its text and marks its posts, a new channel gets
//! the guided start with quick replies, Too noisy wakes the coordinator, and
//! the settings, the log and Cancel all follow-ups work over the client API.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use consort_proto::client_api::{
    ChannelCreate, ChannelUpdate, EntityPayload, GroupCreate, MemoryWrite, MessageList,
    MessageRate, ProactivityCancelFollowUps, ProactivityGet, ProactivityLog, ProactivitySet,
    ProactivitySnooze, SnapshotScope,
};
use consort_proto::ids::ChannelId;
use consort_proto::model::{
    Author, CHECK_IN_NOTE_PATH, ExpectedHash, ItemStatus, MemoryScope, ProactiveKind,
    ProactiveOutcome, ProactiveRating, ProactivityLevel, QuietHours, RoutineKind, SessionState,
    SnoozeChoice, SuggestionAction,
};
use consort_proto::session_host_link::TurnEndReason;
use consort_proto::time::Timestamp;
use serde_json::json;

use consort_proto::frame::MessageFamily;

use crate::common::*;

const QUIET: Duration = Duration::from_millis(400);
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;

fn create(name: &str) -> ChannelCreate {
    ChannelCreate {
        name: name.to_owned(),
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
    }
}

/// Quiet hours far from now, in UTC, so notifications may go out now.
fn hours_far_from_now() -> QuietHours {
    let at = |offset: i64| {
        let m = Timestamp::now()
            .plus_millis(offset)
            .as_millis()
            .div_euclid(MINUTE)
            .rem_euclid(24 * 60);
        format!("{:02}:{:02}", m / 60, m % 60)
    };
    QuietHours {
        start: at(12 * HOUR),
        end: at(12 * HOUR + 2 * MINUTE),
    }
}

/// A channel at Normal with its check-in. Returns the channel and the time
/// at which its check-in is due.
async fn channel_at_normal(world: &mut World, name: &str) -> (ChannelId, Timestamp) {
    // Tutti goes to Quiet, so its own check-in and brief stay out of the way.
    let mut tutti = ChannelUpdate::new(world.tutti);
    tutti.proactivity = Some(Some(ProactivityLevel::Quiet));
    world.client.request(tutti).await.unwrap();
    let channel = world.client.request(create(name)).await.unwrap().channel;
    let mut update = ChannelUpdate::new(channel.id);
    update.proactivity = Some(Some(ProactivityLevel::Normal));
    world.client.request(update).await.unwrap();
    let snapshot = world
        .client
        .snapshot(SnapshotScope::Channel(channel.id))
        .await;
    let check_in = snapshot
        .routines
        .iter()
        .find(|r| r.kind == RoutineKind::CheckIn)
        .expect("a check-in at Normal");
    (channel.id, check_in.next_run_at.unwrap())
}

async fn log_of(
    world: &mut World,
    channel_id: ChannelId,
) -> Vec<consort_proto::model::ProactiveWake> {
    world
        .client
        .request(ProactivityLog {
            channel_id: Some(channel_id),
            ..Default::default()
        })
        .await
        .unwrap()
        .wakes
}

#[tokio::test]
async fn a_check_in_with_nothing_new_never_reaches_the_worker() {
    let mut world = World::start().await;
    // The channel comes while paused, so it gets no greeting turn.
    let (channel_id, due) = channel_at_normal(&mut world, "house-lights").await;
    world
        .client
        .request(ProactivitySet {
            paused: Some(false),
            time_zone: Some("UTC".into()),
            quiet_hours: Some(hours_far_from_now()),
            ..Default::default()
        })
        .await
        .unwrap();
    world.hub.handle.run_routines_at(due).await;
    if let Some((_, command)) = world.worker.try_next_command(QUIET).await {
        panic!(
            "the hub skips the model, but the worker got {}",
            command.type_name()
        );
    }
    let log = log_of(&mut world, channel_id).await;
    assert_eq!(log[0].outcome, ProactiveOutcome::SkippedNoChange);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_proactive_turn_drops_its_text_and_marks_its_posts() {
    let mut world = World::start().await;
    // The channel comes while paused, so it gets no greeting turn.
    let (channel_id, due) = channel_at_normal(&mut world, "house-lights").await;
    world
        .client
        .request(ProactivitySet {
            paused: Some(false),
            time_zone: Some("UTC".into()),
            quiet_hours: Some(hours_far_from_now()),
            ..Default::default()
        })
        .await
        .unwrap();
    // The channel's note watches mail, so the check-in runs.
    world
        .client
        .request(MemoryWrite {
            scope: MemoryScope::Channel(channel_id),
            path: CHECK_IN_NOTE_PATH.to_owned(),
            content: "watch: outside\nWatch the Brightline mail.".to_owned(),
            expected_sha256: ExpectedHash::NotExists,
        })
        .await
        .unwrap();

    // A silent check-in: assistant text but no posting call.
    world.hub.handle.run_routines_at(due).await;
    let start = world.worker.accept_session_start().await;
    let text = start.turn.as_ref().unwrap().text.clone();
    assert!(text.contains("<check_in routine="), "{text}");
    // A proactive turn runs in Ask, whatever the channel's default mode.
    assert_eq!(
        start.turn_approval_mode,
        Some(consort_proto::model::ApprovalMode::Ask)
    );
    assert!(text.contains("Watch the Brightline mail."), "{text}");
    assert!(
        !text.contains("log"),
        "the log entry stays in the hub: {text}"
    );
    world
        .worker
        .send_events(
            start.session_id,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                assistant_text(
                    start.session_id,
                    "Nothing to report.",
                    ItemStatus::Completed,
                ),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    let entry = world
        .client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::ProactiveWake(w)) if w.outcome == ProactiveOutcome::Silent)
        })
        .await;
    let Some(EntityPayload::ProactiveWake(wake)) = upserted(&entry) else {
        unreachable!()
    };
    assert_eq!(wake.kind, ProactiveKind::CheckIn);
    let page = world
        .client
        .request(MessageList {
            channel_id,
            thread_root_id: None,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(
        page.messages.iter().all(|m| m.author != Author::Assistant),
        "the text of a proactive turn is not posted"
    );

    // The next check-in posts with notify. The post carries the mark.
    let next = due.plus_millis(6 * HOUR + 10 * MINUTE);
    world.hub.handle.run_routines_at(next).await;
    let (_, command) = world.worker.next_command().await;
    let consort_proto::session_host_link::HubToSessionHost::SessionTurn(turn) = command else {
        panic!("a turn in the same session");
    };
    assert!(turn.turn.text.contains("<check_in"));
    assert_eq!(
        turn.approval_mode,
        Some(consort_proto::model::ApprovalMode::Ask)
    );
    world
        .worker
        .send_events(start.session_id, vec![turn_started("t2")])
        .await;
    let posted = world
        .worker
        .tool_json(
            start.session_id,
            "post_message",
            json!({"body": "Brightline replied with a new quote.", "notify": true}),
        )
        .await;
    let id = posted["message_id"].as_str().unwrap().to_owned();
    let entry = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.id.to_string() == id && m.proactive.is_some()))
        .await;
    let Some(EntityPayload::Message(message)) = upserted(&entry) else {
        unreachable!()
    };
    let mark = message.proactive.clone().unwrap();
    assert_eq!(mark.kind, ProactiveKind::CheckIn);
    assert!(mark.notify);
    world
        .worker
        .send_events(
            start.session_id,
            vec![turn_ended("t2", TurnEndReason::Completed)],
        )
        .await;
    world
        .client
        .wait_feed(|e| {
            matches!(upserted(e), Some(EntityPayload::ProactiveWake(w)) if w.outcome == ProactiveOutcome::Notified)
        })
        .await;
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_new_channel_gets_the_guided_start_with_quick_replies() {
    let mut world = World::start_proactive().await;
    let channel = world
        .client
        .request(create("reading"))
        .await
        .unwrap()
        .channel;
    let start = world.worker.accept_session_start().await;
    assert_eq!(start.channel_id, channel.id);
    let text = start.turn.as_ref().unwrap().text.clone();
    assert!(
        text.contains("<channel_created name=\"reading\" kind=\"standard\"/>"),
        "{text}"
    );
    world
        .worker
        .send_events(
            start.session_id,
            vec![state(SessionState::Active), turn_started("t1")],
        )
        .await;
    world
        .worker
        .tool_json(
            start.session_id,
            "post_message",
            json!({"body": "Hello. This is reading, a new channel."}),
        )
        .await;
    let posted = world
        .worker
        .tool_json(
            start.session_id,
            "post_message",
            json!({"body": "Do you want me to help you set it up?", "suggestions": [
                {"label": "Yes"}, {"label": "Not now"}, {"label": "No, I set it up myself"}
            ]}),
        )
        .await;
    let id = posted["message_id"].as_str().unwrap().to_owned();
    let entry = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.id.to_string() == id && !m.suggestions.is_empty()))
        .await;
    let Some(EntityPayload::Message(message)) = upserted(&entry) else {
        unreachable!()
    };
    let labels: Vec<_> = message
        .suggestions
        .iter()
        .map(|s| s.label.as_str())
        .collect();
    assert_eq!(labels, ["Yes", "Not now", "No, I set it up myself"]);
    assert!(
        message
            .suggestions
            .iter()
            .all(|s| s.action == SuggestionAction::Send)
    );
    assert_eq!(
        message.proactive.as_ref().unwrap().kind,
        ProactiveKind::ChannelCreated
    );
    world
        .worker
        .send_events(
            start.session_id,
            vec![turn_ended("t1", TurnEndReason::Completed)],
        )
        .await;

    // The user taps Yes: a normal turn, so its posts carry no mark.
    world.say(channel.id, "Yes").await;
    let (_, command) = world.worker.next_command().await;
    let consort_proto::session_host_link::HubToSessionHost::SessionTurn(turn) = command else {
        panic!("a turn");
    };
    assert!(turn.turn.text.contains("<user_message"));
    assert_eq!(
        turn.approval_mode, None,
        "a normal turn uses the session's mode"
    );
    world
        .worker
        .send_events(start.session_id, vec![turn_started("t2")])
        .await;
    let question = world
        .worker
        .tool_json(
            start.session_id,
            "post_message",
            json!({"body": "What is reading for?", "suggestions": [
                {"label": "Books I want to read"}, {"label": "I will type it", "action": "type"}
            ]}),
        )
        .await;
    let id = question["message_id"].as_str().unwrap().to_owned();
    let entry = world
        .client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.id.to_string() == id && !m.suggestions.is_empty()))
        .await;
    let Some(EntityPayload::Message(question)) = upserted(&entry) else {
        unreachable!()
    };
    assert_eq!(question.proactive, None);
    assert_eq!(question.suggestions[1].action, SuggestionAction::Type);
    let log = log_of(&mut world, channel.id).await;
    assert_eq!(log[0].kind, ProactiveKind::ChannelCreated);
    assert_eq!(log[0].outcome, ProactiveOutcome::Posted);
    world.hub.handle.shutdown().await.unwrap();
}

/// The simple New channel dialog sends a name and an icon, and the "+" of a
/// group adds the group. Both still get the guided start.
#[tokio::test]
async fn a_channel_from_the_simple_dialog_in_a_group_gets_the_guided_start() {
    let mut world = World::start_proactive().await;
    let group = world
        .client
        .request(GroupCreate {
            name: "Home".to_owned(),
        })
        .await
        .unwrap()
        .group;
    let channel = world
        .client
        .request(ChannelCreate {
            icon: Some("book-open".to_owned()),
            group_id: Some(group.id),
            ..create("reading")
        })
        .await
        .unwrap()
        .channel;
    assert_eq!(channel.group_id, Some(group.id));
    let start = world.worker.accept_session_start().await;
    assert_eq!(start.channel_id, channel.id);
    let text = start.turn.as_ref().unwrap().text.clone();
    assert!(
        text.contains("<channel_created name=\"reading\" kind=\"standard\"/>"),
        "{text}"
    );
    let log = log_of(&mut world, channel.id).await;
    assert_eq!(log[0].kind, ProactiveKind::ChannelCreated);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn too_noisy_wakes_the_coordinator_with_the_rated_message() {
    let mut world = World::start_proactive().await;
    world
        .client
        .request(ProactivitySet {
            quiet_hours: Some(hours_far_from_now()),
            time_zone: Some("UTC".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let channel = world
        .client
        .request(create("reading"))
        .await
        .unwrap()
        .channel;
    let start = world.worker.accept_session_start().await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![state(SessionState::Active), turn_started("t1")],
        )
        .await;
    let posted = world
        .worker
        .tool_json(start.session_id, "post_message", json!({"body": "Hello."}))
        .await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![turn_ended("t1", TurnEndReason::Completed)],
        )
        .await;
    let message_id = posted["message_id"].as_str().unwrap().parse().unwrap();
    world
        .client
        .request(MessageRate {
            message_id,
            rating: Some(ProactiveRating::TooNoisy),
        })
        .await
        .unwrap();
    let (_, command) = world.worker.next_command().await;
    let consort_proto::session_host_link::HubToSessionHost::SessionTurn(turn) = command else {
        panic!("a turn");
    };
    assert!(
        turn.turn.text.contains(&format!(
            "<proactive_feedback message=\"{message_id}\" rating=\"too_noisy\"/>"
        )),
        "{}",
        turn.turn.text
    );
    let log = log_of(&mut world, channel.id).await;
    assert_eq!(log[0].kind, ProactiveKind::Feedback);
    world.hub.handle.shutdown().await.unwrap();
}

/// Only the test world pauses. A new hub starts with Pause all off, Tutti at
/// Normal and a new channel at Quiet (the plan's defaults).
#[tokio::test]
async fn a_new_hub_starts_with_pause_all_off_and_the_default_levels() {
    let mut world = World::start_proactive().await;
    let settings = world
        .client
        .request(ProactivityGet {})
        .await
        .unwrap()
        .settings;
    assert!(!settings.paused, "Pause all is off on a new hub");
    assert_eq!(settings.snoozed_until, None);
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    let tutti = snapshot
        .channels
        .iter()
        .find(|c| c.id == world.tutti)
        .expect("Tutti");
    assert_eq!(tutti.proactivity, None, "no level of its own");
    assert_eq!(tutti.proactivity_level(), ProactivityLevel::Normal);
    let channel = world
        .client
        .request(create("reading"))
        .await
        .unwrap()
        .channel;
    assert_eq!(channel.proactivity_level(), ProactivityLevel::Quiet);
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_settings_snooze_the_log_and_cancel_all_work_over_the_client_api() {
    let mut world = World::start().await;
    let start = world
        .client
        .request(ProactivityGet {})
        .await
        .unwrap()
        .settings;
    assert!(start.paused, "the test world pauses");
    assert_eq!(start.quiet_hours, QuietHours::default());
    assert_eq!(start.notify_cap_per_day, 3);

    let changed = world
        .client
        .request(ProactivitySet {
            paused: Some(false),
            quiet_hours: Some(QuietHours {
                start: "22:00".into(),
                end: "06:00".into(),
            }),
            time_zone: Some("America/Chicago".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .settings;
    assert!(!changed.paused);
    assert_eq!(changed.time_zone.as_deref(), Some("America/Chicago"));
    world
        .client
        .wait_feed(
            |e| matches!(upserted(e), Some(EntityPayload::ProactivitySettings(s)) if !s.paused),
        )
        .await;
    let bad = world
        .client
        .request(ProactivitySet {
            quiet_hours: Some(QuietHours {
                start: "25:00".into(),
                end: "06:00".into(),
            }),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(bad.code, consort_proto::error::ErrorCode::Invalid);

    let snoozed = world
        .client
        .request(ProactivitySnooze {
            choice: SnoozeChoice::TwoHours,
        })
        .await
        .unwrap()
        .settings;
    let until = snoozed.snoozed_until.unwrap().as_millis() - Timestamp::now().as_millis();
    assert!((2 * HOUR - MINUTE..=2 * HOUR).contains(&until), "{until}");
    let ended = world
        .client
        .request(ProactivitySnooze {
            choice: SnoozeChoice::Off,
        })
        .await
        .unwrap()
        .settings;
    assert_eq!(ended.snoozed_until, None);

    // The snapshot holds the settings, and the hub made Tutti's routines.
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    assert_eq!(
        snapshot.proactivity.unwrap().time_zone.as_deref(),
        Some("America/Chicago")
    );
    let kinds: Vec<_> = snapshot.routines.iter().map(|r| r.kind).collect();
    assert!(kinds.contains(&RoutineKind::CheckIn) && kinds.contains(&RoutineKind::Brief));

    // A follow-up that the assistant set, then Cancel all follow-ups.
    let coordinator = world.start_coordinator("Remind me later").await;
    world
        .worker
        .tool_json(
            coordinator.session_id,
            "follow_up_later",
            json!({"after_minutes": 60, "reason": "Check the vendor reply."}),
        )
        .await;
    let canceled = world
        .client
        .request(ProactivityCancelFollowUps { channel_id: None })
        .await
        .unwrap()
        .canceled;
    assert_eq!(canceled, 1);
    let snapshot = world.client.snapshot(SnapshotScope::All).await;
    assert!(
        snapshot
            .routines
            .iter()
            .all(|r| r.kind != RoutineKind::FollowUp)
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_stale_task_wakes_its_coordinator_once() {
    let mut world = World::start().await;
    let channel = world.client.request(create("sayso")).await.unwrap().channel;
    world
        .client
        .request(ProactivitySet {
            paused: Some(false),
            time_zone: Some("UTC".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let task = world
        .client
        .request(consort_proto::client_api::TaskCreate {
            channel_id: channel.id,
            title: "Fix it".to_owned(),
            brief: "Fix the bug.".to_owned(),
            model_selection: None,
            placement: consort_proto::model::PlacementRequest::default(),
            approval_mode: None,
            start_at: None,
        })
        .await
        .unwrap()
        .task;
    let task_start = world.worker.accept_session_start().await;
    let s = task_start.session_id;
    world
        .worker
        .send_events(
            s,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                assistant_text(s, "Done.", ItemStatus::Completed),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;
    // Ready for review wakes the coordinator. The worker takes that turn.
    let coordinator = world.worker.accept_session_start().await;
    assert!(
        coordinator
            .turn
            .as_ref()
            .unwrap()
            .text
            .contains("<task_update")
    );
    world
        .worker
        .send_events(
            coordinator.session_id,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                turn_ended("t1", TurnEndReason::Completed),
            ],
        )
        .await;

    // 72 hours later, at noon, the hub nudges once.
    let day = &Timestamp::now().plus_millis(4 * 24 * HOUR).to_rfc3339()[..10];
    let noon = Timestamp::parse_rfc3339(&format!("{day}T12:00:00Z")).unwrap();
    world.hub.handle.nudge_stale_tasks_at(noon).await.unwrap();
    let (_, command) = world.worker.next_command().await;
    let consort_proto::session_host_link::HubToSessionHost::SessionTurn(nudge) = command else {
        panic!("the nudge turn");
    };
    assert!(
        nudge
            .turn
            .text
            .contains(&format!("<stale_task task=\"{}\"", task.id)),
        "{}",
        nudge.turn.text
    );
    world
        .worker
        .send_events(
            coordinator.session_id,
            vec![
                turn_started("t2"),
                turn_ended("t2", TurnEndReason::Completed),
            ],
        )
        .await;
    world
        .hub
        .handle
        .nudge_stale_tasks_at(noon.plus_millis(HOUR))
        .await
        .unwrap();
    if let Some((_, command)) = world.worker.try_next_command(QUIET).await {
        panic!(
            "one nudge per status entry, but the worker got {}",
            command.type_name()
        );
    }
    world.hub.handle.shutdown().await.unwrap();
}
