//! What each [`Command`] asks of the hub.

use std::sync::Arc;

use consort_proto::{
    client_api::{
        ChannelArchive, ChannelCreate, ChannelDelete, ChannelMove, ChannelUpdate,
        DecisionAnswerRequest, GroupUpdate, MessageDelete, MessageEdit, MessageList, ReadMark,
        TaskInterrupt, TaskReopen, TaskResolve, TaskRetry, WorkerPairingCode, WorkerRename,
        WorkerRevoke, WorkerSetLabels, WorkerSetPaused,
    },
    ids::ChannelId,
    model::{DecisionAnswer, ModelSelection, RequestOptionKind},
};
use tokio::sync::{oneshot, watch};

use super::{Shared, invalid, link::ThreadPager, outbox::queue_send, pair};
use crate::{
    Backend, ClientError, Command,
    cache::{Cache, PendingSend},
    format,
    view::{ApprovalChoice, Connection, PairingCodeView, ViewState},
};

/// The hub backend: every command runs as its own task, so one command that
/// waits for the hub does not hold up the others.
pub(crate) struct HubBackend {
    pub(crate) shared: Arc<Shared>,
}

impl Backend for HubBackend {
    fn handle(
        &mut self,
        command: Command,
        reply: oneshot::Sender<Result<(), ClientError>>,
        _publish: &watch::Sender<Arc<ViewState>>,
    ) {
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let _ = reply.send(run(shared, command).await);
        });
    }
}

fn parse<T: std::str::FromStr>(id: &str, what: &str) -> Result<T, ClientError> {
    id.parse().map_err(|_| {
        ClientError::Other(format!(
            "Consort cannot use this {what}, because its ID is not valid. Reload the app, then try again"
        ))
    })
}

fn selection(label: &Option<String>) -> Option<ModelSelection> {
    label.as_ref().map(|l| {
        let (account, model) = l.split_once(", ").unwrap_or((l.as_str(), ""));
        ModelSelection {
            account_label: account.to_owned(),
            model: model.to_owned(),
            options: Default::default(),
        }
    })
}

