//! The mock backend: commands change the mock view state the way the hub
//! would change the real one.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::watch;

use crate::{
    AcpSpec, Backend, ClientError, Command, CommandOutput,
    derive::{refresh, waiting_detail},
    format,
    view::*,
};

pub(crate) struct MockBackend {
    state: Arc<Mutex<ViewState>>,
    next_id: u64,
}

/// A change that the mock makes a while after a command, the way a worker
/// answers later: a sign-in that ends, or an install that finishes.
type Later = crate::mock_backend_ext::Later;

/// What a command did to the mock.
#[derive(Default)]
struct Applied {
    /// The channel that gets the demo reply.
    reply_to: Option<String>,
    output: CommandOutput,
    later: Vec<Later>,
}

fn publish_state(state: &Mutex<ViewState>, publish: &watch::Sender<Arc<ViewState>>) {
    let snapshot = match state.lock() {
        Ok(mut s) => {
            refresh(&mut s);
            s.clone()
        }
        Err(_) => return,
    };
    let _ = publish.send(Arc::new(snapshot));
}

impl MockBackend {
    pub(crate) fn new(state: ViewState) -> MockBackend {
        MockBackend {
            state: Arc::new(Mutex::new(state)),
            next_id: 1,
        }
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}_mock_{}", self.next_id)
    }

    fn apply(&mut self, command: Command) -> Result<Applied, ClientError> {
        let mut applied = Applied::default();
        let new_account_id = self.id("acc");
        let new_sign_in_id = self.id("sgn");
        let now = format::now_ms();
        let time = format::time_label(now);
        let message_id = self.id("msg");
        let channel_id_new = self.id("chn");
        let card_id = self.id("mock");
        let mut state = self
            .state
            .lock()
            .map_err(|_| ClientError::Other("The mock state is poisoned".into()))?;
        let mut reply_to = None;
        let archiving = matches!(command, Command::ArchiveChannel { .. });
        match command {
            Command::SendMessage {
                channel_id,
                body,
                attachments,
                ..
            } => {
                crate::command::check_channel_id(&channel_id)?;
                timeline_mut(&mut state, &channel_id)?;
                let attachments = crate::mock_files::take_ready(&mut state, &attachments)?;
                let timeline = timeline_mut(&mut state, &channel_id)?;
                timeline.items.push(TimelineItem::Message(MessageView {
                    id: message_id,
                    channel_id: channel_id.clone(),
                    speaker: Speaker::You,
                    time_label: time,
                    created_at: now,
                    body,
                    attachments,
                    ..MessageView::default()
                }));
                reply_to = Some(channel_id);
            }
            Command::ReplyToTask {
                task_id,
                body,
                attachments,
                ..
            } => {
                if !state.threads.iter().any(|t| t.task_id == task_id) {
                    return Err(not_found());
                }
                let attachments = crate::mock_files::take_ready(&mut state, &attachments)?;
                let thread = state
                    .threads
                    .iter_mut()
                    .find(|t| t.task_id == task_id)
                    .ok_or_else(not_found)?;
                let mut bodies = vec![PostBody::Text(body)];
                if !attachments.is_empty() {
                    bodies.push(PostBody::Attachments(attachments));
                }
                thread.posts.push(ThreadPost {
                    id: message_id,
                    speaker: Speaker::You,
                    time_label: time,
                    bodies,
                    pending: false,
                    source_message_id: None,
                    refused: None,
                });
                if let Some(task) = state.tasks.iter_mut().find(|t| t.id == task_id)
                    && task.status == TaskStatus::Review
                {
                    task.status = TaskStatus::Working;
                }
            }
            Command::EditMessage { message_id, body } => {
                for timeline in &mut state.timelines {
                    for item in &mut timeline.items {
                        if let TimelineItem::Message(m) = item
                            && m.id == message_id
                        {
                            m.body = body.clone();
                            m.edited = true;
                        }
                    }
                }
            }
            Command::DeleteMessage { message_id } => {
                for timeline in &mut state.timelines {
                    timeline.items.retain(|item| item.key() != message_id);
                }
            }
            Command::CreateChannel(new) => {
                let name = new.name.trim().to_owned();
                if name.is_empty() {
                    return Err(ClientError::Hub {
                        code: "invalid".into(),
                        message: "Enter a name for the channel.".into(),
                    });
                }
                let group_id = match new.new_group.as_deref().map(str::trim) {
                    Some("") => {
                        return Err(ClientError::Hub {
                            code: "invalid".into(),
                            message: "Enter a name for the group.".into(),
                        });
                    }
                    Some(group_name) => {
                        let id = state
                            .groups
                            .iter()
                            .find(|g| g.name.eq_ignore_ascii_case(group_name))
                            .map(|g| g.id.clone())
                            .unwrap_or_else(|| {
                                let id = format!("grp_{}", channel_id_new);
                                state.groups.push(GroupView {
                                    id: id.clone(),
                                    name: group_name.to_owned(),
                                });
                                id
                            });
                        Some(id)
                    }
                    None => new.group_id,
                };
                let inherited = tutti_selection(&state);
                state.channels.push(ChannelView {
                    id: channel_id_new.clone(),
                    kind: ChannelKind::Standard,
                    name,
                    purpose: new.purpose,
                    icon: new.icon.unwrap_or_else(|| "hash".into()),
                    group_id,
                    coordinator_worker: Some("hub".into()),
                    coordinator_selection: crate::picker::effective_selection(
                        new.coordinator_selection,
                        inherited,
                    ),
                    preferred_workers: new.preferred_workers,
                    task_selection: new.task_selection,
                    default_isolation: new.isolation,
                    tags: checked_tags(&new.tags)?,
                    memory_enabled: true,
                    ..ChannelView::default()
                });
                state.timelines.push(TimelineView {
                    channel_id: channel_id_new,
                    items: Vec::new(),
                    has_older: false,
                    loaded: true,
                });
            }
            Command::UpdateChannel {
                channel_id,
                changes,
            } => {
                let new_repos = changes
                    .repos
                    .clone()
                    .map(|r| crate::mock_convo::channel_repos(&state.convo.repos, r));
                let channel = state
                    .channels
                    .iter_mut()
                    .find(|c| c.id == channel_id)
                    .ok_or_else(not_found)?;
                if let Some(name) = changes.name {
                    channel.name = name;
                }
                if let Some(purpose) = changes.purpose {
                    channel.purpose = purpose;
                }
                if let Some(instructions) = changes.instructions {
                    channel.instructions = instructions;
                }
                if let Some(icon) = changes.icon {
                    channel.icon = icon;
                }
                if let Some(group) = changes.group_id {
                    channel.group_id = Some(group);
                }
                if let Some(tags) = changes.tags {
                    channel.tags = checked_tags(&tags)?;
                }
                if let Some(chosen) = changes.task_selection {
                    channel.task_selection = chosen;
                }
                if let Some(isolation) = changes.default_isolation {
                    channel.default_isolation = isolation;
                }
                if let Some(mode) = changes.default_approval_mode {
                    channel.default_approval_mode = mode;
                }
                if let Some(servers) = changes.allowed_local_mcp_servers {
                    channel.allowed_local_mcp_servers = servers;
                }
                if let Some(memory) = changes.memory_enabled {
                    channel.memory_enabled = memory;
                }
                if let Some(repos) = new_repos {
                    for repo in &repos {
                        if !state.convo.repos.iter().any(|r| r.id == repo.id) {
                            state.convo.repos.push(repo.clone());
                        }
                    }
                    let channel = state
                        .channels
                        .iter_mut()
                        .find(|c| c.id == channel_id)
                        .ok_or_else(not_found)?;
                    channel.repos = repos;
                }
                let channel = state
                    .channels
                    .iter_mut()
                    .find(|c| c.id == channel_id)
                    .ok_or_else(not_found)?;
                let name = channel.name.clone();
                for task in state
                    .tasks
                    .iter_mut()
                    .filter(|t| t.channel_id == channel_id)
                {
                    task.channel_name = name.clone();
                }
                if let Some(chosen) = changes.coordinator_selection {
                    set_coordinator_selection(&mut state, &channel_id, chosen);
                }
                if let Some(level) = changes.proactivity {
                    crate::mock_proactivity::apply(
                        &mut state,
                        Command::SetChannelProactivity { channel_id, level },
                    )?;
                }
            }
            Command::ArchiveChannel { channel_id } | Command::DeleteChannel { channel_id } => {
                if state
                    .channel(&channel_id)
                    .is_some_and(|c| c.kind == ChannelKind::Tutti)
                {
                    return Err(ClientError::Hub {
                        code: "conflict".into(),
                        message: "Tutti cannot be archived or deleted.".into(),
                    });
                }
                if archiving
                    && let Some(channel) = state.channels.iter().find(|c| c.id == channel_id)
                {
                    let archived = ArchivedChannelView {
                        id: channel.id.clone(),
                        name: channel.name.clone(),
                        icon: channel.icon.clone(),
                    };
                    state.archived_channels.push(archived);
                }
                state.channels.retain(|c| c.id != channel_id);
                state.tasks.retain(|t| t.channel_id != channel_id);
            }
            Command::UnarchiveChannel { channel_id } => {
                let Some(at) = state
                    .archived_channels
                    .iter()
                    .position(|c| c.id == channel_id)
                else {
                    return Err(not_found());
                };
                let archived = state.archived_channels.remove(at);
                state.channels.push(ChannelView {
                    id: archived.id.clone(),
                    kind: ChannelKind::Standard,
                    name: archived.name,
                    icon: archived.icon,
                    ..ChannelView::default()
                });
                state.timelines.push(TimelineView {
                    channel_id: archived.id,
                    items: Vec::new(),
                    has_older: false,
                    loaded: true,
                });
            }
            Command::MoveChannel {
                channel_id,
                group_id,
            } => {
                if let Some(group) = &group_id
                    && !state.groups.iter().any(|g| &g.id == group)
                {
                    return Err(not_found());
                }
                let channel = state
                    .channels
                    .iter_mut()
                    .find(|c| c.id == channel_id)
                    .ok_or_else(not_found)?;
                channel.group_id = group_id;
            }
            Command::CreateGroup { name, channel_id } => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    return Err(ClientError::Hub {
                        code: "invalid".into(),
                        message: "Enter a name for the group.".into(),
                    });
                }
                let id = format!("grp_{channel_id_new}");
                state.groups.push(GroupView {
                    id: id.clone(),
                    name,
                });
                if let Some(channel_id) = channel_id {
                    state
                        .channels
                        .iter_mut()
                        .find(|c| c.id == channel_id)
                        .ok_or_else(not_found)?
                        .group_id = Some(id);
                }
            }
            Command::ArchiveTask { task_id } => {
                if !state.tasks.iter().any(|t| t.id == task_id) {
                    return Err(not_found());
                }
                state.tasks.retain(|t| t.id != task_id);
                state.threads.retain(|t| t.task_id != task_id);
                for timeline in &mut state.timelines {
                    timeline.items.retain(
                        |item| !matches!(item, TimelineItem::TaskCard(c) if c.task_id == task_id),
                    );
                }
            }
            // The mock never goes offline.
            Command::Reconnect => {}
            Command::SetGroupCollapsed {
                group_id,
                collapsed,
            } => {
                if let Some(group) = state.sidebar.groups.iter_mut().find(|g| g.id == group_id) {
                    group.collapsed = collapsed;
                }
            }
            Command::MarkRead { channel_id } => {
                let sidebar = &mut state.sidebar;
                for c in std::iter::once(&mut sidebar.tutti)
                    .chain(
                        sidebar
                            .groups
                            .iter_mut()
                            .flat_map(|g| g.channels.iter_mut()),
                    )
                    .chain(sidebar.ungrouped.iter_mut())
                {
                    if c.id == channel_id {
                        c.unread = 0;
                    }
                }
            }
            // The mock holds every message already.
            Command::LoadThread { .. }
            | Command::LoadOlder { .. }
            | Command::DismissSendFailure { .. } => {}
            Command::ResolveTask { task_id } => {
                set_status(&mut state, &task_id, TaskStatus::Resolved)?
            }
            Command::ReopenTask { task_id } => {
                set_status(&mut state, &task_id, TaskStatus::Review)?
            }
            Command::RetryTask { task_id } => {
                set_status(&mut state, &task_id, TaskStatus::Working)?;
                if let Some(task) = state.tasks.iter_mut().find(|t| t.id == task_id) {
                    task.status_reason = None;
                }
            }
            Command::SetTaskModel { task_id, selection } => {
                let thread = state
                    .threads
                    .iter_mut()
                    .find(|t| t.task_id == task_id)
                    .ok_or_else(not_found)?;
                if !thread.can_change_model {
                    return Err(ClientError::Other(
                        "The task is resolved. Mark it as not done first, then change its model."
                            .into(),
                    ));
                }
                thread.model_label = Some(selection.label());
                thread.selection = Some(selection);
            }
            Command::EditDraft {
                message_id,
                title,
                body,
            } => edit_mock_draft(&mut state, &message_id, title, body)?,
            Command::StopTask { task_id } => set_status(&mut state, &task_id, TaskStatus::Review)?,
            Command::AnswerApproval {
                approval_id,
                choice,
                ..
            } => {
                let mut task_id = None;
                for_each_approval(&mut state, &approval_id, |approval| {
                    approval.state = RequestState::Answered;
                    approval.answer = Some(choice);
                    task_id = approval.task_id.clone();
                });
                crate::mock_backend_ext::after_card_answer(
                    &mut state,
                    &approval_id,
                    choice,
                    &card_id,
                    &mut applied.later,
                );
                if let Some(task_id) = task_id {
                    settle(&mut state, &task_id);
                }
            }
            Command::AnswerDecision {
                decision_id,
                option,
                text,
            } => {
                let mut task_id = None;
                for timeline in &mut state.timelines {
                    for item in &mut timeline.items {
                        if let TimelineItem::Decision(card) = item
                            && card.decision.id == decision_id
                        {
                            card.decision.state = RequestState::Answered;
                            card.decision.chosen = option;
                            card.decision.answer_text = text.clone();
                        }
                    }
                }
                for thread in &mut state.threads {
                    for post in &mut thread.posts {
                        for body in &mut post.bodies {
                            if let PostBody::Decision(d) = body
                                && d.id == decision_id
                            {
                                d.state = RequestState::Answered;
                                d.chosen = option;
                                d.answer_text = text.clone();
                                task_id = d.task_id.clone();
                            }
                        }
                    }
                }
                if let Some(task_id) = task_id {
                    settle(&mut state, &task_id);
                }
            }
            Command::WorkerPairingCode => {
                state.worker_pairing = Some(PairingCodeView {
                    code: "KQ7M-4XPD".into(),
                    hub_url: "https://hub.example.com".into(),
                    expires_at: now + 10 * 60_000,
                });
            }
            Command::CancelWorkerPairing => state.worker_pairing = None,
            Command::Pair { .. } => state.connection = Connection::Demo,
            Command::Unpair => state.connection = Connection::NotPaired,
            Command::RenameWorker { worker_id, name } => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    return Err(ClientError::Hub {
                        code: "invalid".into(),
                        message: "Enter a name for the worker.".into(),
                    });
                }
                worker_mut(&mut state, &worker_id)?.name = name;
            }
            Command::SetWorkerTags { worker_id, tags } => {
                let tags = checked_tags(&tags)?;
                worker_mut(&mut state, &worker_id)?.tags = tags;
            }
            Command::SetWorkerPolicy {
                worker_id,
                max_approval_mode,
                allowed_roots,
                allow_shell,
            } => {
                let worker = worker_mut(&mut state, &worker_id)?;
                if !worker.online {
                    return Err(ClientError::Hub {
                        code: "worker_offline".into(),
                        message: format!(
                            "{} is offline, so its limits did not change. Try again when the worker is online.",
                            worker.name
                        ),
                    });
                }
                if let Some(mode) = max_approval_mode {
                    worker.policy.max_approval_mode = mode;
                }
                if let Some(roots) = allowed_roots {
                    worker.policy.allowed_roots = roots;
                }
                if let Some(shell) = allow_shell {
                    worker.policy.allow_shell = shell;
                }
            }
            Command::SetCommandCheck { choice } => {
                state.command_check.choice = choice;
                refresh_endpoint_use(&mut state);
            }
            Command::SaveEndpoint {
                id,
                name,
                base_url,
                api_key,
            } => match id.and_then(|id| state.endpoints.iter().position(|e| e.id == id)) {
                Some(at) => {
                    let endpoint = &mut state.endpoints[at];
                    endpoint.name = name;
                    endpoint.base_url = base_url;
                    if let Some(key) = api_key {
                        endpoint.key_last_four = last_four(&key);
                    }
                }
                None => {
                    let Some(key) = api_key else {
                        return Err(ClientError::Hub {
                            code: "invalid_request".to_owned(),
                            message: "Paste the API key of the endpoint.".to_owned(),
                        });
                    };
                    let id = format!("end_{}", state.endpoints.len() + 1);
                    state.endpoints.push(crate::command_check::EndpointView {
                        id,
                        name,
                        base_url,
                        key_last_four: last_four(&key),
                        in_use: false,
                    });
                }
            },
            Command::RemoveEndpoint { id } => {
                state.endpoints.retain(|e| e.id != id);
                let choice = &mut state.command_check.choice;
                if choice
                    .decision
                    .as_ref()
                    .is_some_and(|c| c.endpoint_id == id)
                {
                    choice.decision = None;
                    if choice.kind == crate::command_check::CheckSourceKind::Decision {
                        choice.kind = crate::command_check::CheckSourceKind::Account;
                    }
                }
                if choice.chat.as_ref().is_some_and(|c| c.endpoint_id == id) {
                    choice.chat = None;
                    if choice.kind == crate::command_check::CheckSourceKind::Chat {
                        choice.kind = crate::command_check::CheckSourceKind::Account;
                    }
                }
                refresh_endpoint_use(&mut state);
            }
            Command::TestEndpoint { .. } => {
                applied.output = CommandOutput::Tested(crate::command_check::TestResultView {
                    ok: true,
                    message: "The endpoint answered and took the key.".to_owned(),
                    details: None,
                });
            }
            Command::TestCommandCheck => {
                applied.output = CommandOutput::Tested(crate::command_check::TestResultView {
                    ok: true,
                    message:
                        "Test passed in 1.2 s. It allowed git status and blocked curl example.sh | sh."
                            .to_owned(),
                    details: None,
                });
            }
            Command::SetFallbackPath {
                other_accounts,
                steps,
            } => {
                let first = state.fallback_path.first.clone();
                let first_selection = first.as_ref().map(|f| f.selection.clone());
                let groups = state.model_groups.clone();
                state.fallback_path.other_accounts = other_accounts;
                state.fallback_path.steps = steps
                    .into_iter()
                    .map(|s| {
                        crate::fallback::step_from_groups(&groups, s, first_selection.as_ref())
                    })
                    .collect();
            }
            Command::GoBack { channel_id } => {
                if let Some(channel) = state.channels.iter_mut().find(|c| c.id == channel_id) {
                    channel.fallback = None;
                }
            }
            Command::SetFavoriteModel {
                account_label,
                model_id,
                favorite,
            } => {
                let star = FavoriteModelView {
                    account_label,
                    model_id,
                };
                let at = state.favorite_models.iter().position(|f| *f == star);
                match (at, favorite) {
                    (None, true) => state.favorite_models.push(star),
                    (Some(i), false) => {
                        state.favorite_models.remove(i);
                    }
                    _ => {}
                }
            }
            Command::SetWorkerPaused { worker_id, paused } => {
                worker_mut(&mut state, &worker_id)?.paused = paused;
            }
            Command::RemoveWorker { worker_id } => state.workers.retain(|w| w.id != worker_id),
            Command::CreateAccount(new) => {
                let label = new.label.trim().to_owned();
                if label.is_empty() {
                    return Err(invalid("Enter a name for the account."));
                }
                let worker = online_worker_mut(&mut state, &new.worker_id)?;
                let harness = match &new.acp {
                    Some(AcpSpec::Registry { id }) => format!("acp:{id}"),
                    _ => new.harness.clone(),
                };
                let (mut provider, mut logo) = provider_of(&harness);
                if let ProviderLogo::Registry { id, .. } = &logo
                    && let Some(agent) = crate::mock::acp_agents().into_iter().find(|a| &a.id == id)
                {
                    provider = agent.name;
                    logo = agent.logo;
                }
                let keyed = !new.env.is_empty();
                let status = if keyed {
                    AccountStatus::Ready
                } else {
                    AccountStatus::NotSignedIn
                };
                worker.accounts.push(HarnessAccountView {
                    id: new_account_id.clone(),
                    label,
                    harness: harness.clone(),
                    provider,
                    logo,
                    detail: if keyed {
                        format!("API key saved on {}.", worker.name)
                    } else {
                        "Not signed in yet.".to_owned()
                    },
                    detail_code: Vec::new(),
                    auth: if keyed {
                        AuthState::KeyFound
                    } else {
                        AuthState::SignedOut
                    },
                    status,
                    action: status.action(),
                    sign_in_methods: crate::mock::sign_in_methods(&harness),
                    secret_names: new.env.names(),
                    email: None,
                });
                applied.output = CommandOutput::AccountCreated(new_account_id);
            }
            Command::UpdateAccount {
                worker_id,
                account_id,
                label,
                env_set,
                env_remove,
            } => {
                let worker = online_worker_mut(&mut state, &worker_id)?;
                let worker_name = worker.name.clone();
                let account = account_mut(worker, &account_id)?;
                if let Some(label) = label {
                    let label = label.trim().to_owned();
                    if label.is_empty() {
                        return Err(invalid("Enter a name for the account."));
                    }
                    account.label = label;
                }
                account.secret_names.retain(|n| !env_remove.contains(n));
                for name in env_set.names() {
                    if !account.secret_names.contains(&name) {
                        account.secret_names.push(name);
                    }
                }
                if !env_set.is_empty() {
                    set_account_status(
                        account,
                        AccountStatus::Ready,
                        format!("API key saved on {worker_name}."),
                    );
                }
            }
            Command::RemoveAccount {
                worker_id,
                account_id,
            } => {
                let worker = online_worker_mut(&mut state, &worker_id)?;
                account_mut(worker, &account_id)?;
                worker.accounts.retain(|a| a.id != account_id);
            }
            Command::SignOutAccount {
                worker_id,
                account_id,
            } => {
                let worker = online_worker_mut(&mut state, &worker_id)?;
                let account = account_mut(worker, &account_id)?;
                account.secret_names.clear();
                set_account_status(
                    account,
                    AccountStatus::NotSignedIn,
                    "Signed out. Sign in to use this account again.".to_owned(),
                );
            }
            Command::StartSignIn {
                worker_id,
                account_id,
                method,
            } => {
                let worker = online_worker_mut(&mut state, &worker_id)?;
                account_mut(worker, &account_id)?;
                let step = match &method {
                    SignInChoice::ClaudePlan | SignInChoice::ClaudeConsole => {
                        SignInStep::NeedsCode {
                            url:
                                "https://claude.ai/oauth/authorize?code=true&client_id=consort-mock"
                                    .into(),
                        }
                    }
                    SignInChoice::ChatgptDevice => SignInStep::DeviceCode {
                        url: "https://auth.openai.com/codex/device".into(),
                        user_code: "KQ7M-4XPD".into(),
                    },
                    SignInChoice::ChatgptBrowser | SignInChoice::AcpAgent { .. } => {
                        SignInStep::OpenUrl {
                            url: "https://example.com/sign-in".into(),
                        }
                    }
                    SignInChoice::ApiKey { .. } | SignInChoice::Terminal { .. } => {
                        return Err(invalid(
                            "This way to sign in has no flow on the worker. Save the key, or run the command on the worker.",
                        ));
                    }
                };
                let waits_for_page = matches!(
                    step,
                    SignInStep::DeviceCode { .. } | SignInStep::OpenUrl { .. }
                );
                state.sign_ins.push(SignInView {
                    id: new_sign_in_id.clone(),
                    worker_id: worker_id.clone(),
                    account_id: account_id.clone(),
                    method: Some(method),
                    step,
                });
                if waits_for_page {
                    // As if the user signed in on the page.
                    let id = new_sign_in_id.clone();
                    applied.later.push((
                        Duration::from_secs(6),
                        Box::new(move |state: &mut ViewState| {
                            finish_sign_in(state, &id, &worker_id, &account_id)
                        }),
                    ));
                }
                applied.output = CommandOutput::SignInStarted(new_sign_in_id);
            }
            Command::SubmitSignInCode { sign_in_id, code } => {
                let sign_in = state
                    .sign_ins
                    .iter_mut()
                    .find(|s| s.id == sign_in_id)
                    .ok_or_else(not_found)?;
                if code.0.trim().is_empty() {
                    return Err(invalid("Paste the code that the sign-in page shows."));
                }
                let wrong = code.0.trim() == "wrong";
                sign_in.step = SignInStep::Verifying;
                let (worker_id, account_id) =
                    (sign_in.worker_id.clone(), sign_in.account_id.clone());
                applied.later.push((
                    Duration::from_millis(1_200),
                    Box::new(move |state: &mut ViewState| {
                        if wrong {
                            if let Some(s) = state.sign_ins.iter_mut().find(|s| s.id == sign_in_id) {
                                s.step = SignInStep::Failed {
                                    message: "Claude did not accept the code. Open the sign-in page again, copy the new code, then try again.".into(),
                                };
                            }
                        } else {
                            finish_sign_in(state, &sign_in_id, &worker_id, &account_id);
                        }
                    }),
                ));
            }
            Command::CancelSignIn { sign_in_id } => {
                let sign_in = state
                    .sign_ins
                    .iter_mut()
                    .find(|s| s.id == sign_in_id)
                    .ok_or_else(not_found)?;
                if !sign_in.step.is_final() {
                    sign_in.step = SignInStep::Cancelled;
                }
            }
            Command::DismissSignIn { sign_in_id } => {
                state.sign_ins.retain(|s| s.id != sign_in_id);
            }
            Command::CheckTools { worker_id } => {
                online_worker_mut(&mut state, &worker_id)?;
            }
            Command::InstallTool { worker_id, tool } => {
                let worker = online_worker_mut(&mut state, &worker_id)?;
                let row = worker
                    .tools
                    .iter_mut()
                    .find(|t| t.id == tool)
                    .ok_or_else(not_found)?;
                row.state = ToolState::Installing;
                row.detail = None;
                applied.later.push((
                    Duration::from_secs(2),
                    Box::new(move |state: &mut ViewState| {
                        if let Some(row) = state
                            .workers
                            .iter_mut()
                            .chain(state.session_host.iter_mut())
                            .filter(|w| w.id == worker_id)
                            .flat_map(|w| w.tools.iter_mut())
                            .find(|t| t.id == tool)
                        {
                            row.version = row
                                .latest
                                .take()
                                .or_else(|| row.version.clone())
                                .or_else(|| Some("1.0.0".into()));
                            row.state = ToolState::Ready;
                        }
                    }),
                ));
            }
            Command::LoadAcpRegistry => {
                state.acp_registry = AcpRegistryView {
                    load: RegistryLoad::Loaded,
                    agents: crate::mock::acp_agents(),
                };
            }
            // Files and uploads.
            command @ (Command::Attach { .. }
            | Command::RetryUpload { .. }
            | Command::RemoveUpload { .. }
            | Command::LoadChannelFiles { .. }
            | Command::MoveChannelFile { .. }
            | Command::DeleteChannelFile { .. }
            | Command::SaveToChannelFiles { .. }
            | Command::FetchBlob { .. }
            | Command::SaveBlob { .. }) => {
                applied.output = crate::mock_files::apply(
                    &mut state,
                    command,
                    &mut self.next_id,
                    &mut applied.later,
                )?;
            }
            // Settings > Hub and Settings > Clients.
            command @ (Command::LoadHubStatus
            | Command::BackUpNow
            | Command::UpdateBackupSettings(_)
            | Command::LoadClients
            | Command::RevokeClient { .. }
            | Command::ClientPairingCode
            | Command::CancelClientPairing) => {
                applied.output = crate::mock_admin::apply(&mut state, command, &mut applied.later)?;
            }
            // Settings > Proactivity.
            command @ (Command::LoadProactivityLog
            | Command::SetProactivity(_)
            | Command::SnoozeProactivity(_)
            | Command::CancelAllFollowUps { .. }
            | Command::SetChannelProactivity { .. }
            | Command::RateMessage { .. }) => {
                applied.output = crate::mock_proactivity::apply(&mut state, command)?;
            }
            // Settings > Voice and the draft card.
            command @ (Command::LoadVoice
            | Command::SetVoice(_)
            | Command::LearnVoice
            | Command::ResetVoice
            | Command::TryVoice { .. }
            | Command::EditVoiceGuide(_)
            | Command::VoiceFeedback { .. }
            | Command::EditApproval { .. }) => {
                applied.output = crate::mock_voice::apply(&mut state, command)?;
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
                applied.output =
                    crate::mock_routines::apply(&mut state, command, &mut self.next_id)?;
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
            | Command::ReorderGroups { .. }) => {
                applied.output = crate::mock_convo::apply(
                    &mut state,
                    command,
                    &mut self.next_id,
                    &mut applied.later,
                )?;
            }
            // Monid.
            command @ (Command::LoadMonidStatus
            | Command::SetMonidKey { .. }
            | Command::RemoveMonidKey
            | Command::UpdateMonidSettings { .. }
            | Command::SetChannelMonid { .. }
            | Command::SearchMonid { .. }
            | Command::LoadMonidCalls { .. }
            | Command::LoadMonidCall { .. }
            | Command::StopMonidCall { .. }
            | Command::EndAccess { .. }
            | Command::AnswerApprovalOption { .. }
            | Command::SetRoutineMonid { .. }) => {
                applied.output = crate::mock_monid::apply(&mut state, command)?;
            }
            // Connectors and skills.
            other => {
                applied.output = crate::mock_backend_ext::apply(
                    &mut state,
                    other,
                    &card_id,
                    &mut applied.later,
                )?;
            }
        }
        applied.reply_to = reply_to;
        Ok(applied)
    }
}

