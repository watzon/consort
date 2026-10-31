//! Tasks: creation, placement, and the client API handlers.

use consort_proto::client_api::*;
use consort_proto::execution_link::{ExecWorkspaceRelease, HubToExecution};
use consort_proto::frame::Empty;
use consort_proto::ids::SessionId;
use consort_proto::ids::TaskId;
use consort_proto::ids::WorkerId;
use consort_proto::model::{
    Author, Channel, MessageKind, Repo, Session, SessionRole, SessionState, Task, Usage,
    ValidationError,
};
use consort_proto::model::{Isolation, ModelSelection};
use consort_proto::session_host_link::{
    HubToSessionHost, InventoryRefresh, ItemDetail, ItemDetailResult, SessionExecution,
    SessionInterrupt, SessionStart, TurnInput, WorkspaceRelease, WorkspaceRepo, WorkspaceSpec,
};
use consort_proto::status::{Actor, TaskStatus};
use consort_proto::time::Timestamp;

use super::flow::transition;
use super::hooks::NewTask;
use super::model_switch::check_selection_for_started_task;
use crate::error::{HubError, Result};
use crate::fallback;
use crate::hub::{Caller, Hub};
use crate::limits::{BRIEF_MAX_BYTES, check_text};
use crate::link::events::{Ctx, Effects, end_session_by_hub, mark_lost_in_tx, queue_stop};
use crate::placement::{Placement, PlacementError, PlacementInput, Subject, place};
use crate::prompts;
use crate::store::conversation::{self, NewMessage};
use crate::store::{machine, work};

const TITLE_MAX_CHARS: usize = 200;

impl Hub {
    /// Creates a task with its card message. A task that is due now goes to
    /// placement at once. The result shows the status after that attempt.
    pub(crate) async fn create_task(&self, new: NewTask) -> Result<Task> {
        let title = new.title.trim().to_owned();
        if title.is_empty() {
            return Err(HubError::invalid("The task needs a title."));
        }
        if title.chars().count() > TITLE_MAX_CHARS {
            return Err(HubError::invalid(format!(
                "The title is longer than {TITLE_MAX_CHARS} characters. Shorten it."
            )));
        }
        if new.brief.trim().is_empty() {
            return Err(HubError::invalid("The task needs a brief."));
        }
        check_text("brief", &new.brief, BRIEF_MAX_BYTES)?;
        let task = self
            .store
            .write(move |tx| {
                let channel = conversation::channel_or_404(tx.db(), new.channel_id)?;
                if channel.archived_at.is_some() {
                    return Err(HubError::conflict(
                        "The channel is archived. Restore it to add tasks.",
                    ));
                }
                let now = tx.now();
                let id = TaskId::new();
                let mut card = NewMessage::new(
                    channel.id,
                    Author::System,
                    MessageKind::TaskCard,
                    title.clone(),
                );
                card.author_task_id = Some(id);
                card.author_routine_id = new.routine_id;
                let card = tx.insert_message(card)?;
                let task = Task {
                    id,
                    channel_id: channel.id,
                    title,
                    brief: new.brief,
                    status: TaskStatus::initial(new.start_at, now),
                    status_reason: None,
                    created_by: new.created_by,
                    card_message_id: card.id,
                    model_selection: new.model_selection,
                    placement: new.placement,
                    approval_mode: new.approval_mode.unwrap_or(channel.default_approval_mode),
                    start_at: new.start_at,
                    current_session_id: None,
                    worker_id: None,
                    review_summary: None,
                    resolved_at: None,
                    session_ended: false,
                    monid_calls: 0,
                    monid_cost_micros: 0,
                    archived_at: None,
                    created_at: now,
                    updated_at: now,
                };
                tx.save_task(&task)?;
                tx.save_task_attachments(task.id, &new.attachments)?;
                if let Some((kind, draft)) = new.proactive {
                    tx.save_proactive_task(&task, kind, draft)?;
                }
                if !new.user_request.trim().is_empty() {
                    tx.save_task_request(task.id, &new.user_request)?;
                }
                if new.voice_task {
                    tx.mark_voice_task(task.id)?;
                }
                Ok(task)
            })
            .await?;
        if task.status == TaskStatus::UpNext {
            self.dispatch_task(task.id).await?;
            let id = task.id;
            return self
                .store
                .read(move |conn| work::task_or_404(conn, id))
                .await;
        }
        Ok(task)
    }

