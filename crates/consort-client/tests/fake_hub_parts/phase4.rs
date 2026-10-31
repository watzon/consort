//! Phase 4 against the fake hub: reactions, message threads, search, New
//! task, archived tasks, the task item view, channel settings and groups.
//! `fake_hub.rs` sends these request types here.

use super::*;
use consort_client::convo::{
    ItemDetailState, ItemKind, NewTask, RepoChoice, RunsOn, SearchLoad, SearchScope, StartWhen,
};
use consort_proto::model::{
    CommandPayload, FileChange, FileChangeKind, FileChangePayload, ItemPayload, ItemStatus,
    Reaction, Repo, SessionItem,
};
use consort_proto::tools::{SearchHit, SearchKind};

/// The phase 4 state of the fake hub.
#[derive(Default)]
pub struct State {
    /// `task.item_detail` answers `worker_offline`.
    pub item_offline: bool,
    /// The archived tasks that `task.list_archived` lists.
    pub archived: Vec<Task>,
    /// The session of the items that `task.items` gives.
    pub session: Option<SessionId>,
}

/// The request types that this part answers.
pub fn handles(kind: &str) -> bool {
    matches!(
        kind,
        "message.react"
            | "message.search"
            | "task.create"
            | "task.delete"
            | "task.list_archived"
            | "task.items"
            | "task.item_detail"
            | "group.delete"
            | "group.reorder"
            | "repo.create"
            | "channel.update"
    )
}

fn items(session: SessionId) -> Vec<SessionItem> {
    let item = |payload: ItemPayload, at: i64| SessionItem {
        id: consort_proto::ids::ItemId::new(),
        session_id: session,
        turn_id: consort_proto::ids::TurnId::new("t1"),
        payload,
        status: ItemStatus::Completed,
        created_at: ts(at),
        updated_at: ts(at),
    };
    vec![
        item(
            ItemPayload::Command(CommandPayload {
                command: "cargo build --release".into(),
                output: "tail".into(),
                output_truncated: true,
                exit_code: Some(0),
                duration_ms: Some(134_000),
                ..CommandPayload::default()
            }),
            1_000,
        ),
        item(
            ItemPayload::FileChange(FileChangePayload {
                changes: vec![FileChange {
                    path: "engine/src/tray.rs".into(),
                    change: FileChangeKind::Modify,
                    from_path: None,
                    lines_added: 10,
                    lines_removed: 4,
                    diff: None,
                }],
            }),
            2_000,
        ),
    ]
}

