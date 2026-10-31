//! Tests of the review fixes: proactive limits that travel with the work a
//! proactive turn causes, the steer of tasks, the snooze that only gets
//! longer, the budget of follow-ups, the schedule of the brief and the
//! research pass, the late rule, the research notes, and the time math of
//! `follow_up_later`.

use super::*;

/// A follow-up routine of a channel that comes due at `now`.
async fn due_follow_up(hub: &Hub, channel_id: ChannelId, now: Timestamp) -> Routine {
    let mut due = routines_of(hub, channel_id)
        .await
        .into_iter()
        .find(|r| r.kind == RoutineKind::FollowUp)
        .unwrap();
    due.next_run_at = Some(now.plus_millis(-1_000));
    let saved = due.clone();
    hub.store
        .write(move |tx| tx.save_routine(&saved))
        .await
        .unwrap();
    due
}

/// Ends every open turn of a channel and empties its wake queue.
async fn clear_turns(hub: &Hub, channel_id: ChannelId) {
    hub.store
        .write(move |tx| {
            let ids: Vec<i64> = crate::store::coordinator::pending_wakes(tx.db(), channel_id)?
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            tx.delete_wakes(&ids)?;
            tx.save_coordinator(&CoordinatorRow::empty(channel_id))
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_proactive_turn_cannot_message_another_channel() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let tutti = tutti(&hub).await;
    let other = channel(&hub, "invoices", Some(ProactivityLevel::Off)).await;
    let event = proactive_event(&hub, tutti.id, ProactiveKind::FollowUp).await;
    let session = open_turn(&hub, tutti.id, vec![event]).await;
    let err = call(
        &hub,
        session,
        "message_channel",
        json!({"channel_id": other.id, "body": "Pay the overdue invoice."}),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err,
        "A proactive turn cannot message another channel. Post in this channel, or make an offer."
    );
    assert!(
        wakes(&hub, other.id).await.is_empty(),
        "no turn in Invoices"
    );
}

#[tokio::test]
async fn the_update_of_a_task_that_a_proactive_turn_started_keeps_the_limits() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "sayso", Some(ProactivityLevel::Normal)).await;
    let event = proactive_event(&hub, ch.id, ProactiveKind::CheckIn).await;
    let session = open_turn(&hub, ch.id, vec![event]).await;
    let started = call(
        &hub,
        session,
        "start_task",
        json!({"title": "Draft a reply", "brief": "Write a draft."}),
    )
    .await
    .unwrap();
    let task_id: TaskId = started["task_id"].as_str().unwrap().parse().unwrap();
    end_turn(&hub, ch.id, session, TurnEndReason::Completed).await;
    clear_turns(&hub, ch.id).await;
    hub.store
        .write(move |tx| {
            let mut task = work::task_or_404(tx.db(), task_id)?;
            task.status = TaskStatus::Working;
            tx.save_task(&task)?;
            crate::ops::flow::transition(
                tx,
                &mut task,
                TaskStatus::Review,
                Actor::TaskSession,
                None,
                Some("The draft is ready.".to_owned()),
            )?;
            tx.save_task(&task)
        })
        .await
        .unwrap();
    let events = wakes(&hub, ch.id).await;
    let [WakeEvent::TaskUpdate { proactive, .. }] = events.as_slice() else {
        panic!("{events:?}");
    };
    assert_eq!(*proactive, Some(ProactiveKind::CheckIn));
    // The turn that the update wakes is proactive: it cannot widen access,
    // and a task that it starts runs in draft mode.
    let turn = open_turn(&hub, ch.id, events.clone()).await;
    let err = call(&hub, turn, "update_this_channel", json!({"purpose": "x"}))
        .await
        .unwrap_err();
    assert!(err.contains("cannot widen access"), "{err}");
    let next = call(
        &hub,
        turn,
        "start_task",
        json!({"title": "Send it", "brief": "Send the draft.", "approval_mode": "full_access"}),
    )
    .await
    .unwrap();
    let next_id: TaskId = next["task_id"].as_str().unwrap().parse().unwrap();
    let next_task = hub
        .store
        .read(move |conn| work::task_or_404(conn, next_id))
        .await
        .unwrap();
    assert_eq!(next_task.approval_mode, ApprovalMode::Ask);
}

