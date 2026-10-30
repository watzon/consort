//! Tasks, sessions, session items, approvals and decisions.

use std::collections::HashMap;

use consort_proto::client_api::{EntityKind, EntityPayload};
use consort_proto::ids::{
    ChannelId, ItemId, MessageId, RequestId, SessionId, TaskId, TurnId, WorkerId,
};
use consort_proto::model::{Approval, BlobRef, Decision, Session, SessionItem, Task};
use consort_proto::status::TaskStatus;
use consort_proto::time::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};

use super::{Tx, get_doc, list_docs, to_doc};
use crate::error::{HubError, Result};

// Tasks ------------------------------------------------------------------

pub fn task(conn: &Connection, id: TaskId) -> Result<Option<Task>> {
    get_doc(
        conn,
        "SELECT data FROM tasks WHERE id = ?1",
        [id.to_string()],
    )
}

pub fn task_or_404(conn: &Connection, id: TaskId) -> Result<Task> {
    task(conn, id)?.ok_or_else(|| HubError::not_found(format!("The task {id} does not exist.")))
}

/// Tasks of one channel, oldest first.
pub fn tasks_in_channel(
    conn: &Connection,
    channel_id: ChannelId,
    include_archived: bool,
) -> Result<Vec<Task>> {
    list_docs(
        conn,
        "SELECT data FROM tasks WHERE channel_id = ?1 AND (?2 OR archived_at IS NULL) ORDER BY rowid",
        params![channel_id.to_string(), include_archived],
    )
}

/// Every task that is not archived, oldest first.
pub fn tasks_not_archived(conn: &Connection) -> Result<Vec<Task>> {
    list_docs(
        conn,
        "SELECT data FROM tasks WHERE archived_at IS NULL ORDER BY rowid",
        [],
    )
}

/// A page of the archived tasks, the most recently archived first, and
/// whether more follow. With a channel, only that channel's tasks.
pub fn archived_tasks(
    conn: &Connection,
    channel_id: Option<ChannelId>,
    offset: u32,
    limit: u32,
) -> Result<(Vec<Task>, bool)> {
    let mut tasks: Vec<Task> = list_docs(
        conn,
        "SELECT data FROM tasks WHERE archived_at IS NOT NULL AND (?1 IS NULL OR channel_id = ?1)
         ORDER BY archived_at DESC, rowid DESC LIMIT ?2 OFFSET ?3",
        params![
            channel_id.map(|c| c.to_string()),
            i64::from(limit) + 1,
            i64::from(offset)
        ],
    )?;
    let has_more = tasks.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    tasks.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    Ok((tasks, has_more))
}

/// Tasks in one status, across channels, oldest first. Archived tasks count.
pub fn tasks_with_status(conn: &Connection, status: TaskStatus) -> Result<Vec<Task>> {
    list_docs(
        conn,
        "SELECT data FROM tasks WHERE status = ?1 ORDER BY rowid",
        [status.as_str()],
    )
}

/// Scheduled tasks whose start time has passed.
pub fn due_scheduled_tasks(conn: &Connection, now: Timestamp) -> Result<Vec<Task>> {
    list_docs(
        conn,
        "SELECT data FROM tasks WHERE status = 'scheduled' AND start_at IS NOT NULL AND start_at <= ?1",
        [now.as_millis()],
    )
}

pub fn task_by_card(conn: &Connection, card_message_id: MessageId) -> Result<Option<Task>> {
    get_doc(
        conn,
        "SELECT data FROM tasks WHERE card_id = ?1",
        [card_message_id.to_string()],
    )
}

pub fn task_by_session(conn: &Connection, session_id: SessionId) -> Result<Option<Task>> {
    get_doc(
        conn,
        "SELECT data FROM tasks WHERE session_id = ?1",
        [session_id.to_string()],
    )
}

