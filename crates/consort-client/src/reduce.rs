//! The reducers: they turn the cache into view state. The rules that the
//! mock shares (sidebar glyphs, Tasks panel groups) live in [`crate::derive`].

use std::collections::BTreeMap;

use consort_proto::{
    ids::{MessageId, TaskId},
    model::{
        self, ApprovalDetails, ApprovalStatus, AuthState as ProtoAuth, Author, ChannelKind as PK,
        DecisionStatus, MessageKind, RequestOptionKind, WorkerState,
    },
    status::TaskStatus as PS,
};

use crate::{
    cache::{Cache, ItemSummary},
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

/// A tool name without the `mcp__<server>__` prefix that harnesses add, and
/// the server named in that prefix.
fn split_tool_name(name: &str) -> (Option<&str>, &str) {
    match name
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
    {
        Some((server, tool)) => (Some(server), tool),
        None => (None, name),
    }
}

/// The line of an approval card whose approval row is gone (answered or
/// canceled before this client loaded it): made from the message body.
fn closed_summary(body: &str, in_task: bool) -> String {
    let body = body.trim();
    if body.starts_with("mcp__") {
        let (server, tool) = split_tool_name(body);
        let who = if in_task { "The task" } else { "Consort" };
        return match server {
            Some(server) => format!("{who} wanted to use {tool} from {server}"),
            None => format!("{who} wanted to use {tool}"),
        };
    }
    if body.is_empty() {
        "A request for your approval".to_owned()
    } else {
        body.to_owned()
    }
}

fn closed_approval(m: &model::Message) -> ApprovalView {
    ApprovalView {
        id: format!("closed-{}", m.id),
        task_id: m.author_task_id.map(|t| t.to_string()),
        summary: closed_summary(&m.body, m.author_task_id.is_some()),
        folder: None,
        detail: None,
        choices: Vec::new(),
        state: RequestState::Closed,
        answer: None,
    }
}

fn closed_decision(m: &model::Message) -> DecisionView {
    DecisionView {
        id: format!("closed-{}", m.id),
        task_id: m.author_task_id.map(|t| t.to_string()),
        question: m.body.clone(),
        context: None,
        options: Vec::new(),
        recommended: None,
        allow_free_text: false,
        state: RequestState::Closed,
        chosen: None,
        answer_text: None,
    }
}

/// A word that names a file by its full path.
fn is_path(word: &str) -> bool {
    word.len() > 1
        && (word.starts_with('/')
            || word.starts_with('~')
            || word.get(1..3) == Some(":\\")
            || word.contains('\\'))
}

/// The last part of a path.
fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

/// A file change line with each full path cut to its file name, such as
/// "Write hello.txt". The detail block keeps the full paths.
fn file_change_summary(summary: &str, paths: &[String]) -> String {
    let words: Vec<&str> = summary.split_whitespace().collect();
    let mut out: Vec<String> = words
        .iter()
        .map(|w| {
            if is_path(w) {
                file_name(w).to_owned()
            } else {
                (*w).to_owned()
            }
        })
        .collect();
    if out.is_empty() {
        out = match paths {
            [one] => vec!["Change".to_owned(), file_name(one).to_owned()],
            many => vec![format!("Change {} files", many.len())],
        };
    }
    out.join(" ")
}

/// The line on an approval card. A tool approval reads "Consort wants to
/// use post_message from hub", never a raw `mcp__hub__post_message`.
fn approval_summary(a: &model::Approval) -> String {
    if let ApprovalDetails::FileChange { paths } = &a.details {
        return file_change_summary(&a.summary, paths);
    }
    let ApprovalDetails::Tool { tool, server, .. } = &a.details else {
        return a.summary.clone();
    };
    let raw = a.summary.trim();
    let readable = !raw.is_empty() && !raw.starts_with("mcp__") && raw != tool;
    if readable {
        return a.summary.clone();
    }
    let (prefix_server, name) = split_tool_name(tool);
    let who = if a.task_id.is_some() {
        "The task"
    } else {
        "Consort"
    };
    match server.as_deref().or(prefix_server) {
        Some(server) => format!("{who} wants to use {name} from {server}"),
        None => format!("{who} wants to use {name}"),
    }
}

/// The folder and the detail block of an approval. The detail block holds
/// the real action: the command, the files, the tool with its server and
/// arguments, or the network target. The summary line is text that the agent
/// wrote, so the user must be able to check it against the detail.
fn approval_detail(details: &ApprovalDetails) -> (Option<String>, Option<String>) {
    match details {
        ApprovalDetails::Command { command, cwd } => (cwd.clone(), Some(command.clone())),
        ApprovalDetails::FileChange { paths, .. } => (None, Some(paths.join("\n"))),
        ApprovalDetails::Tool {
            server,
            tool,
            arguments,
        } => {
            let (prefix_server, name) = split_tool_name(tool);
            let mut lines = vec![format!("Tool: {name}")];
            if let Some(server) = server.as_deref().or(prefix_server) {
                lines.push(format!("Server: {server}"));
            }
            let arguments = match arguments {
                serde_json::Value::Null => None,
                serde_json::Value::Object(map) if map.is_empty() => None,
                value => serde_json::to_string_pretty(value).ok(),
            };
            let mut text = lines.join("\n");
            if let Some(arguments) = arguments {
                text.push_str("\n\n");
                text.push_str(&arguments);
            }
            (None, Some(text))
        }
        ApprovalDetails::Network { target } => (None, Some(format!("Network: {target}"))),
        ApprovalDetails::Other { text } => (
            None,
            text.as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned),
        ),
    }
}