    /// Runs placement for a task in Up next. A task that cannot start stays
    /// in Up next with a reason. A task that can never start fails.
    pub(crate) async fn dispatch_task(&self, task_id: TaskId) -> Result<()> {
        let inputs = self
            .store
            .read(move |conn| {
                let Some(task) = work::task(conn, task_id)? else {
                    return Ok(None);
                };
                if task.status != TaskStatus::UpNext {
                    return Ok(None);
                }
                let channel = conversation::channel_or_404(conn, task.channel_id)?;
                let workers = machine::workers(conn)?;
                let open = work::open_session_counts(conn)?;
                let repos = conversation::repos(conn)?;
                let tutti = conversation::tutti_channel(conn)?;
                let holds = machine::account_holds(conn, Timestamp::now())?;
                let path = crate::store::preferences::preferences(conn)?.fallback_path;
                Ok(Some((
                    task, channel, workers, open, repos, tutti, holds, path,
                )))
            })
            .await?;
        let Some((task, channel, workers, open, all_repos, tutti, holds, path)) = inputs else {
            return Ok(());
        };

        let repo_ids = task
            .placement
            .repos
            .clone()
            .unwrap_or_else(|| channel.repo_ids.clone());
        let repos: Vec<Repo> = repo_ids
            .iter()
            .filter_map(|id| all_repos.iter().find(|r| r.id == *id).cloned())
            .collect();
        let isolation = task
            .placement
            .isolation
            .unwrap_or(channel.default_isolation);
        let chosen = task
            .model_selection
            .clone()
            .or_else(|| channel.task_selection.clone());
        // With no choice for tasks, a task runs on what the assistant runs
        // on. This is a default and not a choice of the user: when the task
        // needs a worker without that account, the default rule decides.
        let inherited = tutti
            .task_selection
            .clone()
            .or_else(|| channel.effective_coordinator_selection(&tutti).cloned());
        // A first choice that is held (a limit or an outage) gives way to the
        // fallback path before placement runs.
        let by_label = fallback::holds_by_label(&workers, &holds);
        let walk = fallback::Walk {
            path: &path,
            workers: &workers,
            holds: &by_label,
            mode: task.approval_mode,
        };
        let chosen_start = chosen
            .as_ref()
            .map(|selection| fallback::choose_start(&walk, Some(selection)));
        let inherited_start = inherited
            .as_ref()
            .map(|selection| fallback::choose_start(&walk, Some(selection)));
        let default_start = fallback::choose_start(&walk, None);
        let input = PlacementInput {
            subject: Subject::Task,
            workers: &workers,
            open_sessions: &open,
            selection: None,
            request: &task.placement,
            approval_mode: task.approval_mode,
            preferred_workers: &channel.preferred_workers,
            channel_tags: &channel.tags,
            repos: &repos,
            isolation,
        };
        let on = |selection: &'_ ModelSelection| {
            place(&PlacementInput {
                selection: Some(selection),
                ..input
            })
        };
        let default_placed = || match &default_start.selection {
            Some(selection) => on(selection),
            None => place(&input),
        };
        let (placed, moved) = match (&chosen_start, &inherited_start) {
            (Some(start), _) => (
                start.selection.as_ref().map_or_else(|| place(&input), on),
                start.moved.clone(),
            ),
            (None, Some(start)) => match start.selection.as_ref().map(on) {
                Some(Ok(placement)) => (Ok(placement), start.moved.clone()),
                _ => (default_placed(), default_start.moved.clone()),
            },
            (None, None) => (default_placed(), default_start.moved.clone()),
        };
        match placed {
            Ok(placement) => {
                self.start_task_session(
                    task, channel, placement, &repos, isolation, moved, &workers,
                )
                .await
            }
            Err(PlacementError::Wait(reason)) => {
                self.store
                    .write(move |tx| {
                        let mut task = work::task_or_404(tx.db(), task_id)?;
                        if task.status == TaskStatus::UpNext
                            && task.status_reason.as_deref() != Some(&reason)
                        {
                            task.status_reason = Some(reason);
                            task.updated_at = tx.now();
                            tx.save_task(&task)?;
                        }
                        Ok(())
                    })
                    .await
            }
            Err(PlacementError::Fail(error)) => {
                let channel_id = task.channel_id;
                self.store
                    .write(move |tx| {
                        let mut task = work::task_or_404(tx.db(), task_id)?;
                        if task.status == TaskStatus::UpNext {
                            transition(
                                tx,
                                &mut task,
                                TaskStatus::Failed,
                                Actor::System,
                                Some(error.message),
                                None,
                            )?;
                            tx.save_task(&task)?;
                        }
                        Ok(())
                    })
                    .await?;
                self.kick_coordinator(channel_id);
                Ok(())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_task_session(
        &self,
        task: Task,
        channel: Channel,
        placement: Placement,
        repos: &[Repo],
        isolation: Isolation,
        moved: Option<fallback::Moved>,
        workers: &[consort_proto::model::Worker],
    ) -> Result<()> {
        // The CLI runs on the session host, and the tools on the worker.
        let worker_id = placement.session_host_id;
        let execution_worker_id = placement.worker_id;
        let execution_worker_name = placement.worker_name.clone();
        let fallback_workers = workers.to_vec();
        let task_id = task.id;
        let channel_id = channel.id;
        let workspace_repos: Vec<WorkspaceRepo> = placement
            .checkouts
            .iter()
            .map(|c| WorkspaceRepo {
                repo_id: c.repo_id,
                url: c.url.clone(),
                branch: repos
                    .iter()
                    .find(|r| r.id == c.repo_id)
                    .map(|r| r.default_branch.clone()),
                checkout_path: c.checkout_path.clone(),
            })
            .collect();
        let mut workspace = WorkspaceSpec {
            repos: workspace_repos,
            isolation,
            carry_uncommitted: false,
        };
        if workspace.validate().is_err() {
            // The worker has no checkout of a repo. A fresh clone needs only the URL.
            workspace.isolation = Isolation::Clone;
        }
        let (global_index, channel_index) = self.memory_indexes(&channel).await?;
        let mut system_prompt =
            prompts::task_system_prompt(&channel, &task, &global_index, &channel_index);
        if let Some(voice) = self.voice_prompt_section(&channel).await? {
            system_prompt.push_str(&voice);
        }
        let first_turn = prompts::task_first_turn(&task);
        let local_mcp = task
            .placement
            .require
            .as_ref()
            .map(|r| r.local_mcp_servers.clone())
            .unwrap_or_default();
        let account_label = placement.account_label.clone();
        let changed = self
            .store
            .write(move |tx| {
                let mut task = work::task_or_404(tx.db(), task_id)?;
                if task.status != TaskStatus::UpNext {
                    return Ok(false);
                }
                let now = tx.now();
                let session = Session {
                    id: SessionId::new(),
                    role: SessionRole::Task,
                    channel_id,
                    task_id: Some(task_id),
                    worker_id,
                    execution_worker_id: Some(execution_worker_id),
                    account_id: placement.account_id,
                    harness: placement.harness.clone(),
                    model: placement.model.clone(),
                    options: placement.options.clone(),
                    workspace_path: String::new(),
                    native_ref: None,
                    state: SessionState::Starting,
                    last_seq: 0,
                    usage: Usage::default(),
                    started_at: now,
                    ended_at: None,
                    end_reason: None,
                };
                tx.save_session(&session)?;
                let extensions = tx.session_extensions(worker_id, channel_id)?;
                let channel_files =
                    tx.channel_files_for_start(session.id, channel_id, Some(task_id))?;
                let attachments = work::task_attachments(tx.db(), task_id)?;
                tx.save_session_runtime(
                    session.id,
                    &work::SessionRuntime {
                        turn_open: true,
                        active_at: Some(now),
                        ..work::SessionRuntime::default()
                    },
                )?;
                tx.queue_command(
                    worker_id,
                    &HubToSessionHost::SessionStart(SessionStart {
                        session_id: session.id,
                        role: SessionRole::Task,
                        channel_id,
                        task_id: Some(task_id),
                        account_id: placement.account_id,
                        model: placement.model.clone(),
                        options: placement.options.clone(),
                        approval_mode: task.approval_mode,
                        workspace,
                        system_prompt_append: system_prompt,
                        allowed_local_mcp_servers: local_mcp,
                        channel_files,
                        extensions,
                        resume: None,
                        turn: Some(TurnInput {
                            text: first_turn,
                            attachments,
                        }),
                        turn_approval_mode: None,
                        execution: Some(SessionExecution {
                            worker_id: execution_worker_id,
                            worker_name: execution_worker_name.clone(),
                        }),
                    }),
                )?;
                transition(
                    tx,
                    &mut task,
                    TaskStatus::Working,
                    Actor::System,
                    None,
                    None,
                )?;
                task.current_session_id = Some(session.id);
                task.session_ended = false;
                task.worker_id = Some(execution_worker_id);
                // The task stays on this account and model until the user
                // changes it, also when the channel changes later.
                task.model_selection = Some(ModelSelection {
                    account_label,
                    model: session.model.clone(),
                    options: session.options.clone(),
                });
                tx.save_task(&task)?;
                if let Some(moved) = &moved {
                    let (text, notice) = fallback::moved_notice(
                        consort_proto::model::FallbackScope::Task,
                        &fallback_workers,
                        moved,
                    );
                    crate::fallback_flow::post_fallback_notice(
                        tx,
                        channel_id,
                        Some(task.card_message_id),
                        Some(task_id),
                        "account_fallback",
                        &text,
                        &notice,
                    )?;
                    for (text, notice) in fallback::signed_out_notices(&moved.lines) {
                        crate::fallback_flow::post_fallback_notice(
                            tx,
                            channel_id,
                            Some(task.card_message_id),
                            Some(task_id),
                            "fallback_signed_out",
                            &text,
                            &notice,
                        )?;
                    }
                }
                Ok(true)
            })
            .await?;
        if changed {
            self.pump_worker(worker_id);
        }
        Ok(())
    }

    /// Runs placement again for every task in Up next. Call it when a worker
    /// connects or sends a new inventory.
    pub(crate) async fn dispatch_waiting_tasks(&self) -> Result<()> {
        let tasks = self
            .store
            .read(|conn| work::tasks_with_status(conn, TaskStatus::UpNext))
            .await?;
        for task in tasks {
            if task.archived_at.is_none()
                && let Err(err) = self.dispatch_task(task.id).await
            {
                tracing::warn!(error = %err, task_id = %task.id, "could not place a task");
            }
        }
        // A task that waits for its new session on another model may have
        // lost its start with a restart of the hub.
        self.start_switched_sessions().await
    }

    /// Moves scheduled tasks whose time has come to Up next and places them.
    pub(crate) async fn start_due_tasks(&self) -> Result<()> {
        let due = self
            .store
            .write(|tx| {
                let mut started = Vec::new();
                for mut task in work::due_scheduled_tasks(tx.db(), tx.now())? {
                    if task.archived_at.is_some() {
                        continue;
                    }
                    transition(tx, &mut task, TaskStatus::UpNext, Actor::System, None, None)?;
                    tx.save_task(&task)?;
                    started.push(task.id);
                }
                Ok(started)
            })
            .await?;
        for id in due {
            self.dispatch_task(id).await?;
        }
        Ok(())
    }

    // Client API handlers ------------------------------------------------

    pub(crate) async fn task_create(
        &self,
        caller: &Caller,
        request: TaskCreate,
    ) -> Result<TaskResult> {
        let created_by = match caller {
            Caller::Client(_) => consort_proto::model::TaskCreator::User,
            Caller::Hub => consort_proto::model::TaskCreator::Routine,
        };
        let task = self
            .create_task(NewTask {
                channel_id: request.channel_id,
                title: request.title,
                brief: request.brief,
                model_selection: request.model_selection,
                placement: request.placement,
                approval_mode: request.approval_mode,
                start_at: request.start_at,
                created_by,
                routine_id: None,
                attachments: Vec::new(),
                proactive: None,
                user_request: String::new(),
                voice_task: false,
            })
            .await?;
        Ok(TaskResult { task })
    }

    pub(crate) async fn task_update(
        &self,
        _caller: &Caller,
        request: TaskUpdate,
    ) -> Result<TaskResult> {
        let id = request.task_id;
        let task = self
            .store
            .write(move |tx| {
                let mut task = work::task_or_404(tx.db(), id)?;
                let not_started = matches!(task.status, TaskStatus::Scheduled | TaskStatus::UpNext);
                let wants_run_change = request.brief.is_some()
                    || request.placement.is_some()
                    || request.approval_mode.is_some()
                    || request.start_at.is_some();
                if wants_run_change && !not_started {
                    return Err(HubError::conflict(
                        "The task already started. You can change only its title and its model now.",
                    ));
                }
                // A started task takes a new model. It is saved now, a
                // running turn is not cut, and the next turn uses it.
                if let (false, Some(selection)) = (not_started, request.model_selection.as_ref())
                    && let Err(reason) = check_selection_for_started_task(
                        tx.db(),
                        &task,
                        selection.as_ref(),
                    )?
                {
                    return Err(HubError::conflict(reason));
                }
                if let Some(title) = request.title {
                    let title = title.trim().to_owned();
                    if title.is_empty() || title.chars().count() > TITLE_MAX_CHARS {
                        return Err(
                            ValidationError::new("title", "must have 1 to 200 characters").into(),
                        );
                    }
                    task.title = title;
                }
                if let Some(brief) = request.brief {
                    if brief.trim().is_empty() {
                        return Err(HubError::invalid("The task needs a brief."));
                    }
                    check_text("brief", &brief, BRIEF_MAX_BYTES)?;
                    task.brief = brief;
                }
                if let Some(selection) = request.model_selection {
                    task.model_selection = selection;
                }
                if let Some(placement) = request.placement {
                    task.placement = placement;
                }
                if let Some(mode) = request.approval_mode {
                    task.approval_mode = mode;
                }
                if let Some(start_at) = request.start_at {
                    let now = tx.now();
                    match (task.status, start_at) {
                        (TaskStatus::Scheduled, None) => {
                            transition(
                                tx,
                                &mut task,
                                TaskStatus::UpNext,
                                Actor::System,
                                None,
                                None,
                            )?;
                        }
                        (TaskStatus::UpNext, Some(at)) if at > now => {
                            return Err(HubError::conflict(
                                "The task is waiting to run. Create a new task to run it later.",
                            ));
                        }
                        _ => {}
                    }
                    task.start_at = start_at;
                }
                task.updated_at = tx.now();
                tx.save_task(&task)?;
                Ok(task)
            })
            .await?;
        if task.status == TaskStatus::UpNext {
            self.dispatch_task(id).await?;
        }
        let task = self
            .store
            .read(move |conn| work::task_or_404(conn, id))
            .await?;
        Ok(TaskResult { task })
    }

    pub(crate) async fn task_resolve(
        &self,
        _caller: &Caller,
        request: TaskResolve,
    ) -> Result<TaskResult> {
        let id = request.task_id;
        let task = self
            .store
            .write(move |tx| {
                let mut task = work::task_or_404(tx.db(), id)?;
                if transition(tx, &mut task, TaskStatus::Resolved, Actor::User, None, None)? {
                    tx.save_task(&task)?;
                }
                Ok(task)
            })
            .await?;
        Ok(TaskResult { task })
    }

    pub(crate) async fn task_reopen(
        &self,
        _caller: &Caller,
        request: TaskReopen,
    ) -> Result<TaskResult> {
        let id = request.task_id;
        let task = self
            .store
            .write(move |tx| {
                let mut task = work::task_or_404(tx.db(), id)?;
                // Not done needs the session and the workspace. Archive
                // deleted the workspace, so the task can only start again.
                if task.status == TaskStatus::Resolved && work::task_workspace_released(tx.db(), id)? {
                    return Err(HubError::conflict(
                        "The workspace of this task was deleted when the task was archived, so it cannot go back to Ready for review. Retry the task to run it again in a new workspace.",
                    ));
                }
                if transition(tx, &mut task, TaskStatus::Review, Actor::User, None, None)? {
                    tx.save_task(&task)?;
                }
                Ok(task)
            })
            .await?;
        Ok(TaskResult { task })
    }

    pub(crate) async fn task_retry(
        &self,
        _caller: &Caller,
        request: TaskRetry,
    ) -> Result<TaskResult> {
        let task = self.retry_task(request.task_id, Actor::User).await?;
        Ok(TaskResult { task })
    }

    /// Sends a task back to Up next and places it again with a new session.
    ///
    /// A failed task can always retry. A task in Ready for review or
    /// Resolved can retry only when its session has ended, because then
    /// nothing else can move it on. The old session may still run on its
    /// worker (a failed turn does not end it), so the hub ends it and the
    /// worker gets `session.stop`. Open questions of the old session are
    /// closed: the new session starts from the brief.
    pub(crate) async fn retry_task(&self, id: TaskId, actor: Actor) -> Result<Task> {
        let hub = self.clone();
        let fx = self
            .store
            .write(move |tx| {
                let task = work::task_or_404(tx.db(), id)?;
                // Refuse a retry that the rules do not allow before anything
                // changes. Ending the session of a running task would fail
                // the task. A task that is in Up next already stays as it is.
                if task.status != TaskStatus::UpNext {
                    task.status.check_transition(TaskStatus::UpNext, actor)?;
                }
                if task.archived_at.is_some() {
                    return Err(HubError::conflict(
                        "The task is archived. Restore it first, then retry it.",
                    ));
                }
                if matches!(task.status, TaskStatus::Review | TaskStatus::Resolved) {
                    let session_open = match task.current_session_id {
                        Some(s) => work::session(tx.db(), s)?.is_some_and(|s| !s.state.is_final()),
                        None => false,
                    };
                    if session_open {
                        return Err(HubError::conflict(
                            "The session of this task still runs, and a retry would end it. Send the task a message to go on with the same session.",
                        ));
                    }
                }
                // The worker deletes the old workspace first. A new session
                // in the same folder could lose its files to that delete.
                if work::task_release_queued(tx.db(), id)? {
                    return Err(HubError::conflict(
                        "A worker has not deleted the old workspace of this task yet. Retry the task when that worker is online.",
                    ));
                }
                // The old session ends first, while the task is still in its
                // old status, so ending it does not change the task.
                let fx = match task.current_session_id {
                    Some(old) => end_session_by_hub(&hub, tx, old, "The task was retried.")?,
                    None => Effects::default(),
                };
                for mut decision in work::pending_decisions_of_task(tx.db(), id)? {
                    decision.status = consort_proto::model::DecisionStatus::Canceled;
                    tx.save_decision(&decision)?;
                }
                let mut task = work::task_or_404(tx.db(), id)?;
                if transition(tx, &mut task, TaskStatus::UpNext, actor, None, None)? {
                    task.current_session_id = None;
                    task.session_ended = false;
                    task.worker_id = None;
                    task.review_summary = None;
                    tx.save_task(&task)?;
                }
                tx.set_task_workspace_released(id, false)?;
                Ok(fx)
            })
            .await?;
        self.run_effects(fx);
        self.dispatch_task(id).await?;
        self.store
            .read(move |conn| work::task_or_404(conn, id))
            .await
    }

    pub(crate) async fn task_interrupt(
        &self,
        _caller: &Caller,
        request: TaskInterrupt,
    ) -> Result<Empty> {
        self.interrupt_task(request.task_id).await?;
        Ok(Empty {})
    }

    /// Stops the running turn of a task. The worker must be online.
    pub(crate) async fn interrupt_task(&self, id: TaskId) -> Result<()> {
        let (task, session) = self
            .store
            .read(move |conn| {
                let task = work::task_or_404(conn, id)?;
                let session = match task.current_session_id {
                    Some(s) => work::session(conn, s)?,
                    None => None,
                };
                Ok((task, session))
            })
            .await?;
        if !matches!(task.status, TaskStatus::Working | TaskStatus::NeedsInput) {
            return Err(HubError::conflict(
                "The task is not running, so there is nothing to stop.",
            ));
        }
        let session = session.ok_or_else(|| HubError::conflict("The task has no session."))?;
        self.worker_rpc(
            session.worker_id,
            HubToSessionHost::SessionInterrupt(SessionInterrupt {
                session_id: session.id,
            }),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn task_archive(
        &self,
        _caller: &Caller,
        request: TaskArchive,
    ) -> Result<TaskResult> {
        let id = request.task_id;
        let hub = self.clone();
        let (task, fx) = self
            .store
            .write(move |tx| {
                let mut task = work::task_or_404(tx.db(), id)?;
                let archived_at = request.archived.then(|| tx.now());
                let mut fx = Effects::default();
                if task.archived_at.is_some() != archived_at.is_some() {
                    task.archived_at = archived_at;
                    task.updated_at = tx.now();
                    // A resolved task is done, so its session stops and its
                    // workspace goes (DECISION 61). Resolve alone keeps
                    // both, because Not done needs them.
                    let release = task.archived_at.is_some() && task.status == TaskStatus::Resolved;
                    if release && task.current_session_id.is_some() {
                        task.session_ended = true;
                    }
                    tx.save_task(&task)?;
                    if release {
                        fx = release_task(&hub, tx, &task, "The task was archived.")?;
                    }
                }
                Ok((task, fx))
            })
            .await?;
        self.run_effects(fx);
        Ok(TaskResult { task })
    }

    pub(crate) async fn task_delete(&self, _caller: &Caller, request: TaskDelete) -> Result<Empty> {
        let id = request.task_id;
        let hub = self.clone();
        let fx = self
            .store
            .write(move |tx| {
                let task = work::task_or_404(tx.db(), id)?;
                let fx = release_task(&hub, tx, &task, "The task was removed.")?;
                tx.purge_task(&task)?;
                Ok(fx)
            })
            .await?;
        self.run_effects(fx);
        Ok(Empty {})
    }

    pub(crate) async fn task_items(
        &self,
        _caller: &Caller,
        request: TaskItems,
    ) -> Result<TaskItemsResult> {
        let (items, has_more) = self
            .store
            .read(move |conn| {
                work::task_or_404(conn, request.task_id)?;
                work::task_items(
                    conn,
                    request.task_id,
                    request.session_id,
                    request.after_item_id,
                    usize::try_from(request.limit.unwrap_or(200)).unwrap_or(200),
                )
            })
            .await?;
        Ok(TaskItemsResult { items, has_more })
    }

    pub(crate) async fn task_item_detail(
        &self,
        _caller: &Caller,
        request: TaskItemDetail,
    ) -> Result<ItemDetailResult> {
        let session = self
            .store
            .read(move |conn| work::session_or_404(conn, request.session_id))
            .await?;
        let value = self
            .worker_rpc(
                session.worker_id,
                HubToSessionHost::ItemDetail(ItemDetail {
                    session_id: request.session_id,
                    item_id: request.item_id,
                }),
            )
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    /// Fails the task of a session whose turn is open but sent nothing for
    /// `max_quiet_ms` while its worker was online. A lost `turn.ended` (for
    /// example an `event_unreadable` in its place) would keep such a task
    /// in Working for ever. The maintenance loop calls it.
    pub(crate) async fn expire_quiet_task_turns(&self, max_quiet_ms: i64) -> Result<()> {
        let candidates = self
            .store
            .read(move |conn| {
                let now = consort_proto::time::Timestamp::now().as_millis();
                let mut out = Vec::new();
                for (session, runtime) in work::task_sessions_with_open_turn(conn)? {
                    let quiet_since = runtime.active_at.unwrap_or(session.started_at);
                    if now - quiet_since.as_millis() <= max_quiet_ms {
                        continue;
                    }
                    let Some(task) = session
                        .task_id
                        .map(|t| work::task(conn, t))
                        .transpose()?
                        .flatten()
                    else {
                        continue;
                    };
                    // Needs input waits for the user, not for the session.
                    if task.status != TaskStatus::Working
                        || task.current_session_id != Some(session.id)
                        || work::pending_count_for_task(conn, task.id)? > 0
                    {
                        continue;
                    }
                    out.push(session);
                }
                Ok(out)
            })
            .await?;
        let quiet = std::time::Duration::from_millis(u64::try_from(max_quiet_ms).unwrap_or(0));
        for session in candidates {
            // Events of an offline worker wait in its spool. Count only the
            // time that the worker was online.
            let online_long_enough = self
                .links
                .get(session.worker_id)
                .is_some_and(|l| !l.outbox.is_closed() && l.connected_at.elapsed() > quiet);
            if !online_long_enough {
                continue;
            }
            tracing::warn!(session_id = %session.id, "a task turn sent nothing for too long, so the hub fails the task");
            if let Err(err) = self.fail_quiet_task_turn(session.id, max_quiet_ms).await {
                tracing::warn!(error = %err, session_id = %session.id, "could not fail a task with a quiet turn");
            }
        }
        Ok(())
    }

    async fn fail_quiet_task_turn(&self, session_id: SessionId, max_quiet_ms: i64) -> Result<()> {
        let hub = self.clone();
        let fx = self
            .store
            .write(move |tx| {
                let mut ctx = Ctx::load(&hub, tx, session_id)?;
                // Check again on the writer: an event may have come in.
                let quiet_since = ctx.runtime.active_at.unwrap_or(ctx.session.started_at);
                if ctx.session.state.is_final()
                    || !ctx.runtime.turn_open
                    || tx.now().as_millis() - quiet_since.as_millis() <= max_quiet_ms
                {
                    return Ok(Effects::default());
                }
                mark_lost_in_tx(&hub, tx, &mut ctx, "The turn sent nothing for too long.")?;
                ctx.set_failed_reason(format!(
                    "The session sent nothing for {} while its turn was open, so the hub stopped waiting. Retry the task.",
                    quiet_text(max_quiet_ms)
                ));
                ctx.persist(tx)?;
                Ok(ctx.fx)
            })
            .await?;
        self.run_effects(fx);
        Ok(())
    }

    /// Asks a worker to send its inventory again.
    pub(crate) async fn request_inventory_refresh(
        &self,
        worker_id: consort_proto::ids::WorkerId,
    ) -> Result<()> {
        self.worker_rpc(
            worker_id,
            HubToSessionHost::InventoryRefresh(InventoryRefresh {}),
        )
        .await?;
        Ok(())
    }
}

/// A time span for a status reason: whole hours, or minutes.
fn quiet_text(ms: i64) -> String {
    let minutes = (ms.max(1) + 59_999) / 60_000;
    if minutes % 60 == 0 {
        let hours = minutes / 60;
        if hours == 1 {
            "1 hour".to_owned()
        } else {
            format!("{hours} hours")
        }
    } else if minutes == 1 {
        "1 minute".to_owned()
    } else {
        format!("{minutes} minutes")
    }
}

/// Stops the open session of a task and asks each worker that ran the task
/// to delete its workspace. A task workspace is keyed by the task ID (see
/// DECISION 91), so one `workspace.release` per worker is enough. The
/// release goes in the queue of the newest session of the task on that
/// worker, after its `session.stop`, so the worker does not delete the
/// folder under a running harness. Returns the work to do after the commit.
pub(crate) fn release_task(
    hub: &Hub,
    tx: &mut crate::store::Tx<'_>,
    task: &Task,
    reason: &str,
) -> Result<Effects> {
    let mut fx = Effects::default();
    tx.set_task_workspace_released(task.id, true)?;
    let sessions = work::sessions_of_task(tx.db(), task.id)?;
    for session in &sessions {
        if !session.state.is_final() {
            // The task leaves, so its status does not change here.
            let mut ended = session.clone();
            ended.state = SessionState::Ended;
            ended.ended_at = Some(tx.now());
            ended.end_reason = Some(reason.to_owned());
            tx.save_session(&ended)?;
            let (approvals, decisions) = work::pending_for_session(tx.db(), session.id)?;
            for mut approval in approvals {
                approval.status = consort_proto::model::ApprovalStatus::Canceled;
                tx.save_approval(&approval)?;
            }
            for mut decision in decisions {
                decision.status = consort_proto::model::DecisionStatus::Canceled;
                tx.save_decision(&decision)?;
            }
            queue_stop(hub, tx, &mut fx, &ended, reason)?;
        }
    }
    // Each worker that the task used keeps a workspace.
    let mut workers: Vec<WorkerId> = Vec::new();
    for session in sessions.iter().rev() {
        if let Some(worker) = session.execution_worker_id
            && !workers.contains(&worker)
        {
            workers.push(worker);
        }
    }
    for worker in workers {
        let removed = machine::worker(tx.db(), worker)?.is_none_or(|w| w.revoked_at.is_some());
        if removed {
            continue;
        }
        tx.queue_exec_command(
            worker,
            &HubToExecution::WorkspaceRelease(ExecWorkspaceRelease {
                key: task.id.to_string(),
                delete: true,
            }),
        )?;
        fx.pump.push(worker);
    }
    // The session host keeps a folder for the harness of the task.
    let mut hosts: Vec<WorkerId> = Vec::new();
    for session in &sessions {
        if !hosts.contains(&session.worker_id) {
            hosts.push(session.worker_id);
        }
    }
    for host in hosts {
        let removed = machine::worker(tx.db(), host)?.is_none_or(|w| w.revoked_at.is_some());
        if removed {
            continue;
        }
        tx.queue_command(
            host,
            &HubToSessionHost::WorkspaceRelease(WorkspaceRelease {
                task_id: task.id,
                delete: true,
            }),
        )?;
        fx.pump.push(host);
    }
    Ok(fx)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use consort_proto::time::Timestamp;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::link::WorkerLink;
    use crate::net::ws::Outbox;

    /// Registers a link for the worker that connected `online_for` ago, and
    /// makes the session quiet for 10 s. Keep the receiver, so the link
    /// stays open.
    async fn quiet_session(
        hub: &Hub,
        session: &Session,
        online_for: Duration,
    ) -> tokio::sync::mpsc::Receiver<String> {
        let (outbox, rx) = Outbox::new(CancellationToken::new());
        let mut link = WorkerLink::new(hub.links.next_conn_id(), session.worker_id, outbox);
        link.connected_at = Instant::now().checked_sub(online_for).unwrap();
        hub.links.register(Arc::new(link));
        let id = session.id;
        hub.store
            .write(move |tx| {
                let mut runtime = work::session_runtime(tx.db(), id)?;
                runtime.active_at = Some(Timestamp::now().plus_millis(-10_000));
                tx.save_session_runtime(id, &runtime)
            })
            .await
            .unwrap();
        rx
    }

    /// Finding 11: a lost `turn.ended` (an `event_unreadable` in its place)
    /// kept a task in Working for ever.
    #[tokio::test]
    async fn a_task_turn_that_sends_nothing_for_too_long_fails_the_task() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let (task, session) = crate::hub::test_task_session(&hub).await;
        let _rx = quiet_session(&hub, &session, Duration::from_secs(60)).await;
        hub.expire_quiet_task_turns(1_000).await.unwrap();
        let (task, session) = hub
            .store
            .read(move |conn| {
                Ok((
                    work::task_or_404(conn, task.id)?,
                    work::session_or_404(conn, session.id)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        let reason = task.status_reason.unwrap();
        assert!(
            reason.contains("sent nothing") && reason.contains("Retry"),
            "{reason}"
        );
        assert_eq!(session.state, SessionState::Lost);
    }

    #[tokio::test]
    async fn a_quiet_turn_of_a_worker_that_just_came_back_waits() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let (task, session) = crate::hub::test_task_session(&hub).await;
        // The worker was offline. Its events may still wait in its spool.
        let _rx = quiet_session(&hub, &session, Duration::ZERO).await;
        hub.expire_quiet_task_turns(1_000).await.unwrap();
        let task = hub
            .store
            .read(move |conn| work::task_or_404(conn, task.id))
            .await
            .unwrap();
        assert_eq!(task.status, TaskStatus::Working);
    }

    /// Review 3, finding 10: an answer moved the task to Working but kept an
    /// old `active_at`, so the quiet-turn check could fail it at once.
    #[tokio::test]
    async fn an_answer_after_a_long_wait_does_not_fail_the_task_at_once() {
        use consort_proto::ids::ApprovalId;
        use consort_proto::model::{
            Approval, ApprovalDetails, ApprovalStatus, RequestOption, RequestOptionKind,
        };
        let (hub, _dir) = crate::hub::test_hub().await;
        let (task, session) = crate::hub::test_task_session(&hub).await;
        // The session went quiet 10 s ago, and its worker is online.
        let _rx = quiet_session(&hub, &session, Duration::from_secs(60)).await;
        let approval_id = ApprovalId::new();
        let (task_id, session_id) = (task.id, session.id);
        hub.store
            .write(move |tx| {
                let mut task = work::task_or_404(tx.db(), task_id)?;
                task.status = TaskStatus::NeedsInput;
                tx.save_task(&task)?;
                tx.save_approval(&Approval {
                    id: approval_id,
                    channel_id: task.channel_id,
                    session_id,
                    task_id: Some(task_id),
                    message_id: task.card_message_id,
                    request_id: "req-1".into(),
                    summary: "Run tests".to_owned(),
                    details: ApprovalDetails::Command {
                        blocked: Vec::new(),
                        check_note: None,
                        command: "cargo test".to_owned(),
                        cwd: None,
                    },
                    options: vec![RequestOption {
                        id: "allow".to_owned(),
                        label: "Allow once".to_owned(),
                        kind: RequestOptionKind::AllowOnce,
                        description: None,
                    }],
                    status: ApprovalStatus::Pending,
                    answer_option_id: None,
                    answered_at: None,
                    answered_by_client_id: None,
                    created_at: tx.now(),
                })
            })
            .await
            .unwrap();
        hub.approval_answer(
            &Caller::Hub,
            ApprovalAnswer {
                approval_id,
                option_id: "allow".to_owned(),
                checked: false,
                checked_hash: None,
            },
        )
        .await
        .unwrap();
        // The wait for the user does not count.
        hub.expire_quiet_task_turns(1_000).await.unwrap();
        let task = hub
            .store
            .read(move |conn| work::task_or_404(conn, task_id))
            .await
            .unwrap();
        assert_eq!(task.status, TaskStatus::Working);
    }

    #[test]
    fn a_quiet_time_reads_in_hours_or_minutes() {
        assert_eq!(quiet_text(2 * 60 * 60 * 1000), "2 hours");
        assert_eq!(quiet_text(60 * 60 * 1000), "1 hour");
        assert_eq!(quiet_text(90 * 60 * 1000), "90 minutes");
        assert_eq!(quiet_text(1_000), "1 minute");
    }
}
