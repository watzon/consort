//! Memory notes, channel files and the full-text search index.

use consort_proto::client_api::{EntityKind, EntityPayload};
use consort_proto::error::ErrorCode;
use consort_proto::ids::{ChannelId, SessionId, TaskId};
use consort_proto::model::{
    ARTIFACTS_DIR, ChannelFile, ExpectedHash, FileRef, MemoryAuthor, MemoryNote, MemoryNoteSummary,
    MemoryScope, Sha256Hash, memory_note_entity_id,
};
use consort_proto::time::Timestamp;
use consort_proto::tools::SearchHit;
use consort_proto::turn_input::SuggestionSource;
use rusqlite::{Connection, OptionalExtension, params};

use super::{Tx, get_doc, list_docs, to_doc};
use crate::error::{HubError, Result};
use crate::util::sha256;

// Memory -----------------------------------------------------------------

fn author_to_text(author: MemoryAuthor) -> String {
    match author {
        MemoryAuthor::User => "user".to_owned(),
        MemoryAuthor::Session(id) => id.to_string(),
    }
}

fn author_from_text(text: &str) -> MemoryAuthor {
    if text == "user" {
        return MemoryAuthor::User;
    }
    text.parse::<SessionId>()
        .map_or(MemoryAuthor::User, MemoryAuthor::Session)
}

fn scope_from_text(text: &str) -> Result<MemoryScope> {
    text.parse()
        .map_err(|e: consort_proto::model::ScopeParseError| HubError::internal(e.to_string()))
}

type NoteRow = (
    String,
    String,
    String,
    String,
    i64,
    String,
    i64,
    Option<String>,
    Option<i64>,
);

fn note_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

fn note_from_parts(parts: NoteRow) -> Result<MemoryNote> {
    let (scope, path, content, hash, version, updated_by, updated_at, suggested_by, deleted_at) =
        parts;
    Ok(MemoryNote {
        scope: scope_from_text(&scope)?,
        path,
        content,
        sha256: hash
            .parse()
            .map_err(|_| HubError::internal("a stored memory hash is invalid"))?,
        version: u64::try_from(version).unwrap_or(0),
        updated_by: author_from_text(&updated_by),
        updated_at: Timestamp::from_millis(updated_at),
        suggested_by: suggested_by.and_then(|s| s.parse::<SuggestionSource>().ok()),
        deleted_at: deleted_at.map(Timestamp::from_millis),
    })
}

/// The columns of a note row, in the order of [`NoteRow`].
const NOTE_COLUMNS: &str =
    "scope, path, content, sha256, version, updated_by, updated_at, suggested_by, deleted_at";
/// The same for a version row, which has no `deleted_at`.
const VERSION_COLUMNS: &str =
    "scope, path, content, sha256, version, updated_by, updated_at, suggested_by, NULL";

fn notes_where(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<MemoryNote>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(params, note_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(note_from_parts(row?)?);
    }
    Ok(out)
}

/// The stored row of a note, also when the note is deleted.
fn note_row(conn: &Connection, scope: MemoryScope, path: &str) -> Result<Option<MemoryNote>> {
    let row = conn
        .query_row(
            &format!("SELECT {NOTE_COLUMNS} FROM memory_notes WHERE scope = ?1 AND path = ?2"),
            params![scope.to_string(), path],
            note_from_row,
        )
        .optional()?;
    row.map(note_from_parts).transpose()
}

/// A note that is not deleted.
pub fn memory_note(
    conn: &Connection,
    scope: MemoryScope,
    path: &str,
) -> Result<Option<MemoryNote>> {
    Ok(note_row(conn, scope, path)?.filter(|note| note.deleted_at.is_none()))
}

/// A note in Recently deleted.
pub fn deleted_memory_note(
    conn: &Connection,
    scope: MemoryScope,
    path: &str,
) -> Result<Option<MemoryNote>> {
    Ok(note_row(conn, scope, path)?.filter(|note| note.deleted_at.is_some()))
}

/// Every note of one scope that is not deleted, ordered by path.
pub fn memory_notes(conn: &Connection, scope: MemoryScope) -> Result<Vec<MemoryNote>> {
    notes_where(
        conn,
        &format!(
            "SELECT {NOTE_COLUMNS} FROM memory_notes WHERE scope = ?1 AND deleted_at IS NULL
             ORDER BY path"
        ),
        [scope.to_string()],
    )
}

