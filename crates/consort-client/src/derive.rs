//! Parts of the view state that follow from other parts: the sidebar glyphs,
//! the Tasks panel groups and counts, and the task cards. The mock and the
//! reducers both call [`refresh`] after a change, so the rules live in one
//! place.

use crate::view::{
    ChannelKind, RequestState, SidebarChannel, SidebarGroup, SidebarView, TaskSection,
    TaskSectionKind, TaskStatus, TasksPanelView, TimelineItem, ViewState,
};

/// Recomputes every derived part of `state`.
pub fn refresh(state: &mut ViewState) {
    for task in &mut state.tasks {
        task.meta = task_meta(task);
    }
    sync_cards(state);
    state.tasks_panel = tasks_panel(state);
    state.sidebar = sidebar(state);
}

/// The line under a task title in the Tasks panel.
pub fn task_meta(task: &crate::view::TaskView) -> String {
    let place = match &task.worker_name {
        Some(worker) => format!("{} on {worker}", task.channel_name),
        None => task.channel_name.clone(),
    };
    let reason = task.status_reason.as_deref().filter(|r| !r.is_empty());
    match task.status {
        TaskStatus::NeedsInput => format!("Needs input. {place}"),
        TaskStatus::Review => format!("Ready for review. {place}"),
        TaskStatus::Working => match &task.account_label {
            Some(account) => format!("{place}, {account}"),
            None => place,
        },
        TaskStatus::Failed => match reason {
            Some(reason) => format!("{place}. {reason}"),
            None => format!("{place}. Failed"),
        },
        TaskStatus::UpNext => format!(
            "{}. {}",
            task.channel_name,
            reason.unwrap_or("Waiting for a worker")
        ),
        TaskStatus::Scheduled => match reason {
            Some(reason) => format!("{}. {reason}", task.channel_name),
            None => task.channel_name.clone(),
        },
        TaskStatus::Resolved => format!("Resolved. {place}"),
    }
}

/// True when the channel's own timeline has an approval or a decision that
/// waits for the user (one from the coordinator, not from a task).
pub fn channel_waits(state: &ViewState, channel_id: &str) -> bool {
    state.timeline(channel_id).is_some_and(|t| {
        t.items.iter().any(|item| match item {
            TimelineItem::Approval(a) => a.approval.state == RequestState::Pending,
            TimelineItem::Decision(d) => d.decision.state == RequestState::Pending,
            _ => false,
        })
    })
}

/// The most urgent status that the sidebar shows for a channel. A pending
/// coordinator approval or decision counts as Needs input.
pub fn channel_status(state: &ViewState, channel_id: &str) -> Option<TaskStatus> {
    if channel_waits(state, channel_id) {
        return Some(TaskStatus::NeedsInput);
    }
    state
        .tasks
        .iter()
        .filter(|t| t.channel_id == channel_id)
        .filter_map(|t| t.status.sidebar_rank().map(|rank| (rank, t.status)))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, status)| status)
}

fn sidebar_channel(state: &ViewState, channel_id: &str, unread: u32) -> Option<SidebarChannel> {
    let channel = state.channel(channel_id)?;
    // Everything shows no task glyph, but its own requests still mark it.
    let status = if channel.kind == ChannelKind::Everything {
        channel_waits(state, channel_id).then_some(TaskStatus::NeedsInput)
    } else {
        channel_status(state, channel_id)
    };
    Some(SidebarChannel {
        id: channel.id.clone(),
        name: channel.name.clone(),
        icon: channel.icon.clone(),
        status,
        attention: status.is_some_and(TaskStatus::is_your_turn) || unread > 0,
        unread,
    })
}

fn unread_of(previous: &SidebarView, channel_id: &str) -> u32 {
    std::iter::once(&previous.everything)
        .chain(previous.groups.iter().flat_map(|g| g.channels.iter()))
        .chain(previous.ungrouped.iter())
        .find(|c| c.id == channel_id)
        .map_or(0, |c| c.unread)
}

/// The sidebar: Everything, the groups in order, then channels with no group.
pub fn sidebar(state: &ViewState) -> SidebarView {
    let previous = &state.sidebar;
    let everything = state
        .everything()
        .and_then(|c| sidebar_channel(state, &c.id, unread_of(previous, &c.id)))
        .unwrap_or_default();
    let groups = state
        .groups
        .iter()
        .map(|group| SidebarGroup {
            id: group.id.clone(),
            name: group.name.clone(),
            collapsed: previous
                .groups
                .iter()
                .find(|g| g.id == group.id)
                .is_some_and(|g| g.collapsed),
            channels: state
                .channels
                .iter()
                .filter(|c| c.group_id.as_deref() == Some(group.id.as_str()))
                .filter_map(|c| sidebar_channel(state, &c.id, unread_of(previous, &c.id)))
                .collect(),
        })
        .collect();
    let ungrouped = state
        .channels
        .iter()
        .filter(|c| c.kind == ChannelKind::Standard)
        .filter(|c| {
            c.group_id
                .as_ref()
                .is_none_or(|g| !state.groups.iter().any(|group| &group.id == g))
        })
        .filter_map(|c| sidebar_channel(state, &c.id, unread_of(previous, &c.id)))
        .collect();
    SidebarView {
        everything,
        groups,
        ungrouped,
        workers_online: state.workers.iter().filter(|w| w.online).count() as u32,
        workers_total: state.workers.len() as u32,
    }
}

