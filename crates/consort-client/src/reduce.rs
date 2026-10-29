//! The reducers: they turn the cache into view state. The rules that the
//! mock shares (sidebar glyphs, Tasks panel groups) live in [`crate::derive`].

use std::collections::BTreeMap;

use consort_proto::{
    model::{
        self, ApprovalDetails, ApprovalStatus, AuthState as ProtoAuth, Author, ChannelKind as PK,
        DecisionStatus, ItemPayload, MessageKind, RequestOptionKind, WorkerState,
    },
    status::TaskStatus as PS,
};

use crate::{
    cache::Cache,
    derive::{refresh, waiting_detail},
    format,
    view::*,
};

pub fn task_status(status: PS) -> TaskStatus {
    match status {
        PS::Scheduled => TaskStatus::Scheduled,
        PS::UpNext => TaskStatus::UpNext,
        PS::Working => TaskStatus::Working,
        PS::NeedsInput => TaskStatus::NeedsInput,
        PS::Review => TaskStatus::Review,
        PS::Failed => TaskStatus::Failed,
        PS::Resolved => TaskStatus::Resolved,
    }
}

fn isolation(i: model::Isolation) -> Isolation {
    match i {
        model::Isolation::InPlace => Isolation::InPlace,
        model::Isolation::Worktree => Isolation::Worktree,
        model::Isolation::Clone => Isolation::Clone,
    }
}

fn os(os: model::Os) -> Os {
    match os {
        model::Os::Macos => Os::MacOs,
        model::Os::Windows => Os::Windows,
        model::Os::Linux => Os::Linux,
    }
}

fn approval_mode(mode: model::ApprovalMode) -> ApprovalMode {
    match mode {
        model::ApprovalMode::Ask => ApprovalMode::Ask,
        model::ApprovalMode::AutoEdits => ApprovalMode::AutoEdits,
        model::ApprovalMode::Auto => ApprovalMode::Auto,
        model::ApprovalMode::FullAccess => ApprovalMode::FullAccess,
    }
}

fn selection_label(s: &model::ModelSelection) -> String {
    if s.model.is_empty() {
        s.account_label.clone()
    } else {
        format!("{}, {}", s.account_label, s.model)
    }
}

/// The choices of an approval, in the order the card shows them.
pub fn approval_choice(kind: RequestOptionKind) -> Option<ApprovalChoice> {
    match kind {
        RequestOptionKind::AllowOnce => Some(ApprovalChoice::AllowOnce),
        RequestOptionKind::AllowAlways => Some(ApprovalChoice::AllowForTask),
        RequestOptionKind::DenyOnce | RequestOptionKind::DenyAlways => Some(ApprovalChoice::Deny),
        RequestOptionKind::Choice => None,
    }
}

fn approval_view(a: &model::Approval) -> ApprovalView {
    let (folder, detail) = match &a.details {
        ApprovalDetails::Command { command, cwd } => (cwd.clone(), Some(command.clone())),
        ApprovalDetails::FileChange { paths, .. } => (None, Some(paths.join("\n"))),
        ApprovalDetails::Tool { tool, server, .. } => (
            None,
            Some(match server {
                Some(server) => format!("{server}: {tool}"),
                None => tool.clone(),
            }),
        ),
        _ => (None, None),
    };
    let mut choices: Vec<ApprovalChoice> = Vec::new();
    for option in &a.options {
        if let Some(choice) = approval_choice(option.kind)
            && !choices.contains(&choice)
        {
            choices.push(choice);
        }
    }
    if choices.is_empty() {
        choices = vec![ApprovalChoice::AllowOnce, ApprovalChoice::Deny];
    }
    choices.sort_by_key(|c| match c {
        ApprovalChoice::AllowOnce => 0,
        ApprovalChoice::AllowForTask => 1,
        ApprovalChoice::Deny => 2,
    });
    let answer = a
        .answer_option_id
        .as_deref()
        .and_then(|id| a.option(id))
        .and_then(|o| approval_choice(o.kind));
    ApprovalView {
        id: a.id.to_string(),
        task_id: a.task_id.map(|t| t.to_string()).unwrap_or_default(),
        summary: a.summary.clone(),
        folder,
        detail,
        choices,
        state: match a.status {
            ApprovalStatus::Pending => RequestState::Pending,
            ApprovalStatus::Allowed | ApprovalStatus::Denied => RequestState::Answered,
            ApprovalStatus::Expired | ApprovalStatus::Canceled => RequestState::Expired,
        },
        answer,
    }
}

