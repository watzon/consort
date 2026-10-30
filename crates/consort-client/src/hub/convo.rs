//! The phase 4 commands: reactions, message threads, search, New task,
//! task delete and restore, the archived tasks, the task item view, and
//! group delete and reorder. See [`crate::convo`].

use std::sync::Arc;

use consort_proto::{
    client_api::{
        GroupDelete, GroupReorder, MessageList, MessageReact, MessageSearch, RepoCreate,
        TaskArchive, TaskCreate, TaskDelete, TaskItemDetail, TaskItems, TaskListArchived,
    },
    ids::{ChannelId, GroupId, MessageId, TaskId},
    model::{Author, ItemPayload, PlacementRequest, Reaction, SessionItem},
    time::Timestamp,
};

use super::{
    Shared,
    commands::{parse_id, proto_approval_mode, proto_isolation, selection},
    invalid,
    link::ThreadPager,
    outbox::queue_send,
};
use crate::{
    ClientError, Command, CommandOutput,
    cache::PendingSend,
    connectors::LoadState,
    convo::{
        ItemDetailState, ItemDetailView, ItemKind, NewTask, RepoChoice, RunsOn, StartWhen,
        duration_label,
    },
    format,
};

/// The most hub results that the search palette shows.
const SEARCH_LIMIT: u32 = 30;

/// The most pages of older messages that the client loads to find a
/// message that a search result names.
const FIND_PAGES: usize = 20;

/// The most items that the item view reads to count them.
const ITEMS_MAX: usize = 5_000;

/// Turns new repo URLs into repos (`repo.create`). The hub gives back the
/// repo it has when the URL is known already.
pub(super) async fn resolve_repos(
    shared: &Arc<Shared>,
    repos: Vec<RepoChoice>,
) -> Result<Vec<RepoChoice>, ClientError> {
    let mut out = Vec::with_capacity(repos.len());
    for repo in repos {
        match repo {
            RepoChoice::New(url) => {
                let created = shared
                    .call(RepoCreate {
                        url,
                        name: None,
                        default_branch: None,
                    })
                    .await?;
                let id = created.repo.id.to_string();
                if let Ok(mut cache) = shared.cache.lock() {
                    cache.repos.insert(id.clone(), created.repo);
                }
                out.push(RepoChoice::Existing(id));
            }
            existing => out.push(existing),
        }
    }
    Ok(out)
}

/// Changes the user's reaction in the cache, so the chip shows at once.
/// Returns whether anything changed.
fn react_locally(shared: &Shared, message_id: &str, emoji: &str, remove: bool) -> bool {
    let changed = shared.cache.lock().is_ok_and(|mut cache| {
        let Some(message) = cache.messages.get_mut(message_id) else {
            return false;
        };
        let at = message
            .reactions
            .iter()
            .position(|r| r.author == Author::User && r.emoji == emoji);
        match (at, remove) {
            (Some(i), true) => {
                message.reactions.remove(i);
                true
            }
            (None, false) => {
                message.reactions.push(Reaction {
                    author: Author::User,
                    emoji: emoji.to_owned(),
                    created_at: Timestamp::now(),
                });
                true
            }
            _ => false,
        }
    });
    if changed {
        shared.publish();
    }
    changed
}

/// Loads the replies of a message's thread once. The root must be in the
/// cache; a root that is older than the loaded page is found first.
async fn load_message_thread(shared: &Arc<Shared>, message_id: &str) -> Result<(), ClientError> {
    let root: MessageId = parse_id(message_id, "message")?;
    let channel = shared
        .cache
        .lock()
        .ok()
        .and_then(|c| c.messages.get(message_id).map(|m| m.channel_id))
        .ok_or_else(|| invalid("That message is not loaded. Open its channel first."))?;
    let claimed = shared.cache.lock().is_ok_and(|mut c| {
        c.convo.threads.insert(message_id.to_owned());
        c.loaded_threads.insert(message_id.to_owned())
    });
    shared.publish();
    if !claimed {
        return Ok(());
    }
    let mut pager = ThreadPager::new(channel, root);
    while let Some(request) = pager.next_request() {
        match shared.call(request).await {
            Ok(page) => {
                let messages = pager.accept(page);
                if let Ok(mut cache) = shared.cache.lock() {
                    cache.merge_messages(messages);
                }
                shared.publish();
            }
            Err(err) => {
                if let Ok(mut cache) = shared.cache.lock() {
                    cache.loaded_threads.remove(message_id);
                }
                return Err(err);
            }
        }
    }
    Ok(())
}