/// Tutti's own model of the assistant.
fn tutti_selection(state: &ViewState) -> Option<SelectionView> {
    state
        .tutti()
        .and_then(|c| c.coordinator_selection.as_ref())
        .map(|e| e.selection.clone())
}

/// Sets or takes away a channel's own model of the assistant, as the hub
/// does. A new Tutti model reaches every channel without its own.
fn set_coordinator_selection(
    state: &mut ViewState,
    channel_id: &str,
    chosen: Option<SelectionView>,
) {
    let is_tutti = state
        .channel(channel_id)
        .is_some_and(|c| c.kind == ChannelKind::Tutti);
    if is_tutti {
        for channel in &mut state.channels {
            let inherits = channel.kind != ChannelKind::Tutti
                && channel
                    .coordinator_selection
                    .as_ref()
                    .is_none_or(|e| e.source == SelectionSource::Tutti);
            if channel.id == channel_id {
                channel.coordinator_selection =
                    crate::picker::effective_selection(chosen.clone(), None);
            } else if inherits {
                channel.coordinator_selection =
                    crate::picker::effective_selection(None, chosen.clone());
            }
        }
        return;
    }
    let tutti = tutti_selection(state);
    if let Some(channel) = state.channels.iter_mut().find(|c| c.id == channel_id) {
        channel.coordinator_selection = crate::picker::effective_selection(chosen, tutti);
    }
}

