//! The outbox: messages that the user sent and the hub has not stored yet.
//!
//! One task sends them, one at a time, in the order the user sent them. A
//! message leaves the outbox when the hub stores it, or when the hub refuses
//! it. A link that drops leaves it in place, because the hub may have stored
//! it already: the next try carries the same `client_message_id`, and the hub
//! keeps only the first copy. The outbox is also saved on disk (see
//! [`crate::store::Store::save_outbox`]).

use std::sync::Arc;

use consort_proto::client_api::MessageSend;
use tokio::sync::oneshot;

use super::{Shared, invalid, is_transport};
use crate::{
    ClientError,
    cache::{PendingSend, SendFailure},
};

/// What happened to a message that a caller queued.
pub(super) enum SendOutcome {
    /// The hub stored it.
    Sent,
    /// It waits in the outbox for a working link.
    Deferred,
    /// The hub refused it. It is not in the outbox any more.
    Failed(ClientError),
}

impl Shared {
    /// Puts a message in the outbox and wakes the sender. The receiver says
    /// what became of it.
    fn enqueue(&self, pending: PendingSend) -> oneshot::Receiver<SendOutcome> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut waiters) = self.waiters.lock() {
            waiters.insert(pending.client_message_id.clone(), tx);
        }
        if let Ok(mut cache) = self.cache.lock() {
            cache.pending.push(pending);
        }
        self.publish();
        self.outbox_wake.notify_one();
        rx
    }

    fn take_waiter(&self, client_message_id: &str) -> Option<oneshot::Sender<SendOutcome>> {
        self.waiters
            .lock()
            .ok()
            .and_then(|mut w| w.remove(client_message_id))
    }

    /// Tells every caller that waits that its message is in the outbox for
    /// later.
    pub(super) fn defer_waiters(&self) {
        let all: Vec<_> = self
            .waiters
            .lock()
            .map(|mut w| w.drain().map(|(_, tx)| tx).collect())
            .unwrap_or_default();
        for tx in all {
            let _ = tx.send(SendOutcome::Deferred);
        }
    }

    /// The oldest message in the outbox, when the link is up.
    fn next_to_send(&self) -> Option<PendingSend> {
        if !self.is_online() {
            return None;
        }
        self.cache
            .lock()
            .ok()
            .and_then(|c| c.pending.first().cloned())
    }

    /// Takes a message out of the outbox after the hub refused it. When no
    /// caller waits to hear about it, the view shows the failure, so the
    /// text is not lost.
    fn drop_refused(&self, pending: &PendingSend, err: &ClientError, report: bool) {
        if let Ok(mut cache) = self.cache.lock() {
            cache
                .pending
                .retain(|p| p.client_message_id != pending.client_message_id);
            if report {
                cache.send_failures.push(SendFailure {
                    id: pending.client_message_id.clone(),
                    channel_id: pending.channel_id.clone(),
                    body: pending.body.clone(),
                    message: err.to_string(),
                });
            }
        }
        self.publish();
    }
}

/// Queues a message and waits until the hub stores it, refuses it, or the
/// link is not there. Offline, the message stays in the outbox and the answer
/// is `Ok`: the message shows as sending and goes out on reconnect. A message
/// that the hub refuses is an error, and the caller keeps its text.
pub(super) async fn queue_send(
    shared: &Arc<Shared>,
    pending: PendingSend,
) -> Result<(), ClientError> {
    match shared.enqueue(pending).await {
        Ok(SendOutcome::Failed(err)) => Err(err),
        Ok(SendOutcome::Sent | SendOutcome::Deferred) | Err(_) => Ok(()),
    }
}

/// Sends the messages in the outbox, oldest first, for as long as the client
/// lives.
pub(crate) async fn run_outbox(shared: Arc<Shared>) {
    loop {
        let Some(next) = shared.next_to_send() else {
            if !shared.is_online() {
                shared.defer_waiters();
            }
            shared.outbox_wake.notified().await;
            continue;
        };
        match send_pending(&shared, &next).await {
            Ok(()) => {
                if let Some(tx) = shared.take_waiter(&next.client_message_id) {
                    let _ = tx.send(SendOutcome::Sent);
                }
            }
            Err(err) if is_transport(&err) => {
                // The hub may or may not have stored it. Keep it, and try
                // again with the same ID when the link is back.
                shared.defer_waiters();
                shared.outbox_wake.notified().await;
            }
            Err(err) => {
                let waiter = shared.take_waiter(&next.client_message_id);
                shared.drop_refused(&next, &err, waiter.is_none());
                if let Some(tx) = waiter {
                    let _ = tx.send(SendOutcome::Failed(err));
                }
            }
        }
    }
}

async fn send_pending(shared: &Arc<Shared>, p: &PendingSend) -> Result<(), ClientError> {
    let body = MessageSend {
        channel_id: p.channel_id.parse().map_err(|_| {
            invalid("Consort cannot use this channel. Reload the app, then try again")
        })?,
        client_message_id: p.client_message_id.clone(),
        body: p.body.clone(),
        attachments: Vec::new(),
        thread_root_id: match &p.thread_root_id {
            Some(id) => Some(id.parse().map_err(|_| {
                invalid("Consort cannot use this thread. Reload the app, then try again")
            })?),
            None => None,
        },
        in_timeline: false,
    };
    let result = shared.call(body).await?;
    if let Ok(mut cache) = shared.cache.lock() {
        cache
            .pending
            .retain(|x| x.client_message_id != p.client_message_id);
        cache
            .messages
            .insert(result.message.id.to_string(), result.message);
    }
    shared.publish();
    Ok(())
}
