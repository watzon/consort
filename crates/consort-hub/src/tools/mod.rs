//! Hub tool dispatch for `tool.list` and `tool.call`.

use consort_proto::execution_link::ExecTool;
use consort_proto::ids::SessionId;
use consort_proto::model::SessionRole;
use consort_proto::tools::{
    HubTool, HubToolCall, ToolAudience, ToolCallResult, ToolDefinition, tools_for,
};
use serde_json::Value;

use crate::error::{HubError, Result};
use crate::hub::{Hub, ToolCtx};
use crate::store::{conversation, work};

pub(crate) mod chat;
pub(crate) mod management;
mod routines;
mod tasks;

/// The tools that work on the connectors of a session's channel.
const CONNECTOR_TOOLS: &[HubTool] = &[
    HubTool::ListConnectorTools,
    HubTool::GetConnectorTool,
    HubTool::RunConnectorTool,
];

/// The tools that need Monid in the channel.
const MONID_TOOLS: &[HubTool] = &[HubTool::GetMonidTool, HubTool::RunMonidTool];

/// The tools that read skills, for a harness that does not load skills
/// itself.
const SKILL_TOOLS: &[HubTool] = &[HubTool::ListSkills, HubTool::ReadSkill];

/// The hub tools of a voice task: reading, its proposal and its summary.
/// It cannot post, publish or ask, so the raw text that it reads stays in
/// its session.
const VOICE_TASK_TOOLS: &[HubTool] = &[
    HubTool::MemoryList,
    HubTool::MemoryRead,
    HubTool::FilesList,
    HubTool::ListWorkers,
    HubTool::Search,
    HubTool::ListConnectorTools,
    HubTool::GetConnectorTool,
    HubTool::RunConnectorTool,
    HubTool::ListSkills,
    HubTool::ReadSkill,
    HubTool::SuggestMemory,
    HubTool::SubmitForReview,
];

/// The tools that the hub lists only while the voice is on.
const VOICE_TOOLS: &[HubTool] = &[HubTool::VoiceGuide, HubTool::ProposeVoiceRule];

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
        // These tools stay listed when the channel lacks the service: a
        // session can ask for access, and its tools work from that moment.
        let (has_connections, has_monid) = self
            .store
            .read(|conn| {
                Ok((
                    !crate::store::connectors::connections(conn)?.is_empty(),
                    crate::store::monid::key_present(conn)?,
                ))
            })
            .await?;
        let native_skills = crate::skills::harness_loads_skills(&ctx.session.harness);
        // `voice_guide` and `propose_voice_rule` are listed only while the
        // voice is on. A voice task gets only its own tools.
        let voice_on = self
            .store
            .read(|conn| Ok(crate::store::voice::settings(conn)?.enabled))
            .await?;
        let voice_task = self.is_voice_task(ctx.task.as_ref()).await?;
        let mut tools: Vec<ToolDefinition> = tools_for(ctx.audience)
            .into_iter()
            .filter(|t| {
                HubTool::from_name(&t.name).is_none_or(|tool| {
                    (has_connections || !CONNECTOR_TOOLS.contains(&tool))
                        && (has_monid || !MONID_TOOLS.contains(&tool))
                        && (has_connections || has_monid || tool != HubTool::SearchTools)
                        && (!native_skills || !SKILL_TOOLS.contains(&tool))
                        && (voice_on || !VOICE_TOOLS.contains(&tool))
                        && (!voice_task || VOICE_TASK_TOOLS.contains(&tool))
                })
            })
            .collect();
        // A session with a worker does its work through the tools of the
        // worker. The session host runs them, and the hub decides about each.
        if ctx.session.execution_worker_id.is_some() {
            tools.extend(ExecTool::ALL.into_iter().map(ExecTool::definition));
        }
        Ok(tools)
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
        // A turn that the assistant started on its own has limits that the
        // hub enforces here, not in the prompt.
        self.check_turn_tool(&ctx, tool).await?;
        if !VOICE_TASK_TOOLS.contains(&tool) && self.is_voice_task(ctx.task.as_ref()).await? {
            return Err(crate::proactivity::turn::denied(
                "A voice task can only read its sources, suggest the guide with suggest_memory and end with submit_for_review.",
            ));
        }
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
            C::FilesRead(a) => self.tool_files_read(ctx, a).await,
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
            C::FollowUpLater(a) => self.tool_follow_up_later(ctx, a).await,
            C::CancelFollowUp(a) => self.tool_cancel_follow_up(ctx, a).await,
            C::SetProactivity(a) => self.tool_set_proactivity(ctx, a).await,
            C::SuggestMemory(a) => self.tool_suggest_memory(ctx, a).await,
            C::ListChannels(a) => self.tool_list_channels(ctx, a).await,
            C::ReadChannel(a) => self.tool_read_channel(ctx, a).await,
            C::MessageChannel(a) => self.tool_message_channel(ctx, a).await,
            C::PostUpdate(a) => self.tool_post_update(ctx, a).await,
            C::SubmitForReview(a) => self.tool_submit_for_review(ctx, a).await,
            C::PublishArtifact(a) => self.tool_publish_artifact(ctx, a).await,
            C::ListConnectorTools(a) => self.tool_list_connector_tools(ctx, a).await,
            C::GetConnectorTool(a) => self.tool_get_connector_tool(ctx, a).await,
            C::RunConnectorTool(a) => self.tool_run_connector_tool(ctx, a).await,
            C::SearchTools(a) => self.tool_search_tools(ctx, a).await,
            C::RequestAccess(a) if a.skill.is_none() => self.tool_request_access(ctx, a).await,
            C::GetMonidTool(a) => self.tool_get_monid_tool(ctx, a).await,
            C::RunMonidTool(a) => self.tool_run_monid_tool(ctx, a).await,
            C::ListSkills(_) => self.tool_list_skills(ctx).await,
            C::ReadSkill(a) => self.tool_read_skill(ctx, a).await,
            C::VoiceGuide(a) => self.tool_voice_guide(ctx, a).await,
            C::VoiceSample(a) => self.tool_voice_sample(ctx, a).await,
            C::ProposeVoiceRule(a) => self.tool_propose_voice_rule(ctx, a).await,
            C::ShowDraft(a) => self.tool_show_draft(ctx, a).await,
            C::SearchSkills(a) => self.tool_search_skills(a).await,
            C::AddSkill(a) => self.tool_add_skill(ctx, a).await,
            other => self.tool_management(ctx, other).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::ids::{AccountId, WorkerId};
    use consort_proto::model::{ModelOptions, Session, SessionState, Usage};
    use consort_proto::time::Timestamp;

    use super::*;
    use crate::hub::test_hub;

    async fn tutti_session(hub: &Hub) -> SessionId {
        let channel = hub.store.read(conversation::tutti_channel).await.unwrap();
        let session = Session {
            execution_worker_id: None,
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

    /// The Tutti coordinator sees the skill tools of step 3.
    #[tokio::test]
    async fn the_tutti_coordinator_sees_the_skill_tools() {
        let (hub, _dir) = test_hub().await;
        let session_id = tutti_session(&hub).await;
        let names: Vec<String> = hub
            .tool_definitions(session_id)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        for name in ["search_skills", "add_skill", "list_channels"] {
            assert!(names.contains(&name.to_owned()), "{name}");
        }
    }
}