/// Summaries of the notes that are not deleted, of one scope or of all
/// scopes, ordered by scope and path.
pub fn memory_list(
    conn: &Connection,
    scope: Option<MemoryScope>,
) -> Result<Vec<MemoryNoteSummary>> {
    Ok(notes_where(
        conn,
        &format!(
            "SELECT {NOTE_COLUMNS} FROM memory_notes
             WHERE (?1 IS NULL OR scope = ?1) AND deleted_at IS NULL ORDER BY scope, path"
        ),
        [scope.map(|s| s.to_string())],
    )?
    .iter()
    .map(MemoryNote::summary)
    .collect())
}

/// Summaries of the notes in Recently deleted, of one scope or of all
/// scopes, the newest deletion first.
pub fn deleted_memory_list(
    conn: &Connection,
    scope: Option<MemoryScope>,
) -> Result<Vec<MemoryNoteSummary>> {
    Ok(notes_where(
        conn,
        &format!(
            "SELECT {NOTE_COLUMNS} FROM memory_notes
             WHERE (?1 IS NULL OR scope = ?1) AND deleted_at IS NOT NULL
             ORDER BY deleted_at DESC, scope, path"
        ),
        [scope.map(|s| s.to_string())],
    )?
    .iter()
    .map(MemoryNote::summary)
    .collect())
}

/// Every stored version of a note, oldest first. A deleted note keeps its
/// versions until the purge.
pub fn memory_history(
    conn: &Connection,
    scope: MemoryScope,
    path: &str,
) -> Result<Vec<MemoryNote>> {
    notes_where(
        conn,
        &format!(
            "SELECT {VERSION_COLUMNS} FROM memory_note_versions
             WHERE scope = ?1 AND path = ?2 ORDER BY version"
        ),
        params![scope.to_string(), path],
    )
}

fn stale_write() -> HubError {
    HubError::new(
        ErrorCode::StaleWrite,
        "The note changed since you read it. Read it again, then retry the write.",
    )
}

fn scope_channel(scope: MemoryScope) -> Option<ChannelId> {
    match scope {
        MemoryScope::Global => None,
        MemoryScope::Channel(id) => Some(id),
    }
}