async fn run(shared: Arc<Shared>, command: Command) -> Result<(), ClientError> {
    match command {
        Command::Pair { hub_url, code } => {
            let identity = pair(&hub_url, &code, &shared.options).await?;
            if let Some(store) = &shared.store {
                store.save(&identity)?;
            }
            if let Ok(mut i) = shared.identity.lock() {
                *i = Some(identity);
            }
            if let Ok(mut cache) = shared.cache.lock() {
                // Messages for the old hub do not go to the new one. The
                // user keeps their text as failed sends, and can send them
                // again.
                cache.fail_pending(
                    "This message did not go out, because you paired this app with a hub again.",
                );
                let failures = std::mem::take(&mut cache.send_failures);
                *cache = Cache {
                    send_failures: failures,
                    ..Cache::default()
                };
            }
            shared.defer_waiters();
            shared.set_connection(Connection::Connecting);
            shared.wake.notify_one();
        }
        Command::SendMessage {
            channel_id,
            body,
            client_message_id,
        } => {
            let pending = PendingSend {
                client_message_id,
                channel_id,
                thread_root_id: None,
                body,
                created_at: format::now_ms(),
            };
            return queue_send(&shared, pending).await;
        }
        Command::ReplyToTask {
            task_id,
            body,
            client_message_id,
        } => {
            let (channel_id, root) = shared
                .cache
                .lock()
                .ok()
                .and_then(|c| {
                    c.tasks
                        .get(&task_id)
                        .map(|t| (t.channel_id.to_string(), t.card_message_id.to_string()))
                })
                .ok_or_else(|| invalid("That task no longer exists."))?;
            let pending = PendingSend {
                client_message_id,
                channel_id,
                thread_root_id: Some(root),
                body,
                created_at: format::now_ms(),
            };
            return queue_send(&shared, pending).await;
        }
        Command::EditMessage { message_id, body } => {
            // Refuse a body that the hub would refuse. The caller keeps its
            // draft.
            crate::limits::check_message_body(&body)?;
            shared
                .call(MessageEdit {
                    message_id: parse(&message_id, "message")?,
                    body,
                })
                .await?;
        }
        Command::DeleteMessage { message_id } => {
            shared
                .call(MessageDelete {
                    message_id: parse(&message_id, "message")?,
                })
                .await?;
        }
        Command::CreateChannel(new) => {
            let created = shared
                .call(ChannelCreate {
                    name: new.name.trim().to_owned(),
                    purpose: new.purpose,
                    instructions: String::new(),
                    icon: None,
                    color: None,
                    group_id: match &new.group_id {
                        Some(g) => Some(parse(g, "group")?),
                        None => None,
                    },
                    coordinator_selection: None,
                    task_selection: selection(&new.task_selection),
                    repo_ids: Vec::new(),
                    default_isolation: Some(match new.isolation {
                        crate::view::Isolation::InPlace => consort_proto::model::Isolation::InPlace,
                        crate::view::Isolation::Worktree => {
                            consort_proto::model::Isolation::Worktree
                        }
                        crate::view::Isolation::Clone => consort_proto::model::Isolation::Clone,
                    }),
                    default_approval_mode: None,
                    memory_enabled: None,
                })
                .await?;
            if !new.preferred_workers.is_empty() {
                let workers = new
                    .preferred_workers
                    .iter()
                    .map(|w| parse(w, "worker"))
                    .collect::<Result<Vec<consort_proto::ids::WorkerId>, _>>()?;
                let update: ChannelUpdate = serde_json::from_value(serde_json::json!({
                    "channel_id": created.channel.id,
                    "preferred_workers": workers,
                }))
                .map_err(|e| ClientError::Other(e.to_string()))?;
                shared.call(update).await?;
            }
        }
        Command::UpdateChannel {
            channel_id,
            changes,
        } => {
            let id: consort_proto::ids::ChannelId = parse(&channel_id, "channel")?;
            let mut patch = serde_json::json!({ "channel_id": id });
            if let Some(name) = changes.name {
                patch["name"] = name.into();
            }
            if let Some(purpose) = changes.purpose {
                patch["purpose"] = purpose.into();
            }
            if let Some(instructions) = changes.instructions {
                patch["instructions"] = instructions.into();
            }
            if let Some(icon) = changes.icon {
                patch["icon"] = icon.into();
            }
            if patch.as_object().is_some_and(|o| o.len() > 1) {
                let update: ChannelUpdate =
                    serde_json::from_value(patch).map_err(|e| ClientError::Other(e.to_string()))?;
                shared.call(update).await?;
            }
            if let Some(group) = changes.group_id {
                shared
                    .call(ChannelMove {
                        channel_id: id,
                        group_id: Some(parse(&group, "group")?),
                        position: 0,
                    })
                    .await?;
            }
        }
        Command::ArchiveChannel { channel_id } => {
            shared
                .call(ChannelArchive {
                    channel_id: parse(&channel_id, "channel")?,
                    archived: true,
                })
                .await?;
        }
        Command::DeleteChannel { channel_id } => {
            shared
                .call(ChannelDelete {
                    channel_id: parse(&channel_id, "channel")?,
                })
                .await?;
        }
        Command::SetGroupCollapsed {
            group_id,
            collapsed,
        } => {
            shared
                .call(GroupUpdate {
                    group_id: parse(&group_id, "group")?,
                    name: None,
                    collapsed: Some(collapsed),
                })
                .await?;
        }
        Command::MarkRead { channel_id } => {
            let channel: ChannelId = parse(&channel_id, "channel")?;
            let (loaded, last, marked) = shared
                .cache
                .lock()
                .ok()
                .map(|c| {
                    let last = c
                        .messages
                        .values()
                        .filter(|m| {
                            m.channel_id == channel
                                && m.shows_in_timeline()
                                && m.deleted_at.is_none()
                        })
                        .max_by_key(|m| (m.created_at, m.id.to_string()))
                        .map(|m| m.id);
                    let marked = c
                        .read_markers
                        .get(&channel_id)
                        .map(|m| m.last_read_message_id);
                    (c.channels_loaded.contains(&channel_id), last, marked)
                })
                .unwrap_or_default();
            // A channel that never loaded shows no messages. The newest one
            // that a live entry brings is not the newest the user has seen,
            // so nothing is marked until the channel has loaded.
            if !loaded {
                return Err(ClientError::Other(
                    "This channel has not loaded yet, so Consort cannot mark it as read. Consort tries to load it again".into(),
                ));
            }
            // Nothing to do when the newest message is read already.
            if let Some(last) = last
                && marked != Some(last)
            {
                // A failure is the caller's to see, so it can try again. The
                // marker stays where it was until the hub takes the new one.
                shared
                    .call(ReadMark {
                        channel_id: channel,
                        thread_root_id: None,
                        last_read_message_id: last,
                    })
                    .await?;
                // Show it at once. The feed repeats it later.
                if let Ok(mut cache) = shared.cache.lock() {
                    cache.read_markers.insert(
                        channel_id.clone(),
                        consort_proto::model::ReadMarker {
                            channel_id: channel,
                            thread_root_id: None,
                            last_read_message_id: last,
                        },
                    );
                }
                shared.publish();
            }
        }
        Command::LoadThread { task_id } => return load_thread(&shared, &task_id).await,
        Command::LoadOlder { channel_id } => {
            let (more, oldest) = shared
                .cache
                .lock()
                .map(|c| {
                    (
                        c.has_older.get(&channel_id).copied().unwrap_or(false),
                        c.oldest_timeline_message(&channel_id),
                    )
                })
                .unwrap_or((false, None));
            if more && let Some(oldest) = oldest {
                let page = shared
                    .call(MessageList {
                        channel_id: parse(&channel_id, "channel")?,
                        thread_root_id: None,
                        before: Some(oldest),
                        limit: Some(HISTORY_PAGE),
                    })
                    .await?;
                if let Ok(mut cache) = shared.cache.lock() {
                    cache.has_older.insert(channel_id.clone(), page.has_more);
                    cache.merge_messages(page.messages);
                }
                shared.publish();
                // An older task card may name a task that no snapshot held.
                shared.loader_wake.notify_one();
            }
        }
        Command::ResolveTask { task_id } => {
            shared
                .call(TaskResolve {
                    task_id: parse(&task_id, "task")?,
                })
                .await?;
        }
        Command::ReopenTask { task_id } => {
            shared
                .call(TaskReopen {
                    task_id: parse(&task_id, "task")?,
                })
                .await?;
        }
        Command::RetryTask { task_id } => {
            shared
                .call(TaskRetry {
                    task_id: parse(&task_id, "task")?,
                })
                .await?;
        }
        Command::StopTask { task_id } => {
            shared
                .call(TaskInterrupt {
                    task_id: parse(&task_id, "task")?,
                })
                .await?;
        }
        Command::AnswerApproval {
            approval_id,
            choice,
        } => {
            // An approval is answered by its ID alone, so one from the
            // channel coordinator (with no task) works the same way.
            let option_id = shared
                .cache
                .lock()
                .ok()
                .and_then(|c| c.approvals.get(&approval_id).cloned())
                .and_then(|a| option_for(&a, choice))
                .ok_or_else(|| invalid("This approval has no such answer. It may have closed."))?;
            let answered = shared
                .call(consort_proto::client_api::ApprovalAnswer {
                    approval_id: parse(&approval_id, "approval")?,
                    option_id,
                })
                .await;
            if let Err(err) = answered {
                if is_closed_reply(&err) {
                    close_approval(&shared, &approval_id);
                }
                return Err(err);
            }
        }
        Command::AnswerDecision {
            decision_id,
            option,
            text,
        } => {
            let answered = shared
                .call(DecisionAnswerRequest {
                    decision_id: parse(&decision_id, "decision")?,
                    answer: DecisionAnswer {
                        option_index: option.and_then(|o| u8::try_from(o).ok()),
                        text,
                    },
                })
                .await;
            if let Err(err) = answered {
                if is_closed_reply(&err) {
                    close_decision(&shared, &decision_id);
                }
                return Err(err);
            }
        }
        Command::DismissSendFailure { id } => {
            if let Ok(mut cache) = shared.cache.lock() {
                cache.send_failures.retain(|f| f.id != id);
            }
            shared.publish();
        }
        Command::WorkerPairingCode => {
            let result = shared.call(WorkerPairingCode {}).await?;
            let hub_url = shared
                .identity()
                .map(|i| i.config.hub_url)
                .unwrap_or_default();
            if let Ok(mut p) = shared.pairing.lock() {
                *p = Some(PairingCodeView {
                    code: result.code.to_string(),
                    hub_url,
                    expires_at: result.expires_at.as_millis(),
                });
            }
            shared.publish();
        }
        Command::CancelWorkerPairing => {
            if let Ok(mut p) = shared.pairing.lock() {
                *p = None;
            }
            shared.publish();
        }
        Command::RenameWorker { worker_id, name } => {
            shared
                .call(WorkerRename {
                    worker_id: parse(&worker_id, "worker")?,
                    name,
                })
                .await?;
        }
        Command::SetWorkerLabels { worker_id, labels } => {
            shared
                .call(WorkerSetLabels {
                    worker_id: parse(&worker_id, "worker")?,
                    labels,
                })
                .await?;
        }
        Command::SetWorkerPaused { worker_id, paused } => {
            shared
                .call(WorkerSetPaused {
                    worker_id: parse(&worker_id, "worker")?,
                    paused,
                })
                .await?;
        }
        Command::RemoveWorker { worker_id } => {
            shared
                .call(WorkerRevoke {
                    worker_id: parse(&worker_id, "worker")?,
                })
                .await?;
        }
    }
    Ok(())
}