/// Loads older pages of a channel until the cache holds `message_id`, so a
/// search result can scroll to it. Stops after [`FIND_PAGES`] pages.
async fn find_message(
    shared: &Arc<Shared>,
    channel_id: &str,
    message_id: &str,
) -> Result<(), ClientError> {
    let channel: ChannelId = parse_id(channel_id, "channel")?;
    for _ in 0..FIND_PAGES {
        let (found, more, oldest) = shared
            .cache
            .lock()
            .map(|c| {
                (
                    c.messages.contains_key(message_id),
                    c.has_older.get(channel_id).copied().unwrap_or(false),
                    c.oldest_timeline_message(channel_id),
                )
            })
            .unwrap_or((true, false, None));
        if found || !more {
            return Ok(());
        }
        let Some(oldest) = oldest else {
            return Ok(());
        };
        let page = shared
            .call(MessageList {
                channel_id: channel,
                thread_root_id: None,
                before: Some(oldest),
                limit: Some(50),
            })
            .await?;
        if let Ok(mut cache) = shared.cache.lock() {
            cache.has_older.insert(channel_id.to_owned(), page.has_more);
            cache.merge_messages(page.messages);
        }
        shared.publish();
    }
    Ok(())
}

/// Runs the search and puts the answer in the cache, unless a newer search
/// started meanwhile.
async fn search(
    shared: &Arc<Shared>,
    query: String,
    scope: crate::convo::SearchScope,
) -> Result<(), ClientError> {
    let words = crate::convo::query_words(&query);
    let serial = shared
        .cache
        .lock()
        .map(|mut c| {
            let s = &mut c.convo.search;
            s.serial += 1;
            s.query = query.clone();
            s.scope = scope;
            s.hits.clear();
            s.load = if words.is_empty() {
                crate::convo::SearchLoad::Idle
            } else {
                crate::convo::SearchLoad::Loading
            };
            s.serial
        })
        .unwrap_or_default();
    shared.publish();
    if words.is_empty() {
        return Ok(());
    }
    let answer = shared
        .call(MessageSearch {
            query,
            channel_id: None,
            limit: Some(SEARCH_LIMIT),
            kinds: scope.kinds(),
        })
        .await;
    if let Ok(mut cache) = shared.cache.lock() {
        let s = &mut cache.convo.search;
        if s.serial != serial {
            return Ok(());
        }
        match &answer {
            Ok(result) => {
                s.hits = result.hits.clone();
                s.load = crate::convo::SearchLoad::Done;
            }
            Err(err) => {
                s.load =
                    crate::convo::SearchLoad::Failed(format!("The hub search did not work. {err}"))
            }
        }
    }
    shared.publish();
    answer.map(|_| ())
}

/// The `task.create` of the New task dialog.
fn task_create(new: NewTask) -> Result<TaskCreate, ClientError> {
    let title = new.title.trim().to_owned();
    if title.is_empty() {
        return Err(invalid("The task needs a title."));
    }
    if new.brief.trim().is_empty() {
        return Err(invalid(
            "The task needs a brief. Say what to do and what done means.",
        ));
    }
    let start_at = match new.start {
        StartWhen::Now => None,
        StartWhen::At(at) => {
            if at <= format::now_ms() {
                return Err(invalid(
                    "That start time has passed. Choose a later time, or choose Now.",
                ));
            }
            Some(Timestamp::from_millis(at))
        }
    };
    Ok(TaskCreate {
        channel_id: parse_id(&new.channel_id, "channel")?,
        title,
        brief: new.brief,
        model_selection: selection(&new.selection),
        placement: PlacementRequest {
            worker_id: match &new.runs_on {
                RunsOn::AnyTagged => None,
                RunsOn::Worker(id) => Some(parse_id(id, "worker")?),
            },
            require: None,
            repos: match &new.repo_id {
                Some(id) => Some(vec![parse_id(id, "repo")?]),
                None => None,
            },
            isolation: new.isolation.map(proto_isolation),
        },
        approval_mode: new.approval_mode.map(proto_approval_mode),
        start_at,
    })
}