/// Whether the hub asked the workers to delete the workspace of a task, for
/// example when the user archived it after it was resolved.
pub fn task_workspace_released(conn: &Connection, task_id: TaskId) -> Result<bool> {
    let at: Option<Option<i64>> = conn
        .query_row(
            "SELECT workspace_released_at FROM tasks WHERE id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(at.flatten().is_some())
}

/// Whether a `workspace.release` for a task still waits in the queue of a
/// worker.
pub fn task_release_queued(conn: &Connection, task_id: TaskId) -> Result<bool> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM commands WHERE kind = 'workspace.release'
             AND json_extract(body, '$.task_id') = ?1 LIMIT 1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

impl Tx<'_> {
    /// Records that the workspace of a task was released, or clears the
    /// record. The task row in the feed does not change.
    pub fn set_task_workspace_released(&mut self, task_id: TaskId, released: bool) -> Result<()> {
        let at = released.then(|| self.now().as_millis());
        self.db().execute(
            "UPDATE tasks SET workspace_released_at = ?2 WHERE id = ?1",
            params![task_id.to_string(), at],
        )?;
        Ok(())
    }

    pub fn save_task(&mut self, task: &Task) -> Result<()> {
        self.db().execute(
            "INSERT INTO tasks (id, channel_id, status, card_id, session_id, start_at, archived_at, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, session_id = excluded.session_id,
                start_at = excluded.start_at, archived_at = excluded.archived_at, data = excluded.data",
            params![
                task.id.to_string(),
                task.channel_id.to_string(),
                task.status.as_str(),
                task.card_message_id.to_string(),
                task.current_session_id.map(|s| s.to_string()),
                task.start_at.map(|t| t.as_millis()),
                task.archived_at.map(|t| t.as_millis()),
                to_doc(task)?
            ],
        )?;
        self.index_search(
            "task",
            &task.id.to_string(),
            Some(task.channel_id),
            "",
            &task.title,
            &task.brief,
        )?;
        self.emit(EntityPayload::Task(task.clone()))
    }

    /// Deletes a task with its card, thread messages, sessions, items,
    /// approvals and decisions. The feed gets a delete for each.
    pub fn purge_task(&mut self, task: &Task) -> Result<()> {
        self.end_deleted_task_run(task.id)?;
        self.forget_task_status(task.id)?;
        let id = task.id.to_string();
        let channel = task.channel_id;

        let session_ids: Vec<String> = {
            let mut stmt = self
                .db()
                .prepare("SELECT id FROM sessions WHERE task_id = ?1")?;
            let rows = stmt.query_map([&id], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for sid in &session_ids {
            let item_ids: Vec<String> = {
                let mut stmt = self
                    .db()
                    .prepare("SELECT item_id FROM session_items WHERE session_id = ?1")?;
                let rows = stmt.query_map([sid], |row| row.get::<_, String>(0))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            for item_id in item_ids {
                self.emit_delete(
                    EntityKind::SessionItem,
                    format!("{sid}:{item_id}"),
                    Some(channel),
                )?;
            }
            self.db()
                .execute("DELETE FROM session_items WHERE session_id = ?1", [sid])?;
            self.db()
                .execute("DELETE FROM session_runtime WHERE session_id = ?1", [sid])?;
            self.db().execute(
                "DELETE FROM session_channel_files WHERE session_id = ?1",
                [sid],
            )?;
            self.db()
                .execute("DELETE FROM sessions WHERE id = ?1", [sid])?;
        }
        for (table, kind) in [
            ("approvals", EntityKind::Approval),
            ("decisions", EntityKind::Decision),
        ] {
            let ids: Vec<String> = {
                let mut stmt = self
                    .db()
                    .prepare(&format!("SELECT id FROM {table} WHERE task_id = ?1"))?;
                let rows = stmt.query_map([&id], |row| row.get::<_, String>(0))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            for waiting_id in ids {
                self.emit_delete(kind, waiting_id, Some(channel))?;
            }
            self.db()
                .execute(&format!("DELETE FROM {table} WHERE task_id = ?1"), [&id])?;
        }
        let thread: Vec<consort_proto::model::Message> =
            super::conversation::thread_replies(self.db(), task.card_message_id)?;
        for message in thread {
            self.purge_message(&message)?;
        }
        if let Some(card) = super::conversation::message(self.db(), task.card_message_id)? {
            self.purge_message(&card)?;
        }
        self.unindex_search("task", &id)?;
        for sql in [
            "DELETE FROM task_inbox WHERE task_id = ?1",
            "DELETE FROM task_attachments WHERE task_id = ?1",
            "DELETE FROM task_requests WHERE task_id = ?1",
            "DELETE FROM voice_tasks WHERE task_id = ?1",
            "DELETE FROM tasks WHERE id = ?1",
        ] {
            self.db().execute(sql, [&id])?;
        }
        self.emit_delete(EntityKind::Task, id, Some(channel))
    }
}

// Sessions ---------------------------------------------------------------

pub fn session(conn: &Connection, id: SessionId) -> Result<Option<Session>> {
    get_doc(
        conn,
        "SELECT data FROM sessions WHERE id = ?1",
        [id.to_string()],
    )
}

pub fn session_or_404(conn: &Connection, id: SessionId) -> Result<Session> {
    session(conn, id)?
        .ok_or_else(|| HubError::not_found(format!("The session {id} does not exist.")))
}

/// Sessions of one worker that are not ended or lost.
pub fn open_sessions_of_worker(conn: &Connection, worker_id: WorkerId) -> Result<Vec<Session>> {
    list_docs(
        conn,
        "SELECT data FROM sessions WHERE worker_id = ?1 AND state NOT IN ('ended', 'lost') ORDER BY rowid",
        [worker_id.to_string()],
    )
}

/// Open session counts by worker, for placement.
pub fn open_session_counts(conn: &Connection) -> Result<HashMap<WorkerId, u32>> {
    let mut stmt = conn.prepare_cached(
        "SELECT worker_id, count(*) FROM sessions WHERE state NOT IN ('ended', 'lost') GROUP BY worker_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (worker, count) = row?;
        if let Ok(id) = worker.parse() {
            out.insert(id, u32::try_from(count).unwrap_or(u32::MAX));
        }
    }
    Ok(out)
}

/// Task sessions that are not ended or lost and have an open turn, with
/// their runtime.
pub fn task_sessions_with_open_turn(conn: &Connection) -> Result<Vec<(Session, SessionRuntime)>> {
    let sessions: Vec<Session> = list_docs(
        conn,
        "SELECT s.data FROM sessions s JOIN session_runtime r ON r.session_id = s.id
         WHERE s.role = 'task' AND s.state NOT IN ('ended', 'lost') AND r.turn_open = 1
         ORDER BY s.rowid",
        [],
    )?;
    let mut out = Vec::with_capacity(sessions.len());
    for session in sessions {
        let runtime = session_runtime(conn, session.id)?;
        out.push((session, runtime));
    }
    Ok(out)
}

/// Sessions of a channel that are not ended or lost, oldest first.
pub fn open_sessions_of_channel(conn: &Connection, channel_id: ChannelId) -> Result<Vec<Session>> {
    list_docs(
        conn,
        "SELECT data FROM sessions WHERE channel_id = ?1 AND state NOT IN ('ended', 'lost') ORDER BY rowid",
        [channel_id.to_string()],
    )
}

/// Sessions of a task, oldest first.
pub fn sessions_of_task(conn: &Connection, task_id: TaskId) -> Result<Vec<Session>> {
    list_docs(
        conn,
        "SELECT data FROM sessions WHERE task_id = ?1 ORDER BY rowid",
        [task_id.to_string()],
    )
}

/// The hub-private state of a session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionRuntime {
    /// The turn that is running, or the last turn.
    pub turn_id: Option<TurnId>,
    pub turn_open: bool,
    /// The last assistant message of the turn, for the fallback post.
    pub last_assistant_text: Option<String>,
    /// Names of the hub tools that the session called in the current turn.
    pub tools_called: Vec<String>,
    /// The summary from `submit_for_review`.
    pub review_summary: Option<String>,
    /// A hash of the channel settings that the system prompt of this session
    /// was built from. A coordinator session is replaced when it changes.
    pub prompt_fingerprint: Option<String>,
    /// The last sign of life of the session: an event batch, or a turn that
    /// the hub opened. The stuck-turn check of task sessions reads it.
    pub active_at: Option<Timestamp>,
    /// True while the open turn is one that the hub opened when it moved the
    /// task from Ready for review back to Working. A refused turn then moves
    /// the task back to Ready for review.
    pub turn_from_review: bool,
}

pub fn session_runtime(conn: &Connection, id: SessionId) -> Result<SessionRuntime> {
    let row = conn
        .query_row(
            "SELECT turn_id, turn_open, last_assistant_text, tools_called, review_summary, prompt_fingerprint, active_at, turn_from_review
             FROM session_runtime WHERE session_id = ?1",
            [id.to_string()],
            |row| {
                Ok((
                    (
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ),
                    (
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, bool>(7)?,
                    ),
                ))
            },
        )
        .optional()?;
    let Some((
        (turn_id, turn_open, last_assistant_text, tools_called),
        (review_summary, prompt_fingerprint, active_at, turn_from_review),
    )) = row
    else {
        return Ok(SessionRuntime::default());
    };
    Ok(SessionRuntime {
        turn_id: turn_id.map(TurnId::new),
        turn_open,
        last_assistant_text,
        tools_called: serde_json::from_str(&tools_called)?,
        review_summary,
        prompt_fingerprint,
        active_at: active_at.map(Timestamp::from_millis),
        turn_from_review,
    })
}

impl Tx<'_> {
    /// Saves a session. Sessions are not in the feed.
    pub fn save_session(&mut self, session: &Session) -> Result<()> {
        self.db().execute(
            "INSERT INTO sessions (id, role, channel_id, task_id, worker_id, state, last_seq, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET state = excluded.state, last_seq = excluded.last_seq,
                data = excluded.data",
            params![
                session.id.to_string(),
                serde_json::to_value(session.role)?
                    .as_str()
                    .unwrap_or("task"),
                session.channel_id.to_string(),
                session.task_id.map(|t| t.to_string()),
                session.worker_id.to_string(),
                serde_json::to_value(session.state)?
                    .as_str()
                    .unwrap_or("starting"),
                i64::try_from(session.last_seq).unwrap_or(i64::MAX),
                to_doc(session)?
            ],
        )?;
        Ok(())
    }

    pub fn save_session_runtime(&mut self, id: SessionId, runtime: &SessionRuntime) -> Result<()> {
        self.db().execute(
            "INSERT INTO session_runtime (session_id, turn_id, turn_open, last_assistant_text, tools_called, review_summary, prompt_fingerprint, active_at, turn_from_review)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(session_id) DO UPDATE SET turn_id = excluded.turn_id,
                turn_open = excluded.turn_open, last_assistant_text = excluded.last_assistant_text,
                tools_called = excluded.tools_called, review_summary = excluded.review_summary,
                prompt_fingerprint = excluded.prompt_fingerprint, active_at = excluded.active_at,
                turn_from_review = excluded.turn_from_review",
            params![
                id.to_string(),
                runtime.turn_id.as_ref().map(|t| t.as_str().to_owned()),
                runtime.turn_open,
                runtime.last_assistant_text,
                serde_json::to_string(&runtime.tools_called)?,
                runtime.review_summary,
                runtime.prompt_fingerprint,
                runtime.active_at.map(|t| t.as_millis()),
                runtime.turn_from_review
            ],
        )?;
        Ok(())
    }
}

// Session items ----------------------------------------------------------

pub fn session_item(
    conn: &Connection,
    session_id: SessionId,
    item_id: ItemId,
) -> Result<Option<SessionItem>> {
    get_doc(
        conn,
        "SELECT data FROM session_items WHERE session_id = ?1 AND item_id = ?2",
        params![session_id.to_string(), item_id.to_string()],
    )
}

/// Items of a task's sessions, oldest first, after an item. Returns the page
/// and whether more items exist. With `session_id`, only that session.
pub fn task_items(
    conn: &Connection,
    task_id: TaskId,
    session_id: Option<SessionId>,
    after_item_id: Option<ItemId>,
    limit: usize,
) -> Result<(Vec<SessionItem>, bool)> {
    let limit = limit.clamp(1, 1000);
    let after_rowid: i64 = match after_item_id {
        Some(item) => conn
            .query_row(
                "SELECT si.rowid FROM session_items si JOIN sessions s ON s.id = si.session_id
                 WHERE s.task_id = ?1 AND si.item_id = ?2 AND (?3 IS NULL OR si.session_id = ?3)",
                params![
                    task_id.to_string(),
                    item.to_string(),
                    session_id.map(|s| s.to_string())
                ],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0),
        None => 0,
    };
    let mut items: Vec<SessionItem> = list_docs(
        conn,
        "SELECT si.data FROM session_items si JOIN sessions s ON s.id = si.session_id
         WHERE s.task_id = ?1 AND si.rowid > ?2 AND (?3 IS NULL OR si.session_id = ?3)
         ORDER BY si.rowid LIMIT ?4",
        params![
            task_id.to_string(),
            after_rowid,
            session_id.map(|s| s.to_string()),
            i64::try_from(limit + 1).unwrap_or(i64::MAX)
        ],
    )?;
    let has_more = items.len() > limit;
    items.truncate(limit);
    Ok((items, has_more))
}

/// Items of a task that are older than `before` (or the newest items when
/// `before` is `None`), newest first, at most `limit`.
pub fn task_items_before(
    conn: &Connection,
    task_id: TaskId,
    before: Option<ItemId>,
    limit: usize,
) -> Result<Vec<SessionItem>> {
    let before_rowid: i64 = match before {
        Some(item) => conn
            .query_row(
                "SELECT si.rowid FROM session_items si JOIN sessions s ON s.id = si.session_id
                 WHERE s.task_id = ?1 AND si.item_id = ?2",
                params![task_id.to_string(), item.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| HubError::not_found(format!("The task has no item {item}.")))?,
        None => i64::MAX,
    };
    list_docs(
        conn,
        "SELECT si.data FROM session_items si JOIN sessions s ON s.id = si.session_id
         WHERE s.task_id = ?1 AND si.rowid < ?2 ORDER BY si.rowid DESC LIMIT ?3",
        params![
            task_id.to_string(),
            before_rowid,
            i64::try_from(limit).unwrap_or(i64::MAX)
        ],
    )
}

impl Tx<'_> {
    /// Writes an item row (a new one, or a replacement with the same ID)
    /// without a feed entry. The caller must emit the item later with
    /// [`Tx::emit_item`]. The hub uses this to merge fast updates.
    pub fn save_item_row(&mut self, item: &SessionItem) -> Result<()> {
        self.db().execute(
            "INSERT INTO session_items (session_id, item_id, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id, item_id) DO UPDATE SET data = excluded.data",
            params![
                item.session_id.to_string(),
                item.id.to_string(),
                to_doc(item)?
            ],
        )?;
        Ok(())
    }

    /// Emits an item with the channel of its session.
    pub fn emit_item(&mut self, item: &SessionItem, channel_id: ChannelId) -> Result<()> {
        self.emit_in(EntityPayload::SessionItem(item.clone()), channel_id)
    }

    /// Saves an item and emits it with the channel of its session.
    pub fn save_item(&mut self, item: &SessionItem, channel_id: ChannelId) -> Result<()> {
        self.save_item_row(item)?;
        self.emit_item(item, channel_id)
    }
}

// Task inbox -------------------------------------------------------------

/// A message for a task session that waits for the current turn to end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxMessage {
    /// The row ID.
    pub id: i64,
    /// The text.
    pub text: String,
    /// The attachments of the message.
    pub attachments: Vec<BlobRef>,
}

/// Messages for a task session that wait for the current turn to end, oldest
/// first.
pub fn task_inbox(conn: &Connection, task_id: TaskId) -> Result<Vec<InboxMessage>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, text, attachments FROM task_inbox WHERE task_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([task_id.to_string()], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, text, attachments) = row?;
        out.push(InboxMessage {
            id,
            text,
            attachments: serde_json::from_str(&attachments)?,
        });
    }
    Ok(out)
}

