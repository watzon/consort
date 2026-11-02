//! The tools of a task session: the hub relays them from the session host to
//! the execution worker, and its approval engine decides about each call.

use std::time::Duration;

use consort_proto::error::ErrorCode;
use consort_proto::execution_link::{
    ExecOp, ExecOpResult, ExecTool, HubToExecution, WorkspaceInspection, WorkspacePrepare,
    WorkspaceSync, WorkspaceSyncResult,
};
use consort_proto::frame::{Frame, FrameId};
use consort_proto::ids::{SessionId, WorkerId};
use consort_proto::model::Session;
use consort_proto::session_host_link::{
    ToolAuthorize, ToolDecision, ToolExec, WorkspaceFiles, WorkspaceOpen,
};
use consort_proto::time::Timestamp;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::approval_engine::{Rule, rule_for};
use crate::error::{HubError, Result};
use crate::hub::Hub;
use crate::link::CallError;
use crate::store::{machine, work};

/// How long a clone or a worktree may take.
const WORKSPACE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How long one tool call may take on the worker. `process_wait` waits for at
/// most 30 s, and a command runs in the background.
const EXEC_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The workspace capability of a session. A task keeps its workspace, so each
/// of its sessions has the same one. Only the hub and the worker see it.
fn capability_of(session: &Session) -> (String, String) {
    let key = session
        .task_id
        .map_or_else(|| session.id.to_string(), |task| task.to_string());
    (format!("cap-{key}"), key)
}

impl Hub {
    /// The session, when it belongs to this session host and has a worker.
    async fn exec_session(
        &self,
        session_host: WorkerId,
        session_id: SessionId,
    ) -> Result<(Session, WorkerId)> {
        let session = self
            .store
            .read(move |conn| work::session(conn, session_id))
            .await?
            .filter(|s| s.worker_id == session_host)
            .ok_or_else(|| {
                HubError::new(
                    ErrorCode::NotFound,
                    "The hub does not know this session for this session host.",
                )
            })?;
        let worker = session.execution_worker_id.ok_or_else(|| {
            HubError::new(
                ErrorCode::PolicyDenied,
                "This session has no worker, so it has no file or process tools.",
            )
        })?;
        Ok((session, worker))
    }

    /// Sends one request to an execution worker and waits for the answer.
    pub(crate) async fn exec_request<T: DeserializeOwned>(
        &self,
        worker: WorkerId,
        message: &HubToExecution,
        timeout: Duration,
    ) -> Result<T> {
        let offline = || {
            HubError::new(
                ErrorCode::WorkerOffline,
                "The worker of this session is offline. Consort tries again when it connects.",
            )
        };
        let link = self.links.get(worker).ok_or_else(offline)?;
        let request = match Frame::family_request(FrameId::generate(), message)? {
            Frame::Request(request) => request,
            _ => return Err(HubError::internal("a request did not encode as a request")),
        };
        let answer = link
            .call(request.id, request.kind, request.body, timeout)
            .await;
        match answer {
            Ok(Ok(value)) => Ok(serde_json::from_value(value)?),
            Ok(Err(body)) => Err(HubError::Api(body)),
            Err(CallError::Encode(reason)) => Err(HubError::invalid(reason)),
            Err(CallError::Gone | CallError::Timeout) => Err(offline()),
        }
    }

    /// `workspace.open`: makes or finds the workspace on the worker.
    pub(crate) async fn workspace_open(
        &self,
        session_host: WorkerId,
        open: WorkspaceOpen,
    ) -> Result<WorkspaceInspection> {
        let (session, worker) = self.exec_session(session_host, open.session_id).await?;
        let (capability, key) = capability_of(&session);
        self.exec_request(
            worker,
            &HubToExecution::WorkspacePrepare(WorkspacePrepare {
                capability,
                task_id: session.task_id,
                key,
                spec: open.spec,
                channel_files: open.channel_files,
            }),
            WORKSPACE_TIMEOUT,
        )
        .await
    }

    /// `workspace.sync`: puts the files of a turn in the workspace.
    pub(crate) async fn workspace_files(
        &self,
        session_host: WorkerId,
        files: WorkspaceFiles,
    ) -> Result<WorkspaceSyncResult> {
        let (session, worker) = self.exec_session(session_host, files.session_id).await?;
        let (capability, _) = capability_of(&session);
        self.exec_request(
            worker,
            &HubToExecution::WorkspaceSync(WorkspaceSync {
                capability,
                attachments: files.attachments,
                channel_files: files.channel_files,
            }),
            WORKSPACE_TIMEOUT,
        )
        .await
    }

    /// `tool.exec`: runs one tool call on the worker. The session host sends
    /// a call again with the same operation ID when the link fails, and the
    /// journal of the worker keeps it from running twice.
    pub(crate) async fn tool_exec(
        &self,
        session_host: WorkerId,
        exec: ToolExec,
    ) -> Result<ExecOpResult> {
        let (session, worker) = self.exec_session(session_host, exec.session_id).await?;
        let Some(tool) = ExecTool::from_name(&exec.tool) else {
            return Err(HubError::invalid(format!(
                "`{}` is not a tool of a worker.",
                exec.tool
            )));
        };
        let (capability, _) = capability_of(&session);
        let mut arguments = exec.arguments;
        if tool == ExecTool::SkillPrepare {
            arguments = self
                .skill_prepare_arguments(&session, worker, arguments)
                .await?;
        }
        let deadline = Timestamp::now().plus_millis(EXEC_TIMEOUT.as_millis() as i64);
        self.exec_request(
            worker,
            &HubToExecution::ExecOp(ExecOp {
                session_id: exec.session_id,
                capability,
                operation_id: exec.operation_id,
                tool: exec.tool,
                arguments,
                deadline,
                policy_version: 0,
            }),
            EXEC_TIMEOUT,
        )
        .await
    }

