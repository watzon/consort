//! Functions that handlers and tools in other files call. The hub lead owns
//! this file. Other owners call these functions and do not change them.

use consort_proto::ids::{ChannelId, RoutineId};
use consort_proto::model::{ApprovalMode, BlobRef, ModelSelection, PlacementRequest, TaskCreator};
use consort_proto::time::Timestamp;

use crate::hub::Hub;

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
    /// Files for the first turn of each session of the task.
    pub attachments: Vec<BlobRef>,
}

impl Hub {
    /// Wakes the coordinator of a channel if queued events wait and no turn
    /// runs. Call it after a transaction that queued a wake event commits.
    /// It never blocks and never fails.
    pub(crate) fn kick_coordinator(&self, channel_id: ChannelId) {
        self.coordinators.kick(self, channel_id);
    }
}