/// The attachments that a coordinator gave a task with `start_task`. Each
/// session of the task gets them with its first turn.
pub fn task_attachments(conn: &Connection, task_id: TaskId) -> Result<Vec<BlobRef>> {
    let text: Option<String> = conn
        .query_row(
            "SELECT attachments FROM task_attachments WHERE task_id = ?1",
            [task_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    match text {
        Some(text) => Ok(serde_json::from_str(&text)?),
        None => Ok(Vec::new()),
    }
}

impl Tx<'_> {
    pub fn push_task_inbox(
        &mut self,
        task_id: TaskId,
        text: &str,
        attachments: &[BlobRef],
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO task_inbox (task_id, text, attachments, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                task_id.to_string(),
                text,
                serde_json::to_string(attachments)?,
                self.now().as_millis()
            ],
        )?;
        Ok(())
    }

    /// Keeps the attachments of a new task for the first turn of its
    /// sessions.
    pub fn save_task_attachments(
        &mut self,
        task_id: TaskId,
        attachments: &[BlobRef],
    ) -> Result<()> {
        if attachments.is_empty() {
            return Ok(());
        }
        self.db().execute(
            "INSERT INTO task_attachments (task_id, attachments) VALUES (?1, ?2)
             ON CONFLICT(task_id) DO UPDATE SET attachments = excluded.attachments",
            params![task_id.to_string(), serde_json::to_string(attachments)?],
        )?;
        Ok(())
    }

    /// Deletes the inbox rows that went into a turn.
    pub fn delete_task_inbox(&mut self, ids: &[i64]) -> Result<()> {
        for id in ids {
            self.db()
                .execute("DELETE FROM task_inbox WHERE id = ?1", [id])?;
        }
        Ok(())
    }

    pub fn clear_task_inbox(&mut self, task_id: TaskId) -> Result<()> {
        self.db().execute(
            "DELETE FROM task_inbox WHERE task_id = ?1",
            [task_id.to_string()],
        )?;
        Ok(())
    }
}