    /// Adds the pinned version of a skill to the arguments of `skill_prepare`,
    /// and lets this worker get the files.
    async fn skill_prepare_arguments(
        &self,
        session: &Session,
        worker: WorkerId,
        mut arguments: Value,
    ) -> Result<Value> {
        let name = arguments
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| HubError::invalid("`name` is required."))?
            .to_owned();
        let channel = session.channel_id;
        let hash = self
            .store
            .write(move |tx| {
                let extensions = tx.session_extensions(worker, channel)?;
                extensions
                    .skills
                    .iter()
                    .find(|s| s.name == name)
                    .map(|s| s.content_hash.clone())
                    .ok_or_else(|| {
                        HubError::not_found(format!(
                            "This channel has no skill `{name}`. Call list_skills to see the skills."
                        ))
                    })
            })
            .await?;
        if let Some(object) = arguments.as_object_mut() {
            object.insert("content_hash".to_owned(), Value::String(hash.to_string()));
        }
        Ok(arguments)
    }

    /// The command rules that count in a channel. See ADR 0019.
    pub(crate) async fn command_rules_of(
        &self,
        channel_id: consort_proto::ids::ChannelId,
    ) -> Result<Vec<consort_proto::model::CommandRule>> {
        self.store
            .read(move |conn| {
                let tutti = crate::store::conversation::tutti_channel(conn)?;
                if tutti.id == channel_id {
                    return Ok(tutti.command_rules);
                }
                let channel = crate::store::conversation::channel_or_404(conn, channel_id)?;
                Ok(crate::command_rules::effective_rules(
                    &tutti.command_rules,
                    &channel.command_rules,
                ))
            })
            .await
    }

    /// `tool.authorize`: the approval engine.
    pub(crate) async fn tool_authorize(
        &self,
        session_host: WorkerId,
        request: ToolAuthorize,
    ) -> Result<ToolDecision> {
        let (session, worker) = self.exec_session(session_host, request.session_id).await?;
        let task_id = session.task_id;
        let (policy, task_mode) = self
            .store
            .read(move |conn| {
                let policy = machine::worker(conn, worker)?
                    .and_then(|w| w.inventory)
                    .map(|i| i.policy);
                let mode = match task_id {
                    Some(id) => work::task(conn, id)?.map(|t| t.approval_mode),
                    None => None,
                };
                Ok((policy, mode))
            })
            .await?;
        let mut mode = request.mode;
        if let Some(task_mode) = task_mode {
            mode = mode.min(task_mode);
        }
        if let Some(policy) = &policy {
            mode = mode.min(policy.max_approval_mode);
        }
        let Some(tool) = ExecTool::from_name(&request.tool) else {
            return Ok(ToolDecision::Ask {
                note: None,
                blocked: Vec::new(),
                check: None,
            });
        };
        let class = tool.class();
        if class == consort_proto::execution_link::ToolClass::Shell
            && policy.as_ref().is_some_and(|p| !p.allow_shell)
        {
            return Ok(ToolDecision::Deny {
                reason: "The policy of this worker does not allow shell commands.".to_owned(),
                check: None,
            });
        }
        // The command rules come before the approval mode, so an Ask rule
        // holds in Full access too. A worker whose policy holds its sessions
        // below Auto keeps asking: a rule cannot give more than the worker.
        if let Some(command) = shell_text(tool, &request.arguments) {
            let rules = self.command_rules_of(session.channel_id).await?;
            match crate::command_rules::evaluate(command, &rules) {
                crate::command_rules::RuleOutcome::Ask(rule) => {
                    return Ok(ToolDecision::Ask {
                        note: Some(format!(
                            "A command rule asks about this: {}",
                            rule.description
                        )),
                        blocked: Vec::new(),
                        check: None,
                    });
                }
                // Text to a running process can be anything, so only `Ask`
                // reads it.
                crate::command_rules::RuleOutcome::Allow(rule)
                    if tool == ExecTool::ProcessStart
                        && policy.as_ref().is_none_or(|p| {
                            p.max_approval_mode >= consort_proto::model::ApprovalMode::Auto
                        }) =>
                {
                    return Ok(ToolDecision::Allow {
                        check: Some(consort_proto::model::CommandCheckOutcome {
                            verdict: consort_proto::model::CommandCheckVerdict::Allowed,
                            source: String::new(),
                            reason: None,
                            rule: None,
                            error: None,
                            command_rule: Some(rule.description.clone()),
                        }),
                    });
                }
                _ => {}
            }
        }
        match rule_for(class, mode) {
            Rule::Allow => Ok(ToolDecision::Allow { check: None }),
            Rule::Ask => Ok(ToolDecision::Ask {
                note: None,
                blocked: Vec::new(),
                check: None,
            }),
            Rule::Check => {
                self.run_command_check(&session, &request.tool, &request.arguments)
                    .await
            }
        }
    }
}

/// The shell text of a tool call that runs one.
fn shell_text(tool: ExecTool, arguments: &Value) -> Option<&str> {
    let field = match tool {
        ExecTool::ProcessStart => "command",
        ExecTool::ProcessWriteStdin => "text",
        _ => return None,
    };
    arguments.get(field).and_then(Value::as_str)
}