fn invalid(message: &str) -> ClientError {
    ClientError::Hub {
        code: "invalid".into(),
        message: message.into(),
    }
}

/// Checks tags the way the hub does, with its `invalid_tag` answer.
fn checked_tags(tags: &[String]) -> Result<Vec<String>, ClientError> {
    consort_proto::model::normalize_tags(tags).map_err(|err| ClientError::Hub {
        code: "invalid_tag".into(),
        message: err.to_string(),
    })
}

/// A worker that can take an account command: the hub refuses one for an
/// offline worker with `worker_offline`.
fn online_worker_mut<'a>(
    state: &'a mut ViewState,
    worker_id: &str,
) -> Result<&'a mut WorkerView, ClientError> {
    let worker = worker_mut(state, worker_id)?;
    if !worker.online {
        return Err(ClientError::Hub {
            code: "worker_offline".into(),
            message: format!(
                "{} is offline. Try again when it is back online.",
                worker.name
            ),
        });
    }
    Ok(worker)
}

fn account_mut<'a>(
    worker: &'a mut WorkerView,
    account_id: &str,
) -> Result<&'a mut HarnessAccountView, ClientError> {
    worker
        .accounts
        .iter_mut()
        .find(|a| a.id == account_id)
        .ok_or_else(not_found)
}