// Approvals and decisions ------------------------------------------------

pub fn approval(conn: &Connection, id: consort_proto::ids::ApprovalId) -> Result<Option<Approval>> {
    get_doc(
        conn,
        "SELECT data FROM approvals WHERE id = ?1",
        [id.to_string()],
    )
}

pub fn approval_by_request(
    conn: &Connection,
    session_id: SessionId,
    request_id: &RequestId,
) -> Result<Option<Approval>> {
    get_doc(
        conn,
        "SELECT data FROM approvals WHERE session_id = ?1 AND request_id = ?2 ORDER BY rowid DESC LIMIT 1",
        params![session_id.to_string(), request_id.as_str()],
    )
}

/// Pending approvals, in one channel or in all.
pub fn pending_approvals(
    conn: &Connection,
    channel_id: Option<ChannelId>,
) -> Result<Vec<Approval>> {
    list_docs(
        conn,
        "SELECT data FROM approvals WHERE status = 'pending' AND (?1 IS NULL OR channel_id = ?1) ORDER BY rowid",
        [channel_id.map(|c| c.to_string())],
    )
}

pub fn decision(conn: &Connection, id: consort_proto::ids::DecisionId) -> Result<Option<Decision>> {
    get_doc(
        conn,
        "SELECT data FROM decisions WHERE id = ?1",
        [id.to_string()],
    )
}

