//! Write in my voice: the settings, the style guide that waits for "Do it",
//! the draft pairs and the `voice_guide` calls.

use consort_proto::client_api::EntityPayload;
use consort_proto::ids::{ApprovalId, SessionId};
use consort_proto::model::{DraftPair, VoiceSettings};
use consort_proto::time::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};

use super::{Tx, get_doc, list_docs, meta_get, parse_doc, to_doc};
use crate::error::Result;

const SETTINGS_KEY: &str = "voice";
const PROPOSAL_KEY: &str = "voice_proposal";

/// The settings. A hub that has none stored yet has the defaults: off.
pub fn settings(conn: &Connection) -> Result<VoiceSettings> {
    match meta_get(conn, SETTINGS_KEY)? {
        Some(text) => parse_doc(&text),
        None => Ok(VoiceSettings::default()),
    }
}

/// The style guide that waits for "Do it", when one waits.
pub fn proposal(conn: &Connection) -> Result<Option<String>> {
    Ok(meta_get(conn, PROPOSAL_KEY)?.filter(|t| !t.is_empty()))
}

/// The pair of a card.
pub fn draft_pair(conn: &Connection, approval_id: ApprovalId) -> Result<Option<DraftPair>> {
    get_doc(
        conn,
        "SELECT data FROM draft_pairs WHERE approval_id = ?1",
        [approval_id.to_string()],
    )
}

/// The edited pairs of a context that no `voice_edits` wake took yet,
/// oldest first.
pub fn unused_edited_pairs(conn: &Connection, context: &str) -> Result<Vec<DraftPair>> {
    list_docs(
        conn,
        "SELECT data FROM draft_pairs
         WHERE context = ?1 AND unchanged = 0 AND used = 0 ORDER BY at, approval_id",
        [context],
    )
}

/// The newest pairs, newest first. `unchanged` picks the text that the
/// assistant wrote (true) or the edited pairs (false).
pub fn recent_pairs(conn: &Connection, unchanged: bool, limit: usize) -> Result<Vec<DraftPair>> {
    list_docs(
        conn,
        "SELECT data FROM draft_pairs WHERE unchanged = ?1 ORDER BY at DESC, approval_id LIMIT ?2",
        params![unchanged, i64::try_from(limit).unwrap_or(i64::MAX)],
    )
}

/// Whether the user sent anything through Consort to this recipient
/// before.
pub fn wrote_to_before(conn: &Connection, recipient: &str) -> Result<bool> {
    let wanted = recipient.trim().to_lowercase();
    if wanted.is_empty() {
        return Ok(false);
    }
    let pairs: Vec<DraftPair> =
        list_docs(conn, "SELECT data FROM draft_pairs ORDER BY at DESC", [])?;
    Ok(pairs.iter().any(|p| {
        p.recipients
            .iter()
            .any(|r| r.trim().to_lowercase() == wanted)
    }))
}

/// The newest context that a session asked `voice_guide` for.
pub fn last_voice_use(conn: &Connection, session_id: SessionId) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT context FROM voice_uses WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
            [session_id.to_string()],
            |row| row.get(0),
        )
        .optional()?)
}

impl Tx<'_> {
    /// Stores the settings and sends a feed upsert.
    pub fn save_voice_settings(&mut self, settings: &VoiceSettings) -> Result<()> {
        self.meta_set(SETTINGS_KEY, &to_doc(settings)?)?;
        self.emit(EntityPayload::VoiceSettings(settings.clone()))
    }

    /// Stores the style guide that waits for "Do it". An empty text drops
    /// it.
    pub fn save_voice_proposal(&mut self, text: &str) -> Result<()> {
        self.meta_set(PROPOSAL_KEY, text)
    }

    /// Stores a draft pair.
    pub fn save_draft_pair(&mut self, pair: &DraftPair) -> Result<()> {
        self.db().execute(
            "INSERT INTO draft_pairs (approval_id, context, unchanged, at, data)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(approval_id) DO UPDATE SET context = excluded.context,
                unchanged = excluded.unchanged, data = excluded.data",
            params![
                pair.approval_id.to_string(),
                pair.context,
                pair.unchanged,
                pair.at.as_millis(),
                to_doc(pair)?,
            ],
        )?;
        Ok(())
    }

    /// Marks pairs as taken by a `voice_edits` wake.
    pub fn mark_pairs_used(&mut self, ids: &[ApprovalId]) -> Result<()> {
        for id in ids {
            self.db().execute(
                "UPDATE draft_pairs SET used = 1 WHERE approval_id = ?1",
                [id.to_string()],
            )?;
        }
        Ok(())
    }

    /// Records a `voice_guide` call.
    pub fn record_voice_use(&mut self, session_id: SessionId, context: &str) -> Result<()> {
        let now = self.now();
        self.db().execute(
            "INSERT INTO voice_uses (session_id, context, at) VALUES (?1, ?2, ?3)",
            params![session_id.to_string(), context, now.as_millis()],
        )?;
        Ok(())
    }

    /// Deletes draft pairs and `voice_guide` calls older than `before`.
    /// Returns the number of deleted pairs.
    pub fn purge_voice(&mut self, before: Timestamp) -> Result<usize> {
        let pairs = self.db().execute(
            "DELETE FROM draft_pairs WHERE at < ?1",
            [before.as_millis()],
        )?;
        self.db()
            .execute("DELETE FROM voice_uses WHERE at < ?1", [before.as_millis()])?;
        Ok(pairs)
    }
}