/// The commands and file changes of a task, in order: the items that the
/// activity chip lists.
fn listed(item: &SessionItem) -> bool {
    matches!(
        item.payload,
        ItemPayload::Command(_) | ItemPayload::FileChange(_)
    )
}

/// Reads every item of a task, oldest first, up to [`ITEMS_MAX`].
async fn task_items(
    shared: &Arc<Shared>,
    task_id: TaskId,
) -> Result<Vec<SessionItem>, ClientError> {
    let mut out = Vec::new();
    let mut after = None;
    loop {
        let page = shared
            .call(TaskItems {
                task_id,
                session_id: None,
                after_item_id: after,
                limit: Some(200),
            })
            .await?;
        after = page.items.last().map(|i| i.id);
        out.extend(page.items);
        if !page.has_more || after.is_none() || out.len() >= ITEMS_MAX {
            return Ok(out);
        }
    }
}

/// Changes the open item, when it is still the item `item_id`.
fn update_item(shared: &Shared, item_id: &str, change: impl FnOnce(&mut ItemDetailView)) {
    if let Ok(mut cache) = shared.cache.lock()
        && let Some(item) = cache.convo.item.as_mut()
        && item.item_id == item_id
    {
        change(item);
    }
    shared.publish();
}

/// The header of an item: its kind, title and line under the title.
fn describe(item: &SessionItem, worker: &str, view: &mut ItemDetailView) {
    let time = format::time_label(item.updated_at.as_millis());
    match &item.payload {
        ItemPayload::Command(c) => {
            view.kind = ItemKind::Output;
            view.title = c.command.clone();
            let ran = match c.duration_ms {
                Some(ms) => format!("ran {} on {worker}, {time}", duration_label(ms)),
                None => format!("ran on {worker}, {time}"),
            };
            view.subtitle = match c.exit_code {
                Some(code) => format!("Exit code {code}, {ran}"),
                None => {
                    let mut chars = ran.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => ran,
                    }
                }
            };
        }
        ItemPayload::FileChange(f) => {
            view.kind = ItemKind::Diff;
            view.title = match f.changes.as_slice() {
                [one] => one.path.clone(),
                many => format!("{} files", many.len()),
            };
            view.lines_added = f.changes.iter().map(|c| c.lines_added).sum();
            view.lines_removed = f.changes.iter().map(|c| c.lines_removed).sum();
            let lines = if view.lines_added == 1 {
                "line"
            } else {
                "lines"
            };
            view.subtitle = format!(
                "{} {lines} added, {} removed, on {worker} at {time}",
                view.lines_added, view.lines_removed
            );
        }
        _ => {}
    }
}