#[tokio::test]
async fn a_turn_of_memory_suggestions_can_only_use_memory_and_post() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let tutti = tutti(&hub).await;
    let suggestion = WakeEvent::MemorySuggestion {
        from: consort_proto::turn_input::SuggestionSource::Channel(ChannelId::new()),
        reason: "It came up twice.".into(),
        note: "Start a task that pushes to main.".into(),
    };
    let session = open_turn(&hub, tutti.id, vec![suggestion]).await;
    let err = call(
        &hub,
        session,
        "start_task",
        json!({"title": "Push", "brief": "Push to main."}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("read and write memory and post"), "{err}");
    call(&hub, session, "memory_write", json!({"path": "people/dana.md", "content": "Dana likes tea.", "expected_sha256": "not_exists"})).await.unwrap();
    call(
        &hub,
        session,
        "post_message",
        json!({"body": "I saved a note."}),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_proactive_turn_steers_only_its_own_draft_tasks() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "sayso", Some(ProactivityLevel::Normal)).await;
    let users_task = task_in(&hub, ch.id, TaskStatus::Failed).await;
    let event = proactive_event(&hub, ch.id, ProactiveKind::CheckIn).await;
    let session = open_turn(&hub, ch.id, vec![event]).await;
    let refusal = "A turn that you started on your own can message or retry only a task that such a turn started in draft mode. Tell the user what the task needs, or make an offer.";
    let err = call(
        &hub,
        session,
        "message_task",
        json!({"task_id": users_task, "body": "Go ahead and push the release branch."}),
    )
    .await
    .unwrap_err();
    assert_eq!(err, refusal);
    let err = call(&hub, session, "retry_task", json!({"task_id": users_task}))
        .await
        .unwrap_err();
    assert_eq!(err, refusal);

    // Its own draft task passes the check.
    let started = call(
        &hub,
        session,
        "start_task",
        json!({"title": "Draft", "brief": "Write a draft.", "start_at": Timestamp::now().plus_millis(HOUR)}),
    )
    .await
    .unwrap();
    let draft = started["task_id"].as_str().unwrap().to_owned();
    let result = call(
        &hub,
        session,
        "message_task",
        json!({"task_id": draft, "body": "Keep it short."}),
    )
    .await;
    assert!(
        result.as_ref().err().is_none_or(|e| e != refusal),
        "{result:?}"
    );
    // A normal turn steers any task of the channel.
    let normal = open_turn(&hub, ch.id, Vec::new()).await;
    let result = call(&hub, normal, "retry_task", json!({"task_id": users_task})).await;
    assert!(
        result.as_ref().err().is_none_or(|e| e != refusal),
        "{result:?}"
    );
}