impl Tx<'_> {
    /// Writes a note if `expected` matches the stored hash. Returns the stored
    /// note. A write with the same content as the stored note changes nothing.
    /// A deleted note counts as no note: a write with `not_exists` brings
    /// the path back with the next version number.
    pub fn write_memory(
        &mut self,
        scope: MemoryScope,
        path: &str,
        content: &str,
        expected: &ExpectedHash,
        by: MemoryAuthor,
        suggested_by: Option<SuggestionSource>,
    ) -> Result<MemoryNote> {
        let current = memory_note(self.db(), scope, path)?;
        if !expected.matches(current.as_ref().map(|n| &n.sha256)) {
            return Err(stale_write());
        }
        let hash = sha256(content.as_bytes());
        if let Some(current) = current.as_ref()
            && current.sha256 == hash
        {
            return Ok(current.clone());
        }
        let previous: Option<i64> = self.db().query_row(
            "SELECT MAX(version) FROM memory_note_versions WHERE scope = ?1 AND path = ?2",
            params![scope.to_string(), path],
            |row| row.get(0),
        )?;
        let version = u64::try_from(previous.unwrap_or(0)).unwrap_or(0) + 1;
        let note = MemoryNote {
            scope,
            path: path.to_owned(),
            content: content.to_owned(),
            sha256: hash,
            version,
            updated_by: by,
            updated_at: self.now(),
            suggested_by,
            deleted_at: None,
        };
        let values = params![
            note.scope.to_string(),
            note.path,
            note.content,
            note.sha256.as_str(),
            i64::try_from(note.version).unwrap_or(i64::MAX),
            author_to_text(note.updated_by),
            note.updated_at.as_millis(),
            note.suggested_by.map(|s| s.to_string()),
        ];
        self.db().execute(
            "INSERT INTO memory_notes (scope, path, content, sha256, version, updated_by,
                updated_at, suggested_by, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)
             ON CONFLICT(scope, path) DO UPDATE SET content = excluded.content,
                sha256 = excluded.sha256, version = excluded.version,
                updated_by = excluded.updated_by, updated_at = excluded.updated_at,
                suggested_by = excluded.suggested_by, deleted_at = NULL",
            values,
        )?;
        self.db().execute(
            "INSERT INTO memory_note_versions (scope, path, content, sha256, version, updated_by,
                updated_at, suggested_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            values,
        )?;
        self.index_memory(&note)?;
        self.emit(EntityPayload::MemoryNote(note.clone()))?;
        Ok(note)
    }

    /// Puts a note in the search index in place of its old text.
    fn index_memory(&mut self, note: &MemoryNote) -> Result<()> {
        let entity_id = note.entity_id();
        self.unindex_search("memory_note", &entity_id)?;
        self.index_search(
            "memory_note",
            &entity_id,
            scope_channel(note.scope),
            &note.scope.to_string(),
            &note.path,
            &note.content,
        )
    }

    /// Moves a note to Recently deleted if `expected` matches the stored
    /// hash. The note and its versions stay until the purge. The feed gets
    /// the note with its `deleted_at`.
    pub fn delete_memory(
        &mut self,
        scope: MemoryScope,
        path: &str,
        expected: &Sha256Hash,
    ) -> Result<()> {
        let mut note = memory_note(self.db(), scope, path)?
            .ok_or_else(|| HubError::not_found(format!("There is no memory note at {path}.")))?;
        if &note.sha256 != expected {
            return Err(stale_write());
        }
        note.deleted_at = Some(self.now());
        self.db().execute(
            "UPDATE memory_notes SET deleted_at = ?3 WHERE scope = ?1 AND path = ?2",
            params![
                scope.to_string(),
                path,
                note.deleted_at.map(|t| t.as_millis())
            ],
        )?;
        self.unindex_search("memory_note", &note.entity_id())?;
        self.emit(EntityPayload::MemoryNote(note))
    }

    /// Brings a note back from Recently deleted, as it was.
    pub fn restore_memory(&mut self, scope: MemoryScope, path: &str) -> Result<MemoryNote> {
        let mut note = deleted_memory_note(self.db(), scope, path)?.ok_or_else(|| {
            HubError::not_found(format!(
                "The note {path} is not in Recently deleted. It may be back already, or more than 30 days old."
            ))
        })?;
        note.deleted_at = None;
        self.db().execute(
            "UPDATE memory_notes SET deleted_at = NULL WHERE scope = ?1 AND path = ?2",
            params![scope.to_string(), path],
        )?;
        self.index_memory(&note)?;
        self.emit(EntityPayload::MemoryNote(note.clone()))?;
        Ok(note)
    }

    /// Renames a note in its scope, with its versions. A deleted note at the
    /// new path is removed for good first.
    pub fn move_memory(
        &mut self,
        scope: MemoryScope,
        from: &str,
        to: &str,
        expected: &Sha256Hash,
    ) -> Result<MemoryNote> {
        let mut note = memory_note(self.db(), scope, from)?
            .ok_or_else(|| HubError::not_found(format!("There is no memory note at {from}.")))?;
        if &note.sha256 != expected {
            return Err(stale_write());
        }
        if from == to {
            return Ok(note);
        }
        if memory_note(self.db(), scope, to)?.is_some() {
            return Err(HubError::conflict(format!(
                "A note at {to} exists already. Choose another name."
            )));
        }
        if deleted_memory_note(self.db(), scope, to)?.is_some() {
            self.purge_memory_note(scope, to)?;
        }
        // Versions of a note that an older hub deleted for good can still
        // hold the new path.
        self.db().execute(
            "DELETE FROM memory_note_versions WHERE scope = ?1 AND path = ?2",
            params![scope.to_string(), to],
        )?;
        let old_entity = note.entity_id();
        for table in ["memory_notes", "memory_note_versions"] {
            self.db().execute(
                &format!("UPDATE {table} SET path = ?3 WHERE scope = ?1 AND path = ?2"),
                params![scope.to_string(), from, to],
            )?;
        }
        self.unindex_search("memory_note", &old_entity)?;
        self.emit_delete(EntityKind::MemoryNote, old_entity, scope_channel(scope))?;
        note.path = to.to_owned();
        self.index_memory(&note)?;
        self.emit(EntityPayload::MemoryNote(note.clone()))?;
        Ok(note)
    }

    /// Removes a note and its versions for good.
    fn purge_memory_note(&mut self, scope: MemoryScope, path: &str) -> Result<()> {
        for table in ["memory_notes", "memory_note_versions"] {
            self.db().execute(
                &format!("DELETE FROM {table} WHERE scope = ?1 AND path = ?2"),
                params![scope.to_string(), path],
            )?;
        }
        let entity_id = memory_note_entity_id(scope, path);
        self.unindex_search("memory_note", &entity_id)?;
        self.emit_delete(EntityKind::MemoryNote, entity_id, scope_channel(scope))
    }

    /// Removes the notes that were deleted before `before`, with their
    /// versions. Returns how many it removed.
    pub fn purge_deleted_memory(&mut self, before: Timestamp) -> Result<usize> {
        let old: Vec<(String, String)> = {
            let mut stmt = self.db().prepare(
                "SELECT scope, path FROM memory_notes WHERE deleted_at IS NOT NULL AND deleted_at < ?1",
            )?;
            let rows =
                stmt.query_map([before.as_millis()], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for (scope, path) in &old {
            self.purge_memory_note(scope_from_text(scope)?, path)?;
        }
        Ok(old.len())
    }
}

// Channel files ----------------------------------------------------------

pub fn channel_file(
    conn: &Connection,
    channel_id: ChannelId,
    path: &str,
) -> Result<Option<ChannelFile>> {
    get_doc(
        conn,
        "SELECT data FROM channel_files WHERE channel_id = ?1 AND path = ?2",
        params![channel_id.to_string(), path],
    )
}

/// The files of a channel, ordered by path, optionally under a path prefix.
pub fn channel_files(
    conn: &Connection,
    channel_id: ChannelId,
    prefix: Option<&str>,
) -> Result<Vec<ChannelFile>> {
    let files: Vec<ChannelFile> = list_docs(
        conn,
        "SELECT data FROM channel_files WHERE channel_id = ?1 ORDER BY path",
        [channel_id.to_string()],
    )?;
    Ok(files
        .into_iter()
        .filter(|f| prefix.is_none_or(|p| f.path.starts_with(p)))
        .collect())
}

pub fn task_artifacts(
    conn: &Connection,
    channel_id: ChannelId,
    task_id: TaskId,
) -> Result<Vec<ChannelFile>> {
    Ok(channel_files(conn, channel_id, None)?
        .into_iter()
        .filter(|f| f.task_id == Some(task_id))
        .collect())
}

/// The channel files that a session sees in `<workspace>/.channel/files/`:
/// every file of the channel, except the artifacts of the session's own
/// task. A task finds its own artifacts in its workspace.
pub fn session_channel_files(
    conn: &Connection,
    channel_id: ChannelId,
    task_id: Option<TaskId>,
) -> Result<Vec<FileRef>> {
    let own = task_id.map(|t| format!("{ARTIFACTS_DIR}/{t}/"));
    Ok(channel_files(conn, channel_id, None)?
        .into_iter()
        .filter(|f| f.deleted_at.is_none())
        .filter(|f| own.as_deref().is_none_or(|own| !f.path.starts_with(own)))
        .map(|f| FileRef {
            path: f.path,
            sha256: f.blob_sha256,
        })
        .collect())
}

/// A digest of a channel file list, to see whether it changed.
fn file_list_digest(files: &[FileRef]) -> String {
    let mut text = String::new();
    for file in files {
        text.push_str(&file.path);
        text.push('\0');
        text.push_str(file.sha256.as_str());
        text.push('\n');
    }
    sha256(text.as_bytes()).to_string()
}

impl Tx<'_> {
    /// The channel files for the `session.start` of a session. The hub
    /// keeps a digest of the list, so a later turn sends the list again only
    /// when it changed (see [`Tx::channel_files_for_turn`]).
    pub fn channel_files_for_start(
        &mut self,
        session_id: SessionId,
        channel_id: ChannelId,
        task_id: Option<TaskId>,
    ) -> Result<Vec<FileRef>> {
        let files = session_channel_files(self.db(), channel_id, task_id)?;
        self.save_channel_files_digest(session_id, &file_list_digest(&files))?;
        Ok(files)
    }

    /// The channel files for the next `session.turn` of a session: the list
    /// when it changed since the session got it last, else `None`.
    pub fn channel_files_for_turn(
        &mut self,
        session_id: SessionId,
        channel_id: ChannelId,
        task_id: Option<TaskId>,
    ) -> Result<Option<Vec<FileRef>>> {
        let files = session_channel_files(self.db(), channel_id, task_id)?;
        let digest = file_list_digest(&files);
        let known: Option<String> = self
            .db()
            .query_row(
                "SELECT digest FROM session_channel_files WHERE session_id = ?1",
                [session_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if known.as_deref() == Some(digest.as_str()) {
            return Ok(None);
        }
        self.save_channel_files_digest(session_id, &digest)?;
        Ok(Some(files))
    }

    fn save_channel_files_digest(&mut self, session_id: SessionId, digest: &str) -> Result<()> {
        self.db().execute(
            "INSERT INTO session_channel_files (session_id, digest) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET digest = excluded.digest",
            params![session_id.to_string(), digest],
        )?;
        Ok(())
    }

    pub fn save_channel_file(&mut self, file: &ChannelFile) -> Result<()> {
        self.db().execute(
            "INSERT INTO channel_files (channel_id, path, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(channel_id, path) DO UPDATE SET data = excluded.data",
            params![file.channel_id.to_string(), file.path, to_doc(file)?],
        )?;
        self.emit(EntityPayload::ChannelFile(file.clone()))
    }

    /// Removes a file row. Channel files are deleted for good: the feed gets a
    /// delete, and the blob stays on disk.
    pub fn delete_channel_file(&mut self, channel_id: ChannelId, path: &str) -> Result<()> {
        self.db().execute(
            "DELETE FROM channel_files WHERE channel_id = ?1 AND path = ?2",
            params![channel_id.to_string(), path],
        )?;
        self.emit_delete(
            EntityKind::ChannelFile,
            format!("{channel_id}:{path}"),
            Some(channel_id),
        )
    }
}

// Search -----------------------------------------------------------------

impl Tx<'_> {
    /// Adds or replaces an entry in the search index. `kind` is `message`,
    /// `memory_note` or `task`. `ref_id` identifies the entity.
    pub fn index_search(
        &mut self,
        kind: &str,
        ref_id: &str,
        channel_id: Option<ChannelId>,
        scope: &str,
        title: &str,
        body: &str,
    ) -> Result<()> {
        self.unindex_search(kind, ref_id)?;
        self.db().execute(
            "INSERT INTO search_idx (kind, ref, channel_id, scope, title, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                kind,
                ref_id,
                channel_id.map(|c| c.to_string()),
                scope,
                title,
                body
            ],
        )?;
        Ok(())
    }

    pub fn unindex_search(&mut self, kind: &str, ref_id: &str) -> Result<()> {
        self.db().execute(
            "DELETE FROM search_idx WHERE kind = ?1 AND ref = ?2",
            params![kind, ref_id],
        )?;
        Ok(())
    }
}

