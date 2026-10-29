//! Memory notes, channel files and the full-text search index.

use consort_proto::client_api::{EntityKind, EntityPayload};
use consort_proto::error::ErrorCode;
use consort_proto::ids::{ChannelId, SessionId, TaskId};
use consort_proto::model::{
    ChannelFile, ExpectedHash, MemoryAuthor, MemoryNote, MemoryNoteSummary, MemoryScope,
    Sha256Hash, memory_note_entity_id,
};
use consort_proto::time::Timestamp;
use consort_proto::tools::SearchHit;
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

fn note_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, String, i64, String, i64)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

type NoteRow = (String, String, String, String, i64, String, i64);

fn note_from_parts(parts: NoteRow) -> Result<MemoryNote> {
    let (scope, path, content, hash, version, updated_by, updated_at) = parts;
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
    })
}

const NOTE_COLUMNS: &str = "scope, path, content, sha256, version, updated_by, updated_at";

pub fn memory_note(
    conn: &Connection,
    scope: MemoryScope,
    path: &str,
) -> Result<Option<MemoryNote>> {
    let row = conn
        .query_row(
            &format!("SELECT {NOTE_COLUMNS} FROM memory_notes WHERE scope = ?1 AND path = ?2"),
            params![scope.to_string(), path],
            note_from_row,
        )
        .optional()?;
    row.map(note_from_parts).transpose()
}

/// Every note of one scope, ordered by path.
pub fn memory_notes(conn: &Connection, scope: MemoryScope) -> Result<Vec<MemoryNote>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {NOTE_COLUMNS} FROM memory_notes WHERE scope = ?1 ORDER BY path"
    ))?;
    let rows = stmt.query_map([scope.to_string()], note_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(note_from_parts(row?)?);
    }
    Ok(out)
}

/// Note summaries of one scope, or of all scopes, ordered by scope and path.
pub fn memory_list(
    conn: &Connection,
    scope: Option<MemoryScope>,
) -> Result<Vec<MemoryNoteSummary>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {NOTE_COLUMNS} FROM memory_notes
         WHERE (?1 IS NULL OR scope = ?1) ORDER BY scope, path"
    ))?;
    let rows = stmt.query_map([scope.map(|s| s.to_string())], note_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(note_from_parts(row?)?.summary());
    }
    Ok(out)
}

/// Every stored version of a note, oldest first. A deleted note keeps its
/// versions.
pub fn memory_history(
    conn: &Connection,
    scope: MemoryScope,
    path: &str,
) -> Result<Vec<MemoryNote>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {NOTE_COLUMNS} FROM memory_note_versions
         WHERE scope = ?1 AND path = ?2 ORDER BY version"
    ))?;
    let rows = stmt.query_map(params![scope.to_string(), path], note_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(note_from_parts(row?)?);
    }
    Ok(out)
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
    pub fn write_memory(
        &mut self,
        scope: MemoryScope,
        path: &str,
        content: &str,
        expected: &ExpectedHash,
        by: MemoryAuthor,
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
        };
        let values = params![
            note.scope.to_string(),
            note.path,
            note.content,
            note.sha256.as_str(),
            i64::try_from(note.version).unwrap_or(i64::MAX),
            author_to_text(note.updated_by),
            note.updated_at.as_millis()
        ];
        self.db().execute(
            &format!(
                "INSERT INTO memory_notes ({NOTE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(scope, path) DO UPDATE SET content = excluded.content,
                    sha256 = excluded.sha256, version = excluded.version,
                    updated_by = excluded.updated_by, updated_at = excluded.updated_at"
            ),
            values,
        )?;
        self.db().execute(
            &format!(
                "INSERT INTO memory_note_versions ({NOTE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
            ),
            values,
        )?;
        let entity_id = note.entity_id();
        self.unindex_search("memory_note", &entity_id)?;
        self.index_search(
            "memory_note",
            &entity_id,
            scope_channel(scope),
            &scope.to_string(),
            &note.path,
            &note.content,
        )?;
        self.emit(EntityPayload::MemoryNote(note.clone()))?;
        Ok(note)
    }

    /// Deletes a note if `expected` matches the stored hash. The versions stay.
    pub fn delete_memory(
        &mut self,
        scope: MemoryScope,
        path: &str,
        expected: &Sha256Hash,
    ) -> Result<()> {
        let current = memory_note(self.db(), scope, path)?
            .ok_or_else(|| HubError::not_found(format!("There is no memory note at {path}.")))?;
        if &current.sha256 != expected {
            return Err(stale_write());
        }
        self.db().execute(
            "DELETE FROM memory_notes WHERE scope = ?1 AND path = ?2",
            params![scope.to_string(), path],
        )?;
        let entity_id = memory_note_entity_id(scope, path);
        self.unindex_search("memory_note", &entity_id)?;
        self.emit_delete(EntityKind::MemoryNote, entity_id, scope_channel(scope))
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

impl Tx<'_> {
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
}
