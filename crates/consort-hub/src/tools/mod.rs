//! Hub tool dispatch for `tool.list` and `tool.call`.

use consort_proto::ids::SessionId;
use consort_proto::model::SessionRole;
use consort_proto::tools::{HubToolCall, ToolAudience, ToolCallResult, ToolDefinition, tools_for};
use serde_json::Value;

use crate::error::{HubError, Result};
use crate::hub::{Hub, ToolCtx};
use crate::store::{conversation, work};

mod chat;
mod routines;
mod tasks;

impl Hub {
    /// Builds the tool context of a session.
    pub(crate) async fn tool_ctx(&self, session_id: SessionId) -> Result<ToolCtx> {
        self.store
            .read(move |conn| {
                let session = work::session_or_404(conn, session_id)?;
                let channel = conversation::channel_or_404(conn, session.channel_id)?;
                let task = match (session.role, session.task_id) {
                    (SessionRole::Task, Some(id)) => work::task(conn, id)?,
                    _ => None,
                };
                let audience = ToolAudience::for_session(session.role, channel.kind);
                Ok(ToolCtx {
                    session,
                    channel,
                    task,
                    audience,
                })
            })
            .await
    }

    /// The `tool.list` result for a session.
    pub(crate) async fn tool_definitions(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<ToolDefinition>> {
        let ctx = self.tool_ctx(session_id).await?;
        Ok(tools_for(ctx.audience))
    }

    /// Runs one `tool.call`. Errors become an `is_error` result.
    pub(crate) async fn call_tool(
        &self,
        session_id: SessionId,
        name: &str,
        arguments: Value,
    ) -> ToolCallResult {
        match self.run_tool(session_id, name, arguments).await {
            Ok(value) => ToolCallResult::json(&value),
            Err(err) => ToolCallResult::error(&err.to_error_body()),
        }
    }

    async fn run_tool(&self, session_id: SessionId, name: &str, arguments: Value) -> Result<Value> {
        let ctx = self.tool_ctx(session_id).await?;
        let call = HubToolCall::parse_for(ctx.audience, name, arguments)
            .map_err(|e| HubError::Api(e.to_error_body()))?;
        self.note_tool_call(&ctx, call.tool()).await?;
        let ctx = &ctx;
        use HubToolCall as C;
        match call {
            C::MemoryList(a) => self.tool_memory_list(ctx, a).await,
            C::MemoryRead(a) => self.tool_memory_read(ctx, a).await,
            C::FilesList(a) => self.tool_files_list(ctx, a).await,
            C::ListWorkers(a) => self.tool_list_workers(ctx, a).await,
            C::AskDecision(a) => self.tool_ask_decision(ctx, a).await,
            C::Search(a) => self.tool_search(ctx, a).await,
            C::PostMessage(a) => self.tool_post_message(ctx, a).await,
            C::Reply(a) => self.tool_reply(ctx, a).await,
            C::UpdateMessage(a) => self.tool_update_message(ctx, a).await,
            C::React(a) => self.tool_react(ctx, a).await,
            C::NoReplyNeeded(a) => self.tool_no_reply_needed(ctx, a).await,
            C::FetchTimeline(a) => self.tool_fetch_timeline(ctx, a).await,
            C::FetchThread(a) => self.tool_fetch_thread(ctx, a).await,
            C::MemoryWrite(a) => self.tool_memory_write(ctx, a).await,
            C::MemoryDelete(a) => self.tool_memory_delete(ctx, a).await,
            C::FilesWrite(a) => self.tool_files_write(ctx, a).await,
            C::StartTask(a) => self.tool_start_task(ctx, a).await,
            C::MessageTask(a) => self.tool_message_task(ctx, a).await,
            C::ListTasks(a) => self.tool_list_tasks(ctx, a).await,
            C::ReadTask(a) => self.tool_read_task(ctx, a).await,
            C::InterruptTask(a) => self.tool_interrupt_task(ctx, a).await,
            C::RetryTask(a) => self.tool_retry_task(ctx, a).await,
            C::RoutineList(a) => self.tool_routine_list(ctx, a).await,
            C::RoutineCreate(a) => self.tool_routine_create(ctx, a).await,
            C::RoutineUpdate(a) => self.tool_routine_update(ctx, a).await,
            C::RoutineDelete(a) => self.tool_routine_delete(ctx, a).await,
            C::SuggestMemory(a) => self.tool_suggest_memory(ctx, a).await,
            C::ListChannels(a) => self.tool_list_channels(ctx, a).await,
            C::ReadChannel(a) => self.tool_read_channel(ctx, a).await,
            C::MessageChannel(a) => self.tool_message_channel(ctx, a).await,
            C::PostUpdate(a) => self.tool_post_update(ctx, a).await,
            C::SubmitForReview(a) => self.tool_submit_for_review(ctx, a).await,
            C::PublishArtifact(a) => self.tool_publish_artifact(ctx, a).await,
        }
    }
}
