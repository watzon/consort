//! A model change for a task that already started.
//!
//! `task.update` saves the new selection at once and never cuts a running
//! turn. The change takes effect at the next turn of the session:
//!
//! - Same account and harness, and the account can switch: the turn carries
//!   a `model_change`, and the worker sets the model in the same session.
//! - Another account or harness, a harness that cannot switch, or a worker
//!   that refused the change (`unsupported`): the hub ends the old session
//!   and starts a new one on the same worker. The new session keeps the
//!   workspace of the task. Its first turn carries the brief and the task
//!   thread as context, then the text of the user.
//!
//! The hub keeps no extra state for this. The plan comes from comparing the
//! saved selection with the session. A task that waits for its new session
//! is in Working with no current session and with the worker it ran on.

use consort_proto::ids::{SessionId, TaskId};
use consort_proto::model::{Message, ModelSelection, Session, SessionRole, SessionState, Task};
use consort_proto::status::{Actor, TaskStatus};
use consort_proto::worker_link::{
    HubToWorker, SessionModelChange, SessionStart, TurnInput, WorkspaceSpec,
};
use rusqlite::Connection;

use crate::error::{HubError, Result};
use crate::hub::Hub;
use crate::limits::TURN_TEXT_MAX_BYTES;
use crate::link::events::{Effects, end_session_by_hub, inbox_batch};
use crate::ops::flow::transition;
use crate::placement::resolve_selection;
use crate::prompts;
use crate::store::{Tx, conversation, machine, work};

/// What the next turn of a task session does about the model.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ModelPlan {
    /// The session already runs the selection, or the task has none.
    Keep,
    /// The turn carries the change. The worker sets it in the same session.
    InSession(SessionModelChange),
    /// A new session on the same worker.
    NewSession,
}

/// Compares the saved selection of a task with its session.
///
/// `Err` holds the reason why the worker of the session cannot run the
/// selection. The session then keeps its model.
pub(crate) fn plan_model(
    conn: &Connection,
    task: &Task,
    session: &Session,
) -> Result<std::result::Result<ModelPlan, String>> {
    let Some(selection) = task.model_selection.as_ref() else {
        return Ok(Ok(ModelPlan::Keep));
    };
    let Some(worker) = machine::worker(conn, session.worker_id)? else {
        return Ok(Ok(ModelPlan::Keep));
    };
    let Some(inventory) = worker.inventory.as_ref() else {
        return Ok(Ok(ModelPlan::Keep));
    };
    let (account, model, options) =
        match resolve_selection(inventory, selection, task.approval_mode) {
            Ok(found) => found,
            Err(reason) => return Ok(Err(format!("{} {reason}", worker.name))),
        };
    if account.account_id == session.account_id
        && model == session.model
        && options == session.options
    {
        return Ok(Ok(ModelPlan::Keep));
    }
    let same_account =
        account.account_id == session.account_id && account.harness == session.harness;
    Ok(Ok(if same_account && account.capabilities.switch_model {
        ModelPlan::InSession(SessionModelChange { model, options })
    } else {
        ModelPlan::NewSession
    }))
}

/// Whether a started task can take a new model selection, and where. `Err`
/// holds the refusal for the user.
pub(crate) fn check_selection_for_started_task(
    conn: &Connection,
    task: &Task,
    selection: Option<&ModelSelection>,
) -> Result<std::result::Result<(), String>> {
    if task.status == TaskStatus::Resolved {
        return Ok(Err(
            "The task is resolved. Mark it as not done first, then change its model.".to_owned(),
        ));
    }
    if task.archived_at.is_some() {
        return Ok(Err(
            "The task is archived. Restore it first, then change its model.".to_owned(),
        ));
    }
    let (Some(selection), Some(session_id)) = (selection, task.current_session_id) else {
        return Ok(Ok(()));
    };
    let Some(session) = work::session(conn, session_id)? else {
        return Ok(Ok(()));
    };
    if session.state.is_final() {
        return Ok(Ok(()));
    }
    let Some(worker) = machine::worker(conn, session.worker_id)? else {
        return Ok(Ok(()));
    };
    let Some(inventory) = worker.inventory.as_ref() else {
        return Ok(Ok(()));
    };
    Ok(
        match resolve_selection(inventory, selection, task.approval_mode) {
            Ok(_) => Ok(()),
            Err(reason) => Err(format!(
                "{} {reason} The task runs there, so choose a model that this worker has.",
                worker.name
            )),
        },
    )
}

