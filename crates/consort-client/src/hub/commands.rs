//! What each [`Command`] asks of the hub.

use std::sync::Arc;

use consort_proto::{
    client_api::{
        AccountCreate, AccountRemove, AccountSignOut, AccountUpdate, AcpRegistryList,
        ChannelArchive, ChannelCreate, ChannelDelete, ChannelMove, ChannelUpdate,
        DecisionAnswerRequest, GroupCreate, GroupUpdate, MessageDelete, MessageEdit, MessageList,
        PreferencesSetFavoriteModel, ReadMark, SignInCancel, SignInStart, SignInSubmitCode,
        TaskArchive, TaskInterrupt, TaskReopen, TaskResolve, TaskRetry, WorkerPairingCode,
        WorkerRename, WorkerRevoke, WorkerSetPaused, WorkerSetTags, WorkerToolInstall,
        WorkerToolsCheck,
    },
    ids::{ChannelId, GroupId},
    model::{
        AcpAccountSpec, DecisionAnswer, FavoriteModel, ModelSelection, OptionValue,
        RequestOptionKind, SignInMethod, ToolId,
    },
};
use futures_util::StreamExt as _;
use tokio::sync::watch;

use super::{Shared, invalid, link::ThreadPager, outbox::queue_send, pair};
use crate::{
    AcpSpec, Backend, ClientError, Command, CommandOutput,
    cache::{Cache, PendingSend},
    format,
    view::{
        ApprovalChoice, Connection, PairingCodeView, RegistryLoad, SelectionOption, SelectionView,
        SignInChoice, ViewState,
    },
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
        reply: crate::Reply,
        _publish: &watch::Sender<Arc<ViewState>>,
    ) {
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let _ = reply.send(run(shared, command).await);
        });
    }
}

/// Reads an ID of `what` for a request. See [`parse`].
pub(crate) fn parse_id<T: std::str::FromStr>(id: &str, what: &str) -> Result<T, ClientError> {
    parse(id, what)
}

fn parse<T: std::str::FromStr>(id: &str, what: &str) -> Result<T, ClientError> {
    id.parse().map_err(|_| {
        ClientError::Other(format!(
            "Consort cannot use this {what}, because its ID is not valid. Restart Consort, then try again"
        ))
    })
}

/// A model selection for the hub: the account label, the model ID (not
/// its name) and the options, as the selection holds them.
pub(crate) fn selection(view: &Option<SelectionView>) -> Option<ModelSelection> {
    view.as_ref().map(|s| ModelSelection {
        account_label: s.account_label.clone(),
        model: s.model_id.clone(),
        options: s
            .options
            .iter()
            .map(|(id, value)| {
                (
                    id.clone(),
                    match value {
                        SelectionOption::Bool(b) => OptionValue::Bool(*b),
                        SelectionOption::Text(t) => OptionValue::Text(t.clone()),
                    },
                )
            })
            .collect(),
    })
}

/// The isolation as the proto names it.
pub(crate) fn proto_isolation(i: crate::view::Isolation) -> consort_proto::model::Isolation {
    match i {
        crate::view::Isolation::InPlace => consort_proto::model::Isolation::InPlace,
        crate::view::Isolation::Worktree => consort_proto::model::Isolation::Worktree,
        crate::view::Isolation::Clone => consort_proto::model::Isolation::Clone,
    }
}

/// The approval mode as the proto names it.
pub(crate) fn proto_approval_mode(
    mode: crate::view::ApprovalMode,
) -> consort_proto::model::ApprovalMode {
    match mode {
        crate::view::ApprovalMode::Ask => consort_proto::model::ApprovalMode::Ask,
        crate::view::ApprovalMode::AutoEdits => consort_proto::model::ApprovalMode::AutoEdits,
        crate::view::ApprovalMode::Auto => consort_proto::model::ApprovalMode::Auto,
        crate::view::ApprovalMode::FullAccess => consort_proto::model::ApprovalMode::FullAccess,
    }
}

