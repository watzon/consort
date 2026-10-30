//! Saving the cache to disk: after a change, a moment later, in one
//! transaction (see [`crate::persist`]). Also the time when the data was
//! last known to be current, and the removal of the saved data.

use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use super::Shared;
use crate::{
    format,
    persist::{self, CacheFile, Owner},
};

/// How long the client waits after a change before it saves. A burst of
/// changes makes one save. The wait does not restart on a new change, so a
/// feed that never stops is still saved at this pace.
pub(super) const SAVE_DELAY: Duration = Duration::from_millis(500);

/// The saved cache of a [`Shared`].
pub(super) enum Slot {
    /// Not opened yet, or removed. The next save opens it.
    Closed,
    Open(Box<CacheFile>),
    /// The file cannot be made. The app runs without saved data, and no save
    /// tries again until the user pairs again.
    Unusable,
}

impl Shared {
    /// Notes that the data on screen is current: the link is up, or it was
    /// up a moment ago.
    pub(super) fn mark_fresh(&self) {
        self.fresh_at.store(format::now_ms(), Ordering::Release);
    }

    /// When the data was last known to be current, in Unix milliseconds, or
    /// 0 when it never was.
    pub(super) fn fresh_at(&self) -> i64 {
        self.fresh_at.load(Ordering::Acquire)
    }

    /// Asks for a save of the cache.
    pub(super) fn request_save(&self) {
        self.save_wake.notify_one();
    }

    /// Saves the cache now, when it is worth saving. Runs on a blocking
    /// thread. A failed save is logged, and the next change tries again.
    fn save_cache(&self) {
        let Some(store) = &self.store else {
            return;
        };
        let Ok(mut slot) = self.cache_file.lock() else {
            return;
        };
        // The identity is read under the lock, so a save that waited for
        // `forget_saved` does not write the data of the old hub.
        let Some(identity) = self.identity() else {
            return;
        };
        let owner = Owner::of(&identity);
        if matches!(*slot, Slot::Closed) {
            let (file, _) = persist::open_or_recover(&store.cache_path(), &owner);
            *slot = file.map_or(Slot::Unusable, |f| Slot::Open(Box::new(f)));
        }
        let Slot::Open(file) = &mut *slot else {
            return;
        };
        let rows = match self.cache.lock() {
            // A half load is not worth keeping. The file keeps the last
            // whole one.
            Ok(cache) if cache.loaded => persist::rows(&cache, self.fresh_at(), &owner),
            _ => return,
        };
        match rows.and_then(|rows| file.write(rows)) {
            Ok(changed) => tracing::debug!(changed, "saved the cache"),
            Err(err) => tracing::warn!(error = %err, "cannot save the cache"),
        }
    }

    /// Removes the saved data, for a client that pairs with another hub or
    /// unpairs.
    pub(super) fn forget_saved(&self) {
        let Some(store) = &self.store else {
            return;
        };
        if let Ok(mut slot) = self.cache_file.lock() {
            // Close the file before it goes: Windows keeps an open file.
            *slot = Slot::Closed;
            persist::delete(&store.cache_path());
        }
        self.fresh_at.store(0, Ordering::Release);
    }
}

/// Saves the cache a moment after each change.
pub(crate) async fn run_save(shared: Arc<Shared>) {
    loop {
        shared.save_wake.notified().await;
        tokio::time::sleep(SAVE_DELAY).await;
        let worker = shared.clone();
        if let Err(err) = tokio::task::spawn_blocking(move || worker.save_cache()).await {
            tracing::warn!(error = %err, "the save of the cache did not finish");
        }
    }
}
