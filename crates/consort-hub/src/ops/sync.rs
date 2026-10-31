//! Snapshots and peer lookups.

use consort_proto::auth::{PeerId, PublicKey};
use consort_proto::client_api::{Snapshot, SnapshotScope};
use consort_proto::ids::ClientId;
use consort_proto::model::{Message, Task};
use consort_proto::status::TaskStatus;

use crate::error::Result;
use crate::hub::Hub;
use crate::limits::{SNAPSHOT_MAX_BYTES, preview_task};
use crate::store::{conversation, feed, knowledge, machine, preferences, routines, sign_ins, work};

/// Puts the tasks into a snapshot that holds everything else already.
///
/// Every task gets a preview of a long brief or review summary, and its ID
/// goes into `tasks_cut`. Tasks that are not resolved always go in. Resolved
/// tasks go in, newest first, while the snapshot stays under
/// [`SNAPSHOT_MAX_BYTES`]. When some do not fit, `tasks_has_more` is true.
fn add_tasks(snapshot: &mut Snapshot, tasks: Vec<Task>) -> Result<()> {
    let (mut resolved, open): (Vec<Task>, Vec<Task>) = tasks
        .into_iter()
        .partition(|t| t.status == TaskStatus::Resolved);
    for mut task in open {
        if preview_task(&mut task) {
            snapshot.tasks_cut.push(task.id);
        }
        snapshot.tasks.push(task);
    }
    // An ID in `tasks_cut` costs about 45 bytes. Count the worst case.
    let mut used = serde_json::to_vec(&*snapshot)?.len();
    resolved.sort_by_key(|t| std::cmp::Reverse(t.resolved_at.unwrap_or(t.updated_at)));
    let mut kept = Vec::new();
    for mut task in resolved {
        let cut = preview_task(&mut task);
        let cost = serde_json::to_vec(&task)?.len() + 1 + if cut { 48 } else { 0 };
        if used + cost > SNAPSHOT_MAX_BYTES {
            snapshot.tasks_has_more = true;
            break;
        }
        used += cost;
        if cut {
            snapshot.tasks_cut.push(task.id);
        }
        kept.push(task);
    }
    // Oldest first, as the other tasks.
    kept.sort_by_key(|t| t.created_at);
    snapshot.tasks.extend(kept);
    Ok(())
}

impl Hub {
    /// Builds a snapshot in one read transaction, so `seq` matches the entities.
    ///
    /// The `all` scope has no messages and no channel files. A channel scope
    /// has its newest 50 timeline messages, oldest first, or fewer when they
    /// are large. `messages_has_more` says whether older ones exist. The `all`
    /// scope also has the sign-ins that have not ended and the preferences.
    /// Tasks go in last, see [`add_tasks`].
    pub(crate) async fn snapshot(&self, scope: SnapshotScope) -> Result<Snapshot> {
        self.store
            .read(move |conn| {
                let tx = conn.unchecked_transaction()?;
                let seq = feed::head_seq(&tx)?;
                let mut snapshot = match scope {
                    SnapshotScope::All => Snapshot {
                        seq,
                        groups: conversation::groups(&tx)?,
                        channels: conversation::channels(&tx)?,
                        repos: conversation::repos(&tx)?,
                        workers: machine::workers(&tx)?,
                        read_markers: conversation::read_markers(&tx)?,
                        approvals: work::pending_approvals(&tx, None)?,
                        decisions: work::pending_decisions(&tx, None)?,
                        routines: routines::routines(&tx)?,
                        sign_ins: sign_ins::open_sign_ins(&tx)?,
                        preferences: Some(preferences::preferences(&tx)?),
                        connections: crate::store::connectors::connections(&tx)?,
                        skills: crate::store::skills::skills(&tx)?,
                        proactivity: Some(crate::store::proactivity::settings(&tx)?),
                        voice: Some(crate::store::voice::settings(&tx)?),
                        ..Snapshot::default()
                    },
                    SnapshotScope::Channel(channel_id) => {
                        let channel = conversation::channel_or_404(&tx, channel_id)?;
                        let (mut messages, messages_has_more) = conversation::messages_page(
                            &tx,
                            channel_id,
                            None,
                            None,
                            consort_proto::client_api::SNAPSHOT_MESSAGE_LIMIT,
                            crate::limits::SNAPSHOT_MESSAGES_MAX_BYTES,
                        )?;
                        messages.reverse();
                        let read_markers = conversation::read_markers(&tx)?
                            .into_iter()
                            .filter(|m| m.channel_id == channel_id)
                            .collect();
                        Snapshot {
                            seq,
                            channels: vec![channel],
                            messages: messages.into_iter().collect::<Vec<Message>>(),
                            messages_has_more,
                            read_markers,
                            approvals: work::pending_approvals(&tx, Some(channel_id))?,
                            decisions: work::pending_decisions(&tx, Some(channel_id))?,
                            routines: routines::routines_in_channel(&tx, channel_id)?,
                            channel_files: knowledge::channel_files(&tx, channel_id, None)?,
                            ..Snapshot::default()
                        }
                    }
                };
                let tasks = match scope {
                    SnapshotScope::All => work::tasks_not_archived(&tx)?,
                    SnapshotScope::Channel(channel_id) => {
                        work::tasks_in_channel(&tx, channel_id, false)?
                    }
                };
                add_tasks(&mut snapshot, tasks)?;
                Ok(snapshot)
            })
            .await
    }

