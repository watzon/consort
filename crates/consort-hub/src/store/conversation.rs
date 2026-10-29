//! Groups, channels, repos, messages and read markers.

use consort_proto::client_api::{EntityKind, EntityPayload};
use consort_proto::ids::{
    ChannelId, ClientId, GroupId, MessageId, RepoId, RoutineId, SessionId, TaskId,
};
use consort_proto::model::{
    Author, BlobRef, Channel, ChannelKind, Group, Message, MessageKind, ReadMarker, Repo,
};
use rusqlite::{Connection, OptionalExtension, params};

use super::{Tx, get_doc, list_docs, to_doc};
use crate::error::{HubError, Result};

// Groups -----------------------------------------------------------------

pub fn group(conn: &Connection, id: GroupId) -> Result<Option<Group>> {
    get_doc(
        conn,
        "SELECT data FROM groups WHERE id = ?1",
        [id.to_string()],
    )
}

/// All groups in sidebar order.
pub fn groups(conn: &Connection) -> Result<Vec<Group>> {
    list_docs(conn, "SELECT data FROM groups ORDER BY position, rowid", [])
}

// Channels ---------------------------------------------------------------

pub fn channel(conn: &Connection, id: ChannelId) -> Result<Option<Channel>> {
    get_doc(
        conn,
        "SELECT data FROM channels WHERE id = ?1",
        [id.to_string()],
    )
}

/// Like [`channel`], with a `not_found` error.
pub fn channel_or_404(conn: &Connection, id: ChannelId) -> Result<Channel> {
    channel(conn, id)?
        .ok_or_else(|| HubError::not_found(format!("The channel {id} does not exist.")))
}

/// Every channel, archived ones included, in sidebar order.
pub fn channels(conn: &Connection) -> Result<Vec<Channel>> {
    list_docs(
        conn,
        "SELECT data FROM channels ORDER BY position, rowid",
        [],
    )
}

pub fn everything_channel(conn: &Connection) -> Result<Channel> {
    get_doc(
        conn,
        "SELECT data FROM channels WHERE kind = 'everything'",
        [],
    )?
    .ok_or_else(|| HubError::internal("the Everything channel is missing"))
}

// Repos ------------------------------------------------------------------

pub fn repo(conn: &Connection, id: RepoId) -> Result<Option<Repo>> {
    get_doc(
        conn,
        "SELECT data FROM repos WHERE id = ?1",
        [id.to_string()],
    )
}

pub fn repos(conn: &Connection) -> Result<Vec<Repo>> {
    list_docs(conn, "SELECT data FROM repos ORDER BY rowid", [])
}

pub fn repo_by_key(conn: &Connection, url_key: &str) -> Result<Option<Repo>> {
    get_doc(conn, "SELECT data FROM repos WHERE url_key = ?1", [url_key])
}

/// A key that is the same for the SSH and HTTPS spellings of one remote:
/// `host/owner/name`, lowercase, without `.git`.
pub fn repo_url_key(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
    rest.replacen(':', "/", 1).to_lowercase()
}

// Messages ---------------------------------------------------------------

/// The input of [`Tx::insert_message`].
#[derive(Debug, Clone)]
pub struct NewMessage {
    pub channel_id: ChannelId,
    pub thread_root_id: Option<MessageId>,
    pub in_timeline: bool,
    pub author: Author,
    pub author_session_id: Option<SessionId>,
    pub author_task_id: Option<TaskId>,
    pub author_routine_id: Option<RoutineId>,
    pub kind: MessageKind,
    pub body: String,
    pub attachments: Vec<BlobRef>,
    /// The client that sent the message and its idempotency key.
    pub origin: Option<(ClientId, String)>,
}

impl NewMessage {
    pub fn new(
        channel_id: ChannelId,
        author: Author,
        kind: MessageKind,
        body: impl Into<String>,
    ) -> Self {
        Self {
            channel_id,
            thread_root_id: None,
            in_timeline: false,
            author,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            kind,
            body: body.into(),
            attachments: Vec::new(),
            origin: None,
        }
    }
}