/// Answers one request, and returns the feed frames to push after it.
pub fn answer(hub: &Shared, req: &RequestFrame) -> (Frame, Vec<Frame>) {
    let mut h = hub.lock().unwrap();
    h.recorded.push((req.kind.clone(), req.body.clone()));
    let mut events = Vec::new();
    let reply = match req.kind.as_str() {
        "message.react" => {
            let body: api::MessageReact = req.decode_as().unwrap();
            let Some(message) = h.messages.iter_mut().find(|m| m.id == body.message_id) else {
                return (
                    req.respond_error(ErrorBody::new(ErrorCode::NotFound, "No such message.")),
                    events,
                );
            };
            message
                .reactions
                .retain(|r| !(r.author == Author::User && r.emoji == body.emoji));
            if !body.remove {
                message.reactions.push(Reaction {
                    author: Author::User,
                    emoji: body.emoji,
                    created_at: Timestamp::now(),
                });
            }
            let message = message.clone();
            events.push(feed(&mut h, EntityPayload::Message(message)));
            req.respond_ok(&Empty {}).unwrap()
        }
        "message.search" => {
            let body: api::MessageSearch = req.decode_as().unwrap();
            let channel = h.channel.as_ref().unwrap().id;
            let message = h.messages.iter().find(|m| m.kind == MessageKind::Text);
            let mut hits = Vec::new();
            if let Some(m) = message
                && (body.kinds.is_empty() || body.kinds.contains(&SearchKind::Message))
            {
                hits.push(SearchHit::Message {
                    message_id: m.id,
                    channel_id: channel,
                    thread_root_id: None,
                    snippet: format!("{} found here.", body.query),
                    created_at: m.created_at,
                });
            }
            if body.kinds.is_empty() || body.kinds.contains(&SearchKind::MemoryNote) {
                hits.push(SearchHit::MemoryNote {
                    scope: consort_proto::model::MemoryScope::Global,
                    path: "people/chris.md".into(),
                    snippet: format!("Chris and {}.", body.query),
                    updated_at: Some(Timestamp::now()),
                });
            }
            req.respond_ok(&consort_proto::tools::SearchResult { hits })
                .unwrap()
        }
        "task.create" => {
            let body: api::TaskCreate = req.decode_as().unwrap();
            let mut task = h.task.clone().unwrap();
            task.id = TaskId::new();
            task.title = body.title;
            task.brief = body.brief;
            req.respond_ok(&TaskResult { task }).unwrap()
        }
        "task.delete" | "group.delete" | "group.reorder" => req.respond_ok(&Empty {}).unwrap(),
        "task.list_archived" => {
            let body: api::TaskListArchived = req.decode_as().unwrap();
            let offset = body.offset.unwrap_or(0) as usize;
            let tasks: Vec<Task> = h
                .phase4
                .archived
                .iter()
                .skip(offset)
                .take(1)
                .cloned()
                .collect();
            let has_more = h.phase4.archived.len() > offset + 1;
            req.respond_ok(&api::TaskListArchivedResult {
                tasks,
                tasks_cut: Vec::new(),
                has_more,
            })
            .unwrap()
        }
        "task.items" => {
            let session = *h.phase4.session.get_or_insert_with(SessionId::new);
            req.respond_ok(&api::TaskItemsResult {
                items: items(session),
                has_more: false,
            })
            .unwrap()
        }
        "task.item_detail" => {
            if h.phase4.item_offline {
                req.respond_error(ErrorBody::new(
                    ErrorCode::WorkerOffline,
                    "The worker is offline.",
                ))
            } else {
                use sha2::Digest as _;
                let text = "line 1\nline 2\nFinished\n".as_bytes().to_vec();
                let digest: [u8; 32] = sha2::Sha256::digest(&text).into();
                let sha = consort_proto::model::Sha256Hash::from_bytes(digest);
                h.blobs.insert(sha.to_string(), text.clone());
                req.respond_ok(&consort_proto::session_host_link::ItemDetailResult {
                    sha256: sha,
                    size: text.len() as u64,
                    mime: "text/plain".into(),
                })
                .unwrap()
            }
        }
        "repo.create" => {
            let body: api::RepoCreate = req.decode_as().unwrap();
            req.respond_ok(&api::RepoResult {
                repo: Repo {
                    id: consort_proto::ids::RepoId::new(),
                    name: "watzon/notes".into(),
                    default_branch: "main".into(),
                    url: body.url,
                },
            })
            .unwrap()
        }
        "channel.update" => {
            let channel = h.channel.clone().unwrap();
            req.respond_ok(&api::ChannelResult { channel }).unwrap()
        }
        other => req.respond_error(ErrorBody::new(
            ErrorCode::Invalid,
            format!("The phase 4 part has no {other}."),
        )),
    };
    (reply, events)
}

fn recorded(hub: &Shared, kind: &str) -> Vec<serde_json::Value> {
    hub.lock()
        .unwrap()
        .recorded
        .iter()
        .filter(|(k, _)| k == kind)
        .map(|(_, b)| b.clone())
        .collect()
}

/// Adds a text message from the assistant to Tutti and returns its ID.
fn add_text(hub: &Shared, body: &str) -> MessageId {
    let mut h = hub.lock().unwrap();
    let channel_id = h.channel.as_ref().unwrap().id;
    let id = MessageId::new();
    h.messages.push(Message {
        id,
        channel_id,
        thread_root_id: None,
        author: Author::Assistant,
        author_session_id: None,
        author_task_id: None,
        author_routine_id: None,
        kind: MessageKind::Text,
        body: body.into(),
        attachments: vec![],
        in_timeline: false,
        client_message_id: None,
        reply_count: 1,
        last_reply_at: Some(Timestamp::now()),
        reactions: vec![Reaction {
            author: Author::Assistant,
            emoji: "👍".into(),
            created_at: Timestamp::now(),
        }],
        edited_at: None,
        deleted_at: None,
        created_at: Timestamp::now(),
        proactive: None,
        suggestions: Vec::new(),
        source_message_id: None,
    });
    id
}