/// Takes a task off its session without failing it: the text waits in the
/// inbox for the new session, the old session ends, and the worker gets
/// `session.stop`. The caller then starts the new session after the commit
/// (see [`Effects::switch_tasks`]).
pub(crate) fn detach_session(
    hub: &Hub,
    tx: &mut Tx<'_>,
    task_id: TaskId,
    text: Option<(&str, &[consort_proto::model::BlobRef])>,
) -> Result<Effects> {
    let mut task = work::task_or_404(tx.db(), task_id)?;
    let old = task.current_session_id.take();
    task.updated_at = tx.now();
    tx.save_task(&task)?;
    if let Some((text, attachments)) = text {
        tx.push_task_inbox(task_id, text, attachments)?;
    }
    let mut fx = match old {
        Some(old) => end_session_by_hub(hub, tx, old, "The user chose another model.")?,
        None => Effects::default(),
    };
    fx.switch_tasks.push(task_id);
    Ok(fx)
}

/// True for a task that waits for the session that replaces its old one.
/// A task in Needs input waits too: a decision that stays open must not
/// hold the new session back, and the next answer must find a session.
pub(crate) fn is_switching(task: &Task) -> bool {
    matches!(task.status, TaskStatus::Working | TaskStatus::NeedsInput)
        && task.current_session_id.is_none()
        && task.worker_id.is_some()
        && !task.session_ended
}

/// The worker took a turn with a model change: the session runs the new
/// model now.
pub(crate) fn model_applied_tx(
    tx: &mut Tx<'_>,
    session_id: SessionId,
    change: &SessionModelChange,
) -> Result<()> {
    if let Some(mut session) = work::session(tx.db(), session_id)? {
        session.model = change.model.clone();
        session.options = change.options.clone();
        tx.save_session(&session)?;
    }
    Ok(())
}

impl Hub {
    /// Starts the new sessions of the tasks that wait for one. Safe to call
    /// at any time: a task that waits for nothing is skipped.
    pub(crate) async fn start_switched_sessions(&self) -> Result<()> {
        let waiting = self
            .store
            .read(|conn| {
                let mut tasks = work::tasks_with_status(conn, TaskStatus::Working)?;
                tasks.extend(work::tasks_with_status(conn, TaskStatus::NeedsInput)?);
                Ok(tasks
                    .into_iter()
                    .filter(is_switching)
                    .map(|t| t.id)
                    .collect::<Vec<_>>())
            })
            .await?;
        for task_id in waiting {
            self.switch_task_session(task_id).await?;
        }
        Ok(())
    }