pub fn message(conn: &Connection, id: MessageId) -> Result<Option<Message>> {
    get_doc(
        conn,
        "SELECT data FROM messages WHERE id = ?1",
        [id.to_string()],
    )
}

pub fn message_or_404(conn: &Connection, id: MessageId) -> Result<Message> {
    message(conn, id)?
        .ok_or_else(|| HubError::not_found(format!("The message {id} does not exist.")))
}

/// The message that a client already sent with this idempotency key.
pub fn message_by_client_id(
    conn: &Connection,
    client_id: ClientId,
    client_message_id: &str,
) -> Result<Option<Message>> {
    get_doc(
        conn,
        "SELECT data FROM messages WHERE origin_client_id = ?1 AND client_message_id = ?2",
        params![client_id.to_string(), client_message_id],
    )
}

/// A page of messages, newest first. `thread_root_id: None` is the timeline:
/// root messages and thread replies that were also posted in the timeline.
/// Returns the page and whether older messages exist. Deleted messages are
/// left out.
pub fn messages_page(
    conn: &Connection,
    channel_id: ChannelId,
    thread_root_id: Option<MessageId>,
    before: Option<MessageId>,
    limit: usize,
) -> Result<(Vec<Message>, bool)> {
    let limit = limit.clamp(1, 500);
    let before_rowid: i64 = match before {
        Some(id) => conn
            .query_row(
                "SELECT rowid FROM messages WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| HubError::not_found(format!("The message {id} does not exist.")))?,
        None => i64::MAX,
    };
    let mut messages: Vec<Message> = match thread_root_id {
        None => list_docs(
            conn,
            "SELECT data FROM messages
             WHERE channel_id = ?1 AND rowid < ?2 AND deleted_at IS NULL
               AND (thread_root_id IS NULL OR in_timeline = 1)
             ORDER BY rowid DESC LIMIT ?3",
            params![
                channel_id.to_string(),
                before_rowid,
                i64::try_from(limit + 1).unwrap_or(i64::MAX)
            ],
        )?,
        Some(root) => list_docs(
            conn,
            "SELECT data FROM messages
             WHERE channel_id = ?1 AND rowid < ?2 AND deleted_at IS NULL AND thread_root_id = ?3
             ORDER BY rowid DESC LIMIT ?4",
            params![
                channel_id.to_string(),
                before_rowid,
                root.to_string(),
                i64::try_from(limit + 1).unwrap_or(i64::MAX)
            ],
        )?,
    };
    let has_more = messages.len() > limit;
    messages.truncate(limit);
    Ok((messages, has_more))
}

/// The replies in a thread, oldest first.
pub fn thread_replies(conn: &Connection, root: MessageId) -> Result<Vec<Message>> {
    list_docs(
        conn,
        "SELECT data FROM messages WHERE thread_root_id = ?1 AND deleted_at IS NULL ORDER BY rowid",
        [root.to_string()],
    )
}

impl Tx<'_> {
    pub fn save_group(&mut self, group: &Group) -> Result<()> {
        self.db().execute(
            "INSERT INTO groups (id, position, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET position = excluded.position, data = excluded.data",
            params![group.id.to_string(), group.position, to_doc(group)?],
        )?;
        self.emit(EntityPayload::Group(group.clone()))
    }

    pub fn delete_group(&mut self, id: GroupId) -> Result<()> {
        self.db()
            .execute("DELETE FROM groups WHERE id = ?1", [id.to_string()])?;
        self.emit_delete(EntityKind::Group, id.to_string(), None)
    }

    pub fn save_channel(&mut self, channel: &Channel) -> Result<()> {
        let kind = match channel.kind {
            ChannelKind::Everything => "everything",
            ChannelKind::Standard => "standard",
        };
        self.db().execute(
            "INSERT INTO channels (id, kind, group_id, position, archived_at, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET group_id = excluded.group_id,
                position = excluded.position, archived_at = excluded.archived_at,
                data = excluded.data",
            params![
                channel.id.to_string(),
                kind,
                channel.group_id.map(|g| g.to_string()),
                channel.position,
                channel.archived_at.map(|t| t.as_millis()),
                to_doc(channel)?
            ],
        )?;
        self.emit(EntityPayload::Channel(channel.clone()))
    }

    /// Deletes a channel and everything that belongs to it. The feed gets one
    /// delete for the channel. A client drops every entity of that channel
    /// when it sees the delete.
    pub fn purge_channel(&mut self, id: ChannelId) -> Result<()> {
        let key = id.to_string();
        let db = self.db();
        for sql in [
            "DELETE FROM search_idx WHERE channel_id = ?1",
            "DELETE FROM messages WHERE channel_id = ?1",
            "DELETE FROM read_markers WHERE channel_id = ?1",
            "DELETE FROM session_items WHERE session_id IN (SELECT id FROM sessions WHERE channel_id = ?1)",
            "DELETE FROM session_runtime WHERE session_id IN (SELECT id FROM sessions WHERE channel_id = ?1)",
            "DELETE FROM sessions WHERE channel_id = ?1",
            "DELETE FROM tasks WHERE channel_id = ?1",
            "DELETE FROM approvals WHERE channel_id = ?1",
            "DELETE FROM decisions WHERE channel_id = ?1",
            "DELETE FROM routine_runs WHERE routine_id IN (SELECT id FROM routines WHERE channel_id = ?1)",
            "DELETE FROM routines WHERE channel_id = ?1",
            "DELETE FROM channel_files WHERE channel_id = ?1",
            "DELETE FROM wake_events WHERE channel_id = ?1",
            "DELETE FROM coordinators WHERE channel_id = ?1",
            "DELETE FROM memory_notes WHERE scope = ?1",
            "DELETE FROM memory_note_versions WHERE scope = ?1",
            "DELETE FROM channels WHERE id = ?1",
        ] {
            db.execute(sql, [&key])?;
        }
        self.emit_delete(EntityKind::Channel, key, Some(id))
    }

    pub fn save_repo(&mut self, repo: &Repo) -> Result<()> {
        self.db().execute(
            "INSERT INTO repos (id, url_key, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET url_key = excluded.url_key, data = excluded.data",
            params![repo.id.to_string(), repo_url_key(&repo.url), to_doc(repo)?],
        )?;
        self.emit(EntityPayload::Repo(repo.clone()))
    }

    /// Adds a message. For a thread reply it also updates the root's reply
    /// count and last reply time. Indexes text messages for search.
    pub fn insert_message(&mut self, new: NewMessage) -> Result<Message> {
        let now = self.now();
        let mut thread_root_id = new.thread_root_id;
        if let Some(root_id) = thread_root_id {
            let root = message(self.db(), root_id)?.ok_or_else(|| {
                HubError::not_found(format!("The message {root_id} does not exist."))
            })?;
            if root.channel_id != new.channel_id {
                return Err(HubError::invalid("The thread root is in another channel."));
            }
            // A reply to a reply joins the thread of the first message.
            thread_root_id = Some(root.thread_root_id.unwrap_or(root.id));
        }
        let message = Message {
            id: MessageId::new(),
            channel_id: new.channel_id,
            thread_root_id,
            author: new.author,
            author_session_id: new.author_session_id,
            author_task_id: new.author_task_id,
            author_routine_id: new.author_routine_id,
            kind: new.kind,
            body: new.body,
            attachments: new.attachments,
            in_timeline: thread_root_id.is_some() && new.in_timeline,
            client_message_id: new.origin.as_ref().map(|(_, id)| id.clone()),
            reply_count: 0,
            last_reply_at: None,
            reactions: Vec::new(),
            edited_at: None,
            deleted_at: None,
            created_at: now,
        };
        self.db().execute(
            "INSERT INTO messages (id, channel_id, thread_root_id, in_timeline, created_at,
                origin_client_id, client_message_id, deleted_at, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8)",
            params![
                message.id.to_string(),
                message.channel_id.to_string(),
                message.thread_root_id.map(|m| m.to_string()),
                message.in_timeline,
                now.as_millis(),
                new.origin.as_ref().map(|(c, _)| c.to_string()),
                message.client_message_id,
                to_doc(&message)?
            ],
        )?;
        if message.kind == MessageKind::Text {
            self.index_search(
                "message",
                &message.id.to_string(),
                Some(message.channel_id),
                "",
                "",
                &message.body,
            )?;
        }
        self.emit(EntityPayload::Message(message.clone()))?;
        if let Some(root_id) = message.thread_root_id
            && let Some(mut root) = super::conversation::message(self.db(), root_id)?
        {
            root.reply_count = root.reply_count.saturating_add(1);
            root.last_reply_at = Some(now);
            self.save_message(&root)?;
        }
        Ok(message)
    }

    /// Writes a changed message (edit, reactions, delete) and emits it.
    /// Keeps the search index in step.
    pub fn save_message(&mut self, message: &Message) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET deleted_at = ?2, data = ?3 WHERE id = ?1",
            params![
                message.id.to_string(),
                message.deleted_at.map(|t| t.as_millis()),
                to_doc(message)?
            ],
        )?;
        if message.kind == MessageKind::Text {
            let key = message.id.to_string();
            self.unindex_search("message", &key)?;
            if message.deleted_at.is_none() {
                self.index_search(
                    "message",
                    &key,
                    Some(message.channel_id),
                    "",
                    "",
                    &message.body,
                )?;
            }
        }
        self.emit(EntityPayload::Message(message.clone()))
    }

    /// Deletes a message row. The feed gets a delete.
    pub fn purge_message(&mut self, message: &Message) -> Result<()> {
        let key = message.id.to_string();
        self.db()
            .execute("DELETE FROM messages WHERE id = ?1", [&key])?;
        self.unindex_search("message", &key)?;
        self.emit_delete(EntityKind::Message, key, Some(message.channel_id))
    }
}

