//! Hub tools for tasks, decisions, workers and task sessions.

use consort_proto::ids::{DecisionId, TaskId};
use consort_proto::model::{
    ApprovalMode, Author, Channel, ChannelFile, Decision, DecisionStatus, Isolation, MessageKind,
    PlacementRequest, TaskCreator, artifact_path, validate_decision,
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
use crate::limits::{REVIEW_SUMMARY_MAX_BYTES, TOOL_MESSAGES_MAX_BYTES, cut_text, preview_task};
use crate::store::{conversation, machine, work};

/// The items that `read_task` returns when the call gives no limit, and the
/// most that it returns.
const READ_TASK_ITEMS: u32 = 50;
const READ_TASK_ITEMS_MAX: u32 = 200;

fn own_task(
    conn: &rusqlite::Connection,
    ctx: &ToolCtx,
    id: TaskId,
) -> Result<consort_proto::model::Task> {
    work::task(conn, id)?
        .filter(|t| t.channel_id == ctx.channel.id)
        .ok_or_else(|| HubError::not_found("This channel has no task with this ID."))
}

/// What [`limit_to_channel`] changed, in two forms: for the coordinator in
/// the tool result, and for the user in the task thread.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Limited {
    pub for_assistant: Vec<String>,
    pub for_user: Vec<String>,
}

impl Limited {
    pub(crate) fn is_empty(&self) -> bool {
        self.for_assistant.is_empty()
    }
}

fn isolation_name(isolation: Isolation) -> &'static str {
    match isolation {
        Isolation::InPlace => "in place",
        Isolation::Worktree => "in a worktree",
        Isolation::Clone => "in a fresh clone",
    }
}

