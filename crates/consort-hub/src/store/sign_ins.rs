//! Sign-ins: one row for each run of a provider's sign-in flow.
//!
//! A row holds the `SignIn` record only. It never holds the code that the
//! user pastes, and never a secret variable.

use consort_proto::client_api::{EntityKind, EntityPayload};
use consort_proto::ids::{SignInId, WorkerId};
use consort_proto::model::{SIGN_IN_RETENTION_MS, SignIn, SignInState};
use rusqlite::{Connection, params};

use super::{Tx, get_doc, list_docs, to_doc};
use crate::error::{HubError, Result};

pub fn sign_in(conn: &Connection, id: SignInId) -> Result<Option<SignIn>> {
    get_doc(
        conn,
        "SELECT data FROM sign_ins WHERE id = ?1",
        [id.to_string()],
    )
}

pub fn sign_in_or_404(conn: &Connection, id: SignInId) -> Result<SignIn> {
    sign_in(conn, id)?
        .ok_or_else(|| HubError::not_found(format!("The sign-in {id} does not exist.")))
}

/// Every stored sign-in, oldest first.
pub fn sign_ins(conn: &Connection) -> Result<Vec<SignIn>> {
    list_docs(conn, "SELECT data FROM sign_ins ORDER BY rowid", [])
}

/// The sign-ins that have not ended, oldest first.
pub fn open_sign_ins(conn: &Connection) -> Result<Vec<SignIn>> {
    Ok(sign_ins(conn)?
        .into_iter()
        .filter(|s| !s.state.is_final())
        .collect())
}

