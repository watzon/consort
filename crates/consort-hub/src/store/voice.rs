//! Write in my voice: the settings, the style guide that waits for "Do it",
//! the draft pairs and the `voice_guide` calls.

use consort_proto::client_api::EntityPayload;
use consort_proto::ids::{ApprovalId, SessionId, TaskId};
use consort_proto::model::{
    Approval, ApprovalDetails, DraftPair, ItemPayload, SessionItem, ToolKind, VoiceSettings,
    outgoing_kind,
};
use consort_proto::time::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use super::{Tx, get_doc, list_docs, meta_get, parse_doc, to_doc};
use crate::error::Result;

/// The brief of a voice task after 90 days. The brief quoted drafts.
pub const PURGED_VOICE_BRIEF: &str =
    "The brief of this voice task was removed after 90 days, because it quoted drafts.";

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

/// The meta key of a learned rule that waits for "Do it".
const RULE_PROPOSAL_KEY: &str = "voice_rule_proposal";

/// The learned rule that waits for "Do it", when the offer that waits is
/// about one rule.
pub(crate) fn rule_proposal(
    conn: &Connection,
) -> Result<Option<crate::voice::learn::RuleProposal>> {
    match meta_get(conn, RULE_PROPOSAL_KEY)?.filter(|t| !t.is_empty()) {
        Some(text) => Ok(Some(parse_doc(&text)?)),
        None => Ok(None),
    }
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

/// Whether a task learns the voice. The mark stays after Learn again.
pub fn is_voice_task(conn: &Connection, task_id: TaskId) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM voice_tasks WHERE task_id = ?1",
            [task_id.to_string()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Every voice task, oldest first.
pub fn voice_tasks(conn: &Connection) -> Result<Vec<TaskId>> {
    let mut stmt = conn.prepare("SELECT task_id FROM voice_tasks ORDER BY at, task_id")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        if let Ok(id) = row?.parse() {
            out.push(id);
        }
    }
    Ok(out)
}

/// The user's own words that started a task, when a coordinator turn with
/// user text started it.
pub fn task_request(conn: &Connection, task_id: TaskId) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT text FROM task_requests WHERE task_id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .optional()?)
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

/// The meta key that says the hub took the user's rules from the note once.
const USER_RULES_ADOPTED_KEY: &str = "voice_user_rules_adopted";

/// The user's rules as the `(you)` marks of `voice.md` say. Only the user's
/// own text may say so: a note that the user restored, or a note from a
/// hub before the rules were in the settings.
fn marked_rules(conn: &Connection) -> Result<Option<Vec<consort_proto::model::VoiceUserRule>>> {
    Ok(super::knowledge::memory_note(
        conn,
        consort_proto::model::MemoryScope::Global,
        consort_proto::model::VOICE_NOTE_PATH,
    )?
    .filter(|n| n.deleted_at.is_none())
    .map(|n| consort_proto::model::VoiceGuide::parse(&n.content).user_rule_list()))
}

/// Once per database: a hub from before the user's rules were in the
/// settings has them only as `(you)` marks in `voice.md`. They become the
/// user's rules. It runs at start, after the migrations.
pub fn adopt_user_rules_once(conn: &Connection) -> Result<()> {
    if meta_get(conn, USER_RULES_ADOPTED_KEY)?.is_some() {
        return Ok(());
    }
    if let Some(rules) = marked_rules(conn)? {
        let mut settings = settings(conn)?;
        if settings.user_rules.is_empty() && !rules.is_empty() {
            settings.user_rules = rules;
            conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![SETTINGS_KEY, to_doc(&settings)?],
            )?;
        }
    }
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, '1') ON CONFLICT(key) DO NOTHING",
        [USER_RULES_ADOPTED_KEY],
    )?;
    Ok(())
}