/// Opens the thread of a task: the full task, then the replies. The thread
/// needs the full brief and review summary, so a task that the cache holds
/// as a preview, or does not hold while tasks are left out of the snapshot,
/// comes from the hub with `task.get`.
async fn load_thread(shared: &Shared, task_id: &str) -> Result<(), ClientError> {
    let known = shared
        .cache
        .lock()
        .is_ok_and(|c| c.tasks.contains_key(task_id));
    if known {
        // The replies need only the card ID, so both loads run together.
        let (task, replies) = tokio::join!(
            shared.ensure_full_task(task_id),
            load_replies(shared, task_id)
        );
        return task.and(replies);
    }
    // The card of the thread is known only after the task is.
    shared.ensure_full_task(task_id).await?;
    load_replies(shared, task_id).await
}

/// Loads the replies of a task's thread once.
async fn load_replies(shared: &Shared, task_id: &str) -> Result<(), ClientError> {
    // Claim the thread first, so two calls do not load it twice.
    let target = shared.cache.lock().ok().and_then(|mut c| {
        let (channel_id, root) = c
            .tasks
            .get(task_id)
            .map(|t| (t.channel_id, t.card_message_id))?;
        c.loaded_threads
            .insert(root.to_string())
            .then_some((channel_id, root))
    });
    let Some((channel_id, root)) = target else {
        return Ok(());
    };
    let mut pager = ThreadPager::new(channel_id, root);
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
                // Let the next open try again.
                if let Ok(mut cache) = shared.cache.lock() {
                    cache.loaded_threads.remove(&root.to_string());
                }
                return Err(err);
            }
        }
    }
    Ok(())
}