fn set_account_status(account: &mut HarnessAccountView, status: AccountStatus, detail: String) {
    account.status = status;
    account.action = status.action();
    account.detail = detail;
    account.detail_code.clear();
    account.auth = match status {
        AccountStatus::SignedIn => AuthState::SignedIn,
        AccountStatus::Ready => AuthState::KeyFound,
        AccountStatus::NotSignedIn => AuthState::SignedOut,
        _ => AuthState::Unknown,
    };
}

/// Ends a sign-in as a success, and signs its account in.
pub(crate) fn finish_sign_in(
    state: &mut ViewState,
    sign_in_id: &str,
    worker_id: &str,
    account_id: &str,
) {
    let Some(sign_in) = state.sign_ins.iter_mut().find(|s| s.id == sign_in_id) else {
        return;
    };
    if sign_in.step.is_final() {
        return;
    }
    let email = "chris@example.com";
    sign_in.step = SignInStep::Succeeded {
        detail: Some(email.into()),
    };
    if let Some(account) = state
        .workers
        .iter_mut()
        .chain(state.session_host.iter_mut())
        .filter(|w| w.id == worker_id)
        .flat_map(|w| w.accounts.iter_mut())
        .find(|a| a.id == account_id)
    {
        account.email = Some(email.into());
        set_account_status(
            account,
            AccountStatus::SignedIn,
            format!("Signed in as {email}."),
        );
    }
}

