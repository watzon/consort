//! Hub tools for tasks, decisions, workers and task sessions.

use consort_proto::ids::{DecisionId, TaskId};
use consort_proto::model::{
    Author, ChannelFile, Decision, DecisionStatus, MessageKind, PlacementRequest, TaskCreator,
    artifact_path, validate_decision,
};
use consort_proto::status::{Actor, TaskStatus};
use consort_proto::tools::*;
use consort_proto::turn_input::{SuggestionSource, WakeEvent};
use serde_json::Value;

use super::super::ops::flow::transition;
use super::super::ops::hooks::NewTask;
use crate::api::to_value;
use crate::error::{HubError, Result};
use crate::hub::{Hub, ToolCtx};
use crate::store::{conversation, machine, work};

/// The most items `read_task` returns.
const READ_TASK_ITEMS: usize = 50;

fn own_task(
    conn: &rusqlite::Connection,
    ctx: &ToolCtx,
    id: TaskId,
) -> Result<consort_proto::model::Task> {
    work::task(conn, id)?
        .filter(|t| t.channel_id == ctx.channel.id)
        .ok_or_else(|| HubError::not_found("This channel has no task with this ID."))
}

fn session_task(ctx: &ToolCtx) -> Result<&consort_proto::model::Task> {
    ctx.task
        .as_ref()
        .ok_or_else(|| HubError::invalid("This tool works only in a task session."))
}

impl Hub {
    pub(crate) async fn tool_list_workers(&self, _ctx: &ToolCtx, _args: NoArgs) -> Result<Value> {
        let workers = self.store.read(machine::workers).await?;
        to_value(&ListWorkersResult {
            workers: workers
                .iter()
                .filter(|w| w.revoked_at.is_none())
                .map(WorkerSummary::from_worker)
                .collect(),
        })
    }

    pub(crate) async fn tool_ask_decision(
        &self,
        ctx: &ToolCtx,
        args: AskDecisionArgs,
    ) -> Result<Value> {
        validate_decision(&args.question, &args.options, args.recommended_index)?;
        let (session, channel, task) = (ctx.session.clone(), ctx.channel.clone(), ctx.task.clone());
        let (decision_id, kick) = self
            .store
            .write(move |tx| {
                let body = args.question.clone();
                let card = match &task {
                    Some(task) => tx.post_in_task_thread(
                        task,
                        Some(&session),
                        Author::Assistant,
                        MessageKind::Decision,
                        &body,
                    )?,
                    None => {
                        let mut new = conversation::NewMessage::new(
                            channel.id,
                            Author::Assistant,
                            MessageKind::Decision,
                            body,
                        );
                        new.author_session_id = Some(session.id);
                        tx.insert_message(new)?
                    }
                };
                let decision = Decision {
                    id: DecisionId::new(),
                    channel_id: channel.id,
                    task_id: task.as_ref().map(|t| t.id),
                    session_id: session.id,
                    message_id: card.id,
                    request_id: None,
                    question: args.question,
                    context: args.context,
                    options: args.options,
                    recommended_index: args.recommended_index,
                    allow_free_text: args.allow_free_text,
                    status: DecisionStatus::Pending,
                    answer: None,
                    answered_at: None,
                    created_at: tx.now(),
                };
                tx.save_decision(&decision)?;
                let mut kick = false;
                if let Some(task) = &task {
                    let mut fresh = work::task_or_404(tx.db(), task.id)?;
                    let wake = format!("Waiting for the user: {}", decision.question);
                    if transition(
                        tx,
                        &mut fresh,
                        TaskStatus::NeedsInput,
                        Actor::System,
                        Some(decision.question.clone()),
                        Some(wake),
                    )? {
                        tx.save_task(&fresh)?;
                        kick = true;
                    }
                }
                Ok((decision.id, kick))
            })
            .await?;
        if kick {
            self.kick_coordinator(ctx.channel.id);
        }
        to_value(&AskDecisionResult { decision_id })
    }

    pub(crate) async fn tool_start_task(
        &self,
        ctx: &ToolCtx,
        args: StartTaskArgs,
    ) -> Result<Value> {
        let task = self
            .create_task(NewTask {
                channel_id: ctx.channel.id,
                title: args.title,
                brief: args.brief,
                model_selection: args.model_selection,
                placement: PlacementRequest {
                    worker_id: args.worker_id,
                    require: args.require,
                    repos: args.repos,
                    isolation: args.isolation,
                },
                approval_mode: args.approval_mode,
                start_at: args.start_at,
                created_by: TaskCreator::Assistant,
                routine_id: None,
            })
            .await?;
        to_value(&StartTaskResult {
            task_id: task.id,
            status: task.status,
            status_reason: task.status_reason,
        })
    }

    pub(crate) async fn tool_message_task(
        &self,
        ctx: &ToolCtx,
        args: MessageTaskArgs,
    ) -> Result<Value> {
        let (task_id, channel_id) = (args.task_id, ctx.channel.id);
        let task = self
            .store
            .read(move |conn| {
                work::task(conn, task_id)?
                    .filter(|t| t.channel_id == channel_id)
                    .ok_or_else(|| HubError::not_found("This channel has no task with this ID."))
            })
            .await?;
        let delivery = self
            .deliver_to_task(task.id, args.body.clone(), Actor::Coordinator)
            .await?;
        let session = ctx.session.clone();
        let body = args.body;
        self.store
            .write(move |tx| {
                tx.post_in_task_thread(
                    &task,
                    Some(&session),
                    Author::Assistant,
                    MessageKind::Text,
                    &body,
                )
                .map(|_| ())
            })
            .await?;
        to_value(&MessageTaskResult {
            task_id: args.task_id,
            delivery,
        })
    }