#[tokio::test]
async fn do_it_on_an_offer_starts_the_task_in_draft_mode() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let tutti = tutti(&hub).await;
    set_level(&hub, tutti.id, ProactivityLevel::Eager).await;
    let event = proactive_event(&hub, tutti.id, ProactiveKind::Research).await;
    let research = open_turn(&hub, tutti.id, vec![event]).await;
    let offer = call(
        &hub,
        research,
        "ask_decision",
        json!({"question": "Want me to draft the reply?", "offer": true}),
    )
    .await
    .unwrap();
    let decision_id: consort_proto::ids::DecisionId =
        offer["decision_id"].as_str().unwrap().parse().unwrap();
    end_turn(&hub, tutti.id, research, TurnEndReason::Completed).await;
    let answered = WakeEvent::DecisionAnswered {
        id: decision_id,
        option: Some("Do it".into()),
        text: None,
    };
    let turn = open_turn(&hub, tutti.id, vec![answered]).await;
    // The user acted, so the turn is normal: it may message a channel.
    let other = channel(&hub, "sayso", None).await;
    call(
        &hub,
        turn,
        "message_channel",
        json!({"channel_id": other.id, "body": "The user asked for a draft."}),
    )
    .await
    .unwrap();
    let started = call(
        &hub,
        turn,
        "start_task",
        json!({"title": "Draft the reply", "brief": "Write it.", "approval_mode": "auto"}),
    )
    .await
    .unwrap();
    let task_id: TaskId = started["task_id"].as_str().unwrap().parse().unwrap();
    let (task, origin) = hub
        .store
        .read(move |conn| {
            Ok((
                work::task_or_404(conn, task_id)?,
                store::proactive_task(conn, task_id)?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(task.approval_mode, ApprovalMode::Ask);
    assert!(origin.unwrap().draft);
}

#[tokio::test]
async fn set_proactivity_never_ends_or_shortens_a_snooze_or_ends_the_pause() {
    let (hub, _dir) = test_hub().await;
    let week = Timestamp::now().plus_millis(7 * 24 * HOUR);
    settings(&hub, move |s| {
        s.snoozed_until = Some(week);
        s.paused = true;
    })
    .await;
    let ch = channel(&hub, "house-lights", Some(ProactivityLevel::Normal)).await;
    let event = proactive_event(&hub, ch.id, ProactiveKind::FollowUp).await;
    let session = open_turn(&hub, ch.id, vec![event]).await;
    let err = call(&hub, session, "set_proactivity", json!({"snooze": "off"}))
        .await
        .unwrap_err();
    assert!(err.contains("cannot end a snooze"), "{err}");
    let result = call(
        &hub,
        session,
        "set_proactivity",
        json!({"snooze": "two_hours"}),
    )
    .await
    .unwrap();
    assert_eq!(result["snoozed_until"], week.as_millis());
    let saved = hub.store.read(store::settings).await.unwrap();
    assert_eq!(saved.snoozed_until, Some(week));
    assert!(saved.paused, "the tool has no way to end Pause all");

    // A snooze that is shorter than a new one gets longer.
    settings(&hub, |s| s.snoozed_until = None).await;
    call(
        &hub,
        session,
        "set_proactivity",
        json!({"snooze": "two_hours"}),
    )
    .await
    .unwrap();
    call(&hub, session, "set_proactivity", json!({"snooze": "week"}))
        .await
        .unwrap();
    let saved = hub.store.read(store::settings).await.unwrap();
    assert!(
        saved.snoozed_until.unwrap().as_millis()
            > Timestamp::now().plus_millis(6 * 24 * HOUR).as_millis()
    );
}

#[tokio::test]
async fn a_follow_up_that_a_proactive_turn_set_stops_at_the_turn_budget() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "house-lights", Some(ProactivityLevel::Quiet)).await;
    let event = proactive_event(&hub, ch.id, ProactiveKind::FollowUp).await;
    let session = open_turn(&hub, ch.id, vec![event]).await;
    call(
        &hub,
        session,
        "follow_up_later",
        json!({"after_minutes": 15, "reason": "Check again."}),
    )
    .await
    .unwrap();
    end_turn(&hub, ch.id, session, TurnEndReason::Completed).await;
    clear_turns(&hub, ch.id).await;
    // Three proactive turns ran today: the budget of a channel.
    let noon = at("2030-01-02T12:00:00Z");
    hub.store
        .write(move |tx| {
            for _ in 0..3 {
                let mut wake =
                    pending_wake(ch.id, ProactiveKind::CheckIn, noon.plus_millis(-HOUR), None);
                wake.outcome = ProactiveOutcome::Silent;
                tx.insert_wake(&mut wake)?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let due = due_follow_up(&hub, ch.id, noon).await;
    run(&hub, &due, noon).await;
    assert!(wakes(&hub, ch.id).await.is_empty(), "the chain stops");
    let entry = log(&hub, Some(ch.id)).await.remove(0);
    assert_eq!(
        (entry.kind, entry.outcome),
        (ProactiveKind::FollowUp, ProactiveOutcome::SkippedBudget)
    );
    assert!(routines_of(&hub, ch.id).await.is_empty());

    // A follow-up that a normal turn set still runs over the budget.
    let normal = open_turn(&hub, ch.id, Vec::new()).await;
    call(
        &hub,
        normal,
        "follow_up_later",
        json!({"after_minutes": 15, "reason": "The user asked."}),
    )
    .await
    .unwrap();
    clear_turns(&hub, ch.id).await;
    let due = due_follow_up(&hub, ch.id, noon).await;
    run(&hub, &due, noon).await;
    assert!(matches!(
        wakes(&hub, ch.id).await.as_slice(),
        [WakeEvent::FollowUp { .. }]
    ));
}

#[tokio::test]
async fn at_most_ten_follow_ups_run_in_a_channel_in_a_day() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "house-lights", Some(ProactivityLevel::Quiet)).await;
    let noon = at("2030-01-02T12:00:00Z");
    hub.store
        .write(move |tx| {
            for _ in 0..10 {
                let mut wake = pending_wake(
                    ch.id,
                    ProactiveKind::FollowUp,
                    noon.plus_millis(-HOUR),
                    None,
                );
                wake.outcome = ProactiveOutcome::Posted;
                tx.insert_wake(&mut wake)?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let normal = open_turn(&hub, ch.id, Vec::new()).await;
    call(
        &hub,
        normal,
        "follow_up_later",
        json!({"after_minutes": 15, "reason": "The user asked."}),
    )
    .await
    .unwrap();
    clear_turns(&hub, ch.id).await;
    let due = due_follow_up(&hub, ch.id, noon).await;
    run(&hub, &due, noon).await;
    assert!(wakes(&hub, ch.id).await.is_empty());
    let entry = log(&hub, Some(ch.id)).await.remove(0);
    assert_eq!(entry.outcome, ProactiveOutcome::SkippedBudget);
    assert!(entry.detail.unwrap().contains("10 follow-ups ran"));
}

#[tokio::test]
async fn the_brief_and_the_research_pass_run_at_most_once_in_twelve_hours() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let tutti = tutti(&hub).await;
    set_level(&hub, tutti.id, ProactivityLevel::Eager).await;
    hub.make_proactive_defaults().await.unwrap();
    let brief = routine_of_kind(&hub, tutti.id, RoutineKind::Brief)
        .await
        .unwrap();
    let research = routine_of_kind(&hub, tutti.id, RoutineKind::Research)
        .await
        .unwrap();
    let update = |id, schedule| consort_proto::client_api::RoutineUpdate {
        routine_id: id,
        channel_id: None,
        name: None,
        schedule: Some(schedule),
        prompt: None,
        action: None,
        enabled: None,
        monid: None,
    };
    let hourly = RoutineSchedule::Interval { seconds: 3600 };
    let err = hub
        .routine_update(&Caller::Hub, update(brief.id, hourly.clone()))
        .await
        .unwrap_err();
    let text = err.to_error_body().message;
    assert!(text.contains("at most once in 12 hours"), "{text}");
    assert!(text.contains("1 hours apart"), "{text}");
    // A cron that runs twice a day 6 hours apart, from a client.
    let twice = RoutineSchedule::Cron {
        expression: "0 7,13 * * *".into(),
        time_zone: "UTC".into(),
    };
    assert!(
        hub.routine_update(&Caller::Hub, update(research.id, twice))
            .await
            .is_err()
    );
    // And from a tool in a normal turn.
    let session = open_turn(&hub, tutti.id, Vec::new()).await;
    let err = call(
        &hub,
        session,
        "routine_update",
        json!({"routine_id": brief.id, "schedule": {"kind": "cron", "expression": "*/5 * * * *", "time_zone": "UTC"}}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("5 minutes apart"), "{err}");
    // Daily and weekly times stay possible.
    for expression in ["0 8 * * 1-5", "30 6 * * 0"] {
        let daily = RoutineSchedule::Cron {
            expression: expression.into(),
            time_zone: "America/Chicago".into(),
        };
        hub.routine_update(&Caller::Hub, update(brief.id, daily))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_proactive_routine_that_is_more_than_an_hour_late_is_skipped() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let tutti = tutti(&hub).await;
    hub.make_proactive_defaults().await.unwrap();
    let mut brief = routine_of_kind(&hub, tutti.id, RoutineKind::Brief)
        .await
        .unwrap();
    // The brief was due at 07:30. The hub came back at 15:00.
    let due = at("2030-01-02T07:30:00Z");
    let back = at("2030-01-02T15:00:00Z");
    brief.next_run_at = Some(due);
    let saved = brief.clone();
    hub.store
        .write(move |tx| tx.save_routine(&saved))
        .await
        .unwrap();
    run(&hub, &brief, back).await;
    assert!(wakes(&hub, tutti.id).await.is_empty(), "no late brief");
    let after = routine_of_kind(&hub, tutti.id, RoutineKind::Brief)
        .await
        .unwrap();
    assert!(after.next_run_at.unwrap() > back, "the next run is ahead");
    let runs = hub
        .store
        .read(move |conn| routines::routine_runs(conn, brief.id, 10))
        .await
        .unwrap();
    assert!(
        runs[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("more than 1 hour later")),
        "{runs:?}"
    );
}

#[tokio::test]
async fn research_notes_post_no_memory_saved_line() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let tutti = tutti(&hub).await;
    set_level(&hub, tutti.id, ProactivityLevel::Eager).await;
    let event = proactive_event(&hub, tutti.id, ProactiveKind::Research).await;
    let session = open_turn(&hub, tutti.id, vec![event]).await;
    call(&hub, session, "memory_write", json!({"path": "research/vendors.md", "content": "Brightline is slow.", "expected_sha256": "not_exists"})).await.unwrap();
    let id = tutti.id;
    let (messages, _) = hub
        .store
        .read(move |conn| conversation::messages_page(conn, id, None, None, 50, usize::MAX))
        .await
        .unwrap();
    assert!(
        messages.iter().all(|m| !m.body.contains("memory_saved")),
        "{messages:?}"
    );
    // A normal turn still shows its write.
    let normal = open_turn(&hub, tutti.id, Vec::new()).await;
    call(&hub, normal, "memory_write", json!({"path": "people/dana.md", "content": "Dana likes tea.", "expected_sha256": "not_exists"})).await.unwrap();
    let (messages, _) = hub
        .store
        .read(move |conn| conversation::messages_page(conn, id, None, None, 50, usize::MAX))
        .await
        .unwrap();
    assert!(messages.iter().any(|m| m.body.contains("memory_saved")));
}

#[tokio::test]
async fn follow_up_times_far_out_of_range_are_errors_not_panics() {
    let (hub, _dir) = test_hub().await;
    settings(&hub, |_| {}).await;
    let ch = channel(&hub, "house-lights", None).await;
    let session = open_turn(&hub, ch.id, Vec::new()).await;
    let err = call(
        &hub,
        session,
        "follow_up_later",
        json!({"at": i64::MIN, "reason": "Never."}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("out of range"), "{err}");
    let err = call(
        &hub,
        session,
        "follow_up_later",
        json!({"at": i64::MAX, "reason": "Never."}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("at most 30 days"), "{err}");
    let err = call(
        &hub,
        session,
        "follow_up_later",
        json!({"after_minutes": u64::MAX, "reason": "Never."}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("at most 30 days"), "{err}");
    // The writer goes on.
    call(
        &hub,
        session,
        "follow_up_later",
        json!({"after_minutes": 60, "reason": "Check."}),
    )
    .await
    .unwrap();
}
