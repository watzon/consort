//! Functions that handlers and tools in other files call. The hub lead owns
//! this file. Other owners call these functions and do not change them.

use consort_proto::ids::{ChannelId, RoutineId};
use consort_proto::model::{
    ApprovalMode, Message, ModelSelection, PlacementRequest, Task, TaskCreator,
};
use consort_proto::status::Actor;
use consort_proto::time::Timestamp;

use crate::error::Result;
use crate::hub::Hub;
use consort_proto::worker_link::{HubToWorker, SessionStop};

/// The input of [`Hub::create_task`].
#[derive(Debug, Clone)]
pub(crate) struct NewTask {
    pub channel_id: ChannelId,
    pub title: String,
    pub brief: String,
    pub model_selection: Option<ModelSelection>,
    pub placement: PlacementRequest,
    pub approval_mode: Option<ApprovalMode>,
    pub start_at: Option<Timestamp>,
    pub created_by: TaskCreator,
    pub routine_id: Option<RoutineId>,
}

impl Hub {
    /// Wakes the coordinator of a channel if queued events wait and no turn
    /// runs. Call it after a transaction that queued a wake event commits.
    /// It never blocks and never fails.
    pub(crate) fn kick_coordinator(&self, channel_id: ChannelId) {
        self.coordinators.kick(self, channel_id);
    }

    /// Sends a message that the user wrote in a task thread to the task
    /// session. The message is already stored. If the task cannot take the
    /// message now, the hub posts a notice in the thread that says why.
    pub(crate) async fn route_user_message_to_task(
        &self,
        task: &Task,
        message: &Message,
    ) -> Result<()> {
        if let Err(err) = self
            .deliver_to_task(task.id, message.body.clone(), Actor::User)
            .await
        {
            let body = err.to_error_body().message;
            let (channel_id, root, task_id) = (task.channel_id, task.card_message_id, task.id);
            self.store
                .write(move |tx| {
                    tx.post_notice(
                        channel_id,
                        Some(root),
                        Some(task_id),
                        "message_not_sent",
                        &body,
                    )
                    .map(|_| ())
                })
                .await?;
        }
        Ok(())
    }

    /// Stops every session of a channel. Call it before you delete the
    /// channel. The stop commands wait in the queue when a worker is offline.
    pub(crate) async fn stop_channel_sessions(&self, channel_id: ChannelId) -> Result<()> {
        let workers = self
            .store
            .write(move |tx| {
                let sessions: Vec<_> = {
                    let mut stmt = tx.db().prepare(
                        "SELECT data FROM sessions WHERE channel_id = ?1 AND state NOT IN ('ended', 'lost')",
                    )?;
                    let rows = stmt.query_map([channel_id.to_string()], |row| row.get::<_, String>(0))?;
                    let mut out = Vec::new();
                    for row in rows {
                        out.push(serde_json::from_str::<consort_proto::model::Session>(&row?)?);
                    }
                    out
                };
                let mut workers = Vec::new();
                for session in sessions {
                    tx.queue_command(
                        session.worker_id,
                        &HubToWorker::SessionStop(SessionStop {
                            session_id: session.id,
                            reason: "The channel was removed.".to_owned(),
                        }),
                    )?;
                    workers.push(session.worker_id);
                }
                Ok(workers)
            })
            .await?;
        for worker_id in workers {
            self.pump_worker(worker_id);
        }
        Ok(())
    }
}
