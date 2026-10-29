//! The worker link: connections, the command queue and session events.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{Frame, FrameId};
use consort_proto::ids::{ItemId, SessionId, WorkerId};
use serde_json::Value;
use tokio::sync::oneshot;

use crate::net::ws::Outbox;

pub(crate) mod commands;
pub(crate) mod events;
pub(crate) mod lifecycle;

type Pending = HashMap<FrameId, oneshot::Sender<Result<Value, ErrorBody>>>;

/// The most hub tool calls that one worker link runs at the same time.
/// More calls wait for a free place. A personal hub runs a few sessions per
/// worker, so this is far above normal use.
pub(crate) const MAX_TOOL_CALLS_PER_LINK: usize = 16;

/// Why a call to a worker did not get an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallError {
    /// The connection is closed or closed while we waited.
    Gone,
    /// The worker did not answer in time.
    Timeout,
    /// The request does not encode as a frame, for example because it is
    /// larger than the frame limit. Sending it again cannot help.
    Encode(String),
}

/// One live worker connection.
pub(crate) struct WorkerLink {
    pub conn_id: u64,
    pub worker_id: WorkerId,
    pub outbox: Outbox,
    /// Places for running tool calls. See [`MAX_TOOL_CALLS_PER_LINK`].
    pub tool_calls: Arc<tokio::sync::Semaphore>,
    pending: Mutex<Pending>,
    inflight: Mutex<HashSet<Option<SessionId>>>,
}

impl WorkerLink {
    pub(crate) fn new(conn_id: u64, worker_id: WorkerId, outbox: Outbox) -> Self {
        Self {
            conn_id,
            worker_id,
            outbox,
            tool_calls: Arc::new(tokio::sync::Semaphore::new(MAX_TOOL_CALLS_PER_LINK)),
            pending: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashSet::new()),
        }
    }

    /// Sends a request and waits for the worker's response.
    pub(crate) async fn call(
        &self,
        id: FrameId,
        kind: String,
        body: Value,
        timeout: Duration,
    ) -> Result<Result<Value, ErrorBody>, CallError> {
        let (reply, receiver) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| CallError::Gone)?
            .insert(id.clone(), reply);
        let frame = Frame::Request(consort_proto::frame::RequestFrame {
            id: id.clone(),
            kind,
            body,
        });
        let text = match frame.to_text() {
            Ok(text) => text,
            Err(err) => {
                self.forget(&id);
                return Err(CallError::Encode(err.to_string()));
            }
        };
        if !self.outbox.send_text(text) {
            self.forget(&id);
            return Err(CallError::Gone);
        }
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err(CallError::Gone),
            Err(_) => {
                self.forget(&id);
                Err(CallError::Timeout)
            }
        }
    }

    fn forget(&self, id: &FrameId) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(id);
        }
    }

    /// Hands a response from the worker to the caller that waits for it.
    pub(crate) fn resolve(&self, re: &FrameId, result: Result<Value, ErrorBody>) {
        let reply = self.pending.lock().ok().and_then(|mut p| p.remove(re));
        if let Some(reply) = reply {
            let _ = reply.send(result);
        }
    }

    /// Marks a queue key (a session, or `None` for worker-level commands) as
    /// busy. Returns false when it is already busy.
    pub(crate) fn mark_inflight(&self, key: Option<SessionId>) -> bool {
        self.inflight.lock().is_ok_and(|mut set| set.insert(key))
    }

    pub(crate) fn clear_inflight(&self, key: Option<SessionId>) {
        if let Ok(mut set) = self.inflight.lock() {
            set.remove(&key);
        }
    }

    /// Fails every waiting call. Used when the connection closes.
    pub(crate) fn fail_pending(&self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
    }
}

/// The time of the newest feed entry that the hub sent for an item, for the
/// 250 ms merge rule.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ItemEmit {
    pub last: Instant,
    pub flush_scheduled: bool,
}

/// What to do with a feed entry for an item update that is still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmitNow {
    Yes,
    /// Wait this long, then emit the newest stored state.
    Later(Duration),
    /// A flush is already scheduled and will emit the newest stored state.
    Scheduled,
}

