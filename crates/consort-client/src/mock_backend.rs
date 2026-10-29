//! The mock backend: commands change the mock view state the way the hub
//! would change the real one.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::{oneshot, watch};

use crate::{
    Backend, ClientError, Command,
    derive::{refresh, waiting_detail},
    format,
    view::*,
};

pub(crate) struct MockBackend {
    state: Arc<Mutex<ViewState>>,
    next_id: u64,
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

    fn apply(&mut self, command: Command) -> Result<Option<String>, ClientError> {
        let now = format::now_ms();
        let time = format::time_label(now);
        let message_id = self.id("msg");
        let channel_id_new = self.id("chn");
        let mut state = self
            .state
            .lock()
            .map_err(|_| ClientError::Other("The mock state is poisoned".into()))?;
        let mut reply_to = None;
        match command {
            Command::SendMessage {
                channel_id, body, ..
            } => {
                let timeline = timeline_mut(&mut state, &channel_id)?;
                timeline.items.push(TimelineItem::Message(MessageView {
                    id: message_id,
                    channel_id: channel_id.clone(),
                    speaker: Speaker::You,
                    time_label: time,
                    created_at: now,
                    body,
                    ..MessageView::default()
                }));
                reply_to = Some(channel_id);
            }
            Command::ReplyToTask { task_id, body, .. } => {
                let thread = state
                    .threads
                    .iter_mut()
                    .find(|t| t.task_id == task_id)
                    .ok_or_else(not_found)?;
                thread.posts.push(ThreadPost {
                    id: message_id,
                    speaker: Speaker::You,
                    time_label: time,
                    bodies: vec![PostBody::Text(body)],
                    pending: false,
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
                state.channels.push(ChannelView {
                    id: channel_id_new.clone(),
                    kind: ChannelKind::Standard,
                    name,
                    purpose: new.purpose,
                    icon: "hash".into(),
                    group_id: new.group_id,
                    coordinator_worker: Some("hub".into()),
                    coordinator_account: Some("Claude Personal".into()),
                    preferred_workers: new.preferred_workers,
                    task_selection: new.task_selection,
                    default_isolation: new.isolation,
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
                let name = channel.name.clone();
                for task in state
                    .tasks
                    .iter_mut()
                    .filter(|t| t.channel_id == channel_id)
                {
                    task.channel_name = name.clone();
                }
            }
            Command::ArchiveChannel { channel_id } | Command::DeleteChannel { channel_id } => {
                if state
                    .channel(&channel_id)
                    .is_some_and(|c| c.kind == ChannelKind::Everything)
                {
                    return Err(ClientError::Hub {
                        code: "conflict".into(),
                        message: "Everything cannot be archived or deleted.".into(),
                    });
                }
                state.channels.retain(|c| c.id != channel_id);
                state.tasks.retain(|t| t.channel_id != channel_id);
            }
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
                for c in std::iter::once(&mut sidebar.everything)
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
            Command::StopTask { task_id } => set_status(&mut state, &task_id, TaskStatus::Review)?,
            Command::AnswerApproval {
                approval_id,
                choice,
            } => {
                let mut task_id = None;
                for_each_approval(&mut state, &approval_id, |approval| {
                    approval.state = RequestState::Answered;
                    approval.answer = Some(choice);
                    task_id = approval.task_id.clone();
                });
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
            Command::SetWorkerLabels { worker_id, labels } => {
                worker_mut(&mut state, &worker_id)?.labels = labels;
            }
            Command::SetWorkerPaused { worker_id, paused } => {
                worker_mut(&mut state, &worker_id)?.paused = paused;
            }
            Command::RemoveWorker { worker_id } => state.workers.retain(|w| w.id != worker_id),
        }
        Ok(reply_to)
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
        .find(|w| w.id == worker_id)
        .ok_or_else(not_found)
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

fn for_each_approval(state: &mut ViewState, id: &str, mut f: impl FnMut(&mut ApprovalView)) {
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
        reply: oneshot::Sender<Result<(), ClientError>>,
        publish: &watch::Sender<Arc<ViewState>>,
    ) {
        let _ = reply.send(self.apply_and_publish(command, publish));
    }
}

impl MockBackend {
    fn apply_and_publish(
        &mut self,
        command: Command,
        publish: &watch::Sender<Arc<ViewState>>,
    ) -> Result<(), ClientError> {
        let reply_to = self.apply(command)?;
        publish_state(&self.state, publish);
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
        Ok(())
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
        let channel = mock::everything_id().to_owned();
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
    fn everything_cannot_be_archived() {
        let client = Client::mock().expect("mock");
        let err =
            block_on(client.archive_channel(mock::everything_id().into())).expect_err("refused");
        assert!(matches!(err, ClientError::Hub { .. }));
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