fn approval_view(a: &model::Approval) -> ApprovalView {
    let (folder, detail) = approval_detail(&a.details);
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
        task_id: a.task_id.map(|t| t.to_string()),
        summary: approval_summary(a),
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

/// The kind and the text of a notice message. The hub writes the body as
/// the JSON `{"kind": "...", "text": "..."}` (DECISIONS row 66). A body that
/// does not have a text is shown as it is, with an empty kind, so no
/// notice is lost.
pub fn notice_parts(body: &str) -> (String, String) {
    #[derive(serde::Deserialize)]
    struct Parsed {
        #[serde(default)]
        kind: String,
        text: String,
    }
    match serde_json::from_str::<Parsed>(body) {
        Ok(parsed) if !parsed.text.trim().is_empty() => (parsed.kind, parsed.text),
        _ => (String::new(), body.to_owned()),
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

    // Indexes, so a timeline of many messages does not scan every approval,
    // decision and reply for each one.
    let approval_by_message: BTreeMap<MessageId, &model::Approval> = cache
        .approvals
        .values()
        .map(|a| (a.message_id, a))
        .collect();
    let decision_by_message: BTreeMap<MessageId, &model::Decision> = cache
        .decisions
        .values()
        .map(|d| (d.message_id, d))
        .collect();
    // The oldest pending approval of each task, for the inline card.
    let mut first_pending: BTreeMap<TaskId, &model::Approval> = BTreeMap::new();
    for a in cache
        .approvals
        .values()
        .filter(|a| a.status == ApprovalStatus::Pending)
    {
        if let Some(task) = a.task_id {
            let slot = first_pending.entry(task).or_insert(a);
            if a.created_at < slot.created_at {
                *slot = a;
            }
        }
    }
    let mut replies_by_root: BTreeMap<MessageId, Vec<&model::Message>> = BTreeMap::new();
    for m in cache.messages.values() {
        if let Some(root) = m.thread_root_id
            && m.deleted_at.is_none()
        {
            replies_by_root.entry(root).or_default().push(m);
        }
    }

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
        messages.sort_by_key(|m| (m.created_at, m.id));
        // Where the read marker sits in time. A marker whose message is not
        // loaded (it is older than the newest page) sits before every
        // loaded message, so the count starts at the oldest one.
        let marker_at = cache
            .read_markers
            .get(&channel.id)
            .and_then(|m| cache.messages.get(&m.last_read_message_id.to_string()))
            .map(|m| (m.created_at, m.id));
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
            let after_marker = marker_at.is_none_or(|at| (m.created_at, m.id) > at);
            // A system notice is not something to read: it never counts.
            if after_marker && m.author != Author::User && m.kind != MessageKind::Notice {
                count += 1;
            }
            push_day(&mut items, at);
            if m.kind == MessageKind::TaskCard
                && let Some(task) = task_by_card.get(&m.id.to_string())
            {
                let status = task_status(task.status);
                let approval = first_pending.get(&task.id).copied().map(approval_view);
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
            } else if m.kind == MessageKind::Approval {
                // Snapshots hold only pending approvals, so an answered one
                // may have just its message left.
                let approval = approval_by_message
                    .get(&m.id)
                    .copied()
                    .map(approval_view)
                    .unwrap_or_else(|| closed_approval(m));
                items.push(TimelineItem::Approval(ApprovalCardView {
                    message_id: m.id.to_string(),
                    speaker: speaker(m),
                    time_label: format::time_label(at),
                    approval,
                }));
            } else if m.kind == MessageKind::Decision {
                let decision = decision_by_message
                    .get(&m.id)
                    .copied()
                    .map(decision_view)
                    .unwrap_or_else(|| closed_decision(m));
                items.push(TimelineItem::Decision(DecisionCardView {
                    message_id: m.id.to_string(),
                    speaker: speaker(m),
                    time_label: format::time_label(at),
                    decision,
                }));
            } else if m.kind == MessageKind::Notice {
                let (kind, text) = notice_parts(&m.body);
                items.push(TimelineItem::Notice(NoticeView {
                    message_id: m.id.to_string(),
                    time_label: format::time_label(at),
                    kind,
                    text,
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
            has_older: cache.has_older.get(&channel.id).copied().unwrap_or(false),
            loaded: cache.channels_loaded.contains(&channel.id),
        });
    }

    // Task threads.
    let mut threads = Vec::new();
    // An archived task has no thread to open.
    for task in cache.tasks.values().filter(|t| t.archived_at.is_none()) {
        let root = task.card_message_id;
        let mut replies: Vec<&model::Message> =
            replies_by_root.get(&root).cloned().unwrap_or_default();
        replies.sort_by_key(|m| m.created_at);
        let mut posts = Vec::new();
        let (mut decisions, mut approvals) = (0, 0);
        for m in replies {
            let body = match m.kind {
                MessageKind::Approval => Some(match approval_by_message.get(&m.id).copied() {
                    Some(a) => {
                        if a.status == ApprovalStatus::Pending {
                            approvals += 1;
                        }
                        PostBody::Approval(approval_view(a))
                    }
                    None => PostBody::Approval(closed_approval(m)),
                }),
                MessageKind::Decision => Some(match decision_by_message.get(&m.id).copied() {
                    Some(d) => {
                        if d.status == DecisionStatus::Pending {
                            decisions += 1;
                        }
                        PostBody::Decision(decision_view(d))
                    }
                    None => PostBody::Decision(closed_decision(m)),
                }),
                MessageKind::Notice => {
                    let (kind, text) = notice_parts(&m.body);
                    Some(PostBody::Notice { kind, text })
                }
                _ => Some(PostBody::Text(m.body.clone())),
            };
            if let Some(body) = body {
                posts.push(ThreadPost {
                    id: m.id.to_string(),
                    speaker: speaker(m),
                    time_label: format::time_label(m.created_at.as_millis()),
                    bodies: vec![body],
                    pending: false,
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
                pending: true,
            });
        }
        // Folds the session's commands and file changes into one line, as
        // the first post.
        if let Some(session) = task.current_session_id {
            let (mut commands, mut files) = (0, std::collections::BTreeSet::new());
            let mut lines = Vec::new();
            for item in cache
                .session_items
                .get(&session.to_string())
                .into_iter()
                .flat_map(|items| items.values())
            {
                match item {
                    ItemSummary::Command(command) => {
                        commands += 1;
                        lines.push(command.clone());
                    }
                    ItemSummary::Files(paths) => {
                        for path in paths {
                            files.insert(path.clone());
                            lines.push(format!("Changed {path}"));
                        }
                    }
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
                        pending: false,
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
            reason: match task_status(task.status) {
                TaskStatus::Failed | TaskStatus::UpNext => task.status_reason.clone(),
                _ => None,
            },
            placement: worker.map(|w| PlacementView {
                worker_name: w.name.clone(),
                worker_os: os(w.os).label().to_owned(),
                worker_online: w.state == WorkerState::Online,
                harness: task_harness(cache, task, w),
                isolation: isolation(task.placement.isolation.unwrap_or(channel_isolation)),
                path: String::new(),
                branch: None,
                repo: task_repo(cache, task),
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
                        .map(|a| {
                            let (detail, detail_code) = account_detail(
                                &a.harness.to_string(),
                                a.harness_version.as_deref(),
                                a.auth_detail.as_deref(),
                            );
                            HarnessAccountView {
                                id: a.account_id.to_string(),
                                label: a.label.clone(),
                                detail,
                                detail_code,
                                auth: match a.auth_state {
                                    ProtoAuth::SignedIn => AuthState::SignedIn,
                                    ProtoAuth::SignedOut => AuthState::SignedOut,
                                    ProtoAuth::Error => AuthState::Error,
                                    ProtoAuth::Unknown => AuthState::Unknown,
                                },
                            }
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
        send_failures: cache
            .send_failures
            .iter()
            .map(|f| SendFailureView {
                id: f.id.clone(),
                channel_id: f.channel_id.clone(),
                task_id: f
                    .thread_root_id
                    .as_ref()
                    .and_then(|root| task_by_card.get(root).map(|t| t.id.to_string())),
                channel_name: cache
                    .channels
                    .get(&f.channel_id)
                    .map(|c| c.name.clone())
                    .unwrap_or_default(),
                body: f.body.clone(),
                message: format!(
                    "{} Put your text back in the message box, then send it again.",
                    sentence(&f.message)
                ),
            })
            .collect(),
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

/// The detail line of a harness account: the harness and its version, then
/// what the worker said about the sign-in. Words in backticks come back as
/// code words, without the backticks.
fn account_detail(
    harness: &str,
    version: Option<&str>,
    auth_detail: Option<&str>,
) -> (String, Vec<String>) {
    let head = match version {
        Some(v) => format!("{harness} {v}"),
        None => harness.to_owned(),
    };
    let text = [Some(head.as_str()), auth_detail]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(sentence)
        .collect::<Vec<_>>()
        .join(" ");
    let mut plain = String::with_capacity(text.len());
    let mut code = Vec::new();
    for (i, part) in text.split('`').enumerate() {
        if i % 2 == 1 && !part.is_empty() {
            code.push(part.to_owned());
        }
        plain.push_str(part);
    }
    (plain, code)
}

/// The text as a sentence: a period goes on the end only when the text
/// does not already end with punctuation.
pub(crate) fn sentence(text: &str) -> String {
    let text = text.trim_end();
    let stop = text
        .trim_end_matches(['`', '"', '\'', ')', ']'])
        .ends_with(['.', '!', '?', ':', ';', '\u{2026}']);
    if stop {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

/// The account and model of a task, best known first: the task's own
/// selection, its channel's default, the Everything default, and, once the
/// task runs on a worker with one signed-in account, that account.
fn task_harness(cache: &Cache, task: &model::Task, worker: &model::Worker) -> String {
    let channel = cache.channels.get(&task.channel_id.to_string());
    let everything = cache.channels.values().find(|c| c.kind == PK::Everything);
    let chosen = task
        .model_selection
        .as_ref()
        .or(channel.and_then(|c| c.task_selection.as_ref()))
        .or(everything.and_then(|c| c.task_selection.as_ref()));
    if let Some(selection) = chosen {
        return selection_label(selection);
    }
    if task.current_session_id.is_some() {
        let signed_in: Vec<&model::HarnessAccount> = worker
            .inventory
            .iter()
            .flat_map(|i| i.accounts.iter())
            .filter(|a| a.enabled && a.auth_state == ProtoAuth::SignedIn)
            .collect();
        if let [only] = signed_in.as_slice() {
            return only.label.clone();
        }
    }
    "The channel's default".to_owned()
}

/// The repo a task's workspace comes from: the task's own repos, else its
/// channel's.
fn task_repo(cache: &Cache, task: &model::Task) -> Option<String> {
    let channel = cache.channels.get(&task.channel_id.to_string());
    let ids = task
        .placement
        .repos
        .clone()
        .or_else(|| channel.map(|c| c.repo_ids.clone()))
        .unwrap_or_default();
    ids.first().map(|id| {
        cache
            .repos
            .get(&id.to_string())
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "a repo".to_owned())
    })
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

    /// What the hub stores when Claude Code asks to use a hub tool in a
    /// coordinator turn: a top-level approval message with no task.
    fn coordinator_approval(w: &mut World) -> ApprovalId {
        let mut message = message(
            MessageId::new(),
            w.everything,
            Author::Assistant,
            MessageKind::Approval,
            None,
            NOW - 1_000,
        );
        message.body = "mcp__hub__post_message".into();
        let id = ApprovalId::new();
        let option = |id: &str, label: &str, kind| RequestOption {
            id: id.into(),
            label: label.into(),
            kind,
            description: None,
        };
        w.cache.approvals.insert(
            id.to_string(),
            Approval {
                id,
                channel_id: w.everything,
                session_id: SessionId::new(),
                task_id: None,
                message_id: message.id,
                request_id: RequestId::new("tool-1"),
                summary: "mcp__hub__post_message".into(),
                details: ApprovalDetails::Tool {
                    server: Some("hub".into()),
                    tool: "post_message".into(),
                    arguments: serde_json::json!({ "body": "Good morning" }),
                },
                options: vec![
                    option("yes", "Yes", RequestOptionKind::AllowOnce),
                    option(
                        "always",
                        "Yes, and don't ask again",
                        RequestOptionKind::AllowAlways,
                    ),
                    option("no", "No", RequestOptionKind::DenyOnce),
                ],
                status: ApprovalStatus::Pending,
                answer_option_id: None,
                answered_at: None,
                answered_by_client_id: None,
                created_at: Timestamp::from_millis(NOW - 1_000),
            },
        );
        w.cache.messages.insert(message.id.to_string(), message);
        id
    }

    #[test]
    fn a_coordinator_approval_is_a_card_in_the_timeline() {
        let mut w = world();
        let id = coordinator_approval(&mut w);
        let state = view(&w.cache);
        let items = &state
            .timeline(&w.everything.to_string())
            .expect("timeline")
            .items;
        let card = items
            .iter()
            .find_map(|i| match i {
                TimelineItem::Approval(card) => Some(card),
                _ => None,
            })
            .expect("an approval card, not a plain message");
        assert!(
            !items
                .iter()
                .any(|i| matches!(i, TimelineItem::Message(m) if m.body.contains("mcp__"))),
            "the raw tool name is not a message"
        );
        let approval = &card.approval;
        assert_eq!(approval.id, id.to_string());
        assert_eq!(approval.task_id, None);
        assert_eq!(
            approval.summary,
            "Consort wants to use post_message from hub"
        );
        assert!(
            approval
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("\"body\": \"Good morning\""))
        );
        assert_eq!(
            approval.choices,
            vec![
                ApprovalChoice::AllowOnce,
                ApprovalChoice::AllowForTask,
                ApprovalChoice::Deny
            ]
        );
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowForTask),
            "Always allow"
        );
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowOnce),
            "Allow once"
        );
        assert_eq!(card.speaker, Speaker::Consort);
        // Everything waits for the user.
        assert_eq!(
            state.sidebar.everything.status,
            Some(TaskStatus::NeedsInput)
        );
        assert!(state.sidebar.everything.attention);
        // The answer goes out by approval ID, with no task.
        let stored = w.cache.approvals.get(&id.to_string()).expect("stored");
        assert_eq!(
            crate::hub::option_for(stored, ApprovalChoice::AllowForTask).as_deref(),
            Some("always")
        );
        assert_eq!(
            crate::hub::option_for(stored, ApprovalChoice::Deny).as_deref(),
            Some("no")
        );
    }

    #[test]
    fn a_raw_tool_name_never_becomes_the_summary() {
        let mut w = world();
        let id = coordinator_approval(&mut w);
        if let Some(a) = w.cache.approvals.get_mut(&id.to_string()) {
            a.details = ApprovalDetails::Tool {
                server: None,
                tool: "mcp__github__create_issue".into(),
                arguments: serde_json::Value::Null,
            };
        }
        let state = view(&w.cache);
        let summary = state
            .timeline(&w.everything.to_string())
            .and_then(|t| {
                t.items.iter().find_map(|i| match i {
                    TimelineItem::Approval(card) => Some(card.approval.summary.clone()),
                    _ => None,
                })
            })
            .expect("card");
        assert_eq!(summary, "Consort wants to use create_issue from github");
    }

    #[test]
    fn a_task_approval_keeps_its_label_and_its_summary() {
        let w = world();
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        let PostBody::Approval(approval) = &thread.posts[0].bodies[0] else {
            panic!("not an approval");
        };
        assert_eq!(
            approval.task_id.as_deref(),
            Some(w.task.to_string().as_str())
        );
        assert_eq!(
            approval.choice_label(ApprovalChoice::AllowForTask),
            "Allow for this task"
        );
        assert_eq!(approval.summary, "Codex wants to run a command");
    }

    #[test]
    fn a_channel_decision_is_a_card_and_marks_the_channel() {
        let mut w = world();
        // The sayso task no longer waits, so only the decision can mark it.
        if let Some(task) = w.cache.tasks.get_mut(&w.task.to_string()) {
            task.status = PS::Working;
        }
        w.cache.approvals.clear();
        w.cache.decisions.clear();
        let msg = message(
            MessageId::new(),
            w.sayso,
            Author::Assistant,
            MessageKind::Decision,
            None,
            NOW - 500,
        );
        let id = DecisionId::new();
        let option = |label: &str, consequence: &str| PO {
            label: label.into(),
            consequence: consequence.into(),
            id: None,
        };
        w.cache.decisions.insert(
            id.to_string(),
            Decision {
                id,
                channel_id: w.sayso,
                task_id: None,
                session_id: SessionId::new(),
                message_id: msg.id,
                request_id: None,
                question: "Which build ships first?".into(),
                context: Some("Both pass.".into()),
                options: vec![
                    option("x86_64", "Most users get it today."),
                    option("arm64", "Mac users get it today."),
                ],
                recommended_index: Some(0),
                allow_free_text: true,
                status: DecisionStatus::Pending,
                answer: None,
                answered_at: None,
                created_at: Timestamp::from_millis(NOW - 500),
            },
        );
        w.cache.messages.insert(msg.id.to_string(), msg);
        let state = view(&w.cache);
        let items = &state
            .timeline(&w.sayso.to_string())
            .expect("timeline")
            .items;
        let card = items
            .iter()
            .find_map(|i| match i {
                TimelineItem::Decision(card) => Some(card),
                _ => None,
            })
            .expect("a decision card");
        assert_eq!(card.decision.question, "Which build ships first?");
        assert_eq!(card.decision.task_id, None);
        assert_eq!(card.decision.recommended, Some(0));
        let row = &state.sidebar.groups[0].channels[0];
        assert_eq!(row.status, Some(TaskStatus::NeedsInput));
        assert!(row.attention);
        // Once answered, the channel shows its task's glyph again.
        let mut answered = w.cache.clone();
        if let Some(d) = answered.decisions.get_mut(&id.to_string()) {
            d.status = DecisionStatus::Answered;
        }
        let state = view(&answered);
        assert_eq!(
            state.sidebar.groups[0].channels[0].status,
            Some(TaskStatus::Working)
        );
    }

    #[test]
    fn an_answered_request_with_no_row_shows_as_closed() {
        let mut w = world();
        let id = coordinator_approval(&mut w);
        // After a restart the snapshot holds only pending approvals.
        w.cache.approvals.remove(&id.to_string());
        let mut decision = message(
            MessageId::new(),
            w.everything,
            Author::Assistant,
            MessageKind::Decision,
            None,
            NOW,
        );
        decision.body = "Ship now?".into();
        w.cache.messages.insert(decision.id.to_string(), decision);
        let state = view(&w.cache);
        let items = &state
            .timeline(&w.everything.to_string())
            .expect("timeline")
            .items;
        let approval = items
            .iter()
            .find_map(|i| match i {
                TimelineItem::Approval(c) => Some(&c.approval),
                _ => None,
            })
            .expect("a closed approval card");
        assert_eq!(approval.state, RequestState::Closed);
        assert_eq!(
            approval.summary,
            "Consort wanted to use post_message from hub"
        );
        assert!(approval.choices.is_empty());
        let decision = items
            .iter()
            .find_map(|i| match i {
                TimelineItem::Decision(c) => Some(&c.decision),
                _ => None,
            })
            .expect("a closed decision card");
        assert_eq!(decision.state, RequestState::Closed);
        assert_eq!(decision.question, "Ship now?");
        // Closed requests do not wait for the user.
        assert_eq!(state.sidebar.everything.status, None);
    }

    #[test]
    fn an_account_detail_ends_with_one_period_and_keeps_code_words() {
        assert_eq!(
            account_detail(
                "codex",
                Some("0.5.1"),
                Some("Run `codex login` on this machine.")
            ),
            (
                "codex 0.5.1. Run codex login on this machine.".to_owned(),
                vec!["codex login".to_owned()]
            )
        );
        assert_eq!(
            account_detail("opencode", None, Some("Run `opencode auth list`")).0,
            "opencode. Run opencode auth list."
        );
        assert_eq!(sentence("Signed in (Pro)"), "Signed in (Pro).");
        assert_eq!(sentence("Is it signed in?"), "Is it signed in?");
        assert_eq!(sentence("See the \"docs.\""), "See the \"docs.\"");
    }

    #[test]
    fn a_file_change_summary_names_the_file_and_keeps_the_path() {
        let mut w = world();
        let path = "/private/tmp/consort-e2e-data/worker/workspaces/tsk_1/hello.txt";
        if let Some(a) = w.cache.approvals.values_mut().next() {
            a.summary = format!("Write {path}");
            a.details = ApprovalDetails::FileChange {
                paths: vec![path.into()],
            };
        }
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        let PostBody::Approval(approval) = &thread.posts[0].bodies[0] else {
            panic!("not an approval");
        };
        assert_eq!(approval.summary, "Write hello.txt");
        assert_eq!(approval.detail.as_deref(), Some(path));
        assert_eq!(
            file_change_summary("", &["C:\\Code\\a.rs".into()]),
            "Change a.rs"
        );
    }

    #[test]
    fn the_placement_names_the_repo_or_a_new_folder_and_the_harness() {
        let mut w = world();
        let worker = consort_proto::model::Worker {
            id: consort_proto::ids::WorkerId::new(),
            name: "bigboi".into(),
            hostname: "bigboi".into(),
            os: consort_proto::model::Os::Windows,
            arch: "x64".into(),
            public_key: consort_proto::auth::PublicKey::from_bytes([1; 32]),
            version: "0.1.0".into(),
            protocol_version: 1,
            labels: vec![],
            paused: false,
            state: consort_proto::model::WorkerState::Online,
            last_seen_at: None,
            inventory: None,
            paired_at: Timestamp::from_millis(NOW),
            revoked_at: None,
        };
        let worker_id = worker.id;
        w.cache.workers.insert(worker_id.to_string(), worker);
        if let Some(task) = w.cache.tasks.get_mut(&w.task.to_string()) {
            task.worker_id = Some(worker_id);
        }
        let state = view(&w.cache);
        let placement = state
            .thread(&w.task.to_string())
            .and_then(|t| t.placement.clone())
            .expect("placement");
        assert_eq!(placement.repo, None, "no repo: a new empty folder");
        assert_eq!(placement.harness, "The channel's default");

        if let Some(task) = w.cache.tasks.get_mut(&w.task.to_string()) {
            task.model_selection = Some(consort_proto::model::ModelSelection {
                account_label: "Claude Personal".into(),
                model: "Opus 5.5".into(),
                options: Default::default(),
            });
        }
        let state = view(&w.cache);
        let placement = state
            .thread(&w.task.to_string())
            .and_then(|t| t.placement.clone())
            .expect("placement");
        assert_eq!(placement.harness, "Claude Personal, Opus 5.5");
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

    /// Finding 11: with the read marker older than the loaded page, five
    /// unread messages counted as zero.
    #[test]
    fn unread_counts_from_the_oldest_loaded_message_when_the_marker_is_older() {
        let mut cache = Cache::default();
        let everything = ChannelId::new();
        let c = channel(everything, ChannelKind::Everything, "Everything", None);
        cache.channels.insert(c.id.to_string(), c);
        for i in 0..5 {
            let m = message(
                MessageId::new(),
                everything,
                Author::Assistant,
                MessageKind::Text,
                None,
                NOW - 10_000 + i,
            );
            cache.messages.insert(m.id.to_string(), m);
        }
        let mine = message(
            MessageId::new(),
            everything,
            Author::User,
            MessageKind::Text,
            None,
            NOW - 9_000,
        );
        cache.messages.insert(mine.id.to_string(), mine);
        // The marker names a message that this client has not loaded.
        cache.read_markers.insert(
            everything.to_string(),
            ReadMarker {
                channel_id: everything,
                thread_root_id: None,
                last_read_message_id: MessageId::new(),
            },
        );
        let state = view(&cache);
        assert_eq!(state.sidebar.everything.unread, 5);
    }

    #[test]
    fn unread_counts_only_the_messages_after_a_marker_that_was_deleted() {
        let mut w = world();
        let ids: Vec<MessageId> = (0..4)
            .map(|i| {
                let m = message(
                    MessageId::new(),
                    w.everything,
                    Author::Assistant,
                    MessageKind::Text,
                    None,
                    NOW - 8_000 + i * 1_000,
                );
                let id = m.id;
                w.cache.messages.insert(id.to_string(), m);
                id
            })
            .collect();
        // The second message is the marker, and it was deleted since.
        if let Some(m) = w.cache.messages.get_mut(&ids[1].to_string()) {
            m.deleted_at = Some(Timestamp::from_millis(NOW));
        }
        w.cache.read_markers.insert(
            w.everything.to_string(),
            ReadMarker {
                channel_id: w.everything,
                thread_root_id: None,
                last_read_message_id: ids[1],
            },
        );
        let state = view(&w.cache);
        assert_eq!(state.sidebar.everything.unread, 2);
    }

    /// Finding 3: deleting a channel left its tasks, messages, approvals and
    /// the rest in the cache, and the Tasks panel still showed the tasks.
    #[test]
    fn a_channel_delete_drops_everything_that_belongs_to_the_channel() {
        let mut w = world();
        // A running command in the task's session, and a message that waits.
        let session = w
            .cache
            .tasks
            .get(&w.task.to_string())
            .and_then(|t| t.current_session_id);
        let session = session.expect("session");
        let item: consort_proto::model::SessionItem = serde_json::from_value(serde_json::json!({
            "id": consort_proto::ids::ItemId::new(),
            "session_id": session,
            "turn_id": "turn-1",
            "kind": "command",
            "payload": {"command": "cargo build", "cwd": null, "output": "x".repeat(10_000), "exit_code": null, "duration_ms": null},
            "status": "in_progress",
            "created_at": NOW,
            "updated_at": NOW
        }))
        .expect("item");
        assert!(w.cache.apply(FeedEntry::upsert(
            1,
            Timestamp::from_millis(NOW),
            EntityPayload::SessionItem(item)
        )));
        w.cache.pending.push(PendingSend {
            client_message_id: "c-1".into(),
            channel_id: w.sayso.to_string(),
            thread_root_id: None,
            body: "Hi".into(),
            created_at: NOW,
        });
        w.cache.has_older.insert(w.sayso.to_string(), true);
        w.cache.channels_loaded.insert(w.sayso.to_string());
        w.cache.loaded_threads.insert(w.card.to_string());
        assert!(w.cache.apply(FeedEntry::delete(
            2,
            Timestamp::from_millis(NOW),
            consort_proto::client_api::EntityKind::Channel,
            w.sayso.to_string(),
            Some(w.sayso),
        )));
        let c = &w.cache;
        assert_eq!(c.channels.len(), 1, "only Everything is left");
        assert!(c.messages.is_empty(), "{:?}", c.messages.len());
        assert!(c.tasks.is_empty());
        assert!(c.approvals.is_empty());
        assert!(c.decisions.is_empty());
        assert!(c.read_markers.is_empty());
        assert!(c.session_items.is_empty());
        assert!(c.pending.is_empty());
        assert!(c.has_older.is_empty());
        assert!(c.channels_loaded.is_empty());
        assert!(c.loaded_threads.is_empty());
        let state = view(&w.cache);
        assert!(state.tasks.is_empty());
        assert_eq!(state.tasks_panel.need_you, 0);
        assert!(state.timeline(&w.sayso.to_string()).is_none());
    }

    /// A session item keeps a command line and not the output.
    #[test]
    fn a_session_item_keeps_only_what_the_view_reads() {
        let mut w = world();
        let session = w
            .cache
            .tasks
            .get(&w.task.to_string())
            .and_then(|t| t.current_session_id)
            .expect("session");
        let item: consort_proto::model::SessionItem = serde_json::from_value(serde_json::json!({
            "id": consort_proto::ids::ItemId::new(),
            "session_id": session,
            "turn_id": "turn-1",
            "kind": "command",
            "payload": {"command": "cargo test", "cwd": null, "output": "x".repeat(10_000), "exit_code": 0, "duration_ms": 5},
            "status": "completed",
            "created_at": NOW,
            "updated_at": NOW
        }))
        .expect("item");
        w.cache.apply(FeedEntry::upsert(
            1,
            Timestamp::from_millis(NOW),
            EntityPayload::SessionItem(item),
        ));
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        let PostBody::Activity { summary, items } = &thread.posts[0].bodies[0] else {
            panic!("no activity line");
        };
        assert_eq!(summary, "Ran 1 command");
        assert_eq!(items, &vec!["cargo test".to_owned()]);
    }

    /// Finding 14: the detail block names the real action, whatever the
    /// summary says.
    #[test]
    fn the_detail_block_names_the_tool_the_server_and_the_network_target() {
        let mut w = world();
        let id = coordinator_approval(&mut w);
        let detail = |w: &World| {
            view(&w.cache)
                .timeline(&w.everything.to_string())
                .and_then(|t| {
                    t.items.iter().find_map(|i| match i {
                        TimelineItem::Approval(card) => Some(card.approval.clone()),
                        _ => None,
                    })
                })
                .expect("card")
        };
        // A tool with a readable, agent-written summary.
        if let Some(a) = w.cache.approvals.get_mut(&id.to_string()) {
            a.summary = "Look up one harmless record".into();
            a.details = ApprovalDetails::Tool {
                server: Some("payments".into()),
                tool: "mcp__payments__refund_all".into(),
                arguments: serde_json::json!({"scope": "everyone"}),
            };
        }
        let card = detail(&w);
        assert_eq!(card.summary, "Look up one harmless record");
        let text = card.detail.expect("detail");
        assert!(text.contains("Tool: refund_all"), "{text}");
        assert!(text.contains("Server: payments"), "{text}");
        assert!(text.contains("\"scope\": \"everyone\""), "{text}");

        if let Some(a) = w.cache.approvals.get_mut(&id.to_string()) {
            a.summary = "Fetch the docs".into();
            a.details = ApprovalDetails::Network {
                target: "evil.example.com".into(),
            };
        }
        let card = detail(&w);
        assert_eq!(card.summary, "Fetch the docs");
        assert_eq!(card.detail.as_deref(), Some("Network: evil.example.com"));

        if let Some(a) = w.cache.approvals.get_mut(&id.to_string()) {
            a.details = ApprovalDetails::Other {
                text: Some("Delete the build folder".into()),
            };
        }
        assert_eq!(
            detail(&w).detail.as_deref(),
            Some("Delete the build folder")
        );
    }

    /// Finding 24: Deny takes `deny_once` before `deny_always`, whatever the
    /// order of the options.
    #[test]
    fn deny_prefers_deny_once_in_any_order() {
        let mut w = world();
        let id = coordinator_approval(&mut w);
        let option = |id: &str, kind| RequestOption {
            id: id.into(),
            label: id.into(),
            kind,
            description: None,
        };
        if let Some(a) = w.cache.approvals.get_mut(&id.to_string()) {
            a.options = vec![
                option("never", RequestOptionKind::DenyAlways),
                option("no", RequestOptionKind::DenyOnce),
                option("yes", RequestOptionKind::AllowOnce),
            ];
        }
        let stored = w.cache.approvals.get(&id.to_string()).expect("stored");
        assert_eq!(
            crate::hub::option_for(stored, ApprovalChoice::Deny).as_deref(),
            Some("no")
        );
        let mut only_always = stored.clone();
        only_always.options = vec![option("never", RequestOptionKind::DenyAlways)];
        assert_eq!(
            crate::hub::option_for(&only_always, ApprovalChoice::Deny).as_deref(),
            Some("never")
        );
    }

    /// Finding 18: archived tasks have no thread, and the view of a large
    /// cache stays correct with the indexes.
    #[test]
    fn an_archived_task_has_no_thread_view() {
        let mut w = world();
        if let Some(t) = w.cache.tasks.get_mut(&w.task.to_string()) {
            t.archived_at = Some(Timestamp::from_millis(NOW));
        }
        let state = view(&w.cache);
        assert!(state.thread(&w.task.to_string()).is_none());
        assert!(state.task(&w.task.to_string()).is_none());
    }

    fn notice(w: &World, root: Option<MessageId>, body: &str, at: i64) -> Message {
        let mut m = message(
            MessageId::new(),
            w.sayso,
            Author::System,
            MessageKind::Notice,
            root,
            at,
        );
        m.body = body.to_owned();
        m
    }

    /// Finding 3 of review cycle 2: a notice body is JSON, and the timeline
    /// showed it raw as Markdown under "Consort".
    #[test]
    fn a_notice_shows_its_text_and_kind_and_is_not_unread() {
        let mut w = world();
        let before = view(&w.cache).sidebar.groups[0].channels[0].unread;
        let n = notice(
            &w,
            None,
            r#"{"kind":"turn_failed","text":"The turn failed. Try again."}"#,
            NOW - 10_000,
        );
        w.cache.messages.insert(n.id.to_string(), n.clone());
        let state = view(&w.cache);
        let timeline = state.timeline(&w.sayso.to_string()).expect("timeline");
        let found = timeline
            .items
            .iter()
            .find_map(|i| match i {
                TimelineItem::Notice(n) => Some(n),
                _ => None,
            })
            .expect("a notice item");
        assert_eq!(found.kind, "turn_failed");
        assert_eq!(found.text, "The turn failed. Try again.");
        assert_eq!(found.message_id, n.id.to_string());
        assert!(
            !timeline
                .items
                .iter()
                .any(|i| matches!(i, TimelineItem::Message(m) if m.body.contains("turn_failed"))),
            "no raw JSON in a message"
        );
        assert_eq!(
            state.sidebar.groups[0].channels[0].unread, before,
            "a notice is not an unread message"
        );
    }

    #[test]
    fn a_notice_that_does_not_parse_shows_its_raw_body() {
        assert_eq!(
            notice_parts("The assistant is not available."),
            (String::new(), "The assistant is not available.".to_owned())
        );
        // JSON with no text, or with an empty text, is shown as it is too.
        assert_eq!(
            notice_parts(r#"{"kind":"x"}"#),
            (String::new(), r#"{"kind":"x"}"#.to_owned())
        );
        assert_eq!(
            notice_parts(r#"{"kind":"x","text":"  "}"#),
            (String::new(), r#"{"kind":"x","text":"  "}"#.to_owned())
        );
        // An unknown extra field does not matter.
        assert_eq!(
            notice_parts(r#"{"kind":"message_not_sent","text":"Not sent.","future":1}"#),
            ("message_not_sent".to_owned(), "Not sent.".to_owned())
        );
    }

    #[test]
    fn a_notice_in_a_thread_is_a_notice_post() {
        let mut w = world();
        let n = notice(
            &w,
            Some(w.card),
            r#"{"kind":"answer_not_delivered","text":"Your answer did not reach the task."}"#,
            NOW - 5_000,
        );
        w.cache.messages.insert(n.id.to_string(), n);
        let state = view(&w.cache);
        let thread = state.thread(&w.task.to_string()).expect("thread");
        let post = thread.posts.last().expect("a post");
        assert_eq!(post.speaker, Speaker::System);
        assert_eq!(
            post.bodies,
            vec![PostBody::Notice {
                kind: "answer_not_delivered".into(),
                text: "Your answer did not reach the task.".into(),
            }]
        );
    }

    /// Finding 7 of review cycle 2: a channel that never loaded showed as
    /// empty, and the first live message looked like the newest one.
    #[test]
    fn a_timeline_is_loaded_only_after_its_channel_snapshot() {
        let mut w = world();
        let id = w.sayso.to_string();
        assert!(!view(&w.cache).timeline(&id).expect("timeline").loaded);
        w.cache.channels_loaded.insert(id.clone());
        assert!(view(&w.cache).timeline(&id).expect("timeline").loaded);
        assert!(
            !view(&w.cache)
                .timeline(&w.everything.to_string())
                .expect("timeline")
                .loaded
        );
    }

    /// Finding 9 of review cycle 2: the reason of a Failed or Up next task
    /// was only in the card.
    #[test]
    fn a_failed_or_up_next_task_has_its_reason_in_the_thread() {
        let mut w = world();
        let reason = "Code signing failed: no identity matches `Developer ID`. Install the certificate, then retry.";
        let id = w.task.to_string();
        for (status, expected) in [
            (PS::Failed, Some(reason)),
            (PS::UpNext, Some(reason)),
            (PS::Working, None),
            (PS::Review, None),
        ] {
            if let Some(t) = w.cache.tasks.get_mut(&id) {
                t.status = status;
                t.status_reason = Some(reason.to_owned());
            }
            let state = view(&w.cache);
            assert_eq!(
                state.thread(&id).expect("thread").reason.as_deref(),
                expected,
                "{status:?}"
            );
        }
    }

    #[test]
    fn a_send_failure_names_its_task_and_says_what_to_do() {
        let mut w = world();
        w.cache.send_failures.push(crate::cache::SendFailure {
            id: "c-9".into(),
            channel_id: w.sayso.to_string(),
            thread_root_id: Some(w.card.to_string()),
            body: "Please retry".into(),
            message: "That task does not exist".into(),
        });
        let state = view(&w.cache);
        let failure = &state.send_failures[0];
        assert_eq!(
            failure.task_id.as_deref(),
            Some(w.task.to_string().as_str())
        );
        assert_eq!(failure.channel_name, "sayso");
        assert_eq!(failure.body, "Please retry");
        assert!(failure.message.starts_with("That task does not exist."));
        assert!(failure.message.contains("Put your text back"));
    }
}