/// Opens an item: its header and place from `task.items`, then the text
/// from `task.item_detail` and the blob.
async fn open_item(
    shared: &Arc<Shared>,
    task_id: String,
    session_id: String,
    item_id: String,
) -> Result<(), ClientError> {
    let task: TaskId = parse_id(&task_id, "task")?;
    let (worker, worker_online, summary) = shared
        .cache
        .lock()
        .map(|c| {
            let worker = c
                .tasks
                .get(&task_id)
                .and_then(|t| t.worker_id)
                .and_then(|w| c.workers.get(&w.to_string()));
            let summary = c
                .session_items
                .get(&session_id)
                .and_then(|items| items.get(&item_id))
                .map(|s| match s {
                    crate::cache::ItemSummary::Command(command) => {
                        (ItemKind::Output, command.clone())
                    }
                    crate::cache::ItemSummary::Files(paths) => {
                        (ItemKind::Diff, paths.first().cloned().unwrap_or_default())
                    }
                });
            (
                worker
                    .map(|w| w.name.clone())
                    .unwrap_or_else(|| "the worker".to_owned()),
                worker.is_some_and(|w| w.state == consort_proto::model::WorkerState::Online),
                summary,
            )
        })
        .unwrap_or_else(|_| ("the worker".to_owned(), false, None));
    let (kind, title) = summary.unwrap_or_default();
    if let Ok(mut cache) = shared.cache.lock() {
        cache.convo.item = Some(ItemDetailView {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            item_id: item_id.clone(),
            kind,
            title,
            worker_name: worker.clone(),
            state: if worker_online {
                ItemDetailState::Loading
            } else {
                ItemDetailState::Offline
            },
            ..ItemDetailView::default()
        });
    }
    shared.publish();

    // The header and the place. A failure here leaves the summary title.
    if let Ok(items) = task_items(shared, task).await {
        let listed: Vec<&SessionItem> = items.iter().filter(|i| listed(i)).collect();
        let index = listed
            .iter()
            .position(|i| i.id.to_string() == item_id)
            .map(|i| i + 1)
            .unwrap_or(0);
        let total = listed.len();
        let found = listed.iter().find(|i| i.id.to_string() == item_id).copied();
        update_item(shared, &item_id, |view| {
            view.index = index;
            view.total = total;
            if let Some(item) = found {
                describe(item, &worker, view);
            }
        });
    }

    let detail = shared
        .call(TaskItemDetail {
            session_id: parse_id(&session_id, "session")?,
            item_id: parse_id(&item_id, "item")?,
        })
        .await;
    let result = match detail {
        Ok(result) => result,
        Err(ClientError::Hub { code, .. }) if code == "worker_offline" => {
            update_item(shared, &item_id, |v| v.state = ItemDetailState::Offline);
            return Ok(());
        }
        Err(err) => {
            let message = match &err {
                ClientError::Hub { code, message } if code == "not_found" => message.clone(),
                other => format!("Consort could not get the full text. {other}"),
            };
            update_item(shared, &item_id, |v| {
                v.state = ItemDetailState::Failed(message)
            });
            return Ok(());
        }
    };
    let path = match shared
        .fetch_blob(&result.sha256.to_string(), result.size)
        .await
    {
        Ok(path) => path,
        Err(err) => {
            update_item(shared, &item_id, |v| {
                v.state = ItemDetailState::Failed(format!(
                    "Consort could not download the full text. {err}"
                ))
            });
            return Ok(());
        }
    };
    let bytes = std::fs::read(&path).map_err(|e| ClientError::Storage {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let is_diff = result.mime == "text/x-diff";
    update_item(shared, &item_id, |v| {
        if is_diff {
            v.kind = ItemKind::Diff;
        }
        v.state = ItemDetailState::Loaded {
            text: Arc::new(text),
        };
    });
    Ok(())
}

/// Runs one phase 4 command.
pub(super) async fn run(
    shared: &Arc<Shared>,
    command: Command,
) -> Result<CommandOutput, ClientError> {
    match command {
        Command::React {
            message_id,
            emoji,
            remove,
        } => {
            let changed = react_locally(shared, &message_id, &emoji, remove);
            let sent = shared
                .call(MessageReact {
                    message_id: parse_id(&message_id, "message")?,
                    emoji: emoji.clone(),
                    remove,
                })
                .await;
            if let Err(err) = sent {
                if changed {
                    react_locally(shared, &message_id, &emoji, !remove);
                }
                return Err(err);
            }
        }
        Command::LoadMessageThread { message_id } => {
            load_message_thread(shared, &message_id).await?;
        }
        Command::FindMessage {
            channel_id,
            message_id,
        } => find_message(shared, &channel_id, &message_id).await?,
        Command::ReplyInThread {
            channel_id,
            root_id,
            body,
            client_message_id,
            attachments,
        } => {
            crate::command::check_channel_id(&channel_id)?;
            let target = crate::uploads::UploadTarget::Thread {
                root_id: root_id.clone(),
            };
            let blobs = shared.wait_for_uploads(&target, &attachments).await?;
            let pending = PendingSend {
                client_message_id,
                channel_id,
                thread_root_id: Some(root_id),
                body,
                created_at: format::now_ms(),
                attachments: blobs,
            };
            queue_send(shared, pending).await?;
            shared.take_uploads(&attachments);
        }
        Command::Search { query, scope } => search(shared, query, scope).await?,
        Command::ClearSearch => {
            if let Ok(mut cache) = shared.cache.lock() {
                let serial = cache.convo.search.serial + 1;
                cache.convo.search = crate::convo::SearchCache {
                    serial,
                    ..Default::default()
                };
            }
            shared.publish();
        }
        Command::CreateTask(new) => {
            shared.call(task_create(new)?).await?;
        }
        Command::DeleteTask { task_id } => {
            shared
                .call(TaskDelete {
                    task_id: parse_id(&task_id, "task")?,
                })
                .await?;
        }
        Command::RestoreTask { task_id } => {
            let restored = shared
                .call(TaskArchive {
                    task_id: parse_id(&task_id, "task")?,
                    archived: false,
                })
                .await?;
            if let Ok(mut cache) = shared.cache.lock() {
                cache
                    .convo
                    .archived
                    .tasks
                    .retain(|t| t.id.to_string() != task_id);
                cache.tasks.insert(task_id, restored.task);
            }
            shared.publish();
        }
        Command::LoadArchivedTasks { more } => {
            let offset = shared
                .cache
                .lock()
                .map(|mut c| {
                    c.convo.archived.load = LoadState::Loading;
                    if more {
                        c.convo.archived.tasks.len()
                    } else {
                        0
                    }
                })
                .unwrap_or(0);
            shared.publish();
            let page = shared
                .call(TaskListArchived {
                    channel_id: None,
                    offset: Some(u32::try_from(offset).unwrap_or(u32::MAX)),
                    limit: Some(50),
                })
                .await;
            if let Ok(mut cache) = shared.cache.lock() {
                let archived = &mut cache.convo.archived;
                match &page {
                    Ok(page) => {
                        if !more {
                            archived.tasks.clear();
                        }
                        archived.tasks.extend(page.tasks.iter().cloned());
                        archived.has_more = page.has_more;
                        archived.load = LoadState::Loaded;
                    }
                    Err(err) => {
                        archived.load = LoadState::Failed(format!(
                            "Consort could not load the archived tasks. {err}"
                        ))
                    }
                }
            }
            shared.publish();
            page?;
        }
        Command::OpenTaskItem {
            task_id,
            session_id,
            item_id,
        } => open_item(shared, task_id, session_id, item_id).await?,
        Command::CloseTaskItem => {
            if let Ok(mut cache) = shared.cache.lock() {
                cache.convo.item = None;
            }
            shared.publish();
        }
        Command::DeleteGroup { group_id } => {
            shared
                .call(GroupDelete {
                    group_id: parse_id(&group_id, "group")?,
                })
                .await?;
        }
        Command::RenameGroup { group_id, name } => {
            let name = name.trim().to_owned();
            if name.is_empty() {
                return Err(invalid("Enter a name for the group."));
            }
            shared
                .call(consort_proto::client_api::GroupUpdate {
                    group_id: parse_id(&group_id, "group")?,
                    name: Some(name),
                    collapsed: None,
                })
                .await?;
        }
        Command::ReorderGroups { group_ids } => {
            let ids = group_ids
                .iter()
                .map(|g| parse_id::<GroupId>(g, "group"))
                .collect::<Result<Vec<_>, _>>()?;
            // The new order shows at once. The feed brings the same.
            if let Ok(mut cache) = shared.cache.lock() {
                for (position, id) in group_ids.iter().enumerate() {
                    if let Some(group) = cache.groups.get_mut(id) {
                        group.position = i64::try_from(position).unwrap_or(i64::MAX);
                    }
                }
            }
            shared.publish();
            shared.call(GroupReorder { group_ids: ids }).await?;
        }
        _ => {
            return Err(ClientError::Other(
                "Consort sent this command to the wrong place. Restart Consort, then try again"
                    .into(),
            ));
        }
    }
    Ok(CommandOutput::Done)
}