impl Tx<'_> {
    /// The user restored or moved a note to `voice.md`: its `(you)` marks
    /// are the user's rules now.
    pub fn adopt_user_rules(&mut self) -> Result<()> {
        let Some(rules) = marked_rules(self.db())? else {
            return Ok(());
        };
        let mut settings = settings(self.db())?;
        if settings.user_rules != rules {
            settings.user_rules = rules;
            self.save_voice_settings(&settings)?;
        }
        Ok(())
    }

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

    /// Stores the learned rule that waits for "Do it". `None` drops it.
    pub(crate) fn save_voice_rule_proposal(
        &mut self,
        rule: Option<&crate::voice::learn::RuleProposal>,
    ) -> Result<()> {
        let text = match rule {
            Some(rule) => to_doc(rule)?,
            None => String::new(),
        };
        self.meta_set(RULE_PROPOSAL_KEY, &text)
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

    /// Marks a task as a voice task.
    pub fn mark_voice_task(&mut self, task_id: TaskId) -> Result<()> {
        let now = self.now();
        self.db().execute(
            "INSERT OR IGNORE INTO voice_tasks (task_id, at) VALUES (?1, ?2)",
            params![task_id.to_string(), now.as_millis()],
        )?;
        Ok(())
    }

    /// Stores the user's own words that started a task.
    pub fn save_task_request(&mut self, task_id: TaskId, text: &str) -> Result<()> {
        self.db().execute(
            "INSERT INTO task_requests (task_id, text) VALUES (?1, ?2)
             ON CONFLICT(task_id) DO UPDATE SET text = excluded.text",
            params![task_id.to_string(), text],
        )?;
        Ok(())
    }

    /// Deletes every copy of a draft older than `before`: the draft pairs,
    /// the `voice_guide` calls, the text on the cards of draft and send
    /// tools, the arguments and results of those calls in session items,
    /// and the briefs of voice tasks (they quote drafts). Returns the
    /// number of deleted pairs.
    pub fn purge_voice(&mut self, before: Timestamp) -> Result<usize> {
        let pairs = self.db().execute(
            "DELETE FROM draft_pairs WHERE at < ?1",
            [before.as_millis()],
        )?;
        self.db()
            .execute("DELETE FROM voice_uses WHERE at < ?1", [before.as_millis()])?;
        self.purge_draft_cards(before)?;
        self.purge_draft_items(before)?;
        self.purge_voice_briefs(before)?;
        Ok(pairs)
    }

    /// Clears the text of old cards of draft and send tools.
    fn purge_draft_cards(&mut self, before: Timestamp) -> Result<()> {
        let approvals: Vec<Approval> = list_docs(
            self.db(),
            "SELECT data FROM approvals WHERE data LIKE '%\"outgoing\"%'",
            [],
        )?;
        for mut approval in approvals {
            if approval.created_at >= before {
                continue;
            }
            let ApprovalDetails::Connector(card) = &mut approval.details else {
                continue;
            };
            let empty = Value::Object(serde_json::Map::new());
            if card.outgoing.is_none()
                || (card.arguments == empty
                    && card.draft_arguments.is_none()
                    && card.fields.is_empty())
            {
                continue;
            }
            card.arguments = empty;
            card.draft_arguments = None;
            card.fields.clear();
            card.flags.clear();
            self.save_approval(&approval)?;
        }
        Ok(())
    }

    /// Clears the arguments and results of old `run_connector_tool` calls
    /// to draft and send tools in session items.
    fn purge_draft_items(&mut self, before: Timestamp) -> Result<()> {
        let rows: Vec<(String, String)> = {
            let mut stmt = self.db().prepare(
                "SELECT session_id, data FROM session_items WHERE data LIKE '%run_connector_tool%'",
            )?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for (session_id, data) in rows {
            let Ok(mut item) = parse_doc::<SessionItem>(&data) else {
                continue;
            };
            if item.created_at >= before {
                continue;
            }
            let ItemPayload::ToolCall(call) = &mut item.payload else {
                continue;
            };
            let slug = call
                .arguments
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("");
            let outgoing = outgoing_kind(slug, ToolKind::Change).is_some();
            let cleared = call
                .arguments
                .get("arguments")
                .is_none_or(|a| a.as_object().is_some_and(serde_json::Map::is_empty));
            if !call.tool.ends_with("run_connector_tool") || !outgoing || cleared {
                continue;
            }
            call.arguments["arguments"] = Value::Object(serde_json::Map::new());
            call.output = None;
            let channel: Option<String> = self
                .db()
                .query_row(
                    "SELECT channel_id FROM sessions WHERE id = ?1",
                    [&session_id],
                    |row| row.get(0),
                )
                .optional()?;
            match channel.and_then(|c| c.parse().ok()) {
                Some(channel_id) => self.save_item(&item, channel_id)?,
                None => self.save_item_row(&item)?,
            }
        }
        Ok(())
    }

    /// Replaces the briefs of old voice tasks.
    fn purge_voice_briefs(&mut self, before: Timestamp) -> Result<()> {
        let ids: Vec<String> = {
            let mut stmt = self
                .db()
                .prepare("SELECT task_id FROM voice_tasks WHERE at < ?1")?;
            let rows = stmt.query_map([before.as_millis()], |row| row.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for id in ids {
            let Ok(id) = id.parse::<TaskId>() else {
                continue;
            };
            if let Some(mut task) = super::work::task(self.db(), id)?
                && task.brief != PURGED_VOICE_BRIEF
            {
                task.brief = PURGED_VOICE_BRIEF.to_owned();
                self.save_task(&task)?;
            }
        }
        Ok(())
    }
}