fn message_in(state: &ViewState, id: &str) -> Option<consort_client::MessageView> {
    state
        .timelines
        .iter()
        .flat_map(|t| t.items.iter())
        .find_map(|i| match i {
            TimelineItem::Message(m) if m.id == id => Some(m.clone()),
            _ => None,
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reaction_shows_at_once_goes_to_the_hub_and_a_second_click_removes_it() {
    let (url, hub) = start_hub().await;
    let root = add_text(&hub, "One step is left.").to_string();
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let state = wait_for(&client, "the message", |s| message_in(s, &root).is_some()).await;
    let m = message_in(&state, &root).unwrap();
    assert_eq!(m.reactions.len(), 1);
    assert!(m.reactions[0].assistant && !m.reactions[0].mine);
    assert_eq!(m.reply_count, 1);
    assert!(
        m.last_reply_label
            .as_deref()
            .unwrap()
            .starts_with("Last reply ")
    );

    client
        .run(Command::React {
            message_id: root.clone(),
            emoji: "👍".into(),
            remove: false,
        })
        .await
        .unwrap();
    let state = wait_for(&client, "both reacted", |s| {
        message_in(s, &root).is_some_and(|m| m.reactions.first().is_some_and(|r| r.mine))
    })
    .await;
    let chip = &message_in(&state, &root).unwrap().reactions[0];
    assert_eq!(chip.count_label().as_deref(), Some("2"));
    client
        .run(Command::React {
            message_id: root.clone(),
            emoji: "👍".into(),
            remove: true,
        })
        .await
        .unwrap();
    wait_for(&client, "the reaction removed", |s| {
        message_in(s, &root).is_some_and(|m| m.reactions.first().is_some_and(|r| !r.mine))
    })
    .await;
    let sent = recorded(&hub, "message.react");
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1]["remove"], serde_json::json!(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_thread_loads_and_a_reply_goes_to_its_root() {
    let (url, hub) = start_hub().await;
    let root = add_text(&hub, "One step is left.");
    {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        h.messages.push(Message {
            id: MessageId::new(),
            channel_id,
            thread_root_id: Some(root),
            author: Author::User,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            kind: MessageKind::Text,
            body: "Can it run on homelab?".into(),
            attachments: vec![],
            in_timeline: false,
            client_message_id: None,
            reply_count: 0,
            last_reply_at: None,
            reactions: vec![],
            edited_at: None,
            deleted_at: None,
            created_at: Timestamp::now(),
            proactive: None,
            suggestions: Vec::new(),
            source_message_id: None,
        });
    }
    let root = root.to_string();
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    wait_for(&client, "the root", |s| message_in(s, &root).is_some()).await;
    client
        .run(Command::LoadMessageThread {
            message_id: root.clone(),
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the thread", |s| {
        s.convo.thread(&root).is_some_and(|t| t.replies.len() == 1)
    })
    .await;
    let thread = state.convo.thread(&root).unwrap();
    assert_eq!(thread.subtitle, "In Tutti, 1 reply");
    assert_eq!(thread.replies[0].body, "Can it run on homelab?");
    // A thread reply is not in the timeline.
    assert!(message_in(&state, &thread.replies[0].id).is_none());

    let channel_id = state.tutti().unwrap().id.clone();
    client
        .run(Command::ReplyInThread {
            channel_id,
            root_id: root.clone(),
            body: "Do that.".into(),
            client_message_id: "cm-thread-1".into(),
            attachments: vec![],
        })
        .await
        .unwrap();
    wait_for(&client, "the reply in the thread", |s| {
        s.convo
            .thread(&root)
            .is_some_and(|t| t.replies.iter().any(|r| r.body == "Do that."))
    })
    .await;
    let stored = hub
        .lock()
        .unwrap()
        .messages
        .iter()
        .find(|m| m.body == "Do that.")
        .cloned()
        .unwrap();
    assert_eq!(stored.thread_root_id.map(|r| r.to_string()), Some(root));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_sends_the_scope_and_shows_the_hits() {
    let (url, hub) = start_hub().await;
    add_text(&hub, "The Windows engine builds on bigboi.");
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    client
        .run(Command::Search {
            query: "windows".into(),
            scope: SearchScope::Memory,
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the hits", |s| {
        s.convo.search.load == SearchLoad::Done
    })
    .await;
    assert_eq!(state.convo.search.hits.len(), 1);
    let hit = &state.convo.search.hits[0];
    assert_eq!(hit.title, "people/chris.md");
    assert!(hit.detail.starts_with("Changed "), "{}", hit.detail);
    let sent = recorded(&hub, "message.search");
    assert_eq!(sent[0]["kinds"], serde_json::json!(["memory_note"]));

    client
        .run(Command::Search {
            query: "windows".into(),
            scope: SearchScope::All,
        })
        .await
        .unwrap();
    let state = wait_for(&client, "all hits", |s| {
        s.convo.search.load == SearchLoad::Done && s.convo.search.scope == SearchScope::All
    })
    .await;
    assert_eq!(state.convo.search.hits.len(), 2);
    assert_eq!(state.convo.search.hits[0].title, "Tutti");
    assert!(state.convo.search.hits[0].tutti);

    // An empty query asks nothing.
    client
        .run(Command::Search {
            query: "  ".into(),
            scope: SearchScope::All,
        })
        .await
        .unwrap();
    wait_for(&client, "no search", |s| {
        s.convo.search.load == SearchLoad::Idle
    })
    .await;
    assert_eq!(recorded(&hub, "message.search").len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_task_sends_the_dialog_fields() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let channel_id = client.state().tutti().unwrap().id.clone();
    let worker = consort_proto::ids::WorkerId::new().to_string();
    let repo = consort_proto::ids::RepoId::new().to_string();
    let at = format_now() + 86_400_000;
    client
        .run(Command::CreateTask(NewTask {
            channel_id: channel_id.clone(),
            title: "Fix the blank tray icon".into(),
            brief: "Find the cause and fix it.".into(),
            selection: None,
            runs_on: RunsOn::Worker(worker.clone()),
            repo_id: Some(repo.clone()),
            isolation: Some(consort_client::Isolation::Clone),
            approval_mode: Some(consort_client::ApprovalMode::AutoEdits),
            start: StartWhen::At(at),
        }))
        .await
        .unwrap();
    let sent = recorded(&hub, "task.create");
    let body = &sent[0];
    assert_eq!(body["title"], "Fix the blank tray icon");
    assert_eq!(body["placement"]["worker_id"], serde_json::json!(worker));
    assert_eq!(body["placement"]["repos"], serde_json::json!([repo]));
    assert_eq!(body["placement"]["isolation"], "clone");
    assert_eq!(body["approval_mode"], "auto_edits");
    assert_eq!(body["start_at"], serde_json::json!(at));

    // An empty brief and a past time are refused before the hub.
    let empty = client
        .run(Command::CreateTask(NewTask {
            channel_id: channel_id.clone(),
            title: "x".into(),
            ..NewTask::default()
        }))
        .await
        .unwrap_err();
    assert!(empty.to_string().contains("brief"), "{empty}");
    let past = client
        .run(Command::CreateTask(NewTask {
            channel_id,
            title: "x".into(),
            brief: "y".into(),
            start: StartWhen::At(1),
            ..NewTask::default()
        }))
        .await
        .unwrap_err();
    assert!(past.to_string().contains("passed"), "{past}");
    assert_eq!(recorded(&hub, "task.create").len(), 1);

    let task_id = client.state().tasks[0].id.clone();
    client
        .run(Command::DeleteTask {
            task_id: task_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        recorded(&hub, "task.delete")[0]["task_id"],
        serde_json::json!(task_id)
    );
}

fn format_now() -> i64 {
    Timestamp::now().as_millis()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archived_tasks_load_in_pages_and_restore() {
    let (url, hub) = start_hub().await;
    {
        let mut h = hub.lock().unwrap();
        let base = h.task.clone().unwrap();
        for title in ["Renew the certificate", "Old backups"] {
            let mut t = base.clone();
            t.id = TaskId::new();
            t.title = title.into();
            t.status = PS::Resolved;
            t.archived_at = Some(Timestamp::now());
            h.phase4.archived.push(t);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    client
        .run(Command::LoadArchivedTasks { more: false })
        .await
        .unwrap();
    let state = wait_for(&client, "the first page", |s| {
        s.convo.archived.tasks.len() == 1
    })
    .await;
    assert_eq!(state.convo.archived.tasks.len(), 1);
    assert!(state.convo.archived.has_more);
    assert!(
        state.convo.archived.tasks[0]
            .archived_label
            .starts_with("Archived ")
    );
    client
        .run(Command::LoadArchivedTasks { more: true })
        .await
        .unwrap();
    let state = wait_for(&client, "the second page", |s| {
        s.convo.archived.tasks.len() == 2
    })
    .await;
    assert_eq!(state.convo.archived.tasks.len(), 2);
    assert!(!state.convo.archived.has_more);
    assert_eq!(
        recorded(&hub, "task.list_archived")[1]["offset"],
        serde_json::json!(1)
    );

    let first = state.convo.archived.tasks[0].id.clone();
    // The fake hub answers `task.archive` with its own task; the client
    // takes the restored task out of the list either way.
    client
        .run(Command::RestoreTask {
            task_id: first.clone(),
        })
        .await
        .unwrap();
    wait_for(&client, "the restored task out of the list", |s| {
        s.convo.archived.tasks.iter().all(|t| t.id != first)
    })
    .await;
    let restore = &recorded(&hub, "task.archive")[0];
    assert_eq!(restore["archived"], serde_json::json!(false));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_item_opens_with_its_place_header_and_text_and_offline_says_so() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let task_id = client.state().tasks[0].id.clone();
    let session = SessionId::new();
    hub.lock().unwrap().phase4.session = Some(session);
    // The first item of the task: the command.
    let first = items(session);
    let _ = first;
    // The item ID is not known before `task.items`, so open by asking it
    // first, as the activity chip would after the feed.
    client
        .run(Command::OpenTaskItem {
            task_id: task_id.clone(),
            session_id: session.to_string(),
            item_id: consort_proto::ids::ItemId::new().to_string(),
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the loaded item", |s| {
        s.convo
            .item
            .as_ref()
            .is_some_and(|i| matches!(i.state, ItemDetailState::Loaded { .. }))
    })
    .await;
    let item = state.convo.item.as_ref().unwrap();
    // An unknown item has no place, but the text still comes.
    assert_eq!(item.index, 0);
    assert_eq!(item.total, 2);
    match &item.state {
        ItemDetailState::Loaded { text } => assert_eq!(text.as_str(), "line 1\nline 2\nFinished\n"),
        other => panic!("not loaded: {other:?}"),
    }
    let detail = recorded(&hub, "task.item_detail");
    assert_eq!(
        detail[0]["session_id"],
        serde_json::json!(session.to_string())
    );

    hub.lock().unwrap().phase4.item_offline = true;
    client
        .run(Command::OpenTaskItem {
            task_id,
            session_id: session.to_string(),
            item_id: consort_proto::ids::ItemId::new().to_string(),
        })
        .await
        .unwrap();
    wait_for(&client, "the offline item", |s| {
        s.convo
            .item
            .as_ref()
            .is_some_and(|i| i.state == ItemDetailState::Offline)
    })
    .await;
    client.run(Command::CloseTaskItem).await.unwrap();
    wait_for(&client, "no item", |s| s.convo.item.is_none()).await;
    let _ = ItemKind::Diff;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_settings_make_a_repo_and_save_every_field() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let channel_id = client.state().tutti().unwrap().id.clone();
    let known = consort_proto::ids::RepoId::new().to_string();
    client
        .run(Command::UpdateChannel {
            channel_id,
            changes: consort_client::ChannelChanges {
                purpose: Some("Keep Sayso shipping".into()),
                default_isolation: Some(consort_client::Isolation::InPlace),
                default_approval_mode: Some(consort_client::ApprovalMode::Ask),
                allowed_local_mcp_servers: Some(vec!["windows-signing".into()]),
                memory_enabled: Some(false),
                repos: Some(vec![
                    RepoChoice::Existing(known.clone()),
                    RepoChoice::New("git@github.com:watzon/notes.git".into()),
                ]),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let created = recorded(&hub, "repo.create");
    assert_eq!(created[0]["url"], "git@github.com:watzon/notes.git");
    let update = &recorded(&hub, "channel.update")[0];
    assert_eq!(update["purpose"], "Keep Sayso shipping");
    assert_eq!(update["default_isolation"], "in_place");
    assert_eq!(update["default_approval_mode"], "ask");
    assert_eq!(
        update["allowed_local_mcp_servers"],
        serde_json::json!(["windows-signing"])
    );
    assert_eq!(update["memory_enabled"], serde_json::json!(false));
    let repos = update["repo_ids"].as_array().unwrap();
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0], serde_json::json!(known));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn groups_delete_and_reorder() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let (a, b) = (
        consort_proto::ids::GroupId::new().to_string(),
        consort_proto::ids::GroupId::new().to_string(),
    );
    client
        .run(Command::ReorderGroups {
            group_ids: vec![b.clone(), a.clone()],
        })
        .await
        .unwrap();
    client
        .run(Command::DeleteGroup {
            group_id: a.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        recorded(&hub, "group.reorder")[0]["group_ids"],
        serde_json::json!([b, a.clone()])
    );
    assert_eq!(
        recorded(&hub, "group.delete")[0]["group_id"],
        serde_json::json!(a)
    );
}