fn not_found() -> ClientError {
    ClientError::Hub {
        code: "not_found".into(),
        message: "That item no longer exists. It may have been deleted on another client.".into(),
    }
}

fn timeline_mut<'a>(
    state: &'a mut ViewState,
    channel_id: &str,
) -> Result<&'a mut TimelineView, ClientError> {
    if !state.timelines.iter().any(|t| t.channel_id == channel_id) {
        state.timelines.push(TimelineView {
            channel_id: channel_id.to_owned(),
            ..TimelineView::default()
        });
    }
    state
        .timelines
        .iter_mut()
        .find(|t| t.channel_id == channel_id)
        .ok_or_else(not_found)
}

fn worker_mut<'a>(
    state: &'a mut ViewState,
    worker_id: &str,
) -> Result<&'a mut WorkerView, ClientError> {
    state
        .workers
        .iter_mut()
        .chain(state.session_host.iter_mut())
        .find(|w| w.id == worker_id)
        .ok_or_else(not_found)
}

/// Changes the title or text of a draft card, as the hub does: the first
/// change keeps the text that the assistant wrote.
fn edit_mock_draft(
    state: &mut ViewState,
    message_id: &str,
    title: Option<String>,
    body: Option<String>,
) -> Result<(), ClientError> {
    let timeline_drafts = state
        .timelines
        .iter_mut()
        .flat_map(|t| t.items.iter_mut())
        .filter_map(|item| match item {
            TimelineItem::Notice(n) => n.draft.as_mut(),
            _ => None,
        });
    let thread_drafts = state
        .threads
        .iter_mut()
        .flat_map(|t| t.posts.iter_mut())
        .flat_map(|p| p.bodies.iter_mut())
        .filter_map(|b| match b {
            PostBody::Notice { draft, .. } => draft.as_mut(),
            _ => None,
        });
    let draft = timeline_drafts
        .chain(thread_drafts)
        .find(|d| d.message_id == message_id)
        .ok_or_else(not_found)?;
    if let Some(title) = title {
        draft.title = title;
    }
    if let Some(body) = body
        && body != draft.body
    {
        draft
            .original_body
            .get_or_insert_with(|| draft.body.clone());
        draft.body = body;
        draft.edited = true;
    }
    Ok(())
}