fn decision_view(d: &model::Decision) -> DecisionView {
    DecisionView {
        id: d.id.to_string(),
        task_id: d.task_id.map(|t| t.to_string()),
        question: d.question.clone(),
        context: d.context.clone(),
        options: d
            .options
            .iter()
            .map(|o| DecisionOption {
                label: o.label.clone(),
                consequence: o.consequence.clone(),
            })
            .collect(),
        recommended: d.recommended_index.map(u32::from),
        allow_free_text: d.allow_free_text,
        state: match d.status {
            DecisionStatus::Pending => RequestState::Pending,
            DecisionStatus::Answered => RequestState::Answered,
            DecisionStatus::Expired | DecisionStatus::Canceled => RequestState::Expired,
        },
        chosen: d
            .answer
            .as_ref()
            .and_then(|a| a.option_index)
            .map(u32::from),
        answer_text: d.answer.as_ref().and_then(|a| a.text.clone()),
    }
}

fn speaker(m: &model::Message) -> Speaker {
    match m.author {
        Author::User => Speaker::You,
        Author::Assistant if m.author_task_id.is_some() => Speaker::Task,
        Author::Assistant => Speaker::Consort,
        Author::System => Speaker::System,
    }
}

/// Builds the whole view state from the cache. `now` is Unix milliseconds.
pub fn view_state(cache: &Cache, connection: Connection, user: UserView, now: i64) -> ViewState {
    let workers_by_id: BTreeMap<String, &model::Worker> =
        cache.workers.iter().map(|(k, v)| (k.clone(), v)).collect();
    let worker_name = |id: &consort_proto::ids::WorkerId| {
        workers_by_id
            .get(&id.to_string())
            .map(|w| w.name.clone())
            .unwrap_or_else(|| "a worker".to_owned())
    };
    let everything = cache.channels.values().find(|c| c.kind == PK::Everything);
    let fallback_selection = everything.and_then(|c| c.coordinator_selection.clone());

    // Channels, Everything first, then by group and position.
    let mut channels: Vec<&model::Channel> = cache
        .channels
        .values()
        .filter(|c| c.archived_at.is_none())
        .collect();
    channels.sort_by_key(|c| (c.kind != PK::Everything, c.position, c.name.clone()));
    let channel_views: Vec<ChannelView> = channels
        .iter()
        .map(|c| {
            let selection = c
                .coordinator_selection
                .clone()
                .or(fallback_selection.clone());
            let account = selection.as_ref().map(|s| s.account_label.clone());
            let worker = c
                .preferred_workers
                .iter()
                .filter_map(|id| workers_by_id.get(&id.to_string()))
                .find(|w| w.state == WorkerState::Online)
                .or_else(|| {
                    workers_by_id.values().find(|w| {
                        w.state == WorkerState::Online
                            && account.as_ref().is_some_and(|label| {
                                w.inventory.as_ref().is_some_and(|inv| {
                                    inv.accounts.iter().any(|a| &a.label == label)
                                })
                            })
                    })
                })
                .map(|w| w.name.clone());
            ChannelView {
                id: c.id.to_string(),
                kind: if c.kind == PK::Everything {
                    ChannelKind::Everything
                } else {
                    ChannelKind::Standard
                },
                name: c.name.clone(),
                purpose: c.purpose.clone(),
                instructions: c.instructions.clone(),
                icon: c.icon.clone().unwrap_or_else(|| {
                    if c.kind == PK::Everything {
                        "brace"
                    } else {
                        "hash"
                    }
                    .to_owned()
                }),
                group_id: c.group_id.map(|g| g.to_string()),
                coordinator_worker: worker,
                coordinator_account: account,
                repos: Vec::new(),
                preferred_workers: c.preferred_workers.iter().map(|w| w.to_string()).collect(),
                task_selection: c.task_selection.as_ref().map(selection_label),
                default_isolation: isolation(c.default_isolation),
            }
        })
        .collect();
    let channel_name = |id: &consort_proto::ids::ChannelId| {
        cache
            .channels
            .get(&id.to_string())
            .map(|c| c.name.clone())
            .unwrap_or_default()
    };

    let mut groups: Vec<&model::Group> = cache.groups.values().collect();
    groups.sort_by_key(|g| (g.position, g.name.clone()));

    // Tasks.
    let mut tasks: Vec<TaskView> = cache
        .tasks
        .values()
        .filter(|t| t.archived_at.is_none())
        .map(|t| {
            let status = task_status(t.status);
            let reason = match status {
                TaskStatus::Scheduled => t.start_at.map(|at| {
                    format!(
                        "Starts {} at {}",
                        format::day_label(at.as_millis(), now).to_lowercase_first(),
                        format::time_label(at.as_millis())
                    )
                }),
                _ => t.status_reason.clone(),
            };
            TaskView {
                id: t.id.to_string(),
                channel_id: t.channel_id.to_string(),
                channel_name: channel_name(&t.channel_id),
                title: t.title.clone(),
                status,
                meta: String::new(),
                worker_name: t.worker_id.as_ref().map(worker_name),
                account_label: t.model_selection.as_ref().map(|s| s.account_label.clone()),
                status_reason: reason,
                age_label: format::age_label(t.updated_at.as_millis(), now),
                updated_at: t.updated_at.as_millis(),
                card_message_id: t.card_message_id.to_string(),
            }
        })
        .collect();
    tasks.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
    let task_by_card: BTreeMap<String, &model::Task> = cache
        .tasks
        .values()
        .map(|t| (t.card_message_id.to_string(), t))
        .collect();

    // Timelines.
    let mut by_channel: BTreeMap<String, Vec<&model::Message>> = BTreeMap::new();
    for m in cache.messages.values() {
        if m.deleted_at.is_none() && m.shows_in_timeline() {
            by_channel
                .entry(m.channel_id.to_string())
                .or_default()
                .push(m);
        }
    }
    let mut timelines = Vec::new();
    let mut unread: BTreeMap<String, u32> = BTreeMap::new();
    for channel in &channel_views {
        let mut messages = by_channel.remove(&channel.id).unwrap_or_default();
        messages.sort_by_key(|m| (m.created_at, m.id.to_string()));
        let marker = cache
            .read_markers
            .get(&channel.id)
            .map(|m| m.last_read_message_id.to_string());
        let mut after_marker = marker.is_none();
        let mut count = 0u32;
        let mut items = Vec::new();
        let mut last_day = String::new();
        let mut push_day = |items: &mut Vec<TimelineItem>, at: i64| {
            let key = format::day_key(at);
            if key != last_day {
                items.push(TimelineItem::Day(DayView {
                    key: format!("day-{}-{key}", channel.id),
                    label: format::day_label(at, now),
                }));
                last_day = key;
            }
        };
        for m in &messages {
            let at = m.created_at.as_millis();
            if after_marker && m.author != Author::User {
                count += 1;
            }
            if marker.as_deref() == Some(m.id.to_string().as_str()) {
                after_marker = true;
            }
            push_day(&mut items, at);
            if m.kind == MessageKind::TaskCard
                && let Some(task) = task_by_card.get(&m.id.to_string())
            {
                let status = task_status(task.status);
                let approval = cache
                    .approvals
                    .values()
                    .filter(|a| a.task_id == Some(task.id) && a.status == ApprovalStatus::Pending)
                    .min_by_key(|a| a.created_at)
                    .map(approval_view);
                let worker = task
                    .worker_id
                    .as_ref()
                    .and_then(|id| workers_by_id.get(&id.to_string()))
                    .map(|w| WorkerChip {
                        name: w.name.clone(),
                        online: w.state == WorkerState::Online,
                        account: task
                            .model_selection
                            .as_ref()
                            .map(|s| s.account_label.clone())
                            .unwrap_or_default(),
                    });
                let replies = m.reply_count;
                let summary = match replies {
                    0 => status.label().to_owned(),
                    1 => format!("{}, 1 reply", status.label()),
                    n => format!("{}, {n} replies", status.label()),
                };
                items.push(TimelineItem::TaskCard(TaskCardView {
                    message_id: m.id.to_string(),
                    task_id: task.id.to_string(),
                    time_label: format::time_label(at),
                    title: task.title.clone(),
                    status,
                    channel_name: (task.channel_id.to_string() != channel.id)
                        .then(|| channel_name(&task.channel_id)),
                    worker,
                    approval,
                    summary,
                    reply_count: replies,
                    latest: match status {
                        TaskStatus::Review => task.review_summary.clone(),
                        TaskStatus::Failed | TaskStatus::UpNext => task.status_reason.clone(),
                        _ => None,
                    },
                }));
            } else {
                items.push(TimelineItem::Message(MessageView {
                    id: m.id.to_string(),
                    channel_id: channel.id.clone(),
                    speaker: speaker(m),
                    time_label: format::time_label(at),
                    created_at: at,
                    title: None,
                    body: m.body.clone(),
                    items: Vec::new(),
                    edited: m.edited_at.is_some(),
                    reply_count: m.reply_count,
                    pending: false,
                }));
            }
        }
        for p in cache
            .pending
            .iter()
            .filter(|p| p.channel_id == channel.id && p.thread_root_id.is_none())
        {
            push_day(&mut items, p.created_at);
            items.push(TimelineItem::Message(MessageView {
                id: format!("pending-{}", p.client_message_id),
                channel_id: channel.id.clone(),
                speaker: Speaker::You,
                time_label: format::time_label(p.created_at),
                created_at: p.created_at,
                body: p.body.clone(),
                pending: true,
                ..MessageView::default()
            }));
        }
        unread.insert(channel.id.clone(), count);
        timelines.push(TimelineView {
            channel_id: channel.id.clone(),
            items,
            has_older: messages.len() >= consort_proto::client_api::SNAPSHOT_MESSAGE_LIMIT,
        });
    }

    // Task threads.
    let mut threads = Vec::new();
    for task in cache.tasks.values() {
        let root = task.card_message_id;
        let mut replies: Vec<&model::Message> = cache
            .messages
            .values()
            .filter(|m| m.thread_root_id == Some(root) && m.deleted_at.is_none())
            .collect();
        replies.sort_by_key(|m| m.created_at);
        let mut posts = Vec::new();
        let (mut decisions, mut approvals) = (0, 0);
        for m in replies {
            let body = match m.kind {
                MessageKind::Approval => cache
                    .approvals
                    .values()
                    .find(|a| a.message_id == m.id)
                    .map(|a| {
                        if a.status == ApprovalStatus::Pending {
                            approvals += 1;
                        }
                        PostBody::Approval(approval_view(a))
                    }),
                MessageKind::Decision => cache
                    .decisions
                    .values()
                    .find(|d| d.message_id == m.id)
                    .map(|d| {
                        if d.status == DecisionStatus::Pending {
                            decisions += 1;
                        }
                        PostBody::Decision(decision_view(d))
                    }),
                _ => Some(PostBody::Text(m.body.clone())),
            };
            if let Some(body) = body {
                posts.push(ThreadPost {
                    id: m.id.to_string(),
                    speaker: speaker(m),
                    time_label: format::time_label(m.created_at.as_millis()),
                    bodies: vec![body],
                });
            }
        }
        for p in cache
            .pending
            .iter()
            .filter(|p| p.thread_root_id.as_deref() == Some(root.to_string().as_str()))
        {
            posts.push(ThreadPost {
                id: format!("pending-{}", p.client_message_id),
                speaker: Speaker::You,
                time_label: format::time_label(p.created_at),
                bodies: vec![PostBody::Text(p.body.clone())],
            });
        }
        // Folds the session's commands and file changes into one line, as
        // the first post.
        if let Some(session) = task.current_session_id {
            let (mut commands, mut files) = (0, std::collections::BTreeSet::new());
            let mut lines = Vec::new();
            for item in cache
                .session_items
                .values()
                .filter(|i| i.session_id == session)
            {
                match &item.payload {
                    ItemPayload::Command(c) => {
                        commands += 1;
                        lines.push(c.command.clone());
                    }
                    ItemPayload::FileChange(f) => {
                        for change in &f.changes {
                            files.insert(change.path.clone());
                            lines.push(format!("Changed {}", change.path));
                        }
                    }
                    _ => {}
                }
            }
            if commands + files.len() > 0 {
                let summary = match (commands, files.len()) {
                    (0, f) => format!("Changed {f} {}", plural(f, "file", "files")),
                    (c, 0) => format!("Ran {c} {}", plural(c, "command", "commands")),
                    (c, f) => format!(
                        "Ran {c} {} and changed {f} {}",
                        plural(c, "command", "commands"),
                        plural(f, "file", "files")
                    ),
                };
                posts.insert(
                    0,
                    ThreadPost {
                        id: format!("activity-{session}"),
                        speaker: Speaker::Task,
                        time_label: format::time_label(task.created_at.as_millis()),
                        bodies: vec![PostBody::Activity {
                            summary,
                            items: lines,
                        }],
                    },
                );
            }
        }
        let worker = task
            .worker_id
            .as_ref()
            .and_then(|id| workers_by_id.get(&id.to_string()));
        let channel_isolation = cache
            .channels
            .get(&task.channel_id.to_string())
            .map(|c| c.default_isolation)
            .unwrap_or_default();
        threads.push(TaskThreadView {
            task_id: task.id.to_string(),
            title: task.title.clone(),
            status: task_status(task.status),
            status_detail: waiting_detail(decisions, approvals),
            placement: worker.map(|w| PlacementView {
                worker_name: w.name.clone(),
                worker_os: os(w.os).label().to_owned(),
                worker_online: w.state == WorkerState::Online,
                harness: task
                    .model_selection
                    .as_ref()
                    .map(selection_label)
                    .unwrap_or_else(|| "The channel's default".to_owned()),
                isolation: isolation(task.placement.isolation.unwrap_or(channel_isolation)),
                path: String::new(),
                branch: None,
            }),
            posts,
        });
    }

    // Workers.
    let mut workers: Vec<WorkerView> = cache
        .workers
        .values()
        .filter(|w| w.revoked_at.is_none())
        .map(|w| {
            let online = w.state == WorkerState::Online;
            let accounts: Vec<HarnessAccountView> = w
                .inventory
                .as_ref()
                .map(|inv| {
                    inv.accounts
                        .iter()
                        .map(|a| HarnessAccountView {
                            id: a.account_id.to_string(),
                            label: a.label.clone(),
                            detail: [
                                Some(match &a.harness_version {
                                    Some(v) => format!("{} {v}.", a.harness),
                                    None => format!("{}.", a.harness),
                                }),
                                a.auth_detail.as_ref().map(|d| format!("{d}.")),
                            ]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(" "),
                            detail_code: Vec::new(),
                            auth: match a.auth_state {
                                ProtoAuth::SignedIn => AuthState::SignedIn,
                                ProtoAuth::SignedOut => AuthState::SignedOut,
                                ProtoAuth::Error => AuthState::Error,
                                ProtoAuth::Unknown => AuthState::Unknown,
                            },
                        })
                        .collect()
                })
                .unwrap_or_default();
            let running = cache
                .tasks
                .values()
                .filter(|t| t.worker_id == Some(w.id) && t.status == PS::Working)
                .count();
            let os_label = os(w.os).label();
            let mut list = vec![os_label.to_owned()];
            list.push(format!(
                "{} {}",
                accounts.len(),
                plural(accounts.len(), "account", "accounts")
            ));
            if running > 0 {
                list.push(format!(
                    "{running} {} running",
                    plural(running, "task", "tasks")
                ));
            }
            let last_seen = w.last_seen_at.map(|t| format::time_label(t.as_millis()));
            let policy = w.inventory.as_ref().map(|i| &i.policy);
            WorkerView {
                id: w.id.to_string(),
                name: w.name.clone(),
                os: os(w.os),
                online,
                paused: w.paused,
                sidebar_detail: if online {
                    os_label.to_owned()
                } else {
                    "Offline".to_owned()
                },
                list_detail: if online {
                    list.join(", ")
                } else {
                    match &last_seen {
                        Some(t) => format!("Offline since {t}"),
                        None => "Offline".to_owned(),
                    }
                },
                status_line: if online {
                    format!("Online. {os_label}, {}, worker {}.", w.arch, w.version)
                } else {
                    match &last_seen {
                        Some(t) => format!(
                            "Offline since {t}. {os_label}, {}, worker {}.",
                            w.arch, w.version
                        ),
                        None => format!("Offline. {os_label}, {}, worker {}.", w.arch, w.version),
                    }
                },
                labels: w.labels.clone(),
                accounts,
                policy: PolicyView {
                    max_approval_mode: policy
                        .map(|p| approval_mode(p.max_approval_mode))
                        .unwrap_or_default(),
                    allowed_roots: policy.map(|p| p.allowed_roots.clone()).unwrap_or_default(),
                    allow_shell: policy.is_some_and(|p| p.allow_shell),
                    shell_name: if w.os == model::Os::Windows {
                        "PowerShell"
                    } else {
                        "the shell"
                    }
                    .to_owned(),
                },
            }
        })
        .collect();
    workers.sort_by_key(|w| (!w.online, w.name.to_lowercase()));

    let mut selections: Vec<String> = Vec::new();
    for w in cache.workers.values() {
        for a in w.inventory.iter().flat_map(|i| i.accounts.iter()) {
            for m in &a.models {
                let label = format!("{}, {}", a.label, m.name);
                if !selections.contains(&label) {
                    selections.push(label);
                }
            }
            if a.models.is_empty() && !selections.contains(&a.label) {
                selections.push(a.label.clone());
            }
        }
    }

    let mut state = ViewState {
        connection,
        user,
        sidebar: SidebarView::default(),
        channels: channel_views,
        groups: groups
            .iter()
            .map(|g| GroupView {
                id: g.id.to_string(),
                name: g.name.clone(),
            })
            .collect(),
        timelines,
        tasks,
        threads,
        tasks_panel: TasksPanelView::default(),
        workers,
        selections,
        worker_pairing: None,
    };
    // Collapsed groups and unread counts come from the hub, so seed the
    // sidebar before the shared rules rebuild it.
    state.sidebar.groups = groups
        .iter()
        .map(|g| SidebarGroup {
            id: g.id.to_string(),
            name: g.name.clone(),
            collapsed: g.collapsed,
            channels: Vec::new(),
        })
        .collect();
    state.sidebar.ungrouped = state
        .channels
        .iter()
        .map(|c| SidebarChannel {
            id: c.id.clone(),
            unread: unread.get(&c.id).copied().unwrap_or(0),
            ..SidebarChannel::default()
        })
        .collect();
    if let Some(e) = state.everything() {
        state.sidebar.everything = SidebarChannel {
            id: e.id.clone(),
            unread: unread.get(&e.id).copied().unwrap_or(0),
            ..SidebarChannel::default()
        };
    }
    refresh(&mut state);
    state
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 { one } else { many }
}