    /// The public key of a peer that may connect. `None` for an unknown or
    /// revoked peer.
    pub(crate) async fn peer_public_key(&self, peer: PeerId) -> Result<Option<PublicKey>> {
        self.store
            .read(move |conn| {
                Ok(match peer {
                    PeerId::Worker(id) => machine::worker(conn, id)?
                        .filter(|w| w.revoked_at.is_none())
                        .map(|w| w.public_key),
                    PeerId::Client(id) => machine::client(conn, id)?
                        .filter(|c| c.revoked_at.is_none())
                        .map(|c| c.public_key),
                })
            })
            .await
    }

    /// The kind of a worker row, when it is not removed.
    pub(crate) async fn machine_kind(
        &self,
        id: consort_proto::ids::WorkerId,
    ) -> Result<Option<consort_proto::model::MachineKind>> {
        self.store
            .read(move |conn| {
                Ok(machine::worker(conn, id)?
                    .filter(|w| w.revoked_at.is_none())
                    .map(|w| w.kind))
            })
            .await
    }

    pub(crate) async fn client_is_active(&self, id: ClientId) -> Result<bool> {
        self.store
            .read(
                move |conn| Ok(machine::client(conn, id)?.is_some_and(|c| c.revoked_at.is_none())),
            )
            .await
    }

    /// Records the time of a client connection.
    pub(crate) async fn touch_client(&self, id: ClientId) -> Result<()> {
        self.store
            .write(move |tx| {
                if let Some(mut client) = machine::client(tx.db(), id)? {
                    client.last_seen_at = Some(tx.now());
                    tx.save_client(&client)?;
                }
                Ok(())
            })
            .await
    }
}

impl Hub {
    /// `task.get`: one task with its full brief and review summary.
    pub(crate) async fn task_get(
        &self,
        _caller: &crate::hub::Caller,
        request: consort_proto::client_api::TaskGet,
    ) -> Result<consort_proto::client_api::TaskResult> {
        let task = self
            .store
            .read(move |conn| work::task_or_404(conn, request.task_id))
            .await?;
        Ok(consort_proto::client_api::TaskResult { task })
    }

    /// `task.list_archived`: a page of the archived tasks, the most
    /// recently archived first, with long texts cut as in a snapshot.
    pub(crate) async fn task_list_archived(
        &self,
        _caller: &crate::hub::Caller,
        request: consort_proto::client_api::TaskListArchived,
    ) -> Result<consort_proto::client_api::TaskListArchivedResult> {
        const DEFAULT: u32 = 50;
        const MAX: u32 = 200;
        let limit = request.limit.unwrap_or(DEFAULT).clamp(1, MAX);
        let offset = request.offset.unwrap_or(0);
        let (mut tasks, has_more) = self
            .store
            .read(move |conn| work::archived_tasks(conn, request.channel_id, offset, limit))
            .await?;
        let mut tasks_cut = Vec::new();
        for task in &mut tasks {
            if crate::limits::preview_task(task) {
                tasks_cut.push(task.id);
            }
        }
        Ok(consort_proto::client_api::TaskListArchivedResult {
            tasks,
            tasks_cut,
            has_more,
        })
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::ids::{ChannelId, MessageId, TaskId};
    use consort_proto::model::{ApprovalMode, PlacementRequest, TaskCreator};
    use consort_proto::time::Timestamp;

    use super::*;
    use crate::limits::TASK_PREVIEW_BYTES;

    fn task(status: TaskStatus, at: i64, brief: &str) -> Task {
        let time = Timestamp::from_millis(at);
        Task {
            id: TaskId::new(),
            channel_id: ChannelId::new(),
            title: "A task".to_owned(),
            brief: brief.to_owned(),
            status,
            status_reason: None,
            created_by: TaskCreator::User,
            card_message_id: MessageId::new(),
            model_selection: None,
            placement: PlacementRequest::default(),
            approval_mode: ApprovalMode::Ask,
            start_at: None,
            current_session_id: None,
            worker_id: None,
            review_summary: Some("s".repeat(8 * 1024)),
            resolved_at: (status == TaskStatus::Resolved).then_some(time),
            session_ended: false,
            monid_calls: 0,
            monid_cost_micros: 0,
            archived_at: None,
            created_at: time,
            updated_at: time,
        }
    }

    #[test]
    fn resolved_tasks_that_do_not_fit_stay_out_and_open_tasks_stay_in() {
        let mut tasks: Vec<Task> = (0..20)
            .map(|i| task(TaskStatus::Working, i, &"w".repeat(30 * 1024)))
            .collect();
        tasks.extend((0..3000).map(|i| task(TaskStatus::Resolved, 100 + i, "short")));
        let newest = tasks.last().unwrap().id;
        let mut snapshot = Snapshot::default();
        add_tasks(&mut snapshot, tasks).unwrap();
        let size = serde_json::to_vec(&snapshot).unwrap().len();
        assert!(size <= SNAPSHOT_MAX_BYTES, "{size}");
        assert!(snapshot.tasks_has_more);
        let open: Vec<_> = snapshot
            .tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Resolved)
            .collect();
        assert_eq!(open.len(), 20);
        // The newest resolved task is kept, and long texts are previews.
        assert!(snapshot.tasks.iter().any(|t| t.id == newest));
        for task in &snapshot.tasks {
            assert!(snapshot.tasks_cut.contains(&task.id));
            assert!(crate::limits::json_len(&task.brief) <= TASK_PREVIEW_BYTES);
        }
        // Oldest first.
        let times: Vec<_> = snapshot
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Resolved)
            .map(|t| t.created_at)
            .collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]));
    }
}