fn set_status(state: &mut ViewState, task_id: &str, status: TaskStatus) -> Result<(), ClientError> {
    let task = state
        .tasks
        .iter_mut()
        .find(|t| t.id == task_id)
        .ok_or_else(not_found)?;
    task.status = status;
    task.updated_at = format::now_ms();
    task.age_label = "Now".into();
    Ok(())
}

pub(crate) fn for_each_approval(
    state: &mut ViewState,
    id: &str,
    mut f: impl FnMut(&mut ApprovalView),
) {
    for timeline in &mut state.timelines {
        for item in &mut timeline.items {
            match item {
                TimelineItem::TaskCard(card) => {
                    if let Some(approval) = card.approval.as_mut()
                        && approval.id == id
                    {
                        f(approval);
                    }
                }
                TimelineItem::Approval(card) if card.approval.id == id => f(&mut card.approval),
                _ => {}
            }
        }
    }
    for thread in &mut state.threads {
        for post in &mut thread.posts {
            for body in &mut post.bodies {
                if let PostBody::Approval(approval) = body
                    && approval.id == id
                {
                    f(approval);
                }
            }
        }
    }
}

/// After an answer: the task goes back to Working when nothing else waits.
fn settle(state: &mut ViewState, task_id: &str) {
    let Some(thread) = state.threads.iter_mut().find(|t| t.task_id == task_id) else {
        return;
    };
    let (mut decisions, mut approvals) = (0, 0);
    for post in &thread.posts {
        for body in &post.bodies {
            match body {
                PostBody::Decision(d) if d.state == RequestState::Pending => decisions += 1,
                PostBody::Approval(a) if a.state == RequestState::Pending => approvals += 1,
                _ => {}
            }
        }
    }
    thread.status_detail = waiting_detail(decisions, approvals);
    if decisions + approvals == 0
        && let Some(task) = state.tasks.iter_mut().find(|t| t.id == task_id)
        && task.status == TaskStatus::NeedsInput
    {
        task.status = TaskStatus::Working;
    }
}

const DEMO_REPLY: &str = "This is a demo with no hub, so no assistant reads your message. Connect Consort to a hub to talk to the assistant.";

impl Backend for MockBackend {
    fn handle(
        &mut self,
        command: Command,
        reply: crate::Reply,
        publish: &watch::Sender<Arc<ViewState>>,
    ) {
        // A send with uploads that still run waits for them, as on the hub.
        let waits_for = match &command {
            Command::SendMessage { attachments, .. } | Command::ReplyToTask { attachments, .. } => {
                Some(attachments.clone())
            }
            _ => None,
        }
        .filter(|ids| {
            !ids.is_empty()
                && self
                    .state
                    .lock()
                    .is_ok_and(|s| !crate::mock_files::uploads_settled(&s, ids))
        });
        if let Some(ids) = waits_for {
            let state = self.state.clone();
            let publish = publish.clone();
            let mut later = MockBackend {
                state: self.state.clone(),
                next_id: self.next_id + 1000,
            };
            tokio::spawn(async move {
                while state
                    .lock()
                    .is_ok_and(|s| !crate::mock_files::uploads_settled(&s, &ids))
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                let _ = reply.send(later.apply_and_publish(command, &publish));
            });
            return;
        }
        let _ = reply.send(self.apply_and_publish(command, publish));
    }
}

