//! Reading and pruning the feed.

use consort_proto::client_api::{FEED_RETENTION_MS, FeedEntry};
use consort_proto::time::Timestamp;
use rusqlite::Connection;
use serde_json::{Value, json};

use super::{Tx, meta_get};
use crate::error::Result;

/// The highest `seq` ever written. It survives pruning.
pub fn head_seq(conn: &Connection) -> Result<u64> {
    let seq: Option<i64> = conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'feed'",
            [],
            |row| row.get(0),
        )
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    Ok(seq.and_then(|s| u64::try_from(s).ok()).unwrap_or(0))
}

/// The highest `seq` that pruning removed. A client whose position is below
/// this value has missed entries and must reload its snapshot.
pub fn floor_seq(conn: &Connection) -> Result<u64> {
    Ok(meta_get(conn, "feed_floor")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

/// Entries with `seq > after_seq`, oldest first.
pub fn entries_after(conn: &Connection, after_seq: u64, limit: usize) -> Result<Vec<FeedEntry>> {
    let mut stmt = conn.prepare_cached(
        "SELECT seq, at, channel_id, entity, entity_id, op, payload
         FROM feed WHERE seq > ?1 ORDER BY seq LIMIT ?2",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            i64::try_from(after_seq).unwrap_or(i64::MAX),
            i64::try_from(limit).unwrap_or(i64::MAX)
        ],
        |row| {
            let payload: Option<String> = row.get(6)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                payload,
            ))
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (seq, at, channel_id, entity, entity_id, op, payload) = row?;
        let payload: Value = match payload {
            Some(text) => serde_json::from_str(&text)?,
            None => Value::Null,
        };
        let value = json!({
            "seq": seq,
            "at": at,
            "channel_id": channel_id,
            "entity": entity,
            "entity_id": entity_id,
            "op": op,
            "payload": payload,
        });
        out.push(serde_json::from_value(value)?);
    }
    Ok(out)
}

impl Tx<'_> {
    /// Deletes feed entries older than the retention window and records the
    /// floor. Returns the number of deleted entries.
    pub fn prune_feed(&mut self) -> Result<usize> {
        let cutoff = self
            .now()
            .plus_millis(-FEED_RETENTION_MS)
            .max(Timestamp::EPOCH);
        let highest: Option<i64> = self.db().query_row(
            "SELECT MAX(seq) FROM feed WHERE at < ?1",
            [cutoff.as_millis()],
            |row| row.get(0),
        )?;
        let Some(highest) = highest else {
            return Ok(0);
        };
        let removed = self
            .db()
            .execute("DELETE FROM feed WHERE seq <= ?1", [highest])?;
        self.meta_set("feed_floor", &highest.to_string())?;
        Ok(removed)
    }
}