    /// Starts the new session of a task that waits for one. The new session
    /// runs the saved selection on the worker of the task. Its first turn
    /// holds the brief, the thread and the waiting messages.
    pub(crate) async fn switch_task_session(&self, task_id: TaskId) -> Result<()> {
        let inputs = self
            .store
            .read(move |conn| {
                let Some(task) = work::task(conn, task_id)? else {
                    return Ok(None);
                };
                if !is_switching(&task) || task.archived_at.is_some() {
                    return Ok(None);
                }
                let channel = conversation::channel_or_404(conn, task.channel_id)?;
                let thread = conversation::thread_replies(conn, task.card_message_id)?;
                let worker = match task.worker_id {
                    Some(id) => machine::worker(conn, id)?,
                    None => None,
                };
                Ok(Some((task, channel, thread, worker)))
            })
            .await?;
        let Some((task, channel, thread, worker)) = inputs else {
            return Ok(());
        };
        let Some(worker) = worker else {
            return self
                .fail_switch(task_id, "The worker of this task is gone.")
                .await;
        };
        let resolved = task.model_selection.as_ref().map(|selection| {
            worker
                .inventory
                .as_ref()
                .ok_or_else(|| format!("{} has not sent its accounts yet.", worker.name))
                .and_then(|inventory| {
                    resolve_selection(inventory, selection, task.approval_mode)
                        .map(|(account, model, options)| {
                            (account.account_id, account.harness.clone(), model, options)
                        })
                        .map_err(|reason| format!("{} {reason}", worker.name))
                })
        });
        let Some(Ok((account_id, harness, model, options))) = resolved else {
            let reason = match resolved {
                Some(Err(reason)) => reason,
                _ => "The task has no model choice.".to_owned(),
            };
            return self
                .fail_switch(
                    task_id,
                    &format!("The task could not move to the new model. {reason}"),
                )
                .await;
        };

        let (global_index, channel_index) = self.memory_indexes(&channel).await?;
        let mut system_prompt =
            prompts::task_system_prompt(&channel, &task, &global_index, &channel_index);
        if let Some(voice) = self.voice_prompt_section(&channel).await? {
            system_prompt.push_str(&voice);
        }
        let local_mcp = task
            .placement
            .require
            .as_ref()
            .map(|r| r.local_mcp_servers.clone())
            .unwrap_or_default();
        let worker_id = worker.id;
        let isolation = task
            .placement
            .isolation
            .unwrap_or(channel.default_isolation);
        let written = self
            .store
            .write(move |tx| {
                let mut task = work::task_or_404(tx.db(), task_id)?;
                if !is_switching(&task) {
                    return Ok(false);
                }
                // Read the inbox in the transaction: a message may have
                // arrived since the first read.
                let inbox = work::task_inbox(tx.db(), task_id)?;
                let (taken, batch) = inbox_batch(&inbox, TURN_TEXT_MAX_BYTES / 2);
                tx.delete_task_inbox(&taken)?;
                let thread = without_echo(thread, &inbox[..taken.len()]);
                let text = prompts::task_switch_turn(&task, &thread, &batch.text);
                let now = tx.now();
                let session = Session {
                    id: SessionId::new(),
                    role: SessionRole::Task,
                    channel_id: task.channel_id,
                    task_id: Some(task_id),
                    worker_id,
                    account_id,
                    harness,
                    model: model.clone(),
                    options: options.clone(),
                    workspace_path: String::new(),
                    native_ref: None,
                    state: SessionState::Starting,
                    last_seq: 0,
                    usage: Default::default(),
                    started_at: now,
                    ended_at: None,
                    end_reason: None,
                };
                tx.save_session(&session)?;
                let extensions = tx.session_extensions(worker_id, task.channel_id)?;
                let channel_files =
                    tx.channel_files_for_start(session.id, task.channel_id, Some(task_id))?;
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
                    &HubToWorker::SessionStart(SessionStart {
                        session_id: session.id,
                        role: SessionRole::Task,
                        channel_id: task.channel_id,
                        task_id: Some(task_id),
                        account_id,
                        model,
                        options,
                        approval_mode: task.approval_mode,
                        // The worker finds the workspace of the task by its
                        // ID. This spec only counts when the worker lost it.
                        workspace: WorkspaceSpec {
                            repos: Vec::new(),
                            isolation,
                            carry_uncommitted: false,
                        },
                        system_prompt_append: system_prompt,
                        allowed_local_mcp_servers: local_mcp,
                        channel_files,
                        extensions,
                        resume: None,
                        turn: Some(TurnInput {
                            text,
                            attachments: batch.attachments,
                        }),
                        turn_approval_mode: None,
                    }),
                )?;
                task.current_session_id = Some(session.id);
                task.updated_at = now;
                tx.save_task(&task)?;
                Ok(true)
            })
            .await;
        let changed = match written {
            Ok(changed) => changed,
            // The command was refused (for example it does not fit in one
            // frame) and the transaction rolled back. Nothing would start
            // the task again, so it fails with the reason.
            Err(HubError::Api(error)) => {
                return self
                    .fail_switch(
                        task_id,
                        &format!("The new session could not start. {}", error.message),
                    )
                    .await;
            }
            Err(other) => return Err(other),
        };
        if changed {
            self.pump_worker(worker_id);
        }
        Ok(())
    }

    /// The new session could not start. The task fails with the reason, and
    /// the messages that waited for it are dropped: Retry starts from the
    /// brief.
    async fn fail_switch(&self, task_id: TaskId, reason: &str) -> Result<()> {
        let reason = reason.to_owned();
        let channel_id = self
            .store
            .write(move |tx| {
                let mut task = work::task_or_404(tx.db(), task_id)?;
                tx.clear_task_inbox(task_id)?;
                transition(
                    tx,
                    &mut task,
                    TaskStatus::Failed,
                    Actor::System,
                    Some(format!(
                        "{reason} Choose another model, then retry the task."
                    )),
                    None,
                )?;
                task.session_ended = true;
                tx.save_task(&task)?;
                Ok(task.channel_id)
            })
            .await?;
        self.kick_coordinator(channel_id);
        Ok(())
    }
}