/// Lowers what the assistant asks for a task to what the channel allows.
/// Text in a web page, a file or a message can steer a coordinator, so the
/// channel settings that the user chose are the limit:
///
/// - the approval mode is at most the default approval mode of the channel,
/// - the isolation is in place only when the default isolation of the
///   channel is in place,
/// - the task gets only the local MCP servers that the channel allows.
///
/// The hub does not refuse the task. It runs with less, and the result
/// says what changed, so the coordinator can tell the user. A task that
/// the user starts from a client is not limited.
pub(crate) fn limit_to_channel(
    channel: &Channel,
    placement: &mut PlacementRequest,
    approval_mode: &mut Option<ApprovalMode>,
) -> Limited {
    let mut limited = Limited::default();
    let allowed_mode = channel.default_approval_mode;
    if let Some(mode) = *approval_mode
        && mode > allowed_mode
    {
        let (asked, allowed) = (
            crate::placement::mode_name(mode),
            crate::placement::mode_name(allowed_mode),
        );
        limited.for_assistant.push(format!(
            "This channel allows at most the approval mode {allowed} for a task that you start, so the task runs in {allowed}, not in {asked}. If the task needs more, ask the user to start it, or to raise the default approval mode of the channel."
        ));
        limited.for_user.push(format!(
            "The assistant asked for the approval mode {asked}. This channel allows at most {allowed}, so the task runs in {allowed}."
        ));
        *approval_mode = Some(allowed_mode);
    }
    if placement.isolation == Some(Isolation::InPlace)
        && channel.default_isolation != Isolation::InPlace
    {
        let instead = isolation_name(channel.default_isolation);
        limited.for_assistant.push(format!(
            "This channel does not let a task that you start run in place, so the task runs {instead}. If it must change the user's own checkout, ask the user to start it, or to set the default isolation of the channel to in place."
        ));
        limited.for_user.push(format!(
            "The assistant asked to run the task in place. This channel does not allow that for the assistant, so the task runs {instead}."
        ));
        placement.isolation = Some(channel.default_isolation);
    }
    if let Some(require) = placement.require.as_mut() {
        let (kept, refused): (Vec<String>, Vec<String>) =
            std::mem::take(&mut require.local_mcp_servers)
                .into_iter()
                .partition(|name| channel.allowed_local_mcp_servers.contains(name));
        require.local_mcp_servers = kept;
        if !refused.is_empty() {
            let names = refused.join(", ");
            limited.for_assistant.push(format!(
                "This channel does not let a task that you start use the local MCP servers {names}, so the task runs without them. If it needs them, ask the user to add them to the allowed local MCP servers of the channel, or to start the task."
            ));
            limited.for_user.push(format!(
                "The assistant asked for the local MCP servers {names}. This channel does not allow them for the assistant, so the task runs without them."
            ));
        }
    }
    limited
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
        mut args: AskDecisionArgs,
    ) -> Result<Value> {
        if args.offer {
            // An offer always has the same three options.
            args.options = crate::proactivity::tools::offer_options();
            args.recommended_index = None;
        }
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
                        let mark = crate::proactivity::tools::decision_posting(
                            tx, session.id, &channel, args.offer,
                        )?;
                        let mut new = conversation::NewMessage::new(
                            channel.id,
                            Author::Assistant,
                            MessageKind::Decision,
                            body,
                        );
                        new.author_session_id = Some(session.id);
                        let mut card = tx.insert_message(new)?;
                        if mark.is_some() {
                            card.proactive = mark;
                            tx.save_message(&card)?;
                        }
                        card
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
        // Read the channel again: the user may have changed its limits
        // since the session started.
        let channel_id = ctx.channel.id;
        let paths = args.attachments.clone();
        let session_id = ctx.session.id;
        let requested = args.approval_mode;
        let (channel, attachments, (proactive_mode, origin)) = self
            .store
            .read(move |conn| {
                let channel = conversation::channel_or_404(conn, channel_id)?;
                let attachments = super::chat::resolve_attachments(conn, channel_id, &paths)?;
                // A task that a proactive turn starts runs in draft mode.
                let mode = crate::proactivity::tools::task_approval_mode(
                    conn, session_id, &channel, requested,
                )?;
                Ok((channel, attachments, mode))
            })
            .await?;
        let mut placement = PlacementRequest {
            worker_id: args.worker_id,
            require: args.require,
            repos: args.repos,
            isolation: args.isolation,
        };
        let mut approval_mode = proactive_mode;
        let limited = limit_to_channel(&channel, &mut placement, &mut approval_mode);
        if !limited.is_empty() {
            tracing::info!(channel_id = %channel_id, session_id = %ctx.session.id, "a coordinator asked for more than the channel allows, so the hub lowered the task");
        }
        let task = self
            .create_task(NewTask {
                channel_id,
                title: args.title,
                brief: args.brief,
                model_selection: args.model_selection,
                placement,
                approval_mode,
                start_at: args.start_at,
                created_by: TaskCreator::Assistant,
                routine_id: None,
                attachments,
                proactive: origin,
            })
            .await?;
        if !limited.is_empty() {
            // The user sees it in the task thread too, not only the
            // coordinator that the text may steer.
            let text = format!(
                "{} To allow more, change the settings of the channel.",
                limited.for_user.join(" ")
            );
            let thread = task.clone();
            self.store
                .write(move |tx| {
                    tx.post_notice(
                        thread.channel_id,
                        Some(thread.card_message_id),
                        Some(thread.id),
                        "task_limited",
                        &text,
                    )
                    .map(|_| ())
                })
                .await?;
        }
        let mut notes = limited.for_assistant;
        if origin.is_some() {
            self.note_turn_task(ctx, task.id).await?;
        }
        if origin.is_some_and(|(_, draft)| draft) {
            notes.push(
                "You started this task on your own, so it runs in draft mode: every outward action waits for the user.".to_owned(),
            );
        }
        to_value(&StartTaskResult {
            task_id: task.id,
            status: task.status,
            status_reason: task.status_reason,
            notes,
        })
    }

    pub(crate) async fn tool_message_task(
        &self,
        ctx: &ToolCtx,
        args: MessageTaskArgs,
    ) -> Result<Value> {
        let (task_id, channel_id) = (args.task_id, ctx.channel.id);
        self.check_steer_task(ctx, task_id).await?;
        let paths = args.attachments.clone();
        let (task, attachments) = self
            .store
            .read(move |conn| {
                let task = work::task(conn, task_id)?
                    .filter(|t| t.channel_id == channel_id)
                    .ok_or_else(|| HubError::not_found("This channel has no task with this ID."))?;
                let attachments = super::chat::resolve_attachments(conn, channel_id, &paths)?;
                Ok((task, attachments))
            })
            .await?;
        let delivery = self
            .deliver_to_task(
                task.id,
                args.body.clone(),
                attachments.clone(),
                Actor::Coordinator,
            )
            .await?;
        let session = ctx.session.clone();
        let body = args.body;
        self.store
            .write(move |tx| {
                let mut new = conversation::NewMessage::new(
                    task.channel_id,
                    Author::Assistant,
                    MessageKind::Text,
                    body,
                );
                new.thread_root_id = Some(task.card_message_id);
                new.author_task_id = Some(task.id);
                new.author_session_id = Some(session.id);
                new.attachments = attachments;
                tx.insert_message(new).map(|_| ())
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
        // Long texts are previews here. read_task gives the full task.
        to_value(&ListTasksResult {
            tasks: tasks
                .into_iter()
                .filter(|t| args.status.is_none_or(|s| t.status == s))
                .map(|mut t| {
                    preview_task(&mut t);
                    t
                })
                .collect(),
        })
    }

    /// The task in full, and a page of its items that fits in
    /// [`TOOL_MESSAGES_MAX_BYTES`] with the task. The page always has at
    /// least one item when one exists: the worker cuts every event to 192
    /// KiB, so one item fits next to the task.
    pub(crate) async fn tool_read_task(&self, ctx: &ToolCtx, args: ReadTaskArgs) -> Result<Value> {
        let ctx = ctx.clone();
        let limit = usize::try_from(
            args.limit
                .unwrap_or(READ_TASK_ITEMS)
                .clamp(1, READ_TASK_ITEMS_MAX),
        )
        .unwrap_or(1);
        let result = self
            .store
            .read(move |conn| {
                let task = own_task(conn, &ctx, args.task_id)?;
                let mut used = serde_json::to_vec(&task)?.len();
                let budget = TOOL_MESSAGES_MAX_BYTES;
                let mut items = Vec::new();
                let has_more;
                match args.since_item_id {
                    // Items after `since`, oldest first.
                    Some(after) => {
                        let (page, more) =
                            work::task_items(conn, task.id, None, Some(after), limit)?;
                        let mut stopped = false;
                        for item in page {
                            let cost = serde_json::to_vec(&item)?.len() + 1;
                            if used + cost > budget && !items.is_empty() {
                                stopped = true;
                                break;
                            }
                            used += cost;
                            items.push(item);
                        }
                        has_more = more || stopped;
                    }
                    // The newest items, or the newest ones older than
                    // `before_item_id`.
                    None => {
                        let page =
                            work::task_items_before(conn, task.id, args.before_item_id, limit + 1)?;
                        let mut taken = 0;
                        for item in page.iter().take(limit) {
                            let cost = serde_json::to_vec(item)?.len() + 1;
                            if used + cost > budget && taken > 0 {
                                break;
                            }
                            used += cost;
                            taken += 1;
                        }
                        has_more = page.len() > taken;
                        items.extend(page.into_iter().take(taken));
                        items.reverse();
                    }
                }
                Ok(ReadTaskResult {
                    task,
                    items,
                    has_more,
                })
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
        self.check_steer_task(ctx, args.task_id).await?;
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
                if ctx.channel.is_tutti() {
                    return Err(HubError::invalid(
                        "The Tutti coordinator writes global memory itself. Use memory_write.",
                    ));
                }
                let tutti = self.store.read(conversation::tutti_channel).await?;
                (SuggestionSource::Channel(ctx.channel.id), tutti.id)
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
        let summary = args.summary.trim();
        if summary.is_empty() {
            return Err(HubError::invalid(
                "The summary is empty. Say what you did and what to check.",
            ));
        }
        // The text comes from the harness, so the hub cuts it and does not refuse it.
        let summary = cut_text(summary, REVIEW_SUMMARY_MAX_BYTES);
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
        // The size comes from the blob that the hub stored, not from the
        // call, so the channel file always tells the truth.
        let stored = tokio::fs::metadata(self.store.blobs_dir().join(sha.as_str()))
            .await
            .ok()
            .filter(|m| m.is_file())
            .ok_or_else(|| {
                HubError::invalid("The hub has no blob for this file. Upload it again.")
            })?;
        if stored.len() != size {
            tracing::warn!(session_id = %ctx.session.id, claimed = size, stored = stored.len(), "an artifact call gave a size that does not match its blob");
        }
        let size = stored.len();
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