#[derive(Default)]
pub struct Links {
    workers: Mutex<HashMap<WorkerId, std::sync::Arc<WorkerLink>>>,
    next_conn: AtomicU64,
    item_emit: Mutex<HashMap<(SessionId, ItemId), ItemEmit>>,
}

pub(crate) const ITEM_MERGE_WINDOW: Duration = Duration::from_millis(250);

impl Links {
    pub(crate) fn next_conn_id(&self) -> u64 {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
    }

    /// Adds a link. Returns the link that it replaces.
    pub(crate) fn register(
        &self,
        link: std::sync::Arc<WorkerLink>,
    ) -> Option<std::sync::Arc<WorkerLink>> {
        self.workers.lock().ok()?.insert(link.worker_id, link)
    }

    /// Removes the link if it is still the current one. Returns whether it was.
    pub(crate) fn remove(&self, worker_id: WorkerId, conn_id: u64) -> bool {
        let Ok(mut workers) = self.workers.lock() else {
            return false;
        };
        if workers
            .get(&worker_id)
            .is_some_and(|l| l.conn_id == conn_id)
        {
            workers.remove(&worker_id);
            true
        } else {
            false
        }
    }

    pub(crate) fn get(&self, worker_id: WorkerId) -> Option<std::sync::Arc<WorkerLink>> {
        self.workers.lock().ok()?.get(&worker_id).cloned()
    }

    pub(crate) fn is_online(&self, worker_id: WorkerId) -> bool {
        self.get(worker_id).is_some_and(|l| !l.outbox.is_closed())
    }

    /// Decides whether a feed entry for a running item goes out now.
    pub(crate) fn item_emit_decision(
        &self,
        session: SessionId,
        item: ItemId,
        now: Instant,
    ) -> EmitNow {
        let Ok(mut map) = self.item_emit.lock() else {
            return EmitNow::Yes;
        };
        let entry = map.entry((session, item));
        match entry {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(ItemEmit {
                    last: now,
                    flush_scheduled: false,
                });
                EmitNow::Yes
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                let state = slot.get_mut();
                let age = now.saturating_duration_since(state.last);
                if age >= ITEM_MERGE_WINDOW && !state.flush_scheduled {
                    state.last = now;
                    EmitNow::Yes
                } else if state.flush_scheduled {
                    EmitNow::Scheduled
                } else {
                    state.flush_scheduled = true;
                    EmitNow::Later(ITEM_MERGE_WINDOW - age)
                }
            }
        }
    }

    /// Called when a scheduled flush has emitted.
    pub(crate) fn item_flushed(&self, session: SessionId, item: ItemId, now: Instant) {
        if let Ok(mut map) = self.item_emit.lock()
            && let Some(state) = map.get_mut(&(session, item))
        {
            state.last = now;
            state.flush_scheduled = false;
        }
    }

    /// Forgets an item that is complete.
    pub(crate) fn item_done(&self, session: SessionId, item: ItemId) {
        if let Ok(mut map) = self.item_emit.lock() {
            map.remove(&(session, item));
        }
    }
}

pub(crate) fn worker_offline(name: &str) -> ErrorBody {
    ErrorBody::new(
        ErrorCode::WorkerOffline,
        format!("{name} is offline. Try again when it is back."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_updates_inside_the_window_merge_into_one_flush() {
        let links = Links::default();
        let (s, i) = (SessionId::new(), ItemId::new());
        let t0 = Instant::now();
        assert_eq!(links.item_emit_decision(s, i, t0), EmitNow::Yes);
        let later = links.item_emit_decision(s, i, t0 + Duration::from_millis(50));
        assert!(matches!(later, EmitNow::Later(d) if d <= ITEM_MERGE_WINDOW));
        // A third update while the flush waits adds nothing.
        assert_eq!(
            links.item_emit_decision(s, i, t0 + Duration::from_millis(100)),
            EmitNow::Scheduled
        );
        links.item_flushed(s, i, t0 + Duration::from_millis(250));
        // After the flush, the window starts again.
        assert!(matches!(
            links.item_emit_decision(s, i, t0 + Duration::from_millis(300)),
            EmitNow::Later(_)
        ));
        links.item_done(s, i);
        assert_eq!(
            links.item_emit_decision(s, i, t0 + Duration::from_millis(310)),
            EmitNow::Yes
        );
    }
}