impl MockBackend {
    fn apply_and_publish(
        &mut self,
        command: Command,
        publish: &watch::Sender<Arc<ViewState>>,
    ) -> Result<CommandOutput, ClientError> {
        let Applied {
            reply_to,
            output,
            later,
        } = self.apply(command)?;
        publish_state(&self.state, publish);
        for (delay, change) in later {
            let state = self.state.clone();
            let publish = publish.clone();
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                if let Ok(mut s) = state.lock() {
                    change(&mut s);
                }
                publish_state(&state, &publish);
            });
        }
        if let Some(channel_id) = reply_to {
            let state = self.state.clone();
            let publish = publish.clone();
            let id = self.id("msg");
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(900)).await;
                let now = format::now_ms();
                if let Ok(mut s) = state.lock()
                    && let Some(timeline) =
                        s.timelines.iter_mut().find(|t| t.channel_id == channel_id)
                {
                    timeline.items.push(TimelineItem::Message(MessageView {
                        id,
                        channel_id,
                        speaker: Speaker::Consort,
                        time_label: format::time_label(now),
                        created_at: now,
                        body: DEMO_REPLY.into(),
                        ..MessageView::default()
                    }));
                }
                publish_state(&state, &publish);
            });
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Client, mock};

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }

    #[test]
    fn answering_both_requests_moves_the_task_to_working() {
        let client = Client::mock().expect("mock");
        let mut sub = client.subscribe();
        block_on(async {
            client
                .answer_approval(
                    "apr_0192f0c4-windows-build".into(),
                    ApprovalChoice::AllowOnce,
                )
                .await
                .expect("answered");
            let state = sub.next().await.expect("state");
            let task = state.task("tsk_0192f0c4-sayso-issues").expect("task");
            assert_eq!(
                task.status,
                TaskStatus::NeedsInput,
                "the decision still waits"
            );
            client
                .answer_decision("dec_0192f0c4-cpu-fallback".into(), Some(0), None)
                .await
                .expect("answered");
            let state = sub.next().await.expect("state");
            let task = state.task("tsk_0192f0c4-sayso-issues").expect("task");
            assert_eq!(task.status, TaskStatus::Working);
            assert_eq!(state.tasks_panel.need_you, 1);
        });
    }

    #[test]
    fn a_sent_message_shows_at_once_and_the_demo_answers() {
        let client = Client::mock().expect("mock");
        let mut sub = client.subscribe();
        let channel = mock::tutti_id().to_owned();
        block_on(async {
            let before = client
                .state()
                .timeline(&channel)
                .map_or(0, |t| t.items.len());
            client
                .send_message(channel.clone(), "Hello".into())
                .await
                .expect("sent");
            let state = sub.next().await.expect("state");
            assert_eq!(
                state.timeline(&channel).map(|t| t.items.len()),
                Some(before + 1)
            );
            let state = sub.next().await.expect("reply");
            assert_eq!(
                state.timeline(&channel).map(|t| t.items.len()),
                Some(before + 2)
            );
        });
    }

    #[test]
    fn tutti_cannot_be_archived() {
        let client = Client::mock().expect("mock");
        let err = block_on(client.archive_channel(mock::tutti_id().into())).expect_err("refused");
        assert!(matches!(err, ClientError::Hub { .. }));
    }

    #[test]
    fn an_archived_channel_moves_to_the_archived_list_and_comes_back() {
        let client = Client::mock().expect("mock");
        let state = client.state();
        let lights = state
            .channels
            .iter()
            .find(|c| c.name == "house-lights")
            .map(|c| c.id.clone())
            .expect("channel");
        block_on(async {
            client
                .archive_channel(lights.clone())
                .await
                .expect("archived");
            let state = client.state();
            assert!(state.channel(&lights).is_none());
            assert!(state.archived_channels.iter().any(|c| c.id == lights));
            client
                .unarchive_channel(lights.clone())
                .await
                .expect("back");
            let state = client.state();
            assert!(state.channel(&lights).is_some());
            assert!(state.archived_channels.iter().all(|c| c.id != lights));
        });
    }

    #[test]
    fn a_group_can_be_made_and_a_channel_can_leave_it() {
        let client = Client::mock().expect("mock");
        let sayso = client
            .state()
            .channels
            .iter()
            .find(|c| c.name == "sayso")
            .map(|c| c.id.clone())
            .expect("channel");
        block_on(async {
            client
                .create_group(" Work ".into(), Some(sayso.clone()))
                .await
                .expect("group");
            let state = client.state();
            let group = state
                .groups
                .iter()
                .find(|g| g.name == "Work")
                .expect("group");
            assert_eq!(
                state
                    .channel(&sayso)
                    .and_then(|c| c.group_id.clone())
                    .as_deref(),
                Some(group.id.as_str())
            );
            assert!(state.sidebar.groups.iter().any(|g| g.name == "Work"));
            client
                .move_channel(sayso.clone(), None)
                .await
                .expect("moved");
            let state = client.state();
            assert_eq!(state.channel(&sayso).and_then(|c| c.group_id.clone()), None);
            assert!(state.sidebar.ungrouped.iter().any(|c| c.id == sayso));
            let err = client
                .create_group("  ".into(), None)
                .await
                .expect_err("refused");
            assert!(err.to_string().contains("Enter a name for the group"));
        });
    }

    #[test]
    fn an_archived_task_leaves_the_lists() {
        let client = Client::mock().expect("mock");
        let failed = client
            .state()
            .tasks
            .iter()
            .find(|t| t.status == TaskStatus::Failed)
            .map(|t| t.id.clone())
            .expect("a failed task");
        block_on(async {
            client.archive_task(failed.clone()).await.expect("archived");
            let state = client.state();
            assert!(state.task(&failed).is_none());
            assert!(state.thread(&failed).is_none());
            assert!(
                state
                    .tasks_panel
                    .active
                    .iter()
                    .all(|s| !s.task_ids.contains(&failed))
            );
        });
    }

    #[test]
    fn a_message_for_no_channel_is_refused() {
        let client = Client::mock().expect("mock");
        let err =
            block_on(client.send_message(String::new(), "Hello".into())).expect_err("refused");
        assert_eq!(err, ClientError::NoChannel);
    }
}

#[cfg(test)]
mod pairing_tests {
    use crate::Client;

    #[test]
    fn a_pairing_code_shows_in_the_state() {
        let client = Client::mock().expect("mock");
        let mut sub = client.subscribe();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                client.worker_pairing_code().await.expect("code");
                let state = sub.next().await.expect("state");
                assert_eq!(
                    state.worker_pairing.as_ref().map(|p| p.code.as_str()),
                    Some("KQ7M-4XPD")
                );
            });
    }
}