trait LowerFirst {
    fn to_lowercase_first(&self) -> String;
}

impl LowerFirst for String {
    /// "Today" becomes "today", for use inside a sentence. Day names keep
    /// their capital.
    fn to_lowercase_first(&self) -> String {
        match self.as_str() {
            "Today" => "today".to_owned(),
            "Yesterday" => "yesterday".to_owned(),
            _ => self.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::{
        client_api::{EntityPayload, FeedEntry},
        ids::{
            ApprovalId, ChannelId, DecisionId, GroupId, MessageId, RequestId, SessionId, TaskId,
        },
        model::{
            Approval, ApprovalDetails, ApprovalMode, ApprovalStatus, Author, Channel, ChannelKind,
            Decision, DecisionOption as PO, DecisionStatus, Group, Isolation, Message, MessageKind,
            PlacementRequest, ReadMarker, RequestOption, RequestOptionKind, Task, TaskCreator,
        },
        status::TaskStatus as PS,
        time::Timestamp,
    };

    use super::*;
    use crate::cache::PendingSend;

    const NOW: i64 = 1_793_300_000_000;

    fn channel(id: ChannelId, kind: ChannelKind, name: &str, group: Option<GroupId>) -> Channel {
        Channel {
            id,
            kind,
            name: name.into(),
            icon: None,
            color: None,
            group_id: group,
            position: 0,
            purpose: String::new(),
            instructions: String::new(),
            coordinator_selection: None,
            task_selection: None,
            preferred_workers: vec![],
            repo_ids: vec![],
            default_isolation: Isolation::Worktree,
            default_approval_mode: ApprovalMode::AutoEdits,
            memory_enabled: true,
            archived_at: None,
            created_at: Timestamp::from_millis(NOW - 100_000),
            updated_at: Timestamp::from_millis(NOW - 100_000),
        }
    }

    fn message(
        id: MessageId,
        channel_id: ChannelId,
        author: Author,
        kind: MessageKind,
        root: Option<MessageId>,
        at: i64,
    ) -> Message {
        Message {
            id,
            channel_id,
            thread_root_id: root,
            author,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            kind,
            body: format!("message at {at}"),
            attachments: vec![],
            in_timeline: false,
            client_message_id: None,
            reply_count: 0,
            last_reply_at: None,
            reactions: vec![],
            edited_at: None,
            deleted_at: None,
            created_at: Timestamp::from_millis(at),
        }
    }

    struct World {
        cache: Cache,
        everything: ChannelId,
        sayso: ChannelId,
        task: TaskId,
        card: MessageId,
    }

    fn world() -> World {
        let mut cache = Cache::default();
        let group = GroupId::new();
        let everything = ChannelId::new();
        let sayso = ChannelId::new();
        cache.groups.insert(
            group.to_string(),
            Group {
                id: group,
                name: "Personal projects".into(),
                position: 0,
                collapsed: false,
            },
        );
        for c in [
            channel(sayso, ChannelKind::Standard, "sayso", Some(group)),
            channel(everything, ChannelKind::Everything, "Everything", None),
        ] {
            cache.channels.insert(c.id.to_string(), c);
        }
        let task = TaskId::new();
        let card = MessageId::new();
        let session = SessionId::new();
        let first = message(
            MessageId::new(),
            sayso,
            Author::Assistant,
            MessageKind::Text,
            None,
            NOW - 50_000,
        );
        let card_msg = message(
            card,
            sayso,
            Author::Assistant,
            MessageKind::TaskCard,
            None,
            NOW - 40_000,
        );
        let approval_msg = message(
            MessageId::new(),
            sayso,
            Author::Assistant,
            MessageKind::Approval,
            Some(card),
            NOW - 30_000,
        );
        let decision_msg = message(
            MessageId::new(),
            sayso,
            Author::Assistant,
            MessageKind::Decision,
            Some(card),
            NOW - 20_000,
        );
        for m in [
            first.clone(),
            card_msg,
            approval_msg.clone(),
            decision_msg.clone(),
        ] {
            cache.messages.insert(m.id.to_string(), m);
        }
        cache.read_markers.insert(
            sayso.to_string(),
            ReadMarker {
                channel_id: sayso,
                thread_root_id: None,
                last_read_message_id: first.id,
            },
        );
        cache.tasks.insert(
            task.to_string(),
            Task {
                id: task,
                channel_id: sayso,
                title: "Go through open Sayso issues".into(),
                brief: String::new(),
                status: PS::NeedsInput,
                status_reason: None,
                created_by: TaskCreator::Assistant,
                card_message_id: card,
                model_selection: None,
                placement: PlacementRequest::default(),
                approval_mode: ApprovalMode::Ask,
                start_at: None,
                current_session_id: Some(session),
                worker_id: None,
                review_summary: None,
                resolved_at: None,
                archived_at: None,
                created_at: Timestamp::from_millis(NOW - 40_000),
                updated_at: Timestamp::from_millis(NOW - 120_000),
            },
        );
        let approval = ApprovalId::new();
        cache.approvals.insert(
            approval.to_string(),
            Approval {
                id: approval,
                channel_id: sayso,
                session_id: session,
                task_id: Some(task),
                message_id: approval_msg.id,
                request_id: RequestId::new("r"),
                summary: "Codex wants to run a command".into(),
                details: ApprovalDetails::Command {
                    command: "cargo build".into(),
                    cwd: Some("C:\\Code".into()),
                },
                options: vec![
                    RequestOption {
                        id: "d".into(),
                        label: "Deny".into(),
                        kind: RequestOptionKind::DenyOnce,
                        description: None,
                    },
                    RequestOption {
                        id: "a".into(),
                        label: "Allow".into(),
                        kind: RequestOptionKind::AllowOnce,
                        description: None,
                    },
                ],
                status: ApprovalStatus::Pending,
                answer_option_id: None,
                answered_at: None,
                answered_by_client_id: None,
                created_at: Timestamp::from_millis(NOW - 30_000),
            },
        );
        let decision = DecisionId::new();
        cache.decisions.insert(
            decision.to_string(),
            Decision {
                id: decision,
                channel_id: sayso,
                task_id: Some(task),
                session_id: session,
                message_id: decision_msg.id,
                request_id: None,
                question: "Ship now?".into(),
                context: None,
                options: vec![
                    PO {
                        label: "Yes".into(),
                        consequence: "It ships.".into(),
                        id: None,
                    },
                    PO {
                        label: "No".into(),
                        consequence: "It waits.".into(),
                        id: None,
                    },
                ],
                recommended_index: Some(1),
                allow_free_text: true,
                status: DecisionStatus::Pending,
                answer: None,
                answered_at: None,
                created_at: Timestamp::from_millis(NOW - 20_000),
            },
        );
        World {
            cache,
            everything,
            sayso,
            task,
            card,
        }
    }

    fn view(cache: &Cache) -> ViewState {
        view_state(cache, Connection::Online, UserView::default(), NOW)
    }

    #[test]
    fn everything_comes_first_and_groups_hold_their_channels() {
        let w = world();
        let state = view(&w.cache);
        assert_eq!(state.channels[0].id, w.everything.to_string());
        assert_eq!(state.channels[0].icon, "brace");
        assert_eq!(state.sidebar.groups[0].channels[0].name, "sayso");
        assert_eq!(
            state.sidebar.groups[0].channels[0].status,
            Some(TaskStatus::NeedsInput)
        );
    }

    #[test]
    fn a_task_card_shows_its_pending_approval_inline() {
        let w = world();
        let state = view(&w.cache);
        let timeline = state.timeline(&w.sayso.to_string()).expect("timeline");
        assert!(matches!(timeline.items[0], TimelineItem::Day(_)));
        let card = timeline
            .items
            .iter()
            .find_map(|i| match i {
                TimelineItem::TaskCard(c) => Some(c),
                _ => None,
            })
            .expect("card");
        assert_eq!(card.task_id, w.task.to_string());
        let approval = card.approval.as_ref().expect("inline approval");
        assert_eq!(
            approval.choices,
            vec![ApprovalChoice::AllowOnce, ApprovalChoice::Deny]
        );
        assert_eq!(approval.folder.as_deref(), Some("C:\\Code"));
        // Thread replies are not in the timeline.
        assert_eq!(timeline.items.len(), 3);
    }

    #[test]
    fn the_thread_holds_the_approval_and_the_decision() {
        let w = world();
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        assert_eq!(thread.status_detail, "1 decision, 1 approval");
        assert!(matches!(thread.posts[0].bodies[0], PostBody::Approval(_)));
        match &thread.posts[1].bodies[0] {
            PostBody::Decision(d) => {
                assert_eq!(d.recommended, Some(1));
                assert_eq!(d.state, RequestState::Pending);
            }
            other => panic!("not a decision: {other:?}"),
        }
        assert_eq!(state.tasks_panel.need_you, 1);
        assert_eq!(state.tasks[0].meta, "Needs input. sayso");
        assert_eq!(state.tasks[0].age_label, "2 min");
    }

    #[test]
    fn unread_counts_follow_the_read_marker() {
        let w = world();
        let state = view(&w.cache);
        let row = &state.sidebar.groups[0].channels[0];
        // The task card comes after the marker. Thread replies do not count.
        assert_eq!(row.unread, 1);
        assert!(row.attention);
    }

    #[test]
    fn a_pending_send_shows_until_the_hub_stores_it() {
        let mut w = world();
        w.cache.pending.push(PendingSend {
            client_message_id: "c-1".into(),
            channel_id: w.everything.to_string(),
            thread_root_id: None,
            body: "Hi".into(),
            created_at: NOW,
        });
        let state = view(&w.cache);
        let items = &state
            .timeline(&w.everything.to_string())
            .expect("timeline")
            .items;
        assert!(
            matches!(items.last(), Some(TimelineItem::Message(m)) if m.pending && m.body == "Hi")
        );

        let mut stored = message(
            MessageId::new(),
            w.everything,
            Author::User,
            MessageKind::Text,
            None,
            NOW,
        );
        stored.body = "Hi".into();
        stored.client_message_id = Some("c-1".into());
        assert!(w.cache.apply(FeedEntry::upsert(
            1,
            Timestamp::from_millis(NOW),
            EntityPayload::Message(stored)
        )));
        let state = view(&w.cache);
        let items = &state
            .timeline(&w.everything.to_string())
            .expect("timeline")
            .items;
        let mine: Vec<_> = items
            .iter()
            .filter(|i| matches!(i, TimelineItem::Message(m) if m.body == "Hi"))
            .collect();
        assert_eq!(mine.len(), 1, "no duplicate after the hub stored it");
        assert!(
            matches!(mine[0], TimelineItem::Message(m) if !m.pending && m.speaker == Speaker::You)
        );
    }

    #[test]
    fn old_feed_entries_are_ignored_and_deletes_apply() {
        let mut w = world();
        w.cache.seq = 10;
        let late = message(
            MessageId::new(),
            w.sayso,
            Author::User,
            MessageKind::Text,
            None,
            NOW,
        );
        assert!(!w.cache.apply(FeedEntry::upsert(
            9,
            Timestamp::from_millis(NOW),
            EntityPayload::Message(late)
        )));
        assert!(w.cache.apply(FeedEntry::delete(
            11,
            Timestamp::from_millis(NOW),
            consort_proto::client_api::EntityKind::Task,
            w.task.to_string(),
            Some(w.sayso),
        )));
        let state = view(&w.cache);
        assert!(state.task(&w.task.to_string()).is_none());
        let _ = w.card;
    }
}