/// The thread without the messages that the new turn already holds: the
/// user's messages that wait in the inbox are in the thread too.
fn without_echo(mut thread: Vec<Message>, taken: &[work::InboxMessage]) -> Vec<Message> {
    for entry in taken {
        if let Some(index) = thread
            .iter()
            .rposition(|m| m.author == consort_proto::model::Author::User && m.body == entry.text)
        {
            thread.remove(index);
        }
    }
    thread
}

impl Hub {
    /// Starts the new session of a task in the background.
    pub(crate) fn spawn_switch(&self, task_id: TaskId) {
        let hub = self.clone();
        tokio::spawn(async move {
            if let Err(err) = hub.switch_task_session(task_id).await {
                tracing::warn!(error = %err, %task_id, "could not start the new session of a task");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::hub::{test_hub, test_task_session};
    use consort_proto::ids::AccountId;
    use consort_proto::model::{
        AuthState, BlobRef, Capabilities, HarnessAccount, Inventory, ModelInfo, Sha256Hash,
        WorkerLoad, WorkerPolicy,
    };

    #[tokio::test]
    async fn a_new_session_whose_start_command_is_refused_fails_the_task() {
        let (hub, _dir) = test_hub().await;
        let (task, session) = test_task_session(&hub).await;
        let account_id = AccountId::new();
        let (task_id, worker_id) = (task.id, session.worker_id);
        hub.store
            .write(move |tx| {
                let mut worker = machine::worker(tx.db(), worker_id)?.unwrap();
                worker.inventory = Some(Inventory {
                    hash: String::new(),
                    hostname: "laptop".to_owned(),
                    os: worker.os,
                    arch: "aarch64".to_owned(),
                    tags: Vec::new(),
                    accounts: vec![HarnessAccount {
                        account_id,
                        harness: "claude".to_owned(),
                        label: "Claude Work".to_owned(),
                        harness_version: None,
                        auth_state: AuthState::SignedIn,
                        auth_detail: None,
                        models: vec![ModelInfo {
                            id: "m1".to_owned(),
                            name: "M1".to_owned(),
                            options: Vec::new(),
                        }],
                        capabilities: Capabilities::default(),
                        enabled: true,
                        profile: false,
                        email: None,
                        plan: None,
                        secret_names: Vec::new(),
                        sign_in_methods: Vec::new(),
                        extras_enabled: false,
                        extras: None,
                    }],
                    local_mcp_servers: Vec::new(),
                    checkouts: Vec::new(),
                    policy: WorkerPolicy::default(),
                    load: WorkerLoad::default(),
                    tools: Vec::new(),
                });
                tx.save_worker(&worker)?;
                // The task waits for a new session. Its waiting message has
                // so many attachments that the start command cannot fit in
                // one frame.
                let mut task = work::task_or_404(tx.db(), task_id)?;
                task.model_selection = Some(ModelSelection {
                    account_label: "Claude Work".to_owned(),
                    model: "m1".to_owned(),
                    options: Default::default(),
                });
                task.current_session_id = None;
                task.worker_id = Some(worker_id);
                tx.save_task(&task)?;
                let file = BlobRef {
                    sha256: Sha256Hash::from_bytes([7; 32]),
                    name: "a.txt".to_owned(),
                    mime: "text/plain".to_owned(),
                    size: 1,
                };
                tx.push_task_inbox(task_id, "Go on", &vec![file; 9_000])?;
                Ok(())
            })
            .await
            .unwrap();

        hub.switch_task_session(task_id).await.unwrap();

        let (task, inbox) = hub
            .store
            .read(move |conn| {
                Ok((
                    work::task_or_404(conn, task_id)?,
                    work::task_inbox(conn, task_id)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert!(
            task.status_reason
                .as_deref()
                .is_some_and(|r| r.contains("could not start") && r.contains("retry")),
            "{:?}",
            task.status_reason
        );
        assert!(inbox.is_empty());
    }
}