#[cfg(test)]
mod account_tests {
    use crate::{Client, ClientError, NewAccount, SecretEnv, view::*};

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }

    async fn settled(
        client: &Client,
        until: impl Fn(&ViewState) -> bool,
    ) -> std::sync::Arc<ViewState> {
        let mut sub = client.subscribe();
        loop {
            let state = sub.current();
            if until(&state) {
                return state;
            }
            sub.next().await.expect("state");
        }
    }

    #[test]
    fn a_new_account_signs_in_with_a_code() {
        let client = Client::mock().expect("mock");
        block_on(async {
            let account_id = client
                .create_account(NewAccount {
                    worker_id: "wrk_bigboi".into(),
                    harness: "claude".into(),
                    label: "Claude Lab".into(),
                    ..NewAccount::default()
                })
                .await
                .expect("created");
            let state = client.state();
            let account = state
                .worker("wrk_bigboi")
                .and_then(|w| w.account(&account_id))
                .expect("account");
            assert_eq!(account.status, AccountStatus::NotSignedIn);
            assert_eq!(account.action, AccountAction::SignIn);
            assert_eq!(account.sign_in_methods[0], SignInChoice::ClaudePlan);
            let sign_in = client
                .start_sign_in(
                    "wrk_bigboi".into(),
                    account_id.clone(),
                    SignInChoice::ClaudePlan,
                )
                .await
                .expect("started");
            assert!(matches!(
                client.state().sign_in(&sign_in).map(|s| &s.step),
                Some(SignInStep::NeedsCode { .. })
            ));
            client
                .submit_sign_in_code(sign_in.clone(), "abc".into())
                .await
                .expect("sent");
            let state = settled(&client, |s| {
                s.sign_in(&sign_in).is_some_and(|s| s.step.is_final())
            })
            .await;
            assert_eq!(
                state.sign_in(&sign_in).map(|s| s.step.clone()),
                Some(SignInStep::Succeeded {
                    detail: Some("chris@example.com".into())
                })
            );
            let account = state
                .worker("wrk_bigboi")
                .and_then(|w| w.account(&account_id))
                .expect("account");
            assert_eq!(account.status, AccountStatus::SignedIn);
            assert_eq!(account.action, AccountAction::SignOut);
        });
    }

    #[test]
    fn an_api_key_account_is_ready_and_names_its_worker() {
        let client = Client::mock().expect("mock");
        block_on(async {
            let mut env = SecretEnv::new();
            env.insert("OPENAI_API_KEY", "sk-secret");
            let id = client
                .create_account(NewAccount {
                    worker_id: "wrk_macbook".into(),
                    harness: "codex".into(),
                    label: "Codex API".into(),
                    acp: None,
                    env,
                })
                .await
                .expect("created");
            let state = client.state();
            let account = state
                .worker("wrk_macbook")
                .and_then(|w| w.account(&id))
                .expect("account");
            assert_eq!(account.status, AccountStatus::Ready);
            assert_eq!(account.detail, "API key saved on MacBook Pro.");
            assert_eq!(account.secret_names, ["OPENAI_API_KEY"]);
        });
    }

    #[test]
    fn an_offline_worker_refuses_account_changes() {
        let client = Client::mock().expect("mock");
        let err =
            block_on(client.sign_out_account("wrk_truenas".into(), "acc_truenas_claude".into()))
                .expect_err("refused");
        assert!(matches!(&err, ClientError::Hub { code, .. } if code == "worker_offline"));
    }

    #[test]
    fn a_bad_tag_gets_the_invalid_tag_answer() {
        let client = Client::mock().expect("mock");
        let err = block_on(client.set_worker_tags("wrk_bigboi".into(), vec!["GPU box".into()]))
            .expect_err("refused");
        match err {
            ClientError::Hub { code, message } => {
                assert_eq!(code, "invalid_tag");
                assert!(message.contains("lowercase letters"), "{message}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_registry_loads_and_a_tool_installs() {
        let client = Client::mock().expect("mock");
        block_on(async {
            client.load_acp_registry().await.expect("loaded");
            let state = client.state();
            assert_eq!(state.acp_registry.load, RegistryLoad::Loaded);
            let found = state.acp_registry.search("goose");
            assert_eq!(found.len(), 1);
            assert!(matches!(
                &found[0].logo,
                ProviderLogo::Registry { svg: Some(_), .. }
            ));
            // The agents that a worker can install come first.
            assert!(
                state
                    .acp_registry
                    .search("")
                    .last()
                    .is_some_and(|a| !a.supported)
            );

            client
                .install_tool("wrk_session_host".into(), "codex".into())
                .await
                .expect("started");
            let tool = |s: &ViewState| {
                s.worker("wrk_session_host")
                    .and_then(|w| w.tools.iter().find(|t| t.id == "codex").cloned())
                    .expect("tool")
            };
            assert_eq!(tool(&client.state()).state, ToolState::Installing);
            let state = settled(&client, |s| tool(s).state == ToolState::Ready).await;
            assert_eq!(tool(&state).version.as_deref(), Some("0.165.0"));
        });
    }

    #[test]
    fn a_channel_keeps_its_tags() {
        let client = Client::mock().expect("mock");
        block_on(async {
            client
                .create_channel(crate::NewChannel {
                    name: "consort".into(),
                    tags: vec!["personal".into()],
                    ..crate::NewChannel::default()
                })
                .await
                .expect("created");
            let state = client.state();
            let channel = state
                .channels
                .iter()
                .find(|c| c.name == "consort")
                .expect("channel");
            assert_eq!(channel.tags, ["personal"]);
            assert_eq!(state.workers_with_tags(&channel.tags).len(), 3);
            let err = client
                .update_channel(
                    channel.id.clone(),
                    crate::ChannelChanges {
                        tags: Some(vec!["Bad Tag".into()]),
                        ..crate::ChannelChanges::default()
                    },
                )
                .await
                .expect_err("refused");
            assert!(matches!(err, ClientError::Hub { code, .. } if code == "invalid_tag"));
        });
    }
}

fn last_four(key: &str) -> String {
    let chars: Vec<char> = key.trim().chars().collect();
    chars[chars.len().saturating_sub(4)..].iter().collect()
}

/// "In use" follows the source of the command check.
fn refresh_endpoint_use(state: &mut ViewState) {
    use crate::command_check::CheckSourceKind;
    let choice = &state.command_check.choice;
    let active = match choice.kind {
        CheckSourceKind::Account => None,
        CheckSourceKind::Decision => choice.decision.as_ref(),
        CheckSourceKind::Chat => choice.chat.as_ref(),
    }
    .map(|c| c.endpoint_id.clone());
    for endpoint in &mut state.endpoints {
        endpoint.in_use = active.as_ref() == Some(&endpoint.id);
    }
}