/// Turns free text into an FTS5 query: every word must match as a prefix.
fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(12)
        .map(|w| format!("\"{w}\"*"))
        .collect();
    if words.is_empty() {
        None
    } else {
        Some(words.join(" "))
    }
}

/// Searches messages, memory notes and task titles. `allow` filters hits, for
/// example by what the caller may read. The result has at most `limit` hits.
pub fn search(
    conn: &Connection,
    query: &str,
    channel_id: Option<ChannelId>,
    limit: usize,
    allow: &dyn Fn(&SearchHit) -> bool,
) -> Result<Vec<SearchHit>> {
    let Some(fts) = fts_query(query) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare_cached(
        "SELECT kind, ref, channel_id, scope, title,
                snippet(search_idx, 5, '', '', '\u{2026}', 18)
         FROM search_idx
         WHERE search_idx MATCH ?1 AND (?2 IS NULL OR channel_id = ?2)
         ORDER BY rank LIMIT ?3",
    )?;
    let rows = stmt.query_map(
        params![
            fts,
            channel_id.map(|c| c.to_string()),
            i64::try_from(limit.saturating_mul(4).max(limit)).unwrap_or(i64::MAX)
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        },
    )?;
    let mut hits = Vec::new();
    for row in rows {
        let (kind, ref_id, _channel, scope, title, snippet) = row?;
        let hit = match kind.as_str() {
            "message" => {
                let Ok(id) = ref_id.parse() else { continue };
                let Some(message) = super::conversation::message(conn, id)? else {
                    continue;
                };
                if message.deleted_at.is_some() {
                    continue;
                }
                SearchHit::Message {
                    message_id: message.id,
                    channel_id: message.channel_id,
                    thread_root_id: message.thread_root_id,
                    snippet,
                    created_at: message.created_at,
                }
            }
            "memory_note" => {
                let Ok(scope) = scope.parse() else { continue };
                SearchHit::MemoryNote {
                    scope,
                    path: title,
                    snippet,
                }
            }
            "task" => {
                let Ok(id) = ref_id.parse() else { continue };
                let Some(task) = super::work::task(conn, id)? else {
                    continue;
                };
                SearchHit::Task {
                    task_id: task.id,
                    channel_id: task.channel_id,
                    title: task.title,
                    status: task.status,
                }
            }
            _ => continue,
        };
        if allow(&hit) {
            hits.push(hit);
            if hits.len() >= limit {
                break;
            }
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::temp_store;

    #[tokio::test]
    async fn memory_writes_check_the_expected_hash() {
        let (store, _dir) = temp_store();
        let scope = MemoryScope::Global;
        let first = store
            .write(move |tx| {
                tx.write_memory(
                    scope,
                    "people/kevin.md",
                    "Kevin likes tea.",
                    &ExpectedHash::NotExists,
                    MemoryAuthor::User,
                    None,
                )
            })
            .await
            .unwrap();
        assert_eq!(first.version, 1);

        // A second create with not_exists is stale.
        let err = store
            .write(move |tx| {
                tx.write_memory(
                    scope,
                    "people/kevin.md",
                    "x",
                    &ExpectedHash::NotExists,
                    MemoryAuthor::User,
                    None,
                )
            })
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::StaleWrite);

        let stale = sha256(b"something else");
        let err = store
            .write(move |tx| {
                tx.write_memory(
                    scope,
                    "people/kevin.md",
                    "x",
                    &ExpectedHash::Sha256(stale),
                    MemoryAuthor::User,
                    None,
                )
            })
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::StaleWrite);

        let current = first.sha256.clone();
        let second = store
            .write(move |tx| {
                tx.write_memory(
                    scope,
                    "people/kevin.md",
                    "Kevin likes coffee.",
                    &ExpectedHash::Sha256(current),
                    MemoryAuthor::User,
                    None,
                )
            })
            .await
            .unwrap();
        assert_eq!(second.version, 2);

        let hits = store
            .read(|conn| search(conn, "coffee", None, 10, &|_| true))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        let history = store
            .read(move |conn| memory_history(conn, scope, "people/kevin.md"))
            .await
            .unwrap();
        assert_eq!(history.len(), 2);
    }

    #[tokio::test]
    async fn the_purge_removes_notes_deleted_before_the_limit_with_their_versions() {
        let (hub, _dir) = crate::hub::test_hub().await;
        let scope = MemoryScope::Global;
        let deleted_at = hub
            .store
            .write(move |tx| {
                for path in ["old.md", "kept.md", "live.md"] {
                    let note = tx.write_memory(
                        scope,
                        path,
                        path,
                        &ExpectedHash::NotExists,
                        MemoryAuthor::User,
                        None,
                    )?;
                    if path != "live.md" {
                        tx.delete_memory(scope, path, &note.sha256)?;
                    }
                }
                Ok(tx.now())
            })
            .await
            .unwrap();
        // Make old.md look deleted long ago.
        hub.store
            .write(move |tx| {
                tx.db().execute(
                    "UPDATE memory_notes SET deleted_at = ?1 WHERE path = 'old.md'",
                    [deleted_at.as_millis() - 31 * 24 * 3_600_000],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let mut feed = hub.store.subscribe();

        let purged = hub
            .store
            .write(move |tx| tx.purge_deleted_memory(deleted_at.plus_millis(-30 * 24 * 3_600_000)))
            .await
            .unwrap();

        assert_eq!(purged, 1);
        let entry = feed.recv().await.unwrap();
        assert_eq!(entry.entity_id, "global:old.md");
        let (deleted, live, history) = hub
            .store
            .read(move |conn| {
                Ok((
                    deleted_memory_list(conn, Some(scope))?,
                    memory_list(conn, Some(scope))?,
                    memory_history(conn, scope, "old.md")?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].path, "kept.md");
        assert_eq!(live.len(), 1);
        assert!(history.is_empty());
    }
}