/// The Tasks panel: counts for the tabs, and the Active sections.
pub fn tasks_panel(state: &ViewState) -> TasksPanelView {
    let mut tasks: Vec<_> = state.tasks.iter().collect();
    // Newest first inside each section.
    tasks.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
    let ids = |pred: &dyn Fn(TaskStatus) -> bool| -> Vec<String> {
        tasks
            .iter()
            .filter(|t| pred(t.status))
            .map(|t| t.id.clone())
            .collect()
    };
    let your_turn = ids(&|s| s.is_your_turn());
    let working = ids(&|s| s == TaskStatus::Working);
    let failed = ids(&|s| s == TaskStatus::Failed);
    let up_next = ids(&|s| s == TaskStatus::UpNext);
    let scheduled = ids(&|s| s == TaskStatus::Scheduled);
    let resolved = ids(&|s| s == TaskStatus::Resolved);
    let mut active = Vec::new();
    for (kind, task_ids) in [
        (TaskSectionKind::YourTurn, your_turn.clone()),
        (TaskSectionKind::Working, working.clone()),
        (TaskSectionKind::Failed, failed.clone()),
    ] {
        if !task_ids.is_empty() {
            active.push(TaskSection { kind, task_ids });
        }
    }
    TasksPanelView {
        need_you: your_turn.len() as u32,
        active_count: (your_turn.len() + working.len() + failed.len()) as u32,
        up_next_count: up_next.len() as u32,
        scheduled_count: scheduled.len() as u32,
        resolved_count: resolved.len() as u32,
        active,
        up_next,
        scheduled,
        resolved,
    }
}

/// Copies each task's title and status onto its cards in every timeline.
fn sync_cards(state: &mut ViewState) {
    let tasks = state.tasks.clone();
    for timeline in &mut state.timelines {
        for item in &mut timeline.items {
            if let TimelineItem::TaskCard(card) = item
                && let Some(task) = tasks.iter().find(|t| t.id == card.task_id)
            {
                card.title = task.title.clone();
                card.status = task.status;
            }
        }
    }
    for thread in &mut state.threads {
        if let Some(task) = tasks.iter().find(|t| t.id == thread.task_id) {
            thread.title = task.title.clone();
            thread.status = task.status;
        }
    }
}

/// "1 decision, 1 approval": what waits for the user in a thread.
pub fn waiting_detail(decisions: usize, approvals: usize) -> String {
    let part = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    [
        part(decisions, "decision", "decisions"),
        part(approvals, "approval", "approvals"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{ChannelView, TaskView};

    fn task(id: &str, channel: &str, status: TaskStatus, updated_at: i64) -> TaskView {
        TaskView {
            id: id.into(),
            channel_id: channel.into(),
            status,
            updated_at,
            ..TaskView::default()
        }
    }

    fn channel(id: &str) -> ChannelView {
        ChannelView {
            id: id.into(),
            name: id.into(),
            ..ChannelView::default()
        }
    }

    #[test]
    fn the_sidebar_shows_the_most_urgent_status() {
        let mut state = ViewState {
            channels: vec![channel("a"), channel("b"), channel("c")],
            tasks: vec![
                task("1", "a", TaskStatus::Working, 1),
                task("2", "a", TaskStatus::Failed, 2),
                task("3", "a", TaskStatus::Review, 3),
                task("4", "b", TaskStatus::Working, 4),
                task("5", "c", TaskStatus::Resolved, 5),
                task("6", "c", TaskStatus::UpNext, 6),
            ],
            ..ViewState::default()
        };
        refresh(&mut state);
        let rows = &state.sidebar.ungrouped;
        assert_eq!(rows[0].status, Some(TaskStatus::Review));
        assert!(rows[0].attention);
        assert_eq!(rows[1].status, Some(TaskStatus::Working));
        assert!(!rows[1].attention);
        assert_eq!(rows[2].status, None);
    }

    #[test]
    fn the_tasks_panel_groups_and_counts() {
        let mut state = ViewState {
            tasks: vec![
                task("old", "a", TaskStatus::NeedsInput, 1),
                task("new", "a", TaskStatus::Review, 9),
                task("w", "a", TaskStatus::Working, 5),
                task("u", "a", TaskStatus::UpNext, 5),
                task("s", "a", TaskStatus::Scheduled, 5),
                task("r", "a", TaskStatus::Resolved, 5),
            ],
            ..ViewState::default()
        };
        refresh(&mut state);
        let panel = &state.tasks_panel;
        assert_eq!(panel.need_you, 2);
        assert_eq!(panel.active_count, 3);
        assert_eq!(
            (
                panel.up_next_count,
                panel.scheduled_count,
                panel.resolved_count
            ),
            (1, 1, 1)
        );
        assert_eq!(panel.active.len(), 2, "no Failed section when none failed");
        assert_eq!(panel.active[0].kind, TaskSectionKind::YourTurn);
        assert_eq!(panel.active[0].task_ids, vec!["new", "old"]);
    }

    #[test]
    fn waiting_detail_reads_naturally() {
        assert_eq!(waiting_detail(1, 1), "1 decision, 1 approval");
        assert_eq!(waiting_detail(0, 2), "2 approvals");
        assert_eq!(waiting_detail(0, 0), "");
    }
}
