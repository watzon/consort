//! Snapshots and peer lookups.

use consort_proto::auth::{PeerId, PublicKey};
use consort_proto::client_api::{Snapshot, SnapshotScope};
use consort_proto::ids::ClientId;
use consort_proto::model::Message;

use crate::error::Result;
use crate::hub::Hub;
use crate::store::{conversation, feed, knowledge, machine, routines, work};

impl Hub {
    /// Builds a snapshot in one read transaction, so `seq` matches the entities.
    ///
    /// The `all` scope has no messages and no channel files. A channel scope
    /// has its newest 50 timeline messages, oldest first.
    pub(crate) async fn snapshot(&self, scope: SnapshotScope) -> Result<Snapshot> {
        self.store
            .read(move |conn| {
                let tx = conn.unchecked_transaction()?;
                let seq = feed::head_seq(&tx)?;
                let snapshot = match scope {
                    SnapshotScope::All => Snapshot {
                        seq,
                        groups: conversation::groups(&tx)?,
                        channels: conversation::channels(&tx)?,
                        repos: conversation::repos(&tx)?,
                        workers: machine::workers(&tx)?,
                        read_markers: conversation::read_markers(&tx)?,
                        tasks: work::tasks_not_archived(&tx)?,
                        approvals: work::pending_approvals(&tx, None)?,
                        decisions: work::pending_decisions(&tx, None)?,
                        routines: routines::routines(&tx)?,
                        ..Snapshot::default()
                    },
                    SnapshotScope::Channel(channel_id) => {
                        let channel = conversation::channel_or_404(&tx, channel_id)?;
                        let (mut messages, _) = conversation::messages_page(
                            &tx,
                            channel_id,
                            None,
                            None,
                            consort_proto::client_api::SNAPSHOT_MESSAGE_LIMIT,
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
                            read_markers,
                            tasks: work::tasks_in_channel(&tx, channel_id, false)?,
                            approvals: work::pending_approvals(&tx, Some(channel_id))?,
                            decisions: work::pending_decisions(&tx, Some(channel_id))?,
                            routines: routines::routines_in_channel(&tx, channel_id)?,
                            channel_files: knowledge::channel_files(&tx, channel_id, None)?,
                            ..Snapshot::default()
                        }
                    }
                };
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