/// The `channel.update` for some changes, or `None` when they change no
/// field of it. A group change is a `channel.move`, so it is not here. A
/// selection of `Some(None)` sends `null`, which takes the selection away.
pub(crate) fn channel_update(
    id: consort_proto::ids::ChannelId,
    changes: crate::command::ChannelChanges,
) -> Result<Option<ChannelUpdate>, ClientError> {
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
    if let Some(tags) = changes.tags {
        patch["tags"] = tags.into();
    }
    let json = |s: Option<ModelSelection>| {
        serde_json::to_value(s).map_err(|e| ClientError::Other(e.to_string()))
    };
    if let Some(chosen) = changes.coordinator_selection {
        patch["coordinator_selection"] = json(selection(&chosen))?;
    }
    if let Some(chosen) = changes.task_selection {
        patch["task_selection"] = json(selection(&chosen))?;
    }
    if let Some(isolation) = changes.default_isolation {
        patch["default_isolation"] = serde_json::to_value(proto_isolation(isolation))
            .map_err(|e| ClientError::Other(e.to_string()))?;
    }
    if let Some(mode) = changes.default_approval_mode {
        patch["default_approval_mode"] = serde_json::to_value(proto_approval_mode(mode))
            .map_err(|e| ClientError::Other(e.to_string()))?;
    }
    if let Some(servers) = changes.allowed_local_mcp_servers {
        patch["allowed_local_mcp_servers"] = servers.into();
    }
    if let Some(memory) = changes.memory_enabled {
        patch["memory_enabled"] = memory.into();
    }
    // `null` goes back to the default level of the channel.
    if let Some(level) = changes.proactivity {
        patch["proactivity"] =
            serde_json::to_value(level).map_err(|e| ClientError::Other(e.to_string()))?;
    }
    // The caller turns new repo URLs into IDs first (`repo.create`).
    if let Some(repos) = changes.repos {
        let ids: Vec<String> = repos
            .into_iter()
            .filter_map(|r| match r {
                crate::convo::RepoChoice::Existing(id) => Some(id),
                crate::convo::RepoChoice::New(_) => None,
            })
            .collect();
        patch["repo_ids"] = ids.into();
    }
    if patch.as_object().is_none_or(|o| o.len() <= 1) {
        return Ok(None);
    }
    serde_json::from_value(patch)
        .map(Some)
        .map_err(|e| ClientError::Other(e.to_string()))
}