/// True for the answers that say a request is closed or gone: `conflict`
/// (answered or closed already) and `not_found`.
fn is_closed_reply(err: &ClientError) -> bool {
    matches!(err, ClientError::Hub { code, .. } if code == "conflict" || code == "not_found")
}

/// The hub says an approval is closed. A snapshot holds only pending
/// approvals, so the cache cannot learn more than this from a reload. If the
/// cache still shows the approval as pending, it drops it: the card then
/// shows as closed, and the answer comes from the feed if it arrives. An
/// approval that the feed already updated stays as it is.
fn close_approval(shared: &Shared, approval_id: &str) {
    if let Ok(mut cache) = shared.cache.lock()
        && cache
            .approvals
            .get(approval_id)
            .is_some_and(|a| a.status == consort_proto::model::ApprovalStatus::Pending)
    {
        cache.approvals.remove(approval_id);
    }
    shared.publish();
}

/// The same for a decision. See [`close_approval`].
fn close_decision(shared: &Shared, decision_id: &str) {
    if let Ok(mut cache) = shared.cache.lock()
        && cache
            .decisions
            .get(decision_id)
            .is_some_and(|d| d.status == consort_proto::model::DecisionStatus::Pending)
    {
        cache.decisions.remove(decision_id);
    }
    shared.publish();
}

/// The size of one page of older timeline messages.
const HISTORY_PAGE: u32 = 50;

/// The harness option that a button stands for. Each kind in the list is
/// tried in order, so Deny picks `deny_once` before `deny_always`, whatever
/// order the harness lists them in.
pub(crate) fn option_for(
    approval: &consort_proto::model::Approval,
    choice: ApprovalChoice,
) -> Option<String> {
    let wanted: &[RequestOptionKind] = match choice {
        ApprovalChoice::AllowOnce => &[RequestOptionKind::AllowOnce],
        ApprovalChoice::AllowForTask => &[RequestOptionKind::AllowAlways],
        ApprovalChoice::Deny => &[RequestOptionKind::DenyOnce, RequestOptionKind::DenyAlways],
    };
    wanted.iter().find_map(|kind| {
        approval
            .options
            .iter()
            .find(|o| o.kind == *kind)
            .map(|o| o.id.clone())
    })
}