// Read markers -----------------------------------------------------------

pub fn read_marker(
    conn: &Connection,
    channel_id: ChannelId,
    thread_root_id: Option<MessageId>,
) -> Result<Option<ReadMarker>> {
    get_doc(
        conn,
        "SELECT data FROM read_markers WHERE channel_id = ?1 AND thread_root_id = ?2",
        params![
            channel_id.to_string(),
            thread_root_id.map(|m| m.to_string()).unwrap_or_default()
        ],
    )
}

pub fn read_markers(conn: &Connection) -> Result<Vec<ReadMarker>> {
    list_docs(conn, "SELECT data FROM read_markers ORDER BY rowid", [])
}

impl Tx<'_> {
    pub fn save_read_marker(&mut self, marker: &ReadMarker) -> Result<()> {
        self.db().execute(
            "INSERT INTO read_markers (channel_id, thread_root_id, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(channel_id, thread_root_id) DO UPDATE SET data = excluded.data",
            params![
                marker.channel_id.to_string(),
                marker
                    .thread_root_id
                    .map(|m| m.to_string())
                    .unwrap_or_default(),
                to_doc(marker)?
            ],
        )?;
        self.emit(EntityPayload::ReadMarker(marker.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_keys_match_across_spellings() {
        let a = repo_url_key("git@github.com:watzon/sayso.git");
        let b = repo_url_key("https://github.com/watzon/sayso");
        let c = repo_url_key("ssh://git@GitHub.com/watzon/Sayso.git/");
        assert_eq!(a, "github.com/watzon/sayso");
        assert_eq!(a, b);
        assert_eq!(a, c);
    }
}
