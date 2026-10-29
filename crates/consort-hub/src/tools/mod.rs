//! Hub tool dispatch for `tool.list` and `tool.call`.

use consort_proto::error::ErrorBody;
use consort_proto::ids::SessionId;
use consort_proto::model::SessionRole;
use consort_proto::tools::{
    HubTool, HubToolCall, ToolAudience, ToolCallResult, ToolDefinition, tools_for,
};
use serde_json::Value;

use crate::error::{HubError, Result};
use crate::hub::{Hub, ToolCtx};
use crate::store::{conversation, work};

mod chat;
mod routines;
mod tasks;

/// The hub tools that the proto defines but the hub does not carry out yet
/// (docs/CONNECTORS-SKILLS-PLAN.md). `tool.list` leaves them out, so no
/// session sees them, and a call gets `unsupported`. Take a tool out of
/// this list when its handler is built.
pub(crate) const NOT_BUILT: &[HubTool] = &[
    HubTool::ListConnectorTools,
    HubTool::GetConnectorTool,
    HubTool::RunConnectorTool,
    HubTool::ListSkills,
    HubTool::ReadSkill,
    HubTool::ListConnections,
    HubTool::ListInstalledSkills,
    HubTool::ConnectService,
    HubTool::AddSkill,
    HubTool::CreateChannel,
    HubTool::UpdateChannel,
    HubTool::ArchiveChannel,
    HubTool::SetDefaultModel,
    HubTool::SetWorkerTags,
    HubTool::InstallTool,
    HubTool::AddAccount,
    HubTool::StartSignIn,
    HubTool::SetAccountExtras,
    HubTool::SearchConnectorCatalog,
    HubTool::UpdateConnection,
    HubTool::Disconnect,
    HubTool::SearchSkills,
    HubTool::UpdateSkill,
    HubTool::RemoveSkill,
    HubTool::UpdateThisChannel,
    HubTool::SetChannelTags,
    HubTool::SetChannelConnector,
    HubTool::SetChannelSkill,
    HubTool::RequestAccess,
];

impl Hub {
    /// Builds the tool context of a session. A session that has ended or is
    /// lost has no hub tools: the hub no longer runs turns for it, so what
    /// it does could not be seen or undone.
    pub(crate) async fn tool_ctx(&self, session_id: SessionId) -> Result<ToolCtx> {
        self.store
            .read(move |conn| {
                let session = work::session_or_404(conn, session_id)?;
                if session.state.is_final() {
                    return Err(HubError::conflict(
                        "This session has ended, so it cannot use hub tools. Stop the session.",
                    ));
                }
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
        Ok(tools_for(ctx.audience)
            .into_iter()
            .filter(|t| HubTool::from_name(&t.name).is_none_or(|tool| !NOT_BUILT.contains(&tool)))
            .collect())
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
        let tool = call.tool();
        let result = self.dispatch_tool(&ctx, call).await;
        // Only a call that worked counts for the fallback post. A failed
        // `post_message` posted nothing, so the turn still needs a post.
        if result.is_ok()
            && let Err(err) = self.note_tool_call(&ctx, tool).await
        {
            tracing::warn!(error = %err, tool = tool.name(), "could not record a hub tool call");
        }
        result
    }

    async fn dispatch_tool(&self, ctx: &ToolCtx, call: HubToolCall) -> Result<Value> {
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
            // See NOT_BUILT.
            C::ListConnectorTools(_)
            | C::GetConnectorTool(_)
            | C::RunConnectorTool(_)
            | C::ListSkills(_)
            | C::ReadSkill(_)
            | C::ListConnections(_)
            | C::ListInstalledSkills(_)
            | C::ConnectService(_)
            | C::AddSkill(_)
            | C::CreateChannel(_)
            | C::UpdateChannel(_)
            | C::ArchiveChannel(_)
            | C::SetDefaultModel(_)
            | C::SetWorkerTags(_)
            | C::InstallTool(_)
            | C::AddAccount(_)
            | C::StartSignIn(_)
            | C::SetAccountExtras(_)
            | C::SearchConnectorCatalog(_)
            | C::UpdateConnection(_)
            | C::Disconnect(_)
            | C::SearchSkills(_)
            | C::UpdateSkill(_)
            | C::RemoveSkill(_)
            | C::UpdateThisChannel(_)
            | C::SetChannelTags(_)
            | C::SetChannelConnector(_)
            | C::SetChannelSkill(_)
            | C::RequestAccess(_) => Err(HubError::Api(ErrorBody::not_built(call.tool().name()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::client_api::{ClientRequest, ConnectionDisconnect, SkillDirectorySources};
    use consort_proto::error::ErrorCode;
    use consort_proto::ids::{AccountId, ConnectionId, WorkerId};
    use consort_proto::model::{ModelOptions, Session, SessionState, Usage};
    use consort_proto::time::Timestamp;
    use serde_json::json;

    use super::*;
    use crate::hub::{Caller, test_hub};

    async fn everything_session(hub: &Hub) -> SessionId {
        let channel = hub
            .store
            .read(conversation::everything_channel)
            .await
            .unwrap();
        let session = Session {
            id: SessionId::new(),
            role: SessionRole::Coordinator,
            channel_id: channel.id,
            task_id: None,
            worker_id: WorkerId::new(),
            account_id: AccountId::new(),
            harness: "fake".to_owned(),
            model: "fake-1".to_owned(),
            options: ModelOptions::new(),
            workspace_path: "/tmp/workspace".to_owned(),
            native_ref: None,
            state: SessionState::Idle,
            last_seq: 0,
            usage: Usage::default(),
            started_at: Timestamp::now(),
            ended_at: None,
            end_reason: None,
        };
        let id = session.id;
        hub.store
            .write(move |tx| tx.save_session(&session))
            .await
            .unwrap();
        id
    }

    /// The tools of the connectors and skills plan are in the proto, but no
    /// session sees them until the hub carries them out.
    #[tokio::test]
    async fn tools_that_are_not_built_stay_out_of_the_list_and_answer_unsupported() {
        let (hub, _dir) = test_hub().await;
        let session_id = everything_session(&hub).await;
        let names: Vec<String> = hub
            .tool_definitions(session_id)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        for tool in NOT_BUILT {
            assert!(!names.contains(&tool.name().to_owned()), "{}", tool.name());
        }
        assert!(names.contains(&"list_channels".to_owned()));
        let result = hub
            .call_tool(
                session_id,
                "run_connector_tool",
                json!({"connector": "gmail_work", "tool": "GMAIL_SEND_EMAIL", "arguments": {}}),
            )
            .await;
        assert!(result.is_error);
        let body = result.structured_content.unwrap();
        assert_eq!(body["error"]["code"], "unsupported");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("run_connector_tool")
        );
    }

    #[tokio::test]
    async fn requests_that_are_not_built_answer_unsupported() {
        let (hub, _dir) = test_hub().await;
        for request in [
            ClientRequest::from(ConnectionDisconnect {
                connection_id: ConnectionId::new(),
            }),
            ClientRequest::from(SkillDirectorySources {}),
        ] {
            let err = crate::api::dispatch(&hub, Caller::Hub, request)
                .await
                .unwrap_err();
            assert_eq!(err.code(), ErrorCode::Unsupported);
            assert!(err.to_error_body().message.starts_with("Not built yet: "));
        }
    }
}