    pub(crate) async fn tool_list_tasks(
        &self,
        ctx: &ToolCtx,
        args: ListTasksArgs,
    ) -> Result<Value> {
        let channel_id = ctx.channel.id;
        let tasks = self
            .store
            .read(move |conn| work::tasks_in_channel(conn, channel_id, false))
            .await?;
        to_value(&ListTasksResult {
            tasks: tasks
                .into_iter()
                .filter(|t| args.status.is_none_or(|s| t.status == s))
                .collect(),
        })
    }

    pub(crate) async fn tool_read_task(&self, ctx: &ToolCtx, args: ReadTaskArgs) -> Result<Value> {
        let ctx = ctx.clone();
        let result = self
            .store
            .read(move |conn| {
                let task = own_task(conn, &ctx, args.task_id)?;
                let items = match args.since_item_id {
                    Some(after) => {
                        work::task_items(conn, task.id, None, Some(after), READ_TASK_ITEMS)?.0
                    }
                    None => work::recent_task_items(conn, task.id, READ_TASK_ITEMS)?,
                };
                Ok(ReadTaskResult { task, items })
            })
            .await?;
        to_value(&result)
    }

    pub(crate) async fn tool_interrupt_task(
        &self,
        ctx: &ToolCtx,
        args: TaskIdArgs,
    ) -> Result<Value> {
        let ctx_copy = ctx.clone();
        let task = self
            .store
            .read(move |conn| own_task(conn, &ctx_copy, args.task_id))
            .await?;
        self.interrupt_task(task.id).await?;
        to_value(&TaskStatusResult {
            task_id: task.id,
            status: task.status,
        })
    }

    pub(crate) async fn tool_retry_task(&self, ctx: &ToolCtx, args: TaskIdArgs) -> Result<Value> {
        let ctx_copy = ctx.clone();
        self.store
            .read(move |conn| own_task(conn, &ctx_copy, args.task_id).map(|_| ()))
            .await?;
        let task = self.retry_task(args.task_id, Actor::Coordinator).await?;
        to_value(&TaskStatusResult {
            task_id: task.id,
            status: task.status,
        })
    }

    pub(crate) async fn tool_suggest_memory(
        &self,
        ctx: &ToolCtx,
        args: SuggestMemoryArgs,
    ) -> Result<Value> {
        let (source, target) = match &ctx.task {
            Some(task) => (SuggestionSource::Task(task.id), ctx.channel.id),
            None => {
                if ctx.channel.is_everything() {
                    return Err(HubError::invalid(
                        "The Everything coordinator writes global memory itself. Use memory_write.",
                    ));
                }
                let everything = self.store.read(conversation::everything_channel).await?;
                (SuggestionSource::Channel(ctx.channel.id), everything.id)
            }
        };
        self.store
            .write(move |tx| {
                tx.queue_wake(
                    target,
                    &WakeEvent::MemorySuggestion {
                        from: source,
                        reason: args.reason,
                        note: args.note,
                    },
                )
            })
            .await?;
        self.kick_coordinator(target);
        to_value(&OkResult::OK)
    }

    pub(crate) async fn tool_post_update(
        &self,
        ctx: &ToolCtx,
        args: PostUpdateArgs,
    ) -> Result<Value> {
        let task = session_task(ctx)?.clone();
        let session = ctx.session.clone();
        if args.body.trim().is_empty() {
            return Err(HubError::invalid(
                "The update is empty. Write what changed.",
            ));
        }
        let message = self
            .store
            .write(move |tx| {
                tx.post_in_task_thread(
                    &task,
                    Some(&session),
                    Author::Assistant,
                    MessageKind::Text,
                    &args.body,
                )
            })
            .await?;
        to_value(&MessageIdResult {
            message_id: message.id,
        })
    }

    pub(crate) async fn tool_submit_for_review(
        &self,
        ctx: &ToolCtx,
        args: SubmitForReviewArgs,
    ) -> Result<Value> {
        session_task(ctx)?;
        let session_id = ctx.session.id;
        let summary = args.summary.trim().to_owned();
        if summary.is_empty() {
            return Err(HubError::invalid(
                "The summary is empty. Say what you did and what to check.",
            ));
        }
        self.store
            .write(move |tx| {
                let mut runtime = work::session_runtime(tx.db(), session_id)?;
                runtime.review_summary = Some(summary);
                tx.save_session_runtime(session_id, &runtime)
            })
            .await?;
        to_value(&OkResult::OK)
    }

    pub(crate) async fn tool_publish_artifact(
        &self,
        ctx: &ToolCtx,
        args: PublishArtifactArgs,
    ) -> Result<Value> {
        let task = session_task(ctx)?.clone();
        let (Some(sha), Some(size)) = (args.blob_sha256.clone(), args.size) else {
            return Err(HubError::invalid(
                "The file was not uploaded. The worker uploads it before it calls this tool.",
            ));
        };
        if !self.store.blobs_dir().join(sha.as_str()).exists() {
            return Err(HubError::invalid(
                "The hub has no blob for this file. Upload it again.",
            ));
        }
        let path = artifact_path(task.id, &args.path);
        consort_proto::model::validate_relative_path("path", &path)?;
        let file = ChannelFile {
            channel_id: ctx.channel.id,
            path: path.clone(),
            blob_sha256: sha,
            size,
            mime: args
                .mime
                .unwrap_or_else(|| "application/octet-stream".to_owned()),
            task_id: Some(task.id),
            title: Some(args.title),
            updated_by: ctx.session.id.to_string(),
            updated_at: consort_proto::time::Timestamp::now(),
            deleted_at: None,
        };
        self.store
            .write(move |tx| tx.save_channel_file(&file))
            .await?;
        to_value(&PublishArtifactResult { path })
    }
}