impl Tx<'_> {
    /// Stores a sign-in and adds a feed upsert.
    pub fn save_sign_in(&mut self, sign_in: &SignIn) -> Result<()> {
        self.db().execute(
            "INSERT INTO sign_ins (id, worker_id, updated_at, data) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 worker_id = excluded.worker_id,
                 updated_at = excluded.updated_at,
                 data = excluded.data",
            params![
                sign_in.id.to_string(),
                sign_in.worker_id.to_string(),
                sign_in.updated_at.as_millis(),
                to_doc(sign_in)?
            ],
        )?;
        self.emit(EntityPayload::SignIn(sign_in.clone()))
    }

    /// Ends every sign-in of the worker that has not ended, with the
    /// `failed` state. Returns the sign-ins that changed.
    pub fn fail_sign_ins_of_worker(
        &mut self,
        worker_id: WorkerId,
        message: &str,
    ) -> Result<Vec<SignIn>> {
        let open: Vec<SignIn> = list_docs(
            self.db(),
            "SELECT data FROM sign_ins WHERE worker_id = ?1 ORDER BY rowid",
            [worker_id.to_string()],
        )?;
        let mut changed = Vec::new();
        for mut sign_in in open.into_iter().filter(|s| !s.state.is_final()) {
            sign_in.state = SignInState::Failed {
                message: message.to_owned(),
            };
            sign_in.updated_at = self.now();
            self.save_sign_in(&sign_in)?;
            changed.push(sign_in);
        }
        Ok(changed)
    }

    /// Ends the sign-ins that got no update before their `expires_at`, with
    /// the `expired` state. Returns the sign-ins that changed.
    pub fn expire_sign_ins(&mut self) -> Result<Vec<SignIn>> {
        let now = self.now();
        let mut changed = Vec::new();
        for mut sign_in in open_sign_ins(self.db())? {
            if sign_in.expires_at > now {
                continue;
            }
            sign_in.state = SignInState::Expired;
            sign_in.updated_at = now;
            self.save_sign_in(&sign_in)?;
            changed.push(sign_in);
        }
        Ok(changed)
    }

    /// Deletes the sign-ins that got their last update longer than
    /// [`SIGN_IN_RETENTION_MS`] ago, and adds the feed deletes. Returns the
    /// number of deleted rows.
    pub fn delete_old_sign_ins(&mut self) -> Result<usize> {
        let cutoff = self.now().plus_millis(-SIGN_IN_RETENTION_MS);
        let old: Vec<String> = {
            let mut stmt = self
                .db()
                .prepare_cached("SELECT id FROM sign_ins WHERE updated_at < ?1 ORDER BY rowid")?;
            let rows = stmt.query_map([cutoff.as_millis()], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for id in &old {
            self.db()
                .execute("DELETE FROM sign_ins WHERE id = ?1", [id])?;
            self.emit_delete(EntityKind::SignIn, id.clone(), None)?;
        }
        Ok(old.len())
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::ids::AccountId;
    use consort_proto::model::{SIGN_IN_IDLE_EXPIRY_MS, SignInMethod};
    use consort_proto::time::Timestamp;

    use super::*;
    use crate::store::testing::temp_store;

    fn record(worker_id: WorkerId, state: SignInState, age_ms: i64) -> SignIn {
        let at = Timestamp::now().plus_millis(-age_ms);
        SignIn {
            id: SignInId::new(),
            worker_id,
            account_id: AccountId::new(),
            method: SignInMethod::ClaudePlan,
            state,
            created_at: at,
            updated_at: at,
            expires_at: at.plus_millis(SIGN_IN_IDLE_EXPIRY_MS),
        }
    }

    #[tokio::test]
    async fn a_sign_in_round_trips_and_emits_a_feed_upsert() {
        let (store, _dir) = temp_store();
        let mut feed = store.subscribe();
        let stored = record(WorkerId::new(), SignInState::Starting, 0);
        let copy = stored.clone();
        store.write(move |tx| tx.save_sign_in(&copy)).await.unwrap();
        let entry = feed.recv().await.unwrap();
        assert_eq!(entry.entity_id, stored.id.to_string());
        let id = stored.id;
        let read = store.read(move |conn| sign_in_or_404(conn, id)).await;
        assert_eq!(read.unwrap(), stored);
        let missing = store
            .read(move |conn| sign_in_or_404(conn, SignInId::new()))
            .await;
        assert_eq!(
            missing.unwrap_err().code(),
            consort_proto::error::ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn only_sign_ins_of_the_worker_that_have_not_ended_fail() {
        let (store, _dir) = temp_store();
        let (mine, other) = (WorkerId::new(), WorkerId::new());
        let open = record(mine, SignInState::Verifying, 0);
        let done = record(mine, SignInState::Succeeded { detail: None }, 0);
        let elsewhere = record(other, SignInState::Starting, 0);
        let (open_id, done_id, other_id) = (open.id, done.id, elsewhere.id);
        let changed = store
            .write(move |tx| {
                tx.save_sign_in(&open)?;
                tx.save_sign_in(&done)?;
                tx.save_sign_in(&elsewhere)?;
                tx.fail_sign_ins_of_worker(mine, "The worker went offline.")
            })
            .await
            .unwrap();
        assert_eq!(changed.len(), 1);
        let (open, done, elsewhere) = store
            .read(move |conn| {
                Ok((
                    sign_in_or_404(conn, open_id)?,
                    sign_in_or_404(conn, done_id)?,
                    sign_in_or_404(conn, other_id)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(
            open.state,
            SignInState::Failed {
                message: "The worker went offline.".to_owned()
            }
        );
        assert_eq!(done.state, SignInState::Succeeded { detail: None });
        assert_eq!(elsewhere.state, SignInState::Starting);
    }

    #[tokio::test]
    async fn idle_sign_ins_expire_and_old_rows_go() {
        let (store, _dir) = temp_store();
        let worker = WorkerId::new();
        let idle = record(
            worker,
            SignInState::Starting,
            SIGN_IN_IDLE_EXPIRY_MS + 1_000,
        );
        let fresh = record(worker, SignInState::Starting, 1_000);
        let ended_long_ago = record(worker, SignInState::Cancelled, SIGN_IN_RETENTION_MS + 1_000);
        let ended_lately = record(worker, SignInState::Cancelled, 1_000);
        let ids = (idle.id, fresh.id, ended_long_ago.id, ended_lately.id);
        let (expired, deleted) = store
            .write(move |tx| {
                for s in [&idle, &fresh, &ended_long_ago, &ended_lately] {
                    tx.save_sign_in(s)?;
                }
                let expired = tx.expire_sign_ins()?;
                let deleted = tx.delete_old_sign_ins()?;
                Ok((expired, deleted))
            })
            .await
            .unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, ids.0);
        assert_eq!(expired[0].state, SignInState::Expired);
        assert_eq!(deleted, 1);
        let remaining = store.read(sign_ins).await.unwrap();
        let remaining: Vec<_> = remaining.iter().map(|s| s.id).collect();
        assert_eq!(remaining, vec![ids.0, ids.1, ids.3]);
    }
}