async fn run(shared: Arc<Shared>, command: Command) -> Result<CommandOutput, ClientError> {
    match command {
        Command::Pair { hub_url, code } => {
            let identity = pair(&hub_url, &code, &shared.options).await?;
            if let Some(store) = &shared.store {
                store.save(&identity)?;
            }
            // A new owner first: from here on, a link or a call of the old
            // hub writes nothing into the cache.
            shared.new_owner(|cache| {
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
            });
            if let Ok(mut i) = shared.identity.lock() {
                *i = Some(identity);
            }
            // The saved data is the old hub's. A live link to the old hub
            // ends, so the client connects to the new one.
            shared.forget_saved();
            shared.restart_link();
            shared.defer_waiters();
            shared.set_connection(Connection::Connecting);
            shared.wake.notify_one();
        }
        Command::Unpair => {
            shared.new_owner(|cache| {
                cache.fail_pending(
                    "This message did not go out, because you unpaired this app from the hub.",
                );
                let failures = std::mem::take(&mut cache.send_failures);
                *cache = Cache {
                    send_failures: failures,
                    ..Cache::default()
                };
            });
            if let Ok(mut i) = shared.identity.lock() {
                *i = None;
            }
            shared.forget_saved();
            if let Some(store) = &shared.store {
                store.clear()?;
            }
            shared.defer_waiters();
            // A live link ends at its next tick. The link loop then sees
            // that no identity is left and says that the app is not paired.
            shared.restart_link();
            shared.set_connection(Connection::NotPaired);
            shared.wake.notify_one();
        }
        Command::SendMessage {
            channel_id,
            body,
            client_message_id,
            attachments,
        } => {
            // No channel has loaded yet, or the ID is not a channel. The
            // message would wait in the outbox and fail later.
            crate::command::check_channel_id(&channel_id)?;
            let target = crate::uploads::UploadTarget::Composer {
                channel_id: channel_id.clone(),
            };
            let blobs = shared.wait_for_uploads(&target, &attachments).await?;
            let pending = PendingSend {
                client_message_id,
                channel_id,
                thread_root_id: None,
                body,
                created_at: format::now_ms(),
                attachments: blobs,
            };
            queue_send(&shared, pending).await?;
            shared.take_uploads(&attachments);
            return Ok(CommandOutput::Done);
        }
        Command::ReplyToTask {
            task_id,
            body,
            client_message_id,
            attachments,
        } => {
            let target = crate::uploads::UploadTarget::Reply {
                task_id: task_id.clone(),
            };
            let blobs = shared.wait_for_uploads(&target, &attachments).await?;
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
                attachments: blobs,
            };
            queue_send(&shared, pending).await?;
            shared.take_uploads(&attachments);
            return Ok(CommandOutput::Done);
        }
        Command::Attach { target, inputs } => {
            return Ok(CommandOutput::Attached(shared.attach(target, inputs)));
        }
        Command::RetryUpload { id } => shared.retry_upload(&id)?,
        Command::RemoveUpload { id } => shared.remove_upload(&id),
        Command::LoadChannelFiles { channel_id } => shared.load_files(&channel_id).await?,
        Command::MoveChannelFile {
            channel_id,
            from_path,
            to_path,
        } => shared.move_file(&channel_id, from_path, to_path).await?,
        Command::DeleteChannelFile { channel_id, path } => {
            shared.delete_file(&channel_id, path).await?
        }
        Command::SaveToChannelFiles {
            channel_id,
            path,
            blob,
        } => shared.put_file(&channel_id, path, blob).await?,
        Command::FetchBlob { sha256, size, name } => {
            let path = shared.fetch_blob(&sha256, size).await?;
            let path = match name {
                Some(name) => shared.named_copy(&sha256, &path, &name)?,
                None => path,
            };
            return Ok(CommandOutput::LocalFile(path.display().to_string()));
        }
        Command::SaveBlob { sha256, size, to } => {
            let path = shared.fetch_blob(&sha256, size).await?;
            std::fs::copy(&path, &to).map_err(|e| ClientError::Storage {
                path: to.clone(),
                message: e.to_string(),
            })?;
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
            // A new group comes first. A group of that name that exists
            // already is used again, so a second try after a failure makes
            // no second group.
            let group_id: Option<GroupId> = match (&new.new_group, &new.group_id) {
                (Some(name), _) => Some(group_named(&shared, name).await?),
                (None, Some(g)) => Some(parse(g, "group")?),
                (None, None) => None,
            };
            let created = shared
                .call(ChannelCreate {
                    name: new.name.trim().to_owned(),
                    purpose: new.purpose,
                    instructions: String::new(),
                    icon: new.icon.clone(),
                    color: None,
                    group_id,
                    coordinator_selection: selection(&new.coordinator_selection),
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
                    tags: new.tags,
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
            let group = changes.group_id.clone();
            let mut changes = changes;
            if let Some(repos) = changes.repos.take() {
                changes.repos = Some(super::convo::resolve_repos(&shared, repos).await?);
            }
            if let Some(update) = channel_update(id, changes)? {
                shared.call(update).await?;
            }
            if let Some(group) = group {
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
        Command::UnarchiveChannel { channel_id } => {
            shared
                .call(ChannelArchive {
                    channel_id: parse(&channel_id, "channel")?,
                    archived: false,
                })
                .await?;
            // The channel has no snapshot in the cache yet.
            shared.loader_wake.notify_one();
        }
        Command::MoveChannel {
            channel_id,
            group_id,
        } => {
            shared
                .call(ChannelMove {
                    channel_id: parse(&channel_id, "channel")?,
                    group_id: match &group_id {
                        Some(g) => Some(parse(g, "group")?),
                        None => None,
                    },
                    position: 0,
                })
                .await?;
        }
        Command::CreateGroup { name, channel_id } => {
            let group = group_for_new(&shared, &name).await?;
            if let Some(channel_id) = channel_id {
                shared
                    .call(ChannelMove {
                        channel_id: parse(&channel_id, "channel")?,
                        group_id: Some(group),
                        position: 0,
                    })
                    .await?;
            }
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
        Command::LoadThread { task_id } => {
            return load_thread(&shared, &task_id)
                .await
                .map(|()| CommandOutput::Done);
        }
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
        Command::ArchiveTask { task_id } => {
            shared
                .call(TaskArchive {
                    task_id: parse(&task_id, "task")?,
                    archived: true,
                })
                .await?;
        }
        Command::Reconnect => {
            // Only an offline link waits on a timer. A permit left over from
            // a call while the link is up would skip the next wait.
            let offline = shared
                .connection
                .lock()
                .is_ok_and(|c| matches!(*c, Connection::Offline { .. }));
            if offline {
                shared.wake.notify_one();
            }
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
        Command::SetWorkerTags { worker_id, tags } => {
            shared
                .call(WorkerSetTags {
                    worker_id: parse(&worker_id, "worker")?,
                    tags,
                })
                .await?;
        }
        Command::SetFavoriteModel {
            account_label,
            model_id,
            favorite,
        } => {
            // The feed entry of the change updates the view, here and in
            // every other client.
            shared
                .call(PreferencesSetFavoriteModel {
                    model: FavoriteModel {
                        account_label,
                        model: model_id,
                    },
                    favorite,
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
        Command::CreateAccount(new) => {
            let label = new.label.trim().to_owned();
            if label.is_empty() {
                return Err(invalid("Enter a name for the account."));
            }
            let created = shared
                .call(AccountCreate {
                    worker_id: parse(&new.worker_id, "worker")?,
                    harness: new.harness,
                    label,
                    acp: new.acp.map(|spec| match spec {
                        AcpSpec::Registry { id } => AcpAccountSpec::Registry { id },
                        AcpSpec::Command { program, args } => {
                            AcpAccountSpec::Command { program, args }
                        }
                    }),
                    env: new.env,
                })
                .await?;
            return Ok(CommandOutput::AccountCreated(
                created.account_id.to_string(),
            ));
        }
        Command::UpdateAccount {
            worker_id,
            account_id,
            label,
            env_set,
            env_remove,
        } => {
            let label = label.map(|l| l.trim().to_owned());
            if label.as_deref() == Some("") {
                return Err(invalid("Enter a name for the account."));
            }
            shared
                .call(AccountUpdate {
                    worker_id: parse(&worker_id, "worker")?,
                    account_id: parse(&account_id, "account")?,
                    label,
                    env_set,
                    env_remove,
                    extras_enabled: None,
                })
                .await?;
        }
        Command::RemoveAccount {
            worker_id,
            account_id,
        } => {
            shared
                .call(AccountRemove {
                    worker_id: parse(&worker_id, "worker")?,
                    account_id: parse(&account_id, "account")?,
                })
                .await?;
        }
        Command::SignOutAccount {
            worker_id,
            account_id,
        } => {
            shared
                .call(AccountSignOut {
                    worker_id: parse(&worker_id, "worker")?,
                    account_id: parse(&account_id, "account")?,
                })
                .await?;
        }
        Command::StartSignIn {
            worker_id,
            account_id,
            method,
        } => {
            let started = shared
                .call(SignInStart {
                    worker_id: parse(&worker_id, "worker")?,
                    account_id: parse(&account_id, "account")?,
                    method: sign_in_method(&method),
                })
                .await?;
            let id = started.sign_in.id.to_string();
            if let Ok(mut cache) = shared.cache.lock() {
                // A feed entry may have come first with a newer state.
                let newer = cache
                    .sign_ins
                    .get(&id)
                    .is_some_and(|s| s.updated_at > started.sign_in.updated_at);
                if !newer {
                    cache.sign_ins.insert(id.clone(), started.sign_in);
                }
            }
            shared.publish();
            return Ok(CommandOutput::SignInStarted(id));
        }
        Command::SubmitSignInCode { sign_in_id, code } => {
            let code = code.0.trim().to_owned();
            if code.is_empty() {
                return Err(invalid("Paste the code that the sign-in page shows."));
            }
            shared
                .call(SignInSubmitCode {
                    sign_in_id: parse(&sign_in_id, "sign-in")?,
                    code,
                })
                .await?;
        }
        Command::CancelSignIn { sign_in_id } => {
            shared
                .call(SignInCancel {
                    sign_in_id: parse(&sign_in_id, "sign-in")?,
                })
                .await?;
        }
        Command::DismissSignIn { sign_in_id } => {
            if let Ok(mut cache) = shared.cache.lock() {
                cache.sign_ins.remove(&sign_in_id);
                cache.sign_ins_dismissed.insert(sign_in_id);
            }
            shared.publish();
        }
        Command::CheckTools { worker_id } => {
            shared
                .call(WorkerToolsCheck {
                    worker_id: parse(&worker_id, "worker")?,
                })
                .await?;
        }
        Command::InstallTool { worker_id, tool } => {
            shared
                .call(WorkerToolInstall {
                    worker_id: parse(&worker_id, "worker")?,
                    tool: ToolId::new(tool),
                })
                .await?;
        }
        Command::LoadAcpRegistry => load_registry(&shared).await?,
        // Settings > Hub and Settings > Clients.
        command @ (Command::LoadHubStatus
        | Command::BackUpNow
        | Command::UpdateBackupSettings(_)
        | Command::LoadClients
        | Command::RevokeClient { .. }
        | Command::ClientPairingCode
        | Command::CancelClientPairing) => return super::admin::run(&shared, command).await,
        // Settings > Proactivity.
        command @ (Command::LoadProactivityLog
        | Command::SetProactivity(_)
        | Command::SnoozeProactivity(_)
        | Command::CancelAllFollowUps { .. }
        | Command::SetChannelProactivity { .. }
        | Command::RateMessage { .. }) => {
            return super::proactivity::run(&shared, command).await;
        }
        // Routines and memory.
        command @ (Command::SaveRoutine { .. }
        | Command::SetRoutineEnabled { .. }
        | Command::DeleteRoutine { .. }
        | Command::RunRoutineNow { .. }
        | Command::LoadRoutineRuns { .. }
        | Command::LoadMemory { .. }
        | Command::ReadMemoryNote { .. }
        | Command::WriteMemoryNote { .. }
        | Command::DeleteMemoryNote { .. }
        | Command::RestoreMemoryNote { .. }
        | Command::MoveMemoryNote { .. }
        | Command::LoadMemoryHistory { .. }) => {
            return super::knowledge::run(&shared, command).await;
        }
        // Phase 4: the conversation and the tasks.
        command @ (Command::React { .. }
        | Command::LoadMessageThread { .. }
        | Command::FindMessage { .. }
        | Command::ReplyInThread { .. }
        | Command::Search { .. }
        | Command::ClearSearch
        | Command::CreateTask(_)
        | Command::DeleteTask { .. }
        | Command::RestoreTask { .. }
        | Command::LoadArchivedTasks { .. }
        | Command::OpenTaskItem { .. }
        | Command::CloseTaskItem
        | Command::DeleteGroup { .. }
        | Command::RenameGroup { .. }
        | Command::ReorderGroups { .. }) => return super::convo::run(&shared, command).await,
        // Connectors, skills and the provider extras.
        other => return super::extensions::run(&shared, other).await,
    }
    Ok(CommandOutput::Done)
}

/// A sign-in method as the proto names it.
pub(crate) fn sign_in_method(choice: &SignInChoice) -> SignInMethod {
    match choice {
        SignInChoice::ClaudePlan => SignInMethod::ClaudePlan,
        SignInChoice::ClaudeConsole => SignInMethod::ClaudeConsole,
        SignInChoice::ChatgptDevice => SignInMethod::ChatgptDevice,
        SignInChoice::ChatgptBrowser => SignInMethod::ChatgptBrowser,
        SignInChoice::ApiKey { variable } => SignInMethod::ApiKey {
            variable: variable.clone(),
        },
        SignInChoice::AcpAgent { method_id, name } => SignInMethod::AcpAgent {
            method_id: method_id.clone(),
            name: name.clone(),
        },
        SignInChoice::Terminal { command } => SignInMethod::Terminal {
            command: command.clone(),
        },
    }
}

/// The largest icon that the client loads for a registry agent.
const ICON_MAX_BYTES: usize = 64 * 1024;
/// How many icons load at once.
const ICON_LOADS: usize = 6;

/// Loads the ACP registry from the hub, then the icons of its agents. A
/// second call while a load runs, or after a load that worked, does
/// nothing. A failed load can be tried again.
async fn load_registry(shared: &Arc<Shared>) -> Result<(), ClientError> {
    let start = shared.cache.lock().is_ok_and(|mut c| {
        if matches!(
            c.acp_registry_load,
            RegistryLoad::Loading | RegistryLoad::Loaded
        ) {
            return false;
        }
        c.acp_registry_load = RegistryLoad::Loading;
        true
    });
    if !start {
        return Ok(());
    }
    shared.publish();
    let result = shared.call(AcpRegistryList {}).await;
    let agents = match result {
        Ok(list) => list.agents,
        Err(err) => {
            if let Ok(mut c) = shared.cache.lock() {
                c.acp_registry_load =
                    RegistryLoad::Failed(format!("Consort cannot load the ACP registry. {err}."));
            }
            shared.publish();
            return Err(err);
        }
    };
    let icons: Vec<(String, String)> = agents
        .iter()
        .filter_map(|a| a.icon_url.clone().map(|url| (a.id.clone(), url)))
        .collect();
    if let Ok(mut c) = shared.cache.lock() {
        c.acp_registry = agents;
        c.acp_registry_load = RegistryLoad::Loaded;
    }
    shared.publish();
    // The icons load in the background. An agent whose icon does not load
    // keeps a neutral glyph.
    let shared = shared.clone();
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                tracing::warn!(error = %err, "cannot make the icon client");
                return;
            }
        };
        futures_util::stream::iter(icons)
            .for_each_concurrent(ICON_LOADS, |(id, url)| {
                let client = client.clone();
                let shared = shared.clone();
                async move {
                    match load_icon(&client, &url).await {
                        Some(svg) => {
                            if let Ok(mut c) = shared.cache.lock() {
                                c.acp_icons.insert(id, svg);
                            }
                            shared.publish();
                        }
                        None => tracing::debug!(%url, "the icon of a registry agent did not load"),
                    }
                }
            })
            .await;
    });
    Ok(())
}

/// Loads one registry icon: an `https` URL that gives an SVG of at most
/// [`ICON_MAX_BYTES`]. Anything else gives `None`.
async fn load_icon(client: &reqwest::Client, url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let response = client.get(parsed).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    if response
        .content_length()
        .is_some_and(|len| len > ICON_MAX_BYTES as u64)
    {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    checked_svg(&bytes)
}

/// The text of an SVG icon, or `None` when the bytes are too large or are
/// not an SVG document.
pub(crate) fn checked_svg(bytes: &[u8]) -> Option<String> {
    if bytes.len() > ICON_MAX_BYTES {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let head = text.trim_start_matches('\u{feff}').trim_start();
    let is_svg = head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg"));
    is_svg.then(|| text.to_owned())
}

/// The name of a group as the hub stores it: trimmed.
fn group_name(name: &str) -> Result<String, ClientError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("Enter a name for the group."));
    }
    Ok(name.to_owned())
}

/// Makes a group and returns its ID.
async fn group_for_new(shared: &Shared, name: &str) -> Result<GroupId, ClientError> {
    let name = group_name(name)?;
    Ok(shared.call(GroupCreate { name }).await?.group.id)
}

/// The group with this name, or a new one when no group has it.
async fn group_named(shared: &Shared, name: &str) -> Result<GroupId, ClientError> {
    let name = group_name(name)?;
    let existing = shared.cache.lock().ok().and_then(|c| {
        c.groups
            .values()
            .find(|g| g.name.trim().eq_ignore_ascii_case(&name))
            .map(|g| g.id)
    });
    match existing {
        Some(id) => Ok(id),
        None => group_for_new(shared, &name).await,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_sends_the_model_id_and_its_options() {
        let view = SelectionView {
            account_label: "Claude Personal".into(),
            model_id: "claude-opus-5-5".into(),
            model_name: "Opus 5.5".into(),
            options: vec![
                ("effort".into(), SelectionOption::Text("high".into())),
                ("thinking".into(), SelectionOption::Bool(true)),
            ],
        };
        let sent = selection(&Some(view)).expect("selection");
        assert_eq!(sent.account_label, "Claude Personal");
        assert_eq!(sent.model, "claude-opus-5-5", "the ID, not the name");
        assert_eq!(
            sent.options.get("effort"),
            Some(&OptionValue::Text("high".into()))
        );
        assert_eq!(sent.options.get("thinking"), Some(&OptionValue::Bool(true)));
        assert_eq!(selection(&None), None);
    }

    #[test]
    fn a_model_change_sends_channel_update_with_the_selection_or_null() {
        let id = consort_proto::ids::ChannelId::new();
        let chosen = SelectionView {
            account_label: "Claude Personal".into(),
            model_id: "opus".into(),
            model_name: "Opus 5.5".into(),
            options: vec![("effort".into(), SelectionOption::Text("high".into()))],
        };
        let update = channel_update(
            id,
            crate::command::ChannelChanges {
                coordinator_selection: Some(Some(chosen)),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        let sent = update.coordinator_selection.clone().flatten().unwrap();
        assert_eq!(sent.model, "opus");
        assert_eq!(
            sent.options.get("effort"),
            Some(&OptionValue::Text("high".into()))
        );
        assert_eq!(update.task_selection, None, "the task model stays");
        let wire = serde_json::to_value(&update).unwrap();
        assert_eq!(
            wire["coordinator_selection"]["account_label"],
            "Claude Personal"
        );

        // "Use the Tutti model" sends null.
        let cleared = channel_update(
            id,
            crate::command::ChannelChanges {
                coordinator_selection: Some(None),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(cleared.coordinator_selection, Some(None));
        let wire = serde_json::to_value(&cleared).unwrap();
        assert!(wire["coordinator_selection"].is_null());
        assert!(
            wire.as_object()
                .unwrap()
                .contains_key("coordinator_selection")
        );

        // A change with no field of channel.update sends nothing.
        assert!(
            channel_update(id, crate::command::ChannelChanges::default())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn every_sign_in_choice_maps_back_to_its_method() {
        let methods = [
            SignInMethod::ClaudePlan,
            SignInMethod::ClaudeConsole,
            SignInMethod::ChatgptDevice,
            SignInMethod::ChatgptBrowser,
            SignInMethod::ApiKey {
                variable: "OPENAI_API_KEY".into(),
            },
            SignInMethod::AcpAgent {
                method_id: "oauth".into(),
                name: "Google account".into(),
            },
            SignInMethod::Terminal {
                command: "goose configure".into(),
            },
        ];
        for method in methods {
            let choice = crate::reduce::sign_in_choice(&method);
            assert_eq!(sign_in_method(&choice), method);
        }
    }

    #[test]
    fn only_a_small_svg_counts_as_an_icon() {
        assert!(checked_svg(b"<svg xmlns='http://www.w3.org/2000/svg'></svg>").is_some());
        assert!(checked_svg(b"<?xml version='1.0'?>\n<svg></svg>").is_some());
        assert!(checked_svg(b"<html><svg></svg></html>").is_none());
        assert!(checked_svg(&[0xff, 0xfe]).is_none());
        let big = format!("<svg>{}</svg>", "a".repeat(ICON_MAX_BYTES));
        assert!(checked_svg(big.as_bytes()).is_none());
    }

    #[test]
    fn a_sign_in_code_never_shows_in_debug_output() {
        let command = Command::SubmitSignInCode {
            sign_in_id: "sgn_1".into(),
            code: crate::SecretText("secret-code".into()),
        };
        assert!(!format!("{command:?}").contains("secret-code"));
    }
}
