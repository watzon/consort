//! Tasks: creation, placement, and the client API handlers.

use consort_proto::client_api::*;
use consort_proto::frame::Empty;
use consort_proto::ids::SessionId;
use consort_proto::ids::TaskId;
use consort_proto::model::Isolation;
use consort_proto::model::{
    Author, Channel, FileRef, MessageKind, Repo, Session, SessionRole, SessionState, Task, Usage,
    ValidationError,
};
use consort_proto::status::{Actor, TaskStatus};
use consort_proto::worker_link::{
    HubToWorker, InventoryRefresh, ItemDetail, ItemDetailResult, SessionInterrupt, SessionStart,
    SessionStop, TurnInput, WorkspaceRepo, WorkspaceSpec,
};

use super::flow::transition;
use super::hooks::NewTask;
use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::limits::{BRIEF_MAX_BYTES, check_text};
use crate::link::events::{Effects, end_session_by_hub};
use crate::placement::{Placement, PlacementError, PlacementInput, Subject, place};
use crate::prompts;
use crate::store::conversation::{self, NewMessage};
use crate::store::{knowledge, machine, work};

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
                    archived_at: None,
                    created_at: now,
                    updated_at: now,
                };
                tx.save_task(&task)?;
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
                let files = knowledge::channel_files(conn, channel.id, None)?;
                Ok(Some((task, channel, workers, open, repos, files)))
            })
            .await?;
        let Some((task, channel, workers, open, all_repos, files)) = inputs else {
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
        let selection = task
            .model_selection
            .clone()
            .or_else(|| channel.task_selection.clone());
        let input = PlacementInput {
            subject: Subject::Task,
            workers: &workers,
            open_sessions: &open,
            selection: selection.as_ref(),
            request: &task.placement,
            approval_mode: task.approval_mode,
            preferred_workers: &channel.preferred_workers,
            repos: &repos,
            isolation,
        };
        match place(&input) {
            Ok(placement) => {
                let files: Vec<FileRef> = files
                    .iter()
                    .map(|f| FileRef {
                        path: f.path.clone(),
                        sha256: f.blob_sha256.clone(),
                    })
                    .collect();
                self.start_task_session(task, channel, placement, &repos, isolation, files)
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

    async fn start_task_session(
        &self,
        task: Task,
        channel: Channel,
        placement: Placement,
        repos: &[Repo],
        isolation: Isolation,
        files: Vec<FileRef>,
    ) -> Result<()> {
        let worker_id = placement.worker_id;
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
        let system_prompt =
            prompts::task_system_prompt(&channel, &task, &global_index, &channel_index);
        let first_turn = prompts::task_first_turn(&task);
        let local_mcp = task
            .placement
            .require
            .as_ref()
            .map(|r| r.local_mcp_servers.clone())
            .unwrap_or_default();
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
                tx.save_session_runtime(
                    session.id,
                    &work::SessionRuntime {
                        turn_open: true,
                        ..work::SessionRuntime::default()
                    },
                )?;
                tx.queue_command(
                    worker_id,
                    &HubToWorker::SessionStart(SessionStart {
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
                        channel_files: files,
                        resume: None,
                        turn: Some(TurnInput {
                            text: first_turn,
                            attachments: Vec::new(),
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
                task.worker_id = Some(worker_id);
                tx.save_task(&task)?;
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
        Ok(())
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
                    || request.model_selection.is_some()
                    || request.placement.is_some()
                    || request.approval_mode.is_some()
                    || request.start_at.is_some();
                if wants_run_change && !not_started {
                    return Err(HubError::conflict(
                        "The task already started. You can change only its title now.",
                    ));
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

    /// Sends a failed task back to Up next and places it again. The old
    /// session may still run on its worker (a failed turn does not end it),
    /// so the hub ends it and the worker gets `session.stop`.
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
                // The old session ends first, while the task is still
                // Failed, so ending it does not change the task.
                let fx = match task.current_session_id {
                    Some(old) => end_session_by_hub(&hub, tx, old, "The task was retried.")?,
                    None => Effects::default(),
                };
                let mut task = work::task_or_404(tx.db(), id)?;
                if transition(tx, &mut task, TaskStatus::UpNext, actor, None, None)? {
                    task.current_session_id = None;
                    task.worker_id = None;
                    task.review_summary = None;
                    tx.save_task(&task)?;
                }
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
            HubToWorker::SessionInterrupt(SessionInterrupt {
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
        let task = self
            .store
            .write(move |tx| {
                let mut task = work::task_or_404(tx.db(), id)?;
                let archived_at = request.archived.then(|| tx.now());
                if task.archived_at.is_some() != archived_at.is_some() {
                    task.archived_at = archived_at;
                    task.updated_at = tx.now();
                    tx.save_task(&task)?;
                }
                Ok(task)
            })
            .await?;
        Ok(TaskResult { task })
    }

    pub(crate) async fn task_delete(&self, _caller: &Caller, request: TaskDelete) -> Result<Empty> {
        let id = request.task_id;
        self.stop_task_session(id).await?;
        self.store
            .write(move |tx| {
                let task = work::task_or_404(tx.db(), id)?;
                tx.purge_task(&task)
            })
            .await?;
        Ok(Empty {})
    }

    /// Stops the session of a task and tells the worker to release the
    /// workspace. The commands wait in the queue when the worker is offline.
    pub(crate) async fn stop_task_session(&self, id: TaskId) -> Result<()> {
        let worker = self
            .store
            .write(move |tx| {
                let Some(task) = work::task(tx.db(), id)? else {
                    return Ok(None);
                };
                let Some(session_id) = task.current_session_id else {
                    return Ok(None);
                };
                let Some(session) = work::session(tx.db(), session_id)? else {
                    return Ok(None);
                };
                if session.state.is_final() {
                    return Ok(None);
                }
                tx.queue_command(
                    session.worker_id,
                    &HubToWorker::SessionStop(SessionStop {
                        session_id,
                        reason: "The task was removed.".to_owned(),
                    }),
                )?;
                Ok(Some(session.worker_id))
            })
            .await?;
        if let Some(worker_id) = worker {
            self.pump_worker(worker_id);
        }
        Ok(())
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
                HubToWorker::ItemDetail(ItemDetail {
                    session_id: request.session_id,
                    item_id: request.item_id,
                }),
            )
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    /// Asks a worker to send its inventory again.
    pub(crate) async fn request_inventory_refresh(
        &self,
        worker_id: consort_proto::ids::WorkerId,
    ) -> Result<()> {
        self.worker_rpc(
            worker_id,
            HubToWorker::InventoryRefresh(InventoryRefresh {}),
        )
        .await?;
        Ok(())
    }
}