pub fn decision_by_request(
    conn: &Connection,
    session_id: SessionId,
    request_id: &RequestId,
) -> Result<Option<Decision>> {
    get_doc(
        conn,
        "SELECT data FROM decisions WHERE session_id = ?1 AND request_id = ?2 ORDER BY rowid DESC LIMIT 1",
        params![session_id.to_string(), request_id.as_str()],
    )
}

pub fn pending_decisions(
    conn: &Connection,
    channel_id: Option<ChannelId>,
) -> Result<Vec<Decision>> {
    list_docs(
        conn,
        "SELECT data FROM decisions WHERE status = 'pending' AND (?1 IS NULL OR channel_id = ?1) ORDER BY rowid",
        [channel_id.map(|c| c.to_string())],
    )
}

/// How many approvals and decisions wait on the user for one task.
pub fn pending_count_for_task(conn: &Connection, task_id: TaskId) -> Result<u32> {
    let count: i64 = conn.query_row(
        "SELECT (SELECT count(*) FROM approvals WHERE task_id = ?1 AND status = 'pending')
              + (SELECT count(*) FROM decisions WHERE task_id = ?1 AND status = 'pending')",
        [task_id.to_string()],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// Pending approvals and decisions of one session.
pub fn pending_for_session(
    conn: &Connection,
    session_id: SessionId,
) -> Result<(Vec<Approval>, Vec<Decision>)> {
    let key = session_id.to_string();
    Ok((
        list_docs(
            conn,
            "SELECT data FROM approvals WHERE session_id = ?1 AND status = 'pending'",
            [&key],
        )?,
        list_docs(
            conn,
            "SELECT data FROM decisions WHERE session_id = ?1 AND status = 'pending'",
            [&key],
        )?,
    ))
}

/// Pending decisions of one task, from any of its sessions.
pub fn pending_decisions_of_task(conn: &Connection, task_id: TaskId) -> Result<Vec<Decision>> {
    list_docs(
        conn,
        "SELECT data FROM decisions WHERE task_id = ?1 AND status = 'pending' ORDER BY rowid",
        [task_id.to_string()],
    )
}

impl Tx<'_> {
    pub fn save_approval(&mut self, approval: &Approval) -> Result<()> {
        self.db().execute(
            "INSERT INTO approvals (id, channel_id, session_id, task_id, request_id, status, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, data = excluded.data",
            params![
                approval.id.to_string(),
                approval.channel_id.to_string(),
                approval.session_id.to_string(),
                approval.task_id.map(|t| t.to_string()),
                approval.request_id.as_str(),
                serde_json::to_value(approval.status)?
                    .as_str()
                    .unwrap_or("pending"),
                to_doc(approval)?
            ],
        )?;
        self.emit(EntityPayload::Approval(approval.clone()))
    }

    pub fn save_decision(&mut self, decision: &Decision) -> Result<()> {
        self.db().execute(
            "INSERT INTO decisions (id, channel_id, session_id, task_id, request_id, status, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, data = excluded.data",
            params![
                decision.id.to_string(),
                decision.channel_id.to_string(),
                decision.session_id.to_string(),
                decision.task_id.map(|t| t.to_string()),
                decision.request_id.as_ref().map(|r| r.as_str().to_owned()),
                serde_json::to_value(decision.status)?
                    .as_str()
                    .unwrap_or("pending"),
                to_doc(decision)?
            ],
        )?;
        self.emit(EntityPayload::Decision(decision.clone()))
    }
}
